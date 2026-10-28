//! The host's system-wide open-file table, a resource every VM and job draws
//! on: the measurement lives in [`cua_home::filetable`]; this adds the
//! admission check that refuses with [`VmmError::HostCapacity`].

pub use cua_home::filetable::*;

use crate::error::VmmError;

/// Refuses new work (`what`: "a Linux Space", "a job") while the host's file
/// table is short of room ([`check`] against [`min_free`]) with
/// [`VmmError::HostCapacity`]. Passes when the OS does not say.
pub fn admit(what: &str) -> crate::Result<()> {
    let Some(table) = read() else {
        return Ok(());
    };
    check(table, min_free()).map_err(|t| VmmError::HostCapacity(refusal(t, what, &top_holders(3))))
}
