//! Local warm pools: Spaces of a shape this host creates are kept ready
//! ahead of time, so the next create of that shape claims one instead of
//! booting.
//!
//! A *shape* is everything that makes two creates interchangeable: the
//! resolved image (by digest), the backend and its runtime, CPUs, memory,
//! ports, probes, command and environment (without the Spaces token). Only
//! plain Spaces pool: an image that runs cua-spacesd, no sidecars, GPU,
//! disk size, image build or private-registry credentials, and not
//! `warm=false`.
//!
//! Every create of a shape is recorded. By default nothing runs: a create
//! takes the backend's cheapest tier without a standing cost (a snapshot
//! restore where it has one, else a cold boot). Members of a shape run only
//! when its traffic says they pay ([`policy`]: keep-alive and pre-warm
//! windows from the shape's inter-arrival times, bounded by a ski-rental
//! break-even), or when an explicit pool asks for them (`pool.warm`, an
//! SDK pool), exactly as many as asked. A *member* is an instance started
//! with [`StartSpec::pool_member`] under a [`cua_vmm::POOL_MEMBER_PREFIX`]
//! name, whose Spaces token is a per-member *pool secret* nobody else sees.
//! A claim ([`cua_vmm::Runtime::claim`]) renames it to the Space and binds it
//! to the Space's token; then cua-spacesd's `Init`, authenticated with the
//! pool secret, rotates the token and makes the guest a new machine
//! (`new_machine`, fresh `entropy`, the clock set to `now`). From then on
//! the pool secret opens nothing, and the Space is indistinguishable from a
//! cold one (its state file, services, delete). Two creates never share a
//! member: a member leaves the pool before it is claimed.
//!
//! Members are hidden from lists, recorded under `<state>/.pool/` (0600:
//! the pool secret, the shape) and leased to this daemon with the ephemeral
//! lease machinery: a daemon that starts adopts the members a previous one
//! left (or removes them), and if no daemon does, the lease reaper deletes
//! them once their daemon is gone.
//!
//! Sizing is memory-aware: members of a backend take at most
//! `pool.memory_percent` of the memory they draw on (the host's, or the
//! built-in Linux runtime VM's for containers on a Mac), and no member
//! starts that would leave less than a floor free; under pressure the pool
//! shrinks. A shape with no create for the policy's horizon is forgotten.
//! The warm path never waits on a registry or on anything a cold create
//! would not do.

pub mod named;
pub mod policy;

use async_trait::async_trait;
use cua_sandbox_core::StateStore;
use cua_vmm::{
    BackendKind, ClaimIdentity, Endpoints, Instance, Runtime as VmmRuntime, StartSpec, VmmError,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The guest environment variables a Spaces token arrives in; they are not
/// part of a shape.
const TOKEN_ENV: [&str; 2] = ["CUA_ENV_TOKEN", "CUA_SPACESD_TOKEN"];

/// How long a member may take to answer as a ready Space.
const MEMBER_READY_TIMEOUT: Duration = Duration::from_secs(180);

/// Pause before a claimed member is replaced, so the claimed Space's first
/// commands do not compete with a member booting.
const REFILL_DELAY: Duration = Duration::from_millis(1500);

/// The longest a refill waits for a burst of creates to pause (see
/// [`WarmPool::refill_later`]).
const REFILL_MAX_WAIT: Duration = Duration::from_millis(7500);

/// The explicit-pool source of the Warm start's Linux Space
/// ([`crate::local::VmmLocal::apply_pools`]). A shape that has it is kept
/// warm on purpose for creates that follow each other: a refill starts the
/// moment a member is claimed, and a create that finds no member ready waits
/// for the one that is starting (it began before this create, so waiting is
/// never slower than a cold start of its own) instead of booting another.
pub const WARM_SOURCE: &str = "warm-pools";

/// The longest [`WarmPool::drain`] waits for cancelled member starts to remove their containers.
const DRAIN_CANCEL_WAIT: Duration = Duration::from_secs(8);

/// The longest a create waits for a member that is starting
/// ([`WARM_SOURCE`]): about what a cold start and the install it saves take.
/// Past it the create starts cold, as it does without the experiment.
const WARM_WAIT_MAX: Duration = Duration::from_secs(30);

/// The longest the agent stack of a member takes to install.
const PREPARE_TIMEOUT: Duration = Duration::from_secs(900);

/// Consecutive failed member starts after which a shape stops refilling
/// until it is used again.
const MAX_FAILURES: u32 = 3;

/// How long a shape that failed [`MAX_FAILURES`] starts in a row waits
/// before it tries one more (a registry or engine that was down recovers).
const FAILURE_COOLDOWN: Duration = Duration::from_secs(300);

/// How often a refill that stays deferred for the same reason says so again.
const DEFER_LOG_EVERY: Duration = Duration::from_secs(120);

/// The estimate of a member's memory until one was measured: a container
/// Space at idle (a gVisor desktop is well under this). VMs count their
/// whole memory.
const CONTAINER_FOOTPRINT_MB: u64 = 600;

/// Pool settings (`pool.*` in `cua config`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PoolSettings {
    /// Members run where a shape's traffic pays for them (`pool.auto`;
    /// off: only explicit pools run).
    pub auto: bool,
    /// Members of one backend take at most this share of the memory they
    /// draw on (`pool.memory_percent`).
    pub memory_percent: u32,
    /// Shapes kept warm regardless of use: image and member count
    /// (`pool.warm`).
    pub warm: Vec<(String, u32)>,
}

impl Default for PoolSettings {
    fn default() -> Self {
        Self {
            auto: true,
            memory_percent: 15,
            warm: Vec::new(),
        }
    }
}

impl PoolSettings {
    /// The effective settings (environment, then `config.toml`).
    pub fn load() -> Self {
        let Ok(s) = cua_sandbox_core::settings::Settings::load() else {
            return Self::default();
        };
        Self::from_settings(&s)
    }

    /// The pool settings of `s`.
    pub fn from_settings(s: &cua_sandbox_core::settings::Settings) -> Self {
        use cua_sandbox_core::settings::key;
        let d = Self::default();
        let num = |name: &str| -> Option<u64> {
            key(name)
                .ok()
                .and_then(|k| s.entry(k).value.trim().parse().ok())
        };
        let warm = key("pool.warm")
            .ok()
            .map(|k| s.entry(k).value)
            .map(|v| parse_warm(&v).unwrap_or_default())
            .unwrap_or_default();
        Self {
            auto: key("pool.auto")
                .ok()
                .map(|k| s.entry(k).value)
                .map(|v| !matches!(v.trim(), "off" | "false" | "0" | "no"))
                .unwrap_or(d.auto),
            memory_percent: num("pool.memory_percent")
                .map(|p| p.min(90) as u32)
                .unwrap_or(d.memory_percent),
            warm,
        }
    }

    /// Whether any member may exist.
    pub fn enabled(&self) -> bool {
        self.auto || self.warm.iter().any(|(_, n)| *n > 0)
    }
}

/// Parses `pool.warm`: comma-separated `IMAGE[=COUNT]` (COUNT defaults to
/// 1; `none` or empty: no shape).
pub fn parse_warm(v: &str) -> Result<Vec<(String, u32)>, String> {
    let v = v.trim();
    if v.is_empty() || v.eq_ignore_ascii_case("none") {
        return Ok(vec![]);
    }
    v.split(',')
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .map(|e| match e.rsplit_once('=') {
            Some((img, n)) => {
                let n: u32 = n
                    .trim()
                    .parse()
                    .map_err(|_| format!("{e:?}: the count after = must be a number"))?;
                if img.trim().is_empty() {
                    return Err(format!("{e:?}: an image is needed before ="));
                }
                Ok((img.trim().to_string(), n))
            }
            None => Ok((e.to_string(), 1)),
        })
        .collect()
}

/// Memory a backend's members draw on, in MiB.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Memory {
    /// All of it (the host's, or the VM's an engine runs in).
    pub total_mb: u64,
    /// What is free for new work now.
    pub available_mb: u64,
}

/// The floor of free memory no member start may cross: a tenth of the
/// memory, at least 1 GiB.
pub fn memory_floor_mb(total_mb: u64) -> u64 {
    (total_mb / 10).max(1024)
}

/// What the pool asks of the machine.
#[async_trait]
pub trait PoolHost: Send + Sync {
    /// Waits until the member at `endpoints` answers as a ready Space with
    /// its pool `secret` (cua-spacesd up, the first command served, and
    /// with a `desktop` its desktop ready and a screenshot served), for at
    /// most `timeout`.
    async fn member_ready(
        &self,
        endpoints: &Endpoints,
        secret: &str,
        desktop: bool,
        timeout: Duration,
    ) -> Result<(), String>;

    /// Makes a claimed member the Space whose token is `token`: cua-spacesd
    /// `Init`, authenticated with the pool `secret`, rotates the token and
    /// sets `new_machine`, fresh entropy and the clock. A backend whose
    /// claim delivers the token itself (a microVM over vsock, a QEMU VM
    /// through its token file) is initialised with the Space's token
    /// only: the pool secret is never presented to it.
    async fn rotate(
        &self,
        endpoints: &Endpoints,
        backend: BackendKind,
        secret: &str,
        token: &str,
    ) -> Result<(), String>;

    /// The memory `backend`'s members draw on, when it can be told without
    /// starting anything.
    async fn memory(&self, backend: BackendKind) -> Option<Memory>;

    /// What running member `name` of `backend` uses now (MiB).
    async fn used_mb(&self, backend: BackendKind, name: &str) -> Option<u64>;

    /// Starts what `backend` runs on when it is down (the built-in Linux
    /// runtime's VM), for shapes `pool.warm` keeps. On-demand shapes never
    /// wake anything: an idle host stays idle.
    async fn wake(&self, backend: BackendKind);

    /// Unclaimed members `backend` holds for this home now, by name, when
    /// it can list them without starting anything (container engines).
    async fn members_present(&self, backend: BackendKind) -> Option<Vec<String>>;

    /// Whether a member of `backend` can be prepared before it is claimed:
    /// it answers calls made with its pool secret (a container does; a
    /// microVM waits paused and a QEMU member restores memory only when it
    /// is claimed, so those are prepared after the claim, as before).
    fn can_prepare(&self, backend: BackendKind) -> bool {
        let _ = backend;
        false
    }

    /// Puts the agent stack of `harnesses` (bridge, runner, skills and the
    /// coding apps) in the member at `endpoints`, which answers to its pool
    /// `secret`, for at most `timeout`. Only called when
    /// [`PoolHost::can_prepare`] says the backend can.
    async fn prepare(
        &self,
        endpoints: &Endpoints,
        secret: &str,
        harnesses: &[String],
        timeout: Duration,
    ) -> Result<(), String> {
        let _ = (endpoints, secret, harnesses, timeout);
        Err("this runtime cannot prepare a member".into())
    }

    /// Reads the files of the apps `harnesses` once in the member at
    /// `endpoints`, so the first session of the Space that claims it does
    /// not read them from the engine's disk (a member booted from a saved
    /// filesystem has an empty file cache). Best effort, in the
    /// background, never needed for correctness.
    async fn warm_files(&self, endpoints: &Endpoints, secret: &str, harnesses: &[String]) {
        let _ = (endpoints, secret, harnesses);
    }

    /// What pins the install of `harness` in a prepared filesystem: the
    /// versions of the apps a build installs for it. A saved filesystem
    /// serves a build whose pins for the harnesses it wants are the same.
    fn pin(&self, harness: &str) -> String {
        harness.to_string()
    }

    /// Whether this machine runs on battery now (no member then).
    fn on_battery(&self) -> bool;

    /// Whether the machine's CPUs are saturated by other work: no member
    /// the policy only wants (rather than a pool asked for) starts then.
    /// Default: never.
    fn busy(&self) -> bool {
        false
    }
}

/// Ready members of one requested shape ([`WarmPool::ready_by_request`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ready {
    /// Image, kind and runtime as a create asked for them.
    pub image: String,
    pub kind: String,
    pub runtime: String,
    /// Each member's CPUs and memory (MiB).
    pub cpus: u32,
    pub memory_mb: u64,
    /// How many are ready.
    pub ready: u32,
}

/// The size a create gets when it names none
/// (`cua_sandbox_core::CreateOptions::new`).
pub const DEFAULT_SIZE: (u32, u64) = (2, 4096);

impl Ready {
    /// The `on="auto"` placement keys these members answer (`warm_key`
    /// parts): their size, and the empty size too when it is the default
    /// (a create that names no size gets exactly these).
    pub fn sizes(&self) -> Vec<(u32, u64)> {
        let mut v = vec![(self.cpus, self.memory_mb)];
        if (self.cpus, self.memory_mb) == DEFAULT_SIZE {
            v.push((0, 0));
        }
        v
    }
}

/// The saved filesystem a member boots from.
#[derive(Clone)]
struct Chosen {
    tag: String,
    image: cua_vmm::ImageSource,
    /// What is installed in it (id to pin).
    have: std::collections::BTreeMap<String, String>,
    /// It has every app the shape wants.
    covers_all: bool,
}

/// A ready member.
#[derive(Clone)]
struct Member {
    name: String,
    secret: String,
    footprint_mb: u64,
    /// The agent stack of the shape's harnesses is in it already.
    prepared: bool,
}

/// A remembered shape.
struct Shape {
    template: StartSpec,
    backend: BackendKind,
    rt: Arc<dyn VmmRuntime>,
    /// What explicit pools ask of it, by source (`pool:<name>`,
    /// `warm:<image>`, `create`): members always kept, at most, until.
    explicit: std::collections::BTreeMap<String, Explicit>,
    members: Vec<Member>,
    starting: u32,
    failures: u32,
    /// When the last member start failed (the cool-down after
    /// [`MAX_FAILURES`] runs from it).
    failed_at: Option<Instant>,
    /// Measured idle footprint of a member (MiB).
    footprint_mb: Option<u64>,
    /// Its creates.
    arrivals: policy::Arrivals,
    /// Measured (EWMA, seconds): a member's start to ready, a create's
    /// start on the default tier, a claim.
    boot_s: Option<f64>,
    default_s: Option<f64>,
    claim_s: Option<f64>,
    /// The policy's last word.
    decision: Option<policy::Decision>,
    /// The last create of it.
    last_used: Option<Instant>,
    /// The words a create of it asked for (image, kind, runtime), as
    /// placement keys warm members.
    requested: Option<(String, String, String)>,
    /// The harnesses whose agent stack a member gets before it is ready
    /// ([`WarmPool::set_prepare`]); empty: members are bare.
    prepare: Vec<String>,
    /// How many of `prepare` (from the front) a member is prepared for before
    /// it can be claimed: the apps the person most likely uses first. The
    /// rest go into the saved filesystem in the background. 0: all of them.
    prepare_first: usize,
    /// Measured (EWMA, seconds): a member's preparation.
    prepare_s: Option<f64>,
    /// The saved filesystem of the first prepared member
    /// ([`cua_vmm::Runtime::save_prepared`]): its tag and the image the next
    /// members start from, already prepared.
    prepared_image: Option<(String, cua_vmm::ImageSource)>,
}

impl Shape {
    fn new(template: StartSpec, backend: BackendKind, rt: Arc<dyn VmmRuntime>) -> Self {
        Self {
            template,
            backend,
            rt,
            explicit: Default::default(),
            members: vec![],
            starting: 0,
            failures: 0,
            failed_at: None,
            footprint_mb: None,
            arrivals: Default::default(),
            boot_s: None,
            default_s: None,
            claim_s: None,
            decision: None,
            requested: None,
            last_used: None,
            prepare: Vec::new(),
            prepare_first: 0,
            prepare_s: None,
            prepared_image: None,
        }
    }

    /// Whether a create may wait for a member that is starting: the shape is
    /// kept warm on purpose for the Warm start and that pool is
    /// not idle.
    fn waits_for_refill(&self) -> bool {
        self.explicit
            .get(WARM_SOURCE)
            .is_some_and(|e| !self.idle(e))
            && self.refill_estimate().is_some_and(|d| d < WARM_WAIT_MAX)
    }

    /// Whether the shape is kept warm for Warm start, whatever the
    /// estimate (the refill after a claim starts at once).
    fn is_warm_source(&self) -> bool {
        self.explicit
            .get(WARM_SOURCE)
            .is_some_and(|e| !self.idle(e))
    }

    /// How long the member that is starting takes to be ready, as measured,
    /// and `None` before this shape has had a member (the first member of a
    /// new install pulls the image and installs the apps: a create does not
    /// wait for that, it starts cold).
    fn refill_estimate(&self) -> Option<Duration> {
        let boot = self.boot_s?;
        let secs = if self.prepared_image.is_some() || self.prepare.is_empty() {
            boot
        } else {
            boot + self.prepare_s?
        };
        Some(Duration::from_secs_f64(secs))
    }

    /// Whether explicit pool `e` is idle now (no create of the shape for
    /// its idle time).
    fn idle(&self, e: &Explicit) -> bool {
        let Some(idle) = e.idle else { return false };
        let last = self.last_used.map_or(e.since, |u| u.max(e.since));
        last.elapsed() >= idle
    }

    /// Members explicit pools keep at all times.
    fn keep(&self) -> u32 {
        self.explicit
            .values()
            .filter(|e| !self.idle(e))
            .map(|e| e.min)
            .max()
            .unwrap_or(0)
    }

    /// The most members explicit pools allow, when one sets a ceiling.
    fn cap(&self) -> Option<u32> {
        self.explicit
            .values()
            .filter(|e| !self.idle(e))
            .filter_map(|e| e.max)
            .max()
    }

    fn target(&self, auto: bool) -> u32 {
        let policy = if auto && on_demand(self.backend) {
            self.decision.as_ref().map(|d| d.members).unwrap_or(0)
        } else {
            0
        };
        let keep = self.keep();
        let t = policy.max(keep);
        match self.cap() {
            Some(c) => t.min(c.max(keep)),
            None => t,
        }
    }

    /// What its members cost and save, as measured (defaults until then:
    /// a container member boots in about 5 s; a VM's in 30 s).
    fn costs(&self) -> policy::Costs {
        let boot = self.boot_s.unwrap_or(match self.backend {
            BackendKind::Container | BackendKind::Firecracker | BackendKind::Vz => 5.0,
            _ => 30.0,
        });
        let saved = match (self.default_s, self.claim_s) {
            (Some(d), Some(c)) => (d - c).max(0.0),
            (Some(d), None) => (d - 0.1).max(0.0),
            _ => 0.0,
        };
        policy::Costs {
            boot_s: boot,
            saved_s: saved,
        }
    }

    /// The reason to show for it.
    fn reason(&self) -> String {
        let policy = self.decision.as_ref().map(|d| d.members).unwrap_or(0);
        if self.keep() > 0 && policy <= self.keep() {
            return policy::Reason::Explicit(self.keep()).to_string();
        }
        if !self.explicit.is_empty() && self.explicit.values().all(|e| self.idle(e)) {
            return "pool idle: none kept ready until its next create".into();
        }
        self.decision
            .as_ref()
            .map(|d| d.reason.to_string())
            .unwrap_or_else(|| policy::Reason::NoRepeat.to_string())
    }

    fn estimate_mb(&self) -> u64 {
        self.footprint_mb.unwrap_or(match self.backend {
            BackendKind::Container => CONTAINER_FOOTPRINT_MB,
            _ => self.template.memory_mb,
        })
    }
}

/// What one explicit pool asks of a shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Explicit {
    /// Members kept at all times.
    pub min: u32,
    /// The most members (the policy may add up to this many).
    pub max: Option<u32>,
    /// Until when (`None`: until withdrawn).
    pub until: Option<Instant>,
    /// Without a create of the shape for this long it keeps none ready
    /// (until the next create).
    pub idle: Option<Duration>,
    /// When it was first asked for (the idle clock starts here).
    pub since: Instant,
}

/// A member's record (`<state>/.pool/<member>.json`, 0600).
#[derive(Clone, Serialize, Deserialize)]
struct Record {
    name: String,
    key: String,
    backend: BackendKind,
    secret: String,
    template: StartSpec,
    #[serde(default)]
    keep: u32,
    /// The agent stack was installed before the member was ready.
    #[serde(default)]
    prepared: bool,
}

impl std::fmt::Debug for Record {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Record")
            .field("name", &self.name)
            .field("key", &"<redacted>")
            .field("backend", &self.backend)
            .field("secret", &"<redacted>")
            .field("template", &self.template)
            .field("keep", &self.keep)
            .field("prepared", &self.prepared)
            .finish()
    }
}

/// What the pool holds now ([`WarmPool::status`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PoolStatus {
    /// Ready members.
    pub members: u32,
    /// Their memory (measured, else estimated), MiB.
    pub memory_mb: u64,
    /// Members starting.
    pub starting: u32,
    /// Why (the policy's word on the shape that matters most now).
    pub reason: String,
}

/// The warm pool of one daemon.
#[derive(Clone)]
pub struct WarmPool {
    inner: Arc<Inner>,
}

struct Inner {
    shapes: Mutex<HashMap<String, Shape>>,
    host: Arc<dyn PoolHost>,
    settings: Mutex<PoolSettings>,
    leases: StateStore,
    dir: PathBuf,
    /// Backends whose `claim` is known to work (`Some(true)`) or not.
    claims: Mutex<HashMap<BackendKind, bool>>,
    refilling: Mutex<HashSet<String>>,
    /// Members being started (not yet in a shape).
    starting: Mutex<HashSet<String>>,
    /// [`WarmPool::drain`] ran: nothing starts any more.
    closed: std::sync::atomic::AtomicBool,
    /// Wakes what waits for [`WarmPool::drain`] ([`WarmPool::or_closed`]).
    closed_signal: tokio::sync::Notify,
    pid: u32,
    /// The policy's clock.
    epoch: Instant,
    /// Ticks so far (maintenance runs every [`MAINTAIN_EVERY`]).
    ticks: std::sync::atomic::AtomicU64,
    class: policy::HostClass,
    /// [`WarmPool::reclaim`] took members back for other work: the policy
    /// starts no member of its own until then.
    held_off_until: Mutex<Option<Instant>>,
    /// Backends whose runtime would not come up: not tried again before this time ([`ENGINE_RETRY`]).
    engine_down_until: Mutex<HashMap<BackendKind, Instant>>,
    /// Woken when a member becomes ready or a start ends without one, so a
    /// create that waits for a starting member ([`WARM_SOURCE`]) looks again.
    arrived: tokio::sync::Notify,
    /// Shapes with a refill scheduled that has not started yet.
    due: Mutex<HashSet<String>>,
    /// Why each shape's refill is deferred now, and when that was last
    /// logged ([`WarmPool::defer`]).
    deferred: Mutex<HashMap<String, (String, Instant)>>,
    /// Shapes whose saved filesystem is being built now, and (after a
    /// failure) when the next try may start.
    building: Mutex<HashSet<String>>,
    build_blocked: Mutex<HashMap<String, Instant>>,
    /// Serialises changes of the index of saved filesystems.
    index_lock: Mutex<()>,
}

/// How long the policy starts no member after other work took capacity back.
const HOLD_OFF: Duration = Duration::from_secs(600);
/// How long a runtime that would not come up (no container engine on this computer) is left alone before the pool tries to
/// wake it again: every refill tick used to retry it, which kept an idle daemon busy.
const ENGINE_RETRY: Duration = Duration::from_secs(300);

/// Most members the policy gives one shape (explicit pools may hold more).
const MAX_AUTO_MEMBERS: u32 = 8;

/// Ticks between maintenance passes (vanished members, strays, memory).
const MAINTAIN_EVERY: u64 = 15;

/// Exponentially weighted average.
fn ewma(old: Option<f64>, new: f64) -> Option<f64> {
    Some(match old {
        Some(o) => 0.7 * o + 0.3 * new,
        None => new,
    })
}

/// The shape key of `spec` on `backend`, or `None` when it does not pool
/// (see the module docs). `spacesd`: the image runs cua-spacesd.
pub fn shape_key(spec: &StartSpec, backend: BackendKind, spacesd: bool) -> Option<String> {
    let plain = spacesd
        && spec.sidecars.is_empty()
        && spec.gpu.is_none()
        && spec.disk_size_gb.is_none()
        && spec.registry_auth.is_none()
        // Commands of the caller's own do not pool (cua-spacesd may not
        // run); a headless Space's (cua-spacesd alone) does.
        && spec
            .command
            .as_ref()
            .is_none_or(|c| c.is_empty() || *c == crate::local::headless_command_line())
        && spec.ssh.is_none()
        && spec.cloud_init_user_data.is_none()
        && spec.extra_args.is_empty()
        && !spec.pool_member
        // A fork source's RAM is a shared file; a member's is not.
        && !spec.forkable
        && TOKEN_ENV.iter().any(|k| spec.env.contains_key(*k));
    let image = match &spec.image {
        cua_vmm::ImageSource::Oci { reference } => reference.as_str(),
        _ => return None,
    };
    // Image builds (`cua-vmm/build:`) and anything this machine made from
    // an instance are not registry images.
    if !plain || image.starts_with("cua-vmm/") {
        return None;
    }
    let canonical = canonical_of(&template_of(spec), backend, image, false);
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(canonical.to_string().as_bytes());
    Some(digest.iter().take(8).map(|b| format!("{b:02x}")).collect())
}

/// The environment variables a headless Space adds to a desktop one's.
const HEADLESS_ENV: [&str; 3] = ["CUA_HEADLESS", "CUA_ENV_NO_DESKTOP", "CUA_ENV_QUIC_PORT"];

/// Everything that makes two templates interchangeable, as JSON. `loose`:
/// without what a bigger or desktop member still satisfies (CPUs, memory,
/// the command and the environment that switch the desktop off), for
/// [`serves`].
fn canonical_of(
    t: &StartSpec,
    backend: BackendKind,
    image: &str,
    loose: bool,
) -> serde_json::Value {
    let mut ports = t.ports.clone();
    ports.sort_unstable();
    ports.dedup();
    let mut env = t.env.clone();
    if loose {
        for k in HEADLESS_ENV {
            env.remove(k);
        }
    }
    serde_json::json!({
        "backend": backend.as_str(),
        "image": image,
        "os": t.os,
        "arch": t.arch,
        "cpus": (!loose).then_some(t.cpus),
        "memory_mb": (!loose).then_some(t.memory_mb),
        "firmware": t.firmware,
        "ports": ports,
        "env": env,
        "probes": (!loose).then_some(&t.probes),
        "restrict_network": t.restrict_network,
        "labels": t.labels,
        "container_runtime": t.container_runtime,
        "command": (!loose).then_some(&t.command),
    })
}

/// Whether the member is ready by every check `want` waits for: a probe on
/// a port is covered by one on the same port that is at least as strict (an
/// HTTP check also means the port accepts connections).
fn probes_covered(member: &StartSpec, want: &StartSpec) -> bool {
    use cua_vmm::Probe;
    want.probes.iter().all(|w| match w {
        Probe::None => true,
        Probe::Tcp { port } => member.probes.iter().any(
            |m| matches!(m, Probe::Tcp { port: p } | Probe::Http { port: p, .. } if p == port),
        ),
        Probe::Http { port, path } => member
            .probes
            .iter()
            .any(|m| matches!(m, Probe::Http { port: p, path: q } if p == port && q == path)),
    })
}

/// Whether a member of shape `member` can be handed to a create of `want`
/// as it is: the same in everything but size and desktop, and at least as
/// big, with a desktop where one is wanted (a headless create runs fine
/// on a desktop member; the other way round it would not).
fn serves(member: &StartSpec, want: &StartSpec, backend: BackendKind) -> bool {
    let (
        cua_vmm::ImageSource::Oci { reference: have },
        cua_vmm::ImageSource::Oci { reference: asked },
    ) = (&member.image, &want.image)
    else {
        return false;
    };
    member.cpus >= want.cpus
        && member.memory_mb >= want.memory_mb
        && (has_desktop(member) || !has_desktop(want))
        && probes_covered(member, want)
        && canonical_of(member, backend, have, true) == canonical_of(want, backend, asked, true)
}

/// Whether a member of `spec` runs a desktop: every Space but a headless
/// one (`CUA_HEADLESS=1` in its environment).
fn has_desktop(spec: &StartSpec) -> bool {
    spec.env.get("CUA_HEADLESS").map(String::as_str) != Some("1")
}

/// `spec` without what is particular to one Space: its name, token, the
/// ephemeral label, pull details and credentials.
fn template_of(spec: &StartSpec) -> StartSpec {
    let mut t = spec.clone();
    t.name = String::new();
    for k in TOKEN_ENV {
        t.env.remove(k);
    }
    t.labels.remove(cua_vmm::container::LABEL_EPHEMERAL);
    t.labels.remove(cua_vmm::container::LABEL_POOL);
    t.pull_layers.clear();
    t.registry_auth = None;
    t.pool_member = false;
    t
}

/// The Spaces token in `spec`'s environment.
fn token_of(spec: &StartSpec) -> Option<String> {
    TOKEN_ENV
        .iter()
        .find_map(|k| spec.env.get(*k))
        .filter(|t| !t.is_empty())
        .cloned()
}

/// What a saved filesystem is keyed by, apart from the apps in it: the
/// backend, the image it was made from and its platform. Nothing else of a
/// shape (network preset, size, ports, environment) is in the saved files,
/// so a change of those never throws a prepared filesystem away.
fn prepared_base(template: &StartSpec, backend: BackendKind) -> String {
    use sha2::{Digest, Sha256};
    let image = match &template.image {
        cua_vmm::ImageSource::Oci { reference } => reference.as_str(),
        _ => "",
    };
    let mut h = Sha256::new();
    for part in [
        backend.as_str(),
        image,
        &format!("{:?}", template.os),
        &format!("{:?}", template.arch),
    ] {
        h.update(part.as_bytes());
        h.update([0]);
    }
    h.finalize()
        .iter()
        .take(6)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A saved filesystem: made from `base`, with these harnesses (and their
/// pins, [`PoolHost::pin`]) installed.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct PreparedEntry {
    tag: String,
    base: String,
    harnesses: std::collections::BTreeMap<String, String>,
}

/// The tag of the saved filesystem of `base` with `harnesses` (id to pin).
fn prepared_tag(base: &str, harnesses: &std::collections::BTreeMap<String, String>) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    for (id, pin) in harnesses {
        h.update(id.as_bytes());
        h.update([0]);
        h.update(pin.as_bytes());
        h.update([0]);
    }
    let d = h.finalize();
    format!(
        "warm-{base}-{}",
        d.iter()
            .take(4)
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    )
}

/// A fresh name for a member.
fn member_name() -> String {
    format!(
        "{}{:016x}",
        cua_vmm::POOL_MEMBER_PREFIX,
        rand::random::<u64>()
    )
}

/// A fresh pool secret (128 random bits, hex).
fn pool_secret() -> String {
    format!("{:032x}", rand::random::<u128>())
}

impl WarmPool {
    /// A pool keeping its records under `<state>/.pool/` and its members'
    /// leases in `leases` (the daemon's state store).
    pub fn new(host: Arc<dyn PoolHost>, leases: StateStore, settings: PoolSettings) -> Self {
        let dir = leases.dir().join(".pool");
        Self {
            inner: Arc::new(Inner {
                shapes: Mutex::new(HashMap::new()),
                host,
                settings: Mutex::new(settings),
                leases,
                dir,
                claims: Mutex::new(HashMap::new()),
                refilling: Mutex::new(HashSet::new()),
                starting: Mutex::new(HashSet::new()),
                closed: Default::default(),
                closed_signal: Default::default(),
                pid: std::process::id(),
                epoch: Instant::now(),
                ticks: Default::default(),
                class: policy::HostClass::current(),
                held_off_until: Mutex::new(None),
                engine_down_until: Mutex::new(HashMap::new()),
                arrived: tokio::sync::Notify::new(),
                due: Mutex::new(HashSet::new()),
                deferred: Mutex::new(HashMap::new()),
                building: Mutex::new(HashSet::new()),
                build_blocked: Mutex::new(HashMap::new()),
                index_lock: Mutex::new(()),
            }),
        }
    }

    /// The settings in force.
    pub fn settings(&self) -> PoolSettings {
        self.inner.settings.lock().unwrap().clone()
    }

    /// Replaces the settings (a tick reads them again).
    pub fn set_settings(&self, s: PoolSettings) {
        *self.inner.settings.lock().unwrap() = s;
    }

    /// What the pool holds, and why: the reason of the shape with the most
    /// members, else of the shape created last.
    pub fn status(&self) -> PoolStatus {
        let shapes = self.inner.shapes.lock().unwrap();
        let mut st = PoolStatus::default();
        for s in shapes.values() {
            st.members += s.members.len() as u32;
            st.starting += s.starting;
            st.memory_mb += s.members.iter().map(|m| m.footprint_mb).sum::<u64>();
        }
        let busiest = shapes
            .values()
            .filter(|s| !s.members.is_empty() || s.starting > 0)
            .max_by_key(|s| s.members.len() as u32 + s.starting);
        let latest = shapes.values().max_by(|a, b| {
            let t = |s: &Shape| s.arrivals.last().unwrap_or(f64::MIN);
            t(a).total_cmp(&t(b))
        });
        st.reason = if !self.settings().auto && busiest.is_none() {
            policy::Reason::Off.to_string()
        } else {
            match busiest.or(latest) {
                Some(s) => s.reason(),
                None => "no Spaces created yet".into(),
            }
        };
        st
    }

    /// Ready members by the words their shape was asked for with (image,
    /// kind, runtime) and its size (CPUs, MiB): what `on="auto"` placement
    /// matches a request against.
    pub fn ready_by_request(&self) -> Vec<Ready> {
        let shapes = self.inner.shapes.lock().unwrap();
        let mut out: HashMap<(String, String, String, u32, u64), u32> = HashMap::new();
        for s in shapes.values() {
            if let (Some((image, kind, runtime)), false) = (&s.requested, s.members.is_empty()) {
                let k = (
                    image.clone(),
                    kind.clone(),
                    runtime.clone(),
                    s.template.cpus,
                    s.template.memory_mb,
                );
                *out.entry(k).or_default() += s.members.len() as u32;
            }
        }
        out.into_iter()
            .map(|((image, kind, runtime, cpus, memory_mb), ready)| Ready {
                image,
                kind,
                runtime,
                cpus,
                memory_mb,
                ready,
            })
            .collect()
    }

    /// Notes the words a create of `spec`'s shape asked for.
    pub fn note_request(
        &self,
        spec: &StartSpec,
        backend: BackendKind,
        spacesd: bool,
        requested: (String, String, String),
    ) {
        let Some(key) = shape_key(spec, backend, spacesd) else {
            return;
        };
        if let Some(s) = self.inner.shapes.lock().unwrap().get_mut(&key) {
            s.requested = Some(requested);
        }
    }

    /// Seconds on the policy's clock.
    fn now_s(&self) -> f64 {
        self.inner.epoch.elapsed().as_secs_f64()
    }

    /// The policy's decision for shape `key` now (stored on the shape).
    fn evaluate(&self, key: &str) {
        let on_battery = self.inner.host.on_battery();
        let now = self.now_s();
        let mut shapes = self.inner.shapes.lock().unwrap();
        let Some(s) = shapes.get_mut(key) else { return };
        let d = policy::decide(
            &s.arrivals,
            now,
            s.costs(),
            self.inner.class,
            on_battery,
            MAX_AUTO_MEMBERS,
        );
        if s.decision.as_ref().map(|o| &o.reason) != Some(&d.reason) {
            tracing::debug!(shape = key, members = d.members, reason = %d.reason, "warm pool");
        }
        s.decision = Some(d);
    }

    /// Claims a ready member of `spec`'s shape for the Space `spec`
    /// describes (its name and token). `None`: no member was ready (or the
    /// claim failed and was cleaned up); the caller starts the Space cold.
    pub async fn claim(
        &self,
        spec: &StartSpec,
        backend: BackendKind,
        spacesd: bool,
    ) -> Option<Instance> {
        let Some(key) = shape_key(spec, backend, spacesd) else {
            tracing::debug!(space = %spec.name, "warm pool: this create is not of a shape that pools");
            return None;
        };
        let Some(token) = token_of(spec) else {
            tracing::debug!(space = %spec.name, "warm pool: this create has no Spaces token yet");
            return None;
        };
        let arrived = self.now_s();
        let want = template_of(spec);
        // The shape's own member first; else a bigger one the experiment
        // keeps warm ([`serves`]), whose shape then pays for the claim.
        let shapes_known = self.inner.shapes.lock().unwrap().len();
        let Some((member, rt, key)) = self.take_member(&key, &want, backend).await else {
            tracing::debug!(space = %spec.name, shapes_known, "warm pool: no member for this create");
            return None;
        };
        let started = Instant::now();
        let identity = ClaimIdentity {
            token: Some(token.clone()),
            hostname: Some(spec.name.clone()),
        };
        let claimed = rt.claim(&member.name, &spec.name, &identity).await;
        let claim_ms = started.elapsed().as_millis() as u64;
        let inst = match claimed {
            Ok(i) => i,
            Err(e) => {
                tracing::warn!(member = %member.name, error = %e, "warm-pool claim failed; starting cold");
                // The member may still have its own name (or be gone).
                self.discard(rt.clone(), member.name.clone());
                self.refill_later(&key, Duration::ZERO);
                return None;
            }
        };
        self.forget_member(&member.name);
        if let Err(e) = self
            .inner
            .host
            .rotate(&inst.endpoints, backend, &member.secret, &token)
            .await
        {
            tracing::warn!(space = %spec.name, error = %e, "a claimed warm-pool member did not take its token; starting cold");
            // Renamed to the Space: delete it under that name before the
            // cold start takes the name.
            match rt.delete(&spec.name).await {
                Ok(()) | Err(VmmError::NotFound(_)) => {}
                Err(e) => {
                    tracing::warn!(space = %spec.name, error = %e, "could not remove a failed claim");
                    return None;
                }
            }
            self.refill_later(&key, Duration::ZERO);
            return None;
        }
        let ready_s = started.elapsed().as_secs_f64();
        tracing::info!(
            space = %spec.name,
            member = %member.name,
            claim_ms,
            ready_ms = (ready_s * 1000.0) as u64,
            prepared = member.prepared,
            "claimed a warm Space"
        );
        if let Some(s) = self.inner.shapes.lock().unwrap().get_mut(&key) {
            s.arrivals.record(arrived);
            s.claim_s = ewma(s.claim_s, ready_s);
            s.last_used = Some(Instant::now());
        }
        self.evaluate(&key);
        // A shape kept warm on purpose is refilled at once: the next create
        // (typed while this Space starts) should find a member ready. Others
        // wait for the claimed Space's first commands to pass.
        let warm = self
            .inner
            .shapes
            .lock()
            .unwrap()
            .get(&key)
            .is_some_and(|s| s.is_warm_source());
        self.refill_later(&key, if warm { Duration::ZERO } else { REFILL_DELAY });
        Some(inst)
    }

    /// Takes a ready member of shape `key`. A shape kept warm for the
    /// Warm start ([`WARM_SOURCE`]) whose member is starting
    /// is waited for (up to [`WARM_WAIT_MAX`]): that member began before this
    /// create, so waiting is never slower than a cold start of its own, and
    /// a cold start would not have the agent stack in it. Any other shape
    /// answers at once.
    async fn take_member(
        &self,
        own: &str,
        want: &StartSpec,
        backend: BackendKind,
    ) -> Option<(Member, Arc<dyn VmmRuntime>, String)> {
        let deadline = Instant::now() + WARM_WAIT_MAX;
        let mut waited = false;
        loop {
            // Registered before the look, so a member that arrives between
            // the look and the wait is not missed.
            let notified = self.inner.arrived.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let starting = {
                let mut shapes = self.inner.shapes.lock().unwrap();
                // The shape itself, then the warm shapes that serve it.
                let mut keys = vec![own.to_string()];
                let mut warm: Vec<&String> = shapes
                    .iter()
                    .filter(|(k, s)| {
                        k.as_str() != own
                            && s.backend == backend
                            && s.is_warm_source()
                            && serves(&s.template, want, backend)
                    })
                    .map(|(k, _)| k)
                    .collect();
                // The smallest that serves first: it holds the least back.
                warm.sort_by_key(|k| {
                    let t = &shapes[*k].template;
                    (t.memory_mb, t.cpus)
                });
                keys.extend(warm.into_iter().cloned());
                for key in &keys {
                    let Some(shape) = shapes.get_mut(key) else {
                        continue;
                    };
                    // One with the coding apps in it first.
                    let at = shape
                        .members
                        .iter()
                        .rposition(|m| m.prepared)
                        .or(shape.members.len().checked_sub(1));
                    if let Some(m) = at.map(|i| shape.members.remove(i)) {
                        if waited {
                            tracing::info!(
                                shape = key.as_str(),
                                "a create waited for the member that was starting"
                            );
                        }
                        if key != own {
                            tracing::info!(
                                shape = key.as_str(),
                                "claiming a warm Space bigger than the one asked for"
                            );
                        }
                        return Some((m, shape.rt.clone(), key.clone()));
                    }
                }
                keys.iter().any(|k| {
                    shapes.get(k).is_some_and(|shape| {
                        shape.waits_for_refill()
                            && (shape.starting > 0
                                || self.inner.refilling.lock().unwrap().contains(k.as_str())
                                || self.inner.due.lock().unwrap().contains(k.as_str()))
                    })
                })
            };
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                tracing::warn!(
                    shape = own,
                    "warm-pools: the member that was starting did not arrive in time; starting cold"
                );
                return None;
            };
            if !starting {
                self.explain_miss(own, want, backend);
                if waited || self.is_warm_shape(own) {
                    tracing::info!(
                        shape = own,
                        "warm-pools: no prepared Space was ready and none is starting; starting cold"
                    );
                }
                return None;
            }
            waited = true;
            if tokio::time::timeout(left, notified).await.is_err() {
                tracing::warn!(
                    shape = own,
                    "warm-pools: the member that was starting did not arrive in time; starting cold"
                );
                return None;
            }
        }
    }

    /// Says why no warm Space of Warm start serves `want` (a debug
    /// aid for shapes that never match).
    fn explain_miss(&self, own: &str, want: &StartSpec, backend: BackendKind) {
        let shapes = self.inner.shapes.lock().unwrap();
        for (k, s) in shapes.iter().filter(|(_, s)| s.is_warm_source()) {
            let image = match &s.template.image {
                cua_vmm::ImageSource::Oci { reference } => reference.as_str(),
                _ => continue,
            };
            let (a, b) = (
                canonical_of(&s.template, backend, image, true),
                canonical_of(want, backend, image, true),
            );
            tracing::debug!(
                shape = own,
                warm = k.as_str(),
                member_cpus = s.template.cpus,
                member_mb = s.template.memory_mb,
                want_cpus = want.cpus,
                want_mb = want.memory_mb,
                same_but_size = (a == b),
                "warm-pools: a warm Space does not serve this create"
            );
            if a != b {
                tracing::debug!(member = %a, want = %b, "warm-pools: the shapes differ");
            }
        }
    }

    /// Whether shape `key` is kept warm for the Warm start.
    fn is_warm_shape(&self, key: &str) -> bool {
        self.inner
            .shapes
            .lock()
            .unwrap()
            .get(key)
            .is_some_and(|s| s.is_warm_source())
    }

    /// Records a create of `spec`'s shape that started on the default tier
    /// (no member was ready) in `start_s` seconds on `rt`: the policy may
    /// now want members of it.
    pub fn observe(
        &self,
        spec: &StartSpec,
        backend: BackendKind,
        spacesd: bool,
        rt: Arc<dyn VmmRuntime>,
        arrived: Instant,
        start_s: f64,
    ) {
        let Some(key) = shape_key(spec, backend, spacesd) else {
            return;
        };
        let at = arrived
            .checked_duration_since(self.inner.epoch)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        {
            let mut shapes = self.inner.shapes.lock().unwrap();
            let shape = shapes
                .entry(key.clone())
                .or_insert_with(|| Shape::new(template_of(spec), backend, rt));
            shape.arrivals.record(at);
            shape.default_s = ewma(shape.default_s, start_s);
            shape.last_used = Some(Instant::now());
            shape.failures = 0;
        }
        self.evaluate(&key);
        // After the Space settles: its first commands come first.
        self.refill_later(&key, REFILL_DELAY);
    }

    /// Keeps members of `template`'s shape for explicit pool `source`
    /// (`pool:<name>`, `warm:<image>`, `create`): `min` at all times, at
    /// most `max`, until `until` (`None`: until withdrawn). Returns the
    /// shape key, or `None` when the shape does not pool.
    #[allow(clippy::too_many_arguments)]
    pub fn set_explicit(
        &self,
        source: &str,
        template: &StartSpec,
        backend: BackendKind,
        spacesd: bool,
        rt: Arc<dyn VmmRuntime>,
        min: u32,
        max: Option<u32>,
        until: Option<Instant>,
        idle: Option<Duration>,
    ) -> Option<String> {
        self.set_explicit_prepared(
            source, template, backend, spacesd, rt, min, max, until, idle, None,
        )
    }

    /// [`WarmPool::set_explicit`] that also sets the harnesses the shape's
    /// members are prepared for before their first start (`Some`; `None`
    /// leaves them as they are).
    #[allow(clippy::too_many_arguments)]
    pub fn set_explicit_prepared(
        &self,
        source: &str,
        template: &StartSpec,
        backend: BackendKind,
        spacesd: bool,
        rt: Arc<dyn VmmRuntime>,
        min: u32,
        max: Option<u32>,
        until: Option<Instant>,
        idle: Option<Duration>,
        prepare: Option<Vec<String>>,
    ) -> Option<String> {
        let key = shape_key(template, backend, spacesd)?;
        {
            let mut shapes = self.inner.shapes.lock().unwrap();
            // A source belongs to one shape: an applied change moves it.
            for (k, s) in shapes.iter_mut() {
                if *k != key {
                    s.explicit.remove(source);
                }
            }
            let shape = shapes
                .entry(key.clone())
                .or_insert_with(|| Shape::new(template_of(template), backend, rt));
            // Applied again (every half minute): the idle clock keeps going.
            let since = shape
                .explicit
                .get(source)
                .map_or_else(Instant::now, |e| e.since);
            shape.explicit.insert(
                source.to_string(),
                Explicit {
                    min,
                    max,
                    until,
                    idle,
                    since,
                },
            );
            shape.failures = 0;
            if let Some(p) = prepare {
                shape.prepare = p;
            }
        }
        self.refill_later(&key, Duration::ZERO);
        Some(key)
    }

    /// Sets how many of the shape's apps (from the front of its list) a member
    /// is prepared for before it can be claimed ([`Shape::prepare_first`]).
    pub fn set_prepare_first(&self, key: &str, n: usize) {
        if let Some(s) = self.inner.shapes.lock().unwrap().get_mut(key) {
            s.prepare_first = n;
        }
    }

    /// Keeps `count` members of `template`'s shape regardless of use
    /// (`pool.warm`, keyed by the shape).
    pub fn keep(
        &self,
        template: &StartSpec,
        backend: BackendKind,
        spacesd: bool,
        rt: Arc<dyn VmmRuntime>,
        count: u32,
    ) -> Option<String> {
        let key = shape_key(template, backend, spacesd)?;
        self.set_explicit(
            &format!("warm:{key}"),
            template,
            backend,
            spacesd,
            rt,
            count,
            None,
            None,
            None,
        )
    }

    /// Withdraws explicit pool sources `keep` says no (pools deleted,
    /// `pool.warm` entries removed). Their members drain at the next tick.
    pub fn retain_explicit(&self, keep: impl Fn(&str) -> bool) {
        let mut shapes = self.inner.shapes.lock().unwrap();
        for s in shapes.values_mut() {
            s.explicit.retain(|src, _| keep(src));
        }
    }

    /// The named pool `name`.
    pub fn named(&self, name: &str) -> Option<named::NamedPool> {
        named::load(&self.inner.dir)
            .into_iter()
            .find(|p| p.name == name)
    }

    /// Every named pool.
    pub fn named_pools(&self) -> Vec<named::NamedPool> {
        named::load(&self.inner.dir)
    }

    /// Stores named pool `np` (replacing one of its name).
    pub fn save_named(&self, np: named::NamedPool) -> std::io::Result<()> {
        let mut all = named::load(&self.inner.dir);
        all.retain(|p| p.name != np.name);
        all.push(np);
        all.sort_by(|a, b| a.name.cmp(&b.name));
        named::save(&self.inner.dir, &all)
    }

    /// Removes named pool `name`; its ready Spaces drain at the next tick.
    pub fn remove_named(&self, name: &str) -> std::io::Result<bool> {
        let mut all = named::load(&self.inner.dir);
        let before = all.len();
        all.retain(|p| p.name != name);
        if all.len() == before {
            return Ok(false);
        }
        named::save(&self.inner.dir, &all)?;
        let source = format!("pool:{name}");
        self.retain_explicit(|s| s != source);
        Ok(true)
    }

    /// Ready and starting members of shape `key`, and why it holds them.
    pub fn shape_status(&self, key: &str) -> Option<(u32, u32, String)> {
        let shapes = self.inner.shapes.lock().unwrap();
        let s = shapes.get(key)?;
        Some((s.members.len() as u32, s.starting, s.reason()))
    }

    /// Whether `backend`'s runtime can claim members (asked once: a claim
    /// of a member that does not exist says "unsupported" or "not found").
    async fn claims_work(&self, backend: BackendKind, rt: &Arc<dyn VmmRuntime>) -> bool {
        if let Some(known) = self.inner.claims.lock().unwrap().get(&backend) {
            return *known;
        }
        let probe = format!("{}probe", cua_vmm::POOL_MEMBER_PREFIX);
        let works = !matches!(
            rt.claim(&probe, &probe, &ClaimIdentity::default()).await,
            Err(VmmError::Unsupported { .. })
        );
        if !works {
            tracing::info!(backend = %backend, "this runtime has no warm-pool claims; its Spaces start cold");
        }
        self.inner.claims.lock().unwrap().insert(backend, works);
        works
    }

    fn refill_later(&self, key: &str, delay: Duration) {
        let pool = self.clone();
        let key = key.to_string();
        self.inner.due.lock().unwrap().insert(key.clone());
        if let Ok(h) = tokio::runtime::Handle::try_current() {
            h.spawn(async move {
                if !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                    // Wait out a burst of creates of this shape: a member
                    // booting competes with the claims still coming (on a
                    // Mac, Apple's container services run one operation at
                    // a time, so a claim queued behind a member's start
                    // took seconds). Not longer than REFILL_MAX_WAIT, and
                    // not at all once the shape has no member left.
                    let give_up = tokio::time::Instant::now() + REFILL_MAX_WAIT;
                    while let Some(wait) = pool.burst_wait(&key, delay) {
                        if tokio::time::Instant::now() + wait > give_up {
                            break;
                        }
                        tokio::time::sleep(wait).await;
                    }
                }
                pool.refill(&key).await;
            });
        }
    }

    /// How much longer a refill of `key` waits for its creates to pause:
    /// `None` once none came within `quiet`, or when the shape has no ready
    /// member left (a refill is then due at once).
    fn burst_wait(&self, key: &str, quiet: Duration) -> Option<Duration> {
        let shapes = self.inner.shapes.lock().unwrap();
        let s = shapes.get(key)?;
        if s.members.is_empty() {
            return None;
        }
        let since = s.last_used?.elapsed();
        (since < quiet).then(|| quiet - since)
    }

    /// Starts members of shape `key` until it has its target, memory
    /// allowing. One refill per shape at a time.
    pub async fn refill(&self, key: &str) {
        let started = self.inner.refilling.lock().unwrap().insert(key.to_string());
        self.inner.due.lock().unwrap().remove(key);
        if !started {
            return;
        }
        struct Done<'a>(&'a Inner, String);
        impl Drop for Done<'_> {
            fn drop(&mut self) {
                self.0.refilling.lock().unwrap().remove(&self.1);
                // A create waiting for a member looks again: none is coming.
                self.0.arrived.notify_waiters();
            }
        }
        let _done = Done(&self.inner, key.to_string());
        loop {
            if self.inner.closed.load(std::sync::atomic::Ordering::SeqCst) {
                return;
            }
            let auto = self.settings().auto;
            let (template, backend, rt, estimate) = {
                let shapes = self.inner.shapes.lock().unwrap();
                let Some(s) = shapes.get(key) else { return };
                let have = s.members.len() as u32 + s.starting;
                if have >= s.target(auto) {
                    self.undefer(key);
                    return;
                }
                if s.failures >= MAX_FAILURES {
                    let wait = s
                        .failed_at
                        .map(|t| FAILURE_COOLDOWN.saturating_sub(t.elapsed()))
                        .unwrap_or_default();
                    if !wait.is_zero() {
                        let why = format!(
                            "the last {} member starts failed; trying again in {}s",
                            s.failures,
                            wait.as_secs()
                        );
                        drop(shapes);
                        self.defer(key, "failing", why);
                        return;
                    }
                }
                (s.template.clone(), s.backend, s.rt.clone(), s.estimate_mb())
            };
            // Waking the Linux runtime can mean installing and booting it: minutes a quitting daemon does not wait.
            let Some(claims) = self.until_closed(self.claims_work(backend, &rt)).await else {
                return;
            };
            if !claims {
                self.forget_shape(key);
                return;
            }
            let keep = {
                let shapes = self.inner.shapes.lock().unwrap();
                shapes.get(key).map(|s| s.keep()).unwrap_or(0)
            };
            if self.held_off() {
                self.defer(
                    key,
                    "held off",
                    "other work took capacity back a moment ago; no member starts for a while"
                        .into(),
                );
                return;
            }
            if keep == 0 && self.inner.host.busy() {
                self.defer(
                    key,
                    "busy",
                    "the machine's CPUs are busy with other work".into(),
                );
                return;
            }
            if keep > 0 {
                let down = self
                    .inner
                    .engine_down_until
                    .lock()
                    .unwrap()
                    .get(&backend)
                    .is_some_and(|t| *t > Instant::now());
                if down {
                    self.defer(
                        key,
                        "engine",
                        "the runtime is not up and did not start; trying again later".into(),
                    );
                    return;
                }
                let woke = self
                    .until_closed(async {
                        if self.inner.host.memory(backend).await.is_none() {
                            self.inner.host.wake(backend).await;
                            self.inner.host.memory(backend).await.is_some()
                        } else {
                            true
                        }
                    })
                    .await;
                match woke {
                    None => return,
                    Some(false) => {
                        self.inner
                            .engine_down_until
                            .lock()
                            .unwrap()
                            .insert(backend, Instant::now() + ENGINE_RETRY);
                        self.defer(
                            key,
                            "engine",
                            "the runtime did not start; trying again later".into(),
                        );
                        return;
                    }
                    Some(true) => {}
                }
            }
            let Some(allowed) = self
                .until_closed(self.memory_allows(backend, estimate, key))
                .await
            else {
                return;
            };
            if let Err(why) = allowed {
                self.defer(key, "memory", why);
                return;
            }
            {
                let mut shapes = self.inner.shapes.lock().unwrap();
                let Some(s) = shapes.get_mut(key) else { return };
                s.starting += 1;
            }
            let started = self.start_member(key, &template, backend, rt.clone()).await;
            let mut shapes = self.inner.shapes.lock().unwrap();
            let Some(s) = shapes.get_mut(key) else {
                // Drained while the member started: it goes too.
                if let Ok(m) = started {
                    drop(shapes);
                    self.inner.starting.lock().unwrap().remove(&m.name);
                    self.discard(rt, m.name);
                }
                return;
            };
            s.starting = s.starting.saturating_sub(1);
            if let Ok(m) = &started {
                self.inner.starting.lock().unwrap().remove(&m.name);
            }
            match started {
                Ok(m) => {
                    s.failures = 0;
                    s.failed_at = None;
                    if s.footprint_mb.is_none() || m.footprint_mb > 0 {
                        s.footprint_mb = Some(m.footprint_mb);
                    }
                    s.members.push(m);
                    self.undefer(key);
                }
                Err(e) => {
                    s.failures += 1;
                    s.failed_at = Some(Instant::now());
                    tracing::warn!(shape = key, error = %e, "a warm-pool member did not start");
                }
            }
            drop(shapes);
            self.inner.arrived.notify_waiters();
        }
    }

    /// Whether one more member estimated at `estimate_mb` fits: the pool
    /// stays within `pool.memory_percent` of the memory and above the free
    /// floor.
    ///
    /// The one warm Space of the Warm start ([`WARM_SOURCE`])
    /// is not held to the percentage while it is the only member held (on a
    /// built-in runtime of 4 GiB, 15 percent is less than one warm Space):
    /// it still needs the free floor, so it never takes memory a Space in use
    /// needs.
    async fn memory_allows(
        &self,
        backend: BackendKind,
        estimate_mb: u64,
        key: &str,
    ) -> Result<(), String> {
        let Some(mem) = self.inner.host.memory(backend).await else {
            return Err("the engine's memory cannot be read now".into());
        };
        let one_warm_home = self
            .inner
            .shapes
            .lock()
            .unwrap()
            .get(key)
            .is_some_and(|s| s.is_warm_source());
        let pct = u64::from(self.settings().memory_percent);
        let held: u64 = {
            let shapes = self.inner.shapes.lock().unwrap();
            shapes
                .values()
                .filter(|s| same_memory(s.backend, backend))
                .map(|s| {
                    s.members.iter().map(|m| m.footprint_mb).sum::<u64>()
                        + u64::from(s.starting) * s.estimate_mb()
                })
                .sum()
        };
        let (pct, held) = if one_warm_home && held == 0 {
            (100, 0)
        } else {
            (pct, held)
        };
        if fits(mem, pct, held, estimate_mb) {
            return Ok(());
        }
        let floor = memory_floor_mb(mem.total_mb);
        let used = mem.total_mb.saturating_sub(mem.available_mb);
        Err(format!(
            "not enough free memory: {} MiB of {} MiB in use by running Spaces, a member needs about {} MiB and {} MiB stay free (pool holds {} of its {} MiB share)",
            used,
            mem.total_mb,
            estimate_mb,
            floor,
            held,
            mem.total_mb * pct / 100
        ))
    }

    /// Says why the refill of `key` waits, once, and again when the reason
    /// changes or every [`DEFER_LOG_EVERY`] while it stays.
    fn defer(&self, key: &str, kind: &str, why: String) {
        let mut d = self.inner.deferred.lock().unwrap();
        let fresh = d
            .get(key)
            .is_none_or(|(k, at)| k != kind || at.elapsed() >= DEFER_LOG_EVERY);
        if fresh {
            tracing::info!(shape = key, reason = %why, "warm pool: refill deferred");
            d.insert(key.to_string(), (kind.to_string(), Instant::now()));
        }
    }

    /// The refill of `key` is no longer deferred.
    fn undefer(&self, key: &str) {
        self.inner.deferred.lock().unwrap().remove(key);
    }

    fn index_path(&self) -> PathBuf {
        self.inner.dir.join("prepared-index.json")
    }

    fn index_load(&self) -> Vec<PreparedEntry> {
        std::fs::read(self.index_path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn index_update(&self, f: impl FnOnce(&mut Vec<PreparedEntry>)) {
        let _g = self.inner.index_lock.lock().unwrap();
        let mut v = self.index_load();
        f(&mut v);
        let path = self.index_path();
        if cua_home::guard_write(&path).is_ok() {
            let _ = std::fs::create_dir_all(&self.inner.dir);
            if let Ok(b) = serde_json::to_vec_pretty(&v) {
                let _ = cua_home::write_private(&path, &b);
            }
        }
    }

    fn index_remove(&self, tag: &str) {
        self.index_update(|v| v.retain(|e| e.tag != tag));
    }

    /// The pin of each of `harnesses` in this build.
    fn pins(&self, harnesses: &[String]) -> std::collections::BTreeMap<String, String> {
        harnesses
            .iter()
            .map(|h| (h.clone(), self.inner.host.pin(h)))
            .collect()
    }

    /// The saved filesystem a member of `template`'s shape boots from: one
    /// of the same image with at least the `first` apps in it (at these
    /// pins), the one with most of `all`. A superset is as good as an exact
    /// match; nothing but the image and the apps keys it, so a build, a
    /// network preset or a size change does not throw it away.
    async fn choose_prepared(
        &self,
        key: &str,
        template: &StartSpec,
        backend: BackendKind,
        rt: &dyn VmmRuntime,
        all: &[String],
        first: &[String],
    ) -> Option<Chosen> {
        let base = prepared_base(template, backend);
        let pins = self.pins(all);
        let mut cands: Vec<(usize, PreparedEntry)> = Vec::new();
        let mut why: Vec<String> = Vec::new();
        for e in self.index_load().into_iter().filter(|e| e.base == base) {
            let has = |h: &String| e.harnesses.get(h) == pins.get(h);
            if let Some(m) = first.iter().find(|h| !has(h)) {
                why.push(format!("{} lacks {m} at this build's pin", e.tag));
                continue;
            }
            cands.push((all.iter().filter(|h| has(h)).count(), e));
        }
        cands.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then(a.1.harnesses.len().cmp(&b.1.harnesses.len()))
        });
        for (n, e) in cands {
            match rt.find_prepared(&e.tag).await {
                Ok(Some(image)) => {
                    tracing::info!(
                        shape = key,
                        tag = %e.tag,
                        covers = n,
                        wanted = all.len(),
                        "warm-pool: prepared filesystem reused"
                    );
                    return Some(Chosen {
                        covers_all: n == all.len(),
                        tag: e.tag,
                        image,
                        have: e.harnesses,
                    });
                }
                _ => {
                    why.push(format!("{} is gone from the engine", e.tag));
                    self.index_remove(&e.tag);
                }
            }
        }
        if why.is_empty() {
            why.push(format!("none saved for base {base}"));
        }
        tracing::info!(
            shape = key,
            wanted = %first.join(","),
            reason = %why.join("; "),
            "warm-pool: no prepared filesystem to reuse; the member is prepared"
        );
        None
    }

    /// Builds the saved filesystem with all of `all` in a member of its own
    /// (booted from `from`, which has some of them), in the background:
    /// the `first` apps are saved as soon as they are in, then the rest.
    /// Members that can be claimed never wait for a save.
    #[allow(clippy::too_many_arguments)]
    fn build_prepared(
        &self,
        key: &str,
        template: &StartSpec,
        backend: BackendKind,
        rt: Arc<dyn VmmRuntime>,
        all: Vec<String>,
        first: Vec<String>,
        from: Option<Chosen>,
    ) {
        if self.inner.closed.load(std::sync::atomic::Ordering::SeqCst) || !rt.keeps_prepared() {
            return;
        }
        if let Some(t) = self.inner.build_blocked.lock().unwrap().get(key)
            && *t > Instant::now()
        {
            return;
        }
        if !self.inner.building.lock().unwrap().insert(key.to_string()) {
            return;
        }
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            self.inner.building.lock().unwrap().remove(key);
            return;
        };
        let (pool, key, template) = (self.clone(), key.to_string(), template.clone());
        handle.spawn(async move {
            let began = Instant::now();
            // Not cut off when the pool closes: `build_inner` ends itself ([`Self::or_closed`]) and removes its own builder
            // container; dropping it here would leave the builder in `starting` and holding the container.
            let r = pool
                .build_inner(&key, &template, backend, rt, &all, &first, from)
                .await;
            pool.inner.building.lock().unwrap().remove(&key);
            match r {
                Ok(true) => tracing::info!(
                    shape = %key,
                    built_ms = began.elapsed().as_millis() as u64,
                    since_start_s = pool.inner.epoch.elapsed().as_secs(),
                    "warm-pool: the prepared filesystem has every app"
                ),
                // This runtime keeps nothing: members are prepared one by one.
                Ok(false) => {
                    pool.inner
                        .build_blocked
                        .lock()
                        .unwrap()
                        .insert(key, Instant::now() + Duration::from_secs(3600));
                }
                Err(e) => {
                    tracing::warn!(shape = %key, error = %e, "warm-pool: could not build the prepared filesystem; trying again later");
                    pool.inner
                        .build_blocked
                        .lock()
                        .unwrap()
                        .insert(key, Instant::now() + Duration::from_secs(600));
                }
            }
        });
    }

    #[allow(clippy::too_many_arguments)]
    async fn build_inner(
        &self,
        key: &str,
        template: &StartSpec,
        backend: BackendKind,
        rt: Arc<dyn VmmRuntime>,
        all: &[String],
        first: &[String],
        from: Option<Chosen>,
    ) -> Result<bool, String> {
        let base = prepared_base(template, backend);
        let pins = self.pins(all);
        let name = member_name();
        let secret = pool_secret();
        let record = Record {
            name: name.clone(),
            key: key.to_string(),
            backend,
            secret: secret.clone(),
            template: template.clone(),
            keep: 0,
            prepared: false,
        };
        self.write_record(&record).map_err(|e| e.to_string())?;
        if let Err(e) = self.inner.leases.write_lease(&name, self.inner.pid) {
            self.remove_record(&name);
            return Err(e.to_string());
        }
        self.inner.starting.lock().unwrap().insert(name.clone());
        let work = self.or_closed(async {
            let mut spec = template.clone();
            spec.name = name.clone();
            spec.pool_member = true;
            if let Some(c) = &from {
                spec.image = c.image.clone();
            }
            spec.env.insert("CUA_ENV_TOKEN".into(), secret.clone());
            spec.labels
                .insert(cua_vmm::container::LABEL_POOL.into(), "true".into());
            let inst = rt.start(&spec).await.map_err(|e| e.to_string())?;
            if backend == BackendKind::Container {
                self.inner
                    .host
                    .member_ready(
                        &inst.endpoints,
                        &secret,
                        has_desktop(&spec),
                        MEMBER_READY_TIMEOUT,
                    )
                    .await?;
            }
            let mut have = from.map(|c| c.have).unwrap_or_default();
            // The apps a member needs to be claimed, saved at once; then the
            // rest.
            for layer in [first, all] {
                let missing: Vec<String> = layer
                    .iter()
                    .filter(|h| have.get(*h) != pins.get(*h))
                    .cloned()
                    .collect();
                if missing.is_empty() {
                    continue;
                }
                let began = Instant::now();
                self.inner
                    .host
                    .prepare(&inst.endpoints, &secret, &missing, PREPARE_TIMEOUT)
                    .await?;
                for h in &missing {
                    if let Some(p) = pins.get(h) {
                        have.insert(h.clone(), p.clone());
                    }
                }
                let tag = prepared_tag(&base, &have);
                let saving = Instant::now();
                match rt.save_prepared(&name, &tag).await {
                    Ok(Some(_)) => {
                        let entry = PreparedEntry {
                            tag: tag.clone(),
                            base: base.clone(),
                            harnesses: have.clone(),
                        };
                        self.index_update(|v| {
                            v.retain(|e| e.tag != tag);
                            v.push(entry);
                        });
                        tracing::info!(
                            shape = key,
                            %tag,
                            harnesses = %have.keys().cloned().collect::<Vec<_>>().join(","),
                            prepare_ms = began.elapsed().as_millis() as u64 - saving.elapsed().as_millis() as u64,
                            save_ms = saving.elapsed().as_millis() as u64,
                            since_start_s = self.inner.epoch.elapsed().as_secs(),
                            "warm-pool: the prepared filesystem is kept for the next member"
                        );
                    }
                    Ok(None) => return Ok(false),
                    Err(e) => return Err(e.to_string()),
                }
            }
            Ok(true)
        })
        .await;
        match rt.delete(&name).await {
            Ok(()) | Err(VmmError::NotFound(_)) => self.forget_member(&name),
            Err(e) => {
                tracing::warn!(member = %name, error = %e, "could not remove a warm-pool builder")
            }
        }
        self.inner.starting.lock().unwrap().remove(&name);
        work
    }

    async fn start_member(
        &self,
        key: &str,
        template: &StartSpec,
        backend: BackendKind,
        rt: Arc<dyn VmmRuntime>,
    ) -> Result<Member, String> {
        let name = member_name();
        let secret = pool_secret();
        let keep = {
            let shapes = self.inner.shapes.lock().unwrap();
            shapes.get(key).map(|s| s.keep()).unwrap_or(0)
        };
        let (all, first_n) = {
            let shapes = self.inner.shapes.lock().unwrap();
            shapes
                .get(key)
                .map(|s| (s.prepare.clone(), s.prepare_first))
                .unwrap_or_default()
        };
        let all = if self.inner.host.can_prepare(backend) {
            all
        } else {
            Vec::new()
        };
        // The apps a member must have before it can be claimed; the others
        // go into the saved filesystem in the background.
        let first: Vec<String> = all
            .iter()
            .take(if first_n == 0 { all.len() } else { first_n })
            .cloned()
            .collect();
        let prepare = first.clone();
        let mut record = Record {
            name: name.clone(),
            key: key.to_string(),
            backend,
            secret: secret.clone(),
            template: template.clone(),
            keep,
            prepared: false,
        };
        // Recorded and leased before anything exists: a crash from here on
        // leaves a lease whose process is gone, and the reaper removes it.
        self.write_record(&record).map_err(|e| e.to_string())?;
        if let Err(e) = self.inner.leases.write_lease(&name, self.inner.pid) {
            self.remove_record(&name);
            return Err(e.to_string());
        }
        let mut spec = template.clone();
        spec.name = name.clone();
        spec.pool_member = true;
        // A member boots from a saved filesystem that has the apps it needs
        // (a build or a daemon before saved it): they are in it already, so
        // it is ready in the time of a boot, not of an install.
        let chosen = if all.is_empty() {
            None
        } else {
            self.choose_prepared(key, template, backend, rt.as_ref(), &all, &first)
                .await
        };
        if let Some(s) = self.inner.shapes.lock().unwrap().get_mut(key) {
            s.prepared_image = chosen.as_ref().map(|c| (c.tag.clone(), c.image.clone()));
        }
        let saved = chosen.as_ref().map(|c| (c.tag.clone(), c.image.clone()));
        if let Some((_, image)) = &saved {
            spec.image = image.clone();
        }
        spec.env.insert("CUA_ENV_TOKEN".into(), secret.clone());
        spec.labels.insert(
            cua_vmm::container::LABEL_POOL.into(),
            if keep > 0 {
                cua_vmm::container::POOL_KEEP.into()
            } else {
                "true".into()
            },
        );
        let started = Instant::now();
        // Until its shape holds it (the refill moves it there), the sweep
        // of unknown members leaves it alone.
        self.inner.starting.lock().unwrap().insert(name.clone());
        let mut prepared = false;
        let ready = self
            .or_closed(async {
                let inst = rt.start(&spec).await.map_err(|e| e.to_string());
                if inst.is_err()
                    && let Some((tag, _)) = &saved
                {
                    // The saved filesystem does not start any more (the engine
                    // lost it): the next member is prepared from scratch.
                    if let Some(s) = self.inner.shapes.lock().unwrap().get_mut(key) {
                        s.prepared_image = None;
                    }
                    self.index_remove(tag);
                }
                let inst = inst?;
                // A container member runs with its pool secret: it is ready
                // once it serves a command and a screenshot. Other backends
                // return from `start` with a member ready to claim (a microVM
                // waits paused, unreachable until claimed).
                if backend == BackendKind::Container {
                    self.inner
                        .host
                        .member_ready(
                            &inst.endpoints,
                            &secret,
                            has_desktop(&spec),
                            MEMBER_READY_TIMEOUT,
                        )
                        .await?;
                }
                Ok::<_, String>(inst)
            })
            .await;
        let booted = started.elapsed();
        // The agent stack goes in while the member is hidden, so the Space
        // that claims it has its coding apps. A member that cannot be
        // prepared is still a member: it is claimed bare, and the stack goes
        // in after the claim as it does without a pool.
        if ready.is_ok() && saved.is_some() {
            prepared = true;
            record.prepared = true;
            let _ = self.write_record(&record);
        } else if let Ok(inst) = &ready
            && !prepare.is_empty()
        {
            let began = Instant::now();
            match self
                .or_closed(async {
                    self.inner
                        .host
                        .prepare(&inst.endpoints, &secret, &prepare, PREPARE_TIMEOUT)
                        .await
                })
                .await
            {
                Ok(()) => {
                    prepared = true;
                    record.prepared = true;
                    let _ = self.write_record(&record);
                    let took = began.elapsed().as_secs_f64();
                    if let Some(s) = self.inner.shapes.lock().unwrap().get_mut(key) {
                        s.prepare_s = ewma(s.prepare_s, took);
                    }
                    tracing::info!(member = %name, harnesses = %prepare.join(","), prepare_ms = (took * 1000.0) as u64, "warm-pool member prepared");
                }
                Err(e) => {
                    tracing::warn!(member = %name, error = %e, "warm-pool member could not be prepared; it is claimed bare");
                }
            }
        }
        match ready {
            Ok(_) => {}
            Err(e) => {
                let _ = rt.delete(&name).await;
                self.forget_member(&name);
                self.inner.starting.lock().unwrap().remove(&name);
                return Err(e);
            }
        };
        if self.inner.closed.load(std::sync::atomic::Ordering::SeqCst) {
            // Drained while it was being made (or prepared): it is not kept.
            let _ = rt.delete(&name).await;
            self.forget_member(&name);
            self.inner.starting.lock().unwrap().remove(&name);
            return Err("the pool closed".to_string());
        }
        let footprint_mb = self.inner.host.used_mb(backend, &name).await.unwrap_or(0);
        if let Some(s) = self.inner.shapes.lock().unwrap().get_mut(key) {
            // The boot alone: preparing and saving are measured on their own.
            s.boot_s = ewma(s.boot_s, booted.as_secs_f64());
        }
        tracing::info!(
            member = %name,
            shape = key,
            ready_ms = started.elapsed().as_millis() as u64,
            claimable_after_start_s = self.inner.epoch.elapsed().as_secs_f64().round() as u64,
            prepared_for = %prepare.join(","),
            from = saved.as_ref().map_or("the image", |(t, _)| t.as_str()),
            footprint_mb,
            "warm-pool member ready"
        );
        // The saved filesystem is built beside the members, never from one
        // that can be claimed (a claim changes what is in it).
        if !all.is_empty() && !chosen.as_ref().is_some_and(|c| c.covers_all) {
            self.build_prepared(
                key,
                template,
                backend,
                rt.clone(),
                all.clone(),
                first.clone(),
                chosen.clone(),
            );
        }
        // A member that booted from the saved filesystem has nothing in its
        // file cache: its apps are read now, while it waits to be claimed.
        if saved.is_some()
            && prepared
            && let Ok(inst) = &ready
        {
            let pool = self.clone();
            let (endpoints, secret, apps) =
                (inst.endpoints.clone(), secret.clone(), prepare.clone());
            tokio::spawn(async move {
                pool.inner.host.warm_files(&endpoints, &secret, &apps).await;
            });
        }
        Ok(Member {
            name,
            secret,
            footprint_mb,
            prepared,
        })
    }

    /// Keeps every shape at what the policy (or its explicit pool) wants
    /// now: members beyond it drain at once, missing ones start. Shapes
    /// without a create for the policy's horizon are forgotten. Every
    /// [`MAINTAIN_EVERY`] ticks it also forgets vanished members, removes
    /// strays and shrinks under memory pressure. The daemon ticks every few
    /// seconds (a pre-warm window opens on time).
    pub async fn tick(&self) {
        let auto = self.settings().auto;
        let now = self.now_s();
        let keys: Vec<String> = self.inner.shapes.lock().unwrap().keys().cloned().collect();
        {
            // Explicit pools that ran out (a create's `warm`).
            let at = Instant::now();
            let mut shapes = self.inner.shapes.lock().unwrap();
            for s in shapes.values_mut() {
                s.explicit.retain(|_, e| e.until.is_none_or(|u| u > at));
            }
        }
        for k in &keys {
            self.evaluate(k);
        }
        let mut drained = Vec::new();
        let mut saved_gone: Vec<(Arc<dyn VmmRuntime>, String)> = Vec::new();
        {
            let mut shapes = self.inner.shapes.lock().unwrap();
            for k in &keys {
                let Some(shape) = shapes.get_mut(k) else {
                    continue;
                };
                let stale = shape.explicit.is_empty()
                    && shape.members.is_empty()
                    && shape.starting == 0
                    && shape
                        .arrivals
                        .last()
                        .is_none_or(|t| now - t > policy::HORIZON_S);
                if stale {
                    if let Some(gone) = shapes.remove(k)
                        && let Some((tag, _)) = gone.prepared_image
                    {
                        saved_gone.push((gone.rt.clone(), tag));
                    }
                    continue;
                }
                let excess = shape
                    .members
                    .len()
                    .saturating_sub(shape.target(auto) as usize);
                for _ in 0..excess {
                    if let Some(m) = shape.members.pop() {
                        drained.push((shape.rt.clone(), m, shape.reason()));
                    }
                }
            }
        }
        for (rt, tag) in saved_gone {
            self.drop_saved(rt, tag);
        }
        for (rt, m, why) in drained {
            tracing::info!(member = %m.name, reason = %why, "warm pool: a member is no longer wanted");
            self.discard(rt, m.name.clone());
        }
        let n = self
            .inner
            .ticks
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if n.is_multiple_of(MAINTAIN_EVERY) {
            self.forget_vanished().await;
            self.sweep_strays().await;
            self.shrink_under_pressure().await;
        }
        for k in keys {
            self.refill_later(&k, Duration::ZERO);
        }
    }

    /// Forgets members that are gone (the built-in runtime's idle stop
    /// removes them with its VM).
    async fn forget_vanished(&self) {
        let all: Vec<(Arc<dyn VmmRuntime>, String)> = {
            let shapes = self.inner.shapes.lock().unwrap();
            shapes
                .values()
                .flat_map(|s| s.members.iter().map(|m| (s.rt.clone(), m.name.clone())))
                .collect()
        };
        for (rt, name) in all {
            let gone = match rt.status(&name).await {
                Ok(cua_vmm::Status::Running | cua_vmm::Status::Paused) => false,
                Err(VmmError::NotFound(_)) => true,
                Ok(_) => {
                    self.discard(rt, name.clone());
                    true
                }
                Err(_) => false,
            };
            if gone {
                let mut shapes = self.inner.shapes.lock().unwrap();
                for s in shapes.values_mut() {
                    s.members.retain(|m| m.name != name);
                }
                drop(shapes);
                self.forget_member(&name);
            }
        }
    }

    /// Removes members no shape knows (left by a daemon that stopped
    /// while the runtime was down, or by a crash between a start and its
    /// record), on backends that can list them.
    async fn sweep_strays(&self) {
        let backends: Vec<(BackendKind, Arc<dyn VmmRuntime>)> = {
            let shapes = self.inner.shapes.lock().unwrap();
            let mut seen = HashSet::new();
            shapes
                .values()
                .filter(|s| seen.insert(s.backend))
                .map(|s| (s.backend, s.rt.clone()))
                .collect()
        };
        for (backend, rt) in backends {
            let Some(present) = self.inner.host.members_present(backend).await else {
                continue;
            };
            let known: HashSet<String> = {
                let shapes = self.inner.shapes.lock().unwrap();
                let mut k: HashSet<String> = shapes
                    .values()
                    .flat_map(|s| s.members.iter().map(|m| m.name.clone()))
                    .collect();
                k.extend(self.inner.starting.lock().unwrap().iter().cloned());
                k
            };
            for name in present.into_iter().filter(|n| !known.contains(n)) {
                tracing::info!(member = %name, "warm pool: removing a member no shape knows");
                self.discard(rt.clone(), name);
            }
        }
    }

    /// Removes members (newest first) while a backend's free memory is
    /// below the floor.
    async fn shrink_under_pressure(&self) {
        let backends: HashSet<BackendKind> = self
            .inner
            .shapes
            .lock()
            .unwrap()
            .values()
            .filter(|s| !s.members.is_empty())
            .map(|s| s.backend)
            .collect();
        for b in backends {
            let Some(mem) = self.inner.host.memory(b).await else {
                continue;
            };
            let mut short = memory_floor_mb(mem.total_mb).saturating_sub(mem.available_mb);
            while short > 0 {
                let victim = {
                    let mut shapes = self.inner.shapes.lock().unwrap();
                    shapes
                        .values_mut()
                        .filter(|s| same_memory(s.backend, b) && !s.members.is_empty())
                        .max_by_key(|s| s.estimate_mb())
                        .and_then(|s| s.members.pop().map(|m| (s.rt.clone(), m)))
                };
                let Some((rt, m)) = victim else { break };
                tracing::info!(member = %m.name, "warm pool: memory is short; removing a member");
                short = short.saturating_sub(m.footprint_mb.max(1));
                self.discard(rt, m.name.clone());
            }
        }
    }

    /// Adopts the members a previous daemon left (`runtime_of` gives each
    /// backend's runtime without booting anything): a running member whose
    /// Space answers to its pool secret rejoins its shape, leased to this
    /// daemon; anything else is removed.
    pub async fn adopt<F>(&self, runtime_of: F)
    where
        F: Fn(BackendKind) -> Option<Arc<dyn VmmRuntime>>,
    {
        for r in self.records() {
            let Some(rt) = runtime_of(r.backend) else {
                continue;
            };
            let status = rt.status(&r.name).await;
            match status {
                // A member may wait paused (a microVM).
                Ok(cua_vmm::Status::Running | cua_vmm::Status::Paused) => {}
                Err(VmmError::NotFound(_)) => {
                    self.forget_member(&r.name);
                    continue;
                }
                _ => {
                    self.discard(rt, r.name.clone());
                    continue;
                }
            }
            let endpoints = match rt.endpoints(&r.name).await {
                Ok(e) => e,
                Err(_) => {
                    self.discard(rt, r.name.clone());
                    continue;
                }
            };
            if r.backend == BackendKind::Container
                && self
                    .inner
                    .host
                    .member_ready(
                        &endpoints,
                        &r.secret,
                        has_desktop(&r.template),
                        Duration::from_secs(10),
                    )
                    .await
                    .is_err()
            {
                self.discard(rt, r.name.clone());
                continue;
            }
            let _ = self.inner.leases.write_lease(&r.name, self.inner.pid);
            let footprint_mb = self
                .inner
                .host
                .used_mb(r.backend, &r.name)
                .await
                .unwrap_or(0);
            let mut shapes = self.inner.shapes.lock().unwrap();
            let shape = shapes.entry(r.key.clone()).or_insert_with(|| {
                let mut s = Shape::new(r.template.clone(), r.backend, rt.clone());
                // Held for the explicit pool it came from until the daemon
                // applies its pools again (within a minute).
                if r.keep > 0 {
                    s.explicit.insert(
                        "adopted".into(),
                        Explicit {
                            min: r.keep,
                            max: None,
                            until: Some(Instant::now() + Duration::from_secs(90)),
                            idle: None,
                            since: Instant::now(),
                        },
                    );
                }
                s
            });
            if footprint_mb > 0 {
                shape.footprint_mb = Some(footprint_mb);
            }
            tracing::info!(member = %r.name, "warm pool: adopted a member");
            shape.members.push(Member {
                name: r.name.clone(),
                secret: r.secret.clone(),
                footprint_mb,
                prepared: r.prepared,
            });
        }
    }

    fn held_off(&self) -> bool {
        self.inner
            .held_off_until
            .lock()
            .unwrap()
            .is_some_and(|t| t > Instant::now())
    }

    /// What each ready member holds: its template's CPUs and memory, and
    /// whether it is a container.
    pub fn held(&self) -> Vec<cua_sandbox_core::WarmHeld> {
        let shapes = self.inner.shapes.lock().unwrap();
        shapes
            .values()
            .flat_map(|s| {
                let one = cua_sandbox_core::WarmHeld {
                    container: s.backend == BackendKind::Container,
                    cpus: s.template.cpus,
                    memory_mb: s.template.memory_mb,
                };
                std::iter::repeat_n(one, s.members.len())
            })
            .collect()
    }

    /// Takes unclaimed members back, one at a time (the largest first),
    /// until the CPU and memory in `need` are covered or none is left, and
    /// waits for each to be gone: other work is about to use what they held.
    /// The policy then starts no member of its own for a while
    /// ([`HOLD_OFF`]). Returns how many it removed.
    pub async fn reclaim(&self, need: cua_sandbox_core::Shortfall) -> u32 {
        let mut cpu = i64::from(need.millicpus);
        let mut mem = need.memory_mb as i64;
        let mut removed = 0;
        while cpu > 0 || mem > 0 {
            let victim = {
                let mut shapes = self.inner.shapes.lock().unwrap();
                shapes
                    .values_mut()
                    .filter(|s| !s.members.is_empty())
                    .max_by_key(|s| s.estimate_mb())
                    .and_then(|s| {
                        let m = s.members.pop()?;
                        Some((s.rt.clone(), m, s.template.cpus, s.template.memory_mb))
                    })
            };
            let Some((rt, m, cpus, memory_mb)) = victim else {
                break;
            };
            tracing::info!(member = %m.name, "warm pool: giving a member back to other work");
            match rt.delete(&m.name).await {
                Ok(()) | Err(VmmError::NotFound(_)) => self.forget_member(&m.name),
                Err(e) => {
                    tracing::warn!(member = %m.name, error = %e, "could not remove a warm-pool member");
                    continue;
                }
            }
            removed += 1;
            cpu -= i64::from(cpus) * 1000;
            mem -= m.footprint_mb.max(memory_mb) as i64;
        }
        if removed > 0 {
            *self.inner.held_off_until.lock().unwrap() = Some(Instant::now() + HOLD_OFF);
        }
        removed
    }

    /// Runs `work`, or gives up (`None`) the moment the pool is drained.
    async fn until_closed<T>(&self, work: impl std::future::Future<Output = T>) -> Option<T> {
        let closed = async {
            loop {
                let notified = self.inner.closed_signal.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if self.inner.closed.load(std::sync::atomic::Ordering::SeqCst) {
                    return;
                }
                notified.await;
            }
        };
        tokio::select! {
            r = work => Some(r),
            _ = closed => None,
        }
    }

    /// [`Self::until_closed`] for work that can fail: the pool closing is an error.
    async fn or_closed<T>(
        &self,
        work: impl std::future::Future<Output = Result<T, String>>,
    ) -> Result<T, String> {
        self.until_closed(work)
            .await
            .unwrap_or_else(|| Err("the pool closed".to_string()))
    }

    /// Removes every member and starts none again (this pool is done): what is being made is
    /// cancelled ([`Self::until_closed`]), then every member is deleted.
    pub async fn drain(&self) {
        self.inner
            .closed
            .store(true, std::sync::atomic::Ordering::SeqCst);
        // What is still being made (an image pull, a member booting, an app install that takes minutes) is cancelled, not
        // waited for: each such task sees the pool closed ([`Self::or_closed`]), removes its own container and ends. Waiting
        // for them to finish on their own held a quitting daemon for up to 90 s (the caller's bound) and a relaunch or
        // update with it. The wait here is only for those removals, a few seconds at most.
        self.inner.closed_signal.notify_waiters();
        let deadline = Instant::now() + DRAIN_CANCEL_WAIT;
        while !self.inner.starting.lock().unwrap().is_empty()
            || !self.inner.refilling.lock().unwrap().is_empty()
        {
            if Instant::now() >= deadline {
                tracing::warn!(
                    "warm pool: members still starting at drain; the reaper removes them"
                );
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let mut saved: Vec<(Arc<dyn VmmRuntime>, String)> = Vec::new();
        let all: Vec<(Arc<dyn VmmRuntime>, Member)> = {
            let mut shapes = self.inner.shapes.lock().unwrap();
            shapes
                .drain()
                .flat_map(|(_, s)| {
                    let rt = s.rt.clone();
                    if let Some((tag, _)) = &s.prepared_image {
                        saved.push((rt.clone(), tag.clone()));
                    }
                    s.members.into_iter().map(move |m| (rt.clone(), m))
                })
                .collect()
        };
        // The saved filesystems stay: a daemon that starts again finds them
        // ([`prepared_tag`]) instead of preparing a member for minutes.
        drop(saved);
        for (rt, m) in all {
            match rt.delete(&m.name).await {
                Ok(()) | Err(VmmError::NotFound(_)) => self.forget_member(&m.name),
                Err(e) => {
                    tracing::warn!(member = %m.name, error = %e, "could not remove a warm-pool member")
                }
            }
        }
    }

    /// Deletes member `name` in the background and forgets it once gone.
    fn discard(&self, rt: Arc<dyn VmmRuntime>, name: String) {
        let pool = self.clone();
        let go = async move {
            match rt.delete(&name).await {
                Ok(()) | Err(VmmError::NotFound(_)) => pool.forget_member(&name),
                // The lease stays: the reaper retries.
                Err(e) => {
                    tracing::warn!(member = %name, error = %e, "could not remove a warm-pool member")
                }
            }
        };
        if let Ok(h) = tokio::runtime::Handle::try_current() {
            h.spawn(go);
        }
    }

    fn forget_shape(&self, key: &str) {
        let gone = self.inner.shapes.lock().unwrap().remove(key);
        if let Some(s) = gone {
            // The saved filesystem stays (a shape that comes back finds it
            // by its image and apps; the disk budget removes the old).
            for m in s.members {
                self.discard(s.rt.clone(), m.name);
            }
        }
    }

    /// Deletes the saved filesystem `tag` in the background.
    fn drop_saved(&self, rt: Arc<dyn VmmRuntime>, tag: String) {
        if let Ok(h) = tokio::runtime::Handle::try_current() {
            h.spawn(async move {
                if let Err(e) = rt.drop_prepared(&tag).await {
                    tracing::debug!(%tag, error = %e, "warm pool: could not delete a saved filesystem");
                }
            });
        }
    }

    /// Drops member `name`'s record and lease.
    fn forget_member(&self, name: &str) {
        self.remove_record(name);
        let _ = self.inner.leases.remove_lease(name);
    }

    fn record_path(&self, name: &str) -> PathBuf {
        self.inner.dir.join(format!("{name}.json"))
    }

    fn write_record(&self, r: &Record) -> std::io::Result<()> {
        let path = self.record_path(&r.name);
        cua_home::guard_write(&path)?;
        std::fs::create_dir_all(&self.inner.dir)?;
        cua_home::write_private(&path, &serde_json::to_vec_pretty(r)?)
    }

    fn remove_record(&self, name: &str) {
        let _ = std::fs::remove_file(self.record_path(name));
    }

    fn records(&self) -> Vec<Record> {
        let Ok(rd) = std::fs::read_dir(&self.inner.dir) else {
            return vec![];
        };
        rd.filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| std::fs::read(e.path()).ok())
            .filter_map(|b| serde_json::from_slice::<Record>(&b).ok())
            .filter(|r| cua_vmm::is_pool_member(&r.name))
            .collect()
    }
}

/// Whether the policy may give a backend's shapes members. A Lume member
/// is a running macOS guest, and a Mac runs at most two: macOS shapes pool
/// only when `pool.warm` names them.
fn on_demand(backend: BackendKind) -> bool {
    backend != BackendKind::Lume
}

/// Whether members of `a` and `b` draw on the same memory: every backend
/// but the container engine runs on the host's.
fn same_memory(a: BackendKind, b: BackendKind) -> bool {
    (a == BackendKind::Container) == (b == BackendKind::Container)
}

/// Whether a member estimated at `estimate_mb` fits next to `held_mb` of
/// members: within `percent` of the memory, and above the free floor.
pub fn fits(mem: Memory, percent: u64, held_mb: u64, estimate_mb: u64) -> bool {
    let budget = mem.total_mb * percent / 100;
    held_mb + estimate_mb <= budget
        && mem.available_mb.saturating_sub(estimate_mb) >= memory_floor_mb(mem.total_mb)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_vmm::{
        CheckpointInfo, ImageSource, InstanceSummary, Isolation, Result as VResult, Status,
    };
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::SeqCst};

    /// A runtime whose instances are entries in a map: members claim by
    /// renaming, like the container backend.
    #[derive(Default)]
    struct FakeRuntime {
        instances: Mutex<HashMap<String, StartSpec>>,
        no_claims: bool,
        starts: AtomicU32,
        fail_starts: AtomicBool,
        /// How long a start takes, in ms (a booting member).
        start_ms: std::sync::atomic::AtomicU64,
        /// Whether `save_prepared` keeps a filesystem, and the tags kept
        /// and dropped.
        can_save: AtomicBool,
        saved: Mutex<Vec<String>>,
        dropped: Mutex<Vec<String>>,
        /// The image of every start, in order.
        images: Mutex<Vec<String>>,
    }

    fn instance(name: &str) -> Instance {
        Instance {
            name: name.into(),
            backend: BackendKind::Container,
            status: Status::Running,
            endpoints: Endpoints {
                host: "127.0.0.1".into(),
                ports: [(3211, 40000)].into(),
                ..Default::default()
            },
            isolation: Isolation::Gvisor,
            arch: None,
        }
    }

    #[async_trait]
    impl VmmRuntime for FakeRuntime {
        fn kind(&self) -> BackendKind {
            BackendKind::Container
        }
        async fn ensure_base(&self, _: &ImageSource, _: &str) -> VResult<CheckpointInfo> {
            Err(VmmError::invalid("unused"))
        }
        async fn start(&self, spec: &StartSpec) -> VResult<Instance> {
            self.starts.fetch_add(1, SeqCst);
            if let ImageSource::Oci { reference } = &spec.image {
                self.images.lock().unwrap().push(reference.clone());
            }
            let ms = self.start_ms.load(SeqCst);
            if ms > 0 {
                tokio::time::sleep(Duration::from_millis(ms)).await;
            }
            if self.fail_starts.load(SeqCst) {
                return Err(VmmError::other("boom"));
            }
            self.instances
                .lock()
                .unwrap()
                .insert(spec.name.clone(), spec.clone());
            Ok(instance(&spec.name))
        }
        async fn stop(&self, _: &str) -> VResult<()> {
            Ok(())
        }
        async fn claim(
            &self,
            member: &str,
            new_name: &str,
            identity: &ClaimIdentity,
        ) -> VResult<Instance> {
            if self.no_claims {
                return Err(VmmError::Unsupported {
                    backend: "fake",
                    op: "claim",
                });
            }
            let mut all = self.instances.lock().unwrap();
            let mut spec = all
                .remove(member)
                .filter(|s| s.pool_member)
                .ok_or_else(|| VmmError::NotFound(member.into()))?;
            if all.contains_key(new_name) {
                return Err(VmmError::AlreadyExists(new_name.into()));
            }
            spec.name = new_name.into();
            spec.env.insert(
                "CUA_ENV_TOKEN".into(),
                identity.token.clone().unwrap_or_default(),
            );
            all.insert(new_name.into(), spec);
            Ok(instance(new_name))
        }
        async fn fork(&self, _: &str, _: &str) -> VResult<()> {
            Ok(())
        }
        fn keeps_prepared(&self) -> bool {
            self.can_save.load(SeqCst)
        }
        async fn save_prepared(&self, _: &str, tag: &str) -> VResult<Option<ImageSource>> {
            if !self.can_save.load(SeqCst) {
                return Ok(None);
            }
            self.saved.lock().unwrap().push(tag.to_string());
            Ok(Some(ImageSource::oci(format!("local/prepared:{tag}"))))
        }
        async fn find_prepared(&self, tag: &str) -> VResult<Option<ImageSource>> {
            Ok(self
                .saved
                .lock()
                .unwrap()
                .iter()
                .any(|t| t == tag)
                .then(|| ImageSource::oci(format!("local/prepared:{tag}"))))
        }
        async fn drop_prepared(&self, tag: &str) -> VResult<()> {
            self.dropped.lock().unwrap().push(tag.to_string());
            Ok(())
        }
        async fn checkpoint(&self, _: &str, _: &str) -> VResult<CheckpointInfo> {
            Err(VmmError::invalid("unused"))
        }
        async fn list(&self) -> VResult<Vec<InstanceSummary>> {
            Ok(vec![])
        }
        async fn status(&self, name: &str) -> VResult<Status> {
            self.instances
                .lock()
                .unwrap()
                .contains_key(name)
                .then_some(Status::Running)
                .ok_or_else(|| VmmError::NotFound(name.into()))
        }
        async fn delete(&self, name: &str) -> VResult<()> {
            self.instances
                .lock()
                .unwrap()
                .remove(name)
                .map(drop)
                .ok_or_else(|| VmmError::NotFound(name.into()))
        }
        async fn endpoints(&self, name: &str) -> VResult<Endpoints> {
            Ok(instance(name).endpoints)
        }
    }

    impl FakeRuntime {
        fn members(&self) -> Vec<String> {
            let mut v: Vec<String> = self
                .instances
                .lock()
                .unwrap()
                .keys()
                .filter(|n| cua_vmm::is_pool_member(n))
                .cloned()
                .collect();
            v.sort();
            v
        }
    }

    /// cua-spacesd of every member: a token per instance port, rotated by
    /// `Init` only when the caller presents the current one.
    struct FakeHost {
        memory: Mutex<Memory>,
        rotations: Mutex<Vec<(String, String)>>,
        fail_rotate: AtomicBool,
        ready: AtomicBool,
        busy: AtomicBool,
        /// Whether members can be prepared, what was prepared (harnesses
        /// per member) and whether preparing fails.
        can_prepare: AtomicBool,
        prepared: Mutex<Vec<Vec<String>>>,
        fail_prepare: AtomicBool,
        prepare_ms: std::sync::atomic::AtomicU64,
        /// The runtime is not up and waking it takes a minute (the built-in Linux runtime installing and booting).
        runtime_down: AtomicBool,
        wake_ms: std::sync::atomic::AtomicU64,
        wakes: std::sync::atomic::AtomicU64,
    }

    impl Default for FakeHost {
        fn default() -> Self {
            Self {
                memory: Mutex::new(Memory {
                    total_mb: 96 * 1024,
                    available_mb: 80 * 1024,
                }),
                rotations: Mutex::new(vec![]),
                fail_rotate: AtomicBool::new(false),
                ready: AtomicBool::new(true),
                busy: AtomicBool::new(false),
                can_prepare: AtomicBool::new(true),
                prepared: Mutex::new(vec![]),
                fail_prepare: AtomicBool::new(false),
                prepare_ms: Default::default(),
                runtime_down: AtomicBool::new(false),
                wake_ms: Default::default(),
                wakes: Default::default(),
            }
        }
    }

    #[async_trait]
    impl PoolHost for FakeHost {
        async fn member_ready(
            &self,
            _: &Endpoints,
            _: &str,
            _: bool,
            _: Duration,
        ) -> Result<(), String> {
            if self.ready.load(SeqCst) {
                Ok(())
            } else {
                Err("not ready".into())
            }
        }
        async fn rotate(
            &self,
            _: &Endpoints,
            _: BackendKind,
            secret: &str,
            token: &str,
        ) -> Result<(), String> {
            if self.fail_rotate.load(SeqCst) {
                return Err("refused".into());
            }
            self.rotations
                .lock()
                .unwrap()
                .push((secret.to_string(), token.to_string()));
            Ok(())
        }
        fn can_prepare(&self, _: BackendKind) -> bool {
            self.can_prepare.load(SeqCst)
        }
        async fn prepare(
            &self,
            _: &Endpoints,
            _: &str,
            harnesses: &[String],
            _: Duration,
        ) -> Result<(), String> {
            let ms = self.prepare_ms.load(SeqCst);
            if ms > 0 {
                tokio::time::sleep(Duration::from_millis(ms)).await;
            }
            if self.fail_prepare.load(SeqCst) {
                return Err("no network".into());
            }
            self.prepared.lock().unwrap().push(harnesses.to_vec());
            Ok(())
        }
        async fn memory(&self, _: BackendKind) -> Option<Memory> {
            if self.runtime_down.load(SeqCst) {
                return None;
            }
            Some(*self.memory.lock().unwrap())
        }
        async fn used_mb(&self, _: BackendKind, _: &str) -> Option<u64> {
            Some(400)
        }
        async fn wake(&self, _: BackendKind) {
            self.wakes.fetch_add(1, SeqCst);
            if self.runtime_down.load(SeqCst) {
                let ms = self.wake_ms.load(SeqCst);
                tokio::time::sleep(if ms > 0 {
                    Duration::from_millis(ms)
                } else {
                    Duration::from_secs(60)
                })
                .await;
            }
        }
        async fn members_present(&self, _: BackendKind) -> Option<Vec<String>> {
            None
        }
        fn on_battery(&self) -> bool {
            false
        }
        fn busy(&self) -> bool {
            self.busy.load(SeqCst)
        }
    }

    fn spec(name: &str, token: &str) -> StartSpec {
        let mut s = StartSpec::new(
            name,
            ImageSource::oci("ghcr.io/trycua/linux@sha256:0123456789abcdef"),
        )
        .cpus(2)
        .memory_mb(4096)
        .port(3211);
        s.env.insert("CUA_ENV_TOKEN".into(), token.into());
        s
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        state: StateStore,
        rt: Arc<FakeRuntime>,
        host: Arc<FakeHost>,
        pool: WarmPool,
    }

    fn fixture(settings: PoolSettings, rt: FakeRuntime) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let state = StateStore::new(dir.path().join("sandboxes"));
        let host = Arc::new(FakeHost::default());
        let pool = WarmPool::new(host.clone(), state.clone(), settings);
        Fixture {
            _dir: dir,
            state,
            rt: Arc::new(rt),
            host,
            pool,
        }
    }

    /// An explicit pool of `n` members of `spec`'s shape, filled now.
    async fn warm_n(f: &Fixture, s: &StartSpec, n: u32) -> String {
        let rt: Arc<dyn VmmRuntime> = f.rt.clone();
        let key = f.pool.keep(s, BackendKind::Container, true, rt, n).unwrap();
        f.pool.refill(&key).await;
        key
    }

    async fn warm(f: &Fixture, s: &StartSpec) -> String {
        warm_n(f, s, 1).await
    }

    /// A create of `spec`'s shape that started cold in `start_s` seconds.
    fn create(f: &Fixture, s: &StartSpec, start_s: f64) {
        let rt: Arc<dyn VmmRuntime> = f.rt.clone();
        f.pool
            .observe(s, BackendKind::Container, true, rt, Instant::now(), start_s);
    }

    #[test]
    fn shapes_ignore_the_name_token_and_ephemeral_label_only() {
        let k = |s: &StartSpec| shape_key(s, BackendKind::Container, true);
        let a = spec("a", "t1");
        let mut b = spec("b", "t2");
        b.labels
            .insert(cua_vmm::container::LABEL_EPHEMERAL.into(), "true".into());
        b.pull_layers = vec![("sha256:1".into(), 1)];
        assert_eq!(k(&a), k(&b));
        assert!(k(&a).is_some());
        assert_ne!(k(&a), k(&spec("a", "t").cpus(4)));
        assert_ne!(k(&a), k(&spec("a", "t").memory_mb(2048)));
        assert_ne!(k(&a), k(&spec("a", "t").port(8080)));
        assert_ne!(k(&a), k(&spec("a", "t").env("FOO", "1")));
        assert_ne!(
            k(&a),
            shape_key(&spec("a", "t"), BackendKind::Qemu, true),
            "the backend is part of the shape"
        );
        let mut img = spec("a", "t");
        img.image = ImageSource::oci("ghcr.io/trycua/linux@sha256:fedcba9876543210");
        assert_ne!(k(&a), k(&img), "another digest is another shape");
    }

    #[test]
    fn only_plain_spacesd_spaces_pool() {
        let k = |s: &StartSpec, spacesd| shape_key(s, BackendKind::Container, spacesd);
        assert!(k(&spec("a", "t"), false).is_none(), "no cua-spacesd");
        let mut s = spec("a", "t");
        s.env.clear();
        assert!(k(&s, true).is_none(), "no Spaces token to rotate to");
        let mut s = spec("a", "t");
        s.gpu = Some("nvidia".into());
        assert!(k(&s, true).is_none(), "GPU");
        let mut s = spec("a", "t");
        s.disk_size_gb = Some(64);
        assert!(k(&s, true).is_none(), "disk size");
        let s = spec("a", "t").command(["sleep", "1"]);
        assert!(k(&s, true).is_none(), "command");
        // A headless Space's command (cua-spacesd alone) pools, apart from
        // the same image with its desktop.
        let mut headless = spec("a", "t");
        headless.command = Some(crate::local::headless_command_line());
        headless.env.insert("CUA_HEADLESS".into(), "1".into());
        let hk = k(&headless, true).expect("a headless shape pools");
        assert_ne!(Some(hk), k(&spec("a", "t"), true));
        assert!(!has_desktop(&headless));
        assert!(has_desktop(&spec("a", "t")));
        let mut s = spec("a", "t");
        s.sidecars.push(cua_vmm::SidecarSpec {
            name: "db".into(),
            image: "postgres".into(),
            command: None,
            env: Default::default(),
            ports: vec![],
        });
        assert!(k(&s, true).is_none(), "sidecars");
        let mut s = spec("a", "t");
        s.image = ImageSource::oci("cua-vmm/build:cua-b-1234");
        assert!(k(&s, true).is_none(), "a local build");
        let mut s = spec("a", "t");
        s.registry_auth = Some(cua_vmm::RegistryCredentials::new("u", "p"));
        assert!(k(&s, true).is_none(), "private registry");
        let mut s = spec("a", "t");
        s.forkable = true;
        assert!(
            shape_key(&s, BackendKind::Qemu, true).is_none(),
            "a fork source (its RAM is a shared file)"
        );
    }

    #[test]
    fn warm_setting_parses() {
        assert_eq!(parse_warm("none").unwrap(), vec![]);
        assert_eq!(parse_warm("").unwrap(), vec![]);
        assert_eq!(
            parse_warm("ghcr.io/trycua/linux:24.04-slim=8, alpine").unwrap(),
            vec![
                ("ghcr.io/trycua/linux:24.04-slim".to_string(), 8),
                ("alpine".to_string(), 1)
            ]
        );
        assert!(parse_warm("img=lots").is_err());
        assert!(parse_warm("=2").is_err());
    }

    #[test]
    fn the_memory_floor_and_share_bound_every_member() {
        let m = |total, avail| Memory {
            total_mb: total,
            available_mb: avail,
        };
        // 8 GiB runtime VM at 15%: 1228 MiB for members.
        assert!(fits(m(8192, 6000), 15, 0, 600));
        assert!(fits(m(8192, 6000), 15, 600, 600));
        assert!(!fits(m(8192, 6000), 15, 1200, 600), "over the share");
        // Free memory would drop below the floor (1 GiB here).
        assert!(!fits(m(8192, 1500), 15, 0, 600));
        assert_eq!(memory_floor_mb(8192), 1024);
        assert_eq!(memory_floor_mb(96 * 1024), 9830);
        // A 96 GiB host keeps many desktops; a 16 GiB laptop's 4 GiB VM one.
        assert!(fits(m(96 * 1024, 80 * 1024), 15, 8 * 600, 600));
        assert!(fits(m(4096, 3000), 15, 0, 600));
        assert!(!fits(m(4096, 3000), 15, 600, 600));
    }

    #[tokio::test]
    async fn the_second_create_of_a_shape_claims_a_member_with_its_own_token() {
        let f = fixture(PoolSettings::default(), FakeRuntime::default());
        let first = spec("one", "token-one");
        let key = warm(&f, &first).await;
        assert_eq!(f.rt.members().len(), 1);
        assert_eq!(f.pool.status().members, 1);
        // A member is leased to this daemon and recorded (0600).
        let member = f.rt.members()[0].clone();
        assert!(f.state.leases().iter().any(|l| l.name == member));
        let rec = f.pool.record_path(&member);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&rec).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let second = spec("two", "token-two");
        let inst = f
            .pool
            .claim(&second, BackendKind::Container, true)
            .await
            .expect("claimed");
        assert_eq!(inst.name, "two");
        // Renamed, out of the pool, its token rotated from the pool secret.
        assert!(f.rt.members().is_empty());
        let rotations = f.host.rotations.lock().unwrap().clone();
        assert_eq!(rotations.len(), 1);
        assert_eq!(rotations[0].1, "token-two");
        assert_ne!(rotations[0].0, "token-two");
        assert!(!rec.exists(), "the record goes with the claim");
        assert!(!f.state.leases().iter().any(|l| l.name == member));
        // The next create finds nothing until the pool refills.
        assert!(
            f.pool
                .claim(&spec("three", "t3"), BackendKind::Container, true)
                .await
                .is_none()
        );
        f.pool.refill(&key).await;
        assert!(
            f.pool
                .claim(&spec("three", "t3"), BackendKind::Container, true)
                .await
                .is_some()
        );
    }

    #[tokio::test]
    async fn a_refill_waits_for_a_burst_of_claims_to_pause() {
        let f = fixture(PoolSettings::default(), FakeRuntime::default());
        let key = warm_n(&f, &spec("one", "t"), 2).await;
        let quiet = Duration::from_millis(1500);
        // No claim yet since the members started: nothing to wait for.
        f.pool
            .inner
            .shapes
            .lock()
            .unwrap()
            .get_mut(&key)
            .unwrap()
            .last_used = None;
        assert_eq!(f.pool.burst_wait(&key, quiet), None);
        // Just claimed, members left: wait for the rest of the quiet time.
        assert!(
            f.pool
                .claim(&spec("a", "ta"), BackendKind::Container, true)
                .await
                .is_some()
        );
        let w = f
            .pool
            .burst_wait(&key, quiet)
            .expect("waits while claims come");
        assert!(w > Duration::from_millis(1000) && w <= quiet, "{w:?}");
        // The last member claimed: refill at once.
        assert!(
            f.pool
                .claim(&spec("b", "tb"), BackendKind::Container, true)
                .await
                .is_some()
        );
        assert_eq!(f.pool.burst_wait(&key, quiet), None);
    }

    #[tokio::test]
    async fn two_claims_never_share_a_member() {
        let f = fixture(PoolSettings::default(), FakeRuntime::default());
        warm(&f, &spec("one", "t")).await;
        let (sa, sb) = (spec("a", "ta"), spec("b", "tb"));
        let (a, b) = tokio::join!(
            f.pool.claim(&sa, BackendKind::Container, true),
            f.pool.claim(&sb, BackendKind::Container, true),
        );
        assert_eq!(a.is_some() as u8 + b.is_some() as u8, 1);
        let f = fixture(PoolSettings::default(), FakeRuntime::default());
        warm_n(&f, &spec("one", "t"), 2).await;
        assert_eq!(f.rt.members().len(), 2);
        let (sa, sb) = (spec("a", "ta"), spec("b", "tb"));
        let (a, b) = tokio::join!(
            f.pool.claim(&sa, BackendKind::Container, true),
            f.pool.claim(&sb, BackendKind::Container, true),
        );
        let (a, b) = (a.unwrap(), b.unwrap());
        assert_ne!(a.name, b.name);
        let secrets: HashSet<String> = f
            .host
            .rotations
            .lock()
            .unwrap()
            .iter()
            .map(|(s, _)| s.clone())
            .collect();
        assert_eq!(secrets.len(), 2, "every member has its own pool secret");
    }

    #[tokio::test]
    async fn another_shape_starts_cold() {
        let f = fixture(PoolSettings::default(), FakeRuntime::default());
        warm(&f, &spec("one", "t")).await;
        assert!(
            f.pool
                .claim(&spec("x", "t").cpus(8), BackendKind::Container, true)
                .await
                .is_none()
        );
        assert_eq!(f.rt.members().len(), 1, "the member stays for its shape");
    }

    #[tokio::test]
    async fn a_member_that_does_not_take_its_token_is_removed() {
        let f = fixture(PoolSettings::default(), FakeRuntime::default());
        warm(&f, &spec("one", "t")).await;
        f.host.fail_rotate.store(true, SeqCst);
        assert!(
            f.pool
                .claim(&spec("two", "t2"), BackendKind::Container, true)
                .await
                .is_none()
        );
        // Deleted under the Space's name, so a cold create can take it.
        assert!(f.rt.status("two").await.is_err());
        assert!(f.rt.members().is_empty());
    }

    #[tokio::test]
    async fn no_member_starts_below_the_memory_floor_or_when_pools_are_off() {
        let f = fixture(PoolSettings::default(), FakeRuntime::default());
        *f.host.memory.lock().unwrap() = Memory {
            total_mb: 8192,
            available_mb: 1200,
        };
        warm(&f, &spec("one", "t")).await;
        assert_eq!(f.rt.starts.load(SeqCst), 0);
        // Pools off: a burst of creates gets no member either.
        let off = PoolSettings {
            auto: false,
            ..Default::default()
        };
        let f = fixture(off, FakeRuntime::default());
        let s = spec("one", "t");
        for _ in 0..6 {
            create(&f, &s, 4.0);
        }
        let key = shape_key(&s, BackendKind::Container, true).unwrap();
        f.pool.refill(&key).await;
        assert_eq!(f.rt.starts.load(SeqCst), 0);
        assert_eq!(f.pool.status().reason, "warm pools are off");
    }

    #[tokio::test]
    async fn a_one_off_create_costs_nothing_and_says_why() {
        let f = fixture(PoolSettings::default(), FakeRuntime::default());
        assert_eq!(f.pool.status().reason, "no Spaces created yet");
        let s = spec("one", "t");
        create(&f, &s, 4.0);
        let key = shape_key(&s, BackendKind::Container, true).unwrap();
        f.pool.refill(&key).await;
        assert_eq!(f.rt.starts.load(SeqCst), 0);
        let st = f.pool.status();
        assert_eq!((st.members, st.reason.as_str()), (0, "no repeat traffic"));
    }

    #[tokio::test]
    async fn a_burst_of_creates_gets_members_and_loses_them_when_it_ends() {
        let f = fixture(PoolSettings::default(), FakeRuntime::default());
        let s = spec("one", "t");
        let key = shape_key(&s, BackendKind::Container, true).unwrap();
        // Small boots make the burst gap short (3 boots).
        create(&f, &s, 4.0);
        f.pool
            .inner
            .shapes
            .lock()
            .unwrap()
            .get_mut(&key)
            .unwrap()
            .boot_s = Some(0.05);
        for _ in 0..4 {
            create(&f, &s, 4.0);
        }
        f.pool.refill(&key).await;
        let started = f.rt.starts.load(SeqCst);
        assert!(started >= 1, "a burst is kept ready");
        assert!(
            f.pool.status().reason.starts_with("burst:"),
            "{:?}",
            f.pool.status()
        );
        // The burst ends: past the gap the members go.
        tokio::time::sleep(Duration::from_millis(400)).await;
        f.pool.tick().await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(f.rt.members().is_empty(), "{:?}", f.pool.status());
    }

    #[tokio::test]
    async fn work_that_needs_the_capacity_takes_idle_members_back_first() {
        let f = fixture(PoolSettings::default(), FakeRuntime::default());
        let s = spec("one", "t");
        let key = warm_n(&f, &s, 3).await;
        assert_eq!(f.rt.members().len(), 3);
        // Five CPUs and eight GiB short: two 2-CPU 4-GiB members cover the
        // memory, three the CPUs.
        let removed = f
            .pool
            .reclaim(cua_sandbox_core::Shortfall {
                millicpus: 5000,
                memory_mb: 8192,
            })
            .await;
        assert_eq!(removed, 3, "{:?}", f.pool.status());
        assert!(f.rt.members().is_empty());
        // The pool does not start them again at once: the capacity is
        // wanted elsewhere.
        f.pool.refill(&key).await;
        assert!(f.rt.members().is_empty(), "held off after a reclaim");
        // Nothing to take back: nothing is removed.
        let again = f
            .pool
            .reclaim(cua_sandbox_core::Shortfall {
                millicpus: 1000,
                memory_mb: 0,
            })
            .await;
        assert_eq!(again, 0);
    }

    #[tokio::test]
    async fn a_reclaim_takes_only_what_the_shortfall_needs() {
        let f = fixture(PoolSettings::default(), FakeRuntime::default());
        warm_n(&f, &spec("one", "t"), 3).await;
        let removed = f
            .pool
            .reclaim(cua_sandbox_core::Shortfall {
                millicpus: 2000,
                memory_mb: 0,
            })
            .await;
        assert_eq!(removed, 1);
        assert_eq!(f.rt.members().len(), 2);
    }

    #[tokio::test]
    async fn no_member_the_policy_only_wants_starts_on_a_saturated_machine() {
        let f = fixture(PoolSettings::default(), FakeRuntime::default());
        f.host.busy.store(true, SeqCst);
        let s = spec("one", "t");
        let key = shape_key(&s, BackendKind::Container, true).unwrap();
        create(&f, &s, 4.0);
        f.pool
            .inner
            .shapes
            .lock()
            .unwrap()
            .get_mut(&key)
            .unwrap()
            .boot_s = Some(0.05);
        for _ in 0..4 {
            create(&f, &s, 4.0);
        }
        f.pool.refill(&key).await;
        assert_eq!(f.rt.starts.load(SeqCst), 0, "busy: no speculative member");
        // A pool somebody asked for still fills.
        let g = fixture(PoolSettings::default(), FakeRuntime::default());
        g.host.busy.store(true, SeqCst);
        warm_n(&g, &spec("two", "t"), 1).await;
        assert_eq!(g.rt.members().len(), 1);
    }

    #[tokio::test]
    async fn memory_pressure_shrinks_the_pool() {
        let f = fixture(PoolSettings::default(), FakeRuntime::default());
        warm_n(&f, &spec("one", "t"), 3).await;
        assert_eq!(f.rt.members().len(), 3);
        *f.host.memory.lock().unwrap() = Memory {
            total_mb: 96 * 1024,
            available_mb: 9830 - 500,
        };
        f.pool.shrink_under_pressure().await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(f.rt.members().len(), 1, "two 400 MiB members cover 500 MiB");
    }

    #[tokio::test]
    async fn macos_shapes_pool_only_when_kept_warm_on_purpose() {
        let f = fixture(PoolSettings::default(), FakeRuntime::default());
        let rt: Arc<dyn VmmRuntime> = f.rt.clone();
        let s = spec("mac", "t").os(cua_vmm::GuestOs::Macos);
        for _ in 0..6 {
            f.pool.observe(
                &s,
                BackendKind::Lume,
                true,
                rt.clone(),
                Instant::now(),
                60.0,
            );
        }
        let key = shape_key(&s, BackendKind::Lume, true).unwrap();
        f.pool.refill(&key).await;
        assert_eq!(f.rt.starts.load(SeqCst), 0, "no macOS member on demand");
        assert_eq!(f.pool.status().members, 0);
        // `pool.warm` keeps one: exactly that many, whatever the traffic.
        f.pool
            .keep(&s, BackendKind::Lume, true, rt.clone(), 1)
            .unwrap();
        f.pool.refill(&key).await;
        assert_eq!(f.rt.starts.load(SeqCst), 1);
        // Creates of the kept shape do not add more.
        f.pool
            .observe(&s, BackendKind::Lume, true, rt, Instant::now(), 60.0);
        f.pool.refill(&key).await;
        assert_eq!(f.rt.starts.load(SeqCst), 1);
    }

    #[tokio::test]
    async fn a_runtime_without_claims_never_pools() {
        let f = fixture(
            PoolSettings::default(),
            FakeRuntime {
                no_claims: true,
                ..Default::default()
            },
        );
        warm(&f, &spec("one", "t")).await;
        assert_eq!(f.rt.starts.load(SeqCst), 0);
        assert_eq!(f.pool.status().members, 0);
    }

    #[tokio::test]
    async fn a_refill_short_of_memory_says_why_and_goes_ahead_when_memory_frees() {
        let f = fixture(PoolSettings::default(), FakeRuntime::default());
        *f.host.memory.lock().unwrap() = Memory {
            total_mb: 6144,
            available_mb: 1500,
        };
        let key = warm(&f, &spec("one", "t")).await;
        assert_eq!(f.rt.starts.load(SeqCst), 0);
        let said = f.pool.inner.deferred.lock().unwrap().get(&key).cloned();
        assert_eq!(said.map(|(kind, _)| kind), Some("memory".to_string()));
        // A Space deleted: the next tick's refill starts the member.
        *f.host.memory.lock().unwrap() = Memory {
            total_mb: 6144,
            available_mb: 5000,
        };
        f.pool.refill(&key).await;
        assert_eq!(f.rt.starts.load(SeqCst), 1);
        assert_eq!(f.pool.status().members, 1);
        assert!(f.pool.inner.deferred.lock().unwrap().get(&key).is_none());
        // Never more than the pool's size.
        f.pool.refill(&key).await;
        assert_eq!(f.rt.starts.load(SeqCst), 1);
    }

    #[tokio::test]
    async fn failed_starts_wait_out_a_cooldown_and_then_try_again() {
        let f = fixture(PoolSettings::default(), FakeRuntime::default());
        f.rt.fail_starts.store(true, SeqCst);
        let key = warm(&f, &spec("one", "t")).await;
        assert_eq!(f.rt.starts.load(SeqCst), MAX_FAILURES);
        // Still cooling down: no start, and the reason is recorded.
        f.pool.refill(&key).await;
        assert_eq!(f.rt.starts.load(SeqCst), MAX_FAILURES);
        let said = f.pool.inner.deferred.lock().unwrap().get(&key).cloned();
        assert_eq!(said.map(|(kind, _)| kind), Some("failing".to_string()));
        // After the cooldown one more start is tried, and a good one resets.
        f.rt.fail_starts.store(false, SeqCst);
        f.pool
            .inner
            .shapes
            .lock()
            .unwrap()
            .get_mut(&key)
            .unwrap()
            .failed_at = Some(Instant::now() - FAILURE_COOLDOWN - Duration::from_secs(1));
        f.pool.refill(&key).await;
        assert_eq!(f.pool.status().members, 1);
    }

    #[tokio::test]
    async fn failed_starts_stop_refilling_and_leave_nothing() {
        let f = fixture(PoolSettings::default(), FakeRuntime::default());
        f.rt.fail_starts.store(true, SeqCst);
        let key = warm(&f, &spec("one", "t")).await;
        f.pool.refill(&key).await;
        assert_eq!(f.rt.starts.load(SeqCst), MAX_FAILURES);
        assert!(f.state.leases().is_empty());
        assert!(f.pool.records().is_empty());
    }

    #[cfg(feature = "spaces")]
    #[tokio::test]
    async fn ready_members_are_reported_by_the_request_that_made_them() {
        let f = fixture(PoolSettings::default(), FakeRuntime::default());
        let s = spec("one", "t").memory_mb(4096);
        warm_n(&f, &s, 2).await;
        let image = "ghcr.io/trycua/linux:24.04-slim".to_string();
        let words = (image.clone(), String::new(), String::new());
        f.pool
            .note_request(&s, BackendKind::Container, true, words.clone());
        let big = spec("big", "t").cpus(4).memory_mb(8192);
        warm(&f, &big).await;
        f.pool
            .note_request(&big, BackendKind::Container, true, words);
        let mut keys: Vec<(String, u32)> = f
            .pool
            .ready_by_request()
            .iter()
            .flat_map(|r| {
                r.sizes().into_iter().map(move |(c, m)| {
                    (
                        cua_cluster::placement::warm_key(&r.image, &r.kind, &r.runtime, c, m),
                        r.ready,
                    )
                })
            })
            .collect();
        keys.sort();
        // The exact strings placement looks up: a default-size create
        // (empty size), an explicit 2/4096 one, and an explicit 4/8192 one.
        let mut want = vec![
            (format!("{image}||||"), 2),
            (format!("{image}|||2|4096"), 2),
            (format!("{image}|||4|8192"), 1),
        ];
        want.sort();
        assert_eq!(keys, want);
    }

    #[tokio::test]
    async fn explicit_pools_ignore_the_policy() {
        let f = fixture(PoolSettings::default(), FakeRuntime::default());
        // A one-off create: nothing of its shape runs.
        let one = spec("one", "t");
        create(&f, &one, 4.0);
        let rt: Arc<dyn VmmRuntime> = f.rt.clone();
        let kept = spec("k", "t").memory_mb(2048);
        let key = f
            .pool
            .keep(&kept, BackendKind::Container, true, rt, 2)
            .unwrap();
        f.pool.refill(&key).await;
        assert_eq!(f.rt.members().len(), 2);
        f.pool.tick().await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(f.rt.members().len(), 2, "exactly the explicit pool");
        let st = f.pool.status();
        assert_eq!((st.members, st.reason.as_str()), (2, "pool of 2"));
        assert_eq!(f.state.leases().len(), 2);
    }

    #[tokio::test]
    async fn a_new_daemon_adopts_live_members_and_drops_the_rest() {
        let f = fixture(PoolSettings::default(), FakeRuntime::default());
        warm_n(&f, &spec("one", "t"), 2).await;
        let members = f.rt.members();
        assert_eq!(members.len(), 2);
        // One member vanished while no daemon ran.
        f.rt.instances.lock().unwrap().remove(&members[0]);
        let next = WarmPool::new(f.host.clone(), f.state.clone(), PoolSettings::default());
        let rt: Arc<dyn VmmRuntime> = f.rt.clone();
        next.adopt(|_| Some(rt.clone())).await;
        assert_eq!(next.status().members, 1);
        assert_eq!(next.records().len(), 1);
        assert_eq!(f.state.leases().len(), 1);
        let inst = next
            .claim(&spec("two", "t2"), BackendKind::Container, true)
            .await
            .expect("an adopted member is claimed");
        assert_eq!(inst.name, "two");
    }

    #[tokio::test]
    async fn a_member_that_never_answers_is_removed() {
        let f = fixture(PoolSettings::default(), FakeRuntime::default());
        f.host.ready.store(false, SeqCst);
        warm(&f, &spec("one", "t")).await;
        assert!(f.rt.members().is_empty());
        assert!(f.state.leases().is_empty());
    }

    /// The `warm-pools` Space: one member, prepared with the coding
    /// apps, kept ready after every claim.
    async fn keep_warm_space(f: &Fixture, s: &StartSpec) -> String {
        let rt: Arc<dyn VmmRuntime> = f.rt.clone();
        f.pool
            .set_explicit_prepared(
                WARM_SOURCE,
                s,
                BackendKind::Container,
                true,
                rt,
                1,
                Some(1),
                None,
                Some(Duration::from_secs(7200)),
                Some(vec!["claude-code".into(), "openai-codex".into()]),
            )
            .unwrap()
    }

    /// Waits until shape `key` holds `n` ready members.
    async fn until_ready(f: &Fixture, key: &str, n: u32) {
        for _ in 0..200 {
            if f.pool.shape_status(key).is_some_and(|s| s.0 >= n) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!(
            "shape {key} never had {n} ready member(s): {:?}",
            f.pool.shape_status(key)
        );
    }

    /// A daemon that quits while a warm member is being prepared (an app install can take minutes) used to wait for the
    /// install: `drain` polled until nothing was starting, up to the caller's 90 s. It cancels the preparation now.
    #[tokio::test]
    async fn quitting_while_a_member_is_being_prepared_does_not_wait_for_the_preparation() {
        let f = fixture(PoolSettings::default(), FakeRuntime::default());
        f.host.prepare_ms.store(60_000, SeqCst);
        let key = keep_warm_space(&f, &spec("one", "t")).await;
        // The member is up and its apps are installing.
        for _ in 0..200 {
            if !f.rt.members().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(!f.rt.members().is_empty(), "a member started for {key}");
        assert!(
            f.pool.shape_status(&key).is_some_and(|s| s.0 == 0),
            "not ready: still preparing"
        );
        let began = Instant::now();
        f.pool.drain().await;
        assert!(
            began.elapsed() < Duration::from_secs(3),
            "drain took {:?}: it waited for the preparation",
            began.elapsed()
        );
        assert!(
            f.rt.members().is_empty(),
            "the member being prepared was removed: {:?}",
            f.rt.members()
        );
    }

    /// The first refill of a daemon can be waking the Linux runtime (installing and booting it takes minutes). Quitting
    /// does not wait for that either.
    #[tokio::test]
    async fn quitting_while_the_runtime_is_being_woken_does_not_wait_for_it() {
        let f = fixture(PoolSettings::default(), FakeRuntime::default());
        f.host.runtime_down.store(true, SeqCst);
        let rt: Arc<dyn VmmRuntime> = f.rt.clone();
        let key = f
            .pool
            .keep(&spec("one", "t"), BackendKind::Container, true, rt, 1)
            .unwrap();
        let pool = f.pool.clone();
        let refill = tokio::spawn(async move { pool.refill(&key).await });
        tokio::time::sleep(Duration::from_millis(200)).await;
        let began = Instant::now();
        f.pool.drain().await;
        let _ = tokio::time::timeout(Duration::from_secs(3), refill)
            .await
            .expect("the refill ended with the pool");
        assert!(
            began.elapsed() < Duration::from_secs(3),
            "{:?}",
            began.elapsed()
        );
        assert!(f.rt.members().is_empty());
    }

    /// A computer with no container engine: the pool wakes it once, then leaves it alone for a while instead of retrying on
    /// every tick (which kept an idle daemon busy).
    #[tokio::test]
    async fn a_runtime_that_will_not_come_up_is_not_woken_on_every_tick() {
        let f = fixture(PoolSettings::default(), FakeRuntime::default());
        f.host.runtime_down.store(true, SeqCst);
        f.host.wake_ms.store(1, SeqCst);
        let rt: Arc<dyn VmmRuntime> = f.rt.clone();
        let key = f
            .pool
            .keep(&spec("one", "t"), BackendKind::Container, true, rt, 1)
            .unwrap();
        for _ in 0..5 {
            f.pool.refill(&key).await;
        }
        assert_eq!(f.host.wakes.load(SeqCst), 1, "woken once, then left alone");
        assert!(f.rt.members().is_empty());
    }

    /// An idle daemon with a ready warm member quits at once.
    #[tokio::test]
    async fn quitting_with_a_ready_member_is_quick_and_takes_it_with_it() {
        let f = fixture(PoolSettings::default(), FakeRuntime::default());
        let key = keep_warm_space(&f, &spec("one", "t")).await;
        until_ready(&f, &key, 1).await;
        let began = Instant::now();
        f.pool.drain().await;
        assert!(
            began.elapsed() < Duration::from_secs(2),
            "{:?}",
            began.elapsed()
        );
        assert!(f.rt.members().is_empty());
    }

    #[tokio::test]
    async fn a_warm_pools_member_has_its_coding_apps_before_it_is_ready() {
        let f = fixture(PoolSettings::default(), FakeRuntime::default());
        let key = keep_warm_space(&f, &spec("one", "t")).await;
        until_ready(&f, &key, 1).await;
        assert_eq!(
            f.host.prepared.lock().unwrap().clone(),
            vec![vec!["claude-code".to_string(), "openai-codex".to_string()]],
            "prepared once, for both apps, before it counted as ready"
        );
        // The record says so: a daemon that adopts it knows.
        let rec = f.pool.records();
        assert_eq!(rec.len(), 1);
        assert!(rec[0].prepared);
    }

    #[tokio::test]
    async fn a_backend_that_cannot_prepare_its_members_keeps_them_bare() {
        let f = fixture(PoolSettings::default(), FakeRuntime::default());
        f.host.can_prepare.store(false, SeqCst);
        let key = keep_warm_space(&f, &spec("one", "t")).await;
        until_ready(&f, &key, 1).await;
        assert!(f.host.prepared.lock().unwrap().is_empty());
        assert!(!f.pool.records()[0].prepared);
        // A claim still works: the stack goes in after it, as without a pool.
        let c = f
            .pool
            .claim(&spec("mine", "tok"), BackendKind::Container, true)
            .await;
        assert!(c.is_some());
    }

    #[tokio::test]
    async fn a_member_that_fails_to_prepare_is_still_a_member() {
        let f = fixture(PoolSettings::default(), FakeRuntime::default());
        f.host.fail_prepare.store(true, SeqCst);
        let key = keep_warm_space(&f, &spec("one", "t")).await;
        until_ready(&f, &key, 1).await;
        assert!(!f.pool.records()[0].prepared, "claimed bare, never refused");
        assert!(
            f.pool
                .claim(&spec("mine", "tok"), BackendKind::Container, true)
                .await
                .is_some()
        );
    }

    #[tokio::test]
    async fn the_next_create_finds_a_member_because_a_claim_refills_at_once() {
        let f = fixture(PoolSettings::default(), FakeRuntime::default());
        let key = keep_warm_space(&f, &spec("one", "t")).await;
        until_ready(&f, &key, 1).await;
        for i in 0..4 {
            let got = f
                .pool
                .claim(
                    &spec(&format!("s{i}"), &format!("t{i}")),
                    BackendKind::Container,
                    true,
                )
                .await;
            assert!(got.is_some(), "create {i} claims a ready member");
            // Not the 1.5 s the other shapes wait: a few polls, not seconds.
            until_ready(&f, &key, 1).await;
        }
        assert_eq!(
            f.host.prepared.lock().unwrap().len(),
            5,
            "one member each, all prepared"
        );
    }

    /// A create the way the Spaces app's Home asks: one CPU, 1 GB, no desktop.
    fn home_spec(name: &str, token: &str) -> StartSpec {
        let mut s = spec(name, token).cpus(1).memory_mb(1024);
        for (k, v) in crate::local::headless_env() {
            s.env.insert(k, v);
        }
        s.command = Some(crate::local::headless_command_line());
        // A create waits for the port to accept; the warm member's check
        // (HTTP health) covers it.
        s.probes = vec![cua_vmm::Probe::tcp(3211)];
        s
    }

    #[tokio::test]
    async fn a_home_claims_the_bigger_warm_space_and_only_that() {
        let f = fixture(PoolSettings::default(), FakeRuntime::default());
        let mut member = spec("one", "t");
        member.probes = vec![cua_vmm::Probe::http(3211, "/health")];
        let warm_key = keep_warm_space(&f, &member).await;
        until_ready(&f, &warm_key, 1).await;
        assert_ne!(
            shape_key(&home_spec("h", "t"), BackendKind::Container, true),
            Some(warm_key.clone()),
            "the home is another shape"
        );
        // Bigger than the member: starts cold.
        let big = spec("big", "t1").cpus(8);
        assert!(
            f.pool
                .claim(&big, BackendKind::Container, true)
                .await
                .is_none()
        );
        // A desktop create is not served by a headless member.
        let started = Instant::now();
        let got = f
            .pool
            .claim(&home_spec("home-a", "t2"), BackendKind::Container, true)
            .await;
        assert!(got.is_some(), "the home claims the warm member");
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "a claim is the whole cost: {:?}",
            started.elapsed()
        );
        // The member is replaced for the next one.
        until_ready(&f, &warm_key, 1).await;
        assert_eq!(f.host.prepared.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_headless_member_does_not_serve_a_desktop_create_nor_a_plain_pool_a_home() {
        let f = fixture(PoolSettings::default(), FakeRuntime::default());
        // Kept warm headless: a desktop create is not served by it.
        let key = keep_warm_space(&f, &home_spec("one", "t")).await;
        until_ready(&f, &key, 1).await;
        assert!(
            f.pool
                .claim(&spec("desk", "t1"), BackendKind::Container, true)
                .await
                .is_none()
        );
        // A shape kept by `pool.warm` (not the experiment) is claimed only by its own shape.
        let f = fixture(PoolSettings::default(), FakeRuntime::default());
        let key = warm(&f, &spec("one", "t")).await;
        assert_eq!(f.pool.shape_status(&key).map(|s| s.0), Some(1));
        assert!(
            f.pool
                .claim(&home_spec("h", "t1"), BackendKind::Container, true)
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn a_create_waits_for_the_member_that_is_starting_instead_of_booting_another() {
        let rt = FakeRuntime::default();
        rt.start_ms.store(150, SeqCst);
        let f = fixture(PoolSettings::default(), rt);
        let key = keep_warm_space(&f, &spec("one", "t")).await;
        // The very first member (a new install) is not waited for: the
        // create starts cold.
        let t = Instant::now();
        assert!(
            f.pool
                .claim(&spec("early", "t0"), BackendKind::Container, true)
                .await
                .is_none(),
            "no member has been ready yet: cold"
        );
        assert!(t.elapsed() < Duration::from_millis(100));
        until_ready(&f, &key, 1).await;
        assert!(
            f.pool
                .claim(&spec("first", "t1"), BackendKind::Container, true)
                .await
                .is_some()
        );
        // Now a member is starting and its boot is known: the next create
        // waits for it.
        let t = Instant::now();
        let got = f
            .pool
            .claim(&spec("mine", "tok"), BackendKind::Container, true)
            .await;
        assert!(got.is_some(), "claimed the member that was starting");
        assert!(
            t.elapsed() >= Duration::from_millis(80),
            "it waited for the boot"
        );
        until_ready(&f, &key, 1).await;
    }

    #[tokio::test]
    async fn a_shape_that_is_not_kept_warm_never_makes_a_create_wait() {
        let rt = FakeRuntime::default();
        rt.start_ms.store(300, SeqCst);
        let f = fixture(PoolSettings::default(), rt);
        let rt2: Arc<dyn VmmRuntime> = f.rt.clone();
        // An ordinary explicit pool (`pool.warm`): a member is starting.
        let key = f
            .pool
            .keep(&spec("one", "t"), BackendKind::Container, true, rt2, 1)
            .unwrap();
        let pool = f.pool.clone();
        let refill = tokio::spawn(async move { pool.refill(&key).await });
        tokio::time::sleep(Duration::from_millis(50)).await;
        let t = Instant::now();
        let got = f
            .pool
            .claim(&spec("mine", "tok"), BackendKind::Container, true)
            .await;
        assert!(got.is_none(), "no member ready: the create starts cold");
        assert!(
            t.elapsed() < Duration::from_millis(100),
            "and does not wait"
        );
        refill.await.unwrap();
    }

    #[tokio::test]
    async fn a_create_stops_waiting_when_the_member_fails_to_start() {
        let rt = FakeRuntime::default();
        rt.start_ms.store(100, SeqCst);
        rt.fail_starts.store(true, SeqCst);
        let f = fixture(PoolSettings::default(), rt);
        keep_warm_space(&f, &spec("one", "t")).await;
        let t = Instant::now();
        let got = f
            .pool
            .claim(&spec("mine", "tok"), BackendKind::Container, true)
            .await;
        assert!(got.is_none(), "cold start, never a refused create");
        assert!(
            t.elapsed() < Duration::from_secs(5),
            "released when the start failed"
        );
    }

    #[tokio::test]
    async fn the_warm_space_goes_back_when_nobody_creates() {
        let f = fixture(PoolSettings::default(), FakeRuntime::default());
        let rt: Arc<dyn VmmRuntime> = f.rt.clone();
        let key = f
            .pool
            .set_explicit_prepared(
                WARM_SOURCE,
                &spec("one", "t"),
                BackendKind::Container,
                true,
                rt,
                1,
                Some(1),
                None,
                // Long enough that the first fill (the fake boots at once) lands before the pool
                // counts as idle; a 1 ms window raced that fill.
                Some(Duration::from_millis(500)),
                Some(vec!["claude-code".into()]),
            )
            .unwrap();
        until_ready(&f, &key, 1).await;
        tokio::time::sleep(Duration::from_millis(600)).await;
        // Idle now: the next tick gives the member back, nothing refills it,
        // and a create never waits for one.
        f.pool.tick().await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        f.pool.refill(&key).await;
        assert_eq!(
            f.rt.members().len(),
            0,
            "an idle warm Space keeps no member"
        );
        let t = Instant::now();
        assert!(
            f.pool
                .claim(&spec("m", "t"), BackendKind::Container, true)
                .await
                .is_none()
        );
        assert!(t.elapsed() < Duration::from_millis(100));
        // Withdrawn (the experiment turned off): the source is gone.
        f.pool.retain_explicit(|src| src != WARM_SOURCE);
        f.pool.tick().await;
        assert_eq!(f.pool.shape_status(&key).map(|s| s.0).unwrap_or(0), 0);
    }

    #[tokio::test]
    async fn the_one_warm_space_is_not_held_to_the_percentage_but_to_the_free_floor() {
        // A small engine VM: 15 percent of it is less than one member.
        let f = fixture(PoolSettings::default(), FakeRuntime::default());
        *f.host.memory.lock().unwrap() = Memory {
            total_mb: 3000,
            available_mb: 2500,
        };
        let rt: Arc<dyn VmmRuntime> = f.rt.clone();
        let plain = f
            .pool
            .keep(
                &spec("a", "t").memory_mb(1024),
                BackendKind::Container,
                true,
                rt,
                1,
            )
            .unwrap();
        f.pool.refill(&plain).await;
        assert_eq!(
            f.rt.members().len(),
            0,
            "an ordinary pool respects the share"
        );
        let key = keep_warm_space(&f, &spec("one", "t")).await;
        until_ready(&f, &key, 1).await;
        assert_eq!(
            f.rt.members().len(),
            1,
            "the warm Space gets its one member"
        );
        // Not below the floor: with 1.2 GB free it starts nothing.
        f.pool
            .claim(&spec("m", "tok"), BackendKind::Container, true)
            .await
            .unwrap();
        *f.host.memory.lock().unwrap() = Memory {
            total_mb: 3000,
            available_mb: 1200,
        };
        f.pool.refill(&key).await;
        assert_eq!(f.rt.members().len(), 0, "the free floor still holds");
    }

    /// Waits until the runtime kept `n` filesystems.
    async fn until_saved(f: &Fixture, n: usize) {
        for _ in 0..300 {
            if f.rt.saved.lock().unwrap().len() >= n {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("never saved {n}: {:?}", f.rt.saved.lock().unwrap());
    }

    #[tokio::test]
    async fn later_members_boot_from_the_filesystem_built_beside_the_first() {
        let f = fixture(PoolSettings::default(), FakeRuntime::default());
        f.rt.can_save.store(true, SeqCst);
        let key = keep_warm_space(&f, &spec("one", "t")).await;
        until_ready(&f, &key, 1).await;
        // The first member is claimable as prepared; its filesystem is kept
        // by a builder of its own, never by a member someone can claim.
        until_saved(&f, 1).await;
        for i in 0..3 {
            assert!(
                f.pool
                    .claim(
                        &spec(&format!("s{i}"), &format!("t{i}")),
                        BackendKind::Container,
                        true
                    )
                    .await
                    .is_some()
            );
            until_ready(&f, &key, 1).await;
        }
        let images = f.rt.images.lock().unwrap().clone();
        assert!(
            !images[0].starts_with("local/prepared:"),
            "the first boots the image"
        );
        assert!(
            images[images.len() - 1].starts_with("local/prepared:warm-"),
            "{images:?}"
        );
        assert_eq!(f.rt.saved.lock().unwrap().len(), 1, "kept once");
        assert!(f.pool.records().iter().all(|r| r.prepared));
        // A daemon that stops keeps it for the next one.
        f.pool.retain_explicit(|src| src != WARM_SOURCE);
        f.pool.drain().await;
        assert!(f.rt.dropped.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn the_first_member_is_claimable_after_the_first_apps_and_the_rest_follow() {
        let f = fixture(PoolSettings::default(), FakeRuntime::default());
        f.rt.can_save.store(true, SeqCst);
        let rt: Arc<dyn VmmRuntime> = f.rt.clone();
        let s0 = spec("one", "t");
        let key = f
            .pool
            .set_explicit_prepared(
                WARM_SOURCE,
                &s0,
                BackendKind::Container,
                true,
                rt,
                1,
                Some(1),
                None,
                Some(Duration::from_secs(7200)),
                Some(vec!["claude-code".into(), "hermes".into(), "goose".into()]),
            )
            .unwrap();
        f.pool.set_prepare_first(&key, 1);
        until_ready(&f, &key, 1).await;
        assert_eq!(
            f.host.prepared.lock().unwrap()[0],
            ["claude-code"],
            "the member waited for the first app only"
        );
        // Beside it: the first app is kept at once, then the whole set.
        until_saved(&f, 2).await;
        let base = prepared_base(&s0, BackendKind::Container);
        let idx = f.pool.index_load();
        let sizes: Vec<usize> = idx.iter().map(|e| e.harnesses.len()).collect();
        assert!(sizes.contains(&1) && sizes.contains(&3), "{sizes:?}");
        assert!(idx.iter().all(|e| e.base == base));
        // The next member starts from the full one, with nothing to install.
        f.pool
            .claim(&spec("s", "t1"), BackendKind::Container, true)
            .await
            .unwrap();
        until_ready(&f, &key, 1).await;
        let before = f.host.prepared.lock().unwrap().len();
        let images = f.rt.images.lock().unwrap().clone();
        assert!(images.last().unwrap().starts_with("local/prepared:warm-"));
        assert_eq!(f.host.prepared.lock().unwrap().len(), before);
        let full = idx
            .iter()
            .max_by_key(|e| e.harnesses.len())
            .unwrap()
            .tag
            .clone();
        assert!(images.last().unwrap().ends_with(&full), "{images:?}");
    }

    #[tokio::test]
    async fn a_changed_network_preset_or_size_keeps_the_prepared_filesystem() {
        let f = fixture(PoolSettings::default(), FakeRuntime::default());
        f.rt.can_save.store(true, SeqCst);
        let s0 = spec("one", "t");
        let have: std::collections::BTreeMap<String, String> = ["claude-code", "openai-codex"]
            .map(|h| (h.to_string(), h.to_string()))
            .into();
        let base = prepared_base(&s0, BackendKind::Container);
        let tag = prepared_tag(&base, &have);
        f.rt.saved.lock().unwrap().push(tag.clone());
        f.pool.index_update(|v| {
            v.push(PreparedEntry {
                tag: tag.clone(),
                base,
                harnesses: have,
            })
        });
        // The same image under another network preset and a bigger size is
        // another shape, but the same saved files.
        let mut s1 = spec("two", "t");
        s1.restrict_network = true;
        s1.cpus = 4;
        assert_ne!(
            shape_key(&s0, BackendKind::Container, true),
            shape_key(&s1, BackendKind::Container, true)
        );
        let key = keep_warm_space(&f, &s1).await;
        until_ready(&f, &key, 1).await;
        assert!(
            f.host.prepared.lock().unwrap().is_empty(),
            "nothing prepared"
        );
        assert!(
            f.rt.images.lock().unwrap()[0].starts_with("local/prepared:warm-"),
            "booted from the kept filesystem"
        );
    }

    #[tokio::test]
    async fn a_filesystem_prepared_at_another_pin_is_not_reused() {
        let f = fixture(PoolSettings::default(), FakeRuntime::default());
        f.rt.can_save.store(true, SeqCst);
        let s0 = spec("one", "t");
        let have: std::collections::BTreeMap<String, String> = ["claude-code", "openai-codex"]
            .map(|h| (h.to_string(), "old-version".to_string()))
            .into();
        let base = prepared_base(&s0, BackendKind::Container);
        let tag = prepared_tag(&base, &have);
        f.rt.saved.lock().unwrap().push(tag.clone());
        f.pool.index_update(|v| {
            v.push(PreparedEntry {
                tag,
                base,
                harnesses: have,
            })
        });
        let key = keep_warm_space(&f, &s0).await;
        until_ready(&f, &key, 1).await;
        assert_eq!(
            f.host.prepared.lock().unwrap()[0],
            ["claude-code", "openai-codex"],
            "prepared again for the apps this build installs"
        );
    }

    #[tokio::test]
    async fn a_runtime_that_cannot_save_prepares_every_member() {
        let f = fixture(PoolSettings::default(), FakeRuntime::default());
        let key = keep_warm_space(&f, &spec("one", "t")).await;
        until_ready(&f, &key, 1).await;
        f.pool
            .claim(&spec("s", "t1"), BackendKind::Container, true)
            .await
            .unwrap();
        until_ready(&f, &key, 1).await;
        assert_eq!(
            f.host.prepared.lock().unwrap().len(),
            2,
            "each member prepared"
        );
        assert!(
            f.rt.images
                .lock()
                .unwrap()
                .iter()
                .all(|i| !i.starts_with("local/prepared:"))
        );
    }

    #[tokio::test]
    async fn a_restarted_daemon_finds_the_filesystem_the_last_one_saved() {
        let f = fixture(PoolSettings::default(), FakeRuntime::default());
        f.rt.can_save.store(true, SeqCst);
        // What a previous daemon saved for this image and these apps (more
        // than are wanted, and under another network preset: still reused).
        let s0 = spec("one", "t");
        let key = shape_key(&s0, BackendKind::Container, true).unwrap();
        let have: std::collections::BTreeMap<String, String> =
            ["claude-code", "openai-codex", "pi"]
                .map(|h| (h.to_string(), h.to_string()))
                .into();
        let tag = prepared_tag(&prepared_base(&s0, BackendKind::Container), &have);
        f.rt.saved.lock().unwrap().push(tag.clone());
        f.pool.index_update(|v| {
            v.push(PreparedEntry {
                tag: tag.clone(),
                base: prepared_base(&s0, BackendKind::Container),
                harnesses: have.clone(),
            })
        });
        let key2 = keep_warm_space(&f, &s0).await;
        assert_eq!(key, key2);
        until_ready(&f, &key, 1).await;
        let images = f.rt.images.lock().unwrap().clone();
        assert_eq!(images.len(), 1);
        assert!(
            images[0].starts_with("local/prepared:warm-"),
            "the first member booted from it: {images:?}"
        );
        assert!(
            f.host.prepared.lock().unwrap().is_empty(),
            "and was not prepared again"
        );
    }
}
