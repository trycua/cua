//! The tunable knobs of the built-in styles (Cua Driver's
//! `set_agent_cursor_motion` fields that shape motion).

use crate::style::{MotionEffects, MotionStyle, MotionTiming, ResolvedEffects};

/// Knobs for [`plan_move`](crate::plan_move). Defaults match Cua Driver.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct MotionParams {
    /// Trajectory style.
    pub style: MotionStyle,
    /// Move duration model.
    pub timing: MotionTiming,
    /// Effect overrides on top of the style's defaults.
    pub effects: MotionEffects,
    /// Control-point offset from start, as fraction of distance. [0, 1]
    pub start_handle: f64,
    /// Control-point offset from end. [0, 1]
    pub end_handle: f64,
    /// Arc scale. 0.25 keeps each style's own arc, 0 is straight. [0, 1]
    pub arc_size: f64,
    /// Shift of the arc's peak: positive = apex near destination. [-1, 1]
    pub arc_flow: f64,
    /// `classic` arrival spring damping: 1.0 = critical, 0.3 = bouncy.
    pub spring: f64,
    /// Fixed move time in ms; 0 means [`DEFAULT_FIXED_MS`](crate::DEFAULT_FIXED_MS)
    /// for `fixed` timing. A positive value with `native` timing also fixes
    /// the time (the driver's legacy behaviour).
    pub glide_duration_ms: f64,
    /// `classic` peak speed, pt/s.
    pub peak_speed: f64,
    /// `classic` speed floor at the start of the glide, pt/s.
    pub min_start_speed: f64,
    /// `classic` speed floor at the end of the glide, pt/s.
    pub min_end_speed: f64,
    /// `classic` minimum turning radius, pt.
    pub turn_radius: f64,
}

impl Default for MotionParams {
    fn default() -> Self {
        Self {
            style: MotionStyle::default(),
            timing: MotionTiming::default(),
            effects: MotionEffects::default(),
            start_handle: 0.3,
            end_handle: 0.3,
            arc_size: 0.25,
            arc_flow: 0.0,
            spring: 0.72,
            glide_duration_ms: 0.0,
            peak_speed: 900.0,
            min_start_speed: 300.0,
            min_end_speed: 200.0,
            turn_radius: 80.0,
        }
    }
}

impl MotionParams {
    /// The style's effects after overrides.
    pub fn resolved_effects(&self) -> ResolvedEffects {
        self.effects.resolve(self.style.default_effects())
    }
}
