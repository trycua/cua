//! The solver's records: what is asked for ([`DemandGroup`]), what exists
//! ([`Host`]), what can be bought ([`Offer`]), the limits
//! ([`Constraints`]) and the answer ([`Plan`]). Plain data: every record is
//! serializable, so a plan can be shown, edited and applied by an agent.

use serde::{Deserialize, Serialize};

/// CPU, memory and disk.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shape {
    /// CPU in thousandths of a core.
    pub millicpus: u64,
    /// Memory, MiB.
    pub memory_mb: u64,
    /// Disk, GiB (per-Space overlay; the shared base is counted per host).
    #[serde(default)]
    pub disk_gb: u64,
}

impl Shape {
    /// A shape.
    pub const fn new(millicpus: u64, memory_mb: u64, disk_gb: u64) -> Self {
        Self {
            millicpus,
            memory_mb,
            disk_gb,
        }
    }

    /// `n` of these.
    pub fn times(self, n: u64) -> Self {
        Self::new(
            self.millicpus.saturating_mul(n),
            self.memory_mb.saturating_mul(n),
            self.disk_gb.saturating_mul(n),
        )
    }

    /// Whether `self` fits in `room`.
    pub fn fits(self, room: Shape) -> bool {
        self.millicpus <= room.millicpus
            && self.memory_mb <= room.memory_mb
            && self.disk_gb <= room.disk_gb
    }

    /// `room` minus `self` (saturating).
    pub fn minus(room: Shape, used: Shape) -> Shape {
        Shape::new(
            room.millicpus.saturating_sub(used.millicpus),
            room.memory_mb.saturating_sub(used.memory_mb),
            room.disk_gb.saturating_sub(used.disk_gb),
        )
    }

    /// How many of `unit` fit in `self` (u64::MAX for a zero unit).
    pub fn how_many(self, unit: Shape) -> u64 {
        let d = |have: u64, need: u64| have.checked_div(need).unwrap_or(u64::MAX);
        d(self.millicpus, unit.millicpus)
            .min(d(self.memory_mb, unit.memory_mb))
            .min(d(self.disk_gb, unit.disk_gb))
    }

    /// The largest share of `of` this takes in any dimension (0..).
    pub fn dominant_share(self, of: Shape) -> f64 {
        let r = |a: u64, b: u64| if b == 0 { 0.0 } else { a as f64 / b as f64 };
        r(self.millicpus, of.millicpus)
            .max(r(self.memory_mb, of.memory_mb))
            .max(r(self.disk_gb, of.disk_gb))
    }
}

/// How soon the work wants to start.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Latency {
    /// A person or an agent waits on it (one Space, `wait`): a new host's
    /// boot time counts against it.
    #[default]
    Interactive,
    /// A batch (fork_many, `--count`, a compat burst): throughput and cost
    /// matter more than the first Space's start.
    Batch,
    /// Always-on (a persistent agent): runs for days; cost per hour rules.
    Steady,
}

/// What a machine must be to run the work.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Requirements {
    /// `container`, `vm`, `microvm` (empty: container).
    #[serde(default)]
    pub kind: String,
    /// `/dev/kvm` (or a Mac's hypervisor) needed: `vm` and `microvm` Spaces
    /// of a Linux or Windows image on a Linux host.
    #[serde(default)]
    pub needs_kvm: bool,
    /// The person allowed a KVM metal host for a small `vm` request
    /// (`ElasticPolicy.allow_metal`). Without it a lone small VM never
    /// buys one: metal costs dollars per hour.
    #[serde(default)]
    pub allow_metal: bool,
    /// `amd64`, `arm64`, or empty (the image has both).
    #[serde(default)]
    pub arch: String,
    /// `macos` for a macOS guest (Macs only), `linux`, or empty (any).
    #[serde(default)]
    pub host_os: String,
    /// The image is on a registry only the user's network reaches: no cloud.
    #[serde(default)]
    pub private_image: bool,
    /// Accelerator (`nvidia`, `apple`, `gpu`), or empty.
    #[serde(default)]
    pub gpu: String,
    /// The image (its base disk is shared per host, OSGym-style).
    #[serde(default)]
    pub image: String,
    /// Base image size, GiB: paid once per host that does not hold it.
    #[serde(default)]
    pub base_gb: u64,
}

/// Work waiting for a place: `count` Spaces of one shape.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DemandGroup {
    /// Its id (a world, an image, a request).
    pub id: String,
    /// How many Spaces.
    pub count: u32,
    /// Each one's size as booked (after learned usage).
    pub per: Shape,
    /// What the machine must be.
    #[serde(default)]
    pub req: Requirements,
    /// Every Space of it on one machine (forks of one world share
    /// copy-on-write pages there).
    #[serde(default)]
    pub whole: bool,
    /// Expected lifetime of each Space, seconds.
    pub duration_secs: u64,
    /// How soon it must start.
    #[serde(default)]
    pub latency: Latency,
    /// Machines that already hold its checkpoint or image (no copy needed).
    #[serde(default)]
    pub holders: Vec<String>,
    /// Seconds to copy its checkpoint to a machine that lacks it.
    #[serde(default)]
    pub sync_secs: u32,
    /// The one machine it must go to (an explicit `host:`), or empty.
    #[serde(default)]
    pub pin_host: String,
    /// Expected, not asked for yet (the rest of a burst): the solve may buy
    /// room for it when that is cheaper than buying again later, and never
    /// places it.
    #[serde(default)]
    pub soft: bool,
    /// For a soft group: what room for one of its Spaces is worth now, USD
    /// (the solve buys it when cheaper; [`super::solve::soft_value`]).
    #[serde(default)]
    pub soft_usd: f64,
}

/// A machine already in the pool: a device of the user's, or a cloud host.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Host {
    /// Its pool name.
    pub name: String,
    /// The cloud it is in (empty: one of the user's devices).
    #[serde(default)]
    pub provider: String,
    /// Region (cloud hosts).
    #[serde(default)]
    pub region: String,
    /// Its machine type (cloud hosts).
    #[serde(default)]
    pub type_name: String,
    /// Free now, after admission and overcommit (what it would accept).
    pub free: Shape,
    /// Its total offered capacity.
    pub total: Shape,
    /// `amd64` / `arm64`.
    #[serde(default)]
    pub arch: String,
    /// `linux`, `macos`, `windows`.
    #[serde(default)]
    pub os: String,
    /// Kinds it creates.
    #[serde(default)]
    pub kinds: Vec<String>,
    /// It has a hypervisor for VMs.
    #[serde(default)]
    pub kvm: bool,
    /// Accelerators (`nvidia:L4`, `apple:M2 Max`).
    #[serde(default)]
    pub gpu: Vec<String>,
    /// USD per hour while it runs (0: a device the user owns).
    #[serde(default)]
    pub usd_per_hour: f64,
    /// Seconds of already-paid time left (cloud hosts billed in steps).
    #[serde(default)]
    pub paid_secs_left: u64,
    /// Its cloud's billing step, seconds (0 or 1: per second).
    #[serde(default)]
    pub billing_step_secs: u64,
    /// Spaces running on it now.
    #[serde(default)]
    pub running: u32,
    /// Takes work now.
    #[serde(default = "yes")]
    pub ready: bool,
    /// Seconds until it is ready (a cloud host still booting: it takes
    /// Spaces, which start once it has joined).
    #[serde(default)]
    pub boot_secs_left: u32,
    /// Round trip from the requester, ms (0: unknown or same machine).
    #[serde(default)]
    pub rtt_ms: u32,
    /// Images (and checkpoints) it holds.
    #[serde(default)]
    pub images: Vec<String>,
}

fn yes() -> bool {
    true
}

impl Host {
    /// A device the user owns (no marginal cost).
    pub fn owned(&self) -> bool {
        self.provider.is_empty()
    }
}

/// A machine type that can be added: one provider, type and region.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Offer {
    /// `aws`, `gcp`, `hetzner`, `modal`.
    pub provider: String,
    /// `cx33`, `c7g.xlarge`; for a Spaces supplier, a size word.
    pub type_name: String,
    /// Region.
    pub region: String,
    /// `amd64` / `arm64`.
    pub arch: String,
    /// What a new host of it accepts (after the host's own reserve and the
    /// pool's overcommit).
    pub capacity: Shape,
    /// It has `/dev/kvm`.
    #[serde(default)]
    pub kvm: bool,
    /// Kinds its hosts create.
    #[serde(default)]
    pub kinds: Vec<String>,
    /// Accelerator, or empty.
    #[serde(default)]
    pub gpu: String,
    /// USD per hour.
    pub usd_per_hour: f64,
    /// Billing step, seconds (Hetzner 3600, AWS 1).
    pub billing_step_secs: u64,
    /// Minimum billed, seconds (Hetzner 3600, AWS 60).
    pub billing_min_secs: u64,
    /// Seconds from "add" to ready (learned per provider, type and region).
    pub boot_secs: u32,
    /// A Spaces supplier (Modal): each "host" is exactly one Space.
    #[serde(default)]
    pub one_space_each: bool,
    /// Most new machines of this provider allowed now (its quota minus what
    /// runs).
    #[serde(default = "many")]
    pub max_new: u32,
    /// Round trip from the requester to its region, ms (0: unknown).
    #[serde(default)]
    pub rtt_ms: u32,
    /// vCPUs a machine of it counts against its quota pool.
    #[serde(default)]
    pub vcpus: u32,
    /// Its quota pool (`hetzner/shared`), whose vCPUs left are in
    /// [`Constraints::quota_left`]; empty: none.
    #[serde(default)]
    pub quota: String,
}

fn many() -> u32 {
    u32::MAX
}

impl Offer {
    /// The provider/type/region key (`hetzner/cx33/fsn1`).
    pub fn key(&self) -> String {
        format!("{}/{}/{}", self.provider, self.type_name, self.region)
    }

    /// Seconds billed for running `secs` (steps and minimum).
    pub fn billed_secs(&self, secs: u64) -> u64 {
        let step = self.billing_step_secs.max(1);
        secs.max(self.billing_min_secs).div_ceil(step) * step
    }

    /// Cost of running one for `secs`, billed.
    pub fn cost_for(&self, secs: u64) -> f64 {
        self.usd_per_hour * self.billed_secs(secs) as f64 / 3600.0
    }
}

/// What the user (or an agent) pinned. Pins are constraints of the same
/// solve, never another code path.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Pins {
    /// Only this provider for new machines.
    #[serde(default)]
    pub provider: Option<String>,
    /// Only this region.
    #[serde(default)]
    pub region: Option<String>,
    /// Only this machine type.
    #[serde(default)]
    pub type_name: Option<String>,
    /// Exactly this many new machines (a dry "add N hosts").
    #[serde(default)]
    pub new_hosts: Option<u32>,
    /// No new machines at all (autoscale off): place on what exists.
    #[serde(default)]
    pub no_scale: bool,
    /// Only the user's own devices (no cloud at all).
    #[serde(default)]
    pub devices_only: bool,
}

/// The limits of one solve.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Constraints {
    /// Relay machines the account may register (32).
    pub relay_limit: u32,
    /// Relay machines registered now.
    pub relay_used: u32,
    /// Each new Space on a host registers its own relay machine (false when
    /// Spaces are reached through their host).
    pub space_takes_relay: bool,
    /// Most new machines in one solve.
    pub max_new_hosts: u32,
    /// Most USD per hour the clouds may cost at once (what runs plus what
    /// is added), if capped.
    pub max_usd_per_hour: Option<f64>,
    /// USD left in the budget period, if capped.
    pub budget_left_usd: Option<f64>,
    /// USD per hour of the cloud hosts running now.
    pub running_usd_per_hour: f64,
    /// An interactive Space should be ready within this many seconds.
    pub interactive_slo_secs: u32,
    /// Pins.
    #[serde(default)]
    pub pins: Pins,
    /// Time the exact solver may take, milliseconds.
    pub time_budget_ms: u64,
    /// vCPUs left per quota pool (`hetzner/shared` -> 8): a cloud's core
    /// limit, read or learned from a refused create. A pool not listed has
    /// no known limit.
    #[serde(default)]
    pub quota_left: std::collections::BTreeMap<String, u32>,
}

impl Default for Constraints {
    fn default() -> Self {
        Self {
            relay_limit: 32,
            relay_used: 0,
            space_takes_relay: false,
            max_new_hosts: 20,
            max_usd_per_hour: Some(DEFAULT_MAX_USD_PER_HOUR),
            budget_left_usd: None,
            running_usd_per_hour: 0.0,
            interactive_slo_secs: 120,
            pins: Pins::default(),
            time_budget_ms: 200,
            quota_left: Default::default(),
        }
    }
}

/// The default cap on what linked clouds may cost at once: $5/hour (safe:
/// about 400 small Hetzner hosts' worth is never reached by accident).
pub const DEFAULT_MAX_USD_PER_HOUR: f64 = 5.0;

/// Where some Spaces of a group go.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "on", rename_all = "snake_case")]
pub enum Target {
    /// A machine in the pool, by name.
    Host {
        /// Its name.
        name: String,
    },
    /// The `index`-th new machine of [`Plan::new_hosts`].
    New {
        /// Index into `new_hosts`.
        index: usize,
    },
}

/// Some Spaces of one group on one machine.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Assignment {
    /// The group's id.
    pub group: String,
    /// Where.
    pub target: Target,
    /// How many.
    pub count: u32,
    /// The machine lacks its checkpoint/image: copied first.
    #[serde(default)]
    pub sync: bool,
}

/// A machine the plan adds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NewHost {
    /// Provider.
    pub provider: String,
    /// Machine type.
    pub type_name: String,
    /// Region.
    pub region: String,
    /// USD per hour.
    pub usd_per_hour: f64,
    /// Expected seconds to ready.
    pub boot_secs: u32,
    /// Billed seconds for the work it gets.
    pub billed_secs: u64,
    /// A Spaces supplier's Space (Modal), not a host.
    #[serde(default)]
    pub one_space: bool,
    /// Round trip from the requester, ms (0: unknown).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rtt_ms: u32,
}

fn is_zero(v: &u32) -> bool {
    *v == 0
}

/// Spaces that found no place, and why.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Unplaced {
    /// The group.
    pub group: String,
    /// How many.
    pub count: u32,
    /// Why (budget, relay limit, nothing runs this kind, ...).
    pub reason: String,
}

/// The answer of one solve.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    /// Where every placed Space goes.
    pub assignments: Vec<Assignment>,
    /// Machines to add.
    pub new_hosts: Vec<NewHost>,
    /// What could not be placed.
    pub unplaced: Vec<Unplaced>,
    /// USD per hour the new machines add.
    pub added_usd_per_hour: f64,
    /// USD the new machines cost for the work, billed.
    pub projected_usd: f64,
    /// A lower bound on any plan's cost for the same work (fractional
    /// packing on the cheapest machine for each group).
    pub lower_bound_usd: f64,
    /// `exact` (integer program, proven optimal for the model) or `greedy`.
    pub solver: String,
    /// Microseconds the solve took.
    pub elapsed_us: u64,
    /// Relay machines registered after the plan runs.
    pub relay_after: u32,
    /// Seconds until the last Space is expected to be ready.
    pub ready_in_secs: u32,
    /// One line per decision, for people.
    pub explain: Vec<String>,
}

impl Plan {
    /// Spaces placed.
    pub fn placed(&self) -> u32 {
        self.assignments.iter().map(|a| a.count).sum()
    }
}
