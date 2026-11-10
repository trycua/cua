// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! niri identity, geometry and focus over niri's IPC socket.
//!
//! Window ids are niri's own window ids. Every IPC connection is attested:
//! the `NIRI_SOCKET` peer and the `WAYLAND_DISPLAY` peer must both be the
//! compositor process first recognised for this driver process. Once niri is
//! recognised, an IPC failure refuses instead of falling back to the generic
//! wlr-foreign-toplevel title join, which would lose the niri id namespace.
//!
//! niri reports a position only for a window in the workspace view
//! (`tile_pos_in_workspace_view`). Since niri#4711 (after 26.04) that
//! includes tiled windows while they intersect the view, so window capture
//! and window-coordinate pointer input work on them too. Releases before
//! that field fall back to niri's own focused-window screenshot for capture;
//! window-coordinate input still refuses, and the desktop scope reaches the
//! window with pixels of the output.
//!
//! Pointer input and window capture first wait, a few short reads at most,
//! for two consecutive reads that agree on the target, then check the layout
//! as niri reports it again during the action ([`PointerGate`]). Nothing keeps a
//! history of layout changes: a `get_desktop_state` screenshot is not proven
//! fresh when desktop-scope input uses its pixels, a move away and back
//! between two reads is not seen, and IPC positions omit niri's animation
//! offset, so a window still animating is checked at its destination.

use anyhow::{anyhow, bail, Context, Result};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use super::compositor_peer::{peer_pid, wayland_connection_with_timeout};
use crate::interaction::{self, settled};

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
    Ok(super::hyprland::DesktopSnapshot {
        frame: screen_frame_in(scene)?,
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
/// largest scale, as Hyprland reports its frame. Desktop scope uses this
/// frame: the capture composes every output and a point must land on one of
/// them, not in a gap.
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

/// Capture one window through niri's own screenshot action. This is the
/// fallback for niri releases before #4711, where a tiled window has no IPC
/// position to crop from. The action captures the focused window to the
/// configured screenshot path; the previous focus is restored and the
/// temporary file is removed.
pub fn screenshot_window(id: u64) -> Result<Vec<u8>> {
    use std::time::{Duration, Instant, SystemTime};

    let window = window_for_id(id)?;
    if !window.on_screen {
        bail!("niri screenshot fallback: the window is not on screen");
    }
    let size = window
        .size
        .ok_or_else(|| anyhow!("niri reports no window size"))?;
    let previous = focused_window_id();
    let dir = screenshot_dir()?;
    let before: std::collections::HashSet<std::path::PathBuf> = std::fs::read_dir(&dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "png"))
                .collect()
        })
        .unwrap_or_default();
    let started = SystemTime::now();

    if previous != Some(id) {
        run_niri_action(&["focus-window", "--id", &id.to_string()])?;
        wait_for_focus(id, Duration::from_millis(800));
    }
    let result = (|| -> Result<Vec<u8>> {
        run_niri_action(&["screenshot-window"])?;
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(path) = newest_screenshot(&dir, &before, started) {
                let bytes = std::fs::read(&path)?;
                let _ = std::fs::remove_file(&path);
                let image = image::load_from_memory(&bytes)?;
                if (image.width(), image.height()) == size {
                    let mut out = std::io::Cursor::new(Vec::new());
                    image.write_to(&mut out, image::ImageFormat::Png)?;
                    return Ok(out.into_inner());
                }
                let image =
                    image.resize_exact(size.0, size.1, image::imageops::FilterType::Triangle);
                let mut out = std::io::Cursor::new(Vec::new());
                image.write_to(&mut out, image::ImageFormat::Png)?;
                return Ok(out.into_inner());
            }
            if Instant::now() >= deadline {
                bail!(
                    "niri screenshot fallback: no new PNG appeared in {}",
                    dir.display()
                );
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    })();
    if let Some(previous) = previous.filter(|previous| *previous != id) {
        let _ = run_niri_action(&["focus-window", "--id", &previous.to_string()]);
    }
    result
}

fn focused_window_id() -> Option<u64> {
    let output = std::process::Command::new("niri")
        .args(["msg", "--json", "focused-window"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    serde_json::from_slice::<serde_json::Value>(&output.stdout)
        .ok()?
        .get("id")?
        .as_u64()
}

fn run_niri_action(args: &[&str]) -> Result<()> {
    let output = std::process::Command::new("niri")
        .args(["msg", "action"])
        .args(args)
        .output()?;
    if !output.status.success() {
        bail!(
            "niri action {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn wait_for_focus(id: u64, budget: std::time::Duration) {
    let deadline = std::time::Instant::now() + budget;
    while std::time::Instant::now() < deadline {
        if focused_window_id() == Some(id) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

fn screenshot_dir() -> Result<std::path::PathBuf> {
    if let Some(home) = std::env::var_os("HOME") {
        let config = std::env::var_os("NIRI_CONFIG")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from(&home).join(".config/niri/config.kdl"));
        if let Ok(text) = std::fs::read_to_string(config) {
            for line in text.lines() {
                let line = line.trim();
                if let Some(rest) = line.strip_prefix("screenshot-path") {
                    let rest = rest.trim();
                    if let Some(value) = rest.strip_prefix('"').and_then(|v| v.split('"').next()) {
                        let expanded = value.replace("~", &home.to_string_lossy());
                        let path = std::path::PathBuf::from(expanded);
                        let parent = path.parent().map(std::path::Path::to_path_buf);
                        return Ok(parent.unwrap_or(path));
                    }
                }
            }
        }
        return Ok(std::path::PathBuf::from(home).join("Pictures/Screenshots"));
    }
    bail!("niri screenshot fallback: HOME is not set")
}

fn newest_screenshot(
    dir: &std::path::Path,
    before: &std::collections::HashSet<std::path::PathBuf>,
    started: std::time::SystemTime,
) -> Option<std::path::PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut best: Option<(std::time::SystemTime, std::path::PathBuf)> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "png") || before.contains(&path) {
            continue;
        }
        let modified = entry.metadata().ok()?.modified().ok()?;
        if modified < started {
            continue;
        }
        if best.as_ref().is_none_or(|(time, _)| modified > *time) {
            best = Some((modified, path));
        }
    }
    best.map(|(_, path)| path)
}

/// Compose every output's capture into the bounding-box desktop frame niri
/// reports. Reuse the Hyprland composer: the frame layout and scale rules are
/// the same.
pub fn compose_desktop_capture(frame: &super::hyprland::DesktopFrame) -> Result<Vec<u8>> {
    let canvas = super::hyprland::compose_frame(frame, |output| {
        let png = super::niri_capture_output_png(&output.name)?;
        Ok(image::load_from_memory_with_format(&png, image::ImageFormat::Png)?.to_rgba8())
    })?;
    let mut encoded = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(canvas).write_to(&mut encoded, image::ImageFormat::Png)?;
    Ok(encoded.into_inner())
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
            probe(
                RECOGNIZED.get().copied(),
                crate::interaction::operation_deadline(QUERY_TIMEOUT),
            )
            .0,
        ));
    }
    if let Err(reason) = recognised() {
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
            "niri {version}: IPC attested to WAYLAND_DISPLAY; window-coordinate input \
             refuses because {count} outputs are enabled (window capture and desktop scope \
             are supported)."
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
    fn desktop_snapshot_spans_every_output() {
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
        let snapshot = desktop_snapshot_in(&two).unwrap();
        assert_eq!(
            (
                snapshot.frame.x,
                snapshot.frame.y,
                snapshot.frame.width,
                snapshot.frame.height
            ),
            (0, 0, 3840, 1080)
        );
        assert_eq!(snapshot.frame.outputs.len(), 2);
        assert_eq!(snapshot.frame.outputs[1].name, "DP-2");
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
    fn newest_screenshot_skips_existing_files_and_returns_the_new_one() {
        let dir = std::env::temp_dir().join(format!("niri-shot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let old = dir.join("old.png");
        std::fs::write(&old, b"old").unwrap();
        let before: std::collections::HashSet<_> = [old.clone()].into_iter().collect();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let started = std::time::SystemTime::now();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let new = dir.join("new.png");
        std::fs::write(&new, b"new").unwrap();
        assert_eq!(newest_screenshot(&dir, &before, started), Some(new));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn health_note_states_attestation_and_the_single_output_limit() {
        assert_eq!(
            describe_health("26.04 (8ed0da4)", Ok(1)),
            "niri 26.04 (8ed0da4): IPC attested to WAYLAND_DISPLAY."
        );
        assert!(describe_health("26.04", Ok(2))
            .contains("window-coordinate input refuses because 2 outputs are enabled"));
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
