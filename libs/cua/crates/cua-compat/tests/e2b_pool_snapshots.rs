//! E2B snapshots of a pool: a fork of a snapshot goes where `cua.on` says
//! (the gateway hands the placement to the backend, whose core starts it
//! on the machine that holds the checkpoint or copies it there), and a
//! snapshot taken on another machine of the pool (listed by the core, never
//! taken through this gateway) is a template: it forks, lists and deletes
//! like one taken here.

use cua_compat::backend::Source;
use cua_compat::backend::mock::MockBackend;
use cua_compat::{Gateway, GatewayOptions, Provider};
use serde_json::{Value, json};
use std::sync::Arc;

struct Gw {
    gw: Gateway,
    backend: Arc<MockBackend>,
    http: reqwest::Client,
    base: String,
    _dir: tempfile::TempDir,
}

async fn start() -> Gw {
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(MockBackend::new());
    let mut o = GatewayOptions::new(Provider::E2b);
    o.listen = "127.0.0.1:0".parse().unwrap();
    o.host_port = 0;
    o.dir = Some(dir.path().to_path_buf());
    o.create_default_user = false;
    let gw = Gateway::start(o, Some(backend.clone())).await.unwrap();
    let base = format!("http://{}", gw.api_addr);
    Gw {
        gw,
        backend,
        http: reqwest::Client::new(),
        base,
        _dir: dir,
    }
}

impl Gw {
    fn req(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, format!("{}{path}", self.base))
            .header("x-api-key", &self.gw.api_key)
    }

    async fn create(&self, body: Value) -> reqwest::Response {
        self.req(reqwest::Method::POST, "/sandboxes")
            .json(&body)
            .send()
            .await
            .unwrap()
    }

    async fn last_create(&self) -> cua_compat::backend::CreateSpec {
        self.backend.created.lock().await.last().cloned().unwrap()
    }
}

/// `AsyncSandbox.create(template=<snapshot>, metadata={"cua.on": ...})`:
/// the fork is asked for on that machine (before, the placement of a fork
/// was dropped and it always started on the gateway's machine).
#[tokio::test]
async fn a_fork_of_a_snapshot_goes_where_cua_on_says() {
    let g = start().await;
    let r = g
        .create(json!({"templateID": "base", "timeout": 60,
            "metadata": {"cua.on": "host:kvm-a"}}))
        .await;
    assert_eq!(r.status(), 201);
    let sb: Value = r.json().await.unwrap();
    let id = sb["sandboxID"].as_str().unwrap();
    let r = g
        .req(reqwest::Method::POST, &format!("/sandboxes/{id}/snapshots"))
        .json(&json!({"name": "world"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201, "{}", r.text().await.unwrap());

    for on in ["host:kvm-a", "auto", "local"] {
        let r = g
            .create(json!({"templateID": "world", "timeout": 60,
                "metadata": {"cua.on": on}}))
            .await;
        assert_eq!(r.status(), 201, "{on}: {}", r.text().await.unwrap());
        let spec = g.last_create().await;
        assert_eq!(spec.source, Source::Checkpoint("world".into()), "{on}");
        assert_eq!(spec.place.on.as_deref(), Some(on), "{on}");
    }
    // No cua.on: no placement of the gateway's own; the core starts the
    // fork where the checkpoint is.
    let r = g
        .create(json!({"templateID": "world", "timeout": 60}))
        .await;
    assert_eq!(r.status(), 201);
    let spec = g.last_create().await;
    assert_eq!(spec.source, Source::Checkpoint("world".into()));
    assert_eq!(spec.place.on, None);
}

/// A snapshot baked on another machine of the pool is a template here:
/// `create(template=...)` forks it, `list_snapshots` shows it, and
/// `delete_snapshot` removes it (afterwards a create is E2B's 404).
#[tokio::test]
async fn a_snapshot_on_another_machine_forks_lists_and_deletes() {
    let g = start().await;
    g.backend.add_checkpoint("aws-world", "baker").await;

    let r = g
        .create(json!({"templateID": "aws-world", "timeout": 60,
            "metadata": {"cua.on": "host:kvm-a"}}))
        .await;
    assert_eq!(r.status(), 201, "{}", r.text().await.unwrap());
    let spec = g.last_create().await;
    assert_eq!(spec.source, Source::Checkpoint("aws-world".into()));
    assert_eq!(spec.place.on.as_deref(), Some("host:kvm-a"));

    let listed: Value = g
        .req(reqwest::Method::GET, "/snapshots")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        listed
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["snapshotID"] == "aws-world"),
        "{listed}"
    );

    let r = g
        .req(reqwest::Method::DELETE, "/templates/aws-world")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 204, "{}", r.text().await.unwrap());
    let r = g
        .create(json!({"templateID": "aws-world", "timeout": 60}))
        .await;
    assert_eq!(r.status(), 404);
    let body: Value = r.json().await.unwrap();
    assert_eq!(body["message"], "template 'aws-world' not found", "{body}");
    // Deleting it again is E2B's 404 too.
    let r = g
        .req(reqwest::Method::DELETE, "/templates/aws-world")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
}
