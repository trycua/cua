//! The warm pool: how many idle, prepared VMs to keep, when to replace
//! them, when to let them all go, and what that may cost.
//!
//! A warm VM is booted (from the golden fork when there is one) and fully
//! prepared: the runner is installed, the job hooks are in place, the
//! cache and mirror mounts are attached. It is *not* registered with
//! GitHub. A claim ([`pick`]) takes one, mints a just-in-time runner
//! configuration for one job and starts the runner: a few calls instead
//! of a boot, about a second.
//!
//! [`plan`] is pure: given the clock, the counts and the demand it says
//! how many VMs to create and which to delete. The pool calls it every
//! tick and does what it says, so the policy is tested without a VM.
//!
//! Rules, in order:
//!
//! 1. **Budget.** At the hard limit nothing is created. Warm VMs may use
//!    at most a quarter of what is left of the month's budget (idle
//!    minutes are the cost of being fast, and they are bounded).
//! 2. **Scale to zero.** With no job and no queued job for
//!    `scale_to_zero_after`, the target is zero: the computer is idle.
//! 3. **Target.** `warm` VMs, more while jobs are queued and VMs are
//!    free, never more than the cap (macOS on one Mac: Apple's two VMs
//!    less the busy ones less other macOS VMs).
//! 4. **Stale.** A warm VM from a golden image that is no longer current
//!    is deleted, and so is one idle past the TTL. Nothing is created in
//!    the same pass: the deleted VMs still hold their slots, and a third
//!    macOS VM is refused.
//! 5. **Excess.** More warm VMs than the target: the oldest go first.

use serde::{Deserialize, Serialize};

/// A ready, unclaimed warm VM.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WarmEntry {
    /// The slot's id.
    pub id: String,
    /// When it became ready (Unix ms).
    pub ready_ms: u64,
    /// The golden image key it started from (`None`: a plain image).
    pub image_key: Option<String>,
}

/// Why a warm VM is deleted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReapReason {
    /// It was idle past `warm_ttl`.
    Ttl,
    /// Its golden image is no longer current.
    StaleImage,
    /// More warm VMs than the target.
    Excess,
    /// The pool is scaling to zero (or the budget forbids warm VMs).
    ScaleDown,
}

impl ReapReason {
    /// The word the event carries.
    pub fn as_str(self) -> &'static str {
        match self {
            ReapReason::Ttl => "warm_ttl",
            ReapReason::StaleImage => "stale_image",
            ReapReason::Excess => "warm_excess",
            ReapReason::ScaleDown => "scale_to_zero",
        }
    }
}

/// What the pool counts at one tick.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// Ready, unclaimed warm VMs.
    pub warm: Vec<WarmEntry>,
    /// VMs being created or prepared (they will be warm).
    pub booting: u32,
    /// VMs with a job: claimed, registered or running one.
    pub busy: u32,
}

/// What asks for VMs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Demand {
    /// Jobs the controller knows are queued and not yet claimed.
    pub queued_jobs: u32,
    /// The last time a job was queued or claimed, or the pool started
    /// (Unix ms): the scale-to-zero clock.
    pub last_demand_ms: u64,
}

/// The policy knobs.
#[derive(Clone, Debug, PartialEq)]
pub struct WarmPolicy {
    /// Warm VMs to keep while there is demand.
    pub warm: u32,
    /// The most VMs at once (busy, booting and warm together).
    pub cap: u32,
    /// Idle longer than this: replaced.
    pub ttl_ms: u64,
    /// No demand for this long: zero warm VMs.
    pub scale_to_zero_after_ms: u64,
    /// The current golden image key, when forks are used.
    pub golden_key: Option<String>,
    /// The spend budget, when the computer is metered.
    pub budget: Option<Budget>,
}

/// A month's spend against its limit.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Budget {
    /// The monthly limit, USD.
    pub limit_usd: f64,
    /// Spent so far this month, USD.
    pub spent_usd: f64,
    /// One VM, one hour, USD (`0`: a computer you own).
    pub usd_per_hour: f64,
}

/// What warm VMs may use of the remaining budget.
pub const WARM_BUDGET_SHARE: f64 = 0.25;

impl Budget {
    /// What is left (never negative).
    pub fn remaining(&self) -> f64 {
        (self.limit_usd - self.spent_usd).max(0.0)
    }

    /// The limit is reached: no new VMs.
    pub fn exhausted(&self) -> bool {
        self.limit_usd > 0.0 && self.spent_usd >= self.limit_usd
    }

    /// The most `warm` idle VMs may cost over `ttl_ms` fits the warm share
    /// of what is left. A free computer always passes.
    pub fn allows_warm(&self, warm: u32, ttl_ms: u64) -> bool {
        if self.exhausted() {
            return false;
        }
        if self.usd_per_hour <= 0.0 {
            return true;
        }
        let hours = ttl_ms as f64 / 3_600_000.0;
        f64::from(warm) * self.usd_per_hour * hours <= self.remaining() * WARM_BUDGET_SHARE
    }

    /// A job may take a VM (cold or warm) while not exhausted and its
    /// first hour fits.
    pub fn admits_job(&self) -> bool {
        !self.exhausted()
            && (self.usd_per_hour <= 0.0 || self.remaining() >= self.usd_per_hour * 0.1)
    }
}

/// What to do now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WarmPlan {
    /// VMs to start preparing as warm.
    pub create: u32,
    /// Warm VMs to delete, and why.
    pub reap: Vec<(String, ReapReason)>,
    /// The target the plan aimed at.
    pub target: u32,
}

/// The plan for this tick.
pub fn plan(p: &WarmPolicy, now: u64, c: &Counts, d: &Demand) -> WarmPlan {
    let mut out = WarmPlan::default();
    let idle_for = now.saturating_sub(d.last_demand_ms);
    let zero = idle_for >= p.scale_to_zero_after_ms && d.queued_jobs == 0;
    let budget_blocks_warm = p
        .budget
        .is_some_and(|b| !b.allows_warm(p.warm.max(1), p.ttl_ms));
    let hard_stop = p.budget.is_some_and(|b| b.exhausted());

    let mut target = if zero || budget_blocks_warm || hard_stop {
        0
    } else {
        p.warm.max(d.queued_jobs)
    };
    target = target.min(p.cap);
    out.target = target;

    // Which warm VMs go.
    let mut keep: Vec<&WarmEntry> = Vec::new();
    for w in &c.warm {
        let stale = match (&p.golden_key, &w.image_key) {
            (Some(g), Some(k)) => g != k,
            (Some(_), None) => true,
            _ => false,
        };
        if stale {
            out.reap.push((w.id.clone(), ReapReason::StaleImage));
        } else if now.saturating_sub(w.ready_ms) > p.ttl_ms {
            out.reap.push((w.id.clone(), ReapReason::Ttl));
        } else {
            keep.push(w);
        }
    }
    // More than the target: the oldest go first (the freshest are the
    // likeliest to match the current image and to be warm in the page cache).
    keep.sort_by_key(|w| std::cmp::Reverse(w.ready_ms));
    let want_keep = target.saturating_sub(c.booting) as usize;
    if keep.len() > want_keep {
        let reason = if target == 0 {
            ReapReason::ScaleDown
        } else {
            ReapReason::Excess
        };
        for w in keep.drain(want_keep..) {
            out.reap.push((w.id.clone(), reason));
        }
    }

    // Creates wait for the deletes: the deleted VMs still hold their
    // slots, and Apple refuses a third macOS VM.
    if !out.reap.is_empty() {
        return out;
    }
    let have = keep.len() as u32 + c.booting;
    let used = c.busy + have;
    let free = p.cap.saturating_sub(used);
    out.create = target.saturating_sub(have).min(free);
    out
}

/// A warm VM for a claim: the freshest with the current golden image,
/// else the oldest of the others (a stale VM still beats a cold boot).
pub fn pick<'a>(entries: &'a [WarmEntry], golden_key: Option<&str>) -> Option<&'a WarmEntry> {
    let current = |w: &&WarmEntry| match golden_key {
        Some(g) => w.image_key.as_deref() == Some(g),
        None => true,
    };
    entries
        .iter()
        .filter(current)
        .max_by_key(|w| w.ready_ms)
        .or_else(|| entries.iter().min_by_key(|w| w.ready_ms))
}

/// The month's spend, kept in the pool's state file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Ledger {
    /// `YYYYMM` of the month the spend is for (`0`: none yet).
    #[serde(default)]
    pub month: u32,
    /// Spent this month, USD.
    #[serde(default)]
    pub spent_usd: f64,
}

impl Eq for Ledger {}

impl Ledger {
    /// Charges `vm_ms` of one VM at `usd_per_hour` at time `now`,
    /// starting a new month's count when the month changed.
    pub fn charge(&mut self, now: u64, vm_ms: u64, usd_per_hour: f64) {
        let m = yyyymm(now);
        if self.month != m {
            self.month = m;
            self.spent_usd = 0.0;
        }
        self.spent_usd += usd_per_hour * vm_ms as f64 / 3_600_000.0;
    }

    /// The spend counting for `now`'s month.
    pub fn spent_in(&self, now: u64) -> f64 {
        if self.month == yyyymm(now) {
            self.spent_usd
        } else {
            0.0
        }
    }
}

/// `YYYYMM` (UTC) of a Unix ms time.
pub fn yyyymm(ms: u64) -> u32 {
    let days = (ms / 86_400_000) as i64;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y * 100 + m) as u32
}

#[cfg(test)]
mod tests;
