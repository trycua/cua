//! Motion / timing configuration.
//! Derived from trope-cua (MIT); see THIRD_PARTY_NOTICES.md.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How the agent cursor travels between targets.
///
/// Each style is a port of a motion-lab candidate
/// (`libs/cua-driver/tools/cursor-gallery/motion-lab/`); the lab id is
/// accepted as an alias on the wire.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MotionStyle {
    /// One confident arc with a small follow-through. The default.
    #[default]
    #[serde(alias = "dc-signature-arc")]
    SignatureArc,
    /// Arc that lands with one soft damped bounce.
    #[serde(alias = "dc-spring-settle")]
    SpringSettle,
    /// Decelerates to a capture radius, then the target pulls it in.
    #[serde(alias = "dc-magnetic")]
    Magnetic,
    /// Wide arc with a short fading trail.
    #[serde(alias = "dc-comet-swoop")]
    CometSwoop,
    /// Precise approach for small targets, swoop for long throws, Fitts
    /// minimum-jerk otherwise.
    #[serde(alias = "adaptive-auto")]
    Adaptive,
    /// The original Dubins glide with an arrival spring.
    #[serde(alias = "dubins-glide", alias = "dubins")]
    Classic,
}

impl MotionStyle {
    pub const ALL: [Self; 6] = [
        Self::SignatureArc,
        Self::SpringSettle,
        Self::Magnetic,
        Self::CometSwoop,
        Self::Adaptive,
        Self::Classic,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SignatureArc => "signature_arc",
            Self::SpringSettle => "spring_settle",
            Self::Magnetic => "magnetic",
            Self::CometSwoop => "comet_swoop",
            Self::Adaptive => "adaptive",
            Self::Classic => "classic",
        }
    }

    /// The motion-lab candidate this style ports.
    pub const fn lab_id(self) -> &'static str {
        match self {
            Self::SignatureArc => "dc-signature-arc",
            Self::SpringSettle => "dc-spring-settle",
            Self::Magnetic => "dc-magnetic",
            Self::CometSwoop => "dc-comet-swoop",
            Self::Adaptive => "adaptive-auto",
            Self::Classic => "dubins-glide",
        }
    }

    /// Parse a public name or a lab id.
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|style| style.as_str() == value || style.lab_id() == value)
            .or_else(|| (value == "dubins").then_some(Self::Classic))
    }

    /// Effects a style turns on when the caller leaves them unset.
    pub const fn default_effects(self) -> ResolvedEffects {
        let none = ResolvedEffects {
            trail: false,
            glow: false,
            magnet: false,
            ripple: false,
            squish: false,
        };
        match self {
            Self::SignatureArc => ResolvedEffects {
                glow: true,
                ripple: true,
                squish: true,
                ..none
            },
            Self::SpringSettle => ResolvedEffects {
                glow: true,
                squish: true,
                ..none
            },
            Self::Magnetic => ResolvedEffects {
                magnet: true,
                ripple: true,
                ..none
            },
            Self::CometSwoop => ResolvedEffects {
                trail: true,
                ripple: true,
                ..none
            },
            Self::Adaptive => ResolvedEffects {
                squish: true,
                ..none
            },
            Self::Classic => none,
        }
    }
}

/// How long each move takes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MotionTiming {
    /// The style's own timing (distance-aware for every style except
    /// `classic`, which is speed-based).
    #[default]
    Native,
    /// Fitts' law: `150 + 120 log2(D / W + 1)` ms, clamped to 300..1000,
    /// where `W` is the target's smaller side.
    Fitts,
    /// Every move takes `glide_duration_ms` (1430 ms when that is 0).
    Fixed,
}

impl MotionTiming {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Fitts => "fitts",
            Self::Fixed => "fixed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        [Self::Native, Self::Fitts, Self::Fixed]
            .into_iter()
            .find(|timing| timing.as_str() == value)
    }
}

/// Per-effect overrides. `None` keeps the style's default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MotionEffects {
    /// Short fading trail behind the cursor.
    pub trail: Option<bool>,
    /// Soft glow that trails the cursor and grows with speed.
    pub glow: Option<bool>,
    /// Target glow when the magnetic style locks on.
    pub magnet: Option<bool>,
    /// Ring that expands from the hotspot on click.
    pub ripple: Option<bool>,
    /// Brief scale-down of the cursor on click.
    pub squish: Option<bool>,
}

/// Effects after applying overrides to a style's defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedEffects {
    pub trail: bool,
    pub glow: bool,
    pub magnet: bool,
    pub ripple: bool,
    pub squish: bool,
}

impl ResolvedEffects {
    pub const NONE: Self = Self {
        trail: false,
        glow: false,
        magnet: false,
        ripple: false,
        squish: false,
    };
}

/// Fixed-timing duration when `glide_duration_ms` is 0: the move time of
/// the Codex-style spring cursor, which ignores distance.
pub const DEFAULT_FIXED_MS: f64 = 1430.0;

/// Runtime-tunable timing and path-shape parameters.
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
    /// Minimum turning radius of the Dubins glide path, in points. Smaller =
    /// tighter curves. Matches the Swift reference default of 80.
    pub turn_radius: f64,
    /// Trajectory style.
    pub style: MotionStyle,
    /// Move duration model.
    pub timing: MotionTiming,
    /// Effect overrides on top of the style's defaults.
    pub effects: MotionEffects,
}

impl Default for MotionConfig {
    fn default() -> Self {
        Self {
            start_handle: 0.3,
            end_handle: 0.3,
            arc_size: 0.25,
            arc_flow: 0.0,
            spring: 0.72,
            glide_duration_ms: 0.0, // 0 = speed-based mode
            dwell_after_click_ms: 80.0,
            // The shared agent idle timeout (presence drops an idle agent then too).
            idle_hide_ms: cua_driver_core::agent_cursor::default_idle_hide_ms(),
            press_duration_ms: 120.0,
            peak_speed: 900.0,
            min_start_speed: 300.0,
            min_end_speed: 200.0,
            turn_radius: 80.0,
            style: MotionStyle::default(),
            timing: MotionTiming::default(),
            effects: MotionEffects::default(),
        }
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
            style: self.style,
            timing: self.timing,
            effects: self.effects,
        }
    }

    /// Apply the `style`, `timing` and `effects` fields of a
    /// `set_agent_cursor_motion` call. Absent or null fields keep their
    /// current value; inside `effects`, `null` restores the style default.
    /// Apply the motion fields of `set_agent_cursor_motion` (also the
    /// `cursor_motion` of `start_session`) on top of `self`. Omitted or null
    /// fields keep their current value; an invalid style, timing or effect is
    /// an error that names the allowed values.
    pub fn with_motion_args(&self, args: &Value) -> Result<Self, String> {
        let number = |name: &str| args.get(name).and_then(Value::as_f64);
        self.with_overrides(
            number("start_handle"),
            number("end_handle"),
            number("arc_size"),
            number("arc_flow"),
            number("spring"),
            number("glide_duration_ms"),
            number("dwell_after_click_ms"),
            number("idle_hide_ms"),
            None,
            number("turn_radius"),
        )
        .with_style_args(args)
    }

    pub fn with_style_args(&self, args: &Value) -> Result<Self, String> {
        let mut out = self.clone();
        match args.get("style") {
            None | Some(Value::Null) => {}
            Some(Value::String(name)) => {
                out.style = MotionStyle::parse(name).ok_or_else(|| {
                    format!(
                        "unknown cursor motion style `{name}`; expected one of {}",
                        MotionStyle::ALL.map(MotionStyle::as_str).join(", ")
                    )
                })?;
            }
            Some(other) => return Err(format!("style must be a string, got {other}")),
        }
        match args.get("timing") {
            None | Some(Value::Null) => {}
            Some(Value::String(name)) => {
                out.timing = MotionTiming::parse(name).ok_or_else(|| {
                    format!(
                        "unknown cursor motion timing `{name}`; expected native, fitts or fixed"
                    )
                })?;
            }
            Some(other) => return Err(format!("timing must be a string, got {other}")),
        }
        match args.get("effects") {
            None | Some(Value::Null) => {}
            Some(Value::Object(map)) => {
                for (key, value) in map {
                    let flag = match value {
                        Value::Null => None,
                        Value::Bool(flag) => Some(*flag),
                        other => {
                            return Err(format!("effects.{key} must be a boolean, got {other}"))
                        }
                    };
                    match key.as_str() {
                        "trail" => out.effects.trail = flag,
                        "glow" => out.effects.glow = flag,
                        "magnet" => out.effects.magnet = flag,
                        "ripple" => out.effects.ripple = flag,
                        "squish" => out.effects.squish = flag,
                        other => {
                            return Err(format!(
                                "unknown cursor effect `{other}`; expected trail, glow, magnet, ripple or squish"
                            ))
                        }
                    }
                }
            }
            Some(other) => return Err(format!("effects must be an object, got {other}")),
        }
        Ok(out)
    }

    /// The style's effects after overrides.
    pub fn resolved_effects(&self) -> ResolvedEffects {
        let base = self.style.default_effects();
        ResolvedEffects {
            trail: self.effects.trail.unwrap_or(base.trail),
            glow: self.effects.glow.unwrap_or(base.glow),
            magnet: self.effects.magnet.unwrap_or(base.magnet),
            ripple: self.effects.ripple.unwrap_or(base.ripple),
            squish: self.effects.squish.unwrap_or(base.squish),
        }
    }

    /// The `motion` object echoed by `set_agent_cursor_motion` and
    /// `get_agent_cursor_state` on every platform.
    pub fn output_json(&self) -> Value {
        let effects = self.resolved_effects();
        serde_json::json!({
            "start_handle": self.start_handle,
            "end_handle": self.end_handle,
            "arc_size": self.arc_size,
            "arc_flow": self.arc_flow,
            "spring": self.spring,
            "glide_duration_ms": self.glide_duration_ms,
            "dwell_after_click_ms": self.dwell_after_click_ms,
            "idle_hide_ms": self.idle_hide_ms,
            "turn_radius": self.turn_radius,
            "style": self.style.as_str(),
            "timing": self.timing.as_str(),
            "effects": {
                "trail": effects.trail,
                "glow": effects.glow,
                "magnet": effects.magnet,
                "ripple": effects.ripple,
                "squish": effects.squish
            }
        })
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
