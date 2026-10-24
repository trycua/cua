//! The elastic pool: `on="auto"` over your devices and the clouds you
//! linked, with no pools, sizes or regions to choose.
//!
//! Every placement runs one solve ([`cua_cluster::elastic`]) over:
//!
//! - **the machines you have**: this one, your devices and cloud hosts
//!   that provide Spaces (free capacity as each reports it), and cloud
//!   hosts still booting (they take Spaces that start once they joined);
//! - **what can be added**: every linked cloud's machine types
//!   ([`cua_sandbox_core::byoc::CloudManager::host_offers`]), priced, with
//!   billing steps, boot times, KVM and arch;
//! - **the limits**: the relay's machine count, your budget and $/hour cap
//!   ([`ElasticPolicy`]), each cloud's quota, and any pins.
//!
//! The plan is executed here: new hosts are added through the cloud
//! contract (one lifecycle for every cloud, `cua_byoc::hosts`), and the
//! Space is created where the plan says, waiting for a new host to join
//! when it went there. Placements are serialized, so a burst of creates
//! sees the hosts the earlier ones added (still booting) and fills them
//! before adding more. [`Spaces::elastic_reconcile`] (run by the daemon)
//! removes idle cloud hosts at their billing boundary and cordons the
//! least-used one when the others have room.
//!
//! Every add and remove is written to `<home>/elastic/ledger.jsonl`; what
//! the hosts cost this month comes from it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cua_cluster::elastic::{
    self as el, Assignment, Constraints, DemandGroup, Host, Offer, Pins, Plan, Shape, Target,
    detect::{Caller, ImageFacts, Request, UsageBook},
    scale::{Action, Arrivals, CloudHost as ScaleHost, Decision},
};
use cua_sandbox_core::byoc::{AddHost, CloudHost, HostTypeOffer};
use cua_sandbox_core::placement::{Kind, On};
use serde::{Deserialize, Serialize};

use crate::{Error, Result, SpaceCreate, Spaces};

/// The relay's machine limit per account.
pub const RELAY_MACHINE_LIMIT: u32 = 32;
/// How long a create waits for a new host to join.
pub const JOIN_WAIT: Duration = Duration::from_secs(15 * 60);
/// An offer that failed to create is skipped this long.
pub const OFFER_BACKOFF: Duration = Duration::from_secs(600);
/// CPU overcommit the pool books mostly-idle Spaces at: the hosts' own
/// container overcommit ([`cua_cluster::sysinfo::container_overcommit`]).
pub fn cpu_overcommit() -> u64 {
    u64::from(cua_cluster::sysinfo::container_overcommit())
}
/// Memory a host keeps for itself (MiB).
pub const HOST_RESERVE_MB: u64 = 1024;
/// Disk a host keeps for itself and its first image (GiB).
pub const HOST_RESERVE_GB: u64 = 15;
/// Name of this machine in plans.
pub const THIS_MACHINE: &str = crate::host_spaces::THIS_MACHINE;

/// Your caps and switches for the elastic pool (`cua cloud budget`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ElasticPolicy {
    /// Add and remove cloud hosts by itself.
    pub autoscale: bool,
    /// Most USD per hour your clouds may cost at once.
    pub max_usd_per_hour: f64,
    /// Most USD per calendar month on elastic hosts.
    pub monthly_usd: f64,
    /// Most hosts one placement adds.
    pub max_new_hosts: u32,
    /// Keep an idle host warm when recent demand predicts a Space soon.
    pub warm: bool,
    /// An interactive Space should start within this many seconds.
    pub interactive_slo_secs: u32,
    /// Spaces on your hosts are reached through their host (no relay
    /// machine each). Off: each registers its own relay machine.
    pub routed: bool,
    /// The person set a budget (`cua cloud budget --max-per-hour` or
    /// `--monthly`): Spaces created without a location may then overflow
    /// from this machine into the linked clouds (`placement.overflow`).
    #[serde(default)]
    pub budget_set: bool,
    /// Buy a KVM metal host for a small `vm` request (it costs dollars an
    /// hour). Off: a lone small VM is refused with this reason; batches of
    /// forks may still buy one.
    #[serde(default)]
    pub allow_metal: bool,
}

impl Default for ElasticPolicy {
    fn default() -> Self {
        Self {
            autoscale: true,
            max_usd_per_hour: 3.0,
            monthly_usd: 50.0,
            max_new_hosts: 10,
            warm: true,
            interactive_slo_secs: 120,
            routed: true,
            budget_set: false,
            allow_metal: false,
        }
    }
}

/// One line of the ledger.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LedgerLine {
    /// Unix seconds.
    pub at: u64,
    /// `add`, `remove`, `cordon`, `place`, `plan`.
    pub event: String,
    /// The host.
    #[serde(default)]
    pub host: String,
    /// Provider.
    #[serde(default)]
    pub provider: String,
    /// Machine type.
    #[serde(default)]
    pub type_name: String,
    /// Region.
    #[serde(default)]
    pub region: String,
    /// USD per hour.
    #[serde(default)]
    pub usd_per_hour: f64,
    /// Billing step, seconds.
    #[serde(default)]
    pub billing_step_secs: u64,
    /// Minimum billed, seconds.
    #[serde(default)]
    pub billing_min_secs: u64,
    /// Why.
    #[serde(default)]
    pub reason: String,
    /// Spaces placed (`place`).
    #[serde(default)]
    pub count: u32,
}

/// What the pool looks like for one solve.
#[derive(Clone, Debug, Default, Serialize)]
pub struct PoolView {
    /// Machines, as the solver sees them.
    pub hosts: Vec<Host>,
    /// `host name -> what create_on_host takes` (`local` for this machine).
    #[serde(skip)]
    pub ids: BTreeMap<String, String>,
    /// The cloud hosts this home added.
    pub cloud: Vec<CloudHost>,
    /// What can be added.
    pub offers: Vec<Offer>,
    /// The limits.
    pub constraints: Constraints,
    /// USD spent on elastic hosts this month (from the ledger).
    pub month_usd: f64,
}

/// A plan with what it was made from (`cua cloud plan`).
#[derive(Clone, Debug, Default, Serialize)]
pub struct ElasticPlan {
    /// The decision.
    pub plan: Plan,
    /// The demand it placed.
    pub groups: Vec<DemandGroup>,
    /// The pool it saw.
    pub pool: PoolView,
}

/// A request for Spaces through the elastic pool.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ElasticRequest {
    /// Image (empty: linux).
    #[serde(default)]
    pub image: String,
    /// `container`, `vm`, `microvm` or empty.
    #[serde(default)]
    pub kind: String,
    /// vCPUs.
    #[serde(default)]
    pub cpus: Option<u32>,
    /// MiB.
    #[serde(default)]
    pub memory_mb: Option<u64>,
    /// How many Spaces.
    #[serde(default = "one")]
    pub count: u32,
    /// Keep them on one machine (forks of one checkpoint).
    #[serde(default)]
    pub whole: bool,
    /// Who asks (`interactive`, `batch`, `compat`, `persistent_agent`).
    #[serde(default)]
    pub caller: Caller,
    /// Expected lifetime, seconds.
    #[serde(default)]
    pub duration_secs: Option<u64>,
    /// `rollout` or empty.
    #[serde(default)]
    pub profile: String,
    /// Pins (provider, region, type, host count, no scale, devices only).
    #[serde(default)]
    pub pins: Pins,
    /// One machine by name.
    #[serde(default)]
    pub pin_host: String,
    /// The one architecture it runs on (`amd64`, `arm64`; empty: the
    /// image's), for example a checkpoint's.
    #[serde(default)]
    pub arch: String,
    /// Only cloud hosts and new cloud machines (this machine is full: an
    /// overflow from a create without a location).
    #[serde(default)]
    pub clouds_only: bool,
}

fn one() -> u32 {
    1
}

/// The elastic pool's state in one Spaces runtime.
#[derive(Default)]
pub struct ElasticState {
    /// Held across a solve and its host adds.
    pub(crate) placing: tokio::sync::Mutex<()>,
    /// Creates through the pool not placed yet (this one included): the
    /// rest of a burst that is already here.
    waiting: std::sync::atomic::AtomicU32,
    inner: std::sync::Mutex<St>,
    /// This machine's warm pools ([`crate::prewarm::LocalPools`]).
    pub(crate) local_pools:
        std::sync::RwLock<Option<std::sync::Arc<dyn crate::prewarm::LocalPools>>>,
    /// One waiter per new host polls for its join; the others share it.
    joins: std::sync::Mutex<BTreeMap<String, std::sync::Arc<tokio::sync::Mutex<Option<String>>>>>,
    /// Creates placing a Space in your cloud now: each will use a live
    /// session of your plan once it is up.
    cloud_inflight: std::sync::atomic::AtomicU32,
}

/// Counts one create in [`ElasticState::waiting`] while it lives.
struct Waiting<'a>(&'a std::sync::atomic::AtomicU32);

impl<'a> Waiting<'a> {
    fn new(n: &'a std::sync::atomic::AtomicU32) -> Self {
        n.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Waiting(n)
    }
}

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// The demand group of the creates queued behind the one being placed.
const QUEUED_GROUP: &str = "queued";

/// How long the first create of a burst that must buy a machine waits for
/// the rest of the burst to arrive, so one solve sizes the machines for
/// all of it (a machine boots in a minute or more; this is noise).
const BURST_GATHER: Duration = Duration::from_millis(400);

#[derive(Default)]
struct St {
    arrivals: Arrivals,
    /// Offers whose create failed, and when.
    failed: BTreeMap<String, Instant>,
    /// Boot seconds measured per offer key.
    boot: BTreeMap<String, u32>,
    /// Places a burst's first solve gave the creates behind it.
    reservations: Vec<Reservation>,
    /// The cloud hosts as the last pool view saw them, by name.
    cloud_hosts: BTreeMap<String, CloudHost>,
    /// Creates under way on each pool host (by name): such a host is in
    /// use, never scaled in while its first Space is still on its way.
    creating: BTreeMap<String, u32>,
}

/// A place reserved for creates of one shape (the rest of a burst).
struct Reservation {
    key: String,
    target: Placed,
    /// The machine, as errors and exclusions name it.
    label: String,
    left: u32,
    at: Instant,
}

/// How long a reserved place waits for its create.
const RESERVATION_TTL: Duration = Duration::from_secs(300);

/// The shape and pins that make creates interchangeable for a reservation.
fn reservation_key(req: &ElasticRequest, opts: &SpaceCreate) -> String {
    format!(
        "{}|{}|{:?}|{:?}|{:?}|{}",
        req.image, req.kind, opts.cpus, opts.memory_mb, req.pins, req.pin_host
    )
}

/// What runs Spaces on this machine: (a container engine, `/dev/kvm`).
fn local_runtimes() -> (bool, bool) {
    let on_path = |name: &str| {
        std::env::var_os("PATH")
            .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(name).is_file()))
    };
    let containers = on_path("docker")
        || on_path("podman")
        || std::path::Path::new("/var/run/docker.sock").exists();
    let kvm = std::path::Path::new("/dev/kvm").exists();
    (containers, kvm)
}

/// The kinds this machine takes in the pool: a Mac runs both itself (its
/// built-in runtime VM); Linux what its engines run (a cloud box that only
/// runs the client, with no container engine and no KVM, takes none).
/// `CUA_ELASTIC_THIS_MACHINE=off` leaves it out, `on` keeps both.
fn this_machine_kinds(
    os: &str,
    setting: Option<&str>,
    (containers, kvm): (bool, bool),
) -> Vec<String> {
    match setting.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        Some("off" | "0" | "false" | "no") => return vec![],
        Some("on" | "1" | "true" | "yes") => return vec!["container".into(), "vm".into()],
        _ => {}
    }
    if os != "linux" {
        return vec!["container".into(), "vm".into()];
    }
    let mut k = Vec::new();
    if containers {
        k.push("container".to_string());
    }
    if kvm {
        k.push("vm".to_string());
    }
    k
}

/// [`detect`] for another module (prewarm books the size a create would).
pub(crate) fn detect_for(r: &ElasticRequest, book: &UsageBook) -> DemandGroup {
    detect(r, book, "request-0".into())
}

/// The image reference a create of `image` uses (aliases resolved).
pub(crate) fn canonical_image_of(image: &str) -> String {
    if image.is_empty() {
        default_linux_image()
    } else {
        cua_image::canonical::alias(image).unwrap_or_else(|| image.to_string())
    }
}

/// Where an assignment of a plan goes, for another module.
pub(crate) enum PlacedTarget {
    /// A running machine (`id`: `local` or its relay machine).
    Host { name: String, id: String },
    /// A host just added (wait for it to join).
    NewHost(CloudHost),
    /// Nowhere a pool can be kept, and why.
    Other(String),
}

pub(crate) fn resolve_for(
    a: &Assignment,
    plan: &Plan,
    added: &[Option<CloudHost>],
    pool: &PoolView,
) -> PlacedTarget {
    match resolve_target(a, plan, added, pool) {
        Placed::Host { name, id } => PlacedTarget::Host { name, id },
        Placed::NewHost { host } => PlacedTarget::NewHost(host),
        Placed::Provider { provider, .. } => PlacedTarget::Other(format!(
            "{provider} runs each Space itself and keeps none ready"
        )),
        Placed::Missing(why) => PlacedTarget::Other(why),
    }
}

/// How a Space on cloud host `h` says where it runs: "Hetzner pool ·
/// hetzner-2 (cpx11, hil) · $0.0338/h".
pub fn cloud_place_line(h: &CloudHost) -> String {
    let title = match h.provider.as_str() {
        "aws" => "AWS",
        "gcp" => "Google Cloud",
        "hetzner" => "Hetzner",
        "modal" => "Modal",
        other => other,
    };
    format!(
        "{title} pool · {} ({}, {}) · ${:.4}/h",
        h.name, h.type_name, h.region, h.usd_per_hour
    )
}

/// The name of the machine a place is on (a new host's own name once it
/// was added), else `fallback`.
fn label_for(t: &Placed, fallback: String) -> String {
    match t {
        Placed::Host { name, .. } => name.clone(),
        Placed::NewHost { host } => host.name.clone(),
        _ => fallback,
    }
}

/// How errors and exclusions name an assignment's machine.
fn label_of(a: &Assignment, plan: &Plan) -> String {
    match &a.target {
        Target::Host { name } => name.clone(),
        Target::New { index } => plan
            .new_hosts
            .get(*index)
            .map(|n| format!("new {}", n.type_name))
            .unwrap_or_default(),
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn dir(home: &Path) -> PathBuf {
    home.join("elastic")
}

/// Reads the policy of `home` (defaults when there is none).
pub fn load_policy(home: &Path) -> ElasticPolicy {
    std::fs::read(dir(home).join("policy.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Writes the policy of `home`.
pub fn save_policy(home: &Path, p: &ElasticPolicy) -> Result<()> {
    let d = dir(home);
    std::fs::create_dir_all(&d)?;
    let tmp = d.join("policy.json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(p)?)?;
    std::fs::rename(tmp, d.join("policy.json"))?;
    Ok(())
}

/// Appends to the ledger of `home`.
pub fn ledger_append(home: &Path, line: &LedgerLine) {
    use std::io::Write;
    let d = dir(home);
    let _ = std::fs::create_dir_all(&d);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(d.join("ledger.jsonl"))
        && let Ok(s) = serde_json::to_string(line)
    {
        let _ = writeln!(f, "{s}");
    }
}

/// The ledger of `home`.
pub fn ledger(home: &Path) -> Vec<LedgerLine> {
    std::fs::read_to_string(dir(home).join("ledger.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// USD the elastic hosts cost since `since` (unix seconds), billed by
/// their steps, from the ledger: hosts still running are counted to now.
pub fn spent_since(lines: &[LedgerLine], since: u64, at: u64) -> f64 {
    let mut open: BTreeMap<&str, &LedgerLine> = BTreeMap::new();
    let mut total = 0.0;
    let billed = |l: &LedgerLine, end: u64| {
        let ran = end.saturating_sub(l.at.max(since));
        let step = l.billing_step_secs.max(1);
        let secs = ran.max(if l.at >= since { l.billing_min_secs } else { 0 });
        l.usd_per_hour * (secs.div_ceil(step) * step) as f64 / 3600.0
    };
    for l in lines {
        match l.event.as_str() {
            "add" => {
                open.insert(&l.host, l);
            }
            "remove" => {
                if let Some(a) = open.remove(l.host.as_str())
                    && l.at >= since
                {
                    total += billed(a, l.at);
                }
            }
            _ => {}
        }
    }
    for a in open.values() {
        total += billed(a, at);
    }
    total
}

/// The first second of this calendar month (UTC).
pub fn month_start(at: u64) -> u64 {
    // Days since the epoch to a civil date (Howard Hinnant's algorithm).
    let days = (at / 86_400) as i64;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    (at - (d as u64 - 1) * 86_400) / 86_400 * 86_400
}

/// The canonical Linux image (what a create that names none runs).
fn default_linux_image() -> String {
    cua_image::canonical::canonical_image_or_linux("linux")
}

/// The guest OS of `image`: the catalog's word, else `windows` for a name
/// that says so, else `linux`.
fn guest_os(image: &str) -> String {
    if let Some(os) = cua_image::catalog::catalog_os(image).filter(|o| !o.is_empty()) {
        return os;
    }
    if image.to_ascii_lowercase().contains("windows") {
        "windows".into()
    } else {
        "linux".into()
    }
}

/// What the image catalog says about `image`, without the network.
pub fn image_facts(image: &str, profile: &str) -> ImageFacts {
    let word = image.trim();
    let resolved = cua_image::canonical::alias(word).unwrap_or_else(|| word.to_string());
    let os = guest_os(&resolved);
    let lower = resolved.to_ascii_lowercase();
    let disk_only = lower.ends_with("-disk") || os == "windows" || os == "macos";
    let slim = lower.contains("slim") || profile == "rollout";
    ImageFacts {
        os: if os.is_empty() {
            "linux".into()
        } else {
            os.clone()
        },
        arches: if os == "windows" {
            vec!["amd64".into()]
        } else if os == "macos" {
            vec!["arm64".into()]
        } else {
            vec![]
        },
        rootfs: !disk_only,
        disk: true,
        desktop: !slim,
        size_gb: match os.as_str() {
            "windows" => 25,
            "macos" => 40,
            _ if lower.contains("docker") => 6,
            _ if slim => 2,
            _ => 5,
        },
        private: cua_cluster::placement::image_is_private(word),
    }
}

/// A Space create as an elastic request: `on` naming a cloud, its
/// `instance_type` and `region` are pins of the solve.
pub fn request_of(opts: &SpaceCreate) -> ElasticRequest {
    let provider = match &opts.on {
        Some(On::Provider(p)) => Some(p.clone()),
        _ => None,
    };
    ElasticRequest {
        image: opts.image.clone().unwrap_or_default(),
        kind: match opts.kind {
            Kind::Auto => String::new(),
            k => k.as_str().to_string(),
        },
        cpus: opts.cpus,
        memory_mb: opts.memory_mb,
        count: 1,
        pins: Pins {
            // A cloud named: only machines there (not your devices).
            devices_only: false,
            provider,
            region: opts.region.clone().filter(|r| !r.is_empty()),
            type_name: opts.instance_type.clone().filter(|t| !t.is_empty()),
            ..Default::default()
        },
        ..Default::default()
    }
}

fn detect(r: &ElasticRequest, book: &UsageBook, id: String) -> DemandGroup {
    let mut facts = image_facts(&r.image, &r.profile);
    if !r.arch.is_empty() {
        facts.arches = vec![r.arch.clone()];
    }
    let mut g = el::detect::detect(
        &Request {
            id,
            image: if r.image.is_empty() {
                default_linux_image()
            } else {
                r.image.clone()
            },
            facts,
            kind: r.kind.clone(),
            cpus: r.cpus,
            memory_mb: r.memory_mb,
            profile: r.profile.clone(),
            count: r.count.max(1),
            whole: r.whole,
            caller: r.caller,
            duration_secs: r.duration_secs,
            latency: None,
            holders: vec![],
            sync_secs: 0,
            pin_host: r.pin_host.clone(),
            gpu: String::new(),
        },
        book,
    );
    g.pin_host = r.pin_host.clone();
    g
}

/// The architecture of a Linux host that restores checkpoints of `abi`
/// (`qemu/kvm/aarch64` -> `arm64`); `None` for one only a Mac restores.
pub fn forks_arch(abi: &str) -> Option<&'static str> {
    match abi.strip_prefix("qemu/kvm/")? {
        "aarch64" | "arm64" => Some("arm64"),
        "x86_64" | "amd64" => Some("amd64"),
        _ => None,
    }
}

/// An offer row as the solver sees it.
pub fn offer_of(o: &HostTypeOffer, boot: Option<u32>) -> Offer {
    let one_space = o.supplies == cua_sandbox_core::byoc::supplies::SPACES;
    let capacity = if one_space {
        Shape::new(u64::from(o.vcpus) * 1000, o.memory_mb, 64)
    } else {
        Shape::new(
            u64::from(o.vcpus) * 1000 * cpu_overcommit(),
            o.memory_mb.saturating_sub(HOST_RESERVE_MB),
            u64::from(if o.disk_gb == 0 { 60 } else { o.disk_gb }).saturating_sub(HOST_RESERVE_GB),
        )
    };
    Offer {
        provider: o.provider.clone(),
        type_name: o.type_name.clone(),
        region: o.region.clone(),
        arch: o.arch.clone(),
        capacity,
        kvm: o.kvm,
        kinds: o.kinds.clone(),
        gpu: o.gpu.clone(),
        usd_per_hour: o.usd_per_hour,
        billing_step_secs: o.billing_step_secs.max(1),
        billing_min_secs: o.billing_min_secs,
        boot_secs: boot.unwrap_or(o.boot_secs),
        one_space_each: one_space,
        max_new: o.max_new,
        rtt_ms: region_rtt(&o.provider, &o.region),
        vcpus: o.vcpus,
        quota: o.quota.clone(),
    }
}

/// Round trip from this machine to a cloud region: measured when a host
/// there reported one (`<home>/elastic/rtt.json`), else estimated from
/// geography ([`el::geo`]); 0 when unknown.
pub fn region_rtt(provider: &str, region: &str) -> u32 {
    el::geo::rtt_to(provider, region).unwrap_or(0)
}

/// Learned quota limits (`hetzner/shared` -> vCPUs) and when they were
/// learned: a refused create teaches the pool's limit for a while.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LearnedQuotas {
    /// pool -> (vCPUs, unix seconds learned).
    pub pools: BTreeMap<String, (u32, u64)>,
}

/// How long a learned limit holds (a raised limit is found again after).
pub const LEARNED_QUOTA_SECS: u64 = 6 * 3600;

fn load_quotas(home: &Path) -> LearnedQuotas {
    std::fs::read(dir(home).join("quotas.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save_quotas(home: &Path, q: &LearnedQuotas) {
    let d = dir(home);
    let _ = std::fs::create_dir_all(&d);
    if let Ok(b) = serde_json::to_vec_pretty(q) {
        let _ = std::fs::write(d.join("quotas.json"), b);
    }
}

/// vCPUs left per quota pool: the cloud's limit (or a learned one) minus
/// what this home's hosts in that pool use.
pub fn quota_left(
    rows: &[HostTypeOffer],
    cloud: &[CloudHost],
    learned: &LearnedQuotas,
    now: u64,
) -> BTreeMap<String, u32> {
    let mut limits: BTreeMap<String, u32> = BTreeMap::new();
    for r in rows.iter().filter(|r| !r.quota.is_empty()) {
        if r.quota_vcpus > 0 {
            limits.insert(r.quota.clone(), r.quota_vcpus);
        } else if let Some((v, at)) = learned.pools.get(&r.quota)
            && now.saturating_sub(*at) < LEARNED_QUOTA_SECS
        {
            limits.insert(r.quota.clone(), *v);
        }
    }
    let mut used: BTreeMap<String, u32> = BTreeMap::new();
    for c in cloud.iter().filter(|c| c.state != "gone") {
        if let Some(r) = rows
            .iter()
            .find(|r| r.provider == c.provider && r.type_name == c.type_name)
            && !r.quota.is_empty()
        {
            *used.entry(r.quota.clone()).or_default() += r.vcpus;
        }
    }
    limits
        .into_iter()
        .map(|(pool, lim)| {
            let u = used.get(&pool).copied().unwrap_or(0);
            (pool, lim.saturating_sub(u))
        })
        .collect()
}

/// Whether a create error says the cloud allows no more machines (or
/// public addresses, one per machine): Hetzner's primary IP and server
/// limits, AWS's address limit.
pub fn is_machine_limit_error(e: &str) -> bool {
    let e = e.to_ascii_lowercase();
    e.contains("primary ip limit")
        || e.contains("ip limit exceeded")
        || e.contains("server limit exceeded")
        || e.contains("addresslimitexceeded")
        || e.contains("instancelimitexceeded")
}

/// The learned-limit key for a provider's machine count.
fn machines_pool(provider: &str) -> String {
    format!("{provider}/machines")
}

/// Machines a provider still allows when a refused create taught its
/// limit (`None`: no limit learned): the limit minus this home's hosts
/// there now.
pub fn machines_left(
    provider: &str,
    cloud: &[CloudHost],
    learned: &LearnedQuotas,
    now: u64,
) -> Option<u32> {
    let (lim, at) = learned.pools.get(&machines_pool(provider))?;
    if now.saturating_sub(*at) >= LEARNED_QUOTA_SECS {
        return None;
    }
    let live = cloud
        .iter()
        .filter(|c| c.provider == provider && c.state != "gone")
        .count() as u32;
    Some(lim.saturating_sub(live))
}

/// Whether a create error says a cloud limit was hit.
pub fn is_quota_error(e: &str) -> bool {
    let e = e.to_ascii_lowercase();
    e.contains("limit_exceeded")
        || e.contains("limit exceeded")
        || e.contains("core limit")
        || e.contains("vcpulimitexceeded")
        || e.contains("quota")
}

impl Spaces {
    /// The elastic pool policy of this home.
    pub fn elastic_policy(&self) -> ElasticPolicy {
        load_policy(self.home_dir())
    }

    /// Sets the elastic pool policy of this home.
    pub fn set_elastic_policy(&self, p: &ElasticPolicy) -> Result<()> {
        save_policy(self.home_dir(), p)
    }

    /// The ledger of elastic hosts.
    pub fn elastic_ledger(&self) -> Vec<LedgerLine> {
        ledger(self.home_dir())
    }

    pub(crate) fn usage_book(&self) -> UsageBook {
        std::fs::read(dir(self.home_dir()).join("usage.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// Records what a finished Space of `image` used, for the next sizes.
    pub fn elastic_record_usage(
        &self,
        image: &str,
        millicpus: Option<u64>,
        memory_mb: u64,
        lifetime_secs: u64,
    ) {
        let mut book = self.usage_book();
        book.record(image, millicpus, memory_mb, lifetime_secs);
        let d = dir(self.home_dir());
        let _ = std::fs::create_dir_all(&d);
        if let Ok(b) = serde_json::to_vec(&book) {
            let _ = std::fs::write(d.join("usage.json"), b);
        }
    }

    /// The pool as one solve sees it.
    pub async fn elastic_view(&self) -> Result<PoolView> {
        let policy = self.elastic_policy();
        let t = now();
        let clouds = self.sandboxes().clouds().ok().cloned();
        let cloud: Vec<CloudHost> = match &clouds {
            Some(c) => c.hosts().await.unwrap_or_default(),
            None => vec![],
        };
        let offer_rows: Vec<HostTypeOffer> = match &clouds {
            Some(c) => c.host_offers().await.unwrap_or_default(),
            None => vec![],
        };
        let mut view = PoolView::default();
        // This machine.
        // What the Spaces here reserve (and creates in flight) is taken out,
        // not the load average: a busy machine still takes what fits.
        let reservations = self.local_reservations().await;
        let (mut this, _) = cua_cluster::sysinfo::this_machine_status_reserving(&reservations, 1.0);
        let kinds = this_machine_kinds(
            std::env::consts::OS,
            std::env::var("CUA_ELASTIC_THIS_MACHINE").ok().as_deref(),
            local_runtimes(),
        );
        this.kinds = kinds.clone();
        if !kinds.is_empty()
            && cua_sandbox_core::placement::validate(&On::Local, Kind::Auto, &Default::default())
                .is_ok()
        {
            view.hosts
                .push(host_of(THIS_MACHINE, std::env::consts::OS, &this, None, t));
            view.ids.insert(THIS_MACHINE.into(), "local".into());
        }
        // Your machines that provide Spaces (devices and joined hosts).
        for h in self.hosts().await.unwrap_or_default() {
            if !h.online {
                continue;
            }
            let Some(status) = h.node.as_ref().map(crate::host_spaces::HostCapacity::to_pb) else {
                continue;
            };
            let ch = cloud
                .iter()
                .find(|c| !c.machine.is_empty() && c.machine == h.id);
            let mut host = host_of(&h.name, &h.os, &status, ch, t);
            if ch.is_some_and(|c| c.state == "cordoned") {
                host.ready = false;
            }
            // At its own Space limit (`cua host config --max-spaces`): it
            // takes none now, whatever its CPU and memory say.
            if h.limits
                .iter()
                .any(|l| l.resource == "spaces" && l.limit > 0 && l.used >= l.limit)
            {
                host.ready = false;
            }
            view.ids.insert(host.name.clone(), h.id.clone());
            view.hosts.push(host);
        }
        // Cloud hosts still on their way.
        let st = self.inner.elastic.inner.lock().unwrap().boot.clone();
        for c in &cloud {
            if !matches!(c.state.as_str(), "creating" | "booting") {
                continue;
            }
            if view.hosts.iter().any(|h| h.name == c.name) {
                continue;
            }
            let Some(o) = offer_rows
                .iter()
                .find(|o| o.provider == c.provider && o.type_name == c.type_name)
            else {
                continue;
            };
            let off = offer_of(o, st.get(&key(o)).copied());
            let elapsed = t.saturating_sub(c.started) as u32;
            view.hosts.push(Host {
                name: c.name.clone(),
                provider: c.provider.clone(),
                region: c.region.clone(),
                type_name: c.type_name.clone(),
                free: off.capacity,
                total: off.capacity,
                arch: off.arch.clone(),
                os: "linux".into(),
                kinds: off.kinds.clone(),
                kvm: off.kvm,
                gpu: vec![],
                usd_per_hour: c.usd_per_hour,
                paid_secs_left: paid_left(c, t),
                billing_step_secs: c.billing_step_secs,
                running: 0,
                ready: true,
                images: vec![],
                boot_secs_left: off.boot_secs.saturating_sub(elapsed).max(10),
                rtt_ms: off.rtt_ms,
            });
            view.ids
                .insert(c.name.clone(), format!("pending:{}", c.name));
        }
        // What can be added.
        let failed = self.inner.elastic.inner.lock().unwrap().failed.clone();
        view.offers = usable_offers(&offer_rows, |o| {
            failed
                .get(&key(o))
                .is_some_and(|at| at.elapsed() <= OFFER_BACKOFF)
        })
        .into_iter()
        .map(|o| offer_of(o, st.get(&key(o)).copied()))
        .collect();
        let learned = load_quotas(self.home_dir());
        for o in view.offers.iter_mut() {
            if let Some(left) = machines_left(&o.provider, &cloud, &learned, t) {
                o.max_new = o.max_new.min(left);
            }
        }
        let running_usd: f64 = cloud
            .iter()
            .filter(|c| c.state != "gone")
            .map(|c| c.usd_per_hour)
            .sum::<f64>()
            + 0.0;
        let lines = self.elastic_ledger();
        view.month_usd = spent_since(&lines, month_start(t), t);
        let relay_used = match self.relay_machines().await {
            Ok(m) => m
                .iter()
                .filter(|m| m.role == "owner" || m.role.is_empty())
                .count() as u32,
            Err(_) => 0,
        };
        view.constraints = Constraints {
            relay_limit: RELAY_MACHINE_LIMIT,
            relay_used,
            space_takes_relay: !policy.routed,
            max_new_hosts: policy.max_new_hosts,
            max_usd_per_hour: Some(policy.max_usd_per_hour),
            budget_left_usd: Some((policy.monthly_usd - view.month_usd).max(0.0)),
            running_usd_per_hour: running_usd,
            interactive_slo_secs: policy.interactive_slo_secs,
            pins: Pins {
                no_scale: !policy.autoscale,
                ..Default::default()
            },
            time_budget_ms: 200,
            quota_left: quota_left(&offer_rows, &cloud, &learned, t),
        };
        self.inner.elastic.inner.lock().unwrap().cloud_hosts =
            cloud.iter().map(|c| (c.name.clone(), c.clone())).collect();
        view.cloud = cloud;
        Ok(view)
    }

    /// What your plan leaves of live sessions for Spaces in your cloud
    /// ([`SessionRoom`]); `None` when no limit applies or the relay cannot
    /// be asked (then nothing is refused here: the relay still refuses a
    /// session over the cap).
    pub async fn cloud_session_room(&self) -> Option<SessionRoom> {
        let relay = self.relay().ok()?;
        let token = relay.tokens.access_token().await.ok()?;
        let client = relay.client().await.ok()?;
        let usage = client.usage(&token).await.ok()??;
        let here = self
            .registry()
            .list()
            .map(|all| all.iter().filter(|s| !s.cloud.is_empty()).count() as u32)
            .unwrap_or(0);
        let inflight = self
            .inner
            .elastic
            .cloud_inflight
            .load(std::sync::atomic::Ordering::SeqCst);
        SessionRoom::of(&usage, here, inflight)
    }

    /// Whether any linked cloud supplies the elastic pool (machines to add,
    /// or Spaces priced per Space).
    pub(crate) async fn elastic_has_clouds(&self) -> bool {
        let Ok(c) = self.sandboxes().clouds() else {
            return false;
        };
        c.host_offers().await.is_ok_and(|o| !o.is_empty())
    }

    /// Whether `provider` is a linked cloud that supplies the elastic pool.
    pub(crate) async fn elastic_supplier(&self, provider: &str) -> bool {
        let Ok(c) = self.sandboxes().clouds() else {
            return false;
        };
        c.host_offers()
            .await
            .is_ok_and(|o| o.iter().any(|o| o.provider == provider))
    }

    /// The plan for `requests` (nothing is changed: `cua cloud plan`).
    pub async fn elastic_plan(&self, requests: &[ElasticRequest]) -> Result<ElasticPlan> {
        self.elastic_plan_expecting(requests, None).await
    }

    /// [`Spaces::elastic_plan`]; with `expect` (the creates queued behind
    /// this one), the rest of a burst (or the steady load) is solved with it
    /// as soft demand, so the machines bought now are sized for what is
    /// coming.
    async fn elastic_plan_expecting(
        &self,
        requests: &[ElasticRequest],
        expect: Option<u32>,
    ) -> Result<ElasticPlan> {
        let mut pool = self.elastic_view().await?;
        let book = self.usage_book();
        let allow_metal = self.elastic_policy().allow_metal;
        let groups: Vec<DemandGroup> = requests
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let mut g = detect(r, &book, format!("request-{i}"));
                g.req.allow_metal = allow_metal;
                g
            })
            .collect();
        // An overflow: your devices are not where it goes.
        if requests.first().is_some_and(|r| r.clouds_only) {
            pool.hosts.retain(|h| !h.provider.is_empty());
        }
        // Pins of the request are constraints of the same solve: a pinned
        // cloud, region or type also narrows the machines already running.
        if let Some(r) = requests.first() {
            merge_pins(&mut pool.constraints.pins, &r.pins);
            let p = &pool.constraints.pins;
            if p.provider.is_some() || p.region.is_some() || p.type_name.is_some() {
                pool.hosts.retain(|h| {
                    p.provider.as_ref().is_none_or(|x| &h.provider == x)
                        && p.region.as_ref().is_none_or(|x| &h.region == x)
                        && p.type_name.as_ref().is_none_or(|x| &h.type_name == x)
                });
            }
        }
        // Ready Spaces a prewarm keeps of this shape: a create claims one,
        // so the room they hold is this request's.
        if let (Some(r), Some(g)) = (requests.first(), groups.first()) {
            let image = canonical_image_of(&r.image);
            let cpus = g.per.millicpus.div_ceil(1000).max(1) as u32;
            let held = crate::prewarm::held(
                &crate::prewarm::load(self.home_dir()),
                now(),
                Some((&image, &r.kind, cpus, g.per.memory_mb)),
            );
            for h in pool.hosts.iter_mut() {
                if let Some(n) = held.get(&h.name) {
                    let back = g.per.times(u64::from(*n));
                    h.free = Shape::new(
                        (h.free.millicpus + back.millicpus).min(h.total.millicpus),
                        (h.free.memory_mb + back.memory_mb).min(h.total.memory_mb),
                        h.free.disk_gb,
                    );
                }
            }
        }
        let mut solved = groups.clone();
        if let Some(queued) = expect {
            let t = now();
            let arrivals = self.inner.elastic.inner.lock().unwrap().arrivals.clone();
            let window = pool
                .offers
                .iter()
                .map(|o| u64::from(o.boot_secs))
                .max()
                .unwrap_or(60);
            let running: u32 = pool.hosts.iter().map(|h| h.running).sum();
            for g in &groups {
                // Creates already queued behind this one are real demand
                // of the same burst: placed in this solve (each places
                // itself after, on what this one bought).
                if queued > 0 {
                    let mut q = g.clone();
                    q.id = QUEUED_GROUP.into();
                    q.count = queued;
                    solved.push(q);
                }
                let more = el::scale::expected_more(
                    &arrivals,
                    t,
                    window,
                    g.duration_secs,
                    running,
                    g.count,
                )
                .saturating_sub(queued);
                solved.extend(el::solve::anticipate(g, more, &pool.offers));
            }
        }
        let plan = el::solve(&solved, &pool.hosts, &pool.offers, &pool.constraints);
        Ok(ElasticPlan { plan, groups, pool })
    }

    /// Adds machines for the pool's jobs that have waited for one
    /// (`cua run --on auto`, compat runs): the coordinator places them once
    /// the machines joined. Runs in the daemon's reconcile loop, on the
    /// device whose clouds are linked.
    pub async fn elastic_provision_pending(&self) -> Result<Vec<CloudHost>> {
        let Some(pool) = self.inner.pool.read().unwrap().clone() else {
            return Ok(vec![]);
        };
        if !self.elastic_policy().autoscale || self.sandboxes().clouds().is_err() {
            return Ok(vec![]);
        }
        let (status, v) = pool
            .request("pending", serde_json::json!({ "min_age_secs": 15 }))
            .await;
        if status != 200 {
            return Ok(vec![]);
        }
        let requests: Vec<ElasticRequest> = v["pending"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|p| ElasticRequest {
                image: p["image"].as_str().unwrap_or_default().into(),
                kind: p["kind"].as_str().unwrap_or_default().into(),
                cpus: p["millicpus"]
                    .as_u64()
                    .filter(|m| *m > 0)
                    .map(|m| m.div_ceil(1000) as u32),
                memory_mb: p["memory_mb"].as_u64().filter(|m| *m > 0),
                count: p["count"].as_u64().unwrap_or(1) as u32,
                caller: Caller::Batch,
                duration_secs: p["timeout_secs"].as_u64().filter(|t| *t > 0),
                // `--on <cloud>`: only that cloud's machines.
                pins: Pins {
                    provider: p["provider"]
                        .as_str()
                        .filter(|c| !c.is_empty())
                        .map(String::from),
                    ..Default::default()
                },
                ..Default::default()
            })
            .collect();
        if requests.is_empty() {
            return Ok(vec![]);
        }
        let _g = self.inner.elastic.placing.lock().await;
        // A solve takes the pins of its first request: one per cloud.
        let mut by_cloud: BTreeMap<Option<String>, Vec<ElasticRequest>> = BTreeMap::new();
        for r in requests {
            by_cloud.entry(r.pins.provider.clone()).or_default().push(r);
        }
        let mut added = Vec::new();
        for group in by_cloud.into_values() {
            let p = self.elastic_plan(&group).await?;
            if p.plan.new_hosts.is_empty() {
                continue;
            }
            // Jobs in your cloud each use a live session: no machine is
            // bought for more than the plan allows (they keep waiting).
            let in_cloud: u32 = p
                .plan
                .assignments
                .iter()
                .filter(|a| lands_in_cloud(a, &p.pool))
                .map(|a| a.count)
                .sum();
            if let Some(room) = self.cloud_session_room().await
                && in_cloud > room.left
            {
                tracing::warn!("elastic pool (waiting jobs): {}", room.refusal(in_cloud));
                continue;
            }
            added.extend(self.elastic_apply(&p.plan).await?.into_iter().flatten());
        }
        Ok(added)
    }

    /// Adds the machines a batch of forks needs when your machines have no
    /// room for it (`fork_batch` with `on="auto"`), and waits until they
    /// joined; returns their relay machine ids. Each entry of `worlds` is
    /// (checkpoint, its ABI, forks, vCPUs and MiB per fork): only checkpoints
    /// a Linux host restores (`qemu/kvm/<arch>`) can grow the pool, and
    /// each group stays whole on one machine (its forks share pages).
    /// `clouds_only`: the overflow of a batch with no location (cloud
    /// machines only, within the budget).
    pub(crate) async fn elastic_grow_for_forks(
        &self,
        worlds: &[(String, String, u32, u32, u64)],
        clouds_only: bool,
    ) -> Result<Vec<String>> {
        if !self.elastic_policy().autoscale || self.sandboxes().clouds().is_err() {
            return Ok(vec![]);
        }
        let requests: Vec<ElasticRequest> = worlds
            .iter()
            .filter_map(|(_, abi, count, cpus, memory_mb)| {
                let arch = forks_arch(abi)?;
                Some(ElasticRequest {
                    kind: "vm".into(),
                    cpus: Some(*cpus),
                    memory_mb: Some(*memory_mb),
                    count: *count,
                    whole: true,
                    caller: Caller::Batch,
                    arch: arch.into(),
                    clouds_only,
                    ..Default::default()
                })
            })
            .collect();
        if requests.is_empty() {
            return Ok(vec![]);
        }
        let added = {
            let _g = self.inner.elastic.placing.lock().await;
            let p = self.elastic_plan(&requests).await?;
            if p.plan.new_hosts.is_empty() {
                return Ok(vec![]);
            }
            // Forks in your cloud each use a live session: no machine is
            // bought for more than the plan allows.
            let in_cloud: u32 = p
                .plan
                .assignments
                .iter()
                .filter(|a| lands_in_cloud(a, &p.pool))
                .map(|a| a.count)
                .sum();
            if let Some(room) = self.cloud_session_room().await
                && in_cloud > room.left
            {
                return Err(room.refusal(in_cloud));
            }
            self.elastic_apply(&p.plan).await?
        };
        let joins = added.iter().flatten().map(|h| self.wait_joined(h));
        let mut machines = Vec::new();
        for r in futures_util::future::join_all(joins).await {
            match r {
                Ok(m) => machines.push(m),
                Err(e) => tracing::warn!("a host added for forks did not join: {e}"),
            }
        }
        Ok(machines)
    }

    /// Adds the machines `plan` asks for; returns them in the plan's order
    /// (Spaces suppliers' entries are not machines and are skipped).
    pub async fn elastic_apply(&self, plan: &Plan) -> Result<Vec<Option<CloudHost>>> {
        self.elastic_apply_pulling(plan, &[]).await
    }

    /// [`Spaces::elastic_apply`]; new hosts pull `images` right after they
    /// joined.
    pub async fn elastic_apply_pulling(
        &self,
        plan: &Plan,
        images: &[String],
    ) -> Result<Vec<Option<CloudHost>>> {
        let prepull: Vec<String> = images.to_vec();
        let clouds = self.sandboxes().clouds()?.clone();
        let adds = plan.new_hosts.iter().map(|n| {
            let clouds = clouds.clone();
            let n = n.clone();
            let prepull = prepull.clone();
            async move {
                if n.one_space {
                    return (n, Ok(None));
                }
                let r = clouds
                    .add_host(&AddHost {
                        provider: n.provider.clone(),
                        type_name: n.type_name.clone(),
                        region: n.region.clone(),
                        prepull: prepull.clone(),
                        ..Default::default()
                    })
                    .await;
                (n, r.map(Some))
            }
        });
        let mut out = Vec::new();
        let mut first_err = None;
        for (n, r) in futures_util::future::join_all(adds).await {
            match r {
                Ok(Some(h)) => {
                    ledger_append(
                        self.home_dir(),
                        &LedgerLine {
                            at: now(),
                            event: "add".into(),
                            host: h.name.clone(),
                            provider: h.provider.clone(),
                            type_name: h.type_name.clone(),
                            region: h.region.clone(),
                            usd_per_hour: h.usd_per_hour,
                            billing_step_secs: h.billing_step_secs,
                            billing_min_secs: h.billing_min_secs,
                            reason: "placement".into(),
                            count: 0,
                        },
                    );
                    out.push(Some(h));
                }
                Ok(None) => out.push(None),
                Err(e) => {
                    let k = format!("{}/{}/{}", n.provider, n.type_name, n.region);
                    tracing::warn!(offer = k, "elastic host not added: {e}");
                    // A cloud limit: learn it (what this home's hosts in
                    // that pool use now is all it allows) for a while.
                    if is_machine_limit_error(&e.to_string()) {
                        let cloud = clouds.hosts().await.unwrap_or_default();
                        let live = cloud
                            .iter()
                            .filter(|c| c.provider == n.provider && c.state != "gone")
                            .count() as u32;
                        let mut q = load_quotas(self.home_dir());
                        q.pools.insert(machines_pool(&n.provider), (live, now()));
                        save_quotas(self.home_dir(), &q);
                        tracing::info!(
                            provider = n.provider,
                            machines = live,
                            "learned a cloud machine limit"
                        );
                    } else if is_quota_error(&e.to_string())
                        && let Ok(rows) = clouds.host_offers().await
                        && let Some(row) = rows
                            .iter()
                            .find(|r| r.provider == n.provider && r.type_name == n.type_name)
                            .filter(|r| !r.quota.is_empty())
                    {
                        let cloud = clouds.hosts().await.unwrap_or_default();
                        let used: u32 = cloud
                            .iter()
                            .filter(|c| c.state != "gone")
                            .filter_map(|c| {
                                rows.iter().find(|r| {
                                    r.provider == c.provider
                                        && r.type_name == c.type_name
                                        && r.quota == row.quota
                                })
                            })
                            .map(|r| r.vcpus)
                            .sum();
                        // The refused machine did not fit: the pool allows
                        // fewer than `used + its vCPUs`, not none.
                        let used = used + row.vcpus.saturating_sub(1);
                        let mut q = load_quotas(self.home_dir());
                        let limit = match q.pools.get(&row.quota) {
                            Some((v, at)) if now().saturating_sub(*at) < LEARNED_QUOTA_SECS => {
                                used.min(*v)
                            }
                            _ => used,
                        };
                        q.pools.insert(row.quota.clone(), (limit, now()));
                        save_quotas(self.home_dir(), &q);
                        let used = limit;
                        tracing::info!(
                            pool = row.quota,
                            vcpus = used,
                            "learned a cloud core limit"
                        );
                    }
                    self.inner
                        .elastic
                        .inner
                        .lock()
                        .unwrap()
                        .failed
                        .insert(k, Instant::now());
                    out.push(None);
                    first_err.get_or_insert(e);
                }
            }
        }
        if out.iter().all(Option::is_none)
            && plan.new_hosts.iter().any(|n| !n.one_space)
            && let Some(e) = first_err
        {
            return Err(Error::Sandbox(e));
        }
        Ok(out)
    }

    /// `on="auto"` through the elastic pool (see the module docs).
    pub(crate) async fn create_on_auto_elastic(
        &self,
        opts: SpaceCreate,
    ) -> Result<crate::SpaceCreated> {
        self.create_elastic(opts, false).await
    }

    /// A create without a location, when this machine is full and a linked
    /// cloud has a budget (`placement.overflow`): the pool's clouds only.
    pub(crate) async fn create_overflow(&self, opts: SpaceCreate) -> Result<crate::SpaceCreated> {
        self.create_elastic(opts, true).await
    }

    /// Whether a create without a location may overflow into the linked
    /// clouds: `placement.overflow` is on, the person set a budget, and a
    /// linked cloud supplies the pool.
    pub async fn overflow_allowed(&self) -> bool {
        let on = cua_sandbox_core::settings::Settings::load().is_ok_and(|s| s.overflow());
        on && self.elastic_policy().budget_set
            && self.sandboxes().clouds().ok().is_some()
            && match self.sandboxes().clouds() {
                Ok(c) => c.host_offers().await.is_ok_and(|o| !o.is_empty()),
                Err(_) => false,
            }
    }

    async fn create_elastic(
        &self,
        opts: SpaceCreate,
        clouds_only: bool,
    ) -> Result<crate::SpaceCreated> {
        let mut req = request_of(&opts);
        req.clouds_only = clouds_only;
        // The host reserves exactly what the pool booked: the size asked for,
        // else the size detected for this image (learned, or its default).
        let mut opts = opts;
        if opts.cpus.is_none() || opts.memory_mb.is_none() {
            let g = detect(&req, &self.usage_book(), String::new());
            opts.cpus = opts
                .cpus
                .or(Some(g.per.millicpus.div_ceil(1000).max(1) as u32));
            opts.memory_mb = opts.memory_mb.or(Some(g.per.memory_mb));
        }
        let key = reservation_key(&req, &opts);
        let mut excluded: Vec<String> = Vec::new();
        let mut errors: Vec<String> = Vec::new();
        let _waiting = Waiting::new(&self.inner.elastic.waiting);
        self.inner
            .elastic
            .inner
            .lock()
            .unwrap()
            .arrivals
            .record(1, now());
        for _attempt in 0..5 {
            // One solve and its adds at a time: a burst sees the hosts the
            // earlier creates added, and the rest of a burst takes the
            // places its first solve reserved without solving again.
            let (target, label, _in_cloud) = {
                let _g = self.inner.elastic.placing.lock().await;
                // What the plan leaves of live sessions: read under the turn,
                // so the creates of a burst count each other.
                let room = self.cloud_session_room().await;
                let (target, label) = if let Some(t) = self.take_reservation(&key, &excluded) {
                    t
                } else {
                    let mut gathered = false;
                    let (p, a) = loop {
                        let mut queued = self
                            .inner
                            .elastic
                            .waiting
                            .load(std::sync::atomic::Ordering::SeqCst)
                            .saturating_sub(1);
                        // Never buy machines for more Spaces than the plan
                        // can run: the rest of a burst is refused, not hosted.
                        if let Some(r) = &room {
                            queued = queued.min(r.left.saturating_sub(1));
                        }
                        let mut p = self
                            .elastic_plan_expecting(std::slice::from_ref(&req), Some(queued))
                            .await?;
                        if !excluded.is_empty() {
                            p.pool.hosts.retain(|h| !excluded.contains(&h.name));
                            p.plan = el::solve(
                                &p.groups,
                                &p.pool.hosts,
                                &p.pool.offers,
                                &p.pool.constraints,
                            );
                        }
                        let mine = p.groups[0].id.clone();
                        if let Some(u) = p.plan.unplaced.iter().find(|u| u.group == mine) {
                            let mut why = u.reason.clone();
                            if !errors.is_empty() {
                                why.push_str(&format!(" (tried: {})", errors.join("; ")));
                            }
                            return Err(Error::LimitExceeded(format!(
                                "no machine can take the Space: {why}"
                            )));
                        }
                        let Some(a) = p.plan.assignments.iter().find(|a| a.group == mine).cloned()
                        else {
                            return Err(Error::LimitExceeded("the plan placed nothing".into()));
                        };
                        // About to buy a machine: let the rest of the burst
                        // arrive first (once, holding the turn), then solve
                        // again for all of it.
                        if matches!(a.target, Target::New { .. }) && !gathered {
                            gathered = true;
                            tokio::time::sleep(BURST_GATHER).await;
                            continue;
                        }
                        // A Space in your cloud uses a live session: none
                        // left, nothing is bought.
                        if lands_in_cloud(&a, &p.pool)
                            && let Some(r) = room.as_ref().filter(|r| r.left == 0)
                        {
                            return Err(r.refusal(
                                self.inner
                                    .elastic
                                    .waiting
                                    .load(std::sync::atomic::Ordering::SeqCst)
                                    .max(1),
                            ));
                        }
                        break (p, a);
                    };
                    let added = match &a.target {
                        Target::New { .. } => {
                            let image = if req.image.is_empty() {
                                default_linux_image()
                            } else {
                                cua_image::canonical::alias(&req.image)
                                    .unwrap_or_else(|| req.image.clone())
                            };
                            match self.elastic_apply_pulling(&p.plan, &[image]).await {
                                Ok(a) => a,
                                // A cloud limit taught the pool what it
                                // allows: plan again with a smaller machine.
                                Err(e)
                                    if is_quota_error(&e.to_string())
                                        || is_machine_limit_error(&e.to_string()) =>
                                {
                                    errors.push(e.to_string());
                                    continue;
                                }
                                Err(e) => return Err(e),
                            }
                        }
                        Target::Host { .. } => vec![],
                    };
                    // The creates queued behind this one take the places
                    // this solve gave them.
                    for q in p
                        .plan
                        .assignments
                        .iter()
                        .filter(|a| a.group == QUEUED_GROUP)
                    {
                        let t = resolve_target(q, &p.plan, &added, &p.pool);
                        let label = label_for(&t, label_of(q, &p.plan));
                        self.reserve(&key, t, q.count, label);
                    }
                    let t = resolve_target(&a, &p.plan, &added, &p.pool);
                    let label = label_for(&t, label_of(&a, &p.plan));
                    (t, label)
                };
                let in_cloud = match &target {
                    Placed::NewHost { .. } | Placed::Provider { .. } => true,
                    Placed::Host { name, .. } => self
                        .inner
                        .elastic
                        .inner
                        .lock()
                        .unwrap()
                        .cloud_hosts
                        .contains_key(name),
                    Placed::Missing(_) => false,
                };
                if in_cloud && let Some(r) = room.as_ref().filter(|r| r.left == 0) {
                    return Err(r.refusal(
                        self.inner
                            .elastic
                            .waiting
                            .load(std::sync::atomic::Ordering::SeqCst)
                            .max(1),
                    ));
                }
                // Counted until the create ends, so the next one sees it.
                let guard =
                    in_cloud.then(|| CloudInflight::new(&self.inner.elastic.cloud_inflight));
                (target, label, guard)
            };
            let r = match target {
                Placed::Host { name, id } => {
                    self.create_on_pool_host_settling(&name, &id, &opts).await
                }
                Placed::NewHost { host } => match self.wait_joined(&host).await {
                    Ok(machine) => {
                        self.create_on_pool_host_settling(&host.name, &machine, &opts)
                            .await
                    }
                    Err(e) => Err(e),
                },
                Placed::Provider {
                    provider,
                    cpus,
                    memory_mb,
                } => {
                    // A Spaces supplier (Modal) runs the Space itself.
                    let mut o = opts.clone();
                    o.on = Some(On::Provider(provider.clone()));
                    if o.cpus.is_none() {
                        o.cpus = Some(cpus);
                    }
                    if o.memory_mb.is_none() {
                        o.memory_mb = Some(memory_mb);
                    }
                    self.create_in_cloud(&provider, o).await.map(|info| {
                        crate::SpaceCreated::Ready {
                            info,
                            reused: false,
                        }
                    })
                }
                Placed::Missing(why) => Err(Error::NotFound(why)),
            };
            match r {
                Ok(c) => return Ok(c),
                Err(e) => {
                    // A request no machine can fix is not retried, and an
                    // error every machine would give (permission, sign-in)
                    // must not buy more machines.
                    let m = e.to_string().to_ascii_lowercase();
                    if matches!(&e, Error::InvalidArgument(_) | Error::Cancelled(_))
                        || m.contains("permission denied")
                        || m.contains("unauthenticated")
                        || m.contains("sign-in")
                        || m.contains("not enrolled")
                    {
                        return Err(e);
                    }
                    tracing::info!(machine = label, "on=auto tries again elsewhere: {e}");
                    // Its reserved places go too.
                    self.drop_reservations_on(&label);
                    errors.push(format!("{label}: {e}"));
                    excluded.push(label);
                }
            }
        }
        Err(Error::LimitExceeded(format!(
            "none of your machines took the Space ({})",
            errors.join("; ")
        )))
    }

    /// Records `count` places on `target` for creates of `key` that are on
    /// their way (the rest of a burst).
    fn reserve(&self, key: &str, target: Placed, count: u32, label: String) {
        if count == 0 || matches!(target, Placed::Missing(_)) {
            return;
        }
        let mut st = self.inner.elastic.inner.lock().unwrap();
        st.reservations
            .retain(|r| r.at.elapsed() < RESERVATION_TTL && r.left > 0);
        st.reservations.push(Reservation {
            key: key.to_string(),
            target,
            label,
            left: count,
            at: Instant::now(),
        });
    }

    /// Takes one reserved place for `key` (not on an `excluded` machine).
    fn take_reservation(&self, key: &str, excluded: &[String]) -> Option<(Placed, String)> {
        let mut st = self.inner.elastic.inner.lock().unwrap();
        st.reservations
            .retain(|r| r.at.elapsed() < RESERVATION_TTL && r.left > 0);
        let r = st
            .reservations
            .iter_mut()
            .find(|r| r.key == key && !excluded.contains(&r.label))?;
        r.left -= 1;
        Some((r.target.clone(), r.label.clone()))
    }

    /// Forgets the places reserved on a machine that refused a Space.
    fn drop_reservations_on(&self, label: &str) {
        self.inner
            .elastic
            .inner
            .lock()
            .unwrap()
            .reservations
            .retain(|r| r.label != label);
    }

    /// [`Spaces::create_on_pool_host`], tried again on the same host while
    /// it only cannot be reached yet (a host that just joined answers
    /// through the relay a little after it is listed: a 502, a transport
    /// error). Buying another machine for that would only churn hosts.
    async fn create_on_pool_host_settling(
        &self,
        name: &str,
        id: &str,
        opts: &SpaceCreate,
    ) -> Result<crate::SpaceCreated> {
        let started = Instant::now();
        let mut wait = Duration::from_secs(2);
        *self
            .inner
            .elastic
            .inner
            .lock()
            .unwrap()
            .creating
            .entry(name.to_string())
            .or_default() += 1;
        let _done = CreatingGuard {
            spaces: self,
            name: name.to_string(),
        };
        loop {
            match self.create_on_pool_host(name, id, opts.clone()).await {
                Err(e) if is_transient_host_error(&e) && started.elapsed() < HOST_SETTLE => {
                    tracing::info!(host = name, "host not reachable yet, trying it again: {e}");
                    tokio::time::sleep(wait).await;
                    wait = (wait * 2).min(Duration::from_secs(20));
                }
                r => return r,
            }
        }
    }

    async fn create_on_pool_host(
        &self,
        name: &str,
        id: &str,
        opts: SpaceCreate,
    ) -> Result<crate::SpaceCreated> {
        let created = self.create_on_pool_host_inner(name, id, opts).await?;
        // A Space on a cloud host says so (lists, the CLI, the app, MCP).
        let host = self
            .inner
            .elastic
            .inner
            .lock()
            .unwrap()
            .cloud_hosts
            .get(name)
            .cloned();
        Ok(match (created, host) {
            (crate::SpaceCreated::Ready { mut info, reused }, Some(h)) => {
                self.mark_cloud_placed(&mut info, &h);
                crate::SpaceCreated::Ready { info, reused }
            }
            (c, _) => c,
        })
    }

    /// Records on Space `info` that it runs on cloud host `h` (its provider
    /// and a place line with the host's price).
    fn mark_cloud_placed(&self, info: &mut crate::SpaceInfo, h: &CloudHost) {
        let place = cloud_place_line(h);
        info.cloud = h.provider.clone();
        info.cloud_place = place.clone();
        let reg = self.registry();
        if let (Ok(Some(mut rec)), Ok(Some(cred))) = (reg.get(&info.id), reg.credential(&info.id)) {
            rec.cloud = h.provider.clone();
            rec.cloud_place = place;
            if let Err(e) = reg.upsert(rec, cred) {
                tracing::warn!(space = info.id, "cloud placement not recorded: {e}");
            }
        }
    }

    async fn create_on_pool_host_inner(
        &self,
        name: &str,
        id: &str,
        opts: SpaceCreate,
    ) -> Result<crate::SpaceCreated> {
        if id == "local" {
            let mut local = opts;
            local.on = Some(On::Local);
            return Box::pin(self.create_inner(local)).await;
        }
        let _ = name;
        // Reached through its host (no relay machine of its own) when the
        // policy says so; a host too old to route takes it the relay way.
        if self.elastic_policy().routed {
            match self.create_on_host_routed(id, opts.clone(), None).await {
                Ok(info) => {
                    return Ok(crate::SpaceCreated::Ready {
                        info,
                        reused: false,
                    });
                }
                Err(e) if e.to_string().contains("attach needs") => {
                    tracing::info!(
                        host = id,
                        "host does not route Spaces yet; using a relay machine"
                    );
                }
                Err(e) => return Err(e),
            }
        }
        self.create_on_host(id, opts)
            .await
            .map(|info| crate::SpaceCreated::Ready {
                info,
                reused: false,
            })
    }

    /// Waits until a host the pool added has joined; returns its relay
    /// machine id.
    pub(crate) async fn wait_joined_pub(&self, host: &CloudHost) -> Result<String> {
        self.wait_joined(host).await
    }

    async fn wait_joined(&self, host: &CloudHost) -> Result<String> {
        // The rest of a burst waits on the same host: one of them polls.
        let slot = self
            .inner
            .elastic
            .joins
            .lock()
            .unwrap()
            .entry(format!("{}@{}", host.name, host.started))
            .or_default()
            .clone();
        let mut joined = slot.lock().await;
        if let Some(m) = joined.as_ref() {
            return Ok(m.clone());
        }
        let m = self.poll_joined(host).await?;
        *joined = Some(m.clone());
        Ok(m)
    }

    async fn poll_joined(&self, host: &CloudHost) -> Result<String> {
        let clouds = self.sandboxes().clouds()?.clone();
        let started = Instant::now();
        cua_sandbox_core::progress::report(cua_sandbox_core::progress::Progress::phase(
            cua_sandbox_core::progress::Phase::Connecting,
        ));
        loop {
            if let Ok(hosts) = clouds.hosts().await
                && let Some(h) = hosts.iter().find(|h| h.name == host.name)
            {
                if h.state == "joined" && !h.machine.is_empty() {
                    // It must also answer as a host that provides Spaces.
                    if self
                        .hosts()
                        .await
                        .unwrap_or_default()
                        .iter()
                        .any(|o| o.id == h.machine && o.online && o.node.is_some())
                    {
                        let k = format!("{}/{}/{}", h.provider, h.type_name, h.region);
                        let boot = now().saturating_sub(h.started) as u32;
                        self.inner
                            .elastic
                            .inner
                            .lock()
                            .unwrap()
                            .boot
                            .insert(k, boot);
                        return Ok(h.machine.clone());
                    }
                }
                if h.state == "gone" {
                    return Err(Error::host(
                        "elastic host",
                        format!("{} ended before it joined", host.name),
                    ));
                }
            }
            if started.elapsed() > JOIN_WAIT {
                return Err(Error::Timeout(format!(
                    "{} did not join the pool within {} min",
                    host.name,
                    JOIN_WAIT.as_secs() / 60
                )));
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }

    /// Forgets the routing entry of a pool host's relay machine with every
    /// Space it provided here (the host was removed or is gone; its Spaces
    /// went with it). Returns the Spaces forgotten.
    pub(crate) async fn forget_routing_host(&self, machine: &str) -> Vec<String> {
        if machine.is_empty() {
            return vec![];
        }
        self.forget_host_spaces(&format!("relay:{machine}")).await
    }

    /// Whether the pool host behind relay machine `machine` is confirmed
    /// lost: its cloud says the machine is terminated or not found (the
    /// host is listed `gone`), or the relay no longer knows a machine that
    /// had joined (its lease expired). A host that is only unreachable
    /// (offline, a partition) is not lost.
    pub(crate) async fn pool_host_lost(&self, machine: &str) -> bool {
        if machine.is_empty() {
            return false;
        }
        let Ok(clouds) = self.sandboxes().clouds() else {
            return false;
        };
        let Ok(cloud) = clouds.hosts().await else {
            return false;
        };
        match cloud.iter().find(|c| c.machine == machine) {
            Some(c) => c.state == "gone",
            // Not a host of this home any more (removed, deleted with
            // `cua cloud disconnect --delete`): lost when the relay no
            // longer knows its machine either. One the relay still lists
            // (your own machine, offline) is not.
            None => self
                .relay_machines()
                .await
                .is_ok_and(|all| !all.iter().any(|m| m.id == machine)),
        }
    }

    async fn forget_stale_routing_hosts(
        &self,
        cloud: &[CloudHost],
        relay: Option<&[crate::host_spaces::HostOffer]>,
    ) {
        let Ok(direct) = self.registry().direct_hosts() else {
            return;
        };
        // The whole relay directory (offline machines too), read only when a
        // route with Spaces might have lost its machine.
        let directory = if direct
            .iter()
            .any(|h| h.space.starts_with("relay:") && !h.spaces.is_empty())
            && self.relay_account().is_some()
        {
            self.relay_machines().await.ok()
        } else {
            None
        };
        for h in direct {
            let Some(machine) = h.space.strip_prefix("relay:") else {
                continue;
            };
            let renamed = cloud
                .iter()
                .any(|c| c.name == h.name && !c.machine.is_empty() && c.machine != machine)
                && !cloud.iter().any(|c| c.machine == machine);
            let unknown = relay.is_some_and(|r| {
                h.spaces.is_empty()
                    && !r.iter().any(|o| o.id == machine)
                    && !cloud.iter().any(|c| c.machine == machine)
            });
            // A route with Spaces whose machine the relay directory no longer
            // has (its host was deleted, its lease expired): the Spaces went
            // with it.
            let vanished = !h.spaces.is_empty()
                && !cloud.iter().any(|c| c.machine == machine)
                && directory
                    .as_ref()
                    .is_some_and(|d| !d.iter().any(|m| m.id == machine));
            if renamed || unknown || vanished {
                tracing::info!(host = h.name, machine, "forgetting a gone pool host");
                self.forget_routing_host(machine).await;
            }
        }
    }

    /// One scale-in pass: removes idle cloud hosts at their billing
    /// boundary, cordons the least-used one when the others have room, and
    /// says what it did.
    pub async fn elastic_reconcile(&self) -> Result<Vec<Decision>> {
        // Prewarms past their TTL end first (on devices too).
        if let Err(e) = self.prewarm_reconcile().await {
            tracing::warn!("prewarm reconcile: {e}");
        }
        let clouds = match self.sandboxes().clouds() {
            Ok(c) => c.clone(),
            Err(_) => return Ok(vec![]),
        };
        let cloud = match clouds.hosts().await {
            Ok(c) => c,
            Err(_) => return Ok(vec![]),
        };
        self.forget_stale_routing_hosts(&cloud, None).await;
        if cloud.is_empty() {
            return Ok(vec![]);
        }
        let policy = self.elastic_policy();
        let offered = self.hosts().await.unwrap_or_default();
        if !offered.is_empty() {
            self.forget_stale_routing_hosts(&cloud, Some(&offered))
                .await;
        }
        // Hosts their cloud ended (terminated outside Cua, or their relay
        // lease expired) go at once with the Spaces they provided: those
        // Spaces are lost, and a durable one is placed again elsewhere.
        let mut cloud = cloud;
        for c in cloud.iter().filter(|c| c.state == "gone") {
            let forgotten = self.forget_routing_host(&c.machine).await;
            if !forgotten.is_empty() {
                tracing::warn!(
                    host = c.name,
                    spaces = ?forgotten,
                    "pool host lost: its Spaces are lost and forgotten"
                );
            }
            match clouds.remove_host(&c.name).await {
                Ok(()) => ledger_append(
                    self.home_dir(),
                    &LedgerLine {
                        at: now(),
                        event: "remove".into(),
                        host: c.name.clone(),
                        provider: c.provider.clone(),
                        type_name: c.type_name.clone(),
                        reason: format!(
                            "lost: its cloud ended it ({} Spaces lost)",
                            forgotten.len()
                        ),
                        ..Default::default()
                    },
                ),
                Err(e) => tracing::warn!(host = c.name, "lost host not removed: {e}"),
            }
        }
        cloud.retain(|c| c.state != "gone");
        // Hosts that never joined within `JOIN_WAIT`, and hosts past their
        // TTL that are no longer serving (a powered-off machine is still
        // billed until it is deleted): removed with their relay entry.
        let t0 = now();
        let mut ended: Vec<String> = Vec::new();
        for c in &cloud {
            let booting = matches!(c.state.as_str(), "creating" | "booting");
            let why = if booting && t0.saturating_sub(c.started) > JOIN_WAIT.as_secs() {
                format!(
                    "never joined the pool within {} min",
                    JOIN_WAIT.as_secs() / 60
                )
            } else if c.expires > 0
                && t0 >= c.expires
                && !matches!(c.state.as_str(), "joined" | "cordoned")
            {
                "past its TTL and no longer serving".to_string()
            } else {
                continue;
            };
            let forgotten = self.forget_routing_host(&c.machine).await;
            match clouds.remove_host(&c.name).await {
                Ok(()) => {
                    tracing::warn!(host = c.name, "pool host removed: {why}");
                    ledger_append(
                        self.home_dir(),
                        &LedgerLine {
                            at: now(),
                            event: "remove".into(),
                            host: c.name.clone(),
                            provider: c.provider.clone(),
                            type_name: c.type_name.clone(),
                            reason: format!("{why} ({} Spaces forgotten)", forgotten.len()),
                            ..Default::default()
                        },
                    );
                    ended.push(c.name.clone());
                }
                Err(e) => tracing::warn!(host = c.name, "host not removed ({why}): {e}"),
            }
        }
        cloud.retain(|c| !ended.contains(&c.name));
        if cloud.is_empty() {
            return Ok(vec![]);
        }
        let t = now();
        let mut idle_since = idle_since_of(&self.elastic_ledger());
        // Machines holding a live prewarm's Spaces are in use.
        let prewarmed = crate::prewarm::held(&crate::prewarm::load(self.home_dir()), t, None);
        // Hosts a create is still reaching (a host that just joined).
        let creating = self.inner.elastic.inner.lock().unwrap().creating.clone();
        let mut hosts = Vec::new();
        for c in &cloud {
            if matches!(c.state.as_str(), "creating" | "booting") {
                continue;
            }
            let node = offered
                .iter()
                .find(|o| o.id == c.machine)
                .and_then(|o| o.node.clone());
            let running: u32 = node.as_ref().map_or(0, |n| n.running.values().sum::<u32>())
                + prewarmed.get(&c.name).copied().unwrap_or(0);
            let used = node.as_ref().map_or(0.0, |n| {
                if n.memory_total_mb == 0 {
                    0.0
                } else {
                    1.0 - n.memory_free_mb as f64 / n.memory_total_mb as f64
                }
            });
            if running == 0 && !idle_since.contains_key(&c.name) {
                idle_since.insert(c.name.clone(), t);
                ledger_append(
                    self.home_dir(),
                    &LedgerLine {
                        at: t,
                        event: "idle".into(),
                        host: c.name.clone(),
                        ..Default::default()
                    },
                );
            }
            hosts.push(ScaleHost {
                name: c.name.clone(),
                provider: c.provider.clone(),
                usd_per_hour: c.usd_per_hour,
                billing_step_secs: c.billing_step_secs,
                billing_min_secs: c.billing_min_secs,
                started: c.started,
                running,
                idle_since: (running == 0).then(|| idle_since.get(&c.name).copied().unwrap_or(t)),
                used,
                slots: node
                    .as_ref()
                    .map_or(1, |n| (n.memory_total_mb / 1024).max(1) as u32),
                boot_secs: 90,
                cordoned: c.state == "cordoned",
                // A host holding a live prewarm is kept and never cordoned
                // (its ready Spaces must stay reachable).
                pinned: c.pinned
                    || prewarmed.contains_key(&c.name)
                    || creating.contains_key(&c.name),
            });
        }
        // A prewarm's host the scaler had cordoned takes Spaces again.
        for c in cloud
            .iter()
            .filter(|c| c.state == "cordoned" && prewarmed.contains_key(&c.name))
        {
            if let Err(e) = clouds.cordon_host(&c.name, false).await {
                tracing::warn!(host = c.name, "prewarm host not reopened: {e}");
            }
        }
        let arrivals = self.inner.elastic.inner.lock().unwrap().arrivals.clone();
        let decisions = el::scale::decide(
            &hosts,
            &arrivals,
            t,
            &el::scale::Policy {
                warm: policy.warm,
                ..Default::default()
            },
        );
        for d in &decisions {
            match &d.action {
                Action::Remove { reason, .. } => {
                    let c = cloud.iter().find(|c| c.name == d.host);
                    match clouds.remove_host(&d.host).await {
                        Ok(()) => {
                            if let Some(c) = c {
                                self.forget_routing_host(&c.machine).await;
                            }
                            ledger_append(
                                self.home_dir(),
                                &LedgerLine {
                                    at: now(),
                                    event: "remove".into(),
                                    host: d.host.clone(),
                                    provider: c.map(|c| c.provider.clone()).unwrap_or_default(),
                                    type_name: c.map(|c| c.type_name.clone()).unwrap_or_default(),
                                    reason: reason.clone(),
                                    ..Default::default()
                                },
                            );
                        }
                        Err(e) => tracing::warn!(host = d.host, "idle host not removed: {e}"),
                    }
                }
                Action::Cordon { reason } => {
                    if let Err(e) = clouds.cordon_host(&d.host, true).await {
                        tracing::warn!(host = d.host, "host not cordoned: {e}");
                    } else {
                        ledger_append(
                            self.home_dir(),
                            &LedgerLine {
                                at: now(),
                                event: "cordon".into(),
                                host: d.host.clone(),
                                reason: reason.clone(),
                                ..Default::default()
                            },
                        );
                    }
                }
                Action::Keep { .. } => {}
            }
        }
        // Hosts that got work again are no longer idle (written once, when
        // that changes).
        for h in hosts.iter().filter(|h| h.running > 0) {
            if idle_since.remove(&h.name).is_some() {
                ledger_append(
                    self.home_dir(),
                    &LedgerLine {
                        at: t,
                        event: "busy".into(),
                        host: h.name.clone(),
                        ..Default::default()
                    },
                );
            }
        }
        Ok(decisions)
    }
}

/// Counts a create on a pool host as under way until it ends.
struct CreatingGuard<'a> {
    spaces: &'a Spaces,
    name: String,
}

impl Drop for CreatingGuard<'_> {
    fn drop(&mut self) {
        let mut st = self.spaces.inner.elastic.inner.lock().unwrap();
        if let Some(n) = st.creating.get_mut(&self.name) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                st.creating.remove(&self.name);
            }
        }
    }
}

/// What the account's plan leaves of live sessions for Spaces in your
/// own cloud: each such Space is reached through the relay, so it uses a
/// live session once it runs (Free allows 2 at once; Pro and Teams more).
#[derive(Clone, Debug, PartialEq)]
pub struct SessionRoom {
    /// Sessions left now.
    pub left: u32,
    /// Counted as used: the relay's live sessions, or this home's cloud
    /// Spaces when more, plus cloud creates under way.
    pub used: u32,
    /// The plan's live-session limit.
    pub limit: u32,
    /// `free`, `pro`, `team`, ...
    pub plan: String,
    /// Where to get more.
    pub upgrade_url: Option<String>,
}

impl SessionRoom {
    /// From the relay's usage report and what this home counts (`here`:
    /// its cloud Spaces, `inflight`: cloud creates under way). `None`: the
    /// account is not metered or has no session limit.
    pub fn of(usage: &cua_host::relay::RelayUsage, here: u32, inflight: u32) -> Option<Self> {
        if !usage.metered {
            return None;
        }
        let limit = usage.live_sessions_limit?;
        let used = usage.live_sessions.max(here).saturating_add(inflight);
        Some(SessionRoom {
            left: limit.saturating_sub(used),
            used,
            limit,
            plan: usage.plan.clone(),
            upgrade_url: usage.upgrade_url.clone(),
        })
    }

    /// The refusal for `asked` Spaces in your cloud that the plan has no
    /// room for, before anything is bought: what the plan allows, what is
    /// in use, what this needs, and how to get more.
    pub fn refusal(&self, asked: u32) -> Error {
        let what = if asked > 1 {
            "this burst"
        } else {
            "this Space"
        };
        let upgrade = self
            .upgrade_url
            .as_deref()
            .filter(|u| !u.is_empty())
            .unwrap_or("cua.ai/pricing");
        let next = match self.plan.as_str() {
            "team" | "teams" => format!("Add seats ({upgrade})"),
            "pro" => format!("Upgrade to Teams ({upgrade})"),
            _ => format!("Upgrade to Pro or Teams ({upgrade})"),
        };
        Error::LimitExceeded(format!(
            "Your plan allows {} live sessions ({} in use); {what} needs {asked}. A Space in \
             your own cloud uses one, so no cloud machine was started. {next}, or lower --count.",
            self.limit, self.used
        ))
    }
}

/// Whether assignment `a` puts Spaces in your cloud (a new machine, or a
/// cloud host already in the pool), where each uses a live session.
pub(crate) fn lands_in_cloud(a: &Assignment, pool: &PoolView) -> bool {
    match &a.target {
        Target::New { .. } => true,
        Target::Host { name } => pool
            .hosts
            .iter()
            .any(|h| &h.name == name && !h.provider.is_empty()),
    }
}

/// Counts a create into your cloud as under way until it ends.
struct CloudInflight<'a>(&'a std::sync::atomic::AtomicU32);

impl<'a> CloudInflight<'a> {
    fn new(n: &'a std::sync::atomic::AtomicU32) -> Self {
        n.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        CloudInflight(n)
    }
}

impl Drop for CloudInflight<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// How long a create keeps trying a pool host that cannot be reached yet.
const HOST_SETTLE: Duration = Duration::from_secs(150);

/// Whether `e` says only that a host could not be reached now (the relay's
/// 502 before the host's cua-spacesd answers, a dropped connection): worth
/// trying the same host again rather than another machine.
pub(crate) fn is_transient_host_error(e: &Error) -> bool {
    if matches!(e, Error::SpacesdNotAvailable { .. }) {
        return true;
    }
    let m = e.to_string().to_ascii_lowercase();
    [
        "502",
        "bad gateway",
        "invalid compression flag",
        "transport error",
        "connection refused",
        "connection reset",
        "broken pipe",
        "not connected",
        "is offline",
        "h2 protocol error",
    ]
    .iter()
    .any(|s| m.contains(s))
}

/// Since when each host has been idle, from the ledger in order: `idle`
/// starts it, and `busy`, `add` and `remove` end it (a name can come back
/// as a new host).
fn idle_since_of(lines: &[LedgerLine]) -> BTreeMap<String, u64> {
    let mut idle: BTreeMap<String, u64> = BTreeMap::new();
    for l in lines {
        match l.event.as_str() {
            "idle" => {
                idle.entry(l.host.clone()).or_insert(l.at);
            }
            "busy" | "add" | "remove" => {
                idle.remove(&l.host);
            }
            _ => {}
        }
    }
    idle
}

fn merge_pins(into: &mut Pins, from: &Pins) {
    if from.provider.is_some() {
        into.provider = from.provider.clone();
    }
    if from.region.is_some() {
        into.region = from.region.clone();
    }
    if from.type_name.is_some() {
        into.type_name = from.type_name.clone();
    }
    if from.new_hosts.is_some() {
        into.new_hosts = from.new_hosts;
    }
    into.no_scale |= from.no_scale;
    into.devices_only |= from.devices_only;
}

/// The offers a solve may use: those that did not just fail, in the region
/// you linked; a provider's nearby region (`fallback`) only once every type
/// it offers in the linked region just failed (no capacity there), nearest
/// such region first.
fn usable_offers(
    rows: &[HostTypeOffer],
    failed: impl Fn(&HostTypeOffer) -> bool,
) -> Vec<&HostTypeOffer> {
    let ok: Vec<&HostTypeOffer> = rows.iter().filter(|o| !failed(o)).collect();
    // Per provider, the nearest region rank that still has a type.
    let mut best: BTreeMap<&str, u8> = BTreeMap::new();
    for o in &ok {
        let b = best.entry(o.provider.as_str()).or_insert(o.fallback);
        *b = (*b).min(o.fallback);
    }
    ok.into_iter()
        .filter(|o| best.get(o.provider.as_str()) == Some(&o.fallback))
        .collect()
}

fn key(o: &HostTypeOffer) -> String {
    format!("{}/{}/{}", o.provider, o.type_name, o.region)
}

fn paid_left(c: &CloudHost, t: u64) -> u64 {
    let h = ScaleHost {
        started: c.started,
        billing_step_secs: c.billing_step_secs,
        billing_min_secs: c.billing_min_secs,
        ..Default::default()
    };
    h.paid_until(t).saturating_sub(t)
}

fn host_of(
    name: &str,
    os: &str,
    s: &cua_proto::env::v1::HostNodeStatus,
    cloud: Option<&CloudHost>,
    t: u64,
) -> Host {
    let disk = 400;
    Host {
        name: name.into(),
        provider: cloud.map(|c| c.provider.clone()).unwrap_or_default(),
        region: cloud.map(|c| c.region.clone()).unwrap_or_default(),
        type_name: cloud.map(|c| c.type_name.clone()).unwrap_or_default(),
        free: Shape::new(
            u64::from(s.millicpus_free) * cpu_overcommit(),
            s.memory_free_mb,
            disk,
        ),
        total: Shape::new(
            u64::from(s.millicpus_total) * cpu_overcommit(),
            s.memory_total_mb,
            disk,
        ),
        arch: el::solve::norm_arch(&s.arch).to_string(),
        os: os.to_string(),
        kinds: s.kinds.clone(),
        kvm: s.kinds.iter().any(|k| k == "vm" || k == "microvm"),
        gpu: s.accelerators.clone(),
        usd_per_hour: cloud.map_or(0.0, |c| c.usd_per_hour),
        paid_secs_left: cloud.map_or(0, |c| paid_left(c, t)),
        billing_step_secs: cloud.map_or(1, |c| c.billing_step_secs),
        running: s.running.values().sum(),
        ready: s.problem.is_empty() && !s.draining,
        images: vec![],
        boot_secs_left: 0,
        rtt_ms: cloud.map_or(0, |c| region_rtt(&c.provider, &c.region)),
    }
}

/// Where one assignment goes, resolved.
#[derive(Clone, Debug)]
enum Placed {
    Host {
        name: String,
        id: String,
    },
    NewHost {
        host: CloudHost,
    },
    Provider {
        provider: String,
        cpus: u32,
        memory_mb: u64,
    },
    Missing(String),
}

fn resolve_target(
    a: &Assignment,
    plan: &Plan,
    added: &[Option<CloudHost>],
    pool: &PoolView,
) -> Placed {
    match &a.target {
        Target::Host { name } => match pool.ids.get(name) {
            Some(id) if id.starts_with("pending:") => {
                match pool.cloud.iter().find(|c| &c.name == name) {
                    Some(c) => Placed::NewHost { host: c.clone() },
                    None => Placed::Missing(format!("{name} is no longer recorded")),
                }
            }
            Some(id) => Placed::Host {
                name: name.clone(),
                id: id.clone(),
            },
            None => Placed::Missing(format!("{name} is not in the pool")),
        },
        Target::New { index } => {
            let n = &plan.new_hosts[*index];
            if n.one_space {
                let o = pool
                    .offers
                    .iter()
                    .find(|o| o.provider == n.provider && o.type_name == n.type_name);
                return Placed::Provider {
                    provider: n.provider.clone(),
                    cpus: o.map_or(1, |o| (o.capacity.millicpus / 1000).max(1) as u32),
                    memory_mb: o.map_or(1024, |o| o.capacity.memory_mb),
                };
            }
            match added.get(*index).cloned().flatten() {
                Some(host) => Placed::NewHost { host },
                None => Placed::Missing(format!(
                    "a new {} {} could not be added",
                    n.provider, n.type_name
                )),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Live sessions left for Spaces in your cloud: the relay's count, or
    /// this home's cloud Spaces when more, plus creates under way; the
    /// refusal says the plan's numbers and how to get more.
    #[test]
    fn a_free_plan_leaves_room_for_two_live_sessions() {
        let usage = |live: u32| cua_host::relay::RelayUsage {
            metered: true,
            plan: "free".into(),
            live_sessions: live,
            live_sessions_limit: Some(2),
            ..Default::default()
        };
        assert_eq!(SessionRoom::of(&usage(0), 0, 0).unwrap().left, 2);
        assert_eq!(SessionRoom::of(&usage(0), 1, 0).unwrap().left, 1);
        assert_eq!(SessionRoom::of(&usage(1), 0, 1).unwrap().left, 0);
        assert_eq!(SessionRoom::of(&usage(5), 0, 0).unwrap().left, 0);
        let unmetered = cua_host::relay::RelayUsage::default();
        assert!(SessionRoom::of(&unmetered, 3, 3).is_none());
        let m = SessionRoom::of(&usage(2), 0, 0)
            .unwrap()
            .refusal(16)
            .to_string();
        assert!(
            m.contains("Your plan allows 2 live sessions (2 in use); this burst needs 16."),
            "{m}"
        );
        assert!(
            m.contains("Upgrade to Pro or Teams (cua.ai/pricing), or lower --count."),
            "{m}"
        );
    }

    /// A host that just joined answers through the relay a little later
    /// (the audit's five-hosts-for-one-create churn): those errors are
    /// tried again on the same host; a refusal is not.
    #[test]
    fn a_just_joined_hosts_relay_502_is_tried_again_on_it() {
        let relay_502 = Error::host(
            "relay:host-aws-1",
            "rpc failed: protocol error: received message with invalid compression flag: 102 \
             (valid flags are 0 and 1) while receiving response with status: 502 Bad Gateway",
        );
        assert!(is_transient_host_error(&relay_502));
        assert!(is_transient_host_error(&Error::SpacesdNotAvailable {
            space: "relay:host-aws-1".into(),
            reason: "gRPC: tcp connect error (Connection refused)".into(),
        }));
        assert!(!is_transient_host_error(&Error::invalid("no such image")));
        assert!(!is_transient_host_error(&Error::LimitExceeded(
            "host is full".into()
        )));
    }

    fn line(at: u64, event: &str, host: &str, usd: f64, step: u64, min: u64) -> LedgerLine {
        LedgerLine {
            at,
            event: event.into(),
            host: host.into(),
            usd_per_hour: usd,
            billing_step_secs: step,
            billing_min_secs: min,
            ..Default::default()
        }
    }

    #[test]
    fn spend_follows_billing_steps() {
        // Hetzner: 10 minutes billed as an hour; AWS: per second, 60 s min.
        let lines = vec![
            line(1_000, "add", "hetzner-1", 0.0684, 3600, 3600),
            line(1_600, "remove", "hetzner-1", 0.0, 0, 0),
            line(1_000, "add", "aws-1", 0.58, 1, 60),
            line(1_030, "remove", "aws-1", 0.0, 0, 0),
            line(2_000, "add", "aws-2", 0.58, 1, 60),
        ];
        let got = spent_since(&lines, 0, 2_000 + 3600);
        let want = 0.0684 + 0.58 * 60.0 / 3600.0 + 0.58;
        assert!((got - want).abs() < 1e-9, "{got} vs {want}");
    }

    #[test]
    fn month_starts_at_midnight_on_the_first() {
        // 2026-10-21T08:00:00Z -> 2026-10-01T00:00:00Z.
        assert_eq!(month_start(1_792_656_000 + 8 * 3600), 1_790_812_800);
    }

    #[test]
    fn images_tell_what_they_need() {
        let f = image_facts("ghcr.io/trycua/linux:24.04-slim", "");
        assert_eq!(f.os, "linux");
        assert!(f.rootfs && !f.desktop);
        let w = image_facts("windows", "");
        assert_eq!(w.os, "windows");
        assert!(!w.rootfs);
        let p = image_facts("localhost:5000/x:1", "");
        assert!(p.private);
    }

    #[test]
    fn forks_of_a_kvm_checkpoint_ask_for_one_whole_kvm_host_of_its_arch() {
        assert_eq!(forks_arch("qemu/kvm/aarch64"), Some("arm64"));
        assert_eq!(forks_arch("qemu/kvm/x86_64"), Some("amd64"));
        // A Mac's checkpoint restores on no cloud host.
        assert_eq!(forks_arch("qemu/hvf/aarch64"), None);
        let r = ElasticRequest {
            kind: "vm".into(),
            cpus: Some(1),
            memory_mb: Some(2048),
            count: 8,
            whole: true,
            caller: Caller::Batch,
            arch: "arm64".into(),
            ..Default::default()
        };
        let g = detect(&r, &UsageBook::default(), "w".into());
        assert!(g.whole && g.req.needs_kvm, "{g:?}");
        assert_eq!(g.req.arch, "arm64");
        assert_eq!((g.count, g.per.millicpus, g.per.memory_mb), (8, 1000, 2048));
    }

    /// A compat run's member (4 GiB, `cua run`'s default) grows a machine it
    /// fits on with the host's own reserve: a cheaper 4 GiB type is never
    /// bought for it (the member would wait on it forever).
    #[test]
    fn growth_for_a_run_buys_a_type_its_member_fits_on() {
        let t = |name: &str, vcpus: u32, gib: u64, usd: f64| HostTypeOffer {
            provider: "hetzner".into(),
            type_name: name.into(),
            region: "fsn1".into(),
            arch: "amd64".into(),
            vcpus,
            memory_mb: gib * 1024,
            disk_gb: 80,
            usd_per_hour: usd,
            billing_step_secs: 3600,
            boot_secs: 60,
            supplies: "hosts".into(),
            max_new: 5,
            kinds: vec!["container".into()],
            ..Default::default()
        };
        let offers: Vec<_> = [t("cx23", 2, 4, 0.0057), t("cpx32", 4, 8, 0.0158)]
            .iter()
            .map(|o| offer_of(o, None))
            .collect();
        let r = ElasticRequest {
            cpus: Some(1),
            memory_mb: Some(4096),
            count: 1,
            caller: Caller::Batch,
            pins: Pins {
                provider: Some("hetzner".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let g = detect(&r, &UsageBook::default(), "run".into());
        let plan = el::solve(&[g], &[], &offers, &Default::default());
        assert_eq!(plan.new_hosts.len(), 1, "{plan:?}");
        assert_eq!(plan.new_hosts[0].type_name, "cpx32", "{plan:?}");
    }

    /// A linked region with no capacity left for any type falls back to
    /// the provider's nearest region; while one type still works there, the
    /// pool stays in the linked region.
    #[test]
    fn a_full_linked_region_falls_back_to_the_nearest_one() {
        let row = |t: &str, region: &str, fallback: u8| HostTypeOffer {
            provider: "hetzner".into(),
            type_name: t.into(),
            region: region.into(),
            fallback,
            ..Default::default()
        };
        let rows = vec![
            row("cx33", "fsn1", 0),
            row("cpx32", "fsn1", 0),
            row("cx33", "nbg1", 1),
            row("cpx32", "nbg1", 1),
            row("cpx32", "hel1", 2),
        ];
        let names = |v: Vec<&HostTypeOffer>| {
            v.iter()
                .map(|o| format!("{}/{}", o.type_name, o.region))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(usable_offers(&rows, |_| false)),
            ["cx33/fsn1", "cpx32/fsn1"]
        );
        assert_eq!(
            names(usable_offers(&rows, |o| o.type_name == "cx33" && o.region == "fsn1")),
            ["cpx32/fsn1"]
        );
        assert_eq!(
            names(usable_offers(&rows, |o| o.region == "fsn1")),
            ["cx33/nbg1", "cpx32/nbg1"]
        );
        assert_eq!(
            names(usable_offers(&rows, |o| o.region != "hel1")),
            ["cpx32/hel1"]
        );
    }

    #[test]
    fn offers_book_overcommitted_cpu_and_keep_a_reserve() {
        let o = HostTypeOffer {
            provider: "hetzner".into(),
            type_name: "cax41".into(),
            region: "fsn1".into(),
            arch: "arm64".into(),
            vcpus: 16,
            memory_mb: 32 * 1024,
            disk_gb: 320,
            kinds: vec!["container".into()],
            usd_per_hour: 0.0684,
            billing_step_secs: 3600,
            billing_min_secs: 3600,
            boot_secs: 75,
            supplies: "hosts".into(),
            max_new: 10,
            quota: "hetzner/shared".into(),
            ..Default::default()
        };
        let x = offer_of(&o, Some(60));
        assert_eq!(x.capacity, Shape::new(64_000, 31 * 1024, 305));
        assert_eq!(x.boot_secs, 60);
        assert!(!x.one_space_each);
        let m = HostTypeOffer {
            supplies: "spaces".into(),
            vcpus: 2,
            memory_mb: 4096,
            ..o
        };
        let y = offer_of(&m, None);
        assert!(y.one_space_each);
        assert_eq!(y.capacity.millicpus, 2000);
    }

    #[test]
    fn policy_round_trips_with_safe_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let p = load_policy(dir.path());
        assert!(p.autoscale);
        assert!(p.max_usd_per_hour > 0.0 && p.max_usd_per_hour <= 5.0);
        let q = ElasticPolicy {
            max_usd_per_hour: 1.5,
            autoscale: false,
            ..p
        };
        save_policy(dir.path(), &q).unwrap();
        assert_eq!(load_policy(dir.path()), q);
    }
}

#[cfg(test)]
mod quota_tests {
    use super::*;

    #[test]
    fn quota_left_is_the_limit_minus_running_hosts() {
        let row = |t: &str, v: u32, pool: &str, lim: u32| HostTypeOffer {
            provider: "hetzner".into(),
            type_name: t.into(),
            vcpus: v,
            quota: pool.into(),
            quota_vcpus: lim,
            ..Default::default()
        };
        let rows = vec![
            row("cax31", 8, "hetzner/shared", 0),
            row("ccx13", 2, "hetzner/dedicated", 20),
        ];
        let host = |t: &str, state: &str| CloudHost {
            provider: "hetzner".into(),
            type_name: t.into(),
            state: state.into(),
            ..Default::default()
        };
        let cloud = vec![host("ccx13", "joined"), host("cax31", "gone")];
        // Unknown shared limit: no constraint until learned.
        let q = quota_left(&rows, &cloud, &LearnedQuotas::default(), 1000);
        assert_eq!(q.get("hetzner/dedicated"), Some(&18));
        assert!(!q.contains_key("hetzner/shared"));
        // Learned: 8, nothing of it in use.
        let mut l = LearnedQuotas::default();
        l.pools.insert("hetzner/shared".into(), (8, 1000));
        let q = quota_left(&rows, &cloud, &l, 1000);
        assert_eq!(q.get("hetzner/shared"), Some(&8));
        // A learned limit expires.
        let q = quota_left(&rows, &cloud, &l, 1000 + LEARNED_QUOTA_SECS);
        assert!(!q.contains_key("hetzner/shared"));
        assert!(is_quota_error(
            "hetzner: create the server: shared core limit exceeded"
        ));
        assert!(is_quota_error(
            "aws: VcpuLimitExceeded: You have requested more vCPU capacity"
        ));
        assert!(!is_quota_error("hetzner: cx23 not available in fsn1"));
    }

    #[tokio::test]
    async fn the_rest_of_a_burst_takes_the_places_its_first_solve_reserved() {
        let dir = tempfile::tempdir().unwrap();
        let s = Spaces::builder().home(dir.path()).build();
        let host = CloudHost {
            name: "aws-3".into(),
            ..Default::default()
        };
        s.reserve(
            "k",
            Placed::NewHost { host: host.clone() },
            2,
            "aws-3".into(),
        );
        s.reserve(
            "k",
            Placed::Host {
                name: "aws-1".into(),
                id: "m1".into(),
            },
            1,
            "aws-1".into(),
        );
        // Another shape takes nothing.
        assert!(s.take_reservation("other", &[]).is_none());
        let a = s.take_reservation("k", &[]).unwrap();
        assert_eq!(a.1, "aws-3");
        // An excluded machine's places are skipped.
        let b = s.take_reservation("k", &["aws-3".into()]).unwrap();
        assert_eq!(b.1, "aws-1");
        // A machine that refused loses its places.
        s.drop_reservations_on("aws-3");
        assert!(s.take_reservation("k", &[]).is_none());
        // Placed on nothing is never reserved.
        s.reserve("k", Placed::Missing("gone".into()), 3, String::new());
        assert!(s.take_reservation("k", &[]).is_none());
        assert_eq!(
            label_for(&Placed::NewHost { host }, "new c7g.2xlarge".into()),
            "aws-3"
        );
    }

    #[test]
    fn a_client_box_without_engines_is_not_a_place_for_spaces() {
        let both = vec!["container".to_string(), "vm".to_string()];
        assert_eq!(this_machine_kinds("macos", None, (false, false)), both);
        assert!(this_machine_kinds("linux", None, (false, false)).is_empty());
        assert_eq!(
            this_machine_kinds("linux", None, (true, false)),
            vec!["container".to_string()]
        );
        assert_eq!(this_machine_kinds("linux", None, (true, true)), both);
        assert!(this_machine_kinds("macos", Some("off"), (true, true)).is_empty());
        assert_eq!(
            this_machine_kinds("linux", Some("on"), (false, false)),
            both
        );
    }

    #[test]
    fn idle_time_restarts_after_work_and_with_a_new_host_of_the_name() {
        let l = |at: u64, event: &str, host: &str| LedgerLine {
            at,
            event: event.into(),
            host: host.into(),
            ..Default::default()
        };
        let lines = vec![
            l(100, "add", "h-1"),
            l(200, "idle", "h-1"),
            l(230, "idle", "h-1"),
            l(300, "busy", "h-1"),
            l(400, "idle", "h-1"),
            l(100, "add", "h-2"),
            l(150, "idle", "h-2"),
            l(160, "remove", "h-2"),
            l(170, "add", "h-2"),
        ];
        let idle = idle_since_of(&lines);
        // Idle again since 400, not since its first idle spell.
        assert_eq!(idle.get("h-1"), Some(&400));
        // A new h-2 is not the old one's idle time.
        assert_eq!(idle.get("h-2"), None);
    }

    #[test]
    fn an_address_limit_is_a_machine_limit_not_a_core_limit() {
        // Live Hetzner refusal: one public IPv4 per server.
        let e = "your cloud: hetzner: create the server: Primary IP limit exceeded \
                 (your project's server limit: ask Hetzner to raise it, or delete servers)";
        assert!(is_machine_limit_error(e));
        assert!(!is_machine_limit_error(
            "hetzner: create the server: shared core limit exceeded"
        ));
        let host = |p: &str, state: &str| CloudHost {
            provider: p.into(),
            state: state.into(),
            ..Default::default()
        };
        let cloud = vec![
            host("hetzner", "joined"),
            host("hetzner", "booting"),
            host("hetzner", "gone"),
            host("aws", "joined"),
        ];
        let mut l = LearnedQuotas::default();
        assert_eq!(machines_left("hetzner", &cloud, &l, 1000), None);
        // Learned at 3 machines; 2 run now: one more.
        l.pools.insert(machines_pool("hetzner"), (3, 1000));
        assert_eq!(machines_left("hetzner", &cloud, &l, 1000), Some(1));
        assert_eq!(machines_left("aws", &cloud, &l, 1000), None);
        assert_eq!(
            machines_left("hetzner", &cloud, &l, 1000 + LEARNED_QUOTA_SECS),
            None
        );
    }
}

impl Spaces {
    /// Before a Space goes: what it uses now and how long it lived, for the
    /// sizes the elastic pool books next time (best effort, bounded).
    pub(crate) async fn learn_usage_before_delete(&self, id: &str) {
        let Ok(Some(rec)) = self.inner.registry.get(id) else {
            return;
        };
        if rec.image.is_empty() {
            return;
        }
        let lifetime = rec
            .added_at
            .as_ref()
            .map(|t| now().saturating_sub(t.seconds.max(0) as u64))
            .unwrap_or(0);
        let usage = tokio::time::timeout(Duration::from_secs(2), async {
            let s = self.space(id).await.ok()?;
            s.usage().await.ok()
        })
        .await
        .ok()
        .flatten();
        // Only a size that is the Space's own (a VM's memory, a container's
        // cgroup): a container without a limit reports its host's.
        if let Some(u) = usage
            && u.memory_used > 0
            && u.memory_limited
        {
            self.elastic_record_usage(&rec.image, None, u.memory_used / (1024 * 1024), lifetime);
        }
    }
}
