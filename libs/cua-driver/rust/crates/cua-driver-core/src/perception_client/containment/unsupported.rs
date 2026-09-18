//! Fallback for targets with no perception worker containment implementation.
//!
//! Launching an unconstrained worker would be a silent downgrade of the
//! cross-platform containment contract, so the launch fails instead.

use std::path::Path;

use cua_driver_contract::VisualParseError;
use tokio::process::Child;

use super::{unsupported_error, ContainmentLimits};

#[allow(dead_code)]
pub(super) struct Guard;

pub(super) fn spawn(
    _executable: &Path,
    _args: &[String],
    _working_directory: &Path,
    _limits: &ContainmentLimits,
) -> Result<(Child, Guard), VisualParseError> {
    Err(unsupported_error(
        "this platform cannot contain the perception worker, so it will not be launched",
    ))
}
