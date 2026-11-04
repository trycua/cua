//! Planned agent-cursor trajectories.
//!
//! The motion math lives in the `cua-cursor-motion` crate, the single source of
//! truth shared with the `@trycua/cursor-motion` web package. This module adapts the
//! driver's [`MotionConfig`] to it.

use crate::motion::MotionConfig;
pub use cua_cursor_motion::ease;
pub use cua_cursor_motion::path::{bow_path, cua_path, natural_side, ArcShape, Path};
pub use cua_cursor_motion::plan::*;
pub use cua_cursor_motion::rng::{hash_string, Rng};
pub use cua_cursor_motion::{wrap_angle, Pt};

/// Plan one move for `motion.style`.
pub fn plan_move(motion: &MotionConfig, req: &MoveRequest) -> Trajectory {
    cua_cursor_motion::plan_move(&motion.params(), req)
}
