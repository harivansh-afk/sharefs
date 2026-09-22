use std::{
    fs::File,
    io::{self, Read},
    path::{Component, Path, PathBuf},
};

use anyhow::{bail, ensure, Context, Result};
use hyper::Uri;
use sharefs::{ShareLink, ShareRequest};

use crate::{args::Args, token::SigningKey, utils::encode_uri};

pub struct Sharing {
    key: SigningKey,
    root: File,
    public_url: String,
    local_roots: Vec<(PathBuf, String)>,
}

impl Sharing {
    pub fn from_args(args: &Args) -> Result<Option<Self>> {
        let Some(key_path) = &args.share_key_file else {
            ensure!(
                args.public_url.is_none() && args.control_socket.is_none(),
                "sharing requires --share-key-file"
            );
            return Ok(None);
        };

        ensure!(cfg!(target_os = "linux"), "sharing requires Linux");
        ensure!(args.auth.has_users(), "sharing requires an owner account");
        ensure!(
            !args.allow_symlink,
            "sharing requires filesystem containment"
        );
        ensure!(!args.path_is_file, "sharing requires a served directory");

        let origin = args
            .public_url
            .as_deref()
            .context("sharing requires --public-url")?;
        let uri: Uri = origin.parse().context("invalid public URL")?;
        ensure!(
            matches!(uri.scheme_str(), Some("http" | "https"))
                && uri
                    .authority()
                    .is_some_and(|authority| !authority.as_str().contains('@'))
                && matches!(uri.path(), "" | "/")
                && uri.query().is_none(),
            "public URL must be an HTTP(S) origin without credentials, path or query"
        );

        ensure!(
            !key_path.canonicalize()?.starts_with(&args.serve_path),
            "share signing key must be outside the served directory"
        );
        let mut key_file = File::open(key_path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            ensure!(
                key_file.metadata()?.permissions().mode() & 0o077 == 0,
                "share signing key must not be accessible to group or others"
            );
        }
        let mut key = [0; 32];
        key_file
            .read_exact(&mut key)
            .context("share signing key must be exactly 32 bytes")?;
        ensure!(
            key_file.read(&mut [0])? == 0,
            "share signing key must be exactly 32 bytes"
        );

        let mut local_roots = Vec::new();
        for (name, path) in &args.share_roots {
            ensure!(
                !name.is_empty()
                    && !name.contains('/')
                    && !matches!(name.as_str(), "." | "..")
                    && path.is_absolute()
                    && !path
                        .components()
                        .any(|part| matches!(part, Component::ParentDir)),
                "share roots must map a directory name to an absolute local path"
            );
            local_roots.push((path.clone(), format!("/{name}")));
        }
        if local_roots.is_empty() {
            local_roots.push((args.serve_path.clone(), String::new()));
        }

        Ok(Some(Self {
            key: SigningKey::new(&key),
            root: File::open(&args.serve_path)?,
            public_url: format!("{}{}", origin.trim_end_matches('/'), args.uri_prefix),
            local_roots,
        }))
    }

    pub fn local_path(&self, path: &Path) -> Result<String> {
        let (root, prefix) = self
            .local_roots
            .iter()
            .filter(|(root, _)| path.starts_with(root))
            .max_by_key(|(root, _)| root.components().count())
            .context("file is outside the served directories")?;

        let relative = path
            .strip_prefix(root)?
            .to_str()
            .context("file name is not valid UTF-8")?;
        Ok(format!("{prefix}/{relative}"))
    }

    pub fn create(&self, request: &ShareRequest, now: u64) -> Result<ShareLink> {
        let expires_at = now
            .checked_add(request.expires_in)
            .filter(|expiry| *expiry > now)
            .context("expiry must be a positive duration")?;

        self.open(&request.path)?;
        let token = self.key.sign(&request.path, expires_at);

        Ok(ShareLink {
            url: format!(
                "{}{}?token={token}",
                self.public_url,
                encode_uri(&request.path[1..])
            ),
            expires_at,
        })
    }

    pub fn open_shared(&self, path: &str, token: &str, now: u64) -> Result<File> {
        ensure!(
            self.key.verify(path, token, now),
            "invalid or expired share link"
        );
        self.open(path)
    }

    /// Resolve and open beneath the served root in one kernel operation. A concurrent
    /// symlink replacement cannot redirect a verified path outside this directory.
    fn open(&self, path: &str) -> Result<File> {
        ensure!(valid_path(path), "invalid share path");

        #[cfg(target_os = "linux")]
        let file = {
            use rustix::fs::{openat2, Mode, OFlags, ResolveFlags};
            File::from(
                openat2(
                    &self.root,
                    &path[1..],
                    OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
                    Mode::empty(),
                    ResolveFlags::BENEATH | ResolveFlags::NO_MAGICLINKS,
                )
                .map_err(io::Error::from)?,
            )
        };
        #[cfg(not(target_os = "linux"))]
        let file = self.root.try_clone()?;

        if !file.metadata()?.is_file() {
            bail!("only regular files can be shared");
        }
        Ok(file)
    }
}

fn valid_path(path: &str) -> bool {
    path.starts_with('/')
        && !path.contains(['\\', '\0'])
        && path
            .split('/')
            .skip(1)
            .all(|part| !matches!(part, "" | "." | ".."))
}
