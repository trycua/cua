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
use cua_keyvault::model::{ItemKind, ItemMeta, LoginRecord, PayloadEntry};
use cua_keyvault::record::{self, CookieRecord};

pub const PASSWORD: &str = "s3cret-Pa55";

/// What a fill would have typed: (target, url, origin, username, password).
pub type Fill = (String, String, String, String, String);

#[derive(Default)]
pub struct FakeBackend {
    pub fills: Mutex<Vec<Fill>>,
    pub delivered: Mutex<Vec<(String, Vec<String>)>>,
    /// Refuse the fill (a page on the wrong origin).
    pub refuse_fill: Mutex<bool>,
    /// (target, import id) of every wipe.
    pub wiped: Mutex<Vec<(String, String)>>,
    /// Bumped to make the next capture read different values.
    pub generation: Mutex<u32>,
    /// The cookie hosts each delivery carried.
    pub delivered_hosts: Mutex<Vec<Vec<String>>>,
    /// The icons the source browser's local store holds (site, png bytes).
    pub icons: Mutex<Vec<(String, Vec<u8>)>>,
}

fn captured(n: record::NewRecord, app: &str) -> Captured {
    let (meta, payload) = n.into_item(app, "Chrome", "Default", "full");
    Captured { meta, payload }
}

/// One captured Password item per login (the vault keeps one secret per
/// item, keyed by origin and username).
pub fn logins_items(logins: &[(&str, &str, &str)]) -> Vec<Captured> {
    logins
        .iter()
        .map(|(o, u, p)| {
            captured(
                record::password_record(&LoginRecord {
                    origin: (*o).into(),
                    username: (*u).into(),
                    password: (*p).into(),
                })
                .unwrap(),
                "chrome",
            )
        })
        .collect()
}

pub fn meta(kind: ItemKind, domain: Option<&str>, key: &str) -> ItemMeta {
    ItemMeta::draft(kind, "chrome", "Chrome", domain, key)
}

/// A captured cookie (host `domain`, root path).
pub fn cookie(domain: &str, name: &str, value: &str) -> Captured {
    captured(
        record::cookie_record(&CookieRecord {
            creation_utc: None,
            expires_utc: 0,
            host_key: domain.into(),
            http_only: true,
            last_update_utc: None,
            name: name.into(),
            partition_key: None,
            last_access_utc: None,
            source_type: None,
            has_cross_site_ancestor: None,
            path: "/".into(),
            priority: None,
            same_site: -1,
            secure: true,
            source_port: None,
            source_scheme: None,
            value: value.as_bytes().to_vec(),
        })
        .unwrap(),
        "chrome",
    )
}

/// A captured file.
pub fn file(path: &str, contents: &str) -> Captured {
    captured(
        record::file_record(path, 0o600, contents.as_bytes()).unwrap(),
        "chrome",
    )
}

#[async_trait::async_trait]
impl Backend for FakeBackend {
    fn favicons(
        &self,
        _app: &str,
        _profile: Option<&str>,
        sites: &[String],
    ) -> cua_keyvault::Result<Vec<(String, Vec<u8>)>> {
        Ok(self
            .icons
            .lock()
            .unwrap()
            .iter()
            .filter(|(s, _)| sites.contains(s))
            .cloned()
            .collect())
    }

    fn inventory(&self, app: &str, _profile: Option<&str>) -> cua_keyvault::Result<Inventory> {
        Ok(Inventory {
            provider_id: app.into(),
            app_display: app.into(),
            ..Default::default()
        })
    }

    fn capture(&self, spec: &ImportSpec) -> cua_keyvault::Result<Vec<Captured>> {
        // Two cookies per site (a session and a csrf token), and the files
        // the spec's paths name (a browser's bookmarks and local state by
        // default). A domain filter keeps only those sites.
        let wanted = |d: &str| {
            spec.domains
                .as_ref()
                .is_none_or(|ds| ds.iter().any(|x| x == d))
        };
        let mut out = Vec::new();
        let gen_ = *self.generation.lock().unwrap();
        for s in spec.sites.iter().filter(|s| wanted(&s.site)) {
            let host = format!(".{}", s.site);
            out.push(cookie(&host, "session", &format!("sess-{}-{gen_}", s.site)));
            out.push(cookie(&host, "csrf", &format!("csrf-{}-{gen_}", s.site)));
        }
        if spec.whole_app {
            let paths = spec
                .paths
                .clone()
                .unwrap_or_else(|| vec!["Default/Bookmarks".into(), "Local State".into()]);
            for p in paths {
                out.push(file(&p, &format!("contents of {p}")));
            }
        }
        for c in &mut out {
            c.meta.provider_id = spec.app.clone();
            c.payload.provider_id = spec.app.clone();
        }
        Ok(out)
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
            .flat_map(|(_, l)| logins_items(l))
            .collect())
    }

    async fn deliver(
        &self,
        target: &str,
        _provider_id: &str,
        _scope: &str,
        entries: Vec<PayloadEntry>,
        _expires_ms: u64,
    ) -> cua_keyvault::Result<DeliveryOutcome> {
        use base64::Engine as _;
        let paths: Vec<String> = entries.iter().map(|e| e.rel_path.clone()).collect();
        let hosts: Vec<String> = entries
            .iter()
            .filter(|e| e.rel_path == record::COOKIES_ENTRY)
            .flat_map(|e| {
                let raw = base64::engine::general_purpose::STANDARD
                    .decode(&e.data)
                    .unwrap();
                let rows: Vec<serde_json::Value> = serde_json::from_slice(&raw).unwrap();
                rows.iter()
                    .map(|r| r["host_key"].as_str().unwrap().to_string())
                    .collect::<Vec<_>>()
            })
            .collect();
        self.delivered_hosts.lock().unwrap().push(hosts);
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

/// Every item with its names visible (opens the browse window first).
pub async fn all_items(r: &Rig) -> Vec<ItemMeta> {
    let page = r.broker.list_items(&r.cua, 0, 1000).await.unwrap();
    if page.names_visible {
        return page.items;
    }
    r.broker.browse(&r.cua).await.unwrap();
    r.broker.list_items(&r.cua, 0, 1000).await.unwrap().items
}

/// Imports the fake browser's saved passwords (first party, presence yes)
/// and returns the password items.
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
        .unwrap();
    all_items(r)
        .await
        .into_iter()
        .filter(|i| i.kind == ItemKind::Password)
        .collect()
}
