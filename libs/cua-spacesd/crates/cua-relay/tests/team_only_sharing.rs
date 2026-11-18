// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Sharing a computer only with people in the owner's team (CUA-1883),
//! against a fake Spaces API. The relay stores no team data: it asks
//! `/internal/v1/share-check` when a share is made and when one is used
//! (cached, fail closed), honors `machine_role`, and cleans up outside
//! shares through `/internal/v1/share-sweep`.
// The hosted relay's optional features (Cargo feature `hosted`).
#![cfg(feature = "hosted")]

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::http::{HeaderMap, StatusCode as AxStatus};
use cua_relay::assertion::ASSERTION_HEADER;
use cua_relay::oidc::testing::FakeIssuer;
use cua_relay::oidc::{OidcConfig, OidcValidator};
use cua_relay::org_access::{OrgAccessConfig, TeamsSource};
use cua_relay::server::{Relay, RelayConfig};
use reqwest::StatusCode;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

const ISSUER: &str = "https://auth.test/realms/cua";
/// Long enough that "inside the TTL" holds on a slow runner; the tests that
/// need it to pass sleep past it with a margin.
const TTL: Duration = Duration::from_secs(4);

/// The fake Spaces API: the owner's team is `team` (lowercase emails or
/// account ids); `no_team` makes the owner a user without a team.
#[derive(Default)]
struct Api {
    down: bool,
    no_team: bool,
    team: HashSet<String>,
    check_calls: usize,
    sweep_calls: usize,
    /// `machine_role` of the whole-team share (empty: `editor`).
    machine_role: String,
    /// Members of org `org-1` (account ids).
    members: HashSet<String>,
}

type Shared = Arc<Mutex<Api>>;

fn in_team(a: &Api, who: &str) -> bool {
    !a.no_team && a.team.contains(&who.to_ascii_lowercase())
}

async fn fake_api(state: Shared) -> String {
    let (check, sweep, access) = (state.clone(), state.clone(), state.clone());
    let app = axum::Router::new()
        .route(
            "/internal/v1/share-check",
            axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
                let state = check.clone();
                async move {
                    let mut a = state.lock().unwrap();
                    if a.down {
                        return (AxStatus::SERVICE_UNAVAILABLE, axum::Json(json!({})));
                    }
                    a.check_calls += 1;
                    let who: Vec<String> = body["who"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|w| w.as_str().unwrap().to_owned())
                        .collect();
                    let results: Vec<Value> = who
                        .iter()
                        .map(|w| {
                            let ok = in_team(&a, w);
                            json!({"who": w, "allowed": ok,
                                   "reason": if ok { "" } else if a.no_team { "no_team" } else { "not_in_team" }})
                        })
                        .collect();
                    (
                        AxStatus::OK,
                        axum::Json(json!({"can_share": !a.no_team, "results": results, "ttl": 60})),
                    )
                }
            }),
        )
        .route(
            "/internal/v1/share-sweep",
            axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
                let state = sweep.clone();
                async move {
                    let mut a = state.lock().unwrap();
                    if a.down {
                        return (AxStatus::SERVICE_UNAVAILABLE, axum::Json(json!({})));
                    }
                    a.sweep_calls += 1;
                    let mut remove = Vec::new();
                    for m in body["machines"].as_array().unwrap() {
                        let gone: Vec<Value> = m["who"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .filter(|w| !in_team(&a, w.as_str().unwrap()))
                            .cloned()
                            .collect();
                        if !gone.is_empty() {
                            remove.push(json!({"machine_id": m["machine_id"], "who": gone}));
                        }
                    }
                    (
                        AxStatus::OK,
                        axum::Json(json!({"remove": remove, "recorded": remove.len()})),
                    )
                }
            }),
        )
        .route(
            "/internal/v1/access",
            axum::routing::get(
                move |axum::extract::Query(q): axum::extract::Query<
                    std::collections::HashMap<String, String>,
                >| {
                    let state = access.clone();
                    async move {
                        let a = state.lock().unwrap();
                        let sub = q.get("sub").cloned().unwrap_or_default();
                        let member = a.members.contains(&sub);
                        let role = if a.machine_role.is_empty() {
                            "editor"
                        } else {
                            a.machine_role.as_str()
                        };
                        (
                            AxStatus::OK,
                            axum::Json(json!({
                                "member": member, "role": "member",
                                "machine_in_org": true, "machine_role": role,
                                "device_approved": true, "ttl": 60,
                            })),
                        )
                    }
                },
            ),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    url
}

fn role_of(assertion: &str) -> String {
    use base64::Engine as _;
    let payload = assertion.split('.').nth(1).expect("payload");
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .expect("base64url");
    let v: Value = serde_json::from_slice(&raw).expect("json");
    v["role"].as_str().unwrap_or_default().to_owned()
}

struct Rig {
    issuer: FakeIssuer,
    relay: Relay,
    base: String,
    api: Shared,
    /// Assertions the computer's local server was handed.
    seen: Arc<Mutex<Vec<String>>>,
    id: String,
    ada: String,
    stop: CancellationToken,
    _state: tempfile::TempDir,
}

async fn rig(team_only: bool) -> Rig {
    let issuer = FakeIssuer::new(ISSUER);
    let state = tempfile::tempdir().unwrap();
    let api: Shared = Arc::default();
    let api_url = fake_api(api.clone()).await;
    let relay = Relay::new(RelayConfig {
        oidc: Some(Arc::new(OidcValidator::with_jwks(
            OidcConfig::new(ISSUER),
            issuer.jwks(),
        ))),
        state_file: Some(state.path().join("machines.json")),
        device_enrollment: false,
        org_access: OrgAccessConfig {
            source: TeamsSource::Org,
            api_url: Some(api_url),
            client_id: "cua-relay-svc".into(),
            ttl: Some(TTL),
            team_only_sharing: team_only,
            ..Default::default()
        },
        ..RelayConfig::default()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let serving = relay.clone();
    tokio::spawn(async move {
        let _ = serving.serve(listener, std::future::pending()).await;
    });
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let record = seen.clone();
    let app = axum::Router::new().fallback(move |headers: HeaderMap| {
        let record = record.clone();
        async move {
            if let Some(a) = headers.get(ASSERTION_HEADER).and_then(|v| v.to_str().ok()) {
                record.lock().unwrap().push(a.to_owned());
            }
            "ok"
        }
    });
    let local = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local_addr = local.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(local, app).await;
    });
    let ada = issuer.token("ada", Some("ada@team.test"), "cua-relay", 600);
    let id = format!("space-{}", uuid::Uuid::new_v4().simple());
    let reg: Value = reqwest::Client::new()
        .post(format!("{base}/v1/machines"))
        .bearer_auth(&ada)
        .json(&json!({"id": id, "name": "design"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let mut join = cua_relay::client::JoinConfig::new(
        base.replace("http://", "ws://"),
        reg["machine_token"].as_str().unwrap().to_owned(),
        id.clone(),
        local_addr,
    );
    join.heartbeat = Duration::from_secs(2);
    let stop = CancellationToken::new();
    tokio::spawn(cua_relay::client::run(join, stop.clone()));
    for _ in 0..200 {
        if relay.machine_ids().contains(&id) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(relay.machine_ids().contains(&id), "computer joined");
    Rig {
        issuer,
        relay,
        base,
        api,
        seen,
        id,
        ada,
        stop,
        _state: state,
    }
}

impl Rig {
    /// A raw API call with the owner's token, as an old client would make:
    /// no gate or check call first.
    async fn patch(&self, body: Value) -> (StatusCode, Value) {
        let r = reqwest::Client::new()
            .patch(format!("{}/v1/machines/{}", self.base, self.id))
            .bearer_auth(&self.ada)
            .json(&body)
            .send()
            .await
            .unwrap();
        let s = r.status();
        (s, r.json().await.unwrap_or(Value::Null))
    }

    async fn open(&self, token: &str) -> StatusCode {
        reqwest::Client::new()
            .get(format!("{}/m/{}/probe", self.base, self.id))
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
            .status()
    }

    fn token(&self, sub: &str, email: &str) -> String {
        self.issuer.token(sub, Some(email), "cua-relay", 600)
    }

    /// The owner's own view of the computer: its `allow` and `viewers`.
    async fn lists(&self) -> (Vec<String>, Vec<String>) {
        let v: Value = reqwest::Client::new()
            .get(format!("{}/v1/machines/{}", self.base, self.id))
            .bearer_auth(&self.ada)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let list = |k: &str| {
            v[k].as_array()
                .map(|a| a.iter().map(|x| x.as_str().unwrap().to_owned()).collect())
                .unwrap_or_default()
        };
        (list("allow"), list("viewers"))
    }
}

async fn expire() {
    tokio::time::sleep(TTL + Duration::from_millis(500)).await;
}

#[tokio::test]
async fn an_old_client_cannot_share_outside_the_team() {
    let rig = rig(true).await;
    rig.api.lock().unwrap().team.insert("bob@team.test".into());

    // Raw PATCH with the owner's token, no gate call: an outsider is refused.
    let (s, v) = rig.patch(json!({"allow": ["eve@else.test"]})).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{v}");
    assert_eq!(v["code"], "share_outside_team", "{v}");
    assert!(
        v["error"].as_str().unwrap().contains("eve@else.test"),
        "{v}"
    );
    assert!(rig.lists().await.0.is_empty());
    let (s, v) = rig.patch(json!({"viewers": ["eve@else.test"]})).await;
    assert_eq!(
        (s, v["code"].as_str()),
        (StatusCode::FORBIDDEN, Some("share_outside_team"))
    );
    // One outsider among teammates refuses the whole change.
    let (s, _) = rig
        .patch(json!({"allow": ["bob@team.test", "eve@else.test"]}))
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert!(rig.lists().await.0.is_empty());

    // A teammate works, as editor and as viewer.
    let (s, v) = rig.patch(json!({"allow": ["bob@team.test"]})).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (s, _) = rig
        .patch(json!({"allow": [], "viewers": ["bob@team.test"]}))
        .await;
    assert_eq!(s, StatusCode::OK);

    // Taking someone off is never refused, outsider or not.
    let (s, _) = rig.patch(json!({"viewers": []})).await;
    assert_eq!(s, StatusCode::OK);

    // Renaming asks nothing.
    let calls = rig.api.lock().unwrap().check_calls;
    let (s, _) = rig.patch(json!({"name": "design 2"})).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(rig.api.lock().unwrap().check_calls, calls);
    rig.stop.cancel();
}

#[tokio::test]
async fn an_owner_without_a_team_gets_sharing_needs_team() {
    let rig = rig(true).await;
    rig.api.lock().unwrap().no_team = true;
    let (s, v) = rig.patch(json!({"allow": ["bob@team.test"]})).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{v}");
    assert_eq!(v["code"], "sharing_needs_team", "{v}");
    assert!(rig.lists().await.0.is_empty());
    rig.stop.cancel();
}

#[tokio::test]
async fn a_new_share_fails_closed_when_the_control_plane_is_down() {
    let rig = rig(true).await;
    rig.api.lock().unwrap().team.insert("bob@team.test".into());
    rig.api.lock().unwrap().down = true;
    let (s, v) = rig.patch(json!({"allow": ["bob@team.test"]})).await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE, "{v}");
    assert_eq!(v["code"], "share_check_unavailable");
    assert!(rig.lists().await.0.is_empty());
    rig.stop.cancel();
}

#[tokio::test]
async fn a_removed_outsider_is_refused_after_the_cache_expires() {
    let rig = rig(true).await;
    rig.api.lock().unwrap().team.insert("bob@team.test".into());
    let (s, _) = rig.patch(json!({"allow": ["bob@team.test"]})).await;
    assert_eq!(s, StatusCode::OK);
    let bob = rig.token("bob", "bob@team.test");

    assert_eq!(rig.open(&bob).await, StatusCode::OK);
    assert_eq!(role_of(rig.seen.lock().unwrap().last().unwrap()), "shared");
    // Inside the TTL, opening again costs no control-plane call.
    let calls = rig.api.lock().unwrap().check_calls;
    for _ in 0..5 {
        assert_eq!(rig.open(&bob).await, StatusCode::OK);
    }
    assert_eq!(rig.api.lock().unwrap().check_calls, calls);

    // Bob leaves the team. The stored share is still there (no sweep yet),
    // yet once the cached answer expires he is refused.
    rig.api.lock().unwrap().team.clear();
    assert!(rig.lists().await.0.contains(&"bob@team.test".to_owned()));
    expire().await;
    assert_eq!(rig.open(&bob).await, StatusCode::FORBIDDEN);
    // The refusal is cached too: no further call inside the TTL.
    let calls = rig.api.lock().unwrap().check_calls;
    assert_eq!(rig.open(&bob).await, StatusCode::FORBIDDEN);
    assert_eq!(rig.api.lock().unwrap().check_calls, calls);
    rig.stop.cancel();
}

#[tokio::test]
async fn a_cached_grant_does_not_outlive_its_ttl_in_an_outage() {
    let rig = rig(true).await;
    rig.api.lock().unwrap().team.insert("bob@team.test".into());
    rig.patch(json!({"allow": ["bob@team.test"]})).await;
    let bob = rig.token("bob", "bob@team.test");
    assert_eq!(rig.open(&bob).await, StatusCode::OK);

    rig.api.lock().unwrap().down = true;
    // Inside the TTL the cached yes still serves.
    assert_eq!(rig.open(&bob).await, StatusCode::OK);
    // Past it, the computer fails closed.
    expire().await;
    assert_eq!(rig.open(&bob).await, StatusCode::SERVICE_UNAVAILABLE);
    // A person never asked about fails closed as well; the owner is never asked.
    assert_eq!(rig.open(&rig.ada).await, StatusCode::OK);
    rig.api.lock().unwrap().down = false;
    assert_eq!(rig.open(&bob).await, StatusCode::OK);
    rig.stop.cancel();
}

#[tokio::test]
async fn simultaneous_opens_make_one_control_plane_call() {
    let rig = rig(true).await;
    rig.api.lock().unwrap().team.insert("bob@team.test".into());
    rig.patch(json!({"allow": ["bob@team.test"]})).await;
    let bob = rig.token("bob", "bob@team.test");
    expire().await;
    let before = rig.api.lock().unwrap().check_calls;
    let opens = (0..20).map(|_| rig.open(&bob));
    for s in futures_util::future::join_all(opens).await {
        assert_eq!(s, StatusCode::OK);
    }
    assert_eq!(rig.api.lock().unwrap().check_calls, before + 1);
    rig.stop.cancel();
}

#[tokio::test]
async fn a_team_share_as_viewer_cannot_send_input() {
    let rig = rig(true).await;
    {
        let mut a = rig.api.lock().unwrap();
        a.members.insert("ada".into());
        a.members.insert("bob".into());
        a.machine_role = "viewer".into();
    }
    rig.issuer
        .set_orgs("bob", json!({"aigis": {"id": "org-1", "groups": []}}));
    let bob = rig.token("bob", "bob@team.test");
    assert_eq!(rig.open(&bob).await, StatusCode::OK);
    assert_eq!(role_of(rig.seen.lock().unwrap().last().unwrap()), "viewer");

    // The team's role goes back to editor: after the cache expires, Bob
    // opens with the editor role.
    rig.api.lock().unwrap().machine_role = "editor".into();
    expire().await;
    assert_eq!(rig.open(&bob).await, StatusCode::OK);
    assert_eq!(role_of(rig.seen.lock().unwrap().last().unwrap()), "shared");
    rig.stop.cancel();
}

#[tokio::test]
async fn the_sweep_removes_outsiders_cuts_them_and_is_idempotent() {
    let rig = rig(true).await;
    {
        let mut a = rig.api.lock().unwrap();
        a.team.insert("bob@team.test".into());
        a.team.insert("sam@team.test".into());
    }
    rig.patch(json!({"allow": ["bob@team.test"], "viewers": ["sam@team.test"]}))
        .await;
    let (bob, sam) = (
        rig.token("bob", "bob@team.test"),
        rig.token("sam", "sam@team.test"),
    );
    assert_eq!(rig.open(&bob).await, StatusCode::OK);
    assert_eq!(rig.open(&sam).await, StatusCode::OK);

    // Both are outside the team now. With the API down nobody is removed.
    {
        let mut a = rig.api.lock().unwrap();
        a.team.clear();
        a.down = true;
    }
    assert!(cua_relay::team_sharing::sweep(&rig.relay).await.is_err());
    let (al, vi) = rig.lists().await;
    assert_eq!(al.len() + vi.len(), 2);

    rig.api.lock().unwrap().down = false;
    let r = cua_relay::team_sharing::sweep(&rig.relay).await.unwrap();
    assert_eq!((r.checked, r.removed), (1, 2));
    let (al, vi) = rig.lists().await;
    assert!(al.is_empty() && vi.is_empty(), "{al:?} {vi:?}");
    assert_eq!(rig.open(&bob).await, StatusCode::FORBIDDEN);
    assert_eq!(rig.open(&sam).await, StatusCode::FORBIDDEN);

    // Again: nothing listed, nothing sent, nothing removed.
    let calls = rig.api.lock().unwrap().sweep_calls;
    let r = cua_relay::team_sharing::sweep(&rig.relay).await.unwrap();
    assert_eq!((r.checked, r.removed), (0, 0));
    assert_eq!(rig.api.lock().unwrap().sweep_calls, calls);
    rig.stop.cancel();
}

#[tokio::test]
async fn the_sweep_keeps_teammates() {
    let rig = rig(true).await;
    {
        let mut a = rig.api.lock().unwrap();
        a.team.insert("bob@team.test".into());
        a.team.insert("eve@else.test".into());
    }
    rig.patch(json!({"allow": ["bob@team.test", "eve@else.test"]}))
        .await;
    rig.api.lock().unwrap().team.remove("eve@else.test");
    let r = cua_relay::team_sharing::sweep(&rig.relay).await.unwrap();
    assert_eq!(r.removed, 1);
    assert_eq!(rig.lists().await.0, vec!["bob@team.test".to_owned()]);
    rig.stop.cancel();
}

#[tokio::test]
async fn with_the_switch_off_nothing_changes() {
    let rig = rig(false).await;
    // No team, an outsider: shared as before, and no control-plane call.
    rig.api.lock().unwrap().no_team = true;
    let (s, v) = rig.patch(json!({"allow": ["eve@else.test"]})).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let eve = rig.token("eve", "eve@else.test");
    assert_eq!(rig.open(&eve).await, StatusCode::OK);
    assert_eq!(rig.api.lock().unwrap().check_calls, 0);
    rig.stop.cancel();
}
