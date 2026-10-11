// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The daemon's capture backend turns what a provider exports into the
//! vault's per-secret records: cookies from the decrypted rows, files for the
//! rest, a domain choice filtering cookies, consent-gated files held back
//! unless the user picked them, and the inventory listing domains from the
//! plaintext columns of the cookie store (so no Keychain prompt).
//!
//! Host-safe: a deterministic in-memory provider and a `FakeHost`; nothing
//! reads a real profile.
#![cfg(unix)]

use std::collections::HashSet;
use std::io::Write;
use std::sync::Arc;

use cua_keyvault::ItemKind;
use cua_keyvault::broker::{Backend, CookieFilter, ImportSpec};
use cua_keyvault::record::{COOKIES_ENTRY, CookieRecord};
use cua_spaces::Spaces;
use cua_spaces_ext::daemon::keyvault::DaemonBackend;
use cua_spaces_ext::teleport::AppSessions;
use cua_teleport::bundle::BundleWriter;
use cua_teleport::{
    AppRef, ExportProvider, ExportRegistry, FakeHost, HostEffects, ManifestItem, Platform,
    TransferManifest, TransferScope, WindowRef,
};

/// A browser that exports decrypted cookie rows for three sites, two files
/// and a consent-gated `Web Data`.
struct Browser {
    host: Arc<dyn HostEffects>,
}

const PATHS: [&str; 4] = [
    "Default/Bookmarks",
    "Default/Preferences",
    "Default/Web Data",
    "Default/Cookies",
];

impl ExportProvider for Browser {
    fn id(&self) -> &str {
        "chrome"
    }
    fn display_name(&self) -> &str {
        "Google Chrome"
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
            app_display_name: "Google Chrome".into(),
            scope,
            items: PATHS
                .iter()
                .map(|p| ManifestItem {
                    label: (*p).into(),
                    rel_path: (*p).into(),
                    est_bytes: 10,
                    count: None,
                    count_noun: None,
                    sensitive: false,
                    default_checked: true,
                })
                .collect(),
            total_est_bytes: 40,
            notes: vec![],
        })
    }
    fn capture_selected(
        &self,
        _app: &AppRef,
        scope: TransferScope,
        include: Option<&HashSet<String>>,
        out: &mut dyn Write,
    ) -> cua_teleport::Result<()> {
        let wants = |p: &str| include.is_none_or(|i| i.contains(p));
        let mut w = BundleWriter::new(out, "chrome", "Google Chrome", scope);
        for p in [
            "Default/Bookmarks",
            "Default/Preferences",
            "Default/Web Data",
        ] {
            if wants(p) {
                w.add_bytes(p, 0o600, format!("bytes of {p}").as_bytes())?;
            }
        }
        if wants("Default/Cookies") {
            let rows = serde_json::json!([
                row(".github.com", "user_session", "/", 0),
                row("api.github.com", "_gh_sess", "/", 0),
                row(".notion.so", "token_v2", "/", 13_397_000_000_000_000_i64),
                row(".doubleclick.net", "IDE", "/", 13_397_000_000_000_000_i64),
                row(".example.test", "pref", "/", 13_700_000_000_000_000_i64),
            ]);
            w.add_bytes(COOKIES_ENTRY, 0o600, rows.to_string().as_bytes())?;
        }
        w.finish()?;
        Ok(())
    }
}

fn row(host: &str, name: &str, path: &str, expires: i64) -> serde_json::Value {
    use base64::Engine as _;
    serde_json::json!({
        "host_key": host, "name": name, "path": path, "expires_utc": expires,
        "value": base64::engine::general_purpose::STANDARD.encode(format!("v-{name}")),
        "is_secure": true, "is_httponly": true, "samesite": 1
    })
}

fn backend() -> (DaemonBackend, tempfile::TempDir) {
    let reg = tempfile::tempdir().unwrap();
    let spaces = Spaces::builder()
        .home(reg.path())
        .operator_display(Arc::new(cua_spaces::operator::NoDisplay))
        .build();
    let mut registry = ExportRegistry::new();
    registry.register(Box::new(Browser {
        host: Arc::new(FakeHost::new()),
    }));
    let sessions = Arc::new(AppSessions::from_registry(registry));
    (DaemonBackend::new(spaces, sessions), reg)
}

fn spec(paths: &[&str]) -> ImportSpec {
    ImportSpec {
        app: "chrome".into(),
        whole_app: true,
        paths: Some(paths.iter().map(|s| s.to_string()).collect()),
        ..Default::default()
    }
}

fn cookies(captured: &[cua_keyvault::broker::Captured]) -> Vec<(String, String)> {
    captured
        .iter()
        .filter(|c| c.meta.kind == ItemKind::Cookie)
        .map(|c| (c.meta.domain.clone().unwrap(), c.meta.key.clone()))
        .collect()
}

#[test]
fn a_browser_capture_is_one_item_per_cookie_and_file() {
    let (b, _reg) = backend();
    let out = b
        .capture(&spec(&[
            "Default/Bookmarks",
            "Default/Preferences",
            "Default/Cookies",
        ]))
        .unwrap();
    assert_eq!(out.len(), 7, "five cookies and two files");
    assert_eq!(
        cookies(&out)[0],
        (".github.com".to_string(), "user_session".to_string())
    );
    let files: Vec<&str> = out
        .iter()
        .filter(|c| c.meta.kind == ItemKind::File)
        .map(|c| c.meta.key.as_str())
        .collect();
    assert_eq!(files, ["Default/Bookmarks", "Default/Preferences"]);
    // Every item names its app and carries a versioned record, and a cookie
    // keeps its attributes (the path is part of its key).
    for c in &out {
        assert_eq!(c.meta.provider_id, "chrome");
        assert_eq!(c.meta.app_display, "Google Chrome");
    }
    let first = out
        .iter()
        .find(|c| c.meta.kind == ItemKind::Cookie)
        .unwrap();
    assert_eq!(first.payload.schema, "cookie@1");
    assert_eq!(first.meta.path.as_deref(), Some("/"));
    let rec: CookieRecord = serde_json::from_str(&first.payload.record).unwrap();
    assert!(rec.secure && rec.http_only && rec.same_site == 1);
    assert!(first.meta.session, "no expiry is a session cookie");
    // The metadata names no value.
    assert!(
        !serde_json::to_string(&first.meta)
            .unwrap()
            .contains("v-user_session")
    );
}

#[test]
fn a_domain_choice_filters_cookies_by_site_and_leaves_files_alone() {
    let (b, _reg) = backend();
    let mut s = spec(&["Default/Bookmarks", "Default/Cookies"]);
    s.domains = Some(vec!["github.com".into(), "notion.so".into()]);
    let out = b.capture(&s).unwrap();
    assert_eq!(
        cookies(&out),
        [
            (".github.com".into(), "user_session".into()),
            ("api.github.com".into(), "_gh_sess".into()),
            (".notion.so".into(), "token_v2".into()),
        ],
        "api.github.com belongs to github.com; the tracker and the other site are gone"
    );
    assert_eq!(
        out.iter().filter(|c| c.meta.kind == ItemKind::File).count(),
        1
    );
    // An empty choice sends no cookies at all.
    s.domains = Some(vec![]);
    assert!(cookies(&b.capture(&s).unwrap()).is_empty());
    // Cookie minimization still applies: session cookies only.
    let mut s = spec(&["Default/Cookies"]);
    s.cookies = CookieFilter {
        session_only: true,
        drop_long_lived: false,
    };
    assert_eq!(cookies(&b.capture(&s).unwrap()).len(), 2);
}

#[test]
fn consent_gated_files_need_the_user_to_pick_them() {
    let (b, _reg) = backend();
    // `Web Data` holds sign-in refresh tokens: in the provider's default
    // selection it is not exported...
    let out = b.capture(&ImportSpec {
        app: "chrome".into(),
        whole_app: true,
        paths: None,
        ..Default::default()
    });
    let keys: Vec<String> = out.unwrap().iter().map(|c| c.meta.key.clone()).collect();
    assert!(!keys.iter().any(|k| k.ends_with("Web Data")), "{keys:?}");
    // ...and when the user explicitly picks it, it is.
    let out = b.capture(&spec(&["Default/Web Data"])).unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].meta.key, "Default/Web Data");
}

#[test]
fn the_inventory_counts_cookies_per_site_without_decrypting_anything() {
    use cua_teleport::browser_cookies::{TestCookieRow, write_cookies_db_for_tests};
    let home = tempfile::tempdir().unwrap();
    let profile = home
        .path()
        .join("Library/Application Support/Google/Chrome/Default");
    let row = |host: &'static str, name: &'static str, expires: i64| TestCookieRow {
        host_key: host,
        name,
        // Not a valid ciphertext: the inventory must never try to read it.
        encrypted_value: b"v10-not-decryptable".to_vec(),
        path: "/",
        expires_utc: expires,
        is_secure: true,
        is_httponly: true,
        samesite: 1,
    };
    write_cookies_db_for_tests(
        &profile,
        &[
            row(".github.com", "user_session", 0),
            row("api.github.com", "_gh_sess", 0),
            row(".github.com", "_ga", 13_700_000_000_000_000),
            row(".doubleclick.net", "IDE", 13_700_000_000_000_000),
            row(".accounts.google.com", "SID", 13_700_000_000_000_000),
        ],
    )
    .unwrap();
    let reg = tempfile::tempdir().unwrap();
    let spaces = Spaces::builder()
        .home(reg.path())
        .operator_display(Arc::new(cua_spaces::operator::NoDisplay))
        .build();
    let host: Arc<dyn HostEffects> = Arc::new(FakeHost::new().with_home(home.path()));
    let mut registry = ExportRegistry::new();
    registry.register(Box::new(Browser { host: host.clone() }));
    let b = DaemonBackend::new(spaces, Arc::new(AppSessions::from_registry(registry)))
        .with_host(host)
        .with_platform(Platform::MacOS);
    let inv = b.inventory("chrome", None).unwrap();
    assert_eq!(inv.app_display, "Google Chrome");
    let by = |d: &str| inv.domains.iter().find(|x| x.domain == d).unwrap().clone();
    let github = by("github.com");
    assert_eq!((github.cookies, github.session_cookies), (3, 2));
    assert!(github.signin, "user_session is a sign-in cookie");
    assert!(!by("doubleclick.net").signin, "a tracker keeps no sign-in");
    assert!(by("google.com").identity_provider);
    assert!(
        inv.notes.iter().all(|n| !n.contains("not listed")),
        "{:?}",
        inv.notes
    );
}
