//! niri identity, geometry and focus over niri's IPC socket.
//!
//! Window ids are niri's own window ids. Every IPC connection is attested:
//! the `NIRI_SOCKET` peer and the `WAYLAND_DISPLAY` peer must both be the
//! compositor process first recognised for this driver process. Once niri is
//! recognised, an IPC failure refuses instead of falling back to the generic
//! wlr-foreign-toplevel title join, which would lose the niri id namespace.
//!
//! niri 26.04 reports a position only for floating windows
//! (`tile_pos_in_workspace_view`): a tiled window's place depends on the
//! view's scroll position, which IPC does not report. Window capture and
//! window-coordinate pointer input therefore refuse a tiled window; the
//! desktop scope reaches it with pixels of the single output.
//!
//! Pointer input and window capture check the layout as niri reports it now,
//! before and again during the action ([`PointerGate`]). Nothing keeps a
//! history of layout changes: a `get_desktop_state` screenshot is not proven
//! fresh when desktop-scope input uses its pixels, a move away and back
//! between two reads is not seen, and IPC positions omit niri's animation
//! offset, so a window still animating is checked at its destination.

use anyhow::{anyhow, bail, Context, Result};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::compositor_peer::{peer_pid, wayland_connection_with_timeout};

mod capture;
mod focus;
mod gate;
mod ipc;
pub(super) mod keyboard;
mod scene;
#[cfg(test)]
mod test_support;

pub use self::ipc::is_session;
#[cfg(test)]
use self::test_support::*;
pub(crate) use self::{capture::*, focus::*, gate::*, ipc::*, scene::*};

// ── Observation ──────────────────────────────────────────────────────────────

pub fn list_windows() -> Result<Vec<Window>> {
    ipc().scene()?.windows()
}

pub fn window_for_id(id: u64) -> Result<Window> {
    ipc().scene()?.window(id)
}

/// The process's only niri window. A process with several windows needs an
/// exact window id; none is guessed.
pub fn window_for_pid(pid: u32) -> Option<Window> {
    let mut owned = list_windows()
        .ok()?
        .into_iter()
        .filter(|window| window.pid == Some(pid));
    let first = owned.next()?;
    owned.next().is_none().then_some(first)
}

/// The window a pid-only keyboard action means: the pid's on-screen window
/// focused most recently (the focused one first). This is focus recency,
/// not stacking, so it is used only to pick a keyboard target.
pub fn window_in_use(pid: u32) -> Option<u64> {
    pick_window_in_use(&list_windows().ok()?, pid)
}

fn pick_window_in_use(windows: &[Window], pid: u32) -> Option<u64> {
    windows
        .iter()
        .filter(|window| window.pid == Some(pid) && window.on_screen)
        .max_by_key(|window| window.focus_rank)
        .map(|window| window.id)
}

pub fn version() -> Result<String> {
    ipc().query(Request::Version)
}

/// Pointer space for `motion_absolute`: `(origin_x, origin_y, width, height)`.
/// Only outputs are queried: the gate rechecks this inside a double click.
pub fn pointer_space() -> Result<(i32, i32, u32, u32)> {
    pointer_space_of(&ipc().query(Request::Outputs)?)
}

/// The desktop frame (the single output's logical rectangle) and the pointer
/// layout, from one snapshot.
pub fn desktop_snapshot() -> Result<super::hyprland::DesktopSnapshot> {
    desktop_snapshot_in(&ipc().scene()?)
}

fn desktop_snapshot_in(scene: &Scene) -> Result<super::hyprland::DesktopSnapshot> {
    let (output, logical) = scene.single_output()?;
    Ok(super::hyprland::DesktopSnapshot {
        frame: super::hyprland::DesktopFrame {
            x: logical.x,
            y: logical.y,
            width: logical.width,
            height: logical.height,
            scale: logical.scale,
            outputs: vec![frame_output(output, logical)],
        },
        pointer: Some(scene.pointer_space()?),
    })
}

fn frame_output(output: &IpcOutput, logical: &IpcLogical) -> super::hyprland::FrameOutput {
    super::hyprland::FrameOutput {
        name: output.name.clone(),
        x: logical.x,
        y: logical.y,
        width: logical.width,
        height: logical.height,
    }
}

/// Every enabled output's logical rectangle, and their bounding box with the
/// largest scale, as Hyprland reports its frame. Desktop-scope input still
/// needs a single output; reporting sizes does not.
fn screen_frame_in(scene: &Scene) -> Result<super::hyprland::DesktopFrame> {
    let mut outputs = Vec::new();
    let mut scale: f64 = 0.0;
    for (output, logical) in scene.laid_out() {
        let logical = logical.checked()?;
        scale = scale.max(logical.scale);
        outputs.push(frame_output(output, logical));
    }
    let rects: Vec<Rect> = outputs
        .iter()
        .map(|output| Rect {
            x: output.x,
            y: output.y,
            width: output.width,
            height: output.height,
        })
        .collect();
    let (x, y, width, height) = bounding_box(&rects).context("niri has no enabled output")?;
    Ok(super::hyprland::DesktopFrame {
        x,
        y,
        width,
        height,
        scale,
        outputs,
    })
}

/// The output frame plus the monitor list `get_screen_size` reports, in the
/// shape the Hyprland adapter publishes.
pub fn screen_report() -> Result<(super::hyprland::DesktopFrame, serde_json::Value)> {
    let scene = ipc().scene()?;
    let frame = screen_frame_in(&scene)?;
    let monitors = frame
        .outputs
        .iter()
        .map(|output| {
            serde_json::json!({
                "name": output.name,
                "width": output.width,
                "height": output.height,
                "scale": scene.outputs[&output.name]
                    .logical
                    .as_ref()
                    .map_or(frame.scale, |logical| logical.scale),
                "powered": true,
                "frame_x": output.x - frame.x,
                "frame_y": output.y - frame.y,
            })
        })
        .collect();
    Ok((frame, serde_json::Value::Array(monitors)))
}

/// wl_output transform of every enabled output, by name, for orienting a
/// capture of one of them.
pub fn output_transforms() -> Result<HashMap<String, u32>> {
    ipc()
        .scene()?
        .laid_out()
        .into_iter()
        .map(|(output, logical)| Ok((output.name.clone(), transform_code(&logical.transform)?)))
        .collect()
}

/// Doctor note: `None` without `NIRI_SOCKET`, otherwise whether this process
/// drives niri, its version and the single-output limit.
pub fn health_note() -> Option<String> {
    if std::env::var_os("NIRI_SOCKET").is_none_or(|socket| socket.is_empty()) {
        return None;
    }
    if !is_session() {
        return Some(inactive_health(
            probe(RECOGNIZED.get().copied(), QUERY_TIMEOUT).0,
        ));
    }
    if let Err(reason) = pinned() {
        return Some(format!(
            "niri: {reason:#}; niri window ids, focus and geometry refuse."
        ));
    }
    let version = version().unwrap_or_else(|error| format!("version unavailable: {error:#}"));
    Some(describe_health(
        &version,
        ipc().scene().map(|scene| scene.laid_out().len()),
    ))
}

fn inactive_health(attestation: Attestation) -> String {
    match attestation {
        Attestation::Unrelated(reason) | Attestation::Refused(reason) => format!(
            "niri: NIRI_SOCKET is not this session's compositor ({reason}); niri window ids, \
             focus and geometry are off."
        ),
        Attestation::Attested(_) => "niri: NIRI_SOCKET is this session's niri, but this process \
             does not use the niri adapter (inject mode or the native Wayland backend is off)."
            .to_owned(),
    }
}

fn describe_health(version: &str, outputs: Result<usize>) -> String {
    match outputs {
        Ok(1) => format!("niri {version}: IPC attested to WAYLAND_DISPLAY."),
        Ok(count) => format!(
            "niri {version}: IPC attested to WAYLAND_DISPLAY; window capture and desktop \
             input refuse because {count} outputs are enabled (one is supported)."
        ),
        Err(error) => format!(
            "niri {version}: IPC attested to WAYLAND_DISPLAY; the output layout is \
             unavailable: {error:#}."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    // ── Placement ───────────────────────────────────────────────────────────

    #[test]
    fn the_focused_then_most_recently_focused_window_ranks_highest() {
        // A main window focused at 805 s and its floating dialog at 804 s.
        let main = tiled(5, 1, (402.0, 448.0));
        let mut dialog = floating(7, 1, (100.0, 60.0), (300, 200));
        let rank = |windows: Vec<Value>, id| one_output(windows).window(id).unwrap().focus_rank;
        assert!(
            rank(vec![main.clone(), dialog.clone()], 5)
                > rank(vec![main.clone(), dialog.clone()], 7)
        );
        dialog["is_focused"] = json!(true);
        assert!(rank(vec![main.clone(), dialog.clone()], 7) > rank(vec![main, dialog], 5));
    }

    #[test]
    fn a_pid_only_keyboard_action_goes_to_the_window_in_use() {
        // A main window focused at 805 s and its focused floating dialog.
        let main = tiled(5, 1, (402.0, 448.0));
        let mut dialog = floating(7, 1, (100.0, 60.0), (300, 200));
        dialog["pid"] = json!(105);
        dialog["is_focused"] = json!(true);
        let mut scene = one_output(vec![main, dialog]);
        scene.workspaces[0].active_window_id = Some(5);
        let windows = scene.windows().unwrap();
        assert_eq!(pick_window_in_use(&windows, 105), Some(7));
        assert_eq!(pick_window_in_use(&windows, 999), None);
    }

    #[test]
    fn desktop_snapshot_uses_the_single_output_and_refuses_several() {
        let one = scene(
            vec![],
            vec![],
            vec![output("DP-2", 100, 50, (1280, 720), 1.5, "Normal")],
            false,
        );
        let snapshot = desktop_snapshot_in(&one).unwrap();
        assert_eq!(
            (
                snapshot.frame.x,
                snapshot.frame.y,
                snapshot.frame.width,
                snapshot.frame.height
            ),
            (100, 50, 853, 480)
        );
        assert_eq!(snapshot.frame.scale, 1.5);
        assert_eq!(snapshot.pointer, Some((100, 50, 854, 480)));
        let two = scene(
            vec![],
            vec![],
            vec![
                output("DP-1", 0, 0, (1920, 1080), 1.0, "Normal"),
                output("DP-2", 1920, 0, (1920, 1080), 1.0, "Normal"),
            ],
            false,
        );
        assert_eq!(
            desktop_snapshot_in(&two).unwrap_err().to_string(),
            "niri window capture and desktop input support a single output; this layout has 2"
        );
    }

    #[test]
    fn screen_size_spans_every_output_with_the_largest_scale() {
        let two = scene(
            vec![],
            vec![],
            vec![
                output("DP-1", 0, 0, (1920, 1080), 1.0, "Normal"),
                output("DP-2", 1920, 0, (2560, 1440), 2.0, "Normal"),
            ],
            false,
        );
        let frame = screen_frame_in(&two).unwrap();
        assert_eq!(
            (frame.x, frame.y, frame.width, frame.height, frame.scale),
            (0, 0, 3200, 1080, 2.0)
        );
        assert_eq!(frame.outputs.len(), 2);
    }

    #[test]
    fn health_note_states_attestation_and_the_single_output_limit() {
        assert_eq!(
            describe_health("26.04 (8ed0da4)", Ok(1)),
            "niri 26.04 (8ed0da4): IPC attested to WAYLAND_DISPLAY."
        );
        assert!(describe_health("26.04", Ok(2)).contains("refuse because 2 outputs are enabled"));
        assert_eq!(
            inactive_health(Attestation::Unrelated(
                "NIRI_SOCKET is served by pid 1 but WAYLAND_DISPLAY by pid 2".into()
            )),
            "niri: NIRI_SOCKET is not this session's compositor (NIRI_SOCKET is served by pid 1 \
             but WAYLAND_DISPLAY by pid 2); niri window ids, focus and geometry are off."
        );
        assert!(inactive_health(Attestation::Attested(1)).starts_with(
            "niri: NIRI_SOCKET is this session's niri, but this process does not use"
        ));
    }
}
