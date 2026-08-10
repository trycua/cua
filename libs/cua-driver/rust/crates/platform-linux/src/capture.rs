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
use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::{Format, ImageOrder, Setup, VisualClass, Visualtype};

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
        .output()
        .map_err(|e| anyhow!("failed to launch ImageMagick import: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let detail = stderr.trim().chars().take(512).collect::<String>();
        bail!("ImageMagick import exited {}: {detail}", out.status);
    }
    if out.stdout.is_empty() {
        bail!("ImageMagick import returned empty stdout");
    }
    Ok(out.stdout)
}

// ── shared pixel conversion ───────────────────────────────────────────────

#[derive(Clone)]
struct PixelCatalog {
    image_byte_order: ImageOrder,
    formats: Vec<Format>,
    visuals: Vec<Visualtype>,
}

#[derive(Clone, Copy)]
struct PackedLayout {
    byte_order: ImageOrder,
    bytes_per_pixel: usize,
    stride: usize,
    len: usize,
}

#[derive(Clone, Copy)]
struct PixelDecoder {
    layout: PackedLayout,
    red_mask: u32,
    green_mask: u32,
    blue_mask: u32,
}

impl PixelCatalog {
    fn from_setup(setup: &Setup) -> Self {
        Self {
            image_byte_order: setup.image_byte_order,
            formats: setup.pixmap_formats.clone(),
            visuals: setup
                .roots
                .iter()
                .flat_map(|screen| screen.allowed_depths.iter())
                .flat_map(|depth| depth.visuals.iter().copied())
                .collect(),
        }
    }

    fn packed_layout(&self, w: u32, h: u32, depth: u8) -> Result<PackedLayout> {
        checked_capture_dimensions(w, h)?;
        let format = self
            .formats
            .iter()
            .find(|format| format.depth == depth)
            .ok_or_else(|| anyhow!("no X11 pixmap format for depth {depth}"))?;
        if !matches!(format.bits_per_pixel, 16 | 24 | 32) {
            bail!(
                "unsupported X11 bits-per-pixel {} for depth {depth}",
                format.bits_per_pixel
            );
        }
        if !matches!(format.scanline_pad, 8 | 16 | 32) {
            bail!(
                "unsupported X11 scanline pad {} for depth {depth}",
                format.scanline_pad
            );
        }

        let row_bits = u64::from(w)
            .checked_mul(u64::from(format.bits_per_pixel))
            .ok_or_else(|| anyhow!("window geometry {w}x{h} overflows row length"))?;
        let pad = u64::from(format.scanline_pad);
        let stride_bits = row_bits
            .checked_add(pad - 1)
            .map(|bits| (bits / pad) * pad)
            .ok_or_else(|| anyhow!("window geometry {w}x{h} overflows padded row length"))?;
        let stride = usize::try_from(stride_bits / 8)
            .map_err(|_| anyhow!("window geometry {w}x{h} stride does not fit usize"))?;
        let len = stride
            .checked_mul(h as usize)
            .ok_or_else(|| anyhow!("window geometry {w}x{h} overflows byte length"))?;
        if len == 0 || len > MAX_CAPTURE_BYTES {
            bail!("window capture size {len} out of bounds (max {MAX_CAPTURE_BYTES})");
        }
        Ok(PackedLayout {
            byte_order: self.image_byte_order,
            bytes_per_pixel: usize::from(format.bits_per_pixel / 8),
            stride,
            len,
        })
    }

    fn decoder(&self, w: u32, h: u32, depth: u8, visual_id: u32) -> Result<PixelDecoder> {
        let layout = self.packed_layout(w, h, depth)?;
        let visual = self
            .visuals
            .iter()
            .find(|visual| visual.visual_id == visual_id)
            .ok_or_else(|| anyhow!("unknown X11 visual 0x{visual_id:x} for depth {depth}"))?;
        if visual.class != VisualClass::TRUE_COLOR {
            bail!(
                "unsupported X11 visual class {:?} for visual 0x{visual_id:x}",
                visual.class
            );
        }
        for (name, mask) in [
            ("red", visual.red_mask),
            ("green", visual.green_mask),
            ("blue", visual.blue_mask),
        ] {
            validate_component_mask(name, mask)?;
        }
        if visual.red_mask & visual.green_mask != 0
            || visual.red_mask & visual.blue_mask != 0
            || visual.green_mask & visual.blue_mask != 0
        {
            bail!("overlapping RGB masks for X11 visual 0x{visual_id:x}");
        }
        Ok(PixelDecoder {
            layout,
            red_mask: visual.red_mask,
            green_mask: visual.green_mask,
            blue_mask: visual.blue_mask,
        })
    }
}

fn checked_capture_dimensions(w: u32, h: u32) -> Result<()> {
    if w == 0 || h == 0 {
        bail!("window geometry is 0x0");
    }
    if w > MAX_CAPTURE_DIM || h > MAX_CAPTURE_DIM {
        bail!("window geometry {w}x{h} exceeds max {MAX_CAPTURE_DIM}px edge");
    }
    Ok(())
}

fn validate_component_mask(name: &str, mask: u32) -> Result<()> {
    if mask == 0 {
        bail!("X11 {name} mask is zero");
    }
    let normalized = mask >> mask.trailing_zeros();
    if normalized & normalized.wrapping_add(1) != 0 {
        bail!("X11 {name} mask 0x{mask:x} is not contiguous");
    }
    Ok(())
}

fn component_to_u8(pixel: u32, mask: u32) -> u8 {
    let shifted_mask = mask >> mask.trailing_zeros();
    let value = (pixel & mask) >> mask.trailing_zeros();
    ((u64::from(value) * 255 + u64::from(shifted_mask) / 2) / u64::from(shifted_mask)) as u8
}

fn packed_zpixmap_to_png(data: &[u8], w: u32, h: u32, decoder: PixelDecoder) -> Result<Vec<u8>> {
    if data.len() != decoder.layout.len {
        bail!(
            "pixel buffer length {} != expected {} ({}x{})",
            data.len(),
            decoder.layout.len,
            w,
            h
        );
    }
    let rgba_len = (w as usize)
        .checked_mul(h as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| anyhow!("window geometry {w}x{h} overflows RGBA length"))?;
    let mut rgba = Vec::with_capacity(rgba_len);
    for y in 0..h as usize {
        let row = &data[y * decoder.layout.stride..(y + 1) * decoder.layout.stride];
        for x in 0..w as usize {
            let start = x * decoder.layout.bytes_per_pixel;
            let bytes = &row[start..start + decoder.layout.bytes_per_pixel];
            let pixel = if decoder.layout.byte_order == ImageOrder::LSB_FIRST {
                bytes.iter().enumerate().fold(0u32, |value, (index, byte)| {
                    value | (u32::from(*byte) << (index * 8))
                })
            } else if decoder.layout.byte_order == ImageOrder::MSB_FIRST {
                bytes
                    .iter()
                    .fold(0u32, |value, byte| (value << 8) | u32::from(*byte))
            } else {
                bail!("unsupported X11 image byte order");
            };
            rgba.extend_from_slice(&[
                component_to_u8(pixel, decoder.red_mask),
                component_to_u8(pixel, decoder.green_mask),
                component_to_u8(pixel, decoder.blue_mask),
                255,
            ]);
        }
    }
    cua_driver_core::image_utils::encode_rgba_to_png(&rgba, w, h)
}

fn xid_to_window(xid: u64) -> Result<u32> {
    u32::try_from(xid).map_err(|_| anyhow!("X11 window id {xid} does not fit u32"))
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
    pixels: PixelCatalog,
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

        let (conn, screen_num) = x11rb::rust_connection::RustConnection::connect(Some(&display))
            .map_err(|e| anyhow!("X11 connect for SHM: {e}"))?;
        let pixels = PixelCatalog::from_setup(conn.setup());

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
            pixels,
            screen_num,
            buffer: None,
        })
    }

    fn detach_buffer_best_effort(&mut self) {
        let _ = self.detach_buffer();
    }

    fn detach_buffer(&mut self) -> Result<()> {
        use x11rb::protocol::shm::ConnectionExt as _;
        if let Some(buf) = self.buffer.take() {
            let seg = buf.seg;
            let detach = self
                .conn
                .shm_detach(seg)
                .map_err(|e| anyhow!("shm_detach request for segment 0x{seg:x}: {e}"))?
                .check()
                .map_err(|e| anyhow!("shm_detach reply for segment 0x{seg:x}: {e}"));
            // Drop the mapping even when the connection is already broken.
            drop(buf);
            detach?;
        }
        Ok(())
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
        self.detach_buffer()?;

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

        let window = xid_to_window(xid)?;
        let geom = self
            .conn
            .get_geometry(window)
            .map_err(|e| anyhow!("get_geometry request: {e}"))?
            .reply()
            .map_err(|e| anyhow!("get_geometry reply: {e}"))?;
        let w = u32::from(geom.width);
        let h = u32::from(geom.height);
        let layout = self.pixels.packed_layout(w, h, geom.depth)?;
        let need = layout.len;

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
            16 | 24 | 32 => {}
            other => bail!("Unsupported depth: {other}"),
        }

        let size = reply.size as usize;
        if size != need {
            bail!(
                "shm_get_image size {size} != expected {need} ({}x{}, depth {}, stride {})",
                w,
                h,
                reply.depth,
                layout.stride
            );
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
            decoder: self.pixels.decoder(w, h, reply.depth, reply.visual)?,
        })
    }
}

struct RawFrame {
    data: Vec<u8>,
    w: u32,
    h: u32,
    decoder: PixelDecoder,
}

fn xshm_state() -> &'static Mutex<XShmState> {
    static STATE: OnceLock<Mutex<XShmState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(XShmState::Uninit))
}

fn capture_via_xshm(xid: u64) -> Result<Vec<u8>> {
    let display = current_display();
    let mut guard = lock_mutex(xshm_state());
    let frame = capture_raw_via_xshm_state(&mut guard, &display, xid)?;

    // Release the session mutex before pixel conversion and PNG encode.
    drop(guard);
    packed_zpixmap_to_png(&frame.data, frame.w, frame.h, frame.decoder)
}

fn capture_raw_via_xshm_state(guard: &mut XShmState, display: &str, xid: u64) -> Result<RawFrame> {
    // Init backoff for this DISPLAY — no per-frame reprobe while active.
    // Expired same-DISPLAY / different-DISPLAY Unsupported → Uninit (probeable).
    if let Err(reason) = guard.consume_init_backoff(display, Instant::now()) {
        bail!("MIT-SHM disabled for DISPLAY={display}: {reason}");
    }

    // Ensure Ready session for current DISPLAY (connect only on init/recovery).
    match ensure_xshm_ready(guard, display) {
        Ok(()) => {}
        Err(e) => {
            let reason = format!("{e:#}");
            *guard = XShmState::unsupported_after_init_failure(
                display.to_string(),
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

    match first {
        Ok(frame) => Ok(frame),
        Err(first_err) => {
            // Cached session failure: discard/detach and reconnect+retry once.
            *guard = XShmState::Uninit;
            let retry_init = ensure_xshm_ready(guard, display);
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
                Ok(frame) => Ok(frame),
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
    }
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
    pixels: PixelCatalog,
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
    packed_zpixmap_to_png(&frame.data, frame.w, frame.h, frame.decoder)
}

fn ensure_xgetimage_ready(guard: &mut Option<XGetImageSession>, display: &str) -> Result<()> {
    if let Some(session) = guard.as_ref() {
        if session.display == display {
            return Ok(());
        }
    }
    *guard = None;
    *guard = Some(XGetImageSession::connect(display.to_string())?);
    Ok(())
}

impl XGetImageSession {
    fn connect(display: String) -> Result<Self> {
        let (conn, _screen) = x11rb::rust_connection::RustConnection::connect(Some(&display))
            .map_err(|e| anyhow!("{e}"))?;
        let pixels = PixelCatalog::from_setup(conn.setup());
        Ok(Self {
            display,
            conn,
            pixels,
        })
    }

    fn capture_raw(&mut self, xid: u64) -> Result<RawFrame> {
        use x11rb::protocol::xproto::{ConnectionExt as _, ImageFormat};

        let window = xid_to_window(xid)?;
        let geom = self
            .conn
            .get_geometry(window)
            .map_err(|e| anyhow!("get_geometry request: {e}"))?
            .reply()
            .map_err(|e| anyhow!("get_geometry reply: {e}"))?;
        let w = u32::from(geom.width);
        let h = u32::from(geom.height);
        let layout = self.pixels.packed_layout(w, h, geom.depth)?;
        let need = layout.len;

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
            16 | 24 | 32 => {}
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
            decoder: self.pixels.decoder(w, h, img.depth, img.visual)?,
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

    fn decode_png_rgba(png: &[u8]) -> Vec<u8> {
        image::load_from_memory_with_format(png, image::ImageFormat::Png)
            .expect("decode PNG")
            .to_rgba8()
            .into_raw()
    }

    #[test]
    fn zpixmap_decoder_honors_little_endian_24bpp_stride_and_masks() {
        let decoder = PixelDecoder {
            layout: PackedLayout {
                byte_order: ImageOrder::LSB_FIRST,
                bytes_per_pixel: 3,
                stride: 12,
                len: 24,
            },
            red_mask: 0x00ff_0000,
            green_mask: 0x0000_ff00,
            blue_mask: 0x0000_00ff,
        };
        // Width 3 at 24bpp with 32-bit scanline padding: 9 payload bytes +
        // 3 ignored padding bytes per row. Colors are deliberately varied.
        let packed = [
            0x33, 0x22, 0x11, 0x66, 0x55, 0x44, 0x99, 0x88, 0x77, 0xde, 0xad, 0xbe, 0xcc, 0xbb,
            0xaa, 0xff, 0xee, 0xdd, 0x03, 0x02, 0x01, 0xef, 0xca, 0xfe,
        ];
        let png = packed_zpixmap_to_png(&packed, 3, 2, decoder).expect("decode little-endian");
        assert_eq!(
            decode_png_rgba(&png),
            vec![
                0x11, 0x22, 0x33, 0xff, 0x44, 0x55, 0x66, 0xff, 0x77, 0x88, 0x99, 0xff, 0xaa, 0xbb,
                0xcc, 0xff, 0xdd, 0xee, 0xff, 0xff, 0x01, 0x02, 0x03, 0xff,
            ]
        );
    }

    #[test]
    fn zpixmap_decoder_honors_big_endian_rgb565_masks() {
        let decoder = PixelDecoder {
            layout: PackedLayout {
                byte_order: ImageOrder::MSB_FIRST,
                bytes_per_pixel: 2,
                stride: 4,
                len: 4,
            },
            red_mask: 0xf800,
            green_mask: 0x07e0,
            blue_mask: 0x001f,
        };
        let packed = [0xf8, 0x00, 0x07, 0xe0];
        let png = packed_zpixmap_to_png(&packed, 2, 1, decoder).expect("decode big-endian");
        assert_eq!(decode_png_rgba(&png), vec![255, 0, 0, 255, 0, 255, 0, 255]);
    }

    #[test]
    fn pixel_catalog_uses_server_bpp_padding_and_rejects_bad_masks() {
        let valid = Visualtype {
            visual_id: 0x21,
            class: VisualClass::TRUE_COLOR,
            bits_per_rgb_value: 8,
            colormap_entries: 256,
            red_mask: 0x00ff_0000,
            green_mask: 0x0000_ff00,
            blue_mask: 0x0000_00ff,
        };
        let mut catalog = PixelCatalog {
            image_byte_order: ImageOrder::LSB_FIRST,
            formats: vec![Format {
                depth: 24,
                bits_per_pixel: 24,
                scanline_pad: 32,
            }],
            visuals: vec![valid],
        };
        let decoder = catalog.decoder(3, 2, 24, 0x21).expect("valid decoder");
        assert_eq!(decoder.layout.bytes_per_pixel, 3);
        assert_eq!(decoder.layout.stride, 12);
        assert_eq!(decoder.layout.len, 24);

        catalog.visuals[0].red_mask = 0x00f0_f000;
        let err = catalog
            .decoder(3, 2, 24, 0x21)
            .err()
            .expect("non-contiguous mask must fail");
        assert!(format!("{err:#}").contains("not contiguous"));
    }

    #[test]
    fn xid_narrowing_rejects_values_above_x11_range() {
        assert_eq!(xid_to_window(u64::from(u32::MAX)).unwrap(), u32::MAX);
        let err = xid_to_window(u64::from(u32::MAX) + 1).expect_err("overflow must fail");
        assert!(format!("{err:#}").contains("does not fit u32"));
    }

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

    #[test]
    fn capture_cascade_preserves_every_backend_error() {
        let err = capture_window_with_backends(
            42,
            |_| Err(anyhow!("shm transport disconnected")),
            |_| Err(anyhow!("get-image bad drawable")),
            |_| Err(anyhow!("import executable unavailable")),
        )
        .expect_err("all failures must be returned");
        let message = format!("{err:#}");
        assert!(message.contains("shm transport disconnected"));
        assert!(message.contains("get-image bad drawable"));
        assert!(message.contains("import executable unavailable"));
    }

    #[test]
    fn empty_fast_paths_fall_through_to_imagemagick() {
        let png = vec![1, 2, 3];
        let result = capture_window_with_backends(
            42,
            |_| Ok(Vec::new()),
            |_| Ok(Vec::new()),
            |_| Ok(png.clone()),
        )
        .expect("fallback succeeds");
        assert_eq!(result, png);
    }

    struct LiveFixture {
        conn: x11rb::rust_connection::RustConnection,
        window: u32,
    }

    struct XvfbServer {
        child: Option<std::process::Child>,
    }

    impl XvfbServer {
        fn start(display: &str) -> Self {
            let mut child = Command::new("Xvfb")
                .args([
                    display,
                    "-screen",
                    "0",
                    "640x480x24",
                    "-ac",
                    "-nolisten",
                    "tcp",
                ])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .expect("start restart-test Xvfb");
            for _ in 0..100 {
                if x11rb::rust_connection::RustConnection::connect(Some(display)).is_ok() {
                    return Self { child: Some(child) };
                }
                if let Some(status) = child.try_wait().expect("poll restart-test Xvfb") {
                    panic!("restart-test Xvfb exited before ready: {status}");
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            let _ = child.kill();
            let _ = child.wait();
            panic!("restart-test Xvfb did not become ready on {display}");
        }

        fn stop(&mut self) {
            if let Some(mut child) = self.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    impl Drop for XvfbServer {
        fn drop(&mut self) {
            self.stop();
        }
    }

    fn create_live_fixture(display: &str, width: u16, height: u16) -> LiveFixture {
        use x11rb::protocol::xproto::{ConnectionExt as _, CreateWindowAux, WindowClass};
        use x11rb::rust_connection::RustConnection;

        let (conn, screen_num) = RustConnection::connect(Some(display)).expect("connect fixture");
        let screen = &conn.setup().roots[screen_num];
        let window = conn.generate_id().expect("generate window id");
        conn.create_window(
            screen.root_depth,
            window,
            screen.root,
            0,
            0,
            width,
            height,
            0,
            WindowClass::INPUT_OUTPUT,
            screen.root_visual,
            &CreateWindowAux::new().background_pixel(0x0011_2233),
        )
        .expect("create_window request")
        .check()
        .expect("create_window sync check");
        conn.map_window(window)
            .expect("map_window request")
            .check()
            .expect("map_window sync check");
        let fixture = LiveFixture { conn, window };
        paint_live_fixture(&fixture, width, height);
        fixture
    }

    fn paint_live_fixture(fixture: &LiveFixture, width: u16, height: u16) {
        use x11rb::protocol::xproto::{ConnectionExt as _, CreateGCAux, Rectangle};

        let split1 = width / 3;
        let split2 = width.saturating_mul(2) / 3;
        for (pixel, rectangle) in [
            (
                0x0017_5b_a8,
                Rectangle {
                    x: 0,
                    y: 0,
                    width: split1,
                    height,
                },
            ),
            (
                0x00c4_3d_52,
                Rectangle {
                    x: split1 as i16,
                    y: 0,
                    width: split2 - split1,
                    height,
                },
            ),
            (
                0x002d_b8_71,
                Rectangle {
                    x: split2 as i16,
                    y: 0,
                    width: width - split2,
                    height,
                },
            ),
        ] {
            let gc = fixture.conn.generate_id().expect("generate GC id");
            fixture
                .conn
                .create_gc(gc, fixture.window, &CreateGCAux::new().foreground(pixel))
                .expect("create_gc request")
                .check()
                .expect("create_gc sync check");
            fixture
                .conn
                .poly_fill_rectangle(fixture.window, gc, &[rectangle])
                .expect("poly_fill_rectangle request")
                .check()
                .expect("poly_fill_rectangle sync check");
            fixture.conn.free_gc(gc).expect("free_gc request");
        }
        fixture.conn.flush().expect("flush fixture paint");
        fixture
            .conn
            .get_input_focus()
            .expect("fixture sync request")
            .reply()
            .expect("fixture sync reply");
    }

    fn raw_frame_png(frame: RawFrame) -> Vec<u8> {
        packed_zpixmap_to_png(&frame.data, frame.w, frame.h, frame.decoder)
            .expect("encode raw frame")
    }

    fn percentile_micros(samples: &mut [u128], percentile: usize) -> u128 {
        samples.sort_unstable();
        samples[(samples.len() - 1) * percentile / 100]
    }

    /// Live correctness and bounded-performance evidence against three Xvfb
    /// servers. The fixture uses nontrivial colors and a non-power-of-two row
    /// width, then proves MIT-SHM and XGetImage decode to identical pixels,
    /// survive resize and DISPLAY switches, reuse one warm segment, and detach
    /// it before replacement. CreateSegment is FD-backed MIT-SHM 1.2, so no
    /// SysV `IPC_RMID` lifecycle exists in this implementation.
    #[test]
    #[ignore = "requires two live X11 servers with MIT-SHM 1.2"]
    fn live_xshm_matches_xgetimage_across_resize_reconnect_and_repetition() {
        use x11rb::connection::Connection;
        use x11rb::protocol::shm::ConnectionExt as _;
        use x11rb::protocol::xproto::{ConfigureWindowAux, ConnectionExt as _, ImageFormat};

        const W1: u16 = 67;
        const H1: u16 = 43;
        const W2: u16 = 131;
        const H2: u16 = 79;
        const REPEATS: usize = 40;
        const IMPORT_REPEATS: usize = 10;

        let display = std::env::var("DISPLAY").expect("DISPLAY must be set");
        let second_display =
            std::env::var("CUA_X11_SECOND_DISPLAY").expect("CUA_X11_SECOND_DISPLAY must be set");
        let restart_display =
            std::env::var("CUA_X11_RESTART_DISPLAY").expect("CUA_X11_RESTART_DISPLAY must be set");
        assert_ne!(
            display, second_display,
            "tests require distinct DISPLAY values"
        );
        assert_ne!(display, restart_display, "restart DISPLAY must be distinct");
        assert_ne!(
            second_display, restart_display,
            "restart DISPLAY must be distinct"
        );
        let fixture = create_live_fixture(&display, W1, H1);
        let second_fixture = create_live_fixture(&second_display, W1, H1);

        let mut xshm = XShmSession::connect(display.clone()).expect("connect XShm session");
        let ver = xshm
            .conn
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

        let mut xget = XGetImageSession::connect(display.clone()).expect("connect XGetImage");
        let shm_png = raw_frame_png(
            xshm.capture_raw(u64::from(fixture.window))
                .expect("initial XShm capture"),
        );
        let xget_png = raw_frame_png(
            xget.capture_raw(u64::from(fixture.window))
                .expect("initial XGetImage capture"),
        );
        let import_png = capture_via_import(u64::from(fixture.window))
            .expect("initial ImageMagick capture oracle");
        let shm_rgba = decode_png_rgba(&shm_png);
        assert_eq!(shm_rgba, decode_png_rgba(&xget_png));
        assert_eq!(shm_rgba, decode_png_rgba(&import_png));
        assert_eq!(&shm_rgba[0..4], &[0x17, 0x5b, 0xa8, 0xff]);
        let middle = ((usize::from(H1 / 2) * usize::from(W1) + usize::from(W1 / 2)) * 4)
            ..((usize::from(H1 / 2) * usize::from(W1) + usize::from(W1 / 2)) * 4 + 4);
        assert_eq!(&shm_rgba[middle], &[0xc4, 0x3d, 0x52, 0xff]);

        let warm_seg = xshm.buffer.as_ref().expect("warm SHM buffer").seg;
        let mut shm_micros = Vec::with_capacity(REPEATS);
        let mut xget_micros = Vec::with_capacity(REPEATS);
        let mut import_micros = Vec::with_capacity(IMPORT_REPEATS);
        for _ in 0..REPEATS {
            let started = Instant::now();
            let frame = xshm
                .capture_raw(u64::from(fixture.window))
                .expect("repeated XShm capture");
            let _ = raw_frame_png(frame);
            shm_micros.push(started.elapsed().as_micros());
            assert_eq!(xshm.buffer.as_ref().expect("reused buffer").seg, warm_seg);

            let started = Instant::now();
            let frame = xget
                .capture_raw(u64::from(fixture.window))
                .expect("repeated XGetImage capture");
            let _ = raw_frame_png(frame);
            xget_micros.push(started.elapsed().as_micros());
        }
        for _ in 0..IMPORT_REPEATS {
            let started = Instant::now();
            let _ = capture_via_import(u64::from(fixture.window))
                .expect("repeated ImageMagick capture");
            import_micros.push(started.elapsed().as_micros());
        }

        fixture
            .conn
            .configure_window(
                fixture.window,
                &ConfigureWindowAux::new()
                    .width(u32::from(W2))
                    .height(u32::from(H2)),
            )
            .expect("resize request")
            .check()
            .expect("resize reply");
        paint_live_fixture(&fixture, W2, H2);
        let resized_shm = raw_frame_png(
            xshm.capture_raw(u64::from(fixture.window))
                .expect("resized XShm capture"),
        );
        let resized_xget = raw_frame_png(
            xget.capture_raw(u64::from(fixture.window))
                .expect("resized XGetImage capture"),
        );
        let resized_import = capture_via_import(u64::from(fixture.window))
            .expect("resized ImageMagick capture oracle");
        assert_eq!(
            decode_png_rgba(&resized_shm),
            decode_png_rgba(&resized_xget)
        );
        assert_eq!(
            decode_png_rgba(&resized_shm),
            decode_png_rgba(&resized_import)
        );
        assert_eq!(
            cua_driver_core::image_utils::png_dimensions(&resized_shm).unwrap(),
            (u32::from(W2), u32::from(H2))
        );
        let resized_seg = xshm.buffer.as_ref().expect("resized SHM buffer").seg;
        assert_ne!(resized_seg, warm_seg, "growth must replace the segment");

        // Explicit detach is synchronous. Reusing the old XID must be rejected
        // by the server while the connection remains healthy.
        xshm.detach_buffer().expect("detach resized segment");
        let detached = xshm
            .conn
            .shm_get_image(
                fixture.window,
                0,
                0,
                W2,
                H2,
                !0u32,
                u8::from(ImageFormat::Z_PIXMAP),
                resized_seg,
                0,
            )
            .expect("detached-segment request")
            .reply();
        assert!(detached.is_err(), "server accepted a detached SHM segment");
        xshm.capture_raw(u64::from(fixture.window))
            .expect("capture allocates replacement after detach");

        let mut shm_state = XShmState::Ready(xshm);
        ensure_xshm_ready(&mut shm_state, &second_display).expect("switch XShm DISPLAY");
        let second_shm = match &mut shm_state {
            XShmState::Ready(session) => {
                assert_eq!(session.display, second_display);
                raw_frame_png(
                    session
                        .capture_raw(u64::from(second_fixture.window))
                        .expect("capture on second XShm DISPLAY"),
                )
            }
            _ => panic!("XShm state not ready after DISPLAY switch"),
        };
        let mut xget_state = Some(xget);
        ensure_xgetimage_ready(&mut xget_state, &second_display).expect("switch XGetImage DISPLAY");
        let second_xget = raw_frame_png(
            xget_state
                .as_mut()
                .expect("second XGetImage session")
                .capture_raw(u64::from(second_fixture.window))
                .expect("capture on second XGetImage DISPLAY"),
        );
        assert_eq!(decode_png_rgba(&second_shm), decode_png_rgba(&second_xget));

        // Prove that a cached connection failure retries once, returns the
        // state to Uninit when the server remains unavailable, then recovers
        // after a server restart at the exact same DISPLAY value.
        let mut restart_server = XvfbServer::start(&restart_display);
        let restart_fixture = create_live_fixture(&restart_display, W1, H1);
        let mut restart_state = XShmState::Uninit;
        capture_raw_via_xshm_state(
            &mut restart_state,
            &restart_display,
            u64::from(restart_fixture.window),
        )
        .expect("initial capture on restart DISPLAY");
        restart_server.stop();
        let disconnected = match capture_raw_via_xshm_state(
            &mut restart_state,
            &restart_display,
            u64::from(restart_fixture.window),
        ) {
            Ok(_) => panic!("capture must fail while restart DISPLAY is down"),
            Err(error) => error,
        };
        assert!(
            format!("{disconnected:#}").contains("capture failed after reconnect"),
            "unexpected disconnect error: {disconnected:#}"
        );
        assert!(matches!(restart_state, XShmState::Uninit));
        drop(restart_fixture);

        restart_server = XvfbServer::start(&restart_display);
        let restarted_fixture = create_live_fixture(&restart_display, W1, H1);
        let restarted_shm = raw_frame_png(
            capture_raw_via_xshm_state(
                &mut restart_state,
                &restart_display,
                u64::from(restarted_fixture.window),
            )
            .expect("capture after same-DISPLAY server restart"),
        );
        let mut restarted_xget = XGetImageSession::connect(restart_display.clone())
            .expect("connect restarted XGetImage");
        let restarted_xget = raw_frame_png(
            restarted_xget
                .capture_raw(u64::from(restarted_fixture.window))
                .expect("XGetImage capture after server restart"),
        );
        assert_eq!(
            decode_png_rgba(&restarted_shm),
            decode_png_rgba(&restarted_xget)
        );
        drop(restart_state);
        drop(restarted_fixture);
        restart_server.stop();

        let shm_p50 = percentile_micros(&mut shm_micros, 50);
        let shm_p95 = percentile_micros(&mut shm_micros, 95);
        let xget_p50 = percentile_micros(&mut xget_micros, 50);
        let xget_p95 = percentile_micros(&mut xget_micros, 95);
        let import_p50 = percentile_micros(&mut import_micros, 50);
        let import_p95 = percentile_micros(&mut import_micros, 95);
        assert!(
            shm_p95.saturating_mul(2) < import_p95,
            "MIT-SHM p95 ({shm_p95}us) must be less than half the ImageMagick p95 ({import_p95}us)"
        );
        println!(
            "CAPTURE_EVIDENCE {{\"display\":\"{}\",\"second_display\":\"{}\",\"restart_display\":\"{}\",\"fast_samples\":{},\"import_samples\":{},\"width\":{},\"height\":{},\"xshm_p50_us\":{},\"xshm_p95_us\":{},\"xgetimage_p50_us\":{},\"xgetimage_p95_us\":{},\"imagemagick_p50_us\":{},\"imagemagick_p95_us\":{},\"performance_bound\":\"xshm_p95_lt_half_imagemagick_p95\",\"pixel_equivalent\":true,\"imagemagick_equivalent\":true,\"resize_equivalent\":true,\"segment_reused\":true,\"detach_verified\":true,\"display_switch_verified\":true,\"server_restart_verified\":true}}",
            display,
            second_display,
            restart_display,
            REPEATS,
            IMPORT_REPEATS,
            W1,
            H1,
            shm_p50,
            shm_p95,
            xget_p50,
            xget_p95,
            import_p50,
            import_p95
        );

        let _ = fixture.conn.destroy_window(fixture.window);
        let _ = fixture.conn.flush();
        let _ = second_fixture.conn.destroy_window(second_fixture.window);
        let _ = second_fixture.conn.flush();
    }
}
