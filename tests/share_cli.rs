mod fixtures;

use std::{fs, os::unix::fs::PermissionsExt, process::Command};

use assert_cmd::prelude::*;
use assert_fs::prelude::*;
use fixtures::{tmpdir, Error, TestServer};
use predicates::prelude::*;
use reqwest::Url;
use sharefs::parse_expiry;

#[test]
fn expiry_syntax_is_positive_checked_and_unambiguous() {
    for (input, expected) in [
        ("1", 1),
        ("30m", 1800),
        ("2h", 7200),
        ("7d", 604800),
        ("1w", 604800),
    ] {
        assert_eq!(parse_expiry(input).unwrap(), expected);
    }
    for input in [
        "",
        "0",
        "0d",
        "+1",
        "-1d",
        "1.5h",
        "1h30m",
        "1y",
        "18446744073709551615w",
    ] {
        assert!(parse_expiry(input).is_err(), "{input}");
    }
}

#[test]
fn cli_help_and_usage_have_conventional_exit_codes() {
    Command::new(assert_cmd::cargo::cargo_bin!("share"))
        .arg("--help")
        .assert()
        .success()
        .stderr("")
        .stdout(predicate::str::contains("share [OPTION]... FILE"));
    Command::new(assert_cmd::cargo::cargo_bin!("share"))
        .arg("--version")
        .assert()
        .success()
        .stderr("");
    Command::new(assert_cmd::cargo::cargo_bin!("share"))
        .assert()
        .code(2)
        .stdout("");
    Command::new(assert_cmd::cargo::cargo_bin!("share"))
        .args(["--expires=0", "file"])
        .assert()
        .code(2)
        .stdout("");
}

#[rstest::rstest]
#[case(false)]
#[case(true)]
fn cli_prints_only_a_working_link_and_rejects_unserved_files(
    #[case] mapped: bool,
) -> Result<(), Error> {
    let state = assert_fs::TempDir::new()?;
    let root = tmpdir();
    let local_dir = if mapped {
        root.path().join("dir1")
    } else {
        root.path().to_owned()
    };
    fs::write(local_dir.join("-notes.txt"), "shared by the Rust client")?;
    let key = state.child("key");
    key.write_binary(&[9; 32])?;
    fs::set_permissions(key.path(), fs::Permissions::from_mode(0o600))?;
    let socket = state.path().join("control.sock");
    let config = state.child("config.yaml");
    config.write_str(&format!("share-roots:\n  dir1: {}\n", local_dir.display()))?;
    let mut args = vec![
        "--share-key-file",
        key.path().to_str().unwrap(),
        "--public-url",
        "https://files.example",
        "--control-socket",
        socket.to_str().unwrap(),
        "--auth",
        "owner:secret@/:rw",
    ];
    if mapped {
        args.extend(["--config", config.path().to_str().unwrap()]);
    }
    let server = TestServer::start(root, 0, args)?;
    assert_eq!(fs::metadata(&socket)?.permissions().mode() & 0o777, 0o600);

    let result = Command::new(assert_cmd::cargo::cargo_bin!("share"))
        .current_dir(&local_dir)
        .args([
            "--socket",
            socket.to_str().unwrap(),
            "--expires=1h",
            "--",
            "-notes.txt",
        ])
        .output()?;
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(result.stderr.is_empty());
    let output = String::from_utf8(result.stdout)?;
    assert_eq!(output.lines().count(), 1);
    let public = Url::parse(output.trim())?;
    let local = server
        .url()
        .join(&format!("{}?{}", public.path(), public.query().unwrap()))?;
    assert_eq!(
        reqwest::blocking::get(local)?.text()?,
        "shared by the Rust client"
    );

    Command::new(assert_cmd::cargo::cargo_bin!("share"))
        .args([
            "--socket",
            socket.to_str().unwrap(),
            key.path().to_str().unwrap(),
        ])
        .assert()
        .code(1)
        .stdout("")
        .stderr(predicate::str::contains("outside the served directories"));
    Ok(())
}
