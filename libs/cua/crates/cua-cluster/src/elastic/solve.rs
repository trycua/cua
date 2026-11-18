//! One solve for placement and scaling: which existing machines take the
//! pending Spaces (the user's devices first, since they cost nothing, then
//! cloud hosts with room), and which machines to add (provider × type ×
//! region) at the least cost within the limits.
//!
//! ## Model
//!
//! Items are the pending groups: a `whole` group (the forks of one world)
//! is one item that goes to one machine; any other group may be split.
//! Targets are the existing machines and "slots" of every offer (up to as
//! many new machines of it as could be needed).
//!
//! minimise  Σ_slot price·billed(level)·lvl[slot,level]                (new machines)
//!         + Σ_h ext_h·w_h                                             (idle cloud hosts kept longer)
//!         + Σ_g M·u_g                                                 (unplaced)
//!         + Σ_g,t (latency_g·boot_t + sync_g,t + ε_t)·x[g,t]          (soft terms)
//! subject to
//!   Σ_t x[g,t] + u_g = n_g                                            (every Space placed or unplaced)
//!   Σ_g per_g·x[g,t] ≤ cap_t·open_t      for each dimension            (capacity; open_t = 1 for existing)
//!   x[g,t] ≤ n_g·a[g,t];  whole g: Σ_t a[g,t] ≤ 1, x[g,t] = n_g·a[g,t]
//!   a[g,slot] ≤ Σ_{level ≥ billed(D_g)} lvl[slot,level];  Σ_level lvl[slot,·] = open_slot
//!   open[o,k] ≥ open[o,k+1]                                           (symmetry)
//!   Σ open ≤ max new (per provider and total); Σ open·price ≤ $/h cap; Σ cost ≤ budget left
//!   relay: Σ open + [Spaces take relay]·Σ placed ≤ relay left
//!   pins: offers filtered (provider/region/type), Σ open = N, or no offers
//!
//! This is exact for the model (per-machine packing, billing steps and
//! minimums, per-machine durations). It is solved with a pure-Rust MILP
//! (good_lp + microlp) when the instance is small and the time budget
//! allows; otherwise, and always as the incumbent, the greedy below runs:
//!
//! **Greedy.** Existing machines first (devices, then holders of the
//! checkpoint, then cloud hosts already busy, best fit), then new
//! machines: repeatedly open the offer with the lowest cost per unit of
//! work it takes (each candidate packs the remaining items first-fit
//! decreasing), and fill it. For one machine type, any-fit packing of
//! d-dimensional items uses at most (d + 0.7)·OPT machines (Garey, Graham,
//! Johnson, Yao 1976; d = 3 here), so greedy ≤ 3.7·OPT + one machine's
//! cost; every plan also reports a lower bound and the tests measure the
//! real gap against the exact solver and brute force.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use super::model::*;

/// Penalty per unplaced Space, USD (larger than any machine).
pub const UNPLACED_USD: f64 = 1000.0;
/// What a second of waiting is worth for an interactive Space, USD.
pub const INTERACTIVE_USD_PER_SEC: f64 = 0.0005;
/// ... for a batch Space.
pub const BATCH_USD_PER_SEC: f64 = 0.00002;
/// What a millisecond of round trip is worth per interactive Space-hour,
/// USD (every call of an agent pays it); a Space is counted for at least a
/// quarter hour.
pub const INTERACTIVE_USD_PER_RTT_MS: f64 = 0.0002;
/// ... per batch Space-hour (a rollout's steps pay it).
pub const BATCH_USD_PER_RTT_MS: f64 = 0.00005;
/// Extra per Space started past the interactive SLO, USD.
pub const SLO_MISS_USD: f64 = 0.5;
/// What a second of copying a checkpoint costs, USD.
pub const SYNC_USD_PER_SEC: f64 = 0.00005;
/// Tie-breaker that prefers the user's devices over cloud hosts, USD.
pub const CLOUD_EPS_USD: f64 = 0.000_001;
/// Above this many integer variables the exact solver is not tried.
pub const EXACT_MAX_VARS: usize = 700;

/// One machine a plan uses, while building it.
#[derive(Clone, Debug)]
struct Bin {
    /// Offer index for a new machine.
    offer: Option<usize>,
    room: Shape,
    /// Longest duration of what it got (new machines and idle hosts).
    max_dur: u64,
    /// Images it holds (base disk paid once).
    images: BTreeSet<String>,
}

/// Why `g` cannot run on existing host `h` (`None`: it can).
pub fn host_misfit(g: &DemandGroup, h: &Host) -> Option<String> {
    if !g.pin_host.is_empty() && g.pin_host != h.name {
        return Some(format!("{} is not {}", h.name, g.pin_host));
    }
    if !h.ready {
        return Some(format!("{} is not ready", h.name));
    }
    common_misfit(
        &g.req,
        &h.name,
        &h.os,
        &h.arch,
        &h.kinds,
        h.kvm,
        &h.gpu,
        !h.owned(),
    )
}

/// Why `g` cannot run on a new machine of `o` (`None`: it can).
pub fn offer_misfit(g: &DemandGroup, o: &Offer) -> Option<String> {
    if !g.pin_host.is_empty() {
        return Some(format!("pinned to {}", g.pin_host));
    }
    let gpu: Vec<String> = if o.gpu.is_empty() {
        vec![]
    } else {
        vec![o.gpu.clone()]
    };
    // A GPU machine is bought only for work that needs one: it costs several
    // times a CPU machine of its size and draws on a scarcer quota.
    if !o.gpu.is_empty() && g.req.gpu.is_empty() {
        return Some(format!(
            "{} is a GPU machine; these Spaces need no GPU",
            o.key()
        ));
    }
    if let Some(m) = common_misfit(
        &g.req,
        &o.key(),
        "linux",
        &o.arch,
        &o.kinds,
        o.kvm,
        &gpu,
        true,
    ) {
        return Some(m);
    }
    let unit = if g.whole {
        g.per.times(u64::from(g.count)).plus_disk(g.req.base_gb)
    } else {
        g.per.plus_disk(g.req.base_gb)
    };
    if !unit.fits(o.capacity) {
        return Some(format!(
            "{} is too small ({} mCPU, {} MiB, {} GiB for {} mCPU, {} MiB, {} GiB)",
            o.key(),
            o.capacity.millicpus,
            o.capacity.memory_mb,
            o.capacity.disk_gb,
            unit.millicpus,
            unit.memory_mb,
            unit.disk_gb
        ));
    }
    // A KVM metal host is bought for a whole-host batch, not for one small
    // VM: it is the only reason to pick it, and it costs dollars an hour.
    if g.req.needs_kvm && o.kvm && !g.req.allow_metal && !g.whole && !fills_metal(g, o, unit) {
        return Some(format!(
            "{} is a KVM metal host (${:.2}/h) and this VM request is too small to need a whole host; \
             use kind=container, or allow metal hosts (`cua cloud budget --allow-metal on`, cloud_budget allow_metal)",
            o.key(),
            o.usd_per_hour
        ));
    }
    if o.one_space_each && g.whole && g.count > 1 {
        return Some(format!(
            "{} runs one Space per machine; this group shares one",
            o.provider
        ));
    }
    None
}

/// A request's total takes at least a quarter of `o` in some dimension.
fn fills_metal(g: &DemandGroup, o: &Offer, unit: Shape) -> bool {
    let total = Shape::new(
        unit.millicpus * u64::from(g.count),
        unit.memory_mb * u64::from(g.count),
        unit.disk_gb * u64::from(g.count),
    );
    total.dominant_share(o.capacity) >= 0.25
}

#[allow(clippy::too_many_arguments)]
fn common_misfit(
    r: &Requirements,
    name: &str,
    os: &str,
    arch: &str,
    kinds: &[String],
    kvm: bool,
    gpu: &[String],
    cloud: bool,
) -> Option<String> {
    if cloud && r.private_image {
        return Some(format!(
            "{name} cannot pull an image from a registry on your own network"
        ));
    }
    if !r.host_os.is_empty() && !os.is_empty() && r.host_os != os {
        return Some(format!("{name} runs {os}, not {}", r.host_os));
    }
    if !r.arch.is_empty() && !arch.is_empty() && norm_arch(&r.arch) != norm_arch(arch) {
        return Some(format!("{name} is {arch}, not {}", r.arch));
    }
    let kind = if r.kind.is_empty() {
        "container"
    } else {
        &r.kind
    };
    if !kinds.is_empty() && !kinds.iter().any(|k| k == kind) {
        return Some(format!("{name} does not create {kind} Spaces"));
    }
    if r.needs_kvm && !kvm {
        return Some(format!("{name} has no hypervisor for VMs (no /dev/kvm)"));
    }
    if !r.gpu.is_empty()
        && !gpu
            .iter()
            .any(|g| g == &r.gpu || g.starts_with(&format!("{}:", r.gpu)) || r.gpu == "gpu")
    {
        return Some(format!("{name} has no {} accelerator", r.gpu));
    }
    None
}

/// `x86_64` → `amd64`, `aarch64` → `arm64`.
pub fn norm_arch(a: &str) -> &str {
    match a {
        "x86_64" | "amd64" | "x64" => "amd64",
        "aarch64" | "arm64" => "arm64",
        o => o,
    }
}

impl Shape {
    fn plus_disk(self, gb: u64) -> Shape {
        Shape::new(self.millicpus, self.memory_mb, self.disk_gb + gb)
    }
}

/// The offers the pins and limits leave.
fn usable_offers<'a>(offers: &'a [Offer], c: &Constraints) -> Vec<(usize, &'a Offer)> {
    if c.pins.no_scale || c.pins.devices_only || c.max_new_hosts == 0 {
        return vec![];
    }
    offers
        .iter()
        .enumerate()
        .filter(|(_, o)| c.pins.provider.as_ref().is_none_or(|p| &o.provider == p))
        .filter(|(_, o)| c.pins.region.as_ref().is_none_or(|r| &o.region == r))
        .filter(|(_, o)| c.pins.type_name.as_ref().is_none_or(|t| &o.type_name == t))
        .filter(|(_, o)| o.max_new > 0 && o.usd_per_hour >= 0.0)
        .collect()
}

fn usable_hosts<'a>(hosts: &'a [Host], c: &Constraints) -> Vec<(usize, &'a Host)> {
    hosts
        .iter()
        .enumerate()
        .filter(|(_, h)| !c.pins.devices_only || h.owned())
        .collect()
}

fn rtt_usd(g: &DemandGroup) -> f64 {
    if g.soft {
        return 0.0;
    }
    let hours = (g.duration_secs as f64 / 3600.0).max(0.25);
    hours
        * match g.latency {
            Latency::Interactive => INTERACTIVE_USD_PER_RTT_MS,
            Latency::Batch | Latency::Steady => BATCH_USD_PER_RTT_MS,
        }
}

fn latency_usd(g: &DemandGroup) -> f64 {
    if g.soft {
        return 0.0;
    }
    match g.latency {
        Latency::Interactive => INTERACTIVE_USD_PER_SEC,
        Latency::Batch => BATCH_USD_PER_SEC,
        Latency::Steady => BATCH_USD_PER_SEC,
    }
}

/// Seconds an idle cloud host must be kept beyond what is paid to run
/// `dur` more, billed in its steps (`step` seconds).
fn extension_cost(h: &Host, dur: u64, step: u64) -> f64 {
    if h.owned() || h.running > 0 {
        return 0.0;
    }
    let extra = dur.saturating_sub(h.paid_secs_left);
    if extra == 0 {
        return 0.0;
    }
    let step = step.max(1);
    h.usd_per_hour * (extra.div_ceil(step) * step) as f64 / 3600.0
}

/// The true cost of a concrete plan under the model (the same function
/// scores the greedy, the exact solver and brute force).
pub fn evaluate(plan: &Plan, groups: &[DemandGroup], hosts: &[Host], c: &Constraints) -> f64 {
    let by_id: BTreeMap<&str, &DemandGroup> = groups.iter().map(|g| (g.id.as_str(), g)).collect();
    let mut cost = 0.0;
    for n in &plan.new_hosts {
        cost += n.usd_per_hour * n.billed_secs as f64 / 3600.0;
    }
    let mut host_dur: BTreeMap<&str, u64> = BTreeMap::new();
    for a in &plan.assignments {
        let g = by_id[a.group.as_str()];
        let (boot, cloud, rtt) = match &a.target {
            Target::Host { name } => {
                let d = host_dur.entry(name.as_str()).or_default();
                *d = (*d).max(g.duration_secs);
                let h = hosts.iter().find(|h| &h.name == name);
                (
                    h.map_or(0, |h| h.boot_secs_left),
                    h.is_some_and(|h| !h.owned()),
                    h.map_or(0, |h| h.rtt_ms),
                )
            }
            Target::New { index } => {
                let n = &plan.new_hosts[*index];
                (n.boot_secs, true, n.rtt_ms)
            }
        };
        let n = f64::from(a.count);
        cost += n * rtt_usd(g) * f64::from(rtt);
        cost += n * latency_usd(g) * f64::from(boot);
        if boot > c.interactive_slo_secs && g.latency == Latency::Interactive && !g.soft {
            cost += n * SLO_MISS_USD;
        }
        if a.sync {
            cost += SYNC_USD_PER_SEC * f64::from(g.sync_secs);
        }
        if cloud {
            cost += n * CLOUD_EPS_USD;
        }
    }
    for (name, d) in host_dur {
        if let Some(h) = hosts.iter().find(|h| h.name == name) {
            cost += extension_cost(h, d, h_step(h));
        }
    }
    for u in &plan.unplaced {
        let g = by_id.get(u.group.as_str());
        cost += f64::from(u.count) * g.map_or(UNPLACED_USD, |g| unplaced_usd(g));
    }
    cost
}

/// What leaving one Space of `g` without a place costs in the model.
fn unplaced_usd(g: &DemandGroup) -> f64 {
    if g.soft {
        g.soft_usd.max(0.0)
    } else {
        UNPLACED_USD
    }
}

/// What expected demand is worth against its cheapest share of a machine:
/// below 1, so it makes the machines bought for real demand bigger when that
/// is cheaper per Space, and never buys a machine by itself.
pub const SOFT_VALUE: f64 = 0.9;

/// What room for one Space of `per` is worth now when more like it are
/// expected: [`SOFT_VALUE`] x the cheapest share of a machine that would hold it for
/// `duration_secs` (whole groups of `count` count as one item). Buying now
/// wins when it is no dearer than buying again later.
pub fn soft_value(per: Shape, whole_count: u32, duration_secs: u64, offers: &[Offer]) -> f64 {
    let unit = per.times(u64::from(whole_count.max(1)));
    offers
        .iter()
        .filter(|o| !o.one_space_each)
        .filter_map(|o| {
            let n = o.capacity.how_many(unit);
            (n > 0 && n < u64::MAX).then(|| o.cost_for(duration_secs) / n as f64)
        })
        .fold(f64::INFINITY, f64::min)
        .min(1e6)
        * SOFT_VALUE
        / f64::from(whole_count.max(1))
}

fn h_step(h: &Host) -> u64 {
    h.billing_step_secs.max(1)
}

/// Solves placement and scaling for `groups`.
pub fn solve(groups: &[DemandGroup], hosts: &[Host], offers: &[Offer], c: &Constraints) -> Plan {
    let start = Instant::now();
    let mut best = greedy(groups, hosts, offers, c);
    best.solver = "greedy".into();
    let greedy_cost = evaluate(&best, groups, hosts, c);
    let mut explain_exact = None;
    if let Some(model) = ExactModel::build(groups, hosts, offers, c) {
        let budget = Duration::from_millis(c.time_budget_ms.max(1));
        match model.solve_within(budget) {
            Ok(Some((p, optimal))) => {
                let exact_cost = evaluate(&p, groups, hosts, c);
                if exact_cost <= greedy_cost + 1e-9
                    && plan_valid(&p, groups, hosts, offers, c).is_ok()
                {
                    best = p;
                    best.solver = if optimal { "exact" } else { "exact-incumbent" }.into();
                }
                explain_exact = Some(format!(
                    "exact{}: ${exact_cost:.4} vs greedy ${greedy_cost:.4} (model cost incl. latency terms)",
                    if optimal {
                        ""
                    } else {
                        " (time limit, best found)"
                    }
                ));
            }
            Ok(None) => {
                explain_exact = Some(format!(
                    "exact solver found nothing within its {} ms budget; greedy plan",
                    budget.as_millis()
                ))
            }
            Err(e) => explain_exact = Some(format!("exact solver failed ({e}); greedy plan")),
        }
    }
    // Expected demand shaped the machines bought; it is not placed.
    let soft: std::collections::BTreeSet<&str> = groups
        .iter()
        .filter(|g| g.soft)
        .map(|g| g.id.as_str())
        .collect();
    if !soft.is_empty() {
        best.assignments
            .retain(|a| !soft.contains(a.group.as_str()));
        best.unplaced.retain(|u| !soft.contains(u.group.as_str()));
        best.explain
            .retain(|l| !soft.iter().any(|g| l.contains(&format!(" x {g} "))));
    }
    best.lower_bound_usd = lower_bound(
        &groups
            .iter()
            .filter(|g| !g.soft)
            .cloned()
            .collect::<Vec<_>>(),
        hosts,
        offers,
        c,
    );
    best.added_usd_per_hour = best.new_hosts.iter().map(|n| n.usd_per_hour).sum::<f64>() + 0.0;
    best.projected_usd = best
        .new_hosts
        .iter()
        .map(|n| n.usd_per_hour * n.billed_secs as f64 / 3600.0)
        .sum::<f64>()
        + 0.0;
    best.relay_after = c.relay_used + relay_added_with(&best, c.space_takes_relay);
    best.ready_in_secs = best
        .assignments
        .iter()
        .map(|a| match a.target {
            Target::New { index } => best.new_hosts[index].boot_secs,
            Target::Host { ref name } => hosts
                .iter()
                .find(|h| &h.name == name)
                .map_or(0, |h| h.boot_secs_left),
        })
        .max()
        .unwrap_or(0);
    if let Some(e) = explain_exact {
        best.explain.push(e);
    }
    best.elapsed_us = start.elapsed().as_micros() as u64;
    best
}

/// Checks a plan against the limits (used on plans an agent edited before
/// applying them, and on every solver answer in tests).
pub fn plan_valid(
    p: &Plan,
    groups: &[DemandGroup],
    hosts: &[Host],
    offers: &[Offer],
    c: &Constraints,
) -> Result<(), String> {
    let by_id: BTreeMap<&str, &DemandGroup> = groups.iter().map(|g| (g.id.as_str(), g)).collect();
    let mut placed: BTreeMap<&str, u32> = BTreeMap::new();
    let mut used_host: BTreeMap<&str, Shape> = BTreeMap::new();
    let mut used_new: BTreeMap<usize, (Shape, BTreeSet<String>)> = BTreeMap::new();
    let mut whole_targets: BTreeMap<&str, usize> = BTreeMap::new();
    let mut spaces_on_new: BTreeMap<usize, u32> = BTreeMap::new();
    for a in &p.assignments {
        let g = by_id
            .get(a.group.as_str())
            .ok_or_else(|| format!("unknown group {}", a.group))?;
        *placed.entry(&g.id).or_default() += a.count;
        if g.whole {
            *whole_targets.entry(&g.id).or_default() += 1;
        }
        let need = g.per.times(u64::from(a.count));
        match &a.target {
            Target::Host { name } => {
                let h = hosts
                    .iter()
                    .find(|h| &h.name == name)
                    .ok_or_else(|| format!("unknown host {name}"))?;
                if let Some(m) = host_misfit(g, h) {
                    return Err(m);
                }
                if c.pins.devices_only && !h.owned() {
                    return Err(format!("{name} is not one of your devices"));
                }
                let u = used_host.entry(name.as_str()).or_default();
                *u = add(*u, need);
            }
            Target::New { index } => {
                let n = p
                    .new_hosts
                    .get(*index)
                    .ok_or_else(|| format!("unknown new host {index}"))?;
                let o = offers
                    .iter()
                    .find(|o| {
                        o.provider == n.provider
                            && o.type_name == n.type_name
                            && o.region == n.region
                    })
                    .ok_or_else(|| {
                        format!("no offer {}/{}/{}", n.provider, n.type_name, n.region)
                    })?;
                if let Some(m) = offer_misfit(g, o) {
                    return Err(m);
                }
                let e = used_new.entry(*index).or_default();
                e.0 = add(e.0, need);
                if g.req.base_gb > 0 && e.1.insert(g.req.image.clone()) {
                    e.0.disk_gb += g.req.base_gb;
                }
                if !e.0.fits(o.capacity) {
                    return Err(format!("new host {index} ({}) is over capacity", o.key()));
                }
                *spaces_on_new.entry(*index).or_default() += a.count;
                if o.one_space_each && spaces_on_new[index] > 1 {
                    return Err(format!("{} runs one Space per machine", o.provider));
                }
            }
        }
    }
    for (name, u) in used_host {
        let h = hosts.iter().find(|h| h.name == name).unwrap();
        if !u.fits(h.free) {
            return Err(format!("{name} is over its free capacity"));
        }
    }
    for g in groups {
        let un: u32 = p
            .unplaced
            .iter()
            .filter(|u| u.group == g.id)
            .map(|u| u.count)
            .sum();
        if !g.soft && placed.get(g.id.as_str()).copied().unwrap_or(0) + un != g.count {
            return Err(format!("group {} is not fully accounted for", g.id));
        }
        if g.whole && whole_targets.get(g.id.as_str()).copied().unwrap_or(0) > 1 {
            return Err(format!("group {} is split", g.id));
        }
    }
    let new = p.new_hosts.len() as u32;
    let new_hosts = p.new_hosts.iter().filter(|n| !n.one_space).count() as u32;
    if c.pins.no_scale && new > 0 {
        return Err("autoscale is off: no new machines".into());
    }
    if let Some(n) = c.pins.new_hosts
        && new_hosts != n
        && p.unplaced.is_empty()
    {
        return Err(format!(
            "{n} new machines were pinned, the plan adds {new_hosts}"
        ));
    }
    if new_hosts > c.max_new_hosts {
        return Err(format!("more than {} new machines", c.max_new_hosts));
    }
    let relay = c.relay_used + relay_added_with(p, c.space_takes_relay);
    if relay > c.relay_limit {
        return Err(format!(
            "{relay} relay machines (the account allows {})",
            c.relay_limit
        ));
    }
    let rate: f64 = p.new_hosts.iter().map(|n| n.usd_per_hour).sum();
    if let Some(m) = c.max_usd_per_hour
        && c.running_usd_per_hour + rate > m + 1e-9
    {
        return Err(format!(
            "${:.3}/h is over the ${m:.2}/h cap",
            c.running_usd_per_hour + rate
        ));
    }
    let spend: f64 = p
        .new_hosts
        .iter()
        .map(|n| n.usd_per_hour * n.billed_secs as f64 / 3600.0)
        .sum();
    if let Some(b) = c.budget_left_usd
        && spend > b + 1e-9
    {
        return Err(format!(
            "${spend:.3} is over the ${b:.2} left in the budget"
        ));
    }
    let mut per_pool: BTreeMap<String, u32> = BTreeMap::new();
    for n in &p.new_hosts {
        if let Some(o) = offers.iter().find(|o| {
            o.provider == n.provider && o.type_name == n.type_name && o.region == n.region
        }) && !o.quota.is_empty()
        {
            *per_pool.entry(o.quota.clone()).or_default() += o.vcpus;
        }
    }
    for (pool, used) in per_pool {
        if let Some(left) = c.quota_left.get(&pool)
            && used > *left
        {
            return Err(format!("{used} vCPUs in {pool}; {left} left"));
        }
    }
    let mut per_provider: BTreeMap<&str, u32> = BTreeMap::new();
    for n in &p.new_hosts {
        *per_provider.entry(n.provider.as_str()).or_default() += 1;
    }
    for (prov, k) in per_provider {
        let max = offers
            .iter()
            .filter(|o| o.provider == prov)
            .map(|o| o.max_new)
            .min()
            .unwrap_or(u32::MAX);
        if k > max {
            return Err(format!("{k} new {prov} machines; it allows {max} more"));
        }
    }
    Ok(())
}

/// Relay machines a plan registers: one per new machine (a Spaces
/// supplier's Space is one), and, when Spaces on hosts register their own,
/// one per Space placed on a host.
pub fn relay_added_with(p: &Plan, space_takes_relay: bool) -> u32 {
    let on_hosts: u32 = p
        .assignments
        .iter()
        .filter(|a| match a.target {
            Target::Host { .. } => true,
            Target::New { index } => !p.new_hosts[index].one_space,
        })
        .map(|a| a.count)
        .sum();
    p.new_hosts.len() as u32 + if space_takes_relay { on_hosts } else { 0 }
}

fn add(a: Shape, b: Shape) -> Shape {
    Shape::new(
        a.millicpus + b.millicpus,
        a.memory_mb + b.memory_mb,
        a.disk_gb + b.disk_gb,
    )
}

/// A lower bound on the cost of new machines for the work that existing
/// free capacity cannot take, ignoring which machine can run what and
/// packing (so it holds for every plan): per dimension, the overflow times
/// the cheapest price per unit of that dimension, for the shortest work.
pub fn lower_bound(
    groups: &[DemandGroup],
    hosts: &[Host],
    offers: &[Offer],
    c: &Constraints,
) -> f64 {
    let offers = usable_offers(offers, c);
    if offers.is_empty() || groups.is_empty() {
        return 0.0;
    }
    let demand = groups.iter().fold(Shape::default(), |s, g| {
        add(s, g.per.times(u64::from(g.count)))
    });
    let free = usable_hosts(hosts, c)
        .iter()
        .fold(Shape::default(), |s, (_, h)| add(s, h.free));
    let over = Shape::minus(demand, free);
    let dur = groups.iter().map(|g| g.duration_secs).min().unwrap_or(0) as f64 / 3600.0;
    let per = |f: fn(&Shape) -> u64| {
        offers
            .iter()
            .filter(|(_, o)| f(&o.capacity) > 0)
            .map(|(_, o)| o.usd_per_hour / f(&o.capacity) as f64)
            .fold(f64::INFINITY, f64::min)
    };
    let cpu = over.millicpus as f64 * per(|s| s.millicpus);
    let mem = over.memory_mb as f64 * per(|s| s.memory_mb);
    let v = cpu.max(mem) * dur;
    if v.is_finite() { v + 0.0 } else { 0.0 }
}

// ---------------------------------------------------------------- greedy

/// An item: some Spaces of one group that move together.
#[derive(Clone, Debug)]
struct Item {
    g: usize,
    count: u32,
}

fn item_need(g: &DemandGroup, count: u32) -> Shape {
    g.per.times(u64::from(count))
}

/// How the greedy picks the next machine to add.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Opening {
    /// Lowest cost per Space it takes (the default).
    Ratio,
    /// The machine that takes the most Spaces (when machine counts or
    /// relay machines bind).
    Fewest,
    /// The lowest cost per Space among machines big enough that the
    /// machines still allowed hold the rest (when a machine-count limit
    /// binds: many small cheap machines would run out of count).
    Packed,
    /// The lowest hourly price that takes any (when a $/h cap binds).
    Cheapest,
}

/// The greedy plan: each opening rule, and the cheapest result.
pub fn greedy(groups: &[DemandGroup], hosts: &[Host], offers: &[Offer], c: &Constraints) -> Plan {
    let mut best: Option<(f64, Plan)> = None;
    for rule in [
        Opening::Ratio,
        Opening::Fewest,
        Opening::Packed,
        Opening::Cheapest,
    ] {
        for (idle_first, flexible_last) in [(true, true), (false, true), (true, false)] {
            let p = greedy_with(groups, hosts, offers, c, rule, idle_first, flexible_last);
            let cost = evaluate(&p, groups, hosts, c);
            if best.as_ref().is_none_or(|(b, _)| cost < *b - 1e-12) {
                best = Some((cost, p));
            }
        }
    }
    best.map(|(_, p)| p).unwrap_or_default()
}

/// The greedy plan with one opening rule (see the module docs).
pub fn greedy_with(
    groups: &[DemandGroup],
    hosts: &[Host],
    offers: &[Offer],
    c: &Constraints,
    rule: Opening,
    idle_first: bool,
    flexible_last: bool,
) -> Plan {
    let mut plan = Plan::default();
    let uh = usable_hosts(hosts, c);
    let uo = usable_offers(offers, c);
    let mut free: Vec<Shape> = hosts.iter().map(|h| h.free).collect();
    let mut host_dur: Vec<u64> = vec![0; hosts.len()];
    let mut relay_left = c.relay_limit.saturating_sub(c.relay_used);
    let mut rate_left = c
        .max_usd_per_hour
        .map(|m| m - c.running_usd_per_hour)
        .unwrap_or(f64::INFINITY);
    let mut budget_left = c.budget_left_usd.unwrap_or(f64::INFINITY);
    let mut new_left = c
        .pins
        .new_hosts
        .map_or(c.max_new_hosts, |n| n.min(c.max_new_hosts));
    let mut quota_left = c.quota_left.clone();
    let mut prov_left: BTreeMap<String, u32> = BTreeMap::new();
    for o in offers {
        let e = prov_left.entry(o.provider.clone()).or_insert(o.max_new);
        *e = (*e).min(o.max_new);
    }
    // Groups: pinned first, interactive before batch, larger first.
    let biggest = hosts
        .iter()
        .map(|h| h.total)
        .chain(offers.iter().map(|o| o.capacity))
        .fold(Shape::new(1, 1, 1), |a, b| {
            Shape::new(
                a.millicpus.max(b.millicpus),
                a.memory_mb.max(b.memory_mb),
                a.disk_gb.max(b.disk_gb),
            )
        });
    let mut order: Vec<usize> = (0..groups.len()).collect();
    order.sort_by(|&a, &b| {
        let ga = &groups[a];
        let gb = &groups[b];
        let size = |g: &DemandGroup| {
            let n = if g.whole { g.count } else { 1 };
            item_need(g, n).dominant_share(biggest)
        };
        // Groups with no cloud to fall back on take the machines first.
        let cloud_ok = |g: &DemandGroup| uo.iter().any(|(_, o)| offer_misfit(g, o).is_none());
        (ga.soft as u8)
            .cmp(&(gb.soft as u8))
            .then(
                (gb.pin_host.is_empty() as u8)
                    .cmp(&(ga.pin_host.is_empty() as u8))
                    .reverse(),
            )
            .then(if flexible_last {
                (cloud_ok(ga) as u8).cmp(&(cloud_ok(gb) as u8))
            } else {
                std::cmp::Ordering::Equal
            })
            .then((ga.latency as u8).cmp(&(gb.latency as u8)))
            .then(
                size(gb)
                    .partial_cmp(&size(ga))
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
            .then(ga.id.cmp(&gb.id))
    });
    let mut remaining: Vec<Item> = Vec::new();
    // Phase 1: machines in the pool.
    for &gi in &order {
        let g = &groups[gi];
        let mut left = g.count;
        let mut prefs: Vec<usize> = uh
            .iter()
            .filter(|(i, h)| host_misfit(g, h).is_none() && free[*i].how_many(g.per) > 0)
            .filter(|(i, h)| {
                idle_first
                    || host_dur[*i] > 0
                    || extension_cost(h, g.duration_secs, h_step(h)) == 0.0
            })
            .map(|(i, _)| *i)
            .collect();
        // Holders, then devices, then busy cloud hosts, then idle ones;
        // best fit (least room left) within each.
        prefs.sort_by(|&a, &b| {
            let rank = |i: usize| {
                let h = &hosts[i];
                let holds = g.holders.contains(&h.name);
                (
                    !holds as u8,
                    (h.boot_secs_left > 0) as u8,
                    !h.owned() as u8,
                    (!h.owned() && h.running == 0 && host_dur[i] == 0) as u8,
                    h.rtt_ms / 25,
                )
            };
            rank(a).cmp(&rank(b)).then(
                free[a]
                    .dominant_share(hosts[a].total.max1())
                    .partial_cmp(&free[b].dominant_share(hosts[b].total.max1()))
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
        });
        for i in prefs {
            if left == 0 {
                break;
            }
            let fit = free[i].how_many(g.per).min(u64::from(left)) as u32;
            let fit = if c.space_takes_relay {
                fit.min(relay_left)
            } else {
                fit
            };
            if fit == 0 || (g.whole && fit < left) {
                continue;
            }
            free[i] = Shape::minus(free[i], item_need(g, fit));
            host_dur[i] = host_dur[i].max(g.duration_secs);
            if c.space_takes_relay {
                relay_left -= fit;
            }
            plan.assignments.push(Assignment {
                group: g.id.clone(),
                target: Target::Host {
                    name: hosts[i].name.clone(),
                },
                count: fit,
                sync: g.sync_secs > 0 && !g.holders.contains(&hosts[i].name),
            });
            left -= fit;
        }
        if left > 0 {
            remaining.push(Item { g: gi, count: left });
        }
    }
    // Phase 2: new machines.
    let mut bins: Vec<Bin> = Vec::new();
    // (group, bin, count): Spaces on machines opened in this solve.
    let mut on_new: Vec<(usize, usize, u32)> = Vec::new();
    remaining.sort_by(|a, b| {
        (groups[a.g].soft as u8)
            .cmp(&(groups[b.g].soft as u8))
            .then(
                item_need(&groups[b.g], b.count)
                    .dominant_share(biggest)
                    .partial_cmp(&item_need(&groups[a.g], a.count).dominant_share(biggest))
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
    });
    let mut queue: Vec<Item> = remaining;
    while !queue.is_empty() {
        // Fill machines opened in this solve first (first fit).
        for item in queue.iter_mut() {
            let g = &groups[item.g];
            for (bi, b) in bins.iter_mut().enumerate() {
                if item.count == 0 {
                    break;
                }
                let o = &offers[b.offer.unwrap()];
                if o.one_space_each || offer_misfit(g, o).is_some() {
                    continue;
                }
                let base = if b.images.contains(&g.req.image) {
                    0
                } else {
                    g.req.base_gb
                };
                if base > b.room.disk_gb {
                    continue;
                }
                let room = Shape::minus(b.room, Shape::new(0, 0, base));
                let mut fit = room.how_many(g.per).min(u64::from(item.count)) as u32;
                if c.space_takes_relay {
                    fit = fit.min(relay_left);
                }
                if fit == 0 || (g.whole && fit < item.count) {
                    continue;
                }
                // A longer Space keeps the machine longer: only when that
                // costs no more than a machine of its own would.
                let extra = o.cost_for(b.max_dur.max(g.duration_secs)) - o.cost_for(b.max_dur);
                if extra > o.cost_for(g.duration_secs) + 1e-12 || extra > budget_left + 1e-12 {
                    continue;
                }
                budget_left -= extra;
                b.room = Shape::minus(room, item_need(g, fit));
                b.images.insert(g.req.image.clone());
                b.max_dur = b.max_dur.max(g.duration_secs);
                if c.space_takes_relay {
                    relay_left -= fit;
                }
                on_new.push((item.g, bi, fit));
                item.count -= fit;
            }
        }
        queue.retain(|i| i.count > 0);
        if queue.is_empty() {
            break;
        }
        // Expected (soft) demand never buys a machine of its own: it only
        // makes the machines bought for real demand bigger.
        if groups[queue[0].g].soft {
            for it in queue.drain(..) {
                plan.unplaced.push(Unplaced {
                    group: groups[it.g].id.clone(),
                    count: it.count,
                    reason: "expected demand: no room bought for it alone".into(),
                });
            }
            break;
        }
        // Open the offer with the lowest cost per unit of work it takes.
        let g0 = &groups[queue[0].g];
        // Relay machines the rest of the queue needs at least, on the
        // largest host offer: a Spaces supplier (one relay machine per
        // Space) may only use relay machines beyond those.
        let largest_host = uo
            .iter()
            .filter(|(_, o)| !o.one_space_each)
            .map(|(_, o)| o.capacity)
            .max_by(|a, b| {
                a.millicpus
                    .saturating_mul(a.memory_mb)
                    .cmp(&b.millicpus.saturating_mul(b.memory_mb))
            });
        let units_rest: u32 = queue.iter().map(|i| i.count).sum::<u32>().saturating_sub(1);
        let hosts_needed = largest_host.map_or(u32::MAX, |cap| {
            let mut share = 0.0;
            for (k, it) in queue.iter().enumerate() {
                let g = &groups[it.g];
                let n = if k == 0 {
                    it.count.saturating_sub(1)
                } else {
                    it.count
                };
                share += item_need(g, n).dominant_share(cap);
            }
            share.ceil() as u32 + if c.space_takes_relay { units_rest } else { 0 }
        });
        // The rest needs at least this many relay machines either way.
        let hosts_needed = hosts_needed.min(units_rest);
        // Spaces each machine still allowed must take for all to fit.
        let hard_rest: u32 = queue
            .iter()
            .filter(|i| !groups[i.g].soft)
            .map(|i| i.count)
            .sum();
        let per_machine = if new_left == 0 {
            u32::MAX
        } else {
            hard_rest.div_ceil(new_left.max(1))
        };
        let mut best: Option<(f64, u32, usize)> = None; // (ratio, boot, offer)
        for (oi, o) in &uo {
            if offer_misfit(g0, o).is_some()
                || (!o.one_space_each && new_left == 0)
                || prov_left.get(&o.provider).copied().unwrap_or(0) == 0
                || relay_left == 0
                || (o.one_space_each && relay_left.saturating_sub(1) < hosts_needed)
                || quota_left.get(&o.quota).is_some_and(|q| *q < o.vcpus)
                || o.usd_per_hour > rate_left + 1e-12
            {
                continue;
            }
            // First-fit decreasing of the queue into one fresh machine.
            let mut room = o.capacity;
            let mut images = BTreeSet::new();
            let mut value = 0.0;
            let mut taken = 0u32;
            let mut dur = 0u64;
            let mut relay_sim = relay_left.saturating_sub(1);
            for it in &queue {
                let g = &groups[it.g];
                if offer_misfit(g, o).is_some() {
                    continue;
                }
                let base = if images.contains(&g.req.image) {
                    0
                } else {
                    g.req.base_gb
                };
                if base > room.disk_gb {
                    continue;
                }
                let r = Shape::minus(room, Shape::new(0, 0, base));
                let mut fit = r.how_many(g.per).min(u64::from(it.count)) as u32;
                if o.one_space_each {
                    fit = fit.min(1);
                } else if c.space_takes_relay {
                    fit = fit.min(relay_sim);
                }
                if fit == 0 || (g.whole && fit < it.count) {
                    continue;
                }
                // Keep the machine's billed time unless the item is worth it.
                if dur > 0
                    && o.cost_for(dur.max(g.duration_secs)) - o.cost_for(dur)
                        > o.cost_for(g.duration_secs)
                {
                    continue;
                }
                room = Shape::minus(r, item_need(g, fit));
                images.insert(g.req.image.clone());
                if c.space_takes_relay && !o.one_space_each {
                    relay_sim -= fit;
                }
                dur = dur.max(g.duration_secs);
                let soft = latency_usd(g) * f64::from(o.boot_secs)
                    + rtt_usd(g) * f64::from(o.rtt_ms)
                    + if g.latency == Latency::Interactive && o.boot_secs > c.interactive_slo_secs {
                        SLO_MISS_USD
                    } else {
                        0.0
                    };
                // Each Space is worth one unit, less what its wait costs;
                // expected ones count for less.
                value += f64::from(fit) / (1.0 + soft) * if g.soft { 0.8 } else { 1.0 };
                if !g.soft {
                    taken += fit;
                }
                if o.one_space_each {
                    break;
                }
            }
            if value <= 0.0 {
                continue;
            }
            let cost = o.cost_for(dur);
            if cost > budget_left + 1e-12 {
                continue;
            }
            let ratio = match rule {
                Opening::Ratio => cost.max(1e-9) / value,
                // Most Spaces first; price breaks ties.
                Opening::Fewest => -value + cost * 1e-6,
                Opening::Cheapest => o.usd_per_hour - value * 1e-9,
                // Too small to finish within the count: only when nothing
                // is big enough, the biggest.
                Opening::Packed if taken < per_machine.min(hard_rest) => 1e9 - value + cost * 1e-6,
                Opening::Packed => cost.max(1e-9) / value,
            };
            let better = match &best {
                None => true,
                Some((r, boot, bi)) => {
                    ratio < *r - r.abs() * 1e-9 - 1e-15
                        || ((ratio - *r).abs() <= r.abs() * 1e-9 + 1e-15
                            && (o.boot_secs, o.key()) < (*boot, offers[*bi].key()))
                }
            };
            if better {
                best = Some((ratio, o.boot_secs, *oi));
            }
        }
        let Some((_, _, oi)) = best else {
            // Nothing can be opened for the first item: say why, drop it.
            let it = queue.remove(0);
            plan.unplaced.push(Unplaced {
                group: groups[it.g].id.clone(),
                count: it.count,
                reason: why_no_offer(
                    &groups[it.g],
                    hosts,
                    offers,
                    &uo,
                    c,
                    new_left,
                    relay_left,
                    rate_left,
                    budget_left,
                    &prov_left,
                    &quota_left,
                ),
            });
            continue;
        };
        let o = &offers[oi];
        if !o.one_space_each {
            new_left -= 1;
        }
        if let Some(q) = quota_left.get_mut(&o.quota) {
            *q = q.saturating_sub(o.vcpus);
        }
        *prov_left.get_mut(&o.provider).unwrap() -= 1;
        rate_left -= o.usd_per_hour;
        relay_left -= 1;
        // Billed for its minimum as soon as it exists; the rest as it fills.
        budget_left -= o.cost_for(0);
        bins.push(Bin {
            offer: Some(oi),
            room: o.capacity,
            max_dur: 0,
            images: BTreeSet::new(),
        });
        if o.one_space_each {
            // The machine is the Space.
            let it = &mut queue[0];
            let g = &groups[it.g];
            let bi = bins.len() - 1;
            let b = &mut bins[bi];
            b.room = Shape::minus(b.room, g.per);
            b.max_dur = g.duration_secs;
            budget_left -= o.cost_for(g.duration_secs) - o.cost_for(0);
            on_new.push((it.g, bi, 1));
            it.count -= 1;
            queue.retain(|i| i.count > 0);
        }
    }
    // Idle cloud hosts held back: they take what found no new machine.
    if !idle_first {
        let mut still = Vec::new();
        for u in plan.unplaced.drain(..) {
            let g = groups.iter().find(|g| g.id == u.group).unwrap();
            let mut left = u.count;
            for (i, h) in &uh {
                if left == 0 {
                    break;
                }
                if host_misfit(g, h).is_some() {
                    continue;
                }
                let mut fit = free[*i].how_many(g.per).min(u64::from(left)) as u32;
                if c.space_takes_relay {
                    fit = fit.min(relay_left);
                }
                if fit == 0 || (g.whole && fit < left) {
                    continue;
                }
                free[*i] = Shape::minus(free[*i], item_need(g, fit));
                host_dur[*i] = host_dur[*i].max(g.duration_secs);
                if c.space_takes_relay {
                    relay_left -= fit;
                }
                plan.assignments.push(Assignment {
                    group: g.id.clone(),
                    target: Target::Host {
                        name: h.name.clone(),
                    },
                    count: fit,
                    sync: g.sync_secs > 0 && !g.holders.contains(&h.name),
                });
                left -= fit;
            }
            if left > 0 {
                still.push(Unplaced { count: left, ..u });
            }
        }
        plan.unplaced = still;
    }
    // Pinned count: add the cheapest allowed machines up to it.
    if let Some(n) = c.pins.new_hosts {
        let opened = bins
            .iter()
            .filter(|b| !offers[b.offer.unwrap()].one_space_each)
            .count() as u32;
        let cheapest = uo.iter().filter(|(_, o)| !o.one_space_each).min_by(|a, b| {
            a.1.cost_for(0)
                .partial_cmp(&b.1.cost_for(0))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        if let Some((oi, o)) = cheapest {
            for _ in opened..n.min(c.max_new_hosts) {
                if o.usd_per_hour > rate_left + 1e-12
                    || o.cost_for(0) > budget_left + 1e-12
                    || relay_left == 0
                {
                    break;
                }
                rate_left -= o.usd_per_hour;
                budget_left -= o.cost_for(0);
                relay_left -= 1;
                bins.push(Bin {
                    offer: Some(*oi),
                    room: o.capacity,
                    max_dur: 0,
                    images: BTreeSet::new(),
                });
                on_new.push((usize::MAX, bins.len() - 1, 0));
            }
        }
    }
    // Machines that got work, in the order they were opened.
    let mut index_of = vec![usize::MAX; bins.len()];
    for (bi, b) in bins.iter().enumerate() {
        if !on_new.iter().any(|(_, x, _)| *x == bi) {
            continue;
        }
        let o = &offers[b.offer.unwrap()];
        index_of[bi] = plan.new_hosts.len();
        plan.new_hosts.push(NewHost {
            provider: o.provider.clone(),
            type_name: o.type_name.clone(),
            region: o.region.clone(),
            usd_per_hour: o.usd_per_hour,
            boot_secs: o.boot_secs,
            billed_secs: o.billed_secs(b.max_dur),
            one_space: o.one_space_each,
            rtt_ms: o.rtt_ms,
        });
    }
    for (gi, bi, n) in on_new {
        if gi == usize::MAX {
            continue; // a pinned machine with no work yet
        }
        let g = &groups[gi];
        plan.assignments.push(Assignment {
            group: g.id.clone(),
            target: Target::New {
                index: index_of[bi],
            },
            count: n,
            sync: g.sync_secs > 0,
        });
    }
    merge_assignments(&mut plan);
    plan.explain = explain(&plan, groups, hosts);
    plan
}

impl Shape {
    fn max1(self) -> Shape {
        Shape::new(
            self.millicpus.max(1),
            self.memory_mb.max(1),
            self.disk_gb.max(1),
        )
    }
}

#[allow(clippy::too_many_arguments)]
fn why_no_offer(
    g: &DemandGroup,
    hosts: &[Host],
    offers: &[Offer],
    uo: &[(usize, &Offer)],
    c: &Constraints,
    new_left: u32,
    relay_left: u32,
    rate_left: f64,
    budget_left: f64,
    prov_left: &BTreeMap<String, u32>,
    quota_left: &BTreeMap<String, u32>,
) -> String {
    let host_reasons: Vec<String> = hosts.iter().filter_map(|h| host_misfit(g, h)).collect();
    if c.pins.no_scale {
        return "your machines are full and autoscale is off (pinned)".into();
    }
    if c.pins.devices_only {
        return "your devices are full (only your devices were allowed)".into();
    }
    if offers.is_empty() {
        return if host_reasons.is_empty() {
            "your machines are full and no cloud is linked: link one with `cua cloud connect`"
                .into()
        } else {
            format!(
                "no machine can run it ({}) and no cloud is linked",
                host_reasons.join("; ")
            )
        };
    }
    if uo.is_empty() {
        return "no linked cloud offer matches the pinned provider, region or type".into();
    }
    let fits: Vec<&&Offer> = uo
        .iter()
        .map(|(_, o)| o)
        .filter(|o| offer_misfit(g, o).is_none())
        .collect();
    if fits.is_empty() {
        let r: BTreeSet<String> = uo.iter().filter_map(|(_, o)| offer_misfit(g, o)).collect();
        return format!(
            "no linked cloud can run it: {}",
            r.into_iter().take(3).collect::<Vec<_>>().join("; ")
        );
    }
    // Which limit stops each offer that could run it; name the first that
    // stops them all.
    let blocked = |why: &dyn Fn(&Offer) -> bool| fits.iter().all(|o| why(o));
    if relay_left == 0 {
        return format!(
            "the account's relay allows {} machines and all are in use",
            c.relay_limit
        );
    }
    if blocked(&|o| !o.one_space_each && new_left == 0) {
        return format!(
            "the plan already adds the most machines allowed ({})",
            c.max_new_hosts
        );
    }
    if blocked(&|o| o.usd_per_hour > rate_left + 1e-12) {
        return format!(
            "your clouds' cost cap (${:.2}/h) is reached: raise it with `cua cloud budget --max-per-hour`",
            c.max_usd_per_hour.unwrap_or(0.0)
        );
    }
    if blocked(&|o| o.cost_for(g.duration_secs) > budget_left + 1e-12) {
        return format!(
            "the budget left (${:.2}) cannot pay for another machine: raise it with `cua cloud budget`",
            c.budget_left_usd.unwrap_or(0.0)
        );
    }
    if blocked(&|o| quota_left.get(&o.quota).is_some_and(|q| *q < o.vcpus)) {
        let pools: BTreeSet<&str> = fits.iter().map(|o| o.quota.as_str()).collect();
        return format!(
            "the cloud's core limit is reached ({}): ask the provider to raise it, or link another cloud",
            pools.into_iter().collect::<Vec<_>>().join(", ")
        );
    }
    if blocked(&|o| prov_left.get(&o.provider).copied().unwrap_or(u32::MAX) == 0) {
        let provs: BTreeSet<&str> = fits.iter().map(|o| o.provider.as_str()).collect();
        return format!(
            "every linked cloud that can run it is at its machine limit ({})",
            provs.into_iter().collect::<Vec<_>>().join(", ")
        );
    }
    "no machine had room left for it within the limits (relay machines, caps)".into()
}

/// Joins assignments of one group to one target.
fn merge_assignments(plan: &mut Plan) {
    let mut out: Vec<Assignment> = Vec::new();
    for a in plan.assignments.drain(..) {
        if let Some(x) = out
            .iter_mut()
            .find(|x| x.group == a.group && x.target == a.target)
        {
            x.count += a.count;
            x.sync |= a.sync;
        } else {
            out.push(a);
        }
    }
    plan.assignments = out;
}

/// One line per decision.
fn explain(plan: &Plan, groups: &[DemandGroup], hosts: &[Host]) -> Vec<String> {
    let mut v = Vec::new();
    for a in &plan.assignments {
        let g = groups.iter().find(|g| g.id == a.group).unwrap();
        let place = match &a.target {
            Target::Host { name } => {
                let h = hosts.iter().find(|h| &h.name == name).unwrap();
                if h.owned() {
                    format!("{name} (your device, free)")
                } else {
                    format!("{name} ({} host already running)", h.provider)
                }
            }
            Target::New { index } => {
                let n = &plan.new_hosts[*index];
                format!(
                    "new {} {} in {} (${:.4}/h, ready in ~{} s)",
                    n.provider, n.type_name, n.region, n.usd_per_hour, n.boot_secs
                )
            }
        };
        v.push(format!(
            "{} x {} -> {}{}",
            a.count,
            g.id,
            place,
            if a.sync {
                " (checkpoint copied there first)"
            } else {
                ""
            }
        ));
    }
    for u in &plan.unplaced {
        v.push(format!(
            "{} x {} not placed: {}",
            u.count, u.group, u.reason
        ));
    }
    v
}

// ---------------------------------------------------------------- exact

/// The MILP of the module docs, built when the instance is small enough.
pub struct ExactModel {
    groups: Vec<DemandGroup>,
    hosts: Vec<Host>,
    offers: Vec<Offer>,
    c: Constraints,
    /// Offer slots: (offer index, k).
    slots: Vec<(usize, usize)>,
}

// The model is index algebra over (group, target) pairs: loops over
// indices read as the formulation in the module docs.
#[allow(clippy::needless_range_loop)]
impl ExactModel {
    /// The model, or `None` when it would be too large to solve in time.
    pub fn build(
        groups: &[DemandGroup],
        hosts: &[Host],
        offers: &[Offer],
        c: &Constraints,
    ) -> Option<Self> {
        if groups.is_empty() {
            return None;
        }
        let uo = usable_offers(offers, c);
        // Slots per offer: enough machines of it for everything that fits
        // it, capped by the limits.
        let mut slots = Vec::new();
        for (oi, o) in &uo {
            let mut need = 0u64;
            for g in groups {
                if offer_misfit(g, o).is_some() {
                    continue;
                }
                need += if g.whole || o.one_space_each {
                    u64::from(g.count)
                } else {
                    u64::from(g.count)
                        .div_ceil(o.capacity.how_many(g.per.plus_disk(g.req.base_gb)).max(1))
                };
            }
            let k = need
                .min(if o.one_space_each {
                    u64::MAX
                } else {
                    u64::from(c.max_new_hosts)
                })
                .min(u64::from(o.max_new))
                .min(u64::from(c.relay_limit.saturating_sub(c.relay_used)));
            for k in 0..k as usize {
                slots.push((*oi, k));
            }
        }
        let targets = hosts.len() + slots.len();
        let levels = durations(groups).len();
        let vars = groups.len() * targets * 2 + slots.len() * (levels + 1) + groups.len();
        if vars > EXACT_MAX_VARS {
            return None;
        }
        Some(Self {
            groups: groups.to_vec(),
            hosts: hosts.to_vec(),
            offers: offers.to_vec(),
            c: c.clone(),
            slots,
        })
    }

    /// Solves the MILP within `budget` (microlp's own time limit: it stops
    /// and returns its best feasible answer, or nothing). `Ok((plan,
    /// optimal))`.
    pub fn solve_within(&self, budget: Duration) -> Result<Option<(Plan, bool)>, String> {
        use microlp::{ComparisonOp as Op, OptimizationDirection, Problem, SolveOutcome, Variable};
        let g = &self.groups;
        let c = &self.c;
        let levels = durations(g);
        let mut p = Problem::new(OptimizationDirection::Minimize);
        p.set_time_limit(budget);
        let nh = self.hosts.len();
        let ns = self.slots.len();
        let nt = nh + ns;
        let usable_h: BTreeSet<usize> = usable_hosts(&self.hosts, c)
            .iter()
            .map(|(i, _)| *i)
            .collect();
        let mut x: Vec<Vec<Option<Variable>>> = vec![vec![None; nt]; g.len()];
        let mut a: Vec<Vec<Option<Variable>>> = vec![vec![None; nt]; g.len()];
        for (gi, gr) in g.iter().enumerate() {
            for t in 0..nt {
                let ok = if t < nh {
                    usable_h.contains(&t)
                        && host_misfit(gr, &self.hosts[t]).is_none()
                        && self.hosts[t].free.how_many(gr.per) > 0
                } else {
                    offer_misfit(gr, &self.offers[self.slots[t - nh].0]).is_none()
                };
                if !ok {
                    continue;
                }
                let (boot, cloud, holder, rtt) = if t < nh {
                    let h = &self.hosts[t];
                    (
                        h.boot_secs_left,
                        !h.owned(),
                        gr.holders.contains(&h.name),
                        h.rtt_ms,
                    )
                } else {
                    let o = &self.offers[self.slots[t - nh].0];
                    (o.boot_secs, true, false, o.rtt_ms)
                };
                let mut per = latency_usd(gr) * f64::from(boot) + rtt_usd(gr) * f64::from(rtt);
                if boot > c.interactive_slo_secs && gr.latency == Latency::Interactive && !gr.soft {
                    per += SLO_MISS_USD;
                }
                if cloud {
                    per += CLOUD_EPS_USD;
                }
                let sync = if gr.sync_secs > 0 && !holder {
                    SYNC_USD_PER_SEC * f64::from(gr.sync_secs)
                } else {
                    0.0
                };
                x[gi][t] = Some(p.add_integer_var(per, (0, gr.count as i32)));
                a[gi][t] = Some(p.add_binary_var(sync));
            }
        }
        let u: Vec<Variable> = g
            .iter()
            .map(|gr| p.add_integer_var(unplaced_usd(gr), (0, gr.count as i32)))
            .collect();
        let open: Vec<Variable> = (0..ns).map(|_| p.add_binary_var(0.0)).collect();
        let lvl: Vec<Vec<Variable>> = self
            .slots
            .iter()
            .map(|(oi, _)| {
                let o = &self.offers[*oi];
                levels
                    .iter()
                    .map(|d| p.add_binary_var(o.cost_for(*d)))
                    .collect()
            })
            .collect();
        let max_dur = levels.last().copied().unwrap_or(0);
        let idle: Vec<Option<Variable>> = self
            .hosts
            .iter()
            .map(|h| {
                (!h.owned() && h.running == 0)
                    .then(|| p.add_binary_var(extension_cost(h, max_dur, h_step(h))))
            })
            .collect();
        // Every Space placed or unplaced; whole groups on one target.
        for (gi, gr) in g.iter().enumerate() {
            let n = f64::from(gr.count);
            let mut sum = vec![(u[gi], 1.0)];
            let mut asum = vec![];
            for t in 0..nt {
                if let (Some(xv), Some(av)) = (x[gi][t], a[gi][t]) {
                    sum.push((xv, 1.0));
                    asum.push((av, 1.0));
                    if gr.whole {
                        p.add_constraint(&[(av, n), (xv, -1.0)][..], Op::Eq, 0.0);
                    } else {
                        p.add_constraint(&[(xv, 1.0), (av, -n)][..], Op::Le, 0.0);
                        p.add_constraint(&[(av, 1.0), (xv, -1.0)][..], Op::Le, 0.0);
                    }
                }
            }
            p.add_constraint(sum, Op::Eq, n);
            if gr.whole && !asum.is_empty() {
                p.add_constraint(asum, Op::Le, 1.0);
            }
        }
        // Existing hosts.
        for t in 0..nh {
            let h = &self.hosts[t];
            for dim in 0..3 {
                let e: Vec<(Variable, f64)> = g
                    .iter()
                    .enumerate()
                    .filter_map(|(gi, gr)| x[gi][t].map(|xv| (xv, dimv(gr.per, dim) as f64)))
                    .collect();
                if !e.is_empty() {
                    p.add_constraint(e, Op::Le, dimv(h.free, dim) as f64);
                }
            }
            if let Some(w) = idle[t] {
                for gi in 0..g.len() {
                    if let Some(av) = a[gi][t] {
                        p.add_constraint(&[(av, 1.0), (w, -1.0)][..], Op::Le, 0.0);
                    }
                }
            }
        }
        // Slots.
        for (s, (oi, k)) in self.slots.iter().enumerate() {
            let o = &self.offers[*oi];
            let t = nh + s;
            for dim in 0..3 {
                let mut e: Vec<(Variable, f64)> = vec![(open[s], -(dimv(o.capacity, dim) as f64))];
                for (gi, gr) in g.iter().enumerate() {
                    if let Some(xv) = x[gi][t] {
                        e.push((xv, dimv(gr.per, dim) as f64));
                        if dim == 2 && gr.req.base_gb > 0 {
                            // Base disk per group on the machine: exact with
                            // one group per image, conservative otherwise.
                            e.push((a[gi][t].unwrap(), gr.req.base_gb as f64));
                        }
                    }
                }
                p.add_constraint(e, Op::Le, 0.0);
            }
            if o.one_space_each {
                let mut e: Vec<(Variable, f64)> = vec![(open[s], -1.0)];
                for gi in 0..g.len() {
                    if let Some(xv) = x[gi][t] {
                        e.push((xv, 1.0));
                    }
                }
                p.add_constraint(e, Op::Le, 0.0);
            }
            let mut ls: Vec<(Variable, f64)> = lvl[s].iter().map(|l| (*l, 1.0)).collect();
            ls.push((open[s], -1.0));
            p.add_constraint(ls, Op::Eq, 0.0);
            for (gi, gr) in g.iter().enumerate() {
                if let Some(av) = a[gi][t] {
                    let mut e = vec![(av, 1.0)];
                    for (l, d) in levels.iter().enumerate() {
                        if *d >= gr.duration_secs {
                            e.push((lvl[s][l], -1.0));
                        }
                    }
                    p.add_constraint(e, Op::Le, 0.0);
                }
            }
            if *k > 0 {
                p.add_constraint(&[(open[s], 1.0), (open[s - 1], -1.0)][..], Op::Le, 0.0);
            }
        }
        // Limits.
        let host_slots: Vec<(Variable, f64)> = self
            .slots
            .iter()
            .enumerate()
            .filter(|(_, (oi, _))| !self.offers[*oi].one_space_each)
            .map(|(s, _)| (open[s], 1.0))
            .collect();
        if !host_slots.is_empty() {
            p.add_constraint(host_slots.clone(), Op::Le, f64::from(c.max_new_hosts));
            if let Some(n) = c.pins.new_hosts {
                p.add_constraint(
                    host_slots.clone(),
                    Op::Eq,
                    f64::from(n).min(host_slots.len() as f64),
                );
            }
        }
        let mut per_prov: BTreeMap<&str, Vec<(Variable, f64)>> = BTreeMap::new();
        let mut rate = vec![];
        let mut spend = vec![];
        for (s, (oi, _)) in self.slots.iter().enumerate() {
            let o = &self.offers[*oi];
            per_prov
                .entry(o.provider.as_str())
                .or_default()
                .push((open[s], 1.0));
            rate.push((open[s], o.usd_per_hour));
            for (l, d) in levels.iter().enumerate() {
                spend.push((lvl[s][l], o.cost_for(*d)));
            }
        }
        for (prov, e) in per_prov {
            let max = self
                .offers
                .iter()
                .filter(|o| o.provider == prov)
                .map(|o| o.max_new)
                .min()
                .unwrap_or(u32::MAX);
            if max < u32::MAX {
                p.add_constraint(e, Op::Le, f64::from(max));
            }
        }
        let mut per_pool: BTreeMap<&str, Vec<(Variable, f64)>> = BTreeMap::new();
        for (s, (oi, _)) in self.slots.iter().enumerate() {
            let o = &self.offers[*oi];
            if c.quota_left.contains_key(&o.quota) {
                per_pool
                    .entry(o.quota.as_str())
                    .or_default()
                    .push((open[s], f64::from(o.vcpus)));
            }
        }
        for (pool, e) in per_pool {
            p.add_constraint(e, Op::Le, f64::from(c.quota_left[pool]));
        }
        if let (Some(m), false) = (c.max_usd_per_hour, rate.is_empty()) {
            p.add_constraint(rate, Op::Le, (m - c.running_usd_per_hour).max(0.0));
        }
        if let (Some(b), false) = (c.budget_left_usd, spend.is_empty()) {
            p.add_constraint(spend, Op::Le, b.max(0.0));
        }
        let mut relay: Vec<(Variable, f64)> = open.iter().map(|v| (*v, 1.0)).collect();
        if c.space_takes_relay {
            for gi in 0..g.len() {
                for t in 0..nt {
                    let one_space = t >= nh && self.offers[self.slots[t - nh].0].one_space_each;
                    if let (Some(xv), false) = (x[gi][t], one_space) {
                        relay.push((xv, 1.0));
                    }
                }
            }
        }
        if !relay.is_empty() {
            p.add_constraint(
                relay,
                Op::Le,
                f64::from(c.relay_limit.saturating_sub(c.relay_used)),
            );
        }
        let outcome = p.solve().map_err(|e| e.to_string())?;
        let optimal = outcome.is_optimal();
        let sol = match outcome {
            SolveOutcome::Solution(s) => s,
            SolveOutcome::Interrupted(_) => return Ok(None),
        };
        let val = |v: Variable| sol.var_value(v);
        // Read the plan.
        let mut plan = Plan::default();
        let mut slot_index: BTreeMap<usize, usize> = BTreeMap::new();
        for s in 0..ns {
            if val(open[s]) > 0.5 {
                let o = &self.offers[self.slots[s].0];
                slot_index.insert(s, plan.new_hosts.len());
                plan.new_hosts.push(NewHost {
                    provider: o.provider.clone(),
                    type_name: o.type_name.clone(),
                    region: o.region.clone(),
                    usd_per_hour: o.usd_per_hour,
                    boot_secs: o.boot_secs,
                    billed_secs: 0,
                    one_space: o.one_space_each,
                    rtt_ms: o.rtt_ms,
                });
            }
        }
        for (gi, gr) in g.iter().enumerate() {
            for t in 0..nt {
                let Some(xv) = x[gi][t] else { continue };
                let n = val(xv).round() as u32;
                if n == 0 {
                    continue;
                }
                let target = if t < nh {
                    Target::Host {
                        name: self.hosts[t].name.clone(),
                    }
                } else {
                    match slot_index.get(&(t - nh)) {
                        Some(i) => Target::New { index: *i },
                        None => continue,
                    }
                };
                let holder = t < nh && gr.holders.contains(&self.hosts[t].name);
                plan.assignments.push(Assignment {
                    group: gr.id.clone(),
                    target,
                    count: n,
                    sync: gr.sync_secs > 0 && !holder,
                });
            }
        }
        // Billed time from what each machine actually got; machines a
        // pinned count added without work bill their minimum.
        for (i, n) in plan.new_hosts.iter_mut().enumerate() {
            let d = plan
                .assignments
                .iter()
                .filter(|a| a.target == Target::New { index: i })
                .map(|a| g.iter().find(|x| x.id == a.group).unwrap().duration_secs)
                .max()
                .unwrap_or(0);
            let o = self
                .offers
                .iter()
                .find(|o| {
                    o.provider == n.provider && o.type_name == n.type_name && o.region == n.region
                })
                .unwrap();
            n.billed_secs = o.billed_secs(d);
        }
        for (gi, gr) in g.iter().enumerate() {
            let placed: u32 = plan
                .assignments
                .iter()
                .filter(|a| a.group == gr.id)
                .map(|a| a.count)
                .sum();
            let un = gr.count - placed.min(gr.count);
            let _ = gi;
            if un > 0 {
                let new_hosts = plan.new_hosts.iter().filter(|n| !n.one_space).count() as u32;
                plan.unplaced.push(Unplaced {
                    group: gr.id.clone(),
                    count: un,
                    reason: why_no_offer(
                        gr,
                        &self.hosts,
                        &self.offers,
                        &usable_offers(&self.offers, c),
                        c,
                        c.max_new_hosts.saturating_sub(new_hosts),
                        c.relay_limit.saturating_sub(
                            c.relay_used + relay_added_with(&plan, c.space_takes_relay),
                        ),
                        c.max_usd_per_hour
                            .map(|m| m - c.running_usd_per_hour)
                            .unwrap_or(f64::INFINITY)
                            - plan.new_hosts.iter().map(|n| n.usd_per_hour).sum::<f64>(),
                        c.budget_left_usd.map_or(f64::INFINITY, |b| {
                            b - plan
                                .new_hosts
                                .iter()
                                .map(|n| n.usd_per_hour * n.billed_secs as f64 / 3600.0)
                                .sum::<f64>()
                        }),
                        &prov_used_left(&self.offers, &plan),
                        &quota_used_left(&self.offers, &plan, &c.quota_left),
                    ),
                });
            }
        }
        plan.explain = explain(&plan, g, &self.hosts);
        Ok(Some((plan, optimal)))
    }
}

/// vCPUs left per quota pool after `plan`.
fn quota_used_left(
    offers: &[Offer],
    plan: &Plan,
    left: &BTreeMap<String, u32>,
) -> BTreeMap<String, u32> {
    let mut m = left.clone();
    for n in &plan.new_hosts {
        if let Some(o) = offers.iter().find(|o| {
            o.provider == n.provider && o.type_name == n.type_name && o.region == n.region
        }) && let Some(q) = m.get_mut(&o.quota)
        {
            *q = q.saturating_sub(o.vcpus);
        }
    }
    m
}

/// Machines each provider may still add after `plan`.
fn prov_used_left(offers: &[Offer], plan: &Plan) -> BTreeMap<String, u32> {
    let mut m: BTreeMap<String, u32> = BTreeMap::new();
    for o in offers {
        let e = m.entry(o.provider.clone()).or_insert(o.max_new);
        *e = (*e).min(o.max_new);
    }
    for n in &plan.new_hosts {
        if let Some(v) = m.get_mut(&n.provider) {
            *v = v.saturating_sub(1);
        }
    }
    m
}

fn dimv(s: Shape, d: usize) -> u64 {
    match d {
        0 => s.millicpus,
        1 => s.memory_mb,
        _ => s.disk_gb,
    }
}

/// The distinct durations of the groups, ascending (billing levels).
fn durations(groups: &[DemandGroup]) -> Vec<u64> {
    let set: BTreeSet<u64> = groups.iter().map(|g| g.duration_secs).collect();
    set.into_iter().collect()
}

/// Expected demand like `g` (the rest of a burst): `expected` more Spaces,
/// as soft groups of `g`'s shape (whole groups stay whole), capped at 64
/// groups. Solve them with `g` so the machines bought for `g` are sized
/// for what is coming.
pub fn anticipate(g: &DemandGroup, expected: u32, offers: &[Offer]) -> Vec<DemandGroup> {
    if expected == 0 || g.soft {
        return vec![];
    }
    let base = DemandGroup {
        soft: true,
        latency: Latency::Batch,
        pin_host: String::new(),
        holders: vec![],
        sync_secs: 0,
        ..g.clone()
    };
    if g.whole && g.count > 1 {
        let n = (expected / g.count).min(64);
        let value = soft_value(g.per, g.count, g.duration_secs, offers);
        (0..n)
            .map(|i| DemandGroup {
                id: format!("{}~expected-{i}", g.id),
                soft_usd: value,
                ..base.clone()
            })
            .collect()
    } else {
        vec![DemandGroup {
            id: format!("{}~expected", g.id),
            count: expected.min(4096),
            whole: false,
            soft_usd: soft_value(g.per, 1, g.duration_secs, offers),
            ..base
        }]
    }
}
