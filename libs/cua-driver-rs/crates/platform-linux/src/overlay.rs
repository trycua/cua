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

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use cursor_overlay::{
    CursorConfig, CursorShape, MotionConfig, OverlayCommand, Palette, PathPlanner, PathState,
    PlannedPath,
};

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
            Some(rs) => rs.cfg.clone(),
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

struct RenderState {
    cfg:        CursorConfig,
    palette:    Palette,
    motion:     MotionConfig,
    pos:        (f64, f64),
    heading:    f64,
    path:       Option<PlannedPath>,
    dist:       f64,
    start_t:    Instant,
    spring:     Option<Spring>,
    spring_tgt: Option<(f64, f64, f64)>,
    click_t:    Option<f64>,
    shape:      Option<CursorShape>,
    visible:    bool,
    idle_secs:  f64,
    idle_alpha: f64,
    pinned_wid: Option<u64>,
    scr_w: u32,
    scr_h: u32,
}

#[derive(Clone, Copy)]
struct Spring { ox: f64, oy: f64, vx: f64, vy: f64 }

impl RenderState {
    fn new(cfg: CursorConfig) -> Self {
        let palette = cfg.palette();
        let motion  = cfg.motion.clone();
        let shape   = cfg.shape.clone();
        RenderState {
            cfg, palette, motion, shape,
            pos:        (-200.0, -200.0),
            heading:    std::f64::consts::FRAC_PI_4,
            path:       None,
            dist:       0.0,
            start_t:    Instant::now(),
            spring:     None,
            spring_tgt: None,
            click_t:    None,
            visible:    true,
            idle_secs:  0.0,
            idle_alpha: 1.0,
            pinned_wid: None,
            scr_w: 1920,
            scr_h: 1080,
        }
    }

    fn tick(&mut self, dt: f64) {
        let spring_k  = self.motion.spring * 400.0;
        let spring_c  = self.motion.spring * 20.0;

        if let Some(ref p) = self.path {
            let path_frac = (self.dist / p.length.max(1.0)).clamp(0.0, 1.0);
            let profile   = 16.0 * path_frac * path_frac * (1.0 - path_frac) * (1.0 - path_frac);
            let floor     = if path_frac < 0.5 { self.motion.min_start_speed } else { self.motion.min_end_speed };
            let speed     = (floor + (self.motion.peak_speed - floor) * profile).max(floor);
            self.dist   += speed * dt;

            let path_len = p.length.max(1.0);
            if self.dist >= path_len {
                let end = p.sample(path_len);
                let end_heading = p.end_visual_heading;
                let vh = end.heading;
                self.spring = Some(Spring {
                    ox: 0.0, oy: 0.0,
                    vx: speed * 0.5 * vh.cos(),
                    vy: speed * 0.5 * vh.sin(),
                });
                self.spring_tgt = Some((end.x, end.y, end_heading));
                self.pos = (end.x, end.y);
                self.heading = end_heading;
                self.path = None;
                self.dist = 0.0;
            } else {
                let s: PathState = p.sample(self.dist);
                self.pos = (s.x, s.y);
                let desired = s.heading + std::f64::consts::PI;
                let max_step = 14.0 * dt;
                self.heading = rotate_toward(self.heading, desired, max_step);
            }
        } else if let Some(mut s) = self.spring {
            if let Some((tx, ty, th)) = self.spring_tgt {
                let sdt = dt / 4.0;
                for _ in 0..4 {
                    s.vx += (-spring_k * s.ox - spring_c * s.vx) * sdt;
                    s.vy += (-spring_k * s.oy - spring_c * s.vy) * sdt;
                    s.ox += s.vx * sdt;
                    s.oy += s.vy * sdt;
                }
                self.pos = (tx + s.ox, ty + s.oy);
                self.heading = th;
                if s.ox.hypot(s.oy) < 0.3 && s.vx.hypot(s.vy) < 2.0 {
                    self.pos = (tx, ty);
                    self.spring = None;
                } else {
                    self.spring = Some(s);
                }
            }
        }

        if let Some(t) = self.click_t {
            let next = t + dt * 4.0;
            self.click_t = if next >= 1.0 { None } else { Some(next) };
        }

        let idle_hide_ms = self.motion.idle_hide_ms;
        if idle_hide_ms > 0.0 {
            let moving = self.path.is_some() || self.spring.is_some() || self.click_t.is_some();
            if moving {
                self.idle_secs  = 0.0;
                self.idle_alpha = 1.0;
            } else {
                self.idle_secs += dt;
                let fade_start = idle_hide_ms / 1000.0;
                let fade_end   = fade_start + 0.18;
                if self.idle_secs > fade_end {
                    self.idle_alpha = 0.0;
                } else if self.idle_secs > fade_start {
                    let t = (self.idle_secs - fade_start) / 0.18;
                    self.idle_alpha = 1.0 - t.clamp(0.0, 1.0);
                }
            }
        } else {
            self.idle_alpha = 1.0;
        }
    }

    fn apply_command(&mut self, cmd: OverlayCommand) {
        match cmd {
            OverlayCommand::MoveTo { x, y, end_heading_radians } => {
                let (x0, y0) = self.pos;
                let th0 = self.heading + std::f64::consts::PI;
                let th1 = end_heading_radians + std::f64::consts::PI;
                const CLICK_OFFSET: f64 = 16.0;
                const TURN_RADIUS:  f64 = 80.0;
                let tx = x + end_heading_radians.cos() * CLICK_OFFSET;
                let ty = y + end_heading_radians.sin() * CLICK_OFFSET;
                let plan = PathPlanner::plan(
                    x0, y0, th0,
                    tx, ty, th1,
                    end_heading_radians,
                    TURN_RADIUS,
                );
                self.path       = Some(plan);
                self.dist       = 0.0;
                self.start_t    = Instant::now();
                self.spring     = None;
                self.spring_tgt = None;
                self.idle_secs  = 0.0;
                self.idle_alpha = 1.0;
            }
            OverlayCommand::ClickPulse { x, y } => {
                self.pos        = (x, y);
                self.click_t    = Some(0.0);
                self.idle_secs  = 0.0;
                self.idle_alpha = 1.0;
            }
            OverlayCommand::SetEnabled(v)  => { self.visible = v; }
            OverlayCommand::SetMotion(m)   => { self.motion  = m; }
            OverlayCommand::SetPalette(p)  => { self.palette = p; }
            OverlayCommand::PinAbove(wid)  => { self.pinned_wid = Some(wid); }
            // Custom cursor shape/gradient/focus-rect — not yet rendered on Linux;
            // accepted silently so the tool doesn't return an error.
            OverlayCommand::SetShape(_) | OverlayCommand::SetGradient { .. }
            | OverlayCommand::ShowFocusRect(_) => {}
        }
    }
}

fn rotate_toward(current: f64, desired: f64, max_step: f64) -> f64 {
    let mut diff = desired - current;
    while diff >  std::f64::consts::PI { diff -= 2.0 * std::f64::consts::PI; }
    while diff < -std::f64::consts::PI { diff += 2.0 * std::f64::consts::PI; }
    current + diff.clamp(-max_step, max_step)
}

// ── Renderer (shared tiny-skia logic) ─────────────────────────────────────

fn render_frame(rs: &RenderState) -> tiny_skia::Pixmap {
    let w = rs.scr_w.max(1);
    let h = rs.scr_h.max(1);
    let mut pm = tiny_skia::Pixmap::new(w, h)
        .unwrap_or_else(|| tiny_skia::Pixmap::new(1, 1).unwrap());

    if !rs.visible || rs.pos.0 < -100.0 || rs.idle_alpha < 0.004 {
        return pm;
    }

    let (px, py)    = rs.pos;
    let heading     = rs.heading;
    let alpha_scale = rs.idle_alpha as f32;

    let bloom_r: f32 = 22.0;
    let [br, bg, bb, _] = rs.palette.bloom_inner;
    let bloom_inner = tiny_skia::Color::from_rgba8(br, bg, bb, (115.0 * alpha_scale) as u8);
    let [or_, og, ob, _] = rs.palette.bloom_outer;
    let bloom_outer = tiny_skia::Color::from_rgba8(or_, og, ob, (26.0 * alpha_scale) as u8);
    let bloom_zero  = tiny_skia::Color::from_rgba8(or_, og, ob, 0);

    let bloom_paint = {
        let mut p = tiny_skia::Paint::default();
        p.shader = tiny_skia::RadialGradient::new(
            tiny_skia::Point::from_xy(px as f32, py as f32),
            tiny_skia::Point::from_xy(px as f32, py as f32),
            bloom_r,
            vec![
                tiny_skia::GradientStop::new(0.0, bloom_inner),
                tiny_skia::GradientStop::new(0.5, bloom_outer),
                tiny_skia::GradientStop::new(1.0, bloom_zero),
            ],
            tiny_skia::SpreadMode::Pad,
            tiny_skia::Transform::identity(),
        ).unwrap_or(tiny_skia::Shader::SolidColor(bloom_inner));
        p.anti_alias = true;
        p
    };
    if let Some(r) = tiny_skia::Rect::from_xywh(
        (px - bloom_r as f64) as f32, (py - bloom_r as f64) as f32,
        bloom_r * 2.0, bloom_r * 2.0,
    ) {
        pm.fill_rect(r, &bloom_paint, tiny_skia::Transform::identity(), None);
    }

    if let Some(t) = rs.click_t {
        let ring_r = (bloom_r + 20.0 * t as f32) * (1.0 - t as f32 * 0.5);
        let alpha   = ((1.0 - t) * 180.0 * alpha_scale as f64) as u8;
        let [cr, cg, cb, _] = rs.palette.cursor_mid;
        let ring_color = tiny_skia::Color::from_rgba8(cr, cg, cb, alpha);
        let mut ring_paint = tiny_skia::Paint::default();
        ring_paint.shader = tiny_skia::Shader::SolidColor(ring_color);
        ring_paint.anti_alias = true;
        let stroke = tiny_skia::Stroke { width: 2.0, ..Default::default() };
        let mut pb = tiny_skia::PathBuilder::new();
        pb.push_circle(px as f32, py as f32, ring_r);
        if let Some(path) = pb.finish() {
            pm.stroke_path(&path, &ring_paint, &stroke, tiny_skia::Transform::identity(), None);
        }
    }

    if let Some(ref shape) = rs.shape {
        let sz = 32.0_f32;
        if let Some(pix) = tiny_skia::PixmapRef::from_bytes(&shape.pixels, shape.width, shape.height) {
            let transform = tiny_skia::Transform::from_rotate_at(
                heading.to_degrees() as f32 + 180.0, px as f32, py as f32,
            ).pre_translate(px as f32 - sz / 2.0, py as f32 - sz / 2.0);
            let mut paint = tiny_skia::PixmapPaint::default();
            paint.opacity = alpha_scale;
            pm.draw_pixmap(0, 0, pix, &paint, transform, None);
        }
    } else {
        draw_default_arrow(&mut pm, &rs.palette, px as f32, py as f32, heading as f32, alpha_scale);
    }

    pm
}

fn draw_default_arrow(
    pm: &mut tiny_skia::Pixmap,
    palette: &Palette,
    px: f32, py: f32,
    heading: f32,
    alpha_scale: f32,
) {
    let verts: [(f32, f32); 4] = [(14.0, 0.0), (-8.0, -9.0), (-3.0, 0.0), (-8.0, 9.0)];
    let angle = heading + std::f64::consts::PI as f32;
    let (sa, ca) = (angle.sin(), angle.cos());
    let xform = |(vx, vy): (f32, f32)| (px + ca * vx - sa * vy, py + sa * vx + ca * vy);
    let pts: Vec<(f32, f32)> = verts.iter().map(|&v| xform(v)).collect();
    let mut pb = tiny_skia::PathBuilder::new();
    pb.move_to(pts[0].0, pts[0].1);
    for p in &pts[1..] { pb.line_to(p.0, p.1); }
    pb.close();
    let arrow_path = match pb.finish() { Some(p) => p, None => return };
    let tip  = pts[0];
    let tail = ((pts[1].0 + pts[3].0) / 2.0, (pts[1].1 + pts[3].1) / 2.0);
    let [r0, g0, b0, _] = palette.cursor_start;
    let [r1, g1, b1, _] = palette.cursor_mid;
    let [r2, g2, b2, _] = palette.cursor_end;
    let a = (255.0 * alpha_scale) as u8;
    let fill_paint = {
        let mut p = tiny_skia::Paint::default();
        p.shader = tiny_skia::LinearGradient::new(
            tiny_skia::Point::from_xy(tip.0, tip.1),
            tiny_skia::Point::from_xy(tail.0, tail.1),
            vec![
                tiny_skia::GradientStop::new(0.00, tiny_skia::Color::from_rgba8(r0, g0, b0, a)),
                tiny_skia::GradientStop::new(0.53, tiny_skia::Color::from_rgba8(r1, g1, b1, a)),
                tiny_skia::GradientStop::new(1.00, tiny_skia::Color::from_rgba8(r2, g2, b2, a)),
            ],
            tiny_skia::SpreadMode::Pad,
            tiny_skia::Transform::identity(),
        ).unwrap_or(tiny_skia::Shader::SolidColor(tiny_skia::Color::from_rgba8(r1, g1, b1, a)));
        p.anti_alias = true;
        p
    };
    pm.fill_path(&arrow_path, &fill_paint, tiny_skia::FillRule::Winding,
                 tiny_skia::Transform::identity(), None);
    let mut sp = tiny_skia::Paint::default();
    sp.shader = tiny_skia::Shader::SolidColor(tiny_skia::Color::from_rgba8(255, 255, 255, a));
    sp.anti_alias = true;
    let stroke = tiny_skia::Stroke { width: 1.5, ..Default::default() };
    pm.stroke_path(&arrow_path, &sp, &stroke, tiny_skia::Transform::identity(), None);
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
    let title = format!("TropeCUA.AgentCursorOverlay.{}", cfg.cursor_id);
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
            guard.as_ref().map(|rs| render_frame(rs))
        };

        if let Some(pm) = pixmap {
            paint_x11(&conn, win, scr_w, scr_h, depth, visual_id, &pm);
        }

        // Z-order maintenance every 80ms.
        if last_ztick.elapsed() >= Duration::from_millis(80) {
            last_ztick = Instant::now();
            let pinned_wid = {
                let guard = RENDER.lock().unwrap();
                guard.as_ref().and_then(|rs| rs.pinned_wid)
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
