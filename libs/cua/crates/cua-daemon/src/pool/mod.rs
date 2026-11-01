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

/// Consecutive failed member starts after which a shape stops refilling
/// until it is used again.
const MAX_FAILURES: u32 = 3;

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

/// A ready member.
#[derive(Clone)]
struct Member {
    name: String,
    secret: String,
    footprint_mb: u64,
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
            footprint_mb: None,
            arrivals: Default::default(),
            boot_s: None,
            default_s: None,
            claim_s: None,
            decision: None,
            requested: None,
            last_used: None,
        }
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
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Record {
    name: String,
    key: String,
    backend: BackendKind,
    secret: String,
    template: StartSpec,
    #[serde(default)]
    keep: u32,
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
    pid: u32,
    /// The policy's clock.
    epoch: Instant,
    /// Ticks so far (maintenance runs every [`MAINTAIN_EVERY`]).
    ticks: std::sync::atomic::AtomicU64,
    class: policy::HostClass,
    /// [`WarmPool::reclaim`] took members back for other work: the policy
    /// starts no member of its own until then.
    held_off_until: Mutex<Option<Instant>>,
}

/// How long the policy starts no member after other work took capacity back.
const HOLD_OFF: Duration = Duration::from_secs(600);

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
    let t = template_of(spec);
    let mut ports = t.ports.clone();
    ports.sort_unstable();
    ports.dedup();
    let canonical = serde_json::json!({
        "backend": backend.as_str(),
        "image": image,
        "os": t.os,
        "arch": t.arch,
        "cpus": t.cpus,
        "memory_mb": t.memory_mb,
        "firmware": t.firmware,
        "ports": ports,
        "env": t.env,
        "probes": t.probes,
        "restrict_network": t.restrict_network,
        "labels": t.labels,
        "container_runtime": t.container_runtime,
        "command": t.command,
    });
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(canonical.to_string().as_bytes());
    Some(digest.iter().take(8).map(|b| format!("{b:02x}")).collect())
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
                pid: std::process::id(),
                epoch: Instant::now(),
                ticks: Default::default(),
                class: policy::HostClass::current(),
                held_off_until: Mutex::new(None),
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
        let key = shape_key(spec, backend, spacesd)?;
        let token = token_of(spec)?;
        let arrived = self.now_s();
        let (member, rt) = {
            let mut shapes = self.inner.shapes.lock().unwrap();
            let shape = shapes.get_mut(&key)?;
            let m = shape.members.pop()?;
            (m, shape.rt.clone())
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
            "claimed a warm Space"
        );
        if let Some(s) = self.inner.shapes.lock().unwrap().get_mut(&key) {
            s.arrivals.record(arrived);
            s.claim_s = ewma(s.claim_s, ready_s);
            s.last_used = Some(Instant::now());
        }
        self.evaluate(&key);
        self.refill_later(&key, REFILL_DELAY);
        Some(inst)
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
        }
        self.refill_later(&key, Duration::ZERO);
        Some(key)
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
        if !self.inner.refilling.lock().unwrap().insert(key.to_string()) {
            return;
        }
        struct Done<'a>(&'a Inner, String);
        impl Drop for Done<'_> {
            fn drop(&mut self) {
                self.0.refilling.lock().unwrap().remove(&self.1);
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
                if have >= s.target(auto) || s.failures >= MAX_FAILURES {
                    return;
                }
                (s.template.clone(), s.backend, s.rt.clone(), s.estimate_mb())
            };
            if !self.claims_work(backend, &rt).await {
                self.forget_shape(key);
                return;
            }
            let keep = {
                let shapes = self.inner.shapes.lock().unwrap();
                shapes.get(key).map(|s| s.keep()).unwrap_or(0)
            };
            if self.held_off() || (keep == 0 && self.inner.host.busy()) {
                tracing::debug!(
                    shape = key,
                    "warm pool: the machine's capacity is wanted elsewhere"
                );
                return;
            }
            if keep > 0 && self.inner.host.memory(backend).await.is_none() {
                self.inner.host.wake(backend).await;
            }
            if !self.memory_allows(backend, estimate).await {
                tracing::debug!(
                    shape = key,
                    "warm pool: not enough memory for another member"
                );
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
                    if s.footprint_mb.is_none() || m.footprint_mb > 0 {
                        s.footprint_mb = Some(m.footprint_mb);
                    }
                    s.members.push(m);
                }
                Err(e) => {
                    s.failures += 1;
                    tracing::warn!(shape = key, error = %e, "a warm-pool member did not start");
                }
            }
        }
    }

    /// Whether one more member estimated at `estimate_mb` fits: the pool
    /// stays within `pool.memory_percent` of the memory and above the free
    /// floor.
    async fn memory_allows(&self, backend: BackendKind, estimate_mb: u64) -> bool {
        let Some(mem) = self.inner.host.memory(backend).await else {
            return false;
        };
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
        fits(mem, pct, held, estimate_mb)
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
        let record = Record {
            name: name.clone(),
            key: key.to_string(),
            backend,
            secret: secret.clone(),
            template: template.clone(),
            keep,
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
        let ready = async {
            let inst = rt.start(&spec).await.map_err(|e| e.to_string())?;
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
        }
        .await;
        match ready {
            Ok(_) => {}
            Err(e) => {
                let _ = rt.delete(&name).await;
                self.forget_member(&name);
                self.inner.starting.lock().unwrap().remove(&name);
                return Err(e);
            }
        };
        let footprint_mb = self.inner.host.used_mb(backend, &name).await.unwrap_or(0);
        if let Some(s) = self.inner.shapes.lock().unwrap().get_mut(key) {
            s.boot_s = ewma(s.boot_s, started.elapsed().as_secs_f64());
        }
        tracing::info!(
            member = %name,
            shape = key,
            ready_ms = started.elapsed().as_millis() as u64,
            footprint_mb,
            "warm-pool member ready"
        );
        Ok(Member {
            name,
            secret,
            footprint_mb,
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
                    shapes.remove(k);
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

    /// Removes every member and starts none again (this pool is done):
    /// waits for members being started, then deletes them all.
    pub async fn drain(&self) {
        self.inner
            .closed
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let deadline = Instant::now() + MEMBER_READY_TIMEOUT + Duration::from_secs(60);
        while !self.inner.starting.lock().unwrap().is_empty()
            || !self.inner.refilling.lock().unwrap().is_empty()
        {
            if Instant::now() >= deadline {
                tracing::warn!(
                    "warm pool: members still starting at drain; the reaper removes them"
                );
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let all: Vec<(Arc<dyn VmmRuntime>, Member)> = {
            let mut shapes = self.inner.shapes.lock().unwrap();
            shapes
                .drain()
                .flat_map(|(_, s)| {
                    let rt = s.rt.clone();
                    s.members.into_iter().map(move |m| (rt.clone(), m))
                })
                .collect()
        };
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
            for m in s.members {
                self.discard(s.rt.clone(), m.name);
            }
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
        async fn memory(&self, _: BackendKind) -> Option<Memory> {
            Some(*self.memory.lock().unwrap())
        }
        async fn used_mb(&self, _: BackendKind, _: &str) -> Option<u64> {
            Some(400)
        }
        async fn wake(&self, _: BackendKind) {}
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
}
