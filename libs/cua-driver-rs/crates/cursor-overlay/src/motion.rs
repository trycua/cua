//! Motion / timing configuration — 1:1 port of `AgentCursorMotion.cs`.

use serde::{Deserialize, Serialize};

/// Runtime-tunable timing and path-shape parameters.
/// All clamp ranges are identical to the C# reference.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MotionConfig {
    /// Control-point offset from start, as fraction of distance. [0, 1]
    pub start_handle: f64,
    /// Control-point offset from end.  [0, 1]
    pub end_handle: f64,
    /// Perpendicular deflection magnitude as fraction of distance. [0, 1]
    pub arc_size: f64,
    /// Deflection asymmetry: positive = apex near destination. [-1, 1]
    pub arc_flow: f64,
    /// Post-arrival spring damping: 1.0 = critical, 0.3 = bouncy. [0.3, 1.0]
    pub spring: f64,
    /// Main glide duration in milliseconds — used only as a legacy override.
    /// When <= 0 the render engine uses speed-based timing instead. [50, 5000]
    pub glide_duration_ms: f64,
    /// Post-click dwell in milliseconds. [0, 5000]
    pub dwell_after_click_ms: f64,
    /// Auto-hide delay in milliseconds. 0 = never hide. [0, 60000]
    pub idle_hide_ms: f64,
    /// Click-press visual duration. [0, 5000]
    pub press_duration_ms: f64,
    /// Peak cursor speed in pts/sec (speed-based mode). Matches Swift peakSpeed=900.
    pub peak_speed: f64,
    /// Minimum cursor speed at start of glide, pts/sec.
    pub min_start_speed: f64,
    /// Minimum cursor speed at end of glide (deceleration floor), pts/sec.
    pub min_end_speed: f64,
}

impl Default for MotionConfig {
    fn default() -> Self {
        Self {
            start_handle: 0.3,
            end_handle: 0.3,
            arc_size: 0.25,
            arc_flow: 0.0,
            spring: 0.72,
            glide_duration_ms: 0.0,  // 0 = speed-based mode
            dwell_after_click_ms: 80.0,
            idle_hide_ms: 20_000.0,  // fade 20s after last activity (matches .NET reference)
            press_duration_ms: 120.0,
            peak_speed: 900.0,
            min_start_speed: 300.0,
            min_end_speed: 200.0,
        }
    }
}

impl MotionConfig {
    pub fn with_overrides(
        &self,
        start_handle: Option<f64>,
        end_handle: Option<f64>,
        arc_size: Option<f64>,
        arc_flow: Option<f64>,
        spring: Option<f64>,
        glide_duration_ms: Option<f64>,
        dwell_after_click_ms: Option<f64>,
        idle_hide_ms: Option<f64>,
        press_duration_ms: Option<f64>,
    ) -> Self {
        fn clamp(v: f64, lo: f64, hi: f64) -> f64 { v.clamp(lo, hi) }
        Self {
            start_handle:         clamp(start_handle.unwrap_or(self.start_handle), 0.0, 1.0),
            end_handle:           clamp(end_handle.unwrap_or(self.end_handle), 0.0, 1.0),
            arc_size:             clamp(arc_size.unwrap_or(self.arc_size), 0.0, 1.0),
            arc_flow:             clamp(arc_flow.unwrap_or(self.arc_flow), -1.0, 1.0),
            spring:               clamp(spring.unwrap_or(self.spring), 0.3, 1.0),
            glide_duration_ms:    clamp(glide_duration_ms.unwrap_or(self.glide_duration_ms), 0.0, 5000.0),
            dwell_after_click_ms: clamp(dwell_after_click_ms.unwrap_or(self.dwell_after_click_ms), 0.0, 5000.0),
            idle_hide_ms:         clamp(idle_hide_ms.unwrap_or(self.idle_hide_ms), 0.0, 60_000.0),
            press_duration_ms:    clamp(press_duration_ms.unwrap_or(self.press_duration_ms), 0.0, 5000.0),
            peak_speed:           self.peak_speed,
            min_start_speed:      self.min_start_speed,
            min_end_speed:        self.min_end_speed,
        }
    }
}
