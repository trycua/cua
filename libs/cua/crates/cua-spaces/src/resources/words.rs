//! The plain words for the numbers, for the CLI. The Spaces apps use the
//! same words from the app core (`resources.rs` there); both are tested
//! against the same sentences.
//!
//! Never "vCPU", "RSS", "APFS", "sparse", "qcow2" or "cgroup": a processor is
//! "busy", memory is "in use", a disk is "space for files".

use super::{ComputerResources, DiskLevel, DiskOnHost, DiskWarning, SpaceResources};

/// A size in binary units labelled the way macOS labels memory: "512 MB",
/// "2.1 GB", "4 GB", "1 TB".
pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["MB", "GB", "TB", "PB"];
    let mut v = bytes as f64 / (1024.0 * 1024.0);
    if v < 1.0 {
        return format!("{} KB", (bytes as f64 / 1024.0).round() as u64);
    }
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    let text = if v >= 100.0 {
        format!("{}", v.round() as u64)
    } else {
        let t = format!("{v:.1}");
        t.strip_suffix(".0").map(str::to_string).unwrap_or(t)
    };
    format!("{text} {}", UNITS[i])
}

/// "1 core", "12 cores".
pub fn cores(n: u32) -> String {
    format!("{n} core{}", if n == 1 { "" } else { "s" })
}

/// "12 cores · 38% busy"; "12 cores" without a reading.
pub fn processor(cores_n: u32, busy: Option<f64>) -> String {
    match busy {
        Some(p) => format!(
            "{} \u{b7} {}% busy",
            cores(cores_n),
            p.round().clamp(0.0, 100.0)
        ),
        None => cores(cores_n),
    }
}

/// "41 of 64 GB in use".
pub fn memory_in_use(used: u64, total: u64) -> String {
    format!("{} in use", of(used, total))
}

/// "3.1 of 8 GB": the number of the first with the unit of the second.
pub fn of(used: u64, total: u64) -> String {
    let used = used.min(total);
    let (u, t) = (size(used), size(total));
    match (u.rsplit_once(' '), t.rsplit_once(' ')) {
        (Some((n, unit)), Some((_, tu))) if unit == tu => format!("{n} of {t}"),
        _ => format!("{u} of {t}"),
    }
}

/// "41 GB free of 1 TB".
pub fn disk_free(free: u64, total: u64) -> String {
    format!("{} free of {}", size(free.min(total)), size(total))
}

/// What a Space costs this computer's disk: "about 6 GB", with "shares
/// space with other copies" when part of it is shared, or only that when
/// the number could not be measured. `None` when there is nothing to say.
pub fn disk_on_host(d: &DiskOnHost) -> Option<String> {
    match (d.bytes, d.shares_space) {
        (Some(b), true) => Some(format!("about {}, shares space with other copies", size(b))),
        (Some(b), false) => Some(format!("about {}", size(b))),
        (None, true) => Some("shares space with other copies".into()),
        (None, false) => None,
    }
}

/// "Your extra Spaces use: 6 cores, 12 GB of memory, about 25 GB for
/// files." `None` when there are none.
pub fn extra_spaces_use(c: &ComputerResources) -> Option<String> {
    if c.spaces_count == 0 {
        return None;
    }
    let disk = if c.spaces_disk_exact {
        format!("about {}", size(c.spaces_disk_bytes))
    } else {
        format!("up to about {}", size(c.spaces_disk_bytes))
    };
    Some(format!(
        "Your extra Spaces use: {}, {} of memory, {disk} for files.",
        cores(c.spaces_cpu_allocated),
        size(c.spaces_memory_allocated),
    ))
}

/// The warning's sentence, when there is one.
pub fn warning(w: &DiskWarning) -> Option<&'static str> {
    match w.level {
        DiskLevel::Ok => None,
        DiskLevel::Low => Some("This Mac is almost out of space. Delete old Spaces or snapshots."),
        DiskLevel::Critical => Some(
            "This Mac is out of space, so new Spaces can't be created. Delete old Spaces or snapshots.",
        ),
    }
}

/// The CPU cell of a Space row: "4 cores · 22% busy", "4 cores" while it
/// is off, "-" when its allocation is unknown.
pub fn space_cpu(s: &SpaceResources) -> String {
    let cores_n = if s.cpu_allocated > 0 {
        s.cpu_allocated
    } else {
        s.usage.map(|u| u.cpu_count).unwrap_or(0)
    };
    match (cores_n, s.usage) {
        (0, _) => "-".into(),
        (n, Some(u)) => processor(n, Some(u.cpu_percent)),
        (n, None) => processor(n, None),
    }
}

/// The memory cell of a Space row: "3.1 of 8 GB", "8 GB" while it is off,
/// "-" when unknown.
pub fn space_memory(s: &SpaceResources) -> String {
    let total = if s.memory_allocated > 0 {
        s.memory_allocated
    } else {
        s.usage
            .filter(|u| u.memory_limited)
            .map(|u| u.memory_total)
            .unwrap_or(0)
    };
    match (total, s.usage) {
        (0, _) => "-".into(),
        (t, Some(u)) => of(u.memory_used, t),
        (t, None) => size(t),
    }
}

/// The disk cell of a Space row: what it costs this computer, else "up to
/// 80 GB" (its size limit), else "-".
pub fn space_disk(s: &SpaceResources) -> String {
    if let Some(text) = s.disk_on_host.as_ref().and_then(disk_on_host) {
        return text;
    }
    match s.disk_allocated {
        0 => "-".into(),
        n => format!("up to {}", size(n)),
    }
}

/// The CPU cell of another computer's row.
pub fn computer_cpu(c: &ComputerResources) -> String {
    if c.cpu_count == 0 {
        "-".into()
    } else {
        processor(c.cpu_count, Some(c.cpu_percent))
    }
}

/// The memory cell of another computer's row.
pub fn computer_memory(c: &ComputerResources) -> String {
    if c.memory_total == 0 {
        "-".into()
    } else {
        of(c.memory_used, c.memory_total)
    }
}

/// The disk cell of another computer's row.
pub fn computer_disk(c: &ComputerResources) -> String {
    if c.disk_total == 0 {
        "-".into()
    } else {
        disk_free(c.disk_free, c.disk_total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::{GIB, GuestUsage};

    const TB: u64 = 1024 * GIB;

    #[test]
    fn sizes_read_like_the_finder() {
        assert_eq!(size(512 * 1024 * 1024), "512 MB");
        assert_eq!(size(4 * GIB), "4 GB");
        assert_eq!(size(3 * GIB + GIB / 10), "3.1 GB");
        assert_eq!(size(41 * GIB), "41 GB");
        assert_eq!(size(TB), "1 TB");
        assert_eq!(size(120 * GIB), "120 GB");
        assert_eq!(size(100 * 1024), "100 KB");
    }

    #[test]
    fn the_words_are_the_ones_the_apps_use() {
        assert_eq!(processor(12, Some(38.0)), "12 cores \u{b7} 38% busy");
        assert_eq!(processor(1, Some(0.4)), "1 core \u{b7} 0% busy");
        assert_eq!(processor(4, None), "4 cores");
        assert_eq!(memory_in_use(41 * GIB, 64 * GIB), "41 of 64 GB in use");
        assert_eq!(of(3 * GIB + GIB / 10, 8 * GIB), "3.1 of 8 GB");
        assert_eq!(of(512 * 1024 * 1024, 8 * GIB), "512 MB of 8 GB");
        assert_eq!(disk_free(41 * GIB, TB), "41 GB free of 1 TB");
    }

    #[test]
    fn what_a_space_costs_this_computer() {
        let shared = DiskOnHost {
            bytes: Some(31 * GIB),
            allocated: 87 * GIB,
            shares_space: true,
        };
        assert_eq!(
            disk_on_host(&shared).unwrap(),
            "about 31 GB, shares space with other copies"
        );
        let own = DiskOnHost {
            bytes: Some(6 * GIB),
            allocated: 6 * GIB,
            shares_space: false,
        };
        assert_eq!(disk_on_host(&own).unwrap(), "about 6 GB");
        let unknown = DiskOnHost {
            bytes: None,
            allocated: 12 * GIB,
            shares_space: true,
        };
        assert_eq!(
            disk_on_host(&unknown).unwrap(),
            "shares space with other copies"
        );
        assert_eq!(disk_on_host(&DiskOnHost::default()), None);
    }

    fn running() -> SpaceResources {
        SpaceResources {
            id: "local:a".into(),
            power_state: "running".into(),
            cpu_allocated: 4,
            memory_allocated: 8 * GIB,
            disk_allocated: 80 * GIB,
            usage: Some(GuestUsage {
                cpu_count: 4,
                cpu_percent: 22.0,
                memory_used: 3 * GIB + GIB / 10,
                memory_total: 8 * GIB,
                memory_limited: true,
                ..Default::default()
            }),
            disk_on_host: Some(DiskOnHost {
                bytes: Some(31 * GIB),
                allocated: 87 * GIB,
                shares_space: true,
            }),
            base: None,
        }
    }

    #[test]
    fn a_running_space_row() {
        let s = running();
        assert_eq!(space_cpu(&s), "4 cores \u{b7} 22% busy");
        assert_eq!(space_memory(&s), "3.1 of 8 GB");
        assert_eq!(
            space_disk(&s),
            "about 31 GB, shares space with other copies"
        );
    }

    #[test]
    fn a_stopped_space_shows_what_it_was_given() {
        let s = SpaceResources {
            power_state: "stopped".into(),
            usage: None,
            disk_on_host: Some(DiskOnHost {
                bytes: Some(6 * GIB),
                allocated: 6 * GIB,
                shares_space: false,
            }),
            ..running()
        };
        assert_eq!(space_cpu(&s), "4 cores");
        assert_eq!(space_memory(&s), "8 GB");
        assert_eq!(space_disk(&s), "about 6 GB");
        // No measurement: its size limit.
        let s = SpaceResources {
            disk_on_host: None,
            ..s
        };
        assert_eq!(space_disk(&s), "up to 80 GB");
        // Nothing known.
        let none = SpaceResources {
            id: "local:x".into(),
            ..Default::default()
        };
        assert_eq!(
            (space_cpu(&none), space_memory(&none), space_disk(&none)),
            ("-".into(), "-".into(), "-".into())
        );
    }

    #[test]
    fn another_computers_row() {
        let c = ComputerResources {
            cpu_count: 12,
            cpu_percent: 38.0,
            memory_total: 64 * GIB,
            memory_used: 41 * GIB,
            disk_total: TB,
            disk_free: 41 * GIB,
            ..Default::default()
        };
        assert_eq!(computer_cpu(&c), "12 cores \u{b7} 38% busy");
        assert_eq!(computer_memory(&c), "41 of 64 GB");
        assert_eq!(computer_disk(&c), "41 GB free of 1 TB");
        assert_eq!(computer_cpu(&ComputerResources::default()), "-");
    }

    #[test]
    fn the_extra_spaces_sentence_and_the_warning() {
        let mut c = ComputerResources {
            spaces_count: 3,
            spaces_cpu_allocated: 6,
            spaces_memory_allocated: 12 * GIB,
            spaces_disk_bytes: 25 * GIB,
            spaces_disk_exact: true,
            ..Default::default()
        };
        assert_eq!(
            extra_spaces_use(&c).unwrap(),
            "Your extra Spaces use: 6 cores, 12 GB of memory, about 25 GB for files."
        );
        c.spaces_disk_exact = false;
        assert!(extra_spaces_use(&c).unwrap().contains("up to about 25 GB"));
        c.spaces_count = 0;
        assert_eq!(extra_spaces_use(&c), None);

        let low = DiskWarning::assess(10 * GIB, TB, 5 * GIB, DiskLevel::Ok);
        assert_eq!(
            warning(&low),
            Some("This Mac is almost out of space. Delete old Spaces or snapshots.")
        );
        let ok = DiskWarning::assess(500 * GIB, TB, 5 * GIB, DiskLevel::Ok);
        assert_eq!(warning(&ok), None);
        let crit = DiskWarning::assess(GIB, TB, 5 * GIB, DiskLevel::Ok);
        assert!(warning(&crit).unwrap().contains("out of space"));
    }

    #[test]
    fn the_copy_has_no_jargon() {
        let texts = [
            processor(12, Some(38.0)),
            memory_in_use(GIB, 2 * GIB),
            disk_free(GIB, 2 * GIB),
            disk_on_host(&DiskOnHost {
                bytes: Some(GIB),
                allocated: 2 * GIB,
                shares_space: true,
            })
            .unwrap(),
            warning(&DiskWarning::assess(GIB, TB, 5 * GIB, DiskLevel::Ok))
                .unwrap()
                .into(),
        ];
        for t in texts {
            for jargon in ["vcpu", "rss", "apfs", "sparse", "qcow", "cgroup", "clone"] {
                assert!(!t.to_lowercase().contains(jargon), "{jargon} in {t}");
            }
        }
    }
}
