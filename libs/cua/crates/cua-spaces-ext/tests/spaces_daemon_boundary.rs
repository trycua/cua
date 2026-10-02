// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Live: the Keyvault site login and session teleport through the MIT/FSL
//! boundary, against a real Linux container Space.
//!
//! The MIT side is everything the caller touches: the MIT cua SDK
//! (`cua_sdk::Cua::connect`) talks to `cua daemon` over its socket, and the
//! MIT Spaces runtime routes the tools. The FSL side is registered the way
//! the Cua Spaces build registers it: a `cua_daemon::Runtime` built with the
//! `CuaSpacesDaemon` extension, whose Keyvault serves `request_site_login`
//! and mediates `teleport_app`.
//!
//! Opt-in: run through `tests/e2e/run-site-login-e2e.sh` (it starts the
//! container and the login site and exports `CUA_SITE_LOGIN_E2E_*`); without
//! them the test prints why and passes. `CUA_LIVE_CHECK_ROOT` places the
//! throwaway cua home (default: a temp dir).
//!
//! Host safety: the cua home, the Chrome and Firefox profiles and the vault
//! are throwaway directories; the host is a `FakeHost` (its Keychain answer
//! is scripted); approval is a fake presence gate. Nothing reads the real
//! Chrome or Firefox profile, the Keychain or ~/.cua.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use cua_daemon::server::{self, ServerConfig};
use cua_daemon::{Runtime, RuntimeConfig};
use cua_keyvault::CallerIdentity;
use cua_keyvault::broker::{FakePresence, InitRequest, PasswordImportSpec};
use cua_sdk::{Cua, SiteLoginOptions, TeleportApprover, TeleportDecision, TeleportManifest};
use cua_spaces_ext::daemon::{Attached, CuaSpacesDaemon};
use cua_teleport::passwords::{encrypt_for_tests, write_login_data_for_tests};
use cua_teleport::{FakeHost, HostOutput, Platform};
use serde_json::Value;

const SITE: &str = "http://login.example.test:8000/";
const SAFE_STORAGE: &str = "synthetic-safe-storage";

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.is_empty())
}

/// A throwaway directory under `CUA_LIVE_CHECK_ROOT` (short: macOS caps
/// Unix socket paths at 104 bytes), else a temp dir.
fn throwaway() -> tempfile::TempDir {
    match env("CUA_LIVE_CHECK_ROOT") {
        Some(root) => {
            std::fs::create_dir_all(&root).unwrap();
            tempfile::Builder::new()
                .prefix("lc")
                .tempdir_in(root)
                .unwrap()
        }
        None => tempfile::Builder::new()
            .prefix("cualc")
            .tempdir_in("/tmp")
            .unwrap(),
    }
}

/// The synthetic Firefox profile teleport sends (a marker in prefs.js).
fn write_firefox_profile(home: &Path) {
    for root in [
        home.join("Library/Application Support/Firefox"),
        home.join(".mozilla/firefox"),
    ] {
        let profile = root.join("Profiles/cuatest.default-release");
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::write(
            root.join("profiles.ini"),
            "[General]\nStartWithLastProfile=1\n\n[Profile0]\nName=default-release\nIsRelative=1\nPath=Profiles/cuatest.default-release\nDefault=1\n",
        )
        .unwrap();
        std::fs::write(
            profile.join("prefs.js"),
            "user_pref(\"cua.test.marker\", \"teleported-through-the-boundary\");\n",
        )
        .unwrap();
        std::fs::write(profile.join("cookies.sqlite"), b"not-a-real-db").unwrap();
        std::fs::write(profile.join("places.sqlite"), b"history").unwrap();
    }
}

/// The synthetic Chrome profile with the site's login, encrypted the way
/// Chrome on this host does, and a host whose Keychain answers the Safe
/// Storage secret.
fn chrome_host(home: &Path, user: &str, password: &str) -> Arc<FakeHost> {
    let platform = Platform::current();
    let profile = home
        .join(cua_teleport::layout::chrome::user_data_dir_for(platform))
        .join("Default");
    let encrypted = match platform {
        Platform::MacOS => encrypt_for_tests(b"v10", SAFE_STORAGE.as_bytes(), 1003, password),
        _ => encrypt_for_tests(b"v10", b"peanuts", 1, password),
    };
    write_login_data_for_tests(&profile, &[(SITE, user, encrypted)]).unwrap();
    Arc::new(FakeHost::new().with_home(home).with_responder(|c| {
        if c.args.iter().any(|a| a == "-w") {
            Ok(HostOutput::ok(format!("{SAFE_STORAGE}\n").into_bytes()))
        } else {
            Ok(HostOutput::ok(b"    \"acct\"<blob>=\"Chrome\"\n".to_vec()))
        }
    }))
}

struct Decide(Option<TeleportDecision>);

impl TeleportApprover for Decide {
    fn approve(&self, _: TeleportManifest) -> Option<TeleportDecision> {
        self.0.clone()
    }
}

#[test]
fn e2e_site_login_and_teleport_through_the_cua_spaces_daemon() {
    std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_stack_size(16 << 20)
                .enable_all()
                .build()
                .unwrap()
                .block_on(scenario())
        })
        .unwrap()
        .join()
        .unwrap();
}

async fn scenario() {
    let Some(url) = env("CUA_SITE_LOGIN_E2E_URL") else {
        eprintln!("skipped: set CUA_SITE_LOGIN_E2E_URL (run tests/e2e/run-site-login-e2e.sh)");
        return;
    };
    let token = env("CUA_SITE_LOGIN_E2E_TOKEN").unwrap_or_default();
    let user = env("CUA_SITE_LOGIN_E2E_USER").expect("CUA_SITE_LOGIN_E2E_USER");
    let password = env("CUA_SITE_LOGIN_E2E_PASSWORD").expect("CUA_SITE_LOGIN_E2E_PASSWORD");

    // The throwaway cua home, the host's app data and the daemon socket.
    let root = throwaway();
    let home: PathBuf = root.path().join("h");
    let host_home = root.path().join("host");
    std::fs::create_dir_all(&home).unwrap();
    write_firefox_profile(&host_home);
    let host = chrome_host(&host_home, &user, &password);

    // The Cua Spaces build of the daemon: the MIT runtime with the FSL
    // extension registered, as `cua-spaces-cli daemon` registers it.
    let runtime = Runtime::new(RuntimeConfig {
        state_dir: Some(home.join("sbx")),
        spaces_home: Some(home.clone()),
        teleport_home: Some(host_home.clone()),
        env_probe_timeout: Some(Duration::from_secs(20)),
        providers: Some(Vec::new()),
        extensions: vec![Arc::new(
            CuaSpacesDaemon::with_test_presence(Arc::new(FakePresence::new(true)))
                .with_test_host(host),
        )],
        ..Default::default()
    })
    .expect("daemon runtime");
    let broker = runtime
        .attached::<Attached>()
        .and_then(|a| a.keyvault())
        .expect("the Cua Spaces daemon hosts the Keyvault")
        .broker();
    let me = CallerIdentity::for_tests("com.trycua.cua", true);
    broker
        .init(
            &me,
            InitRequest {
                os_protector: false,
                passphrase: Some("correct horse battery".into()),
                recovery_key: false,
            },
        )
        .await
        .unwrap();
    let items = broker
        .import_passwords(
            &me,
            PasswordImportSpec {
                app: "chrome".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(items.saved, 1, "one saved login imported");
    let sock = home.join("cua.sock");
    let daemon = server::start(
        runtime,
        ServerConfig {
            socket_path: Some(sock.clone()),
            // The Space env passthrough rides the loopback listener.
            loopback: Some("127.0.0.1:0".parse().unwrap()),
            token: "live-check".into(),
            discovery_path: Some(home.join("daemon.json")),
            bridge_ticket_ttl: Duration::from_secs(30),
        },
    )
    .await
    .expect("daemon");

    // The caller: the MIT SDK over the daemon socket.
    let cua = Cua::connect(Some(sock.display().to_string()), None).unwrap();
    let spaces = cua.spaces();
    let info = spaces
        .add(url.clone(), Some(token), Some("live-check".into()))
        .await
        .unwrap();
    let space = spaces.space(info.id.clone()).await.unwrap();

    // 1. Site login: pending until the user approves in the Keyvault, then
    // filled and submitted in the Space; the password never reaches the
    // caller.
    let pending = space
        .request_site_login(
            SITE.into(),
            Some(SiteLoginOptions {
                agent: Some("ada".into()),
                ..Default::default()
            }),
        )
        .await
        .unwrap();
    assert_eq!(pending.status, "pending", "{pending:?}");
    let id = pending.request_id.clone().expect("a Keyvault request");
    broker.approve(&me, &id, Default::default()).await.unwrap();
    let filled = space
        .request_site_login(
            SITE.into(),
            Some(SiteLoginOptions {
                request_id: Some(id),
                wait_secs: Some(30),
                ..Default::default()
            }),
        )
        .await
        .unwrap();
    assert_eq!(filled.status, "filled", "{filled:?}");
    assert!(filled.submitted, "{filled:?}");
    assert!(
        !format!("{filled:?}").contains(&password),
        "the password never leaves the Keyvault"
    );
    let status = space
        .bash(
            "curl -s http://login.example.test:8000/status".into(),
            Some(30_000),
        )
        .await
        .unwrap();
    let status: Value = serde_json::from_str(status.stdout.trim()).unwrap_or(Value::Null);
    assert_eq!(
        status["signed_in"].as_str(),
        Some(user.as_str()),
        "{status}"
    );
    println!("site login: signed in through the Cua Spaces daemon's Keyvault");

    // 2. Session teleport through `Space::teleport` itself (not a raw tool
    // call): the manifest, a first call that only files the Keyvault
    // request (`status: "pending"`, never an `Err` -- the SDK retry-id fix),
    // then the same call again with that `request_id` once the user
    // approves, which the Keyvault then delivers.
    let manifest = space
        .teleport_manifest("firefox".into(), None)
        .await
        .unwrap();
    assert_eq!(manifest.app, "firefox");
    // The default-checked set withholds `cookies.sqlite` (a sensitive,
    // signed-in-session item). The requester's own `include` is shown to
    // the human as `would_send` but is never authoritative (red-team E2:
    // an MCP/automation caller cannot dictate what moves); what actually
    // gets captured for a whole-app request is the human's OWN choice at
    // approval time (`ApproveOptions::paths`). Compute the same widened
    // selection a human reviewing "would send: ..., cookies.sqlite" and
    // choosing to include it would produce, and approve with it -- this is
    // what makes a human's "yes, include the signed-in cookies" choice
    // actually reach the capture.
    let cookies_path = manifest
        .items
        .iter()
        .find(|i| i.relative_path.ends_with("cookies.sqlite"))
        .map(|i| i.relative_path.clone())
        .expect("the manifest offers Firefox's cookies.sqlite");
    let mut include: Vec<String> = manifest
        .items
        .iter()
        .filter(|i| i.is_checked_by_default)
        .map(|i| i.relative_path.clone())
        .collect();
    if !include.contains(&cookies_path) {
        include.push(cookies_path);
    }
    let approver = Arc::new(Decide(Some(TeleportDecision {
        include: Some(include.clone()),
        acknowledge_sensitive: true,
    })));
    let filed = space
        .teleport("firefox".into(), None, approver.clone(), None)
        .await
        .unwrap();
    assert_eq!(filed.status, "pending", "{filed:?}");
    let request = filed
        .request_id
        .clone()
        .expect("a Keyvault request id to retry with");
    broker
        .approve(
            &me,
            &request,
            cua_keyvault::broker::ApproveOptions {
                paths: Some(include),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let moved = space
        .teleport("firefox".into(), None, approver, Some(request))
        .await
        .unwrap();
    assert_eq!(moved.status, "moved", "{moved:?}");
    // The Keyvault delivers the session items (Firefox: its cookies) into
    // the Space's own profile: the synthetic bytes arrive unchanged.
    assert!(!moved.transferred_paths.is_empty(), "{moved:?}");
    let found = space
        .bash(
            // Wherever this OS keeps Firefox's profile.
            "find ~ -name cookies.sqlite -path '*irefox*' 2>/dev/null | head -1 | xargs -r cat"
                .into(),
            Some(30_000),
        )
        .await
        .unwrap();
    assert_eq!(
        found.stdout.trim(),
        "not-a-real-db",
        "the session landed in the Space: {found:?} ({moved:?})"
    );
    println!("teleport: the Keyvault delivered the session through the Cua Spaces daemon");

    let _ = cua.shutdown_daemon().await;
    drop(daemon);
}
