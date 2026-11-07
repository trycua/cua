// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Default Space settings (`GET | PUT /v1/spaces/settings`), their
//! resolution (`GET /v1/spaces/default`), and detach-not-delete for a
//! removed computer's Spaces (`DELETE /v1/machines/{id}`).

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use cua_relay::devices::DevicePolicy;
use cua_relay::oidc::testing::FakeIssuer;
use cua_relay::oidc::{OidcConfig, OidcValidator};
use cua_relay::server::{Relay, RelayConfig};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

const ISSUER: &str = "https://auth.test/realms/cua";
const GONE: &str = "its computer was removed from the account";

fn config(issuer: &FakeIssuer, dir: &Path) -> RelayConfig {
    RelayConfig {
        oidc: Some(Arc::new(OidcValidator::with_jwks(
            OidcConfig::new(ISSUER),
            issuer.jwks(),
        ))),
        device_policy: DevicePolicy {
            grace_secs: 14 * 86_400,
            ..DevicePolicy::default()
        },
        state_file: Some(dir.join("machines.json")),
        ..RelayConfig::default()
    }
}

async fn serve(relay: &Relay) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let relay = relay.clone();
    tokio::spawn(async move {
        let _ = relay.serve(listener, std::future::pending()).await;
    });
    base
}

struct Env {
    relay: Relay,
    base: String,
    issuer: FakeIssuer,
    dir: tempfile::TempDir,
    ada: String,
    bob: String,
}

impl Env {
    async fn start() -> Env {
        let issuer = FakeIssuer::new(ISSUER);
        let dir = tempfile::tempdir().unwrap();
        let relay = Relay::new(config(&issuer, dir.path()));
        let base = serve(&relay).await;
        let ada = issuer.token("ada-sub", Some("ada@example.com"), "cua-relay", 3000);
        let bob = issuer.token("bob-sub", Some("bob@example.com"), "cua-relay", 3000);
        Env {
            relay,
            base,
            issuer,
            dir,
            ada,
            bob,
        }
    }

    async fn call(
        &self,
        method: Method,
        path: &str,
        token: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut r = reqwest::Client::new()
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(token);
        if let Some(b) = body {
            r = r.json(&b);
        }
        let r = r.send().await.unwrap();
        let status = r.status();
        let text = r.text().await.unwrap_or_default();
        (
            status,
            serde_json::from_str(&text).unwrap_or(Value::String(text)),
        )
    }

    /// Registers a computer (or, with `host`, a Space on it); returns its
    /// id and machine token.
    async fn register(
        &self,
        token: &str,
        id: &str,
        host: Option<&str>,
        allow: &[&str],
    ) -> (String, String) {
        let (s, v) = self
            .call(
                Method::POST,
                "/v1/machines",
                token,
                Some(json!({"id": id, "name": id, "host": host, "allow": allow})),
            )
            .await;
        assert!(s.is_success(), "register {id}: {s} {v}");
        (
            id.to_owned(),
            v["machine_token"].as_str().unwrap().to_owned(),
        )
    }

    async fn new_id(&self) -> String {
        format!("m{}", uuid::Uuid::new_v4().simple())
    }

    /// Connects `id` and returns once the relay sees it.
    async fn connect(&self, id: &str, token: &str) -> CancellationToken {
        let unused = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local = unused.local_addr().unwrap();
        drop(unused);
        let mut join = cua_relay::client::JoinConfig::new(
            self.base.replace("http://", "ws://"),
            token.to_owned(),
            id.to_owned(),
            local,
        );
        join.heartbeat = Duration::from_secs(1);
        let stop = CancellationToken::new();
        tokio::spawn(cua_relay::client::run(join, stop.clone()));
        for _ in 0..200 {
            if self.relay.machine_ids().contains(&id.to_owned()) {
                return stop;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("{id} did not connect");
    }

    async fn listed(&self, token: &str) -> Vec<String> {
        let (s, v) = self.call(Method::GET, "/v1/machines", token, None).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        v["machines"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["id"].as_str().unwrap().to_owned())
            .collect()
    }

    async fn put(&self, token: &str, body: Value) -> (StatusCode, Value) {
        self.call(Method::PUT, "/v1/spaces/settings", token, Some(body))
            .await
    }

    async fn resolve(&self, token: &str, here: Option<&str>) -> Value {
        let q = here.map(|h| format!("?here={h}")).unwrap_or_default();
        let (s, v) = self
            .call(Method::GET, &format!("/v1/spaces/default{q}"), token, None)
            .await;
        assert_eq!(s, StatusCode::OK, "{v}");
        v
    }
}

fn space(id: &str) -> String {
    format!("relay:{id}")
}

fn orders(v: &Value) -> Vec<(String, bool, String)> {
    v["order"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["space"].as_str().unwrap().to_owned(),
                e["online"].as_bool().unwrap(),
                e["role"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

#[tokio::test]
async fn settings_start_unset_and_round_trip() {
    let env = Env::start().await;
    let (a, _) = env.register(&env.ada, &env.new_id().await, None, &[]).await;
    let (b, _) = env.register(&env.ada, &env.new_id().await, None, &[]).await;

    let (s, v) = env
        .call(Method::GET, "/v1/spaces/settings", &env.ada, None)
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        v,
        json!({"default_space": null, "fallback": [], "fallback_on": true, "updated_at": 0})
    );

    let body = json!({
        "default_space": space(&a),
        "fallback": [format!("{}/dev-1", space(&b))],
        "fallback_on": false,
    });
    let (s, v) = env.put(&env.ada, body.clone()).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(v["updated_at"].as_u64().unwrap() > 0);
    let (_, got) = env
        .call(Method::GET, "/v1/spaces/settings", &env.ada, None)
        .await;
    assert_eq!(got, v);
    assert_eq!(got["default_space"], body["default_space"]);
    assert_eq!(got["fallback_on"], false);

    // PUT replaces the whole object: what is not sent goes back to its default.
    let (s, v) = env.put(&env.ada, json!({"default_space": space(&b)})).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["fallback"], json!([]));
    assert_eq!(v["fallback_on"], true);

    // Settings are per account.
    let (_, bobs) = env
        .call(Method::GET, "/v1/spaces/settings", &env.bob, None)
        .await;
    assert_eq!(bobs["default_space"], Value::Null);
}

#[tokio::test]
async fn bad_settings_are_refused_with_the_reason() {
    let env = Env::start().await;
    let (a, a_token) = env.register(&env.ada, &env.new_id().await, None, &[]).await;
    let (bob_box, _) = env.register(&env.bob, &env.new_id().await, None, &[]).await;

    for bad in [
        "abcdefgh12",
        "relay:short",
        "relay:",
        "relay:ABCDEFGH12",
        "relay:abcdefgh12/UP",
    ] {
        let (s, v) = env.put(&env.ada, json!({"default_space": bad})).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{bad}");
        assert_eq!(v["error"], format!("invalid Space id: {bad}"));
    }
    let (s, v) = env
        .put(&env.ada, json!({"fallback": [space(&a), space(&a)]}))
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(v["error"], format!("duplicate Space id: {}", space(&a)));

    let many: Vec<String> = (0..17).map(|i| format!("{}/s-{i}", space(&a))).collect();
    let (s, v) = env.put(&env.ada, json!({"fallback": many})).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(v["error"], "at most 16 fallback Spaces");
    let sixteen: Vec<String> = (0..16).map(|i| format!("{}/s-{i}", space(&a))).collect();
    let (s, v) = env.put(&env.ada, json!({"fallback": sixteen})).await;
    assert_eq!(s, StatusCode::OK, "{v}");

    // Someone else's computer, and one that does not exist, look the same.
    for other in [space(&bob_box), space("doesnotexist1")] {
        let (s, v) = env.put(&env.ada, json!({"default_space": other})).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{other}");
        assert_eq!(v["error"], format!("{other} is not a computer you can use"));
        let (s, _) = env.put(&env.ada, json!({"fallback": [other]})).await;
        assert_eq!(s, StatusCode::FORBIDDEN);
    }
    // A refused change leaves the settings as they were.
    let (_, got) = env
        .call(Method::GET, "/v1/spaces/settings", &env.ada, None)
        .await;
    assert_eq!(got["fallback"].as_array().unwrap().len(), 16);

    // Shared with Bob: now he can use it.
    let (shared, _) = env
        .register(&env.ada, &env.new_id().await, None, &["bob@example.com"])
        .await;
    let (s, v) = env
        .put(&env.bob, json!({"default_space": space(&shared)}))
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");

    // A computer token is refused, on both routes.
    for (method, path, body) in [
        (Method::GET, "/v1/spaces/settings", None),
        (Method::PUT, "/v1/spaces/settings", Some(json!({}))),
        (Method::GET, "/v1/spaces/default", None),
    ] {
        let (s, v) = env.call(method, path, &a_token, body).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{path}");
        assert_eq!(v["error"], "a machine token cannot do this");
    }
}

#[tokio::test]
async fn settings_survive_a_restart() {
    let env = Env::start().await;
    let (a, _) = env.register(&env.ada, &env.new_id().await, None, &[]).await;
    let (s, v) = env
        .put(
            &env.ada,
            json!({"default_space": space(&a), "fallback": [space(&a)], "fallback_on": false}),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let file = env.dir.path().join("machines.spaces.json");
    assert!(file.exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "private file mode");
    }

    let relay2 = Relay::new(config(&env.issuer, env.dir.path()));
    let base2 = serve(&relay2).await;
    let r = reqwest::Client::new()
        .get(format!("{base2}/v1/spaces/settings"))
        .bearer_auth(&env.ada)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let got: Value = r.json().await.unwrap();
    assert_eq!(got, v);
}

#[tokio::test]
async fn the_default_resolves_to_the_first_online_place() {
    let env = Env::start().await;
    // Created oldest first: old, then new. The ids sort the other way round,
    // so "oldest first" is not just id order.
    let (old, old_token) = env
        .register(
            &env.ada,
            &format!("mzzzz{}", uuid::Uuid::new_v4().simple()),
            None,
            &[],
        )
        .await;
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let (new, new_token) = env
        .register(
            &env.ada,
            &format!("maaaa{}", uuid::Uuid::new_v4().simple()),
            None,
            &[],
        )
        .await;
    let (dflt, dflt_token) = env.register(&env.ada, &env.new_id().await, None, &[]).await;
    let (here, here_token) = env.register(&env.ada, &env.new_id().await, None, &[]).await;

    // Nothing set.
    let v = env.resolve(&env.ada, Some(&here)).await;
    assert_eq!(v["reason"], "unset");
    assert_eq!(v["space"], space(&here));
    assert_eq!(v["default_space"], Value::Null);
    let v = env.resolve(&env.ada, None).await;
    assert_eq!(
        (v["reason"].as_str(), v["space"].clone()),
        (Some("unset"), Value::Null)
    );

    let (s, v) = env
        .put(&env.ada, json!({"default_space": space(&dflt)}))
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");

    // Nothing online: use this device. With no `here` (or one not on the
    // relay) the answer is null and the client uses this device.
    let v = env.resolve(&env.ada, Some(&here)).await;
    assert_eq!(
        (v["reason"].as_str(), v["space"].clone()),
        (Some("here"), json!(space(&here)))
    );
    assert_eq!(v["default_online"], false);
    assert_eq!(v["default_space"], space(&dflt));
    assert_eq!(
        orders(&v),
        vec![
            (space(&dflt), false, "default".into()),
            (space(&here), false, "here".into()),
        ]
    );
    let v = env.resolve(&env.ada, None).await;
    assert_eq!(
        (v["reason"].as_str(), v["space"].clone()),
        (Some("here"), Value::Null)
    );
    let v = env.resolve(&env.ada, Some("notonrelay1")).await;
    assert_eq!(v["space"], Value::Null);

    // The newer computer comes online first, then the older: automatic
    // order is oldest first.
    let _s1 = env.connect(&new, &new_token).await;
    let v = env.resolve(&env.ada, Some(&here)).await;
    assert_eq!(
        (v["reason"].as_str(), v["space"].clone()),
        (Some("fallback"), json!(space(&new)))
    );
    let _s2 = env.connect(&old, &old_token).await;
    let v = env.resolve(&env.ada, Some(&here)).await;
    assert_eq!(v["space"], space(&old));
    assert_eq!(
        orders(&v),
        vec![
            (space(&dflt), false, "default".into()),
            (space(&old), true, "fallback".into()),
            (space(&new), true, "fallback".into()),
            (space(&here), false, "here".into()),
        ]
    );

    // An explicit fallback list is used in its order, offline entries shown.
    let (s, v) = env
        .put(
            &env.ada,
            json!({"default_space": space(&dflt), "fallback": [space(&new), space(&here), space(&old)]}),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let v = env.resolve(&env.ada, Some(&here)).await;
    assert_eq!(v["space"], space(&new));
    assert_eq!(
        orders(&v),
        vec![
            (space(&dflt), false, "default".into()),
            (space(&new), true, "fallback".into()),
            (space(&old), true, "fallback".into()),
            (space(&here), false, "here".into()),
        ],
        "here is skipped inside the list and always last"
    );

    // Fallback off: only the default, then this device.
    let (s, _) = env
        .put(
            &env.ada,
            json!({"default_space": space(&dflt), "fallback_on": false}),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    let v = env.resolve(&env.ada, Some(&here)).await;
    assert_eq!(
        (v["reason"].as_str(), v["space"].clone()),
        (Some("here"), json!(space(&here)))
    );
    assert_eq!(orders(&v).len(), 2);

    // The default comes online: it wins.
    let (s, _) = env
        .put(&env.ada, json!({"default_space": space(&dflt)}))
        .await;
    assert_eq!(s, StatusCode::OK);
    let _s3 = env.connect(&dflt, &dflt_token).await;
    let v = env.resolve(&env.ada, Some(&here)).await;
    assert_eq!(
        (v["reason"].as_str(), v["space"].clone()),
        (Some("default"), json!(space(&dflt)))
    );
    assert_eq!(v["default_online"], true);
    // A Space of the default computer counts as online with it.
    let child = format!("{}/dev-1", space(&dflt));
    let (s, _) = env.put(&env.ada, json!({"default_space": child})).await;
    assert_eq!(s, StatusCode::OK);
    let v = env.resolve(&env.ada, Some(&here)).await;
    assert_eq!(
        (v["reason"].as_str(), v["space"].clone()),
        (Some("default"), json!(child))
    );
    let _ = here_token;
}

#[tokio::test]
async fn a_default_that_is_no_longer_visible_is_skipped_but_kept() {
    let env = Env::start().await;
    let (shared, shared_token) = env
        .register(&env.ada, &env.new_id().await, None, &["bob@example.com"])
        .await;
    let (mine, mine_token) = env.register(&env.bob, &env.new_id().await, None, &[]).await;
    let _s1 = env.connect(&shared, &shared_token).await;
    let _s2 = env.connect(&mine, &mine_token).await;

    let (s, v) = env
        .put(&env.bob, json!({"default_space": space(&shared)}))
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(
        env.resolve(&env.bob, Some(&mine)).await["space"],
        space(&shared)
    );

    // Ada stops sharing it with Bob: skipped, Bob's own computer is used,
    // and the stored default is still there.
    let (s, v) = env
        .call(
            Method::PATCH,
            &format!("/v1/machines/{shared}"),
            &env.ada,
            Some(json!({"allow": []})),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let v = env.resolve(&env.bob, Some(&mine)).await;
    assert_eq!(v["default_space"], space(&shared));
    assert_eq!(v["default_online"], false);
    assert_eq!(
        (v["reason"].as_str(), v["space"].clone()),
        (Some("here"), json!(space(&mine)))
    );
    assert!(!orders(&v).iter().any(|(s, ..)| s == &space(&shared)));

    // Shared again: it is back.
    let (s, _) = env
        .call(
            Method::PATCH,
            &format!("/v1/machines/{shared}"),
            &env.ada,
            Some(json!({"allow": ["bob@example.com"]})),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        env.resolve(&env.bob, Some(&mine)).await["space"],
        space(&shared)
    );
}

#[tokio::test]
async fn removing_a_computer_detaches_its_spaces_and_registering_it_again_restores_them() {
    let env = Env::start().await;
    let host = env.new_id().await;
    let (_, host_token) = env.register(&env.ada, &host, None, &[]).await;
    let (kid, kid_token) = env
        .register(&env.ada, &env.new_id().await, Some(&host), &[])
        .await;
    let _live = env.connect(&kid, &kid_token).await;
    let probe = |token: &str| {
        let url = format!("{}/m/{kid}/probe", env.base);
        let token = token.to_owned();
        async move {
            let r = reqwest::Client::new()
                .get(url)
                .bearer_auth(token)
                .send()
                .await
                .unwrap();
            (r.status(), r.text().await.unwrap_or_default())
        }
    };
    assert_eq!(env.listed(&env.ada).await.len(), 2);
    let (s, body) = probe(&env.ada).await;
    assert_ne!(s, StatusCode::FORBIDDEN, "{body}");

    // The computer's own token removes it.
    let (s, v) = env
        .call(
            Method::DELETE,
            &format!("/v1/machines/{host}"),
            &host_token,
            None,
        )
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT, "{v}");
    assert!(env.listed(&env.ada).await.is_empty(), "the Space is hidden");
    let (s, body) = probe(&env.ada).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert!(body.contains(GONE), "{body}");
    // Its live connection was cut, and its token is refused too.
    for _ in 0..100 {
        if !env.relay.machine_ids().contains(&kid) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(!env.relay.machine_ids().contains(&kid));
    let (s, v) = env
        .call(Method::GET, "/v1/machines", &kid_token, None)
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{v}");
    assert_eq!(v["error"], GONE);
    // Not something to default to while it is detached.
    let (s, _) = env
        .put(&env.ada, json!({"default_space": space(&kid)}))
        .await;
    assert_eq!(s, StatusCode::OK, "it is still the account's own record");
    let v = env.resolve(&env.ada, None).await;
    assert_eq!(v["default_space"], space(&kid));
    assert!(orders(&v).is_empty(), "{v}");

    // The computer registers again with the same id: the Space is back.
    env.register(&env.ada, &host, None, &[]).await;
    let ids = env.listed(&env.ada).await;
    assert!(ids.contains(&kid) && ids.contains(&host), "{ids:?}");
    let (s, body) = probe(&env.ada).await;
    assert_ne!(s, StatusCode::FORBIDDEN, "{body}");
    let (s, v) = env
        .call(Method::GET, "/v1/machines", &kid_token, None)
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(
        orders(&env.resolve(&env.ada, None).await).len(),
        1,
        "the default is usable again"
    );
}

#[tokio::test]
async fn deleting_a_computer_with_its_spaces_forgets_them() {
    let env = Env::start().await;
    let host = env.new_id().await;
    let (_, host_token) = env.register(&env.ada, &host, None, &[]).await;
    let (kid, kid_token) = env
        .register(&env.ada, &env.new_id().await, Some(&host), &[])
        .await;
    let (kid2, _) = env
        .register(&env.ada, &env.new_id().await, Some(&host), &[])
        .await;
    let (other_host, _) = env.register(&env.ada, &env.new_id().await, None, &[]).await;
    let (other_kid, _) = env
        .register(&env.ada, &env.new_id().await, Some(&other_host), &[])
        .await;
    let _live = env.connect(&kid, &kid_token).await;

    let (s, v) = env
        .call(
            Method::DELETE,
            &format!("/v1/machines/{host}?spaces=maybe"),
            &env.ada,
            None,
        )
        .await;
    assert_eq!(
        (s, v["error"].clone()),
        (StatusCode::BAD_REQUEST, json!("spaces: delete"))
    );
    // A computer token cannot ask for it, and nothing was removed.
    let (s, v) = env
        .call(
            Method::DELETE,
            &format!("/v1/machines/{host}?spaces=delete"),
            &host_token,
            None,
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{v}");
    assert_eq!(env.listed(&env.ada).await.len(), 5);
    // Someone it is not shared with cannot see it at all.
    let (s, _) = env
        .call(
            Method::DELETE,
            &format!("/v1/machines/{host}?spaces=delete"),
            &env.bob,
            None,
        )
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    let (s, v) = env
        .call(
            Method::DELETE,
            &format!("/v1/machines/{host}?spaces=delete"),
            &env.ada,
            None,
        )
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT, "{v}");
    assert_eq!(
        env.listed(&env.ada).await.len(),
        2,
        "only the other computer and its Space remain"
    );
    for _ in 0..100 {
        if !env.relay.machine_ids().contains(&kid) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(
        !env.relay.machine_ids().contains(&kid),
        "connection dropped"
    );
    let (s, _) = env
        .call(Method::GET, &format!("/v1/machines/{kid2}"), &env.ada, None)
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _) = env
        .call(Method::GET, "/v1/machines", &kid_token, None)
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "its token is gone");

    // Registering the computer again does not bring them back.
    env.register(&env.ada, &host, None, &[]).await;
    let ids = env.listed(&env.ada).await;
    assert_eq!(ids.len(), 3, "{ids:?}");
    assert!(!ids.contains(&kid) && !ids.contains(&kid2) && ids.contains(&other_kid));
}
