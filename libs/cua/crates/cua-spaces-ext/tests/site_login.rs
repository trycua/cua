// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `request_site_login` through the daemon's real Keyvault backend and the
//! Spaces MCP server, against an in-process mock spacesd.
//!
//! It proves the wiring end to end on the host side: saved passwords import
//! from a synthetic Chrome profile (decrypted with Chrome's own scheme), the
//! tool files a request and never fills without the user's answer, a denial
//! refuses and types nothing, and an approval reaches the Space's cua-driver
//! (the mock has none, so the fill fails there, audited). The real fill
//! against a real browser is the opt-in Docker e2e
//! (`tests/e2e/run-site-login-e2e.sh`).
//!
//! Host-safe: the vault is a passphrase vault in a temp dir, presence is a
//! fake, the Chrome profile lives in a temp home read through a `FakeHost`
//! (its "Keychain" is a scripted answer), and the target is a mock.
#![cfg(unix)]

use std::sync::Arc;
use std::time::Duration;

use cua_keyvault::CallerIdentity;
use cua_keyvault::broker::{FakePresence, InitRequest, PasswordImportSpec};
use cua_spaces::Spaces;
use cua_spaces::mcp::McpServer;
use cua_spaces_ext::daemon::keyvault::{DaemonBackend, Keyvault};
use cua_spaces_ext::teleport::AppSessions;
use cua_spacesd_client::testing::{MockAuth, MockServer};
use cua_teleport::passwords::{MAC_SAFE_STORAGE, encrypt_for_tests, write_login_data_for_tests};
use cua_teleport::{FakeHost, HostOutput, Platform};
use serde_json::{Value, json};

const PASSWORD: &str = "s3cret-Pa55";
const SAFE_STORAGE: &str = "synthetic-safe-storage";

/// A temp home holding a Chrome profile with one saved login for the test
/// site, encrypted the way Chrome on this platform does, and a FakeHost that
/// answers the Safe Storage Keychain read with a synthetic secret.
fn synthetic_chrome() -> (tempfile::TempDir, Arc<FakeHost>) {
    let home = tempfile::tempdir().unwrap();
    let platform = Platform::current();
    let profile = home
        .path()
        .join(cua_teleport::layout::chrome::user_data_dir_for(platform))
        .join("Default");
    let encrypted = match platform {
        Platform::MacOS => encrypt_for_tests(b"v10", SAFE_STORAGE.as_bytes(), 1003, PASSWORD),
        _ => encrypt_for_tests(b"v10", b"peanuts", 1, PASSWORD),
    };
    write_login_data_for_tests(
        &profile,
        &[(
            "http://login.example.test:8000/",
            "ada@example.test",
            encrypted,
        )],
    )
    .unwrap();
    let host = Arc::new(FakeHost::new().with_home(home.path()).with_responder(|c| {
        assert!(
            c.args.iter().any(|a| a == MAC_SAFE_STORAGE),
            "only the Safe Storage item is read: {c:?}"
        );
        if c.args.iter().any(|a| a == "-w") {
            Ok(HostOutput::ok(format!("{SAFE_STORAGE}\n").into_bytes()))
        } else {
            Ok(HostOutput::ok(b"    \"acct\"<blob>=\"Chrome\"\n".to_vec()))
        }
    }));
    (home, host)
}

struct Rig {
    _reg: tempfile::TempDir,
    _vault: tempfile::TempDir,
    _home: tempfile::TempDir,
    _mock: Arc<MockServer>,
    kv: Keyvault,
    mcp: McpServer,
    cua: CallerIdentity,
    space: String,
}

async fn rig() -> Rig {
    rig_with(None).await
}

/// [`rig`], routed through a relay path prefix when `relay_prefix` names one
/// (relay emulation; see `MockAuth::prefix`), so the Space's
/// [`cua_spacesd_client::EndpointKind`] is `Relay` instead of `Direct`.
async fn rig_with(relay_prefix: Option<&str>) -> Rig {
    let mock = Arc::new(
        MockServer::start(MockAuth {
            token: Some("t".into()),
            prefix: relay_prefix.map(str::to_owned),
            ..Default::default()
        })
        .await,
    );
    // The tool needs the Space's cua-driver registry.
    mock.state.advertise(&["driver"]);
    let reg = tempfile::tempdir().unwrap();
    let spaces = Spaces::builder()
        .home(reg.path())
        .operator_display(Arc::new(cua_spaces::operator::NoDisplay))
        .probe_timeout(Duration::from_secs(20))
        .build();
    let url = match relay_prefix {
        Some(p) => format!("{}{p}", mock.url()),
        None => mock.url(),
    };
    let info = spaces
        .add(&url, Some("t".into()), Some("work".into()))
        .await
        .unwrap();
    let (home, host) = synthetic_chrome();
    let backend = Arc::new(
        DaemonBackend::new(spaces.clone(), Arc::new(AppSessions::builtin())).with_host(host),
    );
    let vault = tempfile::tempdir().unwrap();
    let kv = Keyvault::new(
        vault.path().join("keyvault"),
        backend,
        Arc::new(FakePresence::new(true)),
        false,
    )
    .unwrap();
    spaces.set_site_login_broker(Some(kv.site_login_broker()));
    let cua = CallerIdentity::for_tests("com.trycua.cua", true);
    kv.broker()
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
        _reg: reg,
        _vault: vault,
        _home: home,
        _mock: mock,
        kv,
        mcp: McpServer::new(spaces),
        cua,
        space: info.id,
    }
}

async fn call(r: &Rig, args: Value) -> (bool, Value) {
    let out = r.mcp.call("request_site_login", args).await;
    let v = out.to_result();
    let text = v.to_string();
    assert!(!text.contains(PASSWORD), "the password leaked: {text}");
    (out.is_error, v)
}

/// The structured error, or the JSON of the text content.
fn structured(v: &Value) -> Value {
    if let Some(s) = v.get("structuredContent") {
        return s.clone();
    }
    v["content"][0]["text"]
        .as_str()
        .and_then(|t| serde_json::from_str(t).ok())
        .unwrap_or(Value::Null)
}

#[tokio::test]
async fn import_decrypts_the_synthetic_profile_into_one_sealed_item() {
    let r = rig().await;
    let items =
        r.kv.broker()
            .import_passwords(
                &r.cua,
                PasswordImportSpec {
                    app: "chrome".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
    assert_eq!(items.saved, 1);
    r.kv.broker().browse(&r.cua).await.unwrap();
    let held = r.kv.broker().list_items(&r.cua, 0, 10).await.unwrap().items;
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].kind, cua_keyvault::ItemKind::Password);
    assert_eq!(
        held[0].domain.as_deref(),
        Some("http://login.example.test:8000")
    );
    let shown = serde_json::to_string(&held).unwrap();
    assert!(!shown.contains(PASSWORD));
}

#[tokio::test]
async fn the_tool_asks_first_refuses_on_deny_and_types_only_after_approval() {
    let r = rig().await;
    let broker = r.kv.broker();
    broker
        .import_passwords(
            &r.cua,
            PasswordImportSpec {
                app: "chrome".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let url = "http://login.example.test:8000/";
    // First call: a request, nothing typed.
    let (err, v) = call(&r, json!({"space": r.space, "url": url, "agent": "ada"})).await;
    assert!(!err, "{v}");
    let s = structured(&v);
    assert_eq!(s["status"], "pending", "{v}");
    let id = s["request_id"].as_str().unwrap().to_string();
    let pending = broker.list_pending(&r.cua).await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].request.agent.as_deref(), Some("ada"));
    // Still undecided.
    let (_, v) = call(
        &r,
        json!({"space": r.space, "url": url, "request_id": id, "wait_secs": 1}),
    )
    .await;
    assert_eq!(structured(&v)["status"], "pending", "{v}");
    // The user declines: refused, typed nothing.
    broker.deny(&r.cua, &id).await.unwrap();
    let (err, v) = call(
        &r,
        json!({"space": r.space, "url": url, "request_id": id, "wait_secs": 1}),
    )
    .await;
    assert!(err, "{v}");
    assert_eq!(structured(&v)["error"]["kind"], "login_refused", "{v}");
    // A new request, approved: the Keyvault reaches the Space's cua-driver
    // (the mock serves none, so the fill itself fails there, audited).
    let (_, v) = call(&r, json!({"space": r.space, "url": url, "agent": "ada"})).await;
    let id = structured(&v)["request_id"].as_str().unwrap().to_string();
    broker
        .approve(&r.cua, &id, Default::default())
        .await
        .unwrap();
    let (err, v) = call(
        &r,
        json!({"space": r.space, "url": url, "request_id": id, "wait_secs": 2}),
    )
    .await;
    assert!(err, "{v}");
    assert!(v.to_string().contains("browser_prepare"), "{v}");
    let audit = broker.audit_tail(&r.cua, 100).await.unwrap();
    let kinds: Vec<(&str, &str)> = audit
        .iter()
        .map(|e| (e.event.kind.as_str(), e.event.decision.as_str()))
        .collect();
    for want in [
        ("login.request", "pending"),
        ("login.denied", "deny"),
        ("login.authorize", "allow"),
        ("login.fill", "error"),
    ] {
        assert!(kinds.contains(&want), "{want:?} in {kinds:?}");
    }
    let audit_text = serde_json::to_string(&audit).unwrap();
    assert!(!audit_text.contains(PASSWORD));
}

#[tokio::test]
async fn without_a_keyvault_the_tool_is_fail_closed() {
    let r = rig().await;
    if let Some(s) = r.mcp.spaces() {
        s.set_site_login_broker(None);
    }
    let (err, v) = call(
        &r,
        json!({"space": r.space, "url": "http://login.example.test:8000/"}),
    )
    .await;
    assert!(err, "{v}");
    assert_eq!(
        structured(&v)["error"]["kind"],
        "host_capability_missing",
        "{v}"
    );
}

/// S1: a Space reached through a relay path refuses to type a saved
/// password into it (nothing reaches the mock's driver registry at all)
/// unless the caller explicitly acks the plaintext risk, since this layer
/// has no way to seal driver input the way it seals a teleport bundle.
///
/// `cua_teleport::RELAY_SEALING_ENFORCED` is the single switch for this and
/// the teleport gate; while it is still off (pre-launch, before the app's
/// consent dialog ships) the unacked fill reaches the driver like any other
/// relay-routed fill, so this asserts on the *current* value instead of
/// hardcoding "refused" -- flip the gate on and this starts asserting the
/// refusal for real.
#[tokio::test]
async fn a_relay_routed_fill_without_the_ack_matches_the_sealing_gate() {
    let r = rig_with(Some("/m/fake-machine")).await;
    r.kv.broker()
        .import_passwords(
            &r.cua,
            PasswordImportSpec {
                app: "chrome".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let url = "http://login.example.test:8000/";
    let (err, v) = call(&r, json!({"space": r.space, "url": url, "agent": "ada"})).await;
    assert!(!err, "{v}");
    let id = structured(&v)["request_id"].as_str().unwrap().to_string();
    r.kv.broker()
        .approve(&r.cua, &id, Default::default())
        .await
        .unwrap();
    let (err, v) = call(
        &r,
        json!({"space": r.space, "url": url, "request_id": id, "wait_secs": 2}),
    )
    .await;
    assert!(err, "{v}");
    let text = v.to_string();
    if cua_teleport::RELAY_SEALING_ENFORCED {
        // Refused before ever reaching the (nonexistent) driver, unlike the
        // direct-mode case in
        // `the_tool_asks_first_refuses_on_deny_and_types_only_after_approval`.
        assert!(text.contains("relay_plaintext_ack"), "{v}");
        assert!(!text.contains("browser_prepare"), "{v}");
    } else {
        // Not yet enforced: it reaches the (nonexistent) mock driver and
        // fails there instead, same shape as the direct-mode case.
        assert!(text.contains("browser_prepare"), "{v}");
    }
    let audit = r.kv.broker().audit_tail(&r.cua, 100).await.unwrap();
    let kinds: Vec<(&str, &str)> = audit
        .iter()
        .map(|e| (e.event.kind.as_str(), e.event.decision.as_str()))
        .collect();
    assert!(kinds.contains(&("login.fill", "error")), "{kinds:?}");
}

/// The same fill succeeds past the gate (and then fails normally at the
/// mock's absent driver, same as the direct-mode case) once the caller
/// explicitly acks the plaintext risk.
#[tokio::test]
async fn a_relay_routed_fill_proceeds_once_acked() {
    let r = rig_with(Some("/m/fake-machine")).await;
    r.kv.broker()
        .import_passwords(
            &r.cua,
            PasswordImportSpec {
                app: "chrome".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let url = "http://login.example.test:8000/";
    let (err, v) = call(
        &r,
        json!({"space": r.space, "url": url, "agent": "ada", "relay_plaintext_ack": true}),
    )
    .await;
    assert!(!err, "{v}");
    let id = structured(&v)["request_id"].as_str().unwrap().to_string();
    r.kv.broker()
        .approve(&r.cua, &id, Default::default())
        .await
        .unwrap();
    let (err, v) = call(
        &r,
        json!({
            "space": r.space, "url": url, "request_id": id, "wait_secs": 2,
            "relay_plaintext_ack": true,
        }),
    )
    .await;
    assert!(err, "{v}");
    let text = v.to_string();
    assert!(!text.contains("relay_plaintext_ack"), "{v}");
    assert!(text.contains("browser_prepare"), "{v}");
}
