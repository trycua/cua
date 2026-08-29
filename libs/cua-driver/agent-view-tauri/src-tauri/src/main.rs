#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::io::{self, BufRead, Write};
use std::sync::Mutex;
use std::thread;

use base64::Engine;
use pip_preview::{
    AgentViewAck, AgentViewCommand, PipConfig, PipFrame, PipTargetKind, PipViewModel,
};
use serde::Serialize;
use tauri::{Emitter, Manager, State, Window};
use tauri_runtime::ResizeDirection;

const MAX_TARGETS: usize = 12;

struct ViewState(Mutex<PipViewModel>);

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

    tauri::Builder::default()
        .manage(ViewState(Mutex::new(PipViewModel::new(MAX_TARGETS))))
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
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
            .shadow(true)
            .resizable(true)
            .always_on_top(true);
            if let (Some(x), Some(y)) = (geometry.x, geometry.y) {
                builder = builder.position(x as f64, y as f64);
            }
            builder.build()?;
            write_ack(AgentViewAck {
                request_id,
                ok: true,
                error: None,
            });
            spawn_command_reader(app.handle().clone());
            Ok(())
        })
        .run(tauri::generate_context!())?;
    Ok(())
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
