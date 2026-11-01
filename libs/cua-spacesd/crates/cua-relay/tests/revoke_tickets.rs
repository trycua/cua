// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Revoking one account cuts that account only, and its machine-minted
//! tickets stop working at once (CUA-1416, CUA-1417): for a direct share,
//! a Space removed from a team, and a member removed from a team.

use std::sync::Arc;
use std::time::Duration;

use cua_relay::oidc::testing::FakeIssuer;
use cua_relay::oidc::{OidcConfig, OidcValidator};
use cua_relay::server::{Relay, RelayConfig};
use futures_util::{SinkExt as _, StreamExt as _};
use reqwest::StatusCode;
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

const ISSUER: &str = "https://auth.test/realms/cua";

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// A spacesd-shaped ticket for `principal` (the fake machine accepts any
/// signature; the relay reads the claims and never checks it).
fn ticket(principal: &str, issued: Option<u64>) -> String {
    use base64::Engine as _;
    let exp = (std::time::SystemTime::now() + Duration::from_secs(600))
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let mut payload = json!({"s": "media", "r": "sess", "p": principal, "e": exp, "n": "abc"});
    if let Some(a) = issued {
        payload["a"] = a.into();
    }
    let body = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(&payload).unwrap());
    format!("v1.{body}.c2ln")
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

struct Env {
    relay: Relay,
    base: String,
    issuer: FakeIssuer,
    http: reqwest::Client,
    stop: CancellationToken,
    _state: tempfile::TempDir,
}

impl Env {
    async fn start() -> Self {
        let issuer = FakeIssuer::new(ISSUER);
        let state = tempfile::tempdir().unwrap();
        let relay = Relay::new(RelayConfig {
            oidc: Some(Arc::new(OidcValidator::with_jwks(
                OidcConfig::new(ISSUER),
                issuer.jwks(),
            ))),
            device_enrollment: false,
            state_file: Some(state.path().join("machines.json")),
            ..RelayConfig::default()
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let serving = relay.clone();
        tokio::spawn(async move {
            let _ = serving.serve(listener, std::future::pending()).await;
        });
        Self {
            relay,
            base,
            issuer,
            http: reqwest::Client::new(),
            stop: CancellationToken::new(),
            _state: state,
        }
    }

    fn token(&self, user: &str) -> String {
        self.issuer
            .token(user, Some(&format!("{user}@example.com")), "cua-relay", 600)
    }

    /// Registers and joins a machine owned by `owner` whose `/media` is an
    /// echo WebSocket (standing in for spacesd's media socket).
    async fn machine(&self, owner: &str) -> String {
        let app = axum::Router::new()
            .route(
                "/media",
                axum::routing::get(|ws: axum::extract::ws::WebSocketUpgrade| async move {
                    ws.on_upgrade(|mut socket| async move {
                        while let Some(Ok(m)) = socket.recv().await {
                            if socket.send(m).await.is_err() {
                                break;
                            }
                        }
                    })
                }),
            )
            .fallback(|| async { "ok" });
        let local = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = local.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(local, app).await;
        });
        let id = format!("space-{}", &uuid::Uuid::new_v4().simple().to_string()[..16]);
        let reg: Value = self
            .http
            .post(format!("{}/v1/machines", self.base))
            .bearer_auth(self.token(owner))
            .json(&json!({"id": id, "name": "shared space"}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let mut join = cua_relay::client::JoinConfig::new(
            self.base.replace("http://", "ws://"),
            reg["machine_token"].as_str().unwrap().to_owned(),
            id.clone(),
            local_addr,
        );
        join.heartbeat = Duration::from_secs(2);
        tokio::spawn(cua_relay::client::run(join, self.stop.clone()));
        for _ in 0..200 {
            if self.relay.machine_ids().contains(&id) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(self.relay.machine_ids().contains(&id), "machine joined");
        id
    }

    async fn send(
        &self,
        method: reqwest::Method,
        url: &str,
        user: &str,
        body: Value,
    ) -> (StatusCode, Value) {
        let r = self
            .http
            .request(method, format!("{}{url}", self.base))
            .bearer_auth(self.token(user))
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = r.status();
        let text = r.text().await.unwrap_or_default();
        (
            status,
            serde_json::from_str(&text).unwrap_or(Value::String(text)),
        )
    }

    /// An account call to the machine (what mints a ticket on spacesd).
    async fn call(&self, id: &str, user: &str) -> StatusCode {
        self.http
            .get(format!("{}/m/{id}/probe", self.base))
            .bearer_auth(self.token(user))
            .send()
            .await
            .unwrap()
            .status()
    }

    /// Opens the media WebSocket with a ticket; `Err(status)` if refused.
    async fn media(&self, id: &str, ticket: &str) -> Result<Ws, u16> {
        let url = format!(
            "{}/m/{id}/media?ticket={ticket}",
            self.base.replace("http://", "ws://")
        );
        match tokio_tungstenite::connect_async(url).await {
            Ok((ws, _)) => Ok(ws),
            Err(tokio_tungstenite::tungstenite::Error::Http(r)) => Err(r.status().as_u16()),
            Err(e) => panic!("media connect: {e}"),
        }
    }
}

/// Whether the socket still carries frames both ways.
async fn alive(ws: &mut Ws) -> bool {
    if ws.send(Message::Text("ping".into())).await.is_err() {
        return false;
    }
    matches!(
        tokio::time::timeout(Duration::from_secs(2), ws.next()).await,
        Ok(Some(Ok(Message::Text(_))))
    )
}

/// Whether the socket is closed within `within`.
async fn closed(ws: &mut Ws, within: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        let _ = ws.send(Message::Text("ping".into())).await;
        match tokio::time::timeout_at(deadline, ws.next()).await {
            Err(_) => return false,
            Ok(None) | Ok(Some(Err(_))) | Ok(Some(Ok(Message::Close(_)))) => return true,
            Ok(Some(Ok(_))) => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    }
}

#[tokio::test]
async fn revoking_a_direct_share_cuts_only_that_guest_and_its_tickets() {
    let env = Env::start().await;
    let id = env.machine("ada").await;
    let url = format!("/v1/machines/{id}");
    // Bob edits, Carol watches.
    let (s, _) = env
        .send(
            reqwest::Method::PATCH,
            &url,
            "ada",
            json!({"allow": ["bob@example.com"], "viewers": ["carol@example.com"]}),
        )
        .await;
    assert!(s.is_success(), "{s}");
    for user in ["ada", "bob", "carol"] {
        assert_eq!(env.call(&id, user).await, StatusCode::OK, "{user}");
    }
    // Everyone live; Bob also holds a second ticket he hasn't used yet.
    let mut ada_ws = env.media(&id, &ticket("ada", None)).await.expect("ada");
    let mut carol_ws = env
        .media(&id, &ticket("viewer:carol", None))
        .await
        .expect("carol");
    let mut bob_ws = env.media(&id, &ticket("bob", None)).await.expect("bob");
    let held = ticket("bob", None);
    let held_with_mint_time = ticket("bob", Some(now_secs() - 1));
    assert!(alive(&mut bob_ws).await);

    // Revoke Bob (the editor).
    let (s, _) = env
        .send(reqwest::Method::PATCH, &url, "ada", json!({"allow": []}))
        .await;
    assert!(s.is_success());

    // CUA-1416: his live session ends and his old tickets are refused,
    // so no input reaches the machine through them either.
    assert!(closed(&mut bob_ws, Duration::from_secs(3)).await, "bob cut");
    assert_eq!(env.media(&id, &held).await.err(), Some(403), "held ticket");
    assert_eq!(env.media(&id, &held_with_mint_time).await.err(), Some(403));
    assert_eq!(env.call(&id, "bob").await, StatusCode::FORBIDDEN);
    // CUA-1417: the owner and the other guest stay live.
    assert!(alive(&mut ada_ws).await, "owner's session survives");
    assert!(alive(&mut carol_ws).await, "other guest's session survives");
    // The owner and Carol can still open new sessions.
    assert!(env.media(&id, &ticket("ada", None)).await.is_ok());
    assert!(env.media(&id, &ticket("viewer:carol", None)).await.is_ok());

    // Shared again, as a viewer: an editor-era ticket from before the
    // revoke stays refused even after he is authorized again...
    env.send(
        reqwest::Method::PATCH,
        &url,
        "ada",
        json!({"viewers": ["carol@example.com", "bob@example.com"]}),
    )
    .await;
    assert_eq!(env.call(&id, "bob").await, StatusCode::OK);
    assert_eq!(env.media(&id, &held).await.err(), Some(403));
    // ...while a ticket minted after the re-share works.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert_eq!(env.call(&id, "bob").await, StatusCode::OK);
    assert!(env
        .media(&id, &ticket("bob", Some(now_secs())))
        .await
        .is_ok());
    env.stop.cancel();
}

#[tokio::test]
async fn an_editor_turned_viewer_loses_its_editor_sessions_only() {
    let env = Env::start().await;
    let id = env.machine("ada").await;
    let url = format!("/v1/machines/{id}");
    env.send(
        reqwest::Method::PATCH,
        &url,
        "ada",
        json!({"allow": ["bob@example.com"]}),
    )
    .await;
    assert_eq!(env.call(&id, "ada").await, StatusCode::OK);
    assert_eq!(env.call(&id, "bob").await, StatusCode::OK);
    let mut ada_ws = env.media(&id, &ticket("ada", None)).await.unwrap();
    let mut bob_ws = env.media(&id, &ticket("bob", None)).await.unwrap();
    let editor_ticket = ticket("bob", None);
    env.send(
        reqwest::Method::PATCH,
        &url,
        "ada",
        json!({"allow": [], "viewers": ["bob@example.com"]}),
    )
    .await;
    assert!(closed(&mut bob_ws, Duration::from_secs(3)).await);
    assert!(alive(&mut ada_ws).await);
    assert_eq!(env.call(&id, "bob").await, StatusCode::OK, "still a viewer");
    assert_eq!(env.media(&id, &editor_ticket).await.err(), Some(403));
    env.stop.cancel();
}

// Teams are an optional hosted-relay feature (Cargo feature `hosted`).
#[cfg(feature = "hosted")]
#[tokio::test]
async fn team_revokes_cut_only_the_member_and_its_tickets() {
    let env = Env::start().await;
    let id = env.machine("ada").await;
    let (s, team) = env
        .send(
            reqwest::Method::POST,
            "/v1/teams",
            "ada",
            json!({"name": "T"}),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{team}");
    let team = team["id"].as_str().unwrap().to_owned();
    let (s, v) = env
        .send(
            reqwest::Method::POST,
            &format!("/v1/teams/{team}/machines"),
            "ada",
            json!({"machine": id, "os": "linux"}),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (s, inv) = env
        .send(
            reqwest::Method::POST,
            &format!("/v1/teams/{team}/invites"),
            "ada",
            json!({}),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{inv}");
    let (s, v) = env
        .send(
            reqwest::Method::POST,
            "/v1/teams/join",
            "bob",
            json!({"code": inv["link"]}),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");

    // 1. The Space is removed from the team.
    assert_eq!(env.call(&id, "ada").await, StatusCode::OK);
    assert_eq!(env.call(&id, "bob").await, StatusCode::OK);
    let mut ada_ws = env.media(&id, &ticket("ada", None)).await.unwrap();
    let mut bob_ws = env.media(&id, &ticket("bob", None)).await.unwrap();
    let held = ticket("bob", None);
    let (s, _) = env
        .send(
            reqwest::Method::DELETE,
            &format!("/v1/teams/{team}/machines/{id}"),
            "ada",
            json!({}),
        )
        .await;
    assert!(s.is_success(), "{s}");
    assert!(
        closed(&mut bob_ws, Duration::from_secs(3)).await,
        "member cut"
    );
    assert!(alive(&mut ada_ws).await, "owner kept");
    assert_eq!(env.media(&id, &held).await.err(), Some(403));
    assert_eq!(env.call(&id, "bob").await, StatusCode::FORBIDDEN);

    // 2. Back in the team, then the member is removed.
    let (s, _) = env
        .send(
            reqwest::Method::POST,
            &format!("/v1/teams/{team}/machines"),
            "ada",
            json!({"machine": id, "os": "linux"}),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert_eq!(env.call(&id, "bob").await, StatusCode::OK);
    let mut bob_ws = env
        .media(&id, &ticket("bob", None))
        .await
        .expect("member back in");
    let held = ticket("bob", None);
    let (s, v) = env
        .send(
            reqwest::Method::DELETE,
            &format!("/v1/teams/{team}/members/bob"),
            "ada",
            json!({}),
        )
        .await;
    assert!(s.is_success(), "{s} {v}");
    assert!(
        closed(&mut bob_ws, Duration::from_secs(3)).await,
        "member cut"
    );
    assert!(alive(&mut ada_ws).await, "owner kept");
    assert_eq!(env.media(&id, &held).await.err(), Some(403));
    assert_eq!(env.call(&id, "bob").await, StatusCode::FORBIDDEN);
    env.stop.cancel();
}
