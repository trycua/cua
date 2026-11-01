//! A discrete-time simulation of the elastic pool: Spaces arrive and end,
//! every arrival is placed by [`solve`], new machines boot after their
//! offer's boot time, [`scale::decide`] removes idle cloud machines at
//! their billing boundary, a machine can be lost (its Spaces are placed
//! again), and every machine's bill is kept by its billing steps. The
//! simulation tests drive the same code the pool runs, over workloads
//! (bursts, steady state, scale-in, host loss, budget limits), and the
//! benchmark uses it to project costs for workloads too large to run.

use std::collections::BTreeMap;

use super::model::*;
use super::scale::{self, Action, Arrivals};
use super::solve::solve;

/// One machine in the simulation.
#[derive(Clone, Debug)]
pub struct SimHost {
    /// As the solver sees it (free capacity updated as Spaces come and go).
    pub host: Host,
    /// Its offer (cloud machines).
    pub offer: Option<Offer>,
    /// Second it was added.
    pub started: u64,
    /// Second it is ready.
    pub ready_at: u64,
    /// Second it was removed (`None`: running).
    pub ended: Option<u64>,
    /// Second it became empty (`None`: busy).
    pub idle_since: Option<u64>,
    /// Cordoned: takes no new Spaces.
    pub cordoned: bool,
}

impl SimHost {
    /// What it cost by `now` (billing steps and minimum).
    pub fn cost(&self, now: u64) -> f64 {
        match &self.offer {
            None => 0.0,
            Some(o) => o.cost_for(self.ended.unwrap_or(now).saturating_sub(self.started)),
        }
    }
}

/// A running Space.
#[derive(Clone, Debug)]
pub struct SimSpace {
    /// Its group.
    pub group: String,
    /// Its machine.
    pub host: String,
    /// Its shape.
    pub per: Shape,
    /// Second it was asked for.
    pub asked: u64,
    /// Second it started (its machine was ready).
    pub started: u64,
    /// Second it ends.
    pub ends: u64,
}

/// One arrival: `count` Spaces of `group` at second `at`.
#[derive(Clone, Debug)]
pub struct Arrival {
    /// Second.
    pub at: u64,
    /// The demand.
    pub group: DemandGroup,
}

/// What a run measured.
#[derive(Clone, Debug, Default)]
pub struct SimReport {
    /// Spaces placed.
    pub placed: u32,
    /// Spaces that found no place.
    pub unplaced: u32,
    /// Machines added.
    pub added: u32,
    /// Machines removed by the scaler.
    pub removed: u32,
    /// Most cloud machines at once.
    pub peak_machines: u32,
    /// Total cost of the cloud machines, USD.
    pub cost_usd: f64,
    /// Space-seconds served.
    pub space_secs: u64,
    /// Wait from ask to start, seconds, per Space (p50, p95, max).
    pub wait_p50: u64,
    /// p95 wait.
    pub wait_p95: u64,
    /// Longest wait.
    pub wait_max: u64,
    /// Most USD/h the clouds cost at once.
    pub peak_usd_per_hour: f64,
    /// Spaces placed again after their machine was lost.
    pub replaced: u32,
}

impl SimReport {
    /// USD per Space-hour served.
    pub fn usd_per_space_hour(&self) -> f64 {
        if self.space_secs == 0 {
            0.0
        } else {
            self.cost_usd / (self.space_secs as f64 / 3600.0)
        }
    }
}

/// The simulated pool.
pub struct Sim {
    /// Every machine, by name.
    pub hosts: BTreeMap<String, SimHost>,
    /// What can be added.
    pub offers: Vec<Offer>,
    /// Limits (the running $/h is kept by the simulation).
    pub constraints: Constraints,
    /// Running Spaces.
    pub spaces: Vec<SimSpace>,
    /// The demand rate the scaler sees.
    pub arrivals: Arrivals,
    /// The scaler's policy.
    pub policy: scale::Policy,
    waits: Vec<u64>,
    report: SimReport,
    next: u32,
}

impl Sim {
    /// A pool of `devices` (owned machines) and `offers`.
    pub fn new(devices: Vec<Host>, offers: Vec<Offer>, constraints: Constraints) -> Self {
        let hosts = devices
            .into_iter()
            .map(|h| {
                (
                    h.name.clone(),
                    SimHost {
                        host: h,
                        offer: None,
                        started: 0,
                        ready_at: 0,
                        ended: None,
                        idle_since: Some(0),
                        cordoned: false,
                    },
                )
            })
            .collect();
        Self {
            hosts,
            offers,
            constraints,
            spaces: vec![],
            arrivals: Arrivals::default(),
            policy: scale::Policy::default(),
            waits: vec![],
            report: SimReport::default(),
            next: 1,
        }
    }

    fn live(&self) -> impl Iterator<Item = &SimHost> {
        self.hosts.values().filter(|h| h.ended.is_none())
    }

    fn running_usd(&self) -> f64 {
        self.live()
            .filter_map(|h| h.offer.as_ref())
            .map(|o| o.usd_per_hour)
            .sum()
    }

    /// The solver's view at `now` (a cordoned machine with room is opened
    /// again rather than a new one bought: see [`Sim::place`]).
    fn view(&self, now: u64) -> Vec<Host> {
        self.live()
            .filter(|h| !h.cordoned)
            .map(|h| {
                let mut x = h.host.clone();
                x.boot_secs_left = h.ready_at.saturating_sub(now) as u32;
                x.running = self.spaces.iter().filter(|s| s.host == h.host.name).count() as u32;
                if let Some(o) = &h.offer {
                    let paid_until =
                        h.started + o.billed_secs(now.saturating_sub(h.started).max(1));
                    x.paid_secs_left = paid_until.saturating_sub(now);
                }
                x
            })
            .collect()
    }

    /// Places `g` at `now`; returns how many found a place.
    pub fn place(&mut self, now: u64, g: &DemandGroup) -> u32 {
        self.arrivals.record(g.count, now);
        self.place_inner(now, g)
    }

    fn place_inner(&mut self, now: u64, g: &DemandGroup) -> u32 {
        let mut c = self.constraints.clone();
        c.running_usd_per_hour = self.running_usd();
        c.relay_used = self.live().filter(|h| h.offer.is_some()).count() as u32;
        if let Some(b) = c.budget_left_usd {
            c.budget_left_usd = Some((b - self.cost_so_far(now)).max(0.0));
        }
        // The rest of a burst is expected to look like its start: what
        // arrived within one boot before this.
        let window = self
            .offers
            .iter()
            .map(|o| u64::from(o.boot_secs))
            .max()
            .unwrap_or(60);
        let expected = super::scale::expected_more(
            &self.arrivals,
            now,
            window,
            g.duration_secs,
            self.spaces.len() as u32,
            g.count,
        );
        let mut groups = vec![g.clone()];
        groups.extend(super::solve::anticipate(g, expected, &self.offers));
        let plan = solve(&groups, &self.view(now), &self.offers, &c);
        // New machines first.
        let mut names = Vec::new();
        for n in &plan.new_hosts {
            let o = self
                .offers
                .iter()
                .find(|o| {
                    o.provider == n.provider && o.type_name == n.type_name && o.region == n.region
                })
                .cloned()
                .expect("an offer");
            let name = format!("{}-{}", o.provider, self.next);
            self.next += 1;
            let host = Host {
                name: name.clone(),
                provider: o.provider.clone(),
                region: o.region.clone(),
                type_name: o.type_name.clone(),
                free: o.capacity,
                total: o.capacity,
                arch: o.arch.clone(),
                os: "linux".into(),
                kinds: o.kinds.clone(),
                kvm: o.kvm,
                gpu: vec![],
                usd_per_hour: o.usd_per_hour,
                paid_secs_left: 0,
                billing_step_secs: o.billing_step_secs,
                running: 0,
                ready: true,
                images: vec![],
                boot_secs_left: 0,
                rtt_ms: o.rtt_ms,
            };
            self.hosts.insert(
                name.clone(),
                SimHost {
                    host,
                    started: now,
                    ready_at: now + u64::from(o.boot_secs),
                    offer: Some(o),
                    ended: None,
                    idle_since: None,
                    cordoned: false,
                },
            );
            self.report.added += 1;
            names.push(name);
        }
        if !plan.new_hosts.is_empty() {
            // Room on a draining machine beats a new one: open it again.
            let mut reopened = false;
            for h in self.hosts.values_mut() {
                if h.cordoned && h.ended.is_none() && g.per.fits(h.host.free) {
                    h.cordoned = false;
                    reopened = true;
                }
            }
            if reopened {
                // Undo the machines just added and place again.
                for n in &names {
                    self.hosts.remove(n);
                    self.report.added -= 1;
                }
                return self.place_inner(now, g);
            }
        }
        let mut placed = 0;
        for a in &plan.assignments {
            let name = match &a.target {
                Target::Host { name } => name.clone(),
                Target::New { index } => names[*index].clone(),
            };
            let h = self.hosts.get_mut(&name).expect("a host");
            h.host.free = Shape::minus(h.host.free, g.per.times(u64::from(a.count)));
            h.idle_since = None;
            let start = h.ready_at.max(now);
            for _ in 0..a.count {
                self.spaces.push(SimSpace {
                    group: g.id.clone(),
                    host: name.clone(),
                    per: g.per,
                    asked: now,
                    started: start,
                    ends: start + g.duration_secs,
                });
                self.waits.push(start - now);
            }
            placed += a.count;
        }
        self.report.placed += placed;
        self.report.unplaced += g.count - placed;
        let machines = self.live().filter(|h| h.offer.is_some()).count() as u32;
        self.report.peak_machines = self.report.peak_machines.max(machines);
        self.report.peak_usd_per_hour = self.report.peak_usd_per_hour.max(self.running_usd());
        placed
    }

    fn cost_so_far(&self, now: u64) -> f64 {
        self.hosts.values().map(|h| h.cost(now)).sum()
    }

    /// Advances to `now`: Spaces that ended free their machines, and the
    /// scaler runs.
    pub fn tick(&mut self, now: u64) {
        let mut ended = Vec::new();
        self.spaces.retain(|s| {
            if s.ends <= now {
                ended.push(s.clone());
                false
            } else {
                true
            }
        });
        for s in ended {
            self.report.space_secs += s.ends - s.started;
            if let Some(h) = self.hosts.get_mut(&s.host) {
                h.host.free = Shape::new(
                    (h.host.free.millicpus + s.per.millicpus).min(h.host.total.millicpus),
                    (h.host.free.memory_mb + s.per.memory_mb).min(h.host.total.memory_mb),
                    (h.host.free.disk_gb + s.per.disk_gb).min(h.host.total.disk_gb),
                );
            }
        }
        for h in self.hosts.values_mut() {
            let busy = self.spaces.iter().any(|s| s.host == h.host.name);
            if !busy && h.idle_since.is_none() && h.ended.is_none() {
                h.idle_since = Some(now);
            }
        }
        let cloud: Vec<scale::CloudHost> = self
            .live()
            .filter_map(|h| {
                let o = h.offer.as_ref()?;
                if h.ready_at > now {
                    return None;
                }
                let running = self.spaces.iter().filter(|s| s.host == h.host.name).count() as u32;
                Some(scale::CloudHost {
                    name: h.host.name.clone(),
                    provider: o.provider.clone(),
                    usd_per_hour: o.usd_per_hour,
                    billing_step_secs: o.billing_step_secs,
                    billing_min_secs: o.billing_min_secs,
                    started: h.started,
                    running,
                    idle_since: h.idle_since,
                    used: 1.0 - h.host.free.dominant_share(h.host.total.max_one()).min(1.0),
                    slots: 32,
                    boot_secs: o.boot_secs,
                    cordoned: h.cordoned,
                    pinned: false,
                })
            })
            .collect();
        for d in scale::decide(&cloud, &self.arrivals, now, &self.policy) {
            match d.action {
                Action::Remove { .. } => {
                    if let Some(h) = self.hosts.get_mut(&d.host) {
                        h.ended = Some(now);
                        self.report.removed += 1;
                    }
                }
                Action::Cordon { .. } => {
                    if let Some(h) = self.hosts.get_mut(&d.host) {
                        h.cordoned = true;
                    }
                }
                Action::Keep { .. } => {}
            }
        }
    }

    /// Loses machine `name` at `now`: its Spaces are placed again (with the
    /// time they had left).
    pub fn lose(&mut self, now: u64, name: &str) {
        let Some(h) = self.hosts.get_mut(name) else {
            return;
        };
        h.ended = Some(now);
        let lost: Vec<SimSpace> = self
            .spaces
            .iter()
            .filter(|s| s.host == name)
            .cloned()
            .collect();
        self.spaces.retain(|s| s.host != name);
        let mut by_group: BTreeMap<(String, u64), u32> = BTreeMap::new();
        for s in &lost {
            self.report.space_secs += now.saturating_sub(s.started);
            *by_group
                .entry((s.group.clone(), s.ends.saturating_sub(now)))
                .or_default() += 1;
        }
        for ((g, left), n) in by_group {
            let per = lost
                .iter()
                .find(|s| s.group == g)
                .map(|s| s.per)
                .unwrap_or_default();
            let group = DemandGroup {
                id: format!("{g}-again"),
                count: n,
                per,
                duration_secs: left.max(1),
                latency: Latency::Interactive,
                ..Default::default()
            };
            let placed = self.place(now, &group);
            self.report.replaced += placed;
        }
    }

    /// Runs `arrivals` (sorted by time) to `until`, ticking every `step`
    /// seconds, and returns the report.
    pub fn run(&mut self, mut arrivals: Vec<Arrival>, until: u64, step: u64) -> SimReport {
        arrivals.sort_by_key(|a| a.at);
        let mut i = 0;
        let mut t = 0;
        while t <= until {
            while i < arrivals.len() && arrivals[i].at <= t {
                let g = arrivals[i].group.clone();
                self.place(t, &g);
                i += 1;
            }
            self.tick(t);
            t += step;
        }
        self.finish(until)
    }

    /// The report at `now` (machines still running are billed to now).
    pub fn finish(&mut self, now: u64) -> SimReport {
        let mut r = self.report.clone();
        for s in &self.spaces {
            r.space_secs += now.saturating_sub(s.started);
        }
        r.cost_usd = self.cost_so_far(now);
        let mut w = self.waits.clone();
        w.sort_unstable();
        let pct = |p: f64| {
            if w.is_empty() {
                0
            } else {
                w[((w.len() - 1) as f64 * p).round() as usize]
            }
        };
        r.wait_p50 = pct(0.5);
        r.wait_p95 = pct(0.95);
        r.wait_max = w.last().copied().unwrap_or(0);
        r
    }

    /// Cloud machines running now.
    pub fn machines(&self) -> usize {
        self.live().filter(|h| h.offer.is_some()).count()
    }
}

impl Shape {
    fn max_one(self) -> Shape {
        Shape::new(
            self.millicpus.max(1),
            self.memory_mb.max(1),
            self.disk_gb.max(1),
        )
    }
}
