#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::io::{self, BufRead, Write};
#[cfg(target_os = "macos")]
use std::os::raw::c_char;
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use base64::Engine;
use pip_preview::{
    AgentViewAck, AgentViewCommand, PipConfig, PipFrame, PipTargetKind, PipViewModel,
};
use serde::Serialize;
use tauri::{webview::PageLoadEvent, Emitter, Manager, State, Window};
use tauri_runtime::ResizeDirection;

const MAX_TARGETS: usize = 12;

struct ViewState(Mutex<PipViewModel>);

struct WallpaperState(Mutex<Option<String>>);

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ViewSnapshot {
    workspaces: Vec<WorkspaceSnapshot>,
    selected_workspace_id: Option<String>,
    active_view_id: Option<String>,
    frames: Vec<FrameSnapshot>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceSnapshot {
    workspace_id: String,
    label: String,
    target_count: usize,
    updated_ms: u64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FrameSnapshot {
    view_id: String,
    target_label: String,
    target_kind: &'static str,
    image_url: String,
    action_label: String,
    timestamp_ms: u64,
    cursor_position: Option<CursorSnapshot>,
}

#[derive(Clone, Serialize)]
struct CursorSnapshot {
    x: f64,
    y: f64,
}

fn snapshot(model: &PipViewModel) -> ViewSnapshot {
    ViewSnapshot {
        workspaces: model
            .workspaces()
            .into_iter()
            .map(|workspace| WorkspaceSnapshot {
                workspace_id: workspace.workspace_id,
                label: workspace.workspace_label,
                target_count: workspace.target_count,
                updated_ms: workspace.updated_ms,
            })
            .collect(),
        selected_workspace_id: model.selected_workspace_id().map(str::to_owned),
        active_view_id: model.active_view_id().map(str::to_owned),
        frames: model
            .selected_frames()
            .into_iter()
            .map(frame_snapshot)
            .collect(),
    }
}

fn frame_snapshot(frame: &PipFrame) -> FrameSnapshot {
    FrameSnapshot {
        view_id: frame.target.view_id(),
        target_label: frame.target.target_label.clone(),
        target_kind: match frame.target.target_kind {
            PipTargetKind::NativeWindow => "window",
            PipTargetKind::BrowserTab => "tab",
        },
        image_url: format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&frame.png_bytes)
        ),
        action_label: frame.action_label.clone(),
        timestamp_ms: frame.timestamp_ms,
        cursor_position: frame.cursor_position.map(|(x, y)| CursorSnapshot { x, y }),
    }
}

#[tauri::command]
fn get_snapshot(state: State<'_, ViewState>) -> ViewSnapshot {
    snapshot(&state.0.lock().expect("Agent View state poisoned"))
}

#[tauri::command]
fn get_wallpaper(state: State<'_, WallpaperState>) -> Option<String> {
    state
        .0
        .lock()
        .expect("Agent View wallpaper state poisoned")
        .clone()
}

#[tauri::command]
fn get_platform() -> &'static str {
    std::env::consts::OS
}

#[tauri::command]
fn show_agent_view(window: Window) -> Result<(), String> {
    window.show().map_err(|error| error.to_string())
}

#[tauri::command]
fn select_workspace(
    app: tauri::AppHandle,
    state: State<'_, ViewState>,
    workspace_id: String,
) -> bool {
    let mut model = state.0.lock().expect("Agent View state poisoned");
    let changed = model.select_workspace(&workspace_id);
    if changed {
        let _ = app.emit("agent-view-state", snapshot(&model));
    }
    changed
}

#[tauri::command]
fn begin_resize(window: Window, direction: String) -> Result<(), String> {
    let direction = match direction.as_str() {
        "north" => ResizeDirection::North,
        "north-east" => ResizeDirection::NorthEast,
        "east" => ResizeDirection::East,
        "south-east" => ResizeDirection::SouthEast,
        "south" => ResizeDirection::South,
        "south-west" => ResizeDirection::SouthWest,
        "west" => ResizeDirection::West,
        "north-west" => ResizeDirection::NorthWest,
        _ => return Err(format!("unknown resize direction: {direction}")),
    };
    window
        .start_resize_dragging(direction)
        .map_err(|error| error.to_string())
}

fn main() -> anyhow::Result<()> {
    anyhow::ensure!(
        std::env::args().any(|argument| argument == "--stdio"),
        "cua-agent-view must be launched by cua-driver"
    );
    let configure = read_configure()?;
    let AgentViewCommand::Configure {
        request_id,
        title,
        geometry,
    } = configure
    else {
        anyhow::bail!("first Agent View command must be configure");
    };
    let cfg = PipConfig {
        enabled: true,
        geometry,
        title,
    };
    let wallpaper_path = wallpaper_path();
    #[cfg(target_os = "macos")]
    let initial_wallpaper = None;
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    let initial_wallpaper = wallpaper_path.as_deref().and_then(wallpaper_image_url);

    tauri::Builder::default()
        .manage(ViewState(Mutex::new(PipViewModel::new(MAX_TARGETS))))
        .manage(WallpaperState(Mutex::new(initial_wallpaper)))
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
            get_wallpaper,
            get_platform,
            show_agent_view,
            select_workspace,
            begin_resize
        ])
        .setup(move |app| {
            let geometry = cfg.geometry;
            let mut builder = tauri::WebviewWindowBuilder::new(
                app,
                "agent-view",
                tauri::WebviewUrl::App("index.html".into()),
            )
            .title(&cfg.title)
            .inner_size(geometry.width as f64, geometry.height as f64)
            .min_inner_size(360.0, 260.0)
            .decorations(false)
            .transparent(true)
            // Do not expose the host desktop through the transparent window
            // while WebView content is still establishing its first paint.
            .visible(false)
            .on_page_load(|window, payload| {
                if matches!(payload.event(), PageLoadEvent::Finished) {
                    // JavaScript reveals the window after the wallpaper has
                    // decoded. This fallback prevents a broken page from
                    // leaving the companion permanently invisible.
                    let window = window.clone();
                    thread::spawn(move || {
                        thread::sleep(Duration::from_secs(5));
                        let _ = window.show();
                    });
                }
            })
            .shadow(true)
            .resizable(true)
            .always_on_top(true);
            #[cfg(target_os = "windows")]
            {
                // Windows draws a square native frame behind undecorated
                // transparent windows when its shadow is enabled. The CSS
                // shell supplies the intended rounded Mica shadow instead.
                builder = builder.shadow(false);
            }
            if let (Some(x), Some(y)) = (geometry.x, geometry.y) {
                builder = builder.position(x as f64, y as f64);
            }
            builder.build()?;
            write_ack(AgentViewAck {
                request_id,
                ok: true,
                error: None,
            });
            #[cfg(target_os = "macos")]
            if let Some(path) = wallpaper_path.clone() {
                let app = app.handle().clone();
                thread::Builder::new()
                    .name("cua-agent-view-wallpaper".to_owned())
                    .spawn(move || {
                        let Some(image_url) = wallpaper_image_url(&path) else {
                            return;
                        };
                        let state = app.state::<WallpaperState>();
                        *state.0.lock().expect("Agent View wallpaper state poisoned") =
                            Some(image_url);
                    })?;
            }
            spawn_command_reader(app.handle().clone());
            Ok(())
        })
        .run(tauri::generate_context!())?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn wallpaper_path() -> Option<String> {
    use std::ffi::CStr;

    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};

    unsafe {
        let screen: *mut AnyObject = msg_send![class!(NSScreen), mainScreen];
        if screen.is_null() {
            return None;
        }
        let workspace: *mut AnyObject = msg_send![class!(NSWorkspace), sharedWorkspace];
        let url: *mut AnyObject = msg_send![workspace, desktopImageURLForScreen: screen];
        if url.is_null() {
            return None;
        }
        let path: *mut AnyObject = msg_send![url, path];
        if path.is_null() {
            return None;
        }
        let utf8: *const c_char = msg_send![path, UTF8String];
        if utf8.is_null() {
            return None;
        }
        Some(CStr::from_ptr(utf8).to_str().ok()?.to_owned())
    }
}

#[cfg(target_os = "macos")]
fn wallpaper_image_url(path: &str) -> Option<String> {
    let output = std::env::temp_dir().join(format!(
        "cua-agent-view-wallpaper-{}.jpg",
        std::process::id()
    ));
    let status = std::process::Command::new("/usr/bin/sips")
        .args([
            "-s",
            "format",
            "jpeg",
            "-s",
            "formatOptions",
            "72",
            "-Z",
            "1280",
            path,
            "--out",
        ])
        .arg(&output)
        .status()
        .ok()?;
    if !status.success() {
        return None;
    }
    let bytes = std::fs::read(&output).ok()?;
    let _ = std::fs::remove_file(output);
    Some(format!(
        "data:image/jpeg;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

#[cfg(target_os = "windows")]
fn wallpaper_path() -> Option<String> {
    let output = std::process::Command::new("reg")
        .args(["query", r"HKCU\Control Panel\Desktop", "/v", "WallPaper"])
        .output()
        .ok()?;
    let stdout = String::from_utf8(output.stdout).ok()?;
    let configured = stdout.lines().find_map(|line| {
        let (_, value) = line.split_once("REG_SZ")?;
        let path = value.trim();
        (!path.is_empty()).then(|| path.to_owned())
    });
    configured
        .filter(|path| std::path::Path::new(path).is_file())
        .or_else(|| {
            let app_data = std::env::var_os("APPDATA")?;
            let path = std::path::PathBuf::from(app_data)
                .join("Microsoft")
                .join("Windows")
                .join("Themes")
                .join("TranscodedWallpaper");
            path.is_file().then(|| path.to_string_lossy().into_owned())
        })
}

#[cfg(target_os = "linux")]
fn wallpaper_path() -> Option<String> {
    xfce_wallpaper_path().or_else(gnome_wallpaper_path)
}

#[cfg(target_os = "linux")]
fn xfce_wallpaper_path() -> Option<String> {
    let properties = std::process::Command::new("xfconf-query")
        .args(["-c", "xfce4-desktop", "-l"])
        .output()
        .ok()?;
    let properties = String::from_utf8(properties.stdout).ok()?;
    properties
        .lines()
        .filter(|property| property.ends_with("/last-image"))
        .find_map(|property| {
            let output = std::process::Command::new("xfconf-query")
                .args(["-c", "xfce4-desktop", "-p", property])
                .output()
                .ok()?;
            let path = String::from_utf8(output.stdout).ok()?.trim().to_owned();
            std::path::Path::new(&path).is_file().then_some(path)
        })
}

#[cfg(target_os = "linux")]
fn gnome_wallpaper_path() -> Option<String> {
    ["picture-uri-dark", "picture-uri"]
        .into_iter()
        .find_map(|key| {
            let output = std::process::Command::new("gsettings")
                .args(["get", "org.gnome.desktop.background", key])
                .output()
                .ok()?;
            let value = String::from_utf8(output.stdout).ok()?;
            let path = value
                .trim()
                .trim_matches('\'')
                .strip_prefix("file://")?
                .replace("%20", " ");
            std::path::Path::new(&path).is_file().then_some(path)
        })
}

#[cfg(target_os = "windows")]
fn wallpaper_image_url(path: &str) -> Option<String> {
    let output = std::env::temp_dir().join(format!(
        "cua-agent-view-wallpaper-{}.jpg",
        std::process::id()
    ));
    let script = r#"
Add-Type -AssemblyName System.Drawing
$image = [System.Drawing.Image]::FromFile($env:CUA_AGENT_VIEW_WALLPAPER_SOURCE)
try {
  $scale = [Math]::Min(1.0, 1280.0 / [Math]::Max($image.Width, $image.Height))
  $width = [Math]::Max(1, [int]($image.Width * $scale))
  $height = [Math]::Max(1, [int]($image.Height * $scale))
  $bitmap = New-Object System.Drawing.Bitmap($width, $height)
  try {
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    try {
      $graphics.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
      $graphics.DrawImage($image, 0, 0, $width, $height)
    } finally {
      $graphics.Dispose()
    }
    $bitmap.Save($env:CUA_AGENT_VIEW_WALLPAPER_DESTINATION, [System.Drawing.Imaging.ImageFormat]::Jpeg)
  } finally {
    $bitmap.Dispose()
  }
} finally {
  $image.Dispose()
}
"#;
    let converted = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env("CUA_AGENT_VIEW_WALLPAPER_SOURCE", path)
        .env("CUA_AGENT_VIEW_WALLPAPER_DESTINATION", &output)
        .status()
        .is_ok_and(|status| status.success());
    if converted {
        let image_url = wallpaper_file_data_url(&output);
        let _ = std::fs::remove_file(output);
        if image_url.is_some() {
            return image_url;
        }
    }
    wallpaper_file_data_url(std::path::Path::new(path))
}

#[cfg(target_os = "linux")]
fn wallpaper_image_url(path: &str) -> Option<String> {
    wallpaper_file_data_url(std::path::Path::new(path))
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn wallpaper_file_data_url(path: &std::path::Path) -> Option<String> {
    const MAX_WALLPAPER_BYTES: u64 = 12 * 1024 * 1024;

    let metadata = std::fs::metadata(path).ok()?;
    if metadata.len() > MAX_WALLPAPER_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let mime = wallpaper_mime(&bytes)?;
    Some(format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

#[cfg(any(target_os = "windows", target_os = "linux", test))]
fn wallpaper_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if bytes.starts_with(b"\xff\xd8\xff") {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        return Some("image/webp");
    }

    // XFCE commonly ships its default wallpaper as SVG. WebKit can decode an
    // SVG data URL directly, preserving the same explicit wallpaper layer used
    // for acrylic on platforms where backdrop-filter cannot sample the desktop.
    let prefix = std::str::from_utf8(&bytes[..bytes.len().min(4096)]).ok()?;
    prefix
        .trim_start_matches('\u{feff}')
        .to_ascii_lowercase()
        .contains("<svg")
        .then_some("image/svg+xml")
}

fn read_configure() -> anyhow::Result<AgentViewCommand> {
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    Ok(serde_json::from_str(&line)?)
}

fn spawn_command_reader(app: tauri::AppHandle) {
    thread::Builder::new()
        .name("cua-agent-view-stdio".to_owned())
        .spawn(move || {
            let stdin = io::stdin();
            for line in stdin.lock().lines() {
                let Ok(line) = line else { break };
                let command = match serde_json::from_str::<AgentViewCommand>(&line) {
                    Ok(command) => command,
                    Err(error) => {
                        eprintln!("cua-agent-view: invalid command: {error}");
                        continue;
                    }
                };
                if !handle_command(&app, command) {
                    break;
                }
            }
            // The driver owns this companion. EOF means the daemon exited or
            // closed the pipe, so the UI must not survive as an orphan.
            app.exit(0);
        })
        .expect("spawn Agent View command reader");
}

fn handle_command(app: &tauri::AppHandle, command: AgentViewCommand) -> bool {
    match command {
        AgentViewCommand::Configure { request_id, .. } => write_ack(AgentViewAck {
            request_id,
            ok: false,
            error: Some("Agent View is already configured".to_owned()),
        }),
        AgentViewCommand::Upsert { request_id, frame } => match frame.into_pip_frame() {
            Ok(frame) => {
                update_model(app, |model| {
                    model.upsert(frame);
                });
                write_ack(AgentViewAck {
                    request_id,
                    ok: true,
                    error: None,
                });
            }
            Err(error) => write_ack(AgentViewAck {
                request_id,
                ok: false,
                error: Some(format!("invalid frame: {error}")),
            }),
        },
        AgentViewCommand::RemoveTarget {
            workspace_id,
            identity_key,
        } => update_model(app, |model| {
            model.remove_target(&workspace_id, &identity_key);
        }),
        AgentViewCommand::RemoveWorkspace { workspace_id } => update_model(app, |model| {
            model.remove_workspace(&workspace_id);
        }),
        AgentViewCommand::SetInputPassthrough {
            request_id,
            passthrough,
        } => {
            let result = app
                .get_webview_window("agent-view")
                .ok_or_else(|| "Agent View window is unavailable".to_owned())
                .and_then(|window| {
                    window
                        .set_ignore_cursor_events(passthrough)
                        .map_err(|error| error.to_string())
                });
            write_ack(AgentViewAck {
                request_id,
                ok: result.is_ok(),
                error: result.err(),
            });
        }
        AgentViewCommand::Shutdown => {
            app.exit(0);
            return false;
        }
    }
    true
}

fn update_model(app: &tauri::AppHandle, update: impl FnOnce(&mut PipViewModel)) {
    let state = app.state::<ViewState>();
    let mut model = state.0.lock().expect("Agent View state poisoned");
    update(&mut model);
    let _ = app.emit("agent-view-state", snapshot(&model));
}

fn write_ack(ack: AgentViewAck) {
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    if serde_json::to_writer(&mut stdout, &ack).is_ok() {
        let _ = stdout.write_all(b"\n");
        let _ = stdout.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::wallpaper_mime;

    #[test]
    fn detects_svg_wallpaper_with_xml_header() {
        assert_eq!(
            wallpaper_mime(b"<?xml version=\"1.0\"?>\n<svg viewBox=\"0 0 1 1\"></svg>"),
            Some("image/svg+xml")
        );
    }

    #[test]
    fn rejects_non_image_wallpaper() {
        assert_eq!(wallpaper_mime(b"not an image"), None);
    }
}
