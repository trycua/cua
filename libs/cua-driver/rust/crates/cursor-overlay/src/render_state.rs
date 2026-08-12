//! Shared cursor-overlay render state, animation tick, and pixel pipeline.
//!
//! Lifts the platform-agnostic render state out of the three per-OS
//! `overlay.rs` files (macOS / Windows / Linux). Before the 2026-05 dedup
//! audit each platform owned a ~600-line copy of the same animation logic
//! that differed only in a few constants and feature flags.
//!
//! ## What lives here
//!
//! - [`RenderStateCore`] — the platform-agnostic animation and semantic state.
//! - [`RenderStateCore::tick_motion`] — speed-profile + spring physics +
//!   click-pulse + idle-fade using runtime [`MotionConfig`] (Windows + Linux).
//! - [`RenderStateCore::tick_swift_constants`] — same physics but with the
//!   hardcoded Swift reference constants used by macOS; returns whether the
//!   path just ended (so the caller can fire arrival signals).
//! - [`RenderStateCore::apply_command_base`] — the OverlayCommand match arms
//!   that all three platforms implement identically (MoveTo / ClickPulse /
//!   SetEnabled / SetMotion / SetTheme / semantic action events / PinAbove).
//!   Returns `false` for variants the core doesn't handle so platforms can
//!   layer their own behaviour on top (e.g. macOS ShowFocusRect).
//! - [`render_frame`] — the tiny-skia paint of the selected cursor theme.
//!   Parametrised by pixmap dimensions and an origin offset so Windows can
//!   pass `(virt_x, virt_y)` while macOS / Linux pass `(0, 0)`.
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
    CompiledTheme, CursorAction, CursorConfig, CursorVisualState, DeliveryModifier, MotionConfig,
    OverlayCommand, PathPlanner, PathState, PlannedPath, Spring, TargetModifier,
};
use std::sync::Arc;

pub const SESSION_BADGE_HOLD_SECS: f64 = 2.0;
pub const SESSION_BADGE_FADE_SECS: f64 = 0.4;

/// Platform-agnostic render state shared by macOS / Windows / Linux overlays.
///
/// Each platform wraps this in its own struct that adds OS-specific fields
/// (e.g. `virt_x/y/w/h` on Windows, `focus_rect` on macOS).
pub struct RenderStateCore {
    /// Frozen copy of the launch-time CursorConfig.
    pub cfg: CursorConfig,
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
    /// Whether a button is currently being held for this cursor.
    pub pressed: bool,
    /// Semantic action and animation playback state.
    pub visual: CursorVisualState,
    /// Decoded installed or embedded theme.
    pub theme: Option<Arc<CompiledTheme>>,
    /// Non-fatal launch-time fallback reason, if an installed theme failed.
    pub theme_fallback: Option<String>,
    /// User-controlled visibility.
    pub visible: bool,
    /// Idle-hide: elapsed seconds since last activity.
    pub idle_secs: f64,
    /// Idle-hide fade: 1.0 = fully visible, 0.0 = fully hidden.
    pub idle_alpha: f64,
    /// Window id the overlay should be pinned above (for z-ordering).
    pub pinned_wid: Option<u64>,
    /// Sanitized caller-facing label painted below the cursor.
    pub session_label: Option<String>,
    /// Elapsed time since the session label was revealed with the cursor.
    pub session_badge_secs: f64,
    /// Whether the user's hardware pointer is currently over this synthetic
    /// cursor. Hover temporarily reveals an already-faded session badge
    /// without changing its one-shot reveal timer.
    pub session_badge_hovered: bool,
    /// Last action-scoped delivery and target context shown in the badge.
    /// This is latched briefly after the semantic action ends so the chips
    /// can fade without keeping modifier artwork inside the Lottie theme.
    pub badge_modifiers: Option<(Option<DeliveryModifier>, Option<TargetModifier>)>,
    /// Elapsed chip fade time after the active semantic action clears.
    pub badge_modifier_fade_secs: Option<f64>,
}

impl RenderStateCore {
    /// Build the core from a launch-time CursorConfig.
    /// `pos` starts at the off-screen sentinel `(-200, -200)` to indicate
    /// "never placed on screen yet" — the click path uses this to detect
    /// first-placement and snap rather than animate.
    pub fn new(cfg: CursorConfig) -> Self {
        let motion = cfg.motion.clone();
        let visual = CursorVisualState {
            reduced_motion: cfg.reduced_motion,
            ..CursorVisualState::default()
        };
        let (theme, theme_fallback) = match crate::load_installed_theme(&cfg.theme_id) {
            Ok(theme) => (theme, None),
            Err(error) => (
                Some(crate::embedded_default_theme()),
                Some(format!(
                    "theme `{}` could not be loaded; using {}: {error}",
                    cfg.theme_id,
                    crate::DEFAULT_THEME_ID
                )),
            ),
        };
        Self {
            cfg,
            motion,
            visual,
            theme,
            theme_fallback,
            pos: (-200.0, -200.0),
            heading: std::f64::consts::FRAC_PI_4,
            path: None,
            dist: 0.0,
            spring: None,
            spring_tgt: None,
            click_t: None,
            pressed: false,
            visible: true,
            idle_secs: 0.0,
            idle_alpha: 1.0,
            pinned_wid: None,
            session_label: None,
            session_badge_secs: SESSION_BADGE_HOLD_SECS + SESSION_BADGE_FADE_SECS,
            session_badge_hovered: false,
            badge_modifiers: None,
            badge_modifier_fade_secs: None,
        }
    }

    fn cursor_is_revealed(&self) -> bool {
        self.visible && self.pos.0 >= -100.0 && self.idle_alpha >= 0.004
    }

    fn reveal_session_badge(&mut self) {
        if self.session_label.is_some() {
            self.session_badge_secs = 0.0;
        }
    }

    pub fn session_badge_alpha(&self) -> f32 {
        if self.session_label.is_none() {
            return 0.0;
        }
        if self.session_badge_hovered {
            return 1.0;
        }
        if self.session_badge_secs <= SESSION_BADGE_HOLD_SECS {
            return 1.0;
        }
        let fade = ((self.session_badge_secs - SESSION_BADGE_HOLD_SECS) / SESSION_BADGE_FADE_SECS)
            .clamp(0.0, 1.0);
        let smooth = fade * fade * (3.0 - 2.0 * fade);
        (1.0 - smooth) as f32
    }

    pub fn session_badge_chip_alpha(&self) -> f32 {
        if self.badge_modifiers.is_none() {
            return 0.0;
        }
        let Some(elapsed) = self.badge_modifier_fade_secs else {
            return 1.0;
        };
        let fade = (elapsed / SESSION_BADGE_FADE_SECS).clamp(0.0, 1.0);
        let smooth = fade * fade * (3.0 - 2.0 * fade);
        (1.0 - smooth) as f32
    }

    pub fn session_badge_is_visible(&self) -> bool {
        self.cursor_is_revealed()
            && (self.session_badge_alpha() > 0.001 || self.session_badge_chip_alpha() > 0.001)
    }

    pub fn session_badge_needs_frame_tick(&self) -> bool {
        self.cursor_is_revealed()
            && ((self.session_label.is_some()
                && self.session_badge_secs < SESSION_BADGE_HOLD_SECS + SESSION_BADGE_FADE_SECS)
                || self.badge_modifier_fade_secs.is_some()
                || self.visual.resolved_action != CursorAction::Idle)
    }

    /// Whether the platform overlay should keep a low-frequency hardware
    /// pointer poll alive for hover-to-reveal. This is deliberately separate
    /// from [`Self::session_badge_needs_frame_tick`]: a faded badge needs hover
    /// hit-testing, not continuous 60 fps repainting.
    pub fn session_badge_needs_hover_poll(&self) -> bool {
        self.session_label.is_some() && self.cursor_is_revealed()
    }

    /// Update hover state from a platform-native hardware pointer sample.
    ///
    /// `self.pos` is the centre of the cursor artwork. The hit radius is a
    /// little larger than the 42 point production artwork so the interaction
    /// remains comfortable around the white outline and glow.
    pub fn update_session_badge_hover(&mut self, pointer: Option<(f64, f64)>) -> bool {
        const HOVER_RADIUS: f64 = crate::theme::DISPLAY_SIZE as f64 * 0.82;
        let hovered = self.session_badge_needs_hover_poll()
            && pointer.is_some_and(|(x, y)| {
                let dx = x - self.pos.0;
                let dy = y - self.pos.1;
                if dx * dx + dy * dy <= HOVER_RADIUS * HOVER_RADIUS {
                    return true;
                }
                crate::session_badge_layout(crate::SessionBadgeInput {
                    label: self.session_label.as_deref(),
                    delivery: self.badge_modifiers.and_then(|modifiers| modifiers.0),
                    target: self.badge_modifiers.and_then(|modifiers| modifiers.1),
                    cursor: (self.pos.0 as f32, self.pos.1 as f32),
                    backing_scale: 1.0,
                    label_alpha: self.session_badge_alpha(),
                    chip_alpha: self.session_badge_chip_alpha(),
                    clip: None,
                })
                .is_some_and(|layout| {
                    let rect = layout.rect;
                    x >= rect.x() as f64
                        && x <= (rect.x() + rect.width()) as f64
                        && y >= rect.y() as f64
                        && y <= (rect.y() + rect.height()) as f64
                })
            });
        let changed = hovered != self.session_badge_hovered;
        self.session_badge_hovered = hovered;
        changed
    }

    /// Return the theme that is actually being painted, including any
    /// non-fatal fallback from an unavailable launch-time selection.
    pub fn active_theme_metadata(&self) -> (String, String, String, Option<String>) {
        match self.theme.as_deref() {
            Some(theme) => (
                theme.id.clone(),
                theme.version.clone(),
                theme.profile.clone(),
                self.theme_fallback.clone(),
            ),
            None => (
                crate::DEFAULT_THEME_ID.into(),
                crate::DEFAULT_THEME_VERSION.into(),
                crate::THEME_PROFILE.into(),
                self.theme_fallback.clone(),
            ),
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
    ///
    /// Returns `true` when the planned path just ended (so the caller can
    /// fire an arrival oneshot to unblock `animate_cursor_to`).
    pub fn tick_motion(&mut self, dt: f64) -> bool {
        let spring_k = self.motion.spring * 400.0;
        let spring_c = self.motion.spring * 20.0;

        let mut fire_arrival = false;

        if let Some(ref p) = self.path {
            let path_len = p.length.max(1.0);
            let path_frac = (self.dist / path_len).clamp(0.0, 1.0);
            let profile = 16.0 * path_frac * path_frac * (1.0 - path_frac) * (1.0 - path_frac);
            let floor = if path_frac < 0.5 {
                self.motion.min_start_speed
            } else {
                self.motion.min_end_speed
            };
            let speed_based = (floor + (self.motion.peak_speed - floor) * profile).max(floor);
            // Fixed-duration override: when `glide_duration_ms > 0` the move
            // takes exactly that long regardless of distance, so an orchestrator
            // can lock glides to a known cadence. `0` (the default) keeps the
            // speed-based timing untouched. Shared verbatim with the macOS
            // reference path (`tick_swift_constants`) — no platform drift.
            let speed = if self.motion.glide_duration_ms > 0.0 {
                path_len / (self.motion.glide_duration_ms / 1000.0)
            } else {
                speed_based
            };
            self.dist += speed * dt;

            if self.dist >= path_len {
                let end = p.sample(path_len);
                let end_heading = p.end_visual_heading;
                let vh = end.heading;
                // In fixed-duration mode the constant speed can be large; base
                // the settle impulse on the normal end-floor so the landing
                // stays as crisp as a speed-based glide instead of overshooting
                // proportionally to a short duration.
                let impulse = if self.motion.glide_duration_ms > 0.0 {
                    self.motion.min_end_speed
                } else {
                    speed
                };
                self.spring = Some(Spring {
                    ox: 0.0,
                    oy: 0.0,
                    vx: impulse * 0.5 * vh.cos(),
                    vy: impulse * 0.5 * vh.sin(),
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
                // Point the arrow exactly along the path tangent (the renderer
                // adds π, so we store tangent+π). Assigned directly rather than
                // rate-limited toward it, so the tip actually tracks the
                // trajectory instead of lagging behind on fast/short glides.
                self.heading = s.heading + std::f64::consts::PI;
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

        fire_arrival
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
            let floor_speed = if u < 0.5 {
                MIN_START_SPEED
            } else {
                MIN_END_SPEED
            };
            let speed_based = floor_speed + (PEAK_SPEED - floor_speed) * profile;
            // Fixed-duration override: when `glide_duration_ms > 0` the move
            // takes exactly that long regardless of distance, so an orchestrator
            // can lock glides to a known cadence. `0` (the default) keeps the
            // speed-based timing untouched. Shared verbatim with the
            // Windows/Linux path (`tick_motion`) — no platform drift.
            let current_speed = if self.motion.glide_duration_ms > 0.0 {
                path_len / (self.motion.glide_duration_ms / 1000.0)
            } else {
                speed_based
            };
            self.dist += current_speed * dt;

            if self.dist >= path_len {
                // Transition to spring settle.
                let end = p.sample(path_len);
                let end_heading = p.end_visual_heading;
                let vh = end.heading;
                // In fixed-duration mode the constant speed can be large; base
                // the settle impulse on the normal end-floor so the landing
                // stays as crisp as a speed-based glide instead of overshooting
                // proportionally to a short duration.
                let impulse = if self.motion.glide_duration_ms > 0.0 {
                    MIN_END_SPEED
                } else {
                    current_speed
                };
                self.spring = Some(Spring {
                    ox: 0.0,
                    oy: 0.0,
                    vx: impulse * SPRING_OVERSHOOT * vh.cos(),
                    vy: impulse * SPRING_OVERSHOOT * vh.sin(),
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
                // Point the arrow exactly along the path tangent (renderer adds
                // π, so store tangent+π). Direct assignment, not rate-limited, so
                // the tip tracks the trajectory instead of lagging on fast moves.
                self.heading = s.heading + std::f64::consts::PI;
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
        let modifiers_before_tick = (self.visual.delivery, self.visual.target);
        self.visual.tick(dt);
        let modifiers_after_tick = (self.visual.delivery, self.visual.target);
        if modifiers_after_tick.0.is_some() || modifiers_after_tick.1.is_some() {
            self.badge_modifiers = Some(modifiers_after_tick);
            self.badge_modifier_fade_secs = None;
        } else if (modifiers_before_tick.0.is_some() || modifiers_before_tick.1.is_some())
            && self.badge_modifiers.is_some()
            && self.badge_modifier_fade_secs.is_none()
        {
            self.badge_modifier_fade_secs = Some(0.0);
        }
        if let Some(elapsed) = self.badge_modifier_fade_secs {
            let next = elapsed + dt.max(0.0);
            if next >= SESSION_BADGE_FADE_SECS {
                self.badge_modifiers = None;
                self.badge_modifier_fade_secs = None;
            } else {
                self.badge_modifier_fade_secs = Some(next);
            }
        }
        if self.session_label.is_some() {
            self.session_badge_secs = (self.session_badge_secs + dt)
                .min(SESSION_BADGE_HOLD_SECS + SESSION_BADGE_FADE_SECS);
        }
        let idle_hide_ms = self.motion.idle_hide_ms;
        if idle_hide_ms > 0.0 {
            let moving = self.path.is_some() || self.spring.is_some() || self.click_t.is_some();
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
                let reveal_badge = !self.cursor_is_revealed();
                // Apply click offset (16 pt along end_heading) before planning,
                // matching Swift `moveTo(point:endAngleRadians:)`:
                //   tx = clickPoint.x + cos(endAngle) * clickOffset
                //   ty = clickPoint.y + sin(endAngle) * clickOffset
                const CLICK_OFFSET: f64 = 16.0;
                let turn_radius = self.motion.turn_radius;
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
                let plan =
                    PathPlanner::plan(x0, y0, th0, tx, ty, th1, end_heading_radians, turn_radius);
                self.path = Some(plan);
                self.dist = 0.0;
                self.spring = None;
                self.spring_tgt = None;
                if matches!(
                    self.visual.resolved_action,
                    CursorAction::Idle | CursorAction::Navigate
                ) {
                    let delivery = self.visual.delivery;
                    let target = self.visual.target;
                    self.visual.begin(CursorAction::Navigate, delivery, target);
                }
                self.idle_secs = 0.0;
                self.idle_alpha = 1.0;
                if reveal_badge {
                    self.reveal_session_badge();
                }
                true
            }
            OverlayCommand::SnapTo {
                x,
                y,
                heading_radians,
            } => {
                let reveal_badge = !self.cursor_is_revealed();
                self.pos = (x, y);
                if let Some(heading) = heading_radians {
                    self.heading = heading;
                }
                self.path = None;
                self.dist = 0.0;
                self.spring = None;
                self.spring_tgt = None;
                if matches!(
                    self.visual.resolved_action,
                    CursorAction::Idle | CursorAction::Navigate
                ) {
                    let delivery = self.visual.delivery;
                    let target = self.visual.target;
                    self.visual.begin(CursorAction::Navigate, delivery, target);
                }
                self.idle_secs = 0.0;
                self.idle_alpha = 1.0;
                if reveal_badge {
                    self.reveal_session_badge();
                }
                true
            }
            OverlayCommand::ClickPulse { x, y } => {
                let reveal_badge = !self.cursor_is_revealed();
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
                if matches!(
                    self.visual.resolved_action,
                    CursorAction::Idle | CursorAction::Navigate | CursorAction::Click
                ) {
                    let delivery = self.visual.delivery;
                    let target = self.visual.target;
                    self.visual.begin(CursorAction::Click, delivery, target);
                }
                self.idle_secs = 0.0;
                self.idle_alpha = 1.0;
                if reveal_badge {
                    self.reveal_session_badge();
                }
                true
            }
            OverlayCommand::SetPressed(v) => {
                self.pressed = v;
                if v {
                    let delivery = self.visual.delivery;
                    let target = self.visual.target;
                    self.visual.begin(CursorAction::Drag, delivery, target);
                } else {
                    self.visual.end(CursorAction::Drag);
                }
                self.idle_secs = 0.0;
                self.idle_alpha = 1.0;
                true
            }
            OverlayCommand::SetEnabled(v) => {
                let reveal_badge = v && !self.visible;
                self.visible = v;
                if reveal_badge {
                    self.reveal_session_badge();
                }
                true
            }
            OverlayCommand::SetMotion(m) => {
                self.motion = m;
                true
            }
            OverlayCommand::PinAbove(wid) => {
                self.pinned_wid = Some(wid);
                true
            }
            OverlayCommand::BeginAction {
                action,
                delivery,
                target,
            } => {
                self.visual.begin(action, delivery, target);
                self.badge_modifiers = if delivery.is_some() || target.is_some() {
                    Some((delivery, target))
                } else {
                    None
                };
                self.badge_modifier_fade_secs = None;
                true
            }
            OverlayCommand::EndAction(action) => {
                self.visual.end(action);
                true
            }
            OverlayCommand::SetTheme {
                theme_id,
                reduced_motion,
            } => {
                match crate::resolve_theme_selection(&theme_id) {
                    Ok(theme) => {
                        self.theme = theme;
                        self.theme_fallback = None;
                        self.cfg.theme_id = theme_id;
                        self.cfg.reduced_motion = reduced_motion;
                        self.visual.reduced_motion = reduced_motion;
                    }
                    Err(error) => {
                        tracing::warn!(
                            theme_id,
                            error = %error,
                            "keeping the active cursor theme after selection failed"
                        );
                    }
                }
                true
            }
            OverlayCommand::SetSessionLabel(label) => {
                let session_label = crate::sanitize_session_label(&label);
                if session_label != self.session_label {
                    self.session_label = session_label;
                    self.session_badge_secs = 0.0;
                }
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
///
/// `backing_scale` is the destination-pixmap-pixels per logical-point ratio
/// (e.g. 2.0 on a retina display where the pixmap is sized at physical
/// pixels). Pass `1.0` when the pixmap is sized at logical pixels.
pub fn render_frame(
    core: &RenderStateCore,
    width: u32,
    height: u32,
    origin_x: f64,
    origin_y: f64,
    focus_rect: Option<FocusRect>,
    backing_scale: f32,
) -> tiny_skia::Pixmap {
    let w = width.max(1);
    let h = height.max(1);
    let mut pm =
        tiny_skia::Pixmap::new(w, h).unwrap_or_else(|| tiny_skia::Pixmap::new(1, 1).unwrap());
    paint_cursor(&mut pm, core, origin_x, origin_y, focus_rect, backing_scale);
    pm
}

/// Paint a single cursor (bloom + click-pulse + optional focus-rect + arrow)
/// into a caller-owned [`tiny_skia::Pixmap`]. tiny-skia's `fill_*` / `stroke_*`
/// are alpha-over, so painting several cursors into the same pixmap composites
/// them with later calls drawn on top — this is what lets the macOS overlay
/// render N owned cursors into one buffer / one NSWindow.
///
/// `origin_x` / `origin_y` are subtracted from `core.pos` before drawing
/// (Windows passes the virtual-screen origin; macOS / Linux pass `(0.0, 0.0)`).
/// Both are in **logical** screen points, just like `core.pos`.
///
/// `backing_scale` is the destination-pixmap-pixels per logical-point ratio.
/// On a 2× retina macOS display the caller sizes the pixmap at the screen's
/// PHYSICAL pixel dimensions (logical × backing_scale) and passes `2.0` so
/// the cursor renders at native resolution instead of being upsampled by
/// Core Animation. When the pixmap is sized at LOGICAL pixels, pass `1.0`.
///
/// Everything that operates in pixmap-pixel space (the cursor anchor `px/py`,
/// bloom radius, click-pulse ring radius, stroke widths, focus-rect coords,
/// arrow `display_size`) is multiplied by `backing_scale` so the cursor still
/// occupies the same on-screen logical footprint but at higher pixel fidelity.
///
/// Quiescent / hidden cursors early-return before touching the pixmap, so an
/// idle session costs essentially nothing in the per-frame composite loop.
pub fn paint_cursor(
    pm: &mut tiny_skia::Pixmap,
    core: &RenderStateCore,
    origin_x: f64,
    origin_y: f64,
    focus_rect: Option<FocusRect>,
    backing_scale: f32,
) {
    if !core.visible || core.pos.0 < -100.0 || core.idle_alpha < 0.004 {
        return;
    }

    let s = backing_scale.max(1.0) as f64; // logical-pt → pixmap-pixel scale
    let sf = s as f32;

    // Cursor anchor in pixmap-pixel space: subtract the (logical) origin
    // first, then scale into pixmap pixels.
    let (px, py) = ((core.pos.0 - origin_x) * s, (core.pos.1 - origin_y) * s);
    let heading = core.heading;
    let alpha_scale = core.idle_alpha as f32;

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

        if let Some(rect) = tiny_skia::Rect::from_xywh(
            (fx * s) as f32,
            (fy * s) as f32,
            (fw * s) as f32,
            (fh * s) as f32,
        ) {
            // Faint fill
            let fill_paint = tiny_skia::Paint {
                shader: tiny_skia::Shader::SolidColor(tiny_skia::Color::from_rgba8(
                    cr, cg, cb, fill_a,
                )),
                ..Default::default()
            };
            pm.fill_rect(rect, &fill_paint, tiny_skia::Transform::identity(), None);

            // Border stroke (2px glow)
            let border_paint = tiny_skia::Paint {
                shader: tiny_skia::Shader::SolidColor(tiny_skia::Color::from_rgba8(
                    cr, cg, cb, border_a,
                )),
                anti_alias: true,
                ..Default::default()
            };
            let stroke = tiny_skia::Stroke {
                width: 2.5 * sf,
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

    if let Some(theme) = core.theme.as_deref() {
        let tint = (theme.id == crate::DEFAULT_THEME_ID)
            .then(|| crate::session_fill_rgba(&core.cfg.cursor_id));
        crate::paint_compiled_theme_with_tint(
            pm,
            theme,
            &core.visual,
            px as f32,
            py as f32,
            heading as f32,
            backing_scale.max(1.0),
            alpha_scale,
            tint,
        );
    } else {
        // Defensive fallback for a manually constructed RenderStateCore. The
        // normal constructor always resolves either the requested theme or the
        // embedded default.
        crate::theme::paint_default_theme_with_fill(
            pm,
            &core.visual,
            px as f32,
            py as f32,
            heading as f32,
            backing_scale.max(1.0),
            alpha_scale,
            crate::session_fill_rgba(&core.cfg.cursor_id),
        );
    }

    let (delivery, target) = core.badge_modifiers.unwrap_or((None, None));
    if let Some(layout) = crate::session_badge_layout(crate::SessionBadgeInput {
        label: core.session_label.as_deref(),
        delivery,
        target,
        cursor: (px as f32, py as f32),
        backing_scale: backing_scale.max(1.0),
        label_alpha: core.session_badge_alpha(),
        chip_alpha: core.session_badge_chip_alpha(),
        clip: Some((pm.width() as f32, pm.height() as f32)),
    }) {
        crate::paint_session_badge(
            pm,
            &layout,
            crate::session_fill_rgba(&core.cfg.cursor_id),
            alpha_scale,
        );
    }
}

#[cfg(test)]
mod glide_duration_tests {
    use super::*;
    use crate::{CursorConfig, PathPlanner};

    /// Run a glide of `dist_pts` to completion and return how many seconds it
    /// took. `tick` selects the platform path: `false` = `tick_motion`
    /// (Windows/Linux), `true` = `tick_swift_constants` (macOS reference).
    fn arrival_secs(glide_ms: f64, dist_pts: f64, swift: bool) -> f64 {
        let mut core = RenderStateCore::new(CursorConfig::default());
        core.motion.glide_duration_ms = glide_ms;
        core.motion.idle_hide_ms = 0.0;
        core.pos = (0.0, 0.0);
        // Aligned headings → an effectively straight path of length ~dist_pts.
        core.path = Some(PathPlanner::plan(
            0.0, 0.0, 0.0, dist_pts, 0.0, 0.0, 0.0, 80.0,
        ));
        core.dist = 0.0;
        let dt = 1.0 / 240.0;
        let mut t = 0.0;
        for _ in 0..200_000 {
            let arrived = if swift {
                core.tick_swift_constants(dt)
            } else {
                core.tick_motion(dt)
            };
            t += dt;
            if arrived {
                break;
            }
        }
        t
    }

    #[test]
    fn fixed_duration_is_distance_independent_on_both_paths() {
        for swift in [false, true] {
            let short = arrival_secs(300.0, 120.0, swift);
            let long = arrival_secs(300.0, 1400.0, swift);
            // Both land in ~300ms regardless of distance (within a few ticks).
            assert!((short - 0.3).abs() < 0.05, "swift={swift} short={short}");
            assert!((long - 0.3).abs() < 0.05, "swift={swift} long={long}");
        }
    }

    #[test]
    fn zero_keeps_speed_based_timing() {
        // glide_duration_ms == 0 (the default) → longer paths take longer, on
        // both platform paths, exactly as before this field was implemented.
        for swift in [false, true] {
            let short = arrival_secs(0.0, 120.0, swift);
            let long = arrival_secs(0.0, 1400.0, swift);
            assert!(
                long > short + 0.2,
                "swift={swift} short={short} long={long}"
            );
        }
    }
}

#[cfg(test)]
mod session_badge_and_action_tests {
    use super::*;
    use crate::{CursorConfig, DeliveryModifier, TargetModifier};

    #[test]
    fn session_badge_holds_then_fades_once() {
        let mut core = RenderStateCore::new(CursorConfig::default());
        assert_eq!(core.session_badge_alpha(), 0.0);
        assert!(core.apply_command_base(
            OverlayCommand::SetSessionLabel("Research".into()),
            false,
            false,
        ));
        assert_eq!(core.session_badge_alpha(), 1.0);

        core.tick_motion(SESSION_BADGE_HOLD_SECS - 0.05);
        assert_eq!(core.session_badge_alpha(), 1.0);
        core.tick_motion(SESSION_BADGE_FADE_SECS * 0.5 + 0.05);
        assert!(core.session_badge_alpha() > 0.0);
        assert!(core.session_badge_alpha() < 1.0);
        core.tick_motion(SESSION_BADGE_FADE_SECS);
        assert_eq!(core.session_badge_alpha(), 0.0);
    }

    #[test]
    fn repeated_session_label_metadata_does_not_restart_badge_timer() {
        let mut core = RenderStateCore::new(CursorConfig::default());
        core.apply_command_base(
            OverlayCommand::SetSessionLabel("Research".into()),
            false,
            false,
        );
        core.tick_motion(SESSION_BADGE_HOLD_SECS + SESSION_BADGE_FADE_SECS);
        assert_eq!(core.session_badge_alpha(), 0.0);

        core.apply_command_base(
            OverlayCommand::SetSessionLabel("Research".into()),
            false,
            false,
        );
        assert_eq!(core.session_badge_alpha(), 0.0);

        core.apply_command_base(
            OverlayCommand::SetSessionLabel("Writing".into()),
            false,
            false,
        );
        assert_eq!(core.session_badge_alpha(), 1.0);
    }

    #[test]
    fn revealing_hidden_cursor_restarts_badge_without_restarting_on_every_move() {
        let mut core = RenderStateCore::new(CursorConfig::default());
        core.apply_command_base(
            OverlayCommand::SetSessionLabel("Research".into()),
            false,
            false,
        );
        core.tick_motion(SESSION_BADGE_HOLD_SECS + SESSION_BADGE_FADE_SECS);
        assert_eq!(core.session_badge_alpha(), 0.0);

        core.apply_command_base(
            OverlayCommand::SnapTo {
                x: 100.0,
                y: 100.0,
                heading_radians: None,
            },
            false,
            false,
        );
        assert_eq!(core.session_badge_alpha(), 1.0);
        assert!(core.session_badge_needs_frame_tick());
        core.tick_motion(0.5);
        let elapsed = core.session_badge_secs;
        core.apply_command_base(
            OverlayCommand::SnapTo {
                x: 120.0,
                y: 120.0,
                heading_radians: None,
            },
            false,
            false,
        );
        assert_eq!(core.session_badge_secs, elapsed);
        core.tick_motion(SESSION_BADGE_HOLD_SECS + SESSION_BADGE_FADE_SECS);
        assert!(!core.session_badge_needs_frame_tick());
    }

    #[test]
    fn hardware_pointer_hover_reveals_only_while_over_cursor() {
        let mut core = RenderStateCore::new(CursorConfig::default());
        core.pos = (300.0, 240.0);
        core.apply_command_base(
            OverlayCommand::SetSessionLabel("Research".into()),
            false,
            false,
        );
        core.tick_motion(SESSION_BADGE_HOLD_SECS + SESSION_BADGE_FADE_SECS);
        assert_eq!(core.session_badge_alpha(), 0.0);
        assert!(core.session_badge_needs_hover_poll());

        assert!(core.update_session_badge_hover(Some((302.0, 238.0))));
        assert_eq!(core.session_badge_alpha(), 1.0);
        assert!(!core.update_session_badge_hover(Some((304.0, 241.0))));
        assert_eq!(core.session_badge_alpha(), 1.0);

        assert!(core.update_session_badge_hover(Some((500.0, 500.0))));
        assert_eq!(core.session_badge_alpha(), 0.0);
    }

    #[test]
    fn movement_preserves_the_active_semantic_action() {
        let mut core = RenderStateCore::new(CursorConfig::default());
        core.pos = (20.0, 20.0);
        core.apply_command_base(
            OverlayCommand::BeginAction {
                action: CursorAction::Text,
                delivery: None,
                target: Some(TargetModifier::Ax),
            },
            false,
            false,
        );
        core.apply_command_base(
            OverlayCommand::MoveTo {
                x: 200.0,
                y: 100.0,
                end_heading_radians: 0.0,
            },
            false,
            false,
        );
        assert_eq!(core.visual.resolved_action, CursorAction::Text);
        assert_eq!(core.visual.target, Some(TargetModifier::Ax));
        core.apply_command_base(
            OverlayCommand::ClickPulse { x: 200.0, y: 100.0 },
            false,
            false,
        );
        assert_eq!(core.visual.resolved_action, CursorAction::Text);
        assert_eq!(core.visual.target, Some(TargetModifier::Ax));
    }

    #[test]
    fn modifiers_live_in_the_badge_then_fade_after_action_completion() {
        let mut core = RenderStateCore::new(CursorConfig::default());
        core.pos = (200.0, 200.0);
        core.apply_command_base(
            OverlayCommand::BeginAction {
                action: CursorAction::Click,
                delivery: Some(DeliveryModifier::Foreground),
                target: Some(TargetModifier::Pixel),
            },
            false,
            false,
        );
        assert_eq!(
            core.badge_modifiers,
            Some((
                Some(DeliveryModifier::Foreground),
                Some(TargetModifier::Pixel)
            ))
        );
        assert_eq!(core.session_badge_chip_alpha(), 1.0);
        assert!(core.session_badge_is_visible());

        let frame = 1.0 / 60.0;
        for _ in 0..=((CursorAction::Click.duration_secs() / frame).ceil() as usize) {
            core.tick_motion(frame);
        }
        assert!(core.session_badge_chip_alpha() > 0.0);
        assert!(core.session_badge_chip_alpha() < 1.0);
        assert!(core.session_badge_needs_frame_tick());

        core.tick_motion(SESSION_BADGE_FADE_SECS);
        assert_eq!(core.badge_modifiers, None);
        assert_eq!(core.session_badge_chip_alpha(), 0.0);
    }

    #[test]
    fn modifier_preemption_replaces_the_badge_context_without_cross_fading() {
        let mut core = RenderStateCore::new(CursorConfig::default());
        core.apply_command_base(
            OverlayCommand::BeginAction {
                action: CursorAction::Observe,
                delivery: Some(DeliveryModifier::Background),
                target: Some(TargetModifier::Ax),
            },
            false,
            false,
        );
        core.apply_command_base(
            OverlayCommand::BeginAction {
                action: CursorAction::Text,
                delivery: Some(DeliveryModifier::Foreground),
                target: Some(TargetModifier::Browser),
            },
            false,
            false,
        );
        assert_eq!(
            core.badge_modifiers,
            Some((
                Some(DeliveryModifier::Foreground),
                Some(TargetModifier::Browser)
            ))
        );
        assert_eq!(core.badge_modifier_fade_secs, None);
        assert_eq!(core.session_badge_chip_alpha(), 1.0);
    }

    #[test]
    fn click_pulse_preserves_declared_context_until_the_action_fades() {
        let mut core = RenderStateCore::new(CursorConfig::default());
        core.apply_command_base(
            OverlayCommand::BeginAction {
                action: CursorAction::Click,
                delivery: Some(DeliveryModifier::Background),
                target: Some(TargetModifier::Ax),
            },
            false,
            false,
        );
        core.apply_command_base(
            OverlayCommand::ClickPulse { x: 40.0, y: 60.0 },
            false,
            false,
        );
        assert_eq!(
            (core.visual.delivery, core.visual.target),
            (Some(DeliveryModifier::Background), Some(TargetModifier::Ax))
        );
        assert_eq!(
            core.badge_modifiers,
            Some((Some(DeliveryModifier::Background), Some(TargetModifier::Ax)))
        );
    }
}

#[cfg(test)]
mod backing_scale_tests {
    use super::*;
    use crate::CursorConfig;

    fn visible_pixel_count(pm: &tiny_skia::Pixmap) -> u32 {
        // Count strongly visible coverage, not the halo's feather pixels.
        // Low-alpha gradient coverage is quantized differently across scales
        // and is not useful evidence for the backing-scale regression.
        pm.data().chunks_exact(4).filter(|px| px[3] > 96).count() as u32
    }

    fn visible_bounds(pm: &tiny_skia::Pixmap) -> (u32, u32) {
        let mut min_x = u32::MAX;
        let mut min_y = u32::MAX;
        let mut max_x = 0;
        let mut max_y = 0;
        for (index, pixel) in pm.data().chunks_exact(4).enumerate() {
            if pixel[3] <= 96 {
                continue;
            }
            let x = index as u32 % pm.width();
            let y = index as u32 / pm.width();
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }
        assert_ne!(min_x, u32::MAX, "render should have visible pixels");
        (max_x - min_x + 1, max_y - min_y + 1)
    }

    fn render_at(backing_scale: f32, logical_size: u32) -> tiny_skia::Pixmap {
        let mut core = RenderStateCore::new(CursorConfig::default());
        // Place the cursor at the centre of the logical area and disable
        // idle-fade so the arrow paints at full alpha regardless of timing.
        let centre = logical_size as f64 / 2.0;
        core.pos = (centre, centre);
        core.idle_alpha = 1.0;
        core.visible = true;

        // The pixmap is sized in *pixmap* pixels (logical × backing_scale)
        // — that's the macOS retina pipeline: allocate at physical pixels,
        // then let paint_cursor scale into them.
        let pm_size = (logical_size as f32 * backing_scale) as u32;
        let mut pm = tiny_skia::Pixmap::new(pm_size, pm_size).unwrap();
        paint_cursor(&mut pm, &core, 0.0, 0.0, None, backing_scale);
        pm
    }

    /// The compiled artifact contains vector geometry. Skia must rasterize it
    /// at the destination backing scale, so linear dimensions grow 1:2:3 and
    /// strongly visible coverage grows approximately with the square.
    #[test]
    fn compiled_vectors_render_at_one_two_and_three_x() {
        let pm_1x = render_at(1.0, 200);
        let pm_2x = render_at(2.0, 200);
        let pm_3x = render_at(3.0, 200);

        let n_1x = visible_pixel_count(&pm_1x);
        let n_2x = visible_pixel_count(&pm_2x);
        let n_3x = visible_pixel_count(&pm_3x);

        assert!(n_1x > 0, "1× render should paint SOMETHING (got {n_1x})");
        assert!(n_2x > 0, "2× render should paint SOMETHING (got {n_2x})");
        assert!(n_3x > 0, "3× render should paint SOMETHING (got {n_3x})");

        let ratio_2x = n_2x as f64 / n_1x as f64;
        let ratio_3x = n_3x as f64 / n_1x as f64;
        assert!(
            ratio_2x > 3.0 && ratio_2x < 5.0,
            "2× backing_scale should produce ~4× more visible pixels: \
             got n_1x={n_1x}, n_2x={n_2x}, ratio={ratio_2x:.2}"
        );
        assert!(
            ratio_3x > 7.0 && ratio_3x < 11.0,
            "3× backing_scale should produce ~9× more visible pixels: \
             got n_1x={n_1x}, n_3x={n_3x}, ratio={ratio_3x:.2}"
        );

        let bounds_1x = visible_bounds(&pm_1x);
        let bounds_2x = visible_bounds(&pm_2x);
        let bounds_3x = visible_bounds(&pm_3x);
        for (one, two, three) in [
            (bounds_1x.0, bounds_2x.0, bounds_3x.0),
            (bounds_1x.1, bounds_2x.1, bounds_3x.1),
        ] {
            assert!(
                (two as f64 / one as f64 - 2.0).abs() < 0.15,
                "2× visible bounds should double: {one}, {two}"
            );
            assert!(
                (three as f64 / one as f64 - 3.0).abs() < 0.20,
                "3× visible bounds should triple: {one}, {three}"
            );
        }
    }
}
