//! The sandbox sizes the SDK asks Cua Cloud for.
//!
//! Two different things live here:
//!
//! - `FLEET_ABSOLUTE_*`: the hard ceiling the SDK enforces before it calls
//!   Fleet. It only refuses nonsense: Fleet's CRD schema allows at most 64
//!   vCPUs per sandbox and 50 sandboxes per pool (trycua/cloud
//!   `clusters/base/osgym/crd.yaml`), and it has no memory maximum, so the
//!   memory bound here is a loose sanity check.
//! - `CLOUD_DEFAULT_RANGE_*`: the everyday sizes the Cua apps offer
//!   (1-8 vCPU, 1-32 GiB). Fleet's admission caps most accounts at about
//!   this range, but an account can be exempt and run bigger sandboxes, so
//!   the SDK never refuses a size just because it is outside this range.
//!   The Spaces app core's `CLOUD_CPU_RANGE` / `CLOUD_MEMORY_GB_RANGE`
//!   mirror it.
//!
//! Fleet is the source of truth for what an account may run: a size its
//! admission refuses comes back as [`Error::AdmissionDenied`] with Fleet's
//! own message.
//!
//! Version: 2026-09-27.
//!
//! [`Error::AdmissionDenied`]: crate::Error::AdmissionDenied

use crate::{Error, Result};
use std::ops::RangeInclusive;

/// vCPUs per cloud sandbox (`vmTemplate.cpuCores`) the SDK accepts: whole
/// numbers up to Fleet's CRD maximum.
pub const FLEET_ABSOLUTE_CPUS: RangeInclusive<u32> = 1..=64;

/// Memory per cloud sandbox in MiB (`vmTemplate.memory`) the SDK accepts:
/// at least 512 MiB, at most a loose 512 GiB sanity bound.
pub const FLEET_ABSOLUTE_MEMORY_MB: RangeInclusive<u32> = 512..=512 * 1024;

/// Most sandboxes per pool (`spec.replicas`, `spec.autoscaling.*PoolSize`),
/// Fleet's CRD maximum.
pub const FLEET_ABSOLUTE_MAX_POOL_SIZE: u32 = 50;

/// The everyday vCPU range of a cloud sandbox, which the Cua apps offer.
/// Not enforced by the SDK (see the module docs).
pub const CLOUD_DEFAULT_RANGE_CPUS: RangeInclusive<u32> = 1..=8;

/// The everyday memory range of a cloud sandbox in MiB (1-32 GiB), which the
/// Cua apps offer. Not enforced by the SDK (see the module docs).
pub const CLOUD_DEFAULT_RANGE_MEMORY_MB: RangeInclusive<u32> = 1024..=32 * 1024;

/// Checks a cloud sandbox size against [`FLEET_ABSOLUTE_CPUS`] and
/// [`FLEET_ABSOLUTE_MEMORY_MB`]. Unset values take Fleet's template
/// defaults (4 vCPUs, 4 GiB). Whether this account may run the size is
/// Fleet's call.
pub fn check_cloud_size(cpu: Option<u32>, memory_mb: Option<u32>) -> Result<()> {
    if let Some(c) = cpu.filter(|c| !FLEET_ABSOLUTE_CPUS.contains(c)) {
        return Err(Error::InvalidArgument(format!(
            "cpu {c}: a Cua Cloud sandbox has {}-{} vCPUs",
            FLEET_ABSOLUTE_CPUS.start(),
            FLEET_ABSOLUTE_CPUS.end()
        )));
    }
    if let Some(m) = memory_mb.filter(|m| !FLEET_ABSOLUTE_MEMORY_MB.contains(m)) {
        return Err(Error::InvalidArgument(format!(
            "memory_mb {m}: a Cua Cloud sandbox has {}-{} MiB (512 MiB to {} GiB)",
            FLEET_ABSOLUTE_MEMORY_MB.start(),
            FLEET_ABSOLUTE_MEMORY_MB.end(),
            FLEET_ABSOLUTE_MEMORY_MB.end() / 1024
        )));
    }
    Ok(())
}

/// Checks a pool size field (`what`: `replicas`, `max_pool_size`, ...)
/// against [`FLEET_ABSOLUTE_MAX_POOL_SIZE`].
pub fn check_pool_size(what: &str, n: u32) -> Result<()> {
    if n > FLEET_ABSOLUTE_MAX_POOL_SIZE {
        return Err(Error::InvalidArgument(format!(
            "{what} {n}: a Cua Cloud pool has at most {FLEET_ABSOLUTE_MAX_POOL_SIZE} sandboxes"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_within_the_ceiling_pass() {
        check_cloud_size(None, None).unwrap();
        check_cloud_size(Some(1), Some(512)).unwrap();
        check_cloud_size(Some(8), Some(32 * 1024)).unwrap();
        // Exempt accounts run these today; Fleet decides, not the SDK.
        check_cloud_size(Some(16), Some(64 * 1024)).unwrap();
        check_cloud_size(Some(64), Some(100 * 1024)).unwrap();
        check_cloud_size(Some(64), Some(512 * 1024)).unwrap();
    }

    #[test]
    fn nonsense_sizes_are_refused_before_fleet() {
        for (cpu, mem) in [
            (Some(0), None),
            (Some(65), None),
            (Some(10_000), None),
            (None, Some(256)),
            (None, Some(511)),
            (None, Some(512 * 1024 + 1)),
            (None, Some(u32::MAX)),
        ] {
            let e = check_cloud_size(cpu, mem).unwrap_err().to_string();
            assert!(e.contains("a Cua Cloud sandbox has"), "{e}");
        }
    }

    #[test]
    fn the_everyday_range_is_inside_the_ceiling() {
        assert!(FLEET_ABSOLUTE_CPUS.contains(CLOUD_DEFAULT_RANGE_CPUS.start()));
        assert!(FLEET_ABSOLUTE_CPUS.contains(CLOUD_DEFAULT_RANGE_CPUS.end()));
        assert!(FLEET_ABSOLUTE_MEMORY_MB.contains(CLOUD_DEFAULT_RANGE_MEMORY_MB.start()));
        assert!(FLEET_ABSOLUTE_MEMORY_MB.contains(CLOUD_DEFAULT_RANGE_MEMORY_MB.end()));
    }

    #[test]
    fn pool_sizes_stop_at_fifty() {
        check_pool_size("replicas", 0).unwrap();
        check_pool_size("replicas", 50).unwrap();
        let e = check_pool_size("max_pool_size", 51)
            .unwrap_err()
            .to_string();
        assert!(e.contains("at most 50"), "{e}");
    }
}
