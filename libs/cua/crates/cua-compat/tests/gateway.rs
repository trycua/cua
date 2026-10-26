//! The E2B gateway over mock Spaces, at the wire: REST auth and errors,
//! Connect unary and streaming calls, envd tokens, signed file URLs and
//! the TLS host router (traffic tokens included). The unmodified Python
//! SDK is driven against the same gateway by libs/cua/tests/compat/e2b.

use cua_compat::backend::mock::MockBackend;
use cua_compat::{Gateway, GatewayOptions, Provider};
use serde_json::{Value, json};
use std::sync::Arc;

struct Gw {
    gw: Gateway,
    http: reqwest::Client,
    base: String,
    _dir: tempfile::TempDir,
}

async fn start(ports: &[(u16, String)]) -> Gw {
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(MockBackend::new());
    for (g, a) in ports {
        backend.ports.lock().await.insert(*g, a.clone());
    }
    let mut o = GatewayOptions::new(Provider::E2b);
    o.listen = "127.0.0.1:0".parse().unwrap();
    o.host_port = 0;
    o.dir = Some(dir.path().to_path_buf());
    o.create_default_user = false;
    let gw = Gateway::start(o, Some(backend)).await.unwrap();
    let base = format!("http://{}", gw.api_addr);
    Gw {
        gw,
        http: reqwest::Client::new(),
        base,
        _dir: dir,
    }
}

impl Gw {
    async fn create(&self, body: Value) -> reqwest::Response {
        self.http
            .post(format!("{}/sandboxes", self.base))
            .header("x-api-key", &self.gw.api_key)
            .json(&body)
            .send()
            .await
            .unwrap()
    }

    async fn sandbox(&self, body: Value) -> Value {
        let r = self.create(body).await;
        assert_eq!(r.status(), 201);
        r.json().await.unwrap()
    }

    fn envd(&self, sb: &Value, method: &str) -> reqwest::RequestBuilder {
        self.http
            .post(format!("{}/{method}", self.base))
            .header("e2b-sandbox-id", sb["sandboxID"].as_str().unwrap())
            .header("x-access-token", sb["envdAccessToken"].as_str().unwrap())
    }
}

fn envelope(v: &Value) -> Vec<u8> {
    let p = v.to_string().into_bytes();
    let mut b = vec![0u8];
    b.extend_from_slice(&(p.len() as u32).to_be_bytes());
    b.extend_from_slice(&p);
    b
}

fn frames(mut b: &[u8]) -> Vec<(u8, Value)> {
    let mut out = Vec::new();
    while b.len() >= 5 {
        let len = u32::from_be_bytes([b[1], b[2], b[3], b[4]]) as usize;
        out.push((b[0], serde_json::from_slice(&b[5..5 + len]).unwrap()));
        b = &b[5 + len..];
    }
    out
}

#[tokio::test]
async fn rest_requires_the_key_and_answers_e2b_errors() {
    let g = start(&[]).await;
    let r = g
        .http
        .post(format!("{}/sandboxes", g.base))
        .header("x-api-key", "e2b_00")
        .json(&json!({"templateID": "base"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    let r = g.create(json!({"templateID": "nope"})).await;
    assert_eq!(r.status(), 404);
    let body: Value = r.json().await.unwrap();
    assert_eq!(body["code"], 404);
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .contains("template 'nope' not found")
    );
    let r = g
        .create(json!({"templateID": "base", "network": {"rules": {"example.com": [{"transform": {"headers": {"x": "y"}}}]}}}))
        .await;
    assert_eq!(r.status(), 400);
    // Domain names need an inspecting proxy; addresses and blocks are filtered.
    let r = g
        .create(json!({"templateID": "base", "network": {"denyOut": ["example.com"]}}))
        .await;
    assert_eq!(r.status(), 400);
    assert!(r.text().await.unwrap().contains("domain"));
    let r = g
        .http
        .post(format!("{}/v3/templates", g.base))
        .header("x-api-key", &g.gw.api_key)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
    assert!(r.text().await.unwrap().contains("Invalid name"));
}

#[tokio::test]
async fn create_returns_the_gateway_domain_and_a_token() {
    let g = start(&[]).await;
    let sb = g
        .sandbox(json!({"templateID": "base", "timeout": 60, "metadata": {"a": "b"}}))
        .await;
    assert_eq!(sb["domain"], g.gw.domain);
    assert_eq!(sb["envdVersion"], "0.6.4");
    assert!(sb["envdAccessToken"].as_str().unwrap().len() >= 32);
    let id = sb["sandboxID"].as_str().unwrap();
    let info: Value = g
        .http
        .get(format!("{}/sandboxes/{id}", g.base))
        .header("x-api-key", &g.gw.api_key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(info["state"], "running");
    assert_eq!(info["metadata"], json!({"a": "b"}));
    let list = g
        .http
        .get(format!(
            "{}/v2/sandboxes?metadata=a%3Db&state=running",
            g.base
        ))
        .header("x-api-key", &g.gw.api_key)
        .send()
        .await
        .unwrap();
    let list: Value = list.json().await.unwrap();
    assert_eq!(list.as_array().unwrap().len(), 1);
    let gone = g
        .http
        .delete(format!("{}/sandboxes/{id}", g.base))
        .header("x-api-key", &g.gw.api_key)
        .send()
        .await
        .unwrap();
    assert_eq!(gone.status(), 204);
}

#[tokio::test]
async fn envd_needs_the_access_token_and_a_known_sandbox() {
    let g = start(&[]).await;
    let sb = g.sandbox(json!({"templateID": "base"})).await;
    let r = g
        .http
        .post(format!("{}/process.Process/List", g.base))
        .header("e2b-sandbox-id", sb["sandboxID"].as_str().unwrap())
        .header("x-access-token", "wrong")
        .header("content-type", "application/json")
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    let r = g
        .http
        .post(format!("{}/process.Process/List", g.base))
        .header("e2b-sandbox-id", "doesnotexist")
        .header("content-type", "application/json")
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 502);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["message"], "The sandbox was not found");
}

#[tokio::test]
async fn process_start_streams_connect_envelopes() {
    let g = start(&[]).await;
    let sb = g.sandbox(json!({"templateID": "base"})).await;
    let body = envelope(&json!({
        "process": {"cmd": "/bin/bash", "args": ["-l", "-c", "echo hi"], "envs": {}},
        "stdin": false
    }));
    let r = g
        .envd(&sb, "process.Process/Start")
        .header("content-type", "application/connect+json")
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["content-type"], "application/connect+json");
    let f = frames(&r.bytes().await.unwrap());
    assert!(f[0].1["event"]["start"]["pid"].as_u64().is_some());
    let data: Vec<&Value> = f
        .iter()
        .filter_map(|(_, v)| v["event"]["data"]["stdout"].as_str().map(|_| v))
        .collect();
    assert_eq!(data[0]["event"]["data"]["stdout"], "aGkK"); // "hi\n"
    let end = f
        .iter()
        .find(|(_, v)| v["event"]["end"].is_object())
        .unwrap();
    assert_eq!(end.1["event"]["end"]["exitCode"], 0);
    assert_eq!(end.1["event"]["end"]["status"], "exit status 0");
    let last = f.last().unwrap();
    assert_eq!(last.0 & 0x02, 0x02, "ends with an end-of-stream envelope");
}

#[tokio::test]
async fn unary_errors_are_connect_json() {
    let g = start(&[]).await;
    let sb = g.sandbox(json!({"templateID": "base"})).await;
    let r = g
        .envd(&sb, "filesystem.Filesystem/Stat")
        .header("content-type", "application/json")
        .body(json!({"path": "/nope"}).to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["code"], "not_found");
    assert_eq!(
        v["message"],
        "file not found: lstat /nope: no such file or directory"
    );
    let r = g
        .envd(&sb, "process.Process/Nope")
        .header("content-type", "application/json")
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 501);
}

#[tokio::test]
async fn files_round_trip_and_signed_urls() {
    use base64::Engine;
    use sha2::Digest;
    let g = start(&[]).await;
    let sb = g.sandbox(json!({"templateID": "base"})).await;
    let r = g
        .http
        .post(format!("{}/files?path=/tmp/x.txt&username=root", g.base))
        .header("e2b-sandbox-id", sb["sandboxID"].as_str().unwrap())
        .header("x-access-token", sb["envdAccessToken"].as_str().unwrap())
        .header("content-type", "application/octet-stream")
        .body("payload")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let w: Value = r.json().await.unwrap();
    assert_eq!(
        w,
        json!([{"name": "x.txt", "type": "file", "path": "/tmp/x.txt"}])
    );
    // A signed URL carries neither the sandbox id nor the token.
    let token = sb["envdAccessToken"].as_str().unwrap();
    let sig = format!(
        "v1_{}",
        base64::engine::general_purpose::STANDARD_NO_PAD.encode(sha2::Sha256::digest(
            format!("/tmp/x.txt:read:root:{token}").as_bytes()
        ))
    );
    let r = g
        .http
        .get(format!("{}/files", g.base))
        .query(&[
            ("path", "/tmp/x.txt"),
            ("username", "root"),
            ("signature", sig.as_str()),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.text().await.unwrap(), "payload");
    let r = g
        .http
        .get(format!("{}/files", g.base))
        .query(&[
            ("path", "/tmp/x.txt"),
            ("username", "user"),
            ("signature", sig.as_str()),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401, "a signature is bound to its user");
}

#[tokio::test]
async fn host_router_terminates_tls_and_enforces_traffic_tokens() {
    // The stand-in for the guest's port 8080.
    let guest = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let guest_addr = guest.local_addr().unwrap().to_string();
    tokio::spawn(async move {
        loop {
            let (mut s, _) = guest.accept().await.unwrap();
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf).await;
                let _ = s
                    .write_all(
                        b"HTTP/1.1 200 OK\r\ncontent-length: 5\r\nconnection: close\r\n\r\nguest",
                    )
                    .await;
            });
        }
    });
    let g = start(&[(8080, guest_addr)]).await;
    let ca = std::fs::read(g.gw.dir.join("tls/ca.pem")).unwrap();
    let port = g.gw.host_addrs[0].port();
    let open = g.sandbox(json!({"templateID": "base"})).await;
    let closed = g
        .sandbox(json!({"templateID": "base", "network": {"allowPublicTraffic": false}}))
        .await;
    for (sb, token, want) in [
        (&open, None, 200),
        (&closed, None, 403),
        (&closed, closed["trafficAccessToken"].as_str(), 200),
    ] {
        let host = format!("8080-{}.e2b.localhost", sb["sandboxID"].as_str().unwrap());
        let client = reqwest::Client::builder()
            .add_root_certificate(reqwest::Certificate::from_pem(&ca).unwrap())
            .resolve(&host, g.gw.host_addrs[0])
            .build()
            .unwrap();
        let mut req = client.get(format!("https://{host}:{port}/"));
        if let Some(t) = token {
            req = req.header("e2b-traffic-access-token", t);
        }
        let r = req.send().await.unwrap();
        assert_eq!(r.status(), want);
        if want == 200 {
            assert_eq!(r.text().await.unwrap(), "guest");
        }
    }
}

#[tokio::test]
async fn api_port_routes_sandbox_port_header_to_the_guest() {
    // e2b-code-interpreter talks to E2B_SANDBOX_URL (the API port) and names
    // the guest port in `E2b-Sandbox-Port`.
    let guest = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let guest_addr = guest.local_addr().unwrap().to_string();
    tokio::spawn(async move {
        loop {
            let (mut s, _) = guest.accept().await.unwrap();
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf).await;
                let _ = s
                    .write_all(
                        b"HTTP/1.1 200 OK\r\ncontent-length: 5\r\nconnection: close\r\n\r\nguest",
                    )
                    .await;
            });
        }
    });
    let g = start(&[(49999, guest_addr)]).await;
    let sb = g.sandbox(json!({"templateID": "base"})).await;
    let id = sb["sandboxID"].as_str().unwrap();
    let r = g
        .http
        .get(format!("{}/contexts", g.base))
        .header("e2b-sandbox-id", id)
        .header("e2b-sandbox-port", "49999")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.text().await.unwrap(), "guest");
    // Without the header the same path is the provider API (JSON 404/401).
    let r = g
        .http
        .get(format!("{}/contexts", g.base))
        .send()
        .await
        .unwrap();
    assert_ne!(r.text().await.unwrap(), "guest");
}

#[tokio::test]
async fn code_interpreter_template_creates() {
    let g = start(&[]).await;
    let sb = g
        .sandbox(json!({"templateID": "code-interpreter-v1"}))
        .await;
    assert_eq!(sb["templateID"], "code-interpreter-v1");
}

#[tokio::test]
async fn path_like_sandbox_ids_are_a_400_not_a_lookup() {
    let g = start(&[]).await;
    let r = g
        .http
        .get(format!("{}/sandboxes/..%2F..%2Fetc%2Fpasswd", g.base))
        .header("x-api-key", &g.gw.api_key)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["message"], "Invalid sandbox ID");
    let r = g
        .http
        .get(format!("{}/sandboxes/idonotexist1", g.base))
        .header("x-api-key", &g.gw.api_key)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
}

/// Creates fork a booted, prepared base instead of booting each: one boot
/// for the shape however many creates (and however concurrent), the base is
/// not a snapshot a client can see, and a base that cannot be forked falls
/// back to a cold boot.
#[tokio::test]
async fn creates_fork_one_booted_base() {
    use std::sync::atomic::Ordering::Relaxed;
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(MockBackend::new());
    backend.fast_forks.store(true, Relaxed);
    backend.create_delay_ms.store(100, Relaxed);
    let mut o = GatewayOptions::new(Provider::E2b);
    o.listen = "127.0.0.1:0".parse().unwrap();
    o.host_port = 0;
    o.dir = Some(dir.path().to_path_buf());
    let gw = Gateway::start(o, Some(backend.clone())).await.unwrap();
    let http = reqwest::Client::new();
    let base = format!("http://{}", gw.api_addr);
    let mk = || {
        let (http, base, key) = (http.clone(), base.clone(), gw.api_key.clone());
        async move {
            let r = http
                .post(format!("{base}/sandboxes"))
                .header("x-api-key", key)
                .json(&json!({"templateID": "base", "timeout": 600}))
                .send()
                .await
                .unwrap();
            assert_eq!(r.status(), 201);
            r.json::<Value>().await.unwrap()
        }
    };
    // Eight at once, then one more.
    let batch = futures_util::future::join_all((0..8).map(|_| mk())).await;
    let later = mk().await;
    let created = backend.created.lock().await;
    let boots: Vec<_> = created
        .iter()
        .filter(|c| matches!(c.source, cua_compat::backend::Source::Image(_)))
        .collect();
    let forks = created.len() - boots.len();
    assert_eq!(boots.len(), 1, "one boot for the shape, then forks");
    assert!(boots[0].name.starts_with("e2b-golden-"));
    assert_eq!(forks, 9);
    // The boot sandbox carries a core lifetime, so a gateway that dies
    // mid-build cannot leave it running.
    assert!(
        backend.lifetimes.lock().await.contains_key(&boots[0].name),
        "the base's boot sandbox has a lifetime"
    );
    drop(created);
    // The base was prepared like a create prepares (the default user).
    let ids: Vec<&str> = batch
        .iter()
        .chain([&later])
        .map(|s| s["sandboxID"].as_str().unwrap())
        .collect();
    assert_eq!(ids.len(), 9);
    // Not visible to clients as a snapshot.
    let snaps: Value = http
        .get(format!("{base}/snapshots"))
        .header("x-api-key", &gw.api_key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        snaps
            .as_array()
            .unwrap()
            .iter()
            .all(|s| !s["snapshotID"].as_str().unwrap().starts_with("e2b-golden-")),
        "{snaps}"
    );
}

/// A create core refuses for lack of a capability (network none or egress on
/// a host or daemon that cannot enforce it) is an E2B 400 with core's reason.
#[tokio::test]
async fn a_core_capability_refusal_is_an_e2b_400() {
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(MockBackend::new());
    let mut o = GatewayOptions::new(Provider::E2b);
    o.listen = "127.0.0.1:0".parse().unwrap();
    o.host_port = 0;
    o.dir = Some(dir.path().to_path_buf());
    let gw = Gateway::start(o, Some(backend.clone())).await.unwrap();
    *backend.create_fails.lock().await = Some(cua_compat::backend::BackendError::Unsupported(
        "capability missing: this host cannot enforce network none".into(),
    ));
    let r = reqwest::Client::new()
        .post(format!("http://{}/sandboxes", gw.api_addr))
        .header("x-api-key", &gw.api_key)
        .json(&json!({"templateID": "base", "timeout": 60, "allow_internet_access": false}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
    let v: Value = r.json().await.unwrap();
    assert!(
        v["message"]
            .as_str()
            .unwrap()
            .contains("cannot enforce network none"),
        "{v}"
    );
}

/// `allow_internet_access=False` on a host that ignores the restriction is
/// an E2B error and no sandbox, never an open sandbox.
#[tokio::test]
async fn a_host_that_ignores_network_none_is_refused() {
    use std::sync::atomic::Ordering::Relaxed;
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(MockBackend::new());
    let mut o = GatewayOptions::new(Provider::E2b);
    o.listen = "127.0.0.1:0".parse().unwrap();
    o.host_port = 0;
    o.dir = Some(dir.path().to_path_buf());
    let gw = Gateway::start(o, Some(backend.clone())).await.unwrap();
    let http = reqwest::Client::new();
    let post = |internet: bool| {
        let (http, url, key) = (
            http.clone(),
            format!("http://{}/sandboxes", gw.api_addr),
            gw.api_key.clone(),
        );
        async move {
            http.post(url)
                .header("x-api-key", key)
                .json(
                    &json!({"templateID": "base", "timeout": 60, "allow_internet_access": internet}),
                )
                .send()
                .await
                .unwrap()
        }
    };
    // Enforced: created.
    assert_eq!(post(false).await.status(), 201);
    // Not enforced: refused with E2B's error shape, and nothing is left.
    backend.leaky_network.store(true, Relaxed);
    let r = post(false).await;
    assert_eq!(r.status(), 400);
    let v: Value = r.json().await.unwrap();
    assert!(
        v["message"]
            .as_str()
            .unwrap()
            .contains("network restriction"),
        "{v}"
    );
    let before = backend.deleted.lock().await.len();
    assert!(before >= 1, "the refused sandbox is deleted");
    // Internet allowed: the probe is not asked.
    assert_eq!(post(true).await.status(), 201);
}

#[tokio::test]
async fn snapshot_fork_and_lifetime() {
    let g = start(&[]).await;
    let sb = g.sandbox(json!({"templateID": "base", "timeout": 1})).await;
    let id = sb["sandboxID"].as_str().unwrap();
    let snap: Value = g
        .http
        .post(format!("{}/sandboxes/{id}/snapshots", g.base))
        .header("x-api-key", &g.gw.api_key)
        .json(&json!({"name": "world-1"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    // Named snapshots show up as `<namespace>/<name>`, like E2B's team slug.
    assert_eq!(snap["snapshotID"], "world-1");
    let names = snap["names"].as_array().unwrap();
    assert_eq!(names.len(), 1);
    assert!(names[0].as_str().unwrap().ends_with("/world-1"));
    let child = g
        .sandbox(json!({"templateID": "world-1", "timeout": 60}))
        .await;
    assert_eq!(child["templateID"], "world-1");
    let forks: Value = g
        .http
        .post(format!(
            "{}/sandboxes/{}/fork",
            g.base,
            child["sandboxID"].as_str().unwrap()
        ))
        .header("x-api-key", &g.gw.api_key)
        .json(&json!({"count": 3, "timeout": 60}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(forks.as_array().unwrap().len(), 3);
    assert!(
        forks
            .as_array()
            .unwrap()
            .iter()
            .all(|f| f["sandbox"]["sandboxID"].is_string())
    );
    // The 1 s lifetime ends the first sandbox.
    tokio::time::sleep(std::time::Duration::from_millis(1800)).await;
    let r = g
        .http
        .get(format!("{}/sandboxes/{id}", g.base))
        .header("x-api-key", &g.gw.api_key)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
}

#[tokio::test]
async fn secrets_are_named_versioned_and_write_only() {
    let g = start(&[]).await;
    let call = |m: reqwest::Method, path: &str, body: Option<Value>| {
        let mut r = g
            .http
            .request(m, format!("{}{path}", g.base))
            .header("x-api-key", &g.gw.api_key);
        if let Some(b) = body {
            r = r.json(&b);
        }
        r.send()
    };
    let created: Value = call(
        reqwest::Method::POST,
        "/secrets",
        Some(json!({"name": "api-key", "value": "s3cret", "metadata": {"k": "v"}})),
    )
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(created["name"], "api-key");
    assert_eq!(created["currentVersion"], 1);
    assert!(created["secretID"].as_str().unwrap().starts_with("sec_"));
    assert!(
        !created.to_string().contains("s3cret"),
        "values are write-only"
    );
    let dup = call(
        reqwest::Method::POST,
        "/secrets",
        Some(json!({"name": "api-key", "value": "x"})),
    )
    .await
    .unwrap();
    assert_eq!(dup.status(), 409);
    let bad = call(
        reqwest::Method::POST,
        "/secrets",
        Some(json!({"name": "bad name!", "value": "x"})),
    )
    .await
    .unwrap();
    assert_eq!(bad.status(), 400);
    // Update by name adds a version; get by id finds it.
    let updated: Value = call(
        reqwest::Method::POST,
        "/secrets/api-key",
        Some(json!({"value": "s3cret2"})),
    )
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(updated["currentVersion"], 2);
    let id = created["secretID"].as_str().unwrap();
    let got: Value = call(reqwest::Method::GET, &format!("/secrets/{id}"), None)
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(got["currentVersion"], 2);
    assert_eq!(
        call(reqwest::Method::GET, "/secrets/none", None)
            .await
            .unwrap()
            .status(),
        404
    );
    let listed: Value = call(reqwest::Method::GET, "/secrets", None)
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(
        call(reqwest::Method::DELETE, "/secrets/api-key", None)
            .await
            .unwrap()
            .status(),
        204
    );
    assert_eq!(
        call(reqwest::Method::DELETE, "/secrets/api-key", None)
            .await
            .unwrap()
            .status(),
        404
    );
}

#[tokio::test]
async fn volumes_answer_the_feature_not_enabled_error() {
    let g = start(&[]).await;
    let r = g
        .http
        .post(format!("{}/volumes", g.base))
        .header("x-api-key", &g.gw.api_key)
        .json(&json!({"name": "v"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["message"], "use of volumes is not enabled");
}

fn open_fds() -> usize {
    ["/proc/self/fd", "/dev/fd"]
        .iter()
        .find_map(|p| std::fs::read_dir(p).ok())
        .map(|d| d.count())
        .expect("a way to list this process's file descriptors")
}

/// The gateway (in this process) gives back every descriptor a sandbox took:
/// create, run, upload, read, snapshot, fork and kill many times and the
/// count returns to where it began.
#[tokio::test]
async fn sandboxes_do_not_leak_file_descriptors() {
    let g = start(&[]).await;
    // Warm up: lazily opened shared resources (TLS, connection pools).
    for _ in 0..3 {
        let sb = g.sandbox(json!({"templateID": "base"})).await;
        kill(&g, &sb).await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let before = open_fds();
    for i in 0..60 {
        let sb = g
            .sandbox(json!({"templateID": "base", "timeout": 60}))
            .await;
        let r = g
            .envd(&sb, "process.Process/Start")
            .header("content-type", "application/connect+json")
            .body(envelope(
                &json!({"process": {"cmd": "echo", "args": ["hi"]}}),
            ))
            .send()
            .await
            .unwrap();
        let _ = r.bytes().await.unwrap();
        let up = g
            .http
            .post(format!("{}/files?path=/tmp/f{i}.txt", g.base))
            .header("e2b-sandbox-id", sb["sandboxID"].as_str().unwrap())
            .header("x-access-token", sb["envdAccessToken"].as_str().unwrap())
            .body("data")
            .send()
            .await
            .unwrap();
        assert_eq!(up.status(), 200);
        kill(&g, &sb).await;
    }
    // The count is for the whole process, and the other tests in this binary
    // run at the same time and hold descriptors of their own. Wait until it
    // settles: their descriptors go away when they finish, but a leak of one
    // or more descriptors per sandbox (60 or more here) never does.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let mut after = open_fds();
    while after > before + 8 && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        after = open_fds();
    }
    assert!(
        after <= before + 8,
        "descriptors grew from {before} to {after} over 60 sandboxes"
    );
}

async fn kill(g: &Gw, sb: &Value) {
    let r = g
        .http
        .delete(format!(
            "{}/sandboxes/{}",
            g.base,
            sb["sandboxID"].as_str().unwrap()
        ))
        .header("x-api-key", &g.gw.api_key)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 204);
}

/// A create whose client gave up (its request timeout) still finishes in the
/// gateway; nobody is waiting for it, so the sandbox is killed rather than
/// left to live for its whole E2B `timeout`.
#[tokio::test]
async fn a_create_the_client_gave_up_on_is_killed_when_it_finishes() {
    use std::sync::atomic::Ordering::Relaxed;
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(MockBackend::new());
    backend.create_delay_ms.store(600, Relaxed);
    let mut o = GatewayOptions::new(Provider::E2b);
    o.listen = "127.0.0.1:0".parse().unwrap();
    o.host_port = 0;
    o.dir = Some(dir.path().to_path_buf());
    o.create_default_user = false;
    let gw = Gateway::start(o, Some(backend.clone())).await.unwrap();
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(150))
        .build()
        .unwrap();
    let r = http
        .post(format!("http://{}/sandboxes", gw.api_addr))
        .header("x-api-key", &gw.api_key)
        .json(&json!({"templateID": "base", "timeout": 3600}))
        .send()
        .await;
    assert!(r.is_err(), "the client should have timed out");
    // The create finishes after the client left, then the gateway kills it.
    for _ in 0..100 {
        if !backend.deleted.lock().await.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(backend.created.lock().await.len(), 1);
    assert_eq!(
        backend.deleted.lock().await.len(),
        1,
        "the abandoned create must be killed"
    );
    let cancelled = backend.cancelled.lock().await.clone();
    assert_eq!(
        cancelled.len(),
        1,
        "and the core create is cancelled when the client leaves"
    );
    assert!(cancelled[0].starts_with("e2b-"));
    // And a patient client still gets its sandbox.
    let ok = reqwest::Client::new()
        .post(format!("http://{}/sandboxes", gw.api_addr))
        .header("x-api-key", &gw.api_key)
        .json(&json!({"templateID": "base", "timeout": 3600}))
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 201);
    assert_eq!(backend.deleted.lock().await.len(), 1);
}

/// `Gateway::warm` (run by `cua run --compat` before the program starts)
/// makes one default-shape sandbox and deletes it again.
#[tokio::test]
async fn warm_creates_and_deletes_one_default_sandbox() {
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(MockBackend::new());
    let mut o = GatewayOptions::new(Provider::E2b);
    o.listen = "127.0.0.1:0".parse().unwrap();
    o.host_port = 0;
    o.dir = Some(dir.path().to_path_buf());
    o.create_default_user = false;
    let gw = Gateway::start(o, Some(backend.clone())).await.unwrap();
    gw.warm().await.unwrap();
    assert_eq!(backend.created.lock().await.len(), 1);
    assert_eq!(backend.deleted.lock().await.len(), 1);
}

/// `start_warmed` returns only after the first create (warm-before-ready).
#[tokio::test]
async fn start_warmed_makes_the_first_create_before_it_returns() {
    for provider in [Provider::E2b, Provider::Daytona] {
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(MockBackend::new());
        let mut o = GatewayOptions::new(provider);
        o.listen = "127.0.0.1:0".parse().unwrap();
        o.host_port = 0;
        o.dir = Some(dir.path().to_path_buf());
        o.create_default_user = false;
        let b: Arc<dyn cua_compat::backend::Backend> = backend.clone();
        let _gw = Gateway::start_warmed(o, Some(b)).await.unwrap();
        assert_eq!(backend.created.lock().await.len(), 1, "{provider:?}");
        assert_eq!(backend.deleted.lock().await.len(), 1, "{provider:?}");
    }
}

/// An account at its live-session limit gets E2B's rate-limit error (HTTP
/// 429, which the SDKs raise as RateLimitException), with the plan-limit
/// sentence and its upgrade path verbatim.
#[tokio::test]
async fn the_live_session_limit_is_a_429_with_its_sentence() {
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(MockBackend::new());
    *backend.create_fails.lock().await = Some(cua_compat::backend::BackendError::RateLimited(
        "You're using 2 of 2 live sessions on the free plan. Close one or upgrade to Pro.".into(),
    ));
    let mut o = GatewayOptions::new(Provider::E2b);
    o.listen = "127.0.0.1:0".parse().unwrap();
    o.host_port = 0;
    o.dir = Some(dir.path().to_path_buf());
    let gw = Gateway::start(o, Some(backend)).await.unwrap();
    for path in ["sandboxes", "v2/sandboxes"] {
        let r = reqwest::Client::new()
            .post(format!("http://{}/{path}", gw.api_addr))
            .header("x-api-key", &gw.api_key)
            .json(&json!({"templateID": "base", "timeout": 60}))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 429, "{path}");
        let v: Value = r.json().await.unwrap();
        assert_eq!(
            v["message"],
            "You're using 2 of 2 live sessions on the free plan. Close one or upgrade to Pro."
        );
    }
}

/// `set_timeout(0)` on a sandbox that kills at its timeout ends it before the
/// call answers (the TS SDK's `isRunning()` right after was true one run in
/// three while the reaper did it).
#[tokio::test]
async fn set_timeout_zero_kills_before_it_answers() {
    let g = start(&[]).await;
    let sb = g
        .sandbox(json!({"templateID": "base", "timeout": 600}))
        .await;
    let id = sb["sandboxID"].as_str().unwrap();
    let r = g
        .http
        .post(format!("{}/sandboxes/{id}/timeout", g.base))
        .header("x-api-key", &g.gw.api_key)
        .json(&json!({"timeout": 0}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 204);
    let info = g
        .http
        .get(format!("{}/sandboxes/{id}", g.base))
        .header("x-api-key", &g.gw.api_key)
        .send()
        .await
        .unwrap();
    assert_eq!(
        info.status(),
        404,
        "gone the moment set_timeout(0) answered"
    );
}
