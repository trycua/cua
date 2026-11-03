// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The relay's memory: its resident set size and its container's limit for
//! `/metrics`, and the client stream budget that keeps worst-case buffering
//! under that limit.
//!
//! Each client stream can hold up to its yamux receive window (256 KiB)
//! plus the HTTP and WebSocket buffers on the way to a client that reads
//! slowly: about 600 KiB measured on 3 Nov 2026 (200 streams that never
//! read, +120 MiB). The fixed default of 4096 streams was about 2.4 GiB of
//! buffers alone, more than the 2 GiB pod that was OOM-killed that day.

/// Worst-case bytes one client stream buffers, with margin over the
/// ~600 KiB measured (`examples/slow_streams.rs`).
pub const STREAM_WORST_CASE_BYTES: u64 = 768 * 1024;

/// Share of the memory limit client stream buffers may take; the rest is
/// the relay's own state (directory, devices, audit log, mesh registry),
/// mesh connections and allocator overhead.
const STREAM_SHARE_PERCENT: u64 = 50;

/// Stream budget bounds: never below a handful (a tiny limit still serves
/// someone), never above the old fixed default.
const MIN_STREAMS: u64 = 64;
const MAX_STREAMS: u64 = 4096;

/// The client stream budget for a container memory limit of `limit` bytes.
pub fn stream_budget_for(limit: u64) -> u64 {
    (limit / 100 * STREAM_SHARE_PERCENT / STREAM_WORST_CASE_BYTES).clamp(MIN_STREAMS, MAX_STREAMS)
}

/// The memory limit of the relay's cgroup (v2 `memory.max`, or v1
/// `memory.limit_in_bytes`), in bytes; `None` without one.
pub fn cgroup_memory_limit() -> Option<u64> {
    let read = |path: &str| std::fs::read_to_string(path).ok();
    let raw = read("/sys/fs/cgroup/memory.max")
        .or_else(|| read("/sys/fs/cgroup/memory/memory.limit_in_bytes"))?;
    parse_limit(&raw)
}

fn parse_limit(raw: &str) -> Option<u64> {
    let raw = raw.trim();
    if raw == "max" {
        return None;
    }
    let bytes: u64 = raw.parse().ok()?;
    // cgroup v1 reports "no limit" as a huge page-aligned number.
    (bytes < 1 << 50).then_some(bytes)
}

/// The process's resident set size, in bytes (Linux).
pub fn resident_bytes() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        // statm: size resident shared ... in pages.
        let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
        let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
        // SAFETY: sysconf has no preconditions.
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        Some(pages * u64::try_from(page).ok()?)
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// Prometheus text for the memory gauges (those the platform reports).
pub(crate) fn metrics() -> String {
    let mut out = String::new();
    if let Some(rss) = resident_bytes() {
        out.push_str(&format!(
            "# HELP cua_relay_process_resident_bytes Resident set size of the relay process.\n\
             # TYPE cua_relay_process_resident_bytes gauge\n\
             cua_relay_process_resident_bytes {rss}\n"
        ));
    }
    if let Some(limit) = cgroup_memory_limit() {
        out.push_str(&format!(
            "# HELP cua_relay_memory_limit_bytes Memory limit of the relay's container.\n\
             # TYPE cua_relay_memory_limit_bytes gauge\n\
             cua_relay_memory_limit_bytes {limit}\n"
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_budget_keeps_worst_case_buffers_within_half_the_limit() {
        for gib in [1u64, 2, 4, 8] {
            let limit = gib << 30;
            let budget = stream_budget_for(limit);
            assert!(
                budget * STREAM_WORST_CASE_BYTES <= limit / 2,
                "{gib} GiB: {budget}"
            );
        }
        assert_eq!(stream_budget_for(2 << 30), 1365);
        assert_eq!(stream_budget_for(4 << 30), 2730);
        assert_eq!(
            stream_budget_for(64 << 30),
            4096,
            "capped at the old default"
        );
        assert_eq!(stream_budget_for(16 << 20), 64, "floor");
    }

    #[test]
    fn cgroup_limits_parse() {
        assert_eq!(parse_limit("4294967296\n"), Some(4 << 30));
        assert_eq!(parse_limit("max\n"), None);
        assert_eq!(parse_limit("9223372036854771712"), None, "v1 unlimited");
        assert_eq!(parse_limit("garbage"), None);
    }

    #[test]
    fn metrics_are_prometheus_text() {
        for line in metrics().lines().filter(|l| !l.starts_with('#')) {
            let (name, value) = line.split_once(' ').expect("name value");
            assert!(name.starts_with("cua_relay_"), "{line}");
            value.parse::<u64>().expect("a byte count");
        }
        #[cfg(target_os = "linux")]
        assert!(resident_bytes().unwrap() > 0);
    }
}
