//! Host-only sign-in through the real CLI, with loopback OIDC/relay servers.
//! Stop at a missing explicit driver, before any service can be installed.
mod common;
use common::*;
use serde_json::json;
use std::sync::{Arc, Mutex};

const ACCESS: &str = "ephemeral-account-access";
const REFRESH: &str = "ephemeral-account-refresh";

async fn issuer(refusal: Option<&'static str>) -> FakeHttp {
    FakeHttp::start(move |r| {
        let base = format!("http://{}", r.headers["host"]);
        match (r.method.as_str(), r.path.split('?').next().unwrap()) {
            ("GET", "/.well-known/openid-configuration") => (
                200,
                json!({
                    "authorization_endpoint": format!("{base}/authorize"),
                    "token_endpoint": format!("{base}/token"),
                    "device_authorization_endpoint": format!("{base}/device")
                }),
            ),
            // Auto really chooses PKCE: do not mask a regression with a
            // missing endpoint or a refused redirect preflight.
            ("GET", "/authorize") => (200, json!({})),
            ("POST", "/device") => (
                200,
                json!({
                    "device_code": "fixture-device-code", "user_code": "ABCD-EFGH",
                    "verification_uri": format!("{base}/verify"),
                    "expires_in": 60, "interval": 1
                }),
            ),
            ("POST", "/token") => {
                assert_eq!(
                    r.form("grant_type").as_deref(),
                    Some("urn:ietf:params:oauth:grant-type:device_code")
                );
                assert_eq!(
                    r.form("device_code").as_deref(),
                    Some("fixture-device-code")
                );
                match refusal {
                    Some(error) => (400, json!({"error": error})),
                    None => (
                        200,
                        json!({"access_token": ACCESS,
                        "refresh_token": REFRESH, "expires_in": 3600}),
                    ),
                }
            }
            _ => (404, json!({"error": "unexpected request"})),
        }
    })
    .await
}

fn home(issuer: &FakeHttp, relay: &FakeHttp) -> Home {
    let mut h = Home::new();
    h.set("CUA_OIDC_ISSUER", &issuer.url)
        .set("CUA_RELAY_URL", &relay.url)
        .set("CUA_OIDC_REDIRECT_PORT", "0")
        .set("CUA_OIDC_LOGIN_TIMEOUT_SECS", "1")
        .set("CUA_OIDC_POLL_UNIT_MS", "1");
    h
}

async fn setup(h: &Home, extra: &[&str]) -> Out {
    let missing = h.dir.path().join("missing-spacesd");
    let mut args = vec![
        "host",
        "setup",
        "--runner",
        "process",
        "--driver-bin",
        missing.to_str().unwrap(),
    ];
    args.extend_from_slice(extra);
    h.run(&args).await
}

fn no_account_state(h: &Home) {
    assert!(!h.cua_home().join("credentials.json").exists());
    let mut dirs = vec![h.dir.path().to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
            } else {
                let name = path.file_name().unwrap().to_string_lossy();
                assert!(
                    !name.contains("device") && !name.contains("session"),
                    "{path:?}"
                );
                let bytes = std::fs::read(&path).unwrap();
                let text = String::from_utf8_lossy(&bytes);
                assert!(
                    !text.contains(ACCESS) && !text.contains(REFRESH),
                    "{path:?}"
                );
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_sign_in_and_retry_keep_only_the_machine_credentials() {
    let oidc = issuer(None).await;
    let registered = Arc::new(Mutex::new(None::<String>));
    let state = registered.clone();
    let relay = FakeHttp::start(move |r| {
        assert_eq!(
            (r.method.as_str(), r.path.as_str()),
            ("POST", "/v1/machines")
        );
        assert_eq!(r.headers["authorization"], format!("Bearer {ACCESS}"));
        let body: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        let id = body["id"].as_str().unwrap().to_string();
        let mut previous = state.lock().unwrap();
        if let Some(old) = previous.as_ref() {
            assert_eq!(&id, old, "retry must not create a second identity");
            assert_eq!(
                r.headers["x-cua-machine-authorization"],
                "Bearer machine-token-1"
            );
        } else {
            assert!(!r.headers.contains_key("x-cua-machine-authorization"));
        }
        let token = if previous.is_some() {
            "machine-token-2"
        } else {
            "machine-token-1"
        };
        *previous = Some(id.clone());
        (200, json!({"machine": {"id": id}, "machine_token": token}))
    })
    .await;
    let h = home(&oidc, &relay);
    for (args, token) in [
        (vec!["--remote"], "machine-token-1"),
        (vec!["--remote", "--json"], "machine-token-2"),
    ] {
        let o = setup(&h, &args).await;
        assert_eq!(o.code, 2, "{o:?}");
        assert!(
            o.stdout.is_empty(),
            "errors stay on stderr even with --json: {o:?}"
        );
        assert!(o.stderr.contains("host setup failed at download:"), "{o:?}");
        assert!(o.stderr.contains("does not exist"), "{o:?}");
        assert!(o.stderr.contains(&format!("{}/verify", oidc.url)), "{o:?}");
        assert!(o.stderr.contains("ABCD-EFGH"), "{o:?}");
        assert!(
            !o.stderr.contains(ACCESS) && !o.stderr.contains(REFRESH),
            "{o:?}"
        );
        assert_eq!(
            std::fs::read_to_string(h.cua_home().join("host/machine-token"))
                .unwrap()
                .trim(),
            token
        );
        no_account_state(&h);
    }
    assert_eq!(relay.requests().len(), 2);
    let requests = oidc.requests();
    assert_eq!(requests.iter().filter(|r| r.path == "/device").count(), 2);
    assert_eq!(requests.iter().filter(|r| r.path == "/token").count(), 2);
    assert!(!requests.iter().any(|r| r.path.starts_with("/authorize")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_refusals_stop_at_token_without_registering() {
    for refusal in ["access_denied", "expired_token"] {
        let oidc = issuer(Some(refusal)).await;
        let relay = FakeHttp::start(|_| (500, json!({"error": "must not register"}))).await;
        let h = home(&oidc, &relay);
        let o = setup(&h, &["--remote", "--json"]).await;
        assert_eq!(o.code, 6, "{o:?}");
        assert!(o.stdout.is_empty(), "{o:?}");
        assert!(o.stderr.contains("host setup failed at token:"), "{o:?}");
        assert!(o.stderr.contains(refusal), "{o:?}");
        assert!(relay.requests().is_empty());
        no_account_state(&h);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn default_uses_pkce_and_remote_reuses_an_existing_session() {
    let oidc = issuer(None).await;
    let relay = FakeHttp::start(|r| {
        assert_eq!(r.path, "/v1/machines");
        assert_eq!(r.headers["authorization"], "Bearer existing-account");
        (401, json!({"error": "fixture account refused"}))
    })
    .await;
    let h = home(&oidc, &relay);
    let o = setup(&h, &[]).await;
    assert_eq!(o.code, 6, "{o:?}");
    assert!(o.stderr.contains("host setup failed at token:"), "{o:?}");
    assert!(
        oidc.requests()
            .iter()
            .any(|r| r.path.starts_with("/authorize?"))
    );
    assert!(!oidc.requests().iter().any(|r| r.path == "/device"));
    assert!(relay.requests().is_empty());
    no_account_state(&h);

    let path = h.cua_home().join("credentials.json");
    let credentials =
        json!({"access_token": "existing-account", "refresh_token": "existing-refresh",
        "expires_at": "2099-01-01T00:00:00+00:00", "token_type": "Bearer"})
        .to_string();
    std::fs::write(&path, &credentials).unwrap();
    let before = oidc.requests().len();
    let o = setup(&h, &["--remote"]).await;
    assert_eq!(o.code, 6, "{o:?}");
    assert!(o.stderr.contains("host setup failed at register:"), "{o:?}");
    assert!(o.stderr.contains("fixture account refused"), "{o:?}");
    assert_eq!(
        oidc.requests().len(),
        before,
        "stored session takes precedence"
    );
    assert_eq!(relay.requests().len(), 1);
    assert_eq!(std::fs::read_to_string(path).unwrap(), credentials);
}

#[tokio::test]
async fn remote_is_documented_and_conflicts_with_direct() {
    let h = Home::new();
    let help = h.run(&["host", "setup", "--help"]).await;
    help.ok();
    assert!(help.stdout.contains("--remote"), "{help:?}");
    let o = h
        .run(&["host", "setup", "--remote", "--direct", "127.0.0.1:3211"])
        .await;
    assert_eq!(o.code, 2, "{o:?}");
    assert!(o.stderr.contains("cannot be used with"), "{o:?}");
    assert!(
        o.stderr.contains("--direct") && o.stderr.contains("--remote"),
        "{o:?}"
    );
    assert!(!h.cua_home().join("host").exists());
}
