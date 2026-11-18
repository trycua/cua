//! When a shape gets a running warm member: an adaptive keep-alive and
//! pre-warm policy over the shape's create inter-arrival times.
//!
//! A running member costs memory and CPU the whole time it waits (a gVisor
//! desktop: about 200 MiB and 15% of a core), so by default none runs: a
//! create takes the cheapest tier that has no standing cost (a backend's
//! snapshot restore where it has one, else a cold boot), as snapshot-restore
//! systems do (REAP, ASPLOS'21; Catalyzer, ASPLOS'20; AWS Lambda SnapStart on
//! Firecracker snapshots). A member is kept only when the shape's own
//! traffic says it pays, and only for as long as it does.
//!
//! The policy adapts the hybrid histogram policy of Shahrad et al.,
//! "Serverless in the Wild" (USENIX ATC'20): from the distribution of a
//! function's inter-arrival times (IATs) it derives a *pre-warm* window
//! (load the instance shortly before the head of the distribution) and a
//! *keep-alive* window (unload it after the tail), instead of a fixed
//! keep-alive. Their Azure Functions trace shows why a fixed one is wrong:
//! most functions are invoked rarely (45% at most once an hour, 81% at most
//! once a minute), so a fixed keep-alive mostly keeps idle instances.
//!
//! Creates of Spaces come in bursts (an agent or a batch creating Spaces in
//! a loop) separated by gaps, so the IATs are split at a burst gap `G`
//! (three member boot times) into two histograms:
//!
//! - **Within a burst** (IATs up to `G`): while a burst runs, members are
//!   kept ready and sized by Little's law (`ceil(rate x boot) + 1`) and by
//!   the largest burst seen within one boot time, so a steady batch never
//!   waits for a refill.
//! - **Between bursts** (IATs above `G`): when the gaps are regular (a
//!   periodic job), members are pre-warmed for the predicted next burst:
//!   started at `0.9 x p5` of the gaps (less the boot time) and kept until
//!   `1.1 x p99`, Shahrad's head and tail with their 10% margins.
//!
//! Both windows are bounded the way the ski-rental problem bounds renting
//! (Karlin et al., "Competitive randomized algorithms for nonuniform
//! problems", SODA'90; applied to keep-alive by FaasCache, Fuerst & Sharma,
//! ASPLOS'21, which casts keep-alive as caching): renting a member costs
//! member-seconds, buying is one slow create, and a member is rented for at
//! most the break-even time `B = V x saved` (the seconds a hit saves over
//! the default tier, priced at `V` member-seconds per second saved). A
//! window longer than `B`, or one that would catch fewer than half of the
//! observed arrivals, is not worth it, and no member runs. Without repeat
//! traffic (fewer than two IATs within the four-hour horizon of the
//! histograms, as in Shahrad's policy) nothing runs: a one-off create costs
//! nothing extra.
//!
//! IceBreaker (ASPLOS'22) and Aquatope (ASPLOS'23) predict arrivals with
//! learned models; this policy keeps to percentiles and one bound so every
//! decision can be stated in a sentence (`cua daemon status`). SOCK
//! (USENIX ATC'18) caches warm *zygotes* per package set; a shape here plays
//! the zygote's role (the image digest and resources a Space starts from).
//!
//! `V` is smaller on a personal machine (a Mac) than on a dedicated host
//! (Linux), and on battery nothing runs. Explicit pools bypass all of this
//! and keep exactly the members asked for.

use std::collections::VecDeque;
use std::fmt;

/// How far back the histograms look (seconds); Shahrad et al. use 4 h.
pub const HORIZON_S: f64 = 4.0 * 3600.0;

/// Arrivals remembered per shape.
const MAX_ARRIVALS: usize = 256;

/// Fewest IATs a decision rests on.
const MIN_IATS: usize = 2;

/// Fewest gaps (between-burst IATs) a pre-warm rests on.
const MIN_GAPS: usize = 2;

/// Pre-warm and keep-alive margins of the hybrid histogram policy.
const HEAD_MARGIN: f64 = 0.9;
const TAIL_MARGIN: f64 = 1.1;

/// Member-seconds a second saved is worth on a personal machine.
pub const V_PERSONAL: f64 = 60.0;
/// ... and on a dedicated host.
pub const V_DEDICATED: f64 = 300.0;

/// Where the daemon runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostClass {
    /// A person's own machine (a Mac): conservative.
    Personal,
    /// A host that runs Spaces for a living (Linux).
    Dedicated,
}

impl HostClass {
    /// This machine's class.
    pub fn current() -> Self {
        if cfg!(target_os = "linux") {
            Self::Dedicated
        } else {
            Self::Personal
        }
    }

    fn value(self) -> f64 {
        match self {
            Self::Personal => V_PERSONAL,
            Self::Dedicated => V_DEDICATED,
        }
    }
}

/// What a shape's members cost and save, as measured (seconds).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Costs {
    /// A member's start to ready (its refill time).
    pub boot_s: f64,
    /// A create's ready time on the default tier (snapshot restore or cold
    /// boot), less a claim's.
    pub saved_s: f64,
}

/// The creates of one shape, in seconds on a monotonic clock.
#[derive(Clone, Debug, Default)]
pub struct Arrivals {
    times: VecDeque<f64>,
}

impl Arrivals {
    /// Records a create at `t`.
    pub fn record(&mut self, t: f64) {
        if self.times.back().is_some_and(|b| t < *b) {
            return;
        }
        self.times.push_back(t);
        while self.times.len() > MAX_ARRIVALS {
            self.times.pop_front();
        }
    }

    /// The last create.
    pub fn last(&self) -> Option<f64> {
        self.times.back().copied()
    }

    /// Creates within the horizon before `now`.
    fn recent(&self, now: f64) -> Vec<f64> {
        self.times
            .iter()
            .copied()
            .filter(|t| now - t <= HORIZON_S)
            .collect()
    }
}

/// Why a shape has the members it has.
#[derive(Clone, Debug, PartialEq)]
pub enum Reason {
    /// Fewer than two IATs in the horizon.
    NoRepeat,
    /// Warm pools are off on this machine.
    Off,
    /// The machine runs on battery.
    OnBattery,
    /// The arrivals are too far apart or too irregular to pay: what a
    /// window would need (seconds) against what it may cost.
    TooSparse { needed_s: f64, bound_s: f64 },
    /// Creates came, but none for `since_s`, and nothing predicts the next.
    Quiet { since_s: f64 },
    /// A burst runs: members kept until `until`.
    Burst { per_min: f64, members: u32 },
    /// The next burst is predicted: members from `from` to `until`.
    Prewarm { in_s: f64, members: u32 },
    /// Between predicted bursts: nothing runs until the next window.
    Waiting { in_s: f64 },
    /// An explicit pool: exactly this many.
    Explicit(u32),
}

impl fmt::Display for Reason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let dur = |s: f64| -> String {
            if s < 120.0 {
                format!("{s:.0} s")
            } else if s < 7200.0 {
                format!("{:.0} min", s / 60.0)
            } else {
                format!("{:.1} h", s / 3600.0)
            }
        };
        match self {
            Self::NoRepeat => f.write_str("no repeat traffic"),
            Self::Off => f.write_str("warm pools are off"),
            Self::OnBattery => f.write_str("on battery"),
            Self::TooSparse { needed_s, bound_s } => write!(
                f,
                "creates too far apart to pay (a window of {} > {})",
                dur(*needed_s),
                dur(*bound_s)
            ),
            Self::Quiet { since_s } => {
                write!(f, "no creates for {}; none expected soon", dur(*since_s))
            }
            Self::Burst { per_min, members } => {
                write!(f, "burst: {per_min:.1} creates/min, {members} kept ready")
            }
            Self::Prewarm { in_s, members } => write!(
                f,
                "next creates expected within {}, {members} ready",
                dur(*in_s)
            ),
            Self::Waiting { in_s } => {
                write!(f, "periodic creates; warming again in {}", dur(*in_s))
            }
            Self::Explicit(n) => write!(f, "pool of {n}"),
        }
    }
}

/// What a shape should have now.
#[derive(Clone, Debug, PartialEq)]
pub struct Decision {
    /// Members wanted ready now.
    pub members: u32,
    /// When the decision may change without a new create (seconds on the
    /// same clock): the end or start of a window.
    pub next_change: Option<f64>,
    /// Why.
    pub reason: Reason,
}

impl Decision {
    fn none(reason: Reason) -> Self {
        Self {
            members: 0,
            next_change: None,
            reason,
        }
    }
}

/// The `p`th percentile (nearest rank) of sorted `v`.
fn pct(v: &[f64], p: f64) -> f64 {
    let rank = ((p / 100.0) * v.len() as f64).ceil().max(1.0) as usize;
    v[rank.min(v.len()) - 1]
}

/// The decision for a shape at `now` (see the module docs). `max_members`
/// caps the members one shape may hold (memory decides the rest).
pub fn decide(
    arrivals: &Arrivals,
    now: f64,
    costs: Costs,
    class: HostClass,
    on_battery: bool,
    max_members: u32,
) -> Decision {
    if on_battery {
        return Decision::none(Reason::OnBattery);
    }
    let times = arrivals.recent(now);
    let iats: Vec<f64> = times.windows(2).map(|w| w[1] - w[0]).collect();
    if iats.len() < MIN_IATS {
        return Decision::none(Reason::NoRepeat);
    }
    let Some(&last) = times.last() else {
        return Decision::none(Reason::NoRepeat);
    };
    let boot = costs.boot_s.max(0.1);
    let bound = class.value() * costs.saved_s.max(0.0);
    let gap = 3.0 * boot;
    let mut short: Vec<f64> = iats.iter().copied().filter(|i| *i <= gap).collect();
    let mut long: Vec<f64> = iats.iter().copied().filter(|i| *i > gap).collect();
    short.sort_by(f64::total_cmp);
    long.sort_by(f64::total_cmp);
    let since = now - last;

    // Within a burst: the creates so far say another follows soon.
    let in_burst = since <= gap && !short.is_empty();
    if in_burst {
        let keep = (TAIL_MARGIN * pct(&short, 99.0)).max(boot);
        if keep > bound {
            return Decision::none(Reason::TooSparse {
                needed_s: keep,
                bound_s: bound,
            });
        }
        let members = burst_members(&times, boot).min(max_members.max(1));
        let rate = 60.0 / pct(&short, 50.0).max(0.001);
        return Decision {
            members,
            next_change: Some(last + keep.max(gap)),
            reason: Reason::Burst {
                per_min: rate,
                members,
            },
        };
    }

    // Between bursts: pre-warm for the next one when the gaps are regular.
    if long.len() < MIN_GAPS {
        return Decision::none(Reason::Quiet { since_s: since });
    }
    let head = HEAD_MARGIN * pct(&long, 5.0);
    let tail = TAIL_MARGIN * pct(&long, 99.0);
    // The window starts early enough for a member to be ready.
    let from = last + (head - 1.5 * boot).max(0.0);
    let until = last + tail;
    let width = until - from;
    if width > bound {
        return Decision::none(Reason::TooSparse {
            needed_s: width,
            bound_s: bound,
        });
    }
    let members = typical_burst(&times, gap).min(max_members.max(1));
    if now < from {
        return Decision {
            members: 0,
            next_change: Some(from),
            reason: Reason::Waiting { in_s: from - now },
        };
    }
    if now < until {
        return Decision {
            members,
            next_change: Some(until),
            reason: Reason::Prewarm {
                in_s: (last + head - now).max(0.0),
                members,
            },
        };
    }
    // Past the window and nothing came: wait for the next create.
    Decision::none(Reason::Quiet { since_s: since })
}

/// Members a running burst needs: Little's law over the recent rate (the
/// creates a member boot time covers, plus one), or the most creates seen
/// within one boot time, whichever is larger.
fn burst_members(times: &[f64], boot: f64) -> u32 {
    let n = times.len();
    let recent = &times[n.saturating_sub(16)..];
    let span = recent.last().unwrap_or(&0.0) - recent.first().unwrap_or(&0.0);
    let rate = if span > 0.0 {
        (recent.len() - 1) as f64 / span
    } else {
        0.0
    };
    let little = (rate * boot).ceil() as u32 + 1;
    let mut most = 1u32;
    for (i, t) in recent.iter().enumerate() {
        let within = recent[i..].iter().take_while(|u| **u - t <= boot).count() as u32;
        most = most.max(within);
    }
    little.max(most).max(1)
}

/// The median size of past bursts (creates separated by at most `gap`).
fn typical_burst(times: &[f64], gap: f64) -> u32 {
    let mut sizes = Vec::new();
    let mut size = 1u32;
    for w in times.windows(2) {
        if w[1] - w[0] <= gap {
            size += 1;
        } else {
            sizes.push(size);
            size = 1;
        }
    }
    sizes.push(size);
    sizes.sort_unstable();
    sizes[sizes.len() / 2].max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GV: Costs = Costs {
        boot_s: 5.0,
        saved_s: 4.0,
    };

    fn arrivals(ts: &[f64]) -> Arrivals {
        let mut a = Arrivals::default();
        for t in ts {
            a.record(*t);
        }
        a
    }

    fn d(ts: &[f64], now: f64, class: HostClass) -> Decision {
        decide(&arrivals(ts), now, GV, class, false, 8)
    }

    #[test]
    fn a_one_off_create_keeps_nothing() {
        let r = d(&[0.0], 1.0, HostClass::Dedicated);
        assert_eq!(r.members, 0);
        assert_eq!(r.reason, Reason::NoRepeat);
        assert_eq!(r.reason.to_string(), "no repeat traffic");
        let r = d(&[0.0, 5000.0], 5001.0, HostClass::Dedicated);
        assert_eq!(r.members, 0, "two creates are one IAT");
    }

    #[test]
    fn rare_creates_keep_nothing() {
        // Every 90 min: a window would cost far more than it saves.
        let ts: Vec<f64> = (0..3).map(|i| i as f64 * 5400.0).collect();
        let r = d(&ts, 11_000.0, HostClass::Personal);
        assert_eq!(r.members, 0);
        assert!(
            matches!(r.reason, Reason::TooSparse { .. }),
            "{:?}",
            r.reason
        );
        // Beyond the four-hour horizon a create is no repeat at all.
        let ts = [0.0, 15_000.0];
        assert_eq!(
            d(&ts, 15_001.0, HostClass::Dedicated).reason,
            Reason::NoRepeat
        );
    }

    #[test]
    fn a_steady_batch_keeps_enough_for_the_refill_time() {
        // One create every 2 s with 5 s boots: Little's law, 3 + 1.
        let ts: Vec<f64> = (0..20).map(|i| i as f64 * 2.0).collect();
        let r = d(&ts, 38.5, HostClass::Personal);
        assert!(r.members >= 4, "{r:?}");
        assert!(matches!(r.reason, Reason::Burst { .. }));
        // It ends quickly once the creates stop.
        let r = d(&ts, 38.0 + 3.0 * 5.0 + 1.0, HostClass::Personal);
        assert_eq!(r.members, 0, "{r:?}");
        assert_eq!(
            r.reason.to_string(),
            "no creates for 16 s; none expected soon"
        );
    }

    #[test]
    fn a_periodic_job_is_prewarmed_just_before_it_comes() {
        // Every 10 min on a dedicated host.
        let ts: Vec<f64> = (0..5).map(|i| i as f64 * 600.0).collect();
        let last = 2400.0;
        let waiting = d(&ts, last + 60.0, HostClass::Dedicated);
        assert_eq!(waiting.members, 0);
        let Reason::Waiting { .. } = waiting.reason else {
            panic!("{waiting:?}")
        };
        let from = waiting.next_change.unwrap();
        assert!(from > last + 500.0 && from < last + 540.0, "{from}");
        let warm = d(&ts, from + 1.0, HostClass::Dedicated);
        assert_eq!(warm.members, 1, "{warm:?}");
        assert!(warm.next_change.unwrap() <= last + 660.0 + 1e-6);
        // Past the window, nothing.
        let past = d(&ts, last + 700.0, HostClass::Dedicated);
        assert_eq!(past.members, 0);
        assert!(matches!(past.reason, Reason::Quiet { .. }));
    }

    #[test]
    fn a_personal_machine_does_not_prewarm_what_a_dedicated_host_would() {
        // Every 10 min, but the gaps vary a lot: a wide window.
        let ts = [0.0, 400.0, 1200.0, 1600.0, 2400.0];
        let p = d(&ts, 2400.0 + 300.0, HostClass::Personal);
        assert_eq!(p.members, 0, "{p:?}");
        assert!(matches!(p.reason, Reason::TooSparse { .. }));
    }

    #[test]
    fn bursts_are_sized_and_prewarmed() {
        // Bursts of 5 creates 1 s apart, every 15 min.
        let mut ts = Vec::new();
        for b in 0..4 {
            for i in 0..5 {
                ts.push(b as f64 * 900.0 + i as f64);
            }
        }
        let last = *ts.last().unwrap();
        let mid = d(
            &ts[..ts.len() - 2],
            ts[ts.len() - 3] + 0.5,
            HostClass::Dedicated,
        );
        assert!(mid.members >= 5, "a burst is covered: {mid:?}");
        let before_next = last - 4.0 + 900.0 - 3.0;
        let pre = d(&ts, before_next, HostClass::Dedicated);
        assert_eq!(pre.members, 5, "the next burst is prewarmed: {pre:?}");
    }

    #[test]
    fn nothing_runs_on_battery_or_when_snapshots_are_as_good() {
        let ts: Vec<f64> = (0..20).map(|i| i as f64 * 2.0).collect();
        let r = decide(&arrivals(&ts), 38.5, GV, HostClass::Dedicated, true, 8);
        assert_eq!(r.reason, Reason::OnBattery);
        // A snapshot restore within 50 ms of a claim saves next to nothing.
        let cheap = Costs {
            boot_s: 5.0,
            saved_s: 0.05,
        };
        let r = decide(&arrivals(&ts), 38.5, cheap, HostClass::Personal, false, 8);
        assert_eq!(r.members, 0, "{r:?}");
    }

    #[test]
    fn reasons_read_as_sentences() {
        assert_eq!(
            Reason::Burst {
                per_min: 30.0,
                members: 3
            }
            .to_string(),
            "burst: 30.0 creates/min, 3 kept ready"
        );
        assert_eq!(
            Reason::Waiting { in_s: 540.0 }.to_string(),
            "periodic creates; warming again in 9 min"
        );
    }
}
