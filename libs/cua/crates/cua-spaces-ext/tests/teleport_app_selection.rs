// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `teleport_app` moves what the caller selected and nothing else.
//!
//! The real `teleport_app` tool, the real Keyvault broker and the daemon's
//! delivery `Backend` run against an in-process mock spacesd, with a fixed
//! export provider that offers three items. The vault already holds a stored
//! session for the app: a request for one non-sensitive item used to be
//! filed as "the whole app", and approving it delivered the stored session
//! instead of the selection.
//!
//! Host-safe: the vault lives in a temp dir with a passphrase protector, the
//! session is fixed in-memory bytes (no real profile, Keychain or biometric
//! prompt), and the target is a mock.
#![cfg(unix)]

use std::collections::HashSet;
use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

use cua_keyvault::CallerIdentity;
use cua_keyvault::broker::{ApproveOptions, Broker, FakePresence, ImportSpec, InitRequest};
use cua_spaces::Spaces;
use cua_spaces::extension::{SpacesExtension, ToolCall};
use cua_spaces_ext::TeleportExtension;
use cua_spaces_ext::daemon::keyvault::{DaemonBackend, Keyvault};
use cua_spaces_ext::teleport::AppSessions;
use cua_spacesd_client::testing::{MockAuth, MockServer};
use cua_teleport::bundle::BundleWriter;
use cua_teleport::{
    AppRef, ExportProvider, ExportRegistry, FakeHost, HostEffects, ManifestItem, Platform,
    TransferManifest, TransferScope, WindowRef,
};
use serde_json::{Value, json};

const TOKEN_MOCK: &str = "t";
const PREFS: &[u8] = b"PREFS-CONTENT-aaaa";
const NOTES: &[u8] = b"NOTES-CONTENT-bbbb";
const SESSION: &[u8] = b"SESSION-SECRET-cccc";

const FILES: [(&str, &[u8]); 3] = [
    ("fixture/prefs.json", PREFS),
    ("fixture/notes.txt", NOTES),
    ("fixture/session.json", SESSION),
];

/// A provider with a harmless item, another harmless one, and a sensitive
/// session item that is checked by default (like a single-app provider's
/// signed-in state). Capture writes only what it was asked to include.
struct FixtureProvider {
    host: Arc<dyn HostEffects>,
}

impl ExportProvider for FixtureProvider {
    fn id(&self) -> &str {
        "fixture"
    }
    fn display_name(&self) -> &str {
        "Fixture"
    }
    fn host(&self) -> &dyn HostEffects {
        self.host.as_ref()
    }
    fn platform_supported(&self, _platform: Platform) -> bool {
        true
    }
    fn matches(&self, app: &AppRef) -> bool {
        app.app_id == "fixture"
    }
    fn app_ids(&self) -> &[&str] {
        &["fixture"]
    }
    fn manifest(
        &self,
        _app: &AppRef,
        _window: Option<&WindowRef>,
        scope: TransferScope,
    ) -> cua_teleport::Result<TransferManifest> {
        let item =
            |label: &str, rel: &str, bytes: &[u8], sensitive: bool, checked: bool| ManifestItem {
                label: label.into(),
                rel_path: rel.into(),
                est_bytes: bytes.len() as u64,
                count: None,
                count_noun: None,
                sensitive,
                default_checked: checked,
            };
        Ok(TransferManifest {
            provider_id: "fixture".into(),
            app_display_name: "Fixture".into(),
            scope,
            items: vec![
                item("Preferences", FILES[0].0, PREFS, false, true),
                item("Notes", FILES[1].0, NOTES, false, true),
                item("Signed-in session", FILES[2].0, SESSION, true, true),
            ],
            total_est_bytes: (PREFS.len() + NOTES.len() + SESSION.len()) as u64,
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
        let mut writer = BundleWriter::new(out, "fixture", "Fixture", scope);
        for (rel, bytes) in FILES {
            if include.is_none_or(|set| set.contains(rel)) {
                writer.add_bytes(rel, 0o600, bytes)?;
            }
        }
        writer.finish()?;
        Ok(())
    }
}

struct Rig {
    _reg: tempfile::TempDir,
    _vault: tempfile::TempDir,
    mock: Arc<MockServer>,
    spaces: Spaces,
    tool: TeleportExtension,
    kv: Keyvault,
    broker: Arc<Broker>,
    cua: CallerIdentity,
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
    spaces
        .add(&mock.url(), Some(TOKEN_MOCK.into()), Some("target".into()))
        .await
        .unwrap();

    let mut registry = ExportRegistry::new();
    registry.register(Box::new(FixtureProvider {
        host: Arc::new(FakeHost::new()),
    }));
    let sessions = Arc::new(AppSessions::from_registry(registry));
    let backend = Arc::new(DaemonBackend::new(spaces.clone(), sessions.clone()));
    let presence = Arc::new(FakePresence::new(true));
    let vault = tempfile::tempdir().unwrap();
    let kv = Keyvault::new(vault.path().join("keyvault"), backend, presence, false).unwrap();
    let broker = kv.broker();
    let cua = CallerIdentity::for_tests("com.trycua.cua", true);
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
    // The user has already saved this app's signed-in session in the vault
    // (the normal state after using Teleport once).
    broker
        .import(
            &cua,
            ImportSpec {
                app: "fixture".into(),
                whole_app: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    Rig {
        _reg: reg,
        _vault: vault,
        mock,
        spaces,
        tool: TeleportExtension::new(Some(sessions)),
        kv,
        broker,
        cua,
    }
}

impl Rig {
    /// One `teleport_app` call as the MCP tool serves it.
    async fn teleport_app(&self, args: Value) -> cua_spaces::Result<Value> {
        let broker = self.kv.session_broker();
        let out = self
            .tool
            .call_tool(ToolCall {
                spaces: &self.spaces,
                tool: "teleport_app",
                args,
                broker: Some(&broker),
                pinned: None,
                caller: Default::default(),
            })
            .await?;
        Ok(out.structured.unwrap_or_else(|| reply_json(&out.content)))
    }

    /// The user approves `request_id` in Cua, with the default options (as the
    /// app and the CLI do).
    async fn approve(&self, request_id: &str) {
        self.broker
            .approve(&self.cua, request_id, ApproveOptions::default())
            .await
            .unwrap();
    }
}

/// The JSON a tool answered in its text content.
fn reply_json(content: &[Value]) -> Value {
    let text = content
        .iter()
        .filter_map(|c| c.get("text").and_then(Value::as_str))
        .collect::<String>();
    serde_json::from_str(&text).expect("the reply is JSON")
}

fn carries(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn would_send_paths(reply: &Value) -> Vec<String> {
    reply["would_send"]["items"]
        .as_array()
        .expect("would_send lists items")
        .iter()
        .map(|i| i["relative_path"].as_str().unwrap().to_string())
        .collect()
}

/// A request for one harmless item moves that item, and neither the stored
/// session nor the other items. The retry carries only the `request_id`,
/// as the SDK's does.
#[tokio::test]
async fn a_narrow_selection_delivers_only_the_selected_items() {
    let r = rig().await;
    let filed = r
        .teleport_app(json!({
            "space": "target",
            "app": "fixture",
            "include": ["fixture/prefs.json"],
            "acknowledge_sensitive": false,
        }))
        .await
        .unwrap();
    assert_eq!(filed["consent_required"], true, "{filed}");
    // The reply says what will move: only the selection.
    assert_eq!(would_send_paths(&filed), ["fixture/prefs.json"]);
    assert_eq!(filed["selected_paths"], json!(["fixture/prefs.json"]));
    let id = filed["request_id"]
        .as_str()
        .expect("request id")
        .to_string();
    // The request on file is for that selection, and not for the stored
    // session the vault already holds.
    let pending = r.broker.list_pending(&r.cua).await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(
        pending[0].request.paths.as_deref(),
        Some(&["fixture/prefs.json".to_string()][..])
    );
    assert!(pending[0].items.is_empty(), "{:?}", pending[0].items);
    assert!(
        r.mock.state.teleport_imports().is_empty(),
        "nothing moved yet"
    );

    r.approve(&id).await;
    let moved = r
        .teleport_app(json!({"space": "target", "app": "fixture", "request_id": id}))
        .await
        .unwrap();
    assert_eq!(moved["moved"], true, "{moved}");

    let imports = r.mock.state.teleport_imports();
    assert_eq!(imports.len(), 1);
    let bundle = &imports[0].bundle;
    assert!(carries(bundle, PREFS), "the selected item moved");
    assert!(!carries(bundle, SESSION), "the stored session did not move");
    assert!(!carries(bundle, NOTES), "an unselected item did not move");
}

/// A sensitive item is refused unless acknowledged, before any request is
/// filed; acknowledged, exactly that item moves.
#[tokio::test]
async fn a_sensitive_selection_needs_the_acknowledgement_and_is_refused_before_filing() {
    let r = rig().await;
    for args in [
        json!({"space": "target", "app": "fixture", "include": ["fixture/session.json"]}),
        json!({"space": "target", "app": "fixture", "include": ["fixture/session.json"],
               "acknowledge_sensitive": false}),
        // The default-checked set holds the signed-in session.
        json!({"space": "target", "app": "fixture"}),
    ] {
        let err = r.teleport_app(args).await.unwrap_err();
        assert!(err.to_string().contains("sensitive"), "{err}");
    }
    assert!(
        r.broker.list_pending(&r.cua).await.unwrap().is_empty(),
        "no request was filed"
    );

    let filed = r
        .teleport_app(json!({
            "space": "target",
            "app": "fixture",
            "include": ["fixture/session.json"],
            "acknowledge_sensitive": true,
        }))
        .await
        .unwrap();
    assert_eq!(would_send_paths(&filed), ["fixture/session.json"]);
    let id = filed["request_id"].as_str().unwrap().to_string();
    r.approve(&id).await;
    let moved = r
        .teleport_app(json!({"space": "target", "app": "fixture", "request_id": id}))
        .await
        .unwrap();
    assert_eq!(moved["moved"], true, "{moved}");
    let imports = r.mock.state.teleport_imports();
    assert!(carries(&imports[0].bundle, SESSION));
    assert!(!carries(&imports[0].bundle, PREFS));
    assert!(!carries(&imports[0].bundle, NOTES));
}

/// A path the manifest does not offer, and an empty list, are refused too.
#[tokio::test]
async fn an_unknown_or_empty_selection_files_nothing() {
    let r = rig().await;
    for include in [json!(["fixture/other.json"]), json!([])] {
        let err = r
            .teleport_app(json!({"space": "target", "app": "fixture", "include": include}))
            .await
            .unwrap_err();
        assert_eq!(err.tag(), "teleport_refused", "{err}");
    }
    assert!(r.broker.list_pending(&r.cua).await.unwrap().is_empty());
}
