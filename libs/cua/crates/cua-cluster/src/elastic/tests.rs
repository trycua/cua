//! Solver tests: feasibility on random instances, optimality against brute
//! force on small ones, the time budget on large ones, and golden decisions
//! for fixed scenarios (drift guard: a change in what the solver decides
//! for these shows up as a diff of `testdata/elastic/*.json`).

use super::*;
use std::collections::BTreeMap;

/// A small deterministic generator (xorshift64*).
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545F4914F6CDD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
    fn pick<'a, T>(&mut self, v: &'a [T]) -> &'a T {
        &v[self.below(v.len() as u64) as usize]
    }
}

fn kinds() -> Vec<String> {
    vec!["container".into(), "vm".into()]
}

pub(crate) fn hetzner(t: &str, cpus: u64, mem_gb: u64, usd: f64, arch: &str) -> Offer {
    Offer {
        provider: "hetzner".into(),
        type_name: t.into(),
        region: "fsn1".into(),
        arch: arch.into(),
        capacity: Shape::new(cpus * 1000 * 4, mem_gb * 1024 - 512, 40 + cpus * 20),
        kvm: false,
        kinds: vec!["container".into()],
        gpu: String::new(),
        usd_per_hour: usd,
        billing_step_secs: 3600,
        billing_min_secs: 3600,
        boot_secs: 45,
        one_space_each: false,
        max_new: 10,
        rtt_ms: 0,
        vcpus: cpus as u32,
        quota: "hetzner/shared".into(),
    }
}

pub(crate) fn aws(t: &str, cpus: u64, mem_gb: u64, usd: f64, kvm: bool) -> Offer {
    Offer {
        provider: "aws".into(),
        type_name: t.into(),
        region: "us-west-2".into(),
        arch: "arm64".into(),
        capacity: Shape::new(cpus * 1000 * 4, mem_gb * 1024 - 1024, 100 + cpus * 10),
        kvm,
        kinds: if kvm {
            kinds()
        } else {
            vec!["container".into()]
        },
        gpu: String::new(),
        usd_per_hour: usd,
        billing_step_secs: 1,
        billing_min_secs: 60,
        boot_secs: 75,
        one_space_each: false,
        max_new: 20,
        rtt_ms: 0,
        vcpus: cpus as u32,
        quota: "aws/standard".into(),
    }
}

pub(crate) fn modal(t: &str, cpus: u64, mem_gb: u64) -> Offer {
    Offer {
        provider: "modal".into(),
        type_name: t.into(),
        region: "us-east".into(),
        arch: "amd64".into(),
        capacity: Shape::new(cpus * 1000, mem_gb * 1024, 64),
        kvm: false,
        kinds: vec!["container".into()],
        gpu: String::new(),
        // Modal list prices: $0.0000131/core-s (physical core = 2 vCPU),
        // $0.00000222/GiB-s (modal.com/pricing, 2026-10-21).
        usd_per_hour: (cpus as f64 / 2.0) * 0.0000131 * 3600.0
            + mem_gb as f64 * 0.00000222 * 3600.0,
        billing_step_secs: 1,
        billing_min_secs: 0,
        boot_secs: 3,
        one_space_each: true,
        max_new: 100,
        rtt_ms: 0,
        vcpus: cpus as u32,
        quota: String::new(),
    }
}

pub(crate) fn mac(name: &str, free_cpus: u64, free_gb: u64) -> Host {
    Host {
        name: name.into(),
        free: Shape::new(free_cpus * 1000, free_gb * 1024, 400),
        total: Shape::new(12_000 * 4, 96 * 1024, 900),
        arch: "arm64".into(),
        os: "macos".into(),
        kinds: kinds(),
        kvm: true,
        ready: true,
        ..Default::default()
    }
}

fn group(id: &str, n: u32, cpu: u64, mem: u64, dur: u64, lat: Latency) -> DemandGroup {
    DemandGroup {
        id: id.into(),
        count: n,
        per: Shape::new(cpu, mem, 1),
        duration_secs: dur,
        latency: lat,
        ..Default::default()
    }
}

fn random_instance(
    r: &mut Rng,
    small: bool,
) -> (Vec<DemandGroup>, Vec<Host>, Vec<Offer>, Constraints) {
    let ng = if small {
        1 + r.below(2)
    } else {
        1 + r.below(6)
    };
    let mut groups = Vec::new();
    let durs = [600u64, 1800, 3600, 5400];
    for i in 0..ng {
        let count = if small {
            1 + r.below(3) as u32
        } else {
            1 + r.below(40) as u32
        };
        let mut g = group(
            &format!("g{i}"),
            count,
            *r.pick(&[500u64, 1000, 2000, 4000]),
            *r.pick(&[512u64, 1024, 2048, 4096]),
            *r.pick(&durs),
            *r.pick(&[Latency::Interactive, Latency::Batch]),
        );
        g.whole = r.below(3) == 0;
        if r.below(4) == 0 {
            g.req.kind = "vm".into();
            g.req.needs_kvm = true;
        }
        if r.below(5) == 0 {
            g.req.arch = r.pick(&["amd64", "arm64"]).to_string();
        }
        groups.push(g);
    }
    let nh = if small { r.below(2) } else { r.below(5) };
    let mut hosts = Vec::new();
    for i in 0..nh {
        let mut h = mac(&format!("mac{i}"), 1 + r.below(8), 1 + r.below(16));
        if r.below(3) == 0 {
            h.provider = "aws".into();
            h.os = "linux".into();
            h.usd_per_hour = 0.1;
            h.running = r.below(2) as u32;
            h.billing_step_secs = 1;
        }
        hosts.push(h);
    }
    let all = [
        hetzner("cx23", 2, 4, 0.0114, "amd64"),
        hetzner("cax21", 4, 8, 0.0144, "arm64"),
        hetzner("ccx23", 4, 16, 0.0457, "amd64"),
        aws("c7g.large", 2, 4, 0.0725, false),
        aws("c7g.metal", 64, 128, 2.32, true),
        modal("1x1", 1, 1),
        modal("2x4", 2, 4),
    ];
    let no = if small {
        1 + r.below(3)
    } else {
        1 + r.below(7)
    };
    let mut offers: Vec<Offer> = Vec::new();
    for _ in 0..no {
        let o = r.pick(&all).clone();
        if !offers.iter().any(|x| x.key() == o.key()) {
            offers.push(o);
        }
    }
    let mut c = Constraints {
        time_budget_ms: 300,
        ..Default::default()
    };
    if r.below(4) == 0 {
        c.max_usd_per_hour = Some(0.05);
    }
    if r.below(5) == 0 {
        c.relay_limit = 3;
    }
    if r.below(6) == 0 {
        c.space_takes_relay = true;
    }
    if r.below(6) == 0 {
        c.budget_left_usd = Some(0.03);
    }
    if small {
        c.max_new_hosts = 3;
    }
    (groups, hosts, offers, c)
}

/// The best concrete plan by enumeration: every way to split each group
/// over the existing hosts, up to `slots` new machines of each offer, and
/// "unplaced".
fn brute_force(
    groups: &[DemandGroup],
    hosts: &[Host],
    offers: &[Offer],
    c: &Constraints,
    slots: usize,
) -> (f64, Plan) {
    #[derive(Clone)]
    enum T {
        H(usize),
        S(usize, usize),
    }
    let usable_o: Vec<usize> = (0..offers.len())
        .filter(|&i| {
            let o = &offers[i];
            !(c.pins.no_scale || c.pins.devices_only)
                && c.pins.provider.as_ref().is_none_or(|p| &o.provider == p)
                && c.pins.region.as_ref().is_none_or(|p| &o.region == p)
                && c.pins.type_name.as_ref().is_none_or(|p| &o.type_name == p)
        })
        .collect();
    let mut targets: Vec<T> = (0..hosts.len()).map(T::H).collect();
    for &o in &usable_o {
        for k in 0..slots {
            targets.push(T::S(o, k));
        }
    }
    let nt = targets.len();
    // Per group: every distribution of its count over targets + unplaced.
    fn comps(n: u32, parts: usize, whole: bool) -> Vec<Vec<u32>> {
        if whole {
            let mut v = Vec::new();
            for i in 0..parts {
                let mut x = vec![0; parts];
                x[i] = n;
                v.push(x);
            }
            return v;
        }
        if parts == 1 {
            return vec![vec![n]];
        }
        let mut out = Vec::new();
        for first in 0..=n {
            for mut rest in comps(n - first, parts - 1, false) {
                rest.insert(0, first);
                out.push(rest);
            }
        }
        out
    }
    let per_group: Vec<Vec<Vec<u32>>> = groups
        .iter()
        .map(|g| comps(g.count, nt + 1, g.whole))
        .collect();
    let mut best = (f64::INFINITY, Plan::default());
    let mut idx = vec![0usize; groups.len()];
    loop {
        // Build the plan.
        let mut plan = Plan::default();
        let mut slot_of: BTreeMap<(usize, usize), usize> = BTreeMap::new();
        let mut ok = true;
        for (gi, g) in groups.iter().enumerate() {
            let d = &per_group[gi][idx[gi]];
            for (ti, &n) in d[..nt].iter().enumerate() {
                if n == 0 {
                    continue;
                }
                let target = match &targets[ti] {
                    T::H(h) => Target::Host {
                        name: hosts[*h].name.clone(),
                    },
                    T::S(o, k) => {
                        // Slots of one offer are used in order (symmetry).
                        if *k > 0 && !slot_of.contains_key(&(*o, k - 1)) {
                            ok = false;
                        }
                        let len = slot_of.len();
                        let i = *slot_of.entry((*o, *k)).or_insert(len);
                        Target::New { index: i }
                    }
                };
                let holder = matches!(&targets[ti], T::H(h) if g.holders.contains(&hosts[*h].name));
                plan.assignments.push(Assignment {
                    group: g.id.clone(),
                    target,
                    count: n,
                    sync: g.sync_secs > 0 && !holder,
                });
            }
            if d[nt] > 0 {
                plan.unplaced.push(Unplaced {
                    group: g.id.clone(),
                    count: d[nt],
                    reason: String::new(),
                });
            }
        }
        if ok {
            let mut order: Vec<((usize, usize), usize)> = slot_of.into_iter().collect();
            order.sort_by_key(|(_, i)| *i);
            for ((o, _), i) in order {
                let off = &offers[o];
                let dur = plan
                    .assignments
                    .iter()
                    .filter(|a| a.target == Target::New { index: i })
                    .map(|a| {
                        groups
                            .iter()
                            .find(|g| g.id == a.group)
                            .unwrap()
                            .duration_secs
                    })
                    .max()
                    .unwrap_or(0);
                plan.new_hosts.push(NewHost {
                    provider: off.provider.clone(),
                    type_name: off.type_name.clone(),
                    region: off.region.clone(),
                    usd_per_hour: off.usd_per_hour,
                    boot_secs: off.boot_secs,
                    billed_secs: off.billed_secs(dur),
                    one_space: off.one_space_each,
                    rtt_ms: off.rtt_ms,
                });
            }
            if plan_valid(&plan, groups, hosts, offers, c).is_ok() {
                let cost = evaluate(&plan, groups, hosts, c);
                if cost < best.0 - 1e-12 {
                    best = (cost, plan);
                }
            }
        }
        // Next combination.
        let mut i = 0;
        loop {
            if i == groups.len() {
                return best;
            }
            idx[i] += 1;
            if idx[i] < per_group[i].len() {
                break;
            }
            idx[i] = 0;
            i += 1;
        }
    }
}

#[test]
fn every_plan_is_feasible_on_random_instances() {
    let mut r = Rng(0x9E3779B97F4A7C15);
    for seed in 0..400 {
        let (g, h, o, c) = random_instance(&mut r, false);
        let p = solve(&g, &h, &o, &c);
        if let Err(e) = plan_valid(&p, &g, &h, &o, &c) {
            panic!("seed {seed}: invalid plan: {e}\n{p:#?}\n{g:#?}\n{h:#?}\n{o:#?}\n{c:#?}");
        }
        assert!(
            p.lower_bound_usd <= p.projected_usd + 1e-9 || !p.unplaced.is_empty(),
            "seed {seed}: bound above cost"
        );
        // Unplaced Spaces always say why.
        assert!(
            p.unplaced.iter().all(|u| !u.reason.is_empty()),
            "seed {seed}: unplaced without a reason"
        );
    }
}

#[test]
fn exact_matches_brute_force_on_small_instances() {
    let mut r = Rng(0xD1B54A32D192ED03);
    let mut compared = 0;
    let mut greedy_worse = 0;
    let mut worst_gap: f64 = 1.0;
    for seed in 0..250 {
        let (g, h, o, c) = random_instance(&mut r, true);
        let total: u32 = g.iter().map(|x| x.count).sum();
        let (bf, bp) = brute_force(&g, &h, &o, &c, total.min(4) as usize);
        let p = solve(&g, &h, &o, &c);
        let cost = evaluate(&p, &g, &h, &c);
        plan_valid(&p, &g, &h, &o, &c).unwrap_or_else(|e| panic!("seed {seed}: {e}"));
        // Never better than the true optimum (it is a valid plan too).
        assert!(
            cost >= bf - 1e-6,
            "seed {seed}: solver {cost} below brute force {bf}?\n{p:#?}\n{bp:#?}\n{g:#?}\n{h:#?}\n{c:#?}"
        );
        if p.solver == "exact" {
            compared += 1;
            assert!(
                (cost - bf).abs() <= 1e-6 * bf.max(1.0),
                "seed {seed}: exact {cost} vs brute force {bf}\nsolver: {p:#?}\nbrute: {bp:#?}\n{g:#?}\n{h:#?}\n{o:#?}\n{c:#?}"
            );
        }
        let gp = greedy(&g, &h, &o, &c);
        let gc = evaluate(&gp, &g, &h, &c);
        if gc > bf + 1e-6 {
            greedy_worse += 1;
            worst_gap = worst_gap.max(gc / bf.max(1e-9));
        }
    }
    assert!(
        compared >= 200,
        "the exact solver ran on only {compared} of 250"
    );
    eprintln!(
        "greedy above optimum on {greedy_worse}/250 small instances, worst ratio {worst_gap:.3}"
    );
}

#[test]
fn large_bursts_solve_within_the_time_budget() {
    // 256 rollouts in 32 worlds of 8, plus interactive Spaces, over 4 Macs
    // and every offer: the exact model is too large, the greedy answers.
    let mut groups: Vec<DemandGroup> = (0..32)
        .map(|w| DemandGroup {
            whole: true,
            ..group(&format!("world-{w}"), 8, 1000, 1536, 1800, Latency::Batch)
        })
        .collect();
    groups.extend((0..50).map(|i| {
        group(
            &format!("agent-{i}"),
            1,
            2000,
            4096,
            3600,
            Latency::Interactive,
        )
    }));
    let hosts: Vec<Host> = (0..4).map(|i| mac(&format!("mac{i}"), 10, 40)).collect();
    let offers = vec![
        hetzner("cx23", 2, 4, 0.0114, "amd64"),
        hetzner("cx33", 4, 8, 0.0194, "amd64"),
        hetzner("cx43", 8, 16, 0.0354, "amd64"),
        hetzner("cax41", 16, 32, 0.0574, "arm64"),
        aws("c7g.xlarge", 4, 8, 0.145, false),
        aws("c7g.4xlarge", 16, 32, 0.58, false),
        modal("2x4", 2, 4),
    ];
    let offers: Vec<Offer> = offers
        .into_iter()
        .map(|o| Offer { max_new: 50, ..o })
        .collect();
    let c = Constraints {
        time_budget_ms: 200,
        max_usd_per_hour: Some(10.0),
        max_new_hosts: 40,
        ..Default::default()
    };
    let t = std::time::Instant::now();
    let p = solve(&groups, &hosts, &offers, &c);
    let took = t.elapsed();
    plan_valid(&p, &groups, &hosts, &offers, &c).unwrap();
    assert!(took.as_millis() < 1500, "took {took:?}");
    assert_eq!(
        p.placed() + p.unplaced.iter().map(|u| u.count).sum::<u32>(),
        306
    );
    assert!(p.unplaced.is_empty(), "{:#?}", p.unplaced);
    // Worlds stay whole.
    for g in groups.iter().filter(|g| g.whole) {
        assert_eq!(p.assignments.iter().filter(|a| a.group == g.id).count(), 1);
    }
}

#[test]
fn devices_first_then_the_cheapest_cloud() {
    let groups = vec![group("rollouts", 64, 1000, 1024, 1800, Latency::Batch)];
    let hosts = vec![mac("studio", 8, 16)];
    let offers = vec![
        aws("c7g.4xlarge", 16, 32, 0.58, false),
        hetzner("cax41", 16, 32, 0.0574, "arm64"),
    ];
    let p = solve(&groups, &hosts, &offers, &Constraints::default());
    let on_mac: u32 = p
        .assignments
        .iter()
        .filter(|a| {
            a.target
                == Target::Host {
                    name: "studio".into(),
                }
        })
        .map(|a| a.count)
        .sum();
    assert_eq!(on_mac, 8, "the Mac's free CPU takes 8 first");
    assert!(
        p.new_hosts.iter().all(|n| n.provider == "hetzner"),
        "{:#?}",
        p.new_hosts
    );
    assert!(p.unplaced.is_empty());
}

#[test]
fn caps_and_pins_are_constraints_with_reasons() {
    let groups = vec![group("burst", 40, 1000, 1024, 1800, Latency::Batch)];
    let offers = vec![hetzner("cax41", 16, 32, 0.0574, "arm64")];
    // Autoscale off: nothing new, a clear reason.
    let c = Constraints {
        pins: Pins {
            no_scale: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let p = solve(&groups, &[], &offers, &c);
    assert!(p.new_hosts.is_empty());
    assert!(
        p.unplaced[0].reason.contains("autoscale is off"),
        "{}",
        p.unplaced[0].reason
    );
    // A $/h cap that pays for one machine.
    let c = Constraints {
        max_usd_per_hour: Some(0.06),
        ..Default::default()
    };
    let p = solve(&groups, &[], &offers, &c);
    assert_eq!(p.new_hosts.len(), 1);
    assert!(
        p.unplaced[0].reason.contains("cost cap"),
        "{}",
        p.unplaced[0].reason
    );
    // Pinned type that cannot run VMs.
    let mut vm = group("vm", 1, 2000, 4096, 3600, Latency::Interactive);
    vm.req.kind = "vm".into();
    vm.req.needs_kvm = true;
    let p = solve(&[vm], &[], &offers, &Constraints::default());
    assert!(
        p.unplaced[0].reason.contains("vm"),
        "{}",
        p.unplaced[0].reason
    );
    // Relay limit: Spaces registering their own relay machine.
    let c = Constraints {
        relay_limit: 32,
        relay_used: 30,
        space_takes_relay: true,
        ..Default::default()
    };
    let p = solve(&groups, &[], &offers, &c);
    assert_eq!(p.relay_after, 32);
    assert!(
        p.unplaced.iter().any(|u| u.reason.contains("relay")),
        "{:#?}",
        p.unplaced
    );
    // Pinned count: exactly two machines.
    let c = Constraints {
        pins: Pins {
            new_hosts: Some(2),
            ..Default::default()
        },
        ..Default::default()
    };
    let p = solve(
        &[group("few", 2, 1000, 1024, 600, Latency::Batch)],
        &[],
        &offers,
        &c,
    );
    assert_eq!(p.new_hosts.len(), 2);
}

#[test]
fn billing_steps_shape_the_choice() {
    // A 5-minute job: per-second AWS beats hourly Hetzner even at a higher
    // rate once Hetzner bills the whole hour.
    let groups = vec![group("short", 16, 1000, 1024, 120, Latency::Batch)];
    let offers = vec![
        hetzner("cax41", 16, 32, 0.0574, "arm64"),
        aws("c7g.4xlarge", 16, 32, 0.58, false),
    ];
    let p = solve(&groups, &[], &offers, &Constraints::default());
    assert_eq!(p.new_hosts.len(), 1);
    // Hetzner: 0.0574 for the hour; AWS: 0.58 * 120/3600 = 0.019.
    assert_eq!(p.new_hosts[0].provider, "aws", "{:#?}", p);
    assert!((p.projected_usd - 0.58 * 120.0 / 3600.0).abs() < 1e-9);
}

#[test]
fn interactive_work_prefers_fast_capacity() {
    // One interactive Space: Modal (3 s) over a host that boots in 45 s.
    let groups = vec![group("agent", 1, 1000, 1024, 600, Latency::Interactive)];
    let offers = vec![hetzner("cx23", 2, 4, 0.0114, "amd64"), modal("1x1", 1, 1)];
    let p = solve(&groups, &[], &offers, &Constraints::default());
    assert_eq!(p.new_hosts[0].provider, "modal", "{:#?}", p);
    // As a batch of 64, the host is cheaper.
    let groups = vec![group("batch", 64, 1000, 1024, 3600, Latency::Batch)];
    let offers = vec![
        hetzner("cax41", 16, 32, 0.0574, "arm64"),
        modal("1x1", 1, 1),
    ];
    let p = solve(&groups, &[], &offers, &Constraints::default());
    assert!(
        p.new_hosts.iter().all(|n| n.provider == "hetzner"),
        "{:#?}",
        p.new_hosts
    );
}

#[test]
fn checkpoint_holders_take_their_worlds() {
    let mut g = group("world-a", 8, 1000, 1024, 1800, Latency::Batch);
    g.whole = true;
    g.holders = vec!["b".into()];
    g.sync_secs = 30;
    let hosts = vec![mac("a", 12, 32), mac("b", 12, 32)];
    let p = solve(&[g], &hosts, &[], &Constraints::default());
    assert_eq!(p.assignments.len(), 1);
    assert_eq!(p.assignments[0].target, Target::Host { name: "b".into() });
    assert!(!p.assignments[0].sync);
}

#[test]
fn golden_decisions() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/elastic");
    std::fs::create_dir_all(&dir).unwrap();
    for (name, (g, h, o, c)) in super::scenarios::all() {
        let mut p = solve(&g, &h, &o, &c);
        p.elapsed_us = 0;
        p.explain.retain(|l| !l.starts_with("exact:"));
        let got = serde_json::to_string_pretty(&p).unwrap() + "\n";
        let path = dir.join(format!("{name}.json"));
        if std::env::var_os("UPDATE_GOLDEN").is_some() {
            std::fs::write(&path, &got).unwrap();
            continue;
        }
        let want = std::fs::read_to_string(&path).unwrap_or_else(|_| {
            panic!(
                "{name}: no golden at {}; run with UPDATE_GOLDEN=1",
                path.display()
            )
        });
        assert_eq!(
            want, got,
            "{name}: the solver's decision changed; if intended, rerun with UPDATE_GOLDEN=1 and review the diff"
        );
    }
}

#[test]
fn whole_group_after_a_device_fills() {
    let mut g0 = group("g0", 1, 4000, 1024, 3600, Latency::Interactive);
    g0.req.kind = "vm".into();
    g0.req.needs_kvm = true;
    let mut g1 = group("g1", 2, 500, 1024, 5400, Latency::Interactive);
    g1.whole = true;
    let mut m = mac("mac0", 8, 2);
    m.free.memory_mb = 2048;
    let o = vec![
        aws("c7g.large", 2, 4, 0.0725, false),
        hetzner("ccx23", 4, 16, 0.0457, "amd64"),
        modal("2x4", 2, 4),
    ];
    let c = Constraints {
        max_usd_per_hour: Some(0.05),
        max_new_hosts: 3,
        ..Default::default()
    };
    let p = greedy_with(&[g0, g1], &[m], &o, &c, Opening::Ratio, true, true);
    assert!(p.unplaced.is_empty(), "{p:#?}");
}

/// On medium instances where the exact solver proves optimality, the
/// greedy's cost stays close (the measured gap is the guard).
#[test]
fn greedy_stays_close_to_the_optimum() {
    let mut r = Rng(0xA0761D6478BD642F);
    let mut ratios = Vec::new();
    for _ in 0..120 {
        let (g, h, o, mut c) = random_instance(&mut r, false);
        c.time_budget_ms = 1_000;
        let Some(model) = solve::ExactModel::build(&g, &h, &o, &c) else {
            continue;
        };
        let Ok(Some((p, true))) = model.solve_within(std::time::Duration::from_millis(1000)) else {
            continue;
        };
        let opt = evaluate(&p, &g, &h, &c);
        let gp = greedy(&g, &h, &o, &c);
        let gc = evaluate(&gp, &g, &h, &c);
        // Compare plans that place the same Spaces (unplaced penalties
        // dominate otherwise and are compared by the small-instance test).
        if gp.placed() == p.placed() && opt > 0.0 {
            ratios.push(gc / opt);
        }
    }
    ratios.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mean = ratios.iter().sum::<f64>() / ratios.len() as f64;
    let p90 = ratios[(ratios.len() * 9) / 10];
    eprintln!(
        "greedy/optimal over {} instances: mean {mean:.3}, p90 {p90:.3}, max {:.3}",
        ratios.len(),
        ratios.last().unwrap()
    );
    assert!(
        ratios.len() >= 40,
        "only {} comparable instances",
        ratios.len()
    );
    assert!(mean <= 1.25, "mean greedy gap {mean:.3}");
}

#[test]
fn a_booting_host_takes_the_next_spaces() {
    // A host added a moment ago (40 s left) takes the next batch instead
    // of a new machine being added for it.
    let mut booting = mac("hetzner-1", 64, 31);
    booting.provider = "hetzner".into();
    booting.os = "linux".into();
    booting.usd_per_hour = 0.0684;
    booting.billing_step_secs = 3600;
    booting.paid_secs_left = 3500;
    booting.boot_secs_left = 40;
    booting.running = 0;
    let groups = vec![group("next", 8, 1000, 1024, 1800, Latency::Batch)];
    let offers = vec![hetzner("cax41", 16, 32, 0.0684, "arm64")];
    let p = solve(&groups, &[booting], &offers, &Constraints::default());
    assert!(p.new_hosts.is_empty(), "{:#?}", p);
    assert_eq!(
        p.assignments[0].target,
        Target::Host {
            name: "hetzner-1".into()
        }
    );
    assert_eq!(p.ready_in_secs, 40);
}

#[test]
fn a_core_quota_bounds_what_is_bought_and_says_so() {
    // Hetzner's shared-core limit with 8 vCPUs left: one cax31 (8), never
    // a cax41 (16); the rest is refused with the reason.
    let groups = vec![group("burst", 64, 1000, 1024, 1800, Latency::Batch)];
    let offers = vec![
        hetzner("cax31", 8, 16, 0.0354, "arm64"),
        hetzner("cax41", 16, 32, 0.0684, "arm64"),
    ];
    let mut c = Constraints::default();
    c.quota_left.insert("hetzner/shared".into(), 8);
    let p = solve(&groups, &[], &offers, &c);
    assert_eq!(p.new_hosts.len(), 1, "{p:#?}");
    assert_eq!(p.new_hosts[0].type_name, "cax31");
    assert!(
        p.unplaced[0].reason.contains("core limit"),
        "{}",
        p.unplaced[0].reason
    );
    plan_valid(&p, &groups, &[], &offers, &c).unwrap();
}

#[test]
fn gpu_machines_only_for_gpu_work() {
    // A 256-Space CPU burst under a $4/h cap: CPU machines only, even when
    // a GPU machine holds more Spaces each.
    let groups = vec![group("burst", 256, 1000, 512, 1800, Latency::Batch)];
    let mut g6 = aws("g6.4xlarge", 16, 64, 1.3348, false);
    g6.gpu = "nvidia".into();
    g6.quota = "aws/g".into();
    let offers = vec![g6.clone(), aws("c7g.4xlarge", 16, 32, 0.5916, false)];
    let c = Constraints {
        max_usd_per_hour: Some(4.0),
        ..Default::default()
    };
    let p = solve(&groups, &[], &offers, &c);
    assert!(
        p.new_hosts.iter().all(|n| n.type_name == "c7g.4xlarge"),
        "{p:#?}"
    );
    plan_valid(&p, &groups, &[], &offers, &c).unwrap();
    // Work that asks for a GPU gets the GPU machine.
    let mut gpu = group("train", 1, 4000, 16384, 3600, Latency::Batch);
    gpu.req.gpu = "nvidia".into();
    let p = solve(&[gpu], &[], &offers, &c);
    assert_eq!(p.new_hosts.len(), 1, "{p:#?}");
    assert_eq!(p.new_hosts[0].type_name, "g6.4xlarge");
}

/// Inputs captured from a live `cua cloud plan --json` (AWS us-west-2 linked,
/// `--provider aws`, 256 CPU Spaces of 1 vCPU / 512 MiB, $4/h cap).
fn live_input(name: &str) -> (Vec<DemandGroup>, Vec<Host>, Vec<Offer>, Constraints) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("testdata/elastic-inputs")
        .join(name);
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    (
        serde_json::from_value(v["groups"].clone()).unwrap(),
        serde_json::from_value(v["hosts"].clone()).unwrap(),
        serde_json::from_value(v["offers"].clone()).unwrap(),
        serde_json::from_value(v["constraints"].clone()).unwrap(),
    )
}

#[test]
fn a_live_aws_burst_fits_its_cap_on_cpu_machines() {
    // Live, this once bought two GPU machines and left 34 Spaces unplaced
    // although six c7g.4xlarge ($3.55/h) hold all 256.
    let (g, h, o, c) = live_input("aws-cpu-burst-256.json");
    let p = solve(&g, &h, &o, &c);
    plan_valid(&p, &g, &h, &o, &c).unwrap();
    assert!(p.unplaced.is_empty(), "{:#?}", p.unplaced);
    for n in &p.new_hosts {
        let offer = o.iter().find(|x| x.type_name == n.type_name).unwrap();
        assert!(offer.gpu.is_empty(), "{} for CPU work", n.type_name);
    }
    assert!(p.added_usd_per_hour <= 4.0, "{}", p.added_usd_per_hour);
    // Even if every offer could run it: ten machines at most (the policy's
    // limit) means bigger ones, not ten small cheap ones and 196 unplaced.
    let mut o = o;
    for x in o.iter_mut() {
        x.gpu.clear();
    }
    let p = solve(&g, &h, &o, &c);
    plan_valid(&p, &g, &h, &o, &c).unwrap();
    assert!(p.unplaced.is_empty(), "{:#?}", p.unplaced);
    assert!(p.new_hosts.len() <= 10);
}

#[test]
fn a_near_region_wins_at_a_similar_price() {
    // The same price in Germany (130 ms) and Oregon (19 ms): Oregon.
    let groups = vec![group("rollouts", 32, 1000, 1024, 3600, Latency::Batch)];
    let far = Offer {
        region: "fsn1".into(),
        rtt_ms: 130,
        ..hetzner("cax41", 16, 32, 0.0684, "arm64")
    };
    let near = Offer {
        region: "hil".into(),
        rtt_ms: 19,
        ..hetzner("cax41", 16, 32, 0.0700, "arm64")
    };
    let p = solve(&groups, &[], &[far, near], &Constraints::default());
    assert!(
        p.new_hosts.iter().all(|n| n.region == "hil"),
        "{:#?}",
        p.new_hosts
    );
}

#[test]
fn one_small_vm_does_not_buy_a_metal_host_unless_allowed() {
    // J-E5: a lone default `vm` request bought a c6g.metal at $2.19/h.
    let mut vm = group("one", 1, 2000, 4096, 600, Latency::Interactive);
    vm.req.kind = "vm".into();
    vm.req.needs_kvm = true;
    let offers = vec![
        aws("c6g.metal", 64, 128, 2.1760, true),
        aws("t4g.medium", 2, 4, 0.0336, false),
    ];
    let c = Constraints::default();
    let p = solve(std::slice::from_ref(&vm), &[], &offers, &c);
    assert!(p.new_hosts.is_empty(), "{p:#?}");
    let why = &p.unplaced[0].reason;
    assert!(
        why.contains("allow_metal") && why.contains("metal"),
        "{why}"
    );
    // The opt-in buys it.
    vm.req.allow_metal = true;
    let p = solve(std::slice::from_ref(&vm), &[], &offers, &c);
    assert_eq!(p.new_hosts[0].type_name, "c6g.metal", "{p:#?}");
    // A whole group of forks still buys a KVM host without the opt-in.
    let mut forks = group("forks", 4, 2000, 4096, 600, Latency::Batch);
    forks.req.kind = "vm".into();
    forks.req.needs_kvm = true;
    forks.whole = true;
    let p = solve(&[forks], &[], &offers, &c);
    assert_eq!(p.new_hosts[0].type_name, "c6g.metal", "{p:#?}");
    // A burst that fills a quarter of the host also may.
    let mut burst = group("burst", 20, 2000, 4096, 600, Latency::Batch);
    burst.req.kind = "vm".into();
    burst.req.needs_kvm = true;
    let p = solve(&[burst], &[], &offers, &c);
    assert_eq!(p.new_hosts[0].type_name, "c6g.metal", "{p:#?}");
}

#[test]
fn a_lone_interactive_space_is_sized_with_a_small_machine() {
    // One Space asked for, 8 h lifetime, nothing before it. The arrival
    // just recorded is not a steady rate: nothing more is expected, so the
    // solve buys the small machine, not a 16 vCPU one for 30 more Spaces.
    let g = group("lone", 1, 1000, 1024, 8 * 3600, Latency::Interactive);
    let offers = vec![
        hetzner("cx23", 2, 4, 0.0114, "amd64"),
        hetzner("cx43", 8, 16, 0.0354, "amd64"),
        hetzner("cax41", 16, 32, 0.0574, "arm64"),
    ];
    let mut arrivals = scale::Arrivals::default();
    arrivals.record(1, 1_000_000);
    let more = scale::expected_more(&arrivals, 1_000_000, 60, g.duration_secs, 0, g.count);
    assert_eq!(more, 0);
    let mut solved = vec![g.clone()];
    solved.extend(solve::anticipate(&g, more, &offers));
    let p = solve(&solved, &[], &offers, &Constraints::default());
    assert_eq!(p.new_hosts.len(), 1, "{p:#?}");
    assert_eq!(p.new_hosts[0].type_name, "cx23", "{p:#?}");
    // A real pattern (a Space every minute) is still anticipated.
    for i in 1..6 {
        arrivals.record(1, 1_000_000 + i * 60);
    }
    let more = scale::expected_more(&arrivals, 1_000_300, 60, 3600, 0, 1);
    assert!(more > 0);
}
