// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `cua share` links through a real relay and a machine joined to it:
//! share origins, public links (anonymous, rate limited, revocable), the
//! browser sign-in (a mock OIDC provider with PKCE) and what each origin may
//! and may not do.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Form, State};
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use cua_relay::oidc::testing::FakeIssuer;
use cua_relay::oidc::{OidcConfig, OidcValidator};
use cua_relay::server::{Relay, RelayConfig};
use cua_relay::share_web::{ShareLimits, ShareWebConfig};
use reqwest::{header, StatusCode};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

const DOMAIN: &str = "relay.test";
const SID: &str = "0123456789abcdef";
const TOKEN: &str = "0123456789abcdef0123456789abcdef01234567";

/// A mock identity provider: discovery, a token endpoint that checks PKCE,
/// and `grant` to stand in for the user signing in at its login page.
struct Idp {
    base: String,
    issuer: FakeIssuer,
    codes: Mutex<HashMap<String, (String, String)>>,
}

impl Idp {
    fn grant(&self, user: &str, challenge: &str) -> String {
        let code = format!("code-{}", uuid::Uuid::new_v4().simple());
        self.codes
            .lock()
            .unwrap()
            .insert(code.clone(), (challenge.to_owned(), user.to_owned()));
        code
    }
}

async fn idp() -> Arc<Idp> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/realms/cua", listener.local_addr().unwrap());
    let idp = Arc::new(Idp {
        base: base.clone(),
        issuer: FakeIssuer::new(&base),
        codes: Mutex::default(),
    });
    let doc = json!({
        "issuer": base,
        "authorization_endpoint": format!("{base}/auth"),
        "token_endpoint": format!("{base}/token"),
    });
    async fn token(
        State(idp): State<Arc<Idp>>,
        Form(form): Form<HashMap<String, String>>,
    ) -> axum::response::Response {
        use base64::Engine as _;
        let Some((challenge, user)) = form
            .get("code")
            .and_then(|c| idp.codes.lock().unwrap().remove(c))
        else {
            return (StatusCode::BAD_REQUEST, "bad code").into_response();
        };
        let verifier = form.get("code_verifier").cloned().unwrap_or_default();
        let digest = ring::digest::digest(&ring::digest::SHA256, verifier.as_bytes());
        let computed = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest.as_ref());
        if computed != challenge || form.get("client_id").map(String::as_str) != Some("share-web") {
            return (StatusCode::BAD_REQUEST, "pkce").into_response();
        }
        let access = idp.issuer.token(
            &user,
            Some(&format!("{user}@example.com")),
            "cua-relay",
            600,
        );
        axum::Json(json!({"access_token": access, "token_type": "Bearer"})).into_response()
    }
    let app = axum::Router::new()
        .route(
            "/realms/cua/.well-known/openid-configuration",
            get(move || {
                let doc = doc.clone();
                async move { axum::Json(doc) }
            }),
        )
        .route("/realms/cua/token", post(token))
        .with_state(idp.clone());
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    idp
}

/// The machine's app: reports what it was asked, and serves a page that
/// tries to set cookies beyond its origin.
async fn app() -> std::net::SocketAddr {
    async fn echo(uri: axum::http::Uri, headers: HeaderMap) -> axum::Json<Value> {
        let h: HashMap<String, String> = headers
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_owned()))
            .collect();
        axum::Json(json!({"uri": uri.to_string(), "headers": h}))
    }
    async fn html() -> impl IntoResponse {
        let mut r = axum::response::Response::new(axum::body::Body::from("<h1>hi</h1>"));
        let h = r.headers_mut();
        h.insert(
            header::CONTENT_TYPE,
            "text/html; charset=utf-8".parse().unwrap(),
        );
        h.append(
            header::SET_COOKIE,
            "a=1; Domain=relay.test; Path=/".parse().unwrap(),
        );
        h.append(
            header::SET_COOKIE,
            "cua_share_session=evil; Path=/".parse().unwrap(),
        );
        r
    }
    let app = axum::Router::new()
        .route("/share/{sid}/html", get(html))
        .route("/share/pub/{token}/html", get(html))
        .fallback(echo);
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(l, app).await;
    });
    addr
}

struct Opts {
    host_mode: bool,
    web: bool,
    limits: ShareLimits,
    trusted_proxies: &'static str,
}

impl Default for Opts {
    fn default() -> Self {
        Self {
            host_mode: true,
            web: true,
            limits: ShareLimits::default(),
            trusted_proxies: "",
        }
    }
}

struct Env {
    relay: Relay,
    base: String,
    idp: Arc<Idp>,
    http: reqwest::Client,
    machine: String,
    stop: CancellationToken,
    _state: tempfile::TempDir,
}

impl Drop for Env {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

impl Env {
    async fn start(o: Opts) -> Self {
        let idp = idp().await;
        let state = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let relay = Relay::new(RelayConfig {
            oidc: Some(Arc::new(OidcValidator::with_jwks(
                OidcConfig::new(idp.base.clone()),
                idp.issuer.jwks(),
            ))),
            device_enrollment: false,
            state_file: Some(state.path().join("machines.json")),
            host_domain: o.host_mode.then(|| DOMAIN.to_owned()),
            public_url: Some(base.clone()),
            share_web: o.web.then(|| ShareWebConfig {
                client_id: "share-web".into(),
                client_secret: None,
            }),
            share_limits: o.limits,
            trusted_proxies: cua_relay::proxies::TrustedProxies::parse(o.trusted_proxies).unwrap(),
            ..RelayConfig::default()
        });
        let serving = relay.clone();
        tokio::spawn(async move {
            let _ = serving.serve(listener, std::future::pending()).await;
        });
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let stop = CancellationToken::new();
        let local = app().await;
        let machine = uuid::Uuid::new_v4().simple().to_string();
        let reg: Value = http
            .post(format!("{base}/v1/machines"))
            .bearer_auth(
                idp.issuer
                    .token("ada", Some("ada@example.com"), "cua-relay", 600),
            )
            .json(&json!({"id": machine, "name": "ada laptop"}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let mut join = cua_relay::client::JoinConfig::new(
            base.replace("http://", "ws://"),
            reg["machine_token"].as_str().unwrap().to_owned(),
            machine.clone(),
            local,
        );
        join.heartbeat = Duration::from_secs(2);
        tokio::spawn(cua_relay::client::run(join, stop.clone()));
        for _ in 0..200 {
            if relay.machine_ids().contains(&machine) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(relay.machine_ids().contains(&machine), "machine joined");
        Self {
            relay,
            base,
            idp,
            http,
            machine,
            stop,
            _state: state,
        }
    }

    fn token(&self, user: &str) -> String {
        self.idp
            .issuer
            .token(user, Some(&format!("{user}@example.com")), "cua-relay", 600)
    }

    fn private_host(&self, sid: &str) -> String {
        format!("s{sid}-{}.{DOMAIN}", &self.machine[..12])
    }

    fn public_host(&self, token: &str) -> String {
        format!("p{token}-{}.{DOMAIN}", &self.machine[..16])
    }

    fn get(&self, host: &str, path: &str) -> reqwest::RequestBuilder {
        self.http
            .get(format!("{}{path}", self.base))
            .header(header::HOST, host)
    }

    /// Signs a browser in on the private share origin `host`; returns the
    /// `cua_share_session=…` cookie pair.
    async fn sign_in(&self, host: &str, user: &str) -> String {
        let code = self.login_code(host, user, "/start?x=1").await;
        let r = self
            .get(host, &format!("/_cua/session?{code}"))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::FOUND, "{:?}", r.text().await);
        let set = r.headers()[header::SET_COOKIE].to_str().unwrap().to_owned();
        for flag in ["HttpOnly", "SameSite=Lax", "Path=/", "Max-Age=43200"] {
            assert!(set.contains(flag), "{flag} in {set}");
        }
        assert!(!set.contains("Domain"), "host-only cookie: {set}");
        set.split(';').next().unwrap().to_owned()
    }

    /// Walks the browser sign-in up to the one-time code; returns the
    /// query string of `/_cua/session` (`code=…&to=…`).
    async fn login_code(&self, host: &str, user: &str, path: &str) -> String {
        let r = self
            .get(host, path)
            .header(header::ACCEPT, "text/html")
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::FOUND);
        let login = r.headers()[header::LOCATION].to_str().unwrap().to_owned();
        assert!(
            login.starts_with(&format!("{}/v1/web/login?", self.base)),
            "{login}"
        );
        let r = self.http.get(&login).send().await.unwrap();
        assert_eq!(r.status(), StatusCode::FOUND);
        let auth = url::Url::parse(r.headers()[header::LOCATION].to_str().unwrap()).unwrap();
        assert!(auth
            .as_str()
            .starts_with(&format!("{}/auth", self.idp.base)));
        let q: HashMap<_, _> = auth.query_pairs().into_owned().collect();
        assert_eq!(q["response_type"], "code");
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["client_id"], "share-web");
        assert_eq!(q["redirect_uri"], format!("{}/v1/web/callback", self.base));
        let code = self.idp.grant(user, &q["code_challenge"]);
        let r = self
            .http
            .get(&q["redirect_uri"])
            .query(&[("code", code.as_str()), ("state", q["state"].as_str())])
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::FOUND, "{:?}", r.text().await);
        let back = url::Url::parse(r.headers()[header::LOCATION].to_str().unwrap()).unwrap();
        assert_eq!(back.host_str(), Some(host));
        assert_eq!(back.path(), "/_cua/session");
        back.query().unwrap().to_owned()
    }
}

fn claims_of(assertion: &str) -> Value {
    use base64::Engine as _;
    let body = assertion.split('.').nth(1).unwrap();
    serde_json::from_slice(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(body)
            .unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn a_browser_signs_in_once_and_reaches_the_app_on_its_own_origin() {
    let env = Env::start(Opts::default()).await;
    let host = env.private_host(SID);
    let cookie = env.sign_in(&host, "ada").await;

    let r = env
        .get(&host, "/page?x=1")
        .header(header::COOKIE, format!("theme=dark; {cookie}"))
        .header("x-forwarded-host", "evil.example")
        .header("x-cua-share-caller", r#"{"account":"eve"}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let seen: Value = r.json().await.unwrap();
    // The app sees its own root, rewritten for the machine, with the real
    // public host, the relay's verdict and none of the credentials.
    assert_eq!(seen["uri"], format!("/share/{SID}/page?x=1"));
    let h = &seen["headers"];
    assert_eq!(h["x-forwarded-host"], host.as_str());
    assert_eq!(h["x-forwarded-proto"], "http");
    assert_eq!(h["cookie"], "theme=dark", "{h}");
    assert!(h.get("x-cua-share-caller").is_none());
    assert!(h.get("authorization").is_none());
    let claims = claims_of(h["x-cua-relay-assertion"].as_str().unwrap());
    assert_eq!(claims["sub"], "ada");
    assert_eq!(claims["role"], "owner");
    assert_eq!(claims["aud"], env.machine.as_str());

    // The page may render here, but its cookies stay on this origin.
    let r = env
        .get(&host, "/html")
        .header(header::COOKIE, cookie.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    assert!(r.headers()[header::CONTENT_TYPE]
        .to_str()
        .unwrap()
        .starts_with("text/html"));
    assert_eq!(r.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    assert_eq!(r.headers()[header::REFERRER_POLICY], "no-referrer");
    assert_eq!(r.headers()["x-robots-tag"], "noindex, nofollow");
    assert_eq!(
        r.headers()[header::CONTENT_SECURITY_POLICY],
        "frame-ancestors 'self'"
    );
    let cookies: Vec<_> = r
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|v| v.to_str().unwrap().to_owned())
        .collect();
    assert_eq!(cookies, ["a=1; Path=/"]);
}

#[tokio::test]
async fn the_cookie_is_good_for_one_share_origin_and_nothing_else() {
    let env = Env::start(Opts::default()).await;
    let host = env.private_host(SID);
    let cookie = env.sign_in(&host, "ada").await;

    // Another share of the same machine: signed in there? No.
    let other = env.private_host("fedcba9876543210");
    let r = env
        .get(&other, "/x")
        .header(header::COOKIE, cookie.clone())
        .header(header::ACCEPT, "text/html")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FOUND, "sent to sign in again");
    let r = env
        .get(&other, "/x")
        .header(header::COOKIE, cookie.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);

    // The machine's own origin, path mode style requests, a public origin
    // and any other route ignore it.
    let bare = format!("{}.{DOMAIN}", env.machine);
    for (host, path) in [
        (bare.as_str(), format!("/share/{SID}/x")),
        (bare.as_str(), "/cua.env.v1.SystemService/Health".to_owned()),
        (bare.as_str(), "/files?path=/etc/passwd".to_owned()),
    ] {
        let r = env
            .get(host, &path)
            .header(header::COOKIE, cookie.clone())
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED, "{host}{path}");
    }
    let r = env
        .get(&env.public_host(TOKEN), "/x")
        .header(header::COOKIE, cookie.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let seen: Value = r.json().await.unwrap();
    assert!(seen["headers"].get("x-cua-relay-assertion").is_none());
    assert!(seen["headers"].get("cookie").is_none(), "{seen}");

    // A cookie signed by another relay key, or a made-up one, is refused.
    let forged = cua_relay::assertion::RelayKey::generate()
        .sign_json("cua-share-session", &json!({"sub": "ada"}));
    for bad in [
        format!("cua_share_session={forged}"),
        "cua_share_session=abc.def.ghi".into(),
    ] {
        let r = env
            .get(&host, "/x")
            .header(header::COOKIE, bad)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    }
}

#[tokio::test]
async fn login_codes_are_one_time_and_bound_to_their_share_origin() {
    let env = Env::start(Opts::default()).await;
    let host = env.private_host(SID);
    let code = env.login_code(&host, "ada", "/").await;
    let ok = env
        .get(&host, &format!("/_cua/session?{code}"))
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::FOUND);
    let again = env
        .get(&host, &format!("/_cua/session?{code}"))
        .send()
        .await
        .unwrap();
    assert_eq!(again.status(), StatusCode::FORBIDDEN, "one time only");

    // A code for one share does not open another, nor the machine's origin.
    let code = env.login_code(&host, "ada", "/").await;
    let other = env.private_host("fedcba9876543210");
    let r = env
        .get(&other, &format!("/_cua/session?{code}"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    let r = env
        .get(
            &format!("{}.{DOMAIN}", env.machine),
            &format!("/_cua/session?{code}"),
        )
        .send()
        .await
        .unwrap();
    assert!(!r.headers().contains_key(header::SET_COOKIE));
    // A relay session token is not a code, and vice versa.
    let wrong = env
        .relay
        .signing_key()
        .sign_json("cua-share-session", &json!({"x": 1}));
    let r = env
        .get(&host, &format!("/_cua/session?code={wrong}"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    // `to` cannot leave the origin.
    let code = env.login_code(&host, "ada", "/").await;
    let code = code.split('&').next().unwrap().to_owned();
    let r = env
        .get(&host, &format!("/_cua/session?{code}&to=//evil.example/"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.headers()[header::LOCATION], "/");
}

#[tokio::test]
async fn the_sign_in_only_returns_to_share_origins_of_this_relay() {
    let env = Env::start(Opts::default()).await;
    for ret in [
        "https://evil.example/".to_owned(),
        format!("http://{}.{DOMAIN}/", env.machine),
        format!(
            "http://{}.{DOMAIN}/",
            env.public_host(TOKEN).split('.').next().unwrap()
        ),
        format!("http://evil@{}/", env.private_host(SID)),
        "javascript:alert(1)".to_owned(),
        // Right shape, no such machine.
        format!("http://s{SID}-aaaaaaaaaaaa.{DOMAIN}/"),
    ] {
        let r = env
            .http
            .get(format!("{}/v1/web/login", env.base))
            .query(&[("return", ret.as_str())])
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::BAD_REQUEST, "{ret}");
    }
    // An unknown state, or a replayed one, finishes nothing.
    let r = env
        .http
        .get(format!("{}/v1/web/callback?code=x&state=nope", env.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn without_a_web_client_a_browser_without_a_token_is_just_refused() {
    let env = Env::start(Opts {
        web: false,
        ..Opts::default()
    })
    .await;
    let r = env
        .get(&env.private_host(SID), "/")
        .header(header::ACCEPT, "text/html")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    let r = env
        .http
        .get(format!("{}/v1/web/login?return=x", env.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
    // A bearer token still works on the share origin.
    let r = env
        .get(&env.private_host(SID), "/api")
        .bearer_auth(env.token("ada"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
}

#[tokio::test]
async fn a_stranger_cannot_use_somebody_elses_private_share() {
    let env = Env::start(Opts::default()).await;
    let host = env.private_host(SID);
    // Bob signs in fine, but the machine is not shared with him.
    let cookie = env.sign_in(&host, "bob").await;
    let r = env
        .get(&host, "/x")
        .header(header::COOKIE, cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    let r = env
        .get(&host, "/x")
        .bearer_auth(env.token("bob"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn public_links_need_no_account_and_are_confined() {
    let env = Env::start(Opts::default()).await;
    let host = env.public_host(TOKEN);
    let r = env
        .get(&host, "/report/index.html?v=2")
        .header(header::AUTHORIZATION, "Bearer secret")
        .header("cookie", "cua_share_session=x; keep=1")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let seen: Value = r.json().await.unwrap();
    assert_eq!(
        seen["uri"],
        format!("/share/pub/{TOKEN}/report/index.html?v=2")
    );
    let h = &seen["headers"];
    assert!(h.get("authorization").is_none(), "{h}");
    assert!(h.get("x-cua-relay-assertion").is_none());
    assert_eq!(h["cookie"], "keep=1");
    assert_eq!(h["x-forwarded-host"], host.as_str());

    // The page renders on its own origin and cannot set a parent cookie.
    let r = env.get(&host, "/html").send().await.unwrap();
    assert!(r.headers()[header::CONTENT_TYPE]
        .to_str()
        .unwrap()
        .starts_with("text/html"));
    assert_eq!(r.headers()[header::SET_COOKIE], "a=1; Path=/");

    // The owner stopping sharing the machine ends public links too.
    let r = env
        .http
        .post(format!(
            "{}/v1/machines/{}/stop-sharing",
            env.base, env.machine
        ))
        .bearer_auth(env.token("ada"))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "{}", r.status());
    let r = env.get(&host, "/x").send().await.unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn public_links_are_rate_limited_per_address_and_per_machine() {
    let env = Env::start(Opts {
        limits: ShareLimits {
            per_ip_rps: 0.2,
            per_ip_burst: 3.0,
            per_machine_rps: 0.2,
            per_machine_burst: 5.0,
        },
        // The test client is on loopback, like a proxy in front of the relay.
        trusted_proxies: "127.0.0.1/32",
        ..Opts::default()
    })
    .await;
    let host = env.public_host(TOKEN);
    let mut codes = Vec::new();
    for _ in 0..4 {
        let r = env
            .get(&host, "/x")
            .header("x-forwarded-for", "203.0.113.7")
            .send()
            .await
            .unwrap();
        codes.push((r.status(), r.headers().get(header::RETRY_AFTER).cloned()));
    }
    assert_eq!(
        codes[..3].iter().filter(|c| c.0 == StatusCode::OK).count(),
        3
    );
    assert_eq!(codes[3].0, StatusCode::TOO_MANY_REQUESTS);
    let wait: u64 = codes[3]
        .1
        .as_ref()
        .unwrap()
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!((1..=10).contains(&wait), "{wait}");
    // Another address has its own bucket...
    let r = env
        .get(&host, "/x")
        .header("x-forwarded-for", "198.51.100.9")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    // ...until the machine's own budget is spent.
    let mut refused = false;
    for i in 0..6 {
        let r = env
            .get(&host, "/x")
            .header("x-forwarded-for", format!("192.0.2.{i}"))
            .send()
            .await
            .unwrap();
        refused |= r.status() == StatusCode::TOO_MANY_REQUESTS;
    }
    assert!(refused, "the per-machine bucket ran dry");
    // The limit is for anonymous links: the owner's bearer on the private
    // origin is untouched.
    let r = env
        .get(&env.private_host(SID), "/x")
        .bearer_auth(env.token("ada"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
}

#[tokio::test]
async fn path_mode_serves_shares_but_never_renders_their_html() {
    let env = Env::start(Opts {
        host_mode: false,
        web: false,
        ..Opts::default()
    })
    .await;
    let m = &env.machine;
    // Private: a bearer is required, and the owner's gets through.
    let r = env
        .http
        .get(format!("{}/m/{m}/share/{SID}/x", env.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    let r = env
        .http
        .get(format!("{}/m/{m}/share/{SID}/x", env.base))
        .bearer_auth(env.token("ada"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let seen: Value = r.json().await.unwrap();
    assert_eq!(seen["uri"], format!("/share/{SID}/x"));
    assert_eq!(seen["headers"]["x-forwarded-prefix"], format!("/m/{m}"));
    // Public: anonymous.
    let r = env
        .http
        .get(format!("{}/m/{m}/share/pub/{TOKEN}/x", env.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    // HTML stays blocked on the relay's shared origin (S6).
    let r = env
        .http
        .get(format!("{}/m/{m}/share/pub/{TOKEN}/html", env.base))
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.headers()[header::CONTENT_TYPE],
        "application/octet-stream"
    );
    assert_eq!(r.headers()[header::CONTENT_DISPOSITION], "attachment");
    assert!(!r.headers().contains_key(header::SET_COOKIE));
    // Anything that is not a share path still needs credentials.
    let r = env
        .http
        .get(format!("{}/m/{m}/files?path=/etc/passwd", env.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn share_origins_belong_to_the_app_and_resolve_one_machine() {
    let env = Env::start(Opts::default()).await;
    // The relay's own routes do not shadow the app on a share origin.
    let r = env
        .get(&env.private_host(SID), "/healthz")
        .bearer_auth(env.token("ada"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let seen: Value = r.json().await.unwrap();
    assert_eq!(seen["uri"], format!("/share/{SID}/healthz"));
    // An unknown machine prefix is not a share.
    let r = env
        .get(&format!("s{SID}-ffffffffffff.{DOMAIN}"), "/")
        .bearer_auth(env.token("ada"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
    // The machine's own origin keeps HTML blocked and cookies stripped.
    let r = env
        .get(
            &format!("{}.{DOMAIN}", env.machine),
            &format!("/share/pub/{TOKEN}/html"),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.headers()[header::CONTENT_TYPE],
        "application/octet-stream"
    );
    assert!(!r.headers().contains_key(header::SET_COOKIE));
    // A machine id prefix that two machines share resolves to neither.
    let two = format!("{}0000", &env.machine[..28]);
    let reg = env
        .http
        .post(format!("{}/v1/machines", env.base))
        .bearer_auth(env.token("ada"))
        .json(&json!({"id": two, "name": "twin"}))
        .send()
        .await
        .unwrap();
    assert!(reg.status().is_success(), "{}", reg.status());
    let r = env
        .get(&env.private_host(SID), "/")
        .bearer_auth(env.token("ada"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND, "ambiguous prefix");
    let r = env
        .get(&format!("s{SID}-{}.{DOMAIN}", env.machine), "/")
        .bearer_auth(env.token("ada"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK, "the full id names one machine");
}

#[tokio::test]
async fn forwarded_for_is_ignored_without_trusted_proxies() {
    let env = Env::start(Opts {
        limits: ShareLimits {
            per_ip_rps: 0.2,
            per_ip_burst: 3.0,
            per_machine_rps: 100.0,
            per_machine_burst: 100.0,
        },
        ..Opts::default()
    })
    .await;
    let host = env.public_host(TOKEN);
    let mut limited = false;
    for i in 0..8 {
        let r = env
            .get(&host, "/x")
            .header("x-forwarded-for", format!("192.0.2.{i}"))
            .send()
            .await
            .unwrap();
        limited |= r.status() == StatusCode::TOO_MANY_REQUESTS;
    }
    assert!(
        limited,
        "a client-chosen X-Forwarded-For must not pick the bucket"
    );
}
