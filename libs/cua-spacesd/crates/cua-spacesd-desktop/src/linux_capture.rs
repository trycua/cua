// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Pure-Rust X11 capture backend for the RCDP CUA adapter.
//!
//! This backend targets a plain X11 server (for example TigerVNC's `Xvnc` on
//! `DISPLAY=:1`). It enumerates real top-level application windows through the
//! EWMH `_NET_CLIENT_LIST(_STACKING)` property and streams each window as
//! top-down, tightly packed BGRA whole frames — the RCDP `VideoCodec::Bgra`
//! path — so no H.264 encoder is required on Linux.
//!
//! Per-window pixels are read with `XGetImage` (`ZPixmap`). When the
//! `Composite` extension is available the window is redirect-backed and the
//! image is read from its off-screen pixmap, so occluded or partially covered
//! windows still capture their full backing store. When Composite is
//! unavailable the backend falls back to reading the window drawable directly,
//! in which case occluded regions are server-defined (usually blank).
//!
//! Everything here is `#[cfg(target_os = "linux")]`; capture is self-contained
//! on `x11rb` (pure-Rust XCB). Input is not injected here: actions are handed
//! to cua-driver (`platform_linux::input::targeted`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cua_media_protocol::SurfaceGeometry;
use cua_spacesd_provider_api::{
    ActionOutcome, AppliedWindowGeometry, CaptureConfig, CaptureEvent, CaptureLease, CaptureSink,
    OwnedFrame, PixelFormat, ProviderError, ProviderErrorCode,
};
use serde_json::Value;
use x11rb::connection::Connection;
use x11rb::protocol::composite::{ConnectionExt as _, Redirect};
use x11rb::protocol::xproto::*;
use x11rb::rust_connection::RustConnection;

use super::NativeTarget;

/// Hard ceiling on the streamed frame rate. The client's requested `max_fps`
/// is honored but never exceeds this, keeping a soft-real-time X11 read loop
/// from saturating the guest.
const MAX_CAPTURE_FPS: u16 = 15;

// ----------------------------------------------------------------------------
// Per-window capture
// ----------------------------------------------------------------------------

struct LinuxCaptureLease {
    stop: Arc<AtomicBool>,
    monitor: Mutex<Option<JoinHandle<()>>>,
}

impl CaptureLease for LinuxCaptureLease {
    fn stop(&self) {
        if self.stop.swap(true, Ordering::AcqRel) {
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

impl Drop for LinuxCaptureLease {
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
    let window = u32::try_from(target.window_id).map_err(|_| {
        ProviderError::new(
            ProviderErrorCode::TargetUnavailable,
            "native window identifier is outside the X11 range",
        )
    })?;

    // Validate the target synchronously so a missing window reports an error to
    // the session instead of silently closing an already-opened stream.
    let (conn, _screen) = RustConnection::connect(None).map_err(connect_error)?;
    conn.get_geometry(window)
        .map_err(|_| target_gone())?
        .reply()
        .map_err(|_| target_gone())?;
    drop(conn);

    let fps = config.max_fps.clamp(1, MAX_CAPTURE_FPS);
    let frame_interval = Duration::from_millis(1000 / u64::from(fps));
    let max_dimension = config.max_dimension.max(1);

    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    let handle = std::thread::Builder::new()
        .name("rcdp-linux-window-capture".into())
        .spawn(move || {
            capture_loop(
                window,
                frame_interval,
                max_dimension,
                sink,
                native_geometry,
                thread_stop,
            );
        })
        .map_err(|error| {
            ProviderError::new(
                ProviderErrorCode::CaptureFailed,
                format!("could not start X11 capture thread: {error}"),
            )
        })?;

    Ok(Arc::new(LinuxCaptureLease {
        stop,
        monitor: Mutex::new(Some(handle)),
    }))
}

fn capture_loop(
    window: Window,
    frame_interval: Duration,
    max_dimension: u32,
    sink: Arc<dyn CaptureSink>,
    native_geometry: Arc<dyn Fn(u32, u32) + Send + Sync>,
    stop: Arc<AtomicBool>,
) {
    let conn = match RustConnection::connect(None) {
        Ok((conn, _)) => conn,
        Err(error) => {
            sink.on_event(CaptureEvent::Suspended(format!(
                "X11 capture connection failed: {error}"
            )));
            return;
        }
    };

    // Best-effort Composite redirect so occluded windows still capture their
    // full backing store. Released automatically when this connection drops.
    let composite = setup_composite(&conn, window);
    if !composite {
        tracing::debug!(
            target: "cua_spacesd_client::linux_capture",
            window,
            "Composite unavailable; capturing the window drawable directly (occluded regions may be blank)"
        );
    }

    let origin = Instant::now();
    let mut last_timestamp_us = 0u64;
    let mut last_dims: Option<(u32, u32)> = None;

    while !stop.load(Ordering::Acquire) {
        let loop_start = Instant::now();

        let geometry = match conn.get_geometry(window).ok().and_then(|c| c.reply().ok()) {
            Some(geometry) => geometry,
            None => {
                sink.on_event(CaptureEvent::Closed);
                break;
            }
        };
        let src_width = u32::from(geometry.width);
        let src_height = u32::from(geometry.height);
        if src_width == 0 || src_height == 0 {
            sleep_remaining(loop_start, frame_interval, &stop);
            continue;
        }

        let Some((mut pixels, source_stride)) = grab_bgra(&conn, window, composite, geometry)
        else {
            sleep_remaining(loop_start, frame_interval, &stop);
            continue;
        };

        let (dst_width, dst_height) = fit_dimensions(src_width, src_height, max_dimension);
        let frame = if (dst_width, dst_height) == (src_width, src_height) {
            repack_tight(&pixels, source_stride, src_width, src_height)
        } else {
            let tight = repack_tight(&pixels, source_stride, src_width, src_height);
            downscale(&tight, src_width, src_height, dst_width, dst_height)
        };
        pixels.clear();

        if last_dims != Some((dst_width, dst_height)) {
            native_geometry(src_width, src_height);
            sink.on_event(CaptureEvent::GeometryChanged(SurfaceGeometry {
                width_px: dst_width,
                height_px: dst_height,
                scale_factor: dst_width as f64 / src_width as f64,
            }));
            last_dims = Some((dst_width, dst_height));
        }

        let capture_timestamp_us = {
            let candidate = origin.elapsed().as_micros().min(u128::from(u64::MAX)) as u64;
            last_timestamp_us = candidate.max(last_timestamp_us.saturating_add(1));
            last_timestamp_us
        };

        let owned = OwnedFrame {
            bytes: frame.into(),
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
            tracing::warn!(target: "cua_spacesd_client::linux_capture", %error, "dropping malformed BGRA frame");
        } else {
            sink.on_event(CaptureEvent::Frame(owned));
        }

        sleep_remaining(loop_start, frame_interval, &stop);
    }
}

/// Enable the Composite extension and redirect the target window so its full
/// backing store can be read even when occluded. Returns `false` when the
/// extension is missing or the redirect was refused.
fn setup_composite(conn: &RustConnection, window: Window) -> bool {
    if conn
        .composite_query_version(0, 4)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .is_none()
    {
        return false;
    }
    conn.composite_redirect_window(window, Redirect::AUTOMATIC)
        .ok()
        .and_then(|cookie| cookie.check().ok())
        .is_some()
}

/// Read the window (or its Composite backing pixmap) as raw `ZPixmap` bytes.
/// Returns the pixel buffer plus its source row stride in bytes.
fn grab_bgra(
    conn: &RustConnection,
    window: Window,
    composite: bool,
    geometry: GetGeometryReply,
) -> Option<(Vec<u8>, usize)> {
    let width = geometry.width;
    let height = geometry.height;

    if composite {
        if let Some(result) = grab_via_pixmap(conn, window, width, height) {
            return Some(result);
        }
    }

    let image = conn
        .get_image(ImageFormat::Z_PIXMAP, window, 0, 0, width, height, !0u32)
        .ok()?
        .reply()
        .ok()?;
    finish_image(image.data, u32::from(width), u32::from(height))
}

fn grab_via_pixmap(
    conn: &RustConnection,
    window: Window,
    width: u16,
    height: u16,
) -> Option<(Vec<u8>, usize)> {
    let pixmap = conn.generate_id().ok()?;
    // Naming a fresh pixmap every frame tracks window resizes: the previous
    // name goes stale as soon as the window's dimensions change.
    if conn
        .composite_name_window_pixmap(window, pixmap)
        .ok()?
        .check()
        .is_err()
    {
        return None;
    }
    let image = conn
        .get_image(ImageFormat::Z_PIXMAP, pixmap, 0, 0, width, height, !0u32)
        .ok()
        .and_then(|cookie| cookie.reply().ok());
    let _ = conn.free_pixmap(pixmap);
    let image = image?;
    finish_image(image.data, u32::from(width), u32::from(height))
}

fn finish_image(mut data: Vec<u8>, width: u32, height: u32) -> Option<(Vec<u8>, usize)> {
    if height == 0 || data.is_empty() {
        return None;
    }
    let stride = data.len() / height as usize;
    if stride < (width as usize) * 4 {
        return None;
    }
    // X `ZPixmap` at depth 24/32 is stored little-endian as B, G, R, X — which
    // is already RCDP's BGRA byte order. Force the fourth byte opaque so
    // depth-24 windows (whose X byte is undefined) are not streamed as
    // transparent.
    for pixel in data.as_chunks_mut::<4>().0 {
        pixel[3] = 0xff;
    }
    Some((data, stride))
}

/// Copy a padded source buffer into a tightly packed `width*4` BGRA buffer.
fn repack_tight(data: &[u8], source_stride: usize, width: u32, height: u32) -> Vec<u8> {
    let row_bytes = width as usize * 4;
    if source_stride == row_bytes && data.len() == row_bytes * height as usize {
        return data.to_vec();
    }
    let mut packed = Vec::with_capacity(row_bytes * height as usize);
    for row in 0..height as usize {
        let start = row * source_stride;
        let end = start + row_bytes;
        if end <= data.len() {
            packed.extend_from_slice(&data[start..end]);
        } else {
            packed.resize(row_bytes * (row + 1), 0);
        }
    }
    packed
}

/// Nearest-neighbor downscale of a tightly packed BGRA image.
pub(super) fn downscale(src: &[u8], src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> Vec<u8> {
    let mut out = vec![0u8; dst_w as usize * dst_h as usize * 4];
    for y in 0..dst_h {
        let src_y = (y as u64 * src_h as u64 / dst_h as u64).min(src_h as u64 - 1) as usize;
        let src_row = src_y * src_w as usize * 4;
        let dst_row = y as usize * dst_w as usize * 4;
        for x in 0..dst_w {
            let src_x = (x as u64 * src_w as u64 / dst_w as u64).min(src_w as u64 - 1) as usize;
            let s = src_row + src_x * 4;
            let d = dst_row + x as usize * 4;
            out[d..d + 4].copy_from_slice(&src[s..s + 4]);
        }
    }
    out
}

/// Fit `width`x`height` within `max_dimension` on the longest edge, preserving
/// aspect ratio. Mirrors the macOS backend so both platforms cap identically.
pub(super) fn fit_dimensions(width: u32, height: u32, max_dimension: u32) -> (u32, u32) {
    let width = width.max(1);
    let height = height.max(1);
    let longest = width.max(height);
    if longest <= max_dimension {
        return (width, height);
    }
    let ratio = max_dimension as f64 / longest as f64;
    (
        ((width as f64 * ratio).round() as u32).max(1),
        ((height as f64 * ratio).round() as u32).max(1),
    )
}

pub(super) fn sleep_remaining(loop_start: Instant, frame_interval: Duration, stop: &AtomicBool) {
    let elapsed = loop_start.elapsed();
    if let Some(remaining) = frame_interval.checked_sub(elapsed) {
        // Wake promptly on stop instead of sleeping the whole interval.
        let deadline = Instant::now() + remaining;
        while Instant::now() < deadline {
            if stop.load(Ordering::Acquire) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5).min(remaining));
        }
    }
}

fn connect_error(error: impl std::fmt::Display) -> ProviderError {
    ProviderError::new(
        ProviderErrorCode::Unsupported,
        format!("could not connect to the X11 display (is DISPLAY set?): {error}"),
    )
}

fn target_gone() -> ProviderError {
    ProviderError::new(
        ProviderErrorCode::TargetUnavailable,
        "target window is no longer available",
    )
}

fn x_error(error: impl std::fmt::Display) -> ProviderError {
    ProviderError::new(
        ProviderErrorCode::DeliveryFailed,
        format!("X11 window geometry request failed: {error}"),
    )
}

// ----------------------------------------------------------------------------
// Background-safe window geometry (resize)
// ----------------------------------------------------------------------------

/// Smallest width and height, in pixels, this provider will ask the server to
/// apply. A degenerate request (0 or a stray tiny value) is clamped up to a
/// usable window rather than collapsing the guest window.
pub(super) const MIN_WINDOW_WIDTH: u32 = 100;
pub(super) const MIN_WINDOW_HEIGHT: u32 = 60;

/// Clamp a requested dimension to at least `min`, keeping the applied window
/// usable. Pure helper split out so it can be unit tested without an X server.
pub(super) fn clamp_dimension(value: u32, min: u32) -> u32 {
    value.max(min)
}

/// Background-safe host-window resize for the X11 provider.
///
/// Issues a single `ConfigureWindow` request that sets only the target
/// window's width and height, then reads back the geometry the server actually
/// applied. It never touches stacking order, focus, or map state, so a streamed
/// guest window is resized without being raised or activated. X11 geometry is
/// already expressed in device pixels, which equal points for the scale-1
/// guests this provider serves, so the requested point dimensions are used
/// directly as pixels.
pub(super) fn resize(
    window_id: u64,
    width_points: u32,
    height_points: u32,
) -> Result<AppliedWindowGeometry, ProviderError> {
    let win = u32::try_from(window_id).map_err(|_| target_gone())?;
    let width = clamp_dimension(width_points, MIN_WINDOW_WIDTH);
    let height = clamp_dimension(height_points, MIN_WINDOW_HEIGHT);

    let (conn, _screen) = RustConnection::connect(None).map_err(connect_error)?;
    conn.configure_window(win, &ConfigureWindowAux::new().width(width).height(height))
        .map_err(x_error)?;
    conn.flush().map_err(x_error)?;
    let geometry = conn
        .get_geometry(win)
        .map_err(x_error)?
        .reply()
        .map_err(x_error)?;

    Ok(AppliedWindowGeometry {
        width_points: u32::from(geometry.width),
        height_points: u32::from(geometry.height),
    })
}

// ----------------------------------------------------------------------------
// Input delivery (delegated to cua-driver)
// ----------------------------------------------------------------------------
//
// Actions the client sends over the RCDP `action` channel are delivered by
// cua-driver's targeted input (`platform_linux::input::targeted`). This
// adapter only maps the action vocabulary, resolves window-local pixels to
// root coordinates, and picks the delivery the session policy grants:
// ALLOW_ACTIVATION -> foreground (activate, confirm the point is not covered,
// XTest), BACKGROUND_ONLY -> background (XSendEvent, refused for toolkits that
// drop synthetic input).
//
// Pixel coordinates arrive already scaled by the adapter from the streamed
// video frame's pixel space into the target window's local pixel space.

/// Steps a drag is split into so drag-aware surfaces see a continuous gesture.
const DRAG_STEPS: i32 = 10;

/// Deliver one RCDP action to an X11 window through cua-driver.
///
/// `arguments` are the action arguments with any pixel coordinates already
/// scaled into the target window's local pixel space. Native identifiers are
/// never carried in `arguments`; the window is addressed by `window_id` alone.
pub(super) fn deliver_action(
    window_id: u64,
    action: &str,
    arguments: &serde_json::Map<String, Value>,
    policy: cua_media_protocol::SessionPolicy,
) -> Result<ActionOutcome, ProviderError> {
    use cua_driver_core::interactive_input::{KeyOp, PointerOp, TargetedButton};
    use platform_linux::input::targeted::{self, KeyRequest, PointerRequest};

    let window = u32::try_from(window_id).map_err(|_| target_gone())?;
    let delivery = crate::driver_input::targeted_delivery(policy);
    let allow_auto_foreground = crate::driver_input::auto_foreground_allowed();
    let modifiers = modifier_list(arguments);
    let pointer = |x_field: &str, y_field: &str, op: PointerOp| {
        let (x, y) = resolve_root_point(window, arguments, x_field, y_field)?;
        targeted::pointer(&PointerRequest {
            window: Some(window),
            delivery,
            allow_auto_foreground,
            x,
            y,
            op,
        })
        .map_err(crate::driver_input::targeted_error)
    };
    let keyboard = |op: KeyOp| {
        targeted::keyboard(&KeyRequest {
            window: Some(window),
            delivery,
            allow_auto_foreground,
            op,
        })
        .map_err(crate::driver_input::targeted_error)
    };
    let report = match action {
        "click" => pointer(
            "x",
            "y",
            PointerOp::Click {
                button: TargetedButton::from_name(str_arg(arguments, "button").unwrap_or("left")),
                count: arguments
                    .get("count")
                    .and_then(Value::as_u64)
                    .unwrap_or(1)
                    .clamp(1, 10) as u32,
                modifiers,
            },
        )?,
        "double_click" => pointer(
            "x",
            "y",
            PointerOp::Click {
                button: TargetedButton::Left,
                count: 2,
                modifiers,
            },
        )?,
        "right_click" => pointer(
            "x",
            "y",
            PointerOp::Click {
                button: TargetedButton::Right,
                count: 1,
                modifiers,
            },
        )?,
        "scroll" => {
            let amount = arguments
                .get("amount")
                .and_then(Value::as_u64)
                .unwrap_or(3)
                .clamp(1, 50) as i32;
            let (dx, dy) = match str_arg(arguments, "direction")
                .unwrap_or("down")
                .to_ascii_lowercase()
                .as_str()
            {
                "up" => (0, -amount),
                "left" => (-amount, 0),
                "right" => (amount, 0),
                _ => (0, amount),
            };
            pointer("x", "y", PointerOp::Scroll { dx, dy })?
        }
        "drag" => {
            let (from_x, from_y) = resolve_root_point(window, arguments, "from_x", "from_y")?;
            let (to_x, to_y) = resolve_root_point(window, arguments, "to_x", "to_y")?;
            let path = (1..=DRAG_STEPS)
                .map(|step| {
                    (
                        from_x + (to_x - from_x) * step / DRAG_STEPS,
                        from_y + (to_y - from_y) * step / DRAG_STEPS,
                    )
                })
                .collect();
            targeted::pointer(&PointerRequest {
                window: Some(window),
                delivery,
                allow_auto_foreground,
                x: from_x,
                y: from_y,
                op: PointerOp::Drag {
                    path,
                    button: TargetedButton::from_name(
                        str_arg(arguments, "button").unwrap_or("left"),
                    ),
                    modifiers,
                },
            })
            .map_err(crate::driver_input::targeted_error)?
        }
        "press_key" | "hotkey" => {
            let key = str_arg(arguments, "key").ok_or_else(|| {
                ProviderError::new(
                    ProviderErrorCode::DeliveryFailed,
                    "press_key requires a `key` argument",
                )
            })?;
            keyboard(KeyOp::Press {
                key: key.to_owned(),
                modifiers,
                repeat: 1,
            })?
        }
        "type_text" => {
            let text = str_arg(arguments, "text").ok_or_else(|| {
                ProviderError::new(
                    ProviderErrorCode::DeliveryFailed,
                    "type_text requires a `text` argument",
                )
            })?;
            keyboard(KeyOp::Type(text.to_owned()))?
        }
        other => {
            return Err(ProviderError::new(
                ProviderErrorCode::Unsupported,
                format!("action {other} is not supported by the X11 input backend"),
            ));
        }
    };
    Ok(ActionOutcome {
        delivered: true,
        detail: Some(serde_json::json!({
            "delivery": match report.delivery {
                cua_driver_core::interactive_input::DeliveryUsed::Background => "background",
                cua_driver_core::interactive_input::DeliveryUsed::Foreground => "foreground",
            },
            "focus_changed": report.focus_changed,
            "pointer_moved": report.pointer_moved,
            "detail": report.detail,
        })),
    })
}

fn str_arg<'a>(arguments: &'a serde_json::Map<String, Value>, key: &str) -> Option<&'a str> {
    arguments.get(key).and_then(Value::as_str)
}

/// Translate a window-local pixel point (from the named argument fields) to a
/// root-space point. Falls back to the window's center when the coordinates
/// are absent.
fn resolve_root_point(
    window: Window,
    arguments: &serde_json::Map<String, Value>,
    x_field: &str,
    y_field: &str,
) -> Result<(i32, i32), ProviderError> {
    let (conn, root) = crate::linux_x11::connect()?;
    let x = arguments.get(x_field).and_then(Value::as_f64);
    let y = arguments.get(y_field).and_then(Value::as_f64);
    let (local_x, local_y) = match (x, y) {
        (Some(x), Some(y)) => (clamp_i16(x), clamp_i16(y)),
        _ => {
            let geometry = conn
                .get_geometry(window)
                .map_err(x_error)?
                .reply()
                .map_err(|_| target_gone())?;
            (
                (i32::from(geometry.width) / 2) as i16,
                (i32::from(geometry.height) / 2) as i16,
            )
        }
    };
    let translated = conn
        .translate_coordinates(window, root, local_x, local_y)
        .map_err(x_error)?
        .reply()
        .map_err(|_| target_gone())?;
    Ok((i32::from(translated.dst_x), i32::from(translated.dst_y)))
}

fn clamp_i16(value: f64) -> i16 {
    value
        .round()
        .clamp(f64::from(i16::MIN), f64::from(i16::MAX)) as i16
}

fn modifier_list(arguments: &serde_json::Map<String, Value>) -> Vec<String> {
    let mut names = Vec::new();
    for key in ["modifiers", "modifier"] {
        if let Some(array) = arguments.get(key).and_then(Value::as_array) {
            names.extend(array.iter().filter_map(Value::as_str).map(str::to_owned));
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dimensions_preserve_aspect_ratio_and_cap_long_edge() {
        assert_eq!(fit_dimensions(2560, 1440, 1280), (1280, 720));
        assert_eq!(fit_dimensions(800, 1200, 600), (400, 600));
        assert_eq!(fit_dimensions(640, 480, 1280), (640, 480));
    }

    #[test]
    fn finish_image_forces_opaque_alpha_and_reports_stride() {
        let data = vec![1, 2, 3, 0, 4, 5, 6, 0];
        let (pixels, stride) = finish_image(data, 2, 1).expect("packed image");
        assert_eq!(stride, 8);
        assert_eq!(pixels, vec![1, 2, 3, 0xff, 4, 5, 6, 0xff]);
    }

    #[test]
    fn repack_drops_row_padding() {
        // 2x2 image with a 4-byte pad after each 8-byte row.
        let padded = vec![
            10, 11, 12, 13, 14, 15, 16, 17, 0, 0, 0, 0, // row 0 + pad
            20, 21, 22, 23, 24, 25, 26, 27, 0, 0, 0, 0, // row 1 + pad
        ];
        let tight = repack_tight(&padded, 12, 2, 2);
        assert_eq!(
            tight,
            vec![10, 11, 12, 13, 14, 15, 16, 17, 20, 21, 22, 23, 24, 25, 26, 27]
        );
    }

    #[test]
    fn clamp_dimension_raises_small_values_to_min() {
        assert_eq!(clamp_dimension(0, MIN_WINDOW_WIDTH), MIN_WINDOW_WIDTH);
        assert_eq!(clamp_dimension(10, MIN_WINDOW_HEIGHT), MIN_WINDOW_HEIGHT);
        assert_eq!(clamp_dimension(1280, MIN_WINDOW_WIDTH), 1280);
        assert_eq!(clamp_dimension(720, MIN_WINDOW_HEIGHT), 720);
    }

    #[test]
    fn downscale_halves_dimensions() {
        // 2x2 distinct pixels -> 1x1 picks the top-left source pixel.
        let src = vec![
            1, 1, 1, 255, 2, 2, 2, 255, // row 0
            3, 3, 3, 255, 4, 4, 4, 255, // row 1
        ];
        let out = downscale(&src, 2, 2, 1, 1);
        assert_eq!(out, vec![1, 1, 1, 255]);
    }

    #[test]
    fn modifier_list_reads_both_argument_shapes() {
        let arguments = serde_json::json!({
            "modifiers": ["ctrl", "shift"],
            "modifier": ["alt"],
        })
        .as_object()
        .expect("object")
        .clone();
        assert_eq!(modifier_list(&arguments), vec!["ctrl", "shift", "alt"]);
    }

    #[test]
    fn clamp_i16_saturates_out_of_range_pixels() {
        assert_eq!(clamp_i16(10.6), 11);
        assert_eq!(clamp_i16(-5.0), -5);
        assert_eq!(clamp_i16(70_000.0), i16::MAX);
        assert_eq!(clamp_i16(-70_000.0), i16::MIN);
    }
}
