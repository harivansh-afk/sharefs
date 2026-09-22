use anyhow::{bail, ensure, Context, Result};
use chrono::Utc;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, Row};
use serde::Serialize;
use std::fs::OpenOptions;
use std::path::Path;
use std::time::Duration;
use uuid::Uuid;

pub fn normalize_share_path(path: &str) -> Result<String> {
    ensure!(
        path.starts_with('/')
            && !path.contains(['\\', '\0'])
            && !path.split('/').any(|part| matches!(part, "." | "..")),
        "share path must be rooted within the served directory"
    );
    Ok(format!(
        "/{}",
        path.split('/')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("/")
    ))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Share {
    pub id: String,
    pub owner: String,
    /// Decoded path within the served root, not an absolute host path.
    pub path: String,
    pub is_dir: bool,
    pub editable: bool,
    pub expires_at: Option<i64>,
    pub created_at: i64,
}

pub struct NewShare<'a> {
    pub owner: &'a str,
    pub path: &'a str,
    pub is_dir: bool,
    pub editable: bool,
    pub expires_at: Option<i64>,
}

/// Stores metadata. Callers enforce account access and filesystem containment.
pub struct ShareStore {
    connection: Connection,
}

impl ShareStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = std::path::absolute(path)?;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error).context("cannot create share database"),
        }
        let metadata = std::fs::symlink_metadata(&path)?;
        ensure!(metadata.is_file(), "share database must be a regular file");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            ensure!(
                metadata.permissions().mode() & 0o077 == 0,
                "share database must not be accessible to group or others"
            );
        }
        let mut connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        connection.busy_timeout(Duration::from_secs(5))?;
        let version: u32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        match version {
            0 => {
                let transaction = connection.transaction()?;
                transaction.execute_batch(
                    "CREATE TABLE shares (
                        id TEXT PRIMARY KEY NOT NULL,
                        owner TEXT NOT NULL,
                        path TEXT NOT NULL,
                        is_dir INTEGER NOT NULL CHECK (is_dir IN (0, 1)),
                        editable INTEGER NOT NULL CHECK (editable IN (0, 1)),
                        expires_at INTEGER CHECK (expires_at >= 0),
                        created_at INTEGER NOT NULL
                    );
                    CREATE INDEX shares_owner ON shares(owner, created_at);
                    PRAGMA user_version = 1;",
                )?;
                transaction.commit()?;
            }
            1 => {}
            _ => bail!("unsupported share database version: {version}"),
        }
        Ok(Self { connection })
    }

    pub fn create(&self, request: NewShare<'_>) -> Result<Share> {
        ensure!(!request.owner.is_empty(), "share owner is required");
        let path = normalize_share_path(request.path)?;
        ensure!(
            request.expires_at.is_none_or(|expiry| expiry >= 0),
            "share expiry must be a Unix timestamp"
        );
        let share = Share {
            id: format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple()),
            owner: request.owner.to_owned(),
            path,
            is_dir: request.is_dir,
            editable: request.editable,
            expires_at: request.expires_at,
            created_at: Utc::now().timestamp(),
        };
        self.connection.execute(
            "INSERT INTO shares (id, owner, path, is_dir, editable, expires_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                share.id,
                share.owner,
                share.path,
                share.is_dir,
                share.editable,
                share.expires_at,
                share.created_at
            ],
        )?;
        Ok(share)
    }

    pub fn resolve(&self, id: &str) -> Result<Option<Share>> {
        self.resolve_at(id, Utc::now().timestamp())
    }

    fn resolve_at(&self, id: &str, now: i64) -> Result<Option<Share>> {
        if id.len() != 64 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Ok(None);
        }
        Ok(self
            .connection
            .query_row(
                "SELECT id, owner, path, is_dir, editable, expires_at, created_at
             FROM shares WHERE id = ?1 AND (expires_at IS NULL OR expires_at > ?2)",
                params![id, now],
                Share::from_row,
            )
            .optional()?)
    }

    pub fn list(&self, owner: &str) -> Result<Vec<Share>> {
        let mut statement = self.connection.prepare(
            "SELECT id, owner, path, is_dir, editable, expires_at, created_at
             FROM shares WHERE owner = ?1 ORDER BY created_at, id",
        )?;
        let shares = statement
            .query_map([owner], Share::from_row)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(shares)
    }

    pub fn revoke(&self, id: &str, owner: &str) -> Result<bool> {
        Ok(self.connection.execute(
            "DELETE FROM shares WHERE id = ?1 AND owner = ?2",
            params![id, owner],
        )? != 0)
    }
}

impl Share {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            owner: row.get(1)?,
            path: row.get(2)?,
            is_dir: row.get(3)?,
            editable: row.get(4)?,
            expires_at: row.get(5)?,
            created_at: row.get(6)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(owner: &str) -> NewShare<'_> {
        NewShare {
            owner,
            path: "/Documents/notes.md",
            is_dir: false,
            editable: false,
            expires_at: None,
        }
    }

    #[test]
    fn records_and_revocation_survive_reopening() -> Result<()> {
        let directory = assert_fs::TempDir::new()?;
        let path = directory.path().join("shares.db");
        let store = ShareStore::open(&path)?;
        let share = store.create(NewShare {
            editable: true,
            ..request("rathi")
        })?;
        drop(store);
        let store = ShareStore::open(&path)?;
        assert_eq!(store.resolve(&share.id)?, Some(share.clone()));
        assert_eq!(store.list("rathi")?, vec![share.clone()]);
        assert!(store.revoke(&share.id, "rathi")?);
        drop(store);
        let store = ShareStore::open(&path)?;
        assert_eq!(store.resolve(&share.id)?, None);
        assert!(store.list("rathi")?.is_empty());
        Ok(())
    }

    #[test]
    fn expiry_and_ownership_are_enforced() -> Result<()> {
        let directory = assert_fs::TempDir::new()?;
        let store = ShareStore::open(directory.path().join("shares.db"))?;
        let share = store.create(NewShare {
            expires_at: Some(100),
            ..request("rathi")
        })?;
        assert!(store.resolve_at(&share.id, 99)?.is_some());
        assert!(store.resolve_at(&share.id, 100)?.is_none());
        assert!(store.list("other")?.is_empty());
        assert!(!store.revoke(&share.id, "other")?);
        assert_eq!(store.list("rathi")?, vec![share.clone()]);
        assert!(store.revoke(&share.id, "rathi")?);
        assert!(!store.revoke(&share.id, "rathi")?);
        Ok(())
    }

    #[test]
    fn paths_cannot_traverse_the_served_root() -> Result<()> {
        let directory = assert_fs::TempDir::new()?;
        let store = ShareStore::open(directory.path().join("shares.db"))?;
        for path in [
            "relative",
            "/../secret",
            "/Documents/../../secret",
            "/Documents/./file",
            "/Documents\\secret",
            "/a\0b",
        ] {
            assert!(
                store
                    .create(NewShare {
                        path,
                        ..request("rathi")
                    })
                    .is_err(),
                "{path:?}"
            );
        }
        let share = store.create(NewShare {
            path: "/Documents//photos/",
            is_dir: true,
            ..request("rathi")
        })?;
        assert_eq!(share.path, "/Documents/photos");
        assert!(share.is_dir);
        assert!(!share.editable);
        assert!(store.resolve("invalid")?.is_none());
        assert_ne!(share.id, store.create(request("rathi"))?.id);
        Ok(())
    }

    #[test]
    fn unsupported_schema_is_not_modified() -> Result<()> {
        let directory = assert_fs::TempDir::new()?;
        let path = directory.path().join("shares.db");
        let store = ShareStore::open(&path)?;
        store.connection.pragma_update(None, "user_version", 2)?;
        drop(store);
        assert!(ShareStore::open(&path).is_err());
        let connection = Connection::open(path)?;
        assert_eq!(
            connection.pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))?,
            2
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn database_is_private_and_symlinks_are_rejected() -> Result<()> {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let directory = assert_fs::TempDir::new()?;
        let path = directory.path().join("shares.db");
        drop(ShareStore::open(&path)?);
        assert_eq!(
            std::fs::metadata(&path)?.permissions().mode() & 0o777,
            0o600
        );
        let link = directory.path().join("link.db");
        symlink(&path, &link)?;
        assert!(ShareStore::open(link).is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))?;
        assert!(ShareStore::open(path).is_err());
        Ok(())
    }
}
