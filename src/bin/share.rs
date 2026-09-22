use std::{
    io::{Read, Write},
    net::Shutdown,
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    process::ExitCode,
    time::Duration,
};

use anyhow::{bail, Context, Result};
use clap::{value_parser, Arg, Command};
use sharefs::{parse_expiry, ShareReply, ShareRequest, DEFAULT_SOCKET, MAX_MESSAGE_BYTES};

fn main() -> ExitCode {
    let matches = Command::new("share")
        .version(env!("CARGO_PKG_VERSION"))
        .about("Create a read-only link to a file served by sharefs")
        .override_usage("share [OPTION]... FILE")
        .arg(
            Arg::new("expires")
                .short('x')
                .long("expires")
                .value_name("DURATION")
                .default_value("7d")
                .value_parser(parse_expiry)
                .help("Expire after DURATION (e.g. 30m, 2h, 7d)"),
        )
        .arg(
            Arg::new("socket")
                .long("socket")
                .value_name("PATH")
                .env("SHAREFS_SOCKET")
                .hide_env(true)
                .default_value(DEFAULT_SOCKET)
                .value_parser(value_parser!(PathBuf))
                .help("Connect to the sharefs control socket"),
        )
        .arg(
            Arg::new("file")
                .value_name("FILE")
                .required(true)
                .value_parser(value_parser!(PathBuf)),
        )
        .get_matches();

    let result = create_link(
        matches.get_one::<PathBuf>("file").unwrap(),
        matches.get_one::<PathBuf>("socket").unwrap(),
        *matches.get_one::<u64>("expires").unwrap(),
    );

    match result {
        Ok(url) => {
            println!("{url}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("share: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn create_link(file: &Path, socket: &Path, expires_in: u64) -> Result<String> {
    let file = file.canonicalize().context("cannot resolve file")?;
    let path = file.to_str().context("file name is not valid UTF-8")?;
    let request = serde_json::to_vec(&ShareRequest {
        path: path.to_owned(),
        expires_in,
    })?;
    if request.len() as u64 > MAX_MESSAGE_BYTES {
        bail!("file path is too long");
    }

    let mut stream = UnixStream::connect(socket).context("cannot connect to sharefs")?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    stream.write_all(&request)?;
    stream.shutdown(Shutdown::Write)?;

    let mut response = Vec::new();
    stream
        .take(MAX_MESSAGE_BYTES + 1)
        .read_to_end(&mut response)?;
    if response.len() as u64 > MAX_MESSAGE_BYTES {
        bail!("sharefs response is too large");
    }

    match serde_json::from_slice(&response).context("invalid sharefs response")? {
        ShareReply::Link(link) => Ok(link.url),
        ShareReply::Error { error } => bail!(error),
    }
}
