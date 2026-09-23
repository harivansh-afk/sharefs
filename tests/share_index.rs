mod digest_auth_util;
mod fixtures;

use std::{fs, os::unix::fs::PermissionsExt, process::Command};

use assert_fs::prelude::*;
use fixtures::{tmpdir, Error, TestServer};
use reqwest::blocking::Client;
use serde_json::{json, Value};

const INDEX: &str = "files/__sharefs__/api/shares";
const PAGE: &str = "files/__sharefs__/shares";
const CREATE: &str = "files/__sharefs__/api/share";

fn state() -> Result<assert_fs::TempDir, Error> {
    let state = assert_fs::TempDir::new()?;
    state.child("key").write_binary(&[7; 32])?;
    fs::set_permissions(state.join("key"), fs::Permissions::from_mode(0o600))?;
    Ok(state)
}

fn start(state: &assert_fs::TempDir) -> Result<TestServer, Error> {
    TestServer::start(
        tmpdir(),
        0,
        [
            "--share-key-file",
            state.join("key").to_str().unwrap(),
            "--share-state-dir",
            state.join("shares").to_str().unwrap(),
            "--control-socket",
            state.join("control.sock").to_str().unwrap(),
            "--public-url",
            "https://files.example",
            "--path-prefix",
            "files",
            "--auth",
            "owner:secret@/:rw|reader:secret@/",
        ],
    )
}

fn create(server: &TestServer, path: &str) -> Result<Value, Error> {
    let request = Client::new()
        .post(server.url().join(CREATE)?)
        .header("Content-Type", "application/json")
        .body(json!({"path":path}).to_string());
    let response = digest_auth_util::send_with_digest_auth(request, "owner", "secret")?;
    assert_eq!(response.status(), 201);
    Ok(serde_json::from_str(&response.text()?)?)
}

fn list(server: &TestServer) -> Result<Value, Error> {
    let request = Client::new().get(server.url().join(INDEX)?);
    let response = digest_auth_util::send_with_digest_auth(request, "owner", "secret")?;
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["cache-control"], "private, no-store");
    Ok(serde_json::from_str(&response.text()?)?)
}

#[test]
fn page_and_index_are_private_and_support_a_path_prefix() -> Result<(), Error> {
    let state = state()?;
    let server = start(&state)?;
    let client = Client::new();
    for path in [INDEX, PAGE] {
        let url = server.url().join(path)?;
        assert_eq!(client.get(url.clone()).send()?.status(), 401);
        assert_eq!(
            client
                .get(url.clone())
                .basic_auth("reader", Some("secret"))
                .send()?
                .status(),
            403
        );
        assert_eq!(
            client
                .post(url.clone())
                .basic_auth("owner", Some("secret"))
                .send()?
                .status(),
            405
        );
        assert_eq!(
            client
                .head(url)
                .basic_auth("owner", Some("secret"))
                .send()?
                .status(),
            200
        );
    }
    assert_eq!(list(&server)?, json!([]));
    let page = client
        .get(server.url().join(PAGE)?)
        .basic_auth("owner", Some("secret"))
        .send()?;
    assert_eq!(page.headers()["cache-control"], "private, no-store");
    assert!(page.text()?.contains(&format!(
        "/files/__sharefs_v{}__/index.js",
        env!("CARGO_PKG_VERSION")
    )));
    let link = create(&server, "/😀.bin")?;
    let listed = list(&server)?;
    assert_eq!(listed[0]["path"], "/😀.bin");
    assert_eq!(listed[0]["url"], link["url"]);
    assert_eq!(listed[0]["expires_at"], link["expires_at"]);
    let token_url = reqwest::Url::parse(link["url"].as_str().unwrap())?;
    let mut index_url = server.url().join(INDEX)?;
    index_url.set_query(token_url.query());
    assert_eq!(client.get(index_url).send()?.status(), 401);
    Ok(())
}

#[test]
fn browser_and_cli_links_persist_and_key_rotation_hides_old_links() -> Result<(), Error> {
    let state = state()?;
    let first = start(&state)?;
    let web = create(&first, "/test.txt")?;
    let cli = Command::new(assert_cmd::cargo::cargo_bin!("share"))
        .args([
            "--socket",
            state.join("control.sock").to_str().unwrap(),
            "--expires=1h",
            first.path().join("😀.bin").to_str().unwrap(),
        ])
        .output()?;
    assert!(
        cli.status.success(),
        "{}",
        String::from_utf8_lossy(&cli.stderr)
    );
    let cli = String::from_utf8(cli.stdout)?.trim().to_owned();
    let links = list(&first)?;
    assert_eq!(links.as_array().unwrap().len(), 2);
    assert!(links
        .as_array()
        .unwrap()
        .iter()
        .any(|link| link["url"] == web["url"]));
    assert!(links
        .as_array()
        .unwrap()
        .iter()
        .any(|link| link["url"] == cli));
    assert_eq!(
        fs::metadata(state.join("shares"))?.permissions().mode() & 0o777,
        0o700
    );
    for entry in fs::read_dir(state.join("shares"))? {
        assert_eq!(entry?.metadata()?.permissions().mode() & 0o777, 0o600);
    }
    drop(first);
    let second = start(&state)?;
    assert_eq!(list(&second)?, links);
    drop(second);
    fs::write(state.join("key"), [8; 32])?;
    let third = start(&state)?;
    assert_eq!(list(&third)?, json!([]));
    Ok(())
}

#[test]
fn index_write_failure_does_not_report_a_created_link() -> Result<(), Error> {
    let state = state()?;
    let server = start(&state)?;
    fs::remove_dir(state.join("shares"))?;
    fs::write(state.join("shares"), "not a directory")?;
    let response = Client::new()
        .post(server.url().join(CREATE)?)
        .basic_auth("owner", Some("secret"))
        .header("Content-Type", "application/json")
        .body(json!({"path":"/test.txt"}).to_string())
        .send()?;
    assert_eq!(response.status(), 500);
    assert!(!response.text()?.contains("token="));
    Ok(())
}

fn token(link: &Value) -> Result<String, Error> {
    Ok(reqwest::Url::parse(link["url"].as_str().unwrap())?
        .query_pairs()
        .find(|(name, _)| name == "token")
        .unwrap()
        .1
        .into_owned())
}

fn public_url(server: &TestServer, link: &Value) -> Result<reqwest::Url, Error> {
    let public = reqwest::Url::parse(link["url"].as_str().unwrap())?;
    Ok(server
        .url()
        .join(&format!("{}?{}", public.path(), public.query().unwrap()))?)
}

fn revoke(
    server: &TestServer,
    path: &str,
    token: &str,
) -> Result<reqwest::blocking::Response, Error> {
    let request = Client::new()
        .delete(server.url().join(CREATE)?)
        .header("Content-Type", "application/json")
        .body(json!({"path":path,"token":token}).to_string());
    Ok(digest_auth_util::send_with_digest_auth(
        request, "owner", "secret",
    )?)
}

#[test]
fn deactivation_survives_restart_without_removing_the_file_or_other_links() -> Result<(), Error> {
    let state = state()?;
    let first = start(&state)?;
    let revoked = create(&first, "/test.txt")?;
    let kept = create(&first, "/test.txt")?;
    assert_ne!(revoked["url"], kept["url"]);
    assert_eq!(list(&first)?.as_array().unwrap().len(), 2);
    let revoked_token = token(&revoked)?;
    for _ in 0..2 {
        let response = revoke(&first, "/test.txt", &revoked_token)?;
        assert_eq!(response.status(), 204);
        assert!(response.text()?.is_empty());
    }
    let client = Client::new();
    let url = public_url(&first, &revoked)?;
    assert_eq!(client.get(url.clone()).send()?.status(), 404);
    assert_eq!(client.head(url.clone()).send()?.status(), 404);
    assert_eq!(
        client
            .get(url)
            .header("Range", "bytes=0-3")
            .send()?
            .status(),
        404
    );
    assert_eq!(
        client.get(public_url(&first, &kept)?).send()?.text()?,
        "This is test.txt"
    );
    assert_eq!(
        fs::read_to_string(first.path().join("test.txt"))?,
        "This is test.txt"
    );
    let links = list(&first)?;
    assert_eq!(links.as_array().unwrap().len(), 1);
    assert_eq!(links[0]["url"], kept["url"]);
    drop(first);

    let second = start(&state)?;
    assert_eq!(
        client.get(public_url(&second, &revoked)?).send()?.status(),
        404
    );
    assert_eq!(
        client.get(public_url(&second, &kept)?).send()?.status(),
        200
    );
    assert_eq!(list(&second)?, links);
    let fresh = create(&second, "/test.txt")?;
    assert_ne!(fresh["url"], revoked["url"]);
    assert_eq!(
        client.get(public_url(&second, &fresh)?).send()?.status(),
        200
    );
    Ok(())
}

#[test]
fn deactivation_requires_owner_credentials_same_origin_and_a_valid_bound_token() -> Result<(), Error>
{
    let state = state()?;
    let server = start(&state)?;
    let link = create(&server, "/test.txt")?;
    let token = token(&link)?;
    let client = Client::new();
    let url = server.url().join(CREATE)?;
    let body = json!({"path":"/test.txt","token":token}).to_string();
    assert_eq!(
        client
            .delete(url.clone())
            .header("Content-Type", "application/json")
            .body(body.clone())
            .send()?
            .status(),
        401
    );
    assert_eq!(
        client
            .delete(url.clone())
            .basic_auth("reader", Some("secret"))
            .header("Content-Type", "application/json")
            .body(body.clone())
            .send()?
            .status(),
        403
    );
    assert_eq!(
        client
            .delete(url.clone())
            .basic_auth("owner", Some("secret"))
            .header("Origin", "https://elsewhere.example")
            .header("Content-Type", "application/json")
            .body(body)
            .send()?
            .status(),
        403
    );
    assert_eq!(revoke(&server, "/index.html", &token)?.status(), 400);
    assert_eq!(revoke(&server, "/test.txt", "../../escape")?.status(), 400);
    assert_eq!(
        client.get(public_url(&server, &link)?).send()?.status(),
        200
    );
    assert_eq!(list(&server)?.as_array().unwrap().len(), 1);
    Ok(())
}

#[test]
fn a_revocation_write_failure_does_not_report_success_or_hide_the_link() -> Result<(), Error> {
    let state = state()?;
    let server = start(&state)?;
    let link = create(&server, "/test.txt")?;
    fs::rename(state.join("shares"), state.join("saved-shares"))?;
    fs::write(state.join("shares"), "not a directory")?;
    assert_eq!(revoke(&server, "/test.txt", &token(&link)?)?.status(), 500);
    assert_eq!(
        Client::new()
            .get(public_url(&server, &link)?)
            .send()?
            .status(),
        200
    );
    Ok(())
}
