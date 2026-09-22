mod fixtures;
mod utils;

use fixtures::{server, Error, TestServer, FILES};
use rstest::rstest;
use xml::escape::escape_str_pcdata;

#[rstest]
fn propfind_dir(server: TestServer) -> Result<(), Error> {
    let resp = fetch!(b"PROPFIND", format!("{}dir1", server.url())).send()?;
    assert_eq!(resp.status(), 207);
    let body = resp.text()?;
    assert!(body.contains("<D:href>/dir1/</D:href>"));
    assert!(body.contains("<D:displayname>dir1</D:displayname>"));
    for f in FILES {
        assert!(body.contains(&format!("<D:href>/dir1/{}</D:href>", utils::encode_uri(f))));
        assert!(body.contains(&format!(
            "<D:displayname>{}</D:displayname>",
            escape_str_pcdata(f)
        )));
    }
    Ok(())
}

#[rstest]
fn propfind_dir_depth0(server: TestServer) -> Result<(), Error> {
    let resp = fetch!(b"PROPFIND", format!("{}dir1", server.url()))
        .header("depth", "0")
        .send()?;
    assert_eq!(resp.status(), 207);
    let body = resp.text()?;
    assert!(body.contains("<D:href>/dir1/</D:href>"));
    assert!(body.contains("<D:displayname>dir1</D:displayname>"));
    assert_eq!(
        body.lines()
            .filter(|v| *v == "<D:status>HTTP/1.1 200 OK</D:status>")
            .count(),
        1
    );
    Ok(())
}

#[rstest]
fn propfind_dir_depth2(server: TestServer) -> Result<(), Error> {
    let resp = fetch!(b"PROPFIND", format!("{}dir1", server.url()))
        .header("depth", "2")
        .send()?;
    assert_eq!(resp.status(), 400);
    let body = resp.text()?;
    assert_eq!(body, "Invalid depth: only 0 and 1 are allowed.");
    Ok(())
}

#[rstest]
fn propfind_404(server: TestServer) -> Result<(), Error> {
    let resp = fetch!(b"PROPFIND", format!("{}404", server.url())).send()?;
    assert_eq!(resp.status(), 404);
    Ok(())
}

#[rstest]
fn propfind_double_slash(server: TestServer) -> Result<(), Error> {
    let resp = fetch!(b"PROPFIND", server.url()).send()?;
    assert_eq!(resp.status(), 207);
    Ok(())
}

#[rstest]
fn propfind_file(server: TestServer) -> Result<(), Error> {
    let resp = fetch!(b"PROPFIND", format!("{}test.html", server.url())).send()?;
    assert_eq!(resp.status(), 207);
    let body = resp.text()?;
    assert!(body.contains("<D:href>/test.html</D:href>"));
    assert!(body.contains("<D:displayname>test.html</D:displayname>"));
    assert_eq!(
        body.lines()
            .filter(|v| *v == "<D:status>HTTP/1.1 200 OK</D:status>")
            .count(),
        1
    );
    Ok(())
}

#[rstest]
fn proppatch_file(#[with(&["-A"])] server: TestServer) -> Result<(), Error> {
    let resp = fetch!(b"PROPPATCH", format!("{}test.html", server.url())).send()?;
    assert_eq!(resp.status(), 207);
    let body = resp.text()?;
    assert!(body.contains("<D:href>/test.html</D:href>"));
    Ok(())
}

#[rstest]
fn proppatch_404(#[with(&["-A"])] server: TestServer) -> Result<(), Error> {
    let resp = fetch!(b"PROPPATCH", format!("{}404", server.url())).send()?;
    assert_eq!(resp.status(), 404);
    Ok(())
}

#[rstest]
fn mkcol_dir(#[with(&["-A"])] server: TestServer) -> Result<(), Error> {
    let resp = fetch!(b"MKCOL", format!("{}newdir", server.url())).send()?;
    assert_eq!(resp.status(), 201);
    Ok(())
}

#[rstest]
fn mkcol_not_allow_upload(server: TestServer) -> Result<(), Error> {
    let resp = fetch!(b"MKCOL", format!("{}newdir", server.url())).send()?;
    assert_eq!(resp.status(), 403);
    Ok(())
}

#[rstest]
fn mkcol_already_exists(#[with(&["-A"])] server: TestServer) -> Result<(), Error> {
    let resp = fetch!(b"MKCOL", format!("{}dir1", server.url())).send()?;
    assert_eq!(resp.status(), 405);
    Ok(())
}

#[rstest]
fn copy_file(#[with(&["-A"])] server: TestServer) -> Result<(), Error> {
    let new_url = format!("{}test2.html", server.url());
    let resp = fetch!(b"COPY", format!("{}test.html", server.url()))
        .header("Destination", &new_url)
        .send()?;
    assert_eq!(resp.status(), 204);
    let resp = reqwest::blocking::get(new_url)?;
    assert_eq!(resp.status(), 200);
    Ok(())
}

#[rstest]
fn copy_not_allow_upload(server: TestServer) -> Result<(), Error> {
    let new_url = format!("{}test2.html", server.url());
    let resp = fetch!(b"COPY", format!("{}test.html", server.url()))
        .header("Destination", &new_url)
        .send()?;
    assert_eq!(resp.status(), 403);
    Ok(())
}

#[rstest]
fn copy_file_404(#[with(&["-A"])] server: TestServer) -> Result<(), Error> {
    let new_url = format!("{}test2.html", server.url());
    let resp = fetch!(b"COPY", format!("{}404", server.url()))
        .header("Destination", &new_url)
        .send()?;
    assert_eq!(resp.status(), 404);
    Ok(())
}

#[rstest]
fn move_file(#[with(&["-A"])] server: TestServer) -> Result<(), Error> {
    let origin_url = format!("{}test.html", server.url());
    let new_url = format!("{}test2.html", server.url());
    let resp = fetch!(b"MOVE", &origin_url)
        .header("Destination", &new_url)
        .send()?;
    assert_eq!(resp.status(), 204);
    let resp = reqwest::blocking::get(new_url)?;
    assert_eq!(resp.status(), 200);
    let resp = reqwest::blocking::get(origin_url)?;
    assert_eq!(resp.status(), 404);
    Ok(())
}

#[rstest]
fn editing_does_not_allow_rename(
    #[with(&["--allow-edit"])] server: TestServer,
) -> Result<(), Error> {
    let origin_url = format!("{}test.html", server.url());
    let new_url = format!("{}test2.html", server.url());
    let resp = fetch!(b"MOVE", &origin_url)
        .header("Destination", &new_url)
        .send()?;
    assert_eq!(resp.status(), 403);
    Ok(())
}

#[rstest]
fn uploading_does_not_allow_rename(
    #[with(&["--allow-upload"])] server: TestServer,
) -> Result<(), Error> {
    let origin_url = format!("{}test.html", server.url());
    let new_url = format!("{}test2.html", server.url());
    let resp = fetch!(b"MOVE", &origin_url)
        .header("Destination", &new_url)
        .send()?;
    assert_eq!(resp.status(), 403);
    Ok(())
}

#[rstest]
fn move_file_404(#[with(&["-A"])] server: TestServer) -> Result<(), Error> {
    let new_url = format!("{}test2.html", server.url());
    let resp = fetch!(b"MOVE", format!("{}404", server.url()))
        .header("Destination", &new_url)
        .send()?;
    assert_eq!(resp.status(), 404);
    Ok(())
}

#[rstest]
fn lock_file(#[with(&["-A"])] server: TestServer) -> Result<(), Error> {
    let resp = fetch!(b"LOCK", format!("{}test.html", server.url())).send()?;
    assert_eq!(resp.status(), 200);
    let body = resp.text()?;
    assert!(body.contains("<D:href>/test.html</D:href>"));
    Ok(())
}

#[rstest]
fn lock_file_404(#[with(&["-A"])] server: TestServer) -> Result<(), Error> {
    let resp = fetch!(b"LOCK", format!("{}404", server.url())).send()?;
    assert_eq!(resp.status(), 404);
    Ok(())
}

#[rstest]
fn unlock_file(#[with(&["-A"])] server: TestServer) -> Result<(), Error> {
    let resp = fetch!(b"LOCK", format!("{}test.html", server.url())).send()?;
    assert_eq!(resp.status(), 200);
    Ok(())
}

#[rstest]
fn unlock_file_404(#[with(&["-A"])] server: TestServer) -> Result<(), Error> {
    let resp = fetch!(b"LOCK", format!("{}404", server.url())).send()?;
    assert_eq!(resp.status(), 404);
    Ok(())
}

#[rstest]
fn rename_permission_is_independent_of_upload_and_edit(
    #[with(&["--allow-rename"])] server: TestServer,
) -> Result<(), Error> {
    let source = server.url().join("test.txt")?;
    let destination = server.url().join("renamed.txt")?;
    let response = fetch!(b"MOVE", source.clone())
        .header("Destination", destination.as_str())
        .send()?;
    assert_eq!(response.status(), 204);
    assert_eq!(
        reqwest::blocking::get(destination)?.text()?,
        "This is test.txt"
    );
    assert_eq!(reqwest::blocking::get(source)?.status(), 404);
    Ok(())
}

#[rstest]
fn rename_cannot_replace_an_existing_file(
    #[with(&["-A"])] server: TestServer,
) -> Result<(), Error> {
    let source = server.url().join("test.txt")?;
    let destination = server.url().join("index.html")?;
    let response = fetch!(b"MOVE", source.clone())
        .header("Destination", destination.as_str())
        .send()?;
    assert_eq!(response.status(), 409);
    assert_eq!(reqwest::blocking::get(source)?.text()?, "This is test.txt");
    assert_eq!(
        reqwest::blocking::get(destination)?.text()?,
        "This is index.html"
    );
    Ok(())
}

#[rstest]
fn rename_stays_in_the_same_directory(#[with(&["-A"])] server: TestServer) -> Result<(), Error> {
    let source = server.url().join("test.txt")?;
    let destination = server.url().join("dir1/moved.txt")?;
    let response = fetch!(b"MOVE", source.clone())
        .header("Destination", destination.as_str())
        .send()?;
    assert_eq!(response.status(), 400);
    assert_eq!(reqwest::blocking::get(source)?.status(), 200);
    assert_eq!(reqwest::blocking::get(destination)?.status(), 404);
    Ok(())
}
