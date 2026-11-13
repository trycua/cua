//! The elastic pool over time with fake clouds ([`super::sim`]): bursts,
//! steady state, scale-in at billing boundaries, host loss and budget
//! limits, driving the same solver and scaler the pool runs.

use super::scenarios::{aws_offers, hetzner_offers, modal_offers};
use super::sim::{Arrival, Sim};
use super::*;

fn mac(free_cpus: u64, free_gb: u64) -> Host {
    Host {
        name: "studio".into(),
        free: Shape::new(free_cpus * 1000, free_gb * 1024, 300),
        total: Shape::new(48_000, 96 * 1024, 900),
        arch: "arm64".into(),
        os: "macos".into(),
        kinds: vec!["container".into(), "vm".into()],
        kvm: true,
        ready: true,
        ..Default::default()
    }
}

fn worlds(at: u64, n: u32, per_world: u32, dur: u64) -> Vec<Arrival> {
    (0..n)
        .map(|w| Arrival {
            at,
            group: DemandGroup {
                id: format!("world-{w}"),
                count: per_world,
                per: Shape::new(1000, 1024, 1),
                whole: true,
                duration_secs: dur,
                latency: Latency::Batch,
                ..Default::default()
            },
        })
        .collect()
}

fn base() -> Constraints {
    Constraints {
        time_budget_ms: 50,
        max_usd_per_hour: Some(5.0),
        ..Default::default()
    }
}

fn clouds() -> Vec<Offer> {
    let mut v = hetzner_offers();
    v.extend(aws_offers());
    v
}

#[test]
fn a_256_space_burst_is_packed_started_within_a_boot_and_scaled_in() {
    let mut sim = Sim::new(vec![mac(40, 48)], clouds(), base());
    let r = sim.run(worlds(0, 32, 8, 1800), 3 * 3600, 30);
    assert_eq!(r.placed, 256, "{r:?}");
    assert_eq!(r.unplaced, 0);
    // The Mac takes 5 worlds; the rest pack 3 worlds per 16-vCPU host.
    assert!(r.peak_machines <= 10, "{r:?}");
    // Nobody waits longer than one boot.
    assert!(r.wait_max <= 90, "{r:?}");
    // Every machine is gone once the work ended and its hour was used.
    assert_eq!(sim.machines(), 0, "{r:?}");
    assert_eq!(r.removed, r.added);
    // Billed whole hours (Hetzner): about one hour per machine.
    assert!(r.cost_usd <= f64::from(r.added) * 0.11, "{r:?}");
    assert!(r.usd_per_space_hour() < 0.01, "{r:?}");
}

#[test]
fn steady_demand_keeps_a_steady_pool() {
    // A Space a minute, each for 20 minutes, for 4 hours: ~20 at once.
    let arrivals: Vec<Arrival> = (0..240)
        .map(|i| Arrival {
            at: i * 60,
            group: DemandGroup {
                id: format!("s{i}"),
                count: 1,
                per: Shape::new(1000, 1024, 1),
                duration_secs: 1200,
                latency: Latency::Batch,
                ..Default::default()
            },
        })
        .collect();
    let mut sim = Sim::new(vec![], hetzner_offers(), base());
    let r = sim.run(arrivals, 6 * 3600, 30);
    assert_eq!(r.unplaced, 0, "{r:?}");
    // No thrash: few machines, each kept while it is paid for, and the
    // bill within 2x of one machine sized for the whole load (a cax41 for
    // the 4 hours plus the hour it ends in: $0.342).
    assert!(r.added <= 10, "{r:?}");
    assert!(r.peak_machines <= 10, "{r:?}");
    assert!(r.cost_usd <= 2.0 * 5.0 * 0.0684, "{r:?}");
    assert_eq!(sim.machines(), 0, "scaled in after demand stopped: {r:?}");
}

#[test]
fn a_lost_host_has_its_spaces_placed_again() {
    let mut sim = Sim::new(vec![], hetzner_offers(), base());
    for a in worlds(0, 6, 8, 3600) {
        sim.place(0, &a.group);
    }
    sim.tick(120);
    let victim = sim
        .hosts
        .values()
        .find(|h| h.offer.is_some() && h.ended.is_none())
        .map(|h| h.host.name.clone())
        .unwrap();
    let on_victim = sim.spaces.iter().filter(|s| s.host == victim).count() as u32;
    assert!(on_victim > 0);
    sim.lose(600, &victim);
    let r = sim.finish(600);
    assert_eq!(r.replaced, on_victim, "{r:?}");
    assert!(sim.spaces.iter().all(|s| s.host != victim));
    assert_eq!(sim.spaces.len(), 48);
}

#[test]
fn budget_caps_hold_and_say_why() {
    let c = Constraints {
        max_usd_per_hour: Some(0.15),
        ..base()
    };
    let mut sim = Sim::new(vec![], clouds(), c);
    let r = sim.run(worlds(0, 32, 8, 1800), 2 * 3600, 30);
    assert!(r.peak_usd_per_hour <= 0.15 + 1e-9, "{r:?}");
    assert!(r.unplaced > 0, "{r:?}");
    assert!(r.placed > 0, "{r:?}");
    // The monthly budget too: nothing beyond it.
    let c = Constraints {
        budget_left_usd: Some(0.10),
        ..base()
    };
    let mut sim = Sim::new(vec![], clouds(), c);
    let r = sim.run(worlds(0, 32, 8, 1800), 2 * 3600, 30);
    assert!(r.cost_usd <= 0.10 + 1e-9, "{r:?}");
}

#[test]
fn work_your_devices_can_take_costs_nothing() {
    let mut sim = Sim::new(vec![mac(40, 48)], clouds(), base());
    let r = sim.run(worlds(0, 4, 8, 1800), 3600, 30);
    assert_eq!(r.placed, 32);
    assert_eq!(r.added, 0);
    assert_eq!(r.cost_usd, 0.0);
    assert_eq!(r.wait_max, 0);
}

#[test]
fn short_interactive_spaces_go_to_a_spaces_supplier() {
    // Five interactive one-off Spaces, 5 minutes each, nothing else
    // running: a per-second Spaces supplier (3 s start) beats an hourly host.
    let mut offers = hetzner_offers();
    offers.extend(modal_offers());
    let arrivals: Vec<Arrival> = (0..5)
        .map(|i| Arrival {
            at: i * 600,
            group: DemandGroup {
                id: format!("i{i}"),
                count: 1,
                per: Shape::new(1000, 1024, 1),
                duration_secs: 300,
                latency: Latency::Interactive,
                ..Default::default()
            },
        })
        .collect();
    let mut sim = Sim::new(vec![], offers, base());
    let r = sim.run(arrivals, 4000, 10);
    assert_eq!(r.placed, 5);
    assert!(r.wait_max <= 3, "{r:?}");
    assert!(r.cost_usd < 0.02, "{r:?}");
}
