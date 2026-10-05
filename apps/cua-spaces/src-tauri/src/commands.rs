// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Tauri commands over [`crate::core::AppCore`]: each is a one-line call into
//! the SDK layer plus, where the UI needs it, an event.
//!
//! Events: `spaces:changed` after the roster changes, `hotspot:changed`
//! with a [`HotspotStatus`], and the `auth:*` sign-in events.

use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::agents::SpaceAgentRun;
use crate::core::{
    AppCore, CmdResult, DaemonStatus, DefaultLocation, FleetStatus, HotspotStatus, LocalStatus,
    RemoteWindow, SentFile, SpaceCreateConfig, SpaceRow, StreamOpts, StreamTargetArg,
    StreamTicketInfo, TeleportResult, TransferManifest,
};
use crate::viewer_windows::{LastPush, LastPushState, TransferMap, TransferState};

/// Tauri-managed handle on the core.
pub struct AppState(pub Arc<AppCore>);

fn changed(app: &AppHandle) {
    let _ = app.emit("spaces:changed", ());
}

fn hotspot_changed(app: &AppHandle, status: &HotspotStatus) {
    let _ = app.emit("hotspot:changed", status);
}

// ------------------------------------------------------------- account

#[tauri::command]
pub async fn fleet_status(
    state: State<'_, AppState>,
    probe: Option<bool>,
) -> CmdResult<FleetStatus> {
    Ok(state.0.fleet_status(probe.unwrap_or(false)).await)
}

/// What `begin_sign_in` returns at once; approval arrives as an event.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignInStart {
    /// `browser` (finish in the browser tab just opened) or `device` (enter
    /// `user_code` at `verification_uri`).
    pub method: String,
    /// Device flow only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_code: Option<String>,
    /// The URL that was opened (sign-in page or device verification page).
    pub verification_uri: String,
}

#[derive(Clone, Debug, Serialize)]
struct SignedInPayload {
    #[serde(skip_serializing_if = "Option::is_none")]
    identity: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
struct SignInFailedPayload {
    reason: String,
}

/// "Sign in to Cua" through the SDK (`cua-auth`): browser sign-in with a
/// loopback redirect, else a device code. Opens the URL and finishes in the
/// background; emits `auth:signed-in` or `auth:sign-in-failed`. The session
/// lands in the store the cua CLI and daemon share.
#[tauri::command]
pub async fn begin_sign_in(app: AppHandle, state: State<'_, AppState>) -> CmdResult<SignInStart> {
    let core = state.0.clone();
    let pending = core.session().begin_login().await?;
    if let Err(error) = crate::open_web_url(&pending.url) {
        eprintln!("[cua-spaces] could not open sign-in URL: {error}");
    }
    let start = SignInStart {
        method: match pending.method {
            crate::auth::Method::Browser => "browser".into(),
            crate::auth::Method::Device => "device".into(),
        },
        user_code: pending.user_code.clone(),
        verification_uri: pending.url.clone(),
    };
    tauri::async_runtime::spawn(async move {
        match pending.complete().await {
            Ok(creds) => match core.install_user_session(creds).await {
                Ok(identity) => {
                    let _ = app.emit("auth:signed-in", SignedInPayload { identity });
                    changed(&app);
                }
                Err(reason) => {
                    let _ = app.emit("auth:sign-in-failed", SignInFailedPayload { reason });
                }
            },
            Err(e) => {
                let _ = app.emit(
                    "auth:sign-in-failed",
                    SignInFailedPayload {
                        reason: e.to_string(),
                    },
                );
            }
        }
    });
    Ok(start)
}

#[tauri::command]
pub async fn sign_out(app: AppHandle, state: State<'_, AppState>) -> CmdResult<()> {
    state.0.sign_out().await;
    let _ = app.emit("auth:signed-out", ());
    hotspot_changed(&app, &state.0.hotspot_status());
    changed(&app);
    Ok(())
}

// -------------------------------------------------------- daemon, local

#[tauri::command]
pub async fn daemon_status(state: State<'_, AppState>) -> CmdResult<DaemonStatus> {
    Ok(state.0.daemon_status(false).await)
}

#[tauri::command]
pub async fn ensure_daemon(state: State<'_, AppState>) -> CmdResult<DaemonStatus> {
    Ok(state.0.daemon_status(true).await)
}

/// This account's Cua Cloud rates, when known (the wizard's estimate).
#[tauri::command]
pub async fn cloud_pricing(
    state: State<'_, AppState>,
) -> CmdResult<Option<cua_spaces_app_core::wizard::CloudPricing>> {
    Ok(state.0.cloud_pricing().await)
}

/// The account's Cua Cloud billing (Settings' Billing row).
#[tauri::command]
pub async fn billing_status(
    state: State<'_, AppState>,
) -> CmdResult<Option<cua_spaces_app_core::billing::BillingStatus>> {
    state.0.billing_status().await
}

#[tauri::command]
pub async fn local_status(state: State<'_, AppState>) -> CmdResult<LocalStatus> {
    Ok(state.0.local_status().await)
}

/// This Mac's CPU architecture (`arm64`, `amd64`): the app core warns
/// about a local Space that runs another one (emulated).
#[tauri::command]
pub fn host_arch() -> String {
    crate::core::host_arch()
}

// -------------------------------------------------------------- roster

#[tauri::command]
pub async fn list_spaces(app: AppHandle, state: State<'_, AppState>) -> CmdResult<Vec<SpaceRow>> {
    let rows = state.0.list_spaces().await?;
    crate::tray::set_space_count(&app, rows.len());
    Ok(rows)
}

#[tauri::command]
pub async fn space_info(state: State<'_, AppState>, space_id: String) -> CmdResult<SpaceRow> {
    state.0.space_info(&space_id).await
}

#[tauri::command]
pub async fn add_space(
    app: AppHandle,
    state: State<'_, AppState>,
    url: String,
    token: Option<String>,
    name: Option<String>,
) -> CmdResult<SpaceRow> {
    let row = state.0.add_space(&url, token, name).await?;
    changed(&app);
    Ok(row)
}

/// Creates a Space in `config.on` (the default location when unset).
#[tauri::command]
pub async fn create_space(
    app: AppHandle,
    state: State<'_, AppState>,
    config: Option<SpaceCreateConfig>,
    pending_id: Option<String>,
) -> CmdResult<SpaceRow> {
    let config = config.unwrap_or_default();
    // The SDK's create progress, for the pending row the webview shows
    // (`spaces::creating` in the app core). The pending id is also the
    // create's `create_id`: `cancel_create` finds it by that.
    let progress = pending_id.clone().map(|pending_id| {
        let app = app.clone();
        cua_spaces::ProgressSink::new(move |p| {
            let _ = app.emit(
                "spaces:create-progress",
                CreateProgressPayload::new(&pending_id, p),
            );
        })
    });
    // The webview's creates reducer records the create (the app core's
    // `telemetry::creates`: started, ready or failed, and how long it took).
    let r = state
        .0
        .create_space_with_progress(config, progress, pending_id)
        .await;
    // The roster also drops the pending tile when a create fails.
    changed(&app);
    r
}

/// `spaces:create-progress`: what a create started with a pending id is doing.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateProgressPayload {
    pub pending_id: String,
    pub phase: String,
    pub fraction: Option<f64>,
    pub detail: String,
    /// Bytes of the download so far, of how many, and how fast (smoothed),
    /// when the step counts them.
    pub bytes_done: Option<u64>,
    pub bytes_total: Option<u64>,
    pub bytes_per_second: Option<f64>,
    /// The id the Space will have (`local:<name>`), when known.
    pub space: Option<String>,
}

impl CreateProgressPayload {
    /// The event for one SDK progress report of the create `pending_id`.
    pub fn new(pending_id: &str, p: &cua_spaces::CreateProgress) -> Self {
        Self {
            pending_id: pending_id.to_string(),
            phase: p.phase.as_str().to_string(),
            fraction: p.fraction,
            detail: p.detail.clone(),
            bytes_done: p.bytes.map(|b| b.done),
            bytes_total: p.bytes.map(|b| b.total).filter(|t| *t > 0),
            bytes_per_second: p.bytes.and_then(|b| b.per_second),
            space: Some(p.target.clone()).filter(|t| !t.is_empty()),
        }
    }
}

/// Cancels the create started with `pending_id` (its `create_id`): it
/// stops and what it made is removed; resolves once that is done. The
/// create itself then fails with an error starting `cancelled: `.
#[tauri::command]
pub async fn cancel_create(
    app: AppHandle,
    state: State<'_, AppState>,
    pending_id: String,
) -> CmdResult<cua_spaces::CancelOutcome> {
    let r = state.0.cancel_create(&pending_id).await;
    changed(&app);
    r
}

/// The New Space wizard's GPU choices on this machine (`WizardEnv.gpus`).
#[tauri::command]
pub async fn gpu_support(
    state: State<'_, AppState>,
) -> CmdResult<Vec<cua_spaces_app_core::wizard::GpuChoice>> {
    Ok(state.0.gpu_support().await)
}

/// Your machines that provide Spaces (`WizardEnv.hosts`): the New Space
/// wizard's Run on menu.
#[tauri::command]
pub async fn list_hosts(
    state: State<'_, AppState>,
) -> CmdResult<Vec<cua_spaces_app_core::wizard::SpaceHost>> {
    Ok(state.0.list_hosts().await)
}

/// Deletes a created Space's sandbox; a Space added by address is only
/// forgotten.
#[tauri::command]
pub async fn delete_space(
    app: AppHandle,
    state: State<'_, AppState>,
    space_id: String,
) -> CmdResult<String> {
    // The user confirmed: nothing streams from it while it is deleted.
    crate::viewer_windows::close_space_windows(&app, &space_id);
    let r = state.0.delete_space(&space_id).await;
    crate::telemetry::feature("space_delete");
    changed(&app);
    hotspot_changed(&app, &state.0.hotspot_status());
    r
}

/// Turns a Space off (suspended or stopped, as its provider can) or back
/// on: the power button next to Delete.
#[tauri::command]
pub async fn set_space_power(
    app: AppHandle,
    state: State<'_, AppState>,
    space_id: String,
    on: bool,
) -> CmdResult<cua_spaces::SpacePower> {
    if !on {
        // Nothing streams from a Space that is going off.
        crate::viewer_windows::close_space_windows(&app, &space_id);
    }
    let r = state.0.set_space_power(&space_id, on).await;
    changed(&app);
    hotspot_changed(&app, &state.0.hotspot_status());
    r
}

/// Where new Spaces are created by default, and where that came from.
#[tauri::command]
pub fn get_default_location(state: State<'_, AppState>) -> CmdResult<DefaultLocation> {
    state.0.default_location()
}

/// Stores the default location (`local` or `cloud`) in `$CUA_HOME/config.toml`.
#[tauri::command]
pub fn set_default_location(state: State<'_, AppState>, on: String) -> CmdResult<DefaultLocation> {
    state.0.set_default_location(&on)
}

#[tauri::command]
pub async fn remove_space(
    app: AppHandle,
    state: State<'_, AppState>,
    space_id: String,
) -> CmdResult<()> {
    let r = state.0.remove_space(&space_id).await;
    changed(&app);
    hotspot_changed(&app, &state.0.hotspot_status());
    r
}

#[tauri::command]
pub async fn keep_alive_space(
    state: State<'_, AppState>,
    space_id: String,
    seconds: u64,
) -> CmdResult<()> {
    state.0.keep_alive_space(&space_id, seconds).await
}

// ----------------------------------------------------------- space ops

#[tauri::command]
pub async fn space_screenshot(
    state: State<'_, AppState>,
    space_id: String,
    max_dimension: Option<u32>,
) -> CmdResult<String> {
    state.0.space_screenshot(&space_id, max_dimension).await
}

#[tauri::command]
pub async fn send_files_to_space(
    state: State<'_, AppState>,
    space_id: String,
    paths: Vec<String>,
    target_directory: Option<String>,
) -> CmdResult<Vec<SentFile>> {
    crate::telemetry::feature("file_send");
    state
        .0
        .send_files(&space_id, &paths, target_directory)
        .await
}

#[tauri::command]
pub async fn list_remote_windows(
    state: State<'_, AppState>,
    space_id: String,
) -> CmdResult<Vec<RemoteWindow>> {
    state.0.list_remote_windows(&space_id).await
}

#[tauri::command]
pub async fn remote_window_thumbnail(
    state: State<'_, AppState>,
    space_id: String,
    window_id: String,
    target_epoch: u64,
) -> CmdResult<Option<String>> {
    state
        .0
        .remote_window_thumbnail(&space_id, &window_id, target_epoch)
        .await
}

/// The icon a Space's desktop shows for an app (the SDK's `Space::app_icon`),
/// as a `data:` URL; `None` when the Space has none (show no icon then).
#[tauri::command]
pub async fn space_app_icon(
    state: State<'_, AppState>,
    space_id: String,
    app_name: String,
    app_id: String,
    pid: Option<u32>,
) -> CmdResult<Option<String>> {
    state
        .0
        .space_app_icon(&space_id, &app_name, &app_id, pid.unwrap_or(0))
        .await
}

/// Icons for many windows' apps in one call (the SDK's `Space::app_icons`:
/// its one icon cache, every miss in one guest round trip), as `data:`
/// URLs in request order; `null` where the Space has none.
#[tauri::command]
pub async fn space_app_icons(
    state: State<'_, AppState>,
    space_id: String,
    requests: Vec<crate::core::AppIconRequest>,
) -> CmdResult<Vec<Option<String>>> {
    state.0.space_app_icons(&space_id, requests).await
}

/// Memory and storage use, for the Space detail's Memory and Storage rows.
#[tauri::command]
pub async fn space_usage(
    state: State<'_, AppState>,
    space_id: String,
) -> CmdResult<serde_json::Value> {
    state.0.space_usage(&space_id).await
}

/// The Space's primary display size, for the Stream section's Desktop row.
#[tauri::command]
pub async fn space_primary_display(
    state: State<'_, AppState>,
    space_id: String,
) -> CmdResult<Option<crate::core::DisplaySize>> {
    state.0.space_primary_display(&space_id).await
}

#[tauri::command]
pub async fn list_space_agents(
    state: State<'_, AppState>,
    space_id: String,
) -> CmdResult<Vec<SpaceAgentRun>> {
    state.0.list_space_agents(&space_id).await
}

/// Installs a coding agent in the Space and opens its interactive CLI in a
/// terminal on the Space's desktop; the user signs in there.
#[tauri::command]
pub async fn launch_agent_terminal(
    state: State<'_, AppState>,
    space_id: String,
    harness: String,
) -> CmdResult<()> {
    state.0.launch_agent_terminal(&space_id, &harness).await
}

// ------------------------------------------------------------- streams

#[tauri::command]
pub async fn open_space_stream(
    state: State<'_, AppState>,
    space_id: String,
    target: StreamTargetArg,
    options: Option<StreamOpts>,
) -> CmdResult<StreamTicketInfo> {
    let r = state
        .0
        .open_stream(&space_id, target, options.unwrap_or_default())
        .await;
    crate::telemetry::feature("space_open_viewer");
    if r.is_ok() {
        crate::telemetry::step("first_stream", true);
    }
    r
}

#[tauri::command]
pub async fn close_space_stream(
    state: State<'_, AppState>,
    space_id: String,
    media_session_id: String,
) -> CmdResult<()> {
    state.0.close_stream(&space_id, &media_session_id).await
}

// ------------------------------------------------------------ teleport

#[tauri::command]
pub async fn teleport_manifest(
    state: State<'_, AppState>,
    app_id: String,
    scope: String,
) -> CmdResult<TransferManifest> {
    state.0.teleport_manifest(&app_id, &scope).await
}

/// Runs the consented teleport. The picker closes right after calling
/// this, so the push owns the Space window's transfer overlay: it stores
/// its parameters for a Retry and emits the terminal done / error itself.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn teleport_push(
    app: AppHandle,
    state: State<'_, AppState>,
    transfers: State<'_, TransferState>,
    last: State<'_, LastPushState>,
    app_id: String,
    scope: String,
    space_id: String,
    include: Option<Vec<String>>,
    acknowledge_sensitive: Option<bool>,
) -> CmdResult<TeleportResult> {
    let include = include.unwrap_or_default();
    let ack = acknowledge_sensitive.unwrap_or(false);
    let app_name = transfers
        .0
        .lock()
        .ok()
        .and_then(|map| {
            map.get(&crate::viewer_windows::space_window_label(&space_id))
                .map(|o| o.app_name.clone())
        })
        .unwrap_or_default();
    if let Ok(mut map) = last.0.lock() {
        map.insert(
            space_id.clone(),
            LastPush {
                app_id: app_id.clone(),
                scope: scope.clone(),
                include: include.clone(),
                app_name,
                acknowledge_sensitive: ack,
            },
        );
    }
    run_push(
        app,
        state.0.clone(),
        transfers.0.clone(),
        LastPush {
            app_id,
            scope,
            include,
            app_name: String::new(),
            acknowledge_sensitive: ack,
        },
        space_id,
    )
    .await
}

/// The push shared by `teleport_push` and the overlay's Retry.
pub(crate) async fn run_push(
    app: AppHandle,
    core: Arc<AppCore>,
    transfers: Arc<TransferMap>,
    push: LastPush,
    space_id: String,
) -> CmdResult<TeleportResult> {
    let started = std::time::Instant::now();
    crate::telemetry::feature("teleport_drop");
    let outcome = core
        .teleport_push(
            &push.app_id,
            &push.scope,
            &space_id,
            &push.include,
            push.acknowledge_sensitive,
        )
        .await;
    // A Touch ID prompt for sensitive items takes focus; give it back to the
    // Space view (its window state, fullscreen or not, is the user's choice).
    {
        use tauri::Manager;
        let label = crate::viewer_windows::space_window_label(&space_id);
        if let Some(window) = app.get_webview_window(&label) {
            let _ = window.set_focus();
        }
    }
    match &outcome {
        Ok(_) => {
            crate::viewer_windows::emit_transfer_terminal(&app, &transfers, &space_id, "done", None)
        }
        Err(m) => crate::viewer_windows::emit_transfer_terminal(
            &app,
            &transfers,
            &space_id,
            "error",
            Some(m.clone()),
        ),
    }
    crate::telemetry::teleport(&push.app_id, "full", "app_with_state", started, &outcome, 0);
    outcome
}

// ------------------------------------------------- "Teleport an app…"

/// "Teleport an app…": every app on this machine, classified by the SDK
/// (`cua_teleport::ux`), narrowed to `space_id`'s OS and CPU when given.
/// Core JSON (snake_case); the webview converts it with `entryFromCore`.
#[tauri::command]
pub async fn teleport_catalog(
    state: State<'_, AppState>,
    space_id: Option<String>,
) -> CmdResult<Vec<cua_teleport::ux::CatalogEntry>> {
    state.0.teleport_app_catalog(space_id.as_deref()).await
}

/// An app's icon (`path`: a bundle or `.desktop` entry) as a PNG data URL,
/// through the SDK's one icon cache.
#[tauri::command]
pub async fn teleport_app_icon(path: String, size: Option<u32>) -> Option<String> {
    let size = size.unwrap_or(64);
    tauri::async_runtime::spawn_blocking(move || {
        cua_teleport::ux::app_icon_png_cached(&path, size)
            .and_then(|png| cua_teleport::ux::window::png_data_url(&png))
    })
    .await
    .ok()
    .flatten()
}

/// The catalog row for a dropped app (a `.app` bundle from Finder or the
/// Dock).
#[tauri::command]
pub fn teleport_entry_for_path(
    state: State<'_, AppState>,
    path: String,
) -> CmdResult<cua_teleport::ux::CatalogEntry> {
    state.0.teleport_entry_for_path(&path)
}

/// Sorts a drop into apps, files and URLs (paths or `file://` URIs).
#[tauri::command]
pub fn teleport_parse_drop(items: Vec<String>) -> DropView {
    let p = cua_teleport::ux::drop::parse(&items);
    DropView {
        kind: p.kind(),
        apps: p.apps,
        files: p.files,
        urls: p.urls,
    }
}

/// `teleport_parse_drop` output.
#[derive(serde::Serialize)]
pub struct DropView {
    pub kind: cua_teleport::ux::DropKind,
    pub apps: Vec<String>,
    pub files: Vec<String>,
    pub urls: Vec<String>,
}

/// What teleporting `entry` into the Space will install, send and import.
#[tauri::command]
pub async fn teleport_plan(
    state: State<'_, AppState>,
    space_id: String,
    entry: cua_teleport::ux::CatalogEntry,
    options: cua_teleport::ux::PlanOptions,
) -> CmdResult<cua_teleport::ux::TeleportPlan> {
    state.0.teleport_app_plan(&space_id, &entry, &options).await
}

/// Runs a plan the user approved. Progress goes to `on_event` (the picker)
/// and to the Space window's transfer overlay.
#[tauri::command]
pub async fn teleport_run(
    app: AppHandle,
    state: State<'_, AppState>,
    transfers: State<'_, TransferState>,
    space_id: String,
    plan: cua_teleport::ux::TeleportPlan,
    consent: cua_teleport::ux::Consent,
    on_event: tauri::ipc::Channel<cua_teleport::ux::RunEvent>,
) -> CmdResult<cua_teleport::ux::RunReport> {
    let transfers = transfers.0.clone();
    crate::viewer_windows::emit_transfer_start(&app, &transfers, &space_id, &plan.app.name);
    // Telemetry: the public catalog id (else `other`), capability and move.
    let started = std::time::Instant::now();
    let (tele_app, tele_cap, tele_move) = (
        plan.app.id.clone(),
        plan.app.capability.as_str(),
        plan.moves.as_str(),
    );
    crate::telemetry::feature("teleport_app_picker");
    let (app2, t2, sid) = (app.clone(), transfers.clone(), space_id.clone());
    let outcome = state
        .0
        .teleport_app_run(&space_id, plan, consent, move |e| {
            if e.total_bytes > 0 {
                crate::viewer_windows::emit_transfer_progress(
                    &app2,
                    &t2,
                    &sid,
                    e.done_bytes,
                    e.total_bytes,
                );
            }
            let _ = on_event.send(e);
        })
        .await;
    match &outcome {
        Ok(_) => {
            crate::viewer_windows::emit_transfer_terminal(&app, &transfers, &space_id, "done", None)
        }
        Err(m) => crate::viewer_windows::emit_transfer_terminal(
            &app,
            &transfers,
            &space_id,
            "error",
            Some(m.clone()),
        ),
    }
    crate::telemetry::teleport(
        &tele_app,
        tele_cap,
        tele_move,
        started,
        &outcome,
        outcome.as_ref().map(|r| r.sent.len() as u64).unwrap_or(0),
    );
    outcome
}

/// A native chooser for files and folders to send with the app.
#[tauri::command]
pub async fn teleport_choose_files(app: AppHandle) -> Vec<String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("Choose files or folders to teleport")
        .pick_files(move |picked| {
            let _ = tx.send(picked.unwrap_or_default());
        });
    rx.await
        .unwrap_or_default()
        .into_iter()
        .filter_map(|p| p.into_path().ok())
        .map(|p| p.to_string_lossy().into_owned())
        .collect()
}

// ------------------------------------------------------------- hotspot

#[tauri::command]
pub async fn start_hotspot(
    app: AppHandle,
    state: State<'_, AppState>,
    space_id: String,
) -> CmdResult<HotspotStatus> {
    let r = state.0.start_hotspot(&space_id).await;
    crate::telemetry::feature("hotspot");
    hotspot_changed(&app, &state.0.hotspot_status());
    r
}

#[tauri::command]
pub async fn stop_hotspot(app: AppHandle, state: State<'_, AppState>) -> CmdResult<HotspotStatus> {
    let r = state.0.stop_hotspot().await;
    hotspot_changed(&app, &state.0.hotspot_status());
    r
}

#[tauri::command]
pub fn hotspot_status(state: State<'_, AppState>) -> HotspotStatus {
    state.0.hotspot_status()
}

// ---------------------------------------------------------------- host

/// Tauri-managed host/onboarding commands (`crate::host`).
pub struct HostState(pub Arc<crate::host::HostCommands>);

fn host_changed(app: &AppHandle, status: &crate::host::HostStatusView) {
    let _ = app.emit("host:changed", status);
}

#[tauri::command]
pub async fn host_status(state: State<'_, HostState>) -> CmdResult<crate::host::HostStatusView> {
    Ok(state.0.status().await)
}

#[tauri::command]
pub async fn host_setup(
    app: AppHandle,
    state: State<'_, HostState>,
    request: crate::host::HostSetupRequest,
) -> CmdResult<crate::host::HostStatusView> {
    let status = state.0.setup(request).await?;
    host_changed(&app, &status);
    Ok(status)
}

#[tauri::command]
pub async fn host_stop_sharing(
    app: AppHandle,
    state: State<'_, HostState>,
) -> CmdResult<crate::host::HostStatusView> {
    let status = state.0.stop_sharing().await?;
    host_changed(&app, &status);
    Ok(status)
}

#[tauri::command]
pub async fn host_start_sharing(
    app: AppHandle,
    state: State<'_, HostState>,
) -> CmdResult<crate::host::HostStatusView> {
    let status = state.0.start_sharing().await?;
    host_changed(&app, &status);
    Ok(status)
}

#[tauri::command]
pub async fn host_configure(
    app: AppHandle,
    state: State<'_, HostState>,
    change: crate::host::HostSettingChange,
) -> CmdResult<crate::host::HostStatusView> {
    let status = state.0.configure(change).await?;
    host_changed(&app, &status);
    Ok(status)
}

#[tauri::command]
pub async fn host_remove(app: AppHandle, state: State<'_, HostState>) -> CmdResult<()> {
    state.0.remove().await?;
    host_changed(&app, &state.0.status().await);
    Ok(())
}

#[tauri::command]
pub fn host_open_settings(url: String) -> CmdResult<()> {
    crate::host::open_settings_url(&url)
}

#[tauri::command]
pub fn onboarding_state(state: State<'_, HostState>) -> crate::host::OnboardingState {
    state.0.onboarding_state()
}

#[tauri::command]
pub fn complete_onboarding(state: State<'_, HostState>, mode: String) -> CmdResult<()> {
    state.0.complete_onboarding(&mode)
}

// ---------------------------------------------------------------- launch at login

/// This platform's login item (`crate::login_item`).
pub struct LoginItemState(pub Box<dyn crate::login_item::LoginItemService>);

/// What the system holds for the app as a login item.
#[tauri::command]
pub fn login_item_status(
    state: State<'_, LoginItemState>,
) -> cua_spaces_app_core::login_item::LoginItemStatus {
    state.0.status()
}

/// Registers or unregisters the app as a login item; what the system holds
/// afterwards.
#[tauri::command]
pub fn login_item_set(
    state: State<'_, LoginItemState>,
    on: bool,
) -> CmdResult<cua_spaces_app_core::login_item::LoginItemStatus> {
    let status = crate::login_item::set_and_read(state.0.as_ref(), on)?;
    crate::telemetry::feature(if on {
        "launch_at_login_on"
    } else {
        "launch_at_login_off"
    });
    Ok(status)
}

// ---------------------------------------------------------------- installer

/// Tauri-managed first-run installer steps (`crate::installer`).
pub struct InstallerState(pub Arc<crate::installer::InstallerCommands>);

/// What installing the bundled `cua` CLI would do (exact target path).
#[tauri::command]
pub async fn installer_cli_plan(
    state: State<'_, InstallerState>,
) -> CmdResult<crate::installer::CliInstallPlan> {
    Ok(state.0.cli_plan().await)
}

/// Install the bundled `cua` CLI; the webview calls this only after consent.
#[tauri::command]
pub async fn installer_install_cli(
    state: State<'_, InstallerState>,
    request: crate::installer::CliInstallRequest,
) -> CmdResult<crate::installer::CliInstallPlan> {
    state.0.install_cli(request).await
}

/// Detected AI coding agents + the default cua skills (cua-agent-setup).
#[tauri::command]
pub async fn installer_detect_agents(
    state: State<'_, InstallerState>,
) -> CmdResult<crate::installer::AgentDetectReport> {
    state.0.detect_agents().await
}

/// Install skills and/or the cua MCP server into the ticked agents.
#[tauri::command]
pub async fn installer_setup_agents(
    state: State<'_, InstallerState>,
    request: crate::installer::AgentSetupRequest,
) -> CmdResult<crate::installer::AgentSetupReport> {
    state.0.setup_agents(request).await
}

// ------------------------------------------------------------- devices

/// The Devices page's data (the core's `DevicesInput`).
#[tauri::command]
pub async fn devices_snapshot(
    state: State<'_, AppState>,
) -> CmdResult<cua_spaces_app_core::devices::DevicesInput> {
    state.0.devices()?.snapshot().await
}

/// Registers this device (enrolled at once, or a one-time code).
#[tauri::command]
pub async fn devices_enroll(state: State<'_, AppState>) -> CmdResult<crate::devices::EnrollResult> {
    state.0.devices()?.enroll().await
}

/// Whether an enrolled device approved this one yet.
#[tauri::command]
pub async fn devices_check_enrolled(state: State<'_, AppState>) -> CmdResult<bool> {
    Ok(state.0.devices()?.check_enrolled().await)
}

/// Whether approving asks for a passphrase (no OS prompt on this system).
#[tauri::command]
pub fn devices_presence_needs_passphrase() -> bool {
    crate::devices::presence_needs_passphrase()
}

/// Approves a device after presence (Touch ID / login password on macOS,
/// else the Keyvault passphrase).
#[tauri::command]
pub async fn devices_approve(
    state: State<'_, AppState>,
    keyvault: State<'_, crate::keyvault::KeyvaultState>,
    code: Option<String>,
    device_id: Option<String>,
    passphrase: Option<String>,
) -> CmdResult<()> {
    let presence = crate::devices::OsPresence {
        keyvault: &keyvault.0,
    };
    state
        .0
        .devices()?
        .approve(&presence, code, device_id, passphrase)
        .await
}

/// Renames a device.
#[tauri::command]
pub async fn devices_rename(state: State<'_, AppState>, id: String, name: String) -> CmdResult<()> {
    state.0.devices()?.rename(&id, &name).await
}

/// Revokes a device (Deny, or Revoke… after its confirmation).
#[tauri::command]
pub async fn devices_revoke(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    state.0.devices()?.revoke(&id).await
}

/// Vouches for a machine that registered without an enrolled device's
/// proof (S5), after the row's confirmation.
#[tauri::command]
pub async fn devices_confirm_machine(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    state.0.devices()?.confirm_machine(&id).await
}

/// Posts a system notification (a device asks for approval).
#[tauri::command]
pub fn devices_notify(app: AppHandle, title: String, body: String) -> CmdResult<()> {
    use tauri_plugin_notification::NotificationExt as _;
    app.notification()
        .builder()
        .title(title)
        .body(body)
        .show()
        .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------- sharing

/// Who a Space is shared with (the Share sheet's input).
#[tauri::command]
pub async fn space_shares(
    state: State<'_, AppState>,
    space_id: String,
) -> CmdResult<cua_spaces::share::SpaceShares> {
    state.0.space_shares(&space_id).await
}

/// Shares a Space (presence first on macOS).
#[tauri::command]
pub async fn share_space(
    state: State<'_, AppState>,
    space_id: String,
    who: String,
    role: String,
) -> CmdResult<cua_spaces::share::SpaceShares> {
    state.0.share_space(&space_id, &who, &role).await
}

/// Stops sharing a Space with one account, at once.
#[tauri::command]
pub async fn unshare_space(
    state: State<'_, AppState>,
    space_id: String,
    who: String,
) -> CmdResult<cua_spaces::share::SpaceShares> {
    state.0.unshare_space(&space_id, &who).await
}

// ------------------------------------------------------------- your cloud

/// Runs one cloud tool (status, a test that creates nothing, connect) in
/// the daemon (see `cloud::TOOLS`).
#[tauri::command]
pub async fn cloud_tool(
    state: State<'_, AppState>,
    tool: String,
    args: serde_json::Value,
) -> CmdResult<serde_json::Value> {
    state.0.cloud_tool(&tool, args).await
}

// ------------------------------------------------ agents, drive, notifications

/// Runs one Spaces tool of the Agents, Drive or Notifications page in the
/// daemon (see `persistent::TOOLS`).
#[tauri::command]
pub async fn agents_tool(
    state: State<'_, AppState>,
    tool: String,
    args: serde_json::Value,
) -> CmdResult<serde_json::Value> {
    state.0.agents_tool(&tool, args).await
}

/// Shows a path inside the mounted Cua Volume in the file manager (Finder
/// on macOS, the folder on Linux). Refuses anything outside the mount point
/// the daemon reports now.
#[tauri::command]
pub async fn drive_reveal(state: State<'_, AppState>, path: String) -> CmdResult<()> {
    state.0.drive_reveal(&path).await
}
