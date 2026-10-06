//! Custom motions: describe a path shape, a speed curve, an overshoot or
//! settle, a duration model and a trail, and get the same kind of
//! trajectory the built-in styles produce. `signature_arc`, `spring_settle`
//! and `comet_swoop` are themselves specs (see [`MotionSpec::for_style`]).

use crate::ease::Ease;
use crate::effects::TrailSpec;
use crate::params::MotionParams;
use crate::path::ArcShape;
use crate::style::{MotionStyle, ResolvedEffects};

/// Which way the path bends.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case", tag = "type"))]
pub enum PathShape {
    Straight,
    /// The Cua cubic bezier. A positive `arc_size` bends to the natural side
    /// (upward for horizontal moves); a negative one bends the other way.
    Arc(ArcShape),
    /// A gentle quadratic bow; `amount` is the apex offset as a fraction of
    /// the distance, on the natural side when positive.
    Bow {
        amount: f64,
    },
}

/// What happens at the end of the glide.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case", tag = "type"))]
pub enum Settle {
    /// Stop exactly on the target.
    None,
    /// Push past the target and come back, peaking at `at` (0..1 of the
    /// move). The overshoot is `amount` of the distance, at most `max_pt`.
    FollowThrough { amount: f64, max_pt: f64, at: f64 },
    /// Reach the target by `glide_end` (0..1 of the move), then wobble
    /// around it from `start` on: damped (`decay`) for `cycles` cycles, with
    /// amplitude `amount` of the distance, at most `max_pt`.
    Spring {
        amount: f64,
        max_pt: f64,
        cycles: f64,
        decay: f64,
        start: f64,
        glide_end: f64,
    },
}

/// How long the move takes before any global timing mode applies.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case", tag = "type"))]
pub enum Duration {
    /// Always `ms`.
    Fixed { ms: f64 },
    /// Fitts' law: `(a + b log2(D / W + 1)).clamp(min_ms, max_ms) * scale`,
    /// where `D` is the distance and `W` the target's smaller side (at least
    /// 4 pt).
    Fitts {
        a: f64,
        b: f64,
        min_ms: f64,
        max_ms: f64,
        scale: f64,
    },
    /// `(base_ms + ms_per_pt * D).clamp(min_ms, max_ms)`.
    Distance {
        base_ms: f64,
        ms_per_pt: f64,
        min_ms: f64,
        max_ms: f64,
    },
}

impl Duration {
    /// The Fitts model the director's-cut styles share.
    pub const fn fitts(scale: f64) -> Self {
        Self::Fitts {
            a: 150.0,
            b: 120.0,
            min_ms: 300.0,
            max_ms: 1000.0,
            scale,
        }
    }

    /// Milliseconds for a move of `d` points to a target `w` points wide.
    pub fn ms(self, d: f64, w: f64) -> f64 {
        match self {
            Self::Fixed { ms } => ms,
            Self::Fitts {
                a,
                b,
                min_ms,
                max_ms,
                scale,
            } => (a + b * (d / w.max(4.0) + 1.0).log2()).clamp(min_ms, max_ms) * scale,
            Self::Distance {
                base_ms,
                ms_per_pt,
                min_ms,
                max_ms,
            } => (base_ms + ms_per_pt * d).clamp(min_ms, max_ms),
        }
    }
}

/// Which way the arrow points while it moves.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum Heading {
    /// The tip leads along the direction of travel once it is moving.
    #[default]
    Tangent,
    /// The arrow keeps its rest pose.
    Fixed,
}

/// A complete custom motion.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MotionSpec {
    pub path: PathShape,
    /// Speed curve along the path.
    pub ease: Ease,
    pub settle: Settle,
    pub duration: Duration,
    pub heading: Heading,
    pub effects: ResolvedEffects,
    pub trail: TrailSpec,
}

impl Default for MotionSpec {
    fn default() -> Self {
        Self::for_style(MotionStyle::SignatureArc, &MotionParams::default())
            .expect("signature_arc is a spec")
    }
}

/// Scale a style's arc by the caller's `arc_size` (0.25, the default, keeps
/// the style's own arc; 0 is straight) and shift its flow by `arc_flow`.
pub(crate) fn knob_shape(params: &MotionParams, arc_size: f64, arc_flow: f64) -> ArcShape {
    let default = MotionParams::default();
    ArcShape {
        start_handle: params.start_handle,
        end_handle: params.end_handle,
        arc_size: arc_size * (params.arc_size / default.arc_size),
        arc_flow: (arc_flow + params.arc_flow).clamp(-1.0, 1.0),
    }
}

impl MotionSpec {
    /// The spec behind a built-in style, with `params` applied. `magnetic`,
    /// `adaptive` and `classic` are simulations rather than specs and return
    /// `None`.
    pub fn for_style(style: MotionStyle, params: &MotionParams) -> Option<Self> {
        let effects = params.resolved_effects();
        let base = |path, ease, settle, duration| Self {
            path,
            ease,
            settle,
            duration,
            heading: Heading::Tangent,
            effects,
            trail: TrailSpec::default(),
        };
        match style {
            // Cua arc 0.16, min-jerk with a 1.8% follow-through capped at
            // 8 pt (`dc-signature-arc`).
            MotionStyle::SignatureArc => Some(base(
                PathShape::Arc(knob_shape(params, 0.16, 0.15)),
                Ease::MinJerk,
                Settle::FollowThrough {
                    amount: 0.018,
                    max_pt: 8.0,
                    at: 0.82,
                },
                Duration::fitts(1.1),
            )),
            // Arc 0.12, min-jerk into one damped 6 pt overshoot
            // (`dc-spring-settle`).
            MotionStyle::SpringSettle => Some(base(
                PathShape::Arc(knob_shape(params, 0.12, 0.0)),
                Ease::MinJerk,
                Settle::Spring {
                    amount: 0.05,
                    max_pt: 6.0,
                    cycles: 1.3,
                    decay: 2.6,
                    start: 0.55,
                    glide_end: 0.68,
                },
                Duration::fitts(1.35),
            )),
            // Wide arc 0.24, flow 0.2, easeInOutCubic (`dc-comet-swoop`).
            MotionStyle::CometSwoop => Some(base(
                PathShape::Arc(knob_shape(params, 0.24, 0.2)),
                Ease::InOutCubic,
                Settle::None,
                Duration::fitts(1.15),
            )),
            MotionStyle::Magnetic | MotionStyle::Adaptive | MotionStyle::Classic => None,
        }
    }
}
