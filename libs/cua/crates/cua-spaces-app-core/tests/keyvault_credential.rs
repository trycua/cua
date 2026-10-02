// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Setup and unlock through the shells' broker client against a real broker
//! configured like a development daemon (no OS key store): the page offers
//! a passphrase, an OS setup fails without a Touch ID prompt, and the
//! passphrase sets up, locks and unlocks the vault.
#![cfg(feature = "keyvault-client")]

use std::sync::Arc;

use cua_keyvault::broker::{
    Backend, BrokerConfig, Captured, DeliveryOutcome, FakePresence, ImportSpec, Inventory,
};
use cua_keyvault::model::PayloadEntry;
use cua_keyvault::{Broker, CallerIdentity, Zeroizing};
use cua_spaces_app_core::keyvault::client::{DirectTransport, KeyvaultCommands};
use cua_spaces_app_core::keyvault::credential::{KvFormMode, KvMethod};
use cua_spaces_app_core::keyvault::view;

struct NoBackend;

#[async_trait::async_trait]
impl Backend for NoBackend {
    fn inventory(&self, _app: &str, _profile: Option<&str>) -> cua_keyvault::Result<Inventory> {
        Err(cua_keyvault::Error::Unsupported("test".into()))
    }
    fn capture(&self, _spec: &ImportSpec) -> cua_keyvault::Result<Vec<Captured>> {
        Err(cua_keyvault::Error::Unsupported("test".into()))
    }
    async fn deliver(
        &self,
        _target: &str,
        _provider_id: &str,
        _scope: &str,
        _entries: Vec<PayloadEntry>,
        _expires_ms: u64,
    ) -> cua_keyvault::Result<DeliveryOutcome> {
        Err(cua_keyvault::Error::Unsupported("test".into()))
    }
    async fn wipe(&self, _target: &str, _import_id: &str) -> cua_keyvault::Result<Vec<String>> {
        Ok(vec![])
    }
}

fn secret(s: &str) -> Zeroizing<String> {
    Zeroizing::new(s.to_string())
}

#[tokio::test]
async fn a_development_daemon_sets_up_and_unlocks_with_a_passphrase() {
    let dir = tempfile::tempdir().unwrap();
    let presence = Arc::new(FakePresence::new(true));
    let broker = Arc::new(
        Broker::new(
            BrokerConfig {
                dir: dir.path().join("keyvault"),
                keychain_path: Some(dir.path().join("never-created.keychain")),
                os_protector: false,
            },
            Arc::new(NoBackend),
            presence.clone(),
        )
        .unwrap(),
    );
    let page = KeyvaultCommands::new(Arc::new(DirectTransport {
        broker,
        caller: CallerIdentity::for_tests("com.trycua.spaces.macos", true),
    }));

    let o = page.overview().await;
    assert_eq!(o.availability, "no_vault");
    let form = view::page(&o, 0).form.expect("a setup form");
    assert_eq!(
        (form.mode, form.method),
        (KvFormMode::Setup, KvMethod::Passphrase)
    );

    // The reported bug: an OS setup is refused before any Touch ID prompt.
    let f = page.setup(true).await.unwrap_err();
    assert_eq!(f.code, "unsupported", "{f:?}");
    assert!(presence.asked.lock().unwrap().is_empty());

    let f = page
        .setup_with_passphrase(secret("too short"))
        .await
        .unwrap_err();
    assert_eq!(f.code, "invalid");
    assert!(presence.asked.lock().unwrap().is_empty());

    let key = page
        .setup_with_passphrase(secret("orbit lantern pickle harbor"))
        .await
        .unwrap()
        .expect("the recovery key, once");
    assert_eq!(key.len(), 47);
    assert_eq!(presence.asked.lock().unwrap().len(), 1);
    assert_eq!(page.overview().await.availability, "ready");

    page.lock().await.unwrap();
    let o = page.overview().await;
    assert_eq!(o.availability, "locked");
    let form = view::page(&o, 0).form.expect("an unlock form");
    assert_eq!(
        (form.mode, form.method),
        (KvFormMode::Unlock, KvMethod::Passphrase)
    );
    let f = page
        .unlock_with_passphrase(secret("not the passphrase"))
        .await
        .unwrap_err();
    assert_eq!(f.code, "invalid");
    assert!(!f.message.contains("not the passphrase"));
    page.unlock_with_passphrase(secret("orbit lantern pickle harbor"))
        .await
        .unwrap();
    assert_eq!(page.overview().await.availability, "ready");
}

/// A backend that captures a small fixed profile and records what is
/// delivered and wiped.
#[derive(Default)]
struct Profile {
    delivered: std::sync::Mutex<Vec<(String, Vec<String>)>>,
    wiped: std::sync::Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl Backend for Profile {
    fn inventory(&self, app: &str, _profile: Option<&str>) -> cua_keyvault::Result<Inventory> {
        Ok(Inventory {
            provider_id: app.into(),
            app_display: "Google Chrome".into(),
            domains: vec![cua_keyvault::broker::DomainInventory {
                domain: "github.com".into(),
                cookies: 2,
                signin: true,
                ..Default::default()
            }],
            ..Default::default()
        })
    }
    fn capture(&self, spec: &ImportSpec) -> cua_keyvault::Result<Vec<Captured>> {
        use cua_keyvault::record;
        let mut out = Vec::new();
        for host in [".github.com", "api.github.com", ".notion.so"] {
            let n = record::cookie_record(&record::CookieRecord {
                creation_utc: None,
                expires_utc: 0,
                host_key: host.into(),
                http_only: true,
                last_update_utc: None,
                name: "session".into(),
                partition_key: None,
                last_access_utc: None,
                source_type: None,
                has_cross_site_ancestor: None,
                path: "/".into(),
                priority: None,
                same_site: 1,
                secure: true,
                source_port: None,
                source_scheme: None,
                value: b"cookie-value-SECRET".to_vec(),
            })?;
            let (meta, payload) = n.into_item(&spec.app, "Google Chrome", "Default", "full");
            out.push(Captured { meta, payload });
        }
        let n = record::file_record("Default/Bookmarks", 0o600, b"{}")?;
        let (meta, payload) = n.into_item(&spec.app, "Google Chrome", "Default", "full");
        out.push(Captured { meta, payload });
        Ok(out)
    }
    async fn deliver(
        &self,
        target: &str,
        _provider_id: &str,
        _scope: &str,
        entries: Vec<PayloadEntry>,
        _expires_ms: u64,
    ) -> cua_keyvault::Result<DeliveryOutcome> {
        let mut d = self.delivered.lock().unwrap();
        d.push((
            target.into(),
            entries.iter().map(|e| e.rel_path.clone()).collect(),
        ));
        Ok(DeliveryOutcome {
            import_id: format!("imp-{}", d.len()),
            ..Default::default()
        })
    }
    async fn wipe(&self, _target: &str, import_id: &str) -> cua_keyvault::Result<Vec<String>> {
        self.wiped.lock().unwrap().push(import_id.into());
        Ok(vec![])
    }
}

/// The page, end to end through the shells' client over a real broker: names
/// stay hidden until the browse window opens, the list groups by app and
/// site, items lock and unlock in one batch, "Never ask again" is stored,
/// delete wipes the live copy, and no value is ever in what the page sees.
#[tokio::test]
async fn the_vault_page_end_to_end_through_the_client() {
    use cua_spaces_app_core::keyvault::KvCommand;
    use cua_spaces_app_core::keyvault::client::KvOutcome;
    use cua_spaces_app_core::keyvault::vault::{self, VaultAction, VaultState};

    let dir = tempfile::tempdir().unwrap();
    let presence = Arc::new(FakePresence::new(true));
    let backend = Arc::new(Profile::default());
    let broker = Arc::new(
        Broker::new(
            BrokerConfig {
                dir: dir.path().join("keyvault"),
                keychain_path: None,
                os_protector: false,
            },
            backend.clone(),
            presence.clone(),
        )
        .unwrap(),
    );
    let cua = CallerIdentity::for_tests("com.trycua.spaces.macos", true);
    let page = KeyvaultCommands::new(Arc::new(DirectTransport {
        broker: broker.clone(),
        caller: cua.clone(),
    }));
    page.setup_with_passphrase(secret("orbit lantern pickle harbor"))
        .await
        .unwrap();
    broker
        .import(
            &cua,
            ImportSpec {
                app: "chrome".into(),
                whole_app: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();

    // Names are hidden until the user confirms with Touch ID.
    let o = page.overview().await;
    assert_eq!(o.items.len(), 4);
    assert!(!o.names_visible);
    let v = vault::view(&o, &VaultState::default(), 0);
    assert!(v.names_hidden);
    assert_eq!(v.apps[0].count, 4, "counts survive");
    let asked = presence.asked.lock().unwrap().len();
    let KvOutcome::Browsing { until_ms } = page.execute(&KvCommand::Browse).await.unwrap() else {
        panic!("browsing")
    };
    assert!(until_ms > 0);
    assert_eq!(presence.asked.lock().unwrap().len(), asked + 1);
    let o = page.overview().await;
    assert!(o.names_visible);
    let st = VaultState::default();
    let v = vault::view(&o, &st, 0);
    assert!(!v.names_hidden);
    assert_eq!(v.apps.len(), 1);
    assert_eq!(v.apps[0].name, "Google Chrome");
    assert_eq!(
        v.apps[0]
            .sites
            .iter()
            .map(|s| (s.site.as_str(), s.count))
            .collect::<Vec<_>>(),
        [("github.com", 2), ("notion.so", 1)]
    );
    assert_eq!(v.apps[0].files.as_ref().unwrap().count, 1);
    // Nothing the page was handed carries a value.
    assert!(
        !serde_json::to_string(&o)
            .unwrap()
            .contains("cookie-value-SECRET")
    );

    // Select github.com and unlock it: one presence for the batch.
    let st = vault::reduce(
        &o,
        &st,
        &VaultAction::ToggleGroup {
            key: "chrome|github.com".into(),
        },
    );
    let sel = vault::view(&o, &st, 0).selection;
    assert_eq!(sel.unlock_ids.len(), 2);
    let asked = presence.asked.lock().unwrap().len();
    let KvOutcome::Locked { changed, skipped } = page
        .execute(&vault::unlock_command(&sel.unlock_ids))
        .await
        .unwrap()
    else {
        panic!("locked")
    };
    assert_eq!((changed.len(), skipped.len()), (2, 0));
    assert_eq!(
        presence.asked.lock().unwrap().len(),
        asked + 1,
        "one Touch ID for the batch"
    );
    let o = page.overview().await;
    let v = vault::view(&o, &VaultState::default(), 0);
    assert_eq!(v.apps[0].sites[0].lock, vault::KvLock::Unlocked);
    assert_eq!(v.apps[0].sites[1].lock, vault::KvLock::Locked);
    assert_eq!(v.apps[0].summary, "4 items, 2 unlocked");

    // "Never ask again" is stored in the vault and visible to the page.
    assert!(vault::unlock_prompt(&o, 1, None).is_some());
    page.execute(&KvCommand::SetSkipUnlockPrompt { on: true })
        .await
        .unwrap();
    let o = page.overview().await;
    assert!(vault::unlock_prompt(&o, 1, None).is_none());
    assert!(view::page(&o, 0).skip_unlock_prompt);
    page.execute(&KvCommand::SetSkipUnlockPrompt { on: false })
        .await
        .unwrap();
    assert!(vault::unlock_prompt(&page.overview().await, 1, None).is_some());

    // Teleport github.com's items, then delete them: the copy is wiped.
    broker
        .teleport(
            &cua,
            cua_keyvault::broker::TeleportRequest {
                token: None,
                items: sel.unlock_ids.clone(),
                target: "dev-1".into(),
                include_passwords: false,
                launch: false,
            },
        )
        .await
        .unwrap();
    assert_eq!(backend.delivered.lock().unwrap()[0].1, ["cookies.json"]);
    let o = page.overview().await;
    assert_eq!(
        vault::live_copy_spaces(&o, &sel.unlock_ids, cua_keyvault::now_ms() as i64),
        1
    );
    let KvOutcome::Deleted { count, wiped } = page
        .execute(&vault::delete_command(&sel.unlock_ids))
        .await
        .unwrap()
    else {
        panic!("deleted")
    };
    assert_eq!((count, wiped.len()), (2, 1));
    assert_eq!(backend.wiped.lock().unwrap().len(), 1);
    assert_eq!(page.overview().await.items.len(), 2);

    // The inventory is a name map: it is shown over the open window, and
    // never carries a value.
    let inv = page.inventory("chrome", None).await.unwrap();
    assert_eq!(inv.domains[0].domain, "github.com");
}
