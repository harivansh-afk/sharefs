mod fixtures;

use fixtures::{port, server, tmpdir, wait_for_port, Error, TestServer};

use assert_cmd::prelude::*;
use assert_fs::fixture::TempDir;
use assert_fs::prelude::*;
use regex::Regex;
use rstest::rstest;
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::Duration;

#[test]
fn concurrent_servers_serve_their_own_files() -> Result<(), Error> {
    let first_root = tmpdir();
    first_root.child("identity.txt").write_str("first")?;
    let second_root = tmpdir();
    second_root.child("identity.txt").write_str("second")?;
    let (first, second) = std::thread::scope(|scope| {
        let first =
            scope.spawn(|| TestServer::start(first_root, 0, ["--bind", "127.0.0.1"]).unwrap());
        let second =
            scope.spawn(|| TestServer::start(second_root, 0, ["--bind", "127.0.0.1"]).unwrap());
        (first.join().unwrap(), second.join().unwrap())
    });
    assert_ne!(first.port(), second.port());
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    for (server, expected) in [(&first, "first"), (&second, "second")] {
        let response = client.get(server.url().join("identity.txt")?).send()?;
        assert_eq!(response.status(), 200);
        assert_eq!(response.text()?, expected);
    }
    Ok(())
}

#[test]
fn occupied_port_is_not_mistaken_for_a_ready_child() -> Result<(), Error> {
    let first = TestServer::start(tmpdir(), 0, ["--bind", "127.0.0.1"])?;
    let second = TestServer::start(tmpdir(), first.port(), ["--bind", "127.0.0.1"]);
    assert!(second.is_err());
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    assert_eq!(client.get(first.url()).send()?.status(), 200);
    Ok(())
}

#[rstest]
#[case(&["-b", "20.205.243.166"])]
fn bind_fails(tmpdir: TempDir, port: u16, #[case] args: &[&str]) -> Result<(), Error> {
    Command::new(assert_cmd::cargo::cargo_bin!())
        .arg(tmpdir.path())
        .arg("-p")
        .arg(port.to_string())
        .args(args)
        .assert()
        .stderr(predicates::str::contains("Failed to bind"))
        .failure();

    Ok(())
}

#[rstest]
#[case(server(&[] as &[&str]), true, false)]
#[case(server(&["-b", "0.0.0.0"]), true, false)]
#[case(server(&["-b", "127.0.0.1", "-b", "::1"]), true, true)]
fn bind_ipv4_ipv6(
    #[case] server: TestServer,
    #[case] bind_ipv4: bool,
    #[case] bind_ipv6: bool,
) -> Result<(), Error> {
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(5))
        .build()?;
    assert_eq!(
        client
            .get(format!("http://127.0.0.1:{}", server.port()))
            .send()
            .is_ok(),
        bind_ipv4
    );
    assert_eq!(
        client
            .get(format!("http://[::1]:{}", server.port()))
            .send()
            .is_ok(),
        bind_ipv6
    );

    Ok(())
}

#[rstest]
#[case(&[] as &[&str])]
#[case(&["--path-prefix", "/prefix"])]
fn validate_printed_urls(tmpdir: TempDir, port: u16, #[case] args: &[&str]) -> Result<(), Error> {
    let mut child = Command::new(assert_cmd::cargo::cargo_bin!())
        .arg(tmpdir.path())
        .arg("-p")
        .arg(port.to_string())
        .args(args)
        .stdout(Stdio::piped())
        .spawn()?;

    wait_for_port(port);

    let stdout = child.stdout.as_mut().expect("Failed to get stdout");
    let mut buf = [0; 1000];
    let buf_len = stdout.read(&mut buf)?;
    let output = std::str::from_utf8(&buf[0..buf_len])?;
    let url_lines = output
        .lines()
        .take_while(|line| !line.is_empty()) /* non-empty lines */
        .collect::<Vec<_>>()
        .join("\n");

    let urls = Regex::new(r"http://[a-zA-Z0-9\.\[\]:/]+")
        .unwrap()
        .captures_iter(url_lines.as_str())
        .filter_map(|caps| caps.get(0).map(|v| v.as_str()))
        .collect::<Vec<_>>();

    assert!(!urls.is_empty());
    reqwest::blocking::get(urls[0])?.error_for_status()?;

    child.kill()?;

    Ok(())
}
