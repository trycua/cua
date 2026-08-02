//! Window screenshot on Linux.
//!
//! Strategy (in order of preference) for per-window X11 capture:
//! 1. Persistent MIT-SHM (`shm_get_image`) via x11rb — warm path, no subprocess
//! 2. Persistent plain `XGetImage` via x11rb — still in-process
//! 3. `import -window <xid> png:-` (ImageMagick compatibility fallback)
//!
//! Main-display capture keeps its own dispatch (Wayland cascade → ImageMagick →
//! root `XGetImage`). Wayland-native per-window paths are not routed into XShm.
//!
//! x11rb MIT-SHM API (pinned 0.13.2):
//! - https://docs.rs/x11rb/0.13.2/x11rb/protocol/shm/trait.ConnectionExt.html
//! - https://docs.rs/x11rb/0.13.2/x11rb/protocol/shm/struct.GetImageReply.html
//! - https://docs.rs/x11rb/0.13.2/x11rb/protocol/shm/struct.CreateSegmentReply.html
//!
//! Request order: `shm_query_version().reply()` before any other SHM call;
//! `generate_id`; `shm_create_segment(seg, size, false).reply()` → `OwnedFd`;
//! map FD; `shm_get_image(..., seg, 0).reply()` writes into mapped memory;
//! `shm_detach(seg)` on replacement/drop.

use anyhow::{anyhow, bail, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use std::process::Command;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

/// Max edge length accepted for a single SHM/XGetImage capture (px).
const MAX_CAPTURE_DIM: u32 = 16_384;
/// Max SHM segment size (1 GiB).
const MAX_CAPTURE_BYTES: usize = 1 << 30;
/// After MIT-SHM init/extension failure, same-DISPLAY may be probed again only
/// after this backoff. Request/capture failures never use this path.
const XSHM_INIT_RETRY_BACKOFF: Duration = Duration::from_secs(30);

/// Capture a window by X11 XID. Returns raw PNG bytes.
pub fn screenshot_window_bytes(xid: u64) -> Result<Vec<u8>> {
    capture_window_with_backends(
        xid,
        capture_via_xshm,
        capture_via_persistent_xgetimage,
        capture_via_import,
    )
}

/// Capture a window by X11 XID. Returns (base64_png, width, height).
pub fn screenshot_window(xid: u64) -> Result<(String, u32, u32)> {
    let bytes = screenshot_window_bytes(xid)?;
    let (w, h) = cua_driver_core::image_utils::png_dimensions(&bytes)?;
    Ok((BASE64.encode(&bytes), w, h))
}

/// Ordered window-capture backend cascade.
///
/// Non-empty XShm success returns immediately; otherwise try non-empty
/// XGetImage; otherwise ImageMagick. If all fail or return empty, the final
/// error preserves all three contexts. Closures are `FnOnce` only (no
/// `Send`/`Sync` bounds) so unit tests can drive them with `Rc`/`Cell`.
fn capture_window_with_backends(
    xid: u64,
    xshm: impl FnOnce(u64) -> Result<Vec<u8>>,
    xgetimage: impl FnOnce(u64) -> Result<Vec<u8>>,
    imagemagick: impl FnOnce(u64) -> Result<Vec<u8>>,
) -> Result<Vec<u8>> {
    let xshm_err = match xshm(xid) {
        Ok(bytes) if !bytes.is_empty() => return Ok(bytes),
        Ok(_) => "XShm returned empty image".to_string(),
        Err(e) => format!("{e:#}"),
    };

    let xgetimage_err = match xgetimage(xid) {
        Ok(bytes) if !bytes.is_empty() => return Ok(bytes),
        Ok(_) => "XGetImage returned empty image".to_string(),
        Err(e) => format!("{e:#}"),
    };

    let imagemagick_err = match imagemagick(xid) {
        Ok(bytes) if !bytes.is_empty() => return Ok(bytes),
        Ok(_) => "ImageMagick returned empty image".to_string(),
        Err(e) => format!("{e:#}"),
    };

    Err(anyhow!(
        "all Linux window capture backends failed\n- XShm: {xshm_err}\n- XGetImage: {xgetimage_err}\n- ImageMagick: {imagemagick_err}"
    ))
}

fn capture_via_import(xid: u64) -> Result<Vec<u8>> {
    let out = Command::new("import")
        .args(["-window", &xid.to_string(), "png:-"])
        .output()?;
    if !out.status.success() || out.stdout.is_empty() {
        bail!("import failed");
    }
    Ok(out.stdout)
}

// ── shared pixel conversion ───────────────────────────────────────────────

/// Convert packed BGRA/BGRX (`w*h*4` bytes) to PNG. Depth 32 uses byte 3 as
/// alpha; depth 24 forces alpha=255. Exact `w*h*4` length is required.
fn bgra_zpixmap_to_png(data: &[u8], w: u32, h: u32, depth: u8) -> Result<Vec<u8>> {
    let expected = checked_image_byte_len(w, h)?;
    if data.len() != expected {
        bail!(
            "pixel buffer length {} != expected {} ({}x{}, depth {})",
            data.len(),
            expected,
            w,
            h,
            depth
        );
    }
    match depth {
        24 | 32 => {}
        other => bail!("Unsupported depth: {other}"),
    }

    let mut rgba = Vec::with_capacity(expected);
    for chunk in data.chunks_exact(4) {
        let (b, g, r) = (chunk[0], chunk[1], chunk[2]);
        let a = if depth == 32 { chunk[3] } else { 255 };
        rgba.extend_from_slice(&[r, g, b, a]);
    }
    cua_driver_core::image_utils::encode_rgba_to_png(&rgba, w, h)
}

fn checked_image_byte_len(w: u32, h: u32) -> Result<usize> {
    if w == 0 || h == 0 {
        bail!("window geometry is 0x0");
    }
    if w > MAX_CAPTURE_DIM || h > MAX_CAPTURE_DIM {
        bail!("window geometry {w}x{h} exceeds max {MAX_CAPTURE_DIM}px edge");
    }
    let bytes = (w as u64)
        .checked_mul(h as u64)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(|| anyhow!("window geometry {w}x{h} overflows byte length"))?;
    if bytes == 0 || bytes > MAX_CAPTURE_BYTES as u64 {
        bail!("window capture size {bytes} out of bounds (max {MAX_CAPTURE_BYTES})");
    }
    Ok(bytes as usize)
}

fn current_display() -> String {
    std::env::var("DISPLAY").unwrap_or_default()
}

fn lock_mutex<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

// ── persistent MIT-SHM session ────────────────────────────────────────────

struct ShmBuffer {
    seg: u32,
    map: memmap2::MmapMut,
    capacity: usize,
}

struct XShmSession {
    display: String,
    conn: x11rb::rust_connection::RustConnection,
    #[allow(dead_code)]
    screen_num: usize,
    buffer: Option<ShmBuffer>,
}

enum XShmState {
    /// No live session yet (or after a full reset).
    Uninit,
    /// MIT-SHM init/extension unavailable for `display` until `retry_after`.
    /// Do not reprob each frame while the backoff is active. Request/capture
    /// failures must never land here — they reset to `Uninit` instead.
    Unsupported {
        display: String,
        reason: String,
        retry_after: Instant,
    },
    Ready(XShmSession),
}

impl XShmState {
    /// Build `Unsupported` from an initialization/extension failure.
    /// `now` is injected so pure policy tests need not sleep.
    fn unsupported_after_init_failure(display: String, reason: String, now: Instant) -> Self {
        Self::Unsupported {
            display,
            reason,
            retry_after: now + XSHM_INIT_RETRY_BACKOFF,
        }
    }

    /// State after a failed capture request even after reconnect+retry.
    /// Never caches as `Unsupported` — stale windows and transport blips must
    /// remain recoverable on the next call.
    fn after_capture_retry_failure() -> Self {
        Self::Uninit
    }

    /// Consume init backoff for `display` at caller-supplied `now`.
    ///
    /// - Same-DISPLAY `Unsupported` before `retry_after`: leave state, `Err(reason)`.
    /// - Same-DISPLAY `Unsupported` at/after deadline: reset to `Uninit`, `Ok(())`.
    /// - Different-DISPLAY `Unsupported`: reset to `Uninit`, `Ok(())`.
    /// - `Ready` / `Uninit`: leave state, `Ok(())`.
    fn consume_init_backoff(
        &mut self,
        display: &str,
        now: Instant,
    ) -> std::result::Result<(), String> {
        match self {
            Self::Unsupported {
                display: d,
                reason,
                retry_after,
            } if d.as_str() == display => {
                if now < *retry_after {
                    Err(reason.clone())
                } else {
                    *self = Self::Uninit;
                    Ok(())
                }
            }
            Self::Unsupported { .. } => {
                *self = Self::Uninit;
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

impl Drop for XShmSession {
    fn drop(&mut self) {
        self.detach_buffer_best_effort();
    }
}

/// Run `map` after a server SHM segment has been created. On map error,
/// invoke `cleanup` exactly once (best-effort detach) and return the
/// original map error unchanged. Success path never calls cleanup.
fn map_created_segment_with_cleanup<T>(
    map: impl FnOnce() -> Result<T>,
    cleanup: impl FnOnce(),
) -> Result<T> {
    match map() {
        Ok(v) => Ok(v),
        Err(e) => {
            cleanup();
            Err(e)
        }
    }
}

impl XShmSession {
    fn connect(display: String) -> Result<Self> {
        use x11rb::protocol::shm::ConnectionExt as _;

        let (conn, screen_num) = x11rb::rust_connection::RustConnection::connect(None)
            .map_err(|e| anyhow!("X11 connect for SHM: {e}"))?;

        // MUST query version before any other SHM request.
        let ver = conn
            .shm_query_version()
            .map_err(|e| anyhow!("shm_query_version request: {e}"))?
            .reply()
            .map_err(|e| anyhow!("shm_query_version reply: {e}"))?;

        // CreateSegment requires MIT-SHM >= 1.2.
        if ver.major_version < 1 || (ver.major_version == 1 && ver.minor_version < 2) {
            bail!(
                "MIT-SHM {}.{} < 1.2 (CreateSegment unsupported)",
                ver.major_version,
                ver.minor_version
            );
        }

        Ok(Self {
            display,
            conn,
            screen_num,
            buffer: None,
        })
    }

    fn detach_buffer_best_effort(&mut self) {
        use x11rb::protocol::shm::ConnectionExt as _;
        if let Some(buf) = self.buffer.take() {
            let _ = self.conn.shm_detach(buf.seg);
            // `buf.map` drops here; never panic on detach failure.
        }
    }

    fn ensure_buffer(&mut self, need: usize) -> Result<()> {
        use x11rb::connection::Connection;
        use x11rb::protocol::shm::ConnectionExt as _;

        if need == 0 {
            bail!("SHM buffer size must be nonzero");
        }
        if need > MAX_CAPTURE_BYTES {
            bail!("SHM buffer size {need} exceeds max {MAX_CAPTURE_BYTES}");
        }
        if let Some(buf) = &self.buffer {
            if buf.capacity >= need {
                return Ok(());
            }
        }

        // Grow: detach old segment and drop old mapping before allocating.
        self.detach_buffer_best_effort();

        let size_u32 =
            u32::try_from(need).map_err(|_| anyhow!("SHM buffer size {need} does not fit u32"))?;
        let seg = self
            .conn
            .generate_id()
            .map_err(|e| anyhow!("generate_id for SHM segment: {e}"))?;
        let reply = self
            .conn
            .shm_create_segment(seg, size_u32, false)
            .map_err(|e| anyhow!("shm_create_segment request: {e}"))?
            .reply()
            .map_err(|e| anyhow!("shm_create_segment reply: {e}"))?;

        let fd = reply.shm_fd;
        // SAFETY: we own the server-returned FD for a segment of exactly
        // `need` bytes (fixed nonzero size from CreateSegment). We never
        // truncate the underlying object while the mapping is live.
        // On map failure, detach the just-created server segment before
        // returning so it is not leaked until connection teardown.
        let map = map_created_segment_with_cleanup(
            || unsafe {
                memmap2::MmapOptions::new()
                    .len(need)
                    .map_mut(&fd)
                    .map_err(|e| anyhow!("mmap MIT-SHM CreateSegment FD: {e}"))
            },
            || {
                if let Ok(cookie) = self.conn.shm_detach(seg) {
                    let _ = cookie.check();
                }
            },
        )?;
        // Mapping retains the pages; FD can close.
        drop(fd);

        self.buffer = Some(ShmBuffer {
            seg,
            map,
            capacity: need,
        });
        Ok(())
    }

    /// Geometry + SHM request/reply + copy into owned Vec. Caller encodes off-lock.
    fn capture_raw(&mut self, xid: u64) -> Result<RawFrame> {
        use x11rb::protocol::shm::ConnectionExt as _;
        use x11rb::protocol::xproto::{ConnectionExt as _, ImageFormat};

        let window = xid as u32;
        let geom = self
            .conn
            .get_geometry(window)
            .map_err(|e| anyhow!("get_geometry request: {e}"))?
            .reply()
            .map_err(|e| anyhow!("get_geometry reply: {e}"))?;
        let w = u32::from(geom.width);
        let h = u32::from(geom.height);
        let need = checked_image_byte_len(w, h)?;

        self.ensure_buffer(need)?;
        let (seg, map_len) = {
            let buf = self
                .buffer
                .as_ref()
                .ok_or_else(|| anyhow!("SHM buffer missing after ensure"))?;
            if buf.map.len() < need {
                bail!("mapped SHM length {} < required {need}", buf.map.len());
            }
            (buf.seg, buf.map.len())
        };

        let reply = self
            .conn
            .shm_get_image(
                window,
                0,
                0,
                geom.width,
                geom.height,
                !0u32,
                u8::from(ImageFormat::Z_PIXMAP),
                seg,
                0,
            )
            .map_err(|e| anyhow!("shm_get_image request: {e}"))?
            .reply()
            .map_err(|e| anyhow!("shm_get_image reply: {e}"))?;

        match reply.depth {
            24 | 32 => {}
            other => bail!("Unsupported depth: {other}"),
        }

        let size = reply.size as usize;
        if size != need {
            bail!("shm_get_image size {size} != expected {need} ({}x{})", w, h);
        }
        if size > map_len {
            bail!("shm_get_image size {size} exceeds mapped {map_len}");
        }

        let data = {
            let buf = self
                .buffer
                .as_ref()
                .ok_or_else(|| anyhow!("SHM buffer missing after get_image"))?;
            buf.map[..size].to_vec()
        };
        Ok(RawFrame {
            data,
            w,
            h,
            depth: reply.depth,
        })
    }
}

struct RawFrame {
    data: Vec<u8>,
    w: u32,
    h: u32,
    depth: u8,
}

fn xshm_state() -> &'static Mutex<XShmState> {
    static STATE: OnceLock<Mutex<XShmState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(XShmState::Uninit))
}

fn capture_via_xshm(xid: u64) -> Result<Vec<u8>> {
    let display = current_display();
    let mut guard = lock_mutex(xshm_state());

    // Init backoff for this DISPLAY — no per-frame reprobe while active.
    // Expired same-DISPLAY / different-DISPLAY Unsupported → Uninit (probeable).
    if let Err(reason) = guard.consume_init_backoff(&display, Instant::now()) {
        bail!("MIT-SHM disabled for DISPLAY={display}: {reason}");
    }

    // Ensure Ready session for current DISPLAY (connect only on init/recovery).
    match ensure_xshm_ready(&mut guard, &display) {
        Ok(()) => {}
        Err(e) => {
            let reason = format!("{e:#}");
            *guard = XShmState::unsupported_after_init_failure(
                display.clone(),
                reason.clone(),
                Instant::now(),
            );
            bail!("MIT-SHM init failed for DISPLAY={display}: {reason}");
        }
    }

    // Warm capture under lock (geometry + SHM + copy only).
    let first = {
        let session = match &mut *guard {
            XShmState::Ready(session) => session,
            _ => bail!("internal: XShm state not Ready after ensure"),
        };
        session.capture_raw(xid)
    };

    let frame = match first {
        Ok(frame) => frame,
        Err(first_err) => {
            // Cached session failure: discard/detach and reconnect+retry once.
            *guard = XShmState::Uninit;
            let retry_init = ensure_xshm_ready(&mut guard, &display);
            let second = match retry_init {
                Ok(()) => {
                    let session = match &mut *guard {
                        XShmState::Ready(session) => session,
                        _ => {
                            return Err(anyhow!("internal: XShm not Ready after reconnect"));
                        }
                    };
                    session.capture_raw(xid)
                }
                Err(e) => Err(e),
            };
            match second {
                Ok(frame) => frame,
                Err(second_err) => {
                    let combined = format!("first: {first_err:#}; retry: {second_err:#}");
                    // Request/capture failures never enter Unsupported.
                    *guard = XShmState::after_capture_retry_failure();
                    bail!(
                        "MIT-SHM capture failed after reconnect for DISPLAY={display}: {combined}"
                    );
                }
            }
        }
    };

    // Release the session mutex before BGRA→RGBA conversion and PNG encode.
    drop(guard);
    bgra_zpixmap_to_png(&frame.data, frame.w, frame.h, frame.depth)
}

fn ensure_xshm_ready(guard: &mut XShmState, display: &str) -> Result<()> {
    match guard {
        XShmState::Ready(session) if session.display == display => Ok(()),
        XShmState::Unsupported {
            display: d, reason, ..
        } if d == display => {
            // Defensive: call sites should consume backoff first.
            bail!("MIT-SHM disabled for DISPLAY={display}: {reason}");
        }
        _ => {
            // DISPLAY change or Uninit: drop old session (Detach on Drop) and reconnect.
            *guard = XShmState::Uninit;
            let session = XShmSession::connect(display.to_string())?;
            *guard = XShmState::Ready(session);
            Ok(())
        }
    }
}

// ── persistent plain XGetImage session ────────────────────────────────────

struct XGetImageSession {
    display: String,
    conn: x11rb::rust_connection::RustConnection,
}

fn xgetimage_state() -> &'static Mutex<Option<XGetImageSession>> {
    static STATE: OnceLock<Mutex<Option<XGetImageSession>>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(None))
}

fn capture_via_persistent_xgetimage(xid: u64) -> Result<Vec<u8>> {
    let display = current_display();
    let mut guard = lock_mutex(xgetimage_state());

    ensure_xgetimage_ready(&mut guard, &display)
        .map_err(|e| anyhow!("XGetImage connect: {e:#}"))?;

    let first = match guard.as_mut() {
        Some(session) => session.capture_raw(xid),
        None => Err(anyhow!("internal: XGetImage session missing after ensure")),
    };

    let frame = match first {
        Ok(frame) => frame,
        Err(first_err) => {
            // Cached connection failure → reconnect once.
            *guard = None;
            ensure_xgetimage_ready(&mut guard, &display)
                .map_err(|e| anyhow!("XGetImage reconnect after error ({first_err:#}): {e:#}"))?;
            match guard.as_mut() {
                Some(session) => session.capture_raw(xid).map_err(|e| {
                    anyhow!("XGetImage failed after reconnect (first: {first_err:#}): {e:#}")
                })?,
                None => {
                    bail!("XGetImage session missing after reconnect (first: {first_err:#})")
                }
            }
        }
    };

    drop(guard);
    bgra_zpixmap_to_png(&frame.data, frame.w, frame.h, frame.depth)
}

fn ensure_xgetimage_ready(guard: &mut Option<XGetImageSession>, display: &str) -> Result<()> {
    if let Some(session) = guard.as_ref() {
        if session.display == display {
            return Ok(());
        }
    }
    *guard = None;
    let (conn, _screen) =
        x11rb::rust_connection::RustConnection::connect(None).map_err(|e| anyhow!("{e}"))?;
    *guard = Some(XGetImageSession {
        display: display.to_string(),
        conn,
    });
    Ok(())
}

impl XGetImageSession {
    fn capture_raw(&mut self, xid: u64) -> Result<RawFrame> {
        use x11rb::protocol::xproto::{ConnectionExt as _, ImageFormat};

        let window = xid as u32;
        let geom = self
            .conn
            .get_geometry(window)
            .map_err(|e| anyhow!("get_geometry request: {e}"))?
            .reply()
            .map_err(|e| anyhow!("get_geometry reply: {e}"))?;
        let w = u32::from(geom.width);
        let h = u32::from(geom.height);
        let need = checked_image_byte_len(w, h)?;

        let img = self
            .conn
            .get_image(
                ImageFormat::Z_PIXMAP,
                window,
                0,
                0,
                geom.width,
                geom.height,
                !0u32,
            )
            .map_err(|e| anyhow!("get_image request: {e}"))?
            .reply()
            .map_err(|e| anyhow!("get_image reply: {e}"))?;

        match img.depth {
            24 | 32 => {}
            other => bail!("Unsupported depth: {other}"),
        }
        if img.data.len() != need {
            bail!(
                "XGetImage data length {} != expected {need} ({}x{})",
                img.data.len(),
                w,
                h
            );
        }

        Ok(RawFrame {
            data: img.data,
            w,
            h,
            depth: img.depth,
        })
    }
}

/// Public version of png_dimensions for use in tool code.
pub fn png_dimensions_pub(data: &[u8]) -> Result<(u32, u32)> {
    cua_driver_core::image_utils::png_dimensions(data)
}

// NOTE: the previously-inline `png_dimensions`, `write_uncompressed_png`,
// `write_png_chunk`, `zlib_store`, `adler32` (and `crc32_ieee` below)
// were extracted to `cua_driver_core::image_utils` in the 2026-05 dedup
// audit so all three platforms call the same code. See
// `CUA_DRIVER_RS_DEDUP_AUDIT.md`. RGBA-encoding callers below now go
// through `cua_driver_core::image_utils::encode_rgba_to_png`.

/// Capture the primary display (root window) as raw PNG bytes.
///
/// Dispatch:
/// - Native Wayland (`CUA_DRIVER_RS_ENABLE_WAYLAND=1` + Wayland session):
///   routes through [`crate::wayland::screenshot_display_dispatch`] which
///   owns the complete GNOME helper → wlroots screencopy →
///   ext-image-copy-capture-v1 → portal Screenshot → X11 cascade. An
///   available GNOME helper's capture failure is terminal.
/// - X11 / Wayland-disabled: ImageMagick `import` → x11rb `XGetImage`.
pub fn screenshot_display_bytes() -> Result<Vec<u8>> {
    screenshot_display_bytes_with_dispatch(
        crate::wayland::is_wayland(),
        crate::wayland::screenshot_display_dispatch,
        screenshot_display_bytes_x11,
    )
}

fn screenshot_display_bytes_with_dispatch(
    wayland_enabled: bool,
    wayland_capture: impl FnOnce() -> Result<Vec<u8>>,
    x11_capture: impl FnOnce() -> Result<Vec<u8>>,
) -> Result<Vec<u8>> {
    if wayland_enabled {
        wayland_capture()
    } else {
        x11_capture()
    }
}

/// X11-only display capture path — extracted so the wayland cascade in
/// [`crate::wayland::screenshot_display_dispatch`] can call it as a final
/// fallback without re-entering [`screenshot_display_bytes`] (which would
/// loop forever once we're on Wayland).
pub(crate) fn screenshot_display_bytes_x11() -> Result<Vec<u8>> {
    // Try `import -window root png:-` (ImageMagick).
    let out = Command::new("import")
        .args(["-window", "root", "png:-"])
        .output();
    if let Ok(o) = out {
        if o.status.success() && !o.stdout.is_empty() {
            return Ok(o.stdout);
        }
    }
    // Fallback: x11rb XGetImage on the root window.
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::*;
    use x11rb::rust_connection::RustConnection;
    let (conn, screen_num) = RustConnection::connect(None)
        .map_err(|e| anyhow::anyhow!("{e}{}", crate::no_display_hint()))?;
    let root = conn.setup().roots[screen_num].root;
    // Get root geometry.
    let geom = conn.get_geometry(root)?.reply()?;
    let w = geom.width as u32;
    let h = geom.height as u32;
    // WSLg / headless XWayland quirk: the X server connects but the root
    // window reports a 0-px geometry until a real output is attached.
    // `get_image` with w/h == 0 yields an empty buffer that later decodes
    // to null/zero dimensions downstream. Fail with an actionable, typed
    // error instead of emitting a 0-px image. See issue #2005.
    if w == 0 || h == 0 {
        anyhow::bail!(
            "X11 root window reports a 0x0 geometry — no usable display to capture.{}",
            crate::no_display_hint()
        );
    }
    let img = conn
        .get_image(ImageFormat::Z_PIXMAP, root, 0, 0, w as u16, h as u16, !0u32)?
        .reply()?;
    let bytes = img.data;
    let bpp = match img.depth {
        32 | 24 => 4usize,
        _ => anyhow::bail!("Unsupported depth"),
    };
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for chunk in bytes.chunks_exact(bpp) {
        let (b, g, r) = (chunk[0], chunk[1], chunk[2]);
        rgba.extend_from_slice(&[r, g, b, 255]);
    }
    cua_driver_core::image_utils::encode_rgba_to_png(&rgba, w, h)
}

/// Capture the primary display, returning (base64_png, width, height).
pub fn screenshot_display() -> Result<(String, u32, u32)> {
    let png_bytes = screenshot_display_bytes()?;
    let (w, h) = cua_driver_core::image_utils::png_dimensions(&png_bytes)?;
    Ok((BASE64.encode(&png_bytes), w, h))
}

// PNG/JPEG/resize/crosshair helpers — re-exports of the shared
// `cua_driver_core::image_utils` module. The previous file-local copies were
// near-identical to the macOS and Windows versions; the dedup-audit
// (2026-05) moved them all to one place.

/// Convert PNG bytes to JPEG at the given quality (1–95).
pub fn png_bytes_to_jpeg(png_bytes: &[u8], quality: u8) -> Result<Vec<u8>> {
    cua_driver_core::image_utils::png_bytes_to_jpeg(png_bytes, quality)
}

/// Downscale `png_bytes` so neither dimension exceeds `max_dim`.
/// If `max_dim == 0` or the image already fits, returns a copy of the
/// original bytes unchanged.
pub fn resize_png_if_needed(png_bytes: &[u8], max_dim: u32) -> Result<Vec<u8>> {
    cua_driver_core::image_utils::resize_png_if_needed(png_bytes, max_dim)
}

/// Draw a red crosshair at pixel (cx, cy) on a PNG image and return
/// modified PNG bytes. Used by recording's click-marker callback to
/// produce click.png.
pub fn crosshair_png_bytes(png_bytes: &[u8], cx: f64, cy: f64) -> Result<Vec<u8>> {
    cua_driver_core::image_utils::crosshair_png_bytes(png_bytes, cx, cy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    /// Mapping failure after a server SHM segment exists must run cleanup
    /// exactly once and still surface the original map error.
    #[test]
    fn xshm_mapping_failure_detaches_created_segment() {
        let cleanups = Rc::new(Cell::new(0u32));
        let cleanups_c = Rc::clone(&cleanups);

        let err = map_created_segment_with_cleanup(
            || Err::<(), _>(anyhow!("mmap failed")),
            || {
                cleanups_c.set(cleanups_c.get() + 1);
            },
        )
        .expect_err("map failure must propagate");

        assert_eq!(
            cleanups.get(),
            1,
            "cleanup must run exactly once on map error"
        );
        let msg = format!("{err:#}");
        assert!(
            msg.contains("mmap failed"),
            "original map error must be preserved, got: {msg}"
        );
    }

    #[test]
    fn xshm_same_display_unsupported_blocked_before_retry_deadline() {
        let t0 = Instant::now();
        let mut state =
            XShmState::unsupported_after_init_failure(":0".into(), "connect refused".into(), t0);
        let before = t0 + Duration::from_secs(29);
        let err = state
            .consume_init_backoff(":0", before)
            .expect_err("must stay blocked before deadline");
        assert_eq!(err, "connect refused");
        match &state {
            XShmState::Unsupported {
                display,
                reason,
                retry_after,
            } => {
                assert_eq!(display, ":0");
                assert_eq!(reason, "connect refused");
                assert_eq!(*retry_after, t0 + XSHM_INIT_RETRY_BACKOFF);
            }
            XShmState::Uninit | XShmState::Ready(_) => {
                panic!("expected Unsupported still cached before deadline")
            }
        }
    }

    #[test]
    fn xshm_same_display_unsupported_becomes_uninit_at_retry_deadline() {
        let t0 = Instant::now();
        let mut state = XShmState::unsupported_after_init_failure(
            ":0".into(),
            "shm_query_version failed".into(),
            t0,
        );
        let at_deadline = t0 + XSHM_INIT_RETRY_BACKOFF;
        state
            .consume_init_backoff(":0", at_deadline)
            .expect("deadline must make same DISPLAY probeable");
        assert!(
            matches!(state, XShmState::Uninit),
            "expired backoff must reset to Uninit"
        );
    }

    #[test]
    fn xshm_same_display_unsupported_becomes_uninit_after_retry_deadline() {
        let t0 = Instant::now();
        let mut state =
            XShmState::unsupported_after_init_failure(":1".into(), "extension missing".into(), t0);
        let after = t0 + XSHM_INIT_RETRY_BACKOFF + Duration::from_millis(1);
        state
            .consume_init_backoff(":1", after)
            .expect("past deadline must make same DISPLAY probeable");
        assert!(matches!(state, XShmState::Uninit));
    }

    #[test]
    fn xshm_different_display_unsupported_is_immediately_probeable() {
        let t0 = Instant::now();
        let mut state =
            XShmState::unsupported_after_init_failure(":0".into(), "init failed on :0".into(), t0);
        // Still well inside the 30s window for :0.
        let now = t0 + Duration::from_secs(1);
        state
            .consume_init_backoff(":1", now)
            .expect("DISPLAY change must ignore prior backoff");
        assert!(
            matches!(state, XShmState::Uninit),
            "different DISPLAY must reset Unsupported to Uninit"
        );
    }

    #[test]
    fn xshm_capture_retry_failure_yields_uninit_not_unsupported() {
        let state = XShmState::after_capture_retry_failure();
        assert!(
            matches!(state, XShmState::Uninit),
            "request/capture retry failure must never enter Unsupported"
        );
        // Explicit counter-check: constructing init-failure Unsupported is a
        // different path and must remain distinct from capture retry policy.
        let init_fail =
            XShmState::unsupported_after_init_failure(":0".into(), "init".into(), Instant::now());
        assert!(matches!(init_fail, XShmState::Unsupported { .. }));
        assert!(!matches!(
            XShmState::after_capture_retry_failure(),
            XShmState::Unsupported { .. }
        ));
    }

    #[test]
    fn available_gnome_helper_failure_is_terminal_at_public_boundary() {
        let x11_called = Cell::new(false);

        let result = screenshot_display_bytes_with_dispatch(
            true,
            || Err(anyhow::anyhow!("GNOME compositor helper capture failed")),
            || {
                x11_called.set(true);
                Ok(vec![1, 2, 3])
            },
        );

        assert_eq!(
            result.unwrap_err().to_string(),
            "GNOME compositor helper capture failed"
        );
        assert!(!x11_called.get(), "public boundary retried X11 capture");
    }

    #[test]
    fn wayland_disabled_uses_x11_capture() {
        let wayland_called = Cell::new(false);

        let result = screenshot_display_bytes_with_dispatch(
            false,
            || {
                wayland_called.set(true);
                Err(anyhow::anyhow!("Wayland capture should not run"))
            },
            || Ok(vec![1, 2, 3]),
        );

        assert_eq!(result.unwrap(), vec![1, 2, 3]);
        assert!(!wayland_called.get());
    }

    #[test]
    fn xshm_success_short_circuits_other_linux_capture_backends() {
        use std::rc::Rc;

        let png = cua_driver_core::image_utils::encode_rgba_to_png(&[255, 0, 0, 255], 1, 1)
            .expect("1x1 PNG");
        assert!(!png.is_empty());

        let xshm_calls = Rc::new(Cell::new(0u32));
        let xgetimage_calls = Rc::new(Cell::new(0u32));
        let imagemagick_calls = Rc::new(Cell::new(0u32));

        let xshm_calls_c = Rc::clone(&xshm_calls);
        let xgetimage_calls_c = Rc::clone(&xgetimage_calls);
        let imagemagick_calls_c = Rc::clone(&imagemagick_calls);
        let png_ret = png.clone();

        let result = capture_window_with_backends(
            42,
            move |xid| {
                xshm_calls_c.set(xshm_calls_c.get() + 1);
                assert_eq!(xid, 42);
                Ok(png_ret)
            },
            move |_xid| {
                xgetimage_calls_c.set(xgetimage_calls_c.get() + 1);
                Err(anyhow::anyhow!("XGetImage must not be invoked"))
            },
            move |_xid| {
                imagemagick_calls_c.set(imagemagick_calls_c.get() + 1);
                Err(anyhow::anyhow!("ImageMagick must not be invoked"))
            },
        );

        assert_eq!(result.expect("xshm success"), png);
        assert_eq!(xshm_calls.get(), 1);
        assert_eq!(xgetimage_calls.get(), 0);
        assert_eq!(imagemagick_calls.get(), 0);
    }

    /// Live native MIT-SHM acceptance: direct `capture_via_xshm` on a mapped
    /// 64×48 child window (no public cascade / XGetImage / ImageMagick).
    #[test]
    #[ignore = "requires a live X11 server with MIT-SHM 1.2"]
    fn live_xshm_captures_mapped_x11_window() {
        use x11rb::connection::Connection;
        use x11rb::protocol::shm::ConnectionExt as _;
        use x11rb::protocol::xproto::{ConnectionExt as _, CreateWindowAux, WindowClass};
        use x11rb::rust_connection::RustConnection;

        const W: u16 = 64;
        const H: u16 = 48;
        // Deterministic solid background (0xRRGGBB in the low 24 bits).
        const BG_PIXEL: u32 = 0x00_33_99_CC;
        const PNG_SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

        let _ = std::env::var("DISPLAY").expect("DISPLAY must be set for live X11 test");
        let (conn, screen_num) = RustConnection::connect(None).expect("connect to DISPLAY");

        let ver = conn
            .shm_query_version()
            .expect("shm_query_version request")
            .reply()
            .expect("shm_query_version reply");
        assert!(
            ver.major_version > 1 || (ver.major_version == 1 && ver.minor_version >= 2),
            "MIT-SHM {}.{} < 1.2",
            ver.major_version,
            ver.minor_version
        );

        let screen = &conn.setup().roots[screen_num];
        let window = conn.generate_id().expect("generate window id");
        let aux = CreateWindowAux::new().background_pixel(BG_PIXEL);
        conn.create_window(
            screen.root_depth,
            window,
            screen.root,
            0,
            0,
            W,
            H,
            0,
            WindowClass::INPUT_OUTPUT,
            screen.root_visual,
            &aux,
        )
        .expect("create_window request")
        .check()
        .expect("create_window sync check");
        conn.map_window(window)
            .expect("map_window request")
            .check()
            .expect("map_window sync check");
        conn.flush().expect("flush after map");

        let png1 = capture_via_xshm(u64::from(window)).expect("first capture_via_xshm");
        assert!(
            png1.starts_with(&PNG_SIG),
            "first capture missing PNG signature"
        );
        let dims1 =
            cua_driver_core::image_utils::png_dimensions(&png1).expect("png_dimensions first");
        assert_eq!(dims1, (u32::from(W), u32::from(H)));

        // Second direct call proves the warm SHM session/buffer is reusable.
        let png2 = capture_via_xshm(u64::from(window)).expect("second capture_via_xshm");
        assert!(
            png2.starts_with(&PNG_SIG),
            "second capture missing PNG signature"
        );
        let dims2 =
            cua_driver_core::image_utils::png_dimensions(&png2).expect("png_dimensions second");
        assert_eq!(dims2, (u32::from(W), u32::from(H)));

        // Best-effort cleanup — never panic here.
        let _ = conn.destroy_window(window);
        let _ = conn.flush();
    }
}
