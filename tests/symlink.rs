mod fixtures;
mod utils;

use assert_fs::fixture::TempDir;
use fixtures::{server, tmpdir, Error, TestServer};
use rstest::rstest;

#[cfg(unix)]
use std::os::unix::fs::symlink as symlink_dir;
#[cfg(windows)]
use std::os::windows::fs::symlink_dir;

#[rstest]
fn default_not_allow_symlink(server: TestServer, tmpdir: TempDir) -> Result<(), Error> {
    // Create symlink directory "foo" to point outside the root
    let dir = "foo";
    symlink_dir(tmpdir.path(), server.path().join(dir)).expect("Couldn't create symlink");
    let resp = reqwest::blocking::get(format!("{}{}", server.url(), dir))?;
    assert_eq!(resp.status(), 404);
    let resp = reqwest::blocking::get(format!("{}{}/index.html", server.url(), dir))?;
    assert_eq!(resp.status(), 404);
    let resp = reqwest::blocking::get(server.url())?;
    let paths = utils::retrieve_index_paths(&resp.text()?);
    assert!(!paths.is_empty());
    assert!(!paths.contains(&format!("{dir}/")));
    Ok(())
}

#[rstest]
fn allow_symlink(
    #[with(&["--allow-symlink"])] server: TestServer,
    tmpdir: TempDir,
) -> Result<(), Error> {
    // Create symlink directory "foo" to point outside the root
    let dir = "foo";
    symlink_dir(tmpdir.path(), server.path().join(dir)).expect("Couldn't create symlink");
    let resp = reqwest::blocking::get(format!("{}{}", server.url(), dir))?;
    assert_eq!(resp.status(), 200);
    let resp = reqwest::blocking::get(format!("{}{}/index.html", server.url(), dir))?;
    assert_eq!(resp.status(), 200);
    let resp = reqwest::blocking::get(server.url())?;
    let paths = utils::retrieve_index_paths(&resp.text()?);
    assert!(!paths.is_empty());
    assert!(paths.contains(&format!("{dir}/")));
    Ok(())
}

#[rstest]
fn upload_cannot_follow_a_dangling_symlink(
    #[with(&["--allow-upload", "--allow-edit"])] server: TestServer,
    tmpdir: TempDir,
) -> Result<(), Error> {
    let outside = tmpdir.path().join("must-not-create");
    symlink_dir(&outside, server.path().join("dangling"))?;
    let response = fetch!(b"PUT", server.url().join("dangling")?)
        .body("escape")
        .send()?;
    assert_eq!(response.status(), 404);
    assert!(!outside.exists());
    let response = fetch!(b"PUT", server.url().join("dangling/child")?)
        .body("escape")
        .send()?;
    assert_eq!(response.status(), 404);
    assert!(!outside.exists());
    Ok(())
}

#[rstest]
fn copy_checks_destination_before_creating_directories(
    #[with(&["--allow-upload", "--allow-edit"])] server: TestServer,
    tmpdir: TempDir,
) -> Result<(), Error> {
    symlink_dir(tmpdir.path(), server.path().join("outside"))?;
    let response = fetch!(b"COPY", server.url().join("test.txt")?)
        .header(
            "Destination",
            server.url().join("outside/must-not-create/file")?.as_str(),
        )
        .send()?;
    assert_eq!(response.status(), 400);
    assert!(!tmpdir.path().join("must-not-create").exists());
    Ok(())
}
