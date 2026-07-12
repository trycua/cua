//! Linux agent-cursor overlay — X11 RGBA override-redirect window.
//!
//! Architecture:
//! - Creates an override-redirect (non-reparented) X11 window with 32-bit ARGB visual
//!   from XComposite.  The window covers the full display area.
//! - A background thread renders frames at ~60 Hz using tiny-skia and XShmPutImage
//!   (or XPutImage fallback) with XRender ARGB compositing.
//! - XShape clips both input and visible pixels. The visible shape follows the
//!   rendered alpha mask so bare X11 window managers do not show a black
//!   full-screen ARGB window when no compositor is present.
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

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
#[cfg(target_os = "linux")]
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
use cursor_overlay::ZOrderEnforcer;
use cursor_overlay::{
    CursorConfig, CursorKey, KeyedOverlayCommand, OverlayCommand, OverlayMsg, Palette,
    RenderStateCore,
};

// ── Global channel ────────────────────────────────────────────────────────

static CMD_TX: OnceLock<std::sync::mpsc::SyncSender<OverlayMsg>> = OnceLock::new();
static CMD_RX_CELL: Mutex<Option<std::sync::mpsc::Receiver<OverlayMsg>>> = Mutex::new(None);
static RENDER: Mutex<Option<RenderMap>> = Mutex::new(None);
static ARRIVAL_TX: Mutex<Option<HashMap<CursorKey, tokio::sync::oneshot::Sender<()>>>> =
    Mutex::new(None);

fn arrival_register(key: CursorKey, tx: tokio::sync::oneshot::Sender<()>) {
    let mut guard = ARRIVAL_TX.lock().unwrap();
    let map = guard.get_or_insert_with(HashMap::new);
    if let Some(old_tx) = map.insert(key, tx) {
        let _ = old_tx.send(());
    }
}

fn arrival_fire(key: &CursorKey) {
    if let Ok(mut guard) = ARRIVAL_TX.lock() {
        if let Some(map) = guard.as_mut() {
            if let Some(tx) = map.remove(key) {
                let _ = tx.send(());
            }
        }
    }
}

struct RenderMap {
    cursors: HashMap<CursorKey, RenderState>,
    scr_w: u32,
    scr_h: u32,
    template: CursorConfig,
    ended: HashSet<CursorKey>,
    last_active: Option<CursorKey>,
}

fn render_state_for_key(template: &CursorConfig, key: &str) -> RenderState {
    let mut rs = RenderState::new(template.clone());
    rs.core.palette = Palette::for_instance(key);
    rs
}

fn apply_msg(map: &mut RenderMap, msg: OverlayMsg) -> Option<CursorKey> {
    match msg {
        OverlayMsg::Remove(key) => {
            if key != "default" {
                map.cursors.remove(&key);
                if let Ok(mut guard) = ARRIVAL_TX.lock() {
                    if let Some(arrivals) = guard.as_mut() {
                        arrivals.remove(&key);
                    }
                }
                if map.last_active.as_deref() == Some(key.as_str()) {
                    map.last_active = None;
                }
                map.ended.insert(key);
            }
            None
        }
        OverlayMsg::Cmd(KeyedOverlayCommand { key, cmd }) => {
            if map.ended.contains(&key) {
                return None;
            }
            let template = map.template.clone();
            let k = key.clone();
            let rs = map
                .cursors
                .entry(key)
                .or_insert_with(|| render_state_for_key(&template, &k));
            rs.apply_command(cmd);
            Some(k)
        }
    }
}

pub fn init(cfg: CursorConfig) {
    let (tx, rx) = std::sync::mpsc::sync_channel(4096);
    let _ = CMD_TX.set(tx);
    *CMD_RX_CELL.lock().unwrap() = Some(rx);
    *ARRIVAL_TX.lock().unwrap() = Some(HashMap::new());
    let mut cursors = HashMap::new();
    cursors.insert("default".to_owned(), RenderState::new(cfg.clone()));
    *RENDER.lock().unwrap() = Some(RenderMap {
        cursors,
        scr_w: 1920,
        scr_h: 1080,
        template: cfg,
        ended: HashSet::new(),
        last_active: None,
    });
}

pub fn send_command(cmd: OverlayCommand) {
    send_command_for("default".to_owned(), cmd);
}

pub fn send_command_for(key: CursorKey, cmd: OverlayCommand) {
    if key.is_empty() {
        return;
    }
    let msg = OverlayMsg::Cmd(KeyedOverlayCommand {
        key: key.clone(),
        cmd: cmd.clone(),
    });
    if let Some(tx) = CMD_TX.get() {
        let _ = tx.try_send(msg.clone());
    }
    // Also forward to the native-Wayland layer-shell overlay when Wayland
    // is opted in. The wayland overlay's `forward` is a no-op when its
    // owner thread isn't started yet (which is the normal X11-only case).
    #[cfg(target_os = "linux")]
    {
        if crate::wayland::is_wayland() {
            if crate::wayland::shell_helper::available() {
                // GNOME has no layer-shell. Drive only the final positioning
                // commands through the compositor helper; it performs its own
                // easing and avoids starting a worker that must fail.
                match &cmd {
                    cursor_overlay::OverlayCommand::ClickPulse { x, y } => {
                        crate::wayland::shell_helper::click_pulse(*x as i32, *y as i32);
                    }
                    cursor_overlay::OverlayCommand::SnapTo { x, y, .. } => {
                        crate::wayland::shell_helper::move_cursor(*x as i32, *y as i32);
                    }
                    _ => {}
                }
            } else {
                let _ = crate::wayland::overlay::forward(&msg);
            }
        }
    }
}

pub fn is_enabled() -> bool {
    is_enabled_for("default")
}

pub fn is_enabled_for(key: &str) -> bool {
    RENDER
        .lock()
        .ok()
        .and_then(|g| {
            g.as_ref().and_then(|m| {
                m.cursors
                    .get(key)
                    .or_else(|| m.cursors.get("default"))
                    .map(|rs| rs.core.visible)
            })
        })
        .unwrap_or(false)
}

pub fn current_position() -> (f64, f64) {
    current_position_for("default")
}

pub fn current_position_for(key: &str) -> (f64, f64) {
    RENDER
        .lock()
        .ok()
        .and_then(|g| {
            g.as_ref()
                .and_then(|m| m.cursors.get(key))
                .map(|rs| rs.core.pos)
        })
        .unwrap_or((-200.0, -200.0))
}

pub fn current_motion_for(key: &str) -> cursor_overlay::MotionConfig {
    RENDER
        .lock()
        .ok()
        .and_then(|guard| {
            guard.as_ref().and_then(|map| {
                map.cursors
                    .get(key)
                    .or_else(|| map.cursors.get("default"))
                    .map(|state| state.core.motion.clone())
            })
        })
        .unwrap_or_default()
}

fn seed_start_if_sentinel(key: &CursorKey, target_x: f64, target_y: f64) -> bool {
    const SEED_OFFSET: f64 = 140.0;
    let mut guard = RENDER.lock().unwrap();
    let Some(map) = guard.as_mut() else {
        return false;
    };
    if map.ended.contains(key) {
        return false;
    }
    let template = map.template.clone();
    let k = key.clone();
    let rs = map
        .cursors
        .entry(key.clone())
        .or_insert_with(|| render_state_for_key(&template, &k));
    if !(rs.core.cfg.enabled && rs.core.pos.0 < -50.0) {
        return false;
    }
    let max_x = map.scr_w.max(2) as f64 - 2.0;
    let max_y = map.scr_h.max(2) as f64 - 2.0;
    let mut sx = (target_x - SEED_OFFSET).clamp(2.0, max_x);
    let mut sy = (target_y - SEED_OFFSET).clamp(2.0, max_y);
    if (sx - target_x).abs() < 8.0 && (sy - target_y).abs() < 8.0 {
        sx = (target_x + SEED_OFFSET).clamp(2.0, max_x);
        sy = (target_y + SEED_OFFSET).clamp(2.0, max_y);
    }
    rs.core.pos = (sx, sy);
    true
}

pub async fn animate_cursor_to(x: f64, y: f64) {
    animate_cursor_to_for("default".to_owned(), x, y).await;
}

pub async fn animate_cursor_to_for(key: CursorKey, x: f64, y: f64) {
    if key.is_empty() {
        return;
    }
    seed_start_if_sentinel(&key, x, y);
    let should_animate = {
        let guard = RENDER.lock().unwrap();
        match guard.as_ref().and_then(|m| m.cursors.get(&key)) {
            Some(rs) if rs.core.cfg.enabled && rs.core.visible && rs.core.pos.0 > -50.0 => true,
            _ => false,
        }
    };
    if !should_animate {
        return;
    }

    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    arrival_register(key.clone(), tx);

    send_command_for(
        key,
        OverlayCommand::MoveTo {
            x,
            y,
            end_heading_radians: std::f64::consts::FRAC_PI_4,
        },
    );

    let _ = rx.await;
}

pub fn remove_cursor(key: CursorKey) {
    if key.is_empty() {
        return;
    }
    if let Some(tx) = CMD_TX.get() {
        let _ = tx.try_send(OverlayMsg::Remove(key));
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
            Some(map) => map.template.clone(),
            None => return,
        }
    };

    if !cfg.enabled {
        return;
    }

    // Wayland layer-shell overlay is started LAZILY on the first
    // send_command_for() that targets a Wayland session (see the
    // wayland::overlay::forward() path). Starting it eagerly here added
    // ~100-300ms to cua-driver mcp startup — enough to push the CI
    // `cursor-click-gif` test (20s budget for the full launch_app →
    // click → type sequence) over its limit, intermittently. The
    // forward() path's own ensure_started() OnceLock guarantees the
    // thread spins up on demand without losing any commands (the
    // first send_command_for that triggers it spawns the thread,
    // future commands reuse it).

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
}

impl RenderState {
    fn new(cfg: CursorConfig) -> Self {
        RenderState {
            core: RenderStateCore::new(cfg),
        }
    }

    fn tick(&mut self, dt: f64) -> bool {
        self.core.tick_motion(dt)
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
fn run_overlay_thread(cfg: CursorConfig, rx: std::sync::mpsc::Receiver<OverlayMsg>) {
    use x11rb::connection::Connection;
    use x11rb::protocol::shape::{ConnectionExt as ShapeConnectionExt, SK, SO};
    use x11rb::protocol::xproto::ConnectionExt as XprotoConnectionExt;
    use x11rb::protocol::xproto::{
        AtomEnum, ColormapAlloc, CreateWindowAux, EventMask, PropMode, WindowClass,
    };
    use x11rb::wrapper::ConnectionExt as WrapperConnectionExt;

    // Connect to X11.
    let (conn, screen_num) = match x11rb::connect(None) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("X11 overlay: cannot connect to display: {e}");
            return;
        }
    };

    let screen = &conn.setup().roots[screen_num];
    let root = screen.root;
    let scr_w = screen.width_in_pixels as u32;
    let scr_h = screen.height_in_pixels as u32;

    // Update render state with screen size.
    {
        let mut guard = RENDER.lock().unwrap();
        if let Some(map) = guard.as_mut() {
            map.scr_w = scr_w;
            map.scr_h = scr_h;
        }
    }

    // Find 32-bit ARGB visual for compositing.
    // Falls back to the default visual if XComposite 32-bit isn't available.
    let (visual_id, depth, colormap) = find_argb_visual(&conn, screen).unwrap_or((
        screen.root_visual,
        screen.root_depth,
        screen.default_colormap,
    ));

    // Create a matching colormap if we got a non-default visual.
    let colormap = if visual_id != screen.root_visual {
        let cm = conn.generate_id().unwrap();
        conn.create_colormap(ColormapAlloc::NONE, cm, root, visual_id)
            .ok();
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
        depth,
        win,
        root,
        0,
        0,
        scr_w as u16,
        scr_h as u16,
        0,
        WindowClass::INPUT_OUTPUT,
        visual_id,
        &win_aux,
    )
    .ok();

    // Set window title (identifies our overlay, matches Windows convention).
    // `Cua.` namespace mirrors the Windows class-name + install-path
    // convention; was `TropeCUA.` (leaked codename from an early C# ref).
    let title = format!("Cua.AgentCursorOverlay.{}", cfg.cursor_id);
    conn.change_property8(
        PropMode::REPLACE,
        win,
        AtomEnum::WM_NAME,
        AtomEnum::STRING,
        title.as_bytes(),
    )
    .ok();

    // Make the window fully click-through using the Shape extension (empty input region).
    // This is the X11 equivalent of WS_EX_TRANSPARENT on Windows. Note that
    // ShapeMask with a None pixmap would *reset* the input shape to the full
    // window — an empty rectangle list is how an empty region is expressed.
    conn.shape_rectangles(
        SO::SET,
        SK::INPUT,
        x11rb::protocol::xproto::ClipOrdering::UNSORTED,
        win,
        0,
        0,
        &[],
    )
    .ok();

    conn.map_window(win).ok();
    conn.flush().ok();

    // Main render loop at ~60Hz.
    let frame_dur = Duration::from_millis(16);
    let mut last_tick = Instant::now();
    let mut last_ztick = Instant::now();
    let z_enforcer = X11ZOrderEnforcer { conn: &conn, win };

    loop {
        let now = Instant::now();
        let dt = now.duration_since(last_tick).as_secs_f64().min(0.05);
        last_tick = now;

        // Drain commands and tick.
        let (arrived, pinned_wid) = {
            let mut guard = RENDER.lock().unwrap();
            if let Some(map) = guard.as_mut() {
                while let Ok(msg) = rx.try_recv() {
                    if let Some(key) = apply_msg(map, msg) {
                        map.last_active = Some(key);
                    }
                }
                let mut arrived = Vec::new();
                for (key, rs) in map.cursors.iter_mut() {
                    if rs.tick(dt) {
                        arrived.push(key.clone());
                    }
                }
                let pinned_wid = map
                    .last_active
                    .as_ref()
                    .and_then(|key| map.cursors.get(key))
                    .and_then(|rs| rs.core.pinned_wid);
                (arrived, pinned_wid)
            } else {
                (Vec::new(), None)
            }
        };

        // Render and paint.
        let pixmap = {
            let guard = RENDER.lock().unwrap();
            guard.as_ref().map(|map| {
                let mut pm = tiny_skia::Pixmap::new(map.scr_w.max(1), map.scr_h.max(1))
                    .unwrap_or_else(|| tiny_skia::Pixmap::new(1, 1).unwrap());
                for rs in map.cursors.values() {
                    // TODO: thread X11/Wayland scale-factor here so cursors
                    // render at native resolution on HiDPI Linux displays
                    // (`XRRGetCrtcInfo` reports per-output DPI; the GNOME
                    // scale-factor setting is exposed via the screen-scaling
                    // protocol). For now we default to 1.0 — preserves
                    // pre-retina-fix behaviour on Linux.
                    cursor_overlay::paint_cursor(&mut pm, &rs.core, 0.0, 0.0, None, 1.0);
                }
                pm
            })
        };

        if let Some(pm) = pixmap {
            paint_x11(&conn, win, scr_w, scr_h, depth, visual_id, &pm);
        }

        for key in &arrived {
            arrival_fire(key);
        }

        // Z-order maintenance every 80ms — delegate to the cross-platform
        // ZOrderEnforcer so the contract for "z+1 of the application under
        // test" is documented once in `cursor_overlay::z_order`.
        if last_ztick.elapsed() >= Duration::from_millis(80) {
            last_ztick = Instant::now();
            z_enforcer.reassert(pinned_wid);
        }

        // Drain any X events (needed to avoid blocking).
        while let Ok(Some(_)) = conn.poll_for_event() {}

        let elapsed = Instant::now().duration_since(last_tick);
        if let Some(remaining) = frame_dur.checked_sub(elapsed) {
            std::thread::sleep(remaining);
        }
    }
}

// ── Z-order enforcer (Linux impl of cursor_overlay::ZOrderEnforcer) ──────

/// X11 implementation of [`cursor_overlay::ZOrderEnforcer`].
///
/// Borrows the X11 connection and overlay window id; lives only inside
/// `run_overlay_thread` (the X11 connection is not `'static`). Called
/// every 80 ms from the render loop.
#[cfg(target_os = "linux")]
struct X11ZOrderEnforcer<'a, C: x11rb::connection::Connection> {
    conn: &'a C,
    win: u32,
}

#[cfg(target_os = "linux")]
impl<'a, C: x11rb::connection::Connection> ZOrderEnforcer for X11ZOrderEnforcer<'a, C> {
    fn reassert(&self, target: Option<u64>) {
        use x11rb::protocol::xproto::{
            ConfigureWindowAux, ConnectionExt as XprotoConnectionExt, StackMode,
        };

        // Per the ZOrderEnforcer trait contract, a stale `target` (window
        // gone) should fall back to the `None` behavior — top of the
        // normal stack, no sibling. Using a stale XID as a `sibling` here
        // triggers BadWindow on every tick of the overlay-enforcer loop
        // (~125 Hz), spamming the X server and silently skipping the
        // intended z-reassertion. Probe liveness via get_window_attributes
        // before committing to the sibling path.
        let target_live = target.and_then(|xid| {
            self.conn
                .get_window_attributes(xid as u32)
                .ok()
                .and_then(|c| c.reply().ok())
                .map(|_| xid)
        });
        let aux = if let Some(target_xid) = target_live {
            // Place overlay just above the pinned X11 window.
            ConfigureWindowAux::new()
                .sibling(target_xid as u32)
                .stack_mode(StackMode::ABOVE)
        } else {
            // No pin (or stale target XID) → raise to the top of the
            // normal stack. (X11 has no OS-level "always-on-top" band like
            // Windows / NSStatusWindowLevel, so a plain ABOVE here cannot
            // accidentally float over a focused foreground app the way
            // HWND_TOPMOST would on Windows.)
            ConfigureWindowAux::new().stack_mode(StackMode::ABOVE)
        };
        self.conn.configure_window(self.win, &aux).ok();
        self.conn.flush().ok();
    }
}

#[cfg(target_os = "linux")]
fn find_argb_visual(
    _conn: &impl x11rb::connection::Connection,
    screen: &x11rb::protocol::xproto::Screen,
) -> Option<(u32, u8, u32)> {
    use x11rb::protocol::xproto::VisualClass;
    // Walk all depth entries looking for a 32-bit ARGB visual.
    for depth_entry in &screen.allowed_depths {
        if depth_entry.depth != 32 {
            continue;
        }
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
    w: u32,
    h: u32,
    depth: u8,
    _visual_id: u32,
    pm: &tiny_skia::Pixmap,
) {
    use x11rb::protocol::shape::{ConnectionExt as ShapeConnectionExt, SK, SO};
    use x11rb::protocol::xproto::{ConnectionExt as XprotoConnectionExt, CreateGCAux, ImageFormat};
    if pm.width() == 0 || pm.height() == 0 {
        return;
    }

    // Create a GC for the window if we don't have one.
    // (Simplified: we recreate it every frame which is safe but not optimal.)
    let gc_id = match conn.generate_id() {
        Ok(id) => id,
        Err(_) => return,
    };
    let gc_aux = CreateGCAux::new();
    if conn.create_gc(gc_id, win, &gc_aux).is_err() {
        return;
    }

    let (bgra, visible_shape) = bgra_and_visible_shape(pm);

    // A 32-bit ARGB window needs a compositor to blend transparent pixels.
    // Without one, zero-alpha pixels display as opaque black. Clip the native
    // window to the rendered non-zero alpha runs so the overlay remains usable
    // under bare Openbox/i3/Xvfb sessions as well as composited desktops.
    let _ = conn.shape_rectangles(
        SO::SET,
        SK::BOUNDING,
        x11rb::protocol::xproto::ClipOrdering::UNSORTED,
        win,
        0,
        0,
        &visible_shape,
    );

    // XPutImage (ZPixmap).
    let _ = conn.put_image(
        ImageFormat::Z_PIXMAP,
        win,
        gc_id,
        w as u16,
        h as u16,
        0,
        0,
        0,
        depth,
        &bgra,
    );

    conn.free_gc(gc_id).ok();
    conn.flush().ok();
}

#[cfg(target_os = "linux")]
fn bgra_and_visible_shape(
    pm: &tiny_skia::Pixmap,
) -> (Vec<u8>, Vec<x11rb::protocol::xproto::Rectangle>) {
    use x11rb::protocol::xproto::Rectangle;

    let width = pm.width() as usize;
    let height = pm.height() as usize;
    let src = pm.data();
    let mut bgra = Vec::with_capacity(src.len());
    let mut rectangles = Vec::new();

    for y in 0..height {
        let mut run_start = None;
        for x in 0..width {
            let offset = (y * width + x) * 4;
            let pixel = &src[offset..offset + 4];
            bgra.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);

            if pixel[3] != 0 {
                run_start.get_or_insert(x);
            } else if let Some(start) = run_start.take() {
                rectangles.push(Rectangle {
                    x: start as i16,
                    y: y as i16,
                    width: (x - start) as u16,
                    height: 1,
                });
            }
        }
        if let Some(start) = run_start {
            rectangles.push(Rectangle {
                x: start as i16,
                y: y as i16,
                width: (width - start) as u16,
                height: 1,
            });
        }
    }

    (bgra, rectangles)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::bgra_and_visible_shape;

    #[test]
    fn visible_shape_contains_only_nontransparent_runs() {
        let mut pixmap = tiny_skia::Pixmap::new(4, 2).unwrap();
        pixmap.data_mut().copy_from_slice(&[
            1, 2, 3, 0, 10, 20, 30, 255, 11, 21, 31, 128, 4, 5, 6, 0, 7, 8, 9, 64, 1, 1, 1, 0, 2,
            2, 2, 0, 12, 22, 32, 255,
        ]);

        let (bgra, rectangles) = bgra_and_visible_shape(&pixmap);

        assert_eq!(&bgra[4..8], &[30, 20, 10, 255]);
        assert_eq!(rectangles.len(), 3);
        assert_eq!(
            (rectangles[0].x, rectangles[0].y, rectangles[0].width),
            (1, 0, 2)
        );
        assert_eq!(
            (rectangles[1].x, rectangles[1].y, rectangles[1].width),
            (0, 1, 1)
        );
        assert_eq!(
            (rectangles[2].x, rectangles[2].y, rectangles[2].width),
            (3, 1, 1)
        );
    }
}

#[cfg(not(target_os = "linux"))]
fn run_overlay_thread(_cfg: CursorConfig, _rx: std::sync::mpsc::Receiver<OverlayMsg>) {}
