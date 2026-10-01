// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A fake Keyvault backend and a rig for the default-policy and site-login
//! tests. Nothing touches the machine: the vault lives in a temp dir behind a
//! passphrase, presence is a [`FakePresence`], and "filling a login" records
//! what would have been typed.

#![allow(dead_code)]

use std::sync::{Arc, Mutex};

use cua_keyvault::CallerIdentity;
use cua_keyvault::broker::{
    Backend, Broker, BrokerConfig, Captured, DeliveryOutcome, FakePresence, ImportSpec,
    InitRequest, Inventory, LoginFill, LoginFilled, PasswordImportSpec,
};
use cua_keyvault::model::{
    ItemKind, ItemMeta, ItemPayload, ItemPolicy, ItemSummary, LoginRecord, PayloadEntry,
};

pub const PASSWORD: &str = "s3cret-Pa55";

/// What a fill would have typed: (target, url, origin, username, password).
pub type Fill = (String, String, String, String, String);

#[derive(Default)]
pub struct FakeBackend {
    pub fills: Mutex<Vec<Fill>>,
    pub delivered: Mutex<Vec<(String, Vec<String>)>>,
    /// Refuse the fill (a page on the wrong origin).
    pub refuse_fill: Mutex<bool>,
}

pub fn logins_item(site: &str, logins: &[(&str, &str, &str)]) -> Captured {
    let records: Vec<LoginRecord> = logins
        .iter()
        .map(|(o, u, p)| LoginRecord {
            origin: (*o).into(),
            username: (*u).into(),
            password: (*p).into(),
        })
        .collect();
    Captured {
        meta: ItemMeta {
            id: String::new(),
            kind: ItemKind::SitePasswords,
            label: format!("{site} passwords (chrome)"),
            provider_id: "chrome".into(),
            app_display: "Chrome".into(),
            site: Some(site.into()),
            account: None,
            source: "Default".into(),
            summary: ItemSummary {
                passwords: records.len() as u32,
                ..Default::default()
            },
            warnings: vec![],
            identity_provider: false,
            policy: ItemPolicy::default(),
            created_ms: 0,
            updated_ms: 0,
            rev: 0,
            record_digest: String::new(),
        },
        payload: ItemPayload {
            provider_id: "chrome".into(),
            scope: "full".into(),
            entries: vec![ItemPayload::logins_entry(&records).unwrap()],
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
        // A browser-site item carrying one cookie bundle entry.
        Ok(spec
            .sites
            .iter()
            .map(|s| {
                let mut c = logins_item(&s.site, &[]);
                c.meta.kind = ItemKind::BrowserSite;
                c.payload.entries = vec![PayloadEntry {
                    rel_path: format!("cookies/{}", s.site),
                    mode: 0o600,
                    data: "Q09PS0lF".into(),
                }];
                c
            })
            .collect())
    }

    fn capture_passwords(&self, spec: &PasswordImportSpec) -> cua_keyvault::Result<Vec<Captured>> {
        let all = [
            (
                "example.test",
                vec![(
                    "http://login.example.test:8000",
                    "ada@example.test",
                    PASSWORD,
                )],
            ),
            (
                "github.com",
                vec![("https://github.com", "octo", "gh-pw-1234")],
            ),
        ];
        Ok(all
            .iter()
            .filter(|(site, _)| spec.sites.is_empty() || spec.sites.iter().any(|s| s == site))
            .map(|(site, l)| logins_item(site, l))
            .collect())
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
        self.delivered
            .lock()
            .unwrap()
            .push((target.into(), paths.clone()));
        Ok(DeliveryOutcome {
            import_id: "imp".into(),
            imported: paths,
            ..Default::default()
        })
    }

    async fn wipe(&self, _target: &str, _import_id: &str) -> cua_keyvault::Result<Vec<String>> {
        Ok(vec![])
    }

    async fn fill_login(&self, target: &str, f: &LoginFill) -> cua_keyvault::Result<LoginFilled> {
        if *self.refuse_fill.lock().unwrap() {
            return Err(cua_keyvault::Error::Forbidden(format!(
                "the tab is not on {}; refusing to type a password there",
                f.origin
            )));
        }
        self.fills.lock().unwrap().push((
            target.into(),
            f.url.clone(),
            f.origin.clone(),
            f.username.clone(),
            f.password.to_string(),
        ));
        Ok(LoginFilled {
            submitted: true,
            page_url: f.url.clone(),
            browser: f.browser.clone(),
        })
    }
}

pub struct Rig {
    pub _dir: tempfile::TempDir,
    pub broker: Broker,
    pub backend: Arc<FakeBackend>,
    pub presence: Arc<FakePresence>,
    /// The Cua app (first party).
    pub cua: CallerIdentity,
    /// A third party (the daemon's MCP caller is one).
    pub agent: CallerIdentity,
}

pub async fn rig() -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(FakeBackend::default());
    let presence = Arc::new(FakePresence::new(true));
    let broker = Broker::new(
        BrokerConfig {
            dir: dir.path().join("keyvault"),
            keychain_path: None,
            os_protector: false,
        },
        backend.clone(),
        presence.clone(),
    )
    .unwrap();
    let cua = CallerIdentity::for_tests("com.trycua.cua", true);
    let agent = CallerIdentity::for_tests("com.example.agent", false);
    broker
        .init(
            &cua,
            InitRequest {
                os_protector: false,
                passphrase: Some("correct horse battery".into()),
                recovery_key: false,
            },
        )
        .await
        .unwrap();
    Rig {
        _dir: dir,
        broker,
        backend,
        presence,
        cua,
        agent,
    }
}

/// Imports the fake browser's saved passwords (first party, presence yes).
pub async fn import_passwords(r: &Rig) -> Vec<ItemMeta> {
    r.broker
        .import_passwords(
            &r.cua,
            PasswordImportSpec {
                app: "chrome".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap()
}
