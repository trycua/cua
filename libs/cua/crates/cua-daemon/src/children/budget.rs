//! A tree's budget: how many Spaces, vCPUs and memory its children may
//! hold at once, and what they may cost (per hour and in total).
//!
//! The owner sets it on the tree's top-level Space (`<home>/spawn/
//! budgets.json`); unset fields take the defaults below, sized so a runaway
//! loop is cheap. A create or fork of a child reserves what it asks before
//! it starts ([`Ledger::reserve`]), so concurrent creates can never pass the
//! budget together: one that does not fit is refused with what is left. The
//! reservation becomes the child's use when it exists, and is dropped when
//! the create fails.
//!
//! Children on this machine cost nothing per hour; children in a linked
//! cloud cost what the cloud bills, and the account's own cloud budget is
//! checked as well (the stricter wins).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Live children of one tree, unless the owner says more.
pub const DEFAULT_MAX_SPACES: u32 = 16;
/// vCPUs its children hold together.
pub const DEFAULT_MAX_VCPUS: u32 = 16;
/// Memory its children hold together (MiB).
pub const DEFAULT_MAX_MEMORY_MB: u64 = 32 * 1024;
/// What its children may cost per hour in linked clouds.
pub const DEFAULT_USD_PER_HOUR: f64 = 2.0;
/// A reservation not settled in this long is dropped (a create that
/// crashed).
pub const RESERVATION_TTL: Duration = Duration::from_secs(30 * 60);

/// The owner's limits for one tree (unset: the default).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Budget {
    /// Live children at once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_spaces: Option<u32>,
    /// vCPUs at once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_vcpus: Option<u32>,
    /// Memory at once (MiB).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_memory_mb: Option<u64>,
    /// Cost per hour of everything running at once ($).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usd_per_hour: Option<f64>,
    /// Cost in total, over the tree's life ($). Unset: no total cap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usd_total: Option<f64>,
}

impl Budget {
    /// With the defaults filled in.
    pub fn effective(&self) -> Limits {
        Limits {
            max_spaces: self.max_spaces.unwrap_or(DEFAULT_MAX_SPACES),
            max_vcpus: self.max_vcpus.unwrap_or(DEFAULT_MAX_VCPUS),
            max_memory_mb: self.max_memory_mb.unwrap_or(DEFAULT_MAX_MEMORY_MB),
            usd_per_hour: self.usd_per_hour.unwrap_or(DEFAULT_USD_PER_HOUR),
            usd_total: self.usd_total,
        }
    }
}

/// A budget with every field decided.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limits {
    /// Live children at once.
    pub max_spaces: u32,
    /// vCPUs at once.
    pub max_vcpus: u32,
    /// Memory at once (MiB).
    pub max_memory_mb: u64,
    /// $ per hour at once.
    pub usd_per_hour: f64,
    /// $ in total.
    pub usd_total: Option<f64>,
}

/// What one child holds, or asks for.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Ask {
    /// vCPUs.
    pub vcpus: u32,
    /// Memory (MiB).
    pub memory_mb: u64,
    /// $ per hour (0 on this machine).
    pub usd_per_hour: f64,
}

/// What a tree's children hold now and have spent.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Usage {
    /// Live children.
    pub spaces: u32,
    /// vCPUs they hold.
    pub vcpus: u32,
    /// Memory they hold (MiB).
    pub memory_mb: u64,
    /// $ per hour they cost.
    pub usd_per_hour: f64,
    /// $ spent so far.
    pub usd_spent: f64,
}

impl Usage {
    fn plus(mut self, a: &Ask) -> Self {
        self.spaces += 1;
        self.vcpus += a.vcpus;
        self.memory_mb += a.memory_mb;
        self.usd_per_hour += a.usd_per_hour;
        self
    }
}

/// Why a child does not fit: the limit that stopped it, in words the
/// agent can read and act on.
#[derive(Clone, Debug, PartialEq)]
pub struct Refusal(pub String);

/// Checks `ask` against `limits` given what is used and reserved.
pub fn fits(limits: &Limits, used: &Usage, ask: &Ask) -> Result<(), Refusal> {
    let after = used.plus(ask);
    if after.spaces > limits.max_spaces {
        return Err(Refusal(format!(
            "this tree may hold {} child Spaces at once and holds {}; delete one, or ask the \
             owner to raise it (`cua spaces budget <root> --spaces N`)",
            limits.max_spaces, used.spaces
        )));
    }
    if after.vcpus > limits.max_vcpus {
        return Err(Refusal(format!(
            "this child asks for {} vCPUs and the tree has {} of {} left; ask for fewer, or ask \
             the owner to raise it (`--cpus N`)",
            ask.vcpus,
            limits.max_vcpus.saturating_sub(used.vcpus),
            limits.max_vcpus
        )));
    }
    if after.memory_mb > limits.max_memory_mb {
        return Err(Refusal(format!(
            "this child asks for {} MiB and the tree has {} of {} MiB left; ask for less, or ask \
             the owner to raise it (`--memory MiB`)",
            ask.memory_mb,
            limits.max_memory_mb.saturating_sub(used.memory_mb),
            limits.max_memory_mb
        )));
    }
    if ask.usd_per_hour > 0.0 && after.usd_per_hour > limits.usd_per_hour + 1e-9 {
        return Err(Refusal(format!(
            "this child costs ${:.2}/h and the tree has ${:.2}/h of ${:.2}/h left; pick a \
             cheaper machine, or ask the owner to raise it (`--usd-per-hour N`)",
            ask.usd_per_hour,
            (limits.usd_per_hour - used.usd_per_hour).max(0.0),
            limits.usd_per_hour
        )));
    }
    if let Some(total) = limits.usd_total
        && ask.usd_per_hour > 0.0
        && used.usd_spent + ask.usd_per_hour > total + 1e-9
    {
        return Err(Refusal(format!(
            "this tree has spent ${:.2} of its ${total:.2}, and one hour of this child \
             (${:.2}) would pass it; ask the owner to raise it (`--usd-total N`)",
            used.usd_spent, ask.usd_per_hour
        )));
    }
    Ok(())
}

struct Reservation {
    id: u64,
    root: String,
    ask: Ask,
    at: Instant,
}

/// What one child was made with, for its tree's use and spend.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Cost {
    /// Its tree.
    pub root: String,
    /// vCPUs.
    pub vcpus: u32,
    /// Memory (MiB).
    pub memory_mb: u64,
    /// $ per hour while it lives.
    pub usd_per_hour: f64,
    /// Since when (unix seconds).
    pub since: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Costs {
    /// Live children.
    #[serde(default)]
    children: BTreeMap<String, Cost>,
    /// $ spent by children that are gone, per tree.
    #[serde(default)]
    spent: BTreeMap<String, f64>,
}

/// The owner's budgets and the reservations of creates in flight.
pub struct Ledger {
    path: PathBuf,
    budgets: Mutex<BTreeMap<String, Budget>>,
    reservations: Mutex<(u64, Vec<Reservation>)>,
    costs: Mutex<Costs>,
}

/// A reservation; [`Ledger::settle`] or [`Ledger::release`] it.
#[derive(Debug)]
#[must_use]
pub struct Ticket(u64);

impl Ledger {
    /// Opens `<home>/spawn/budgets.json`.
    pub fn open(home: &Path) -> Self {
        let path = home.join("spawn").join("budgets.json");
        let budgets = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let costs = std::fs::read(path.with_file_name("costs.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self {
            path,
            budgets: Mutex::new(budgets),
            reservations: Mutex::new((1, Vec::new())),
            costs: Mutex::new(costs),
        }
    }

    fn save_costs(&self, costs: &Costs) {
        let p = self.path.with_file_name("costs.json");
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let tmp = p.with_extension("json.tmp");
        if std::fs::write(&tmp, serde_json::to_vec_pretty(costs).unwrap_or_default()).is_ok() {
            let _ = std::fs::rename(&tmp, &p);
        }
    }

    /// Records a child made in `root`'s tree with `ask`, from `now`.
    pub fn record(&self, root: &str, child: &str, ask: &Ask, now: u64) {
        let mut c = self.costs.lock().unwrap();
        c.children.insert(
            child.to_owned(),
            Cost {
                root: root.to_owned(),
                vcpus: ask.vcpus,
                memory_mb: ask.memory_mb,
                usd_per_hour: ask.usd_per_hour,
                since: now,
            },
        );
        self.save_costs(&c);
    }

    /// What `child` was made with, if recorded.
    pub fn cost(&self, child: &str) -> Option<Cost> {
        self.costs.lock().unwrap().children.get(child).cloned()
    }

    /// Closes the records of children not in `live`: what they cost goes to
    /// their tree's spend.
    pub fn close_gone(&self, live: &std::collections::BTreeSet<String>, now: u64) {
        let mut c = self.costs.lock().unwrap();
        let gone: Vec<String> = c
            .children
            .keys()
            .filter(|k| !live.contains(*k))
            .cloned()
            .collect();
        if gone.is_empty() {
            return;
        }
        for id in gone {
            if let Some(x) = c.children.remove(&id) {
                *c.spent.entry(x.root.clone()).or_default() += cost_so_far(&x, now);
            }
        }
        self.save_costs(&c);
    }

    /// `root`'s tree: $ per hour of its live children, and $ spent in all.
    pub fn spend(&self, root: &str, now: u64) -> (f64, f64) {
        let c = self.costs.lock().unwrap();
        let live = c.children.values().filter(|x| x.root == root);
        let per_hour = live.clone().map(|x| x.usd_per_hour).sum();
        let spent = c.spent.get(root).copied().unwrap_or(0.0)
            + live.map(|x| cost_so_far(x, now)).sum::<f64>();
        (per_hour, spent)
    }

    /// The budget of `root`'s tree.
    pub fn budget(&self, root: &str) -> Budget {
        self.budgets
            .lock()
            .unwrap()
            .get(root)
            .cloned()
            .unwrap_or_default()
    }

    /// Sets the budget of `root`'s tree (`Budget::default()` removes it).
    pub fn set_budget(&self, root: &str, budget: Budget) -> std::io::Result<()> {
        let mut all = self.budgets.lock().unwrap();
        if budget == Budget::default() {
            all.remove(root);
        } else {
            all.insert(root.to_owned(), budget);
        }
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&*all).unwrap_or_default())?;
        std::fs::rename(&tmp, &self.path)
    }

    /// What creates in flight hold in `root`'s tree.
    pub fn reserved(&self, root: &str) -> Usage {
        let mut r = self.reservations.lock().unwrap();
        r.1.retain(|x| x.at.elapsed() < RESERVATION_TTL);
        r.1.iter()
            .filter(|x| x.root == root)
            .fold(Usage::default(), |u, x| u.plus(&x.ask))
    }

    /// Reserves `ask` in `root`'s tree when it fits what is `used` (the
    /// live children) plus what is reserved, all under one lock so two
    /// creates cannot both take the last room.
    pub fn reserve(
        &self,
        root: &str,
        limits: &Limits,
        used: &Usage,
        ask: Ask,
    ) -> Result<Ticket, Refusal> {
        let mut r = self.reservations.lock().unwrap();
        r.1.retain(|x| x.at.elapsed() < RESERVATION_TTL);
        let held =
            r.1.iter()
                .filter(|x| x.root == root)
                .fold(*used, |u, x| u.plus(&x.ask));
        fits(limits, &held, &ask)?;
        let id = r.0;
        r.0 += 1;
        r.1.push(Reservation {
            id,
            root: root.to_owned(),
            ask,
            at: Instant::now(),
        });
        Ok(Ticket(id))
    }

    /// The create finished (its child now counts as used) or failed: the
    /// reservation goes either way.
    pub fn release(&self, ticket: Ticket) {
        self.reservations
            .lock()
            .unwrap()
            .1
            .retain(|x| x.id != ticket.0);
    }
}

fn cost_so_far(c: &Cost, now: u64) -> f64 {
    c.usd_per_hour * now.saturating_sub(c.since) as f64 / 3600.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(vcpus: u32, memory_mb: u64, usd: f64) -> Ask {
        Ask {
            vcpus,
            memory_mb,
            usd_per_hour: usd,
        }
    }

    #[test]
    fn defaults_and_plain_refusals() {
        let l = Budget::default().effective();
        assert_eq!(
            (l.max_spaces, l.max_vcpus, l.max_memory_mb),
            (16, 16, 32 * 1024)
        );
        let used = Usage {
            spaces: 3,
            vcpus: 14,
            memory_mb: 4096,
            ..Default::default()
        };
        let e = fits(&l, &used, &ask(4, 1024, 0.0)).unwrap_err();
        assert!(e.0.contains("2 of 16 left"), "{}", e.0);
        assert!(fits(&l, &used, &ask(2, 1024, 0.0)).is_ok());
        let e = fits(&l, &used, &ask(1, 40 * 1024, 0.0)).unwrap_err();
        assert!(e.0.contains("MiB"), "{}", e.0);
        let full = Usage {
            spaces: 16,
            ..Default::default()
        };
        assert!(
            fits(&l, &full, &ask(1, 1, 0.0))
                .unwrap_err()
                .0
                .contains("16 child")
        );
    }

    #[test]
    fn dollars_per_hour_and_total() {
        let l = Budget {
            usd_per_hour: Some(1.0),
            usd_total: Some(5.0),
            ..Default::default()
        }
        .effective();
        let used = Usage {
            usd_per_hour: 0.6,
            usd_spent: 4.5,
            ..Default::default()
        };
        // Local children cost nothing: never refused on $.
        assert!(fits(&l, &used, &ask(1, 1, 0.0)).is_ok());
        assert!(
            fits(&l, &used, &ask(1, 1, 0.5))
                .unwrap_err()
                .0
                .contains("/h")
        );
        let used = Usage {
            usd_spent: 4.8,
            ..Default::default()
        };
        assert!(
            fits(&l, &used, &ask(1, 1, 0.3))
                .unwrap_err()
                .0
                .contains("spent")
        );
    }

    #[test]
    fn concurrent_reservations_never_pass_the_budget() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = std::sync::Arc::new(Ledger::open(dir.path()));
        let limits = Budget {
            max_spaces: Some(5),
            ..Default::default()
        }
        .effective();
        let granted = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let threads: Vec<_> = (0..32)
            .map(|_| {
                let (ledger, granted) = (ledger.clone(), granted.clone());
                std::thread::spawn(move || {
                    if let Ok(t) = ledger.reserve("r", &limits, &Usage::default(), ask(1, 512, 0.0))
                    {
                        granted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        std::mem::forget(t);
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        assert_eq!(granted.load(std::sync::atomic::Ordering::SeqCst), 5);
        assert_eq!(ledger.reserved("r").spaces, 5);
        // Another tree has its own room.
        assert!(
            ledger
                .reserve("other", &limits, &Usage::default(), ask(1, 1, 0.0))
                .is_ok()
        );
    }

    #[test]
    fn a_released_reservation_frees_its_room_and_budgets_persist() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::open(dir.path());
        let limits = Budget {
            max_spaces: Some(1),
            ..Default::default()
        }
        .effective();
        let t = ledger
            .reserve("r", &limits, &Usage::default(), ask(1, 1, 0.0))
            .unwrap();
        assert!(
            ledger
                .reserve("r", &limits, &Usage::default(), ask(1, 1, 0.0))
                .is_err()
        );
        ledger.release(t);
        let t = ledger
            .reserve("r", &limits, &Usage::default(), ask(1, 1, 0.0))
            .unwrap();
        ledger.release(t);
        let b = Budget {
            max_vcpus: Some(64),
            usd_total: Some(10.0),
            ..Default::default()
        };
        ledger.set_budget("r", b.clone()).unwrap();
        assert_eq!(Ledger::open(dir.path()).budget("r"), b);
        ledger.set_budget("r", Budget::default()).unwrap();
        assert_eq!(Ledger::open(dir.path()).budget("r"), Budget::default());
    }

    #[test]
    fn spend_is_live_cost_plus_what_gone_children_cost() {
        let dir = tempfile::tempdir().unwrap();
        let l = Ledger::open(dir.path());
        l.record("r", "c1", &ask(2, 1024, 0.5), 1000);
        l.record("r", "c2", &ask(2, 1024, 1.0), 1000);
        l.record("other", "o1", &ask(2, 1024, 9.0), 1000);
        let (h, spent) = l.spend("r", 1000 + 3600);
        assert!((h - 1.5).abs() < 1e-9 && (spent - 1.5).abs() < 1e-9);
        // c2 is deleted: it stops costing per hour, and what it cost stays.
        l.close_gone(&["c1".to_string(), "o1".to_string()].into(), 1000 + 3600);
        let (h, spent) = l.spend("r", 1000 + 7200);
        assert!((h - 0.5).abs() < 1e-9, "{h}");
        assert!((spent - (1.0 + 1.0)).abs() < 1e-9, "{spent}");
        // And it survives a restart.
        let back = Ledger::open(dir.path());
        assert!((back.spend("r", 1000 + 7200).1 - 2.0).abs() < 1e-9);
        assert_eq!(back.cost("c1").unwrap().vcpus, 2);
    }
}
