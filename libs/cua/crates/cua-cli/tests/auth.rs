//! `cua auth`, `cua wif-token` against a fake OIDC issuer, a fake GitHub
//! OIDC endpoint and the fake Fleet API.

mod common;
use common::*;
use cua_fleet::testing::FakeFleet;
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};

async fn fake_issuer() -> FakeHttp {
    let polls = Arc::new(AtomicU32::new(0));
    let refreshes = Arc::new(AtomicU32::new(0));
    // The URL is only known after binding; endpoints are relative to it via
    // the Host header.
    FakeHttp::start(move |r| {
        let host = format!("http://{}", r.headers.get("host").cloned().unwrap_or_default());
        match (r.method.as_str(), r.path.as_str()) {
            ("GET", "/.well-known/openid-configuration") => (
                200,
                json!({
                    "token_endpoint": format!("{host}/token"),
                    "device_authorization_endpoint": format!("{host}/device"),
                    "revocation_endpoint": format!("{host}/revoke"),
                }),
            ),
            ("POST", "/device") => {
                assert_eq!(r.form("client_id").as_deref(), Some("cua-cli"));
                (
                    200,
                    json!({"device_code": "dc-1", "user_code": "ABCD-EFGH",
                        "verification_uri": "https://login.test/device",
                        "verification_uri_complete": "https://login.test/device?code=ABCD-EFGH",
                        "expires_in": 60, "interval": 1}),
                )
            }
            ("POST", "/token") => match r.form("grant_type").as_deref() {
                Some("urn:ietf:params:oauth:grant-type:device_code") => {
                    if polls.fetch_add(1, Ordering::SeqCst) == 0 {
                        (400, json!({"error": "authorization_pending"}))
                    } else {
                        (
                            200,
                            json!({"access_token": jwt(json!({"preferred_username": "tester", "sub": "u-1", "azp": "cua-cli", "iss": "https://auth.test"})),
                                "refresh_token": "rt-1", "expires_in": 3600, "token_type": "Bearer"}),
                        )
                    }
                }
                Some("refresh_token") => {
                    assert_eq!(r.form("refresh_token").as_deref(), Some("rt-1"));
                    let n = refreshes.fetch_add(1, Ordering::SeqCst);
                    (
                        200,
                        json!({"access_token": jwt(json!({"preferred_username": "tester", "sub": "u-1", "n": n})),
                            "expires_in": 3600}),
                    )
                }
                Some("client_credentials") => (
                    200,
                    json!({"access_token": jwt(json!({"azp": r.form("client_id"), "sub": "svc"})),
                        "expires_in": 300, "token_type": "Bearer"}),
                ),
                other => (400, json!({"error": format!("unsupported {other:?}")})),
            },
            ("POST", "/revoke") => (200, json!({})),
            _ => (404, json!({"error": "no route"})),
        }
    })
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn device_login_session_drives_fleet_then_refresh_and_logout() {
    let issuer = fake_issuer().await;
    let fake = FakeFleet::new();
    fake.add_namespace("team-a");
    fake.add_namespace("team-b");
    let fleet = cua_daemon::fixtures::start_fleet_http(fake.clone()).await;
    let mut h = Home::new();
    h.set("CUA_OIDC_ISSUER", &issuer.url)
        .set("CUA_OIDC_POLL_UNIT_MS", "5")
        .set("CUA_FLEET_BASE_URL", &fleet.base_url);

    let o = h.run(&["auth", "status"]).await;
    assert_eq!(o.code, 1);
    assert!(o.stdout.contains("Not logged in"), "{o:?}");

    let o = h.run(&["auth", "login", "--no-browser"]).await;
    o.ok();
    assert!(o.stdout.contains("ABCD-EFGH"), "{o:?}");
    assert!(
        o.stdout.contains("Logged in to run.cua.ai as tester."),
        "{o:?}"
    );
    let cred_file = h.cua_home().join("credentials.json");
    let creds = read_json(&cred_file);
    assert_eq!(creds["refresh_token"], "rt-1");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&cred_file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    let polls = issuer
        .requests()
        .iter()
        .filter(|r| {
            r.form("grant_type")
                .is_some_and(|g| g.ends_with("device_code"))
        })
        .count();
    assert_eq!(polls, 2, "pending once, then granted");

    let o = h.run(&["auth", "status"]).await;
    o.ok();
    assert!(o.stdout.contains("Access token expires"), "{o:?}");

    // whoami with no env credentials uses the session token against Fleet.
    let o = h.run(&["--json", "auth", "whoami"]).await;
    o.ok();
    let v = o.json();
    assert_eq!(v["source"], "cua auth login session");
    assert_eq!(v["user"], "tester");
    assert_eq!(v["namespaces"], 2);
    let access = creds["access_token"].as_str().unwrap();
    assert!(fake.requests().iter().any(|r| r.path == "/api/namespaces"
        && r.header("authorization") == Some(&format!("Bearer {access}"))));

    // An expired session refreshes and persists the new token.
    let mut expired = creds.clone();
    expired["expires_at"] = json!("2020-01-01T00:00:00+00:00");
    std::fs::write(&cred_file, expired.to_string()).unwrap();
    let o = h.run(&["auth", "status"]).await;
    assert!(o.stdout.contains("expired"), "{o:?}");
    h.run(&["auth", "whoami"]).await.ok();
    let refreshed = read_json(&cred_file);
    assert_ne!(refreshed["access_token"], creds["access_token"]);
    assert_eq!(refreshed["refresh_token"], "rt-1", "refresh token kept");

    let o = h.run(&["auth", "logout"]).await;
    o.ok();
    assert!(o.stdout.contains("Logged out."));
    assert!(!cred_file.exists());
    let revoke = issuer
        .requests()
        .into_iter()
        .find(|r| r.path == "/revoke")
        .unwrap();
    assert_eq!(revoke.form("token").as_deref(), Some("rt-1"));
    let o = h.run(&["auth", "logout"]).await;
    assert!(o.stdout.contains("Not logged in."));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn whoami_reports_client_credentials_and_workload_tokens() {
    let issuer = fake_issuer().await;
    let fake = FakeFleet::new();
    fake.add_namespace("ns1");
    let fleet = cua_daemon::fixtures::start_fleet_http(fake.clone()).await;
    let mut h = Home::new();
    h.set("CUA_FLEET_BASE_URL", &fleet.base_url);

    let o = h.run(&["auth", "whoami"]).await;
    assert_eq!(o.code, 1);
    assert!(o.stdout.contains("Not authenticated"), "{o:?}");

    h.set("CUA_CLIENT_ID", "ukey-test")
        .set("CUA_CLIENT_SECRET", "s3cret")
        .set("CUA_TOKEN_URL", format!("{}/token", issuer.url));
    let o = h.run(&["auth", "whoami"]).await;
    o.ok();
    assert!(o.stdout.contains("client credentials (ukey-test)"), "{o:?}");
    assert!(o.stdout.contains("Namespaces: 1"), "{o:?}");
    assert!(!o.stdout.contains("s3cret") && !o.stderr.contains("s3cret"));
    let cc = issuer
        .requests()
        .into_iter()
        .find(|r| r.form("grant_type").as_deref() == Some("client_credentials"));
    assert!(cc.is_some(), "client-credentials grant used");

    h.set("FLEETS_TOKEN", "workload-token");
    let o = h.run(&["--json", "auth", "whoami"]).await;
    o.ok();
    assert_eq!(o.json()["source"], "FLEETS_TOKEN");
    assert!(
        fake.requests()
            .iter()
            .any(|r| r.header("authorization") == Some("Bearer workload-token"))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fleet_user_api_keys() {
    let fake = FakeFleet::new();
    let fleet = cua_daemon::fixtures::start_fleet_http(fake.clone()).await;
    let mut h = Home::new();
    h.set("CUA_FLEET_BASE_URL", &fleet.base_url)
        .set("FLEETS_TOKEN", "t");
    let o = h.run(&["auth", "keys", "ls"]).await;
    o.ok();
    assert!(o.stdout.contains("No API keys."));
    let o = h
        .run(&["auth", "keys", "create", "ci", "--scope", "sandboxes"])
        .await;
    o.ok();
    assert!(o.stdout.contains("CUA_CLIENT_ID=ukey-fake1"), "{o:?}");
    assert!(o.stdout.contains("CUA_CLIENT_SECRET=secret-1"), "{o:?}");
    let o = h.run(&["--json", "auth", "keys", "ls"]).await;
    o.ok();
    assert_eq!(o.json()[0]["name"], "ci");
    h.run(&["auth", "keys", "rm", "key-1"]).await.ok();
    assert!(fake.user_keys().is_empty());
    let o = h.run(&["auth", "keys", "rm", "key-1"]).await;
    assert_ne!(o.code, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wif_token_github_prints_only_the_token() {
    let gh = FakeHttp::start(|r| {
        if r.headers.get("authorization").map(String::as_str) != Some("bearer req-token") {
            return (401, json!({}));
        }
        if !r.path.contains("audience=fleets") || !r.path.contains("api-version=2.0") {
            return (400, json!({"path": r.path}));
        }
        (200, json!({"value": "gh-oidc-jwt"}))
    })
    .await;
    let mut h = Home::new();
    let o = h.run(&["wif-token", "github"]).await;
    assert_eq!(o.code, 4, "{o:?}");
    assert!(
        o.stderr.contains("ACTIONS_ID_TOKEN_REQUEST_URL is missing"),
        "{o:?}"
    );
    h.set(
        "ACTIONS_ID_TOKEN_REQUEST_URL",
        format!("{}/token?api-version=2.0&audience=other", gh.url),
    );
    let o = h.run(&["wif-token", "github"]).await;
    assert!(
        o.stderr
            .contains("ACTIONS_ID_TOKEN_REQUEST_TOKEN is missing"),
        "{o:?}"
    );
    h.set("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "req-token");
    let o = h.run(&["wif-token", "github"]).await;
    o.ok();
    assert_eq!(o.stdout, "gh-oidc-jwt\n");
    h.set("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "wrong");
    let o = h.run(&["wif-token", "github"]).await;
    assert_ne!(o.code, 0);
    assert!(o.stderr.contains("HTTP 401"), "{o:?}");
}

#[tokio::test]
async fn version_flag() {
    let h = Home::new();
    for f in ["-v", "--version", "-V"] {
        let o = h.run(&[f]).await;
        o.ok();
        assert!(o.stdout.starts_with("cua "), "{o:?}");
    }
}
