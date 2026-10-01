// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The Spaces surface of the SDK, one suite in three topologies (embedded,
//! daemon over the Unix socket, daemon over loopback + token), against a
//! real cua-spacesd server core in-process (`cua_spaces_e2e`): temp
//! guest HOME/PATH (Init), temp Downloads and teleport home, a FakeHost
//! teleport receiver and sender, fake driver tools. Nothing touches host
//! apps, profiles or the keychain; every wait is bounded.
//!
//! A Keyvault-routed teleport (`Space::teleport`) reaches the Cua Keyvault
//! over `$CUA_HOME/keyvault.sock`, by design the same real, per-OS-user
//! socket in every process (never scoped by `RuntimeConfig`): a malicious
//! embedder overriding it is exactly what the Keyvault treats as untrusted
//! (red-team F16). So this suite never leaves `$CUA_HOME` at its real
//! default while a teleport call can run: [`with_isolated_cua_home`] points
//! it at this test's own throwaway `cua` dir and serializes the three
//! topologies against each other for the span (a process-wide env var), so
//! none of them can ever reach the real `~/.cua/keyvault.sock`.

use cua_daemon::{
    Runtime, RuntimeConfig,
    server::{self, DaemonHandle, ServerConfig},
};
use cua_sdk::{
    Cua, CuaError, PresenceIdentity, SpaceSendFileOptions, SpaceStreamOptions, TeleportApprover,
    TeleportDecision, TeleportManifest,
};
use cua_spaces_e2e::{self as testing, Driver, TOKEN};
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

/// Serializes the span in which `$CUA_HOME` is overridden (a process-wide
/// env var) across this binary's concurrently-running tests.
static CUA_HOME_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Points `$CUA_HOME` at `dir` for `f`, restoring it after. Holds
/// [`CUA_HOME_LOCK`] for the whole span so no other test in this binary
/// observes or overrides it meanwhile.
async fn with_isolated_cua_home<T>(dir: &Path, f: impl std::future::Future<Output = T>) -> T {
    let _guard = CUA_HOME_LOCK.lock().await;
    let saved = std::env::var_os("CUA_HOME");
    // SAFETY: serialized by `CUA_HOME_LOCK`, the only place in this binary
    // that touches `CUA_HOME`.
    unsafe { std::env::set_var("CUA_HOME", dir) };
    let out = f.await;
    // SAFETY: as above.
    unsafe {
        match &saved {
            Some(v) => std::env::set_var("CUA_HOME", v),
            None => std::env::remove_var("CUA_HOME"),
        }
    }
    out
}

#[derive(Clone, Copy, Debug)]
enum Topology {
    Embedded,
    // Unix sockets only; Windows runs the loopback topology.
    #[cfg_attr(not(unix), allow(dead_code))]
    DaemonSocket,
    DaemonLoopback,
}

struct World {
    driver: Driver,
    dirs: tempfile::TempDir,
    _daemon: Option<DaemonHandle>,
}

async fn world(t: Topology) -> (World, Arc<Cua>) {
    let driver = testing::driver().await;
    driver.confine_direct().await;
    let dirs = tempfile::tempdir().unwrap();
    // Teleport reads a synthetic Firefox profile through a FakeHost rooted
    // here: never the real profile.
    let teleport_home = dirs.path().join("host-home");
    testing::write_firefox_profile(&teleport_home);
    let runtime = Runtime::new(RuntimeConfig {
        state_dir: Some(dirs.path().join("sandboxes")),
        spaces_home: Some(dirs.path().join("cua")),
        teleport_home: Some(teleport_home),
        env_probe_timeout: Some(Duration::from_secs(5)),
        // The Cua Spaces build of the daemon: teleport, the Cua
        // Drive, persistent agents and the Keyvault.
        extensions: vec![std::sync::Arc::new(
            cua_spaces_ext::daemon::CuaSpacesDaemon::default(),
        )],
        ..Default::default()
    })
    .unwrap();
    let (cua, daemon) = match t {
        Topology::Embedded => (Cua::from_runtime(runtime), None),
        Topology::DaemonSocket | Topology::DaemonLoopback => {
            let cfg = ServerConfig {
                socket_path: Some(dirs.path().join("cua.sock")),
                loopback: Some("127.0.0.1:0".parse().unwrap()),
                token: "daemon-token".into(),
                discovery_path: Some(dirs.path().join("daemon.json")),
                bridge_ticket_ttl: Duration::from_secs(30),
            };
            let h = server::start(runtime, cfg).await.unwrap();
            let cua = match t {
                Topology::DaemonSocket => Cua::connect(
                    Some(h.socket_path.as_ref().unwrap().display().to_string()),
                    None,
                )
                .unwrap(),
                _ => Cua::connect(h.loopback_url.clone(), Some(h.token.clone())).unwrap(),
            };
            (cua, Some(h))
        }
    };
    (
        World {
            driver,
            dirs,
            _daemon: daemon,
        },
        cua,
    )
}

/// Approves with a fixed decision and records what it was shown.
struct Approver {
    decision: Option<TeleportDecision>,
    seen: Mutex<Vec<TeleportManifest>>,
}

impl TeleportApprover for Approver {
    fn approve(&self, manifest: TeleportManifest) -> Option<TeleportDecision> {
        self.seen.lock().unwrap().push(manifest);
        self.decision.clone()
    }
}

fn approver(decision: Option<TeleportDecision>) -> Arc<Approver> {
    Arc::new(Approver {
        decision,
        seen: Mutex::new(vec![]),
    })
}

async fn suite(t: Topology) {
    let (w, cua) = world(t).await;
    let cua_home_dir = w.dirs.path().join("cua");
    with_isolated_cua_home(&cua_home_dir, suite_body(t, &w, &cua)).await;
}

async fn suite_body(t: Topology, w: &World, cua: &Arc<Cua>) {
    let d = &w.driver;
    let spaces = cua.spaces();

    // --- registry
    let info = spaces
        .add(d.url.clone(), Some(TOKEN.into()), Some("test".into()))
        .await
        .unwrap();
    assert!(
        info.id.starts_with("direct:127.0.0.1:"),
        "{t:?} {}",
        info.id
    );
    assert_eq!(info.provider, "direct");
    assert_eq!(info.name, "test");
    assert!(info.features.contains(&"driver".to_string()));
    assert!(info.added_at.is_some());
    let registry = std::fs::read_to_string(w.dirs.path().join("cua/spaces.json")).unwrap();
    assert!(registry.contains(&info.id) && !registry.contains(TOKEN));
    assert_eq!(spaces.list().await.unwrap(), vec![info.clone()]);
    assert_eq!(spaces.resolve("test".into()).await.unwrap().id, info.id);
    assert!(matches!(
        spaces.resolve("nope".into()).await,
        Err(CuaError::NotFound(_))
    ));
    let wrong = spaces
        .add(d.url.clone(), Some("wrong".into()), None)
        .await
        .unwrap_err();
    assert!(
        matches!(wrong, CuaError::Unauthenticated(_)),
        "{t:?} {wrong:?}"
    );

    // --- exec
    let space = spaces.space(info.id.clone()).await.unwrap();
    assert_eq!(space.id(), info.id);
    assert!(space.supports("driver".into()));
    let out = space
        .bash("echo hi; echo oops >&2; exit 3".into(), None)
        .await
        .unwrap();
    assert_eq!(out.stdout, "hi\n");
    assert_eq!(out.stderr, "oops\n");
    assert_eq!(out.exit_code, Some(3));
    assert_eq!(out.rendered, "hi\n[stderr]\noops\n[exit 3]");
    assert_eq!(
        std::path::Path::new(&space.home().await.unwrap()),
        d.home,
        "{t:?}: guest HOME is the temp dir"
    );
    let guest = d.home.join("w/note.txt").display().to_string();
    let wr = space.write(guest.clone(), b"data".to_vec()).await.unwrap();
    assert_eq!(wr.bytes, 4);
    assert_eq!(std::fs::read(d.home.join("w/note.txt")).unwrap(), b"data");

    // --- files
    let host = tempfile::tempdir().unwrap();
    let src = host.path().join("up.bin");
    std::fs::write(&src, vec![7u8; 300_000]).unwrap();
    let up = space
        .upload(
            src.display().to_string(),
            Some(d.home.join("up.bin").display().to_string()),
        )
        .await
        .unwrap();
    assert!(up.verified && up.bytes == 300_000, "{up:?}");
    let back = host.path().join("back");
    std::fs::create_dir_all(&back).unwrap();
    let down = space
        .download(
            d.home.join("up.bin").display().to_string(),
            back.display().to_string(),
        )
        .await
        .unwrap();
    assert!(down.verified, "{down:?}");
    assert_eq!(std::fs::read(back.join("up.bin")).unwrap().len(), 300_000);
    let drop = host.path().join("hello.txt");
    std::fs::write(&drop, b"hello space").unwrap();
    let sent = space
        .send_file(
            drop.display().to_string(),
            SpaceSendFileOptions {
                target_directory: Some("inbox".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(sent.verified && sent.files.len() == 1, "{sent:?}");
    assert_eq!(
        std::fs::read(d.downloads.join("inbox/hello.txt")).unwrap(),
        b"hello space"
    );

    // --- driver tools
    let tools = space.list_tools(None).await.unwrap();
    assert!(tools.iter().any(|t| t.name == "get_screen_size"));
    let r = space
        .call_tool("get_screen_size".into(), None, Some("mcp".into()), None)
        .await
        .unwrap();
    assert!(!r.is_error);
    assert_eq!(r.text, "1280x800");
    assert!(matches!(
        space.list_tools(Some("blender".into())).await,
        Err(CuaError::NotFound(_))
    ));

    // --- streams and presence: this server has no desktop, and says so.
    let e = space
        .open_stream(SpaceStreamOptions::default())
        .await
        .unwrap_err();
    assert!(
        matches!(&e, CuaError::CapabilityMissing(m) if m.contains("desktop_stream")),
        "{t:?} {e:?}"
    );
    let e = space
        .join_presence(
            PresenceIdentity {
                id: "u1".into(),
                display_name: "Tester".into(),
                ..Default::default()
            },
            Some(2_000),
        )
        .await
        .err()
        .expect("presence refused");
    assert!(matches!(e, CuaError::CapabilityMissing(_)), "{e:?}");

    // --- teleport: consent is a callback, and a sensitive item needs an
    // explicit acknowledgement. Sensitive items (cookies/sign-ins, saved
    // passwords, history) are opt-in, never checked by default -- so the
    // default selection alone never exercises `acknowledge_sensitive`; the
    // test below selects `cookies.sqlite` explicitly (as a real consent UI
    // would with its checkbox ticked) to cover that path.
    let m = space
        .teleport_manifest("firefox".into(), None)
        .await
        .unwrap();
    assert_eq!(m.app, "firefox");
    let cookies_item = m
        .items
        .iter()
        .find(|i| i.relative_path.ends_with("cookies.sqlite"))
        .expect("the fixture's cookies.sqlite is in the manifest");
    assert!(cookies_item.is_sensitive, "{cookies_item:?}");
    assert!(
        !cookies_item.is_checked_by_default,
        "sign-ins are opt-in, never ticked by default: {cookies_item:?}"
    );
    assert!(
        m.items
            .iter()
            .all(|i| !(i.is_sensitive && i.is_checked_by_default)),
        "no sensitive item is ever ticked by default: {m:?}"
    );
    // The defaults plus the opted-in cookie, the way a real consent UI
    // sends it once the user ticks "Keep me signed in".
    let mut with_cookies: Vec<String> = m
        .items
        .iter()
        .filter(|i| i.is_checked_by_default)
        .map(|i| i.relative_path.clone())
        .collect();
    with_cookies.push(cookies_item.relative_path.clone());
    let declined = approver(None);
    assert!(matches!(
        space
            .teleport("firefox".into(), None, declined.clone(), None)
            .await,
        Err(CuaError::TeleportRefused(_))
    ));
    assert_eq!(declined.seen.lock().unwrap().len(), 1);
    let unacknowledged = approver(Some(TeleportDecision {
        include: Some(with_cookies.clone()),
        acknowledge_sensitive: false,
    }));
    assert!(matches!(
        space
            .teleport("firefox".into(), None, unacknowledged, None)
            .await,
        Err(CuaError::TeleportRefused(_))
    ));
    let acknowledged = || {
        approver(Some(TeleportDecision {
            include: Some(with_cookies.clone()),
            acknowledge_sensitive: true,
        }))
    };
    if matches!(t, Topology::Embedded) {
        // A pure embedded runtime (`Cua::from_runtime`, no `server::start`)
        // never serves `$CUA_HOME/keyvault.sock`: `Space::teleport`'s
        // Keyvault delivery fails closed with `HostCapabilityMissing`
        // rather than silently falling back to a direct, non-Keyvault
        // upload. A real machine always has a `cua daemon` process serving
        // that same socket (the Tauri and SwiftUI apps' `teleport_push`
        // calls this same `Space::teleport` directly, in-process, but reach
        // that separately-running daemon's Keyvault over the socket); only
        // this bare-runtime-with-no-daemon-anywhere shape has none.
        let err = space
            .teleport("firefox".into(), None, acknowledged(), None)
            .await
            .unwrap_err();
        assert!(matches!(err, CuaError::HostCapabilityMissing(_)), "{err:?}");
    } else {
        // Through the daemon, `teleport_app` is consent-gated and its
        // Keyvault is a real, separately-initialized vault this suite never
        // runs `cua keyvault init` against: it refuses the request rather
        // than filing it.
        let err = space
            .teleport("firefox".into(), None, acknowledged(), None)
            .await
            .unwrap_err();
        assert!(matches!(err, CuaError::TeleportRefused(_)), "{err:?}");
    }

    // --- the contract, verbatim
    let tools: serde_json::Value =
        serde_json::from_str(&spaces.list_tools_json().await.unwrap()).unwrap();
    assert_eq!(
        tools["tools"].as_array().unwrap().len(),
        cua_spaces::contract::tools().len()
    );
    let r = spaces
        .call_tool_json("list_spaces".into(), None)
        .await
        .unwrap();
    assert!(!r.is_error && r.text.contains(&info.id), "{r:?}");
    let r = spaces
        .call_tool_json(
            "space_bash".into(),
            Some(format!(
                r#"{{"space":"{}","command":"echo tool"}}"#,
                info.id
            )),
        )
        .await
        .unwrap();
    assert_eq!(r.text, "tool\n[exit 0]");
    let r = spaces
        .call_tool_json(
            "space_bash".into(),
            Some(r#"{"space":"space://direct/nowhere:1","command":"true"}"#.into()),
        )
        .await
        .unwrap();
    assert!(r.is_error, "{r:?}");
    assert!(matches!(
        spaces.call_tool_json("nope".into(), None).await,
        Err(CuaError::NotFound(_))
    ));
    let caps: serde_json::Value =
        serde_json::from_str(&spaces.agent_capabilities().await.unwrap()).unwrap();
    assert!(caps["harnesses"].as_array().is_some_and(|h| !h.is_empty()));

    // --- agents: runs speak ACP, so a fake CLI cannot stand in for a model;
    // this checks what needs none. Real runs: `cargo test -p cua-agents --test
    // e2e_live` (libs/cua/crates/cua-agents/tests/e2e/run-agents-e2e.sh).
    assert!(space.agent_list().await.unwrap().is_empty());
    let ghost = space
        .agent_status("run-00000000".into(), None)
        .await
        .unwrap_err();
    assert!(matches!(ghost, CuaError::NotFound(_)), "{ghost:?}");

    // --- lifecycle
    let msg = spaces.delete(info.id.clone()).await.unwrap();
    assert!(msg.contains(&info.id), "{msg}");
    assert!(spaces.list().await.unwrap().is_empty());
    assert!(matches!(
        spaces.remove(info.id.clone()).await,
        Err(CuaError::NotFound(_))
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spaces_embedded() {
    tokio::time::timeout(Duration::from_secs(120), suite(Topology::Embedded))
        .await
        .expect("suite timed out");
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spaces_daemon_over_unix_socket() {
    tokio::time::timeout(Duration::from_secs(120), suite(Topology::DaemonSocket))
        .await
        .expect("suite timed out");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spaces_daemon_over_loopback_token() {
    tokio::time::timeout(Duration::from_secs(120), suite(Topology::DaemonLoopback))
        .await
        .expect("suite timed out");
}
