//! Cua Cursor Motion through the UniFFI boundary.
//!
//! The planner lives in the dependency-free `cua-cursor-motion` crate; this
//! module mirrors its public API as UniFFI records, enums and one object, so
//! every generated SDK (Python, TypeScript, the C header) can plan and play
//! the agent cursor's motions. A planned move is a `CursorTrajectory`: read
//! all samples once with `samples()` rather than calling across the FFI every
//! frame.

use cua_cursor_motion as cm;
use cua_driver_contract::{
    CursorMotionEffects, CursorMotionEffectsOutput, CursorMotionStyle, CursorMotionTiming,
};
use std::sync::Arc;

fn style_in(style: CursorMotionStyle) -> cm::MotionStyle {
    cm::MotionStyle::parse(style.as_str()).expect("contract styles match cua-cursor-motion")
}

fn style_out(style: cm::MotionStyle) -> CursorMotionStyle {
    CursorMotionStyle::ALL
        .into_iter()
        .find(|s| s.as_str() == style.as_str())
        .expect("contract styles match cua-cursor-motion")
}

fn timing_in(timing: CursorMotionTiming) -> cm::MotionTiming {
    match timing {
        CursorMotionTiming::Native => cm::MotionTiming::Native,
        CursorMotionTiming::Fitts => cm::MotionTiming::Fitts,
        CursorMotionTiming::Fixed => cm::MotionTiming::Fixed,
    }
}

fn overrides_in(e: CursorMotionEffects) -> cm::MotionEffects {
    cm::MotionEffects {
        trail: e.trail,
        glow: e.glow,
        magnet: e.magnet,
        ripple: e.ripple,
        squish: e.squish,
    }
}

fn effects_in(e: CursorMotionEffectsOutput) -> cm::ResolvedEffects {
    cm::ResolvedEffects {
        trail: e.trail,
        glow: e.glow,
        magnet: e.magnet,
        ripple: e.ripple,
        squish: e.squish,
    }
}

fn effects_out(e: cm::ResolvedEffects) -> CursorMotionEffectsOutput {
    CursorMotionEffectsOutput {
        trail: e.trail,
        glow: e.glow,
        magnet: e.magnet,
        ripple: e.ripple,
        squish: e.squish,
    }
}

/// Knobs of the built-in styles. Defaults match Cua Driver
/// (`default_cursor_motion_params`).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct CursorMotionParams {
    pub style: CursorMotionStyle,
    pub timing: CursorMotionTiming,
    pub effects: CursorMotionEffects,
    pub start_handle: f64,
    pub end_handle: f64,
    pub arc_size: f64,
    pub arc_flow: f64,
    pub spring: f64,
    pub glide_duration_ms: f64,
    pub peak_speed: f64,
    pub min_start_speed: f64,
    pub min_end_speed: f64,
    pub turn_radius: f64,
}

impl From<CursorMotionParams> for cm::MotionParams {
    fn from(p: CursorMotionParams) -> Self {
        Self {
            style: style_in(p.style),
            timing: timing_in(p.timing),
            effects: overrides_in(p.effects),
            start_handle: p.start_handle,
            end_handle: p.end_handle,
            arc_size: p.arc_size,
            arc_flow: p.arc_flow,
            spring: p.spring,
            glide_duration_ms: p.glide_duration_ms,
            peak_speed: p.peak_speed,
            min_start_speed: p.min_start_speed,
            min_end_speed: p.min_end_speed,
            turn_radius: p.turn_radius,
        }
    }
}

/// Cua Driver's default motion knobs.
#[uniffi::export]
pub fn default_cursor_motion_params() -> CursorMotionParams {
    let d = cm::MotionParams::default();
    CursorMotionParams {
        style: style_out(d.style),
        timing: CursorMotionTiming::Native,
        effects: CursorMotionEffects::default(),
        start_handle: d.start_handle,
        end_handle: d.end_handle,
        arc_size: d.arc_size,
        arc_flow: d.arc_flow,
        spring: d.spring,
        glide_duration_ms: d.glide_duration_ms,
        peak_speed: d.peak_speed,
        min_start_speed: d.min_start_speed,
        min_end_speed: d.min_end_speed,
        turn_radius: d.turn_radius,
    }
}

/// A point in screen points (y down).
#[derive(Debug, Clone, Copy, PartialEq, uniffi::Record)]
pub struct CursorMotionPoint {
    pub x: f64,
    pub y: f64,
}

/// A target rect in screen points.
#[derive(Debug, Clone, Copy, PartialEq, uniffi::Record)]
pub struct CursorMotionRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// One move to plan.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct CursorMoveRequest {
    /// Current hotspot.
    pub from_point: CursorMotionPoint,
    /// Requested hotspot.
    pub to_point: CursorMotionPoint,
    /// Current heading; the rest heading (pi/4) when unset.
    pub from_heading: Option<f64>,
    /// Heading on arrival; the rest heading when unset.
    pub end_heading: Option<f64>,
    /// The target element's rect; it sets the Fitts width.
    pub target: Option<CursorMotionRect>,
    /// Same seed, same motion.
    pub seed: String,
    /// A short straight glide without effects.
    pub reduced_motion: bool,
}

impl From<CursorMoveRequest> for cm::MoveRequest {
    fn from(r: CursorMoveRequest) -> Self {
        Self {
            from: cm::Pt::new(r.from_point.x, r.from_point.y),
            from_heading: r.from_heading.unwrap_or(cm::REST_HEADING),
            to: cm::Pt::new(r.to_point.x, r.to_point.y),
            end_heading: r.end_heading.unwrap_or(cm::REST_HEADING),
            target: r.target.map(|t| [t.x, t.y, t.width, t.height]),
            seed: r.seed,
            reduced_motion: r.reduced_motion,
        }
    }
}

/// Path shape of a custom motion.
#[derive(Debug, Clone, Copy, PartialEq, uniffi::Enum)]
pub enum CursorPathShape {
    Straight,
    /// The Cua cubic bezier; a positive `arc_size` bends to the natural side.
    Arc {
        start_handle: f64,
        end_handle: f64,
        arc_size: f64,
        arc_flow: f64,
    },
    /// A gentle quadratic bow.
    Bow {
        amount: f64,
    },
}

/// Speed curve along the path.
#[derive(Debug, Clone, Copy, PartialEq, uniffi::Enum)]
pub enum CursorEase {
    Linear,
    MinJerk,
    Smootherstep,
    InOutCubic,
    InOutSine,
    OutCubic,
    CubicBezier { x1: f64, y1: f64, x2: f64, y2: f64 },
}

/// Overshoot or settle at the end of the glide.
#[derive(Debug, Clone, Copy, PartialEq, uniffi::Enum)]
pub enum CursorSettle {
    None,
    FollowThrough {
        amount: f64,
        max_pt: f64,
        at: f64,
    },
    Spring {
        amount: f64,
        max_pt: f64,
        cycles: f64,
        decay: f64,
        start: f64,
        glide_end: f64,
    },
}

/// Duration model of a custom motion.
#[derive(Debug, Clone, Copy, PartialEq, uniffi::Enum)]
pub enum CursorMotionDuration {
    Fixed {
        ms: f64,
    },
    Fitts {
        a: f64,
        b: f64,
        min_ms: f64,
        max_ms: f64,
        scale: f64,
    },
    Distance {
        base_ms: f64,
        ms_per_pt: f64,
        min_ms: f64,
        max_ms: f64,
    },
}

/// `Tangent`: the tip leads the motion. `Fixed`: the rest pose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum CursorHeading {
    Tangent,
    Fixed,
}

/// The comet trail.
#[derive(Debug, Clone, Copy, PartialEq, uniffi::Record)]
pub struct CursorTrailSpec {
    pub secs: f64,
    pub fade_len: f64,
    pub steps: u32,
    pub tail_width: f64,
    pub head_width: f64,
    pub alpha: f64,
    pub anchor_offset: f64,
}

/// A complete custom motion.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct CursorMotionSpec {
    pub path: CursorPathShape,
    pub ease: CursorEase,
    pub settle: CursorSettle,
    pub duration: CursorMotionDuration,
    pub heading: CursorHeading,
    pub effects: CursorMotionEffectsOutput,
    pub trail: CursorTrailSpec,
}

impl From<CursorMotionSpec> for cm::MotionSpec {
    fn from(s: CursorMotionSpec) -> Self {
        Self {
            path: match s.path {
                CursorPathShape::Straight => cm::PathShape::Straight,
                CursorPathShape::Arc {
                    start_handle,
                    end_handle,
                    arc_size,
                    arc_flow,
                } => cm::PathShape::Arc(cm::ArcShape {
                    start_handle,
                    end_handle,
                    arc_size,
                    arc_flow,
                }),
                CursorPathShape::Bow { amount } => cm::PathShape::Bow { amount },
            },
            ease: match s.ease {
                CursorEase::Linear => cm::Ease::Linear,
                CursorEase::MinJerk => cm::Ease::MinJerk,
                CursorEase::Smootherstep => cm::Ease::Smootherstep,
                CursorEase::InOutCubic => cm::Ease::InOutCubic,
                CursorEase::InOutSine => cm::Ease::InOutSine,
                CursorEase::OutCubic => cm::Ease::OutCubic,
                CursorEase::CubicBezier { x1, y1, x2, y2 } => {
                    cm::Ease::CubicBezier { x1, y1, x2, y2 }
                }
            },
            settle: match s.settle {
                CursorSettle::None => cm::Settle::None,
                CursorSettle::FollowThrough { amount, max_pt, at } => {
                    cm::Settle::FollowThrough { amount, max_pt, at }
                }
                CursorSettle::Spring {
                    amount,
                    max_pt,
                    cycles,
                    decay,
                    start,
                    glide_end,
                } => cm::Settle::Spring {
                    amount,
                    max_pt,
                    cycles,
                    decay,
                    start,
                    glide_end,
                },
            },
            duration: match s.duration {
                CursorMotionDuration::Fixed { ms } => cm::Duration::Fixed { ms },
                CursorMotionDuration::Fitts {
                    a,
                    b,
                    min_ms,
                    max_ms,
                    scale,
                } => cm::Duration::Fitts {
                    a,
                    b,
                    min_ms,
                    max_ms,
                    scale,
                },
                CursorMotionDuration::Distance {
                    base_ms,
                    ms_per_pt,
                    min_ms,
                    max_ms,
                } => cm::Duration::Distance {
                    base_ms,
                    ms_per_pt,
                    min_ms,
                    max_ms,
                },
            },
            heading: match s.heading {
                CursorHeading::Tangent => cm::Heading::Tangent,
                CursorHeading::Fixed => cm::Heading::Fixed,
            },
            effects: effects_in(s.effects),
            trail: trail_in(s.trail),
        }
    }
}

fn trail_in(t: CursorTrailSpec) -> cm::TrailSpec {
    cm::TrailSpec {
        secs: t.secs,
        fade_len: t.fade_len,
        steps: t.steps as usize,
        tail_width: t.tail_width,
        head_width: t.head_width,
        alpha: t.alpha,
        anchor_offset: t.anchor_offset,
    }
}

fn trail_out(t: cm::TrailSpec) -> CursorTrailSpec {
    CursorTrailSpec {
        secs: t.secs,
        fade_len: t.fade_len,
        steps: u32::try_from(t.steps).unwrap_or(u32::MAX),
        tail_width: t.tail_width,
        head_width: t.head_width,
        alpha: t.alpha,
        anchor_offset: t.anchor_offset,
    }
}

impl From<cm::MotionSpec> for CursorMotionSpec {
    fn from(s: cm::MotionSpec) -> Self {
        Self {
            path: match s.path {
                cm::PathShape::Straight => CursorPathShape::Straight,
                cm::PathShape::Arc(a) => CursorPathShape::Arc {
                    start_handle: a.start_handle,
                    end_handle: a.end_handle,
                    arc_size: a.arc_size,
                    arc_flow: a.arc_flow,
                },
                cm::PathShape::Bow { amount } => CursorPathShape::Bow { amount },
            },
            ease: match s.ease {
                cm::Ease::Linear => CursorEase::Linear,
                cm::Ease::MinJerk => CursorEase::MinJerk,
                cm::Ease::Smootherstep => CursorEase::Smootherstep,
                cm::Ease::InOutCubic => CursorEase::InOutCubic,
                cm::Ease::InOutSine => CursorEase::InOutSine,
                cm::Ease::OutCubic => CursorEase::OutCubic,
                cm::Ease::CubicBezier { x1, y1, x2, y2 } => {
                    CursorEase::CubicBezier { x1, y1, x2, y2 }
                }
            },
            settle: match s.settle {
                cm::Settle::None => CursorSettle::None,
                cm::Settle::FollowThrough { amount, max_pt, at } => {
                    CursorSettle::FollowThrough { amount, max_pt, at }
                }
                cm::Settle::Spring {
                    amount,
                    max_pt,
                    cycles,
                    decay,
                    start,
                    glide_end,
                } => CursorSettle::Spring {
                    amount,
                    max_pt,
                    cycles,
                    decay,
                    start,
                    glide_end,
                },
            },
            duration: match s.duration {
                cm::Duration::Fixed { ms } => CursorMotionDuration::Fixed { ms },
                cm::Duration::Fitts {
                    a,
                    b,
                    min_ms,
                    max_ms,
                    scale,
                } => CursorMotionDuration::Fitts {
                    a,
                    b,
                    min_ms,
                    max_ms,
                    scale,
                },
                cm::Duration::Distance {
                    base_ms,
                    ms_per_pt,
                    min_ms,
                    max_ms,
                } => CursorMotionDuration::Distance {
                    base_ms,
                    ms_per_pt,
                    min_ms,
                    max_ms,
                },
            },
            heading: match s.heading {
                cm::Heading::Tangent => CursorHeading::Tangent,
                cm::Heading::Fixed => CursorHeading::Fixed,
            },
            effects: effects_out(s.effects),
            trail: trail_out(s.trail),
        }
    }
}

/// The spec behind a built-in style (`signature_arc`, `spring_settle` and
/// `comet_swoop`); `None` for the simulated styles.
#[uniffi::export]
pub fn cursor_motion_spec_for_style(
    style: CursorMotionStyle,
    params: CursorMotionParams,
) -> Option<CursorMotionSpec> {
    cm::MotionSpec::for_style(style_in(style), &params.into()).map(Into::into)
}

/// One sample: seconds from the start, the hotspot, and the arrow's heading.
#[derive(Debug, Clone, Copy, PartialEq, uniffi::Record)]
pub struct CursorMotionSample {
    pub t: f64,
    pub x: f64,
    pub y: f64,
    pub heading: f64,
}

impl From<cm::Sample> for CursorMotionSample {
    fn from(s: cm::Sample) -> Self {
        Self {
            t: s.t,
            x: s.x,
            y: s.y,
            heading: s.heading,
        }
    }
}

/// Speed glow.
#[derive(Debug, Clone, Copy, PartialEq, uniffi::Record)]
pub struct CursorGlow {
    pub x: f64,
    pub y: f64,
    pub r: f64,
    pub alpha: f64,
}

/// One round-capped stroke of the comet trail.
#[derive(Debug, Clone, Copy, PartialEq, uniffi::Record)]
pub struct CursorTrailSegment {
    pub ax: f64,
    pub ay: f64,
    pub bx: f64,
    pub by: f64,
    pub width: f64,
    pub alpha: f64,
}

/// Glow around the target after a magnetic lock-on.
#[derive(Debug, Clone, Copy, PartialEq, uniffi::Record)]
pub struct CursorMagnet {
    pub rect: CursorMotionRect,
    pub glow: f64,
}

/// Expanding click ring.
#[derive(Debug, Clone, Copy, PartialEq, uniffi::Record)]
pub struct CursorRipple {
    pub x: f64,
    pub y: f64,
    pub r: f64,
    pub width: f64,
    pub alpha: f64,
}

/// The effects to paint at one moment.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct CursorEffectFrame {
    pub glow: Option<CursorGlow>,
    pub trail: Vec<CursorTrailSegment>,
    pub magnet: Option<CursorMagnet>,
    pub ripple: Option<CursorRipple>,
    /// Scale-down of the cursor, 0 = none.
    pub squish: f64,
}

impl From<cm::EffectFrame> for CursorEffectFrame {
    fn from(f: cm::EffectFrame) -> Self {
        Self {
            glow: f.glow.map(|g| CursorGlow {
                x: g.x,
                y: g.y,
                r: g.r,
                alpha: g.alpha,
            }),
            trail: f
                .trail
                .into_iter()
                .map(|s| CursorTrailSegment {
                    ax: s.a.0,
                    ay: s.a.1,
                    bx: s.b.0,
                    by: s.b.1,
                    width: s.width,
                    alpha: s.alpha,
                })
                .collect(),
            magnet: f.magnet.map(|m| CursorMagnet {
                rect: rect_out(m.rect),
                glow: m.glow,
            }),
            ripple: f.ripple.map(|r| CursorRipple {
                x: r.x,
                y: r.y,
                r: r.r,
                width: r.width,
                alpha: r.alpha,
            }),
            squish: f.squish,
        }
    }
}

fn rect_out(r: [f64; 4]) -> CursorMotionRect {
    CursorMotionRect {
        x: r[0],
        y: r[1],
        width: r[2],
        height: r[3],
    }
}

/// A planned move, played back by time.
#[derive(Debug, uniffi::Object)]
pub struct CursorTrajectory {
    inner: cm::Trajectory,
}

#[uniffi::export]
impl CursorTrajectory {
    /// Every sample at 120 Hz. Read once; interpolate locally per frame.
    pub fn samples(&self) -> Vec<CursorMotionSample> {
        self.inner.samples.iter().copied().map(Into::into).collect()
    }

    /// Interpolated sample at `t` seconds, clamped to the ends.
    pub fn sample_at(&self, t: f64) -> CursorMotionSample {
        self.inner.sample_at(t).into()
    }

    /// When the tip first reaches the target, seconds.
    pub fn arrival_t(&self) -> f64 {
        self.inner.arrival_t
    }

    /// Seconds until the last sample (any settle included).
    pub fn duration(&self) -> f64 {
        self.inner.duration()
    }

    /// Magnetic lock-on time, seconds.
    pub fn snap_t(&self) -> Option<f64> {
        self.inner.snap_t
    }

    /// The target used for timing (the request's, or a 24 pt box).
    pub fn target(&self) -> CursorMotionRect {
        rect_out(self.inner.target)
    }

    pub fn target_known(&self) -> bool {
        self.inner.target_known
    }

    pub fn effects(&self) -> CursorMotionEffectsOutput {
        effects_out(self.inner.effects)
    }

    /// When the move and its trail or magnet glow have finished, seconds.
    pub fn linger(&self) -> f64 {
        cm::effects::linger(&self.inner)
    }

    /// The move's effects (glow, trail, magnet) at `t`, around the arrow's
    /// body. `blend` is whether the surface can draw translucency.
    pub fn effect_frame(&self, t: f64, blend: bool) -> CursorEffectFrame {
        let s = self.inner.sample_at(t);
        let body = cm::anchor_for_pointer(s.x, s.y, s.heading);
        cm::effects::motion_frame(&self.inner, t, body, blend).into()
    }
}

/// Plan one move for a built-in style.
#[uniffi::export]
pub fn plan_cursor_move(
    params: CursorMotionParams,
    request: CursorMoveRequest,
) -> Arc<CursorTrajectory> {
    Arc::new(CursorTrajectory {
        inner: cm::plan_move(&params.into(), &request.into()),
    })
}

/// Plan one move for a custom spec.
#[uniffi::export]
pub fn plan_cursor_spec(
    spec: CursorMotionSpec,
    request: CursorMoveRequest,
) -> Arc<CursorTrajectory> {
    Arc::new(CursorTrajectory {
        inner: cm::plan_spec(&spec.into(), &request.into()),
    })
}

/// Click effects (ripple, squish) `age` seconds after a click at `(x, y)`.
#[uniffi::export]
pub fn cursor_click_effects(
    effects: CursorMotionEffectsOutput,
    age: f64,
    x: f64,
    y: f64,
) -> CursorEffectFrame {
    let mut frame = cm::EffectFrame::default();
    cm::effects::add_click(&mut frame, effects_in(effects), age, (x, y));
    frame.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ffi_planner_matches_the_crate() {
        let params = CursorMotionParams {
            style: CursorMotionStyle::CometSwoop,
            ..default_cursor_motion_params()
        };
        let request = CursorMoveRequest {
            from_point: CursorMotionPoint { x: 100.0, y: 100.0 },
            to_point: CursorMotionPoint { x: 700.0, y: 400.0 },
            from_heading: None,
            end_heading: None,
            target: Some(CursorMotionRect {
                x: 660.0,
                y: 380.0,
                width: 80.0,
                height: 40.0,
            }),
            seed: "s".into(),
            reduced_motion: false,
        };
        let ffi = plan_cursor_move(params.clone(), request.clone());
        let native = cm::plan_move(&params.clone().into(), &request.clone().into());
        assert_eq!(ffi.samples().len(), native.samples.len());
        assert_eq!(ffi.arrival_t(), native.arrival_t);
        let spec = cursor_motion_spec_for_style(CursorMotionStyle::CometSwoop, params).unwrap();
        let from_spec = plan_cursor_spec(spec, request);
        assert_eq!(from_spec.samples(), ffi.samples());
        assert!(!ffi
            .effect_frame(ffi.duration() / 2.0, true)
            .trail
            .is_empty());
    }
}
