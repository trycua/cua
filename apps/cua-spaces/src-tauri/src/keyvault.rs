// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The Keyvault page: UI commands over the Cua Keyvault broker that
//! `cua daemon` hosts on `$CUA_HOME/keyvault.sock`.
//!
//! The app is a first-party client of the broker and nothing more. Every
//! operation here is an existing broker request (`cua_keyvault::ipc::Request`):
//! the page reads items, pending consent requests, grants, unattended rules,
//! deliveries and the audit tail, and asks the broker to disable the vault,
//! narrow or widen an item's policy, revoke a grant, remove a rule, wipe a
//! delivery, or answer a consent request. The app never captures credentials,
//! never reads secret values (the broker has no "reveal" operation), and never
//! runs its own Touch ID prompt for these actions: the daemon asks for user
//! presence itself, so a click in this window alone can never widen access.
//!
//! Identity is the kernel's, not ours: the broker checks this process's code
//! signature. A build that is not signed by Cua is not first party, and the
//! broker refuses to list items to it; the page says so instead of pretending.
//!
//! The client itself (transport, overview, actions) is the app core's
//! (`cua-spaces-app-core::keyvault::client`), shared with the SwiftUI app;
//! this file only exposes it as Tauri commands.

pub use cua_spaces_app_core::keyvault::client::{
    DirectTransport, KeyvaultCommands, KvTransport, SocketTransport, AUDIT_TAIL,
};
use cua_spaces_app_core::keyvault::client::{KvFailure, Zeroizing};
use cua_spaces_app_core::keyvault::{KeyvaultOverview, KvGrant, KvItem};
use serde::Deserialize;

/// The production commands: the daemon's socket in `$CUA_HOME`.
pub fn from_env() -> KeyvaultCommands {
    KeyvaultCommands::for_cua_home(&crate::core::cua_home())
}

/// Tauri-managed handle.
pub struct KeyvaultState(pub KeyvaultCommands);

/// A page action's input: item ids for the per-site / per-account toggles.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnattendedArgs {
    pub item_ids: Vec<String>,
    pub unattended: bool,
}

type Cmd<T> = Result<T, String>;

fn msg(f: KvFailure) -> String {
    f.message
}

#[tauri::command]
pub async fn keyvault_overview(state: tauri::State<'_, KeyvaultState>) -> Cmd<KeyvaultOverview> {
    Ok(state.0.overview().await)
}

/// Sets up with the OS key store (Touch ID confirms in the daemon).
#[tauri::command]
pub async fn keyvault_setup(state: tauri::State<'_, KeyvaultState>) -> Cmd<Option<String>> {
    state
        .0
        .setup(cfg!(any(target_os = "macos", target_os = "windows")))
        .await
        .map_err(msg)
}

#[tauri::command]
pub async fn keyvault_unlock(state: tauri::State<'_, KeyvaultState>) -> Cmd<()> {
    state.0.unlock().await.map_err(msg)
}

/// Sets up with a passphrase (a daemon without the OS key store). The
/// passphrase goes only to the broker over the verified socket; it is never
/// logged or kept, and zeroized here.
#[tauri::command]
pub async fn keyvault_setup_passphrase(
    state: tauri::State<'_, KeyvaultState>,
    passphrase: String,
) -> Cmd<Option<String>> {
    state
        .0
        .setup_with_passphrase(Zeroizing::new(passphrase))
        .await
        .map_err(msg)
}

/// Unlocks with the passphrase (sent only to the broker; zeroized here).
#[tauri::command]
pub async fn keyvault_unlock_passphrase(
    state: tauri::State<'_, KeyvaultState>,
    passphrase: String,
) -> Cmd<()> {
    state
        .0
        .unlock_with_passphrase(Zeroizing::new(passphrase))
        .await
        .map_err(msg)
}

#[tauri::command]
pub async fn keyvault_set_disabled(
    state: tauri::State<'_, KeyvaultState>,
    disabled: bool,
) -> Cmd<()> {
    state.0.set_disabled(disabled).await.map_err(msg)
}

#[tauri::command]
pub async fn keyvault_set_unattended(
    state: tauri::State<'_, KeyvaultState>,
    args: UnattendedArgs,
) -> Cmd<Vec<KvItem>> {
    state
        .0
        .set_unattended(&args.item_ids, args.unattended)
        .await
        .map_err(msg)
}

#[tauri::command]
pub async fn keyvault_revoke_grant(
    state: tauri::State<'_, KeyvaultState>,
    id: String,
) -> Cmd<usize> {
    state.0.revoke_grant(&id).await.map_err(msg)
}

#[tauri::command]
pub async fn keyvault_remove_rule(state: tauri::State<'_, KeyvaultState>, id: String) -> Cmd<()> {
    state.0.remove_rule(&id).await.map_err(msg)
}

#[tauri::command]
pub async fn keyvault_release(
    state: tauri::State<'_, KeyvaultState>,
    target: String,
) -> Cmd<Vec<String>> {
    state.0.release(&target).await.map_err(msg)
}

#[tauri::command]
pub async fn keyvault_approve(
    state: tauri::State<'_, KeyvaultState>,
    request_id: String,
    items: Option<Vec<String>>,
) -> Cmd<KvGrant> {
    state.0.approve(&request_id, items).await.map_err(msg)
}

#[tauri::command]
pub async fn keyvault_deny(state: tauri::State<'_, KeyvaultState>, request_id: String) -> Cmd<()> {
    state.0.deny(&request_id).await.map_err(msg)
}
