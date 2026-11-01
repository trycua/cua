//! Trace replay of the warm-pool policy ([`cua_daemon::pool::policy`])
//! against never-warm and always-warm (one member per shape, kept 60 min
//! after the last create), with measured tier latencies and member costs.
//!
//!     cargo run -p cua-daemon --example pool_policy_sim -- [--class personal|dedicated] [--default-s S]
//!
//! The simulator drives the real `decide` function the daemon runs, ticking
//! every 2 s as the daemon does and after every create; members take
//! `--boot-s` to become ready and a create claims a ready one, else it
//! takes the default tier (`--default-s`: a gVisor golden restore, about
//! 0.7 s, or a cold boot, about 4.3 s on an M2 Max Mac). Prints one Markdown
//! table per default tier.

use cua_daemon::pool::policy::{Arrivals, Costs, HostClass, decide};
use rand::{Rng, SeedableRng, rngs::StdRng};

/// Measured on an M2 Max Mac (gVisor, linux:24.04-slim): a claim to ready,
/// a member's start to ready, and an idle member's CPU (cores) and RAM (GB).
const CLAIM_S: f64 = 0.088;
const BOOT_S: f64 = 5.0;
const IDLE_CORES: f64 = 0.15;
const IDLE_GB: f64 = 0.21;
/// A member's boot burns about a core.
const BOOT_CORES: f64 = 1.0;
const TICK_S: f64 = 2.0;

#[derive(Clone, Copy, PartialEq)]
enum Strategy {
    Never,
    Always,
    Policy,
}

impl Strategy {
    fn name(self) -> &'static str {
        match self {
            Self::Never => "never-warm",
            Self::Always => "always-warm (1, 60 min)",
            Self::Policy => "adaptive policy",
        }
    }
}

struct Outcome {
    latencies: Vec<f64>,
    member_s: f64,
    cpu_s: f64,
    gb_s: f64,
    hits: usize,
}

/// Replays `arrivals` (seconds, sorted) of one shape.
fn replay(arrivals: &[f64], strategy: Strategy, default_s: f64, class: HostClass) -> Outcome {
    let mut out = Outcome {
        latencies: Vec::new(),
        member_s: 0.0,
        cpu_s: 0.0,
        gb_s: 0.0,
        hits: 0,
    };
    let Some(&end_arrival) = arrivals.last() else {
        return out;
    };
    let horizon = end_arrival + 3600.0 * 2.0;
    // Members: the time each is (or will be) ready.
    let mut members: Vec<f64> = Vec::new();
    let mut seen = Arrivals::default();
    let mut last = f64::NEG_INFINITY;
    let costs = Costs {
        boot_s: BOOT_S,
        saved_s: (default_s - CLAIM_S).max(0.0),
    };
    let mut t = 0.0f64;
    let mut i = 0;
    // Events: ticks and arrivals, in time order.
    while t <= horizon {
        let next_tick = (t / TICK_S).floor() * TICK_S + TICK_S;
        let next = arrivals.get(i).copied().filter(|a| *a < next_tick);
        let now = next.unwrap_or(next_tick);
        // Cost of what ran since `t`.
        let dt = now - t;
        for m in &members {
            let booting = (m.min(now) - t).max(0.0).min(dt);
            out.member_s += dt;
            out.cpu_s += booting * BOOT_CORES + (dt - booting) * IDLE_CORES;
            out.gb_s += dt * IDLE_GB;
        }
        t = now;
        if next.is_some() {
            i += 1;
            seen.record(t);
            last = t;
            if let Some(k) = members.iter().position(|r| *r <= t) {
                members.remove(k);
                out.hits += 1;
                out.latencies.push(CLAIM_S);
            } else {
                out.latencies.push(default_s);
            }
        }
        let want = match strategy {
            Strategy::Never => 0,
            Strategy::Always => {
                if last.is_finite() && t - last <= 3600.0 {
                    1
                } else {
                    0
                }
            }
            Strategy::Policy => decide(&seen, t, costs, class, false, 8).members,
        } as usize;
        while members.len() > want {
            // The member that would be ready last goes first.
            let k = members
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .map(|(k, _)| k)
                .unwrap();
            members.remove(k);
        }
        while members.len() < want {
            members.push(t + BOOT_S);
        }
    }
    out
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(f64::total_cmp);
    let rank = ((p / 100.0) * v.len() as f64).ceil().max(1.0) as usize;
    v[rank.min(v.len()) - 1]
}

/// Workload traces: name and the arrivals of each shape.
fn traces(rng: &mut StdRng) -> Vec<(&'static str, Vec<Vec<f64>>)> {
    let mut v = Vec::new();
    v.push(("one-off (1 create)", vec![vec![10.0]]));
    v.push((
        "occasional (every 3 h, 24 h)",
        vec![(0..8).map(|k| 10.0 + k as f64 * 10_800.0).collect()],
    ));
    v.push((
        "periodic (every 10 min, 6 h)",
        vec![(0..36).map(|k| 10.0 + k as f64 * 600.0).collect()],
    ));
    let mut bursty = Vec::new();
    for b in 0..18 {
        let start = 10.0 + b as f64 * 1200.0 + rng.random_range(-30.0..30.0);
        for k in 0..10 {
            bursty.push(start + k as f64 * rng.random_range(0.5..2.0));
        }
    }
    bursty.sort_by(f64::total_cmp);
    v.push(("bursty (10 creates every 20 min, 6 h)", vec![bursty]));
    v.push((
        "steady batch (1 create / 4 s, 200)",
        vec![(0..200).map(|k| 10.0 + k as f64 * 4.0).collect()],
    ));
    // Azure Functions shape (Shahrad et al., ATC'20): per-app rates spread
    // over orders of magnitude, most apps rare. 40 apps, 12 h, Poisson.
    let mut apps = Vec::new();
    for _ in 0..40 {
        let mean_iat = 10f64.powf(rng.random_range(1.0..5.0)); // 10 s .. 28 h
        let mut t = rng.random_range(0.0..mean_iat.min(3600.0));
        let mut a = Vec::new();
        while t < 12.0 * 3600.0 {
            a.push(t);
            let u: f64 = rng.random_range(1e-9..1.0);
            t += -mean_iat * u.ln();
        }
        if !a.is_empty() {
            apps.push(a);
        }
    }
    v.push(("Azure-shaped mix (40 shapes, 12 h)", apps));
    v
}

fn main() {
    let mut class = HostClass::Personal;
    let mut tiers = vec![("golden restore", 0.70), ("cold boot", 4.25)];
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--class" => {
                class = match args.next().as_deref() {
                    Some("dedicated") => HostClass::Dedicated,
                    _ => HostClass::Personal,
                }
            }
            "--default-s" => {
                let s: f64 = args.next().and_then(|v| v.parse().ok()).unwrap_or(0.7);
                tiers = vec![("given", s)];
            }
            other => {
                eprintln!("unknown option {other}");
                std::process::exit(2);
            }
        }
    }
    let mut rng = StdRng::seed_from_u64(20261006);
    let traces = traces(&mut rng);
    println!(
        "Host class: {class:?}. Claim {CLAIM_S} s, member boot {BOOT_S} s, idle member \
         {IDLE_CORES} cores / {IDLE_GB} GB, tick {TICK_S} s.\n"
    );
    for (tier, default_s) in tiers {
        println!("### Default tier: {tier} ({default_s} s)\n");
        println!(
            "| workload | strategy | creates | hit % | p50 s | p95 s | member-s | CPU core-s | RAM GB-s |"
        );
        println!("|---|---|---|---|---|---|---|---|---|");
        for (name, shapes) in &traces {
            for strategy in [Strategy::Never, Strategy::Always, Strategy::Policy] {
                let mut lat = Vec::new();
                let (mut ms, mut cpu, mut gb, mut hits) = (0.0, 0.0, 0.0, 0);
                for arrivals in shapes {
                    let o = replay(arrivals, strategy, default_s, class);
                    lat.extend(o.latencies);
                    ms += o.member_s;
                    cpu += o.cpu_s;
                    gb += o.gb_s;
                    hits += o.hits;
                }
                let n = lat.len();
                println!(
                    "| {name} | {} | {n} | {:.0} | {:.3} | {:.3} | {:.0} | {:.0} | {:.0} |",
                    strategy.name(),
                    100.0 * hits as f64 / n.max(1) as f64,
                    pct(&mut lat, 50.0),
                    pct(&mut lat, 95.0),
                    ms,
                    cpu,
                    gb
                );
            }
        }
        println!();
    }
}
