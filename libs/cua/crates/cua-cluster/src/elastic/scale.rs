//! Scale-in: when an idle cloud machine goes away, and which machine stops
//! taking new work so it can drain (consolidation).
//!
//! Spaces are not migrated (a running VM or container keeps its machine),
//! so consolidation works the way Karpenter's does for pods that cannot
//! move: the least-used cloud machine is **cordoned** (placement skips it)
//! while the others have room for what recent demand predicts, and it is
//! removed once empty. An empty machine is removed at the end of the time
//! already paid for (Hetzner bills whole hours: an idle server is kept
//! until a couple of minutes before its next hour starts, free; AWS and
//! GCP bill by the second after a 60 s minimum: removed after a short idle
//! grace). A small warm buffer is kept only when recent demand predicts a
//! Space before a new machine could boot.

use serde::{Deserialize, Serialize};

/// A cloud machine as the scaler sees it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CloudHost {
    /// Its pool name.
    pub name: String,
    /// Provider.
    pub provider: String,
    /// USD per hour.
    pub usd_per_hour: f64,
    /// Billing step, seconds.
    pub billing_step_secs: u64,
    /// Minimum billed, seconds.
    pub billing_min_secs: u64,
    /// Unix seconds it was created (billing starts).
    pub started: u64,
    /// Spaces on it now.
    pub running: u32,
    /// Unix seconds it became empty (`None`: it has Spaces).
    pub idle_since: Option<u64>,
    /// Share of its capacity in use (0..1).
    pub used: f64,
    /// Spaces it holds at most (of the typical shape).
    pub slots: u32,
    /// Seconds a new machine like it takes to be ready.
    pub boot_secs: u32,
    /// Already cordoned.
    #[serde(default)]
    pub cordoned: bool,
    /// Pinned by the user (`cua cloud hosts add`): never removed by the
    /// scaler, only by `cua cloud hosts rm`.
    #[serde(default)]
    pub pinned: bool,
}

impl CloudHost {
    /// Unix seconds when the time paid so far ends (the next billing
    /// boundary at or after `now`).
    pub fn paid_until(&self, now: u64) -> u64 {
        let ran = now.saturating_sub(self.started);
        let step = self.billing_step_secs.max(1);
        let billed = ran.max(self.billing_min_secs).div_ceil(step) * step;
        self.started + billed
    }
}

/// How the scaler behaves.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Policy {
    /// Remove an hourly-billed idle machine this many seconds before its
    /// paid time ends (the delete must land before the next step starts).
    pub boundary_margin_secs: u64,
    /// Remove a per-second machine after this long idle.
    pub idle_grace_secs: u64,
    /// Most idle machines kept warm for predicted demand.
    pub max_warm: u32,
    /// Keep machines warm at all.
    pub warm: bool,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            boundary_margin_secs: 180,
            idle_grace_secs: 120,
            max_warm: 1,
            warm: true,
        }
    }
}

/// What to do with one machine.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Action {
    /// Keep it; look again at `until` (unix seconds).
    Keep {
        /// When to decide again.
        until: u64,
        /// Why.
        reason: String,
    },
    /// Stop placing new work on it so it drains.
    Cordon {
        /// Why.
        reason: String,
    },
    /// Remove it now.
    Remove {
        /// Why.
        reason: String,
        /// USD saved versus keeping it another billing step.
        saves_usd: f64,
    },
}

/// One decision.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    /// The machine.
    pub host: String,
    /// What to do.
    pub action: Action,
}

/// Recent demand: Spaces asked for per second, smoothed, and the last few
/// minutes of arrivals (a burst's size so far).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Arrivals {
    /// Exponentially weighted rate, Spaces per second.
    pub rate: f64,
    /// Unix seconds of the last update.
    pub at: u64,
    /// `(second, count)` of the last [`RECENT_SECS`].
    #[serde(default)]
    pub recent: std::collections::VecDeque<(u64, u32)>,
}

/// How far back [`Arrivals::recent_count`] can look.
pub const RECENT_SECS: u64 = 600;

/// Arrivals needed in the window before the steady load is estimated.
pub const STEADY_MIN_ARRIVALS: u32 = 4;

/// Half-life of the arrival rate, seconds.
pub const ARRIVALS_HALF_LIFE_SECS: f64 = 600.0;

impl Arrivals {
    /// Records `n` Spaces asked for at `now`.
    pub fn record(&mut self, n: u32, now: u64) {
        self.decay(now);
        // Each arrival adds its share of one half-life window.
        self.rate += f64::from(n) * std::f64::consts::LN_2 / ARRIVALS_HALF_LIFE_SECS;
        match self.recent.back_mut() {
            Some((t, c)) if *t == now => *c += n,
            _ => self.recent.push_back((now, n)),
        }
        while self
            .recent
            .front()
            .is_some_and(|(t, _)| now.saturating_sub(*t) > RECENT_SECS)
        {
            self.recent.pop_front();
        }
    }

    /// Spaces asked for in the last `window` seconds before `now`: what
    /// the next `window` seconds of a burst are expected to bring.
    pub fn recent_count(&self, now: u64, window: u64) -> u32 {
        self.recent
            .iter()
            .filter(|(t, _)| now.saturating_sub(*t) <= window)
            .map(|(_, c)| *c)
            .sum()
    }

    /// The rate at `now`.
    pub fn rate_at(&self, now: u64) -> f64 {
        let dt = now.saturating_sub(self.at) as f64;
        self.rate * (-(dt * std::f64::consts::LN_2) / ARRIVALS_HALF_LIFE_SECS).exp()
    }

    fn decay(&mut self, now: u64) {
        self.rate = self.rate_at(now);
        self.at = now;
    }
}

/// Spaces expected soon besides the `asking` ones now: the rest of a burst
/// (what arrived within one boot `window` before), or the steady load
/// (Little's law: rate x lifetime) beyond the `running` ones, whichever is
/// more. The steady load needs evidence of a pattern: with fewer than
/// [`STEADY_MIN_ARRIVALS`] arrivals in the window one Space is just one
/// Space, not a rate (a lone Space with an 8 h lifetime would otherwise
/// expect dozens more and buy a 16 vCPU machine).
pub fn expected_more(
    a: &Arrivals,
    now: u64,
    window: u64,
    lifetime_secs: u64,
    running: u32,
    asking: u32,
) -> u32 {
    let burst = a.recent_count(now, window).saturating_sub(asking);
    let seen: u32 = a.recent.iter().map(|(_, c)| *c).sum();
    let steady = if seen < STEADY_MIN_ARRIVALS {
        0
    } else {
        (a.rate_at(now) * lifetime_secs as f64).ceil() as u32
    };
    burst.max(steady.saturating_sub(running + asking))
}

/// Decisions for every cloud machine at `now`.
pub fn decide(hosts: &[CloudHost], arrivals: &Arrivals, now: u64, p: &Policy) -> Vec<Decision> {
    let rate = arrivals.rate_at(now);
    let mut out = Vec::new();
    // Warm buffer: machines kept for demand expected within one boot.
    let mut idle: Vec<&CloudHost> = hosts
        .iter()
        .filter(|h| h.running == 0 && !h.pinned)
        .collect();
    // Cheapest to keep first (they are the ones worth keeping warm).
    idle.sort_by(|a, b| {
        a.usd_per_hour
            .partial_cmp(&b.usd_per_hour)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.name.cmp(&b.name))
    });
    let mut warm_left = if p.warm { p.max_warm } else { 0 };
    for h in &idle {
        let paid_until = h.paid_until(now);
        let hourly = h.billing_step_secs >= 600;
        let left = paid_until.saturating_sub(now);
        // Demand expected while it is paid for anyway, and during a boot.
        let during_paid = rate * left as f64;
        let during_boot = rate * f64::from(h.boot_secs);
        if hourly {
            if left > p.boundary_margin_secs {
                out.push(Decision {
                    host: h.name.clone(),
                    action: Action::Keep {
                        until: paid_until - p.boundary_margin_secs,
                        reason: format!("idle, but paid until {} s from now (billed hourly)", left),
                    },
                });
                continue;
            }
            if warm_left > 0 && during_boot >= 1.0 {
                warm_left -= 1;
                out.push(Decision {
                    host: h.name.clone(),
                    action: Action::Keep {
                        until: paid_until + h.billing_step_secs - p.boundary_margin_secs,
                        reason: format!(
                            "kept warm: {:.1} Spaces expected within a boot ({} s)",
                            during_boot, h.boot_secs
                        ),
                    },
                });
                continue;
            }
            out.push(Decision {
                host: h.name.clone(),
                action: Action::Remove {
                    reason: "idle at the end of its paid hour".into(),
                    saves_usd: h.usd_per_hour * h.billing_step_secs as f64 / 3600.0,
                },
            });
        } else {
            let idle_for = h.idle_since.map_or(0, |s| now.saturating_sub(s));
            let min_left = (h.started + h.billing_min_secs).saturating_sub(now);
            if idle_for < p.idle_grace_secs || min_left > 0 {
                let until = now
                    + (p.idle_grace_secs.saturating_sub(idle_for))
                        .max(min_left)
                        .max(1);
                out.push(Decision {
                    host: h.name.clone(),
                    action: Action::Keep {
                        until,
                        reason: "idle grace".into(),
                    },
                });
                continue;
            }
            if warm_left > 0 && during_boot >= 1.0 {
                warm_left -= 1;
                out.push(Decision {
                    host: h.name.clone(),
                    action: Action::Keep {
                        until: now + u64::from(h.boot_secs).max(60),
                        reason: format!(
                            "kept warm: {:.1} Spaces expected within a boot ({} s)",
                            during_boot, h.boot_secs
                        ),
                    },
                });
                continue;
            }
            let _ = during_paid;
            out.push(Decision {
                host: h.name.clone(),
                action: Action::Remove {
                    reason: format!("idle for {idle_for} s"),
                    saves_usd: h.usd_per_hour * p.idle_grace_secs as f64 / 3600.0,
                },
            });
        }
    }
    // Consolidation: cordon the least-used busy machine when the others
    // have room for what is expected over its remaining work's horizon.
    let mut busy: Vec<&CloudHost> = hosts
        .iter()
        .filter(|h| h.running > 0 && !h.cordoned && !h.pinned)
        .collect();
    // One machine drains at a time.
    let draining = hosts.iter().any(|h| h.cordoned && h.running > 0);
    if busy.len() >= 2 && !draining {
        busy.sort_by(|a, b| {
            a.used
                .partial_cmp(&b.used)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(
                    b.usd_per_hour
                        .partial_cmp(&a.usd_per_hour)
                        .unwrap_or(std::cmp::Ordering::Equal),
                )
                .then(a.name.cmp(&b.name))
        });
        let low = busy[0];
        let room_elsewhere: f64 = busy[1..]
            .iter()
            .map(|h| (1.0 - h.used).max(0.0) * f64::from(h.slots))
            .sum();
        let expected = rate * 1800.0; // the next half hour
        if low.used < 0.3 && room_elsewhere >= expected + f64::from(low.running) {
            out.push(Decision {
                host: low.name.clone(),
                action: Action::Cordon {
                    reason: format!(
                        "{:.0}% used; the other machines have room for {:.0} Spaces ({:.1} expected in 30 min)",
                        low.used * 100.0,
                        room_elsewhere,
                        expected
                    ),
                },
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hz(name: &str, started: u64, running: u32, idle_since: Option<u64>) -> CloudHost {
        CloudHost {
            name: name.into(),
            provider: "hetzner".into(),
            usd_per_hour: 0.0684,
            billing_step_secs: 3600,
            billing_min_secs: 3600,
            started,
            running,
            idle_since,
            used: if running > 0 { 0.5 } else { 0.0 },
            slots: 32,
            boot_secs: 50,
            ..Default::default()
        }
    }

    fn aws(name: &str, started: u64, idle_since: Option<u64>) -> CloudHost {
        CloudHost {
            provider: "aws".into(),
            usd_per_hour: 0.58,
            billing_step_secs: 1,
            billing_min_secs: 60,
            boot_secs: 80,
            ..hz(name, started, 0, idle_since)
        }
    }

    #[test]
    fn hourly_machines_go_at_the_end_of_the_paid_hour() {
        let a = Arrivals::default();
        let h = hz("hz-1", 1000, 0, Some(1500));
        // 10 minutes in: paid for 50 more; keep until 3 minutes before.
        let d = decide(std::slice::from_ref(&h), &a, 1600, &Policy::default());
        assert_eq!(
            d[0].action,
            Action::Keep {
                until: 1000 + 3600 - 180,
                reason: "idle, but paid until 3000 s from now (billed hourly)".into()
            }
        );
        let d = decide(&[h], &a, 1000 + 3600 - 100, &Policy::default());
        assert!(matches!(d[0].action, Action::Remove { .. }), "{d:?}");
    }

    #[test]
    fn per_second_machines_go_after_a_short_grace() {
        let a = Arrivals::default();
        let h = aws("aws-1", 1000, Some(1100));
        assert!(matches!(
            decide(std::slice::from_ref(&h), &a, 1150, &Policy::default())[0].action,
            Action::Keep { .. }
        ));
        assert!(matches!(
            decide(&[h], &a, 1300, &Policy::default())[0].action,
            Action::Remove { .. }
        ));
    }

    #[test]
    fn warm_only_when_demand_predicts_it() {
        let mut a = Arrivals::default();
        let h = aws("aws-1", 1000, Some(1100));
        // A steady 0.05 Spaces/s: 4 expected within an 80 s boot.
        for t in (0..1300).step_by(20) {
            a.record(1, t);
        }
        let d = decide(
            &[h.clone(), aws("aws-2", 1000, Some(1100))],
            &a,
            1300,
            &Policy::default(),
        );
        let kept = d
            .iter()
            .filter(|x| matches!(x.action, Action::Keep { .. }))
            .count();
        assert_eq!(kept, 1, "one warm machine at most: {d:?}");
        // Demand gone an hour later: removed.
        let d = decide(&[h], &a, 1300 + 4 * 3600, &Policy::default());
        assert!(matches!(d[0].action, Action::Remove { .. }));
    }

    #[test]
    fn the_least_used_machine_is_cordoned_when_others_have_room() {
        let a = Arrivals::default();
        let mut low = hz("hz-low", 0, 2, None);
        low.used = 0.06;
        let mut mid = hz("hz-mid", 0, 10, None);
        mid.used = 0.3;
        let d = decide(&[low, mid], &a, 100, &Policy::default());
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].host, "hz-low");
        assert!(matches!(d[0].action, Action::Cordon { .. }));
    }

    #[test]
    fn pinned_machines_stay() {
        let mut h = aws("aws-1", 0, Some(0));
        h.pinned = true;
        assert!(decide(&[h], &Arrivals::default(), 99_999, &Policy::default()).is_empty());
    }
}
