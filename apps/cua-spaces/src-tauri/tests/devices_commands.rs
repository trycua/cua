// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The `devices_*` command layer (`cua_spaces_lib::devices::DevicesService`)
//! against the fake relay, with in-memory device keys and a fake presence
//! gate. Hermetic: no keychain, no credential store, no OS prompt.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use cua_host::testing::FakeRelay;
use cua_host::{DeviceAuth, MemoryKeySlot, StaticToken};
use cua_spaces_lib::devices::{DevicesService, PresenceGate, PRESENCE_REASON};

const TOKEN: &str = "acct-token";

struct FakePresence {
    allow: AtomicBool,
    asked: Mutex<Vec<(String, Option<String>)>>,
}

impl FakePresence {
    fn new(allow: bool) -> Self {
        Self {
            allow: AtomicBool::new(allow),
            asked: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl PresenceGate for FakePresence {
    async fn confirm(&self, reason: &str, passphrase: Option<String>) -> Result<(), String> {
        self.asked
            .lock()
            .unwrap()
            .push((reason.to_string(), passphrase));
        if self.allow.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err("authentication was cancelled".into())
        }
    }
}

/// A device of its own machine: each reports a test machine id, never this
/// host's, so two test devices are two machines (on one machine a newly
/// enrolled key replaces the other, see cua-host's re-keying) and the test
/// reads no host hardware id.
fn device(relay: &FakeRelay, name: &str) -> DevicesService {
    let tokens = Arc::new(StaticToken(TOKEN.into()));
    let auth = DeviceAuth::new(
        &relay.url,
        tokens.clone(),
        Arc::new(MemoryKeySlot::default()),
        name,
    )
    .unwrap()
    .with_machine_id(Some(format!("test-machine-{name}")));
    DevicesService::new(Arc::new(auth), tokens)
}

#[tokio::test]
async fn enroll_snapshot_approve_with_presence_rename_and_revoke() {
    let relay = FakeRelay::start().await;
    relay.add_account(TOKEN, "user-1", Some("ada@example.com"));
    relay.set_enforce_after(1_900_000_000);
    let mac = device(&relay, "MacBook");
    let laptop = device(&relay, "Work laptop");

    // The first device enrolls at once; the second waits with a code.
    let first = mac.enroll().await.unwrap();
    assert!(first.enrolled && first.code.is_none());
    assert!(mac.check_enrolled().await);
    let second = laptop.enroll().await.unwrap();
    assert!(!second.enrolled);
    let code = second.code.clone().unwrap();
    assert!(!laptop.check_enrolled().await);

    // The snapshot is the core's DevicesInput.
    let snap = mac.snapshot().await.unwrap();
    assert_eq!(snap.enforce_after, Some(1_900_000_000));
    assert_eq!(snap.devices.len(), 2);
    let this = snap.devices.iter().find(|d| d.current).unwrap();
    assert_eq!(this.name, "MacBook");
    assert_eq!(snap.local_device_id.as_deref(), Some(this.id.as_str()));
    assert_eq!(this.platform.as_deref(), Some(std::env::consts::OS));
    let pending = snap.devices.iter().find(|d| !d.current).unwrap();
    assert_eq!(pending.state, "pending");
    assert!(snap.audit.iter().any(|e| e.kind == "device_registered"));
    let view = cua_spaces_app_core::devices::devices_view(&snap, 1_800_000_000);
    assert_eq!(view.approvals.len(), 1);
    let json = serde_json::to_value(&snap).unwrap();
    assert!(json.get("localDeviceId").is_some() && json.get("enforceAfter").is_some());

    // Presence refused: nothing reaches the relay.
    let no = FakePresence::new(false);
    assert_eq!(
        mac.approve(&no, Some(code.clone()), None, None)
            .await
            .unwrap_err(),
        "authentication was cancelled"
    );
    assert_eq!(no.asked.lock().unwrap()[0].0, PRESENCE_REASON);
    assert!(!laptop.check_enrolled().await);
    // Nothing to approve: refused before presence is asked.
    assert!(mac
        .approve(&no, None, Some(" ".into()), None)
        .await
        .is_err());
    assert_eq!(no.asked.lock().unwrap().len(), 1);

    // Presence given: approved by the code.
    let yes = FakePresence::new(true);
    mac.approve(&yes, Some(code), None, Some("pass".into()))
        .await
        .unwrap();
    assert_eq!(
        yes.asked.lock().unwrap()[0],
        (PRESENCE_REASON.to_string(), Some("pass".to_string()))
    );
    assert!(laptop.check_enrolled().await);

    // Rename (cleaned by the core) and revoke.
    assert!(mac.rename(&pending.id, "   ").await.is_err());
    mac.rename(&pending.id, "  Studio laptop ").await.unwrap();
    let snap = mac.snapshot().await.unwrap();
    let renamed = snap.devices.iter().find(|d| d.id == pending.id).unwrap();
    assert_eq!(renamed.name, "Studio laptop");
    mac.revoke(&pending.id).await.unwrap();
    assert!(!laptop.check_enrolled().await);
    let snap = mac.snapshot().await.unwrap();
    assert_eq!(
        snap.devices
            .iter()
            .find(|d| d.id == pending.id)
            .unwrap()
            .state,
        "revoked"
    );
}

#[tokio::test]
async fn a_pending_device_cannot_approve_rename_or_revoke() {
    let relay = FakeRelay::start().await;
    relay.add_account(TOKEN, "user-1", None);
    let mac = device(&relay, "MacBook");
    mac.enroll().await.unwrap();
    let laptop = device(&relay, "Laptop");
    let code = laptop.enroll().await.unwrap().code.unwrap();
    let yes = FakePresence::new(true);
    assert!(laptop.approve(&yes, Some(code), None, None).await.is_err());
    let snap = laptop.snapshot().await.unwrap();
    let mac_id = snap
        .devices
        .iter()
        .find(|d| d.name == "MacBook")
        .unwrap()
        .id
        .clone();
    assert!(laptop.rename(&mac_id, "x").await.is_err());
    assert!(laptop.revoke(&mac_id).await.is_err());
}
