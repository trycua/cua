// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The host/onboarding command layer (`cua_spaces_lib::host::HostCommands`,
//! which the `host_*` / `onboarding_*` Tauri commands call) against a fake
//! backend. Hermetic: temp dirs only; nothing is installed and no service
//! manager is touched.

use async_trait::async_trait;
use cua_spaces_lib::host::{
    ConnectedClientView, HostBackend, HostCommands, HostSettingChange, HostSetupRequest,
    HostStatusView, OnboardingMode, OnboardingStore, ServiceStateView,
};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct FakeHost {
    status: Mutex<HostStatusView>,
    calls: Mutex<Vec<String>>,
    fail_status: bool,
}

#[async_trait]
impl HostBackend for FakeHost {
    async fn status(&self) -> Result<HostStatusView, String> {
        if self.fail_status {
            return Err("relay unreachable".into());
        }
        Ok(self.status.lock().unwrap().clone())
    }
    async fn setup(&self, request: HostSetupRequest) -> Result<HostStatusView, String> {
        self.calls.lock().unwrap().push(format!(
            "setup {} {:?} {:?} {:?}",
            request.mode, request.relay_url, request.direct, request.allow
        ));
        let mut status = self.status.lock().unwrap();
        *status = HostStatusView {
            configured: true,
            mode: Some(request.mode.clone()),
            relay_url: request.relay_url.clone(),
            direct_url: request.direct.map(|d| format!("http://{d}")),
            machine_id: Some("0123abcd4567".into()),
            name: request.name,
            sharing: true,
            service: ServiceStateView {
                installed: true,
                running: true,
                kind: "process".into(),
                detail: String::new(),
            },
            online: Some(true),
            clients: vec![ConnectedClientView {
                id: "u2".into(),
                email: Some("grace@example.com".into()),
                streams: 2,
                ..Default::default()
            }],
            ..Default::default()
        };
        Ok(status.clone())
    }
    async fn stop_sharing(&self) -> Result<HostStatusView, String> {
        self.calls.lock().unwrap().push("stop".into());
        let mut status = self.status.lock().unwrap();
        status.sharing = false;
        status.clients.clear();
        Ok(status.clone())
    }
    async fn start_sharing(&self) -> Result<HostStatusView, String> {
        self.calls.lock().unwrap().push("start".into());
        let mut status = self.status.lock().unwrap();
        status.sharing = true;
        Ok(status.clone())
    }
    async fn remove(&self) -> Result<(), String> {
        self.calls.lock().unwrap().push("remove".into());
        *self.status.lock().unwrap() = HostStatusView::default();
        Ok(())
    }
    async fn configure(&self, change: HostSettingChange) -> Result<HostStatusView, String> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("configure {change:?}"));
        let mut status = self.status.lock().unwrap();
        if let Some(d) = change.share_desktop {
            status.share_desktop = d;
        }
        if let Some(p) = change.provide_spaces {
            status.provide_spaces = p;
        }
        Ok(status.clone())
    }
}

fn commands(
    dir: &std::path::Path,
    fake: Arc<FakeHost>,
    mode: Option<OnboardingMode>,
) -> HostCommands {
    HostCommands::new(fake, OnboardingStore::in_dir(&dir.join("config")), mode)
}

#[tokio::test]
async fn onboarding_persists_and_reports_the_installer_mode() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Arc::new(FakeHost::default());
    let cmds = commands(dir.path(), fake.clone(), Some(OnboardingMode::Host));
    let state = cmds.onboarding_state();
    assert!(!state.completed);
    assert_eq!(state.installer_mode, Some(OnboardingMode::Host));
    let json = serde_json::to_value(&state).unwrap();
    assert_eq!(
        json,
        serde_json::json!({"completed": false, "installerMode": "host"})
    );

    assert!(cmds.complete_onboarding("teleport").is_err());
    cmds.complete_onboarding("client").unwrap();
    // A fresh process (same config dir) sees it completed.
    let again = commands(dir.path(), fake, None).onboarding_state();
    assert!(again.completed);
    assert_eq!(again.mode, Some(OnboardingMode::Client));
    assert!(dir.path().join("config/onboarding.json").is_file());
}

#[tokio::test]
async fn host_flow_setup_stop_resume_remove() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Arc::new(FakeHost::default());
    let cmds = commands(dir.path(), fake.clone(), None);
    assert!(!cmds.status().await.configured);

    let request: HostSetupRequest = serde_json::from_value(serde_json::json!({
        "mode": "relay", "name": " Studio ", "allow": ["grace@example.com", " "]
    }))
    .unwrap();
    let status = cmds.setup(request).await.unwrap();
    assert!(status.configured && status.sharing);
    let json = serde_json::to_value(&status).unwrap();
    // camelCase contract shape for the webview.
    assert_eq!(json["machineId"], "0123abcd4567");
    assert_eq!(json["clients"][0]["email"], "grace@example.com");
    assert_eq!(json["service"]["running"], true);

    let stopped = cmds.stop_sharing().await.unwrap();
    assert!(!stopped.sharing);
    assert!(stopped.clients.is_empty());
    assert!(cmds.start_sharing().await.unwrap().sharing);
    cmds.remove().await.unwrap();
    assert!(!cmds.status().await.configured);
    assert_eq!(
        *fake.calls.lock().unwrap(),
        vec![
            "setup relay None None Some([\"grace@example.com\"])".to_string(),
            "stop".into(),
            "start".into(),
            "remove".into()
        ]
    );
}

#[tokio::test]
async fn direct_mode_is_validated_before_the_backend_runs() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Arc::new(FakeHost::default());
    let cmds = commands(dir.path(), fake.clone(), None);
    let bad = HostSetupRequest {
        mode: "direct".into(),
        direct: Some("not-an-address".into()),
        ..Default::default()
    };
    assert!(cmds.setup(bad).await.unwrap_err().contains("ip:port"));
    assert!(fake.calls.lock().unwrap().is_empty());
    let ok = HostSetupRequest {
        mode: "direct".into(),
        direct: Some("192.168.1.20:3211".into()),
        relay_url: Some("https://ignored".into()),
        ..Default::default()
    };
    let status = cmds.setup(ok).await.unwrap();
    assert_eq!(
        status.direct_url.as_deref(),
        Some("http://192.168.1.20:3211")
    );
    assert_eq!(status.relay_url, None);
}

#[tokio::test]
async fn an_unreachable_backend_reads_as_not_configured_with_the_reason() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Arc::new(FakeHost {
        fail_status: true,
        ..Default::default()
    });
    let status = commands(dir.path(), fake, None).status().await;
    assert!(!status.configured);
    assert_eq!(status.error.as_deref(), Some("relay unreachable"));
}

#[test]
fn host_status_converts_from_any_contract_shaped_status() {
    #[derive(serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Other {
        configured: bool,
        machine_id: String,
        sharing: bool,
        extra_field: u8,
    }
    let view = HostStatusView::from_serializable(&Other {
        configured: true,
        machine_id: "m".into(),
        sharing: true,
        extra_field: 1,
    })
    .unwrap();
    assert!(view.configured && view.sharing);
    assert_eq!(view.machine_id.as_deref(), Some("m"));
}

/// The real `cua-host` backend the app wires in, against cua-host's fake
/// relay and fake service manager (temp home; nothing is installed).
#[tokio::test]
async fn cua_host_backend_runs_the_relay_flow_end_to_end() {
    use cua_host::service::FakeServiceManager;
    use cua_host::testing::FakeRelay;
    use cua_spaces_lib::host_backend::CuaHostBackend;

    let relay = FakeRelay::start().await;
    relay.add_account("acct-token", "user-1", Some("ada@example.com"));
    let dir = tempfile::tempdir().unwrap();
    let driver = dir.path().join("cua-spacesd");
    std::fs::write(&driver, b"#!/bin/sh\n").unwrap();
    let manager = Arc::new(FakeServiceManager::default());
    let host = cua_host::Host::new(dir.path().join(".cua")).with_service_manager(manager.clone());
    let backend = CuaHostBackend::new(host, Arc::new(cua_host::StaticToken("acct-token".into())))
        .with_driver_bin(&driver);
    let cmds = HostCommands::new(
        Arc::new(backend),
        OnboardingStore::in_dir(&dir.path().join("config")),
        None,
    );

    assert!(!cmds.status().await.configured);
    let status = cmds
        .setup(HostSetupRequest {
            mode: "relay".into(),
            relay_url: Some(relay.url.clone()),
            name: Some("Studio".into()),
            allow: Some(vec!["grace@example.com".into()]),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(status.configured && status.sharing, "{status:?}");
    assert_eq!(status.mode.as_deref(), Some("relay"));
    assert_eq!(status.name.as_deref(), Some("Studio"));
    let id = status.machine_id.clone().expect("machine id");
    assert!(relay.machine(&id).is_some(), "registered with the relay");

    // Presence: a client connected through the relay shows on This machine.
    relay.set_online(&id, true, "0.1.0");
    relay.add_client(
        &id,
        cua_host::ConnectedClient {
            id: "user-2".into(),
            email: Some("grace@example.com".into()),
            name: None,
            streams: 2,
            since: 1,
        },
    );
    let status = cmds.status().await;
    assert_eq!(status.online, Some(true));
    assert_eq!(status.clients.len(), 1);
    assert_eq!(
        status.clients[0].email.as_deref(),
        Some("grace@example.com")
    );

    // The two settings: providing Spaces, then the desktop off (a spare
    // machine). Every change is in the Spaces activity.
    assert!(status.share_desktop && !status.provide_spaces);
    let spare = cmds
        .configure(HostSettingChange {
            provide_spaces: Some(true),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(
        spare.provide_spaces && spare.max_macos_vms == 2,
        "{spare:?}"
    );
    let spare = cmds
        .configure(HostSettingChange {
            share_desktop: Some(false),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(!spare.share_desktop && spare.provide_spaces);
    assert_eq!(spare.spaces_audit[0].action, "config");
    let spec = manager.spec.lock().unwrap().clone().unwrap();
    assert!(
        spec.args.iter().any(|a| a == "--no-desktop"),
        "{:?}",
        spec.args
    );
    assert!(cmds.configure(HostSettingChange::default()).await.is_err());

    let stopped = cmds.stop_sharing().await.unwrap();
    assert!(!stopped.sharing);
    assert!(!relay.machine(&id).unwrap().sharing);
    assert!(cmds.start_sharing().await.unwrap().sharing);
    cmds.remove().await.unwrap();
    assert!(!cmds.status().await.configured);
    assert!(relay.machine(&id).is_none(), "removed from the relay");
    let calls = manager.calls.lock().unwrap().clone();
    assert!(
        calls.contains(&"install".to_string()) && calls.contains(&"uninstall".to_string()),
        "{calls:?}"
    );
}

#[tokio::test]
async fn cua_host_backend_relay_setup_without_sign_in_is_refused() {
    use cua_host::service::FakeServiceManager;
    use cua_spaces_lib::host_backend::CuaHostBackend;
    let dir = tempfile::tempdir().unwrap();
    let driver = dir.path().join("cua-spacesd");
    std::fs::write(&driver, b"#!/bin/sh\n").unwrap();
    let manager = Arc::new(FakeServiceManager::default());
    let host = cua_host::Host::new(dir.path().join(".cua")).with_service_manager(manager.clone());
    let backend = CuaHostBackend::new(host, Arc::new(cua_host::NoAccount)).with_driver_bin(&driver);
    let cmds = HostCommands::new(Arc::new(backend), OnboardingStore::in_dir(dir.path()), None);
    let error = cmds
        .setup(HostSetupRequest {
            mode: "relay".into(),
            relay_url: Some("http://127.0.0.1:9".into()),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(!error.is_empty());
    assert!(
        manager.calls.lock().unwrap().is_empty(),
        "nothing installed"
    );
}

/// Relay machines of the signed-in account appear in `list_spaces` as
/// `relay:<id>` with no manual add.
#[tokio::test]
async fn relay_machines_of_the_account_appear_in_the_roster() {
    use cua_host::testing::FakeRelay;
    use cua_spaces_lib::core::{AppCore, CoreConfig};

    let relay = FakeRelay::start().await;
    relay.add_account("acct-token", "user-1", Some("ada@example.com"));
    let client = cua_host::RelayClient::new(&relay.url).unwrap();
    client
        .register(
            "acct-token",
            &cua_host::relay::RegisterRequest {
                id: "0123abcd4567".into(),
                name: "office-pc".into(),
                allow: vec![],
                host: None,
                meta: Default::default(),
            },
        )
        .await
        .unwrap();
    relay.set_online("0123abcd4567", true, "0.1.0");

    let home = tempfile::tempdir().unwrap();
    let mut cfg = CoreConfig::hermetic(home.path());
    cfg.probe_timeout = std::time::Duration::from_secs(2);
    cfg.relay = Some(cua_spaces::RelayAccount::new(
        relay.url.clone(),
        Arc::new(cua_host::StaticToken("acct-token".into())),
    ));
    let app = AppCore::new(cfg);
    let rows = app.list_spaces().await.unwrap();
    let row = rows
        .iter()
        .find(|r| r.id == "relay:0123abcd4567")
        .unwrap_or_else(|| panic!("relay machine missing: {rows:?}"));
    assert_eq!(row.provider, cua_spaces::Provider::Relay);
    assert_eq!(row.name, "office-pc");
    let json = serde_json::to_value(row).unwrap();
    assert_eq!(json["provider"], "relay");
    // Nothing was written to the shared registry: relay machines come from
    // the directory each time.
    let reg = std::fs::read_to_string(home.path().join("spaces.json")).unwrap_or_default();
    assert!(!reg.contains("0123abcd4567"));
}
