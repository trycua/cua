// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The web viewer through the relay from a phone's web view: the page is
//! public, and its own calls carry the machine-minted viewer ticket in
//! `x-cua-env-authorization` (a web view cannot set an account bearer on
//! them). The relay checks the ticket's account against revocation and
//! forwards the ticket itself; everything else still needs the account.

use std::sync::Arc;
use std::time::Duration;

use cua_push::{MockProvider, PushHub, Registry};
use cua_relay::oidc::testing::FakeIssuer;
use cua_relay::oidc::{OidcConfig, OidcValidator};
use cua_relay::push_api::PushConfig;
use cua_relay::server::{Relay, RelayConfig};
use reqwest::StatusCode;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

const ISSUER: &str = "https://auth.test/realms/cua";

/// A spacesd-shaped viewer ticket (the fake machine accepts any signature;
/// the relay reads the claims and never checks it).
fn ticket(principal: &str) -> String {
    ticket_with(principal, "viewer", 600)
}

/// A ticket of `scope` valid for `secs` from now (negative: expired).
fn ticket_with(principal: &str, scope: &str, secs: i64) -> String {
    use base64::Engine as _;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let exp = (now + secs * 1000) as u64;
    let payload =
        json!({"s": scope, "r": "{}", "p": format!("viewer:{principal}"), "e": exp, "n": "abc"});
    let body = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(&payload).unwrap());
    format!("v1.{body}.c2ln")
}

struct Env {
    relay: Relay,
    base: String,
    issuer: FakeIssuer,
    http: reqwest::Client,
    stop: CancellationToken,
}

impl Env {
    /// A relay with phone support on (`CUA_RELAY_PUSH=on`).
    async fn start() -> Self {
        Self::start_with(true).await
    }

    /// A relay with phone support `phone` on or off. Off is the relay as it
    /// was before phones: the viewer rules below do not apply to it.
    async fn start_with(phone: bool) -> Self {
        let issuer = FakeIssuer::new(ISSUER);
        let hub = Arc::new(PushHub::new(
            Registry::in_memory(),
            Arc::new(MockProvider::new()),
        ));
        let relay = Relay::new(RelayConfig {
            oidc: Some(Arc::new(OidcValidator::with_jwks(
                OidcConfig::new(ISSUER),
                issuer.jwks(),
            ))),
            device_enrollment: false,
            push: phone.then(|| PushConfig::new(hub)),
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
        }
    }

    fn token(&self, user: &str) -> String {
        self.issuer
            .token(user, Some(&format!("{user}@example.com")), "cua-relay", 600)
    }

    /// A machine whose "spacesd" echoes the credential headers it received
    /// and serves a viewer page and a secret.
    async fn machine(&self, owner: &str) -> String {
        let app = axum::Router::new()
            .route(
                "/viewer/",
                axum::routing::get(|| async {
                    ([("content-type", "text/html")], "<html>viewer</html>")
                }),
            )
            .route(
                "/cua.env.v1.StreamService/RequestKeyframe",
                axum::routing::any(|| async { (axum::http::StatusCode::UNAUTHORIZED, "no") }),
            )
            .route(
                "/cua.env.v1.SystemService/GetCapabilities",
                axum::routing::any(|h: axum::http::HeaderMap| async move {
                    let get = |n: &str| {
                        h.get(n)
                            .and_then(|v| v.to_str().ok())
                            .unwrap_or("")
                            .to_string()
                    };
                    axum::Json(json!({
                        "env_authorization": get("x-cua-env-authorization"),
                        "authorization": get("authorization"),
                        "has_assertion": h.contains_key("x-cua-relay-assertion"),
                    }))
                }),
            )
            .fallback(|| async { "secret files" });
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
            .json(&json!({"id": id, "name": "a space"}))
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
}

/// A call a viewer ticket may make (the relay forwards only those), which the
/// test machine answers with the credentials it received.
const ECHO: &str = "cua.env.v1.SystemService/GetCapabilities";

/// A raw HTTP/1.1 request, so a path reaches the relay exactly as written
/// (an HTTP client normalizes `..` and encodings first).
async fn raw(base: &str, method: &str, path: &str, headers: &str) -> (u16, String, String) {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let addr = base.trim_start_matches("http://");
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    let req = format!("{method} {path} HTTP/1.1\r\nhost: {addr}\r\nconnection: close\r\n{headers}content-length: 0\r\n\r\n");
    s.write_all(req.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    s.read_to_end(&mut out).await.unwrap();
    let text = String::from_utf8_lossy(&out).to_string();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    (status, head.to_string(), body.to_string())
}

#[tokio::test]
async fn the_viewer_page_is_the_relays_own_and_public_but_nothing_else_is() {
    let env = Env::start().await;
    let id = env.machine("ada").await;
    let page = env
        .http
        .get(format!("{}/m/{id}/viewer/", env.base))
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    let body = page.text().await.unwrap();
    assert!(
        body.contains("<html") && !body.contains("viewer</html>"),
        "the relay's viewer, not the machine's page"
    );
    // A page the machine would serve differently comes from the relay anyway.
    let r = env
        .http
        .get(format!("{}/m/{id}/viewer/viewer.js", env.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    // Everything else still needs the account.
    for (method, path) in [
        ("GET", "files?path=/etc/passwd"),
        ("GET", "echo"),
        ("POST", "viewer/"),
        ("PUT", "viewer/x"),
        ("DELETE", "viewer/x"),
        ("GET", "viewerx/"),
        ("GET", "viewerx"),
    ] {
        let (status, _, _) = raw(&env.base, method, &format!("/m/{id}/{path}"), "").await;
        assert_eq!(status, 401, "{method} {path}");
    }
}

#[tokio::test]
async fn paths_are_decoded_once_and_an_allowlist_decides() {
    let env = Env::start().await;
    let id = env.machine("ada").await;
    for path in [
        "viewer/../files",
        "viewer/./x",
        "viewer/%2e%2e/files",
        "viewer/%2E%2E/files",
        "viewer/%252e%252e/files",
        "viewer/%252f",
        "viewer//x",
        "viewer/%2f%2fx",
        "viewer/a%5cb",
        "viewer/%00",
        "viewer/%0d%0aX-Evil:1",
        "viewer/a;b",
    ] {
        let (status, _, body) = raw(&env.base, "GET", &format!("/m/{id}/{path}"), "").await;
        assert_eq!(status, 401, "{path}: {body}");
    }
    let (status, _, _) = raw(&env.base, "GET", &format!("/m/{id}/viewer/%76iewer.js"), "").await;
    assert_eq!(status, 200, "one level of encoding is decoded");
}

#[tokio::test]
async fn unknown_offline_and_other_peoples_machines_get_the_same_answer() {
    let env = Env::start().await;
    let theirs = env.machine("bob").await;
    // Registered, never joined: offline.
    let reg: Value = env
        .http
        .post(format!("{}/v1/machines", env.base))
        .bearer_auth(env.token("ada"))
        .json(&json!({"id": "offline-machine-1", "name": "off"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(reg.get("machine_token").is_some());
    let mut seen = Vec::new();
    for id in [theirs.as_str(), "offline-machine-1", "no-such-machine-9"] {
        for path in ["viewer/", "viewer/viewer.js", "viewer/missing.js", "viewer"] {
            let (status, head, body) = raw(&env.base, "GET", &format!("/m/{id}/{path}"), "").await;
            // Everything but the date, which is the same for the three ids.
            let head: Vec<String> = head
                .lines()
                .filter(|l| !l.to_ascii_lowercase().starts_with("date:"))
                .map(str::to_owned)
                .collect();
            seen.push((path, status, head, body));
        }
    }
    for chunk in seen.chunks(4).skip(1) {
        assert_eq!(
            chunk,
            &seen[..4],
            "the answer must not depend on the machine id"
        );
    }
    // The preflight of the viewer's own calls is the same too, and says how.
    let mut pre = Vec::new();
    for id in [theirs.as_str(), "offline-machine-1", "no-such-machine-9"] {
        let (status, head, _) = raw(
            &env.base,
            "OPTIONS",
            &format!("/m/{id}/cua.env.v1.ComputerService/GetScreenSize"),
            "origin: null\r\naccess-control-request-method: POST\r\naccess-control-request-headers: x-cua-env-authorization\r\n",
        )
        .await;
        let head: Vec<String> = head
            .lines()
            .filter(|l| !l.to_ascii_lowercase().starts_with("date:"))
            .map(str::to_owned)
            .collect();
        assert_eq!(status, 204);
        pre.push(head);
    }
    assert!(pre.windows(2).all(|w| w[0] == w[1]));
    let joined = pre[0].join("\n").to_ascii_lowercase();
    assert!(
        joined.contains("access-control-allow-origin: *")
            && joined.contains("x-cua-env-authorization"),
        "{joined}"
    );
}

#[tokio::test]
async fn viewer_responses_are_sandboxed_and_not_sniffed() {
    let env = Env::start().await;
    let id = env.machine("ada").await;
    for path in [
        "viewer/",
        "viewer/viewer.js",
        "viewer/index.css",
        "viewer/missing",
    ] {
        let (_, head, _) = raw(&env.base, "GET", &format!("/m/{id}/{path}"), "").await;
        let low = head.to_ascii_lowercase();
        assert!(
            low.contains("x-content-type-options: nosniff"),
            "{path}: {head}"
        );
        assert!(
            low.contains("referrer-policy: no-referrer"),
            "{path}: {head}"
        );
        if path != "viewer/missing" {
            let csp = low
                .lines()
                .find(|l| l.starts_with("content-security-policy:"))
                .expect("a policy");
            assert!(csp.ends_with("; sandbox allow-scripts"), "{path}: {csp}");
            assert!(!csp.contains("allow-same-origin"));
        }
    }
    let (_, head, _) = raw(&env.base, "GET", &format!("/m/{id}/viewer/"), "").await;
    assert!(
        head.to_ascii_lowercase()
            .contains("cache-control: no-store"),
        "{head}"
    );
}

#[tokio::test]
async fn the_unauthenticated_viewer_path_is_rate_limited_per_address() {
    let env = Env::start().await;
    let id = env.machine("ada").await;
    let mut limited = None;
    for i in 0..((cua_relay::viewer::BURST as usize) + 40) {
        let (status, head, _) = raw(&env.base, "GET", &format!("/m/{id}/viewer/"), "").await;
        if status == 429 {
            limited = Some((i, head));
            break;
        }
    }
    let (i, head) = limited.expect("the flood was limited");
    assert!(
        i >= cua_relay::viewer::BURST as usize - 5,
        "not before the burst: {i}"
    );
    assert!(head.to_ascii_lowercase().contains("retry-after:"), "{head}");
}

#[tokio::test]
async fn a_ticket_in_the_header_is_forwarded_only_when_live_viewer_and_on_a_viewer_call() {
    let env = Env::start().await;
    let id = env.machine("ada").await;
    let url = format!("{}/m/{id}/{ECHO}", env.base);
    let call = |t: String, method: reqwest::Method, path: &str| {
        let url = format!("{}/m/{id}/{path}", env.base);
        let http = env.http.clone();
        async move {
            http.request(method, url)
                .header("x-cua-env-authorization", format!("Bearer {t}"))
                .send()
                .await
                .unwrap()
                .status()
        }
    };
    let good = ticket("ada");
    let _ = url;
    assert_eq!(
        call(good.clone(), reqwest::Method::POST, ECHO).await,
        StatusCode::OK
    );
    // Expired, of another scope, not a ticket, or too long: refused at the relay.
    assert_eq!(
        call(
            ticket_with("ada", "viewer", -5),
            reqwest::Method::POST,
            ECHO
        )
        .await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            ticket_with("ada", "media", 600),
            reqwest::Method::POST,
            ECHO
        )
        .await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call("v1.x.y".into(), reqwest::Method::POST, ECHO).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            format!("v1.{}.sig", "a".repeat(3000)),
            reqwest::Method::POST,
            ECHO
        )
        .await,
        StatusCode::UNAUTHORIZED
    );
    // Not a call the viewer makes: another method on a gRPC path, another path:
    // a live ticket on a call it does not cover is told so, the same for any computer.
    assert_eq!(
        call(good.clone(), reqwest::Method::GET, ECHO).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(good.clone(), reqwest::Method::POST, "echo").await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(good.clone(), reqwest::Method::POST, "viewer/").await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(good, reqwest::Method::DELETE, "v1/machines").await,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn an_address_whose_tickets_machines_keep_refusing_is_stopped() {
    let env = Env::start().await;
    let id = env.machine("ada").await;
    let t = ticket("ada");
    let mut stopped = None;
    for i in 0..(cua_relay::viewer::TICKET_FAILURES as usize + 10) {
        let r = env
            .http
            .post(format!(
                "{}/m/{id}/cua.env.v1.StreamService/RequestKeyframe",
                env.base
            ))
            .header("x-cua-env-authorization", format!("Bearer {t}"))
            .send()
            .await
            .unwrap();
        if r.status() == StatusCode::TOO_MANY_REQUESTS {
            stopped = Some(i);
            break;
        }
    }
    let i = stopped.expect("the relay stopped forwarding");
    assert!(i >= cua_relay::viewer::TICKET_FAILURES as usize, "{i}");
    // A good call from the same address is held too until the window ends,
    // and an account's own bearer is not affected.
    let r = env
        .http
        .post(format!("{}/m/{id}/{ECHO}", env.base))
        .bearer_auth(env.token("ada"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
}

#[tokio::test]
async fn a_viewer_ticket_in_the_header_is_forwarded_and_other_bearers_are_not() {
    let env = Env::start().await;
    let id = env.machine("ada").await;
    let t = ticket("ada");
    let r: Value = env
        .http
        .post(format!("{}/m/{id}/{ECHO}", env.base))
        .header("x-cua-env-authorization", format!("Bearer {t}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["env_authorization"], format!("Bearer {t}"));
    assert_eq!(r["has_assertion"], false);
    let r = env
        .http
        .post(format!("{}/m/{id}/{ECHO}", env.base))
        .header("x-cua-env-authorization", "Bearer some-static-env-token")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    // With the account's own bearer the relay's assertion replaces the header.
    let r: Value = env
        .http
        .post(format!("{}/m/{id}/{ECHO}", env.base))
        .bearer_auth(env.token("ada"))
        .header("x-cua-env-authorization", format!("Bearer {t}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["env_authorization"], "");
    assert_eq!(r["authorization"], "");
    assert_eq!(r["has_assertion"], true);
}

#[tokio::test]
async fn a_ticket_of_an_account_that_lost_access_stops_working_at_once() {
    let env = Env::start().await;
    let id = env.machine("ada").await;
    let share = env
        .http
        .patch(format!("{}/v1/machines/{id}", env.base))
        .bearer_auth(env.token("ada"))
        .json(&json!({"viewers": ["bob@example.com"]}))
        .send()
        .await
        .unwrap();
    assert!(share.status().is_success(), "{}", share.status());
    let open = env
        .http
        .post(format!("{}/m/{id}/{ECHO}", env.base))
        .bearer_auth(env.token("bob"))
        .send()
        .await
        .unwrap();
    assert_eq!(open.status(), StatusCode::OK);
    let t = ticket("bob");
    let call = |t: String| {
        let http = env.http.clone();
        let url = format!("{}/m/{id}/{ECHO}", env.base);
        async move {
            http.post(url)
                .header("x-cua-env-authorization", format!("Bearer {t}"))
                .send()
                .await
                .unwrap()
                .status()
        }
    };
    assert_eq!(call(t.clone()).await, StatusCode::OK);
    let unshare = env
        .http
        .patch(format!("{}/v1/machines/{id}", env.base))
        .bearer_auth(env.token("ada"))
        .json(&json!({"viewers": []}))
        .send()
        .await
        .unwrap();
    assert!(unshare.status().is_success());
    assert_eq!(call(t).await, StatusCode::FORBIDDEN);
}

/// A relay without `CUA_RELAY_PUSH=on` keeps the viewer behavior it had:
/// the machine's own page answers an anonymous viewer request, and any
/// ticket the machine could read is forwarded, expired or not.
#[tokio::test]
async fn without_phone_support_the_viewer_behaves_as_before() {
    let env = Env::start_with(false).await;
    let id = env.machine("ada").await;
    let page = env
        .http
        .get(format!("{}/m/{id}/viewer/", env.base))
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    assert!(
        page.text().await.unwrap().contains("viewer</html>"),
        "the machine's page, not the relay's"
    );
    let expired = ticket_with("ada", "viewer", -60);
    let echo: Value = env
        .http
        .post(format!("{}/m/{id}/{ECHO}", env.base))
        .header("x-cua-env-authorization", format!("Bearer {expired}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(echo["env_authorization"], format!("Bearer {expired}"));
}
