// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Wayland/Hyprland capture backend for the RCDP CUA adapter.
//!
//! The sibling [`super::linux_capture`] backend targets a plain X11 server and
//! enumerates windows through the EWMH `_NET_CLIENT_LIST` property. That does
//! not work on a Wayland session such as Omarchy's Hyprland: native application
//! windows are Wayland surfaces with no X11 window id, and XWayland exposes no
//! usable root client list, so the X11 backend sees nothing to stream.
//!
//! This backend drives the Wayland session in process:
//!
//!   * Hyprland's IPC socket (`j/clients`, `j/monitors`, the queries `hyprctl
//!     -j` sends), with `hyprctl` itself as the fallback: enumeration and live
//!     per-window geometry. Windows are addressed by their Hyprland `address`
//!     (a hex pointer, e.g. `0x55…`), which round-trips through the shared
//!     `NativeTarget.window_id: u64`.
//!   * `wlr-screencopy` ([`super::linux_screencopy`]) for pixels: one
//!     persistent Wayland connection per stream copies the output region
//!     covering the window (or the whole monitor) into shared memory,
//!     damage-driven, at up to [`MAX_CAPTURE_FPS`]. Frames whose pixels did
//!     not change are skipped. When the compositor offers no screencopy (or
//!     `CUA_SPACESD_WAYLAND_CAPTURE=grim`), `grim -t ppm -g "X,Y WxH" -` runs
//!     per frame instead, at up to [`MAX_GRIM_CAPTURE_FPS`].
//!
//! Frames are tightly-packed BGRA (the RCDP `VideoCodec::Bgra` path); H.264
//! streams put the encoder behind it ([`super::encoded_capture`]).
//!
//! Because region capture reads the composited output, a window that is
//! partially occluded captures whatever is drawn on top of it (unlike the X11
//! backend's Composite-redirected backing store). For the foreground window
//! that is being streamed this is correct; occluded-window fidelity is a follow-up
//! (the `hyprland-toplevel-export` protocol captures a specific toplevel).
//!
//! Input is not injected here: RCDP actions on Wayland go through the
//! cua-driver tool registry, like macOS and Windows, and so does streaming
//! (interactive) input, folded into whole gestures by
//! [`super::tool_input`]. Background-safe resize goes through
//! `hyprctl dispatch`.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cua_media_protocol::SurfaceGeometry;
use cua_spacesd_provider_api::{
    AppliedWindowGeometry, CaptureConfig, CaptureEvent, CaptureLease, CaptureSink, OwnedFrame,
    PixelFormat, ProviderError, ProviderErrorCode,
};
use cua_spacesd_session::media::clock;
use serde_json::Value;

use super::linux_capture::{
    clamp_dimension, downscale, fit_dimensions, MIN_WINDOW_HEIGHT, MIN_WINDOW_WIDTH,
};
use super::{NativeTarget, NativeWindow};

/// Hard ceiling on the streamed frame rate with in-process screencopy.
const MAX_CAPTURE_FPS: u16 = 30;

/// Ceiling for the `grim` fallback, which spawns a process per frame; a
/// tighter cap keeps that spawn cost from saturating the guest.
const MAX_GRIM_CAPTURE_FPS: u16 = 10;

// ----------------------------------------------------------------------------
// Backend selection
// ----------------------------------------------------------------------------

/// Whether this process is inside a Hyprland Wayland session, in which case the
/// provider must use this backend instead of the X11 one. Keyed on
/// `HYPRLAND_INSTANCE_SIGNATURE`, which Hyprland exports into the graphical
/// session (and thus into the computer-server / cua-spacesd user services running in
/// it) and which no X11 session sets.
pub(super) fn active() -> bool {
    std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some()
}

// ----------------------------------------------------------------------------
// hyprctl address <-> shared window id
// ----------------------------------------------------------------------------

/// Parse a Hyprland `address` (`"0x55f0…"`) into the shared `window_id`.
fn addr_to_id(address: &str) -> Option<u64> {
    let hex = address.strip_prefix("0x").unwrap_or(address);
    u64::from_str_radix(hex, 16).ok()
}

/// Render a `window_id` back to the `address:` selector `hyprctl` expects.
fn id_to_addr(window_id: u64) -> String {
    format!("0x{window_id:x}")
}

// ----------------------------------------------------------------------------
// hyprctl invocation
// ----------------------------------------------------------------------------

/// A Hyprland query as JSON: `j/<command>` on Hyprland's IPC socket when the
/// query is a single word (no process spawn), else (and when the socket does
/// not answer) `hyprctl -j <args…>`.
fn hyprctl_json(args: &[&str]) -> Option<Value> {
    if let [command] = args {
        if let Some(value) = ipc_json(command) {
            return Some(value);
        }
    }
    let output = Command::new("hyprctl").arg("-j").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    serde_json::from_slice(&output.stdout).ok()
}

/// Bound on one IPC reply (a `clients` list of hundreds of windows is well
/// under 1 MB).
const MAX_IPC_REPLY: usize = 8 * 1024 * 1024;

/// Hyprland's request socket, as `hyprctl` finds it: under
/// `$XDG_RUNTIME_DIR/hypr/<signature>/` (Hyprland 0.40 and later), else
/// `/tmp/hypr/<signature>/`.
fn ipc_socket_paths() -> Vec<PathBuf> {
    let Ok(signature) = std::env::var("HYPRLAND_INSTANCE_SIGNATURE") else {
        return Vec::new();
    };
    if signature.is_empty() || signature.contains('/') || signature.contains("..") {
        return Vec::new();
    }
    let mut paths = Vec::new();
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
        paths.push(
            PathBuf::from(runtime)
                .join("hypr")
                .join(&signature)
                .join(".socket.sock"),
        );
    }
    paths.push(
        PathBuf::from("/tmp/hypr")
            .join(&signature)
            .join(".socket.sock"),
    );
    paths
}

/// One `j/<command>` query on Hyprland's IPC socket.
fn ipc_json(command: &str) -> Option<Value> {
    let path = ipc_socket_paths().into_iter().find(|p| p.exists())?;
    let mut stream = UnixStream::connect(path).ok()?;
    let timeout = Some(Duration::from_secs(1));
    stream.set_read_timeout(timeout).ok()?;
    stream.set_write_timeout(timeout).ok()?;
    stream.write_all(format!("j/{command}").as_bytes()).ok()?;
    let mut reply = Vec::new();
    stream
        .take(MAX_IPC_REPLY as u64 + 1)
        .read_to_end(&mut reply)
        .ok()?;
    if reply.len() > MAX_IPC_REPLY {
        return None;
    }
    serde_json::from_slice(&reply).ok()
}

/// A window's on-screen placement in the compositor's logical coordinate space,
/// as reported by `hyprctl clients`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WinRect {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

/// Read one client's current rectangle by address (fresh each frame so a moved
/// or resized window keeps streaming its real pixels).
fn window_rect(address: &str) -> Option<WinRect> {
    let clients = hyprctl_json(&["clients"])?;
    rect_from_clients(&clients, address)
}

/// Pure extraction of a client rectangle from a `hyprctl clients` payload.
fn rect_from_clients(clients: &Value, address: &str) -> Option<WinRect> {
    clients
        .as_array()?
        .iter()
        .find(|c| c.get("address").and_then(Value::as_str) == Some(address))
        .and_then(client_rect)
}

/// The `at: [x, y]` / `size: [w, h]` rectangle of a single client entry.
fn client_rect(client: &Value) -> Option<WinRect> {
    let at = client.get("at")?.as_array()?;
    let size = client.get("size")?.as_array()?;
    let x = at.first()?.as_i64()? as i32;
    let y = at.get(1)?.as_i64()? as i32;
    let width = size.first()?.as_i64()?;
    let height = size.get(1)?.as_i64()?;
    if width <= 0 || height <= 0 {
        return None;
    }
    Some(WinRect {
        x,
        y,
        width: width as u32,
        height: height as u32,
    })
}

// ----------------------------------------------------------------------------
// Enumeration
// ----------------------------------------------------------------------------

/// Enumerate the session's real top-level windows via `hyprctl clients`.
///
/// `on_screen_only` drops windows that are unmapped or hidden (on another
/// workspace / minimised). Empty-title windows are skipped, matching the X11
/// backend's treatment of decoration-only helper surfaces.
///
/// A failed `hyprctl clients` (Hyprland's IPC socket busy or timing out) is an
/// error, never an empty desktop: an empty list would retire every catalogued
/// target, and when the windows come back their epochs advance, so every
/// handle a caller holds goes stale ("window is gone") and focus or input
/// meant for one window silently lands on another.
pub(super) fn enumerate_windows(on_screen_only: bool) -> Result<Vec<NativeWindow>, String> {
    enumerate_windows_with(|| hyprctl_json(&["clients"]), on_screen_only)
}

/// Attempts at `hyprctl clients` before an enumeration counts as failed.
const CLIENTS_ATTEMPTS: usize = 3;

fn enumerate_windows_with(
    mut fetch: impl FnMut() -> Option<Value>,
    on_screen_only: bool,
) -> Result<Vec<NativeWindow>, String> {
    for attempt in 1..=CLIENTS_ATTEMPTS {
        if let Some(clients) = fetch().filter(Value::is_array) {
            return Ok(windows_from_clients(&clients, on_screen_only));
        }
        tracing::debug!(target: "cua_spacesd_client::linux_wayland", attempt, "hyprctl clients enumeration failed");
        if attempt < CLIENTS_ATTEMPTS {
            std::thread::sleep(std::time::Duration::from_millis(50 * attempt as u64));
        }
    }
    Err(format!(
        "Hyprland did not answer `hyprctl clients` after {CLIENTS_ATTEMPTS} attempts"
    ))
}

/// Pure mapping of a `hyprctl clients` payload onto the shared window model.
fn windows_from_clients(clients: &Value, on_screen_only: bool) -> Vec<NativeWindow> {
    let Some(array) = clients.as_array() else {
        return Vec::new();
    };
    let mut result = Vec::with_capacity(array.len());
    for client in array {
        let Some(address) = client.get("address").and_then(Value::as_str) else {
            continue;
        };
        let Some(window_id) = addr_to_id(address) else {
            continue;
        };
        let mapped = client
            .get("mapped")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let hidden = client
            .get("hidden")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let visible = mapped && !hidden;
        if on_screen_only && !visible {
            continue;
        }

        let title = client
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if title.trim().is_empty() {
            continue;
        }

        let Some(rect) = client_rect(client) else {
            continue;
        };

        let class = client
            .get("class")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .or_else(|| client.get("initialClass").and_then(Value::as_str))
            .unwrap_or_default()
            .to_string();
        let pid = client.get("pid").and_then(Value::as_i64).unwrap_or(0);

        result.push(NativeWindow {
            native: NativeTarget { pid, window_id },
            application_id: (!class.is_empty()).then(|| class.clone()),
            app_name: class,
            title,
            geometry: SurfaceGeometry {
                width_px: rect.width.max(1),
                height_px: rect.height.max(1),
                scale_factor: 1.0,
            },
            visible,
        });
    }
    result
}

// ----------------------------------------------------------------------------
// Capture
// ----------------------------------------------------------------------------

/// Pixel source of one Wayland capture stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Backend {
    /// In-process `wlr-screencopy` ([`super::linux_screencopy`]).
    Screencopy,
    /// One `grim` process per frame: the fallback when the compositor offers
    /// no screencopy, or `CUA_SPACESD_WAYLAND_CAPTURE=grim`.
    Grim,
}

impl Backend {
    /// Frame-rate ceiling. `grim` spawns a process per frame, so it keeps the
    /// old, lower cap.
    fn max_fps(self) -> u16 {
        match self {
            Backend::Screencopy => MAX_CAPTURE_FPS,
            Backend::Grim => MAX_GRIM_CAPTURE_FPS,
        }
    }
}

/// What the capture loop asks of a pixel source.
trait FrameSource: Send {
    fn backend(&self) -> Backend;
    /// Capture `rect` (global logical coordinates) from now on.
    fn set_rect(&mut self, rect: WinRect) -> Result<(), String>;
    /// Keep capturing until a frame is available and `due` has passed, or
    /// until `give_up`. Returns whether a frame is available.
    fn wait(&mut self, due: Instant, give_up: Instant) -> Result<bool, String>;
    /// The available frame as packed BGRA, `(bgra, width_px, height_px)`.
    fn take(&mut self) -> Option<(Vec<u8>, u32, u32)>;
    /// Stop capturing until the next `wait` (the stream is paused).
    fn pause(&mut self) {}
}

/// Live controls of a running capture thread.
struct Control {
    stop: AtomicBool,
    paused: AtomicBool,
    keyframe: AtomicBool,
    max_fps: AtomicU16,
}

struct WaylandCaptureLease {
    control: Arc<Control>,
    monitor: Mutex<Option<JoinHandle<()>>>,
}

impl CaptureLease for WaylandCaptureLease {
    fn request_keyframe(&self) {
        self.control.keyframe.store(true, Ordering::Release);
    }

    fn set_max_fps(&self, fps: u16) {
        self.control.max_fps.store(fps.max(1), Ordering::Relaxed);
    }

    fn set_paused(&self, paused: bool) {
        self.control.paused.store(paused, Ordering::Release);
    }

    fn stop(&self) {
        if self.control.stop.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Some(handle) = self
            .monitor
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            let _ = handle.join();
        }
    }
}

impl Drop for WaylandCaptureLease {
    fn drop(&mut self) {
        self.stop();
    }
}

pub(super) fn start(
    target: NativeTarget,
    config: &CaptureConfig,
    sink: Arc<dyn CaptureSink>,
    native_geometry: Arc<dyn Fn(u32, u32) + Send + Sync>,
) -> Result<Arc<dyn CaptureLease>, ProviderError> {
    let address = id_to_addr(target.window_id);

    // Validate the target synchronously so a missing window reports an error to
    // the session instead of silently opening a stream that never produces a
    // frame.
    if window_rect(&address).is_none() {
        return Err(ProviderError::new(
            ProviderErrorCode::TargetUnavailable,
            "Hyprland reports no window at this address",
        ));
    }

    let rect = move || {
        let clients = hyprctl_json(&["clients"]).ok_or("Hyprland did not answer `clients`")?;
        Ok(rect_from_clients(&clients, &address))
    };
    spawn_capture(Box::new(rect), config, sink, native_geometry)
}

/// Stream a fixed output region (a whole monitor, from [`displays`]) through
/// the same grabber as windows.
pub(super) fn start_region(
    bounds: (i32, i32, u32, u32),
    config: &CaptureConfig,
    sink: Arc<dyn CaptureSink>,
) -> Result<Arc<dyn CaptureLease>, ProviderError> {
    let (x, y, width, height) = bounds;
    if width == 0 || height == 0 {
        return Err(ProviderError::new(
            ProviderErrorCode::TargetUnavailable,
            "the display has no size",
        ));
    }
    let rect = WinRect {
        x,
        y,
        width,
        height,
    };
    spawn_capture(
        Box::new(move || Ok(Some(rect))),
        config,
        sink,
        Arc::new(|_, _| {}),
    )
}

/// The captured rectangle, re-read before every frame: `Ok(None)` means the
/// window is gone, `Err` that Hyprland did not answer (the last rectangle is
/// kept).
type RectFn = Box<dyn FnMut() -> Result<Option<WinRect>, String> + Send>;

/// Opens a pixel source of the given backend.
type OpenFn = Box<dyn FnMut(Backend) -> Result<Box<dyn FrameSource>, String> + Send>;

fn spawn_capture(
    rect: RectFn,
    config: &CaptureConfig,
    sink: Arc<dyn CaptureSink>,
    native_geometry: Arc<dyn Fn(u32, u32) + Send + Sync>,
) -> Result<Arc<dyn CaptureLease>, ProviderError> {
    let control = Arc::new(Control {
        stop: AtomicBool::new(false),
        paused: AtomicBool::new(false),
        keyframe: AtomicBool::new(false),
        max_fps: AtomicU16::new(config.max_fps.max(1)),
    });
    // 0 means no limit (one-shot grabs ask for the native size).
    let max_dimension = if config.max_dimension == 0 {
        u32::MAX
    } else {
        config.max_dimension
    };
    let thread_control = control.clone();
    let handle = std::thread::Builder::new()
        .name("rcdp-wayland-capture".into())
        .spawn(move || {
            capture_loop(
                rect,
                Box::new(open_source),
                preferred_backend(),
                &thread_control,
                max_dimension,
                sink,
                native_geometry,
            );
        })
        .map_err(|error| {
            ProviderError::new(
                ProviderErrorCode::CaptureFailed,
                format!("could not start Wayland capture thread: {error}"),
            )
        })?;

    Ok(Arc::new(WaylandCaptureLease {
        control,
        monitor: Mutex::new(Some(handle)),
    }))
}

fn preferred_backend() -> Backend {
    match std::env::var("CUA_SPACESD_WAYLAND_CAPTURE").as_deref() {
        Ok("grim") => Backend::Grim,
        _ => Backend::Screencopy,
    }
}

fn open_source(backend: Backend) -> Result<Box<dyn FrameSource>, String> {
    Ok(match backend {
        Backend::Screencopy => Box::new(ScreencopySource::default()),
        Backend::Grim => Box::new(GrimSource::default()),
    })
}

/// How long one wait for a frame lasts before the loop re-checks the window,
/// stop, pause and keyframe requests.
const IDLE_TICK: Duration = Duration::from_millis(250);

#[allow(clippy::too_many_arguments)]
fn capture_loop(
    mut rect_fn: RectFn,
    mut open: OpenFn,
    preferred: Backend,
    control: &Control,
    max_dimension: u32,
    sink: Arc<dyn CaptureSink>,
    native_geometry: Arc<dyn Fn(u32, u32) + Send + Sync>,
) {
    let mut source = match open(preferred) {
        Ok(source) => source,
        Err(error) => {
            tracing::warn!(target: "cua_spacesd_client::linux_wayland", %error, "Wayland screencopy unavailable; capturing with grim");
            match open(Backend::Grim) {
                Ok(source) => source,
                Err(error) => {
                    sink.on_event(CaptureEvent::Suspended(error));
                    return;
                }
            }
        }
    };
    let mut rect: Option<WinRect> = None;
    let mut last_timestamp_us = 0u64;
    let mut last_dims: Option<(u32, u32)> = None;
    let mut last_publish = Instant::now() - Duration::from_secs(1);
    // The last captured pixels (to skip unchanged frames) and the last frame
    // sent (resent on a keyframe request).
    let mut last_source: Option<(Arc<[u8]>, u32, u32)> = None;
    let mut last_frame: Option<OwnedFrame> = None;
    let mut suspended = false;
    let mut was_paused = false;
    let mut stats = StreamStats::new();

    let suspend = |suspended: &mut bool, reason: String| {
        if !*suspended {
            *suspended = true;
            sink.on_event(CaptureEvent::Suspended(reason));
        }
    };

    while !control.stop.load(Ordering::Acquire) {
        if control.paused.load(Ordering::Acquire) {
            if !was_paused {
                source.pause();
                was_paused = true;
            }
            std::thread::sleep(Duration::from_millis(20));
            continue;
        }
        was_paused = false;

        match rect_fn() {
            Ok(Some(current)) => rect = Some(current),
            Ok(None) => {
                // Window gone (closed or moved off all workspaces): end the stream.
                sink.on_event(CaptureEvent::Closed);
                return;
            }
            // Hyprland busy: keep the last rectangle.
            Err(error) => {
                tracing::debug!(target: "cua_spacesd_client::linux_wayland", %error, "window geometry unavailable")
            }
        }
        let Some(current) = rect else {
            std::thread::sleep(Duration::from_millis(50));
            continue;
        };
        if let Err(error) = source.set_rect(current) {
            suspend(&mut suspended, error);
            std::thread::sleep(IDLE_TICK);
            continue;
        }

        let fps = control
            .max_fps
            .load(Ordering::Relaxed)
            .clamp(1, source.backend().max_fps());
        let interval = Duration::from_secs_f64(1.0 / f64::from(fps));
        let due = last_publish + interval;
        let give_up = Instant::now().max(due) + IDLE_TICK;
        let available = match source.wait(due, give_up) {
            Ok(available) => available,
            Err(error) if source.backend() == Backend::Screencopy => {
                tracing::warn!(target: "cua_spacesd_client::linux_wayland", %error, "Wayland screencopy failed; capturing with grim");
                match open(Backend::Grim) {
                    Ok(grim) => source = grim,
                    Err(error) => {
                        suspend(&mut suspended, error);
                        return;
                    }
                }
                continue;
            }
            Err(error) => {
                suspend(&mut suspended, error);
                std::thread::sleep(interval.max(Duration::from_millis(100)));
                continue;
            }
        };
        let keyframe = control.keyframe.swap(false, Ordering::AcqRel);
        let captured = if available { source.take() } else { None };
        stats.backend = source.backend();
        let Some((bgra, src_width, src_height)) = captured else {
            // Nothing changed: answer a keyframe request with the last frame.
            if keyframe {
                if let Some(frame) = last_frame.clone() {
                    sink.on_event(CaptureEvent::Frame(frame));
                }
            }
            continue;
        };
        if src_width == 0 || src_height == 0 {
            continue;
        }
        stats.captured += 1;
        if suspended {
            suspended = false;
            sink.on_event(CaptureEvent::Resumed);
        }
        let unchanged = last_source.as_ref().is_some_and(|(pixels, w, h)| {
            (*w, *h) == (src_width, src_height) && pixels[..] == bgra[..]
        });
        if unchanged {
            stats.unchanged += 1;
            if keyframe {
                if let Some(frame) = last_frame.clone() {
                    sink.on_event(CaptureEvent::Frame(frame));
                }
            }
            continue;
        }

        let (dst_width, dst_height) = fit_dimensions(src_width, src_height, max_dimension);
        let pixels: Arc<[u8]> = bgra.into();
        let frame: Arc<[u8]> = if (dst_width, dst_height) == (src_width, src_height) {
            pixels.clone()
        } else {
            downscale(&pixels, src_width, src_height, dst_width, dst_height).into()
        };
        last_source = Some((pixels, src_width, src_height));

        if last_dims != Some((dst_width, dst_height)) {
            native_geometry(src_width, src_height);
            sink.on_event(CaptureEvent::GeometryChanged(SurfaceGeometry {
                width_px: dst_width,
                height_px: dst_height,
                scale_factor: dst_width as f64 / src_width as f64,
            }));
            last_dims = Some((dst_width, dst_height));
        }

        let now = Instant::now();
        last_publish = now;
        let capture_timestamp_us = {
            let candidate = clock::to_media_us(now);
            last_timestamp_us = candidate.max(last_timestamp_us.saturating_add(1));
            last_timestamp_us
        };

        let owned = OwnedFrame {
            bytes: frame,
            format: PixelFormat::Bgra8,
            width_px: dst_width,
            height_px: dst_height,
            bytes_per_row: Some(dst_width * 4),
            capture_timestamp_us,
            encode_duration_us: None,
            codec_epoch: 1,
            keyframe: true,
        };
        if let Err(error) = owned.validate() {
            tracing::warn!(target: "cua_spacesd_client::linux_wayland", %error, "dropping malformed BGRA frame");
        } else {
            last_frame = Some(owned.clone());
            stats.sent += 1;
            sink.on_event(CaptureEvent::Frame(owned));
        }
    }
}

/// Per-stream counters, logged once when the stream ends: how many frames
/// the source delivered, how many were sent and how many were skipped as
/// unchanged, so a low frame rate can be placed (compositor or capture).
struct StreamStats {
    started: Instant,
    backend: Backend,
    captured: u64,
    sent: u64,
    unchanged: u64,
}

impl StreamStats {
    fn new() -> Self {
        Self {
            started: Instant::now(),
            backend: Backend::Screencopy,
            captured: 0,
            sent: 0,
            unchanged: 0,
        }
    }
}

impl Drop for StreamStats {
    fn drop(&mut self) {
        let seconds = self.started.elapsed().as_secs_f64();
        tracing::info!(
            target: "cua_spacesd_client::linux_wayland",
            backend = ?self.backend,
            seconds = format!("{seconds:.1}"),
            captured = self.captured,
            sent = self.sent,
            unchanged = self.unchanged,
            sent_fps = format!("{:.1}", self.sent as f64 / seconds.max(0.001)),
            "wayland capture ended"
        );
    }
}

/// [`Backend::Screencopy`]: one persistent screencopy session, reconnected
/// when the captured rectangle moves to another monitor.
#[derive(Default)]
struct ScreencopySource {
    session: Option<(String, super::linux_screencopy::Screencopy)>,
    monitors: Vec<MonitorRect>,
}

/// A monitor's name and global logical rectangle.
#[derive(Debug, Clone, PartialEq)]
struct MonitorRect {
    name: String,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

fn monitor_rects(monitors: &Value) -> Vec<MonitorRect> {
    displays_from_monitors(monitors)
        .into_iter()
        .map(|d| MonitorRect {
            name: d.id,
            x: d.bounds.0 as i32,
            y: d.bounds.1 as i32,
            width: d.bounds.2.round() as u32,
            height: d.bounds.3.round() as u32,
        })
        .collect()
}

/// The monitor holding the centre of `rect`, else the first one.
fn monitor_for(monitors: &[MonitorRect], rect: WinRect) -> Option<&MonitorRect> {
    let cx = rect.x + (rect.width / 2) as i32;
    let cy = rect.y + (rect.height / 2) as i32;
    monitors
        .iter()
        .find(|m| cx >= m.x && cx < m.x + m.width as i32 && cy >= m.y && cy < m.y + m.height as i32)
        .or_else(|| monitors.first())
}

impl FrameSource for ScreencopySource {
    fn backend(&self) -> Backend {
        Backend::Screencopy
    }

    fn set_rect(&mut self, rect: WinRect) -> Result<(), String> {
        if monitor_for(&self.monitors, rect).is_none_or(|m| {
            super::linux_screencopy::output_region(
                (rect.x, rect.y, rect.width, rect.height),
                (m.x, m.y),
                (m.width, m.height),
            )
            .is_none()
        }) {
            // First frame, or the rectangle left the known monitors.
            let monitors =
                hyprctl_json(&["monitors"]).ok_or("Hyprland did not answer `monitors`")?;
            self.monitors = monitor_rects(&monitors);
        }
        let monitor = monitor_for(&self.monitors, rect)
            .cloned()
            .ok_or("Hyprland reports no monitor")?;
        let region = super::linux_screencopy::output_region(
            (rect.x, rect.y, rect.width, rect.height),
            (monitor.x, monitor.y),
            (monitor.width, monitor.height),
        )
        .ok_or("the window is not on any monitor")?;
        if self.session.as_ref().map(|(name, _)| name) != Some(&monitor.name) {
            self.session = None;
            let session = super::linux_screencopy::Screencopy::connect(&monitor.name)?;
            self.session = Some((monitor.name.clone(), session));
        }
        if let Some((_, session)) = self.session.as_mut() {
            session.set_region(region);
        }
        Ok(())
    }

    fn wait(&mut self, due: Instant, give_up: Instant) -> Result<bool, String> {
        match self.session.as_mut() {
            Some((_, session)) => session.wait(due, give_up),
            None => Err("no screencopy session".into()),
        }
    }

    fn take(&mut self) -> Option<(Vec<u8>, u32, u32)> {
        let frame = self.session.as_mut()?.1.take()?;
        Some((frame.bgra, frame.width, frame.height))
    }

    fn pause(&mut self) {
        if let Some((_, session)) = self.session.as_mut() {
            session.abandon();
        }
    }
}

/// [`Backend::Grim`]: a `grim` process per frame.
#[derive(Default)]
struct GrimSource {
    rect: Option<WinRect>,
    frame: Option<(Vec<u8>, u32, u32)>,
}

impl FrameSource for GrimSource {
    fn backend(&self) -> Backend {
        Backend::Grim
    }

    fn set_rect(&mut self, rect: WinRect) -> Result<(), String> {
        self.rect = Some(rect);
        Ok(())
    }

    fn wait(&mut self, due: Instant, give_up: Instant) -> Result<bool, String> {
        let now = Instant::now();
        if due > give_up {
            std::thread::sleep(give_up.saturating_duration_since(now));
            return Ok(false);
        }
        std::thread::sleep(due.saturating_duration_since(now));
        let rect = self.rect.ok_or("no capture rectangle")?;
        let (rgb, width, height) = grab_region(&rect)?;
        self.frame = Some((rgb_to_bgra(&rgb, width, height), width, height));
        Ok(true)
    }

    fn take(&mut self) -> Option<(Vec<u8>, u32, u32)> {
        self.frame.take()
    }
}

/// Capture the output region covering `rect` as tightly-packed RGB via `grim`,
/// returning `(rgb, width_px, height_px)` at the output's real pixel size.
fn grab_region(rect: &WinRect) -> Result<(Vec<u8>, u32, u32), String> {
    let geometry = format!("{},{} {}x{}", rect.x, rect.y, rect.width, rect.height);
    let output = Command::new("grim")
        .args(["-t", "ppm", "-g", &geometry, "-"])
        .output()
        .map_err(|error| format!("grim spawn failed: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "grim exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    parse_ppm(&output.stdout).ok_or_else(|| "grim produced an unparseable PPM".to_string())
}

// ----------------------------------------------------------------------------
// Displays and one-shot capture
// ----------------------------------------------------------------------------

/// The session's monitors (`hyprctl monitors`) as displays: id and name are
/// the output name, bounds are logical (pixels / scale), the focused monitor
/// (else the first) is primary.
pub(super) fn displays() -> Vec<cua_spacesd_provider_api::ProviderDisplay> {
    hyprctl_json(&["monitors"])
        .map(|monitors| displays_from_monitors(&monitors))
        .unwrap_or_default()
}

fn displays_from_monitors(monitors: &Value) -> Vec<cua_spacesd_provider_api::ProviderDisplay> {
    let Some(array) = monitors.as_array() else {
        return Vec::new();
    };
    let any_focused = array
        .iter()
        .any(|m| m.get("focused").and_then(Value::as_bool) == Some(true));
    let mut out = Vec::new();
    for (index, m) in array.iter().enumerate() {
        let name = m.get("name").and_then(Value::as_str).unwrap_or_default();
        let width = m.get("width").and_then(Value::as_u64).unwrap_or(0) as u32;
        let height = m.get("height").and_then(Value::as_u64).unwrap_or(0) as u32;
        if name.is_empty() || width == 0 || height == 0 {
            continue;
        }
        let scale = m
            .get("scale")
            .and_then(Value::as_f64)
            .filter(|s| *s > 0.0)
            .unwrap_or(1.0);
        let x = m.get("x").and_then(Value::as_i64).unwrap_or(0) as f64;
        let y = m.get("y").and_then(Value::as_i64).unwrap_or(0) as f64;
        let primary = if any_focused {
            m.get("focused").and_then(Value::as_bool) == Some(true)
        } else {
            index == 0
        };
        out.push(cua_spacesd_provider_api::ProviderDisplay {
            id: name.to_string(),
            name: name.to_string(),
            primary,
            bounds: (x, y, f64::from(width) / scale, f64::from(height) / scale),
            native_width_px: width,
            native_height_px: height,
            scale_factor: scale,
            refresh_rate_hz: m
                .get("refreshRate")
                .and_then(Value::as_f64)
                .map(|r| r.round() as u32)
                .unwrap_or(60),
        });
    }
    out
}

/// The focused window's `(pid, window id)` (`hyprctl activewindow`), where
/// a desktop stream's keys go.
pub(super) fn focused_window() -> Option<(i64, u64)> {
    let active = hyprctl_json(&["activewindow"])?;
    let window_id = active
        .get("address")
        .and_then(Value::as_str)
        .and_then(addr_to_id)?;
    let pid = active
        .get("pid")
        .and_then(Value::as_i64)
        .filter(|pid| *pid > 0)?;
    Some((pid, window_id))
}

/// Hyprland's window-manager side for the gRPC backend: the active window and
/// exact geometry, through its IPC (not input; nothing is focused or raised).
pub(crate) struct HyprlandWindowManager;

impl crate::grpc::tool_backend::WindowManager for HyprlandWindowManager {
    fn focused_window_id(&self) -> Option<u64> {
        hyprctl_json(&["activewindow"])?
            .get("address")
            .and_then(Value::as_str)
            .and_then(addr_to_id)
    }

    fn window_action(
        &self,
        window_id: u64,
        action: &crate::grpc::backend::WindowAction,
    ) -> Option<Result<(), String>> {
        let commands =
            window_action_commands(&format!("address:{}", id_to_addr(window_id)), action)?;
        Some(
            commands
                .iter()
                .try_for_each(|(lua, legacy)| dispatch(lua, legacy)),
        )
    }

    fn set_bounds(
        &self,
        window_id: u64,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    ) -> Result<(), String> {
        let address = id_to_addr(window_id);
        let floating = hyprctl_json(&["clients"])
            .and_then(|clients| {
                clients.as_array()?.iter().find_map(|c| {
                    (c.get("address").and_then(Value::as_str) == Some(address.as_str()))
                        .then(|| c.get("floating").and_then(Value::as_bool).unwrap_or(false))
                })
            })
            .ok_or_else(|| "Hyprland reports no window at this address".to_string())?;
        for (lua, legacy) in bounds_commands(&address, floating, x, y, width, height) {
            dispatch(&lua, &legacy)?;
        }
        Ok(())
    }
}

/// Run one Hyprland dispatcher: the Lua form (Hyprland with a Lua config, as
/// Omarchy 4 ships), else the legacy `dispatch <name> <args>` form.
fn dispatch(lua: &str, legacy: &[String]) -> Result<(), String> {
    let run = |args: &[&str]| -> Result<String, String> {
        let output = Command::new("hyprctl")
            .arg("dispatch")
            .args(args)
            .output()
            .map_err(|error| format!("hyprctl: {error}"))?;
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    };
    let first = run(&[lua])?;
    if first == "ok" {
        return Ok(());
    }
    let legacy: Vec<&str> = legacy.iter().map(String::as_str).collect();
    let second = run(&legacy)?;
    if second == "ok" {
        Ok(())
    } else {
        Err(format!("hyprctl dispatch: {first}; legacy form: {second}"))
    }
}

/// Hyprland dispatchers (Lua form, legacy form) for a window action. Hyprland
/// has no minimized state, so Minimize is left to the caller (unsupported).
fn window_action_commands(
    target: &str,
    action: &crate::grpc::backend::WindowAction,
) -> Option<Vec<(String, Vec<String>)>> {
    use crate::grpc::backend::WindowAction;
    let focus = (
        format!("hl.dsp.focus({{ window = \"{target}\" }})"),
        vec!["focuswindow".to_string(), target.to_string()],
    );
    let maximized = |on: bool| {
        (
            format!(
                "hl.dsp.window.fullscreen({{ mode = \"maximized\", action = \"{}\", window = \"{target}\" }})",
                if on { "set" } else { "unset" }
            ),
            // Legacy fullscreenstate acts on the active window, focused first.
            vec!["fullscreenstate".to_string(), if on { "1 -1" } else { "0 -1" }.to_string()],
        )
    };
    Some(match action {
        WindowAction::Activate => vec![focus],
        WindowAction::Maximize => vec![focus, maximized(true)],
        WindowAction::Restore => vec![focus, maximized(false)],
        WindowAction::Close { force: false } => vec![(
            format!("hl.dsp.window.close({{ window = \"{target}\" }})"),
            vec!["closewindow".to_string(), target.to_string()],
        )],
        WindowAction::Close { force: true } => vec![(
            format!("hl.dsp.window.kill({{ window = \"{target}\" }})"),
            vec!["killwindow".to_string(), target.to_string()],
        )],
        WindowAction::Minimize => return None,
    })
}

/// The dispatchers (Lua form, legacy form) that give a window exact geometry,
/// in order: exact sizes need a floating window (a tiled one follows its
/// layout), and the size goes before the position.
fn bounds_commands(
    address: &str,
    floating: bool,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Vec<(String, Vec<String>)> {
    let target = format!("address:{address}");
    let (x, y) = (x.round() as i64, y.round() as i64);
    let (w, h) = (width.round() as i64, height.round() as i64);
    let mut out = Vec::new();
    if !floating {
        out.push((
            format!("hl.dsp.window.float({{ action = \"enable\", window = \"{target}\" }})"),
            vec!["setfloating".into(), target.clone()],
        ));
    }
    out.push((
        format!("hl.dsp.window.resize({{ x = {w}, y = {h}, window = \"{target}\" }})"),
        vec![
            "resizewindowpixel".into(),
            format!("exact {w} {h},{target}"),
        ],
    ));
    out.push((
        format!("hl.dsp.window.move({{ x = {x}, y = {y}, window = \"{target}\" }})"),
        vec!["movewindowpixel".into(), format!("exact {x} {y},{target}")],
    ));
    out
}

/// The session clipboard through wl-clipboard (`wl-paste`, `wl-copy`), which
/// Omarchy ships. Hyprland mirrors it to XWayland.
pub(crate) struct WlClipboard;

impl crate::grpc::tool_backend::TextClipboard for WlClipboard {
    fn get_text(&self) -> Result<Option<String>, String> {
        let output = Command::new("wl-paste")
            .args(["--no-newline", "--type", "text/plain;charset=utf-8"])
            .output()
            .map_err(|error| format!("wl-paste: {error}"))?;
        if output.status.success() {
            return Ok(Some(String::from_utf8_lossy(&output.stdout).into_owned()));
        }
        // wl-paste exits 1 with "No selection" / "No suitable type" when the
        // clipboard is empty or holds no text.
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("No selection") || stderr.contains("No suitable type") {
            Ok(None)
        } else {
            Err(format!("wl-paste: {}", stderr.trim()))
        }
    }

    fn set_text(&self, text: &str) -> Result<(), String> {
        use std::io::Write;
        use std::process::Stdio;
        let mut child = if text.is_empty() {
            Command::new("wl-copy")
                .arg("--clear")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
        } else {
            // wl-copy forks a server that owns the selection; keep its stdout
            // and stderr off our pipes so waiting on the parent returns.
            Command::new("wl-copy")
                .args(["--type", "text/plain;charset=utf-8"])
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
        }
        .map_err(|error| format!("wl-copy: {error}"))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(text.as_bytes())
                .map_err(|error| format!("wl-copy: {error}"))?;
        }
        let status = child.wait().map_err(|error| format!("wl-copy: {error}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("wl-copy exited {status}"))
        }
    }
}

// ----------------------------------------------------------------------------
// Background-safe resize (hyprctl)
// ----------------------------------------------------------------------------

/// Resize a window to the requested pixel size without raising, focusing, or
/// moving it, then read back the size Hyprland actually applied. Uses
/// `hyprctl dispatch resizewindowpixel exact`, which targets a window by
/// address and never touches stacking order or the active window.
pub(super) fn resize(
    window_id: u64,
    width_points: u32,
    height_points: u32,
) -> Result<AppliedWindowGeometry, ProviderError> {
    let address = id_to_addr(window_id);
    let width = clamp_dimension(width_points, MIN_WINDOW_WIDTH);
    let height = clamp_dimension(height_points, MIN_WINDOW_HEIGHT);

    // Resize only (no float, no move): background-safe, never raises or
    // focuses. Lua form first (Hyprland Lua configs), then the legacy one.
    let target = format!("address:{address}");
    dispatch(
        &format!("hl.dsp.window.resize({{ x = {width}, y = {height}, window = \"{target}\" }})"),
        &[
            "resizewindowpixel".to_string(),
            format!("exact {width} {height},{target}"),
        ],
    )
    .map_err(|error| ProviderError::new(ProviderErrorCode::CaptureFailed, error))?;

    let applied = window_rect(&address).ok_or_else(|| {
        ProviderError::new(
            ProviderErrorCode::TargetUnavailable,
            "window disappeared during resize",
        )
    })?;
    Ok(AppliedWindowGeometry {
        width_points: applied.width,
        height_points: applied.height,
    })
}

// ----------------------------------------------------------------------------
// Pixel helpers
// ----------------------------------------------------------------------------

/// Parse a binary PPM (`P6`) into `(rgb, width, height)`. Handles the standard
/// `grim` header shape (`P6\n<w> <h>\n<maxval>\n`) plus optional `#` comments
/// and arbitrary ASCII whitespace between the header fields.
fn parse_ppm(bytes: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    let rest = bytes.strip_prefix(b"P6")?;
    let mut cursor = rest;
    let width = next_ppm_uint(&mut cursor)?;
    let height = next_ppm_uint(&mut cursor)?;
    let maxval = next_ppm_uint(&mut cursor)?;
    // Only 8-bit channels are handled (grim emits maxval 255).
    if maxval == 0 || maxval > 255 {
        return None;
    }
    // Exactly one whitespace byte separates the header from the raster data.
    let (_, data) = cursor.split_first()?;
    let pixels = (width as usize)
        .checked_mul(height as usize)?
        .checked_mul(3)?;
    if data.len() < pixels {
        return None;
    }
    Some((data[..pixels].to_vec(), width, height))
}

/// Read the next unsigned integer from a PPM header, skipping leading ASCII
/// whitespace and whole `#` comment lines. Advances `cursor` past the number.
fn next_ppm_uint(cursor: &mut &[u8]) -> Option<u32> {
    loop {
        // Skip whitespace.
        while let Some((&b, tail)) = cursor.split_first() {
            if b.is_ascii_whitespace() {
                *cursor = tail;
            } else {
                break;
            }
        }
        // Skip a comment line, then re-check for whitespace/comments.
        if let Some((&b'#', tail)) = cursor.split_first() {
            *cursor = tail;
            while let Some((&b, tail)) = cursor.split_first() {
                *cursor = tail;
                if b == b'\n' {
                    break;
                }
            }
            continue;
        }
        break;
    }
    let mut value: u32 = 0;
    let mut seen = false;
    while let Some((&b, tail)) = cursor.split_first() {
        if b.is_ascii_digit() {
            value = value.checked_mul(10)?.checked_add(u32::from(b - b'0'))?;
            seen = true;
            *cursor = tail;
        } else {
            break;
        }
    }
    seen.then_some(value)
}

/// Expand tightly-packed RGB into tightly-packed BGRA (opaque alpha): the RCDP
/// `Bgra8` frame format the session encoder consumes.
fn rgb_to_bgra(rgb: &[u8], width: u32, height: u32) -> Vec<u8> {
    let pixels = (width as usize) * (height as usize);
    let mut out = Vec::with_capacity(pixels * 4);
    for chunk in rgb.as_chunks::<3>().0.iter().take(pixels) {
        out.push(chunk[2]); // B
        out.push(chunk[1]); // G
        out.push(chunk[0]); // R
        out.push(0xff); // A
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_enumeration_is_an_error_not_an_empty_desktop() {
        let mut calls = 0;
        let result = enumerate_windows_with(
            || {
                calls += 1;
                None
            },
            false,
        );
        assert!(result.is_err(), "{result:?}");
        assert_eq!(calls, CLIENTS_ATTEMPTS);
        // A non-array reply (an IPC error string) is a failure too.
        assert!(enumerate_windows_with(|| Some(serde_json::json!("timeout")), false).is_err());
    }

    #[test]
    fn enumeration_retries_a_transient_ipc_failure() {
        let mut calls = 0;
        let windows = enumerate_windows_with(
            || {
                calls += 1;
                (calls == 2).then(|| serde_json::json!([]))
            },
            false,
        )
        .expect("second attempt answers");
        assert!(windows.is_empty());
        assert_eq!(calls, 2);
    }
    use serde_json::json;

    #[test]
    fn address_round_trips_through_window_id() {
        let id = addr_to_id("0x55f0abcd1234").unwrap();
        assert_eq!(id_to_addr(id), "0x55f0abcd1234");
        assert_eq!(addr_to_id("deadbeef"), Some(0xdead_beef));
        assert_eq!(addr_to_id("0xnothex"), None);
    }

    #[test]
    fn maps_hyprctl_clients_onto_windows() {
        let clients = json!([
            {
                "address": "0x1", "mapped": true, "hidden": false,
                "at": [10, 20], "size": [800, 600],
                "class": "omarchy-terminal", "title": "shell", "pid": 718, "xwayland": false
            },
            // Skipped: hidden (on another workspace) under on_screen_only.
            { "address": "0x2", "mapped": true, "hidden": true, "at": [0, 0], "size": [1, 1],
              "class": "x", "title": "bg", "pid": 9 },
            // Skipped: empty title (decoration-only helper).
            { "address": "0x3", "mapped": true, "hidden": false, "at": [0, 0], "size": [5, 5],
              "class": "x", "title": "", "pid": 10 }
        ]);

        let visible = windows_from_clients(&clients, true);
        assert_eq!(visible.len(), 1);
        let w = &visible[0];
        assert_eq!(w.native.window_id, 1);
        assert_eq!(w.native.pid, 718);
        assert_eq!(w.app_name, "omarchy-terminal");
        assert_eq!(w.title, "shell");
        assert_eq!(w.geometry.width_px, 800);
        assert_eq!(w.geometry.height_px, 600);
        assert!(w.visible);

        // Without on_screen_only the hidden window returns too (but not the
        // empty-title one).
        assert_eq!(windows_from_clients(&clients, false).len(), 2);
    }

    #[test]
    fn extracts_rect_by_address() {
        let clients = json!([
            { "address": "0xaa", "at": [3, 4], "size": [12, 34] },
            { "address": "0xbb", "at": [0, 0], "size": [1, 1] }
        ]);
        assert_eq!(
            rect_from_clients(&clients, "0xaa"),
            Some(WinRect {
                x: 3,
                y: 4,
                width: 12,
                height: 34
            })
        );
        assert_eq!(rect_from_clients(&clients, "0xzz"), None);
    }

    #[test]
    fn bounds_commands_float_tiled_windows_then_size_then_position() {
        let c = bounds_commands("0x1", false, 10.0, 20.4, 300.0, 200.6);
        assert_eq!(c.len(), 3);
        assert_eq!(
            c[0].0,
            r#"hl.dsp.window.float({ action = "enable", window = "address:0x1" })"#
        );
        assert_eq!(
            c[1].0,
            r#"hl.dsp.window.resize({ x = 300, y = 201, window = "address:0x1" })"#
        );
        assert_eq!(
            c[1].1,
            vec![
                "resizewindowpixel".to_string(),
                "exact 300 201,address:0x1".into()
            ]
        );
        assert_eq!(
            c[2].0,
            r#"hl.dsp.window.move({ x = 10, y = 20, window = "address:0x1" })"#
        );
        assert_eq!(bounds_commands("0x1", true, 0.0, 0.0, 1.0, 1.0).len(), 2);
    }

    #[test]
    fn window_actions_map_to_hyprland_dispatchers() {
        use crate::grpc::backend::WindowAction;
        let max = window_action_commands("address:0x1", &WindowAction::Maximize).unwrap();
        assert_eq!(
            max[0].1,
            vec!["focuswindow".to_string(), "address:0x1".into()]
        );
        assert!(max[1].0.contains(r#"mode = "maximized", action = "set""#));
        let restore = window_action_commands("address:0x1", &WindowAction::Restore).unwrap();
        assert!(restore[1].0.contains(r#"action = "unset""#));
        let close =
            window_action_commands("address:0x1", &WindowAction::Close { force: false }).unwrap();
        assert_eq!(close[0].1[0], "closewindow");
        assert!(window_action_commands("address:0x1", &WindowAction::Minimize).is_none());
    }

    #[test]
    fn maps_hyprctl_monitors_onto_displays() {
        let monitors = serde_json::json!([
            {"name": "Virtual-1", "width": 1280, "height": 800, "x": 0, "y": 0,
             "scale": 1.0, "refreshRate": 60.0, "focused": false},
            {"name": "HDMI-A-1", "width": 3840, "height": 2160, "x": 1280, "y": 0,
             "scale": 2.0, "refreshRate": 59.94, "focused": true},
            {"name": "", "width": 10, "height": 10}
        ]);
        let d = displays_from_monitors(&monitors);
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].id, "Virtual-1");
        assert!(!d[0].primary && d[1].primary);
        assert_eq!(d[1].bounds, (1280.0, 0.0, 1920.0, 1080.0));
        assert_eq!(
            (
                d[1].native_width_px,
                d[1].scale_factor,
                d[1].refresh_rate_hz
            ),
            (3840, 2.0, 60)
        );
        let lone = displays_from_monitors(
            &serde_json::json!([{"name": "A", "width": 800, "height": 600}]),
        );
        assert!(lone[0].primary);
    }

    #[test]
    fn parses_grim_ppm() {
        // "P6\n2 1\n255\n" + two RGB pixels (red, green).
        let mut ppm = b"P6\n2 1\n255\n".to_vec();
        ppm.extend_from_slice(&[255, 0, 0, 0, 255, 0]);
        let (rgb, w, h) = parse_ppm(&ppm).unwrap();
        assert_eq!((w, h), (2, 1));
        assert_eq!(rgb, vec![255, 0, 0, 0, 255, 0]);

        let bgra = rgb_to_bgra(&rgb, w, h);
        // Red pixel -> B,G,R,A = 0,0,255,255 ; green -> 0,255,0,255.
        assert_eq!(bgra, vec![0, 0, 255, 255, 0, 255, 0, 255]);
    }

    #[test]
    fn ppm_parser_skips_comments_and_extra_whitespace() {
        let mut ppm = b"P6\n# grim\n 1  1 \n255\n".to_vec();
        ppm.extend_from_slice(&[9, 8, 7]);
        let (rgb, w, h) = parse_ppm(&ppm).unwrap();
        assert_eq!((w, h), (1, 1));
        assert_eq!(rgb, vec![9, 8, 7]);
    }

    #[test]
    fn ppm_parser_rejects_truncated_data() {
        let ppm = b"P6\n4 4\n255\n\x00\x01"; // header promises 48 bytes, has 2
        assert!(parse_ppm(ppm).is_none());
    }

    // ------------------------------------------------------------------
    // Capture loop, with scripted pixel sources.
    // ------------------------------------------------------------------

    use std::collections::VecDeque;

    #[derive(Default)]
    struct Events(Mutex<Vec<CaptureEvent>>);

    impl CaptureSink for Events {
        fn on_event(&self, event: CaptureEvent) {
            self.0.lock().unwrap().push(event);
        }
    }

    impl Events {
        fn frames(&self) -> Vec<Vec<u8>> {
            self.0
                .lock()
                .unwrap()
                .iter()
                .filter_map(|e| match e {
                    CaptureEvent::Frame(f) => Some(f.bytes.to_vec()),
                    _ => None,
                })
                .collect()
        }

        fn count(&self, pred: impl Fn(&CaptureEvent) -> bool) -> usize {
            self.0.lock().unwrap().iter().filter(|e| pred(e)).count()
        }
    }

    /// What each `wait` of a scripted source yields, in order.
    type Script = Arc<Mutex<VecDeque<Result<Option<u8>, String>>>>;

    /// Scripted source: each `wait` yields the next scripted result.
    struct Scripted {
        backend: Backend,
        script: Script,
        frame: Option<(Vec<u8>, u32, u32)>,
        rects: Arc<Mutex<Vec<WinRect>>>,
        pauses: Arc<AtomicU16>,
    }

    impl FrameSource for Scripted {
        fn backend(&self) -> Backend {
            self.backend
        }
        fn set_rect(&mut self, rect: WinRect) -> Result<(), String> {
            self.rects.lock().unwrap().push(rect);
            Ok(())
        }
        fn wait(&mut self, _due: Instant, _give_up: Instant) -> Result<bool, String> {
            std::thread::sleep(Duration::from_millis(1));
            match self.script.lock().unwrap().pop_front() {
                Some(Ok(Some(value))) => {
                    self.frame = Some((vec![value, value, value, 255], 1, 1));
                    Ok(true)
                }
                Some(Ok(None)) | None => Ok(false),
                Some(Err(error)) => Err(error),
            }
        }
        fn take(&mut self) -> Option<(Vec<u8>, u32, u32)> {
            self.frame.take()
        }
        fn pause(&mut self) {
            self.pauses.fetch_add(1, Ordering::Relaxed);
        }
    }

    struct Harness {
        events: Arc<Events>,
        control: Arc<Control>,
        opened: Arc<Mutex<Vec<Backend>>>,
        rects: Arc<Mutex<Vec<WinRect>>>,
        pauses: Arc<AtomicU16>,
    }

    const RECT: WinRect = WinRect {
        x: 10,
        y: 20,
        width: 1,
        height: 1,
    };

    /// Run the loop until `done` holds (or 5 s pass), with per-backend
    /// scripts; `screencopy` = None means it cannot be opened.
    fn run(
        screencopy: Option<Vec<Result<Option<u8>, String>>>,
        grim: Vec<Result<Option<u8>, String>>,
        rect_fn: RectFn,
        between: impl Fn(&Harness),
        done: impl Fn(&Harness) -> bool,
    ) -> Harness {
        let harness = Harness {
            events: Arc::default(),
            control: Arc::new(Control {
                stop: AtomicBool::new(false),
                paused: AtomicBool::new(false),
                keyframe: AtomicBool::new(false),
                max_fps: AtomicU16::new(30),
            }),
            opened: Arc::default(),
            rects: Arc::default(),
            pauses: Arc::default(),
        };
        let screencopy = screencopy.map(|s| Arc::new(Mutex::new(VecDeque::from(s))));
        let grim = Arc::new(Mutex::new(VecDeque::from(grim)));
        let (opened, rects, pauses) = (
            harness.opened.clone(),
            harness.rects.clone(),
            harness.pauses.clone(),
        );
        let open: OpenFn = Box::new(move |backend| {
            opened.lock().unwrap().push(backend);
            let script = match backend {
                Backend::Screencopy => screencopy.clone().ok_or("no screencopy")?,
                Backend::Grim => grim.clone(),
            };
            Ok(Box::new(Scripted {
                backend,
                script,
                frame: None,
                rects: rects.clone(),
                pauses: pauses.clone(),
            }) as Box<dyn FrameSource>)
        });
        let (control, events) = (harness.control.clone(), harness.events.clone());
        let thread = std::thread::spawn(move || {
            capture_loop(
                rect_fn,
                open,
                Backend::Screencopy,
                &control,
                u32::MAX,
                events,
                Arc::new(|_, _| {}),
            )
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done(&harness) && !thread.is_finished() && Instant::now() < deadline {
            between(&harness);
            std::thread::sleep(Duration::from_millis(2));
        }
        harness.control.stop.store(true, Ordering::Release);
        thread.join().unwrap();
        harness
    }

    fn fixed_rect() -> RectFn {
        Box::new(|| Ok(Some(RECT)))
    }

    #[test]
    fn unchanged_frames_are_skipped() {
        let h = run(
            Some(vec![
                Ok(Some(1)),
                Ok(Some(1)),
                Ok(Some(1)),
                Ok(Some(2)),
                Ok(Some(2)),
            ]),
            vec![],
            fixed_rect(),
            |_| {},
            |h| h.events.frames().len() >= 2,
        );
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(
            h.events.frames(),
            vec![vec![1, 1, 1, 255], vec![2, 2, 2, 255]]
        );
        assert_eq!(*h.opened.lock().unwrap(), vec![Backend::Screencopy]);
        assert!(h.rects.lock().unwrap().iter().all(|r| *r == RECT));
        // One geometry event for the one frame size.
        assert_eq!(
            h.events
                .count(|e| matches!(e, CaptureEvent::GeometryChanged(_))),
            1
        );
    }

    #[test]
    fn a_keyframe_request_resends_the_last_frame_while_idle() {
        let h = run(
            Some(vec![Ok(Some(7))]),
            vec![],
            fixed_rect(),
            |h| {
                if h.events.frames().len() == 1 {
                    h.control.keyframe.store(true, Ordering::Release);
                }
            },
            |h| h.events.frames().len() >= 2,
        );
        let frames = h.events.frames();
        assert!(frames.len() >= 2, "{frames:?}");
        assert!(frames.iter().all(|f| *f == vec![7, 7, 7, 255]));
    }

    #[test]
    fn screencopy_unavailable_falls_back_to_grim() {
        let h = run(
            None,
            vec![Ok(Some(3))],
            fixed_rect(),
            |_| {},
            |h| !h.events.frames().is_empty(),
        );
        assert_eq!(
            *h.opened.lock().unwrap(),
            vec![Backend::Screencopy, Backend::Grim]
        );
        assert_eq!(h.events.frames(), vec![vec![3, 3, 3, 255]]);
    }

    #[test]
    fn screencopy_failure_mid_stream_switches_to_grim() {
        let h = run(
            Some(vec![Ok(Some(1)), Err("compositor failed".into())]),
            vec![Ok(Some(2))],
            fixed_rect(),
            |_| {},
            |h| h.events.frames().len() >= 2,
        );
        assert_eq!(
            *h.opened.lock().unwrap(),
            vec![Backend::Screencopy, Backend::Grim]
        );
        assert_eq!(
            h.events.frames(),
            vec![vec![1, 1, 1, 255], vec![2, 2, 2, 255]]
        );
    }

    #[test]
    fn a_closed_window_ends_the_stream_but_an_ipc_hiccup_does_not() {
        let calls = Arc::new(AtomicU16::new(0));
        let counter = calls.clone();
        let rect: RectFn = Box::new(move || match counter.fetch_add(1, Ordering::Relaxed) {
            0 => Ok(Some(RECT)),
            1 => Err("busy".into()),
            _ => Ok(None),
        });
        let h = run(
            Some(vec![Ok(Some(1)), Ok(Some(2))]),
            vec![],
            rect,
            |_| {},
            |h| h.events.count(|e| matches!(e, CaptureEvent::Closed)) > 0,
        );
        assert_eq!(h.events.count(|e| matches!(e, CaptureEvent::Closed)), 1);
        // The frame after the IPC hiccup still used the last rectangle.
        assert_eq!(
            h.events.frames(),
            vec![vec![1, 1, 1, 255], vec![2, 2, 2, 255]]
        );
    }

    #[test]
    fn pausing_stops_the_source_once() {
        let h = run(
            Some(vec![Ok(Some(1))]),
            vec![],
            fixed_rect(),
            |h| {
                if h.events.frames().len() == 1 {
                    h.control.paused.store(true, Ordering::Release);
                }
            },
            |h| h.pauses.load(Ordering::Relaxed) > 0,
        );
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(h.pauses.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn grim_keeps_the_lower_frame_rate_cap() {
        assert_eq!(Backend::Screencopy.max_fps(), 30);
        assert_eq!(Backend::Grim.max_fps(), 10);
    }

    #[test]
    fn monitor_is_picked_by_the_rectangle_centre() {
        let monitors = monitor_rects(&serde_json::json!([
            {"name": "DP-1", "width": 1280, "height": 800, "scale": 1.0, "x": 0, "y": 0},
            {"name": "DP-2", "width": 3840, "height": 2160, "scale": 2.0, "x": 1280, "y": 0}
        ]));
        assert_eq!(monitors[1].width, 1920);
        let on_second = WinRect {
            x: 1200,
            y: 10,
            width: 400,
            height: 100,
        };
        assert_eq!(monitor_for(&monitors, on_second).unwrap().name, "DP-2");
        let off = WinRect {
            x: -5000,
            y: 0,
            width: 10,
            height: 10,
        };
        assert_eq!(monitor_for(&monitors, off).unwrap().name, "DP-1");
    }
}
