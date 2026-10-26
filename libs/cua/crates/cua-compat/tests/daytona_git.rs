//! The Daytona git toolbox at the wire over mock Spaces: authorization,
//! request validation and error shapes (the parts that need no guest
//! shell). Behaviour against a real git is covered by the shared SDK suite
//! (libs/cua/tests/compat/daytona, `git.*` cases).

use cua_compat::backend::mock::MockBackend;
use cua_compat::{Gateway, GatewayOptions, Provider};
use serde_json::{Value, json};
use std::sync::Arc;

struct Gw {
    gw: Gateway,
    http: reqwest::Client,
    base: String,
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
    let base = format!("http://{}/api", gw.api_addr);
    let http = reqwest::Client::new();
    let sb: Value = http
        .post(format!("{base}/sandbox"))
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
        base,
        id,
        _dir: dir,
    }
}

impl Gw {
    fn url(&self, p: &str) -> String {
        format!("{}/toolbox/{}/git/{p}", self.base, self.id)
    }
    async fn post(&self, p: &str, body: Value) -> (u16, Value) {
        let r = self
            .http
            .post(self.url(p))
            .bearer_auth(&self.gw.api_key)
            .json(&body)
            .send()
            .await
            .unwrap();
        let s = r.status().as_u16();
        (s, r.json().await.unwrap_or(Value::Null))
    }
    async fn get(&self, p: &str) -> (u16, Value) {
        let r = self
            .http
            .get(self.url(p))
            .bearer_auth(&self.gw.api_key)
            .send()
            .await
            .unwrap();
        let s = r.status().as_u16();
        (s, r.json().await.unwrap_or(Value::Null))
    }
}

#[tokio::test]
async fn requires_the_api_key() {
    let g = start().await;
    let r = g
        .http
        .get(g.url("status?path=r"))
        .bearer_auth("wrong")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    let r = g
        .http
        .post(format!("{}/toolbox/nope/git/init", g.base))
        .bearer_auth(&g.gw.api_key)
        .json(&json!({"path": "r"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
}

#[tokio::test]
async fn rejects_bad_requests_before_touching_the_guest() {
    let g = start().await;
    let daemon = |v: &Value| v["source"] == "DAYTONA_DAEMON";
    for (route, body, want) in [
        ("clone", json!({"path": "r"}), "url is required"),
        ("clone", json!({"url": "u"}), "path is required"),
        (
            "clone",
            json!({"url": "u", "path": "r", "branch": "--upload-pack=x"}),
            "invalid branch",
        ),
        ("init", json!({}), "path is required"),
        (
            "init",
            json!({"path": "r", "initial_branch": "-x"}),
            "invalid initial_branch",
        ),
        ("add", json!({"path": "r"}), "files is required"),
        (
            "commit",
            json!({"path": "r", "author": "a", "email": "e"}),
            "message is required",
        ),
        (
            "commit",
            json!({"path": "r", "message": "m", "email": "e"}),
            "author is required",
        ),
        (
            "push",
            json!({"path": "r", "remote": "-x"}),
            "invalid remote",
        ),
        (
            "pull",
            json!({"path": "r", "branch": "a b"}),
            "invalid branch",
        ),
        ("checkout", json!({"path": "r"}), "branch is required"),
        (
            "checkout",
            json!({"path": "r", "branch": "--orphan"}),
            "invalid branch",
        ),
        (
            "reset",
            json!({"path": "r", "mode": "bogus"}),
            "mode must be one of",
        ),
        ("restore", json!({"path": "r"}), "files is required"),
        (
            "remotes",
            json!({"path": "r", "name": "o"}),
            "url is required",
        ),
        (
            "credentials",
            json!({"username": "u"}),
            "password is required",
        ),
        ("config", json!({"value": "v"}), "key is required"),
        (
            "config",
            json!({"key": "a.b", "scope": "worktree"}),
            "scope must be",
        ),
        (
            "config",
            json!({"key": "a.b", "scope": "local"}),
            "path is required when scope is local",
        ),
        ("config/user", json!({"name": "n"}), "email is required"),
        ("branches", json!({"path": "r"}), "name is required"),
        (
            "branches",
            json!({"path": "r", "name": "-D"}),
            "invalid name",
        ),
    ] {
        let (s, v) = g.post(route, body.clone()).await;
        assert_eq!(s, 400, "{route} {body}: {v}");
        assert!(
            v["message"].as_str().unwrap_or_default().contains(want),
            "{route} {body}: {v}"
        );
        assert!(
            daemon(&v),
            "{route}: toolbox errors come from the daemon: {v}"
        );
    }
    for p in ["status", "history", "branches", "remotes"] {
        let (s, v) = g.get(p).await;
        assert_eq!(s, 400, "{p}: {v}");
        assert!(v["message"].as_str().unwrap().contains("path is required"));
    }
    let (s, v) = g.get("config").await;
    assert_eq!(
        (s, v["message"].as_str().unwrap_or_default()),
        (400, "key is required")
    );
    let (s, v) = g.get("config?key=user.name&scope=local").await;
    assert_eq!(s, 400, "{v}");
}
