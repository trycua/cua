//! macOS agent-cursor overlay — transparent click-through NSWindow.
//!
//! ## Architecture
//!
//! The MCP/tokio server runs on a **background thread** (spawned in
//! `cua-driver/src/main.rs`).  AppKit MUST run on the **main thread**.
//! The two sides communicate through a global lock-free channel:
//!
//! - MCP tool calls → `send_command(OverlayCommand)` → `CMD_TX` (SyncSender)
//! - main thread → `run_on_main_thread()` → drains `CMD_RX` every frame
//!
//! The render loop uses a GCD background thread at ~60 fps.  Each tick it
//! renders the animation state into a `tiny_skia::Pixmap`, converts to a
//! `CGImage`, and dispatches `CALayer.setContents` back to the main queue.
//!
//! ## Coordinate system
//!
//! All coordinates are **screen points** with the **top-left origin**
//! (matching `OverlayCommand::MoveTo` and AX element coordinates).  The
//! NSWindow covers `NSScreen.mainScreen.frame` which AppKit places with
//! a bottom-left origin, so we flip Y when drawing into the Pixmap.

use std::ffi::c_void;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use cursor_overlay::{
    CursorConfig, CursorShape, MotionConfig, OverlayCommand, Palette, PathPlanner, PathState,
    PlannedPath,
};

// ── Arrival-signal channel ────────────────────────────────────────────────

static ARRIVAL_TX: Mutex<Option<tokio::sync::oneshot::Sender<()>>> = Mutex::new(None);

// ── Global overlay state ──────────────────────────────────────────────────

static CMD_TX: OnceLock<std::sync::mpsc::SyncSender<OverlayCommand>> = OnceLock::new();
// Single-consumer slot; receiver is moved into run_on_main_thread().
static CMD_RX_CELL: Mutex<Option<std::sync::mpsc::Receiver<OverlayCommand>>> = Mutex::new(None);
static RENDER: Mutex<Option<RenderState>> = Mutex::new(None);

/// Initialise global overlay state (call once, before run_on_main_thread).
pub fn init(cfg: CursorConfig) {
    let (tx, rx) = std::sync::mpsc::sync_channel(4096);
    let _ = CMD_TX.set(tx);
    *CMD_RX_CELL.lock().unwrap() = Some(rx);
    *RENDER.lock().unwrap() = Some(RenderState::new(cfg));
}

/// Send a command from any thread (MCP tool, etc.).  Non-blocking; drops if
/// the channel is full (old commands are less important than new ones).
pub fn send_command(cmd: OverlayCommand) {
    if let Some(tx) = CMD_TX.get() {
        let _ = tx.try_send(cmd);
    }
}

/// Return a snapshot of the current motion config (for use by set_agent_cursor_motion
/// to apply partial overrides without losing other knobs).
pub fn current_motion() -> MotionConfig {
    RENDER.lock().unwrap()
        .as_ref()
        .map(|rs| rs.motion.clone())
        .unwrap_or_default()
}

/// Animate the overlay cursor to `(x, y)` and suspend until the Dubins path
/// completes and the spring overshoot begins.
///
/// Mirrors Swift's `AgentCursor.shared.animateAndWait(to:)`.
/// Returns immediately (no animation) when:
/// - the overlay is disabled, or
/// - the cursor is still at the off-screen sentinel `(-200, -200)` — in that
///   case the caller should rely on `ClickPulse` to snap the cursor.
pub async fn animate_cursor_to(x: f64, y: f64) {
    // Check whether animation should run.
    let should_animate = {
        let guard = RENDER.lock().unwrap();
        match guard.as_ref() {
            Some(rs) if rs.cfg.enabled && rs.pos.0 > -50.0 => true,
            _ => false,
        }
    };
    if !should_animate {
        return;
    }

    // Create a one-shot channel; store the sender so the render thread can
    // fire it when the path finishes.
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    {
        let mut guard = ARRIVAL_TX.lock().unwrap();
        // Cancel any previous waiter (superseded by new animation).
        if let Some(old_tx) = guard.take() {
            let _ = old_tx.send(());
        }
        *guard = Some(tx);
    }

    // Send the MoveTo command (click offset applied inside apply_command).
    send_command(OverlayCommand::MoveTo {
        x,
        y,
        // Arrive pointing upper-left (45°), matching the macOS system-cursor
        // convention and Swift reference (`endAngleDegrees: 45`).
        end_heading_radians: std::f64::consts::FRAC_PI_4,
    });

    // Await arrival signal (fired from render thread when Dubins path ends).
    let _ = rx.await;
}

/// Block the calling thread (must be the OS main thread) running the AppKit
/// event loop and the overlay window.  Never returns normally.
///
/// Call this from `main()` after spawning the tokio background thread.
pub fn run_on_main_thread() {
    // Take the receiver.
    let rx = match CMD_RX_CELL.lock().unwrap().take() {
        Some(r) => r,
        None => {
            // init() was never called — no overlay, just spin.
            loop { std::thread::park(); }
        }
    };

    let cfg = {
        let guard = RENDER.lock().unwrap();
        match &*guard {
            Some(rs) => rs.cfg.clone(),
            None => return,
        }
    };

    if !cfg.enabled {
        loop { std::thread::park(); }
    }

    // ------------------------------------------------------------------
    // AppKit setup (all on the main thread).
    // ------------------------------------------------------------------
    unsafe { run_appkit(cfg, rx) };
}

// ── Animation / render state ──────────────────────────────────────────────

struct RenderState {
    cfg:       CursorConfig,
    palette:   Palette,
    motion:    MotionConfig,
    /// Current rendered position (screen top-left coords).
    pos:       (f64, f64),
    /// Visual heading in radians (tip direction = motion_dir + π).
    heading:   f64,
    /// In-flight path (None = at rest).
    path:      Option<PlannedPath>,
    dist:      f64,          // arc-distance travelled so far
    /// Spring settle after path arrival.
    spring:    Option<Spring>,
    spring_tgt: Option<(f64, f64, f64)>, // (x, y, heading)
    /// Flash phase for ClickPulse (0..1 or None).
    click_t:   Option<f64>,
    /// Window (frame) size.
    win_w:     f64,
    win_h:     f64,
    /// Custom cursor shape (if any); None = gradient arrow.
    shape:     Option<CursorShape>,
    /// Runtime-overridden gradient colours (from set_agent_cursor_style).
    /// Each entry is [R, G, B, A].  Empty = use palette defaults.
    gradient_colors: Vec<[u8; 4]>,
    /// Runtime-overridden bloom colour (from set_agent_cursor_style).
    /// None = use palette default.
    bloom_override: Option<[u8; 4]>,
    /// Whether the overlay is visible (user-controlled).
    visible:   bool,
    /// Idle-hide: elapsed seconds since last activity.
    idle_secs: f64,
    /// Idle-hide fade: 1.0 = fully visible, 0.0 = fully hidden.
    idle_alpha: f64,
    /// Currently pinned window id for z-ordering.
    pinned_wid: Option<u64>,
    /// Focus-highlight rectangle `[x, y, w, h]` in screen coords; None = not shown.
    focus_rect: Option<[f64; 4]>,
    /// Fade progress for the focus rect: 0.0 = fully visible, 1.0 = gone.
    focus_rect_t: f64,
}

#[derive(Clone, Copy)]
struct Spring {
    ox: f64, oy: f64,
    vx: f64, vy: f64,
}

impl RenderState {
    fn new(cfg: CursorConfig) -> Self {
        let palette = cfg.palette();
        let motion  = cfg.motion.clone();
        let shape   = cfg.shape.clone();
        RenderState {
            cfg, palette, motion, shape,
            gradient_colors: vec![],
            bloom_override: None,
            pos:        (-200.0, -200.0),
            heading:    std::f64::consts::FRAC_PI_4,
            path:       None,
            dist:       0.0,
            spring:     None,
            spring_tgt: None,
            click_t:    None,
            win_w:      0.0,
            win_h:      0.0,
            visible:    true,
            idle_secs:  0.0,
            idle_alpha: 1.0,
            pinned_wid: None,
            focus_rect: None,
            focus_rect_t: 1.0,
        }
    }

    fn tick(&mut self, dt: f64) -> bool {
        // Returns true if an arrival signal should be fired (path just ended).
        // Swift constants.
        const PEAK_SPEED:       f64 = 900.0;
        const MIN_START_SPEED:  f64 = 300.0;
        const MIN_END_SPEED:    f64 = 200.0;
        const SPRING_K:         f64 = 400.0;
        const SPRING_C:         f64 = 17.0;
        const SPRING_OVERSHOOT: f64 = 0.8;

        let mut fire_arrival = false;

        if let Some(ref p) = self.path {
            let path_len = p.length.max(1.0);
            let u = (self.dist / path_len).min(1.0);

            // Smootherstep speed profile (normalised: peak = 1.0).
            let profile = (30.0 * u * u * (1.0 - u) * (1.0 - u)) / 1.875;
            let floor_speed = if u < 0.5 { MIN_START_SPEED } else { MIN_END_SPEED };
            let current_speed = floor_speed + (PEAK_SPEED - floor_speed) * profile;
            self.dist += current_speed * dt;

            if self.dist >= path_len {
                // Transition to spring settle.
                let end = p.sample(path_len);
                let end_heading = p.end_visual_heading;
                let vh = end.heading;
                self.spring = Some(Spring {
                    ox: 0.0, oy: 0.0,
                    vx: current_speed * SPRING_OVERSHOOT * vh.cos(),
                    vy: current_speed * SPRING_OVERSHOOT * vh.sin(),
                });
                self.spring_tgt = Some((end.x, end.y, end_heading));
                self.pos = (end.x, end.y);
                self.heading = end_heading;
                self.path = None;
                self.dist = 0.0;
                fire_arrival = true;
            } else {
                let s: PathState = p.sample(self.dist);
                self.pos = (s.x, s.y);
                // Smooth heading rotation toward motion heading.
                let desired = s.heading + std::f64::consts::PI;
                let max_step = 14.0 * dt;
                self.heading = rotate_toward(self.heading, desired, max_step);
            }
        } else if let Some(mut s) = self.spring {
            if let Some((tx, ty, th)) = self.spring_tgt {
                let substeps = 4;
                let sdt = dt / substeps as f64;
                for _ in 0..substeps {
                    s.vx += (-SPRING_K * s.ox - SPRING_C * s.vx) * sdt;
                    s.vy += (-SPRING_K * s.oy - SPRING_C * s.vy) * sdt;
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

        // Advance click pulse.
        if let Some(t) = self.click_t {
            let next = t + dt * 4.0; // full pulse over 0.25s
            self.click_t = if next >= 1.0 { None } else { Some(next) };
        }

        // Advance focus-rect fade (fades out over ~600ms).
        if self.focus_rect.is_some() {
            self.focus_rect_t = (self.focus_rect_t + dt / 0.6).min(1.0);
            if self.focus_rect_t >= 1.0 {
                self.focus_rect = None;
                self.focus_rect_t = 1.0;
            }
        }

        // Idle-hide: accumulate idle time; fade out over 180ms once threshold reached.
        let idle_hide_ms = self.motion.idle_hide_ms;
        if idle_hide_ms > 0.0 {
            let moving = self.path.is_some()
                || self.spring.is_some()
                || self.click_t.is_some();
            if moving {
                self.idle_secs  = 0.0;
                self.idle_alpha = 1.0;
            } else {
                self.idle_secs += dt;
                let fade_start = idle_hide_ms / 1000.0;
                let fade_end   = fade_start + 0.18; // 180ms fade like Windows ref
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

        fire_arrival
    }

    fn apply_command(&mut self, cmd: OverlayCommand) {
        match cmd {
            OverlayCommand::MoveTo { x, y, end_heading_radians } => {
                // Apply click offset (16 pt along end_heading) before planning,
                // matching Swift `moveTo(point:endAngleRadians:)`:
                //   tx = clickPoint.x + cos(endAngle) * clickOffset
                //   ty = clickPoint.y + sin(endAngle) * clickOffset
                const CLICK_OFFSET: f64 = 16.0;
                const TURN_RADIUS:  f64 = 80.0;
                let tx = x + end_heading_radians.cos() * CLICK_OFFSET;
                let ty = y + end_heading_radians.sin() * CLICK_OFFSET;

                // If the cursor is still at the initial off-screen sentinel,
                // snap it to the offset target so the path starts on-screen.
                if self.pos.0 < -50.0 {
                    self.pos = (tx, ty);
                }
                let (x0, y0) = self.pos;
                let th0 = self.heading + std::f64::consts::PI;
                let th1 = end_heading_radians + std::f64::consts::PI;
                let plan = PathPlanner::plan(
                    x0, y0, th0,
                    tx, ty, th1,
                    end_heading_radians,
                    TURN_RADIUS,
                );
                self.path = Some(plan);
                self.dist = 0.0;
                self.spring = None;
                self.spring_tgt = None;
                self.idle_secs = 0.0;
                self.idle_alpha = 1.0;
            }
            OverlayCommand::ClickPulse { x, y } => {
                // Only snap position on first placement (sentinel state).
                // After that the cursor stays where the animation landed.
                if self.pos.0 < -50.0 {
                    // Apply same click offset so tip lands at click point.
                    const CLICK_OFFSET: f64 = 16.0;
                    let angle = std::f64::consts::FRAC_PI_4;
                    self.pos = (x + angle.cos() * CLICK_OFFSET, y + angle.sin() * CLICK_OFFSET);
                }
                self.click_t = Some(0.0);
                self.idle_secs = 0.0;
                self.idle_alpha = 1.0;
            }
            OverlayCommand::SetEnabled(v) => {
                self.visible = v;
            }
            OverlayCommand::SetMotion(m) => {
                self.motion = m;
            }
            OverlayCommand::SetPalette(p) => {
                self.palette = p;
            }
            OverlayCommand::PinAbove(wid) => {
                self.pinned_wid = Some(wid);
            }
            OverlayCommand::SetShape(shape) => {
                self.shape = shape;
            }
            OverlayCommand::SetGradient { gradient_colors, bloom_color } => {
                self.gradient_colors = gradient_colors;
                self.bloom_override = bloom_color;
            }
            OverlayCommand::ShowFocusRect(rect) => {
                self.focus_rect = rect;
                self.focus_rect_t = 0.0; // reset fade to fully visible
            }
        }
    }
}

fn rotate_toward(current: f64, desired: f64, max_step: f64) -> f64 {
    let mut diff = desired - current;
    while diff >  std::f64::consts::PI { diff -= 2.0 * std::f64::consts::PI; }
    while diff < -std::f64::consts::PI { diff += 2.0 * std::f64::consts::PI; }
    current + diff.clamp(-max_step, max_step)
}

// ── tiny-skia rendering ───────────────────────────────────────────────────

fn render_frame(rs: &RenderState) -> tiny_skia::Pixmap {
    let w = rs.win_w.max(1.0) as u32;
    let h = rs.win_h.max(1.0) as u32;
    let mut pm = tiny_skia::Pixmap::new(w, h).unwrap_or_else(|| tiny_skia::Pixmap::new(1, 1).unwrap());

    if !rs.visible || rs.pos.0 < -100.0 || rs.idle_alpha < 0.004 {
        return pm;
    }

    let (px, py) = rs.pos;
    let heading  = rs.heading;
    let alpha_scale = rs.idle_alpha as f32;

    // --- Bloom (radial gradient behind the arrow) ---
    let bloom_r: f32 = 22.0;
    // Use runtime bloom_override if set, otherwise fall back to palette.
    let (br, bg, bb) = if let Some([r, g, b, _]) = rs.bloom_override {
        (r, g, b)
    } else {
        let [r, g, b, _] = rs.palette.bloom_inner;
        (r, g, b)
    };
    let bloom_inner = tiny_skia::Color::from_rgba8(br, bg, bb, (115.0 * alpha_scale) as u8);
    let (or_, og, ob) = if let Some([r, g, b, _]) = rs.bloom_override {
        (r, g, b)
    } else {
        let [r, g, b, _] = rs.palette.bloom_outer;
        (r, g, b)
    };
    let bloom_outer = tiny_skia::Color::from_rgba8(or_, og, ob, (26.0 * alpha_scale) as u8);
    let bloom_zero  = tiny_skia::Color::from_rgba8(or_, og, ob, 0);

    let bloom_paint = {
        let mut p = tiny_skia::Paint::default();
        p.shader = tiny_skia::RadialGradient::new(
            tiny_skia::Point::from_xy(px as f32, py as f32),
            tiny_skia::Point::from_xy(px as f32, py as f32), // focal = center
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

    let bloom_rect = tiny_skia::Rect::from_xywh(
        (px - bloom_r as f64) as f32, (py - bloom_r as f64) as f32,
        bloom_r * 2.0, bloom_r * 2.0,
    );
    if let Some(r) = bloom_rect {
        pm.fill_rect(r, &bloom_paint, tiny_skia::Transform::identity(), None);
    }

    // --- Focus rect highlight ---
    // Cyan glow border + faint fill, matching Swift AgentCursor.showFocusRect.
    if let Some([fx, fy, fw, fh]) = rs.focus_rect {
        let t = rs.focus_rect_t as f32;
        let fade = (1.0 - t) * (1.0 - t); // quadratic ease-out
        let border_a = (230.0 * fade * alpha_scale) as u8;
        let fill_a   = (20.0  * fade * alpha_scale) as u8;
        // Cyan: #5EC0E8
        let (cr, cg, cb) = (0x5Eu8, 0xC0u8, 0xE8u8);

        let rect = tiny_skia::Rect::from_xywh(fx as f32, fy as f32, fw as f32, fh as f32);
        if let Some(rect) = rect {
            // Faint fill
            let mut fill_paint = tiny_skia::Paint::default();
            fill_paint.shader = tiny_skia::Shader::SolidColor(
                tiny_skia::Color::from_rgba8(cr, cg, cb, fill_a)
            );
            pm.fill_rect(rect, &fill_paint, tiny_skia::Transform::identity(), None);

            // Border stroke (2px glow)
            let mut border_paint = tiny_skia::Paint::default();
            border_paint.shader = tiny_skia::Shader::SolidColor(
                tiny_skia::Color::from_rgba8(cr, cg, cb, border_a)
            );
            border_paint.anti_alias = true;
            let stroke = tiny_skia::Stroke { width: 2.5, ..Default::default() };
            let mut pb = tiny_skia::PathBuilder::new();
            pb.push_rect(rect);
            if let Some(path) = pb.finish() {
                pm.stroke_path(&path, &border_paint, &stroke,
                               tiny_skia::Transform::identity(), None);
            }
        }
    }

    // --- Click pulse ring ---
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

    // --- Arrow (custom shape or default gradient arrow) ---
    if let Some(ref shape) = rs.shape {
        // Custom icon: draw as a 32×32 image centered at (px, py), opacity-faded.
        let sz = 32.0_f32;
        if let Some(pix) = tiny_skia::PixmapRef::from_bytes(&shape.pixels, shape.width, shape.height) {
            let transform = tiny_skia::Transform::from_rotate_at(
                heading.to_degrees() as f32 + 180.0,
                px as f32, py as f32,
            ).pre_translate(px as f32 - sz / 2.0, py as f32 - sz / 2.0);
            let mut paint = tiny_skia::PixmapPaint::default();
            paint.opacity = alpha_scale;
            pm.draw_pixmap(0, 0, pix, &paint, transform, None);
        }
    } else {
        draw_default_arrow(
            &mut pm, &rs.palette,
            if rs.gradient_colors.is_empty() { None } else { Some(&rs.gradient_colors) },
            px as f32, py as f32, heading as f32, alpha_scale,
        );
    }

    pm
}

fn draw_default_arrow(
    pm: &mut tiny_skia::Pixmap,
    palette: &Palette,
    gradient_override: Option<&Vec<[u8; 4]>>,
    px: f32, py: f32,
    heading: f32,
    alpha_scale: f32,
) {
    // Arrow vertices (tip at +x).
    let verts: [(f32, f32); 4] = [(14.0, 0.0), (-8.0, -9.0), (-3.0, 0.0), (-8.0, 9.0)];

    // Rotate by (heading + π) so tip points in the motion direction.
    let angle = heading + std::f64::consts::PI as f32;
    let (sa, ca) = (angle.sin(), angle.cos());
    let transform_pt = |(vx, vy): (f32, f32)| -> (f32, f32) {
        (px + ca * vx - sa * vy, py + sa * vx + ca * vy)
    };

    let pts: Vec<(f32, f32)> = verts.iter().map(|&v| transform_pt(v)).collect();

    let mut pb = tiny_skia::PathBuilder::new();
    pb.move_to(pts[0].0, pts[0].1);
    for p in &pts[1..] { pb.line_to(p.0, p.1); }
    pb.close();
    let arrow_path = match pb.finish() { Some(p) => p, None => return };

    // Gradient fill: start color at tip, end color at tail.
    // Use runtime overrides when available, otherwise fall back to palette.
    let tip  = pts[0];
    let tail = ((pts[1].0 + pts[3].0) / 2.0, (pts[1].1 + pts[3].1) / 2.0);
    let (r0, g0, b0) = if let Some(g) = gradient_override.and_then(|g| g.first()) {
        (g[0], g[1], g[2])
    } else {
        let [r, g, b, _] = palette.cursor_start; (r, g, b)
    };
    let (r1, g1, b1) = if let Some(g) = gradient_override.and_then(|g| g.get(1).or_else(|| g.first())) {
        (g[0], g[1], g[2])
    } else {
        let [r, g, b, _] = palette.cursor_mid; (r, g, b)
    };
    let (r2, g2, b2) = if let Some(g) = gradient_override.and_then(|g| g.last()) {
        (g[0], g[1], g[2])
    } else {
        let [r, g, b, _] = palette.cursor_end; (r, g, b)
    };

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
        ).unwrap_or(tiny_skia::Shader::SolidColor(
            tiny_skia::Color::from_rgba8(r1, g1, b1, a)
        ));
        p.anti_alias = true;
        p
    };

    pm.fill_path(&arrow_path, &fill_paint, tiny_skia::FillRule::Winding,
                 tiny_skia::Transform::identity(), None);

    // White outline (faded with alpha_scale).
    let mut stroke_paint = tiny_skia::Paint::default();
    stroke_paint.shader = tiny_skia::Shader::SolidColor(tiny_skia::Color::from_rgba8(255, 255, 255, a));
    stroke_paint.anti_alias = true;
    let stroke = tiny_skia::Stroke { width: 1.5, ..Default::default() };
    pm.stroke_path(&arrow_path, &stroke_paint, &stroke, tiny_skia::Transform::identity(), None);
}

// ── AppKit / CGImage plumbing ─────────────────────────────────────────────

unsafe fn run_appkit(_cfg: CursorConfig, rx: std::sync::mpsc::Receiver<OverlayCommand>) {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    use objc2_foundation::{NSPoint, NSRect, NSSize};

    // ---- NSApplication ----
    // Verify main thread (MainThreadMarker is a zero-size compile-time token).
    let _mtm = objc2_foundation::MainThreadMarker::new()
        .expect("run_appkit must be called from the main thread");

    let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
    // NSApplicationActivationPolicyAccessory = 1 (no Dock icon, no menu bar)
    // setActivationPolicy: returns BOOL (success), not void.
    let _: bool = msg_send![app, setActivationPolicy: 1i64];
    // Finish launching without presenting a UI (needed for NSApp.run())
    let _: () = msg_send![app, finishLaunching];

    // ---- Main screen frame ----
    let main_screen: *mut AnyObject = msg_send![class!(NSScreen), mainScreen];
    if main_screen.is_null() {
        // Headless environment (CI without display) — skip overlay entirely.
        // The MCP server continues on the background thread.
        return;
    }
    let screen_frame: NSRect = msg_send![main_screen, frame];
    let win_w = screen_frame.size.width;
    let win_h = screen_frame.size.height;

    // ---- NSWindow: single alloc + initWithContentRect:... ----
    let win: *mut AnyObject = {
        let allocated: *mut AnyObject = msg_send![class!(NSWindow), alloc];
        // NSWindowStyleMaskBorderless = 0
        // NSBackingStoreBuffered = 2
        let w: *mut AnyObject = msg_send![allocated,
            initWithContentRect: screen_frame
            styleMask: 0u64
            backing: 2u64
            defer: false
        ];
        w
    };
    if win.is_null() { return; }

    let _: () = msg_send![win, setOpaque: false];
    let clear: *mut AnyObject = msg_send![class!(NSColor), clearColor];
    let _: () = msg_send![win, setBackgroundColor: clear];
    let _: () = msg_send![win, setHasShadow: false];
    let _: () = msg_send![win, setIgnoresMouseEvents: true];
    // NSNormalWindowLevel = 0.  The overlay lives at the normal window level so
    // it appears in CGWindowList layer=0 results (which agents inspect via
    // list_windows).  Z-ordering above the target is managed dynamically via
    // orderWindow:relativeTo: (see dispatch_pin_above / render_loop repin).
    let _: () = msg_send![win, setLevel: 0i64];
    // NSWindowCollectionBehaviorCanJoinAllSpaces(1<<0) | FullScreenAuxiliary(1<<8) | Stationary(1<<4)
    let _: () = msg_send![win, setCollectionBehavior: (1u64 | (1<<8) | (1<<4))];
    let _: () = msg_send![win, setReleasedWhenClosed: false];
    let _: () = msg_send![win, setHidesOnDeactivate: false];

    // ---- Layer-backed content view ----
    let content_view: *mut AnyObject = msg_send![win, contentView];
    let _: () = msg_send![content_view, setWantsLayer: true];
    let layer: *mut AnyObject = msg_send![content_view, layer];

    // Set layer geometry
    let _: () = msg_send![layer, setContentsScale: 1.0_f64];
    // kCAGravityTopLeft — the string literal "topLeft"
    let gravity_ns: *mut AnyObject = msg_send![class!(NSString),
        stringWithUTF8String: b"topLeft\0".as_ptr()
    ];
    let _: () = msg_send![layer, setContentsGravity: gravity_ns];

    // ---- Update RenderState with screen size ----
    {
        let mut guard = RENDER.lock().unwrap();
        if let Some(rs) = guard.as_mut() {
            rs.win_w = win_w;
            rs.win_h = win_h;
        }
    }

    // ---- Show the window ----
    let _: () = msg_send![win, orderFrontRegardless];

    // ---- Render thread (60 fps) ----
    let layer_ptr = layer as usize;
    let win_ptr   = win as usize;
    std::thread::spawn(move || {
        render_loop(layer_ptr, win_ptr, rx, win_w, win_h);
    });

    // ---- NSApplication run loop (blocks until process exits) ----
    let _: () = msg_send![app, run];
}

fn render_loop(
    layer_ptr: usize,
    win_ptr:   usize,
    rx: std::sync::mpsc::Receiver<OverlayCommand>,
    _win_w: f64, _win_h: f64,
) {
    let target_frame_ms = Duration::from_millis(16); // ~60 fps
    let mut last_tick = Instant::now();
    // Repin bookkeeping: track last pinned wid and a frame counter for
    // the periodic defensive-repin (every ~60 frames ≈ 1 s).
    let mut last_pinned: Option<u64> = None;
    let mut repin_frames: u32 = 0;

    loop {
        let now = Instant::now();
        let dt = now.duration_since(last_tick).as_secs_f64().min(0.05);
        last_tick = now;

        // Drain incoming commands and tick animation.
        let (pinned_wid, fire_arrival) = {
            let mut guard = RENDER.lock().unwrap();
            match guard.as_mut() {
                Some(rs) => {
                    while let Ok(cmd) = rx.try_recv() {
                        rs.apply_command(cmd);
                    }
                    let arrived = rs.tick(dt);
                    (rs.pinned_wid, arrived)
                }
                None => break,
            }
        };

        // Fire arrival signal so animate_cursor_to() can unblock.
        if fire_arrival {
            if let Ok(mut guard) = ARRIVAL_TX.lock() {
                if let Some(tx) = guard.take() {
                    let _ = tx.send(());
                }
            }
        }

        // Repin: immediately on target change, then defensive every ~1 s.
        repin_frames += 1;
        let pin_changed = pinned_wid != last_pinned;
        last_pinned = pinned_wid;
        if let Some(wid) = pinned_wid {
            if pin_changed || repin_frames >= 60 {
                dispatch_pin_above(win_ptr, wid);
                repin_frames = 0;
            }
        } else if repin_frames >= 60 {
            repin_frames = 0;
        }

        // Render frame.
        let pixmap = {
            let guard = RENDER.lock().unwrap();
            if let Some(rs) = guard.as_ref() {
                render_frame(rs)
            } else {
                break;
            }
        };

        // Convert to CGImage and update layer on the main queue.
        dispatch_set_layer_contents(layer_ptr, pixmap);

        // Sleep remainder of frame budget.
        let elapsed = Instant::now().duration_since(last_tick);
        if let Some(remaining) = target_frame_ms.checked_sub(elapsed) {
            std::thread::sleep(remaining);
        }
    }
}

/// Convert a `tiny_skia::Pixmap` to a `CGImage` and set it as the contents
/// of the given `CALayer` via `dispatch_async(main_queue, ...)`.
fn dispatch_set_layer_contents(layer_ptr: usize, pixmap: tiny_skia::Pixmap) {
    // Build the CGImage from the pixmap bytes.
    let cg_image_ptr = match pixmap_to_cgimage(&pixmap) {
        Some(p) => p,
        None => return,
    };

    // Box the payload for the C callback.
    let payload = Box::new((layer_ptr, cg_image_ptr));

    // GCD symbols from libdispatch (part of the macOS system library stubs).
    // `dispatch_get_main_queue()` is an inline C function; the underlying
    // symbol is `_dispatch_main_q`, a *struct* (not a pointer).
    // We declare it as `u8` (opaque placeholder) and take its ADDRESS to
    // obtain the `dispatch_queue_t` (pointer to the struct).
    #[link(name = "dispatch", kind = "dylib")]
    extern "C" {
        // Opaque placeholder — we only ever take &_dispatch_main_q, never read it.
        static _dispatch_main_q: u8;
        fn dispatch_async_f(queue: *const c_void, context: *mut c_void,
                            work: unsafe extern "C" fn(*mut c_void));
    }

    unsafe extern "C" fn set_contents_cb(ctx: *mut c_void) {
        let (layer_ptr, cg_image_ptr): (usize, usize) = *Box::from_raw(ctx as *mut _);
        let layer = layer_ptr as *mut objc2::runtime::AnyObject;
        // setContents: expects an `id` (type '@'), not a raw void pointer.
        // CGImageRef is toll-free bridged to NSObject, so we cast it to *mut AnyObject.
        let cg_id = cg_image_ptr as *mut objc2::runtime::AnyObject;
        let _: () = objc2::msg_send![layer, setContents: cg_id];
        // Release the CGImage ref we retained in pixmap_to_cgimage.
        CGImageRelease(cg_image_ptr as *mut c_void);
    }

    extern "C" { fn CGImageRelease(image: *mut c_void); }

    unsafe {
        // &_dispatch_main_q is the queue pointer (same as dispatch_get_main_queue()).
        let main_queue = &raw const _dispatch_main_q as *const c_void;
        dispatch_async_f(
            main_queue,
            Box::into_raw(payload) as *mut c_void,
            set_contents_cb,
        );
    }
}

/// Order the overlay NSWindow just above `target_wid` in the global window
/// server list.  Called from the render thread; dispatches to the main queue
/// (AppKit must be used on the main thread).
///
/// `NSWindowAbove = 1`; `orderWindow:relativeTo:` accepts any CGWindowID as
/// the `relativeTo` argument — it works cross-application via CGS.
fn dispatch_pin_above(win_ptr: usize, target_wid: u64) {
    use std::ffi::c_void;

    #[link(name = "dispatch", kind = "dylib")]
    extern "C" {
        static _dispatch_main_q: u8;
        fn dispatch_async_f(
            queue: *const c_void,
            context: *mut c_void,
            work: unsafe extern "C" fn(*mut c_void),
        );
    }

    unsafe extern "C" fn reorder_cb(ctx: *mut c_void) {
        let (win_ptr, target_wid): (usize, u64) = *Box::from_raw(ctx as *mut (usize, u64));
        let win = win_ptr as *mut objc2::runtime::AnyObject;
        // NSWindowAbove = 1; relativeTo: takes NSInteger (i64 on 64-bit)
        let _: () = objc2::msg_send![win, orderWindow: 1i64 relativeTo: target_wid as i64];
    }

    let payload = Box::new((win_ptr, target_wid));
    unsafe {
        let main_queue = &raw const _dispatch_main_q as *const c_void;
        dispatch_async_f(
            main_queue,
            Box::into_raw(payload) as *mut c_void,
            reorder_cb,
        );
    }
}

/// Create a `CGImage` from a `tiny_skia::Pixmap` (premultiplied RGBA).
/// Returns a `+1` retained pointer that the caller must release.
fn pixmap_to_cgimage(pixmap: &tiny_skia::Pixmap) -> Option<usize> {
    let w = pixmap.width() as usize;
    let h = pixmap.height() as usize;
    if w == 0 || h == 0 { return None; }

    let data = pixmap.data();
    let bytes_per_row = w * 4;

    // tiny-skia produces premultiplied RGBA with bytes in memory order [R, G, B, A].
    // CGImage flag breakdown (Apple CGBitmapInfo / CGImageAlphaInfo enums):
    //   kCGImageAlphaPremultipliedLast = 0x0001  → alpha is the LAST channel  (RGBA)
    //   kCGImageAlphaPremultipliedFirst = 0x0002 → alpha is the FIRST channel (ARGB)  ← NOT what we want
    //   kCGBitmapByteOrder32Big        = 0x4000  → big-endian 32-bit pixel,
    //     so memory order is the same as component order (bytes = [R, G, B, A]).
    // Combined: kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big = 0x4001
    // This correctly maps tiny-skia's [R, G, B, A] bytes to the display RGB channels.
    const BITMAP_INFO: u32 = 0x0001 | 0x4000; // kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big

    // Release callback: CGDataProvider calls this when it is done with the buffer.
    // `info` is the Box<Vec<u8>> we passed as the `info` argument below.
    unsafe extern "C" fn release_pixel_data(info: *mut c_void, _data: *const c_void, _size: usize) {
        // Re-box and drop to free the buffer.
        drop(Box::from_raw(info as *mut Vec<u8>));
    }

    unsafe {
        extern "C" {
            fn CGColorSpaceCreateDeviceRGB() -> *mut c_void;
            fn CGColorSpaceRelease(cs: *mut c_void);
            fn CGDataProviderCreateWithData(
                info: *mut c_void,
                data: *const c_void,
                size: usize,
                release_data: Option<unsafe extern "C" fn(*mut c_void, *const c_void, usize)>,
            ) -> *mut c_void;
            fn CGDataProviderRelease(provider: *mut c_void);
            fn CGImageCreate(
                width: usize, height: usize,
                bits_per_component: usize,
                bits_per_pixel: usize,
                bytes_per_row: usize,
                color_space: *mut c_void,
                bitmap_info: u32,
                provider: *mut c_void,
                decode: *const f64,
                should_interpolate: bool,
                intent: u32,
            ) -> *mut c_void;
        }

        // Copy the pixel data into a heap Vec; the data provider will own it
        // and free it via release_pixel_data when the CGImage is released.
        let copied: Vec<u8> = data.to_vec();
        let len = copied.len();
        let ptr = copied.as_ptr();
        // Leak the Vec into a raw Box so we can pass it as the `info` opaque pointer.
        let copied_box: *mut Vec<u8> = Box::into_raw(Box::new(copied));

        let cs = CGColorSpaceCreateDeviceRGB();
        let provider = CGDataProviderCreateWithData(
            copied_box as *mut c_void,
            ptr as *const c_void,
            len,
            Some(release_pixel_data), // frees copied_box when provider is released
        );
        let img = CGImageCreate(
            w, h,
            8,  // bits_per_component
            32, // bits_per_pixel
            bytes_per_row,
            cs,
            BITMAP_INFO,
            provider,
            std::ptr::null(),
            false,
            0, // kCGRenderingIntentDefault
        );

        CGColorSpaceRelease(cs);
        CGDataProviderRelease(provider);
        // Do NOT drop copied_box here — release_pixel_data owns it now.

        if img.is_null() { None } else { Some(img as usize) }
    }
}
