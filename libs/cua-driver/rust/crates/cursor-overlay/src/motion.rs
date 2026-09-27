//! Motion / timing configuration — 1:1 port of `AgentCursorMotion.cs`.
//!
//! The canonical default values live in [`assets/motion.default.json`] and are
//! embedded here at compile time. `MotionConfig::default()` is derived from
//! that single shared source, so the checked-in spec, the production runtime,
//! and the cursor-gallery exporter cannot drift apart. Changing a default is a
//! two-file review: edit the JSON, then update the independent Rust literal in
//! [`self::spec_tests`] until the parity test passes again.
//!
//! The embedded asset is trusted repository input, not runtime configuration.
//! Invalid JSON or values therefore fail fast on first access; there is
//! deliberately no silent fallback to a second set of defaults.

use serde::de::Error as _;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

const MOTION_SPEC_SCHEMA: &str = "cua.cursor-motion/1";

/// Click-point offset in points: the arrow artwork is centred this far
/// down-right of the reported click coordinate so its tip lands on it.
/// The authoritative value is `shared.click_offset` in the embedded
/// repository-owned movement spec; callers use [`click_offset`] instead of
/// re-declaring a local constant.
pub fn click_offset() -> f64 {
    motion_spec().shared.click_offset
}

const MOTION_SPEC_JSON: &str = include_str!("../assets/motion.default.json");

/// Repository-owned movement specification (`assets/motion.default.json`).
///
/// Only the `shared` section maps onto [`MotionConfig`]; the `spring_settle`
/// section records the deliberate macOS vs Windows/Linux settle divergence
/// consumed by `render_state`'s tick paths.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MotionSpec {
    pub schema: String,
    pub shared: MotionSpecShared,
    pub spring_settle: MotionSpecSpringSettle,
}

/// Bounded movement tunables shared by every platform.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MotionSpecShared {
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
    /// When <= 0 the render engine uses speed-based timing instead. [0, 5000]
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
    /// Minimum turning radius of the Dubins glide path, in points. Smaller =
    /// tighter curves. Matches the Swift reference default of 80.
    pub turn_radius: f64,
    /// Distance from the click point to the artwork centre, in points.
    pub click_offset: f64,
}

/// Per-platform post-arrival spring settle constants.
///
/// macOS runs the Swift reference constants directly (`fixed` mode) and does
/// not scale them with the runtime `spring` override — a documented platform
/// limitation, not an accident. Windows/Linux derive both constants from the
/// runtime `spring` scalar (`derived` mode).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MotionSpecSpringSettle {
    pub macos: SpringSettleMacos,
    pub windows_linux: SpringSettleWindowsLinux,
}

/// How a platform obtains its post-arrival spring constants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpringSettleMode {
    Fixed,
    Derived,
}

/// macOS `tick_swift_constants` settle: fixed constants.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpringSettleMacos {
    pub mode: SpringSettleMode,
    pub k: f64,
    pub c: f64,
    pub overshoot: f64,
}

/// Windows/Linux `tick_motion` settle: derived from the runtime spring scalar.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpringSettleWindowsLinux {
    pub mode: SpringSettleMode,
    pub k_per_spring: f64,
    pub c_per_spring: f64,
    pub overshoot: f64,
}

fn embedded_motion_spec() -> &'static MotionSpec {
    static SPEC: OnceLock<MotionSpec> = OnceLock::new();
    SPEC.get_or_init(|| parse_embedded_motion_spec(MOTION_SPEC_JSON))
}

fn parse_embedded_motion_spec(raw: &str) -> MotionSpec {
    parse_motion_spec(raw).unwrap_or_else(|error| {
        panic!(
            "embedded assets/motion.default.json failed validation: {error}; \
             there is deliberately no fallback because the checked-in spec is the \
             single source of movement defaults"
        )
    })
}

/// Parse a movement specification from raw JSON using the same strict schema
/// as the embedded default. Exporters and tests use this for override files;
/// production always uses the embedded copy.
pub fn parse_motion_spec(raw: &str) -> Result<MotionSpec, serde_json::Error> {
    let spec: MotionSpec = serde_json::from_str(raw)?;
    spec.validate().map_err(serde_json::Error::custom)?;
    Ok(spec)
}

/// The embedded repository-owned movement specification.
pub fn motion_spec() -> &'static MotionSpec {
    embedded_motion_spec()
}

impl MotionSpec {
    fn validate(&self) -> Result<(), String> {
        if self.schema != MOTION_SPEC_SCHEMA {
            return Err(format!(
                "schema must be {MOTION_SPEC_SCHEMA:?}, got {:?}",
                self.schema
            ));
        }

        let shared = &self.shared;
        validate_range("shared.start_handle", shared.start_handle, 0.0, 1.0)?;
        validate_range("shared.end_handle", shared.end_handle, 0.0, 1.0)?;
        validate_range("shared.arc_size", shared.arc_size, 0.0, 1.0)?;
        validate_range("shared.arc_flow", shared.arc_flow, -1.0, 1.0)?;
        validate_range("shared.spring", shared.spring, 0.3, 1.0)?;
        validate_range(
            "shared.glide_duration_ms",
            shared.glide_duration_ms,
            0.0,
            5000.0,
        )?;
        validate_range(
            "shared.dwell_after_click_ms",
            shared.dwell_after_click_ms,
            0.0,
            5000.0,
        )?;
        validate_range("shared.idle_hide_ms", shared.idle_hide_ms, 0.0, 60_000.0)?;
        validate_range(
            "shared.press_duration_ms",
            shared.press_duration_ms,
            0.0,
            5000.0,
        )?;
        validate_range("shared.peak_speed", shared.peak_speed, 50.0, 5000.0)?;
        validate_range(
            "shared.min_start_speed",
            shared.min_start_speed,
            1.0,
            shared.peak_speed,
        )?;
        validate_range(
            "shared.min_end_speed",
            shared.min_end_speed,
            1.0,
            shared.peak_speed,
        )?;
        validate_range("shared.turn_radius", shared.turn_radius, 1.0, 1000.0)?;
        validate_range(
            "shared.click_offset",
            shared.click_offset,
            f64::EPSILON,
            1000.0,
        )?;

        let macos = &self.spring_settle.macos;
        if macos.mode != SpringSettleMode::Fixed {
            return Err("spring_settle.macos.mode must be `fixed`".into());
        }
        validate_positive("spring_settle.macos.k", macos.k)?;
        validate_non_negative("spring_settle.macos.c", macos.c)?;
        validate_range("spring_settle.macos.overshoot", macos.overshoot, 0.0, 1.0)?;

        let windows_linux = &self.spring_settle.windows_linux;
        if windows_linux.mode != SpringSettleMode::Derived {
            return Err("spring_settle.windows_linux.mode must be `derived`".into());
        }
        validate_positive(
            "spring_settle.windows_linux.k_per_spring",
            windows_linux.k_per_spring,
        )?;
        validate_non_negative(
            "spring_settle.windows_linux.c_per_spring",
            windows_linux.c_per_spring,
        )?;
        validate_range(
            "spring_settle.windows_linux.overshoot",
            windows_linux.overshoot,
            0.0,
            1.0,
        )?;

        Ok(())
    }
}

fn validate_range(name: &str, value: f64, min: f64, max: f64) -> Result<(), String> {
    if !value.is_finite() {
        return Err(format!("{name} must be finite"));
    }
    if !(min..=max).contains(&value) {
        return Err(format!("{name} must be in [{min}, {max}], got {value}"));
    }
    Ok(())
}

fn validate_positive(name: &str, value: f64) -> Result<(), String> {
    if !value.is_finite() || value <= 0.0 {
        return Err(format!("{name} must be finite and positive"));
    }
    Ok(())
}

fn validate_non_negative(name: &str, value: f64) -> Result<(), String> {
    if !value.is_finite() || value < 0.0 {
        return Err(format!("{name} must be finite and non-negative"));
    }
    Ok(())
}

impl MotionSpecShared {
    /// The `MotionConfig` view of the shared tunables (everything except
    /// `click_offset`, which is geometry consumed by the command layer).
    pub fn motion_config(&self) -> MotionConfig {
        MotionConfig {
            start_handle: self.start_handle,
            end_handle: self.end_handle,
            arc_size: self.arc_size,
            arc_flow: self.arc_flow,
            spring: self.spring,
            glide_duration_ms: self.glide_duration_ms,
            dwell_after_click_ms: self.dwell_after_click_ms,
            idle_hide_ms: self.idle_hide_ms,
            press_duration_ms: self.press_duration_ms,
            peak_speed: self.peak_speed,
            min_start_speed: self.min_start_speed,
            min_end_speed: self.min_end_speed,
            turn_radius: self.turn_radius,
        }
    }
}

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
    /// When <= 0 the render engine uses speed-based timing instead. [0, 5000]
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
    /// Minimum turning radius of the Dubins glide path, in points. Smaller =
    /// tighter curves. Matches the Swift reference default of 80.
    pub turn_radius: f64,
}

impl Default for MotionConfig {
    fn default() -> Self {
        motion_spec().shared.motion_config()
    }
}

impl MotionConfig {
    #[allow(clippy::too_many_arguments)]
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
        turn_radius: Option<f64>,
    ) -> Self {
        fn clamp(v: f64, lo: f64, hi: f64) -> f64 {
            v.clamp(lo, hi)
        }
        Self {
            start_handle: clamp(start_handle.unwrap_or(self.start_handle), 0.0, 1.0),
            end_handle: clamp(end_handle.unwrap_or(self.end_handle), 0.0, 1.0),
            arc_size: clamp(arc_size.unwrap_or(self.arc_size), 0.0, 1.0),
            arc_flow: clamp(arc_flow.unwrap_or(self.arc_flow), -1.0, 1.0),
            spring: clamp(spring.unwrap_or(self.spring), 0.3, 1.0),
            glide_duration_ms: clamp(
                glide_duration_ms.unwrap_or(self.glide_duration_ms),
                0.0,
                5000.0,
            ),
            dwell_after_click_ms: clamp(
                dwell_after_click_ms.unwrap_or(self.dwell_after_click_ms),
                0.0,
                5000.0,
            ),
            idle_hide_ms: clamp(idle_hide_ms.unwrap_or(self.idle_hide_ms), 0.0, 60_000.0),
            press_duration_ms: clamp(
                press_duration_ms.unwrap_or(self.press_duration_ms),
                0.0,
                5000.0,
            ),
            peak_speed: self.peak_speed,
            min_start_speed: self.min_start_speed,
            min_end_speed: self.min_end_speed,
            turn_radius: clamp(turn_radius.unwrap_or(self.turn_radius), 1.0, 1000.0),
        }
    }
}

/// Post-arrival spring physics state.
///
/// When the cursor reaches the end of a planned path the engine
/// hands control to a spring-damper that overshoots a touch and
/// settles to the target. This struct holds the spring's mutable
/// state across ticks. Identical across all platform crates — was
/// duplicated 3× before the 2026-05 dedup audit.
///
/// `(ox, oy)` = offset from the spring target; `(vx, vy)` = velocity.
#[derive(Clone, Copy, Default)]
pub struct Spring {
    pub ox: f64,
    pub oy: f64,
    pub vx: f64,
    pub vy: f64,
}

#[cfg(test)]
mod spec_tests {
    use super::*;

    /// Independent literal of the production defaults. This is the second
    /// entry of the double-entry ledger: the checked-in JSON spec and this
    /// Rust literal must agree, so tuning a default is always a conscious
    /// two-file change a reviewer has to approve.
    fn reference_defaults() -> MotionConfig {
        MotionConfig {
            start_handle: 0.3,
            end_handle: 0.3,
            arc_size: 0.25,
            arc_flow: 0.0,
            spring: 0.72,
            glide_duration_ms: 0.0, // 0 = speed-based mode
            dwell_after_click_ms: 80.0,
            idle_hide_ms: 20_000.0,
            press_duration_ms: 120.0,
            peak_speed: 900.0,
            min_start_speed: 300.0,
            min_end_speed: 200.0,
            turn_radius: 80.0,
        }
    }

    #[test]
    fn embedded_spec_reproduces_the_rust_motion_defaults() {
        assert_eq!(MotionConfig::default(), reference_defaults());
        assert_eq!(motion_spec().schema, "cua.cursor-motion/1");
    }

    #[test]
    fn embedded_spec_values_are_within_production_clamp_ranges() {
        let defaults = MotionConfig::default();
        let identity =
            defaults.with_overrides(None, None, None, None, None, None, None, None, None, None);
        assert_eq!(identity, defaults);
        assert_eq!(motion_spec().shared.click_offset, click_offset());
        assert_eq!(click_offset(), 16.0);
    }

    #[test]
    fn spec_rejects_unknown_fields_and_bad_json() {
        assert!(parse_motion_spec("{").is_err());
        assert!(parse_motion_spec(r#"{"schema":"cua.cursor-motion/1"}"#).is_err());
        let raw = MOTION_SPEC_JSON.replace("\"spring\": 0.72", "\"torque\": 9.0");
        assert!(parse_motion_spec(&raw).is_err());
    }

    #[test]
    fn spec_rejects_foreign_schema_and_invalid_modes() {
        let foreign_schema = MOTION_SPEC_JSON.replace(
            "\"schema\": \"cua.cursor-motion/1\"",
            "\"schema\": \"cua.cursor-motion/2\"",
        );
        let error = parse_motion_spec(&foreign_schema).unwrap_err();
        assert!(error.to_string().contains("schema must be"), "{error}");

        let wrong_platform_mode =
            MOTION_SPEC_JSON.replacen("\"mode\": \"fixed\"", "\"mode\": \"derived\"", 1);
        let error = parse_motion_spec(&wrong_platform_mode).unwrap_err();
        assert!(error.to_string().contains("macos.mode"), "{error}");

        let unknown_mode =
            MOTION_SPEC_JSON.replacen("\"mode\": \"fixed\"", "\"mode\": \"wobbly\"", 1);
        let error = parse_motion_spec(&unknown_mode).unwrap_err();
        assert!(error.to_string().contains("unknown variant"), "{error}");
    }

    #[test]
    fn spec_rejects_non_finite_out_of_range_and_inverted_speeds() {
        let mut non_finite = motion_spec().clone();
        non_finite.shared.peak_speed = f64::INFINITY;
        let error = non_finite.validate().unwrap_err();
        assert!(error.contains("finite"), "{error}");

        let too_slow =
            MOTION_SPEC_JSON.replacen("\"peak_speed\": 900.0", "\"peak_speed\": 10.0", 1);
        let error = parse_motion_spec(&too_slow).unwrap_err();
        assert!(error.to_string().contains("peak_speed"), "{error}");

        let floor_above_peak =
            MOTION_SPEC_JSON.replacen("\"min_end_speed\": 200.0", "\"min_end_speed\": 2000.0", 1);
        let error = parse_motion_spec(&floor_above_peak).unwrap_err();
        assert!(error.to_string().contains("min_end_speed"), "{error}");
    }

    #[test]
    #[should_panic(expected = "there is deliberately no fallback")]
    fn invalid_embedded_spec_fails_fast_instead_of_using_shadow_defaults() {
        parse_embedded_motion_spec("{}");
    }

    #[test]
    fn spring_settle_platform_divergence_is_recorded_deliberately() {
        let settle = &motion_spec().spring_settle;
        // macOS runs the fixed Swift reference constants.
        assert_eq!(settle.macos.mode, SpringSettleMode::Fixed);
        assert_eq!(settle.macos.k, 400.0);
        assert_eq!(settle.macos.c, 17.0);
        assert_eq!(settle.macos.overshoot, 0.8);
        // Windows/Linux derive from the runtime spring scalar.
        assert_eq!(settle.windows_linux.mode, SpringSettleMode::Derived);
        assert_eq!(settle.windows_linux.k_per_spring, 400.0);
        assert_eq!(settle.windows_linux.c_per_spring, 20.0);
        assert_eq!(settle.windows_linux.overshoot, 0.5);
        // At the default spring (0.72) the derived constants stay stiffer and
        // less damped than the macOS reference: 288/14.4 vs 400/17.
        let d = &settle.windows_linux;
        let spring = reference_defaults().spring;
        assert!((d.k_per_spring * spring - 288.0).abs() < 1e-9);
        assert!((d.c_per_spring * spring - 14.4).abs() < 1e-9);
    }
}
