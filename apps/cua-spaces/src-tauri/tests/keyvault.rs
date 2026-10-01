// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The Keyvault page commands against a real broker.
//!
//! Hermetic: the vault lives in a temp dir with a passphrase protector (no
//! login keychain, no generation anchor in the keychain), user presence is a
//! fake gate, and capture/delivery are a fake backend that returns fixture
//! items. Nothing reads a host app or touches the real machine.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use cua_keyvault::broker::{
    AccessRequest, Backend, Broker, Captured, Decision, DeliveryOutcome, FakePresence, ImportSpec,
    InitRequest, Inventory, Selector, SiteChoice, TeleportRequest,
};
use cua_keyvault::model::{
    CookieInfo, ItemKind, ItemMeta, ItemPayload, ItemPolicy, ItemSummary, PayloadEntry,
};
use cua_keyvault::CallerIdentity;
use cua_spaces_ext::daemon::keyvault::Keyvault;
use cua_spaces_lib::keyvault::{DirectTransport, KeyvaultCommands, SocketTransport};

#[derive(Default)]
struct FakeBackend {
    delivered: Mutex<Vec<(String, Vec<String>)>>,
    wiped: Mutex<Vec<(String, String)>>,
}

fn fixture(app: &str, site: Option<&str>, account: Option<&str>) -> Captured {
    let kind = if site.is_some() {
        ItemKind::BrowserSite
    } else {
        ItemKind::AppSession
    };
    let label = match site {
        Some(s) => format!("{s} ({app}, Default)"),
        None => format!("{app} (whole app session)"),
    };
    Captured {
        meta: ItemMeta {
            id: String::new(),
            kind,
            label,
            provider_id: app.into(),
            app_display: app.into(),
            site: site.map(Into::into),
            account: account.map(Into::into),
            source: "Default".into(),
            summary: ItemSummary {
                cookies: vec![CookieInfo {
                    name: "fixture_session".into(),
                    domain: format!(".{}", site.unwrap_or("example.test")),
                    session: true,
                    expires_ms: None,
                }],
                ..Default::default()
            },
            warnings: vec![],
            identity_provider: site == Some("accounts.example-idp.test"),
            policy: ItemPolicy::default(),
            created_ms: 0,
            updated_ms: 0,
            rev: 0,
            record_digest: String::new(),
        },
        payload: ItemPayload {
            provider_id: app.into(),
            scope: "full".into(),
            entries: vec![PayloadEntry {
                rel_path: format!("fixture/{}", site.unwrap_or(app)),
                mode: 0o600,
                // "FIXTURE-NOT-A-SECRET"
                data: "RklYVFVSRS1OT1QtQS1TRUNSRVQ=".into(),
            }],
        },
    }
}

#[async_trait::async_trait]
impl Backend for FakeBackend {
    fn inventory(&self, app: &str, _profile: Option<&str>) -> cua_keyvault::Result<Inventory> {
        Ok(Inventory {
            provider_id: app.into(),
            app_display: app.into(),
            ..Default::default()
        })
    }

    fn capture(&self, spec: &ImportSpec) -> cua_keyvault::Result<Vec<Captured>> {
        let mut out: Vec<Captured> = spec
            .sites
            .iter()
            .map(|s| fixture(&spec.app, Some(&s.site), Some("ada@example.test")))
            .collect();
        if spec.whole_app {
            out.push(fixture(&spec.app, None, Some("Example Workspace")));
        }
        Ok(out)
    }

    async fn deliver(
        &self,
        target: &str,
        _provider_id: &str,
        payloads: Vec<ItemPayload>,
        _expires_ms: u64,
    ) -> cua_keyvault::Result<DeliveryOutcome> {
        let paths: Vec<String> = payloads
            .iter()
            .flat_map(|p| p.entries.iter().map(|e| e.rel_path.clone()))
            .collect();
        let mut d = self.delivered.lock().unwrap();
        d.push((target.into(), paths.clone()));
        Ok(DeliveryOutcome {
            import_id: format!("imp-{}", d.len()),
            imported: paths,
            ..Default::default()
        })
    }

    async fn wipe(&self, target: &str, import_id: &str) -> cua_keyvault::Result<Vec<String>> {
        self.wiped
            .lock()
            .unwrap()
            .push((target.into(), import_id.into()));
        Ok(vec![])
    }
}

struct Rig {
    _dir: tempfile::TempDir,
    kv: Keyvault,
    broker: Arc<Broker>,
    backend: Arc<FakeBackend>,
    presence: Arc<FakePresence>,
    cua: CallerIdentity,
    koala: CallerIdentity,
    /// The page commands as the signed Cua Spaces app.
    page: KeyvaultCommands,
}

async fn rig() -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(FakeBackend::default());
    let presence = Arc::new(FakePresence::new(true));
    // `os_protector: false`: a passphrase-only vault with a file generation
    // anchor, so the test never reaches the login keychain.
    let kv = Keyvault::new(
        dir.path().join("keyvault"),
        backend.clone(),
        presence.clone(),
        false,
    )
    .unwrap();
    let broker = kv.broker();
    let cua = CallerIdentity::for_tests("com.trycua.spaces.prototype", true);
    let koala = CallerIdentity::for_tests("com.example.koalabot", false);
    broker
        .init(
            &cua,
            InitRequest {
                os_protector: false,
                passphrase: Some("fixture passphrase, not a secret".into()),
                recovery_key: false,
            },
        )
        .await
        .unwrap();
    broker
        .import(
            &cua,
            ImportSpec {
                app: "chrome".into(),
                sites: vec![
                    SiteChoice {
                        site: "github.example.test".into(),
                        include_storage: false,
                        include_passwords: false,
                    },
                    SiteChoice {
                        site: "accounts.example-idp.test".into(),
                        include_storage: false,
                        include_passwords: false,
                    },
                ],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    broker
        .import(
            &cua,
            ImportSpec {
                app: "slack".into(),
                whole_app: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let page = KeyvaultCommands::new(Arc::new(DirectTransport {
        broker: broker.clone(),
        caller: cua.clone(),
    }));
    presence.asked.lock().unwrap().clear();
    Rig {
        _dir: dir,
        kv,
        broker,
        backend,
        presence,
        cua,
        koala,
        page,
    }
}

fn item_id(items: &[cua_spaces_app_core::keyvault::KvItem], label_prefix: &str) -> String {
    items
        .iter()
        .find(|i| i.label.starts_with(label_prefix))
        .unwrap_or_else(|| panic!("no item {label_prefix}"))
        .id
        .clone()
}

/// A grant for `item` to `target`, requested by the third party and approved
/// through the page.
async fn granted_token(r: &Rig, item: &str, target: &str) -> String {
    let pending = r
        .broker
        .request_access(
            &r.koala,
            AccessRequest {
                selectors: vec![Selector::Item { id: item.into() }],
                targets: vec![target.into()],
                reason: "fixture".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    r.page.approve(&pending.id, None).await.unwrap();
    match r
        .broker
        .await_decision(&r.koala, &pending.id, Duration::from_millis(10))
        .await
        .unwrap()
    {
        Decision::Granted { token, .. } => token,
        other => panic!("expected a grant, got {other:?}"),
    }
}

#[tokio::test]
async fn overview_lists_items_with_nothing_selected_by_default() {
    let r = rig().await;
    let o = r.page.overview().await;
    assert_eq!(o.availability, "ready", "{:?}", o.message);
    assert!(o.server_verified);
    assert_eq!(o.items.len(), 3);
    // Nothing is selected by default: no item may run unattended, and no
    // rule, grant, pending request or delivery exists until the user acts.
    assert!(o.items.iter().all(|i| !i.policy.unattended));
    assert!(o.rules.is_empty() && o.grants.is_empty());
    assert!(o.pending.is_empty() && o.deliveries.is_empty());
    // Per site and per account, never values: ListItems is the redacted view.
    let gh = o
        .items
        .iter()
        .find(|i| i.site.as_deref() == Some("github.example.test"))
        .unwrap();
    assert_eq!(gh.account.as_deref(), Some("ada@example.test"));
    assert!(gh
        .summary
        .cookies
        .iter()
        .all(|c| c.name.is_empty() && c.domain.is_empty()));
    let json = serde_json::to_string(&o).unwrap();
    assert!(
        !json.contains("RklYVFVSRS1OT1QtQS1TRUNSRVQ"),
        "payload leaked"
    );
    // The audit log records the imports and the chain verifies.
    assert!(o.audit.iter().any(|e| e.kind == "item.import"));
    assert!(o.audit_verification.as_ref().unwrap().ok);
    assert!(o.partial_errors.is_empty(), "{:?}", o.partial_errors);
}

#[tokio::test]
async fn kill_switch_refuses_teleport_app_and_outstanding_grants() {
    let r = rig().await;
    let items = r.page.overview().await.items;
    let gh = item_id(&items, "github.example.test");
    let token = granted_token(&r, &gh, "dev-1").await;

    // The daemon-hosted `teleport_app` seam files requests while enabled.
    let seam = r.kv.session_broker();
    let before = seam
        .request_access("slack", "dev-1", 900, "fixture")
        .await
        .expect("enabled vault accepts a request");

    // Disable from the page: no presence needed to turn it on.
    r.presence.asked.lock().unwrap().clear();
    r.page.set_disabled(true).await.unwrap();
    assert!(r.presence.asked.lock().unwrap().is_empty());
    let o = r.page.overview().await;
    assert!(o.status.as_ref().unwrap().disabled);
    assert!(o.pending.is_empty(), "disabling drops pending requests");
    assert!(o.audit.iter().any(|e| e.kind == "vault.disable"));

    // `teleport_app` (the MCP seam the daemon routes it through) is refused.
    let err = seam
        .request_access("slack", "dev-1", 900, "fixture")
        .await
        .expect_err("a disabled vault refuses teleport_app");
    assert_eq!(err.code, "disabled");
    // A request filed before the switch cannot be collected either.
    let collected = seam
        .await_and_deliver("slack", "dev-1", &before, Duration::from_millis(10))
        .await;
    assert!(
        !matches!(
            collected,
            Ok(cua_spaces::teleport_broker::SessionDelivery::Delivered { .. })
        ),
        "a disabled vault delivered"
    );

    // A grant issued before the switch no longer delivers anything.
    let refused = r
        .broker
        .teleport(
            &r.koala,
            TeleportRequest {
                token: Some(token),
                items: vec![gh.clone()],
                target: "dev-1".into(),
            },
        )
        .await;
    assert!(refused.is_err(), "outstanding tokens die with the switch");
    assert!(
        r.backend.delivered.lock().unwrap().is_empty(),
        "nothing delivered"
    );

    // Turning it back on asks for presence; a declined prompt keeps it off.
    r.presence.set(false);
    assert!(r.page.set_disabled(false).await.is_err());
    assert!(r.page.overview().await.status.unwrap().disabled);
    r.presence.set(true);
    r.page.set_disabled(false).await.unwrap();
    assert!(!r.page.overview().await.status.unwrap().disabled);
    assert_eq!(r.presence.asked.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn per_item_toggles_narrow_freely_and_widen_only_with_presence() {
    let r = rig().await;
    let items = r.page.overview().await.items;
    let gh = item_id(&items, "github.example.test");
    let slack = item_id(&items, "slack");
    let idp = item_id(&items, "accounts.example-idp.test");

    // Widening (on) goes through the daemon's presence gate.
    r.presence.set(false);
    assert!(r
        .page
        .set_unattended(std::slice::from_ref(&gh), true)
        .await
        .is_err());
    r.presence.set(true);
    let out = r
        .page
        .set_unattended(&[gh.clone(), slack.clone()], true)
        .await
        .unwrap();
    assert!(out.iter().all(|i| i.policy.unattended));
    let asked = r.presence.asked.lock().unwrap().len();
    assert_eq!(asked, 3, "one declined prompt, then one per widened item");

    // Narrowing (off) never prompts, and touches only the named item.
    let out = r
        .page
        .set_unattended(std::slice::from_ref(&gh), false)
        .await
        .unwrap();
    assert!(!out[0].policy.unattended);
    assert_eq!(r.presence.asked.lock().unwrap().len(), asked);
    let o = r.page.overview().await;
    let by_id = |id: &str| o.items.iter().find(|i| i.id == id).unwrap().clone();
    assert!(!by_id(&gh).policy.unattended);
    assert!(by_id(&slack).policy.unattended);
    assert!(o.audit.iter().filter(|e| e.kind == "item.policy").count() >= 3);

    // Identity providers always ask: the page refuses to widen them.
    let err = r.page.set_unattended(&[idp], true).await.unwrap_err();
    assert_eq!(err.code, "forbidden");

    // An empty selection is a no-op.
    assert!(r.page.set_unattended(&[], true).await.unwrap().is_empty());
}

#[tokio::test]
async fn pending_requests_can_be_denied_and_grants_revoked() {
    let r = rig().await;
    let items = r.page.overview().await.items;
    let gh = item_id(&items, "github.example.test");

    let pending = r
        .broker
        .request_access(
            &r.koala,
            AccessRequest {
                selectors: vec![Selector::Item { id: gh.clone() }],
                targets: vec!["dev-1".into()],
                reason: "fixture".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let o = r.page.overview().await;
    assert_eq!(o.pending.len(), 1);
    assert_eq!(o.pending[0].caller_display, r.koala.display());
    r.page.deny(&pending.id).await.unwrap();
    assert!(r.page.overview().await.pending.is_empty());

    let token = granted_token(&r, &gh, "dev-1").await;
    let o = r.page.overview().await;
    assert_eq!(o.grants.len(), 1);
    assert!(o.audit.iter().any(|e| e.kind == "consent.allow"));
    assert!(o.audit.iter().any(|e| e.kind == "consent.deny"));
    assert_eq!(r.page.revoke_grant(&o.grants[0].id).await.unwrap(), 1);
    assert!(r.page.overview().await.grants[0].revoked);
    assert!(r
        .broker
        .teleport(
            &r.koala,
            TeleportRequest {
                token: Some(token),
                items: vec![gh],
                target: "dev-1".into(),
            },
        )
        .await
        .is_err());
    assert!(r.backend.delivered.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_delivery_can_be_wiped_from_the_page() {
    let r = rig().await;
    let items = r.page.overview().await.items;
    let gh = item_id(&items, "github.example.test");
    let token = granted_token(&r, &gh, "dev-1").await;
    r.broker
        .teleport(
            &r.koala,
            TeleportRequest {
                token: Some(token),
                items: vec![gh],
                target: "dev-1".into(),
            },
        )
        .await
        .unwrap();
    let o = r.page.overview().await;
    assert_eq!(o.deliveries.iter().filter(|d| !d.wiped).count(), 1);
    let wiped = r.page.release("dev-1").await.unwrap();
    assert_eq!(wiped.len(), 1);
    assert_eq!(r.backend.wiped.lock().unwrap().len(), 1);
    assert!(r.page.overview().await.deliveries.iter().all(|d| d.wiped));
}

#[tokio::test]
async fn an_unsigned_app_is_told_why_it_sees_nothing() {
    let r = rig().await;
    let page = KeyvaultCommands::new(Arc::new(DirectTransport {
        broker: r.broker.clone(),
        caller: r.koala.clone(),
    }));
    let o = page.overview().await;
    assert_eq!(o.availability, "not_first_party");
    assert!(o.message.unwrap().contains("signed by Cua"));
    assert!(o.items.is_empty());
    // And it cannot flip the kill switch.
    assert_eq!(page.set_disabled(true).await.unwrap_err().code, "forbidden");
    let _ = &r.cua;
}

#[tokio::test]
async fn a_locked_vault_says_so() {
    let r = rig().await;
    r.broker.lock(&r.cua).await.unwrap();
    let o = r.page.overview().await;
    assert_eq!(o.availability, "locked");
    assert!(o.items.is_empty());
}

#[tokio::test]
async fn no_daemon_is_not_running() {
    let dir = tempfile::Builder::new()
        .prefix("kvui")
        .tempdir_in("/tmp")
        .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let page = KeyvaultCommands::new(Arc::new(SocketTransport::at(dir.path().join("kv.sock"))));
    let o = page.overview().await;
    assert_eq!(o.availability, "not_running");
    assert!(o.status.is_none());
    assert_eq!(
        page.set_disabled(true).await.unwrap_err().code,
        "not_running"
    );
}

#[tokio::test]
async fn setup_creates_the_vault_and_returns_the_recovery_key_once() {
    let dir = tempfile::tempdir().unwrap();
    let presence = Arc::new(FakePresence::new(true));
    let kv = Keyvault::new(
        dir.path().join("keyvault"),
        Arc::new(FakeBackend::default()),
        presence.clone(),
        false,
    )
    .unwrap();
    let page = KeyvaultCommands::new(Arc::new(DirectTransport {
        broker: kv.broker(),
        caller: CallerIdentity::for_tests("com.trycua.spaces.prototype", true),
    }));
    assert_eq!(page.overview().await.availability, "no_vault");
    let key = page.setup(false).await.unwrap().expect("a recovery key");
    assert!(key.len() > 20, "{key}");
    assert_eq!(
        presence.asked.lock().unwrap().len(),
        1,
        "setup asks for presence"
    );
    let o = page.overview().await;
    assert_eq!(o.availability, "ready");
    assert!(o.items.is_empty());
    assert!(
        !serde_json::to_string(&o).unwrap().contains(&key),
        "never shown again"
    );
    // A second setup is refused.
    assert!(page.setup(false).await.is_err());
    // Locked, then unlocked by the OS protector: this vault has none, so the
    // broker says so and the page stays locked.
    kv.broker()
        .lock(&CallerIdentity::for_tests(
            "com.trycua.spaces.prototype",
            true,
        ))
        .await
        .unwrap();
    assert!(page.unlock().await.is_err());
    assert_eq!(page.overview().await.availability, "locked");
}

/// A development daemon (no OS key store): the page offers a passphrase,
/// an OS setup is refused before any Touch ID prompt, and the passphrase
/// sets up and unlocks the vault.
#[tokio::test]
async fn a_passphrase_sets_up_and_unlocks_without_the_key_store() {
    use cua_spaces_app_core::keyvault::client::Zeroizing;
    use cua_spaces_app_core::keyvault::credential::KvMethod;
    let dir = tempfile::tempdir().unwrap();
    let presence = Arc::new(FakePresence::new(true));
    let kv = Keyvault::new(
        dir.path().join("keyvault"),
        Arc::new(FakeBackend::default()),
        presence.clone(),
        false,
    )
    .unwrap();
    let page = KeyvaultCommands::new(Arc::new(DirectTransport {
        broker: kv.broker(),
        caller: CallerIdentity::for_tests("com.trycua.spaces.prototype", true),
    }));
    let o = page.overview().await;
    let form = cua_spaces_app_core::keyvault::view::page(&o, 0)
        .form
        .unwrap();
    assert_eq!(form.method, KvMethod::Passphrase);
    assert_eq!(page.setup(true).await.unwrap_err().code, "unsupported");
    assert!(presence.asked.lock().unwrap().is_empty());
    let pass = "fixture passphrase, not a secret";
    assert!(page
        .setup_with_passphrase(Zeroizing::new(pass.into()))
        .await
        .unwrap()
        .is_some());
    kv.broker()
        .lock(&CallerIdentity::for_tests(
            "com.trycua.spaces.prototype",
            true,
        ))
        .await
        .unwrap();
    assert_eq!(page.overview().await.availability, "locked");
    assert_eq!(
        page.unlock_with_passphrase(Zeroizing::new("wrong passphrase!".into()))
            .await
            .unwrap_err()
            .code,
        "invalid"
    );
    page.unlock_with_passphrase(Zeroizing::new(pass.into()))
        .await
        .unwrap();
    assert_eq!(page.overview().await.availability, "ready");
}
