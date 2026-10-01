// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Dragging a real app window onto a Space.
//!
//! The gesture is detected from global mouse events plus the window list:
//!
//! 1. mouse down: remember the frontmost normal window under the cursor and
//!    where its frame sat;
//! 2. mouse dragged: once that window's frame origin has shifted by more
//!    than [`MOVE_THRESHOLD`] points it is a window gesture (a text
//!    selection or in-app drag never moves a window), so emit `start`, then
//!    `move` for every later event;
//! 3. mouse up: `end`.
//!
//! A resize from the left or top edge (or a corner) shifts the origin too,
//! so `start` alone does not mean the window is being moved. Each `start`
//! carries the frame at mouse down and the frame now, and every
//! [`FRAME_SAMPLE_EVERY`]th `move` the frame again: callers tell a move from
//! a resize by comparing them (the Spaces app core's
//! `notch::drag_trigger::classify`). A resize from the right or bottom edge
//! never shifts the origin and never starts.
//!
//! [`WindowDragTracker`] is that state machine over any [`WindowSource`], so
//! it is tested with fixture window lists. [`WindowDragMonitor`] runs it on
//! the real machine: macOS today (a listen-only `CGEventTap`, which needs
//! the Accessibility permission). Linux and Windows return
//! [`UxError::Unsupported`]: Wayland does not let a client observe other
//! apps' windows or the global pointer, and the X11 and Windows backends are
//! not built yet.
//!
//! The dragged window's preview is [`capture_thumbnail_png`]: that one
//! window only, downscaled, PNG bytes in memory. Nothing is written to disk
//! or sent anywhere.

use serde::{Deserialize, Serialize};

use super::UxError;

/// A left-drag counts as a window drag once the window frame moved this far
/// (points, either axis).
pub const MOVE_THRESHOLD: f64 = 8.0;
/// A drag's `move` events carry the window's frame every this many events
/// (reading the window list on every mouse event is wasteful).
pub const FRAME_SAMPLE_EVERY: u32 = 6;
/// Default preview width (pixels).
pub const THUMBNAIL_WIDTH: usize = 320;
/// Windows smaller than this (points) are helpers, not user windows.
pub const MIN_WINDOW_SIDE: f64 = 80.0;

/// A rectangle in global top-left display points.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    /// Whether the point is inside.
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }

    /// Whether the rectangles overlap.
    pub fn intersects(&self, o: &Rect) -> bool {
        self.x < o.x + o.width
            && self.x + self.width > o.x
            && self.y < o.y + o.height
            && self.y + self.height > o.y
    }
}

/// One window as the platform reports it (front to back).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowInfo {
    /// Platform window id (macOS `kCGWindowNumber`).
    pub window_id: u32,
    /// Owning process.
    pub pid: i64,
    /// Owning app's name.
    pub owner: String,
    /// Title (empty without the Screen Recording permission on macOS).
    pub title: String,
    /// Window layer (0 = normal app windows).
    pub layer: i32,
    /// Opacity.
    pub alpha: f64,
    /// Frame.
    pub bounds: Option<Rect>,
    /// On some Space/desktop and mapped (macOS private Spaces API; `true`
    /// when unknown).
    pub visible: bool,
    /// The owning app's bundle (macOS), when known.
    pub bundle_path: Option<String>,
}

/// The windows a picker offers: normal, sized, opaque, on a display,
/// visible, titled (when titles are readable at all), one per (app, title),
/// excluding `exclude_pid` (the calling app).
pub fn user_windows(
    windows: &[WindowInfo],
    exclude_pid: i64,
    displays: Option<Rect>,
) -> Vec<WindowInfo> {
    let titles_readable = windows.iter().any(|w| !w.title.trim().is_empty());
    let mut seen = std::collections::HashSet::new();
    windows
        .iter()
        .filter(|w| w.pid != exclude_pid && w.layer == 0 && !w.owner.is_empty())
        .filter(|w| {
            let b = w.bounds.unwrap_or_default();
            b.width >= MIN_WINDOW_SIDE && b.height >= MIN_WINDOW_SIDE
        })
        .filter(|w| w.alpha > 0.01)
        .filter(|w| displays.is_none_or(|d| w.bounds.is_some_and(|b| b.intersects(&d))))
        .filter(|w| !(titles_readable && w.title.trim().is_empty()))
        .filter(|w| w.visible)
        .filter(|w| seen.insert((w.owner.clone(), w.title.clone())))
        .cloned()
        .collect()
}

/// The frontmost normal window containing the point (not `exclude_pid`'s).
pub fn frontmost_at(
    windows: &[WindowInfo],
    x: f64,
    y: f64,
    exclude_pid: i64,
) -> Option<&WindowInfo> {
    windows.iter().find(|w| {
        w.pid != exclude_pid
            && w.layer == 0
            && !w.owner.is_empty()
            && w.bounds.is_some_and(|b| b.contains(x, y))
    })
}

/// Whether a frame moved past `threshold` (Chebyshev distance).
pub fn moved(origin: (f64, f64), current: (f64, f64), threshold: f64) -> bool {
    (current.0 - origin.0).abs() > threshold || (current.1 - origin.1).abs() > threshold
}

/// Where the tracker reads windows from.
pub trait WindowSource {
    /// On-screen windows, front to back.
    fn windows(&self) -> Vec<WindowInfo>;
}

impl WindowSource for Vec<WindowInfo> {
    fn windows(&self) -> Vec<WindowInfo> {
        self.clone()
    }
}

/// Mouse phases the tracker consumes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseEvent {
    /// Left button pressed.
    Down,
    /// Moved with the left button held.
    Dragged,
    /// Left button released.
    Up,
}

/// A phase of a window drag.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DragPhase {
    /// The window started following the cursor.
    Start,
    /// The cursor moved.
    Move,
    /// Released.
    End,
}

/// The window being dragged.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DraggedWindow {
    /// Platform window id (for the thumbnail).
    pub window_id: u32,
    /// Owning process.
    pub pid: i64,
    /// Owning app's name.
    pub app_name: String,
    /// Window title.
    pub title: String,
    /// The owning app's bundle, when known (resolve it with the catalog).
    pub bundle_path: Option<String>,
}

/// One window-drag event, in global top-left display points.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowDragEvent {
    pub phase: DragPhase,
    pub x: f64,
    pub y: f64,
    /// Present on `start` and `end`.
    pub window: Option<DraggedWindow>,
    /// The window's frame at mouse down (`start` only).
    #[serde(default)]
    pub start_frame: Option<Rect>,
    /// The window's frame now: on `start`, on `end`, and on every
    /// [`FRAME_SAMPLE_EVERY`]th `move`.
    #[serde(default)]
    pub frame: Option<Rect>,
}

#[derive(Default)]
struct Pending {
    window: Option<WindowInfo>,
    frame: Option<Rect>,
}

/// The detection state machine (see the module docs).
pub struct WindowDragTracker<S: WindowSource> {
    source: S,
    exclude_pid: i64,
    threshold: f64,
    pending: Pending,
    started: Option<DraggedWindow>,
    moves: u32,
}

impl<S: WindowSource> WindowDragTracker<S> {
    /// A tracker over `source`, ignoring `exclude_pid`'s own windows.
    pub fn new(source: S, exclude_pid: i64) -> Self {
        Self {
            source,
            exclude_pid,
            threshold: MOVE_THRESHOLD,
            pending: Pending::default(),
            started: None,
            moves: 0,
        }
    }

    fn frame_of(&self, window_id: u32) -> Option<Rect> {
        self.source
            .windows()
            .into_iter()
            .find(|c| c.window_id == window_id)
            .and_then(|c| c.bounds)
    }

    /// The window source (tests swap fixture frames between events).
    pub fn source_mut(&mut self) -> &mut S {
        &mut self.source
    }

    /// Whether a drag is in progress.
    pub fn is_dragging(&self) -> bool {
        self.started.is_some()
    }

    /// Feeds one mouse event; returns what to emit.
    pub fn handle(&mut self, event: MouseEvent, x: f64, y: f64) -> Option<WindowDragEvent> {
        match event {
            MouseEvent::Down => {
                self.pending = Pending::default();
                self.started = None;
                self.moves = 0;
                let windows = self.source.windows();
                if let Some(w) = frontmost_at(&windows, x, y, self.exclude_pid) {
                    self.pending.frame = w.bounds;
                    self.pending.window = Some(w.clone());
                }
                None
            }
            MouseEvent::Dragged => {
                if let Some(started) = &self.started {
                    self.moves += 1;
                    let id = started.window_id;
                    let frame = self
                        .moves
                        .is_multiple_of(FRAME_SAMPLE_EVERY)
                        .then(|| self.frame_of(id))
                        .flatten();
                    return Some(WindowDragEvent {
                        phase: DragPhase::Move,
                        x,
                        y,
                        window: None,
                        start_frame: None,
                        frame,
                    });
                }
                let (w, start) = (self.pending.window.as_ref()?, self.pending.frame?);
                let current = self.frame_of(w.window_id)?;
                if !moved((start.x, start.y), (current.x, current.y), self.threshold) {
                    return None;
                }
                let dragged = DraggedWindow {
                    window_id: w.window_id,
                    pid: w.pid,
                    app_name: w.owner.clone(),
                    title: w.title.clone(),
                    bundle_path: w.bundle_path.clone(),
                };
                self.started = Some(dragged.clone());
                Some(WindowDragEvent {
                    phase: DragPhase::Start,
                    x,
                    y,
                    window: Some(dragged),
                    start_frame: Some(start),
                    frame: Some(current),
                })
            }
            MouseEvent::Up => {
                self.pending = Pending::default();
                let w = self.started.take()?;
                let frame = self.frame_of(w.window_id);
                Some(WindowDragEvent {
                    phase: DragPhase::End,
                    x,
                    y,
                    window: Some(w),
                    start_frame: None,
                    frame,
                })
            }
        }
    }
}

/// Downscales a captured BGRA window image (rows may be padded) to packed
/// RGBA no wider than `target_width`, keeping the aspect ratio.
pub fn thumbnail_rgba(
    bgra: &[u8],
    width: usize,
    height: usize,
    bytes_per_row: usize,
    target_width: usize,
) -> Option<(Vec<u8>, usize, usize)> {
    if width == 0
        || height == 0
        || target_width == 0
        || bytes_per_row < width.checked_mul(4)?
        || bgra.len() < bytes_per_row.checked_mul(height)?
    {
        return None;
    }
    let out_w = width.min(target_width).max(1);
    let out_h = ((height * out_w + width / 2) / width).max(1);
    let mut out = Vec::with_capacity(out_w * out_h * 4);
    for oy in 0..out_h {
        let sy = (oy * height / out_h).min(height - 1);
        let row = sy * bytes_per_row;
        for ox in 0..out_w {
            let sx = (ox * width / out_w).min(width - 1);
            let i = row + sx * 4;
            out.extend_from_slice(&[bgra[i + 2], bgra[i + 1], bgra[i], bgra[i + 3]]);
        }
    }
    Some((out, out_w, out_h))
}

/// PNG-encodes packed RGBA (in memory).
pub fn encode_png(rgba: &[u8], width: usize, height: usize) -> Option<Vec<u8>> {
    if width == 0 || height == 0 || rgba.len() != width.checked_mul(height)?.checked_mul(4)? {
        return None;
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, width as u32, height as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        // Previews live for seconds: encode fast rather than small.
        enc.set_compression(png::Compression::Fast);
        let mut w = enc.write_header().ok()?;
        w.write_image_data(rgba).ok()?;
    }
    Some(out)
}

/// `data:image/png;base64,...` for a webview `<img>`.
pub fn png_data_url(png: &[u8]) -> Option<String> {
    if png.is_empty() {
        return None;
    }
    Some(format!("data:image/png;base64,{}", base64(png)))
}

fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        s.push(T[(n >> 18) as usize & 63] as char);
        s.push(T[(n >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        s.push(if c.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    s
}

// ---- the real machine -------------------------------------------------

fn guard() -> Result<(), UxError> {
    if crate::host::host_effects_forbidden() {
        return Err(UxError::HostEffectsRefused(
            "window enumeration, capture and drag monitoring are refused in tests \
             (cfg(test) or CUA_ENV_TEST_SANDBOX=1); use fixture windows"
                .into(),
        ));
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn unsupported() -> UxError {
    UxError::Unsupported(if cfg!(target_os = "linux") {
        "window-drag detection is macOS only today: Wayland does not let a client observe \
         other apps' windows or the pointer, and the X11 backend is not built yet"
            .into()
    } else {
        "window-drag detection is macOS only today; the Windows backend is not built yet".into()
    })
}

/// Whether window-drag detection is available here.
pub fn supported() -> bool {
    cfg!(target_os = "macos")
}

/// Whether this process may observe global mouse events (macOS
/// Accessibility). `false` elsewhere.
pub fn permission_granted() -> bool {
    #[cfg(target_os = "macos")]
    {
        super::window_macos::ax_trusted()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// Asks for the permission (opens System Settings on macOS). Returns the
/// state after asking.
pub fn request_permission() -> Result<bool, UxError> {
    guard()?;
    #[cfg(target_os = "macos")]
    {
        Ok(super::window_macos::request_ax_trust())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(unsupported())
    }
}

/// The real on-screen windows, front to back, across all Spaces.
pub fn list_windows() -> Result<Vec<WindowInfo>, UxError> {
    guard()?;
    #[cfg(target_os = "macos")]
    {
        Ok(super::window_macos::all_windows())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(unsupported())
    }
}

/// The user's windows (see [`user_windows`]), excluding this process's.
pub fn list_user_windows() -> Result<Vec<WindowInfo>, UxError> {
    let all = list_windows()?;
    #[cfg(target_os = "macos")]
    let displays = super::window_macos::display_union();
    #[cfg(not(target_os = "macos"))]
    let displays = None;
    Ok(user_windows(&all, std::process::id() as i64, displays))
}

/// A PNG preview of one window only (never the screen or another window),
/// at most `max_width` pixels wide, in memory. `Ok(None)` when it cannot be
/// captured (macOS: no Screen Recording permission).
pub fn capture_thumbnail_png(window_id: u32, max_width: usize) -> Result<Option<Vec<u8>>, UxError> {
    guard()?;
    #[cfg(target_os = "macos")]
    {
        Ok(super::window_macos::capture(
            window_id,
            max_width.clamp(16, 1024),
        ))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (window_id, max_width);
        Err(unsupported())
    }
}

/// Global window-drag detection on the real machine. Dropping it (or
/// [`Self::stop`]) removes the event tap.
pub struct WindowDragMonitor {
    #[cfg(target_os = "macos")]
    inner: super::window_macos::Monitor,
}

impl WindowDragMonitor {
    /// Starts watching; `on_event` runs on the monitor thread. Fails with
    /// `PermissionDenied` without the Accessibility permission (macOS) and
    /// `Unsupported` elsewhere.
    pub fn start(on_event: Box<dyn Fn(WindowDragEvent) + Send + 'static>) -> Result<Self, UxError> {
        guard()?;
        #[cfg(target_os = "macos")]
        {
            if !super::window_macos::ax_trusted() {
                return Err(UxError::PermissionDenied(
                    "window-drag detection needs the Accessibility permission \
                     (System Settings, Privacy & Security, Accessibility)"
                        .into(),
                ));
            }
            Ok(Self {
                inner: super::window_macos::Monitor::start(on_event)?,
            })
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = on_event;
            Err(unsupported())
        }
    }

    /// Stops watching.
    pub fn stop(&self) {
        #[cfg(target_os = "macos")]
        self.inner.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn win(id: u32, owner: &str, title: &str, x: f64, y: f64) -> WindowInfo {
        WindowInfo {
            window_id: id,
            pid: id as i64 * 10,
            owner: owner.into(),
            title: title.into(),
            layer: 0,
            alpha: 1.0,
            bounds: Some(Rect {
                x,
                y,
                width: 400.0,
                height: 300.0,
            }),
            visible: true,
            bundle_path: Some(format!("/fixture/{owner}.app")),
        }
    }

    #[test]
    fn a_moving_window_starts_a_drag_and_jitter_does_not() {
        let mut t =
            WindowDragTracker::new(vec![win(1, "Blender", "scene.blend", 100.0, 100.0)], 999);
        assert_eq!(t.handle(MouseEvent::Down, 150.0, 110.0), None);
        // A text selection: the window stays put.
        assert_eq!(t.handle(MouseEvent::Dragged, 300.0, 110.0), None);
        // A wobble under the threshold.
        *t.source_mut() = vec![win(1, "Blender", "scene.blend", 104.0, 103.0)];
        assert_eq!(t.handle(MouseEvent::Dragged, 154.0, 113.0), None);
        // The frame follows the cursor: start, with the window.
        *t.source_mut() = vec![win(1, "Blender", "scene.blend", 140.0, 100.0)];
        let s = t.handle(MouseEvent::Dragged, 190.0, 110.0).unwrap();
        assert_eq!(s.phase, DragPhase::Start);
        let w = s.window.unwrap();
        assert_eq!(
            (w.window_id, w.app_name.as_str(), w.title.as_str()),
            (1, "Blender", "scene.blend")
        );
        assert_eq!(w.bundle_path.as_deref(), Some("/fixture/Blender.app"));
        assert!(t.is_dragging());
        let m = t.handle(MouseEvent::Dragged, 200.0, 20.0).unwrap();
        assert_eq!((m.phase, m.window), (DragPhase::Move, None));
        let e = t.handle(MouseEvent::Up, 200.0, 20.0).unwrap();
        assert_eq!(e.phase, DragPhase::End);
        assert_eq!(e.window.unwrap().window_id, 1);
        assert!(!t.is_dragging());
        assert_eq!(t.handle(MouseEvent::Up, 0.0, 0.0), None);
    }

    fn sized(x: f64, y: f64, width: f64, height: f64) -> WindowInfo {
        let mut w = win(1, "Google Chrome", "tab", x, y);
        w.bounds = Some(Rect {
            x,
            y,
            width,
            height,
        });
        w
    }

    #[test]
    fn start_carries_both_frames_so_a_resize_can_be_told_apart() {
        // The left edge dragged 20 pt left: the origin shifts and the width
        // grows by the same amount. The tracker starts (it cannot tell);
        // the frames let the caller see a resize.
        let mut t = WindowDragTracker::new(vec![sized(100.0, 100.0, 400.0, 300.0)], 999);
        t.handle(MouseEvent::Down, 101.0, 200.0);
        *t.source_mut() = vec![sized(80.0, 100.0, 420.0, 300.0)];
        let s = t.handle(MouseEvent::Dragged, 81.0, 200.0).unwrap();
        assert_eq!(s.phase, DragPhase::Start);
        assert_eq!(s.start_frame.unwrap().width, 400.0);
        assert_eq!(s.frame.unwrap().width, 420.0);
        // Moves sample the frame every FRAME_SAMPLE_EVERY events.
        let frames: Vec<bool> = (0..FRAME_SAMPLE_EVERY * 2)
            .map(|i| {
                t.handle(MouseEvent::Dragged, 80.0 - i as f64, 200.0)
                    .unwrap()
                    .frame
                    .is_some()
            })
            .collect();
        assert_eq!(frames.iter().filter(|f| **f).count(), 2);
        assert!(frames[FRAME_SAMPLE_EVERY as usize - 1]);
        let e = t.handle(MouseEvent::Up, 60.0, 200.0).unwrap();
        assert_eq!(e.frame.unwrap().x, 80.0);
        assert_eq!(e.start_frame, None);
        // A right-edge resize never shifts the origin: no start at all.
        let mut t = WindowDragTracker::new(vec![sized(100.0, 100.0, 400.0, 300.0)], 999);
        t.handle(MouseEvent::Down, 499.0, 200.0);
        *t.source_mut() = vec![sized(100.0, 100.0, 520.0, 300.0)];
        assert_eq!(t.handle(MouseEvent::Dragged, 619.0, 200.0), None);
    }

    #[test]
    fn own_windows_desktop_and_empty_space_never_drag() {
        let mut own = win(2, "Cua Spaces", "Spaces", 0.0, 0.0);
        own.pid = 999;
        let mut desktop = win(3, "Dock", "", 0.0, 0.0);
        desktop.layer = -2147483623;
        let mut t = WindowDragTracker::new(vec![own, desktop], 999);
        t.handle(MouseEvent::Down, 10.0, 10.0);
        *t.source_mut() = vec![];
        assert_eq!(t.handle(MouseEvent::Dragged, 100.0, 100.0), None);
        assert_eq!(t.handle(MouseEvent::Up, 100.0, 100.0), None);
        // A click on nothing, then a drag: nothing.
        let mut t = WindowDragTracker::new(vec![win(1, "A", "a", 500.0, 500.0)], 999);
        t.handle(MouseEvent::Down, 10.0, 10.0);
        assert_eq!(t.handle(MouseEvent::Dragged, 30.0, 30.0), None);
    }

    #[test]
    fn the_front_window_wins_and_a_closed_window_does_not_start() {
        let front = win(1, "Front", "f", 0.0, 0.0);
        let back = win(2, "Back", "b", 0.0, 0.0);
        let list = vec![front.clone(), back];
        assert_eq!(frontmost_at(&list, 5.0, 5.0, 0).unwrap().owner, "Front");
        let mut t = WindowDragTracker::new(list, 0);
        t.handle(MouseEvent::Down, 5.0, 5.0);
        *t.source_mut() = vec![]; // the window closed mid-gesture
        assert_eq!(t.handle(MouseEvent::Dragged, 50.0, 50.0), None);
    }

    #[test]
    fn user_windows_drop_helpers_offscreen_hidden_and_duplicates() {
        let display = Rect {
            x: 0.0,
            y: 0.0,
            width: 1440.0,
            height: 900.0,
        };
        let mut tiny = win(2, "Chrome Helper", "h", 0.0, 0.0);
        tiny.bounds = Some(Rect {
            x: 0.0,
            y: 0.0,
            width: 10.0,
            height: 10.0,
        });
        let mut clear = win(3, "Ghost", "g", 0.0, 0.0);
        clear.alpha = 0.0;
        let off = win(4, "Steam", "s", -5000.0, 0.0);
        let mut hidden = win(5, "Unity Hub", "u", 0.0, 0.0);
        hidden.visible = false;
        let untitled = win(6, "CursorViewService", "", 0.0, 0.0);
        let dup = win(7, "Blender", "scene.blend", 10.0, 10.0);
        let mine = WindowInfo {
            pid: 42,
            ..win(8, "Me", "m", 0.0, 0.0)
        };
        let list = vec![
            win(1, "Blender", "scene.blend", 0.0, 0.0),
            tiny,
            clear,
            off,
            hidden,
            untitled,
            dup,
            mine,
        ];
        let out = user_windows(&list, 42, Some(display));
        assert_eq!(out.iter().map(|w| w.window_id).collect::<Vec<_>>(), [1]);
        // Without readable titles, untitled windows stay.
        let untitled_only = vec![win(1, "A", "", 0.0, 0.0)];
        assert_eq!(user_windows(&untitled_only, 0, None).len(), 1);
    }

    #[test]
    fn thumbnails_downscale_and_encode_in_memory() {
        // Fixture image: 2x1 BGRA with a padded row.
        let bgra = [255, 0, 0, 255, 0, 0, 255, 255, 9, 9, 9, 9];
        let (rgba, w, h) = thumbnail_rgba(&bgra, 2, 1, 12, 320).unwrap();
        assert_eq!((w, h), (2, 1));
        assert_eq!(rgba, [0, 0, 255, 255, 255, 0, 0, 255]);
        let big = vec![7u8; 1280 * 800 * 4];
        let (rgba, w, h) = thumbnail_rgba(&big, 1280, 800, 1280 * 4, THUMBNAIL_WIDTH).unwrap();
        assert_eq!((w, h), (320, 200));
        let png = encode_png(&rgba, w, h).unwrap();
        assert_eq!(&png[1..4], b"PNG");
        let url = png_data_url(&png).unwrap();
        assert!(url.starts_with("data:image/png;base64,iVBORw0KGgo"));
        assert!(thumbnail_rgba(&[0; 4], 2, 2, 8, 320).is_none());
        assert!(encode_png(&[0; 3], 1, 1).is_none());
        assert_eq!(base64(b"hi"), "aGk=");
        assert_eq!(base64(b"abc"), "YWJj");
    }

    #[test]
    fn the_real_machine_is_refused_in_tests() {
        assert!(matches!(
            list_windows(),
            Err(UxError::HostEffectsRefused(_))
        ));
        assert!(matches!(
            capture_thumbnail_png(1, 320),
            Err(UxError::HostEffectsRefused(_))
        ));
        assert!(matches!(
            WindowDragMonitor::start(Box::new(|_| {})).map(|_| ()),
            Err(UxError::HostEffectsRefused(_))
        ));
    }
}
