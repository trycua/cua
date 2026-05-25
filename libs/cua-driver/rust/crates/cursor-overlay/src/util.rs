//! Tiny shared utilities for per-OS overlay renderers.
//!
//! These were duplicated verbatim in `platform-windows/src/overlay.rs` and
//! `platform-linux/src/overlay.rs`. Centralising them here keeps the math
//! truth in one place + lets future ports (Wayland-native, X11 v2, etc.)
//! reuse the same primitives without re-deriving them.

/// Rotate the `current` angle toward `desired` by at most `max_step` radians
/// per call, normalising both inputs to (-π, π].
///
/// Used by the per-OS overlay render loops to ease the cursor's heading
/// toward its planned-path tangent without instant snap. Both Windows + Linux
/// renderers want identical semantics here — exact constants matter because
/// the easing rate is tuned against the same Motion config knobs.
pub fn rotate_toward(current: f64, desired: f64, max_step: f64) -> f64 {
    let mut diff = desired - current;
    while diff > std::f64::consts::PI {
        diff -= 2.0 * std::f64::consts::PI;
    }
    while diff < -std::f64::consts::PI {
        diff += 2.0 * std::f64::consts::PI;
    }
    current + diff.clamp(-max_step, max_step)
}

#[cfg(test)]
mod tests {
    use super::*;
    const PI: f64 = std::f64::consts::PI;

    #[test]
    fn no_motion_when_aligned() {
        assert_eq!(rotate_toward(0.5, 0.5, 0.1), 0.5);
    }

    #[test]
    fn clamps_to_max_step() {
        // desired is +1.0 ahead, max_step = 0.1, so we only move 0.1
        assert!((rotate_toward(0.0, 1.0, 0.1) - 0.1).abs() < 1e-12);
    }

    #[test]
    fn picks_short_way_around() {
        // current = 0.95π, desired = -0.95π (almost a full rotation away).
        // Short way is +0.1π (across the wrap), NOT -1.9π.
        let out = rotate_toward(0.95 * PI, -0.95 * PI, 0.5);
        // We expect motion in the POSITIVE direction (forward wrap).
        assert!(out > 0.95 * PI, "expected positive motion, got {}", out);
    }

    #[test]
    fn negative_direction_when_shorter() {
        // current = -0.95π, desired = 0.95π — short way is the negative wrap.
        let out = rotate_toward(-0.95 * PI, 0.95 * PI, 0.5);
        assert!(out < -0.95 * PI, "expected negative motion, got {}", out);
    }
}
