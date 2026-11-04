use super::*;

const T0: u64 = 1_800_000_000_000;
const MIN: u64 = 60_000;

fn policy() -> WarmPolicy {
    WarmPolicy {
        warm: 1,
        cap: 2,
        ttl_ms: 30 * MIN,
        scale_to_zero_after_ms: 15 * MIN,
        golden_key: None,
        budget: None,
    }
}

fn entry(id: &str, ready: u64) -> WarmEntry {
    WarmEntry {
        id: id.into(),
        ready_ms: ready,
        image_key: None,
    }
}

fn demand(last: u64) -> Demand {
    Demand {
        queued_jobs: 0,
        last_demand_ms: last,
    }
}

#[test]
fn an_empty_pool_with_demand_creates_the_warm_count() {
    let p = plan(&policy(), T0, &Counts::default(), &demand(T0 - MIN));
    assert_eq!((p.create, p.target), (1, 1));
    assert!(p.reap.is_empty());
}

#[test]
fn nothing_more_is_created_when_the_target_is_met_or_booting() {
    let c = Counts {
        warm: vec![entry("a", T0 - MIN)],
        ..Default::default()
    };
    assert_eq!(plan(&policy(), T0, &c, &demand(T0)).create, 0);
    let c = Counts {
        booting: 1,
        ..Default::default()
    };
    assert_eq!(plan(&policy(), T0, &c, &demand(T0)).create, 0);
}

#[test]
fn apples_two_vm_limit_caps_warm_vms_behind_busy_ones() {
    let mut pol = policy();
    pol.warm = 2;
    // Two jobs run: no room for a warm VM at all.
    let c = Counts {
        busy: 2,
        ..Default::default()
    };
    assert_eq!(plan(&pol, T0, &c, &demand(T0)).create, 0);
    // One job runs: one warm VM fits, not two.
    let c = Counts {
        busy: 1,
        ..Default::default()
    };
    assert_eq!(plan(&pol, T0, &c, &demand(T0)).create, 1);
    // The cap the pool computes (other macOS VMs on the Mac) is respected.
    pol.cap = 1;
    let c = Counts::default();
    assert_eq!(plan(&pol, T0, &c, &demand(T0)).create, 1);
}

#[test]
fn queued_jobs_raise_the_target_up_to_the_free_slots() {
    let mut d = demand(T0);
    d.queued_jobs = 2;
    let p = plan(&policy(), T0, &Counts::default(), &d);
    assert_eq!((p.target, p.create), (2, 2));
    d.queued_jobs = 5;
    let p = plan(&policy(), T0, &Counts::default(), &d);
    assert_eq!((p.target, p.create), (2, 2), "never above the cap");
}

#[test]
fn scale_to_zero_after_quiet_time() {
    let c = Counts {
        warm: vec![entry("a", T0 - 5 * MIN), entry("b", T0 - 4 * MIN)],
        ..Default::default()
    };
    // Demand was 20 minutes ago: scale to zero, oldest first.
    let p = plan(&policy(), T0, &c, &demand(T0 - 20 * MIN));
    assert_eq!(p.target, 0);
    assert_eq!(p.create, 0);
    let ids: Vec<_> = p.reap.iter().map(|r| (r.0.as_str(), r.1)).collect();
    assert!(ids.contains(&("a", ReapReason::ScaleDown)));
    assert!(ids.contains(&("b", ReapReason::ScaleDown)));
    // A queued job stops the scale down.
    let mut d = demand(T0 - 20 * MIN);
    d.queued_jobs = 1;
    assert!(plan(&policy(), T0, &c, &d).target >= 1);
    // And with the pool empty and idle nothing is created.
    let p = plan(&policy(), T0, &Counts::default(), &demand(T0 - 20 * MIN));
    assert_eq!((p.create, p.target), (0, 0));
}

#[test]
fn ttl_replaces_without_creating_in_the_same_pass() {
    let c = Counts {
        warm: vec![entry("old", T0 - 31 * MIN)],
        ..Default::default()
    };
    let p = plan(&policy(), T0, &c, &demand(T0));
    assert_eq!(p.reap, vec![("old".to_string(), ReapReason::Ttl)]);
    assert_eq!(p.create, 0, "the deleted VM still holds its slot");
    // Next pass, once it is gone, the replacement is created.
    let p = plan(&policy(), T0 + 3_000, &Counts::default(), &demand(T0));
    assert_eq!(p.create, 1);
}

#[test]
fn a_stale_golden_image_is_replaced() {
    let mut pol = policy();
    pol.golden_key = Some("g2".into());
    let c = Counts {
        warm: vec![
            WarmEntry {
                id: "a".into(),
                ready_ms: T0,
                image_key: Some("g1".into()),
            },
            WarmEntry {
                id: "b".into(),
                ready_ms: T0,
                image_key: Some("g2".into()),
            },
            entry("c", T0),
        ],
        ..Default::default()
    };
    pol.warm = 2;
    let p = plan(&pol, T0 + 1, &c, &demand(T0));
    let stale: Vec<_> = p
        .reap
        .iter()
        .filter(|r| r.1 == ReapReason::StaleImage)
        .map(|r| r.0.as_str())
        .collect();
    assert_eq!(
        stale,
        vec!["a", "c"],
        "a missing key counts as stale when a golden image is current"
    );
}

#[test]
fn excess_removes_the_oldest() {
    let mut pol = policy();
    pol.cap = 3;
    let c = Counts {
        warm: vec![
            entry("a", T0 - 3 * MIN),
            entry("b", T0 - 2 * MIN),
            entry("c", T0 - MIN),
        ],
        ..Default::default()
    };
    let p = plan(&pol, T0, &c, &demand(T0));
    assert_eq!(p.target, 1);
    let gone: Vec<_> = p.reap.iter().map(|r| r.0.as_str()).collect();
    assert_eq!(gone, vec!["b", "a"], "the freshest stays");
    assert!(p.reap.iter().all(|r| r.1 == ReapReason::Excess));
}

#[test]
fn the_budget_bounds_warm_vms_and_stops_creates_at_the_limit() {
    let mut pol = policy();
    // $0.65/h computer, $100 left: a warm VM for 30 minutes costs $0.33,
    // under a quarter of $100.
    pol.budget = Some(Budget {
        limit_usd: 100.0,
        spent_usd: 0.0,
        usd_per_hour: 0.65,
    });
    assert_eq!(plan(&pol, T0, &Counts::default(), &demand(T0)).create, 1);
    // $1 left: a quarter is $0.25, below $0.33: no warm VM.
    pol.budget = Some(Budget {
        limit_usd: 100.0,
        spent_usd: 99.0,
        usd_per_hour: 0.65,
    });
    assert_eq!(plan(&pol, T0, &Counts::default(), &demand(T0)).create, 0);
    // At the limit nothing is created even for queued jobs here.
    pol.budget = Some(Budget {
        limit_usd: 100.0,
        spent_usd: 100.0,
        usd_per_hour: 0.65,
    });
    let mut d = demand(T0);
    d.queued_jobs = 2;
    assert_eq!(plan(&pol, T0, &Counts::default(), &d).create, 0);
    assert!(!pol.budget.unwrap().admits_job());
    // A computer you own is free: always allowed.
    pol.budget = Some(Budget {
        limit_usd: 10.0,
        spent_usd: 0.0,
        usd_per_hour: 0.0,
    });
    assert_eq!(plan(&pol, T0, &Counts::default(), &demand(T0)).create, 1);
}

#[test]
fn pick_prefers_the_freshest_current_vm_then_the_oldest_other() {
    let es = vec![
        WarmEntry {
            id: "a".into(),
            ready_ms: 1,
            image_key: Some("g".into()),
        },
        WarmEntry {
            id: "b".into(),
            ready_ms: 9,
            image_key: Some("g".into()),
        },
        WarmEntry {
            id: "c".into(),
            ready_ms: 5,
            image_key: Some("old".into()),
        },
    ];
    assert_eq!(pick(&es, Some("g")).unwrap().id, "b");
    assert_eq!(pick(&es, None).unwrap().id, "b");
    assert_eq!(
        pick(&es, Some("none")).unwrap().id,
        "a",
        "stale: the oldest"
    );
    assert!(pick(&[], Some("g")).is_none());
}

#[test]
fn the_ledger_rolls_over_with_the_month() {
    let mut l = Ledger::default();
    let oct = 1_793_188_801_123; // 2026-10-28
    l.charge(oct, 3_600_000, 0.65);
    l.charge(oct, 1_800_000, 0.65);
    assert!((l.spent_in(oct) - 0.975).abs() < 1e-9);
    assert_eq!(l.month, 202610);
    let nov = oct + 10 * 86_400_000;
    assert_eq!(l.spent_in(nov), 0.0);
    l.charge(nov, 3_600_000, 1.0);
    assert_eq!((l.month, l.spent_usd), (202611, 1.0));
    assert_eq!(yyyymm(0), 197001);
    assert_eq!(yyyymm(951_868_800_000), 200003);
}
