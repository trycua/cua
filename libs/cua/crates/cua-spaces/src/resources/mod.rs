//! What this Mac, its extra Spaces and your other computers use: processor,
//! memory and disk, for the apps, the menu bar and the CLI.
//!
//! * [`Resources`] is one snapshot: [`ComputerResources`] for this Mac, a
//!   [`SpaceResources`] per extra (local) Space, and one
//!   [`ComputerResources`] per other computer that published a summary.
//! * [`Spaces::resources`] reads a snapshot (cached for a few seconds);
//!   [`Spaces::watch_resources`] streams a snapshot, then changes only
//!   ([`ResourcesUpdate`]), and a [`DiskWarning`] whenever the low-disk
//!   level changes.
//! * One sampler per runtime does the work. It ticks every
//!   [`VISIBLE_TICK`] while a client says a window is visible, every
//!   [`HIDDEN_TICK`] otherwise (the menu bar warning still needs it), and
//!   measures a Space's disk at most every [`DISK_MEASURE_EVERY`].
//! * Other computers publish [`summary`] numbers through the relay
//!   machine record (`meta`); reading them costs nothing extra because the
//!   machine list is already fetched.
//!
//! Numbers only: counts, bytes and percentages. Never file names, paths,
//! process names or window titles.

use serde::{Deserialize, Serialize};
use std::time::Duration;

pub mod creates;
pub mod disk;
pub mod host;
mod hub;
pub mod publish;
pub mod summary;
mod wire;
pub mod words;

pub(crate) use hub::Hub;
pub use hub::{Measure, ResourceWatch};

/// One GiB.
pub const GIB: u64 = 1 << 30;
/// One MiB.
pub const MIB: u64 = 1 << 20;

/// How often the sampler runs while a client says a window is visible.
pub const VISIBLE_TICK: Duration = Duration::from_secs(10);
/// How often it runs with no visible window (the menu bar warning).
pub const HIDDEN_TICK: Duration = Duration::from_secs(300);
/// A [`Spaces::resources`] poll with `visible` counts as a visible window
/// for this long.
pub const POLL_LEASE: Duration = Duration::from_secs(30);
/// A snapshot younger than this is returned as it is.
pub const MAX_AGE: Duration = Duration::from_secs(5);
/// One Space's disk is measured at most this often (and again after it is
/// created, deleted or changes power state).
pub const DISK_MEASURE_EVERY: Duration = Duration::from_secs(300);

/// An update carries an entry only when a percentage moved by more than
/// this many points ...
pub const DELTA_POINTS: f64 = 1.0;
/// ... or a size moved by more than this many bytes.
pub const DELTA_BYTES: u64 = 256 * MIB;

/// Free disk below this many bytes warns, unless the volume is so small that
/// [`LOW_FREE_PERCENT`] of it is more.
pub const LOW_FREE_FLOOR: u64 = 50 * GIB;
/// Free disk below this share of the volume warns ...
pub const LOW_FREE_PERCENT: u64 = 15;
/// ... but the line never rises above this, so a big disk does not nag.
pub const LOW_FREE_CAP: u64 = 100 * GIB;
/// A cleared warning stays until free space is this far above its line.
pub const WARNING_HYSTERESIS: u64 = 2 * GIB;

/// The id of this Mac in [`Resources::this_mac`].
pub const THIS_MAC: &str = "this-mac";

/// How low the free disk is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiskLevel {
    /// Plenty.
    #[default]
    Ok,
    /// Under the warning line ([`low_line`]).
    Low,
    /// Under the floor where creates already fail (`CUA_DISK_MIN_FREE`).
    Critical,
}

impl DiskLevel {
    /// `ok`, `low` or `critical`.
    pub fn as_str(self) -> &'static str {
        match self {
            DiskLevel::Ok => "ok",
            DiskLevel::Low => "low",
            DiskLevel::Critical => "critical",
        }
    }

    /// The inverse of [`DiskLevel::as_str`]; anything else is `Ok`.
    pub fn parse(word: &str) -> Self {
        match word {
            "low" => DiskLevel::Low,
            "critical" => DiskLevel::Critical,
            _ => DiskLevel::Ok,
        }
    }
}

/// The low-disk signal for the UI and the menu bar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskWarning {
    /// How low it is.
    pub level: DiskLevel,
    /// Free bytes on the volume that holds Spaces.
    pub free: u64,
    /// Size of that volume.
    pub total: u64,
    /// The line that was crossed (0 when [`DiskLevel::Ok`]).
    pub threshold: u64,
}

/// The free-space line below which a volume of `total` bytes warns: 50 GiB
/// or 15% of the volume, whichever is higher, never above 100 GiB.
pub fn low_line(total: u64) -> u64 {
    (total / 100 * LOW_FREE_PERCENT).clamp(LOW_FREE_FLOOR, LOW_FREE_CAP)
}

impl DiskWarning {
    /// The warning for `free` of `total` bytes. `min_free` is the floor
    /// where creates fail. `previous` makes a raised warning stay until
    /// free space is [`WARNING_HYSTERESIS`] above its line, so it does not
    /// flicker.
    pub fn assess(free: u64, total: u64, min_free: u64, previous: DiskLevel) -> Self {
        let line = low_line(total);
        let keeps = |at_least: DiskLevel, line: u64| {
            previous >= at_least && free < line.saturating_add(WARNING_HYSTERESIS)
        };
        let (level, threshold) = if free < min_free || keeps(DiskLevel::Critical, min_free) {
            (DiskLevel::Critical, min_free)
        } else if free < line || keeps(DiskLevel::Low, line) {
            (DiskLevel::Low, line)
        } else {
            (DiskLevel::Ok, 0)
        };
        DiskWarning {
            level,
            free,
            total,
            threshold,
        }
    }
}

/// A computer: this Mac, or one of your other computers (from its summary).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComputerResources {
    /// [`THIS_MAC`], or the relay machine id.
    pub id: String,
    /// Display name (empty for this Mac).
    pub name: String,
    /// Reachable now (always true for this Mac).
    pub online: bool,
    /// When the computer measured this, unix ms (another computer's can be
    /// minutes old).
    pub sampled_at_ms: u64,
    /// Processor cores.
    pub cpu_count: u32,
    /// Busy now, 0 to 100.
    pub cpu_percent: f64,
    /// 1-minute load average.
    pub load_average_1m: f64,
    /// Memory size, bytes.
    pub memory_total: u64,
    /// Memory in use, bytes (without reclaimable cache).
    pub memory_used: u64,
    /// Size of the volume that holds Spaces, bytes.
    pub disk_total: u64,
    /// Free bytes on it.
    pub disk_free: u64,
    /// Extra Spaces on it.
    pub spaces_count: u32,
    /// Of those, running.
    pub spaces_running: u32,
    /// Cores given to the running ones.
    pub spaces_cpu_allocated: u32,
    /// Memory given to the running ones, bytes.
    pub spaces_memory_allocated: u64,
    /// Memory the running guests report in use, bytes.
    pub spaces_memory_used: u64,
    /// Bytes all its extra Spaces take on it.
    pub spaces_disk_bytes: u64,
    /// False when some Space's disk could only be estimated.
    pub spaces_disk_exact: bool,
    /// The low-disk signal.
    pub warning: DiskWarning,
}

/// What `SystemService.Metrics` reports from inside a Space.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GuestUsage {
    /// Cores the guest sees.
    pub cpu_count: u32,
    /// Busy now, 0 to 100.
    pub cpu_percent: f64,
    /// 1-minute load average.
    pub load_average_1m: f64,
    /// Memory in use, bytes.
    pub memory_used: u64,
    /// Memory size, bytes.
    pub memory_total: u64,
    /// `memory_total` is the guest's own limit.
    pub memory_limited: bool,
    /// Disk in use, bytes.
    pub disk_used: u64,
    /// Disk size, bytes.
    pub disk_total: u64,
    /// `disk_total` is the guest's own disk.
    pub disk_limited: bool,
}

impl From<crate::stream::Usage> for GuestUsage {
    fn from(u: crate::stream::Usage) -> Self {
        GuestUsage {
            cpu_count: u.cpu_count,
            cpu_percent: u.cpu_percent,
            load_average_1m: u.load_average_1m,
            memory_used: u.memory_used,
            memory_total: u.memory_total,
            memory_limited: u.memory_limited,
            disk_used: u.disk_used,
            disk_total: u.disk_total,
            disk_limited: u.disk_limited,
        }
    }
}

/// What a Space costs this Mac, as opposed to what the guest sees.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskOnHost {
    /// Bytes only this Space holds (what deleting it frees). `None` when
    /// they could not be measured.
    pub bytes: Option<u64>,
    /// Blocks its files occupy, clones counted in full: an upper bound.
    pub allocated: u64,
    /// Part of it is shared with other copies.
    pub shares_space: bool,
}

/// One extra Space on this Mac.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceResources {
    /// `local:<name>`.
    pub id: String,
    /// `running`, `stopped`, `suspended`, or empty when unknown.
    pub power_state: String,
    /// Cores given to it (0: unknown).
    pub cpu_allocated: u32,
    /// Memory given to it, bytes (0: unknown).
    pub memory_allocated: u64,
    /// Its disk's size limit, bytes (0: unknown).
    pub disk_allocated: u64,
    /// The guest's own numbers; `None` while it is off or unreachable.
    pub usage: Option<GuestUsage>,
    /// What it costs this Mac; `None` for a container.
    pub disk_on_host: Option<DiskOnHost>,
}

/// One sample of everything.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Resources {
    /// Grows by one per sample.
    pub sequence: u64,
    /// Unix ms.
    pub sampled_at_ms: u64,
    /// This Mac.
    pub this_mac: ComputerResources,
    /// Extra Spaces on this Mac.
    pub spaces: Vec<SpaceResources>,
    /// Your other computers, from their summaries.
    pub computers: Vec<ComputerResources>,
    /// [`ComputerResources::warning`] of this Mac, for the menu bar.
    pub warning: DiskWarning,
}

/// What changed between two snapshots, past [`DELTA_POINTS`] and
/// [`DELTA_BYTES`].
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourcesUpdate {
    /// The snapshot it brings the receiver to.
    pub sequence: u64,
    /// Unix ms.
    pub sampled_at_ms: u64,
    /// This Mac, when it changed.
    pub this_mac: Option<ComputerResources>,
    /// Extra Spaces that changed or appeared (replace by `id`).
    pub spaces: Vec<SpaceResources>,
    /// Other computers that changed or appeared (replace by `id`).
    pub computers: Vec<ComputerResources>,
    /// Ids that are gone.
    pub removed: Vec<String>,
}

impl ResourcesUpdate {
    /// Nothing changed.
    pub fn is_empty(&self) -> bool {
        self.this_mac.is_none()
            && self.spaces.is_empty()
            && self.computers.is_empty()
            && self.removed.is_empty()
    }
}

/// What a watcher receives.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type", content = "value")]
pub enum ResourcesEvent {
    /// Everything, once at the start.
    Snapshot(Resources),
    /// Only what changed.
    Update(ResourcesUpdate),
    /// The low-disk level changed (including back to `ok`).
    LowDisk(DiskWarning),
}

fn moved(a: f64, b: f64) -> bool {
    (a - b).abs() > DELTA_POINTS
}

fn grew(a: u64, b: u64) -> bool {
    a.abs_diff(b) > DELTA_BYTES
}

impl ComputerResources {
    /// Whether `self` differs from `sent` by more than an update carries.
    pub fn differs(&self, sent: &ComputerResources) -> bool {
        self.id != sent.id
            || self.name != sent.name
            || self.online != sent.online
            || self.cpu_count != sent.cpu_count
            || moved(self.cpu_percent, sent.cpu_percent)
            || grew(self.memory_total, sent.memory_total)
            || grew(self.memory_used, sent.memory_used)
            || grew(self.disk_total, sent.disk_total)
            || grew(self.disk_free, sent.disk_free)
            || self.spaces_count != sent.spaces_count
            || self.spaces_running != sent.spaces_running
            || self.spaces_cpu_allocated != sent.spaces_cpu_allocated
            || grew(self.spaces_memory_allocated, sent.spaces_memory_allocated)
            || grew(self.spaces_memory_used, sent.spaces_memory_used)
            || grew(self.spaces_disk_bytes, sent.spaces_disk_bytes)
            || self.spaces_disk_exact != sent.spaces_disk_exact
            || self.warning.level != sent.warning.level
    }
}

impl SpaceResources {
    /// Whether `self` differs from `sent` by more than an update carries.
    pub fn differs(&self, sent: &SpaceResources) -> bool {
        let usage = match (&self.usage, &sent.usage) {
            (None, None) => false,
            (Some(a), Some(b)) => {
                a.cpu_count != b.cpu_count
                    || moved(a.cpu_percent, b.cpu_percent)
                    || grew(a.memory_used, b.memory_used)
                    || grew(a.memory_total, b.memory_total)
                    || grew(a.disk_used, b.disk_used)
                    || grew(a.disk_total, b.disk_total)
            }
            _ => true,
        };
        let disk = match (&self.disk_on_host, &sent.disk_on_host) {
            (None, None) => false,
            (Some(a), Some(b)) => {
                a.shares_space != b.shares_space
                    || a.bytes.is_some() != b.bytes.is_some()
                    || grew(a.bytes.unwrap_or(0), b.bytes.unwrap_or(0))
                    || grew(a.allocated, b.allocated)
            }
            _ => true,
        };
        usage
            || disk
            || self.id != sent.id
            || self.power_state != sent.power_state
            || self.cpu_allocated != sent.cpu_allocated
            || self.memory_allocated != sent.memory_allocated
            || self.disk_allocated != sent.disk_allocated
    }
}

impl Resources {
    /// What a receiver that holds `sent` needs to reach `self`: empty when
    /// nothing moved past the thresholds.
    pub fn update_since(&self, sent: &Resources) -> ResourcesUpdate {
        let mut u = ResourcesUpdate {
            sequence: self.sequence,
            sampled_at_ms: self.sampled_at_ms,
            ..Default::default()
        };
        if self.this_mac.differs(&sent.this_mac) {
            u.this_mac = Some(self.this_mac.clone());
        }
        for s in &self.spaces {
            match sent.spaces.iter().find(|o| o.id == s.id) {
                Some(old) if !s.differs(old) => {}
                _ => u.spaces.push(s.clone()),
            }
        }
        for c in &self.computers {
            match sent.computers.iter().find(|o| o.id == c.id) {
                Some(old) if !c.differs(old) => {}
                _ => u.computers.push(c.clone()),
            }
        }
        for old in &sent.spaces {
            if !self.spaces.iter().any(|s| s.id == old.id) {
                u.removed.push(old.id.clone());
            }
        }
        for old in &sent.computers {
            if !self.computers.iter().any(|c| c.id == old.id) {
                u.removed.push(old.id.clone());
            }
        }
        u
    }

    /// `self` after `update`, the way a receiver applies it.
    pub fn apply(&mut self, update: &ResourcesUpdate) {
        self.sequence = update.sequence;
        self.sampled_at_ms = update.sampled_at_ms;
        if let Some(m) = &update.this_mac {
            self.this_mac = m.clone();
            self.warning = m.warning;
        }
        self.spaces.retain(|s| !update.removed.contains(&s.id));
        self.computers.retain(|c| !update.removed.contains(&c.id));
        for s in &update.spaces {
            match self.spaces.iter_mut().find(|o| o.id == s.id) {
                Some(o) => *o = s.clone(),
                None => self.spaces.push(s.clone()),
            }
        }
        for c in &update.computers {
            match self.computers.iter_mut().find(|o| o.id == c.id) {
                Some(o) => *o = c.clone(),
                None => self.computers.push(c.clone()),
            }
        }
    }
}

/// Unix ms now.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests;
