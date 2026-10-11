// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Devices: this device as a client of the signed-in account on the relay
//! (`cua devices` in the app). The page, the enroll sheet and the approval
//! sheet are the app core's (`devices.*`, shared with the SwiftUI app); the
//! commands here only move data to and from the relay through `cua-host`.
//!
//! Approving another device widens who can reach the account's machines,
//! so [`DevicesService::approve`] asks for presence first: Touch ID or the
//! login password on macOS (the app's existing biometric gate), elsewhere
//! the Keyvault passphrase, checked by the broker. Nothing else prompts:
//! unattended agents keep their device session without the user.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use cua_host::{AccountTokens, DeviceAuth, DeviceState, RelayClient};
use cua_spaces_app_core::devices::{AuditInput, DeviceInput, DevicesInput, MachineInput};
use serde::Serialize;

/// Newest audit events the page reads.
pub const AUDIT_LIMIT: usize = 50;

/// The reason the presence prompt shows.
pub const PRESENCE_REASON: &str = "approve a device for your Cua account";

/// Asks the person at this device to confirm (Touch ID, the login password
/// or a passphrase). `Ok` only when they did.
#[async_trait]
pub trait PresenceGate: Send + Sync {
    /// Confirms `reason`; `passphrase` is what the person typed, if the
    /// gate needs one.
    async fn confirm(&self, reason: &str, passphrase: Option<String>) -> Result<(), String>;
}

/// `devices_enroll`'s result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnrollResult {
    /// Enrolled at once (a fresh sign-in on the account's first device).
    pub enrolled: bool,
    /// The one-time code to approve from an enrolled device otherwise.
    pub code: Option<String>,
}

fn state_word(s: DeviceState) -> &'static str {
    match s {
        DeviceState::Pending => "pending",
        DeviceState::Enrolled => "enrolled",
        DeviceState::Expired => "expired",
        DeviceState::Revoked => "revoked",
    }
}

fn err(e: cua_host::Error) -> String {
    e.to_string()
}

/// The relay calls behind the `devices_*` commands.
pub struct DevicesService {
    auth: Arc<DeviceAuth>,
    tokens: Arc<dyn AccountTokens>,
}

impl DevicesService {
    /// Over this device's `auth` and the account's `tokens`.
    pub fn new(auth: Arc<DeviceAuth>, tokens: Arc<dyn AccountTokens>) -> Self {
        Self { auth, tokens }
    }

    /// The account's devices, the grace period, the newest audit events and
    /// the machines' names, as the core's `DevicesInput`. Only the device
    /// list must succeed.
    pub async fn snapshot(&self) -> Result<DevicesInput, String> {
        let listing = self.auth.listing().await.map_err(err)?;
        let audit = self.auth.audit(AUDIT_LIMIT).await.unwrap_or_default();
        let mut machine_names = HashMap::new();
        let mut machines_input = Vec::new();
        if let Some(session) = self.auth.try_session().await {
            if let (Ok(token), Ok(relay)) = (
                self.tokens.access_token().await,
                RelayClient::new(self.auth.relay_url()),
            ) {
                if let Ok(machines) = relay
                    .with_device_session(Some(session))
                    .machines(&token)
                    .await
                {
                    machines_input = machines
                        .iter()
                        .map(|m| MachineInput {
                            id: m.id.clone(),
                            name: m.name.clone(),
                            confirmed: m.confirmed,
                        })
                        .collect();
                    machine_names = machines.into_iter().map(|m| (m.id, m.name)).collect();
                }
            }
        }
        Ok(DevicesInput {
            devices: listing
                .devices
                .into_iter()
                .map(|d| DeviceInput {
                    state: state_word(d.state).into(),
                    platform: (!d.platform.is_empty()).then_some(d.platform),
                    id: d.id,
                    name: d.name,
                    enrolled_until: d.enrolled_until,
                    last_seen: d.last_seen,
                    current: d.current,
                })
                .collect(),
            audit: audit
                .into_iter()
                .map(|e| AuditInput {
                    ts: e.ts,
                    kind: e.kind,
                    device: e.device,
                    machine: e.machine,
                    subject: e.subject,
                    detail: e.detail,
                })
                .collect(),
            local_device_id: self.auth.device_id().map_err(err)?,
            pending_code: None,
            enforce_after: (listing.enforce_after > 0).then_some(listing.enforce_after),
            machine_names,
            machines: machines_input,
        })
    }

    /// Registers this device: enrolled at once after a fresh sign-in on the
    /// account's first device, else a one-time code.
    pub async fn enroll(&self) -> Result<EnrollResult, String> {
        let e = self.auth.enroll().await.map_err(err)?;
        Ok(EnrollResult {
            enrolled: e.device.state == DeviceState::Enrolled,
            code: e.code,
        })
    }

    /// Whether the relay lets this device open a session now, asked afresh
    /// (polled while waiting for an approval).
    pub async fn check_enrolled(&self) -> bool {
        self.auth.reset_session().await;
        self.auth.session().await.is_ok()
    }

    /// Approves the device showing `code` (or re-verifies `device_id`)
    /// after `presence` confirms. Nothing reaches the relay otherwise.
    pub async fn approve(
        &self,
        presence: &dyn PresenceGate,
        code: Option<String>,
        device_id: Option<String>,
        passphrase: Option<String>,
    ) -> Result<(), String> {
        let code = code.filter(|c| !c.trim().is_empty());
        let device_id = device_id.filter(|d| !d.trim().is_empty());
        if code.is_none() && device_id.is_none() {
            return Err("enter the code the other device shows".into());
        }
        presence.confirm(PRESENCE_REASON, passphrase).await?;
        self.auth
            .approve(code.as_deref(), device_id.as_deref())
            .await
            .map(|_| ())
            .map_err(err)
    }

    /// Renames a device (the name as the core cleans it).
    pub async fn rename(&self, id: &str, name: &str) -> Result<(), String> {
        let name = cua_spaces_app_core::devices::clean_name(name)
            .ok_or_else(|| "enter a name".to_string())?;
        self.auth.rename(id, &name).await.map(|_| ()).map_err(err)
    }

    /// Revokes a device (Deny on a pending one; Revoke… after its
    /// confirmation).
    pub async fn revoke(&self, id: &str) -> Result<(), String> {
        self.auth.revoke(id).await.map(|_| ()).map_err(err)
    }

    /// Vouches for a machine that registered without an enrolled device's
    /// proof (S5), from this enrolled device.
    pub async fn confirm_machine(&self, id: &str) -> Result<(), String> {
        let session = self.auth.session().await.map_err(err)?;
        let token = self.tokens.access_token().await.map_err(err)?;
        RelayClient::new(self.auth.relay_url())
            .map_err(err)?
            .with_device_session(Some(session))
            .confirm(&token, id)
            .await
            .map(|_| ())
            .map_err(err)
    }
}

/// The app's presence gate: the biometric prompt on macOS, else the
/// Keyvault passphrase checked by the broker.
pub struct OsPresence<'a> {
    /// The Keyvault broker client (the passphrase check off macOS).
    pub keyvault: &'a crate::keyvault::KeyvaultCommands,
}

#[async_trait]
impl PresenceGate for OsPresence<'_> {
    async fn confirm(&self, reason: &str, passphrase: Option<String>) -> Result<(), String> {
        if cfg!(target_os = "macos") {
            let reason = reason.to_string();
            return tokio::task::spawn_blocking(move || crate::biometric::authorize(&reason))
                .await
                .map_err(|e| e.to_string())?;
        }
        match passphrase.filter(|p| !p.is_empty()) {
            Some(p) => self
                .keyvault
                .unlock_with_passphrase(cua_spaces_app_core::keyvault::client::Zeroizing::new(p))
                .await
                .map_err(|f| format!("the Keyvault passphrase was not accepted ({})", f.message)),
            None => Err(
                "enter your Keyvault passphrase to approve (set one up in Keyvault first)".into(),
            ),
        }
    }
}

/// Whether presence on this system is a passphrase the page must ask for.
pub fn presence_needs_passphrase() -> bool {
    !cfg!(target_os = "macos")
}
