//! Fallback for targets with no perception worker containment implementation.
//!
//! Launching an unconstrained worker would be a silent downgrade of the
//! cross-platform containment contract, so the launch fails instead.
//! [`super::capabilities`] reports every boundary as absent, which makes
//! [`super::spawn`] fail closed before it ever reaches this module.

use std::path::Path;

use cua_driver_contract::VisualParseError;

use super::{unsupported_error, ContainedChild, ContainmentLimits, FilesystemBoundary, RawExit};

/// Duplex streams stand in for the protocol pipes so the cross-platform client
/// type-checks on a target that never launches a worker.
pub(super) type WorkerStdin = tokio::io::DuplexStream;
pub(super) type WorkerStdout = tokio::io::DuplexStream;

pub(super) struct Guard;

impl Guard {
    pub(super) fn note_reaped(&mut self) {}

    pub(super) fn memory_ceiling_exceeded(&self) -> bool {
        false
    }
}

pub(super) struct Process;

impl Process {
    pub(super) async fn wait(&mut self) -> std::io::Result<RawExit> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "this platform never launches a contained perception worker",
        ))
    }
}

pub(super) async fn spawn(
    _executable: &Path,
    _args: &[String],
    _working_directory: &Path,
    _boundary: &FilesystemBoundary,
    _limits: &ContainmentLimits,
) -> Result<ContainedChild, VisualParseError> {
    Err(unsupported_error(
        "this platform cannot contain the perception worker, so it will not be launched",
    ))
}
