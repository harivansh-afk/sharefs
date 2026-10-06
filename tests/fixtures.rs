use assert_fs::fixture::TempDir;
use assert_fs::prelude::*;
use port_check::free_local_port;
use reqwest::Url;
use rstest::fixture;
use std::io::{BufRead, BufReader};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread::sleep;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[allow(dead_code)]
pub type Error = Box<dyn std::error::Error>;

#[allow(dead_code)]
pub const BIN_FILE: &str = "😀.bin";

/// File names for testing purpose
#[allow(dead_code)]
pub static FILES: &[&str] = &[
    "test.txt",
    "test.html",
    "index.html",
    #[cfg(not(target_os = "windows"))]
    "file\n1.txt",
    BIN_FILE,
];

/// Directory names for testing directory don't exist
#[allow(dead_code)]
pub static DIR_NO_FOUND: &str = "dir-no-found/";

/// Directory names for testing directory don't have index.html
#[allow(dead_code)]
pub static DIR_NO_INDEX: &str = "dir-no-index/";

/// Directory names for testing hidden
#[allow(dead_code)]
pub static DIR_GIT: &str = ".git/";

/// Directory names for testings assets override
#[allow(dead_code)]
pub static DIR_ASSETS: &str = "dir-assets/";

/// Directory names for testing purpose
#[allow(dead_code)]
pub static DIRECTORIES: &[&str] = &["dir1/", "dir2/", "dir3/", DIR_NO_INDEX, DIR_GIT, DIR_ASSETS];

/// Test fixture which creates a temporary directory with a few files and directories inside.
/// The directories also contain files.
#[fixture]
#[allow(dead_code)]
pub fn tmpdir() -> TempDir {
    let tmpdir = assert_fs::TempDir::new().expect("Couldn't create a temp dir for tests");
    for file in FILES {
        if *file == BIN_FILE {
            tmpdir.child(file).write_binary(b"bin\0\x00123").unwrap();
        } else {
            tmpdir
                .child(file)
                .write_str(&format!("This is {file}"))
                .unwrap();
        }
    }
    for directory in DIRECTORIES {
        if *directory == DIR_ASSETS {
            tmpdir
                .child(format!("{}{}", directory, "index.html"))
                .write_str("__ASSETS_PREFIX__index.js;<template id=\"index-data\">__INDEX_DATA__</template>")
                .unwrap();
        } else {
            for file in FILES {
                if *directory == DIR_NO_INDEX && *file == "index.html" {
                    continue;
                }
                if *file == BIN_FILE {
                    tmpdir
                        .child(format!("{directory}{file}"))
                        .write_binary(b"bin\0\x00123")
                        .unwrap();
                } else {
                    tmpdir
                        .child(format!("{directory}{file}"))
                        .write_str(&format!("This is {directory}{file}"))
                        .unwrap();
                }
            }
        }
    }
    tmpdir.child("dir4/hidden").touch().unwrap();
    tmpdir
        .child("content-types/bin.tar")
        .write_binary(b"\x7f\x45\x4c\x46\x02\x01\x00\x00")
        .unwrap();
    tmpdir
        .child("content-types/bin")
        .write_binary(b"\x7f\x45\x4c\x46\x02\x01\x00\x00")
        .unwrap();
    tmpdir
        .child("content-types/file-utf8.txt")
        .write_str("世界")
        .unwrap();
    tmpdir
        .child("content-types/file-gbk.txt")
        .write_binary(b"\xca\xc0\xbd\xe7")
        .unwrap();
    tmpdir
        .child("content-types/file")
        .write_str("世界")
        .unwrap();

    tmpdir
}

/// Get a free port.
#[fixture]
#[allow(dead_code)]
pub fn port() -> u16 {
    free_local_port().expect("Couldn't find a free local port")
}

/// Start an isolated server and wait for its own listening announcement.
#[fixture]
#[allow(dead_code)]
pub fn server<I>(#[default(&[] as &[&str])] args: I) -> TestServer
where
    I: IntoIterator,
    I::Item: AsRef<std::ffi::OsStr>,
{
    TestServer::start(tmpdir(), 0, args).expect("server startup failed")
}

/// Wait a max of 2s for the port to become available.
#[allow(dead_code)]
pub fn wait_for_port(port: u16) {
    let start_wait = Instant::now();
    let addresses = [
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
        SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), port),
    ];

    while !addresses
        .iter()
        .any(|address| TcpStream::connect_timeout(address, Duration::from_millis(100)).is_ok())
    {
        sleep(Duration::from_millis(250));

        if start_wait.elapsed().as_secs() > 2 {
            panic!("timeout waiting for port {port}");
        }
    }
}

#[allow(dead_code)]
pub struct TestServer {
    port: u16,
    share_port: Option<u16>,
    tmpdir: TempDir,
    child: Child,
    is_tls: bool,
    output_reader: Option<JoinHandle<()>>,
}

#[allow(dead_code)]
impl TestServer {
    pub fn start<I>(tmpdir: TempDir, port: u16, args: I) -> Result<Self, Error>
    where
        I: IntoIterator,
        I::Item: AsRef<std::ffi::OsStr>,
    {
        let mut child = Command::new(assert_cmd::cargo::cargo_bin!())
            .arg(tmpdir.path())
            .arg("--port")
            .arg(port.to_string())
            .args(args)
            .stdout(Stdio::piped())
            .spawn()?;
        let stdout = child.stdout.take().expect("child stdout was piped");
        let (ready, receive) = mpsc::sync_channel(1);
        let output_reader = thread::spawn(move || {
            let mut reported = false;
            let mut share_port = None;
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if reported {
                    continue;
                }
                if let Some(address) = line.trim().strip_prefix("Sharing on ") {
                    share_port = Url::parse(address).ok().and_then(|url| url.port());
                    continue;
                }
                let text = line
                    .trim()
                    .strip_prefix("Listening on ")
                    .unwrap_or(line.trim());
                if let Ok(url) = Url::parse(text) {
                    if let Some(port) = url.port_or_known_default() {
                        let _ = ready.send((port, url.scheme() == "https", share_port));
                        reported = true;
                    }
                }
            }
        });
        let mut server = Self {
            port: 0,
            share_port: None,
            tmpdir,
            child,
            is_tls: false,
            output_reader: Some(output_reader),
        };
        let (port, is_tls, share_port) = receive
            .recv_timeout(Duration::from_secs(5))
            .map_err(|error| format!("server did not announce a listening address: {error}"))?;
        if port == 0 {
            return Err("server announced port zero".into());
        }
        if let Some(status) = server.child.try_wait()? {
            return Err(format!("server exited during startup: {status}").into());
        }
        server.port = port;
        server.share_port = share_port;
        server.is_tls = is_tls;
        Ok(server)
    }

    pub fn url(&self) -> Url {
        let protocol = if self.is_tls { "https" } else { "http" };
        Url::parse(&format!("{}://localhost:{}", protocol, self.port)).unwrap()
    }

    pub fn path(&self) -> &std::path::Path {
        self.tmpdir.path()
    }

    pub fn share_url(&self) -> Url {
        Url::parse(&format!(
            "http://127.0.0.1:{}",
            self.share_port.expect("share listener")
        ))
        .unwrap()
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.output_reader.take() {
            let _ = reader.join();
        }
    }
}
