// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The Tauri shell. Every command is a one-liner over `openkoalabot-example-core`;
//! the only state here is which Bot threads and which presence session this
//! window holds.
//!
//! The registry lives in the app's own data directory (never `~/.cua`).
//! Session teleport waits for the approval dialog's decision; it ships with
//! Cua Spaces (source-available), so on this app's in-process runtime it is
//! refused with a message that says so. App teleport (the "Teleport an
//! app..." picker and window drags) is not part of this sample.
//!
//! Webview data stays in the app's data directory too ([`isolate_webview`]):
//! every window gets a data directory under it (Windows, Linux) or a
//! non-persistent store (macOS, where WKWebView would otherwise write to the
//! real `~/Library/WebKit/<app>` whatever `HOME` is). The page's saved state
//! goes through [`UiStateStore`], never `localStorage`.

use openkoalabot_example_core::bots::{AgentOptions, BotInfo, Bots, ThreadView};
use openkoalabot_example_core::cua_spaces::groups::{GroupChat, GroupChatStore};
use openkoalabot_example_core::cua_spaces::routines::{
    self as routines, FileStorage, FiringRecord, Routine, RoutineStore, Schedule,
};
use openkoalabot_example_core::files::SentSummary;
use openkoalabot_example_core::presence::{Avatar, Presence};
use openkoalabot_example_core::stream;
use openkoalabot_example_core::teleport::Decision;
use openkoalabot_example_core::thread::RosterEntry;
use openkoalabot_example_core::ui_state::UiStateStore;
use openkoalabot_example_core::{Core, CoreConfig, Error};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tauri::{Emitter, Manager, State};
use tokio::sync::Mutex;

type R<T> = Result<T, Error>;

struct AppState {
    core: Core,
    /// Every Bot and its thread: the shell's sends, the routine scheduler and
    /// group chats all go through it.
    bots: Arc<Bots>,
    routines: Arc<Mutex<RoutineStore>>,
    groups: Mutex<GroupChatStore>,
    presence: Mutex<Option<Presence>>,
    ui_state: Arc<UiStateStore>,
}

/// Where a window's webview keeps its data: under `data_dir`, never the
/// platform's per-user default.
///
/// Windows (WebView2) and Linux (WebKitGTK) take a data directory. WKWebView
/// cannot be pointed at one, so macOS windows use a non-persistent
/// (incognito) store and nothing reaches `~/Library/WebKit`; the page's
/// state is saved through `ui_state_set` instead. The init script hands the
/// saved state to the page before it runs.
fn isolate_webview<'a, R: tauri::Runtime, M: Manager<R>>(
    builder: tauri::WebviewWindowBuilder<'a, R, M>,
    data_dir: &std::path::Path,
    ui_state: &UiStateStore,
) -> tauri::WebviewWindowBuilder<'a, R, M> {
    builder
        .data_directory(webview_data_dir(data_dir))
        .incognito(cfg!(target_os = "macos"))
        .initialization_script(ui_state.init_script())
}

/// The webview data directory inside the app's data directory.
fn webview_data_dir(data_dir: &std::path::Path) -> PathBuf {
    data_dir.join("webview")
}

/// Saves one key of the page's state (`null` removes it).
#[tauri::command]
fn ui_state_set(s: State<'_, AppState>, key: String, value: serde_json::Value) -> R<()> {
    s.ui_state.set(&key, value)
}

/// Opens the "Install Cua" link (only the fixed Cua links).
#[tauri::command]
fn open_cua_link(url: String) -> R<()> {
    openkoalabot_example_core::teleport::open_cua_link(&url)
}

#[tauri::command]
fn list_spaces(s: State<'_, AppState>) -> R<Vec<cua_spaces_info::SpaceInfo>> {
    s.core.list_spaces()
}

#[tauri::command]
async fn add_space(
    s: State<'_, AppState>,
    url: String,
    token: Option<String>,
    name: Option<String>,
) -> R<cua_spaces_info::SpaceInfo> {
    s.core
        .add_space(
            &url,
            token.filter(|t| !t.is_empty()),
            name.filter(|n| !n.is_empty()),
        )
        .await
}

#[tauri::command]
async fn create_cloud_space(
    s: State<'_, AppState>,
    image: Option<String>,
) -> R<cua_spaces_info::SpaceInfo> {
    s.core
        .create_cloud_space(image.filter(|i| !i.is_empty()), None)
        .await
}

/// The New Space wizard's Create: one `Spaces::create` call, local or in
/// the cloud (see `openkoalabot_example_core::plan`).
#[tauri::command]
async fn create_space(
    s: State<'_, AppState>,
    plan: openkoalabot_example_core::plan::SpacePlan,
) -> R<cua_spaces_info::SpaceInfo> {
    s.core.create_space(&plan).await
}

/// A scripted walkthrough for demos and screenshots
/// (`OPENKOALABOTS_WALKTHROUGH=<file.json>`): the UI adds the listed Spaces,
/// hires the Bots and sends their messages through the same commands as a
/// person would. `None` when unset.
#[tauri::command]
fn walkthrough() -> R<Option<serde_json::Value>> {
    let Some(path) = std::env::var_os("OPENKOALABOTS_WALKTHROUGH") else {
        return Ok(None);
    };
    Ok(Some(serde_json::from_slice(&std::fs::read(path)?)?))
}

#[tauri::command]
fn cloud_configured(s: State<'_, AppState>) -> bool {
    s.core.config().cloud_from_env
}

#[tauri::command]
async fn delete_space(s: State<'_, AppState>, id: String) -> R<String> {
    s.core.delete_space(&id).await
}

/// A media ticket for the desktop, or for one window when `window_id` is set.
#[tauri::command]
async fn open_stream(
    s: State<'_, AppState>,
    space: String,
    max_fps: u32,
    max_dimension: u32,
    window_id: Option<String>,
) -> R<serde_json::Value> {
    let space = s.core.space(&space).await?;
    let ticket = match window_id.as_deref() {
        Some(w) if !w.is_empty() => {
            stream::open_window_ticket(&space, w, max_fps, max_dimension).await?
        }
        _ => stream::open_ticket(&space, max_fps, max_dimension).await?,
    };
    Ok(serde_json::to_value(ticket)?)
}

/// The Space's streamable windows (the Computer panel's window list).
#[tauri::command]
async fn list_windows(s: State<'_, AppState>, space: String) -> R<Vec<stream::WindowRow>> {
    stream::windows(&s.core.space(&space).await?).await
}

/// Picture in picture: a small always-on-top window showing the desktop or
/// one window. `label` and `route` come from `@trycua/cua/spaces/pip`
/// (`pipWindowLabel`, `encodePipRoute`); an open one is brought forward.
/// Emits `pip-closed` with the label when it closes.
#[tauri::command]
fn open_pip(
    app: tauri::AppHandle,
    label: String,
    route: String,
    title: String,
    width: f64,
    height: f64,
) -> R<()> {
    if !label.starts_with("pip-")
        || !label
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(Error::Invalid(format!("not a PiP window label: {label}")));
    }
    if let Some(w) = app.get_webview_window(&label) {
        let _ = w.set_focus();
        return Ok(());
    }
    let (width, height) = pip_window_size(width, height);
    let builder = tauri::WebviewWindowBuilder::new(
        &app,
        &label,
        tauri::WebviewUrl::App(format!("index.html#{route}").into()),
    )
    .title(&title)
    .inner_size(width, height)
    .min_inner_size(PIP_MIN.0, PIP_MIN.1)
    .always_on_top(true)
    .visible_on_all_workspaces(true)
    .resizable(true);
    // No title bar strip and no traffic lights: the window is the picture,
    // with rounded corners and a shadow, like macOS's own picture in
    // picture. It closes from its hover-only close button or Escape
    // (`PipView`) and moves by dragging the picture.
    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true);
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| Error::Invalid(format!("no app data directory: {e}")))?;
    let state = app.state::<AppState>();
    let w = isolate_webview(builder, &data_dir, &state.ui_state)
        .build()
        .map_err(|e| Error::Invalid(format!("could not open the PiP window: {e}")))?;
    #[cfg(target_os = "macos")]
    hide_traffic_lights(&w);
    let emitter = app.clone();
    w.on_window_event(move |e| {
        if matches!(e, tauri::WindowEvent::Destroyed) {
            let _ = emitter.emit("pip-closed", label.clone());
        }
    });
    Ok(())
}

/// Hides the close, minimize and zoom buttons of a PiP window (AppKit calls
/// run on the main thread).
#[cfg(target_os = "macos")]
fn hide_traffic_lights(w: &tauri::WebviewWindow) {
    let Ok(ns_window) = w.ns_window() else {
        return;
    };
    let ns_window = ns_window as usize;
    let _ = w.run_on_main_thread(move || {
        use objc2_app_kit::NSWindow;
        // SAFETY: Tauri hands out the live NSWindow of a window it just built;
        // this runs on the main thread, where AppKit requires it.
        let window = unsafe { &*(ns_window as *const NSWindow) };
        for kind in PIP_HIDDEN_BUTTONS {
            if let Some(button) = window.standardWindowButton(kind) {
                button.setHidden(true);
            }
        }
    });
}

/// The standard window buttons a PiP window hides: all three.
#[cfg(target_os = "macos")]
const PIP_HIDDEN_BUTTONS: [objc2_app_kit::NSWindowButton; 3] = [
    objc2_app_kit::NSWindowButton::CloseButton,
    objc2_app_kit::NSWindowButton::MiniaturizeButton,
    objc2_app_kit::NSWindowButton::ZoomButton,
];

/// The smallest and largest PiP window, in logical points.
const PIP_MIN: (f64, f64) = (160.0, 90.0);
const PIP_MAX: (f64, f64) = (1600.0, 1200.0);

/// The PiP window's size for a picture of `width` x `height` points: the
/// window is exactly the picture (no title bar strip), scaled to fit
/// `PIP_MIN`..`PIP_MAX` with its aspect kept, so the stream fills it with no
/// bars. A size that is not positive falls back to 16:9 at the minimum.
fn pip_window_size(width: f64, height: f64) -> (f64, f64) {
    if !(width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0) {
        return PIP_MIN;
    }
    let down = (PIP_MAX.0 / width).min(PIP_MAX.1 / height).min(1.0);
    let (w, h) = (width * down, height * down);
    let up = (PIP_MIN.0 / w).max(PIP_MIN.1 / h).max(1.0);
    ((w * up).round(), (h * up).round())
}

/// Closes a PiP window (no-op when it is not open).
#[tauri::command]
fn close_pip(app: tauri::AppHandle, label: String) -> R<()> {
    if label.starts_with("pip-")
        && let Some(w) = app.get_webview_window(&label)
    {
        w.close()
            .map_err(|e| Error::Invalid(format!("could not close the PiP window: {e}")))?;
    }
    Ok(())
}

/// The shell's Bot list (names, agents, Spaces), so routines and group
/// chats can name and hire them.
#[tauri::command]
async fn bots_sync(s: State<'_, AppState>, bots: Vec<BotInfo>) -> R<()> {
    s.bots.upsert(bots).await;
    Ok(())
}

#[tauri::command]
async fn bot_send(
    s: State<'_, AppState>,
    space: String,
    bot: String,
    agent: String,
    text: String,
    name: Option<String>,
) -> R<ThreadView> {
    let known = s.bots.info(&bot).await;
    s.bots
        .upsert(vec![BotInfo {
            name: name
                .or(known.map(|k| k.name))
                .unwrap_or_else(|| bot.clone()),
            id: bot.clone(),
            agent,
            space,
        }])
        .await;
    s.bots.send(&bot, &text).await
}

#[tauri::command]
async fn bot_poll(s: State<'_, AppState>, bot: String) -> R<ThreadView> {
    s.bots.poll(&bot).await
}

/// A routine as the panel lists it: the saved routine plus its label.
#[derive(Serialize)]
struct RoutineRow {
    #[serde(flatten)]
    routine: Routine,
    label: String,
}

#[derive(Serialize)]
struct RoutinesView {
    routines: Vec<RoutineRow>,
    log: Vec<FiringRecord>,
}

async fn routines_view(s: &AppState) -> RoutinesView {
    let g = s.routines.lock().await;
    RoutinesView {
        routines: g
            .routines
            .iter()
            .map(|r| RoutineRow {
                label: r.schedule.label(),
                routine: r.clone(),
            })
            .collect(),
        log: g.log.iter().take(20).cloned().collect(),
    }
}

#[tauri::command]
async fn routines_list(s: State<'_, AppState>) -> R<RoutinesView> {
    Ok(routines_view(&s).await)
}

#[tauri::command]
async fn routine_create(
    s: State<'_, AppState>,
    bot: String,
    title: String,
    prompt: String,
    schedule: Schedule,
) -> R<RoutinesView> {
    if title.trim().is_empty() || prompt.trim().is_empty() {
        return Err(Error::Invalid(
            "a routine needs a title and a prompt".into(),
        ));
    }
    if schedule.next_fire(chrono::Utc::now()).is_none() {
        return Err(Error::Invalid("that schedule never fires".into()));
    }
    s.routines.lock().await.create(
        &bot,
        title.trim(),
        prompt.trim(),
        schedule,
        true,
        chrono::Utc::now(),
    );
    Ok(routines_view(&s).await)
}

#[tauri::command]
async fn routine_set_enabled(s: State<'_, AppState>, id: String, enabled: bool) -> R<RoutinesView> {
    s.routines.lock().await.set_enabled(&id, enabled);
    Ok(routines_view(&s).await)
}

#[tauri::command]
async fn routine_delete(s: State<'_, AppState>, id: String) -> R<RoutinesView> {
    s.routines.lock().await.delete(&id);
    Ok(routines_view(&s).await)
}

/// Run now: the routine fires whatever the clock says. The store is not
/// locked while the Bot is being started.
#[tauri::command]
async fn routine_run(s: State<'_, AppState>, id: String) -> R<RoutinesView> {
    let (routine, runner) = {
        let g = s.routines.lock().await;
        let r = g
            .routine(&id)
            .cloned()
            .ok_or_else(|| Error::Invalid(format!("no routine {id}")))?;
        (r, g.runner())
    };
    let firing = match runner {
        Some(r) => r.fire(&routine).await,
        None => routines::RoutineFiring::Failed {
            reason: "no runner attached".into(),
        },
    };
    s.routines
        .lock()
        .await
        .record(&routine, firing, chrono::Utc::now());
    Ok(routines_view(&s).await)
}

#[derive(Serialize)]
struct GroupView {
    #[serde(flatten)]
    chat: GroupChat,
    label: String,
    working: Vec<String>,
}

fn group_views(g: &GroupChatStore) -> Vec<GroupView> {
    g.chats
        .iter()
        .map(|c| GroupView {
            label: c.membership_label(),
            working: g.working_bots(&c.id),
            chat: c.clone(),
        })
        .collect()
}

#[tauri::command]
async fn groups_list(s: State<'_, AppState>) -> R<Vec<GroupView>> {
    Ok(group_views(&*s.groups.lock().await))
}

#[tauri::command]
async fn group_create(
    s: State<'_, AppState>,
    title: String,
    members: Vec<String>,
) -> R<Vec<GroupView>> {
    let mut g = s.groups.lock().await;
    g.create(title.trim(), &members)
        .map_err(|e| Error::Invalid(e.to_string()))?;
    Ok(group_views(&g))
}

#[tauri::command]
async fn group_send(s: State<'_, AppState>, chat: String, text: String) -> R<Vec<GroupView>> {
    let mut g = s.groups.lock().await;
    g.send(&text, &chat).await;
    Ok(group_views(&g))
}

/// One poll: folds the members' new replies into the transcript.
#[tauri::command]
async fn group_collect(s: State<'_, AppState>, chat: String) -> R<Vec<GroupView>> {
    let mut g = s.groups.lock().await;
    g.collect_replies(&chat).await;
    Ok(group_views(&g))
}

#[tauri::command]
async fn group_add(s: State<'_, AppState>, chat: String, bot: String) -> R<Vec<GroupView>> {
    let mut g = s.groups.lock().await;
    let _ = g.add(&bot, &chat).await;
    Ok(group_views(&g))
}

#[tauri::command]
async fn group_remove(s: State<'_, AppState>, chat: String, bot: String) -> R<Vec<GroupView>> {
    let mut g = s.groups.lock().await;
    let _ = g.remove(&bot, &chat).await;
    Ok(group_views(&g))
}

#[tauri::command]
async fn group_delete(s: State<'_, AppState>, chat: String) -> R<Vec<GroupView>> {
    let mut g = s.groups.lock().await;
    g.delete(&chat);
    Ok(group_views(&g))
}

#[tauri::command]
async fn roster(s: State<'_, AppState>, space: String) -> R<Vec<RosterEntry>> {
    openkoalabot_example_core::thread::roster(&s.core.space(&space).await?).await
}

#[tauri::command]
async fn send_file(s: State<'_, AppState>, space: String, path: String) -> R<SentSummary> {
    openkoalabot_example_core::files::send_verified(
        &s.core.space(&space).await?,
        &PathBuf::from(path),
        "openkoalabots",
    )
    .await
}

/// The composer's paperclip: the webview hands over the file's bytes (it
/// has no host path); they are staged in a private temp directory, sent
/// and verified like a dropped file, and the staging copy is removed.
#[tauri::command]
async fn send_file_bytes(
    s: State<'_, AppState>,
    space: String,
    name: String,
    bytes: Vec<u8>,
) -> R<SentSummary> {
    const MAX: usize = 64 * 1024 * 1024;
    if bytes.len() > MAX {
        return Err(Error::Invalid(format!(
            "attachments over {} MiB: drop the file on the Computer panel instead",
            MAX / (1024 * 1024)
        )));
    }
    let file_name = std::path::Path::new(&name)
        .file_name()
        .map(|n| n.to_owned())
        .unwrap_or_else(|| "attachment.bin".into());
    let dir = std::env::temp_dir().join(format!(
        "openkoalabots-attach-{}",
        openkoalabot_example_core::nonce()
    ));
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(file_name);
    let sent = async {
        std::fs::write(&path, &bytes)?;
        openkoalabot_example_core::files::send_verified(
            &s.core.space(&space).await?,
            &path,
            "openkoalabots",
        )
        .await
    }
    .await;
    let _ = std::fs::remove_dir_all(&dir);
    sent
}

#[tauri::command]
async fn teleport_manifest(s: State<'_, AppState>, app: String) -> R<serde_json::Value> {
    Ok(serde_json::to_value(
        openkoalabot_example_core::teleport::manifest(&s.core, &app, None).await?,
    )?)
}

/// Runs after the approval dialog: `decision` is what the human approved.
#[tauri::command]
async fn teleport_app(
    s: State<'_, AppState>,
    space: String,
    app: String,
    decision: Decision,
) -> R<serde_json::Value> {
    let space = s.core.space(&space).await?;
    let receipt =
        openkoalabot_example_core::teleport::teleport(&s.core, &space, &app, None, |_| {
            Some(decision)
        })
        .await?;
    Ok(serde_json::to_value(receipt)?)
}

#[tauri::command]
async fn presence_join(s: State<'_, AppState>, space: String, name: String) -> R<Vec<Avatar>> {
    let mut p = Presence::join(
        &s.core.space(&space).await?,
        &format!("openkoalabots:{name}"),
        &name,
        false,
        Duration::from_secs(10),
    )
    .await?;
    let avatars = p.avatars_now();
    if let Some(old) = s.presence.lock().await.replace(p) {
        let _ = old.leave().await;
    }
    Ok(avatars)
}

#[tauri::command]
async fn presence_pump(s: State<'_, AppState>) -> R<Vec<Avatar>> {
    let mut guard = s.presence.lock().await;
    let p = guard
        .as_mut()
        .ok_or_else(|| Error::Invalid("not in presence".into()))?;
    p.pump(Duration::from_millis(250), 32).await?;
    Ok(p.avatars_now())
}

#[tauri::command]
async fn presence_cursor(s: State<'_, AppState>, x: f64, y: f64) -> R<()> {
    match s.presence.lock().await.as_ref() {
        Some(p) => p.move_cursor(x, y).await,
        None => Ok(()),
    }
}

#[tauri::command]
async fn presence_leave(s: State<'_, AppState>) -> R<()> {
    match s.presence.lock().await.take() {
        Some(p) => p.leave().await,
        None => Ok(()),
    }
}

/// `SpaceInfo`, re-exported under a short path for the command signatures.
mod cua_spaces_info {
    pub use openkoalabot_example_core::SpaceInfo;
}

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            let data_dir = dir.clone();
            let ui_state = Arc::new(UiStateStore::in_dir(&dir));
            let routines_file = dir.join("routines.json");
            // Local Spaces run on the SDK's own runtime (containers, QEMU,
            // Lume); their state stays in this app's data directory.
            let config = CoreConfig::in_dir(dir)
                .with_cloud_from_env(
                    std::env::var_os("CUA_CLIENT_ID").is_some()
                        || std::env::var_os("FLEETS_TOKEN").is_some(),
                )
                .with_local_runtime(Arc::new(cua_daemon::local::VmmLocal::default()));
            let core = Core::new(config)?;
            // Agent runs use the default model unless OPENKOALABOTS_MODEL_URL
            // (and friends) name another endpoint.
            let bots = Arc::new(Bots::new(core.clone(), AgentOptions::from_env()));
            let mut store = RoutineStore::new(Box::new(FileStorage(routines_file)));
            store.attach(bots.clone());
            let routines = Arc::new(Mutex::new(store));
            let mut groups = GroupChatStore::new();
            groups.attach(bots.clone());
            // One scheduler loop for every routine, on Tauri's runtime.
            let every = Duration::from_secs(
                std::env::var("OPENKOALABOTS_ROUTINE_TICK_SECS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(15),
            );
            let sched = routines.clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    routines::tick_shared(&sched, chrono::Utc::now()).await;
                    tokio::time::sleep(every).await;
                }
            });
            app.manage(AppState {
                core,
                bots,
                routines,
                groups: Mutex::new(groups),
                presence: Mutex::new(None),
                ui_state: ui_state.clone(),
            });
            // The main window (tauri.conf.json, `create: false`), with its
            // webview data kept in this app's data directory.
            let main = app
                .config()
                .app
                .windows
                .iter()
                .find(|w| w.label == "main")
                .cloned()
                .ok_or("tauri.conf.json has no main window")?;
            isolate_webview(
                tauri::WebviewWindowBuilder::from_config(app.handle(), &main)?,
                &data_dir,
                &ui_state,
            )
            .build()?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            ui_state_set,
            list_spaces,
            add_space,
            create_cloud_space,
            create_space,
            cloud_configured,
            walkthrough,
            delete_space,
            open_stream,
            list_windows,
            open_pip,
            close_pip,
            bots_sync,
            bot_send,
            bot_poll,
            routines_list,
            routine_create,
            routine_set_enabled,
            routine_delete,
            routine_run,
            groups_list,
            group_create,
            group_send,
            group_collect,
            group_add,
            group_remove,
            group_delete,
            roster,
            send_file,
            send_file_bytes,
            teleport_manifest,
            teleport_app,
            open_cua_link,
            presence_join,
            presence_pump,
            presence_cursor,
            presence_leave,
        ])
        .run(tauri::generate_context!())
        .expect("error while running openkoalabots");
}

#[cfg(test)]
mod tests {
    use super::pip_window_size;

    #[test]
    fn pip_window_is_the_picture_with_no_title_bar_added() {
        // The sizes the UI asks for (`pipSize`, 480 on the long edge).
        assert_eq!(pip_window_size(480.0, 300.0), (480.0, 300.0));
        assert_eq!(pip_window_size(480.0, 253.0), (480.0, 253.0));
    }

    #[test]
    fn pip_window_keeps_the_aspect_when_clamped() {
        assert_eq!(pip_window_size(3200.0, 2000.0), (1600.0, 1000.0));
        assert_eq!(pip_window_size(1000.0, 2400.0), (500.0, 1200.0));
        assert_eq!(pip_window_size(80.0, 50.0), (160.0, 100.0));
        assert_eq!(pip_window_size(100.0, 40.0), (225.0, 90.0));
    }

    #[test]
    fn pip_window_falls_back_for_a_size_that_is_not_positive() {
        assert_eq!(pip_window_size(0.0, 300.0), (160.0, 90.0));
        assert_eq!(pip_window_size(480.0, f64::NAN), (160.0, 90.0));
    }
}
