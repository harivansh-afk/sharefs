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
    fs::set_permissions(state.path(), fs::Permissions::from_mode(0o700))?;
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
            "--share-state-dir",
            state.path().to_str().unwrap(),
            "--public-url",
            "https://files.example",
            "--share-listen",
            "127.0.0.1:0",
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
fn separate_listener_only_serves_signed_reads_and_observes_revocation() -> Result<(), Error> {
    let (server, _state) = server()?;
    let client = Client::new();
    let (_, link) = create(&server, "/test.txt")?;
    let public = server
        .share_url()
        .join(&format!("{}?{}", link.path(), link.query().unwrap()))?;
    assert_eq!(
        client.get(public.clone()).send()?.text()?,
        "This is test.txt"
    );
    assert_eq!(client.head(public.clone()).send()?.status(), 200);
    let range = client
        .get(public.clone())
        .header("Range", "bytes=0-3")
        .send()?;
    assert_eq!(range.status(), 206);
    assert_eq!(range.text()?, "This");

    for path in [
        "",
        "?json",
        "test.txt",
        "__sharefs__/health",
        "__sharefs__/shares",
        "__sharefs__/api/shares",
        "__sharefs__/api/share",
        "__sharefs_v0.5.4__/index.js",
    ] {
        for authenticated in [false, true] {
            let request = client.get(server.share_url().join(path)?);
            let request = if authenticated {
                request.basic_auth("owner", Some("secret"))
            } else {
                request
            };
            let response = request.send()?;
            assert_eq!(
                response.status(),
                404,
                "{path}, authenticated={authenticated}"
            );
            assert!(!response.headers().contains_key("www-authenticate"));
        }
    }
    for method in [
        reqwest::Method::PUT,
        reqwest::Method::POST,
        reqwest::Method::DELETE,
        reqwest::Method::PATCH,
        reqwest::Method::OPTIONS,
        reqwest::Method::from_bytes(b"PROPFIND")?,
        reqwest::Method::from_bytes(b"COPY")?,
        reqwest::Method::from_bytes(b"MOVE")?,
    ] {
        assert_eq!(
            client
                .request(method, public.clone())
                .basic_auth("owner", Some("secret"))
                .body("overwrite")
                .send()?
                .status(),
            403
        );
    }
    for path in [
        "/index.html",
        "/",
        "/__sharefs__/health",
        "/__sharefs__/api/shares",
    ] {
        let mut other = public.clone();
        other.set_path(path);
        assert_eq!(
            client
                .get(other)
                .basic_auth("owner", Some("secret"))
                .send()?
                .status(),
            404
        );
    }
    let mut duplicate = public.clone();
    duplicate.set_query(Some(&format!("{}&token=invalid", public.query().unwrap())));
    assert_eq!(client.get(duplicate).send()?.status(), 404);
    assert_eq!(
        fs::read_to_string(server.path().join("test.txt"))?,
        "This is test.txt"
    );

    // A single Sharing instance owns the index for both listeners.
    let token = public
        .query_pairs()
        .find(|(name, _)| name == "token")
        .unwrap()
        .1
        .into_owned();
    let response = client
        .delete(server.url().join(API)?)
        .basic_auth("owner", Some("secret"))
        .header("Content-Type", "application/json")
        .body(json!({"path": "/test.txt", "token": token}).to_string())
        .send()?;
    assert!(response.status().is_success());
    assert_eq!(client.get(public).send()?.status(), 404);
    Ok(())
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

    assert!(client
        .put(server.url().join("test.txt")?)
        .basic_auth("owner", Some("secret"))
        .body("changed in place")
        .send()?
        .status()
        .is_success());
    assert_eq!(client.get(url.clone()).send()?.text()?, "changed in place");
    fs::write(server.path().join("replacement.txt"), "replaced atomically")?;
    fs::rename(
        server.path().join("replacement.txt"),
        server.path().join("test.txt"),
    )?;
    let updated = client.get(url.clone()).send()?;
    assert!(updated.headers()["cache-control"]
        .to_str()?
        .contains("no-store"));
    assert_eq!(updated.text()?, "replaced atomically");
    assert_eq!(
        client
            .put(url.clone())
            .body("unauthorized")
            .send()?
            .status(),
        403
    );
    assert_eq!(client.delete(url.clone()).send()?.status(), 403);
    assert_eq!(client.get(url).send()?.text()?, "replaced atomically");
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

#[test]
fn raw_files_are_sandboxed_for_owners_and_recipients() -> Result<(), Error> {
    let (server, _state) = server()?;
    let client = Client::new();
    let (_, public) = create(&server, "/test.html")?;
    let responses = [
        client.get(public).send()?,
        client
            .get(server.url().join("test.html")?)
            .basic_auth("owner", Some("secret"))
            .send()?,
    ];
    for response in responses {
        assert_eq!(response.status(), 200);
        let policy = response.headers()["content-security-policy"].to_str()?;
        assert!(policy.contains("sandbox allow-downloads;"));
        assert!(policy.contains("default-src 'none'"));
        assert!(policy.contains("form-action 'none'"));
        assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    }
    let ui = client
        .get(server.url())
        .basic_auth("owner", Some("secret"))
        .send()?;
    assert!(!ui.headers().contains_key("content-security-policy"));
    Ok(())
}
