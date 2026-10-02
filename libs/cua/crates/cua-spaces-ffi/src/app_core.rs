// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The Cua Spaces app core (`cua-spaces-app-core`) for native shells: the
//! view models and state machines the Tauri app and the SwiftUI app share.
//!
//! Every function is a one-line call into the core; the records are the
//! core's own types, mirrored in [`app_core_types`](super::app_core_types).
//! Reducers are pure: a shell keeps the state, sends actions, and renders
//! the view. [`KeyvaultClient`] is the broker client both shells run.

use std::path::PathBuf;
use std::sync::Arc;

use crate::app_core_types::*;
use cua_sdk::{CuaError, Result};
use cua_spaces_app_core as core;

// ---- Spaces ---------------------------------------------------------------

/// Registry rows (`Spaces.list`) as Spaces.
#[uniffi::export]
pub fn app_rows_to_spaces(rows: Vec<AppSpaceRow>, now_ms: i64) -> Vec<AppSpace> {
    core::spaces::rows_to_spaces(&rows, now_ms)
}

/// "brave-otter" as "Brave Otter".
#[uniffi::export]
pub fn app_display_name(name: String) -> String {
    core::spaces::display_name(&name)
}

/// The menu bar status line ("3 Spaces").
#[uniffi::export]
pub fn app_status_line(count: u32) -> String {
    core::spaces::status_line(count)
}

/// The ambient status dots.
#[uniffi::export]
pub fn app_ambient_dots(spaces: Vec<AppSpace>) -> AppAmbientDots {
    core::spaces::ambient_dots(&spaces)
}

/// The Space list's first state.
#[uniffi::export]
pub fn app_roster_initial(spaces: Vec<AppSpace>) -> AppRosterState {
    core::spaces::roster::initial(&spaces)
}

/// Advances the Space list.
#[uniffi::export]
pub fn app_roster_reduce(state: AppRosterState, action: AppRosterAction) -> AppRosterState {
    core::spaces::roster::reduce(&state, &action)
}

/// Advances the Spaces being created (see `spaces::creating`).
#[uniffi::export]
pub fn app_creates_reduce(state: AppCreatesState, action: AppCreateAction) -> AppCreatesState {
    core::spaces::creating::reduce(&state, &action)
}

/// The registry's Spaces plus a row per Space being created.
#[uniffi::export]
pub fn app_creates_compose(spaces: Vec<AppSpace>, state: AppCreatesState) -> Vec<AppSpace> {
    core::spaces::creating::compose(&spaces, &state)
}

/// Whether `id` is a Space still being created (or whose create failed).
#[uniffi::export]
pub fn app_creates_is_pending(id: String) -> bool {
    core::spaces::creating::is_pending(&id)
}

/// Drops finished creates the registry lists and finished deletes it no
/// longer lists (after each registry refresh).
#[uniffi::export]
pub fn app_creates_settle(state: AppCreatesState, spaces: Vec<AppSpace>) -> AppCreatesState {
    core::spaces::creating::settle(&state, &spaces)
}

/// Whether the Space `id` is being deleted: a Delete for it does nothing.
#[uniffi::export]
pub fn app_creates_is_deleting(state: AppCreatesState, id: String) -> bool {
    core::spaces::creating::is_deleting(&state, &id)
}

/// Whether a power action runs for the Space `id`: its button waits.
#[uniffi::export]
pub fn app_creates_is_powering(state: AppCreatesState, id: String) -> bool {
    core::spaces::creating::is_powering(&state, &id)
}

/// The main window's sidebar.
#[uniffi::export]
pub fn app_sidebar(spaces: Vec<AppSpace>, query: String, selected_id: String) -> AppSidebarView {
    core::spaces::sidebar::sidebar(&spaces, &query, &selected_id)
}

/// A Space's detail.
#[uniffi::export]
pub fn app_space_detail(space: AppSpace) -> AppSpaceDetail {
    core::spaces::sidebar::detail(&space)
}

/// The selected Space's detail with its memory and storage use (the SDK's
/// `Space.usage`, refreshed every `app_usage_refresh_ms`). `host_arch` is
/// this Mac's CPU architecture (`arm64`, `x86_64`): a local Space of
/// another one shows a warning on its Architecture.
#[uniffi::export]
pub fn app_space_detail_live(
    space: AppSpace,
    usage: Option<AppSpaceUsage>,
    host_arch: Option<String>,
) -> AppSpaceDetail {
    core::spaces::sidebar::detail_live(&space, usage.as_ref(), host_arch.as_deref())
}

/// [`app_space_detail_live`] without what Settings, Experiments hides (the
/// Share button while Sharing is off).
#[uniffi::export]
pub fn app_space_detail_with(
    space: AppSpace,
    usage: Option<AppSpaceUsage>,
    host_arch: Option<String>,
    experiments: AppExperiments,
) -> AppSpaceDetail {
    core::spaces::sidebar::detail_with(&space, usage.as_ref(), host_arch.as_deref(), &experiments)
}

/// This machine's CPU architecture as the Spaces apps pass it to the core
/// (`arm64`, `amd64`).
#[uniffi::export]
pub fn app_host_arch() -> String {
    core::model::this_host_arch().to_string()
}

/// How often a shell refreshes a visible detail's memory and storage use.
#[uniffi::export]
pub fn app_usage_refresh_ms() -> u64 {
    core::spaces::sidebar::USAGE_REFRESH_MS
}

/// The "This machine" roster entry.
#[uniffi::export]
pub fn app_this_machine_space(status: Option<AppHostSummaryInput>, now_ms: i64) -> AppSpace {
    core::host::this_machine_space(status.as_ref(), now_ms, core::model::SpaceOs::Macos)
}

/// Adds the "This machine" entry to a roster, first.
#[uniffi::export]
pub fn app_with_this_machine(
    spaces: Vec<AppSpace>,
    status: Option<AppHostSummaryInput>,
    now_ms: i64,
) -> Vec<AppSpace> {
    core::host::with_this_machine(
        &spaces,
        status.as_ref(),
        now_ms,
        core::model::SpaceOs::Macos,
    )
}

/// The words of a Space's sections (Stream, Agents, Teleport).
#[uniffi::export]
pub fn app_space_detail_copy() -> AppDetailCopy {
    core::spaces::sidebar::detail_copy()
}

/// What a Space's preview card shows before (or instead of) its live
/// desktop: "Connecting…", a Connect button, or the Space's own line.
#[uniffi::export]
pub fn app_desktop_cover(input: AppDesktopCoverInput) -> AppDesktopCover {
    core::spaces::cover::desktop_cover(&input)
}

/// How fresh the shells keep each Space's thumbnail.
#[uniffi::export]
pub fn app_thumbnail_policy() -> AppThumbnailPolicy {
    core::spaces::cover::thumbnail_policy()
}

/// A Space's Stream section: the Desktop row, then one row per window.
#[uniffi::export]
pub fn app_stream_section(input: AppStreamSectionInput) -> AppStreamSection {
    core::spaces::stream::stream_section(&input)
}

/// The open picture-in-picture panels (row ids) after a panel opened,
/// closed or the shell re-read them: the Stream section's `open`.
#[uniffi::export]
pub fn app_stream_pip_reduce(open: Vec<String>, event: AppPipEvent) -> Vec<String> {
    core::spaces::stream::pip_reduce(&open, &event)
}

/// What a row's picture-in-picture button does: close its open panel, or
/// open one.
#[uniffi::export]
pub fn app_stream_pip_click(open: Vec<String>, row: String) -> AppPipCommand {
    core::spaces::stream::pip_click(&open, &row)
}

/// The Stream section's Desktop row id (a window row's id is its handle).
#[uniffi::export]
pub fn app_stream_desktop_row_id() -> String {
    core::spaces::stream::DESKTOP_ROW_ID.into()
}

/// The teleport picker's tab strip.
#[uniffi::export]
pub fn app_picker_grid_tabs(space_name: String) -> Vec<AppPickerGridTabItem> {
    core::teleport::grid::grid_tabs(&space_name)
}

/// The picker's Apps tab as tiles (`windows`: this machine's, front to back,
/// for each app's frontmost-window preview).
#[uniffi::export]
pub fn app_picker_app_grid(state: AppPickerState, windows: Vec<AppOpenWindow>) -> AppPickerGrid {
    core::teleport::grid::app_grid(&state, &windows)
}

/// The picker's Open windows tab.
#[uniffi::export]
pub fn app_picker_window_grid(
    windows: Vec<AppOpenWindow>,
    query: String,
    selected: Option<String>,
) -> AppPickerGrid {
    core::teleport::grid::window_grid(&windows, &query, selected.as_deref())
}

/// The picker's From <Space> tab.
#[uniffi::export]
pub fn app_picker_remote_grid(
    windows: Vec<AppRemoteWindow>,
    query: String,
    selected: Option<String>,
) -> AppPickerGrid {
    core::teleport::grid::remote_grid(&windows, &query, selected.as_deref())
}

/// The picker grid's primary button for a tab ("Continue", "Teleport to
/// <Space>", "Stream to This Mac"), live with a choosable tile selected.
#[uniffi::export]
pub fn app_picker_grid_primary(
    tab: AppPickerGridTab,
    space_name: String,
    grid: AppPickerGrid,
) -> AppPickerGridPrimary {
    core::teleport::grid::grid_primary(tab, &space_name, &grid)
}

/// Arrow keys over a picker grid: the tile `delta` choosable places away.
#[uniffi::export]
pub fn app_picker_grid_step(
    grid: AppPickerGrid,
    selected: Option<String>,
    delta: i32,
) -> Option<String> {
    core::teleport::grid::grid_step(&grid, selected.as_deref(), delta)
}

/// The banner when Delete fails.
#[uniffi::export]
pub fn app_delete_failed_text(name: String, error: String) -> String {
    core::spaces::sidebar::delete_failed_text(&name, &error)
}

/// The power button next to Delete, for a Space that turns off and on.
#[uniffi::export]
pub fn app_power_button(space: AppSpace) -> Option<AppPowerButton> {
    core::spaces::sidebar::power_button(&space)
}

/// The main window's chrome (account line, New Space, empty state).
#[uniffi::export]
pub fn app_main_chrome(input: AppChromeInput) -> AppMainChrome {
    core::window::chrome(&input)
}

/// The menu bar item's menu.
#[uniffi::export]
pub fn app_menu_bar(spaces: u32) -> Vec<AppMenuItem> {
    core::window::menu_bar(spaces)
}

/// The menu bar item's menu with the live Keyvault sharing line
/// ([`kv_sharing_label`]) when there is one.
#[uniffi::export]
pub fn app_menu_bar_with_keyvault(spaces: u32, keyvault: Option<String>) -> Vec<AppMenuItem> {
    core::window::menu_bar_with_keyvault(spaces, keyvault.as_deref())
}

// ---- Presence ------------------------------------------------------------------

/// The name on this user's presence cursor: the account's name, else its
/// email's local part, else its username, else this computer's account
/// (full, then short name). Never empty, never an agent's or "You".
#[uniffi::export]
pub fn app_presence_name(
    name: Option<String>,
    email: Option<String>,
    username: Option<String>,
    os_full_name: Option<String>,
    os_user: Option<String>,
) -> String {
    core::presence::presence_name(
        name.as_deref(),
        email.as_deref(),
        username.as_deref(),
        os_full_name.as_deref(),
        os_user.as_deref(),
    )
}

/// The stable principal id this user joins presence as.
#[uniffi::export]
pub fn app_presence_principal_id(
    email: Option<String>,
    subject: Option<String>,
    username: Option<String>,
    os_user: Option<String>,
) -> String {
    core::presence::presence_principal_id(
        email.as_deref(),
        subject.as_deref(),
        username.as_deref(),
        os_user.as_deref(),
    )
}

// ---- This machine ------------------------------------------------------------

/// What the sidebar row's summary reads from a host state.
#[uniffi::export]
pub fn app_host_summary_input(state: AppHostState) -> AppHostSummaryInput {
    state.summary_input()
}

/// The "This machine" page (`None` while the host status loads).
#[uniffi::export]
pub fn app_host_panel(state: Option<AppHostState>) -> AppHostPanelView {
    core::host::panel(state.as_ref())
}

/// Permission panes still to grant, as rows.
#[uniffi::export]
pub fn app_host_permission_rows(permissions: Vec<AppHostPermissionInput>) -> Vec<AppPermissionRow> {
    core::host::permission_rows(&permissions)
}

/// The host setup form's first state.
#[uniffi::export]
pub fn app_host_form_initial() -> AppHostFormState {
    core::host::form_initial()
}

/// Advances the host setup form.
#[uniffi::export]
pub fn app_host_form_reduce(
    state: AppHostFormState,
    action: AppHostFormAction,
) -> AppHostFormState {
    core::host::form_reduce(&state, &action)
}

/// The host setup form as drawn (`identity`: the signed-in account).
#[uniffi::export]
pub fn app_host_form_view(state: AppHostFormState, identity: Option<String>) -> AppHostFormView {
    core::host::form_view(&state, identity.as_deref())
}

/// The settings change a host toggle action runs (`None` for the other
/// actions): pass it to `Host.configure`.
#[uniffi::export]
pub fn app_host_setting_change(id: AppHostActionId) -> Option<AppHostSettingChange> {
    core::host::setting_change(id)
}

/// The cua SDK's host status as the app core reads it.
#[uniffi::export]
pub fn app_host_state(status: cua_sdk::HostStatus) -> AppHostState {
    AppHostState {
        configured: status.configured,
        mode: status.mode,
        relay_url: status.relay_url,
        direct_url: status.direct_url,
        name: status.name,
        sharing: status.sharing,
        service_installed: status.service_installed,
        service_running: status.service_running,
        service_kind: status.service_kind,
        online: status.online,
        clients: status
            .clients
            .into_iter()
            .map(|c| core::host::HostClient {
                id: c.id,
                email: c.email,
                name: c.name,
                streams: Some(c.streams),
            })
            .collect(),
        permissions: status
            .permissions
            .into_iter()
            .map(|p| core::host::HostPermissionInput {
                id: p.id,
                title: p.title,
                settings_url: Some(p.settings_url),
                instructions: Some(p.instructions),
                granted: false,
            })
            .collect(),
        error: status.error,
        recent_access: status
            .recent_access
            .into_iter()
            .map(|a| core::host::HostAccess {
                at_ms: a.at_ms,
                via: a.via,
                who: a.who,
                what: a.what,
            })
            .collect(),
        access_log_error: status.access_log_error,
        share_desktop: status.share_desktop,
        provide_spaces: status.provide_spaces,
        max_spaces: status.max_spaces,
        max_macos_vms: status.max_macos_vms,
        provided_spaces: status
            .provided_spaces
            .into_iter()
            .map(|p| core::host::HostProvidedSpace {
                relay_machine: p.relay_machine,
                local_space: p.local_space,
                name: p.name,
                image: p.image,
                os: p.os,
                kind: p.kind,
                created_by: p.created_by,
                created_at_ms: p.created_at_ms,
            })
            .collect(),
        spaces_audit: status
            .spaces_audit
            .into_iter()
            .map(|a| core::host::HostSpacesAudit {
                at_ms: a.at_ms,
                action: a.action,
                who: a.who,
                space: a.space,
                detail: a.detail,
            })
            .collect(),
        spaces_audit_error: status.spaces_audit_error,
    }
}

// ---- Devices -----------------------------------------------------------------

/// The Devices page at `now` (Unix seconds).
#[uniffi::export]
pub fn app_devices_view(input: AppDevicesInput, now: u64) -> AppDevicesView {
    core::devices::devices_view(&input, now)
}

/// A device name as typed for Rename (`None` when empty).
#[uniffi::export]
pub fn app_devices_clean_name(name: String) -> Option<String> {
    core::devices::clean_name(&name)
}

/// The enroll sheet's first state.
#[uniffi::export]
pub fn app_enroll_initial() -> AppEnrollState {
    core::devices::enroll_initial()
}

/// Advances the enroll sheet.
#[uniffi::export]
pub fn app_enroll_reduce(state: AppEnrollState, action: AppEnrollAction) -> AppEnrollState {
    core::devices::enroll_reduce(&state, &action)
}

/// The enroll sheet as drawn.
#[uniffi::export]
pub fn app_enroll_view(state: AppEnrollState) -> AppEnrollView {
    core::devices::enroll_view(&state)
}

/// Opens the approval sheet for a prompt.
#[uniffi::export]
pub fn app_approve_open(prompt: AppApprovalPrompt) -> AppApproveSheetState {
    core::devices::approve_open(&prompt)
}

/// Advances the approval sheet. `devices` is the account's current devices
/// (for approving the waiting device of a code's name by id once the code
/// expires).
#[uniffi::export]
pub fn app_approve_reduce(
    state: AppApproveSheetState,
    action: AppApproveSheetAction,
    devices: Vec<AppDeviceInput>,
) -> AppApproveSheetState {
    core::devices::approve_reduce(&state, &action, &devices)
}

/// The approval sheet as drawn. `devices` is the account's current devices
/// (see [`app_approve_reduce`]).
#[uniffi::export]
pub fn app_approve_view(
    state: AppApproveSheetState,
    devices: Vec<AppDeviceInput>,
) -> AppApproveSheetView {
    core::devices::approve_view(&state, &devices)
}

/// The SDK's devices snapshot as the Devices page reads it
/// (`pending_code`: the code this device shows while it waits).
#[uniffi::export]
pub fn app_devices_input(
    snapshot: cua_sdk::DevicesSnapshot,
    pending_code: Option<String>,
) -> AppDevicesInput {
    AppDevicesInput {
        devices: snapshot
            .devices
            .into_iter()
            .map(|d| core::devices::DeviceInput {
                id: d.id,
                name: d.name,
                state: d.state,
                enrolled_until: d.enrolled_until,
                last_seen: d.last_seen,
                current: d.current,
                platform: (!d.platform.is_empty()).then_some(d.platform),
            })
            .collect(),
        audit: snapshot
            .audit
            .into_iter()
            .map(|e| core::devices::AuditInput {
                ts: e.ts,
                kind: e.kind,
                device: e.device,
                machine: e.machine,
                subject: e.subject,
                detail: e.detail,
            })
            .collect(),
        local_device_id: snapshot.local_device_id,
        pending_code,
        enforce_after: snapshot.enforce_after,
        machine_names: snapshot.machine_names,
        machines: snapshot
            .machines
            .into_iter()
            .map(|m| core::devices::MachineInput {
                id: m.id,
                name: m.name,
                confirmed: m.confirmed,
            })
            .collect(),
    }
}

/// Devices page input from JSON (fixtures, parity flows).
#[uniffi::export]
pub fn app_devices_input_from_json(json: String) -> Result<AppDevicesInput> {
    from_json("devices input", &json)
}

/// An enroll sheet action from JSON (parity flows).
#[uniffi::export]
pub fn app_enroll_action_from_json(json: String) -> Result<AppEnrollAction> {
    from_json("enroll action", &json)
}

/// An approval sheet action from JSON (parity flows).
#[uniffi::export]
pub fn app_approve_sheet_action_from_json(json: String) -> Result<AppApproveSheetAction> {
    from_json("approve action", &json)
}

// ---- Connect a cloud ---------------------------------------------------------

/// A new "Connect a cloud" sheet.
#[uniffi::export]
pub fn app_cloud_connect_initial() -> AppCloudConnectState {
    core::cloud_connect::cloud_connect_initial()
}

/// Advances the "Connect a cloud" sheet.
#[uniffi::export]
pub fn app_cloud_connect_reduce(
    input: AppCloudConnectInput,
    state: AppCloudConnectState,
    action: AppCloudConnectAction,
) -> AppCloudConnectState {
    core::cloud_connect::cloud_connect_reduce(&input, &state, &action)
}

/// The "Connect a cloud" sheet as drawn.
#[uniffi::export]
pub fn app_cloud_connect_view(
    input: AppCloudConnectInput,
    state: AppCloudConnectState,
) -> AppCloudConnectView {
    core::cloud_connect::cloud_connect_view(&input, &state)
}

/// "Connect a cloud" input from JSON (fixtures, parity flows).
#[uniffi::export]
pub fn app_cloud_connect_input_from_json(json: String) -> Result<AppCloudConnectInput> {
    from_json("cloud connect input", &json)
}

/// A "Connect a cloud" action from JSON (parity flows).
#[uniffi::export]
pub fn app_cloud_connect_action_from_json(json: String) -> Result<AppCloudConnectAction> {
    from_json("cloud connect action", &json)
}

/// The sheet's input from the SDK's `cloud_status` JSON.
#[uniffi::export]
pub fn app_cloud_connect_input_from_status_json(json: String) -> Result<AppCloudConnectInput> {
    let status: serde_json::Value = from_json("cloud status", &json)?;
    Ok(core::cloud_connect::connect_input_from_status(&status))
}

/// The wizard's connected clouds from the SDK's `cloud_status` JSON.
#[uniffi::export]
pub fn app_connected_clouds_from_status_json(json: String) -> Result<Vec<AppConnectedCloud>> {
    let status: serde_json::Value = from_json("cloud status", &json)?;
    Ok(core::cloud_connect::connected_clouds_from_status(&status))
}

/// Whether a `default.on` value names one of the user's clouds.
#[uniffi::export]
pub fn app_is_cloud_word(on: String) -> bool {
    core::cloud_connect::is_cloud_word(&on)
}

/// A connected cloud from JSON (parity flows).
#[uniffi::export]
pub fn app_connected_cloud_from_json(json: String) -> Result<AppConnectedCloud> {
    from_json("connected cloud", &json)
}

// ---- Share -----------------------------------------------------------------

/// A new Share sheet.
#[uniffi::export]
pub fn app_share_initial() -> AppShareSheetState {
    core::share::share_initial()
}

/// Advances the Share sheet.
#[uniffi::export]
pub fn app_share_reduce(
    input: AppShareInput,
    state: AppShareSheetState,
    action: AppShareSheetAction,
) -> AppShareSheetState {
    core::share::share_reduce(&input, &state, &action)
}

/// The Share sheet as drawn.
#[uniffi::export]
pub fn app_share_view(input: AppShareInput, state: AppShareSheetState) -> AppShareSheetView {
    core::share::share_view(&input, &state)
}

/// Share sheet input from JSON (fixtures, parity flows).
#[uniffi::export]
pub fn app_share_input_from_json(json: String) -> Result<AppShareInput> {
    from_json("share input", &json)
}

/// A Share sheet action from JSON (parity flows).
#[uniffi::export]
pub fn app_share_action_from_json(json: String) -> Result<AppShareSheetAction> {
    from_json("share action", &json)
}

// ---- Persistent agents ------------------------------------------------------

/// A new Agents page.
#[uniffi::export]
pub fn app_agents_initial() -> AppAgentsState {
    core::persistent::agents_initial()
}

/// Advances the Agents page.
#[uniffi::export]
pub fn app_agents_reduce(
    input: AppAgentsInput,
    state: AppAgentsState,
    action: AppAgentsAction,
) -> AppAgentsState {
    core::persistent::agents_reduce(&input, &state, &action)
}

/// The Agents page as drawn at `now_ms`.
#[uniffi::export]
pub fn app_agents_view(input: AppAgentsInput, state: AppAgentsState, now_ms: u64) -> AppAgentsView {
    core::persistent::agents_view(&input, &state, now_ms)
}

/// Agents page input from JSON (tool results, fixtures, parity flows).
#[uniffi::export]
pub fn app_agents_input_from_json(json: String) -> Result<AppAgentsInput> {
    from_json("agents input", &json)
}

/// An Agents page action from JSON (parity flows).
#[uniffi::export]
pub fn app_agents_action_from_json(json: String) -> Result<AppAgentsAction> {
    from_json("agents action", &json)
}

// ---- Drive -----------------------------------------------------------------

/// A new Drive page (at the root, listing it).
#[uniffi::export]
pub fn app_drive_initial() -> AppDriveState {
    core::drive_page::drive_initial()
}

/// Advances the Drive page.
#[uniffi::export]
pub fn app_drive_reduce(state: AppDriveState, action: AppDriveAction) -> AppDriveState {
    core::drive_page::drive_reduce(&state, &action)
}

/// The Drive page as drawn.
#[uniffi::export]
pub fn app_drive_view(input: AppDriveInput, state: AppDriveState) -> AppDriveView {
    core::drive_page::drive_view(&input, &state)
}

// ---- Storage (Settings) ---------------------------------------------------

/// Settings' Storage section before anything is read.
#[uniffi::export]
pub fn app_storage_initial() -> AppStorageState {
    core::drive_settings::storage_initial()
}

/// Advances the Storage section.
#[uniffi::export]
pub fn app_storage_reduce(state: AppStorageState, action: AppStorageAction) -> AppStorageState {
    core::drive_settings::storage_reduce(&state, &action)
}

/// The Storage section as drawn (a Settings section).
#[uniffi::export]
pub fn app_storage_section(input: AppStorageInput, state: AppStorageState) -> AppSettingsSection {
    core::drive_settings::storage_section(&input, &state)
}

/// What a Storage row's button asks for.
#[uniffi::export]
pub fn app_storage_press(input: AppStorageInput, id: String) -> Option<AppStorageAction> {
    core::drive_settings::storage_press(&input, &id)
}

/// What a Storage row's choice asks for.
#[uniffi::export]
pub fn app_storage_choose(id: String, option: String) -> Option<AppStorageAction> {
    core::drive_settings::storage_choose(&id, &option)
}

/// What a Storage field's edit asks for.
#[uniffi::export]
pub fn app_storage_edit(id: String, value: String) -> Option<AppStorageAction> {
    core::drive_settings::storage_edit(&id, &value)
}

/// One line naming a Storage command.
#[uniffi::export]
pub fn app_storage_request_text(request: AppStorageRequest) -> String {
    core::drive_settings::storage_request_text(&request)
}

/// Storage input from JSON (the daemon's answers, as they come).
#[uniffi::export]
pub fn app_storage_input_from_json(json: String) -> Result<AppStorageInput> {
    from_json("storage input", &json)
}

/// A Storage action from JSON (a test's or save's answer, parity flows).
#[uniffi::export]
pub fn app_storage_action_from_json(json: String) -> Result<AppStorageAction> {
    from_json("storage action", &json)
}

/// The Storage request as the tool's JSON arguments (`volume_storage_set`).
#[uniffi::export]
pub fn app_storage_update_json(update: AppDriveStorageUpdate) -> String {
    serde_json::to_string(&update).unwrap_or_default()
}

/// Drive page input from JSON.
#[uniffi::export]
pub fn app_drive_input_from_json(json: String) -> Result<AppDriveInput> {
    from_json("drive input", &json)
}

/// A Drive page action from JSON (parity flows).
#[uniffi::export]
pub fn app_drive_action_from_json(json: String) -> Result<AppDriveAction> {
    from_json("drive action", &json)
}

// ---- Notifications ---------------------------------------------------------

/// Which feed entries to post now, and the marker to save
/// (`AppSettings.notifications_seen_ms`; 0 on the first run).
#[uniffi::export]
pub fn app_notifications_plan(
    feed: Vec<AppNotificationInput>,
    seen_ms: u64,
) -> AppNotificationsPlan {
    core::notifications::notifications_plan(&feed, seen_ms)
}

/// The notifications list at `now_ms`.
#[uniffi::export]
pub fn app_notifications_view(
    feed: Vec<AppNotificationInput>,
    now_ms: u64,
) -> AppNotificationsView {
    core::notifications::notifications_view(&feed, now_ms)
}

/// Feed entries from JSON (the `notifications_list` result's array).
#[uniffi::export]
pub fn app_notifications_from_json(json: String) -> Result<Vec<AppNotificationInput>> {
    from_json("notifications", &json)
}

// ---- Settings, About and updates ------------------------------------------

/// Settings, About: name, version, links, copyright and the update controls.
#[uniffi::export]
pub fn app_about_view(input: AppAboutInput) -> AppAboutView {
    core::about::view(&input)
}

/// The Sparkle channels an updater on `channel` may install from.
#[uniffi::export]
pub fn app_about_allowed_channels(channel: AppUpdateChannel) -> Vec<String> {
    core::about::allowed_channels(channel)
}

/// What this launch does (refresh after an update; the version to record).
#[uniffi::export]
pub fn app_about_after_launch(input: AppLaunchInput) -> AppLaunchPlan {
    core::about::after_launch(&input)
}

/// Whether the running daemon is this app's own (restart it after an update).
#[uniffi::export]
pub fn app_about_restart_daemon(check: AppDaemonCheck) -> bool {
    core::about::restart_daemon(&check)
}

/// The one notice after a failed refresh (`None` when it worked).
#[uniffi::export]
pub fn app_about_refresh_notice(report: AppRefreshReport) -> Option<String> {
    core::about::refresh_notice(&report)
}

/// The About input from JSON (parity flows).
#[uniffi::export]
pub fn app_about_input_from_json(json: String) -> Result<AppAboutInput> {
    from_json("about input", &json)
}

/// A launch from JSON (parity flows).
#[uniffi::export]
pub fn app_launch_input_from_json(json: String) -> Result<AppLaunchInput> {
    from_json("launch input", &json)
}

/// A daemon check from JSON (parity flows).
#[uniffi::export]
pub fn app_daemon_check_from_json(json: String) -> Result<AppDaemonCheck> {
    from_json("daemon check", &json)
}

/// A refresh report from JSON (parity flows).
#[uniffi::export]
pub fn app_refresh_report_from_json(json: String) -> Result<AppRefreshReport> {
    from_json("refresh report", &json)
}

// ---- New Space -------------------------------------------------------------

/// The wizard's first state.
#[uniffi::export]
pub fn app_wizard_initial(env: AppWizardEnv) -> AppWizardState {
    core::wizard::initial(&env)
}

/// Advances the wizard.
#[uniffi::export]
pub fn app_wizard_reduce(
    state: AppWizardState,
    action: AppWizardAction,
    env: AppWizardEnv,
) -> AppWizardState {
    core::wizard::reduce(&state, &action, &env)
}

/// The wizard as drawn.
#[uniffi::export]
pub fn app_wizard_view(state: AppWizardState, env: AppWizardEnv) -> AppWizardView {
    core::wizard::view(&state, &env)
}

/// The SDK call a plan makes (`create_space`).
#[uniffi::export]
pub fn app_wizard_create_args(plan: AppCreatePlan) -> AppCreateSpaceArgs {
    core::wizard::create_args(&plan)
}

/// The "Run on" menu: This Mac, your machines, your clouds.
#[uniffi::export]
pub fn app_wizard_placement_options(
    state: AppWizardState,
    env: AppWizardEnv,
) -> Vec<AppPlacementOption> {
    core::wizard::placement_options(&state, &env)
}

/// The wizard's environment from JSON (fixtures, parity).
#[uniffi::export]
pub fn app_wizard_env_from_json(json: String) -> Result<AppWizardEnv> {
    from_json("wizard env", &json)
}

/// A wizard action from JSON (fixtures, parity).
#[uniffi::export]
pub fn app_wizard_action_from_json(json: String) -> Result<AppWizardAction> {
    from_json("wizard action", &json)
}

/// One of your machines that provides Spaces (the SDK's `Spaces.hosts()`)
/// as the wizard reads it.
#[uniffi::export]
pub fn app_space_host(host: cua_sdk::SpacesHost) -> AppSpaceHost {
    AppSpaceHost {
        id: host.id,
        name: host.name,
        via: host.via,
        online: host.online,
        os: host.os,
        limits: host
            .limits
            .into_iter()
            .map(|l| core::wizard::HostLimit {
                resource: l.resource,
                used: l.used,
                limit: l.limit,
                reason: l.reason,
            })
            .collect(),
    }
}

/// The presets matching the image field's text, grouped.
#[uniffi::export]
pub fn app_wizard_image_suggestions(query: String) -> Vec<AppImageGroup> {
    core::wizard::image_suggestions(&query)
}

/// Why a typed image ref cannot be used, in one line (`None`: it can).
#[uniffi::export]
pub fn app_wizard_validate_image_ref(image_ref: String) -> Option<String> {
    core::wizard::validate_image_ref(&image_ref)
}

/// The notice while a plan is being created.
#[uniffi::export]
pub fn app_wizard_creating_text(plan: AppCreatePlan) -> String {
    core::wizard::creating_text(&plan)
}

/// The notice when `create_space` failed.
#[uniffi::export]
pub fn app_wizard_create_failed_text(error: String) -> String {
    core::wizard::create_failed_text(&error)
}

// ---- Teleport ----------------------------------------------------------------

/// "Teleport an app..." first state.
#[uniffi::export]
pub fn app_picker_initial(space_name: String) -> AppPickerState {
    core::teleport::flow::initial(&space_name)
}

/// Advances the picker.
#[uniffi::export]
pub fn app_picker_reduce(state: AppPickerState, event: AppPickerEvent) -> AppPickerState {
    core::teleport::flow::reduce(&state, &event)
}

/// The pick list's sections.
#[uniffi::export]
pub fn app_picker_sections(state: AppPickerState) -> Vec<AppEntrySection> {
    core::teleport::flow::sections(&state)
}

/// Planning is possible.
#[uniffi::export]
pub fn app_picker_can_plan(state: AppPickerState) -> bool {
    core::teleport::flow::can_plan(&state)
}

/// The options step's opt-in checkboxes ("Keep me signed in", "Saved
/// passwords", "Browsing history"), with the signed-in state move chosen.
#[uniffi::export]
pub fn app_picker_sensitive_options(state: AppPickerState) -> Vec<AppSensitiveOption> {
    core::teleport::flow::sensitive_options(&state)
}

/// The groups the plan asks for (`TeleportPlanOptions.sensitive_groups`).
#[uniffi::export]
pub fn app_picker_plan_sensitive(state: AppPickerState) -> Vec<AppSensitiveGroup> {
    core::teleport::flow::plan_sensitive(&state)
}

/// The SDK's plan-option group for a picker group.
#[uniffi::export]
pub fn app_sensitive_group_sdk(group: AppSensitiveGroup) -> super::TeleportSensitiveGroup {
    use core::teleport::flow::SensitiveGroup as G;
    match group {
        G::SignIns => super::TeleportSensitiveGroup::SignIns,
        G::Passwords => super::TeleportSensitiveGroup::Passwords,
        G::History => super::TeleportSensitiveGroup::History,
    }
}

/// The review screen, when a plan is under review.
#[uniffi::export]
pub fn app_picker_review(state: AppPickerState) -> Option<AppReviewView> {
    core::teleport::flow::review(&state)
}

/// The key a review's site choice is remembered under (`<app>|<space>`).
#[uniffi::export]
pub fn app_review_remember_key(app: String, space: String) -> String {
    core::teleport::review::remember_key(&app, &space)
}

/// What was picked last time for `key` (an app and a Space), if anything.
#[uniffi::export]
pub fn app_review_remembered(
    choices: Vec<AppRememberedChoice>,
    key: String,
) -> Option<Vec<String>> {
    core::teleport::review::remembered(&choices, &key)
}

/// `choices` with the sites just sent remembered for `key`.
#[uniffi::export]
pub fn app_review_remember(
    choices: Vec<AppRememberedChoice>,
    key: String,
    domains: Vec<String>,
) -> Vec<AppRememberedChoice> {
    core::teleport::review::remember(&choices, &key, &domains)
}

/// The consent the confirmed review carries.
#[uniffi::export]
pub fn app_picker_consent(state: AppPickerState) -> AppTeleportConsent {
    core::teleport::flow::consent(&state)
}

/// Run progress in `[0, 1]`.
#[uniffi::export]
pub fn app_picker_progress(state: AppPickerState) -> f64 {
    core::teleport::flow::progress(&state)
}

/// What the run is doing, in words ("Packing profile", "Uploading 12 /
/// 80 MB"), for the line under the progress bar; none unless running.
#[uniffi::export]
pub fn app_picker_status(state: AppPickerState) -> Option<String> {
    core::teleport::flow::status(&state)
}

/// [`app_picker_status`] for a run's events as the SDK delivered them.
#[uniffi::export]
pub fn app_teleport_run_status(events: Vec<crate::TeleportRunEvent>) -> Option<String> {
    let events: Vec<_> = events.into_iter().map(app_teleport_run_event).collect();
    core::teleport::flow::run_status(&events)
}

/// An SDK run event as the picker's (`AppPickerEvent.progress`).
#[uniffi::export]
pub fn app_teleport_run_event(event: crate::TeleportRunEvent) -> AppTeleportRunEvent {
    use core::teleport::flow::RunPhase;
    AppTeleportRunEvent {
        step: event.step,
        steps: event.steps,
        kind: event.kind,
        phase: serde_json::from_value(serde_json::Value::String(event.phase))
            .unwrap_or(RunPhase::Progress),
        detail: event.detail,
        done_bytes: event.done_bytes,
        total_bytes: event.total_bytes,
    }
}

/// "12 MB".
#[uniffi::export]
pub fn app_format_bytes(bytes: u64) -> String {
    core::teleport::flow::format_bytes(bytes)
}

/// The primary button of the window picker.
#[uniffi::export]
pub fn app_picker_primary(tab: AppPickerTab, selected_supported: Option<bool>) -> AppPickerPrimary {
    core::teleport::windows::picker_primary(tab, selected_supported)
}

/// A Space's remote windows grouped by app.
#[uniffi::export]
pub fn app_group_windows_by_app(windows: Vec<AppRemoteWindow>) -> Vec<AppRemoteWindowGroup> {
    core::teleport::windows::group_windows_by_app(&windows)
}

/// Advances the transfer overlay (`None` hides it).
#[uniffi::export]
pub fn app_transfer_reduce(
    state: Option<AppTransferOverlayState>,
    signal: AppTransferSignal,
) -> Option<AppTransferOverlayState> {
    core::teleport::transfer::reduce(state.as_ref(), &signal)
}

/// The transfer overlay's title.
#[uniffi::export]
pub fn app_transfer_title(state: AppTransferOverlayState) -> String {
    core::teleport::transfer::title(&state)
}

/// A catalog row from `Teleport.catalog`, as the picker's entry.
#[uniffi::export]
pub fn app_catalog_entry(entry: crate::TeleportCatalogEntry) -> AppCatalogEntry {
    catalog_entry(entry)
}

fn catalog_entry(e: crate::TeleportCatalogEntry) -> AppCatalogEntry {
    use core::teleport::flow::{Capability, Move};
    AppCatalogEntry {
        id: e.id,
        name: e.name,
        host_path: e.host_path,
        host_app_id: e.host_app_id,
        version: e.version,
        capability: match e.capability {
            crate::TeleportCapability::Full => Capability::Full,
            crate::TeleportCapability::InstallOnly => Capability::InstallOnly,
            crate::TeleportCapability::Unsupported => Capability::Unsupported,
        },
        reason: e.reason,
        moves: e
            .moves
            .into_iter()
            .map(|m| match m {
                crate::TeleportMove::AppOnly => Move::AppOnly,
                crate::TeleportMove::AppWithFiles => Move::AppWithFiles,
                crate::TeleportMove::AppWithState => Move::AppWithState,
            })
            .collect(),
        provider_id: e.provider_id,
        sensitive_groups: e
            .sensitive_groups
            .into_iter()
            .map(|g| {
                use core::teleport::flow::SensitiveGroup as G;
                match g {
                    super::TeleportSensitiveGroup::SignIns => G::SignIns,
                    super::TeleportSensitiveGroup::Passwords => G::Passwords,
                    super::TeleportSensitiveGroup::History => G::History,
                }
            })
            .collect(),
        install_source: e.install_source,
        install_id: e.install_id,
        install_version: e.install_version,
        launch_bin: e.launch_bin,
        last_used_ms: e.last_used_ms.map(|v| v as i64),
        json: e.json,
    }
}

/// A plan from `Teleport.plan`, as the review's plan.
#[uniffi::export]
pub fn app_teleport_plan(plan: crate::TeleportPlan) -> AppTeleportPlan {
    use core::teleport::flow::{ConsentItem, ConsentKind, Move, PlanStepView};
    AppTeleportPlan {
        app: catalog_entry(plan.app),
        space_id: plan.space_id,
        moves: match plan.moves {
            crate::TeleportMove::AppOnly => Move::AppOnly,
            crate::TeleportMove::AppWithFiles => Move::AppWithFiles,
            crate::TeleportMove::AppWithState => Move::AppWithState,
        },
        steps: plan
            .steps
            .into_iter()
            .map(|s| PlanStepView {
                kind: s.kind,
                summary: s.summary,
            })
            .collect(),
        consent: plan
            .consent
            .into_iter()
            .map(|c| ConsentItem {
                kind: match c.kind {
                    crate::TeleportConsentKind::Install => ConsentKind::Install,
                    crate::TeleportConsentKind::File => ConsentKind::File,
                    crate::TeleportConsentKind::Folder => ConsentKind::Folder,
                    crate::TeleportConsentKind::State => ConsentKind::State,
                    crate::TeleportConsentKind::Secret => ConsentKind::Secret,
                },
                key: c.key,
                label: c.label,
                detail: c.detail,
                bytes: c.bytes,
                sensitive: c.sensitive,
            })
            .collect(),
        sensitive: plan.sensitive,
        total_bytes: plan.total_bytes,
        warnings: plan.warnings,
        relay_unsealed: plan.relay_unsealed,
        json: plan.json,
    }
}

// ---- Notch -----------------------------------------------------------------

/// Whether a Space takes a teleport drop (the notch tiles' and the Space
/// rows' rule).
#[uniffi::export]
pub fn app_space_accepts_drop(space: AppSpace) -> bool {
    core::notch::accepts_drop(&space)
}

/// The notch rectangle and panel frames for a screen.
#[uniffi::export]
pub fn app_notch_layout(screen: AppScreenFacts, prompt: bool) -> AppNotchLayout {
    core::notch::layout(&screen, prompt)
}

/// The notch panel's first state.
#[uniffi::export]
pub fn app_notch_initial() -> AppNotchState {
    core::notch::NotchState::default()
}

/// Advances the notch panel.
#[uniffi::export]
pub fn app_notch_reduce(state: AppNotchState, event: AppNotchEvent) -> AppNotchTransition {
    core::notch::reduce(&state, &event)
}

/// The notch panel as drawn.
#[uniffi::export]
pub fn app_notch_view(state: AppNotchState, spaces: Vec<AppSpace>) -> AppNotchView {
    core::notch::view(&state, &spaces)
}

/// The springs and delays every shell animates with.
#[uniffi::export]
pub fn app_notch_motion() -> AppNotchMotion {
    core::notch::MOTION
}

/// Closed and open corner radii.
#[uniffi::export]
pub fn app_notch_radii() -> Vec<AppNotchRadii> {
    vec![core::notch::CLOSED_RADII, core::notch::OPEN_RADII]
}

/// An OS icon's artwork (a single-color 24 x 24 SVG), by the id a tile
/// carries (`os-ubuntu`, ...).
#[uniffi::export]
pub fn app_os_icon_svg(id: String) -> Option<String> {
    core::notch::os_icon_svg(&id).map(str::to_string)
}

/// The system symbol drawn instead of the SVG on macOS (`apple.logo`).
#[uniffi::export]
pub fn app_os_icon_system_symbol(id: String) -> Option<String> {
    core::notch::os_icon_system_symbol(&id).map(str::to_string)
}

/// The drop-target tile under a point (AppKit coordinates) in the open
/// panel; `row` when the line above the tiles shows.
#[uniffi::export]
pub fn app_notch_tile_at(
    layout: AppNotchLayout,
    tiles: Vec<AppNotchTile>,
    row: bool,
    x: f64,
    y: f64,
) -> Option<String> {
    core::notch::tile_at(&layout, &tiles, row, x, y)
}

/// The window-drag trigger's idle state.
#[uniffi::export]
pub fn app_drag_trigger_initial() -> AppDragTriggerState {
    core::notch::drag_trigger::initial()
}

/// Advances the window-drag trigger over the notch displays: move or
/// resize, the trigger line, the dwell, expand and collapse.
#[uniffi::export]
pub fn app_drag_trigger_apply(
    state: AppDragTriggerState,
    event: AppDragTriggerEvent,
    displays: Vec<AppDragDisplay>,
) -> AppDragTriggerTransition {
    core::notch::drag_trigger::apply(&state, &event, &displays)
}

/// A window's frame at mouse down against a later one: move, resize, or
/// not known yet.
#[uniffi::export]
pub fn app_drag_classify(start: AppLogicalRect, now: AppLogicalRect) -> AppDragKind {
    core::notch::drag_trigger::classify(&start, &now)
}

/// The trigger geometry for screens (the primary first), in global
/// top-left points.
#[uniffi::export]
pub fn app_drag_displays(screens: Vec<AppScreenFacts>) -> Vec<AppDragDisplay> {
    core::notch::drag_trigger::displays(&screens)
}

/// The same for a portal whose expanded panel is its top-centred switcher
/// window (the Tauri app; the parity flows check it here too).
#[uniffi::export]
pub fn app_drag_portal_displays(screens: Vec<AppScreenFacts>) -> Vec<AppDragDisplay> {
    core::notch::drag_trigger::portal_displays(&screens)
}

/// The activity ring's fill without real progress, in thousandths.
#[uniffi::export]
pub fn app_notch_estimated_progress(elapsed_ms: i64, estimate_ms: u32) -> u32 {
    core::notch::estimated_progress(elapsed_ms, estimate_ms)
}

// ---- Keyvault --------------------------------------------------------------

/// The page chrome.
#[uniffi::export]
pub fn kv_page(overview: KeyvaultOverview, now_ms: i64) -> KvPage {
    core::keyvault::view::page(&overview, now_ms)
}

/// The always-visible signal while Keyvault sign-ins are live in a Space
/// (the notch indicator and the menu bar line); none when nothing is live.
#[uniffi::export]
pub fn kv_sharing_label(overview: KeyvaultOverview, now_ms: i64) -> Option<String> {
    core::keyvault::view::sharing_label(&overview, now_ms)
}

/// [`kv_sharing_label`] without the copies the user dismissed (import ids):
/// the notch indicator.
#[uniffi::export]
pub fn kv_visible_sharing_label(
    overview: KeyvaultOverview,
    now_ms: i64,
    dismissed: Vec<String>,
) -> Option<String> {
    core::keyvault::view::visible_sharing_label(&overview, now_ms, &dismissed)
}

/// The ids of `spaces` signed in through the Keyvault (a live copy is in
/// them), less the `dismissed` copies: "Signed in" in the Spaces list
/// (nothing dismissed), the key on notch tiles.
#[uniffi::export]
pub fn kv_signed_in_spaces(
    overview: KeyvaultOverview,
    now_ms: i64,
    dismissed: Vec<String>,
    spaces: Vec<AppSpace>,
) -> Vec<String> {
    core::keyvault::view::signed_in_spaces(&overview, now_ms, &dismissed, &spaces)
}

/// The dismissed copies still live.
#[uniffi::export]
pub fn kv_prune_dismissed(
    overview: KeyvaultOverview,
    now_ms: i64,
    dismissed: Vec<String>,
) -> Vec<String> {
    core::keyvault::view::prune_dismissed(&overview, now_ms, &dismissed)
}

/// The Access row of `space`'s copies, to focus from its "Signed in" badge.
#[uniffi::export]
pub fn kv_space_access_key(
    overview: KeyvaultOverview,
    now_ms: i64,
    space: AppSpace,
) -> Option<String> {
    core::keyvault::view::space_access_key(&overview, now_ms, &space)
}

/// The sidebar: All Items, Waiting, Access, Recent, and one row per app.
#[uniffi::export]
pub fn kv_sidebar(overview: KeyvaultOverview, now_ms: i64) -> KvSidebar {
    core::keyvault::browse::sidebar(&overview, now_ms)
}

/// The pane for a sidebar selection (the vault list is [`kv_vault_view`]).
#[uniffi::export]
pub fn kv_list(overview: KeyvaultOverview, selection: KvSelection, now_ms: i64) -> KvListView {
    core::keyvault::browse::list(&overview, &selection, now_ms)
}

/// The vault list's next state (search, selection, open groups).
#[uniffi::export]
pub fn kv_vault_reduce(
    overview: KeyvaultOverview,
    state: KvVaultState,
    action: KvVaultAction,
) -> KvVaultState {
    core::keyvault::vault::reduce(&overview, &state, &action)
}

/// The vault list: apps, sites and items with their locks, the selection and
/// the batch bar.
#[uniffi::export]
pub fn kv_vault_view(overview: KeyvaultOverview, state: KvVaultState, now_ms: i64) -> KvVaultView {
    core::keyvault::vault::view(&overview, &state, now_ms)
}

/// The selection without what the vault no longer holds.
#[uniffi::export]
pub fn kv_vault_prune(overview: KeyvaultOverview, state: KvVaultState) -> KvVaultState {
    core::keyvault::vault::prune(&overview, &state)
}

/// What the Keyvault holds for `provider_id` that a teleport can send.
#[uniffi::export]
pub fn kv_vault_source(overview: KeyvaultOverview, provider_id: String) -> KvVaultSource {
    core::keyvault::vault::vault_source(&overview, &provider_id)
}

/// The command that locks items.
#[uniffi::export]
pub fn kv_lock_command(ids: Vec<String>) -> KvCommand {
    core::keyvault::vault::lock_command(&ids)
}

/// The command that unlocks items (the daemon asks for Touch ID once).
#[uniffi::export]
pub fn kv_unlock_command(ids: Vec<String>) -> KvCommand {
    core::keyvault::vault::unlock_command(&ids)
}

/// The command that deletes items and wipes their copies in Spaces.
#[uniffi::export]
pub fn kv_delete_command(ids: Vec<String>) -> KvCommand {
    core::keyvault::vault::delete_command(&ids)
}

/// The unlock prompt for `count` items (`name`: the one item's name). None
/// when the user chose "Never ask again".
#[uniffi::export]
pub fn kv_unlock_prompt(
    overview: KeyvaultOverview,
    count: u32,
    name: Option<String>,
) -> Option<KvUnlockPrompt> {
    core::keyvault::vault::unlock_prompt(&overview, count, name.as_deref())
}

/// The unlock prompt whatever the setting says.
#[uniffi::export]
pub fn kv_unlock_prompt_always(count: u32, name: Option<String>) -> KvUnlockPrompt {
    core::keyvault::vault::unlock_prompt_always(count, name.as_deref())
}

/// The delete confirmation.
#[uniffi::export]
pub fn kv_delete_confirm(count: u32, live_copies: u32) -> KvDeleteConfirm {
    core::keyvault::vault::delete_confirm(count, live_copies)
}

/// How many Spaces hold a live copy of any of `ids`.
#[uniffi::export]
pub fn kv_live_copy_spaces(overview: KeyvaultOverview, ids: Vec<String>, now_ms: i64) -> u32 {
    core::keyvault::vault::live_copy_spaces(&overview, &ids, now_ms)
}

/// Opens the approval sheet with nothing selected.
#[uniffi::export]
pub fn kv_approval_open(request_id: String) -> KvApprovalState {
    core::keyvault::approval::open(&request_id)
}

/// Advances the approval sheet.
#[uniffi::export]
pub fn kv_approval_reduce(
    overview: KeyvaultOverview,
    state: KvApprovalState,
    action: KvApprovalAction,
) -> KvApprovalState {
    core::keyvault::approval::reduce(&overview, &state, &action)
}

/// The approval sheet as drawn.
#[uniffi::export]
pub fn kv_approval_view(overview: KeyvaultOverview, state: KvApprovalState) -> KvApprovalView {
    core::keyvault::approval::view(&overview, &state)
}

/// What Approve sends (none while it cannot).
#[uniffi::export]
pub fn kv_approval_approve_command(
    overview: KeyvaultOverview,
    state: KvApprovalState,
) -> Option<KvCommand> {
    core::keyvault::approval::approve_command(&overview, &state)
}

/// What Deny sends.
#[uniffi::export]
pub fn kv_approval_deny_command(state: KvApprovalState) -> KvCommand {
    core::keyvault::approval::deny_command(&state)
}

/// The Keyvault's fixed words.
#[uniffi::export]
pub fn kv_labels() -> KvLabels {
    core::keyvault::view::labels()
}

/// The banner after setup: the recovery key, shown once.
#[uniffi::export]
pub fn kv_recovery_key_text(key: String) -> String {
    core::keyvault::view::recovery_key_text(&key)
}

/// A duration in words ("5 min").
#[uniffi::export]
pub fn kv_duration(ms: i64) -> String {
    core::keyvault::view::duration(ms)
}

/// The setup or unlock form for an overview (also `KvPage.form`): Touch ID
/// when the daemon can use the OS key store, else a passphrase.
#[uniffi::export]
pub fn kv_credential_form(overview: KeyvaultOverview) -> Option<KvCredentialForm> {
    core::keyvault::credential::credential_form(&overview)
}

/// The passphrase fields' hint and whether the form can be sent. Pure: the
/// passphrase is only measured, never kept.
#[uniffi::export]
pub fn kv_passphrase_check(
    mode: KvFormMode,
    passphrase: String,
    confirm: String,
) -> KvPassphraseCheck {
    let passphrase = core::keyvault::client::Zeroizing::new(passphrase);
    let confirm = core::keyvault::client::Zeroizing::new(confirm);
    core::keyvault::credential::passphrase_check(mode, &passphrase, &confirm)
}

fn kv_error(f: core::keyvault::client::KvFailure) -> CuaError {
    match f.code.as_str() {
        "not_running" => CuaError::DaemonNotRunning(f.message),
        "connect" => CuaError::Transport(f.message),
        "not_found" => CuaError::NotFound(f.message),
        "forbidden" | "denied" | "presence_failed" => CuaError::PermissionDenied(f.message),
        "invalid" | "locked" | "no_vault" | "disabled" => CuaError::InvalidArgument(f.message),
        "unsupported" => CuaError::Unsupported(f.message),
        "impostor" | "not_first_party" => CuaError::Unauthenticated(f.message),
        _ => CuaError::Internal(format!("{}: {}", f.code, f.message)),
    }
}

/// The Keyvault broker client: the verified connection to the broker
/// `cua daemon` hosts on `$CUA_HOME/keyvault.sock`. The broker checks this
/// process's code signature; approvals, re-enabling and unattended-on make
/// the daemon ask for Touch ID. No call returns a secret value.
#[derive(uniffi::Object)]
pub struct KeyvaultClient {
    inner: core::keyvault::client::KeyvaultCommands,
}

#[uniffi::export]
impl KeyvaultClient {
    /// The broker in `cua_home` (`None`: `$CUA_HOME`, else `~/.cua`).
    #[uniffi::constructor]
    pub fn new(cua_home: Option<String>) -> Arc<Self> {
        let home = cua_home.map(PathBuf::from).unwrap_or_else(|| {
            std::env::var_os("CUA_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    std::env::var_os("HOME")
                        .map(PathBuf::from)
                        .unwrap_or_default()
                        .join(".cua")
                })
        });
        Arc::new(Self {
            inner: core::keyvault::client::KeyvaultCommands::for_cua_home(&home),
        })
    }

    /// Everything the page shows. Never fails: an unavailable Keyvault is a
    /// state (`availability`).
    pub async fn overview(&self) -> KeyvaultOverview {
        let inner = self.inner.clone();
        cua_sdk::support::run(async move { Ok(inner.overview().await) })
            .await
            .unwrap_or_default()
    }

    /// Site icons the vault holds (not secret; empty while names are hidden).
    pub async fn favicons(&self) -> Vec<KvFavicon> {
        let inner = self.inner.clone();
        cua_sdk::support::run(async move { Ok(inner.favicons().await) })
            .await
            .unwrap_or_default()
    }

    /// Runs one page action (a broker request).
    pub async fn execute(&self, command: KvCommand) -> Result<KvOutcome> {
        let inner = self.inner.clone();
        cua_sdk::support::run(async move { inner.execute(&command).await.map_err(kv_error) }).await
    }

    /// Creates the vault with a passphrase (and a recovery key, returned
    /// once). The passphrase goes only to the broker over the verified
    /// Keyvault socket; it is never logged or kept, and zeroized here.
    pub async fn setup_with_passphrase(&self, passphrase: String) -> Result<Option<String>> {
        let inner = self.inner.clone();
        let passphrase = core::keyvault::client::Zeroizing::new(passphrase);
        cua_sdk::support::run(async move {
            inner
                .setup_with_passphrase(passphrase)
                .await
                .map_err(kv_error)
        })
        .await
    }

    /// Unlocks with the passphrase (sent only to the broker; zeroized here).
    pub async fn unlock_with_passphrase(&self, passphrase: String) -> Result<()> {
        let inner = self.inner.clone();
        let passphrase = core::keyvault::client::Zeroizing::new(passphrase);
        cua_sdk::support::run(async move {
            inner
                .unlock_with_passphrase(passphrase)
                .await
                .map_err(kv_error)
        })
        .await
    }

    /// Locks the vault.
    pub async fn lock(&self) -> Result<()> {
        let inner = self.inner.clone();
        cua_sdk::support::run(async move { inner.lock().await.map_err(kv_error) }).await
    }

    /// What `app` holds, per domain with counts (the daemon asks for Touch ID
    /// when the browse window is closed). Never a value.
    pub async fn inventory(&self, app: String, profile: Option<String>) -> Result<KvInventory> {
        let inner = self.inner.clone();
        cua_sdk::support::run(async move { inner.inventory(&app, profile).await.map_err(kv_error) })
            .await
    }
}

// ---- Onboarding and settings ---------------------------------------------

/// First run's first state.
#[uniffi::export]
pub fn app_onboarding_initial(
    installer_mode: Option<AppOnboardingMode>,
    identity: Option<String>,
) -> AppOnboardingState {
    core::onboarding::initial(installer_mode, identity)
}

/// Advances first run.
#[uniffi::export]
pub fn app_onboarding_reduce(
    state: AppOnboardingState,
    action: AppOnboardingAction,
) -> AppOnboardingState {
    core::onboarding::reduce(&state, &action)
}

/// First run as drawn.
#[uniffi::export]
pub fn app_onboarding_view(state: AppOnboardingState) -> AppOnboardingView {
    core::onboarding::view(&state)
}

/// The SDK's billing status as the app core takes it (Settings' Billing
/// row).
#[uniffi::export]
pub fn app_billing_status(status: cua_sdk::FleetBillingStatus) -> AppBillingStatus {
    core::billing::BillingStatus {
        billing_enabled: status.billing_enabled,
        card: status.card.map(|c| core::billing::BillingCard {
            brand: c.brand,
            last4: c.last4,
        }),
        credit: status.credit.map(|c| core::billing::BillingCredit {
            balance_usd_cents: c.balance_usd_cents,
        }),
        billing_url: status.billing_url,
    }
}

/// Reads the settings file (defaults when missing or damaged).
#[uniffi::export]
pub fn app_settings_load(path: String) -> AppSettings {
    core::settings::load(std::path::Path::new(&path))
}

/// Writes the settings file.
#[uniffi::export]
pub fn app_settings_save(path: String, settings: AppSettings) -> Result<()> {
    core::settings::save(std::path::Path::new(&path), &settings)
        .map_err(|e| CuaError::Internal(e.to_string()))
}

/// The Settings page.
#[uniffi::export]
pub fn app_settings_page(input: AppSettingsInput) -> AppSettingsPage {
    core::settings::page(&input)
}

/// The Settings page with Settings, Storage after General while the Cua
/// Volume experiment is on (left out while it is off).
#[uniffi::export]
pub fn app_settings_with_storage(
    page: AppSettingsPage,
    storage: AppSettingsSection,
    experiments: AppExperiments,
) -> AppSettingsPage {
    core::settings::with_storage(&page, &storage, &experiments)
}

/// Settings, Experiments: one switch and one line per experiment.
#[uniffi::export]
pub fn app_experiments_page(experiments: AppExperiments) -> AppSettingsPage {
    core::experiments::page(&experiments)
}

/// The switches after a row's choice (`on` or `off`).
#[uniffi::export]
pub fn app_experiments_choose(
    experiments: AppExperiments,
    row: String,
    option: String,
) -> AppExperiments {
    core::experiments::choose(&experiments, &row, &option)
}

/// The switches from JSON (fixtures, parity).
#[uniffi::export]
pub fn app_experiments_from_json(json: String) -> Result<AppExperiments> {
    from_json("experiments", &json)
}

/// What the app does about launch at login at launch (the user's choice,
/// whether the first run finished, whether this machine provides Spaces or
/// runs persistent agents, and what the system reports).
#[uniffi::export]
pub fn app_login_item_launch_plan(
    choice: Option<bool>,
    onboarded: bool,
    serves: bool,
    status: AppLoginItemStatus,
) -> AppLoginItemPlan {
    core::login_item::launch_plan(choice, onboarded, serves, status)
}

/// First run's fixed words.
#[uniffi::export]
pub fn app_onboarding_copy() -> AppOnboardingCopy {
    core::onboarding::copy()
}

/// The Sign in page's line once signed in.
#[uniffi::export]
pub fn app_onboarding_signed_in_text(identity: Option<String>) -> String {
    core::onboarding::signed_in_text(identity.as_deref())
}

/// The Sign in page's line while the browser is open.
#[uniffi::export]
pub fn app_onboarding_sign_in_code_text(user_code: Option<String>) -> String {
    core::onboarding::sign_in_code_text(user_code.as_deref())
}

/// The install target's tooltip when a `cua` is already there.
#[uniffi::export]
pub fn app_onboarding_replaces_text(installed_version: Option<String>) -> String {
    core::onboarding::replaces_text(installed_version.as_deref())
}

/// The Command line page's line once `cua` is current.
#[uniffi::export]
pub fn app_onboarding_installed_at_text(target: String) -> String {
    core::onboarding::installed_at_text(&target)
}

/// A presentation card's animated miniature (the notch card, or with
/// `menu_bar` the menu bar card).
#[uniffi::export]
pub fn app_presentation_preview(menu_bar: bool) -> AppPresentationPreview {
    core::onboarding_preview::preview(menu_bar)
}

/// The miniature `t_ms` into its loop (it wraps).
#[uniffi::export]
pub fn app_presentation_preview_frame(menu_bar: bool, t_ms: u32) -> AppPreviewFrame {
    core::onboarding_preview::frame(menu_bar, t_ms)
}

/// The miniature with Reduce Motion: expanded, still.
#[uniffi::export]
pub fn app_presentation_preview_still(menu_bar: bool) -> AppPreviewFrame {
    core::onboarding_preview::still(menu_bar)
}

/// The AI agents page's background computer-use card: its miniature (an
/// agent clicking in a background window while the user works in front).
#[uniffi::export]
pub fn app_driver_preview() -> AppDriverPreview {
    core::driver_preview::preview()
}

/// The card's miniature `t_ms` into its loop (it wraps).
#[uniffi::export]
pub fn app_driver_preview_frame(t_ms: u32) -> AppDriverFrame {
    core::driver_preview::frame(t_ms)
}

/// The card's miniature with Reduce Motion: mid-task, still.
#[uniffi::export]
pub fn app_driver_preview_still() -> AppDriverFrame {
    core::driver_preview::still()
}

/// The first run's Cua Volume page: its miniature (files from a Space
/// arriving in the Cua Volume volume in Finder).
#[uniffi::export]
pub fn app_drive_mount_preview() -> AppDriveMountPreview {
    core::drive_mount_preview::preview()
}

/// The page's miniature `t_ms` into its loop (it wraps).
#[uniffi::export]
pub fn app_drive_mount_preview_frame(t_ms: u32) -> AppDriveMountFrame {
    core::drive_mount_preview::frame(t_ms)
}

/// The page's miniature with Reduce Motion: files arriving, still.
#[uniffi::export]
pub fn app_drive_mount_preview_still() -> AppDriveMountFrame {
    core::drive_mount_preview::still()
}

/// `volume_mount_status`'s JSON as the core takes it.
#[uniffi::export]
pub fn app_drive_mount_from_json(json: String) -> Result<AppDriveMountInput> {
    from_json("drive mount status", &json)
}

/// The menu bar item's menu: the Spaces the user can open (the notch's
/// count), Cua Volume's sync state next to it and its conflicts, then the
/// actions.
#[uniffi::export]
pub fn app_menu(input: AppMenuInput) -> Vec<AppMenuItem> {
    core::window::menu(&input)
}

/// The Spaces the user can open (the notch tab and the menu bar item).
#[uniffi::export]
pub fn app_openable_count(spaces: Vec<AppSpace>) -> u32 {
    core::spaces::openable_count(&spaces)
}

/// `volume_sync_status`'s JSON as the core takes it.
#[uniffi::export]
pub fn app_drive_sync_from_json(json: String) -> Result<AppDriveSyncInput> {
    from_json("drive sync status", &json)
}

/// A hotkey label (`⌘⇧Space`), or none.
#[uniffi::export]
pub fn app_format_hotkey(combo: AppKeyCombo) -> Option<String> {
    core::settings::format_hotkey(&combo)
}

/// Home-relative path (`~/x`).
#[uniffi::export]
pub fn app_display_path(path: String, home: Option<String>) -> String {
    core::paths::display_path(&path, home.as_deref())
}

/// A coding agent's display name.
#[uniffi::export]
pub fn app_agent_name(agent: String) -> String {
    core::agents::agent_name(&agent)
}

// ---- Agent runs -----------------------------------------------------------

/// A Space's agent rows from its run records (`Space.agent_list()`'s
/// `json`, one record each): attention first, then newest. The same mapping
/// the Tauri app uses.
#[uniffi::export]
pub fn app_agent_rows(runs_json: Vec<String>) -> Vec<AppSpaceAgentRun> {
    let runs: Vec<serde_json::Value> = runs_json
        .iter()
        .map(|j| serde_json::from_str(j).unwrap_or(serde_json::Value::Null))
        .collect();
    core::agents::agent_rows_from_json(&runs)
}

/// A run's status word ("Running", "Idle", ...).
#[uniffi::export]
pub fn app_agent_status_label(status: AppAgentStatus) -> String {
    status.label().to_string()
}

/// The line under an agent's name.
#[uniffi::export]
pub fn app_agent_subtitle(run: AppSpaceAgentRun) -> String {
    core::agents::agent_subtitle(&run)
}

// ---- Coding agents on this machine --------------------------------------

/// Settings rows for the coding agents (`total`: bundled skills).
#[uniffi::export]
pub fn app_agent_settings_rows(
    statuses: Vec<AppAgentSetupStatus>,
    total: u32,
) -> Vec<AppAgentSettingsRow> {
    core::agents::agent_settings_rows(&statuses, total)
}

/// What an agent setup did for one agent.
#[uniffi::export]
pub fn app_agent_setup_summary(
    outcomes: Vec<AppAgentSetupOutcomeInput>,
    agent: String,
    name: String,
) -> AppAgentSetupSummary {
    core::agents::agent_setup_summary(&outcomes, &agent, &name)
}

/// The SDK's agent detection as the app core reads it.
#[uniffi::export]
pub fn app_agent_setup_status(info: cua_sdk::AgentInfo) -> AppAgentSetupStatus {
    AppAgentSetupStatus {
        id: info.id,
        name: info.name,
        installed: info.installed,
        skills_dir: info.skills_dir,
        mcp_config: info.mcp_config,
        cua_configured: info.cua_configured,
        skills_installed: info.skills_installed,
        skills_outdated: info.skills_outdated,
        error: info.error,
    }
}

/// The SDK's setup outcome as the app core reads it.
#[uniffi::export]
pub fn app_agent_setup_outcome(outcome: cua_sdk::AgentSetupOutcome) -> AppAgentSetupOutcomeInput {
    use cua_sdk::{AgentSetupChange as C, AgentSetupTarget as T};
    AppAgentSetupOutcomeInput {
        agents: outcome.agents,
        target: match outcome.target {
            T::Skill => "skill",
            T::Mcp => "mcp",
        }
        .into(),
        item: outcome.item,
        change: match outcome.change {
            C::Created => "created",
            C::Updated => "updated",
            C::Unchanged => "unchanged",
            C::Removed => "removed",
            C::Skipped => "skipped",
            C::Failed => "failed",
        }
        .into(),
        detail: outcome.detail,
    }
}

// ---- Teleport drops ---------------------------------------------------------

/// The drop well's line while files go.
#[uniffi::export]
pub fn app_drop_sending_text(paths: Vec<String>) -> String {
    core::teleport::transfer::drop_sending_text(&paths)
}

/// The drop well's line once they landed.
#[uniffi::export]
pub fn app_drop_sent_text(files: Vec<AppSentFileInfo>) -> String {
    core::teleport::transfer::drop_sent_text(&files)
}

/// A `send_file` report as the files the drop well names.
#[uniffi::export]
pub fn app_sent_file(report: cua_sdk::SpaceSendFileReport) -> AppSentFileInfo {
    let name = report
        .source
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(&report.source)
        .to_string();
    AppSentFileInfo {
        name,
        dest: report.dest,
        bytes: report.bytes,
    }
}

// ---- First run: the command line and host setup -------------------------

/// Normalises and checks a host setup request (the Tauri app's
/// `host_setup` runs the same check before any service is touched).
#[uniffi::export]
pub fn app_host_validate_setup(request: AppHostSetupRequest) -> Result<AppHostSetupRequest> {
    core::host::validate_setup(request).map_err(CuaError::InvalidArgument)
}

/// Comma or space separated accounts, as the host setup form's "Also allow".
#[uniffi::export]
pub fn app_host_parse_allow_list(value: String) -> Vec<String> {
    core::host::parse_allow_list(&value)
}

/// Whether a direct-mode address is `ip:port` or `host:port`.
#[uniffi::export]
pub fn app_host_looks_like_listen(value: String) -> bool {
    core::host::looks_like_listen(&value)
}

/// Puts the app's bundled `cua` on PATH on first launch (there is no
/// onboarding page for it). The same installer the Tauri app runs;
/// [`plan`](Self::plan) reads what installing would do without writing.
#[derive(uniffi::Object)]
pub struct AppCliInstaller {
    inner: core::installer::CliInstaller,
}

#[uniffi::export]
impl AppCliInstaller {
    /// This machine: the `cua` next to `executable` (the app's own
    /// executable), `$HOME`, `$PATH` and `$SHELL`.
    #[uniffi::constructor]
    pub fn for_executable(executable: String) -> Arc<Self> {
        Arc::new(Self {
            inner: core::installer::CliInstaller::from_env(std::path::Path::new(&executable)),
        })
    }

    /// Explicit paths (tests): the bundled CLI, the bin directory, the PATH
    /// value to check and the shell profile a PATH change edits.
    #[uniffi::constructor]
    pub fn with_paths(
        bundled: Option<String>,
        bin_dir: String,
        path_env: String,
        profile: String,
    ) -> Arc<Self> {
        Arc::new(Self {
            inner: core::installer::CliInstaller {
                bundled: bundled.map(PathBuf::from),
                bin_dir: bin_dir.into(),
                path_env,
                profile: profile.into(),
                prefer_symlink: false,
            },
        })
    }

    /// What installing would do (nothing is written).
    pub async fn plan(&self) -> AppCliInstallPlan {
        let inner = self.inner.clone();
        cua_sdk::support::run(async move { Ok(inner.plan().await) })
            .await
            .unwrap_or_default()
    }

    /// Installs the CLI (after the user consented to the plan).
    pub async fn install(&self, request: AppCliInstallRequest) -> Result<AppCliInstallPlan> {
        let inner = self.inner.clone();
        cua_sdk::support::run(
            async move { inner.install(&request).await.map_err(CuaError::Runtime) },
        )
        .await
    }
}

// ---- Usage telemetry ------------------------------------------------------------

/// The app started: attributes this process's events to `spaces_app` at
/// `version`, lets the app show the first-run notice itself (nothing is
/// sent before it has been shown once), and records `app_launched` (on a
/// first run, once the notice shows). Call once at launch, before the
/// first `Cua`. Returns whether the notice was already shown here.
#[uniffi::export]
pub fn app_telemetry_start(version: String) -> bool {
    let t = cua_telemetry::global();
    t.set_product("spaces_app", &version);
    t.set_notice_mode(cua_telemetry::NoticeMode::External);
    core::telemetry::start(t)
}

/// Records `signals` on the SDK's telemetry client (the switch, the notice
/// and the schema apply) and marks the day active. Returns how many events
/// were queued.
#[uniffi::export]
pub fn app_telemetry_record(signals: Vec<AppTelemetrySignal>) -> u32 {
    core::telemetry::record(cua_telemetry::global(), &signals) as u32
}

/// The app started (`app_launched`).
#[uniffi::export]
pub fn app_telemetry_launched() -> Vec<AppTelemetrySignal> {
    core::telemetry::launched()
}

/// A Spaces app feature (a fixed name).
#[uniffi::export]
pub fn app_telemetry_feature(feature: String) -> Vec<AppTelemetrySignal> {
    core::telemetry::feature_used(&feature)
}

/// The first run left Welcome with its usage-data switch at `on`: writes
/// the machine's setting when it changed (the same as Settings and `cua
/// telemetry off`), then records that the notice was shown. Nothing is
/// queued or sent before; with the switch off, nothing after either.
#[uniffi::export]
pub fn app_telemetry_welcome_left(on: bool) -> Result<()> {
    core::telemetry::welcome_left(cua_telemetry::global(), on)
        .map_err(|e| CuaError::Internal(e.to_string()))
}

/// The events one first-run step means (call with the state before it).
#[uniffi::export]
pub fn app_telemetry_onboarding(
    state: AppOnboardingState,
    action: AppOnboardingAction,
) -> Vec<AppTelemetrySignal> {
    core::telemetry::onboarding(&state, &action)
}

/// "Start using Cua Spaces".
#[uniffi::export]
pub fn app_telemetry_onboarding_finished(state: AppOnboardingState) -> Vec<AppTelemetrySignal> {
    core::telemetry::onboarding_finished(&state)
}

/// The events one step of the Spaces being created means, at `now_ms`
/// (call with the state before it).
#[uniffi::export]
pub fn app_telemetry_creates(
    state: AppCreatesState,
    action: AppCreateAction,
    now_ms: i64,
) -> Vec<AppTelemetrySignal> {
    core::telemetry::creates(&state, &action, now_ms)
}

/// The events one step of Settings, Storage means.
#[uniffi::export]
pub fn app_telemetry_storage(
    input: AppStorageInput,
    state: AppStorageState,
    action: AppStorageAction,
) -> Vec<AppTelemetrySignal> {
    core::telemetry::storage(&input, &state, &action)
}

/// The events one step of the Share sheet means.
#[uniffi::export]
pub fn app_telemetry_share(
    input: AppShareInput,
    state: AppShareSheetState,
    action: AppShareSheetAction,
) -> Vec<AppTelemetrySignal> {
    core::telemetry::share(&input, &state, &action)
}

/// The events of a change in Settings, Experiments (call with the switches
/// before and after it).
#[uniffi::export]
pub fn app_telemetry_experiments_changed(
    before: AppExperiments,
    after: AppExperiments,
) -> Vec<AppTelemetrySignal> {
    core::telemetry::experiments_changed(&before, &after)
}

/// Which experiments are on (at launch).
#[uniffi::export]
pub fn app_telemetry_experiments_on(experiments: AppExperiments) -> Vec<AppTelemetrySignal> {
    core::telemetry::experiments_on(&experiments)
}

/// Settings, Storage's state from JSON (fixtures, parity).
#[uniffi::export]
pub fn app_storage_state_from_json(json: String) -> Result<AppStorageState> {
    from_json("storage state", &json)
}

/// The Share sheet's state from JSON (fixtures, parity).
#[uniffi::export]
pub fn app_share_state_from_json(json: String) -> Result<AppShareSheetState> {
    from_json("share state", &json)
}

/// The enroll sheet's state from JSON (fixtures, parity).
#[uniffi::export]
pub fn app_enroll_state_from_json(json: String) -> Result<AppEnrollState> {
    from_json("enroll state", &json)
}

/// The events one step of the enroll sheet means.
#[uniffi::export]
pub fn app_telemetry_enroll(
    state: AppEnrollState,
    action: AppEnrollAction,
) -> Vec<AppTelemetrySignal> {
    core::telemetry::enroll(&state, &action)
}

// ---- Fixtures ----------------------------------------------------------------

fn from_json<T: serde::de::DeserializeOwned>(what: &str, json: &str) -> Result<T> {
    serde_json::from_str(json).map_err(|e| CuaError::InvalidArgument(format!("{what}: {e}")))
}

/// The menu bar item's menu input from JSON (fixtures, parity).
#[uniffi::export]
pub fn app_menu_input_from_json(json: String) -> Result<AppMenuInput> {
    from_json("menu input", &json)
}

/// Spaces from the webview's JSON shape (fixtures, parity flows).
#[uniffi::export]
pub fn app_spaces_from_json(json: String) -> Result<Vec<AppSpace>> {
    from_json("spaces", &json)
}

/// Registry rows from JSON (fixtures, parity flows).
#[uniffi::export]
pub fn app_space_rows_from_json(json: String) -> Result<Vec<AppSpaceRow>> {
    from_json("rows", &json)
}

/// Picker catalog entries from JSON (fixtures, parity flows).
#[uniffi::export]
pub fn app_catalog_entries_from_json(json: String) -> Result<Vec<AppCatalogEntry>> {
    from_json("entries", &json)
}

/// A teleport plan from JSON (fixtures, parity flows).
#[uniffi::export]
pub fn app_teleport_plan_from_json(json: String) -> Result<AppTeleportPlan> {
    from_json("plan", &json)
}

/// A host state from JSON (fixtures, parity flows).
#[uniffi::export]
pub fn app_host_state_from_json(json: String) -> Result<AppHostState> {
    from_json("host state", &json)
}

/// A host setup form action from JSON (parity flows).
#[uniffi::export]
pub fn app_host_form_action_from_json(json: String) -> Result<AppHostFormAction> {
    from_json("host form action", &json)
}

/// The chrome's input from JSON (parity flows).
#[uniffi::export]
pub fn app_chrome_input_from_json(json: String) -> Result<AppChromeInput> {
    from_json("chrome input", &json)
}

/// The Settings page's input from JSON (parity flows).
#[uniffi::export]
pub fn app_settings_input_from_json(json: String) -> Result<AppSettingsInput> {
    from_json("settings input", &json)
}

/// Coding agents' setup states from JSON (fixtures, parity flows).
#[uniffi::export]
pub fn app_agent_setup_statuses_from_json(json: String) -> Result<Vec<AppAgentSetupStatus>> {
    from_json("agent statuses", &json)
}

/// Agent setup outcomes from JSON (parity flows).
#[uniffi::export]
pub fn app_agent_setup_outcomes_from_json(json: String) -> Result<Vec<AppAgentSetupOutcomeInput>> {
    from_json("agent outcomes", &json)
}

/// A first-run action from JSON (parity flows).
#[uniffi::export]
pub fn app_onboarding_action_from_json(json: String) -> Result<AppOnboardingAction> {
    from_json("onboarding action", &json)
}

/// Sent files from JSON (parity flows).
#[uniffi::export]
pub fn app_sent_files_from_json(json: String) -> Result<Vec<AppSentFileInfo>> {
    from_json("sent files", &json)
}

/// A Keyvault overview from JSON (fixtures, parity flows). Fixture data
/// only: it carries metadata, never a secret value.
#[uniffi::export]
pub fn kv_overview_from_json(json: String) -> Result<KeyvaultOverview> {
    from_json("overview", &json)
}

// ---- Host setup from the first-run form -----------------------------------

/// Binding options for a first-run form request, after the app core
/// validated it.
pub(crate) fn setup_options_for_request(
    request: AppHostSetupRequest,
) -> Result<cua_sdk::HostSetupOptions> {
    let request = core::host::validate_setup(request).map_err(CuaError::InvalidArgument)?;
    Ok(cua_sdk::HostSetupOptions {
        mode: Some(request.mode),
        relay_url: request.relay_url,
        direct: request.direct,
        name: request.name,
        allow: request.allow.unwrap_or_default(),
        driver_bin: None,
        runner: None,
        profile: request.profile,
        share_desktop: request.share_desktop,
        provide_spaces: request.provide_spaces,
    })
}

/// Host setup from the first-run form, as both Spaces apps send it:
/// validated by the app core ([`app_host_validate_setup`]), then the SDK's
/// `Host.setup` (the Swift app calls it as `host.setupRequest(...)`).
#[uniffi::export]
pub async fn app_host_setup(
    host: Arc<cua_sdk::Host>,
    request: AppHostSetupRequest,
    account_token: Option<String>,
) -> Result<cua_sdk::HostStatus> {
    let options = setup_options_for_request(request)?;
    host.setup(options, account_token).await
}

#[cfg(test)]
mod host_setup_tests {
    use super::*;

    #[test]
    fn form_requests_are_validated_by_the_core_then_mapped() {
        let o = setup_options_for_request(AppHostSetupRequest {
            mode: " Direct ".into(),
            direct: Some("10.0.0.2:4000".into()),
            relay_url: Some("https://ignored".into()),
            name: Some("  ".into()),
            allow: Some(vec![" a@b ".into(), "".into()]),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(o.mode.as_deref(), Some("direct"));
        assert_eq!(o.relay_url, None);
        assert_eq!(o.name, None);
        assert_eq!(o.allow, vec!["a@b".to_string()]);
        assert_eq!(o.direct.as_deref(), Some("10.0.0.2:4000"));
        assert!(
            setup_options_for_request(AppHostSetupRequest {
                mode: "direct".into(),
                direct: Some("nowhere".into()),
                ..Default::default()
            })
            .is_err()
        );
        // A spare machine (the form's profile) reaches the SDK as such
        // (cua-sdk's own tests map it to cua-host).
        let o = setup_options_for_request(AppHostSetupRequest {
            mode: "relay".into(),
            profile: Some("spare".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(o.profile.as_deref(), Some("spare"));
    }
}

#[cfg(test)]
mod size_limit_tests {
    use cua_spaces_app_core::wizard::{CLOUD_CPU_RANGE, CLOUD_MEMORY_GB_RANGE};

    /// The wizard's cloud sliders offer the everyday range
    /// (`cua_fleet::CLOUD_DEFAULT_RANGE_*`, what `fleet_size_limits`
    /// returns), which sits inside what the SDK accepts
    /// (`cua_fleet::FLEET_ABSOLUTE_*`).
    #[test]
    fn the_wizard_offers_the_everyday_cloud_sizes() {
        let (cpus, mem) = (
            cua_fleet::CLOUD_DEFAULT_RANGE_CPUS,
            cua_fleet::CLOUD_DEFAULT_RANGE_MEMORY_MB,
        );
        assert_eq!(CLOUD_CPU_RANGE, (*cpus.start(), *cpus.end()));
        assert!(CLOUD_MEMORY_GB_RANGE.0 * 1024 >= *mem.start());
        assert_eq!(CLOUD_MEMORY_GB_RANGE.1 * 1024, *mem.end());
        let l = cua_sdk::fleet_size_limits();
        assert_eq!((l.min_cpus, l.max_cpus), (*cpus.start(), *cpus.end()));
        assert_eq!(
            (l.min_memory_mb, l.max_memory_mb),
            (*mem.start(), *mem.end())
        );
        let (abs_cpus, abs_mem) = (
            cua_fleet::FLEET_ABSOLUTE_CPUS,
            cua_fleet::FLEET_ABSOLUTE_MEMORY_MB,
        );
        assert!(abs_cpus.contains(cpus.start()) && abs_cpus.contains(cpus.end()));
        assert!(abs_mem.contains(mem.start()) && abs_mem.contains(mem.end()));
    }
}
