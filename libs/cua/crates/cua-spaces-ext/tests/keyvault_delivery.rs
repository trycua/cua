// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The Keyvault broker, hosted with the daemon's real delivery `Backend`,
//! against an in-process mock spacesd. It proves that teleport delivery is
//! broker-mediated: a delivery runs only behind a granted capability, the
//! session reaches the target over the authenticated channel, it is wiped on
//! release, the capability token never travels with it, a capability for a
//! different target is refused, and a tripped kill switch delivers nothing.
//!
//! Host-safe: the vault lives in a temp dir with a passphrase protector, the
//! session is a fixed in-memory blob from a deterministic export provider (no
//! real profile, Keychain or biometric prompt), and the target is a mock.
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
use cua_spacesd_client::testing::{MockAuth, MockServer};
use cua_teleport::bundle::BundleWriter;
use cua_teleport::{
    AppRef, ExportProvider, ExportRegistry, FakeHost, HostEffects, ManifestItem, Platform,
    TransferManifest, TransferScope, WindowRef,
};

const SECRET: &[u8] = b"SEKRIT-github-session-cookie-value";
const TOKEN_MOCK: &str = "t";

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
    mock: Arc<MockServer>,
    spaces: Spaces,
    broker: Arc<Broker>,
    cua: CallerIdentity,
    koala: CallerIdentity,
    target: String,
}

async fn rig() -> Rig {
    let mock = Arc::new(
        MockServer::start(MockAuth {
            token: Some(TOKEN_MOCK.into()),
            ..Default::default()
        })
        .await,
    );
    let reg = tempfile::tempdir().unwrap();
    let spaces = Spaces::builder()
        .home(reg.path())
        .operator_display(Arc::new(cua_spaces::operator::NoDisplay))
        .probe_timeout(Duration::from_secs(20))
        .build();
    let info = spaces
        .add(&mock.url(), Some(TOKEN_MOCK.into()), Some("target".into()))
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
        mock,
        spaces,
        broker,
        cua,
        koala,
        target: info.id,
    }
}

/// Third party requests the chrome app into the target; the user
/// approves; the returned token authorizes exactly one delivery.
async fn grant(r: &Rig, target: &str) -> (String, Vec<String>) {
    let pending = r
        .broker
        .request_access(
            &r.koala,
            AccessRequest {
                selectors: vec![Selector::App {
                    app: "chrome".into(),
                }],
                targets: vec![target.into()],
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
    match r
        .broker
        .await_decision(&r.koala, &pending.id, Duration::from_millis(50))
        .await
        .unwrap()
    {
        Decision::Granted { token, items, .. } => (token, items),
        other => panic!("expected a grant, got {other:?}"),
    }
}

fn carries(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[tokio::test]
async fn broker_delivers_a_granted_capability_and_wipes_on_release() {
    let r = rig().await;
    let (token, items) = grant(&r, &r.target).await;
    assert!(!items.is_empty(), "approval imported the site");

    let outcome = r
        .broker
        .teleport(
            &r.koala,
            TeleportRequest {
                token: Some(token.clone()),
                items: items.clone(),
                target: r.target.clone(),
                include_passwords: false,
                launch: false,
            },
        )
        .await
        .expect("a granted capability delivers");
    assert_eq!(outcome.deliveries.len(), 1);
    // Auto-wipe is off by default: the copy stays until it is wiped.
    assert_eq!(outcome.expires_ms, 0, "no expiry by default");

    let imports = r.mock.state.teleport_imports();
    assert_eq!(imports.len(), 1, "the target received exactly one import");
    let import = &imports[0];
    assert!(
        carries(&import.bundle, SECRET),
        "the session reached the target"
    );
    assert_eq!(
        import.options.expires_at_ms, 0,
        "the receiver got no expiry, so only a wipe removes it"
    );
    // The capability token never travels with the delivery (not in argv-like
    // fields, not in the bundle): the broker delivers over its own channel.
    assert_eq!(import.app, "chrome");
    assert!(!import.app.contains(&token));
    assert!(
        !carries(&import.bundle, token.as_bytes()),
        "the token is never delivered to the target"
    );

    // Release wipes the delivered copy on the target.
    let wiped = r.broker.release(&r.cua, &r.target).await.unwrap();
    assert!(!wiped.is_empty(), "release reported a wipe");
    let wipes = r.mock.state.teleport_wipes();
    assert_eq!(wipes.len(), 1);
    assert_eq!(wipes[0].import_id, import.import_id);
    assert!(
        r.mock.state.teleport_imports().is_empty(),
        "the target no longer holds the import"
    );
}

#[tokio::test]
async fn a_capability_for_a_different_target_is_refused() {
    let r = rig().await;
    // A genuinely different Space (its own mock, so a distinct immutable id).
    let other_mock = MockServer::start(MockAuth {
        token: Some(TOKEN_MOCK.into()),
        ..Default::default()
    })
    .await;
    let other = r
        .spaces
        .add(
            &other_mock.url(),
            Some(TOKEN_MOCK.into()),
            Some("other".into()),
        )
        .await
        .expect("second space")
        .id;
    assert_ne!(other, r.target, "the two Spaces have distinct ids");
    let (token, items) = grant(&r, &r.target).await;

    // The token is scoped to `target`; a delivery to another Space's id fails
    // closed and moves nothing (red-team F1: the grant binds to the id).
    let err = r
        .broker
        .teleport(
            &r.koala,
            TeleportRequest {
                token: Some(token),
                items,
                target: other,
                include_passwords: false,
                launch: false,
            },
        )
        .await
        .expect_err("a capability for another target is refused");
    let _ = err;
    assert!(
        other_mock.state.teleport_imports().is_empty(),
        "nothing was delivered to the wrong target"
    );
}

#[tokio::test]
async fn a_tripped_kill_switch_denies_delivery() {
    let r = rig().await;
    let (token, items) = grant(&r, &r.target).await;
    r.broker.set_disabled(&r.cua, true).await.unwrap();

    let err = r
        .broker
        .teleport(
            &r.koala,
            TeleportRequest {
                token: Some(token),
                items,
                target: r.target.clone(),
                include_passwords: false,
                launch: false,
            },
        )
        .await
        .expect_err("the kill switch denies delivery");
    assert!(matches!(err, cua_keyvault::Error::Disabled));
    assert!(
        r.mock.state.teleport_imports().is_empty(),
        "nothing was delivered while disabled"
    );
}

/// A direct teleport through the daemon's backend reports where it is:
/// reading (the Keychain moment on macOS), packing, the upload's bytes and
/// the import, in that order; with auto-wipe off the copy has no expiry.
#[tokio::test]
async fn a_direct_teleport_reports_its_stages_through_the_daemon_backend() {
    use cua_keyvault::broker::{ImportSpec, TeleportStage};
    let r = rig().await;
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = {
        let seen = seen.clone();
        Arc::new(move |s: TeleportStage| seen.lock().unwrap().push(s))
    };
    let outcome = r
        .broker
        .import_and_teleport_with_progress(
            &r.cua,
            ImportSpec {
                app: "chrome".into(),
                whole_app: true,
                ..Default::default()
            },
            r.target.clone(),
            false,
            Some(sink),
        )
        .await
        .expect("a first-party direct teleport delivers");
    assert_eq!(outcome.expires_ms, 0, "auto-wipe is off by default");
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.first(), Some(&TeleportStage::Reading), "{seen:?}");
    let packing = seen
        .iter()
        .position(|s| *s == TeleportStage::Packing)
        .expect("packs");
    let importing = seen
        .iter()
        .position(|s| *s == TeleportStage::Importing)
        .expect("imports");
    assert!(packing < importing, "{seen:?}");
    assert!(
        seen[packing..importing]
            .iter()
            .any(|s| matches!(s, TeleportStage::Uploading { total, .. } if *total > 0)),
        "the upload reports its bytes: {seen:?}"
    );
    assert!(
        !seen.contains(&TeleportStage::Saving),
        "save: false saves nothing"
    );
    let imports = r.mock.state.teleport_imports();
    assert_eq!(imports.len(), 1);
    assert_eq!(imports[0].options.expires_at_ms, 0);
}
