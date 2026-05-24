//! Linux agent-cursor overlay — X11 RGBA override-redirect window.
//!
//! Architecture:
//! - Creates an override-redirect (non-reparented) X11 window with 32-bit ARGB visual
//!   from XComposite.  The window covers the full display area.
//! - A background thread renders frames at ~60 Hz using tiny-skia and XShmPutImage
//!   (or XPutImage fallback) with XRender ARGB compositing.
//! - Mouse events pass through via `XShapeSelectInput(ShapeInput, empty-region)`.
//! - Z-ordering: `XRaiseWindow` every 80ms to stay above normal windows.
//! - Wayland: when WAYLAND_DISPLAY is set but DISPLAY is also available (XWayland),
//!   the X11 path is used.  Pure Wayland support is a TODO.
//!
//! ## Cross-platform note (2026-05 dedup audit)
//!
//! Animation state + render pipeline live in `cursor_overlay::render_state`
//! (`RenderStateCore`, `tick_motion`, `apply_command_base`, `render_frame`).
//! What stays here is the X11 window plumbing: connection setup,
//! override-redirect visual, ShapeInput passthrough, and the XPutImage paint.

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use cursor_overlay::{CursorConfig, OverlayCommand, RenderStateCore};

// ── Global channel ────────────────────────────────────────────────────────

static CMD_TX: OnceLock<std::sync::mpsc::SyncSender<OverlayCommand>> = OnceLock::new();
static CMD_RX_CELL: Mutex<Option<std::sync::mpsc::Receiver<OverlayCommand>>> = Mutex::new(None);
static RENDER: Mutex<Option<RenderState>> = Mutex::new(None);

pub fn init(cfg: CursorConfig) {
    let (tx, rx) = std::sync::mpsc::sync_channel(4096);
    let _ = CMD_TX.set(tx);
    *CMD_RX_CELL.lock().unwrap() = Some(rx);
    *RENDER.lock().unwrap() = Some(RenderState::new(cfg));
}

pub fn send_command(cmd: OverlayCommand) {
    if let Some(tx) = CMD_TX.get() {
        let _ = tx.try_send(cmd);
    }
}

/// Spawn the overlay on a dedicated thread.  Non-blocking.
pub fn run_on_thread() {
    let rx = match CMD_RX_CELL.lock().unwrap().take() {
        Some(r) => r,
        None => return,
    };

    let cfg = {
        let guard = RENDER.lock().unwrap();
        match &*guard {
            Some(rs) => rs.core.cfg.clone(),
            None => return,
        }
    };

    if !cfg.enabled {
        return;
    }

    std::thread::Builder::new()
        .name("cua-overlay-x11".into())
        .spawn(move || {
            run_overlay_thread(cfg, rx);
        })
        .expect("spawn overlay thread");
}

// ── Animation state ───────────────────────────────────────────────────────
//
// The platform-agnostic fields + tick + apply_command + render pipeline live
// in `cursor_overlay::render_state` (2026-05 dedup audit). What stays here
// is the X11-specific screen dimensions.

struct RenderState {
    core: RenderStateCore,
    /// X11 screen dimensions in pixels (populated after XOpenDisplay).
    scr_w: u32,
    scr_h: u32,
}

impl RenderState {
    fn new(cfg: CursorConfig) -> Self {
        RenderState {
            core: RenderStateCore::new(cfg),
            scr_w: 1920,
            scr_h: 1080,
        }
    }

    fn tick(&mut self, dt: f64) {
        self.core.tick_motion(dt);
    }

    fn apply_command(&mut self, cmd: OverlayCommand) {
        // Linux uses the non-sentinel-snap behaviour for both MoveTo and
        // ClickPulse: every command updates `self.pos` unconditionally.
        // Custom-shape / gradient / focus-rect commands are not rendered on
        // Linux at present; `apply_command_base` consumes SetShape +
        // SetGradient and returns false for ShowFocusRect — both cases drop
        // the visual update silently so callers don't see an error.
        let _ = self.core.apply_command_base(cmd, false, false);
    }
}

// ── X11 thread ────────────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
fn run_overlay_thread(cfg: CursorConfig, rx: std::sync::mpsc::Receiver<OverlayCommand>) {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::*;
    use x11rb::protocol::xproto::ConnectionExt as _;
    use x11rb::protocol::shape::*;
    use x11rb::protocol::shape::ConnectionExt as _;
    use x11rb::wrapper::ConnectionExt as _;
    use x11rb::COPY_FROM_PARENT;

    // Connect to X11.
    let (conn, screen_num) = match x11rb::connect(None) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("X11 overlay: cannot connect to display: {e}");
            return;
        }
    };

    let screen = &conn.setup().roots[screen_num];
    let root   = screen.root;
    let scr_w  = screen.width_in_pixels as u32;
    let scr_h  = screen.height_in_pixels as u32;

    // Update render state with screen size.
    {
        let mut guard = RENDER.lock().unwrap();
        if let Some(rs) = guard.as_mut() {
            rs.scr_w = scr_w;
            rs.scr_h = scr_h;
        }
    }

    // Find 32-bit ARGB visual for compositing.
    // Falls back to the default visual if XComposite 32-bit isn't available.
    let (visual_id, depth, colormap) = find_argb_visual(&conn, screen)
        .unwrap_or((screen.root_visual, screen.root_depth, screen.default_colormap));

    // Create a matching colormap if we got a non-default visual.
    let colormap = if visual_id != screen.root_visual {
        let cm = conn.generate_id().unwrap();
        conn.create_colormap(ColormapAlloc::NONE, cm, root, visual_id).ok();
        cm
    } else {
        colormap
    };

    // Create the overlay window.
    let win = conn.generate_id().unwrap();
    let win_aux = CreateWindowAux::new()
        .background_pixel(0)
        .border_pixel(0)
        .colormap(colormap)
        // Override-redirect = no window manager decoration, no focus.
        .override_redirect(1u32)
        // Input passthrough: do not receive button/key events.
        .event_mask(EventMask::NO_EVENT);

    conn.create_window(
        depth, win, root,
        0, 0, scr_w as u16, scr_h as u16,
        0,
        WindowClass::INPUT_OUTPUT,
        visual_id,
        &win_aux,
    ).ok();

    // Set window title (identifies our overlay, matches Windows convention).
    // `Cua.` namespace mirrors the Windows class-name + install-path
    // convention; was `TropeCUA.` (leaked codename from an early C# ref).
    let title = format!("Cua.AgentCursorOverlay.{}", cfg.cursor_id);
    conn.change_property8(
        PropMode::REPLACE, win,
        AtomEnum::WM_NAME,
        AtomEnum::STRING,
        title.as_bytes(),
    ).ok();

    // Make the window fully click-through using the Shape extension (empty input region).
    // This is the X11 equivalent of WS_EX_TRANSPARENT on Windows.
    let empty_region_pixmap = conn.generate_id().unwrap();
    conn.create_pixmap(1, empty_region_pixmap, root, 1, 1).ok();
    conn.shape_mask(
        x11rb::protocol::shape::SO::SET,
        x11rb::protocol::shape::SK::INPUT,
        win,
        0, 0,
        x11rb::NONE,
    ).ok();
    conn.free_pixmap(empty_region_pixmap).ok();

    conn.map_window(win).ok();
    conn.flush().ok();

    // Main render loop at ~60Hz.
    let frame_dur = Duration::from_millis(16);
    let mut last_tick = Instant::now();
    let mut last_ztick = Instant::now();

    loop {
        let now = Instant::now();
        let dt  = now.duration_since(last_tick).as_secs_f64().min(0.05);
        last_tick = now;

        // Drain commands and tick.
        {
            let mut guard = RENDER.lock().unwrap();
            if let Some(rs) = guard.as_mut() {
                while let Ok(cmd) = rx.try_recv() {
                    rs.apply_command(cmd);
                }
                rs.tick(dt);
            }
        }

        // Render and paint.
        let pixmap = {
            let guard = RENDER.lock().unwrap();
            guard.as_ref().map(|rs| {
                cursor_overlay::render_frame(
                    &rs.core,
                    rs.scr_w.max(1),
                    rs.scr_h.max(1),
                    0.0, 0.0, // Linux uses screen-local coords (no origin offset)
                    None,     // focus-rect is macOS-only
                )
            })
        };

        if let Some(pm) = pixmap {
            paint_x11(&conn, win, scr_w, scr_h, depth, visual_id, &pm);
        }

        // Z-order maintenance every 80ms.
        if last_ztick.elapsed() >= Duration::from_millis(80) {
            last_ztick = Instant::now();
            let pinned_wid = {
                let guard = RENDER.lock().unwrap();
                guard.as_ref().and_then(|rs| rs.core.pinned_wid)
            };
            let aux = if let Some(target_xid) = pinned_wid {
                // Place overlay just above the pinned X11 window.
                ConfigureWindowAux::new()
                    .sibling(target_xid as u32)
                    .stack_mode(StackMode::ABOVE)
            } else {
                ConfigureWindowAux::new().stack_mode(StackMode::ABOVE)
            };
            conn.configure_window(win, &aux).ok();
            conn.flush().ok();
        }

        // Drain any X events (needed to avoid blocking).
        while let Ok(Some(_)) = conn.poll_for_event() {}

        let elapsed = Instant::now().duration_since(last_tick);
        if let Some(remaining) = frame_dur.checked_sub(elapsed) {
            std::thread::sleep(remaining);
        }
    }
}

#[cfg(target_os = "linux")]
fn find_argb_visual(
    conn: &impl x11rb::connection::Connection,
    screen: &x11rb::protocol::xproto::Screen,
) -> Option<(u32, u8, u32)> {
    use x11rb::protocol::xproto::VisualClass;
    // Walk all depth entries looking for a 32-bit ARGB visual.
    for depth_entry in &screen.allowed_depths {
        if depth_entry.depth != 32 { continue; }
        for visual in &depth_entry.visuals {
            if visual.class == VisualClass::TRUE_COLOR {
                return Some((visual.visual_id, 32, screen.default_colormap));
            }
        }
    }
    None
}

/// Blit a tiny-skia pixmap to the X11 window using XPutImage (ZPixmap).
/// The pixmap is premultiplied RGBA; X11 ARGB is premultiplied BGRA.
#[cfg(target_os = "linux")]
fn paint_x11(
    conn: &impl x11rb::connection::Connection,
    win: u32,
    w: u32, h: u32,
    depth: u8,
    _visual_id: u32,
    pm: &tiny_skia::Pixmap,
) {
    use x11rb::protocol::xproto::*;
    use x11rb::protocol::xproto::ConnectionExt as _;
    if pm.width() == 0 || pm.height() == 0 { return; }

    // Create a GC for the window if we don't have one.
    // (Simplified: we recreate it every frame which is safe but not optimal.)
    let gc_id = match conn.generate_id() {
        Ok(id) => id,
        Err(_) => return,
    };
    let gc_aux = CreateGCAux::new();
    if conn.create_gc(gc_id, win, &gc_aux).is_err() { return; }

    // Convert RGBA premult → BGRA premult for X11.
    let src = pm.data();
    let mut bgra: Vec<u8> = Vec::with_capacity(src.len());
    for chunk in src.chunks_exact(4) {
        bgra.push(chunk[2]); // B
        bgra.push(chunk[1]); // G
        bgra.push(chunk[0]); // R
        bgra.push(chunk[3]); // A
    }

    // XPutImage (ZPixmap).
    let _ = conn.put_image(
        ImageFormat::Z_PIXMAP,
        win,
        gc_id,
        w as u16, h as u16,
        0, 0,
        0,
        depth,
        &bgra,
    );

    conn.free_gc(gc_id).ok();
    conn.flush().ok();
}

#[cfg(not(target_os = "linux"))]
fn run_overlay_thread(_cfg: CursorConfig, _rx: std::sync::mpsc::Receiver<OverlayCommand>) {}
