#![doc = include_str!("../README.md")]

pub mod dubins;
pub mod ease;
pub mod effects;
pub mod geom;
pub mod params;
pub mod path;
pub mod plan;
pub mod rng;
pub mod spec;
pub mod style;

pub use ease::Ease;
pub use effects::{EffectFrame, TrailSpec};
pub use geom::{anchor_for_pointer, pointer_for_anchor, wrap_angle, Pt, POINTER_ANCHOR_OFFSET};
pub use params::MotionParams;
pub use path::ArcShape;
pub use plan::{
    plan_move, plan_spec, MoveRequest, Sample, Trajectory, DEFAULT_TARGET_PT, DT_MS, REST_HEADING,
};
pub use rng::Rng;
pub use spec::{Duration, Heading, MotionSpec, PathShape, Settle};
pub use style::{MotionEffects, MotionStyle, MotionTiming, ResolvedEffects, DEFAULT_FIXED_MS};
