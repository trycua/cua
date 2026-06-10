//! Hyprland window discovery and capture helpers.
//!
//! The primary Linux backend is X11/XWayland. On Hyprland, native Wayland
//! clients never appear in _NET_CLIENT_LIST, but hyprctl clients -j exposes
//! enough read-only metadata for list_windows, and per-window screenshots
//! go through the hyprland-toplevel-export-v1 protocol
//! (`crate::wayland_capture`) — which copies the toplevel's own buffer, so
//! occluded/background windows capture their real content. grim region
//! cropping remains only as a fallback when the protocol path fails.

use anyhow::{bail, Result};
use serde::Deserialize;
use std::process::Command;
use std::time::{Duration, Instant};

use crate::x11::WindowInfo;

#[derive(Debug, Deserialize)]
struct HyprClient {
    address: String,
    mapped: bool,
    hidden: bool,
    pid: i64,
    title: String,
    class: String,
    at: [i32; 2],
    size: [i32; 2],
    #[serde(default)]
    monitor: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct HyprMonitor {
    id: i64,
    name: String,
    #[serde(default)]
    scale: f64,
    #[serde(default)]
    focused: bool,
}

#[derive(Debug, Deserialize)]
struct HyprActiveWindow {
    #[serde(default)]
    address: Option<String>,
}

pub fn list_windows(filter_pid: Option<u32>) -> Vec<WindowInfo> {
    list_windows_inner(filter_pid).unwrap_or_default()
}

/// Per-window screenshot. Tries hyprland-toplevel-export first (true
/// surface capture: correct content for occluded/background windows and
/// windows on other workspaces), falling back to a grim screen-region crop
/// of the client geometry when the protocol path is unavailable.
///
/// The protocol capture runs on a bounded scratch thread: the frame
/// dispatch loops are deadline-bounded internally, but the initial
/// connect/registry handshake is not, and this function is called
/// synchronously from the recording write path — a wedged compositor must
/// cost at most the timeout, not a hang.
pub fn screenshot_window_bytes(window_id: u64) -> Result<Vec<u8>> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let spawn = std::thread::Builder::new().name("wl-shot".into()).spawn(move || {
        let _ = tx.send(crate::wayland_capture::capture_toplevel_png(window_id));
    });
    let result = match spawn {
        Ok(_) => rx
            .recv_timeout(Duration::from_secs(6))
            .map_err(|_| anyhow::anyhow!("toplevel-export capture timed out")),
        Err(e) => Err(anyhow::anyhow!("capture thread spawn failed: {e}")),
    };
    match result {
        Ok(Ok(png)) => return Ok(png),
        Ok(Err(e)) => {
            tracing::debug!(
                "toplevel-export capture failed for 0x{window_id:x} ({e:#}); \
                 falling back to grim region crop"
            );
        }
        Err(e) => {
            tracing::debug!(
                "toplevel-export capture for 0x{window_id:x} did not complete ({e:#}); \
                 falling back to grim region crop"
            );
        }
    }
    screenshot_window_bytes_grim(window_id)
}

fn screenshot_window_bytes_grim(window_id: u64) -> Result<Vec<u8>> {
    let client = clients()?
        .into_iter()
        .find(|c| parse_address(&c.address) == Some(window_id))
        .ok_or_else(|| anyhow::anyhow!("Hyprland client 0x{window_id:x} not found"))?;
    if client.size[0] <= 1 || client.size[1] <= 1 {
        bail!("Hyprland client 0x{window_id:x} has invalid geometry");
    }

    let geometry = format!(
        "{},{} {}x{}",
        client.at[0], client.at[1], client.size[0], client.size[1]
    );
    let out = Command::new("grim")
        .args(["-g", &geometry, "-t", "png", "-"])
        .output()?;
    if !out.status.success() || out.stdout.is_empty() {
        bail!("grim failed for Hyprland geometry {geometry}");
    }
    Ok(out.stdout)
}

/// Full-desktop screenshot via grim (all outputs composited). Used by the
/// display capture path when running under Wayland, where the X11 root
/// only shows XWayland content.
pub fn screenshot_display_bytes_grim() -> Result<Vec<u8>> {
    let out = Command::new("grim").args(["-t", "png", "-"]).output()?;
    if !out.status.success() || out.stdout.is_empty() {
        bail!("grim full-output capture failed");
    }
    Ok(out.stdout)
}

/// True when running inside a Hyprland session (hyprctl reachable).
pub fn is_hyprland_session() -> bool {
    std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some()
}

/// Name of the currently focused monitor (e.g. "DP-1"), for selecting the
/// wl_output to record.
pub fn focused_monitor_name() -> Option<String> {
    monitors()
        .ok()?
        .into_iter()
        .find(|m| m.focused)
        .map(|m| m.name)
}

/// Render scale of the monitor a window currently sits on (e.g. 1.5 for
/// fractional scaling). toplevel-export buffers are physical pixels at
/// this scale while hyprctl/AT-SPI geometry is logical, so element
/// coordinates must be multiplied by it to land in screenshot pixels.
pub fn monitor_scale_for_window(window_id: u64) -> Option<f64> {
    let client = clients()
        .ok()?
        .into_iter()
        .find(|c| parse_address(&c.address) == Some(window_id))?;
    let monitor_id = client.monitor?;
    monitors()
        .ok()?
        .into_iter()
        .find(|m| m.id == monitor_id)
        .map(|m| if m.scale > 0.0 { m.scale } else { 1.0 })
}

/// Address of the currently active (focused) Hyprland window, if any.
pub fn active_window_address() -> Option<u64> {
    if !is_hyprland_session() {
        return None;
    }
    let out = Command::new("hyprctl").args(["activewindow", "-j"]).output().ok()?;
    if !out.status.success() || out.stdout.is_empty() {
        return None;
    }
    let active: HyprActiveWindow = serde_json::from_slice(&out.stdout).ok()?;
    parse_address(&active.address?)
}

/// Refocus a window by address (best effort).
///
/// Hyprland ≥0.55 replaced the hyprlang dispatch grammar with Lua
/// (`hl.dsp.focus({ window = 'address:0x...' })`); older releases use the
/// legacy `focuswindow address:0x...` form. Try modern first, then legacy.
pub fn focus_window(address: u64) {
    let modern = format!("hl.dsp.focus({{ window = 'address:0x{address:x}' }})");
    if hyprctl_dispatch(&modern) {
        return;
    }
    let legacy = format!("focuswindow address:0x{address:x}");
    let _ = hyprctl_dispatch(&legacy);
}

/// Run `hyprctl dispatch <arg>`; true only when the compositor answered
/// "ok" (hyprctl can exit 0 while printing an error).
fn hyprctl_dispatch(arg: &str) -> bool {
    Command::new("hyprctl")
        .args(["dispatch", arg])
        .output()
        .map(|o| {
            o.status.success()
                && String::from_utf8_lossy(&o.stdout).trim_start().starts_with("ok")
        })
        .unwrap_or(false)
}

/// Preserve the active window across an app launch: snapshot the focused
/// window now and watch — for the next ~2 s — for focus moving to a
/// window that did not exist before the launch, putting it back.
///
/// Hyprland focuses newly mapped windows by default; the driver's contract
/// is that the user's frontmost window must not change, so launch_app
/// restores it. Only focus grabs by NEW windows are reverted — a user
/// alt-tabbing to a pre-existing window during the watch window is left
/// alone. Detached and best-effort: if the previous window closed or
/// hyprctl fails, focus is simply left alone.
pub fn spawn_focus_restore_guard() {
    if !is_hyprland_session() {
        return;
    }
    let Some(previous) = active_window_address() else {
        return;
    };
    let preexisting: std::collections::HashSet<u64> = clients()
        .unwrap_or_default()
        .iter()
        .filter_map(|c| parse_address(&c.address))
        .collect();
    std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
            match active_window_address() {
                Some(current) if current != previous && !preexisting.contains(&current) => {
                    focus_window(previous);
                    return;
                }
                _ => {}
            }
        }
    });
}

fn list_windows_inner(filter_pid: Option<u32>) -> Result<Vec<WindowInfo>> {
    let mut out = Vec::new();
    for client in clients()? {
        if !client.mapped || client.hidden || client.size[0] <= 1 || client.size[1] <= 1 {
            continue;
        }
        let pid = u32::try_from(client.pid).ok();
        if let Some(filter_pid) = filter_pid {
            if pid != Some(filter_pid) {
                continue;
            }
        }
        let Some(window_id) = parse_address(&client.address) else {
            continue;
        };
        let title = if client.title.trim().is_empty() {
            client.class
        } else {
            client.title
        };
        if title.trim().is_empty() {
            continue;
        }
        out.push(WindowInfo {
            xid: window_id,
            pid,
            title,
            x: client.at[0],
            y: client.at[1],
            width: client.size[0] as u32,
            height: client.size[1] as u32,
        });
    }
    Ok(out)
}

fn clients() -> Result<Vec<HyprClient>> {
    if !is_hyprland_session() {
        return Ok(Vec::new());
    }
    let out = Command::new("hyprctl").args(["clients", "-j"]).output()?;
    if !out.status.success() || out.stdout.is_empty() {
        bail!("hyprctl clients -j failed");
    }
    Ok(serde_json::from_slice(&out.stdout)?)
}

fn monitors() -> Result<Vec<HyprMonitor>> {
    if !is_hyprland_session() {
        bail!("not a Hyprland session");
    }
    let out = Command::new("hyprctl").args(["monitors", "-j"]).output()?;
    if !out.status.success() || out.stdout.is_empty() {
        bail!("hyprctl monitors -j failed");
    }
    Ok(serde_json::from_slice(&out.stdout)?)
}

fn parse_address(address: &str) -> Option<u64> {
    u64::from_str_radix(address.trim_start_matches("0x"), 16).ok()
}
