//! Planned agent-cursor trajectories.
//!
//! The motion math lives in the `cua-motion` crate, the single source of
//! truth shared with the `@trycua/motion` web package. This module adapts the
//! driver's [`MotionConfig`] to it.

use crate::motion::MotionConfig;
pub use cua_motion::ease;
pub use cua_motion::path::{bow_path, cua_path, natural_side, ArcShape, Path};
pub use cua_motion::plan::*;
pub use cua_motion::rng::{hash_string, Rng};
pub use cua_motion::{wrap_angle, Pt};

/// Plan one move for `motion.style`.
pub fn plan_move(motion: &MotionConfig, req: &MoveRequest) -> Trajectory {
    cua_motion::plan_move(&motion.params(), req)
}
