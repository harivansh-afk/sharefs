mod digest_auth_util;
mod fixtures;

use std::{fs, os::unix::fs::PermissionsExt};

use assert_fs::prelude::*;
use fixtures::{tmpdir, Error, TestServer};
use reqwest::{blocking::Client, Url};
use serde_json::{json, Value};

const API: &str = "__sharefs__/api/share";

fn server() -> Result<(TestServer, assert_fs::TempDir), Error> {
    let state = assert_fs::TempDir::new()?;
    let key = state.child("key");
    key.write_binary(&[7; 32])?;
    fs::set_permissions(key.path(), fs::Permissions::from_mode(0o600))?;

    let server = TestServer::start(
        tmpdir(),
        0,
        [
            "--bind",
            "127.0.0.1",
            "--share-key-file",
            key.path().to_str().unwrap(),
            "--public-url",
            "https://files.example",
            "--auth",
            "owner:secret@/:rw|reader:secret@/",
            "--allow-upload",
            "--allow-edit",
        ],
    )?;
    Ok((server, state))
}

fn create(server: &TestServer, path: &str) -> Result<(Value, Url), Error> {
    let response = Client::new()
        .post(server.url().join(API)?)
        .basic_auth("owner", Some("secret"))
        .header("Content-Type", "application/json")
        .body(json!({"path": path, "expires_in": 3600}).to_string())
        .send()?;
    assert_eq!(response.status(), 201);

    let body: Value = serde_json::from_str(&response.text()?)?;
    let public = Url::parse(body["url"].as_str().unwrap())?;
    assert_eq!(
        public.origin().ascii_serialization(),
        "https://files.example"
    );
    let local = server
        .url()
        .join(&format!("{}?{}", public.path(), public.query().unwrap()))?;
    Ok((body, local))
}

#[test]
fn links_read_live_files_and_support_head_and_ranges() -> Result<(), Error> {
    let (server, _state) = server()?;
    let client = Client::new();
    let (_, url) = create(&server, "/test.txt")?;

    assert_eq!(
        client.get(server.url().join("test.txt")?).send()?.status(),
        401
    );
    assert_eq!(client.get(url.clone()).send()?.text()?, "This is test.txt");
    assert_eq!(client.head(url.clone()).send()?.status(), 200);

    let response = client
        .get(url.clone())
        .header("Range", "bytes=0-3")
        .send()?;
    assert_eq!(response.status(), 206);
    assert_eq!(response.text()?, "This");

    // Public multipart requests fall back to a bounded full-file stream.
    let response = client
        .get(url.clone())
        .header("Range", "bytes=0-3,5-6")
        .send()?;
    assert_eq!(response.status(), 200);
    assert_eq!(response.text()?, "This is test.txt");

    fs::write(server.path().join("test.txt"), "changed in place")?;
    assert_eq!(client.get(url.clone()).send()?.text()?, "changed in place");
    assert_eq!(
        client
            .put(url.clone())
            .body("unauthorized")
            .send()?
            .status(),
        403
    );
    assert_eq!(client.delete(url.clone()).send()?.status(), 403);
    assert_eq!(client.get(url).send()?.text()?, "changed in place");
    Ok(())
}

#[test]
fn tokens_cannot_be_reused_for_other_paths_or_owner_apis() -> Result<(), Error> {
    let (server, _state) = server()?;
    let client = Client::new();
    let (_, mut url) = create(&server, "/test.txt")?;
    url.set_path("/index.html");
    assert_eq!(client.get(url.clone()).send()?.status(), 404);

    url.set_path(&format!("/{API}"));
    assert_eq!(client.post(url.clone()).body("{}").send()?.status(), 401);

    url.set_path("/test.txt");
    let token = url.query().unwrap().strip_prefix("token=").unwrap();
    let mut raw = token.as_bytes().to_vec();
    raw[20] = if raw[20] == b'A' { b'B' } else { b'A' };
    url.set_query(Some(&format!("token={}", String::from_utf8(raw)?)));
    assert_eq!(client.get(url).send()?.status(), 404);
    Ok(())
}

#[test]
fn minting_requires_owner_auth_valid_input_and_same_origin() -> Result<(), Error> {
    let (server, _state) = server()?;
    let client = Client::new();
    let url = server.url().join(API)?;
    assert_eq!(client.post(url.clone()).send()?.status(), 401);
    assert_eq!(
        client
            .post(url.clone())
            .basic_auth("reader", Some("secret"))
            .send()?
            .status(),
        403
    );
    assert_eq!(
        client
            .get(url.clone())
            .basic_auth("owner", Some("secret"))
            .send()?
            .status(),
        405
    );

    for body in [
        json!({"path": "/dir1"}),
        json!({"path": "/../test.txt"}),
        json!({"path": "/missing.txt"}),
        json!({"path": "/test.txt", "expires_in": 0}),
        json!({"path": "/test.txt", "expires_in": u64::MAX}),
        json!({"path": "/test.txt", "editable": true}),
    ] {
        let response = client
            .post(url.clone())
            .basic_auth("owner", Some("secret"))
            .header("Content-Type", "application/json")
            .body(body.to_string())
            .send()?;
        assert_eq!(response.status(), 400, "{body}");
    }
    let response = client
        .post(url.clone())
        .basic_auth("owner", Some("secret"))
        .header("Content-Type", "application/json")
        .header("Origin", "https://elsewhere.example")
        .body(r#"{"path":"/test.txt"}"#)
        .send()?;
    assert_eq!(response.status(), 403);

    let response = client
        .post(url)
        .basic_auth("owner", Some("secret"))
        .header("Content-Type", "application/json")
        .body("x".repeat(16_385))
        .send()?;
    assert_eq!(response.status(), 413);
    Ok(())
}

#[test]
fn unicode_paths_and_links_survive_a_server_restart() -> Result<(), Error> {
    let (first, state) = server()?;
    let (link, _) = create(&first, "/😀.bin")?;
    drop(first);
    drop(state);

    let (second, _state) = server()?;
    let public = Url::parse(link["url"].as_str().unwrap())?;
    let url = second
        .url()
        .join(&format!("{}?{}", public.path(), public.query().unwrap()))?;
    assert_eq!(
        Client::new().get(url).send()?.bytes()?.as_ref(),
        b"bin\0\x00123"
    );
    Ok(())
}

#[test]
fn changing_a_shared_path_to_an_escaping_symlink_is_rejected() -> Result<(), Error> {
    use std::os::unix::fs::symlink;

    let (server, state) = server()?;
    let outside = state.child("private.txt");
    outside.write_str("outside the served root")?;
    let (_, url) = create(&server, "/test.txt")?;

    fs::remove_file(server.path().join("test.txt"))?;
    symlink(outside.path(), server.path().join("test.txt"))?;
    assert_eq!(Client::new().get(url).send()?.status(), 404);
    Ok(())
}

#[test]
fn digest_authenticated_owner_can_share_a_file() -> Result<(), Error> {
    let (server, _state) = server()?;
    let request = Client::new()
        .post(server.url().join(API)?)
        .header("Content-Type", "application/json")
        .body(json!({"path":"/test.txt"}).to_string());
    let response = digest_auth_util::send_with_digest_auth(request, "owner", "secret")?;
    assert_eq!(response.status(), 201);
    let link: Value = serde_json::from_str(&response.text()?)?;
    let public = Url::parse(link["url"].as_str().unwrap())?;
    let url = server
        .url()
        .join(&format!("{}?{}", public.path(), public.query().unwrap()))?;
    assert_eq!(reqwest::blocking::get(url)?.text()?, "This is test.txt");
    Ok(())
}
