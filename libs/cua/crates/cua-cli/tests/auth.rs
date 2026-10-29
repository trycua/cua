//! `cua auth` against a fake OIDC issuer.

mod common;
use common::*;
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
async fn device_login_then_status_and_logout() {
    let issuer = fake_issuer().await;
    let mut h = Home::new();
    h.set("CUA_OIDC_ISSUER", &issuer.url)
        .set("CUA_OIDC_POLL_UNIT_MS", "5");

    let o = h.run(&["auth", "status"]).await;
    assert_eq!(o.code, 1);
    assert!(o.stdout.contains("Not logged in"), "{o:?}");

    let o = h.run(&["auth", "login", "--no-browser"]).await;
    o.ok();
    assert!(o.stdout.contains("ABCD-EFGH"), "{o:?}");
    assert!(o.stdout.contains("Logged in to Cua as tester."), "{o:?}");
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

/// Stores a signed-in session (the file store) holding `access_token`.
fn sign_in(h: &Home, access_token: &str) {
    let expires = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
    std::fs::create_dir_all(h.cua_home()).unwrap();
    std::fs::write(
        h.cua_home().join("credentials.json"),
        json!({"access_token": access_token, "refresh_token": "rt-1", "expires_at": expires})
            .to_string(),
    )
    .unwrap();
}

/// A fake Cua API (`GET /v1/me`) for the account behind `token`.
async fn fake_cua_api(token: &str) -> FakeHttp {
    let bearer = format!("Bearer {token}");
    FakeHttp::start(move |r| {
        if r.method == "GET"
            && r.path == "/v1/me"
            && r.headers.get("authorization") == Some(&bearer)
        {
            (
                200,
                json!({"sub": "u-ada", "email": "ada@aigis.test",
                    "orgs": [{"id": "o-1", "alias": "aigis", "name": "AIGIS", "role": "owner"}],
                    "entitlements": {"plan": "pro", "status": "active", "warnings": []}}),
            )
        } else {
            (401, json!({"error": "unauthorized"}))
        }
    })
    .await
}

fn ada_token() -> String {
    jwt(json!({
        "preferred_username": "ada", "email": "ada@aigis.test", "sub": "u-ada",
        "azp": "cua-cli", "iss": "https://auth.cua.test/realms/cyclops-cs",
        "exp": 4_102_444_800_i64,
    }))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn whoami_reads_the_cua_sign_in_and_only_calls_the_cua_api() {
    let token = ada_token();
    let api = fake_cua_api(&token).await;
    // The identity provider is never asked: the token is still valid.
    let issuer = fake_issuer().await;
    let mut h = Home::new();
    h.set("CUA_SPACES_API_URL", &api.url)
        .set("CUA_OIDC_ISSUER", &issuer.url);
    sign_in(&h, &token);

    let o = h.run(&["auth", "whoami"]).await;
    o.ok();
    assert!(
        o.stdout
            .starts_with("Logged in to Cua as ada (ada@aigis.test).\n"),
        "{o:?}"
    );
    for line in [
        "Plan:    pro",
        "Teams:   1",
        "Client:  cua-cli",
        "Expires: 2100-01-01T00:00:00+00:00",
    ] {
        assert!(o.stdout.contains(line), "{line}: {o:?}");
    }
    let v = {
        let o = h.run(&["--json", "auth", "whoami"]).await;
        o.ok();
        o.json()
    };
    assert_eq!(
        v,
        json!({"authenticated": true, "user": "ada", "email": "ada@aigis.test",
            "subject": "u-ada", "client": "cua-cli",
            "issuer": "https://auth.cua.test/realms/cyclops-cs",
            "expires_at": "2100-01-01T00:00:00+00:00", "plan": "pro", "teams": 1})
    );
    // Nothing else was called: the fake API saw the two `/v1/me` reads, with
    // the session's token, and the identity provider none.
    let reqs = api.requests();
    assert_eq!(reqs.len(), 2, "{reqs:?}");
    assert!(reqs.iter().all(|r| r.path == "/v1/me"), "{reqs:?}");
    assert!(issuer.requests().is_empty(), "{:?}", issuer.requests());
    for text in [&o.stdout, &o.stderr] {
        for gone in ["Fleet", "Namespaces", "Credential", "run.cua.ai"] {
            assert!(!text.contains(gone), "{gone}: {o:?}");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn whoami_without_a_session_says_so() {
    let h = Home::new();
    let o = h.run(&["auth", "whoami"]).await;
    assert_eq!(o.code, 1, "{o:?}");
    assert_eq!(o.stdout, "Not logged in. Run 'cua auth login'.\n");
    let o = h.run(&["--json", "auth", "whoami"]).await;
    assert_eq!(o.code, 1, "{o:?}");
    assert_eq!(o.json(), json!({"authenticated": false}));
    // The old Fleet credentials are not a Cua sign-in any more.
    let mut h = Home::new();
    h.set("FLEETS_TOKEN", "t")
        .set("CUA_CLIENT_ID", "ukey-x")
        .set("CUA_CLIENT_SECRET", "s3cret");
    let o = h.run(&["auth", "whoami"]).await;
    assert_eq!(o.code, 1, "{o:?}");
    assert!(o.stdout.contains("Not logged in"), "{o:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn whoami_keeps_the_claims_when_the_cua_api_is_down_and_fails_when_it_refuses() {
    let token = ada_token();
    let mut h = Home::new();
    sign_in(&h, &token);
    // Nothing listens on port 9.
    h.set("CUA_SPACES_API_URL", "http://127.0.0.1:9");
    let o = h.run(&["auth", "whoami"]).await;
    o.ok();
    assert!(
        o.stdout
            .starts_with("Logged in to Cua as ada (ada@aigis.test).\n"),
        "{o:?}"
    );
    assert!(
        o.stdout.contains("the Cua API could not be reached"),
        "{o:?}"
    );
    assert!(!o.stdout.contains("Plan:"), "{o:?}");
    let o = h.run(&["--json", "auth", "whoami"]).await;
    o.ok();
    assert_eq!(o.json()["user"], "ada");
    assert!(o.json()["plan"].is_null() && o.json()["teams"].is_null());

    // A refused session (401): the same unauthenticated failure as every
    // other command (exit 6), with nothing printed on stdout.
    let api = fake_cua_api("another-token").await;
    h.set("CUA_SPACES_API_URL", &api.url);
    let o = h.run(&["auth", "whoami"]).await;
    assert_eq!(o.code, 6, "{o:?}");
    assert!(o.stdout.is_empty(), "{o:?}");
    assert!(o.stderr.contains("no longer valid"), "{o:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_and_whoami_say_logged_in_to_cua_not_run_cua_ai() {
    let token = ada_token();
    let api = fake_cua_api(&token).await;
    let mut h = Home::new();
    h.set("CUA_SPACES_API_URL", &api.url);
    sign_in(&h, &token);
    for args in [
        &["auth", "status"][..],
        &["--json", "auth", "status"],
        &["auth", "whoami"],
        &["--json", "auth", "whoami"],
    ] {
        let o = h.run(args).await;
        o.ok();
        for text in [&o.stdout, &o.stderr] {
            assert!(
                !text.contains("run.cua.ai") && !text.contains("Fleet"),
                "{args:?}: {o:?}"
            );
        }
    }
    let o = h.run(&["auth", "status"]).await;
    assert!(o.stdout.contains("Logged in to Cua."), "{o:?}");
    let o = h.run(&["auth", "whoami"]).await;
    assert!(o.stdout.starts_with("Logged in to Cua as "), "{o:?}");
}

/// Commands and options of the ended Cua Cloud stay as hidden stubs: each
/// fails with the one ended message (exit 4) and they are not in `--help`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn removed_cloud_commands_fail_with_the_ended_message() {
    let h = Home::new();
    // The error as every command prints it (exit 4, "not supported").
    let ended = format!("cua: unsupported: {}\n", cua_sandbox_core::CLOUD_ENDED);
    let families: [(&str, &[&[&str]]); 6] = [
        (
            "fleet",
            &[
                &["fleet", "pools", "ls"],
                &["fleet", "pools", "gc", "--idle", "2h"],
                &["fleet", "pool", "export", "ci", "--terraform"],
                &["--json", "fleet"],
                &["fleet", "--help"],
            ],
        ),
        (
            "pools",
            &[&["pools", "ls"], &["pools", "gc", "--idle", "1h"]],
        ),
        (
            "auth keys",
            &[
                &["auth", "keys", "ls"],
                &["auth", "keys", "create", "ci", "--scope", "sandboxes"],
                &["auth", "keys", "rm", "key-1"],
            ],
        ),
        (
            "image",
            &[
                &["image", "ls"],
                &["img", "list", "--namespace", "ns"],
                &["image", "info", "desktop"],
                &["image", "rm", "desktop", "--force"],
                &["image", "delete", "desktop"],
                &["image", "create", "-f", "image.json"],
            ],
        ),
        (
            "sandbox",
            &[
                &["sb", "keep-alive", "dev", "--for", "2h"],
                &["sandbox", "keep-alive", "cloud:dev"],
                &["sb", "ls", "--cloud"],
                &["sb", "info", "dev", "--cloud"],
                &["sb", "exec", "--cloud", "dev", "true"],
            ],
        ),
        ("wif-token", &[&["wif-token", "github"]]),
    ];
    for (family, runs) in families {
        for args in runs {
            let mut full = vec!["--embedded"];
            full.extend_from_slice(args);
            let o = h.run(&full).await;
            assert_eq!(o.code, 4, "{family} {args:?}: {o:?}");
            assert!(o.stdout.is_empty(), "{family} {args:?}: {o:?}");
            assert_eq!(o.stderr, ended, "{family} {args:?}: {o:?}");
        }
    }
}

/// The stubs are hidden: not in the top-level or group help.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn removed_cloud_commands_are_not_in_help() {
    let h = Home::new();
    let lists = |help: &str, name: &str| {
        help.lines()
            .any(|l| l.split_whitespace().next() == Some(name))
    };
    for (args, hidden) in [
        (&["--help"][..], &["fleet", "pools", "wif-token"][..]),
        (&["auth", "--help"], &["keys"]),
        (
            &["image", "--help"],
            &["ls", "info", "rm", "create", "list"],
        ),
        (&["sb", "--help"], &["keep-alive"]),
    ] {
        let o = h.run(args).await;
        o.ok();
        for name in hidden {
            assert!(
                !lists(&o.stdout, name),
                "{args:?} lists {name}: {}",
                o.stdout
            );
        }
    }
    let o = h.run(&["sb", "create", "--help"]).await;
    o.ok();
    // (`--pool` names a pool on this computer: `cua sb pool create`.)
    for flag in ["--apply", "--warm", "--claim-ttl", "--max-pool-size"] {
        assert!(!o.stdout.contains(flag), "{flag}: {}", o.stdout);
    }
    let o = h.run(&["sb", "ls", "--help"]).await;
    o.ok();
    assert!(!o.stdout.contains("--cloud"), "{}", o.stdout);
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
