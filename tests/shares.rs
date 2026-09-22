mod fixtures;

use assert_fs::prelude::*;
use fixtures::{tmpdir, Error, TestServer};
use reqwest::blocking::Client;
use serde_json::{json, Value};
use std::time::Duration;

const API: &str = "__sharefs__/api/shares";

fn client() -> Client {
    Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap()
}

#[test]
fn authenticated_share_lifecycle() -> Result<(), Error> {
    let state = assert_fs::TempDir::new()?;
    let database = state.path().join("shares.db");
    let server = TestServer::start(
        tmpdir(),
        0,
        [
            "--bind",
            "127.0.0.1",
            "--share-db",
            database.to_str().unwrap(),
            "--auth",
            "rathi:secret@/:rw|other:secret@/:rw|reader:secret@/",
            "--allow-upload",
            "--allow-delete",
        ],
    )?;
    let client = client();
    let url = server.url().join(API)?;
    assert_eq!(client.get(url.clone()).send()?.status(), 401);
    assert_eq!(
        client
            .get(url.clone())
            .basic_auth("reader", Some("secret"))
            .send()?
            .status(),
        403
    );
    let response = client
        .post(url.clone())
        .basic_auth("rathi", Some("secret"))
        .header("Content-Type", "application/json")
        .body(json!({"path":"/test.txt","editable":true}).to_string())
        .send()?;
    assert_eq!(response.status(), 201);
    let share: Value = serde_json::from_str(&response.text()?)?;
    assert_eq!(share["owner"], "rathi");
    assert_eq!(share["path"], "/test.txt");
    assert_eq!(share["editable"], true);
    assert_eq!(share["is_dir"], false);
    let detail = server
        .url()
        .join(&format!("{API}/{}", share["id"].as_str().unwrap()))?;
    assert_eq!(
        client
            .get(detail.clone())
            .basic_auth("rathi", Some("secret"))
            .send()?
            .status(),
        200
    );
    let list: Value = serde_json::from_str(
        &client
            .get(url.clone())
            .basic_auth("rathi", Some("secret"))
            .send()?
            .text()?,
    )?;
    assert_eq!(list.as_array().unwrap().len(), 1);
    let other: Value = serde_json::from_str(
        &client
            .get(url.clone())
            .basic_auth("other", Some("secret"))
            .send()?
            .text()?,
    )?;
    assert_eq!(other, json!([]));
    assert_eq!(
        client
            .get(detail.clone())
            .basic_auth("other", Some("secret"))
            .send()?
            .status(),
        404
    );
    assert_eq!(
        client
            .delete(detail.clone())
            .basic_auth("other", Some("secret"))
            .send()?
            .status(),
        404
    );
    assert_eq!(
        client
            .delete(detail.clone())
            .basic_auth("rathi", Some("secret"))
            .send()?
            .status(),
        204
    );
    assert_eq!(
        client
            .get(detail)
            .basic_auth("rathi", Some("secret"))
            .send()?
            .status(),
        404
    );
    assert_eq!(
        std::fs::read_to_string(server.path().join("test.txt"))?,
        "This is test.txt"
    );
    assert!(database.is_file());
    Ok(())
}

#[test]
fn share_requests_respect_paths_and_permissions() -> Result<(), Error> {
    let state = assert_fs::TempDir::new()?;
    let database = state.path().join("shares.db");
    let server = TestServer::start(
        tmpdir(),
        0,
        [
            "--bind",
            "127.0.0.1",
            "--share-db",
            database.to_str().unwrap(),
            "--auth",
            "rathi:secret@/:rw,/dir1:ro",
            "--allow-upload",
            "--allow-delete",
        ],
    )?;
    let client = client();
    let url = server.url().join(API)?;
    for (body, status) in [
        (json!({"path":"/../shares.db"}), 400),
        (json!({"path":"/missing.txt"}), 403),
        (json!({"path":"/test.txt","owner":"other"}), 400),
        (json!({"path":"/test.txt","expires_at":1}), 400),
        (json!({"path":"/dir1/test.txt","editable":true}), 403),
        (json!({"path":"/dir1"}), 201),
    ] {
        let response = client
            .post(url.clone())
            .basic_auth("rathi", Some("secret"))
            .header("Content-Type", "application/json")
            .body(body.to_string())
            .send()?;
        assert_eq!(response.status().as_u16(), status, "{body}");
    }
    assert_eq!(
        client
            .post(url.clone())
            .basic_auth("rathi", Some("secret"))
            .header("Content-Type", "application/json")
            .header("Origin", "https://other.example")
            .body(json!({"path":"/test.txt"}).to_string())
            .send()?
            .status(),
        403
    );
    assert_eq!(
        client
            .post(url)
            .basic_auth("rathi", Some("secret"))
            .body(json!({"path":"/test.txt"}).to_string())
            .send()?
            .status(),
        415
    );
    Ok(())
}

#[test]
fn share_database_cannot_be_served() -> Result<(), Error> {
    let root = tmpdir();
    let database = root.path().join("shares.db");
    let result = TestServer::start(
        root,
        0,
        [
            "--share-db",
            database.to_str().unwrap(),
            "--auth",
            "rathi:secret@/:rw",
        ],
    );
    assert!(result.is_err());
    let state = assert_fs::TempDir::new()?;
    state.child("shares.db").touch()?;
    let result = TestServer::start(
        tmpdir(),
        0,
        [
            "--share-db",
            state.path().join("shares.db").to_str().unwrap(),
        ],
    );
    assert!(result.is_err());
    Ok(())
}
