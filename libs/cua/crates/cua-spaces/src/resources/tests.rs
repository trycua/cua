use super::*;

const TB: u64 = 1024 * GIB;

#[test]
fn the_warning_line_is_50_gb_or_15_percent_whichever_is_higher_capped_at_100() {
    // A small disk: 15% is under the 50 GiB floor.
    assert_eq!(low_line(100 * GIB), 50 * GIB);
    // A 512 GiB disk: 15% is 76 GiB.
    assert_eq!(low_line(512 * GIB), 512 * GIB / 100 * 15);
    // A 1 TiB disk: 15% is 153 GiB, capped at 100.
    assert_eq!(low_line(TB), 100 * GIB);
    assert_eq!(low_line(8 * TB), 100 * GIB);
}

#[test]
fn free_disk_under_the_line_warns_and_under_the_floor_is_critical() {
    let min = 5 * GIB;
    let at = |free: u64, total: u64| DiskWarning::assess(free, total, min, DiskLevel::Ok);
    // Plenty.
    let ok = at(300 * GIB, TB);
    assert_eq!((ok.level, ok.threshold), (DiskLevel::Ok, 0));
    // 99 GiB free of 1 TiB: under the capped 100 GiB line.
    let low = at(99 * GIB, TB);
    assert_eq!((low.level, low.threshold), (DiskLevel::Low, 100 * GIB));
    // 40 GiB free of 100 GiB: under 50 GiB.
    assert_eq!(at(40 * GIB, 100 * GIB).level, DiskLevel::Low);
    // 60 GiB free of 100 GiB: fine (15% would be 15 GiB, the floor is 50).
    assert_eq!(at(60 * GIB, 100 * GIB).level, DiskLevel::Ok);
    // Under the floor where creates fail.
    let critical = at(4 * GIB, TB);
    assert_eq!(
        (critical.level, critical.threshold),
        (DiskLevel::Critical, min)
    );
    assert_eq!((critical.free, critical.total), (4 * GIB, TB));
}

#[test]
fn a_raised_warning_clears_only_above_its_line_with_margin() {
    let min = 5 * GIB;
    let total = TB;
    let line = low_line(total);
    // Just over the line, but a warning was up: it stays.
    let w = DiskWarning::assess(line + GIB, total, min, DiskLevel::Low);
    assert_eq!(w.level, DiskLevel::Low);
    // 2 GiB over: it clears.
    let w = DiskWarning::assess(line + 2 * GIB, total, min, DiskLevel::Low);
    assert_eq!(w.level, DiskLevel::Ok);
    // From critical, climbing past the floor with margin: now low, not ok.
    let w = DiskWarning::assess(min + 3 * GIB, total, min, DiskLevel::Critical);
    assert_eq!(w.level, DiskLevel::Low);
    // Critical stays within 2 GiB of the floor.
    let w = DiskWarning::assess(min + GIB, total, min, DiskLevel::Critical);
    assert_eq!(w.level, DiskLevel::Critical);
    // With no warning up, the line itself is where it starts.
    assert_eq!(
        DiskWarning::assess(line + GIB, total, min, DiskLevel::Ok).level,
        DiskLevel::Ok
    );
}

#[test]
fn levels_have_words_and_json_names() {
    for l in [DiskLevel::Ok, DiskLevel::Low, DiskLevel::Critical] {
        assert_eq!(DiskLevel::parse(l.as_str()), l);
        assert_eq!(serde_json::to_value(l).unwrap(), l.as_str());
    }
    assert_eq!(DiskLevel::parse("nonsense"), DiskLevel::Ok);
}

fn space(id: &str) -> SpaceResources {
    SpaceResources {
        id: id.into(),
        power_state: "running".into(),
        cpu_allocated: 4,
        memory_allocated: 8 * GIB,
        disk_allocated: 80 * GIB,
        usage: Some(GuestUsage {
            cpu_count: 4,
            cpu_percent: 22.0,
            memory_used: 3 * GIB,
            memory_total: 8 * GIB,
            memory_limited: true,
            disk_used: 20 * GIB,
            disk_total: Some(80 * GIB),
            disk_limited: true,
            ..Default::default()
        }),
        disk_on_host: Some(DiskOnHost {
            bytes: Some(6 * GIB),
            allocated: 31 * GIB,
            shares_space: true,
        }),
        base: None,
    }
}

fn snap(sequence: u64) -> Resources {
    let this_mac = ComputerResources {
        id: THIS_MAC.into(),
        online: true,
        cpu_count: 12,
        cpu_percent: 38.0,
        memory_total: 64 * GIB,
        memory_used: 41 * GIB,
        disk_total: TB,
        disk_free: 300 * GIB,
        ..Default::default()
    };
    Resources {
        sequence,
        sampled_at_ms: sequence * 1000,
        warning: this_mac.warning,
        this_mac,
        spaces: vec![space("local:a"), space("local:b")],
        computers: vec![],
    }
}

#[test]
fn a_small_wobble_sends_nothing() {
    let sent = snap(1);
    let mut now = snap(2);
    // 1 point of processor, 200 MB of memory: under both thresholds.
    now.this_mac.cpu_percent += 1.0;
    now.this_mac.memory_used += 200 * MIB;
    now.spaces[0].usage.as_mut().unwrap().cpu_percent += 0.9;
    now.spaces[1].usage.as_mut().unwrap().memory_used += 255 * MIB;
    assert!(now.update_since(&sent).is_empty());
}

#[test]
fn a_change_past_a_threshold_sends_only_that_entry() {
    let sent = snap(1);
    let mut now = snap(2);
    now.spaces[1].usage.as_mut().unwrap().cpu_percent = 50.0;
    let u = now.update_since(&sent);
    assert_eq!(u.sequence, 2);
    assert_eq!(u.this_mac, None);
    assert_eq!(u.spaces.len(), 1);
    assert_eq!(u.spaces[0].id, "local:b");
    assert!(u.removed.is_empty());

    let mut now = snap(3);
    now.this_mac.memory_used += 300 * MIB;
    let u = now.update_since(&sent);
    assert!(u.this_mac.is_some());
    assert!(u.spaces.is_empty());
}

#[test]
fn power_and_allocation_changes_always_send() {
    let sent = snap(1);
    let mut now = snap(2);
    now.spaces[0].power_state = "stopped".into();
    now.spaces[0].usage = None;
    now.spaces[1].cpu_allocated = 6;
    let u = now.update_since(&sent);
    assert_eq!(u.spaces.len(), 2);
}

#[test]
fn spaces_that_appear_or_vanish_are_in_the_update() {
    let sent = snap(1);
    let mut now = snap(2);
    now.spaces.remove(0);
    now.spaces.push(space("local:c"));
    now.computers.push(ComputerResources {
        id: "m-1".into(),
        online: true,
        ..Default::default()
    });
    let u = now.update_since(&sent);
    assert_eq!(
        u.spaces.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
        ["local:c"]
    );
    assert_eq!(u.removed, ["local:a"]);
    assert_eq!(u.computers.len(), 1);
    // The computer going away is a removal too.
    let back = snap(3).update_since(&now);
    assert!(back.removed.contains(&"m-1".to_string()));
    assert!(back.removed.contains(&"local:c".to_string()));
}

#[test]
fn applying_updates_reaches_the_same_picture() {
    let mut held = snap(1);
    let mut truth = snap(1);
    for step in 2..6u64 {
        truth.sequence = step;
        truth.sampled_at_ms = step * 1000;
        truth.this_mac.cpu_percent = 10.0 * step as f64;
        truth.spaces[0].usage.as_mut().unwrap().memory_used = step * GIB;
        if step == 4 {
            truth.spaces.pop();
        }
        let u = truth.update_since(&held);
        held.apply(&u);
    }
    assert_eq!(held, truth);
}

#[test]
fn the_snapshot_json_is_camel_case_and_numbers_only() {
    let mut s = snap(7);
    s.computers.push(ComputerResources {
        id: "m-1".into(),
        name: "Mac mini".into(),
        online: true,
        ..Default::default()
    });
    let v = serde_json::to_value(&s).unwrap();
    assert_eq!(v["sequence"], 7);
    assert!(v["thisMac"]["cpuPercent"].is_number());
    assert_eq!(v["thisMac"]["warning"]["level"], "ok");
    assert_eq!(v["spaces"][0]["powerState"], "running");
    assert_eq!(v["spaces"][0]["usage"]["memoryLimited"], true);
    assert_eq!(v["spaces"][0]["diskOnHost"]["sharesSpace"], true);
    let back: Resources = serde_json::from_value(v).unwrap();
    assert_eq!(back, s);
    // No field of a Space says where its files are or what it runs.
    let text = serde_json::to_string(&s.spaces[0]).unwrap();
    for word in ["path", "dir", "file", "process", "window", "name"] {
        assert!(!text.contains(word), "{word} in {text}");
    }
}

#[test]
fn events_have_a_stable_json_shape() {
    let e = ResourcesEvent::LowDisk(DiskWarning::assess(GIB, TB, 5 * GIB, DiskLevel::Ok));
    let v = serde_json::to_value(&e).unwrap();
    assert_eq!(v["type"], "lowDisk");
    assert_eq!(v["value"]["level"], "critical");
    assert_eq!(serde_json::from_value::<ResourcesEvent>(v).unwrap(), e);
}

/// gVisor's root overlay: `MaxInt64` rounded down to a page.
const GVISOR_UNLIMITED: u64 = (i64::MAX as u64) & !4095;

#[test]
fn a_size_that_is_a_sentinel_is_no_size() {
    use super::known_size;
    for sentinel in [i64::MAX as u64, u64::MAX, GVISOR_UNLIMITED, 0] {
        assert_eq!(known_size(sentinel), None, "{sentinel}");
    }
    assert_eq!(known_size(80 * GIB), Some(80 * GIB));
    assert_eq!(known_size(1), Some(1));
}

#[test]
fn a_guest_with_an_unlimited_root_reports_no_disk_size() {
    let u = GuestUsage::from(crate::stream::Usage {
        disk_used: 241_664,
        disk_total: GVISOR_UNLIMITED,
        ..Default::default()
    });
    assert_eq!(u.disk_total, None);
    assert_eq!(u.disk_used, 241_664);
    // A VM's own disk keeps its size.
    let vm = GuestUsage::from(crate::stream::Usage {
        disk_used: 20 * GIB,
        disk_total: 80 * GIB,
        disk_limited: true,
        ..Default::default()
    });
    assert_eq!((vm.disk_total, vm.disk_limited), (Some(80 * GIB), true));
}

#[test]
fn json_from_an_older_daemon_never_carries_a_sentinel() {
    for raw in [
        "9223372036854775807",
        "18446744073709551615",
        "-1",
        "0",
        "null",
    ] {
        let u: GuestUsage = serde_json::from_str(&format!(
            r#"{{"cpuCount":2,"cpuPercent":1,"loadAverage1m":0,"memoryUsed":1,"memoryTotal":2,"memoryLimited":true,"diskUsed":5,"diskTotal":{raw},"diskLimited":false}}"#
        ))
        .unwrap();
        assert_eq!(u.disk_total, None, "{raw}");
    }
    let u: GuestUsage = serde_json::from_str(
        r#"{"cpuCount":2,"cpuPercent":1,"loadAverage1m":0,"memoryUsed":1,"memoryTotal":2,"memoryLimited":true,"diskUsed":5,"diskLimited":true}"#,
    )
    .unwrap();
    assert_eq!(u.disk_total, None);
}

#[test]
fn the_wire_from_an_older_daemon_never_carries_a_sentinel() {
    use cua_proto::daemon::v1 as pb;
    for sentinel in [i64::MAX as u64, u64::MAX, GVISOR_UNLIMITED, 0] {
        let s = SpaceResources::from(pb::SpaceResources {
            id: "local:a".into(),
            usage: Some(pb::GuestUsage {
                disk_used: 1_000,
                disk_total: sentinel,
                disk_limited: true,
                ..Default::default()
            }),
            ..Default::default()
        });
        let u = s.usage.unwrap();
        assert_eq!((u.disk_total, u.disk_limited), (None, false), "{sentinel}");
    }
}

#[test]
fn shared_bases_count_each_base_once() {
    use super::disk::{BaseMeasure, shared_bases};
    let m = |key: &str, bytes: Option<u64>| {
        Some(BaseMeasure {
            key: key.into(),
            bytes,
        })
    };
    let ubuntu = "ghcr.io/trycua/linux:24.04".to_string();
    let macos = "ghcr.io/trycua/macos:26".to_string();
    let out = shared_bases(&[
        // Copy-on-write: two containers on one image.
        (ubuntu.clone(), m("sha256:u", Some(3 * GIB))),
        (ubuntu.clone(), m("sha256:u", Some(3 * GIB))),
        // The last user of its base.
        (macos.clone(), m("sha256:m", Some(25 * GIB))),
        // Not copy-on-write (or a runtime that keeps its base apart).
        ("x".into(), None),
        // Can't tell its own data from the base's.
        ("example.org/windows:11".into(), m("sha256:w", None)),
    ]);
    let u = out[0].clone().unwrap();
    assert_eq!(
        (u.bytes, u.shared_with, u.image.as_str()),
        (Some(3 * GIB), 2, ubuntu.as_str())
    );
    assert_eq!(out[1], out[0]);
    let last = out[2].clone().unwrap();
    assert_eq!((last.bytes, last.shared_with), (Some(25 * GIB), 1));
    assert_eq!(out[3], None);
    let w = out[4].clone().unwrap();
    assert_eq!((w.bytes, w.shared_with), (None, 1));
}

#[test]
fn clones_of_one_base_report_the_most_any_shares() {
    use super::disk::{BaseMeasure, shared_bases, shared_blocks};
    // A clone that changed more shares fewer blocks with its base.
    let a = DiskOnHost {
        bytes: Some(6 * GIB),
        allocated: 31 * GIB,
        shares_space: true,
    };
    let b = DiskOnHost {
        bytes: Some(10 * GIB),
        allocated: 30 * GIB,
        shares_space: true,
    };
    let unknown = DiskOnHost {
        bytes: None,
        allocated: 30 * GIB,
        shares_space: true,
    };
    assert_eq!(shared_blocks(&unknown), None);
    let m = |d: &DiskOnHost| {
        Some(BaseMeasure {
            key: "macos".into(),
            bytes: shared_blocks(d),
        })
    };
    let out = shared_bases(&[
        ("macos".into(), m(&a)),
        ("macos".into(), m(&b)),
        ("macos".into(), m(&unknown)),
    ]);
    for s in &out {
        let s = s.clone().unwrap();
        assert_eq!((s.bytes, s.shared_with), (Some(25 * GIB), 3));
    }
}
