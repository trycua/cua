// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Cua Spaces portal shell.
//!
//! The Rust side owns the window: one transparent, borderless, always-on-top
//! webview positioned at the top centre of the current monitor. The renderer
//! asks for a [`WindowMode`] and receives the resulting geometry back.
//!
//! Beyond the portal window, the shell hosts the cua SDK: [`core`] links
//! `cua-spaces` in-process (the shared `~/.cua` Spaces registry, streams,
//! files, teleport, hotspot, agents) and connects to or starts `cua daemon`
//! (the webview media bridge for Fleet Spaces, and the process CLI / MCP
//! clients share Spaces through). [`commands`] are thin Tauri wrappers over
//! it; [`viewer_windows`], [`window_drag`] and the notch stay app UI, and
//! [`control`] lets `cua daemon mcp` ask the app to present a Space.

pub mod agent_config;
pub mod agent_setup;
pub mod agents;
pub mod app_core;
pub mod auth;
pub mod biometric;
pub mod cloud;
pub mod commands;
pub mod control;
pub mod core;
pub mod devices;
pub mod geometry;
pub mod host;
pub mod host_backend;
pub mod installer;
pub mod keyvault;
pub mod login_item;
pub mod persistent;
pub mod presence;
pub mod sf_symbol;
pub mod share;
pub mod space_switch;
pub mod telemetry;
pub mod tray;
pub mod viewer_windows;
pub mod webview_data;
pub mod window_drag;

use std::sync::Mutex;

use serde::Serialize;
use tauri::{LogicalPosition, LogicalSize, Manager, State, WebviewWindow};

use geometry::{
    compute_geometry, logical_monitor, parse_display_override, resolve_display_style, DisplayStyle,
    DisplayStyleSource, LogicalRect, PortalGeometry, WindowMode,
};

#[cfg(target_os = "macos")]
use objc2_app_kit::{NSStatusWindowLevel, NSWindow, NSWindowCollectionBehavior};

const PORTAL_LABEL: &str = "portal";
const DISPLAY_ENV: &str = "CUA_SPACES_DISPLAY";

/// The recording and test hooks (`CUA_SPACES_START_VIEW`, `CUA_SPACES_WIZARD_*`,
/// `CUA_SPACES_CAPTURE_*`). Captures and demos use debug builds; a release build
/// reads none of them, so its environment can't steer what it opens or selects.
const DEV_HOOKS: &[&str] = &[
    "CUA_SPACES_START_VIEW",
    "CUA_SPACES_WIZARD_IMAGE",
    "CUA_SPACES_WIZARD_ON",
    "CUA_SPACES_WIZARD_DISK",
    "CUA_SPACES_CAPTURE_SPACE",
    "CUA_SPACES_CAPTURE_APP",
    "CUA_SPACES_CAPTURE_FILES",
];

fn dev_hook(name: &str) -> Option<String> {
    dev_hook_in(cfg!(debug_assertions), name, |k| std::env::var(k).ok())
}

fn dev_hook_in(
    enabled: bool,
    name: &str,
    lookup: impl Fn(&str) -> Option<String>,
) -> Option<String> {
    debug_assert!(
        DEV_HOOKS.contains(&name),
        "{name} is not a declared dev hook"
    );
    if enabled {
        lookup(name)
    } else {
        None
    }
}

#[derive(Debug, Clone, Copy)]
struct Inner {
    mode: WindowMode,
    style: DisplayStyle,
    source: DisplayStyleSource,
    accessory_activation: bool,
}

/// Shared shell state behind a mutex; commands are short and never hold it
/// across window calls that could re-enter.
pub struct PortalState(Mutex<Inner>);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortalEnvironment {
    platform: &'static str,
    display_style: DisplayStyle,
    display_style_source: DisplayStyleSource,
    accessory_activation: bool,
    native: bool,
    geometry: PortalGeometry,
}

fn platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "other"
    }
}

/// Monitor currently hosting the window (falls back to the primary), in
/// logical points, plus its scale factor.
fn current_monitor(window: &WebviewWindow) -> Result<(LogicalRect, f64), String> {
    let monitor = window
        .current_monitor()
        .map_err(|e| e.to_string())?
        .or(window.primary_monitor().map_err(|e| e.to_string())?)
        .ok_or_else(|| "no monitor available".to_string())?;
    let pos = monitor.position();
    let size = monitor.size();
    let scale = monitor.scale_factor();
    Ok((
        logical_monitor(pos.x, pos.y, size.width, size.height, scale),
        scale,
    ))
}

/// Resize then reposition the window for `mode`, returning the applied geometry.
fn apply_mode(
    window: &WebviewWindow,
    mode: WindowMode,
    style: DisplayStyle,
) -> Result<PortalGeometry, String> {
    let (monitor, scale) = current_monitor(window)?;
    let geometry = compute_geometry(mode, style, monitor, scale);
    let frame = geometry.frame;
    window
        .set_size(LogicalSize::new(frame.width, frame.height))
        .map_err(|e| e.to_string())?;
    window
        .set_position(LogicalPosition::new(frame.x, frame.y))
        .map_err(|e| e.to_string())?;
    Ok(geometry)
}

/// Configure the portal as a macOS status window so AppKit permits its frame
/// in the menu-bar/notch band. Tauri's generic `alwaysOnTop` maps to a floating
/// window level, which macOS constrains to the ordinary visible work area.
/// Poll the OS cursor ~25×/s and emit `notch:hover` (bool) to the portal window
/// as the cursor enters/leaves the window's frame. Focus-independent, so the
/// notch's springy hover works even when another app is frontmost. The renderer
/// only reacts in ambient mode (the CSS is scoped to `data-mode="ambient"`).
fn spawn_notch_hover_poller(window: WebviewWindow) {
    use tauri::Emitter;
    std::thread::spawn(move || {
        let mut last = false;
        loop {
            std::thread::sleep(std::time::Duration::from_millis(40));
            let (cursor, pos, size) = match (
                window.cursor_position(),
                window.outer_position(),
                window.outer_size(),
            ) {
                (Ok(cursor), Ok(pos), Ok(size)) => (cursor, pos, size),
                _ => continue,
            };
            let within = cursor.x >= pos.x as f64
                && cursor.x < pos.x as f64 + size.width as f64
                && cursor.y >= pos.y as f64
                && cursor.y < pos.y as f64 + size.height as f64;
            if within != last {
                last = within;
                let _ = window.emit("notch:hover", within);
            }
        }
    });
}

#[cfg(target_os = "macos")]
fn configure_notch_window(window: &WebviewWindow) -> Result<(), String> {
    let pointer = window.ns_window().map_err(|error| error.to_string())?;
    if pointer.is_null() {
        return Err("portal NSWindow handle is null".to_string());
    }

    // SAFETY: Tauri owns this NSWindow for at least as long as `window`. The
    // setup callback runs on the AppKit main thread, and we do not retain or
    // release the borrowed pointer.
    let native = unsafe { &*pointer.cast::<NSWindow>() };
    native.setLevel(NSStatusWindowLevel);
    native.setCollectionBehavior(notch_collection_behavior());
    native.setMovable(false);
    native.setMovableByWindowBackground(false);
    native.setHidesOnDeactivate(false);
    native.setCanHide(false);
    Ok(())
}

#[cfg(target_os = "macos")]
pub(crate) fn notch_collection_behavior() -> NSWindowCollectionBehavior {
    NSWindowCollectionBehavior::CanJoinAllSpaces
        | NSWindowCollectionBehavior::Stationary
        | NSWindowCollectionBehavior::IgnoresCycle
        | NSWindowCollectionBehavior::FullScreenAuxiliary
}

#[cfg(all(test, target_os = "macos"))]
mod macos_tests {
    use super::*;

    #[test]
    fn notch_window_policy_stays_visible_without_joining_window_cycle() {
        let behavior = notch_collection_behavior();
        assert!(behavior.contains(NSWindowCollectionBehavior::CanJoinAllSpaces));
        assert!(behavior.contains(NSWindowCollectionBehavior::Stationary));
        assert!(behavior.contains(NSWindowCollectionBehavior::FullScreenAuxiliary));
        assert!(behavior.contains(NSWindowCollectionBehavior::IgnoresCycle));
        assert!(!behavior.contains(NSWindowCollectionBehavior::ParticipatesInCycle));
    }
}

fn snapshot(window: &WebviewWindow, inner: Inner) -> Result<PortalEnvironment, String> {
    let (monitor, scale) = current_monitor(window)?;
    Ok(PortalEnvironment {
        platform: platform(),
        display_style: inner.style,
        display_style_source: inner.source,
        accessory_activation: inner.accessory_activation,
        native: true,
        geometry: compute_geometry(inner.mode, inner.style, monitor, scale),
    })
}

#[tauri::command]
fn get_environment(
    window: WebviewWindow,
    state: State<'_, PortalState>,
) -> Result<PortalEnvironment, String> {
    let inner = *state.0.lock().map_err(|e| e.to_string())?;
    snapshot(&window, inner)
}

#[tauri::command]
fn set_window_mode(
    window: WebviewWindow,
    state: State<'_, PortalState>,
    mode: WindowMode,
) -> Result<PortalGeometry, String> {
    let style = state.0.lock().map_err(|e| e.to_string())?.style;
    let geometry = apply_mode(&window, mode, style)?;
    state.0.lock().map_err(|e| e.to_string())?.mode = mode;
    Ok(geometry)
}

#[tauri::command]
fn set_display_style(
    window: WebviewWindow,
    state: State<'_, PortalState>,
    style: DisplayStyle,
) -> Result<PortalEnvironment, String> {
    let inner = {
        let mut inner = state.0.lock().map_err(|e| e.to_string())?;
        inner.style = style;
        inner.source = DisplayStyleSource::Override;
        *inner
    };
    apply_mode(&window, inner.mode, inner.style)?;
    snapshot(&window, inner)
}

/// Open an `http(s)` URL in the user's default browser (Settings "Sign in to
/// Cua"). Restricted to web schemes so a webview call can never launch an
/// arbitrary local handler, and it hands the URL to the OS opener as a single
/// argument (never a shell string) so nothing is interpreted by a shell.
#[tauri::command]
fn open_external(url: String) -> Result<(), String> {
    open_web_url(&url)
}

/// Open an `http(s)` URL in the user's default browser. Shared by the
/// `open_external` command and the device-flow sign-in (which opens the
/// verification URL). Restricted to web schemes so a webview call can never
/// launch an arbitrary local handler, and it hands the URL to the OS opener as
/// a single argument (never a shell string) so nothing is interpreted by a
/// shell.
pub(crate) fn open_web_url(url: &str) -> Result<(), String> {
    let url = checked_web_url(url)?;
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut c = std::process::Command::new("open");
        c.arg(url);
        c
    };
    // Not `cmd /C start`: cmd parses its command line, so a URL would be
    // interpreted by it. The URL protocol handler takes it verbatim.
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut c = std::process::Command::new("rundll32.exe");
        c.args(["url.dll,FileProtocolHandler", url]);
        c
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut command = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(url);
        c
    };
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("failed to open {url}: {error}"))
}

/// The URL `open_web_url` hands to the OS opener: an `http(s)` URL with a
/// host, made only of printable ASCII other than quotes, backslashes, angle
/// brackets, carets, pipes and backticks (a real URL percent-encodes them),
/// so no opener can read it as anything but one URL.
fn checked_web_url(url: &str) -> Result<&str, String> {
    let trimmed = url.trim();
    let rest = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))
        .ok_or_else(|| "only http(s) URLs may be opened".to_string())?;
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if host.is_empty() {
        return Err("the URL has no host".to_string());
    }
    if let Some(bad) = trimmed.chars().find(|c| {
        !c.is_ascii_graphic() || matches!(c, '"' | '\\' | '<' | '>' | '^' | '|' | '`' | '{' | '}')
    }) {
        return Err(format!(
            "the URL contains a character that must be percent-encoded ({:?})",
            bad
        ));
    }
    Ok(trimmed)
}

/// The host backend: the `cua-host` library driven by the app's sign-in.
fn host_backend(core: &std::sync::Arc<core::AppCore>) -> std::sync::Arc<dyn host::HostBackend> {
    std::sync::Arc::new(host_backend::CuaHostBackend::new(
        cua_host::Host::new(core::cua_home()),
        std::sync::Arc::new(host_backend::SessionTokens(core.session())),
    ))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let config = core::CoreConfig::from_env();
    telemetry::init(&config.home);
    let core = core::AppCore::new(config);

    tauri::Builder::default()
        // In-app auto-update (checked from the portal on launch) + the process
        // plugin it uses to relaunch after installing.
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .manage(PortalState(Mutex::new(Inner {
            mode: WindowMode::Ambient,
            style: DisplayStyle::NoNotch,
            source: DisplayStyleSource::Default,
            accessory_activation: false,
        })))
        .manage(commands::AppState(core.clone()))
        .manage(keyvault::KeyvaultState(keyvault::from_env()))
        .manage(viewer_windows::ViewerConfigs::default())
        .manage(viewer_windows::TeleportPickerState::default())
        .manage(viewer_windows::SpacesListState::default())
        .manage(viewer_windows::SpaceScreenshots::default())
        .manage(viewer_windows::TransferState::default())
        .manage(viewer_windows::LastPushState::default())
        .manage(window_drag::WindowDragState::default())
        .manage(std::sync::Arc::new(presence::PresenceSessions::default()))
        // A closed viewer window leaves the presence sessions it joined.
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Destroyed = event {
                if let Some(sessions) =
                    window.try_state::<std::sync::Arc<presence::PresenceSessions>>()
                {
                    sessions.window_closed(window.label());
                }
                viewer_windows::stream_panel_closed(window.app_handle(), window.label());
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_environment,
            set_window_mode,
            set_display_style,
            open_external,
            sf_symbol::sf_symbol,
            commands::fleet_status,
            commands::begin_sign_in,
            commands::sign_out,
            commands::daemon_status,
            commands::ensure_daemon,
            commands::local_status,
            commands::cloud_pricing,
            commands::billing_status,
            commands::host_arch,
            commands::list_spaces,
            commands::space_info,
            commands::add_space,
            commands::create_space,
            commands::cancel_create,
            commands::gpu_support,
            commands::list_hosts,
            commands::delete_space,
            commands::set_space_power,
            commands::get_default_location,
            commands::set_default_location,
            commands::remove_space,
            commands::keep_alive_space,
            commands::space_screenshot,
            commands::send_files_to_space,
            commands::list_remote_windows,
            commands::remote_window_thumbnail,
            commands::space_app_icon,
            commands::space_app_icons,
            commands::space_primary_display,
            commands::space_usage,
            commands::list_space_agents,
            commands::launch_agent_terminal,
            commands::open_space_stream,
            commands::close_space_stream,
            commands::teleport_manifest,
            commands::teleport_push,
            commands::teleport_catalog,
            commands::teleport_app_icon,
            commands::teleport_entry_for_path,
            commands::teleport_parse_drop,
            commands::teleport_plan,
            commands::teleport_run,
            commands::teleport_choose_files,
            commands::start_hotspot,
            commands::stop_hotspot,
            commands::hotspot_status,
            viewer_windows::viewer_config,
            viewer_windows::open_space_window,
            viewer_windows::pin_space_pip,
            viewer_windows::unpin_space_pip,
            viewer_windows::stream_panels,
            viewer_windows::close_stream_window,
            viewer_windows::set_pip_aspect,
            viewer_windows::set_window_stream,
            viewer_windows::stream_remote_windows,
            viewer_windows::resize_stream_window,
            viewer_windows::open_teleport_picker,
            viewer_windows::close_teleport_picker,
            viewer_windows::teleport_picker_config,
            viewer_windows::open_spaces_list,
            viewer_windows::open_new_space,
            viewer_windows::open_main_settings,
            viewer_windows::close_spaces_list,
            viewer_windows::spaces_list_config,
            viewer_windows::cache_space_screenshot,
            viewer_windows::begin_space_transfer,
            viewer_windows::update_space_transfer,
            viewer_windows::retry_space_transfer,
            viewer_windows::cancel_space_transfer,
            window_drag::ax_trusted,
            window_drag::request_ax_trust,
            window_drag::list_open_windows,
            window_drag::capture_window_thumbnail,
            window_drag::app_icon,
            window_drag::set_foreign_window_hidden,
            window_drag::start_window_drag,
            window_drag::drag_trigger_displays,
            agent_setup::agent_setup_detect,
            agent_setup::agent_setup_configure,
            agent_setup::agent_setup_remove,
            agent_config::list_teleportable_apps,
            agent_config::set_teleport_policy,
            commands::host_status,
            commands::host_setup,
            commands::host_stop_sharing,
            commands::host_start_sharing,
            commands::host_remove,
            commands::host_configure,
            commands::host_open_settings,
            commands::onboarding_state,
            commands::complete_onboarding,
            commands::installer_cli_plan,
            commands::installer_install_cli,
            commands::installer_detect_agents,
            commands::installer_setup_agents,
            commands::devices_snapshot,
            commands::devices_enroll,
            commands::devices_check_enrolled,
            commands::devices_presence_needs_passphrase,
            commands::devices_approve,
            commands::devices_rename,
            commands::devices_revoke,
            commands::devices_confirm_machine,
            commands::devices_notify,
            commands::space_shares,
            commands::share_space,
            commands::cloud_tool,
            commands::unshare_space,
            commands::agents_tool,
            commands::drive_reveal,
            tray::tray_set_menu,
            app_core::app_core_call,
            keyvault::keyvault_overview,
            keyvault::keyvault_setup,
            keyvault::keyvault_unlock,
            keyvault::keyvault_setup_passphrase,
            keyvault::keyvault_unlock_passphrase,
            keyvault::keyvault_set_disabled,
            keyvault::keyvault_set_unattended,
            keyvault::keyvault_revoke_grant,
            keyvault::keyvault_remove_rule,
            keyvault::keyvault_release,
            keyvault::keyvault_approve,
            keyvault::keyvault_deny,
            telemetry::telemetry_status,
            telemetry::telemetry_set_enabled,
            telemetry::telemetry_acknowledge_notice,
            telemetry::telemetry_record_feature,
            telemetry::telemetry_record_step,
            telemetry::telemetry_record_signals,
            telemetry::telemetry_welcome_left,
            telemetry::telemetry_record_stream,
            webview_data::ui_storage_set,
            commands::login_item_status,
            commands::login_item_set,
            presence::presence_join,
            presence::presence_publish,
            presence::presence_leave
        ])
        .setup(move |app| {
            let accessory_activation = false;

            // Webview data stays in the app's data directory (webview_data):
            // the page's settings file, then the tauri.conf.json windows
            // (`create: false`) built with that policy, before anything
            // looks them up.
            {
                use tauri::Manager as _;
                let data_dir = webview_data::app_data_dir(app)?;
                app.manage(webview_data::UiStorageState(webview_data::UiStorage::in_dir(
                    &data_dir,
                )));
                let storage = app.state::<webview_data::UiStorageState>();
                // Started at login (after the first run): the menu bar item
                // and the notch only, no main window, as when it is closed.
                let quiet_start = login_item::launched_at_login(std::env::args())
                    && host::OnboardingStore::in_dir(
                        &app.path()
                            .app_config_dir()
                            .unwrap_or_else(|_| core::cua_home().join("spaces-app")),
                    )
                    .completed();
                for mut config in app.config().app.windows.clone() {
                    if quiet_start && config.label == viewer_windows::MAIN_LABEL {
                        config.visible = false;
                    }
                    webview_data::isolate(
                        tauri::WebviewWindowBuilder::from_config(app.handle(), &config)?,
                        &data_dir,
                        &storage.0,
                    )
                    .build()?;
                }
            }

            // The teleport picker's app list and icons, cached before it
            // first opens.
            core.teleport_prefetch();

            // "This machine" host setup + first-run onboarding. The installer
            // (or MDM) may preselect the choice with `--mode host|client` or
            // `<cua home>/spaces-install-mode`.
            {
                use tauri::Manager as _;
                let config_dir = app
                    .path()
                    .app_config_dir()
                    .unwrap_or_else(|_| core::cua_home().join("spaces-app"));
                let installer_mode = host::installer_mode_from_args(std::env::args())
                    .or_else(|| host::installer_mode_from_file(&core::cua_home()));
                app.manage(commands::HostState(std::sync::Arc::new(
                    host::HostCommands::new(
                        host_backend(&core),
                        host::OnboardingStore::in_dir(&config_dir),
                        installer_mode,
                    ),
                )));
            }

            // Launch at login: this platform's login item (autostart entry,
            // Run key or LaunchAgent).
            app.manage(commands::LoginItemState(login_item::platform()));

            // First-run installer steps: the bundled `cua` sidecar onto PATH
            // and agent onboarding through it.
            {
                let exe = std::env::current_exe().unwrap_or_default();
                app.manage(commands::InstallerState(std::sync::Arc::new(
                    installer::InstallerCommands::from_env(&exe),
                )));
            }

            // Loopback control server: `cua daemon mcp` asks the app to pin,
            // open or window-stream a Space through it.
            control::spawn(app.handle().clone(), core.clone());

            // Connect to (or start) `cua daemon` in the background, so CLI and
            // MCP clients share Spaces with the app and Fleet streams have
            // their media bridge. Failure only disables Fleet streaming.
            {
                let core = core.clone();
                tauri::async_runtime::spawn(async move {
                    let status = core.daemon_status(true).await;
                    if let Some(error) = status.error {
                        eprintln!("[cua-spaces] cua daemon unavailable: {error}");
                    }
                });
            }

            // A regular app: the main window is in the Dock and Cmd-Tab; the
            // notch and the menu bar item are the quick paths into it.
            #[cfg(target_os = "macos")]
            {
                app.set_activation_policy(tauri::ActivationPolicy::Regular);
            }

            // The menu bar item (template Cua mark). Left click opens the main
            // window; right click shows its small menu.
            tray::install(app.handle())?;

            // Closing the main window hides it; the Dock, the menu bar item and
            // the notch bring it back.
            if let Some(main) = app.get_webview_window(viewer_windows::MAIN_LABEL) {
                // Open straight onto New Space, Settings, the Keyvault, This
                // machine, its setup form or first run's presentation or Done page (demos,
                // captures; the SwiftUI app takes the same start views).
                if let Some(view) = dev_hook("CUA_SPACES_START_VIEW") {
                    if matches!(
                        view.as_str(),
                        "new-space"
                            | "settings"
                            | "keyvault"
                            | "this-machine"
                            | "host-setup"
                            | "onboarding-presentation"
                            | "onboarding-drive"
                            | "onboarding-done"
                            | "drive"
                    ) {
                        let _ = main.eval(format!(
                            "if (!location.search.includes('view=')) location.search = '?view={view}'"
                        ));
                    }
                    // The Resources step for CUA_SPACES_WIZARD_IMAGE on this
                    // Mac (or CUA_SPACES_WIZARD_ON=cloud), with
                    // CUA_SPACES_WIZARD_DISK GB (the SwiftUI app takes the same).
                    if view == "new-space-resources" {
                        let q = |k: &str| {
                            dev_hook(k)
                                .unwrap_or_default()
                                .chars()
                                .filter(|c| c.is_ascii_alphanumeric() || "._-:/@".contains(*c))
                                .collect::<String>()
                        };
                        let _ = main.eval(format!(
                            "if (!location.search.includes('view=')) location.search = \
                             '?view=new-space-resources&image={}&on={}&disk={}'",
                            q("CUA_SPACES_WIZARD_IMAGE"),
                            q("CUA_SPACES_WIZARD_ON"),
                            q("CUA_SPACES_WIZARD_DISK")
                        ));
                    }
                }
                // Captures and demos: CUA_SPACES_START_VIEW=teleport opens
                // the teleport picker for the first reachable Space (or
                // CUA_SPACES_CAPTURE_SPACE), preselecting CUA_SPACES_CAPTURE_APP
                // (an app bundle path) with CUA_SPACES_CAPTURE_FILES
                // (`:`-separated) and going on to the consent screen.
                if dev_hook("CUA_SPACES_START_VIEW").as_deref() == Some("teleport") {
                    let handle = app.handle().clone();
                    let core = core.clone();
                    tauri::async_runtime::spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                        let want = dev_hook("CUA_SPACES_CAPTURE_SPACE");
                        let Ok(rows) = core.list_spaces().await else { return };
                        let Some(space) = rows
                            .into_iter()
                            .find(|r| want.as_deref().map_or(r.reachable, |w| r.id == w))
                        else {
                            return;
                        };
                        let entry = dev_hook("CUA_SPACES_CAPTURE_APP")
                            .and_then(|p| core.teleport_entry_for_path(&p).ok())
                            .and_then(|e| serde_json::to_value(e).ok());
                        let files = dev_hook("CUA_SPACES_CAPTURE_FILES")
                            .map(|v| {
                                std::env::split_paths(&v)
                                    .map(|p| p.to_string_lossy().into_owned())
                                    .collect()
                            })
                            .unwrap_or_default();
                        let request = viewer_windows::TeleportPickerRequest {
                            space_id: space.id,
                            space_name: space.name,
                            app: None,
                            auto_review: entry.is_some(),
                            entry,
                            files,
                        };
                        let state = handle.state::<viewer_windows::TeleportPickerState>();
                        let _ = viewer_windows::open_teleport_picker(handle.clone(), state, request);
                    });
                }
                // Captures: `switcher` expands the notch panel without the
                // cursor (the portal listens for `portal:expand`); `tray-menu`
                // opens the menu bar item's menu as a context menu over the
                // main window, so it can be captured window-only.
                match dev_hook("CUA_SPACES_START_VIEW").as_deref() {
                    Some("switcher") => {
                        let handle = app.handle().clone();
                        tauri::async_runtime::spawn(async move {
                            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                            if let Some(portal) = handle.get_webview_window(PORTAL_LABEL) {
                                use tauri::Emitter as _;
                                let _ = portal.emit("portal:expand", ());
                            }
                        });
                    }
                    Some("tray-menu") => {
                        let handle = app.handle().clone();
                        tauri::async_runtime::spawn(async move {
                            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                            let menu_handle = handle.clone();
                            let _ = handle.run_on_main_thread(move || {
                                if let (Some(main), Some(menu)) = (
                                    menu_handle.get_webview_window(viewer_windows::MAIN_LABEL),
                                    menu_handle
                                        .try_state::<tray::TrayMenu>()
                                        .and_then(|m| m.menu()),
                                ) {
                                    let _ = main.popup_menu_at(
                                        &menu,
                                        tauri::LogicalPosition::new(120.0, 80.0),
                                    );
                                }
                            });
                        });
                    }
                    _ => {}
                }
                let hide = main.clone();
                main.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        let _ = hide.hide();
                    }
                });
            }

            let window = app
                .get_webview_window(PORTAL_LABEL)
                .ok_or("portal window missing from tauri.conf.json")?;

            #[cfg(target_os = "macos")]
            configure_notch_window(&window)?;

            let override_style = std::env::var(DISPLAY_ENV)
                .ok()
                .and_then(|v| parse_display_override(&v));
            let monitor = current_monitor(&window).ok().map(|(m, _)| m);
            let (style, source) = resolve_display_style(override_style, monitor);

            {
                let state: State<'_, PortalState> = app.state();
                let mut inner = state.0.lock().map_err(|e| e.to_string())?;
                inner.style = style;
                inner.source = source;
                inner.accessory_activation = accessory_activation;
            }

            apply_mode(&window, WindowMode::Ambient, style)?;
            window.show()?;
            window.set_focus()?;

            // Drive the notch hover state from the GLOBAL cursor position rather
            // than CSS :hover — an accessory window gets no mouse-moved events
            // while the app is inactive, so :hover never fires. Polling the OS
            // cursor works regardless of focus.
            spawn_notch_hover_poller(window.clone());

            // Best-effort: install the global window-drag monitor when the
            // Accessibility permission is already granted. Without it the app
            // emits `window-drag-permission`; the renderer can prompt and call
            // `start_window_drag` to install it after the user grants access.
            {
                let handle = app.handle().clone();
                let state: State<'_, window_drag::WindowDragState> = app.state();
                window_drag::ensure_monitor(&handle, &state);
            }

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building Cua Spaces")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                // Flush queued usage events briefly; the rest are spooled.
                telemetry::shutdown();
            }
            // Clicking the Dock icon with no window showing reopens the main
            // window.
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { has_visible_windows, .. } = event {
                if !has_visible_windows {
                    let _ = viewer_windows::show_main_window(app, None);
                }
            }
            #[cfg(not(target_os = "macos"))]
            let _ = (app, event);
        });
}

#[cfg(test)]
mod open_web_url_tests {
    use super::checked_web_url;

    #[test]
    fn only_plain_web_urls_reach_the_opener() {
        for ok in [
            "https://checkout.stripe.com/c/pay/cs_test_1#fid=abc",
            "https://cua.ai/device?user_code=ABCD-EFGH&x=1",
            "  http://localhost:8080/x  ",
        ] {
            assert_eq!(checked_web_url(ok), Ok(ok.trim()));
        }
        for bad in [
            "",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "cua://keyvault",
            "https://",
            "https:///path",
            "https://a.test/x y",
            "https://a.test/x\ty",
            "https://a.test/\"q\"",
            "https://a.test/a\\b",
            "https://a.test/a^b",
            "https://a.test/a|b",
            "https://a.test/<b>",
            "https://a.test/a`b`",
            "https://a.test/\u{e9}",
        ] {
            assert!(checked_web_url(bad).is_err(), "{bad:?}");
        }
    }
}

#[cfg(test)]
mod dev_hook_tests {
    use super::{dev_hook_in, DEV_HOOKS};

    #[test]
    fn release_builds_read_no_hooks() {
        for name in DEV_HOOKS {
            assert_eq!(dev_hook_in(false, name, |_| Some("x".into())), None);
            assert_eq!(
                dev_hook_in(true, name, |_| Some("x".into())),
                Some("x".into())
            );
        }
    }

    /// Every hook read in the shell goes through `dev_hook`, never the
    /// environment directly.
    #[test]
    fn every_hook_read_goes_through_the_gate() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            for name in DEV_HOOKS {
                for direct in [
                    format!("env::var(\"{name}\")"),
                    format!("env::var_os(\"{name}\")"),
                ] {
                    if text.contains(&direct) {
                        offenders.push(format!("{}: {direct}", path.display()));
                    }
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "ungated dev hook reads: {offenders:?}"
        );
    }
}
