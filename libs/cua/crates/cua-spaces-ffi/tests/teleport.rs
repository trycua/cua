// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `Cua.teleport()` (the Cua Spaces app export) against the in-process mock
//! spacesd, embedded and through the daemon.
//!
//! Host-safe: the sender acts on a `cua_teleport::FakeHost` whose home is a
//! temporary directory holding a fake Slack profile, so no real app,
//! profile, Keychain or authorization prompt is ever touched. The mock only
//! reassembles and SHA-checks the uploaded bundle; nothing is imported.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use cua_daemon::fixtures;
use cua_daemon::server::{self, ServerConfig};
use cua_daemon::{Runtime, RuntimeConfig};
use cua_sdk::{Cua, CuaError, SandboxCreateOptions};
use cua_spaces_ffi::{Teleport, TeleportApproval, TeleportApprovalRequest, TeleportScope};
use cua_teleport::layout::electron::app_support_root;
use cua_teleport::{FakeHost, Platform};

const TOKEN: &str = "env-token";

fn fake_home() -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    let prof = home
        .path()
        .join(app_support_root(Platform::current()))
        .join("Slack");
    std::fs::create_dir_all(prof.join("Local Storage/leveldb")).unwrap();
    std::fs::write(prof.join("Cookies"), b"slack-cookies").unwrap();
    std::fs::write(prof.join("Preferences"), b"{}").unwrap();
    std::fs::write(
        prof.join("Local Storage/leveldb/000003.log"),
        vec![1u8; 3000],
    )
    .unwrap();
    home
}

fn direct(url: &str) -> SandboxCreateOptions {
    SandboxCreateOptions {
        on: Some(format!("direct:{url}")),
        kind: None,
        runtime: None,
        image: String::new(),
        name: Some("teleport-direct".into()),
        token: Some(TOKEN.into()),
        pool: None,
        os: None,
        cpus: None,
        memory_mb: None,
        ports: vec![],
        services: Default::default(),
        wait_for: vec![],
        ready_timeout_ms: None,
        env: Default::default(),
        fleet_replicas: None,
        fleet_ttl_seconds: None,
        warm: None,
        max_pool_size: None,
        command: None,
        cloud: None,
        sidecars: vec![],
        registry_secret: None,
        build: None,
        network: None,
        overlays: vec![],
        keep_on_failure: false,
        gpu: None,
    }
}

#[derive(Default)]
struct Record {
    approve: bool,
    seen: Mutex<Vec<TeleportApprovalRequest>>,
}

impl TeleportApproval for Record {
    fn approve(&self, request: TeleportApprovalRequest) -> bool {
        self.seen.lock().unwrap().push(request);
        self.approve
    }
}

async fn run_suite(cua: Arc<Cua>, env: &fixtures::SpacesdFixture) {
    let home = fake_home();
    let host = Arc::new(FakeHost::new().with_home(home.path()));
    let teleport = Teleport::with_host(host.clone());

    let providers = teleport.providers();
    let slack = providers.iter().find(|p| p.id == "slack").unwrap();
    assert!(
        slack
            .app_ids
            .contains(&"com.tinyspeck.slackmacgap".to_string())
    );
    assert_eq!(
        slack.install_probe.as_deref(),
        Some("/Applications/Slack.app")
    );

    let manifest = teleport
        .manifest("Slack".into(), TeleportScope::Full, None)
        .await
        .unwrap();
    assert_eq!(manifest.app, "slack");
    assert_eq!(manifest.scope, "full");
    let cookies = manifest
        .items
        .iter()
        .find(|i| i.relative_path == "electron/Cookies")
        .unwrap();
    assert!(cookies.is_sensitive && cookies.is_checked_by_default);

    let sb = cua.sandboxes().create(direct(&env.url)).await.unwrap();

    // Declined consent: nothing read, nothing sent, no OS prompt.
    let decline = Arc::new(Record::default());
    let err = teleport
        .send(
            sb.clone(),
            "Slack".into(),
            TeleportScope::Full,
            None,
            Some(decline.clone()),
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, CuaError::PermissionDenied(_)), "{err}");
    assert_eq!(decline.seen.lock().unwrap().len(), 1);
    assert!(host.authorizations().is_empty());
    assert!(env.mock.state.teleport_imports().is_empty());

    // An item the manifest does not offer.
    let err = teleport
        .send(
            sb.clone(),
            "Slack".into(),
            TeleportScope::Full,
            Some(vec!["electron/nope".into()]),
            None,
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, CuaError::InvalidArgument(_)), "{err}");

    // Approved default selection: consent, then the (fake) OS gate, then the
    // upload.
    let approve = Arc::new(Record {
        approve: true,
        ..Default::default()
    });
    let result = teleport
        .send(
            sb.clone(),
            "Slack".into(),
            TeleportScope::Full,
            None,
            Some(approve.clone()),
            None,
        )
        .await
        .unwrap();
    let seen = approve.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1);
    assert!(seen[0].sensitive);
    assert_eq!(host.authorizations().len(), 1);
    assert_eq!(result.provider_id, "slack");
    assert!(result.launched);
    assert!(result.sent.contains(&"electron/Cookies".to_string()));

    let imports = env.mock.state.teleport_imports();
    assert_eq!(imports.len(), 1);
    assert_eq!(imports[0].app, "slack");
    assert!(imports[0].options.launch_after);
    assert_eq!(imports[0].bundle.len() as u64, result.bundle_bytes);
    let entries =
        cua_teleport::bundle::BundleReader::open(std::io::Cursor::new(imports[0].bundle.clone()))
            .unwrap()
            .read_all()
            .unwrap();
    let cookies = entries
        .iter()
        .find(|e| e.rel_path == "electron/Cookies")
        .unwrap();
    assert_eq!(cookies.bytes, b"slack-cookies");

    // The same upload through an explicit env connection.
    let env_client = sb.spacesd(Some(5_000)).await.unwrap();
    let again = teleport
        .send_env(
            env_client,
            "Slack".into(),
            TeleportScope::Full,
            Some(vec!["electron/Preferences".into()]),
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(again.sent, ["electron/Preferences"]);
    assert_eq!(env.mock.state.teleport_imports().len(), 2);
    // Only a non-sensitive item: no second OS prompt.
    assert_eq!(host.authorizations().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn teleport_embedded_and_through_the_daemon() {
    tokio::time::timeout(Duration::from_secs(120), async {
        for daemon in [false, true] {
            let env = fixtures::start_env(Some(TOKEN), None).await;
            let dirs = tempfile::tempdir().unwrap();
            let runtime = Runtime::new(RuntimeConfig {
                state_dir: Some(dirs.path().join("sandboxes")),
                // Never the real ~/.cua registry or teleport recents.
                spaces_home: Some(dirs.path().join("cua")),
                teleport_home: Some(dirs.path().join("cua")),
                env_probe_timeout: Some(Duration::from_secs(5)),
                ..Default::default()
            })
            .unwrap();
            if daemon {
                let cfg = ServerConfig {
                    socket_path: None,
                    loopback: Some("127.0.0.1:0".parse().unwrap()),
                    token: "daemon-token".into(),
                    discovery_path: Some(dirs.path().join("daemon.json")),
                    bridge_ticket_ttl: Duration::from_secs(30),
                };
                let h = server::start(runtime, cfg).await.unwrap();
                let cua = Cua::connect(h.loopback_url.clone(), Some(h.token.clone())).unwrap();
                run_suite(cua, &env).await;
            } else {
                run_suite(Cua::from_runtime(runtime), &env).await;
            }
        }
    })
    .await
    .expect("teleport suite timed out");
}
