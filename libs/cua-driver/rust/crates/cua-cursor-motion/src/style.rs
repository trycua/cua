//! The built-in styles, timing modes and effect switches.

/// How the agent cursor travels between targets.
///
/// Each style is a port of a motion-lab candidate
/// (`libs/cua-driver/tools/cursor-gallery/motion-lab/`); the lab id is
/// accepted as an alias.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum MotionStyle {
    /// One confident arc with a small follow-through. The default.
    #[default]
    #[cfg_attr(feature = "serde", serde(alias = "dc-signature-arc"))]
    SignatureArc,
    /// Arc that lands with one soft damped bounce.
    #[cfg_attr(feature = "serde", serde(alias = "dc-spring-settle"))]
    SpringSettle,
    /// Decelerates to a capture radius, then the target pulls it in.
    #[cfg_attr(feature = "serde", serde(alias = "dc-magnetic"))]
    Magnetic,
    /// Wide arc with a short fading trail.
    #[cfg_attr(feature = "serde", serde(alias = "dc-comet-swoop"))]
    CometSwoop,
    /// Precise approach for small targets, swoop for long throws, Fitts
    /// minimum-jerk otherwise.
    #[cfg_attr(feature = "serde", serde(alias = "adaptive-auto"))]
    Adaptive,
    /// The original Dubins glide with an arrival spring.
    #[cfg_attr(feature = "serde", serde(alias = "dubins-glide", alias = "dubins"))]
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
        let none = ResolvedEffects::NONE;
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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum MotionTiming {
    /// The style's own timing (distance-aware for every style except
    /// `classic`, which is speed-based).
    Native,
    /// Fitts' law: `150 + 120 log2(D / W + 1)` ms, clamped to 300..1000,
    /// where `W` is the target's smaller side.
    Fitts,
    /// Every move takes `glide_duration_ms` (1430 ms when that is 0).
    Fixed,
    /// The style's own timing, but no move is shorter than 450 ms, with a
    /// gentler sine ease. Slows short hops; long moves keep their time.
    Relaxed,
    /// Like `relaxed` with a 600 ms floor. The default.
    #[default]
    Steady,
    /// Like `relaxed` with a 750 ms floor.
    Calm,
    /// Fitts' law with a larger constant: `400 + 120 log2(D / W + 1)` ms,
    /// clamped to 450..1300. Every distance is slower than `fitts`.
    Deliberate,
}

impl MotionTiming {
    pub const ALL: [Self; 7] = [
        Self::Native,
        Self::Fitts,
        Self::Fixed,
        Self::Relaxed,
        Self::Steady,
        Self::Calm,
        Self::Deliberate,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Fitts => "fitts",
            Self::Fixed => "fixed",
            Self::Relaxed => "relaxed",
            Self::Steady => "steady",
            Self::Calm => "calm",
            Self::Deliberate => "deliberate",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|timing| timing.as_str() == value)
    }

    /// Minimum move time of the floor modes (`relaxed`, `steady`, `calm`).
    pub const fn floor_ms(self) -> Option<f64> {
        match self {
            Self::Relaxed => Some(450.0),
            Self::Steady => Some(600.0),
            Self::Calm => Some(750.0),
            _ => None,
        }
    }

    /// The names, comma-separated, for error messages.
    pub fn names() -> String {
        Self::ALL.map(Self::as_str).join(", ")
    }
}

/// Per-effect overrides. `None` keeps the style's default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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

impl MotionEffects {
    /// Apply these overrides to `base`.
    pub fn resolve(self, base: ResolvedEffects) -> ResolvedEffects {
        ResolvedEffects {
            trail: self.trail.unwrap_or(base.trail),
            glow: self.glow.unwrap_or(base.glow),
            magnet: self.magnet.unwrap_or(base.magnet),
            ripple: self.ripple.unwrap_or(base.ripple),
            squish: self.squish.unwrap_or(base.squish),
        }
    }
}

/// Effects after applying overrides to a style's defaults.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
