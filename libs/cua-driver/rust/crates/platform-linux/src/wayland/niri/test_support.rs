//! Fixtures shared by the niri tests.

use super::*;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

// Shapes recorded from niri 26.04 (8ed0da4) in a nested session.
pub(super) fn floating(id: u64, workspace: u64, at: (f64, f64), size: (i32, i32)) -> Value {
    json!({"id": id, "title": format!("Float {id}"), "app_id": "float.a", "pid": 100 + id,
        "workspace_id": workspace, "is_focused": false, "is_floating": true, "is_urgent": false,
        "layout": {"pos_in_scrolling_layout": null, "tile_size": [f64::from(size.0), f64::from(size.1)],
            "window_size": [size.0, size.1], "tile_pos_in_workspace_view": [at.0, at.1],
            "window_offset_in_tile": [0.0, 0.0]},
        "focus_timestamp": {"secs": 804, "nanos": 995946831}})
}

pub(super) fn tiled(id: u64, workspace: u64, tile: (f64, f64)) -> Value {
    json!({"id": id, "title": "Tiled", "app_id": "tiled.b", "pid": 100 + id,
        "workspace_id": workspace, "is_focused": false, "is_floating": false, "is_urgent": false,
        "layout": {"pos_in_scrolling_layout": [1, 1], "tile_size": [tile.0, tile.1],
            "window_size": [tile.0 as i32, tile.1 as i32], "tile_pos_in_workspace_view": null,
            "window_offset_in_tile": [0.0, 0.0]},
        "focus_timestamp": {"secs": 805, "nanos": 828275861}})
}

pub(super) fn workspace(id: u64, output: &str, active: bool, active_window: Option<u64>) -> Value {
    json!({"id": id, "idx": id, "name": null, "output": output, "is_urgent": false,
        "is_active": active, "is_focused": active, "active_window_id": active_window})
}

pub(super) fn output(
    name: &str,
    x: i32,
    y: i32,
    mode: (u32, u32),
    scale: f64,
    transform: &str,
) -> Value {
    let width = (f64::from(mode.0) / scale) as u32;
    let height = (f64::from(mode.1) / scale) as u32;
    json!({"name": name, "make": "Smithay", "model": "Winit", "serial": null, "physical_size": null,
        "modes": [{"width": mode.0, "height": mode.1, "refresh_rate": 60000, "is_preferred": true}],
        "current_mode": 0, "is_custom_mode": true, "vrr_supported": false, "vrr_enabled": false,
        "logical": {"x": x, "y": y, "width": width, "height": height, "scale": scale, "transform": transform}})
}

pub(super) fn scene(
    windows: Vec<Value>,
    workspaces: Vec<Value>,
    outputs: Vec<Value>,
    overview: bool,
) -> Scene {
    Scene {
        windows: serde_json::from_value(Value::Array(windows)).unwrap(),
        workspaces: serde_json::from_value(Value::Array(workspaces)).unwrap(),
        outputs: outputs
            .into_iter()
            .map(|o| {
                (
                    o["name"].as_str().unwrap().to_owned(),
                    serde_json::from_value(o).unwrap(),
                )
            })
            .collect(),
        overview_open: overview,
    }
}

pub(super) fn one_output(windows: Vec<Value>) -> Scene {
    scene(
        windows,
        vec![
            workspace(1, "winit", true, None),
            workspace(2, "winit", false, None),
        ],
        vec![output("winit", 0, 0, (1280, 720), 1.5, "Flipped180")],
        false,
    )
}

pub(super) fn placement(scene: &Scene, id: u64) -> Result<Placed, Unplaced> {
    scene.window(id).unwrap().placement
}

// ── Fake niri ───────────────────────────────────────────────────────────

#[derive(Default)]
pub(super) struct Fake {
    pub(super) windows: Vec<Value>,
    pub(super) workspaces: Vec<Value>,
    pub(super) overview: bool,
    /// FocusWindow is answered `Handled` but changes nothing.
    pub(super) ignore_focus: bool,
    /// FocusWindow on these ids changes nothing (a window that closed).
    pub(super) ignore_focus_on: Vec<u64>,
    /// FocusWindow on these ids switches their workspace, but keyboard
    /// focus stays elsewhere (a layer-shell surface took it).
    pub(super) focus_taken_on: Vec<u64>,
    /// After a FocusWorkspace, this many Workspaces replies still show
    /// the layout from before it (niri applies it asynchronously).
    pub(super) workspace_lag: usize,
    pub(super) stale: Option<(usize, Vec<Value>)>,
    pub(super) requests: Vec<String>,
    pub(super) connections: usize,
}

impl Fake {
    pub(super) fn reply(&mut self, request: &str) -> Value {
        self.requests.push(request.to_owned());
        let request: Value = serde_json::from_str(request).unwrap();
        match request.as_str() {
            Some("Windows") => json!({"Ok": {"Windows": self.windows}}),
            Some("Workspaces") => match &mut self.stale {
                Some((reads, stale)) if *reads > 0 => {
                    *reads -= 1;
                    json!({"Ok": {"Workspaces": stale}})
                }
                _ => json!({"Ok": {"Workspaces": self.workspaces}}),
            },
            Some("OverviewState") => {
                json!({"Ok": {"OverviewState": {"is_open": self.overview}}})
            }
            Some("FocusedWindow") => json!({"Ok": {"FocusedWindow":
                self.windows.iter().find(|w| w["is_focused"] == true)}}),
            _ => {
                let action = &request["Action"];
                if let Some(id) = action["FocusWindow"]["id"].as_u64() {
                    if !self.ignore_focus && !self.ignore_focus_on.contains(&id) {
                        let workspace = self
                            .windows
                            .iter()
                            .find(|w| w["id"] == id)
                            .map(|w| w["workspace_id"].clone());
                        if let Some(workspace) = workspace.and_then(|w| w.as_u64()) {
                            self.show_workspace(workspace, Some(id));
                        }
                        if self.focus_taken_on.contains(&id) {
                            for window in &mut self.windows {
                                window["is_focused"] = json!(false);
                            }
                        }
                    }
                } else if let Some(id) = action["FocusWorkspace"]["reference"]["Id"].as_u64() {
                    let active = self
                        .workspaces
                        .iter()
                        .find(|w| w["id"] == id)
                        .and_then(|w| w["active_window_id"].as_u64());
                    if self.workspace_lag > 0 {
                        self.stale = Some((self.workspace_lag, self.workspaces.clone()));
                    }
                    self.show_workspace(id, active);
                } else {
                    return json!({"Err": "error parsing request"});
                }
                json!({"Ok": "Handled"})
            }
        }
    }

    /// niri's focus change: the workspace becomes active on its output
    /// and focused, and `window` its active, focused window.
    fn show_workspace(&mut self, id: u64, window: Option<u64>) {
        let output = self
            .workspaces
            .iter()
            .find(|w| w["id"] == id)
            .map(|w| w["output"].clone());
        for workspace in &mut self.workspaces {
            let this = workspace["id"] == id;
            if Some(&workspace["output"]) == output.as_ref() {
                workspace["is_active"] = json!(this);
            }
            workspace["is_focused"] = json!(this);
            if this && window.is_some() {
                workspace["active_window_id"] = json!(window);
            }
        }
        for w in &mut self.windows {
            w["is_focused"] = json!(window.is_some() && w["id"] == json!(window));
        }
    }

    pub(super) fn active(&self, id: u64) -> bool {
        self.workspaces
            .iter()
            .any(|w| w["id"] == id && w["is_active"] == true)
    }

    pub(super) fn focused(&self) -> Option<u64> {
        self.windows
            .iter()
            .find(|w| w["is_focused"] == true)
            .and_then(|w| w["id"].as_u64())
    }
}

pub(super) fn fake_ipc(fake: &Arc<Mutex<Fake>>) -> Ipc<impl FnMut(Instant) -> Result<UnixStream>> {
    let fake = fake.clone();
    Ipc {
        connect: move |_| {
            let (client, server) = UnixStream::pair()?;
            let fake = fake.clone();
            fake.lock().unwrap().connections += 1;
            std::thread::spawn(move || {
                let mut lines = Lines::new(server);
                while let Ok(Some(line)) = lines.next(Instant::now() + Duration::from_secs(2)) {
                    let reply = fake
                        .lock()
                        .unwrap()
                        .reply(std::str::from_utf8(&line).unwrap());
                    if lines
                        .stream
                        .write_all(format!("{reply}\n").as_bytes())
                        .is_err()
                    {
                        return;
                    }
                }
            });
            Ok(client)
        },
        per_attempt: Duration::from_secs(1),
        total: Duration::from_secs(2),
        backoff: Duration::from_millis(1),
        lines: None,
    }
}

pub(super) fn ipc_with(
    connect: impl FnMut(Instant) -> Result<UnixStream>,
    per_attempt: Duration,
    total: Duration,
) -> Ipc<impl FnMut(Instant) -> Result<UnixStream>> {
    Ipc {
        connect,
        per_attempt,
        total,
        backoff: Duration::from_millis(1),
        lines: None,
    }
}

pub(super) fn unchanged(_: Mark) -> Result<bool> {
    Ok(false)
}

pub(super) fn display(output: &str, size: (u32, u32)) -> Result<DisplayCapture> {
    oriented(output, 6, size)
}

pub(super) fn oriented(output: &str, transform: u32, size: (u32, u32)) -> Result<DisplayCapture> {
    Ok(DisplayCapture {
        png: vec![1],
        output: output.into(),
        transform,
        size,
    })
}

// ── Focus transactions ──────────────────────────────────────────────────

pub(super) fn focus_fixture() -> Arc<Mutex<Fake>> {
    let mut prior = tiled(5, 1, (402.0, 448.0));
    prior["is_focused"] = json!(true);
    Arc::new(Mutex::new(Fake {
        windows: vec![prior, floating(7, 2, (100.0, 60.0), (300, 200))],
        workspaces: vec![
            workspace(1, "winit", true, Some(5)),
            workspace(2, "winit", false, Some(7)),
        ],
        ..Fake::default()
    }))
}

/// A fresh history; each test needs its own event stream.
pub(super) fn history() -> &'static History {
    Box::leak(Box::new(History::new()))
}

/// A niri event stream that sends `events`, each after its delay, then
/// stays open for `open`.
pub(super) fn event_stream_open(events: Vec<(Duration, Value)>, open: Duration) -> UnixStream {
    let (client, mut server) = UnixStream::pair().unwrap();
    std::thread::spawn(move || {
        let mut request = [0u8; 64];
        let _ = server.read(&mut request);
        let _ = server.write_all(b"{\"Ok\":\"Handled\"}\n");
        for (delay, event) in events {
            std::thread::sleep(delay);
            if server.write_all(format!("{event}\n").as_bytes()).is_err() {
                return;
            }
        }
        std::thread::sleep(open);
    });
    client
}

pub(super) fn event_stream(events: Vec<(Duration, Value)>) -> UnixStream {
    event_stream_open(events, Duration::from_secs(5))
}

// ── Tiled windows and desktop frames ────────────────────────────────────

/// Tiled window 5 (active) and 6 beside it, and floating window 7, on
/// workspace 1 of output winit placed at x=1920; workspace 2 hidden.
pub(super) fn tiled_scene(focused: Option<u64>) -> Scene {
    let mut windows = vec![
        tiled(5, 1, (402.0, 448.0)),
        tiled(6, 1, (402.0, 448.0)),
        floating(7, 1, (100.0, 60.0), (300, 200)),
        tiled(8, 2, (402.0, 448.0)),
    ];
    for window in &mut windows {
        window["is_focused"] = json!(Some(window["id"].as_u64().unwrap()) == focused);
    }
    scene(
        windows,
        vec![
            workspace(1, "winit", true, Some(5)),
            workspace(2, "winit", false, Some(8)),
        ],
        vec![output("winit", 1920, 0, (1280, 720), 1.5, "Flipped180")],
        false,
    )
}

/// The frame a desktop capture of [`tiled_scene`] gives, checked against
/// `history`.
pub(super) fn desktop_frame(history: &'static History) -> DesktopFrame {
    let mark = history
        .settle(
            || Ok(event_stream(vec![])),
            Duration::from_millis(10),
            Duration::from_secs(1),
        )
        .unwrap();
    capture_desktop_in(
        || Ok(tiled_scene(Some(5))),
        || Ok(mark),
        |mark| history.changed_since(mark, Expected::default()),
        || Ok(()),
    )
    .unwrap()
    .1
}
