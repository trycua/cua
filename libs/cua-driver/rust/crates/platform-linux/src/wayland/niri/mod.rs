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
/// position to crop from. The action takes the window by id, so focus does
/// not move, and writes to a file in a directory only this process can
/// read; the file and directory are removed afterwards. niri also puts the
/// screenshot on the clipboard, with no opt-out, so the previous clipboard
/// text is put back, and a clipboard holding non-text content refuses
/// before the action runs.
pub fn screenshot_window(window: &Window) -> Result<Vec<u8>> {
    if !window.on_screen {
        bail!("niri screenshot fallback: the window is not on screen");
    }
    let size = window
        .size
        .ok_or_else(|| anyhow!("niri reports no window size"))?;
    let version = version()?;
    if !writes_screenshot_to_path(&version) {
        bail!(
            "niri {version} cannot write a window screenshot to a path of the driver's \
             (niri 25.11 or later is required)"
        );
    }
    let clipboard = crate::clipboard::LinuxClipboard::new();
    let bytes = with_clipboard_restored(&clipboard, || screenshot_window_to_path(window.id))?;
    tracing::debug!(
        "niri: screenshotted window {} by id into a driver-owned file",
        window.id
    );
    fit_window_png(bytes, size)
}

/// `ScreenshotWindow` gained `path` in niri 25.11. Older releases ignore the
/// field and save into the user's screenshot directory.
fn writes_screenshot_to_path(version: &str) -> bool {
    let mut parts = version
        .split(|c: char| !c.is_ascii_digit())
        .map(str::parse::<u32>);
    match (parts.next(), parts.next()) {
        (Some(Ok(major)), Some(Ok(minor))) => (major, minor) >= (25, 11),
        _ => false,
    }
}

/// Run `screenshot`, which replaces the clipboard with an image, then put the
/// previous clipboard text back. Only text can be put back, so a clipboard
/// that holds anything else refuses before `screenshot` runs.
fn with_clipboard_restored(
    clipboard: &dyn cua_driver_core::clipboard::ClipboardBackend,
    screenshot: impl FnOnce() -> Result<Vec<u8>>,
) -> Result<Vec<u8>> {
    let previous = clipboard.read_text().map_err(|error| {
        anyhow!("niri screenshot fallback: the clipboard could not be read: {error}")
    })?;
    if previous.is_none() && !clipboard.available_formats().unwrap_or_default().is_empty() {
        bail!(
            "niri screenshot fallback: niri's window screenshot replaces the clipboard, \
             which holds non-text content that cannot be put back; nothing was captured"
        );
    }
    let shot = screenshot();
    // niri sets its selection from its main loop while a worker writes the
    // file, so the selection can land after the file. Wait for it first.
    let deadline = Instant::now() + Duration::from_secs(1);
    while Instant::now() < deadline
        && !clipboard
            .available_formats()
            .unwrap_or_default()
            .iter()
            .any(|format| format == "image/png")
    {
        std::thread::sleep(Duration::from_millis(25));
    }
    let _ = clipboard.write_text(previous.unwrap_or_default());
    shot
}

/// Screenshot a window by id into a fresh directory only this process can
/// read, then remove both.
fn screenshot_window_to_path(id: u64) -> Result<Vec<u8>> {
    use std::os::unix::fs::DirBuilderExt as _;
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    let dir = std::env::temp_dir().join(format!("cua-driver-niri-{}-{unique}", std::process::id()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&dir)
        .with_context(|| {
            format!(
                "niri screenshot fallback: could not create {}",
                dir.display()
            )
        })?;
    let path = dir.join("window.png");
    let result = (|| -> Result<Vec<u8>> {
        let path_arg = path
            .to_str()
            .ok_or_else(|| anyhow!("temporary path {} is not UTF-8", path.display()))?;
        ipc().action(serde_json::json!({ "ScreenshotWindow": {
            "id": id,
            "write_to_disk": true,
            "show_pointer": false,
            "path": path_arg,
        }}))?;
        // niri answers before its worker has written the file, and the
        // write is not atomic: wait until the file decodes as a whole PNG.
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Ok(bytes) = std::fs::read(&path) {
                if image::load_from_memory_with_format(&bytes, image::ImageFormat::Png).is_ok() {
                    return Ok(bytes);
                }
            }
            if Instant::now() >= deadline {
                bail!(
                    "niri screenshot fallback: no complete screenshot appeared at {}",
                    path.display()
                );
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    })();
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir(&dir);
    result
}

/// Scale a capture to the window size niri reports, so window-relative
/// coordinates and the screenshot share one frame.
fn fit_window_png(bytes: Vec<u8>, size: (u32, u32)) -> Result<Vec<u8>> {
    let image = image::load_from_memory(&bytes)?;
    if (image.width(), image.height()) == size {
        return Ok(bytes);
    }
    let image = image.resize_exact(size.0, size.1, image::imageops::FilterType::Triangle);
    let mut out = std::io::Cursor::new(Vec::new());
    image.write_to(&mut out, image::ImageFormat::Png)?;
    Ok(out.into_inner())
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
    fn fit_window_png_scales_a_capture_to_the_reported_window_size() {
        use image::ImageEncoder;
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(
                &[255, 0, 0, 255, 0, 0, 0, 255],
                2,
                1,
                image::ExtendedColorType::Rgba8,
            )
            .unwrap();
        let exact = fit_window_png(png.clone(), (2, 1)).unwrap();
        assert_eq!(&exact, &png);
        let fitted = fit_window_png(png, (4, 2)).unwrap();
        let image = image::load_from_memory(&fitted).unwrap();
        assert_eq!((image.width(), image.height()), (4, 2));
    }

    #[test]
    fn screenshot_path_needs_niri_25_11() {
        assert!(writes_screenshot_to_path("26.04 (8ed0da4)"));
        assert!(writes_screenshot_to_path("25.11"));
        assert!(!writes_screenshot_to_path("25.08 (unknown commit)"));
        assert!(!writes_screenshot_to_path("unknown"));
    }

    struct FakeClipboard {
        text: Option<String>,
        formats: Vec<String>,
        writes: std::sync::Mutex<Vec<String>>,
    }

    impl cua_driver_core::clipboard::ClipboardBackend for FakeClipboard {
        fn available_formats(&self) -> Result<Vec<String>, String> {
            Ok(self.formats.clone())
        }
        fn read_text(&self) -> Result<Option<String>, String> {
            Ok(self.text.clone())
        }
        fn write_text(&self, text: String) -> Result<(), String> {
            self.writes.lock().unwrap().push(text);
            Ok(())
        }
        fn write_image(&self, _: &str) -> Result<(), String> {
            Err("unused".into())
        }
        fn write_file_url(&self, _: &str) -> Result<(), String> {
            Err("unused".into())
        }
    }

    #[test]
    fn window_screenshot_puts_the_clipboard_text_back() {
        let clipboard = FakeClipboard {
            text: Some("KEEP-ME".into()),
            formats: vec!["image/png".into()],
            writes: Default::default(),
        };
        assert_eq!(
            with_clipboard_restored(&clipboard, || Ok(vec![1])).unwrap(),
            vec![1]
        );
        assert_eq!(
            *clipboard.writes.lock().unwrap(),
            vec!["KEEP-ME".to_owned()]
        );
    }

    #[test]
    fn window_screenshot_refuses_a_clipboard_it_cannot_put_back() {
        let clipboard = FakeClipboard {
            text: None,
            formats: vec!["image/png".into()],
            writes: Default::default(),
        };
        let error = with_clipboard_restored(&clipboard, || panic!("must not screenshot"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("non-text content"), "{error}");
        assert!(clipboard.writes.lock().unwrap().is_empty());
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
