use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::RwLock,
};

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use sharefs::MAX_MESSAGE_BYTES;

#[derive(Debug)]
pub struct RecordWriteFailed;

impl std::fmt::Display for RecordWriteFailed {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Cannot record share link")
    }
}

pub struct ShareIndex {
    directory: PathBuf,
    revoked: RwLock<HashSet<String>>,
}

#[derive(Deserialize, Serialize)]
pub struct StoredShare {
    pub path: String,
    pub token: String,
    pub created_at: u64,
    pub expires_at: u64,
}

impl ShareIndex {
    pub fn open(path: &Path, root: &Path, local_roots: &[(PathBuf, String)]) -> Result<Self> {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)?;
        let metadata = fs::symlink_metadata(path)?;
        ensure!(
            metadata.is_dir(),
            "share state must be a directory, not a symlink"
        );
        ensure!(
            metadata.permissions().mode() & 0o077 == 0,
            "share state must be private (mode 0700)"
        );

        let directory = path.canonicalize()?;
        ensure!(
            !directory.starts_with(root),
            "share state must be outside the served directory"
        );
        for (source, _) in local_roots {
            let source = source.canonicalize().unwrap_or_else(|_| source.clone());
            ensure!(
                !directory.starts_with(source),
                "share state must be outside the served directories"
            );
        }
        let now = crate::utils::unix_now().as_secs();
        let mut revoked = HashSet::new();
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str().and_then(|name| name.strip_suffix(".revoked")) else {
                continue;
            };
            let (expiry, token) = name.split_once('-').context("invalid share revocation")?;
            ensure!(entry.file_type()?.is_file(), "invalid share revocation");
            if expiry.parse::<u64>()? > now {
                revoked.insert(token.to_owned());
            }
        }
        Ok(Self {
            directory,
            revoked: RwLock::new(revoked),
        })
    }

    pub fn is_revoked(&self, token: &str) -> bool {
        self.revoked
            .read()
            .map(|revoked| revoked.contains(token))
            .unwrap_or(true)
    }

    pub fn revoke(&self, token: &str, expires_at: u64) -> Result<()> {
        let path = self.directory.join(format!("{expires_at}-{token}.revoked"));
        let file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => File::open(path)?,
            Err(error) => return Err(error.into()),
        };
        ensure!(file.metadata()?.is_file(), "invalid share revocation");
        file.sync_all()?;
        File::open(&self.directory)?.sync_all()?;
        self.revoked
            .write()
            .map_err(|_| anyhow::anyhow!("share revocations unavailable"))?
            .insert(token.to_owned());
        Ok(())
    }

    pub fn record(&self, entry: &StoredShare) -> Result<()> {
        let destination = self
            .directory
            .join(format!("{}-{}.json", entry.expires_at, entry.token));
        let temporary = self
            .directory
            .join(format!(".{}.tmp", uuid::Uuid::new_v4()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        let result = (|| {
            file.write_all(&serde_json::to_vec(entry)?)?;
            file.sync_all()?;
            fs::rename(&temporary, destination)?;
            File::open(&self.directory)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }

    pub fn entries(&self, now: u64) -> Result<Vec<StoredShare>> {
        let mut entries = Vec::new();
        for file in fs::read_dir(&self.directory)? {
            let file = file?;
            let name = file.file_name();
            let Some(name) = name.to_str().and_then(|name| name.strip_suffix(".json")) else {
                continue;
            };
            let Some((expiry, _)) = name.split_once('-') else {
                continue;
            };
            if !expiry.parse::<u64>().is_ok_and(|expiry| expiry > now)
                || !file.file_type()?.is_file()
            {
                continue;
            }
            let mut body = Vec::new();
            File::open(file.path())?
                .take(MAX_MESSAGE_BYTES + 1)
                .read_to_end(&mut body)?;
            ensure!(
                body.len() as u64 <= MAX_MESSAGE_BYTES,
                "share record is too large"
            );
            let entry: StoredShare =
                serde_json::from_slice(&body).context("invalid share record")?;
            if entry.expires_at > now {
                entries.push(entry);
            }
        }
        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expired_records_are_skipped_and_duplicate_links_have_one_record() -> Result<()> {
        let state = assert_fs::TempDir::new()?;
        let served = state.join("served");
        fs::create_dir(&served)?;
        let index = ShareIndex::open(&state.join("shares"), &served, &[])?;
        for (token, expiry) in [("expired", 10), ("active", 20), ("active", 20)] {
            index.record(&StoredShare {
                path: "/file.txt".into(),
                token: token.into(),
                created_at: 1,
                expires_at: expiry,
            })?;
        }
        let entries = index.entries(10)?;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].token, "active");
        assert!(index.entries(20)?.is_empty());
        Ok(())
    }

    #[test]
    fn state_cannot_be_public_or_exposed_through_a_served_directory() -> Result<()> {
        let state = assert_fs::TempDir::new()?;
        let served = state.join("served");
        fs::create_dir(&served)?;
        assert!(ShareIndex::open(&served.join("index"), &served, &[]).is_err());
        let mapped = state.join("mapped");
        fs::create_dir(&mapped)?;
        assert!(ShareIndex::open(
            &mapped.join("index"),
            &served,
            &[(mapped, "/Documents".into())]
        )
        .is_err());
        let public = state.join("public");
        fs::create_dir(&public)?;
        fs::set_permissions(&public, fs::Permissions::from_mode(0o755))?;
        assert!(ShareIndex::open(&public, &served, &[]).is_err());
        let symlink = state.join("symlink");
        std::os::unix::fs::symlink(&served, &symlink)?;
        assert!(ShareIndex::open(&symlink, &served, &[]).is_err());
        Ok(())
    }
}
