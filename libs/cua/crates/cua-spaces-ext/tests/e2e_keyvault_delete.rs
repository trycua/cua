// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Live e2e of deleting Keyvault items (#4887) against a real cua-spacesd:
//! the real `DaemonBackend` and broker deliver a session, then delete the
//! items with the Space present, after the Space was removed, and after the
//! target forgot the import. Skipped unless `CUA_SPACES_E2E_URL` (and
//! `CUA_SPACES_E2E_TOKEN`) point at a spacesd with a throwaway HOME.
#![cfg(unix)]

use std::collections::HashSet;
use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

use cua_keyvault::CallerIdentity;
use cua_keyvault::broker::{
    AccessRequest, ApproveOptions, Broker, Decision, FakePresence, InitRequest, Selector,
    TeleportRequest,
};
use cua_spaces::Spaces;
use cua_spaces_ext::daemon::keyvault::{DaemonBackend, Keyvault};
use cua_spaces_ext::teleport::AppSessions;
use cua_teleport::bundle::BundleWriter;
use cua_teleport::{
    AppRef, ExportProvider, ExportRegistry, FakeHost, HostEffects, ManifestItem, Platform,
    TransferManifest, TransferScope, WindowRef,
};

const SECRET: &[u8] = b"SEKRIT-github-session-cookie-value";

/// A deterministic provider that exports one fixed, non-sensitive blob as the
/// `chrome` session, so capture never reads a real profile or prompts.
struct FixedProvider {
    host: Arc<dyn HostEffects>,
}

impl ExportProvider for FixedProvider {
    fn id(&self) -> &str {
        "chrome"
    }
    fn display_name(&self) -> &str {
        "Fixed Chrome (test)"
    }
    fn host(&self) -> &dyn HostEffects {
        self.host.as_ref()
    }
    fn platform_supported(&self, _platform: Platform) -> bool {
        true
    }
    fn matches(&self, app: &AppRef) -> bool {
        app.app_id == "chrome"
    }
    fn app_ids(&self) -> &[&str] {
        &["chrome"]
    }
    fn manifest(
        &self,
        _app: &AppRef,
        _window: Option<&WindowRef>,
        scope: TransferScope,
    ) -> cua_teleport::Result<TransferManifest> {
        Ok(TransferManifest {
            provider_id: "chrome".into(),
            app_display_name: "Fixed Chrome (test)".into(),
            scope,
            items: vec![ManifestItem {
                label: "cookies".into(),
                rel_path: "Default/Cookies".into(),
                est_bytes: SECRET.len() as u64,
                count: None,
                count_noun: None,
                // Not sensitive: the biometric gate never fires in the test.
                sensitive: false,
                default_checked: true,
            }],
            total_est_bytes: SECRET.len() as u64,
            notes: vec![],
        })
    }
    fn capture_selected(
        &self,
        _app: &AppRef,
        scope: TransferScope,
        _include: Option<&HashSet<String>>,
        out: &mut dyn Write,
    ) -> cua_teleport::Result<()> {
        let mut writer = BundleWriter::new(out, "chrome", "Fixed Chrome (test)", scope);
        writer.add_bytes("Default/Cookies", 0o600, SECRET)?;
        writer.finish()?;
        Ok(())
    }
}

struct Rig {
    _reg: tempfile::TempDir,
    _vault: tempfile::TempDir,
    spaces: Spaces,
    broker: Arc<Broker>,
    cua: CallerIdentity,
    koala: CallerIdentity,
    url: String,
    token: String,
    target: std::sync::Mutex<String>,
}

fn env() -> Option<(String, String)> {
    let url = std::env::var("CUA_SPACES_E2E_URL")
        .ok()
        .filter(|s| !s.is_empty())?;
    Some((
        url,
        std::env::var("CUA_SPACES_E2E_TOKEN").unwrap_or_default(),
    ))
}

async fn rig(url: String, token: String) -> Rig {
    let reg = tempfile::tempdir().unwrap();
    let spaces = Spaces::builder()
        .home(reg.path())
        .operator_display(Arc::new(cua_spaces::operator::NoDisplay))
        .probe_timeout(Duration::from_secs(20))
        .build();
    let info = spaces
        .add(&url, Some(token.clone()), Some("target".into()))
        .await
        .unwrap();
    let mut registry = ExportRegistry::new();
    registry.register(Box::new(FixedProvider {
        host: Arc::new(FakeHost::new()),
    }));
    let sessions = Arc::new(AppSessions::from_registry(registry));
    let backend = Arc::new(DaemonBackend::new(spaces.clone(), sessions));
    let presence = Arc::new(FakePresence::new(true));
    let vault = tempfile::tempdir().unwrap();
    let kv = Keyvault::new(vault.path().join("keyvault"), backend, presence, false).unwrap();
    let broker = kv.broker();
    let cua = CallerIdentity::for_tests("com.trycua.cua", true);
    let koala = CallerIdentity::for_tests("com.example.koalabot", false);
    broker
        .init(
            &cua,
            InitRequest {
                os_protector: false,
                passphrase: Some("correct horse battery".into()),
                recovery_key: true,
            },
        )
        .await
        .unwrap();
    Rig {
        _reg: reg,
        _vault: vault,
        spaces,
        broker,
        cua,
        koala,
        url,
        token,
        target: std::sync::Mutex::new(info.id),
    }
}

/// Grants chrome into the target and delivers it; returns the item ids.
async fn deliver(r: &Rig) -> Vec<String> {
    let target = r.target.lock().unwrap().clone();
    let pending = r
        .broker
        .request_access(
            &r.koala,
            AccessRequest {
                selectors: vec![Selector::App {
                    app: "chrome".into(),
                }],
                targets: vec![target.clone()],
                uses: Some(0),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    r.broker
        .approve(&r.cua, &pending.id, ApproveOptions::default())
        .await
        .unwrap();
    let (token, items) = match r
        .broker
        .await_decision(&r.koala, &pending.id, Duration::from_millis(50))
        .await
        .unwrap()
    {
        Decision::Granted { token, items, .. } => (token, items),
        other => panic!("expected a grant, got {other:?}"),
    };
    r.broker
        .teleport(
            &r.koala,
            TeleportRequest {
                token: Some(token),
                items: items.clone(),
                target,
                include_passwords: false,
                launch: false,
            },
        )
        .await
        .expect("delivery to the real spacesd");
    items
}

#[tokio::test]
async fn e2e_items_delivered_to_a_gone_space_or_import_can_be_deleted() {
    let Some((url, token)) = env() else {
        eprintln!("skipped: set CUA_SPACES_E2E_URL / CUA_SPACES_E2E_TOKEN");
        return;
    };
    let r = rig(url, token).await;

    // 1. The Space exists: deleting wipes the delivered copy (as before).
    let items = deliver(&r).await;
    r.broker.delete_items(&r.cua, items).await.unwrap();
    eprintln!("case 1 (Space present): deleted");

    // 2. The target forgot the import (expired, or a re-created Space).
    let items = deliver(&r).await;
    let deliveries = r.broker.list_deliveries(&r.cua).await.unwrap();
    let live = deliveries.iter().find(|d| !d.wiped).unwrap();
    let space = r.spaces.space(&live.target).await.unwrap();
    space
        .spacesd()
        .unwrap()
        .teleport()
        .wipe_import(cua_spacesd_client::pb::WipeImportRequest {
            import_id: live.import_id.clone(),
            all: false,
        })
        .await
        .unwrap();
    r.broker.delete_items(&r.cua, items).await.unwrap();
    eprintln!("case 2 (import gone): deleted");

    // 3. The Space was removed (the reporter's steps 1 to 3).
    let items = deliver(&r).await;
    let id = r.target.lock().unwrap().clone();
    r.spaces.remove(&id).await.unwrap();
    r.broker.delete_items(&r.cua, items).await.unwrap();
    eprintln!("case 3 (Space removed): deleted");

    // 4. A same-name Space re-created afterwards does not block it either.
    let items = deliver_after_readd(&r).await;
    r.broker.delete_items(&r.cua, items).await.unwrap();
    eprintln!("case 4 (re-created Space): deleted");
}

async fn deliver_after_readd(r: &Rig) -> Vec<String> {
    let info = r
        .spaces
        .add(&r.url, Some(r.token.clone()), Some("target".into()))
        .await
        .unwrap();
    *r.target.lock().unwrap() = info.id.clone();
    let items = deliver(r).await;
    r.spaces.remove(&info.id).await.unwrap();
    let info = r
        .spaces
        .add(&r.url, Some(r.token.clone()), Some("target".into()))
        .await
        .unwrap();
    *r.target.lock().unwrap() = info.id;
    items
}
