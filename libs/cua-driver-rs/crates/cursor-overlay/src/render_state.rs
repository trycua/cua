//! Shared cursor-overlay render state, animation tick, and pixel pipeline.
//!
//! Lifts the platform-agnostic render state out of the three per-OS
//! `overlay.rs` files (macOS / Windows / Linux). Before the 2026-05 dedup
//! audit each platform owned a ~600-line copy of the same animation logic
//! that differed only in a few constants and feature flags.
//!
//! ## What lives here
//!
//! - [`RenderStateCore`] — the platform-agnostic animation fields
//!   (`cfg`, `palette`, `motion`, `pos`, `heading`, `path`, `dist`, `spring`,
//!   `spring_tgt`, `click_t`, `shape`, `visible`, `idle_secs`, `idle_alpha`,
//!   `pinned_wid`, `gradient_colors`, `bloom_override`).
//! - [`RenderStateCore::tick_motion`] — speed-profile + spring physics +
//!   click-pulse + idle-fade using runtime [`MotionConfig`] (Windows + Linux).
//! - [`RenderStateCore::tick_swift_constants`] — same physics but with the
//!   hardcoded Swift reference constants used by macOS; returns whether the
//!   path just ended (so the caller can fire arrival signals).
//! - [`RenderStateCore::apply_command_base`] — the OverlayCommand match arms
//!   that all three platforms implement identically (MoveTo / ClickPulse /
//!   SetEnabled / SetMotion / SetPalette / PinAbove / SetShape / SetGradient).
//!   Returns `false` for variants the core doesn't handle so platforms can
//!   layer their own behaviour on top (e.g. macOS ShowFocusRect).
//! - [`render_frame`] — the tiny-skia paint of bloom + click-pulse + arrow.
//!   Parametrised by pixmap dimensions and an origin offset so Windows can
//!   pass `(virt_x, virt_y)` while macOS / Linux pass `(0, 0)`.
//! - [`draw_default_arrow`] — gradient-arrow rasteriser.
//!
//! ## What stays per-platform
//!
//! - The OS window / surface (NSWindow / HWND / X11 Window) and its message
//!   loop or run-loop.
//! - The paint dispatch: `dispatch_set_layer_contents` (CGImage),
//!   `UpdateLayeredWindow` (BGRA DIB), `XPutImage` (BGRA ZPixmap).
//! - Origin/coordinate translation (Windows uses virtual-screen offset;
//!   macOS uses NSScreen coordinates; Linux uses display coordinates).
//! - Platform-specific extras like macOS's `focus_rect` (post-arrival
//!   element highlight — drawn inside [`render_frame`] when the caller
//!   supplies one via the optional argument).

use crate::{
    CursorConfig, CursorShape, MotionConfig, OverlayCommand, Palette, PathPlanner, PathState,
    PlannedPath, Spring,
};

/// Platform-agnostic render state shared by macOS / Windows / Linux overlays.
///
/// Each platform wraps this in its own struct that adds OS-specific fields
/// (e.g. `virt_x/y/w/h` on Windows, `focus_rect` on macOS).
pub struct RenderStateCore {
    /// Frozen copy of the launch-time CursorConfig.
    pub cfg: CursorConfig,
    /// Current colour palette (mutable via [`OverlayCommand::SetPalette`]).
    pub palette: Palette,
    /// Current motion / timing config (mutable via [`OverlayCommand::SetMotion`]).
    pub motion: MotionConfig,
    /// Current rendered position in screen / overlay-window coordinates.
    pub pos: (f64, f64),
    /// Visual heading in radians (tip direction = motion_dir + π).
    pub heading: f64,
    /// In-flight planned path; `None` = at rest.
    pub path: Option<PlannedPath>,
    /// Arc-distance travelled along the current path so far.
    pub dist: f64,
    /// Post-arrival spring-settle state.
    pub spring: Option<Spring>,
    /// Target the spring is settling toward: `(x, y, heading)`.
    pub spring_tgt: Option<(f64, f64, f64)>,
    /// Click-pulse phase 0..1; `None` = no pulse in flight.
    pub click_t: Option<f64>,
    /// Custom cursor shape; `None` = built-in gradient arrow.
    pub shape: Option<CursorShape>,
    /// User-controlled visibility.
    pub visible: bool,
    /// Idle-hide: elapsed seconds since last activity.
    pub idle_secs: f64,
    /// Idle-hide fade: 1.0 = fully visible, 0.0 = fully hidden.
    pub idle_alpha: f64,
    /// Window id the overlay should be pinned above (for z-ordering).
    pub pinned_wid: Option<u64>,
    /// Runtime-overridden gradient colours (from `set_agent_cursor_style`).
    /// Empty = use palette defaults.
    pub gradient_colors: Vec<[u8; 4]>,
    /// Runtime-overridden bloom colour.  `None` = palette default.
    pub bloom_override: Option<[u8; 4]>,
}

impl RenderStateCore {
    /// Build the core from a launch-time CursorConfig.
    /// `pos` starts at the off-screen sentinel `(-200, -200)` to indicate
    /// "never placed on screen yet" — the click path uses this to detect
    /// first-placement and snap rather than animate.
    pub fn new(cfg: CursorConfig) -> Self {
        let palette = cfg.palette();
        let motion = cfg.motion.clone();
        let shape = cfg.shape.clone();
        Self {
            cfg,
            palette,
            motion,
            shape,
            gradient_colors: vec![],
            bloom_override: None,
            pos: (-200.0, -200.0),
            heading: std::f64::consts::FRAC_PI_4,
            path: None,
            dist: 0.0,
            spring: None,
            spring_tgt: None,
            click_t: None,
            visible: true,
            idle_secs: 0.0,
            idle_alpha: 1.0,
            pinned_wid: None,
        }
    }

    /// Advance the animation by `dt` seconds using runtime [`MotionConfig`]
    /// for peak / floor / spring constants. Used by Windows + Linux.
    ///
    /// The speed profile is `16·u²·(1-u)²` (peaks at 1.0 at u=0.5) — the
    /// 1:1 port of `AgentCursorRenderer`'s smootherstep envelope. Floor
    /// speed switches from `min_start_speed` to `min_end_speed` at the
    /// midpoint so the cursor decelerates as it approaches the target.
    /// Spring overshoot is `0.5` (Windows/Linux convention).
    pub fn tick_motion(&mut self, dt: f64) {
        let spring_k = self.motion.spring * 400.0;
        let spring_c = self.motion.spring * 20.0;

        if let Some(ref p) = self.path {
            let path_frac = (self.dist / p.length.max(1.0)).clamp(0.0, 1.0);
            let profile =
                16.0 * path_frac * path_frac * (1.0 - path_frac) * (1.0 - path_frac);
            let floor = if path_frac < 0.5 {
                self.motion.min_start_speed
            } else {
                self.motion.min_end_speed
            };
            let speed = (floor + (self.motion.peak_speed - floor) * profile).max(floor);
            self.dist += speed * dt;

            let path_len = p.length.max(1.0);
            if self.dist >= path_len {
                let end = p.sample(path_len);
                let end_heading = p.end_visual_heading;
                let vh = end.heading;
                self.spring = Some(Spring {
                    ox: 0.0,
                    oy: 0.0,
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
                self.heading = crate::util::rotate_toward(self.heading, desired, max_step);
            }
        } else if let Some(mut s) = self.spring {
            if let Some((tx, ty, th)) = self.spring_tgt {
                let substeps = 4;
                let sdt = dt / substeps as f64;
                for _ in 0..substeps {
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

        self.tick_idle(dt);
    }

    /// Advance the animation by `dt` seconds using the hardcoded Swift
    /// reference constants (`peakSpeed=900`, `minStart=300`, `minEnd=200`,
    /// `springK=400`, `springC=17`, `springOvershoot=0.8`).  Used by macOS,
    /// which mirrors `AgentCursorRenderer.swift` 1:1.
    ///
    /// Returns `true` when the path just ended (so the caller can fire its
    /// arrival oneshot to unblock `animate_cursor_to`).
    ///
    /// The speed profile is `(30·u²·(1-u)²) / 1.875` which is algebraically
    /// equivalent to the `16·u²·(1-u)²` form used by [`tick_motion`]; both
    /// peak at 1.0 at u=0.5.  The original Swift code uses the 30/1.875
    /// form so we preserve it here for parity.
    pub fn tick_swift_constants(&mut self, dt: f64) -> bool {
        const PEAK_SPEED: f64 = 900.0;
        const MIN_START_SPEED: f64 = 300.0;
        const MIN_END_SPEED: f64 = 200.0;
        const SPRING_K: f64 = 400.0;
        const SPRING_C: f64 = 17.0;
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
                    ox: 0.0,
                    oy: 0.0,
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
                self.heading = crate::util::rotate_toward(self.heading, desired, max_step);
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

        self.tick_idle(dt);

        fire_arrival
    }

    /// Shared idle-hide / fade logic — accumulate idle time when nothing is
    /// moving, then fade `idle_alpha` from 1→0 over 180ms once
    /// `motion.idle_hide_ms` has elapsed.  Identical across all platforms.
    fn tick_idle(&mut self, dt: f64) {
        let idle_hide_ms = self.motion.idle_hide_ms;
        if idle_hide_ms > 0.0 {
            let moving =
                self.path.is_some() || self.spring.is_some() || self.click_t.is_some();
            if moving {
                self.idle_secs = 0.0;
                self.idle_alpha = 1.0;
            } else {
                self.idle_secs += dt;
                let fade_start = idle_hide_ms / 1000.0;
                let fade_end = fade_start + 0.18; // 180ms fade like Windows ref
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

    /// Handle the OverlayCommand variants that are identical across all
    /// three platforms.  Returns `true` if the command was consumed; `false`
    /// for variants the platform must handle itself (e.g. macOS's
    /// `ShowFocusRect`).
    ///
    /// `move_to_snap_sentinel` controls macOS-only behaviour: when `true`,
    /// `MoveTo` snaps `self.pos` to the offset target if the cursor is
    /// still at the off-screen sentinel (`pos.0 < -50.0`).  Windows/Linux
    /// pass `false` here.
    ///
    /// `click_pulse_sentinel_only` likewise controls macOS-only behaviour:
    /// when `true`, `ClickPulse` only updates `self.pos` if the cursor is
    /// still at the sentinel (the animation already landed it there
    /// otherwise).  Windows/Linux pass `false`, which always snaps
    /// `self.pos` to the click point.
    pub fn apply_command_base(
        &mut self,
        cmd: OverlayCommand,
        move_to_snap_sentinel: bool,
        click_pulse_sentinel_only: bool,
    ) -> bool {
        match cmd {
            OverlayCommand::MoveTo {
                x,
                y,
                end_heading_radians,
            } => {
                // Apply click offset (16 pt along end_heading) before planning,
                // matching Swift `moveTo(point:endAngleRadians:)`:
                //   tx = clickPoint.x + cos(endAngle) * clickOffset
                //   ty = clickPoint.y + sin(endAngle) * clickOffset
                const CLICK_OFFSET: f64 = 16.0;
                const TURN_RADIUS: f64 = 80.0;
                let tx = x + end_heading_radians.cos() * CLICK_OFFSET;
                let ty = y + end_heading_radians.sin() * CLICK_OFFSET;

                // macOS-only: if the cursor is still at the initial off-screen
                // sentinel, snap it to the offset target so the path starts on-screen.
                if move_to_snap_sentinel && self.pos.0 < -50.0 {
                    self.pos = (tx, ty);
                }
                let (x0, y0) = self.pos;
                let th0 = self.heading + std::f64::consts::PI;
                let th1 = end_heading_radians + std::f64::consts::PI;
                let plan = PathPlanner::plan(
                    x0,
                    y0,
                    th0,
                    tx,
                    ty,
                    th1,
                    end_heading_radians,
                    TURN_RADIUS,
                );
                self.path = Some(plan);
                self.dist = 0.0;
                self.spring = None;
                self.spring_tgt = None;
                self.idle_secs = 0.0;
                self.idle_alpha = 1.0;
                true
            }
            OverlayCommand::ClickPulse { x, y } => {
                if click_pulse_sentinel_only {
                    // macOS: only snap position on first placement (sentinel state).
                    // After that the cursor stays where the animation landed.
                    if self.pos.0 < -50.0 {
                        // Apply same click offset so tip lands at click point.
                        const CLICK_OFFSET: f64 = 16.0;
                        let angle = std::f64::consts::FRAC_PI_4;
                        self.pos = (
                            x + angle.cos() * CLICK_OFFSET,
                            y + angle.sin() * CLICK_OFFSET,
                        );
                    }
                } else {
                    self.pos = (x, y);
                }
                self.click_t = Some(0.0);
                self.idle_secs = 0.0;
                self.idle_alpha = 1.0;
                true
            }
            OverlayCommand::SetEnabled(v) => {
                self.visible = v;
                true
            }
            OverlayCommand::SetMotion(m) => {
                self.motion = m;
                true
            }
            OverlayCommand::SetPalette(p) => {
                self.palette = p;
                true
            }
            OverlayCommand::PinAbove(wid) => {
                self.pinned_wid = Some(wid);
                true
            }
            OverlayCommand::SetShape(shape) => {
                self.shape = shape;
                true
            }
            OverlayCommand::SetGradient {
                gradient_colors,
                bloom_color,
            } => {
                self.gradient_colors = gradient_colors;
                self.bloom_override = bloom_color;
                true
            }
            OverlayCommand::ShowFocusRect(_) => false, // caller-specific
        }
    }
}

// ── tiny-skia rendering ──────────────────────────────────────────────────

/// Optional focus-rect overlay drawn on top of the cursor (macOS only at
/// the moment — the other platforms always pass `None`).
#[derive(Clone, Copy)]
pub struct FocusRect {
    /// Rectangle `[x, y, w, h]` in screen coordinates (top-left origin),
    /// relative to the same origin the cursor `pos` uses.
    pub rect: [f64; 4],
    /// Fade progress 0.0 = fully visible, 1.0 = gone.
    pub t: f64,
}

/// Render the cursor + bloom + click-pulse + (optional) focus-rect into a
/// fresh tiny-skia [`tiny_skia::Pixmap`] of `(width, height)`.
///
/// `origin_x`, `origin_y` are subtracted from the cursor `core.pos` before
/// drawing — Windows passes the virtual-screen `(virt_x, virt_y)` so the
/// pixmap is laid out in window-local coordinates.  macOS / Linux pass
/// `(0.0, 0.0)`.
pub fn render_frame(
    core: &RenderStateCore,
    width: u32,
    height: u32,
    origin_x: f64,
    origin_y: f64,
    focus_rect: Option<FocusRect>,
) -> tiny_skia::Pixmap {
    let w = width.max(1);
    let h = height.max(1);
    let mut pm = tiny_skia::Pixmap::new(w, h)
        .unwrap_or_else(|| tiny_skia::Pixmap::new(1, 1).unwrap());

    if !core.visible || core.pos.0 < -100.0 || core.idle_alpha < 0.004 {
        return pm;
    }

    let (px, py) = (core.pos.0 - origin_x, core.pos.1 - origin_y);
    let heading = core.heading;
    let alpha_scale = core.idle_alpha as f32;

    // --- Bloom (radial gradient behind the arrow) ---
    let bloom_r: f32 = 22.0;
    // Use runtime bloom_override if set, otherwise fall back to palette.
    let (br, bg, bb) = if let Some([r, g, b, _]) = core.bloom_override {
        (r, g, b)
    } else {
        let [r, g, b, _] = core.palette.bloom_inner;
        (r, g, b)
    };
    let bloom_inner = tiny_skia::Color::from_rgba8(br, bg, bb, (115.0 * alpha_scale) as u8);
    let (or_, og, ob) = if let Some([r, g, b, _]) = core.bloom_override {
        (r, g, b)
    } else {
        let [r, g, b, _] = core.palette.bloom_outer;
        (r, g, b)
    };
    let bloom_outer = tiny_skia::Color::from_rgba8(or_, og, ob, (26.0 * alpha_scale) as u8);
    let bloom_zero = tiny_skia::Color::from_rgba8(or_, og, ob, 0);

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
        )
        .unwrap_or(tiny_skia::Shader::SolidColor(bloom_inner));
        p.anti_alias = true;
        p
    };

    if let Some(r) = tiny_skia::Rect::from_xywh(
        (px - bloom_r as f64) as f32,
        (py - bloom_r as f64) as f32,
        bloom_r * 2.0,
        bloom_r * 2.0,
    ) {
        pm.fill_rect(r, &bloom_paint, tiny_skia::Transform::identity(), None);
    }

    // --- Focus rect highlight (macOS only — others pass None) ---
    // Cyan glow border + faint fill, matching Swift AgentCursor.showFocusRect.
    if let Some(fr) = focus_rect {
        let [fx, fy, fw, fh] = fr.rect;
        let t = fr.t as f32;
        let fade = (1.0 - t) * (1.0 - t); // quadratic ease-out
        let border_a = (230.0 * fade * alpha_scale) as u8;
        let fill_a = (20.0 * fade * alpha_scale) as u8;
        // Cyan: #5EC0E8
        let (cr, cg, cb) = (0x5Eu8, 0xC0u8, 0xE8u8);

        if let Some(rect) =
            tiny_skia::Rect::from_xywh(fx as f32, fy as f32, fw as f32, fh as f32)
        {
            // Faint fill
            let mut fill_paint = tiny_skia::Paint::default();
            fill_paint.shader = tiny_skia::Shader::SolidColor(
                tiny_skia::Color::from_rgba8(cr, cg, cb, fill_a),
            );
            pm.fill_rect(rect, &fill_paint, tiny_skia::Transform::identity(), None);

            // Border stroke (2px glow)
            let mut border_paint = tiny_skia::Paint::default();
            border_paint.shader = tiny_skia::Shader::SolidColor(
                tiny_skia::Color::from_rgba8(cr, cg, cb, border_a),
            );
            border_paint.anti_alias = true;
            let stroke = tiny_skia::Stroke {
                width: 2.5,
                ..Default::default()
            };
            let mut pb = tiny_skia::PathBuilder::new();
            pb.push_rect(rect);
            if let Some(path) = pb.finish() {
                pm.stroke_path(
                    &path,
                    &border_paint,
                    &stroke,
                    tiny_skia::Transform::identity(),
                    None,
                );
            }
        }
    }

    // --- Click pulse ring ---
    if let Some(t) = core.click_t {
        let ring_r = (bloom_r + 20.0 * t as f32) * (1.0 - t as f32 * 0.5);
        let alpha = ((1.0 - t) * 180.0 * alpha_scale as f64) as u8;
        let [cr, cg, cb, _] = core.palette.cursor_mid;
        let ring_color = tiny_skia::Color::from_rgba8(cr, cg, cb, alpha);
        let mut ring_paint = tiny_skia::Paint::default();
        ring_paint.shader = tiny_skia::Shader::SolidColor(ring_color);
        ring_paint.anti_alias = true;
        let stroke = tiny_skia::Stroke {
            width: 2.0,
            ..Default::default()
        };
        let mut pb = tiny_skia::PathBuilder::new();
        pb.push_circle(px as f32, py as f32, ring_r);
        if let Some(path) = pb.finish() {
            pm.stroke_path(
                &path,
                &ring_paint,
                &stroke,
                tiny_skia::Transform::identity(),
                None,
            );
        }
    }

    // --- Arrow (custom shape or default gradient arrow) ---
    if let Some(ref shape) = core.shape {
        // Custom icon: draw as a 32×32 image centered at (px, py), opacity-faded.
        let sz = 32.0_f32;
        if let Some(pix) =
            tiny_skia::PixmapRef::from_bytes(&shape.pixels, shape.width, shape.height)
        {
            let transform = tiny_skia::Transform::from_rotate_at(
                heading.to_degrees() as f32 + 180.0,
                px as f32,
                py as f32,
            )
            .pre_translate(px as f32 - sz / 2.0, py as f32 - sz / 2.0);
            let mut paint = tiny_skia::PixmapPaint::default();
            paint.opacity = alpha_scale;
            pm.draw_pixmap(0, 0, pix, &paint, transform, None);
        }
    } else {
        let grad_override = if core.gradient_colors.is_empty() {
            None
        } else {
            Some(&core.gradient_colors)
        };
        draw_default_arrow(
            &mut pm,
            &core.palette,
            grad_override,
            px as f32,
            py as f32,
            heading as f32,
            alpha_scale,
        );
    }

    pm
}

/// Rasterise the built-in gradient arrow at `(px, py)` rotated by
/// `heading` radians.  `alpha_scale` is the idle-fade multiplier
/// (1.0 = fully opaque, 0.0 = fully faded out).
///
/// `gradient_override` lets `set_agent_cursor_style` substitute custom
/// gradient stops at runtime.  When `None` the palette's
/// `cursor_start/cursor_mid/cursor_end` are used.
pub fn draw_default_arrow(
    pm: &mut tiny_skia::Pixmap,
    palette: &Palette,
    gradient_override: Option<&Vec<[u8; 4]>>,
    px: f32,
    py: f32,
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
    for p in &pts[1..] {
        pb.line_to(p.0, p.1);
    }
    pb.close();
    let arrow_path = match pb.finish() {
        Some(p) => p,
        None => return,
    };

    // Gradient fill: start color at tip, end color at tail.
    // Use runtime overrides when available, otherwise fall back to palette.
    let tip = pts[0];
    let tail = (
        (pts[1].0 + pts[3].0) / 2.0,
        (pts[1].1 + pts[3].1) / 2.0,
    );
    let (r0, g0, b0) = if let Some(g) = gradient_override.and_then(|g| g.first()) {
        (g[0], g[1], g[2])
    } else {
        let [r, g, b, _] = palette.cursor_start;
        (r, g, b)
    };
    let (r1, g1, b1) = if let Some(g) =
        gradient_override.and_then(|g| g.get(1).or_else(|| g.first()))
    {
        (g[0], g[1], g[2])
    } else {
        let [r, g, b, _] = palette.cursor_mid;
        (r, g, b)
    };
    let (r2, g2, b2) = if let Some(g) = gradient_override.and_then(|g| g.last()) {
        (g[0], g[1], g[2])
    } else {
        let [r, g, b, _] = palette.cursor_end;
        (r, g, b)
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
        )
        .unwrap_or(tiny_skia::Shader::SolidColor(
            tiny_skia::Color::from_rgba8(r1, g1, b1, a),
        ));
        p.anti_alias = true;
        p
    };

    pm.fill_path(
        &arrow_path,
        &fill_paint,
        tiny_skia::FillRule::Winding,
        tiny_skia::Transform::identity(),
        None,
    );

    // White outline (faded with alpha_scale).
    let mut stroke_paint = tiny_skia::Paint::default();
    stroke_paint.shader =
        tiny_skia::Shader::SolidColor(tiny_skia::Color::from_rgba8(255, 255, 255, a));
    stroke_paint.anti_alias = true;
    let stroke = tiny_skia::Stroke {
        width: 1.5,
        ..Default::default()
    };
    pm.stroke_path(
        &arrow_path,
        &stroke_paint,
        &stroke,
        tiny_skia::Transform::identity(),
        None,
    );
}
