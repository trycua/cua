//! The summary a computer publishes through its relay machine record, so
//! your other computers can show it without a connection to it.
//!
//! One `meta` key, [`META_KEY`], holds compact JSON of at most
//! [`MAX_LEN`] characters (the relay's value limit): sizes in MiB, times in
//! unix seconds. Numbers only: counts, bytes and percentages.

use super::{ComputerResources, DiskLevel, DiskWarning, MIB, low_line};
use serde::{Deserialize, Serialize};

/// The relay machine meta key.
pub const META_KEY: &str = "cua.res";
/// The relay's limit on a meta value, in characters.
pub const MAX_LEN: usize = 256;
/// The summary format.
pub const VERSION: u32 = 1;

/// How often a computer publishes while someone watches it.
pub const WATCHED_EVERY: std::time::Duration = std::time::Duration::from_secs(60);
/// How often it publishes otherwise.
pub const IDLE_EVERY: std::time::Duration = std::time::Duration::from_secs(300);

#[derive(Serialize, Deserialize)]
struct Wire {
    v: u32,
    /// Measured at, unix seconds.
    t: u64,
    /// Cores.
    c: u32,
    /// Busy, whole percent.
    b: u32,
    /// Memory size and in use, MiB.
    mt: u64,
    mu: u64,
    /// Volume size and free, MiB.
    dt: u64,
    df: u64,
    /// Extra Spaces: count, running.
    n: u32,
    r: u32,
    /// Cores and memory (MiB) given to the running ones.
    sc: u32,
    sm: u64,
    /// Memory in use by the running ones, MiB.
    su: u64,
    /// Disk the extra Spaces take, MiB.
    sd: u64,
    /// Every Space's disk was measured (1) or some were estimated (0).
    x: u8,
    /// Warning: 0 ok, 1 low, 2 critical.
    w: u8,
}

fn mib(bytes: u64) -> u64 {
    bytes / MIB
}

/// `c` as the value of [`META_KEY`].
pub fn encode(c: &ComputerResources) -> String {
    let wire = Wire {
        v: VERSION,
        t: c.sampled_at_ms / 1000,
        c: c.cpu_count,
        b: c.cpu_percent.round().clamp(0.0, 100.0) as u32,
        mt: mib(c.memory_total),
        mu: mib(c.memory_used),
        dt: mib(c.disk_total),
        df: mib(c.disk_free),
        n: c.spaces_count,
        r: c.spaces_running,
        sc: c.spaces_cpu_allocated,
        sm: mib(c.spaces_memory_allocated),
        su: mib(c.spaces_memory_used),
        sd: mib(c.spaces_disk_bytes),
        x: u8::from(c.spaces_disk_exact),
        w: c.warning.level as u8,
    };
    serde_json::to_string(&wire).expect("plain numbers serialize")
}

/// A computer's published summary, as the computer `id` named `name`
/// (`online` as the relay lists it). `None` for text that is not a summary
/// of a version this build knows.
pub fn decode(id: &str, name: &str, online: bool, value: &str) -> Option<ComputerResources> {
    let w: Wire = serde_json::from_str(value).ok()?;
    if w.v != VERSION {
        return None;
    }
    let level = match w.w {
        1 => DiskLevel::Low,
        2 => DiskLevel::Critical,
        _ => DiskLevel::Ok,
    };
    let total = w.dt.saturating_mul(MIB);
    let free = w.df.saturating_mul(MIB);
    Some(ComputerResources {
        id: id.into(),
        name: name.into(),
        online,
        sampled_at_ms: w.t.saturating_mul(1000),
        cpu_count: w.c,
        cpu_percent: f64::from(w.b.min(100)),
        load_average_1m: 0.0,
        memory_total: w.mt.saturating_mul(MIB),
        memory_used: w.mu.saturating_mul(MIB),
        disk_total: total,
        disk_free: free,
        spaces_count: w.n,
        spaces_running: w.r,
        spaces_cpu_allocated: w.sc,
        spaces_memory_allocated: w.sm.saturating_mul(MIB),
        spaces_memory_used: w.su.saturating_mul(MIB),
        spaces_disk_bytes: w.sd.saturating_mul(MIB),
        spaces_disk_exact: w.x == 1,
        warning: DiskWarning {
            level,
            free,
            total,
            threshold: match level {
                DiskLevel::Ok => 0,
                DiskLevel::Low => low_line(total),
                DiskLevel::Critical => cua_vmm::disk::DEFAULT_MIN_FREE,
            },
        },
    })
}

/// Whether two computers' summaries differ enough to publish again before
/// the interval is up: the warning level changed.
pub fn urgent(now: &ComputerResources, published: &ComputerResources) -> bool {
    now.warning.level != published.warning.level
}

#[cfg(test)]
mod tests {
    use super::*;

    fn big() -> ComputerResources {
        ComputerResources {
            id: crate::resources::THIS_MAC.into(),
            online: true,
            sampled_at_ms: 1_760_000_000_000,
            cpu_count: 128,
            cpu_percent: 100.0,
            memory_total: 8 * 1024 * 1024 * MIB,
            memory_used: 8 * 1024 * 1024 * MIB,
            disk_total: 64 * 1024 * 1024 * MIB,
            disk_free: 64 * 1024 * 1024 * MIB,
            spaces_count: 999,
            spaces_running: 999,
            spaces_cpu_allocated: 9999,
            spaces_memory_allocated: 8 * 1024 * 1024 * MIB,
            spaces_memory_used: 8 * 1024 * 1024 * MIB,
            spaces_disk_bytes: 64 * 1024 * 1024 * MIB,
            spaces_disk_exact: true,
            warning: DiskWarning {
                level: DiskLevel::Critical,
                free: 1,
                total: 2,
                threshold: 3,
            },
            ..Default::default()
        }
    }

    #[test]
    fn the_largest_summary_fits_the_relay_limit() {
        let s = encode(&big());
        assert!(s.len() <= MAX_LEN, "{} chars: {s}", s.len());
        assert!(s.chars().all(|c| !c.is_control()));
    }

    #[test]
    fn a_summary_round_trips_in_whole_units() {
        let c = ComputerResources {
            id: "m-1".into(),
            online: true,
            sampled_at_ms: 1_760_000_123_000,
            cpu_count: 12,
            cpu_percent: 38.4,
            memory_total: 64 * 1024 * MIB,
            memory_used: 41 * 1024 * MIB,
            disk_total: 1024 * 1024 * MIB,
            disk_free: 41 * 1024 * MIB,
            spaces_count: 3,
            spaces_running: 2,
            spaces_cpu_allocated: 6,
            spaces_memory_allocated: 12 * 1024 * MIB,
            spaces_memory_used: 5 * 1024 * MIB,
            spaces_disk_bytes: 25 * 1024 * MIB,
            spaces_disk_exact: true,
            warning: DiskWarning::assess(
                41 * 1024 * MIB,
                1024 * 1024 * MIB,
                5 << 30,
                DiskLevel::Ok,
            ),
            ..Default::default()
        };
        let back = decode("m-1", "Mac mini", true, &encode(&c)).unwrap();
        assert_eq!(back.name, "Mac mini");
        assert_eq!(back.cpu_count, 12);
        assert_eq!(back.cpu_percent, 38.0);
        assert_eq!(back.memory_used, c.memory_used);
        assert_eq!(back.disk_free, c.disk_free);
        assert_eq!(back.spaces_running, 2);
        assert!(back.spaces_disk_exact);
        assert_eq!(back.warning.level, DiskLevel::Low);
        assert_eq!(back.sampled_at_ms, c.sampled_at_ms);
    }

    #[test]
    fn text_that_is_not_a_known_summary_is_ignored() {
        assert!(decode("m", "", true, "not json").is_none());
        assert!(decode("m", "", true, r#"{"v":99}"#).is_none());
        assert!(decode("m", "", true, "").is_none());
    }

    #[test]
    fn a_summary_carries_numbers_only() {
        let mut c = big();
        c.name = "Francesco's Mac".into();
        let s = encode(&c);
        assert!(!s.contains("Francesco"), "{s}");
        assert!(
            s.chars()
                .all(|ch| ch.is_ascii_digit() || "{}\":,vtcbmudnrsxwf".contains(ch)),
            "{s}"
        );
    }
}
