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
use cua_keyvault::model::ItemPayload;
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
        _payloads: Vec<ItemPayload>,
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
