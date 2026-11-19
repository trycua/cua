// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Live Hyprland regression for the native cursor overlay's focus contract.
//! Needs Hyprland 0.48 or later, the first release whose `hyprctl -j layers`
//! reports each layer's `pid`.
//!
//! Run under the shared desktop lock:
//! `flock /tmp/cua-hyprland-live.lock -c '<cargo test command>'`.

#![cfg(target_os = "linux")]

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use cua_driver_testkit::IsolatedStateRoot;
use serde_json::{json, Value};

const SESSION: &str = "hyprland-overlay-focus";
const IO_TIMEOUT: Duration = Duration::from_secs(10);
/// Hyprland events that mean a window was mapped, focused or a workspace
/// switched.
const FOCUS_EVENTS: &[&str] = &["openwindow", "activewindowv2", "workspacev2"];

#[derive(Clone, Debug)]
struct HyprState {
    workspace: String,
    active_window: String,
}

fn hypr_json(query: &str) -> Value {
    let output = Command::new("hyprctl")
        .args(["-j", query])
        .output()
        .unwrap_or_else(|error| panic!("run hyprctl -j {query}: {error}"));
    assert!(output.status.success(), "hyprctl -j {query} failed");
    serde_json::from_slice(&output.stdout).expect("parse hyprctl JSON")
}

/// `(major, minor)` from `hyprctl -j version`'s `tag`, such as `v0.56.0`.
fn hypr_version() -> (u64, u64) {
    let version = hypr_json("version");
    let tag = version["tag"].as_str().expect("Hyprland version tag");
    let mut parts = tag.trim_start_matches('v').split(['.', '-']);
    let mut next = || {
        parts
            .next()
            .and_then(|part| part.parse().ok())
            .unwrap_or_else(|| panic!("parse Hyprland version tag {tag:?}"))
    };
    (next(), next())
}

fn hypr_state() -> HyprState {
    let workspace = hypr_json("activeworkspace")["name"]
        .as_str()
        .expect("active workspace name")
        .to_owned();
    let active_window = hypr_json("activewindow")["address"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    HyprState {
        workspace,
        active_window,
    }
}

/// Records focus-related events from Hyprland's event socket until dropped.
struct FocusEvents {
    stop: Arc<AtomicBool>,
    reader: Option<thread::JoinHandle<Result<Vec<String>, String>>>,
}

impl FocusEvents {
    fn subscribe() -> Self {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
        let signature =
            std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").expect("HYPRLAND_INSTANCE_SIGNATURE");
        let path = PathBuf::from(runtime)
            .join("hypr")
            .join(signature)
            .join(".socket2.sock");
        let stream = UnixStream::connect(&path)
            .unwrap_or_else(|error| panic!("connect to {}: {error}", path.display()));
        stream
            .set_read_timeout(Some(Duration::from_millis(100)))
            .expect("set event socket timeout");
        let stop = Arc::new(AtomicBool::new(false));
        let reader = {
            let stop = Arc::clone(&stop);
            thread::spawn(move || {
                let mut events = Vec::new();
                let mut lines = BufReader::new(stream);
                let mut line = String::new();
                loop {
                    match lines.read_line(&mut line) {
                        Ok(0) => return Err("Hyprland closed its event socket".to_owned()),
                        Ok(_) => {
                            let event = line.trim_end();
                            let name = event.split(">>").next().unwrap_or_default();
                            if FOCUS_EVENTS.contains(&name) {
                                events.push(event.to_owned());
                            }
                            line.clear();
                        }
                        // Stop only once the socket is idle, so events Hyprland
                        // already sent are read first.
                        Err(error)
                            if matches!(
                                error.kind(),
                                ErrorKind::WouldBlock | ErrorKind::TimedOut
                            ) =>
                        {
                            if stop.load(Ordering::Relaxed) {
                                return Ok(events);
                            }
                        }
                        Err(error) => return Err(format!("read Hyprland event socket: {error}")),
                    }
                }
            })
        };
        Self {
            stop,
            reader: Some(reader),
        }
    }

    fn finish(mut self) -> Vec<String> {
        self.stop.store(true, Ordering::Relaxed);
        self.reader
            .take()
            .expect("event reader")
            .join()
            .expect("event reader panicked")
            .unwrap_or_else(|error| panic!("{error}"))
    }
}

impl Drop for FocusEvents {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn connect(socket: &Path) -> UnixStream {
    let stream = UnixStream::connect(socket).expect("connect to test-owned daemon");
    stream
        .set_read_timeout(Some(IO_TIMEOUT))
        .expect("set daemon read timeout");
    stream
        .set_write_timeout(Some(IO_TIMEOUT))
        .expect("set daemon write timeout");
    stream
}

fn daemon_call(socket: &Path, name: &str, args: Value) -> Value {
    let mut stream = connect(socket);
    writeln!(
        stream,
        "{}",
        json!({"method": "call", "name": name, "args": args})
    )
    .expect("write daemon request");
    stream.flush().expect("flush daemon request");
    let mut response = String::new();
    BufReader::new(stream)
        .read_line(&mut response)
        .expect("read daemon response");
    let response: Value = serde_json::from_str(&response).expect("parse daemon response");
    assert_eq!(response["ok"], true, "{name} failed: {response}");
    response
}

/// Test-owned daemon. Dropping it stops the daemon and restores the
/// workspace and window focus it started from, including after a panic. Its
/// socket lives in the isolated state root, which is removed with it.
struct Daemon {
    child: Child,
    socket: PathBuf,
    before: HyprState,
    _state: IsolatedStateRoot,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        shutdown(&mut self.child, &self.socket, &self.before);
    }
}

fn shutdown(child: &mut Child, socket: &Path, before: &HyprState) {
    if let Ok(mut stream) = UnixStream::connect(socket) {
        let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
        let _ = stream.write_all(b"{\"method\":\"shutdown\"}\n");
        let _ = stream.flush();
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if child.try_wait().ok().flatten().is_some() {
            break;
        }
        thread::sleep(Duration::from_millis(25));
    }
    if child.try_wait().ok().flatten().is_none() {
        let _ = child.kill();
        let _ = child.wait();
    }

    let _ = Command::new("hyprctl")
        .args(["dispatch", "workspace", &before.workspace])
        .output();
    if !before.active_window.is_empty() && before.active_window != "0x0" {
        let _ = Command::new("hyprctl")
            .args([
                "dispatch",
                "focuswindow",
                &format!("address:{}", before.active_window),
            ])
            .output();
    }
}

#[test]
#[ignore]
fn native_overlay_never_maps_an_active_hyprland_client() {
    assert!(
        std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some(),
        "live Hyprland session required"
    );
    let version = hypr_version();
    assert!(
        version >= (0, 48),
        "Hyprland 0.48 or later required for layer pids, found {version:?}"
    );

    let before = hypr_state();
    let focus_events = FocusEvents::subscribe();
    let state = IsolatedStateRoot::new().expect("create isolated driver state root");
    let socket = state.path().join("agent.sock");
    let binary = env!("CARGO_BIN_EXE_cua-driver");
    let child = Command::new(binary)
        .args(["serve", "--socket"])
        .arg(&socket)
        .args(["--no-permissions-gate", "--dangerously-bypass-approvals"])
        .envs(state.env())
        .env("CUA_DRIVER_RS_ENABLE_WAYLAND", "1")
        .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "false")
        .env("CUA_OVERLAY_DEBUG", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start source-built overlay daemon");
    let pid = child.id();
    let mut daemon = Daemon {
        child,
        socket: socket.clone(),
        before: before.clone(),
        _state: state,
    };

    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && !socket.exists() {
        assert!(
            daemon.child.try_wait().unwrap().is_none(),
            "daemon exited during startup"
        );
        thread::sleep(Duration::from_millis(25));
    }
    assert!(socket.exists(), "daemon socket was not created");

    // Declare the explicit session the cursor calls below select.
    daemon_call(&socket, "start_session", json!({"session": SESSION}));
    daemon_call(
        &socket,
        "set_agent_cursor_motion",
        json!({
            "session": SESSION,
            "glide_duration_ms": 250,
            "idle_hide_ms": 0
        }),
    );
    daemon_call(
        &socket,
        "set_agent_cursor_enabled",
        json!({"session": SESSION, "enabled": true}),
    );
    daemon_call(
        &socket,
        "move_cursor",
        json!({"session": SESSION, "x": 500.0, "y": 500.0}),
    );
    thread::sleep(Duration::from_secs(1));

    let after = hypr_state();
    // `hyprctl monitors` lists only enabled outputs.
    let mut layers_per_output = hypr_json("monitors")
        .as_array()
        .expect("Hyprland monitors array")
        .iter()
        .map(|monitor| {
            let name = monitor["name"].as_str().expect("monitor name").to_owned();
            (name, 0usize)
        })
        .collect::<BTreeMap<_, _>>();
    let daemon_clients = hypr_json("clients")
        .as_array()
        .expect("Hyprland clients array")
        .iter()
        .filter(|client| client["pid"].as_u64() == Some(u64::from(pid)))
        .cloned()
        .collect::<Vec<_>>();
    for (output, layers) in hypr_json("layers").as_object().expect("Hyprland layer map") {
        let count = layers["levels"]
            .as_object()
            .into_iter()
            .flat_map(|levels| levels.values())
            .flat_map(|level| level.as_array().into_iter().flatten())
            .filter(|layer| layer["pid"].as_u64() == Some(u64::from(pid)))
            .filter(|layer| layer["namespace"] == "cua-agent-cursor")
            .count();
        if count > 0 {
            *layers_per_output.entry(output.clone()).or_default() += count;
        }
    }

    let focus_events = focus_events.finish();
    drop(daemon);

    assert!(
        daemon_clients.is_empty(),
        "overlay daemon mapped a focusable Hyprland client: {daemon_clients:#?}"
    );
    assert_eq!(
        after.workspace, before.workspace,
        "overlay startup switched Hyprland workspaces"
    );
    assert_eq!(
        after.active_window, before.active_window,
        "overlay startup changed the active Hyprland client"
    );

    assert!(
        focus_events.is_empty(),
        "overlay activity mapped a window or moved focus: {focus_events:#?}"
    );
    assert!(
        layers_per_output.values().all(|&count| count == 1),
        "native layer-shell overlay did not map exactly once per enabled output: \
         {layers_per_output:#?}"
    );
}
