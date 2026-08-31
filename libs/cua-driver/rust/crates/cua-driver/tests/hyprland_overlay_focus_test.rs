//! Live Hyprland regression for the native cursor overlay's focus contract.
//!
//! Run under the shared desktop lock:
//! `flock /tmp/cua-hyprland-live.lock -c '<cargo test command>'`.

#![cfg(target_os = "linux")]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

const SOCKET: &str = "/tmp/cua-overlay-agent.sock";

#[derive(Debug)]
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

fn daemon_call(name: &str, args: Value) -> Value {
    let mut stream = UnixStream::connect(SOCKET).expect("connect to test-owned daemon");
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

fn shutdown(child: &mut Child, before: &HyprState) {
    if Path::new(SOCKET).exists() {
        if let Ok(mut stream) = UnixStream::connect(SOCKET) {
            let _ = stream.write_all(b"{\"method\":\"shutdown\"}\n");
            let _ = stream.flush();
        }
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
    assert!(
        !Path::new(SOCKET).exists(),
        "refusing to reuse occupied daemon socket {SOCKET}"
    );

    let before = hypr_state();
    let binary = env!("CARGO_BIN_EXE_cua-driver");
    let mut child = Command::new(binary)
        .args([
            "serve",
            "--socket",
            SOCKET,
            "--no-permissions-gate",
            "--dangerously-bypass-approvals",
        ])
        .env("CUA_DRIVER_RS_ENABLE_WAYLAND", "1")
        .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "false")
        .env("CUA_OVERLAY_DEBUG", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start source-built overlay daemon");
    let pid = child.id();

    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && !Path::new(SOCKET).exists() {
        assert!(
            child.try_wait().unwrap().is_none(),
            "daemon exited during startup"
        );
        thread::sleep(Duration::from_millis(25));
    }
    assert!(Path::new(SOCKET).exists(), "daemon socket was not created");

    daemon_call(
        "set_agent_cursor_motion",
        json!({
            "session": "hyprland-overlay-focus",
            "glide_duration_ms": 250,
            "idle_hide_ms": 0
        }),
    );
    daemon_call(
        "set_agent_cursor_enabled",
        json!({"session": "hyprland-overlay-focus", "enabled": true}),
    );
    daemon_call(
        "move_cursor",
        json!({"session": "hyprland-overlay-focus", "x": 500.0, "y": 500.0}),
    );
    thread::sleep(Duration::from_secs(1));

    let after = hypr_state();
    let monitor_count = hypr_json("monitors")
        .as_array()
        .expect("Hyprland monitors array")
        .len();
    let daemon_clients = hypr_json("clients")
        .as_array()
        .expect("Hyprland clients array")
        .iter()
        .filter(|client| client["pid"].as_u64() == Some(u64::from(pid)))
        .cloned()
        .collect::<Vec<_>>();
    let overlay_layers = hypr_json("layers")
        .as_object()
        .expect("Hyprland layer map")
        .values()
        .flat_map(|output| {
            output["levels"]
                .as_object()
                .into_iter()
                .flat_map(|levels| levels.values())
        })
        .flat_map(|level| level.as_array().into_iter().flatten())
        .filter(|layer| layer["pid"].as_u64() == Some(u64::from(pid)))
        .filter(|layer| layer["namespace"] == "cua-agent-cursor")
        .count();

    shutdown(&mut child, &before);
    thread::sleep(Duration::from_millis(200));

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

    assert_eq!(
        overlay_layers, monitor_count,
        "native layer-shell overlay did not map exactly once per enabled output"
    );
    assert!(
        !Path::new(SOCKET).exists(),
        "test-owned daemon socket remained after shutdown"
    );
}
