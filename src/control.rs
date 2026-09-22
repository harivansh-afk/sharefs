use std::{
    os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt},
    path::Path,
    sync::Arc,
    time::Duration,
};

use anyhow::{ensure, Result};
use sharefs::{ShareReply, ShareRequest, MAX_MESSAGE_BYTES};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    sync::{OwnedSemaphorePermit, Semaphore},
    task::JoinHandle,
    time::timeout,
};

use crate::{sharing::Sharing, utils::unix_now};

pub fn listen(path: &Path, sharing: Arc<Sharing>) -> Result<JoinHandle<()>> {
    if let Ok(metadata) = std::fs::symlink_metadata(path) {
        ensure!(
            metadata.file_type().is_socket(),
            "control socket path is not a socket"
        );
        match std::os::unix::net::UnixStream::connect(path) {
            Ok(_) => anyhow::bail!("control socket is already in use"),
            Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
                std::fs::remove_file(path)?;
            }
            Err(error) => return Err(error.into()),
        }
    }

    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    let owner = std::fs::metadata(path)?.uid();
    let slots = Arc::new(Semaphore::new(8));

    Ok(tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            if !stream.peer_cred().is_ok_and(|peer| peer.uid() == owner) {
                continue;
            }
            let Ok(permit) = slots.clone().try_acquire_owned() else {
                continue;
            };
            let sharing = sharing.clone();

            tokio::spawn(async move {
                let _ = timeout(Duration::from_secs(5), serve(stream, sharing, permit)).await;
            });
        }
    }))
}

async fn serve(
    mut stream: UnixStream,
    sharing: Arc<Sharing>,
    permit: OwnedSemaphorePermit,
) -> Result<()> {
    let mut body = Vec::new();
    (&mut stream)
        .take(MAX_MESSAGE_BYTES + 1)
        .read_to_end(&mut body)
        .await?;
    ensure!(
        body.len() as u64 <= MAX_MESSAGE_BYTES,
        "share request is too large"
    );

    let result = async {
        let mut request: ShareRequest = serde_json::from_slice(&body)?;
        request.path = sharing.local_path(Path::new(&request.path))?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            sharing.create(&request, unix_now().as_secs())
        })
        .await?
    }
    .await;

    let reply = match result {
        Ok(link) => ShareReply::Link(link),
        Err(error) => ShareReply::Error {
            error: error.to_string(),
        },
    };
    stream.write_all(&serde_json::to_vec(&reply)?).await?;
    Ok(())
}
