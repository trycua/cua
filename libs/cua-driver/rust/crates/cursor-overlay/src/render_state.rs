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
//! - [`RenderStateCore::tick_motion`] — advances the planned
//!   [`Trajectory`](crate::trajectory::Trajectory), click effects and the
//!   idle fade on every platform; returns whether the cursor just arrived
//!   (so the caller can fire arrival signals).
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

use crate::trajectory::{plan_move, MoveRequest, Pt, Trajectory};
use crate::{
    CompiledTheme, CursorAction, CursorConfig, CursorVisualState, DeliveryModifier, MotionConfig,
    OverlayCommand, TargetModifier,
};
use cua_driver_core::agent_cursor::AgentCursorVisibility;
use std::sync::Arc;

pub const SESSION_BADGE_HOLD_SECS: f64 = 2.0;
pub const SESSION_BADGE_FADE_SECS: f64 = 0.4;

/// Position of a cursor that has never been placed. It lies far outside any
/// compositor layout: layouts reach negative coordinates when a monitor sits
/// left of or above the primary one, so a small negative sentinel (the old
/// `(-200, -200)`) is indistinguishable from a real position there.
pub const UNPLACED_POS: (f64, f64) = (-1.0e9, -1.0e9);

/// Whether `pos` is a real position rather than [`UNPLACED_POS`].
pub fn is_placed(pos: (f64, f64)) -> bool {
    pos.0 > -1.0e8
}

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
    /// Planned move being played; `None` at rest. It stays a little past
    /// its last sample while a trail or magnet glow fades out.
    pub trajectory: Option<Trajectory>,
    /// Seconds since the current trajectory started.
    pub motion_t: f64,
    /// Whether the current trajectory still owes its arrival signal.
    pub arrival_pending: bool,
    /// Moves planned so far; seeds each trajectory.
    pub move_seq: u64,
    /// Seconds since the last click, while click effects play.
    pub click_age: Option<f64>,
    /// Hotspot of the last click, for the ripple.
    pub click_point: (f64, f64),
    /// Click-pulse phase 0..1; `None` = no pulse in flight.
    pub click_t: Option<f64>,
    /// Whether a button is currently being held for this cursor.
    pub pressed: bool,
    /// Semantic action and animation playback state.
    pub visual: CursorVisualState,
    /// The current visual action came from a semantic `BeginAction` cue
    /// (not the display action a move, snap or click plays after itself).
    /// Only such a cue holds the idle clock while it lasts.
    semantic_cue: bool,
    /// Decoded installed or embedded theme.
    pub theme: Option<Arc<CompiledTheme>>,
    /// Non-fatal launch-time fallback reason, if an installed theme failed.
    pub theme_fallback: Option<String>,
    /// User-controlled visibility.
    pub visible: bool,
    /// The pinned target window is on another workspace (macOS Space), so the
    /// cursor must not paint over the user's current workspace at that
    /// window's coordinates. Platform adapters set it when handling
    /// `PinAbove`; `false` when membership is unknown.
    pub pinned_target_off_workspace: bool,
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
    /// Whether this surface can alpha-blend translucent effects (glow,
    /// trail). Platforms clear it where they cannot, such as X11 without a
    /// compositing manager.
    pub effects_capable: bool,
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
            semantic_cue: false,
            theme,
            theme_fallback,
            pos: UNPLACED_POS,
            heading: std::f64::consts::FRAC_PI_4,
            trajectory: None,
            motion_t: 0.0,
            arrival_pending: false,
            move_seq: 0,
            click_age: None,
            click_point: (0.0, 0.0),
            click_t: None,
            pressed: false,
            visible: true,
            idle_secs: 0.0,
            idle_alpha: 1.0,
            pinned_wid: None,
            pinned_target_off_workspace: false,
            session_label: None,
            session_badge_secs: SESSION_BADGE_HOLD_SECS + SESSION_BADGE_FADE_SECS,
            session_badge_hovered: false,
            badge_modifiers: None,
            badge_modifier_fade_secs: None,
            effects_capable: true,
        }
    }

    /// Screen-space bounds `[x, y, width, height]` (logical points, same
    /// origin as `pos`) of the motion effects painted this frame: trail,
    /// glow, magnet and click ripple. `None` when no effect is visible.
    /// Platforms that repaint only a tile or dirty rect around the cursor
    /// must union this in so effects are not clipped or left behind.
    pub fn effect_bounds(&self) -> Option<[f64; 4]> {
        if !self.is_revealed() || self.pinned_target_off_workspace {
            return None;
        }
        let frame = self.effect_frame();
        let mut bounds: Option<[f64; 4]> = None;
        let mut add = |x0: f64, y0: f64, x1: f64, y1: f64| {
            bounds = Some(match bounds {
                None => [x0, y0, x1, y1],
                Some([a, b, c, d]) => [a.min(x0), b.min(y0), c.max(x1), d.max(y1)],
            });
        };
        if let Some(glow) = frame.glow {
            add(
                glow.x - glow.r,
                glow.y - glow.r,
                glow.x + glow.r,
                glow.y + glow.r,
            );
        }
        for seg in &frame.trail {
            let pad = seg.width / 2.0 + 1.0;
            add(
                seg.a.0.min(seg.b.0) - pad,
                seg.a.1.min(seg.b.1) - pad,
                seg.a.0.max(seg.b.0) + pad,
                seg.a.1.max(seg.b.1) + pad,
            );
        }
        if let Some(magnet) = frame.magnet {
            let [x, y, w, h] = magnet.rect;
            let pad = MAGNET_INFLATE + 8.0;
            add(x - pad, y - pad, x + w + pad, y + h + pad);
        }
        if let Some(ripple) = frame.ripple {
            let r = ripple.r + ripple.width;
            add(ripple.x - r, ripple.y - r, ripple.x + r, ripple.y + r);
        }
        bounds.map(|[x0, y0, x1, y1]| [x0, y0, x1 - x0, y1 - y0])
    }

    /// Whether a planned move is still travelling (not just fading effects).
    pub fn is_moving(&self) -> bool {
        self.trajectory
            .as_ref()
            .is_some_and(|traj| self.motion_t < traj.duration())
    }

    /// Drop any in-flight move and its effects, leaving the cursor where it
    /// is. A pending arrival is dropped with it.
    pub fn cancel_motion(&mut self) {
        self.trajectory = None;
        self.motion_t = 0.0;
        self.arrival_pending = false;
    }

    fn reduced_motion(&self) -> bool {
        self.visual.reduced_motion == crate::ReducedMotion::On
    }

    /// Geometry of the motion effects to paint this frame.
    fn effect_frame(&self) -> EffectFrame {
        if self.reduced_motion() {
            return EffectFrame::default();
        }
        let mut frame = match self.trajectory.as_ref() {
            Some(traj) => {
                effects::motion_frame(traj, self.motion_t, self.pos, self.effects_capable)
            }
            None => EffectFrame::default(),
        };
        if let Some(age) = self.click_age {
            effects::add_click(
                &mut frame,
                self.motion.resolved_effects(),
                age,
                self.click_point,
            );
        }
        frame
    }

    /// Whether the cursor currently paints pixels: user-visible, placed
    /// (not [`UNPLACED_POS`]), and not fully idle-faded.
    pub fn is_revealed(&self) -> bool {
        self.visible && is_placed(self.pos) && self.idle_alpha >= 0.004
    }

    /// Whether an input tool should wait for this cursor to glide.
    /// Explicitly disabled cursors keep their launch configuration, but
    /// do not paint motion and must not delay input on an invisible path.
    /// Idle fading does not disable a future glide, which reveals the cursor.
    pub fn should_animate_to_target(&self) -> bool {
        self.cfg.enabled && self.visible && is_placed(self.pos)
    }

    /// Whether a revealed cursor keeps changing pixels while it rests.
    ///
    /// The default theme levitates through the shared float motion (the
    /// resting "bob"), and a custom theme may loop a multi-frame animation for
    /// its current action. Both are part of the cursor's visual identity, so
    /// every platform must keep delivering frames while this holds.
    ///
    /// Resting motion is bounded by idle hide: the cursor levitates while its
    /// idle-hide countdown runs and stops when the fade hides it. A cursor
    /// configured never to hide (`idle_hide_ms == 0`) rests still, so the
    /// overlay can stop rendering once activity settles; a full-output
    /// redraw per frame on Wayland, or a layered-window upload on Windows,
    /// must not run for as long as the cursor stays on screen. Reduced motion
    /// freezes both (no bob, still frame), a hidden, unplaced, faded, or
    /// off-workspace cursor paints nothing, and a single-frame custom theme
    /// has nothing to animate.
    pub fn has_resting_motion(&self) -> bool {
        if !self.is_revealed()
            || self.motion.idle_hide_ms <= 0.0
            || self.pinned_target_off_workspace
            || self.visual.reduced_motion == crate::ReducedMotion::On
        {
            return false;
        }
        match self.theme.as_deref() {
            // The defensive no-theme fallback paints the embedded default.
            None => true,
            Some(theme) if theme.id == crate::DEFAULT_THEME_ID => true,
            Some(theme) => theme
                .animation_for_action(self.visual.resolved_action)
                .is_some_and(|animation| animation.frames.len() > 1),
        }
    }

    /// Whether the idle-hide fade (the 180 ms alpha ramp after
    /// `motion.idle_hide_ms` of inactivity) is currently animating.
    pub fn idle_fade_in_progress(&self) -> bool {
        self.motion.idle_hide_ms > 0.0
            && self.is_revealed()
            && self.idle_secs >= self.motion.idle_hide_ms / 1000.0
    }

    /// The shared frame-tick predicate: true while the next tick can change
    /// this cursor's pixels, so the platform render loop must run at frame
    /// cadence. It covers an in-flight glide, spring settle, click pulse,
    /// session-badge or semantic-action animation, resting motion
    /// ([`Self::has_resting_motion`]), and the idle fade. A brand-new sentinel
    /// cursor, a fully faded cursor, and a reduced-motion cursor waiting out
    /// its opaque idle-hide delay are quiescent; platforms advance that
    /// countdown with [`Self::idle_fade_wait`] or their own slow heartbeat.
    pub fn needs_frame_tick(&self) -> bool {
        self.trajectory.is_some()
            || self.click_t.is_some()
            || self.click_age.is_some()
            || self.session_badge_needs_frame_tick()
            || self.has_resting_motion()
            || self.idle_fade_in_progress()
    }

    /// Time until the idle fade starts for a placed, visible, settled cursor,
    /// so a parked render loop can wake exactly when pixels begin to change.
    /// `None` when idle hide is off, the cursor is moving or not shown, or
    /// the fade has already started.
    pub fn idle_fade_wait(&self) -> Option<std::time::Duration> {
        if !self.visible
            || !is_placed(self.pos)
            || self.motion.idle_hide_ms <= 0.0
            || self.trajectory.is_some()
            || self.click_t.is_some()
            || self.click_age.is_some()
        {
            return None;
        }
        let remaining = self.motion.idle_hide_ms / 1000.0 - self.idle_secs;
        (remaining.is_finite() && remaining > 0.0)
            .then(|| std::time::Duration::from_secs_f64(remaining))
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
        self.is_revealed()
            && (self.session_badge_alpha() > 0.001 || self.session_badge_chip_alpha() > 0.001)
    }

    pub fn session_badge_needs_frame_tick(&self) -> bool {
        self.is_revealed()
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
        self.session_label.is_some() && self.is_revealed()
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

    /// Advance the animation by `dt` seconds on every platform: play the
    /// planned trajectory, age click effects, and run the idle fade.
    ///
    /// Returns `true` on the tick the cursor arrives (the hotspot first
    /// reaches the target), so the caller can fire the arrival oneshot that
    /// unblocks `animate_cursor_to`. Any follow-through or settle keeps
    /// playing after that, during the click.
    pub fn tick_motion(&mut self, dt: f64) -> bool {
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        let mut fire_arrival = false;
        if let Some(traj) = self.trajectory.as_ref() {
            self.motion_t += dt;
            let s = traj.sample_at(self.motion_t);
            self.pos = crate::anchor_for_pointer(s.x, s.y, s.heading);
            self.heading = s.heading;
            if self.arrival_pending && self.motion_t >= traj.arrival_t {
                self.arrival_pending = false;
                fire_arrival = true;
            }
            if self.motion_t >= effects::linger(traj) {
                if self.arrival_pending {
                    self.arrival_pending = false;
                    fire_arrival = true;
                }
                self.trajectory = None;
                self.motion_t = 0.0;
            }
        }

        if let Some(t) = self.click_t {
            let next = t + dt * 4.0; // full pulse over 0.25s
            self.click_t = if next >= 1.0 { None } else { Some(next) };
        }
        if let Some(age) = self.click_age {
            let next = age + dt;
            self.click_age = (next < CLICK_FX_SECS).then_some(next);
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
        // The idle clock and fade curve are the shared agent cursor contract
        // (`cua_driver_core::agent_cursor`); this only feeds it frame deltas.
        let idle_hide_ms = self.motion.idle_hide_ms;
        if idle_hide_ms > 0.0 {
            // An active semantic cue (a keyboard-first press shows the
            // cursor before any motion) and a held button keep it visible
            // like motion does. The display action a move, snap or click
            // plays after itself does not: that motion already restarted
            // the clock, and the timeout counts from it.
            let moving = self.is_moving()
                || self.click_t.is_some()
                || self.pressed
                || (self.semantic_cue && self.visual.resolved_action != CursorAction::Idle);
            let mut idle = AgentCursorVisibility::new();
            idle.set_idle_secs(self.idle_secs);
            idle.tick(dt, moving);
            self.idle_secs = idle.idle_secs();
            self.idle_alpha = idle.alpha(idle_hide_ms);
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
    /// still at [`UNPLACED_POS`].  Windows/Linux
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
                target,
            } => {
                let reveal_badge = !self.is_revealed();
                // macOS-only: if the cursor has never been placed, put it on
                // the target so the move starts on-screen.
                if move_to_snap_sentinel && !is_placed(self.pos) {
                    self.pos = crate::anchor_for_pointer(x, y, end_heading_radians);
                    self.heading = end_heading_radians;
                }
                // Plan the hotspot so it lands on `(x, y)`; the anchor follows
                // from the heading at every sample.
                let (fx, fy) = crate::pointer_for_anchor(self.pos.0, self.pos.1, self.heading);
                self.move_seq += 1;
                let request = MoveRequest {
                    from: Pt::new(fx, fy),
                    from_heading: self.heading,
                    to: Pt::new(x, y),
                    end_heading: end_heading_radians,
                    target,
                    seed: format!("{}|{}", self.cfg.cursor_id, self.move_seq),
                    reduced_motion: self.reduced_motion(),
                };
                self.trajectory = Some(plan_move(&self.motion, &request));
                self.motion_t = 0.0;
                self.arrival_pending = true;
                if matches!(
                    self.visual.resolved_action,
                    CursorAction::Idle | CursorAction::Navigate
                ) {
                    let delivery = self.visual.delivery;
                    let target = self.visual.target;
                    self.visual.begin(CursorAction::Navigate, delivery, target);
                    self.semantic_cue = false;
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
                let reveal_badge = !self.is_revealed();
                self.pos = (x, y);
                if let Some(heading) = heading_radians {
                    self.heading = heading;
                }
                self.cancel_motion();
                if matches!(
                    self.visual.resolved_action,
                    CursorAction::Idle | CursorAction::Navigate
                ) {
                    let delivery = self.visual.delivery;
                    let target = self.visual.target;
                    self.visual.begin(CursorAction::Navigate, delivery, target);
                    self.semantic_cue = false;
                }
                self.idle_secs = 0.0;
                self.idle_alpha = 1.0;
                if reveal_badge {
                    self.reveal_session_badge();
                }
                true
            }
            OverlayCommand::ClickPulse { x, y } => {
                let reveal_badge = !self.is_revealed();
                // macOS only snaps on first placement (sentinel state); after
                // that the cursor stays where the animation landed. Windows
                // and Linux always snap. Both anchor the click point so the
                // hotspot stays on it instead of jumping by the anchor offset.
                // A move still settling onto this click point (follow-through,
                // bounce) finishes on it by itself; snapping would cut it off.
                let settling_here = self.trajectory.as_ref().is_some_and(|traj| {
                    let end = traj.end();
                    self.motion_t < traj.duration() && (end.x - x).hypot(end.y - y) <= 2.0
                });
                if (!click_pulse_sentinel_only || !is_placed(self.pos)) && !settling_here {
                    self.pos = crate::anchor_for_pointer(x, y, self.heading);
                }
                self.click_t = Some(0.0);
                self.click_age = Some(0.0);
                self.click_point = (x, y);
                if matches!(
                    self.visual.resolved_action,
                    CursorAction::Idle | CursorAction::Navigate | CursorAction::Click
                ) {
                    let delivery = self.visual.delivery;
                    let target = self.visual.target;
                    self.visual.begin(CursorAction::Click, delivery, target);
                    self.semantic_cue = false;
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
                    self.semantic_cue = false;
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
            OverlayCommand::ApplyMotion(args) => {
                if let Ok(motion) = self.motion.with_motion_args(&args) {
                    self.motion = motion;
                }
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
                self.semantic_cue = true;
                // A semantic cue re-reveals an idle-faded cursor that already
                // has a position; `tick_idle` keeps it visible until the cue ends.
                self.idle_secs = 0.0;
                self.idle_alpha = 1.0;
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

// ── Motion effects ───────────────────────────────────────────────────────
//
// The effect geometry (trail, glow, magnet, ripple, squish) comes from
// `cua_cursor_motion::effects`; this module only paints it.

use cua_cursor_motion::effects::{self, EffectFrame, CLICK_FX_SECS, MAGNET_INFLATE};

/// Effect colour: the session tint lifted toward white.
fn effect_rgb(tint: [u8; 4]) -> (u8, u8, u8) {
    let [r, g, b] = effects::effect_rgb([tint[0], tint[1], tint[2]]);
    (r, g, b)
}

fn effect_paint(rgb: (u8, u8, u8), alpha: f64) -> tiny_skia::Paint<'static> {
    tiny_skia::Paint {
        shader: tiny_skia::Shader::SolidColor(tiny_skia::Color::from_rgba8(
            rgb.0,
            rgb.1,
            rgb.2,
            (alpha.clamp(0.0, 1.0) * 255.0).round() as u8,
        )),
        anti_alias: true,
        ..Default::default()
    }
}

/// Paint the motion effects that sit under the cursor artwork. Coordinates
/// are logical; `to_px` maps them into the pixmap.
fn paint_effects_under(
    pm: &mut tiny_skia::Pixmap,
    frame: &EffectFrame,
    rgb: (u8, u8, u8),
    alpha_scale: f64,
    to_px: &dyn Fn(f64, f64) -> (f32, f32),
    s: f32,
) {
    use tiny_skia::{
        GradientStop, PathBuilder, Point, RadialGradient, SpreadMode, Stroke, Transform,
    };
    if let Some(glow) = frame.glow {
        let (gx, gy) = to_px(glow.x, glow.y);
        let r = glow.r as f32 * s;
        let a = glow.alpha * alpha_scale;
        let color = |a: f64| {
            tiny_skia::Color::from_rgba8(rgb.0, rgb.1, rgb.2, (a.clamp(0.0, 1.0) * 255.0) as u8)
        };
        if let Some(shader) = RadialGradient::new(
            Point::from_xy(gx, gy),
            Point::from_xy(gx, gy),
            r,
            vec![
                GradientStop::new(0.0, color(a)),
                GradientStop::new(1.0, color(0.0)),
            ],
            SpreadMode::Pad,
            Transform::identity(),
        ) {
            let paint = tiny_skia::Paint {
                shader,
                anti_alias: true,
                ..Default::default()
            };
            if let Some(circle) = PathBuilder::from_circle(gx, gy, r) {
                pm.fill_path(
                    &circle,
                    &paint,
                    tiny_skia::FillRule::Winding,
                    Transform::identity(),
                    None,
                );
            }
        }
    }
    for seg in &frame.trail {
        let mut pb = PathBuilder::new();
        let (ax, ay) = to_px(seg.a.0, seg.a.1);
        let (bx, by) = to_px(seg.b.0, seg.b.1);
        pb.move_to(ax, ay);
        pb.line_to(bx, by);
        if let Some(path) = pb.finish() {
            let stroke = Stroke {
                width: seg.width as f32 * s,
                line_cap: tiny_skia::LineCap::Round,
                ..Default::default()
            };
            pm.stroke_path(
                &path,
                &effect_paint(rgb, seg.alpha * alpha_scale),
                &stroke,
                Transform::identity(),
                None,
            );
        }
    }
    if let Some(magnet) = frame.magnet {
        let [x, y, w, h] = magnet.rect;
        let i = MAGNET_INFLATE;
        let (x0, y0) = to_px(x - i, y - i);
        let (x1, y1) = to_px(x + w + i, y + h + i);
        let radius = (8.0 * s).min((x1 - x0) / 2.0).min((y1 - y0) / 2.0);
        if let Some(path) = rounded_rect(x0, y0, x1 - x0, y1 - y0, radius) {
            // Wide faint strokes stand in for a blur.
            for (width, a) in [(14.0, 0.10), (8.0, 0.22), (3.0, 0.9)] {
                let stroke = Stroke {
                    width: width * s,
                    ..Default::default()
                };
                pm.stroke_path(
                    &path,
                    &effect_paint(rgb, a * magnet.glow * alpha_scale),
                    &stroke,
                    Transform::identity(),
                    None,
                );
            }
        }
    }
}

/// Paint the click ripple, which sits over the cursor artwork.
fn paint_effects_over(
    pm: &mut tiny_skia::Pixmap,
    frame: &EffectFrame,
    rgb: (u8, u8, u8),
    alpha_scale: f64,
    to_px: &dyn Fn(f64, f64) -> (f32, f32),
    s: f32,
) {
    if let Some(ripple) = frame.ripple {
        let (cx, cy) = to_px(ripple.x, ripple.y);
        if let Some(circle) = tiny_skia::PathBuilder::from_circle(cx, cy, ripple.r as f32 * s) {
            let stroke = tiny_skia::Stroke {
                width: ripple.width as f32 * s,
                ..Default::default()
            };
            pm.stroke_path(
                &circle,
                &effect_paint(rgb, ripple.alpha * alpha_scale),
                &stroke,
                tiny_skia::Transform::identity(),
                None,
            );
        }
    }
}

fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    if !(w > 0.0 && h > 0.0) {
        return None;
    }
    let r = r.max(0.0);
    let k = 0.552_284_8 * r;
    let mut pb = tiny_skia::PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish()
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
    if !core.is_revealed() || core.pinned_target_off_workspace {
        return;
    }

    let s = backing_scale.max(1.0) as f64; // logical-pt → pixmap-pixel scale
    let sf = s as f32;

    // Cursor anchor in pixmap-pixel space: subtract the (logical) origin
    // first, then scale into pixmap pixels.
    let (px, py) = ((core.pos.0 - origin_x) * s, (core.pos.1 - origin_y) * s);
    let heading = core.heading;
    // Themes pivot on their hotspot, which is drawn at the pointer point.
    let (tip_x, tip_y) = crate::pointer_for_anchor(core.pos.0, core.pos.1, heading);
    let (tip_x, tip_y) = ((tip_x - origin_x) * s, (tip_y - origin_y) * s);
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

    let effects = core.effect_frame();
    let effect_rgb = effect_rgb(crate::session_fill_rgba(&core.cfg.cursor_id));
    let to_px = |x: f64, y: f64| (((x - origin_x) * s) as f32, ((y - origin_y) * s) as f32);
    paint_effects_under(pm, &effects, effect_rgb, f64::from(alpha_scale), &to_px, sf);
    // Click squish scales the artwork about its hotspot.
    let art_scale = backing_scale.max(1.0) * (1.0 - effects.squish as f32);

    if let Some(theme) = core.theme.as_deref() {
        let tint = (theme.id == crate::DEFAULT_THEME_ID)
            .then(|| crate::session_fill_rgba(&core.cfg.cursor_id));
        crate::paint_compiled_theme_with_tint(
            pm,
            theme,
            &core.visual,
            tip_x as f32,
            tip_y as f32,
            heading as f32,
            art_scale,
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
            tip_x as f32,
            tip_y as f32,
            heading as f32,
            art_scale,
            alpha_scale,
            crate::session_fill_rgba(&core.cfg.cursor_id),
        );
    }
    paint_effects_over(pm, &effects, effect_rgb, f64::from(alpha_scale), &to_px, sf);

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
    use crate::{CursorConfig, MotionStyle};

    /// Run a move of `dist_pts` until it arrives and return how many seconds
    /// it took.
    fn arrival_secs(style: MotionStyle, glide_ms: f64, dist_pts: f64) -> f64 {
        let mut core = RenderStateCore::new(CursorConfig::default());
        core.motion.style = style;
        core.motion.glide_duration_ms = glide_ms;
        core.motion.idle_hide_ms = 0.0;
        // Heading pi: the classic glide leaves and arrives heading +x, an
        // effectively straight path of length ~dist_pts.
        core.heading = std::f64::consts::PI;
        core.pos = crate::anchor_for_pointer(0.0, 0.0, core.heading);
        core.apply_command_base(
            OverlayCommand::MoveTo {
                x: dist_pts,
                y: 0.0,
                end_heading_radians: std::f64::consts::PI,
                target: None,
            },
            false,
            false,
        );
        let dt = 1.0 / 240.0;
        let mut t = 0.0;
        for _ in 0..200_000 {
            let arrived = core.tick_motion(dt);
            t += dt;
            if arrived {
                break;
            }
        }
        t
    }

    #[test]
    fn fixed_duration_is_distance_independent() {
        for style in [MotionStyle::Classic, MotionStyle::SignatureArc] {
            let short = arrival_secs(style, 300.0, 120.0);
            let long = arrival_secs(style, 300.0, 1400.0);
            assert!((short - 0.3).abs() < 0.06, "{style:?} short={short}");
            assert!((long - 0.3).abs() < 0.06, "{style:?} long={long}");
        }
    }

    #[test]
    fn zero_keeps_distance_aware_timing() {
        for style in MotionStyle::ALL {
            let short = arrival_secs(style, 0.0, 120.0);
            let long = arrival_secs(style, 0.0, 1400.0);
            assert!(long > short + 0.1, "{style:?} short={short} long={long}");
        }
    }

    #[test]
    fn arrival_lets_the_settle_play_during_the_click() {
        let mut core = RenderStateCore::new(CursorConfig::default());
        core.motion.style = MotionStyle::SpringSettle;
        core.pos = (100.0, 100.0);
        core.apply_command_base(
            OverlayCommand::MoveTo {
                x: 700.0,
                y: 400.0,
                end_heading_radians: std::f64::consts::FRAC_PI_4,
                target: None,
            },
            false,
            false,
        );
        let dt = 1.0 / 120.0;
        while !core.tick_motion(dt) {}
        assert!(core.is_moving(), "the bounce is still playing at arrival");
        // Windows/Linux snap on ClickPulse; a settling move must not be cut.
        core.apply_command_base(
            OverlayCommand::ClickPulse { x: 700.0, y: 400.0 },
            false,
            false,
        );
        assert!(core.is_moving());
        for _ in 0..600 {
            core.tick_motion(dt);
        }
        assert!(core.trajectory.is_none());
        let (px, py) = crate::pointer_for_anchor(core.pos.0, core.pos.1, core.heading);
        assert!((px - 700.0).abs() < 1e-6 && (py - 400.0).abs() < 1e-6);
    }

    /// The comet trail starts at the arrow's body, under the artwork, not at
    /// the tip: the head sits `POINTER_ANCHOR_OFFSET` behind the hotspot along
    /// the arrow's axis, so the tip stays clean.
    #[test]
    fn the_comet_trail_starts_behind_the_tip() {
        let mut core = RenderStateCore::new(CursorConfig::default());
        core.motion.style = MotionStyle::CometSwoop;
        core.pos = (100.0, 100.0);
        core.apply_command_base(
            OverlayCommand::MoveTo {
                x: 900.0,
                y: 500.0,
                end_heading_radians: std::f64::consts::FRAC_PI_4,
                target: None,
            },
            false,
            false,
        );
        let mut checked = 0;
        for _ in 0..240 {
            core.tick_motion(1.0 / 120.0);
            let frame = core.effect_frame();
            // The head segment (full width) is the one that ends at the cursor;
            // a slow last step leaves it out.
            let Some(head) = frame.trail.last().filter(|seg| seg.width > 11.99) else {
                continue;
            };
            let tip = crate::pointer_for_anchor(core.pos.0, core.pos.1, core.heading);
            let to_tip = (head.b.0 - tip.0).hypot(head.b.1 - tip.1);
            assert!(
                (to_tip - crate::POINTER_ANCHOR_OFFSET).abs() < 1e-6,
                "trail head {:?} is {to_tip} from the tip {tip:?}",
                head.b
            );
            assert!(
                (head.b.0 - core.pos.0).hypot(head.b.1 - core.pos.1) < 1e-6,
                "the head is the cursor's anchor"
            );
            checked += 1;
        }
        assert!(checked > 20, "the trail was only drawn {checked} times");
    }

    /// A very short trail (the first frames of a move) fades out instead of
    /// showing a stub.
    #[test]
    fn a_very_short_comet_trail_is_faint() {
        let mut core = RenderStateCore::new(CursorConfig::default());
        core.motion.style = MotionStyle::CometSwoop;
        core.pos = (100.0, 100.0);
        core.apply_command_base(
            OverlayCommand::MoveTo {
                x: 900.0,
                y: 500.0,
                end_heading_radians: std::f64::consts::FRAC_PI_4,
                target: None,
            },
            false,
            false,
        );
        let mut strongest_early = 0.0_f64;
        let mut strongest_cruise = 0.0_f64;
        for frame_no in 0..240 {
            core.tick_motion(1.0 / 120.0);
            let alpha = core
                .effect_frame()
                .trail
                .iter()
                .map(|seg| seg.alpha)
                .fold(0.0_f64, f64::max);
            if frame_no < 4 {
                strongest_early = strongest_early.max(alpha);
            } else if frame_no > 40 {
                strongest_cruise = strongest_cruise.max(alpha);
            }
        }
        assert!(
            strongest_early < strongest_cruise * 0.5,
            "early {strongest_early} vs cruising {strongest_cruise}"
        );
    }

    #[test]
    fn effects_report_bounds_while_they_play() {
        let mut core = RenderStateCore::new(CursorConfig::default());
        core.motion.style = MotionStyle::CometSwoop;
        core.pos = (100.0, 100.0);
        core.apply_command_base(
            OverlayCommand::MoveTo {
                x: 900.0,
                y: 500.0,
                end_heading_radians: std::f64::consts::FRAC_PI_4,
                target: None,
            },
            false,
            false,
        );
        for _ in 0..30 {
            core.tick_motion(1.0 / 120.0);
        }
        let bounds = core.effect_bounds().expect("trail bounds while moving");
        assert!(bounds[2] > 10.0 && bounds[3] > 5.0);
        core.effects_capable = false;
        assert!(
            core.effect_bounds().is_none(),
            "no trail without alpha blending"
        );
        core.effects_capable = true;
        for _ in 0..600 {
            core.tick_motion(1.0 / 120.0);
        }
        assert!(core.effect_bounds().is_none());
        assert!(!core.needs_frame_tick() || core.has_resting_motion());
    }
}

#[cfg(test)]
mod session_badge_and_action_tests {
    use super::*;
    use crate::{CursorConfig, DeliveryModifier, TargetModifier};

    #[test]
    fn idle_hide_zero_keeps_a_positioned_session_cursor_visible() {
        let mut core = RenderStateCore::new(CursorConfig::default());
        core.motion.idle_hide_ms = 0.0;
        assert!(core.apply_command_base(
            OverlayCommand::ClickPulse { x: 40.0, y: 60.0 },
            false,
            false,
        ));

        core.tick_motion(2.0);

        assert!(core.is_revealed());
        assert_eq!(
            core.pos,
            crate::anchor_for_pointer(40.0, 60.0, core.heading)
        );
        assert_eq!(core.idle_alpha, 1.0);
    }

    #[test]
    fn semantic_cue_re_reveals_an_idle_hidden_cursor_until_it_ends() {
        let frame = 1.0 / 60.0;
        let mut core = RenderStateCore::new(CursorConfig::default());
        core.motion.idle_hide_ms = 200.0;
        core.apply_command_base(
            OverlayCommand::ClickPulse { x: 40.0, y: 60.0 },
            false,
            false,
        );
        for _ in 0..120 {
            core.tick_motion(frame);
        }
        assert_eq!(core.idle_alpha, 0.0, "the positioned cursor idles out");

        core.apply_command_base(
            OverlayCommand::BeginAction {
                action: CursorAction::Text,
                delivery: None,
                target: None,
            },
            false,
            false,
        );
        assert_eq!(core.idle_alpha, 1.0, "a keyboard cue re-reveals it");
        for _ in 0..120 {
            core.tick_motion(frame);
        }
        assert_eq!(core.idle_alpha, 1.0, "it stays visible while the cue runs");

        core.apply_command_base(OverlayCommand::EndAction(CursorAction::Text), false, false);
        for _ in 0..120 {
            core.tick_motion(frame);
        }
        assert_eq!(core.idle_alpha, 0.0, "it idles out again after the cue");
    }

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
    fn movement_and_click_pulse_preserve_the_active_semantic_context() {
        // Text keeps its action through a pulse; Click re-begins itself and
        // must carry the declared delivery and target across that restart.
        for action in [CursorAction::Text, CursorAction::Click] {
            let mut core = RenderStateCore::new(CursorConfig::default());
            core.pos = (20.0, 20.0);
            core.apply_command_base(
                OverlayCommand::BeginAction {
                    action,
                    delivery: Some(DeliveryModifier::Background),
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
                    target: None,
                },
                false,
                false,
            );
            assert_eq!(core.visual.resolved_action, action);
            assert_eq!(
                (core.visual.delivery, core.visual.target),
                (Some(DeliveryModifier::Background), Some(TargetModifier::Ax)),
                "{action:?} after move"
            );
            core.apply_command_base(
                OverlayCommand::ClickPulse { x: 200.0, y: 100.0 },
                false,
                false,
            );
            assert_eq!(core.visual.resolved_action, action);
            assert_eq!(
                (core.visual.delivery, core.visual.target),
                (Some(DeliveryModifier::Background), Some(TargetModifier::Ax)),
                "{action:?} after click pulse"
            );
            assert_eq!(
                core.badge_modifiers,
                Some((Some(DeliveryModifier::Background), Some(TargetModifier::Ax))),
                "{action:?} badge context"
            );
        }
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

    #[test]
    fn cursor_pinned_to_an_off_workspace_window_paints_nothing() {
        let mut core = RenderStateCore::new(CursorConfig::default());
        core.pos = (32.0, 32.0);
        core.idle_alpha = 1.0;
        core.visible = true;
        core.pinned_target_off_workspace = true;
        let mut pm = tiny_skia::Pixmap::new(64, 64).unwrap();
        paint_cursor(&mut pm, &core, 0.0, 0.0, None, 1.0);
        assert_eq!(visible_pixel_count(&pm), 0);

        core.pinned_target_off_workspace = false;
        paint_cursor(&mut pm, &core, 0.0, 0.0, None, 1.0);
        assert!(visible_pixel_count(&pm) > 0);
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

#[cfg(test)]
mod agent_idle_tests {
    use super::*;
    use crate::CursorConfig;
    use cua_driver_core::agent_cursor::{AGENT_CURSOR_FADE, AGENT_CURSOR_IDLE_TIMEOUT};

    fn placed() -> RenderStateCore {
        let mut core = RenderStateCore::new(CursorConfig::default());
        assert!(core.apply_command_base(
            OverlayCommand::SnapTo {
                x: 40.0,
                y: 60.0,
                heading_radians: None,
            },
            false,
            false,
        ));
        core
    }

    /// Every platform renderer ticks this core, so this is the overlay side of
    /// the shared idle contract: visible for the presence agent timeout, then
    /// gone within one fade.
    #[test]
    fn default_cursor_fades_at_the_shared_agent_idle_timeout() {
        let mut core = placed();
        assert_eq!(
            core.motion.idle_hide_ms,
            AGENT_CURSOR_IDLE_TIMEOUT.as_millis() as f64
        );
        let idle = AGENT_CURSOR_IDLE_TIMEOUT.as_secs_f64();
        let step = 0.05;
        let mut t = 0.0;
        while t + step < idle - 0.1 {
            core.tick_motion(step);
            t += step;
        }
        assert!(core.is_revealed(), "still visible at {t}s");
        while t < idle + AGENT_CURSOR_FADE.as_secs_f64() + step {
            core.tick_motion(step);
            t += step;
        }
        assert!(!core.is_revealed(), "hidden after the fade at {t}s");
    }

    #[test]
    fn disabled_cursor_does_not_wait_for_an_invisible_glide() {
        let mut core = placed();
        assert!(core.should_animate_to_target());
        core.apply_command_base(OverlayCommand::SetEnabled(false), false, false);
        assert!(!core.should_animate_to_target());
        core.apply_command_base(OverlayCommand::SetEnabled(true), false, false);
        assert!(core.should_animate_to_target());
        core.idle_alpha = 0.0;
        assert!(
            core.should_animate_to_target(),
            "idle fading must still permit a new glide"
        );
        core.cfg.enabled = false;
        assert!(!core.should_animate_to_target());
    }

    /// A held button is activity for as long as it is held; the timeout
    /// then counts from the release, like any other motion.
    #[test]
    fn a_held_button_keeps_the_cursor_until_release() {
        let mut core = placed();
        core.apply_command_base(OverlayCommand::SetPressed(true), false, false);
        let idle = AGENT_CURSOR_IDLE_TIMEOUT.as_secs_f64();
        let step = 0.05;
        let mut t = 0.0;
        while t < 2.0 * idle {
            core.tick_motion(step);
            t += step;
        }
        assert!(core.is_revealed(), "held for {t}s and still visible");
        core.apply_command_base(OverlayCommand::SetPressed(false), false, false);
        let mut since = 0.0;
        while since + step < idle - 0.1 {
            core.tick_motion(step);
            since += step;
        }
        assert!(core.is_revealed(), "visible {since}s after the release");
        while since < idle + AGENT_CURSOR_FADE.as_secs_f64() + step {
            core.tick_motion(step);
            since += step;
        }
        assert!(!core.is_revealed(), "hidden {since}s after the release");
    }

    #[test]
    fn one_cursor_idling_out_leaves_an_active_one_visible() {
        let mut idle = placed();
        let mut active = placed();
        // 20 Hz for the idle timeout plus a second; the active cursor acts
        // every 5 s.
        let ticks = (AGENT_CURSOR_IDLE_TIMEOUT.as_secs() + 1) * 20;
        for tick in 0..ticks {
            idle.tick_motion(0.05);
            active.tick_motion(0.05);
            if tick % 100 == 0 {
                active.apply_command_base(
                    OverlayCommand::SnapTo {
                        x: 50.0 + tick as f64,
                        y: 60.0,
                        heading_radians: None,
                    },
                    false,
                    false,
                );
            }
        }
        assert!(!idle.is_revealed());
        assert!(active.is_revealed());
    }
}

#[cfg(test)]
mod pointer_anchor_tests {
    use super::*;
    use crate::{
        track_pointer_command, CompiledAnimation, CompiledDrawCommand, CompiledFrame,
        CompiledGeometry, CompiledTheme, CompiledTransform, CursorAction, CursorConfig,
    };
    use std::f64::consts::{FRAC_PI_2, FRAC_PI_4};

    /// A theme whose only artwork is a small disc centred on a non-central
    /// hotspot, so the disc centroid is the painted hotspot.
    fn hotspot_marker_theme() -> Arc<CompiledTheme> {
        let animation = CompiledAnimation {
            still_frame: 0,
            frames: vec![CompiledFrame {
                commands: vec![CompiledDrawCommand {
                    geometries: vec![CompiledGeometry::Ellipse {
                        center: [55.0, 30.0],
                        size: [8.0, 8.0],
                    }],
                    transform: CompiledTransform::default(),
                    opacity: 1.0,
                    fill: Some([255, 0, 0, 255]),
                    stroke: None,
                }],
            }],
        };
        Arc::new(CompiledTheme {
            id: "com.example.hotspot".into(),
            name: "Hotspot".into(),
            version: "1.0.0".into(),
            author: "Example Author".into(),
            license: "MIT".into(),
            profile: crate::THEME_PROFILE.into(),
            source_hash: [0; 32],
            hotspot: [55, 30],
            actions: CursorAction::ALL
                .into_iter()
                .map(|action| (action.as_str().to_owned(), animation.clone()))
                .collect(),
        })
    }

    fn cursor() -> RenderStateCore {
        let mut core = RenderStateCore::new(CursorConfig::default());
        core.theme = Some(hotspot_marker_theme());
        core.motion.idle_hide_ms = 0.0;
        core
    }

    /// Painted hotspot in logical points.
    fn painted_hotspot(core: &RenderStateCore, backing_scale: f32) -> (f64, f64) {
        let (width, height) = (
            (400.0 * backing_scale) as u32,
            (300.0 * backing_scale) as u32,
        );
        let pm = render_frame(core, width, height, 0.0, 0.0, None, backing_scale);
        let (weight, x_sum, y_sum) = pm.data().chunks_exact(4).enumerate().fold(
            (0.0, 0.0, 0.0),
            |(weight, x_sum, y_sum), (index, pixel)| {
                let alpha = f64::from(pixel[3]);
                let x = (index % pm.width() as usize) as f64 + 0.5;
                let y = (index / pm.width() as usize) as f64 + 0.5;
                (weight + alpha, x_sum + x * alpha, y_sum + y * alpha)
            },
        );
        assert!(weight > 0.0, "hotspot marker was not painted");
        let scale = f64::from(backing_scale);
        (x_sum / weight / scale, y_sum / weight / scale)
    }

    fn assert_hotspot_at(core: &RenderStateCore, backing_scale: f32, x: f64, y: f64, when: &str) {
        let (hx, hy) = painted_hotspot(core, backing_scale);
        assert!(
            (hx - x).abs() <= 0.5 && (hy - y).abs() <= 0.5,
            "{when}: hotspot painted at ({hx:.2}, {hy:.2}), expected ({x}, {y}) \
             at heading {:.3} and {backing_scale}x",
            core.heading
        );
    }

    fn settle(core: &mut RenderStateCore, _macos: bool) {
        for _ in 0..1200 {
            core.tick_motion(1.0 / 60.0);
            if core.trajectory.is_none() && core.click_age.is_none() {
                return;
            }
        }
        panic!("cursor did not settle");
    }

    /// Every pointer-producing command anchors the cursor so the theme hotspot
    /// is painted on the requested coordinate, on both the macOS
    /// (sentinel-only click snap, Swift constants) and Windows/Linux paths.
    #[test]
    fn hotspot_lands_on_requested_pointer_for_move_click_and_drag() {
        for macos in [false, true] {
            for backing_scale in [1.0_f32, 2.0] {
                // First placement from the off-screen sentinel.
                let mut core = cursor();
                core.apply_command_base(
                    OverlayCommand::ClickPulse { x: 90.0, y: 70.0 },
                    macos,
                    macos,
                );
                assert_hotspot_at(&core, backing_scale, 90.0, 70.0, "sentinel click");

                for heading in [FRAC_PI_4, 0.0, FRAC_PI_2, 3.0 * FRAC_PI_4] {
                    core.apply_command_base(
                        OverlayCommand::MoveTo {
                            x: 220.0,
                            y: 160.0,
                            end_heading_radians: heading,
                            target: None,
                        },
                        macos,
                        macos,
                    );
                    settle(&mut core, macos);
                    assert_hotspot_at(&core, backing_scale, 220.0, 160.0, "settled move");

                    core.apply_command_base(
                        OverlayCommand::ClickPulse { x: 220.0, y: 160.0 },
                        macos,
                        macos,
                    );
                    assert_hotspot_at(&core, backing_scale, 220.0, 160.0, "click pulse");

                    core.apply_command_base(
                        OverlayCommand::MoveTo {
                            x: 90.0,
                            y: 70.0,
                            end_heading_radians: heading,
                            target: None,
                        },
                        macos,
                        macos,
                    );
                    settle(&mut core, macos);
                }

                core.apply_command_base(track_pointer_command(150.0, 110.0), macos, macos);
                assert_hotspot_at(&core, backing_scale, 150.0, 110.0, "tracked drag");
            }
        }
    }
}

/// Proof that moving the motion math into the `cua-cursor-motion` crate changed no
/// behaviour. `tests/fixtures/motion_equivalence.json` was captured from the
/// driver before the move (origin/main 79a4a4635) by this module's
/// `capture()`: a SHA-256 over the bits of every sample of 1650 planned
/// trajectories (6 styles x 3 timings x 8 knob sets x 11 moves, plus reduced
/// motion) and of every effect frame of 24 scripted move-and-click runs, with
/// a few readable spot samples, counts and per-case sums. On the capture
/// platform it must stay bit-identical; elsewhere, within 1e-12 relative.
#[cfg(test)]
mod motion_equivalence_tests {
    use super::*;
    use crate::trajectory::{plan_move, MoveRequest, Pt};
    use crate::{CursorConfig, MotionConfig, MotionEffects, MotionStyle, MotionTiming};
    use serde_json::{json, Value};
    use sha2::{Digest, Sha256};
    use std::f64::consts::{FRAC_PI_4, PI};

    /// A SHA-256 over the exact bits of every value, plus a count and two
    /// sums (plain and position-weighted) that survive the last-bit
    /// differences between platforms' math libraries.
    struct Bits {
        sha: Sha256,
        n: u64,
        s0: f64,
        s1: f64,
    }

    impl Bits {
        fn new() -> Self {
            Self {
                sha: Sha256::new(),
                n: 0,
                s0: 0.0,
                s1: 0.0,
            }
        }
        fn add(&mut self, v: f64) {
            self.n += 1;
            self.s0 += v;
            self.s1 += v * (self.n % 1000) as f64;
        }
        fn f(&mut self, v: f64) {
            self.sha.update(v.to_bits().to_le_bytes());
            self.add(v);
        }
        fn opt(&mut self, v: Option<f64>) {
            match v {
                Some(v) => {
                    self.sha.update([1]);
                    self.add(1.0);
                    self.f(v);
                }
                None => {
                    self.sha.update([0]);
                    self.add(0.0);
                }
            }
        }
        fn flag(&mut self, v: bool) {
            self.sha.update([u8::from(v)]);
            self.add(f64::from(u8::from(v)));
        }
        fn done(self) -> (String, Value) {
            let sha = self
                .sha
                .finalize()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            (sha, json!([self.n, self.s0, self.s1]))
        }
    }

    type Move = ((f64, f64), f64, (f64, f64), f64, Option<[f64; 4]>);

    fn moves() -> Vec<Move> {
        vec![
            (
                (100.0, 100.0),
                FRAC_PI_4,
                (600.0, 420.0),
                FRAC_PI_4,
                Some([560.0, 400.0, 80.0, 40.0]),
            ),
            (
                (600.0, 420.0),
                FRAC_PI_4,
                (120.0, 80.0),
                FRAC_PI_4,
                Some([100.0, 70.0, 40.0, 20.0]),
            ),
            (
                (50.0, 700.0),
                0.0,
                (1240.0, 60.0),
                FRAC_PI_4,
                Some([1220.0, 50.0, 40.0, 20.0]),
            ),
            (
                (300.0, 300.0),
                FRAC_PI_4,
                (310.0, 304.0),
                FRAC_PI_4,
                Some([305.0, 300.0, 10.0, 8.0]),
            ),
            ((800.0, 200.0), -PI / 2.0, (800.0, 640.0), FRAC_PI_4, None),
            (
                (10.0, 10.0),
                FRAC_PI_4,
                (10.0, 10.0),
                FRAC_PI_4,
                Some([0.0, 0.0, 20.0, 20.0]),
            ),
            ((0.0, 0.0), 3.0, (2400.0, 1300.0), 0.0, None),
            ((900.0, 500.0), FRAC_PI_4, (897.0, 501.0), FRAC_PI_4, None),
            (
                (1400.0, 90.0),
                2.2,
                (40.0, 880.0),
                -1.0,
                Some([20.0, 860.0, 300.0, 200.0]),
            ),
            (
                (200.0, 600.0),
                FRAC_PI_4,
                (700.0, 600.0),
                FRAC_PI_4,
                Some([f64::NAN, 0.0, 10.0, 10.0]),
            ),
            (
                (640.0, 360.0),
                FRAC_PI_4,
                (660.0, 900.0),
                FRAC_PI_4,
                Some([655.0, 895.0, 12.0, 12.0]),
            ),
        ]
    }

    fn knobs() -> Vec<(&'static str, MotionConfig)> {
        let d = MotionConfig::default();
        vec![
            ("default", d.clone()),
            (
                "straight",
                MotionConfig {
                    arc_size: 0.0,
                    ..d.clone()
                },
            ),
            (
                "wide",
                MotionConfig {
                    arc_size: 0.6,
                    arc_flow: -0.7,
                    ..d.clone()
                },
            ),
            (
                "handles",
                MotionConfig {
                    start_handle: 0.1,
                    end_handle: 0.8,
                    arc_flow: 0.9,
                    ..d.clone()
                },
            ),
            (
                "springy",
                MotionConfig {
                    spring: 0.35,
                    turn_radius: 30.0,
                    ..d.clone()
                },
            ),
            (
                "legacy_glide",
                MotionConfig {
                    glide_duration_ms: 300.0,
                    ..d.clone()
                },
            ),
            (
                "long_glide",
                MotionConfig {
                    glide_duration_ms: 2500.0,
                    ..d.clone()
                },
            ),
            (
                "speeds",
                MotionConfig {
                    peak_speed: 1500.0,
                    min_start_speed: 100.0,
                    min_end_speed: 50.0,
                    ..d
                },
            ),
        ]
    }

    fn trajectories() -> Value {
        let mut cases = Vec::new();
        for style in MotionStyle::ALL {
            for timing in [
                MotionTiming::Native,
                MotionTiming::Fitts,
                MotionTiming::Fixed,
            ] {
                for (knob, base) in knobs() {
                    for (i, (from, fh, to, eh, target)) in moves().into_iter().enumerate() {
                        for reduced in [false, true] {
                            if reduced && (knob != "default" || timing != MotionTiming::Native) {
                                continue;
                            }
                            let m = MotionConfig {
                                style,
                                timing,
                                ..base.clone()
                            };
                            let req = MoveRequest {
                                from: Pt::new(from.0, from.1),
                                from_heading: fh,
                                to: Pt::new(to.0, to.1),
                                end_heading: eh,
                                target,
                                seed: format!("eq|{i}"),
                                reduced_motion: reduced,
                            };
                            let traj = plan_move(&m, &req);
                            let mut h = Bits::new();
                            for s in &traj.samples {
                                h.f(s.t);
                                h.f(s.x);
                                h.f(s.y);
                                h.f(s.heading);
                            }
                            h.f(traj.arrival_t);
                            h.opt(traj.snap_t);
                            for v in traj.target {
                                h.f(v);
                            }
                            h.flag(traj.target_known);
                            let fx = traj.effects;
                            for v in [fx.trail, fx.glow, fx.magnet, fx.ripple, fx.squish] {
                                h.flag(v);
                            }
                            let spots: Vec<Value> = (0..=2)
                                .map(|k| {
                                    let s = traj.sample_at(traj.duration() * k as f64 / 2.0);
                                    json!([s.t, s.x, s.y, s.heading])
                                })
                                .collect();
                            let (sha, agg) = h.done();
                            cases.push(json!({
                                "style": style.as_str(),
                                "timing": timing.as_str(),
                                "knobs": knob,
                                "move": i,
                                "reduced": reduced,
                                "samples": traj.samples.len(),
                                "arrival_t": traj.arrival_t,
                                "snap_t": traj.snap_t,
                                "spots": spots,
                                "sha256": sha,
                                "agg": agg,
                            }));
                        }
                    }
                }
            }
        }
        Value::Array(cases)
    }

    fn hash_frame(h: &mut Bits, core: &RenderStateCore) {
        h.f(core.pos.0);
        h.f(core.pos.1);
        h.f(core.heading);
        let frame = core.effect_frame();
        h.flag(frame.glow.is_some());
        if let Some(g) = frame.glow {
            for v in [g.x, g.y, g.r, g.alpha] {
                h.f(v);
            }
        }
        h.f(frame.trail.len() as f64);
        for seg in &frame.trail {
            for v in [seg.a.0, seg.a.1, seg.b.0, seg.b.1, seg.width, seg.alpha] {
                h.f(v);
            }
        }
        h.flag(frame.magnet.is_some());
        if let Some(m) = frame.magnet {
            for v in m.rect {
                h.f(v);
            }
            h.f(m.glow);
        }
        h.flag(frame.ripple.is_some());
        if let Some(r) = frame.ripple {
            for v in [r.x, r.y, r.r, r.width, r.alpha] {
                h.f(v);
            }
        }
        h.f(frame.squish);
        h.opt(
            core.effect_bounds()
                .map(|b| b[0] + b[1] * 3.0 + b[2] * 7.0 + b[3] * 11.0),
        );
    }

    fn effect_runs() -> Value {
        let all_on = MotionEffects {
            trail: Some(true),
            glow: Some(true),
            magnet: Some(true),
            ripple: Some(true),
            squish: Some(true),
        };
        let mut runs = Vec::new();
        for style in MotionStyle::ALL {
            for (fx_name, effects) in [("default", MotionEffects::default()), ("all_on", all_on)] {
                for capable in [true, false] {
                    let mut core = RenderStateCore::new(CursorConfig::default());
                    core.motion = MotionConfig {
                        style,
                        effects,
                        ..MotionConfig::default()
                    };
                    core.effects_capable = capable;
                    core.pos = (100.0, 100.0);
                    let mut h = Bits::new();
                    let mut arrivals = 0;
                    let script: [(usize, OverlayCommand); 4] = [
                        (
                            0,
                            OverlayCommand::MoveTo {
                                x: 900.0,
                                y: 500.0,
                                end_heading_radians: FRAC_PI_4,
                                target: Some([860.0, 480.0, 80.0, 40.0]),
                            },
                        ),
                        (150, OverlayCommand::ClickPulse { x: 900.0, y: 500.0 }),
                        (
                            200,
                            OverlayCommand::MoveTo {
                                x: 140.0,
                                y: 620.0,
                                end_heading_radians: FRAC_PI_4,
                                target: None,
                            },
                        ),
                        (260, OverlayCommand::ClickPulse { x: 140.0, y: 620.0 }),
                    ];
                    let mut script = script.into_iter().peekable();
                    for frame_no in 0..420 {
                        while script.peek().is_some_and(|(at, _)| *at == frame_no) {
                            let (_, cmd) = script.next().unwrap();
                            core.apply_command_base(cmd, false, false);
                        }
                        if core.tick_motion(1.0 / 120.0) {
                            arrivals += 1;
                            h.f(frame_no as f64);
                        }
                        hash_frame(&mut h, &core);
                    }
                    let (sha, agg) = h.done();
                    runs.push(json!({
                        "style": style.as_str(),
                        "effects": fx_name,
                        "capable": capable,
                        "arrivals": arrivals,
                        "sha256": sha,
                                "agg": agg,
                    }));
                }
            }
        }
        Value::Array(runs)
    }

    fn capture() -> Value {
        json!({ "trajectories": trajectories(), "effect_runs": effect_runs() })
    }

    /// Numbers within 1e-12 relative plus 1e-9 absolute (last-bit rounding
    /// differences stay far below that; any real change does not); strings,
    /// booleans and nulls exactly.
    fn close(path: &str, was: &Value, is: &Value) {
        match (was, is) {
            (Value::Number(a), Value::Number(b)) => {
                let (a, b) = (a.as_f64().unwrap(), b.as_f64().unwrap());
                let tol = 1e-12 * a.abs().max(b.abs()) + 1e-9;
                assert!((a - b).abs() <= tol, "{path}: {a} vs {b}");
            }
            (Value::Array(a), Value::Array(b)) => {
                assert_eq!(a.len(), b.len(), "{path}: length");
                for (i, (a, b)) in a.iter().zip(b).enumerate() {
                    close(&format!("{path}[{i}]"), a, b);
                }
            }
            (Value::Object(a), Value::Object(b)) => {
                assert_eq!(a.len(), b.len(), "{path}: keys");
                for (k, v) in a {
                    if k != "sha256" {
                        close(&format!("{path}.{k}"), v, &b[k]);
                    }
                }
            }
            _ => assert_eq!(was, is, "{path}"),
        }
    }

    /// The motion did not change when its math moved into
    /// `cua-cursor-motion`. On macOS arm64, where the fixture was captured,
    /// every sample is bit-identical (the SHA-256s match). Other platforms'
    /// math libraries round the last bit of `sin`, `exp` and friends
    /// differently, so there the sample counts must match exactly and every
    /// spot sample, timing and per-case sum within 1e-12 relative.
    #[test]
    fn motion_is_unchanged_from_the_pre_cua_cursor_motion_driver() {
        let fixture: Value =
            serde_json::from_str(include_str!("../tests/fixtures/motion_equivalence.json"))
                .expect("fixture json");
        // Parse both through the same JSON reader: serde_json's default float
        // parsing is not round-trip exact.
        let now: Value = serde_json::from_str(&capture().to_string()).unwrap();
        let bit_exact = cfg!(all(target_os = "macos", target_arch = "aarch64"));
        for kind in ["trajectories", "effect_runs"] {
            let (was, is) = (
                fixture[kind].as_array().unwrap(),
                now[kind].as_array().unwrap(),
            );
            assert_eq!(was.len(), is.len(), "{kind} count");
            for (i, (was, is)) in was.iter().zip(is).enumerate() {
                close(&format!("{kind}[{i}]"), was, is);
                if bit_exact {
                    assert_eq!(was["sha256"], is["sha256"], "{kind}[{i}] bits changed");
                }
            }
        }
        assert_eq!(fixture["trajectories"].as_array().unwrap().len(), 1650);
        assert_eq!(fixture["effect_runs"].as_array().unwrap().len(), 24);
    }
}
