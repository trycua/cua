//! The Daytona toolbox at the wire over mock Spaces: statuses and error
//! codes the generated clients depend on (daytona_toolbox_api_client 0.225.0
//! `_response_types_map` per call), bulk transfer framing, session rules.

use cua_compat::backend::mock::MockBackend;
use cua_compat::{Gateway, GatewayOptions, Provider};
use serde_json::{Value, json};
use std::sync::Arc;

struct Gw {
    gw: Gateway,
    http: reqwest::Client,
    api: String,
    id: String,
    _dir: tempfile::TempDir,
}

async fn start() -> Gw {
    let dir = tempfile::tempdir().unwrap();
    let mut o = GatewayOptions::new(Provider::Daytona);
    o.listen = "127.0.0.1:0".parse().unwrap();
    o.host_port = 0;
    o.dir = Some(dir.path().to_path_buf());
    o.create_default_user = false;
    let gw = Gateway::start(o, Some(Arc::new(MockBackend::new())))
        .await
        .unwrap();
    let api = format!("http://{}/api", gw.api_addr);
    let http = reqwest::Client::new();
    let sb: Value = http
        .post(format!("{api}/sandbox"))
        .bearer_auth(&gw.api_key)
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = sb["id"].as_str().unwrap().to_string();
    Gw {
        gw,
        http,
        api,
        id,
        _dir: dir,
    }
}

/// A `files[0]` bulk-upload body: `(content type, body)`.
fn upload_body(path: &str, data: &[u8]) -> (String, Vec<u8>) {
    let b = "testboundary7f3a";
    let mut body = format!(
        "--{b}\r\nContent-Disposition: form-data; name=\"files[0].path\"\r\n\r\n{path}\r\n--{b}\r\nContent-Disposition: form-data; name=\"files[0].file\"; filename=\"f\"\r\nContent-Type: application/octet-stream\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(data);
    body.extend_from_slice(format!("\r\n--{b}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={b}"), body)
}

impl Gw {
    async fn upload(&self, path: &str, data: &[u8]) -> u16 {
        let (ct, body) = upload_body(path, data);
        self.tb(reqwest::Method::POST, "/files/bulk-upload")
            .header("content-type", ct)
            .body(body)
            .send()
            .await
            .unwrap()
            .status()
            .as_u16()
    }

    fn tb(&self, m: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.http
            .request(m, format!("{}/toolbox/{}{path}", self.api, self.id))
            .bearer_auth(&self.gw.api_key)
    }
}

#[tokio::test]
async fn a_command_past_its_timeout_is_a_408_with_the_daemon_code() {
    let g = start().await;
    let r = g
        .tb(reqwest::Method::POST, "/process/execute")
        .json(&json!({"command": "sleep 20000", "timeout": 1}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 408);
    let b: Value = r.json().await.unwrap();
    assert_eq!(b["code"], "PROCESS_EXECUTION_TIMEOUT");
    assert_eq!(b["source"], "DAYTONA_DAEMON");
    assert_eq!(b["statusCode"], 408);
}

#[tokio::test]
async fn files_report_their_own_codes_and_bulk_parts_are_matched_by_filename_star() {
    let g = start().await;
    let r = g
        .tb(reqwest::Method::GET, "/files/info?path=/nope")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
    let b: Value = r.json().await.unwrap();
    assert_eq!(
        (b["code"].as_str(), b["source"].as_str()),
        (Some("FILE_NOT_FOUND"), Some("DAYTONA_DAEMON"))
    );
    // bulk download: one good part, one error part with structured details, a non-ASCII name
    assert_eq!(g.upload("/tmp/naïve.txt", b"data").await, 200);
    let r = g
        .tb(reqwest::Method::POST, "/files/bulk-download")
        .json(&json!({"paths": ["/tmp/naïve.txt", "/tmp/missing"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let ct = r.headers()["content-type"].to_str().unwrap().to_string();
    assert!(ct.starts_with("multipart/form-data; boundary="));
    let body = r.text().await.unwrap();
    assert!(
        body.contains("filename*=UTF-8''%2Ftmp%2Fna%C3%AFve.txt"),
        "{body}"
    );
    assert!(body.contains("name=\"error\""), "{body}");
    assert!(
        body.contains("\"code\":\"FILE_NOT_FOUND\"")
            && body.contains("\"source\":\"DAYTONA_DAEMON\""),
        "{body}"
    );
}

#[tokio::test]
async fn statuses_follow_the_generated_clients() {
    let g = start().await;
    let r = g
        .tb(reqwest::Method::POST, "/process/session")
        .json(&json!({"sessionId": "s"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201);
    let r = g
        .tb(reqwest::Method::POST, "/process/session")
        .json(&json!({"sessionId": "s"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 409, "creating a session twice");
    let r = g
        .tb(reqwest::Method::DELETE, "/process/session/s")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 204);
    let r = g
        .tb(reqwest::Method::DELETE, "/process/session/s")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
    let b: Value = r.json().await.unwrap();
    assert_eq!(b["code"], "PROCESS_NOT_FOUND");
    // delete file: 204, then 404 with the file code
    g.upload("/tmp/a", b"1").await;
    assert_eq!(
        g.tb(reqwest::Method::DELETE, "/files?path=/tmp/a")
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    let r = g
        .tb(reqwest::Method::DELETE, "/files?path=/tmp/a")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
    assert_eq!(r.json::<Value>().await.unwrap()["code"], "FILE_NOT_FOUND");
}

#[tokio::test]
async fn a_move_does_not_replace_its_destination() {
    let g = start().await;
    for name in ["/tmp/m1", "/tmp/m2"] {
        g.upload(name, name.as_bytes()).await;
    }
    let r = g
        .tb(
            reqwest::Method::POST,
            "/files/move?source=/tmp/m1&destination=/tmp/m2",
        )
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 409, "{}", r.text().await.unwrap());
    let r = g
        .tb(
            reqwest::Method::POST,
            "/files/move?source=/tmp/m1&destination=/tmp/m3",
        )
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn sandbox_level_errors_come_from_the_api_not_the_daemon() {
    let g = start().await;
    let r = g
        .http
        .post(format!("{}/toolbox/no-such-sandbox/process/execute", g.api))
        .bearer_auth(&g.gw.api_key)
        .json(&json!({"command": "echo"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
    assert_eq!(r.json::<Value>().await.unwrap()["source"], "DAYTONA_API");
}
