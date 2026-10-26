//! Spaces on your machines: a machine set up with `cua host setup
//! --provide-spaces` creates Spaces for the owner's other devices, through
//! the relay. "Spin up 2 macOS Spaces on my spare Mac mini" is
//! `create_space(on="host:spare mac mini", image="macos")`, twice.
//!
//! Two halves:
//!
//! - **Client** ([`Spaces::create`] with [`On::Host`], [`Spaces::delete`]
//!   of a Space a host provides, [`Spaces::host_spaces`]): resolves the
//!   machine by id or name ([`cua_host::match_machine`]; an ambiguous name
//!   returns the choices), registers a relay machine for the new Space as
//!   the signed-in account from this enrolled device (with `host` set, so
//!   lists group it under the host), and asks the host's
//!   `HostSpacesService` to create the Space and attach it to that machine.
//!   The Space is then `relay:<machine>`, like any machine of the account,
//!   and per-agent computer access applies to it as to any other machine.
//! - **Host** ([`HostSpacesServer`], served by the host's cua daemon behind
//!   its cua-spacesd): checks the host settings and the caller, keeps the
//!   capacity limits (two macOS VMs per Mac, by Apple's license; the host's
//!   Space limit), creates the Space with this machine's runtimes (Lume for
//!   macOS, Docker for Linux), attaches it to the relay
//!   (`SystemService.AttachRelay`, the second half of
//!   [`Spaces::relay_register`]), and records every create, delete and
//!   refusal in `~/.cua/host/spaces-audit.jsonl`.
//!
//! The host never holds an account credential: the client registers each
//! Space's relay machine and hands the host only that machine's token.
//!
//! **Without the relay.** A machine set up with `cua host setup --direct
//! <ip:port> --provide-spaces` (reached by its Tailscale or LAN address)
//! serves the same `HostSpacesService` on its direct listener to the holder
//! of its env token. A laptop adds it with `cua spaces add <addr> --host
//! --token <token>` (a [`crate::registry::DirectHost`]), and
//! [`Spaces::resolve_host`] falls back to those hosts when the relay does
//! not know the name. A Space created there is not attached to the relay:
//! the host forwards a port on its own address to the Space's cua-spacesd,
//! and the client adds it as `direct:<host address>:<port>` with the
//! Space's token. The host takes those connections (and host calls) only
//! from loopback, Tailscale and private LAN addresses unless set up with
//! `--allow-any-address`.
//!
//! **Cluster jobs.** The same host runs jobs for a cluster coordinator
//! (`HostJobsService`, [`HostSpacesServer::jobs`]): each in a fresh Space
//! it deletes afterwards ([`crate::host_jobs`]), accepted from the host's
//! own free CPU and memory, recorded in `~/.cua/host/jobs/`. What a
//! scheduler needs to place work here is [`HostSpacesServer::node_status`]
//! (`GetHostSpacesResponse.node`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cua_host::provided::{self, ProvidedSpace};
use cua_host::{DirectHosting, Host, MachineMatch, RelayClient};
use cua_proto::env::v1 as pb;
use cua_sandbox_core::placement::{Kind, On, Runtime};
use serde::Deserialize;

use crate::registry::DirectHost;
use crate::share::RELAY_ATTACH_FEATURE;
use crate::{Error, Result, SpaceCreate, SpaceId, SpaceInfo, Spaces};

/// The spacesd feature a host reports while it provides Spaces.
pub const HOST_SPACES_FEATURE: &str = "host_spaces";

/// This machine, as `on="auto"` lists it among the candidates.
pub const THIS_MACHINE: &str = "this machine";

/// Above this many machines that fit, `on="auto"` picks the first to ask
/// with the power of two choices.
pub const P2C_ABOVE: usize = 8;

/// Metadata carrying the relay-verified caller from the host's driver to
/// its daemon (set by cua-spacesd; absent for a call made on the host
/// itself).
pub const CALLER_METADATA: &str = "x-cua-host-caller";

/// How long a client waits for a new Space to come online on the relay.
const ONLINE_WAIT: Duration = Duration::from_secs(60);

/// Who asked the host for a Space.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct HostCaller {
    /// Account id, or `local` for a call made on the host itself.
    pub account: String,
    /// Verified email, when the relay shares it.
    #[serde(default)]
    pub email: Option<String>,
    /// Display name.
    #[serde(default)]
    pub name: Option<String>,
    /// `owner` or `shared` (an editor of the host).
    #[serde(default)]
    pub role: String,
    /// `relay` or `token`.
    #[serde(default)]
    pub via: String,
}

impl HostCaller {
    /// A caller on this machine (its own app or CLI): the owner.
    pub fn local() -> Self {
        Self {
            account: "local".into(),
            role: "owner".into(),
            via: "token".into(),
            ..Default::default()
        }
    }

    /// The caller in [`CALLER_METADATA`]; `None` (a direct call to this
    /// machine's daemon) is [`HostCaller::local`].
    pub fn from_metadata(value: Option<&str>) -> Result<Self> {
        match value {
            None => Ok(Self::local()),
            Some(v) => serde_json::from_str(v)
                .map_err(|e| Error::invalid(format!("{CALLER_METADATA}: {e}"))),
        }
    }

    /// The caller in a request's [`CALLER_METADATA`]. cua-spacesd escapes
    /// non-ASCII as JSON `\uXXXX`, but releases up to 0.3.0
    /// sent a Unicode display name as raw UTF-8, which is not ASCII text, so
    /// any valid UTF-8 value is accepted.
    pub fn from_grpc_metadata(metadata: &tonic::metadata::MetadataMap) -> Result<Self> {
        let value = metadata
            .get(CALLER_METADATA)
            .map(|v| {
                std::str::from_utf8(v.as_bytes())
                    .map_err(|_| Error::invalid(format!("{CALLER_METADATA}: not UTF-8")))
            })
            .transpose()?;
        Self::from_metadata(value)
    }

    /// `Name <email> (account)` for the audit and people.
    pub fn label(&self) -> String {
        if self.account == "local" {
            return "local".into();
        }
        let mut s = String::new();
        if let Some(n) = self.name.as_deref().filter(|n| !n.is_empty()) {
            s.push_str(n);
            s.push(' ');
        }
        if let Some(e) = self.email.as_deref().filter(|e| !e.is_empty()) {
            s.push_str(&format!("<{e}> "));
        }
        s.push_str(&format!("({})", self.account));
        s
    }

    /// The host's owner (or someone on the host itself).
    pub fn is_owner(&self) -> bool {
        self.role == "owner"
    }

    /// Owner or editor: may create Spaces on the host.
    pub fn may_create(&self) -> bool {
        matches!(self.role.as_str(), "owner" | "shared")
    }
}

/// The relay URL a Space on this host dials: the relay the client named,
/// except that a relay on this machine's loopback (a self-hosted or test
/// relay) is reached from a guest at the host's address as the guest sees
/// it (`host.docker.internal` from Docker Desktop and Colima containers,
/// the Apple Virtualization NAT gateway from a Lume VM or a microVM).
pub fn guest_relay_url(relay_url: &str, kind: &str) -> String {
    let Ok(mut url) = url::Url::parse(relay_url) else {
        return relay_url.to_string();
    };
    let loopback = match url.host() {
        Some(url::Host::Domain(d)) => {
            let d = d.to_ascii_lowercase();
            d == "localhost" || d.ends_with(".localhost")
        }
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    };
    if !loopback {
        return relay_url.to_string();
    }
    let host = if matches!(kind, "vm" | "microvm") && cfg!(target_os = "macos") {
        "192.168.64.1"
    } else {
        "host.docker.internal"
    };
    if url.set_host(Some(host)).is_err() {
        return relay_url.to_string();
    }
    url.to_string().trim_end_matches('/').to_string()
}

/// `image` as the host runs it: an alias (`macos`, `macos:26`, `linux`)
/// resolved to its reference, else the reference as given; and the guest
/// OS when known.
/// What a `CreateHostSpace` carries of `opts`: the image, shape, network,
/// guest environment and GPU. The caller sets how the Space is reached.
fn host_request(opts: &SpaceCreate, fork: Option<&str>) -> pb::CreateHostSpaceRequest {
    pb::CreateHostSpaceRequest {
        image: opts.image.clone().unwrap_or_default(),
        kind: match opts.kind {
            Kind::Auto => String::new(),
            k => k.as_str().into(),
        },
        runtime: match &opts.runtime {
            Runtime::Auto => String::new(),
            r => r.as_str().into(),
        },
        name: opts.name.clone().unwrap_or_default(),
        cpus: opts.cpus.unwrap_or(0),
        memory_mb: opts.memory_mb.unwrap_or(0),
        disk_gb: opts.disk_gb.unwrap_or(0),
        network: network_word(opts.network).into(),
        fork_source: fork.unwrap_or_default().to_string(),
        env: opts.env.clone().into_iter().collect(),
        gpu: opts.gpu.clone().unwrap_or_default(),
        headless: opts.headless,
        egress: egress_to_pb(&opts.egress),
        ..Default::default()
    }
}

/// The wire word of a guest network setting (`""` for the default, so a
/// host that predates the field sees what it always did).
fn network_word(n: cua_sandbox_core::NetworkMode) -> &'static str {
    match n {
        cua_sandbox_core::NetworkMode::Default => "",
        cua_sandbox_core::NetworkMode::None => "none",
    }
}

pub(crate) fn resolve_image(image: &str) -> (String, String) {
    let word = image.trim();
    if word.is_empty() {
        return (
            cua_image::canonical::canonical_image_or_linux("linux"),
            "linux".into(),
        );
    }
    if let Some((reference, os)) =
        cua_image::canonical::alias_with(word, &|n| std::env::var(n).ok())
    {
        return (reference, os.as_str().to_string());
    }
    let os = if provided::is_macos_image(word) {
        "macos"
    } else {
        ""
    };
    (word.to_string(), os.into())
}

/// Why the host `name` (its own Space `s`) does not take host Space
/// calls: Spaces are off on it, or its cua-spacesd predates them (it does
/// not report `host_spaces` at all).
fn not_providing(s: &crate::Space, name: &str) -> Error {
    if s.feature(HOST_SPACES_FEATURE).is_none() {
        let version = s.capabilities().version.trim();
        return Error::host(
            "host spaces",
            format!(
                "{name} runs cua-spacesd {}, which is too old to provide Spaces (it does not \
                 report `{HOST_SPACES_FEATURE}`). On {name}, update cua and run `cua host setup` \
                 again: it installs a current cua-spacesd",
                if version.is_empty() {
                    "of an unknown version"
                } else {
                    version
                }
            ),
        );
    }
    Error::host(
        "host spaces",
        format!("{name} does not provide Spaces (on it: `cua host config --provide-spaces on`)"),
    )
}

/// The base URLs the cua-spacesd of the host with cua home `home` serves
/// snapshot files on: its direct listener, or each of its addresses when it
/// listens on all. Empty for a host that is not in direct mode.
pub fn snapshot_urls_of(home: &std::path::Path) -> Vec<String> {
    let Ok(Some(policy)) = Host::new(home).policy() else {
        return vec![];
    };
    let Some(d) = policy.direct.as_ref() else {
        return vec![];
    };
    let Ok(addr) = d.listen.parse::<std::net::SocketAddr>() else {
        return vec![];
    };
    let base = |ip: std::net::IpAddr| {
        format!(
            "http://{}{}",
            std::net::SocketAddr::new(ip, addr.port()),
            cua_snapshot_sync::HTTP_BASE
        )
    };
    if !addr.ip().is_unspecified() {
        return vec![base(addr.ip())];
    }
    let mut ips: Vec<std::net::IpAddr> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .map(|i| i.ip())
        .filter(|ip| ip.is_ipv4() && !ip.is_loopback() && !ip.is_unspecified())
        .collect();
    ips.sort();
    ips.dedup();
    ips.into_iter().map(base).collect()
}

/// The wire form of a host's egress policy, and back.
fn egress_to_pb(p: &crate::egress::EgressPolicy) -> Option<pb::HostEgressPolicy> {
    (!p.is_empty()).then(|| pb::HostEgressPolicy {
        allow: p.allow.clone(),
        deny: p.deny.clone(),
    })
}

fn egress_from_pb(p: Option<pb::HostEgressPolicy>) -> crate::egress::EgressPolicy {
    p.map(|p| crate::egress::EgressPolicy {
        allow: p.allow,
        deny: p.deny,
    })
    .unwrap_or_default()
}

/// What a host answers for a Space's enforcement: `host`, or empty.
fn enforced_word(e: Option<crate::egress::Enforcement>) -> String {
    e.map(|e| e.as_str().to_string()).unwrap_or_default()
}

fn to_pb(s: &ProvidedSpace) -> pb::HostSpace {
    pb::HostSpace {
        relay_machine: s.relay_machine.clone(),
        local_space: s.local_space.clone(),
        name: s.name.clone(),
        image: s.image.clone(),
        os: s.os.clone(),
        kind: s.kind.clone(),
        runtime: s.runtime.clone(),
        created_by: s.created_by.clone(),
        created_at_ms: s.created_at_ms as i64,
        direct_port: u32::from(s.direct_port),
        // Only the create response carries a direct Space's token.
        direct_token: String::new(),
        // Filled in by the host that knows ([`HostSpacesServer::with_egress`]).
        egress_enforced: String::new(),
        // Only the create response says which network it applied.
        network: String::new(),
    }
}

#[derive(Default)]
struct Reserved {
    spaces: u32,
    macos: u32,
}

/// Holds a capacity slot until the Space is recorded (or the create fails).
struct Slot<'a> {
    reserved: &'a Mutex<Reserved>,
    macos: bool,
}

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        let mut r = self.reserved.lock().expect("reserved");
        r.spaces = r.spaces.saturating_sub(1);
        if self.macos {
            r.macos = r.macos.saturating_sub(1);
        }
    }
}

/// How long `CancelHostSpace` waits for a create that already made its
/// Space to record it, before it looks the Space up to delete it.
const CANCEL_RECORD_WAIT: Duration = Duration::from_secs(60);

/// The creates a host runs now, by key ([`pb::HostSpaceCreating::key`]):
/// the caller's account, and what the create is doing.
#[derive(Clone, Default)]
struct Creating(Arc<Mutex<HashMap<String, (String, pb::HostSpaceCreating)>>>);

impl Creating {
    /// Lists the create `key` of `account` until the returned guard drops
    /// (the create ended, failed, or its call was dropped). `None` for an
    /// empty key (a caller that predates keys).
    fn follow(&self, key: &str, account: &str) -> Option<Following> {
        if key.is_empty() {
            return None;
        }
        let entry = pb::HostSpaceCreating {
            key: key.to_string(),
            phase: cua_sandbox_core::progress::Phase::Preparing.as_str().into(),
            ..Default::default()
        };
        self.lock()
            .insert(key.to_string(), (account.to_string(), entry));
        Some(Following {
            creating: self.clone(),
            key: key.to_string(),
        })
    }

    /// The creates `caller` may see: every one for the owner, its own
    /// otherwise.
    fn listed(&self, caller: &HostCaller) -> Vec<pb::HostSpaceCreating> {
        let mut out: Vec<_> = self
            .lock()
            .values()
            .filter(|(account, _)| caller.is_owner() || *account == caller.account)
            .map(|(_, c)| c.clone())
            .collect();
        out.sort_by(|a, b| a.key.cmp(&b.key));
        out
    }

    /// Whether the create `key` is still running on this host.
    fn running(&self, key: &str) -> bool {
        self.lock().contains_key(key)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, (String, pb::HostSpaceCreating)>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// A create [`Creating`] lists while this lives.
struct Following {
    creating: Creating,
    key: String,
}

impl Following {
    /// The create's progress sink: each report replaces what is listed.
    fn sink(&self) -> crate::ProgressSink {
        let creating = self.creating.clone();
        let key = self.key.clone();
        crate::ProgressSink::new(move |p| {
            if let Some((_, c)) = creating.lock().get_mut(&key) {
                *c = creating_pb(&key, p);
            }
        })
    }
}

impl Drop for Following {
    fn drop(&mut self) {
        self.creating.lock().remove(&self.key);
    }
}

/// A progress report as [`pb::HostSpaceCreating`].
fn creating_pb(key: &str, p: &crate::CreateProgress) -> pb::HostSpaceCreating {
    pb::HostSpaceCreating {
        key: key.to_string(),
        phase: p.phase.as_str().into(),
        fraction: p.fraction,
        detail: p.detail.clone(),
        bytes_done: p.bytes.map(|b| b.done),
        bytes_total: p.bytes.map(|b| b.total).filter(|t| *t > 0),
        bytes_per_second: p.bytes.and_then(|b| b.per_second),
    }
}

/// [`pb::HostSpaceCreating`] as a progress report of the caller's own
/// create. `None` for a phase it does not know, and for `ready`: the host
/// is done, but the caller still connects to the Space and says ready
/// itself.
fn host_progress(c: &pb::HostSpaceCreating) -> Option<crate::CreateProgress> {
    use cua_sandbox_core::progress::{Phase, Progress, Transfer};
    let phase = Phase::parse(&c.phase).filter(|p| *p != Phase::Ready)?;
    let mut p = Progress::phase(phase).detail(c.detail.clone());
    if let Some(f) = c.fraction {
        p = p.fraction(f);
    }
    if let Some(done) = c.bytes_done {
        p = p.bytes(Transfer {
            done,
            total: c.bytes_total.unwrap_or(0),
            per_second: c.bytes_per_second,
        });
    }
    Some(p)
}

/// How often a client asks a host how its create is doing.
const HOST_PROGRESS_POLL: Duration = Duration::from_secs(1);

/// How long a client asks every [`HOST_PROGRESS_POLL`] before it decides
/// the host does not list creates (it predates
/// [`pb::GetHostSpacesResponse::creating`], or its answers do not get
/// through) and asks only every [`HOST_PROGRESS_SLOW_POLL`].
const HOST_PROGRESS_UNLISTED: Duration = Duration::from_secs(20);

/// How often a client asks a host that has not listed the create yet, past
/// [`HOST_PROGRESS_UNLISTED`]: a host that lists it late is still followed.
const HOST_PROGRESS_SLOW_POLL: Duration = Duration::from_secs(10);

/// How long one ask may take: a host (or relay) that does not answer must
/// not stop the following.
const HOST_PROGRESS_ASK: Duration = Duration::from_secs(10);

/// How long a create on a host may go without a report to the caller: past
/// it, the last report (or "preparing") is said again. The host's create
/// is one call that can run for the whole download of a 20+ GB macOS
/// image; the apps' create stream fails a create that reports nothing for
/// 15 minutes, so a host that reports nothing (an older one, or one whose
/// listing does not get through) must not read as a lost create.
const HOST_PROGRESS_HEARTBEAT: Duration = Duration::from_secs(60);

/// Runs `call` (the `CreateHostSpace` of the create `key`) and meanwhile
/// reports what the host lists that create doing as this create's own
/// progress, in order: the image download with its bytes, the disk, the
/// boot. A host that does not list it gets the last report again every
/// [`HOST_PROGRESS_HEARTBEAT`], so the create stays alive while the host
/// works.
async fn following_host<T>(
    host: &cua_spacesd_client::SpacesdClient,
    key: &str,
    call: impl std::future::Future<Output = T>,
) -> T {
    let ask = || async {
        let r = tokio::time::timeout(
            HOST_PROGRESS_ASK,
            host.host_spaces()
                .get_host_spaces(pb::GetHostSpacesRequest {}),
        )
        .await
        .ok()?
        .ok()?;
        Some(r.into_inner().creating)
    };
    following_host_with(ask, key, call).await
}

/// [`following_host`] with `ask` for the host's list of creates (`None`:
/// no answer).
async fn following_host_with<T, A, F>(
    mut ask: A,
    key: &str,
    call: impl std::future::Future<Output = T>,
) -> T
where
    A: FnMut() -> F,
    F: std::future::Future<Output = Option<Vec<pb::HostSpaceCreating>>>,
{
    use cua_sandbox_core::progress::{Phase, Progress, report};
    let poll = async {
        let started = tokio::time::Instant::now();
        let mut listed = false;
        let mut last: Option<crate::CreateProgress> = None;
        let mut said = tokio::time::Instant::now();
        loop {
            let every = if listed || started.elapsed() < HOST_PROGRESS_UNLISTED {
                HOST_PROGRESS_POLL
            } else {
                HOST_PROGRESS_SLOW_POLL
            };
            tokio::time::sleep(every).await;
            let fresh = ask()
                .await
                .and_then(|all| all.into_iter().find(|c| c.key == key))
                .and_then(|c| {
                    listed = true;
                    host_progress(&c)
                })
                // Never back to an earlier phase, never the same report twice.
                .filter(|p| {
                    !last
                        .as_ref()
                        .is_some_and(|l| p.phase.rank() < l.phase.rank() || l == p)
                });
            if let Some(p) = fresh {
                report(p.clone());
                last = Some(p);
                said = tokio::time::Instant::now();
            } else if said.elapsed() >= HOST_PROGRESS_HEARTBEAT {
                // Still working on the host: say so again.
                report(
                    last.clone()
                        .unwrap_or_else(|| Progress::phase(Phase::Preparing)),
                );
                said = tokio::time::Instant::now();
            }
        }
    };
    tokio::pin!(call);
    tokio::select! {
        out = &mut call => out,
        () = poll => unreachable!("the poll never ends"),
    }
}

/// The host half: what this machine's cua daemon does for
/// `HostSpacesService` (and `HostJobsService`) calls.
pub struct HostSpacesServer {
    spaces: Spaces,
    /// The cluster job runner, opened on first use.
    jobs: tokio::sync::OnceCell<Arc<cua_cluster::NodeJobs>>,
    /// This start of the daemon (`HostNodeStatus.boot_id`).
    boot_id: String,
    reserved: Mutex<Reserved>,
    /// The creates running here, with what each is doing now.
    creating: Creating,
    /// The ports this host forwards to the Spaces it provides directly,
    /// by the Space's `local:` id.
    forwards: Mutex<HashMap<String, tokio::task::JoinHandle<()>>>,
}

impl Drop for HostSpacesServer {
    fn drop(&mut self) {
        if let Ok(forwards) = self.forwards.get_mut() {
            for task in forwards.values() {
                task.abort();
            }
        }
    }
}

impl HostSpacesServer {
    /// Serves Spaces from `spaces` (the daemon's runtime; its home is the
    /// cua home whose `host/` holds the settings).
    pub fn new(spaces: Spaces) -> Arc<Self> {
        Arc::new(Self {
            spaces,
            jobs: tokio::sync::OnceCell::new(),
            boot_id: format!("{:016x}", rand::random::<u64>()),
            reserved: Mutex::default(),
            creating: Creating::default(),
            forwards: Mutex::default(),
        })
    }

    /// The cluster job runner (`HostJobsService`), opened on first use on
    /// `<cua home>/host/jobs`, with this machine's capacity
    /// ([`cua_cluster::NodeConfig::detect`]) less what the Spaces it
    /// provides reserve.
    pub async fn jobs(&self) -> Result<Arc<cua_cluster::NodeJobs>> {
        self.jobs
            .get_or_try_init(|| async {
                let home = self.spaces.home_dir().to_path_buf();
                // A gang member's peers on other machines are reached over
                // the Cua mesh (CUA-1783), from whichever Space it runs in.
                let router: Arc<dyn cua_cluster::peers::PeerRouter> =
                    Arc::new(cua_cluster::gang_mesh::MeshPeerRouter::new(&home));
                // A dedicated host's own Space runs through its own
                // cua-spacesd (`cua host setup --dedicated`).
                let host = self.host();
                let machine =
                    cua_cluster::machine::MachineExecutor::new(&home, move || host_spacesd(&host))
                        .with_peer_router(router.clone());
                let exec = crate::host_jobs::SpacesExecutor::local(self.spaces.clone())
                    .await
                    .with_peer_router(router)
                    .with_machine(machine, crate::host_jobs::dedicated(self.host()));
                let mut cfg = cua_cluster::NodeConfig::detect(self.host_dir().join("jobs"));
                cfg.boot_id = self.boot_id.clone();
                let jobs = cua_cluster::NodeJobs::open(cfg, Arc::new(exec))?;
                let host_dir = self.host_dir();
                jobs.set_external(move || provided_reservation(&host_dir));
                // Before a job is refused for CPU or memory, take back what
                // idle warm members and expired or orphaned VMs hold.
                let spaces = self.spaces.clone();
                jobs.set_reclaim(move |millicpus, memory_mb| {
                    let spaces = spaces.clone();
                    async move {
                        let r = spaces
                            .reclaim(cua_sandbox_core::Shortfall {
                                millicpus,
                                memory_mb,
                            })
                            .await;
                        if r.any() {
                            tracing::info!(
                                warm_members = r.warm_members,
                                ended = r.ended.len(),
                                "freed capacity before refusing a job"
                            );
                        }
                    }
                });
                Ok::<_, Error>(jobs)
            })
            .await
            .cloned()
    }

    /// Opens the job runner when the daemon starts and a ledger exists, so
    /// jobs a crash interrupted end (and their Spaces go) at once.
    pub async fn resume_jobs(&self) {
        if cua_cluster::ledger::ledger_path(&self.host_dir().join("jobs")).is_file()
            && let Err(e) = self.jobs().await
        {
            tracing::warn!("the cluster job runner did not open: {e}");
        }
    }

    /// The job runner for `caller`: the host must provide Spaces, and the
    /// caller may create them.
    pub async fn jobs_for(&self, caller: &HostCaller) -> Result<Arc<cua_cluster::NodeJobs>> {
        let policy = self.policy()?;
        if !policy.provide_spaces {
            return Err(Error::host(
                "host spaces",
                "this machine does not provide Spaces, so it runs no jobs (on it: `cua host config --provide-spaces on`)",
            ));
        }
        if !caller.may_create() {
            return Err(Error::Relay(cua_host::Error::PermissionDenied(
                "only the owner and the accounts this machine is shared with (as editors) can run jobs on it"
                    .into(),
            )));
        }
        self.jobs().await
    }

    /// The job runner for `caller` to look up or cancel its own jobs: the
    /// caller may create Spaces here, whether or not the host takes new
    /// work now (turning provide Spaces off drains the host: jobs already
    /// on it finish and their results are collected).
    pub async fn jobs_of(&self, caller: &HostCaller) -> Result<Arc<cua_cluster::NodeJobs>> {
        if !caller.may_create() {
            return Err(Error::Relay(cua_host::Error::PermissionDenied(
                "only the owner and the accounts this machine is shared with (as editors) can run jobs on it"
                    .into(),
            )));
        }
        self.jobs().await
    }

    /// The Spaces this host serves from.
    pub fn spaces(&self) -> &Spaces {
        &self.spaces
    }

    /// This host as a scheduler sees it (`GetHostSpacesResponse.node`).
    pub async fn node_status(&self) -> Option<pb::HostNodeStatus> {
        self.forget_lost_spaces().await;
        let jobs = self.jobs().await.ok()?;
        let mut st = jobs.status();
        if st.problem.is_empty()
            && let Ok(policy) = self.policy()
            && !policy.provide_spaces
        {
            st.problem = "this machine does not provide Spaces".into();
        }
        // The checkpoints forks can start from here (batched forks place
        // by them).
        let (snapshots, abis) = self.spaces.local_snapshots().await;
        st.snapshots = snapshots.into_iter().collect();
        st.snapshot_abis = abis;
        Some(st)
    }

    /// Forgets the provided Spaces whose sandbox is gone (deleted behind the
    /// host's back, or lost with a machine that went down): their records
    /// would keep reserving CPU and memory for nothing.
    async fn forget_lost_spaces(&self) {
        let Ok(all) = provided::load_provided(&self.host_dir()) else {
            return;
        };
        for s in all {
            let Some(name) = s.local_space.strip_prefix("local:") else {
                continue;
            };
            if self.spaces.sandboxes().state().load(name).is_none() {
                tracing::info!(space = %s.local_space, "forgetting a provided Space whose sandbox is gone");
                self.close_forward(&s.local_space);
                forget_provided(&self.spaces, &s.local_space).await;
            }
        }
    }

    /// `ListHostSnapshots`: the owner and editors.
    pub async fn snapshots_list(
        &self,
        caller: &HostCaller,
    ) -> Result<pb::ListHostSnapshotsResponse> {
        self.snapshot_caller(caller, false)?;
        Ok(self.spaces.list_snapshots().await)
    }

    /// `ExportHostSnapshot`: the owner only (the files are the
    /// checkpoint's memory and disk).
    pub async fn snapshot_export(
        &self,
        caller: &HostCaller,
        req: pb::ExportHostSnapshotRequest,
    ) -> Result<pb::ExportHostSnapshotResponse> {
        self.snapshot_caller(caller, true)?;
        let (m, digest) = self.spaces.export_snapshot(&req.name).await?;
        let grant = self
            .spaces
            .snapshot_grant(&digest, u64::from(req.grant_ttl_secs))?;
        Ok(pb::ExportHostSnapshotResponse {
            snapshot: Some(pb::HostSnapshot {
                name: m.name.clone(),
                digest,
                abi: m.abi.clone(),
                size_bytes: m.size(),
                ..Default::default()
            }),
            manifest: m.to_bytes(),
            grant,
            path: cua_snapshot_sync::HTTP_BASE.into(),
            urls: self.snapshot_urls(),
        })
    }

    /// `PullHostSnapshot`: the owner and editors (a fork needs it here).
    pub async fn snapshot_pull(
        &self,
        caller: &HostCaller,
        req: pb::PullHostSnapshotRequest,
        progress: Option<cua_snapshot_sync::ProgressFn>,
    ) -> Result<pb::HostSnapshot> {
        self.snapshot_caller(caller, false)?;
        let manifest = if req.manifest.is_empty() {
            None
        } else {
            Some(
                cua_snapshot_sync::Manifest::from_bytes(&req.manifest)
                    .map_err(|e| Error::invalid(format!("snapshot manifest: {e}")))?,
            )
        };
        let r = self
            .spaces
            .pull_snapshot(
                manifest,
                &req.name,
                &req.sources,
                req.fallback.as_ref(),
                progress,
            )
            .await;
        self.audit(
            if r.is_ok() { "snapshot" } else { "failed" },
            caller,
            &req.name,
            &match &r {
                Ok(s) => format!("copied checkpoint {} ({})", s.name, s.digest),
                Err(e) => format!("copy a checkpoint: {e}"),
            },
        );
        r
    }

    /// `PublishHostSnapshot`: the owner only.
    pub async fn snapshot_publish(
        &self,
        caller: &HostCaller,
        req: pb::PublishHostSnapshotRequest,
        progress: Option<cua_snapshot_sync::ProgressFn>,
    ) -> Result<u64> {
        self.snapshot_caller(caller, true)?;
        let fallback = req
            .fallback
            .ok_or_else(|| Error::invalid("publishing needs a fallback (s3 or dir)"))?;
        self.spaces
            .publish_snapshot(&req.name, &fallback, progress)
            .await
    }

    /// `CreateHostSnapshot`: checkpoints a Space this host provides
    /// (`Sandboxes::checkpoint_with` on its sandbox here); it keeps running
    /// and the checkpoint stays on this host. The owner checkpoints any;
    /// another account only the Spaces it created, and it may delete the
    /// checkpoints it took ([`HostSpacesServer::snapshot_delete`]).
    pub async fn snapshot_create(
        &self,
        caller: &HostCaller,
        req: pb::CreateHostSnapshotRequest,
    ) -> Result<pb::HostSnapshot> {
        self.snapshot_caller(caller, false)?;
        let space = req
            .space
            .trim()
            .strip_prefix("relay:")
            .unwrap_or(req.space.trim())
            .to_string();
        let checkpoint = req.name.trim().to_string();
        if checkpoint.is_empty() {
            return Err(Error::invalid("a checkpoint needs a name"));
        }
        let Some(record) = provided::load_provided(&self.host_dir())?
            .into_iter()
            .find(|s| !space.is_empty() && (s.relay_machine == space || s.local_space == space))
        else {
            return Err(self.refuse(
                caller,
                &space,
                Error::NotFound(format!("{space} is not a Space this host provides")),
            ));
        };
        if !caller.is_owner() && record.created_by_account != caller.account {
            return Err(self.refuse(
                caller,
                &space,
                Error::Relay(cua_host::Error::PermissionDenied(format!(
                    "{space} was created by someone else; only they or the host's owner can checkpoint it"
                ))),
            ));
        }
        let SpaceId::Local { name } = SpaceId::parse(&record.local_space)? else {
            return Err(Error::invalid(format!(
                "{} is not a local Space",
                record.local_space
            )));
        };
        let subject = if record.is_direct() {
            record.local_space.clone()
        } else {
            record.relay_machine.clone()
        };
        match self
            .spaces
            .sandboxes()
            .checkpoint_with(&name, &checkpoint, req.pin)
            .await
        {
            Ok(rec) => {
                if let Err(e) = snapshot_takers::record(&self.host_dir(), &checkpoint, caller) {
                    tracing::warn!(checkpoint, "who took the checkpoint is not recorded: {e}");
                }
                self.audit(
                    "checkpoint",
                    caller,
                    &subject,
                    &format!("checkpoint {checkpoint} of {}", record.local_space),
                );
                Ok(crate::snapshots::host_snapshot_of(&rec))
            }
            Err(e) => {
                let e = Error::from(e);
                self.audit("failed", caller, &subject, &format!("checkpoint: {e}"));
                Err(e)
            }
        }
    }

    /// `DeleteHostSnapshot`: deletes a checkpoint this host holds (forks
    /// made from it keep running; a pinned one is refused until it is
    /// unpinned). The owner deletes any; another account only the ones it
    /// took here with [`HostSpacesServer::snapshot_create`].
    pub async fn snapshot_delete(
        &self,
        caller: &HostCaller,
        req: pb::DeleteHostSnapshotRequest,
    ) -> Result<String> {
        self.snapshot_caller(caller, false)?;
        let checkpoint = req.name.trim().to_string();
        let host = self.host_name();
        if checkpoint.is_empty()
            || self
                .spaces
                .sandboxes()
                .checkpoint_record(&checkpoint)
                .is_none()
        {
            return Err(Error::NotFound(format!(
                "checkpoint {checkpoint} (not on {host})"
            )));
        }
        if !caller.is_owner()
            && snapshot_takers::taker(&self.host_dir(), &checkpoint).as_deref()
                != Some(caller.account.as_str())
        {
            return Err(self.refuse(
                caller,
                &checkpoint,
                Error::Relay(cua_host::Error::PermissionDenied(format!(
                    "checkpoint {checkpoint} was taken by someone else; only they or the host's owner can delete it"
                ))),
            ));
        }
        if let Err(e) = self.spaces.sandboxes().delete_checkpoint(&checkpoint).await {
            let e = Error::from(e);
            self.audit(
                "failed",
                caller,
                &checkpoint,
                &format!("delete checkpoint: {e}"),
            );
            return Err(e);
        }
        let _ = snapshot_takers::forget(&self.host_dir(), &checkpoint);
        self.audit("delete", caller, &checkpoint, "checkpoint");
        Ok(format!("Deleted checkpoint {checkpoint} on {host}."))
    }

    fn snapshot_caller(&self, caller: &HostCaller, owner: bool) -> Result<()> {
        let ok = if owner {
            caller.is_owner()
        } else {
            caller.may_create()
        };
        if ok {
            Ok(())
        } else {
            Err(Error::Relay(cua_host::Error::PermissionDenied(
                if owner {
                    "only this machine's owner can export or publish its checkpoints"
                } else {
                    "only the owner and the accounts this machine is shared with (as editors) can copy checkpoints to it"
                }
                .into(),
            )))
        }
    }

    /// The base URLs this host's cua-spacesd serves snapshot files on
    /// ([`snapshot_urls_of`]).
    pub fn snapshot_urls(&self) -> Vec<String> {
        snapshot_urls_of(self.spaces.home_dir())
    }

    /// Opens the forwarded port of every Space this host provides directly
    /// that is not open yet (after the daemon started again). The daemon
    /// calls it when it starts; `GetHostSpaces` and turning a Space on do
    /// too. Nothing for a host that is not in direct mode.
    pub async fn restore_direct_forwards(&self) {
        let Ok(policy) = self.policy() else {
            return;
        };
        if !policy.provide_spaces {
            return;
        }
        let Ok(all) = provided::load_provided(&self.host_dir()) else {
            return;
        };
        let loopback = routed_hosting();
        for record in all.iter().filter(|s| s.is_direct()) {
            let hosting = if record.routed {
                &loopback
            } else {
                match policy.direct.as_ref() {
                    Some(d) => d,
                    None => continue,
                }
            };
            if self.forward_open(&record.local_space) {
                continue;
            }
            if let Err(e) = self
                .open_forward(hosting, &record.local_space, record.direct_port)
                .await
            {
                tracing::warn!(
                    space = %record.local_space,
                    port = record.direct_port,
                    "the forwarded port of a direct Space did not open again: {e}"
                );
            }
        }
    }

    fn forward_open(&self, local_space: &str) -> bool {
        self.forwards
            .lock()
            .expect("forwards")
            .get(local_space)
            .is_some_and(|t| !t.is_finished())
    }

    fn close_forward(&self, local_space: &str) {
        if let Some(task) = self.forwards.lock().expect("forwards").remove(local_space) {
            task.abort();
        }
    }

    /// Forwards `port` (0: any free one) on the host's direct address to
    /// the cua-spacesd of its local Space `local_space`, and returns the
    /// port. Each connection looks the Space up again (a VM's address can
    /// change when it boots again); one from outside loopback, Tailscale
    /// and private LAN addresses is dropped unless the host allows any.
    async fn open_forward(
        &self,
        direct: &DirectHosting,
        local_space: &str,
        port: u16,
    ) -> Result<u16> {
        let listen: std::net::SocketAddr = direct.listen.parse().map_err(|_| {
            Error::invalid(format!(
                "this host's direct listen address {:?} is not ip:port; run `cua host setup --direct` again",
                direct.listen
            ))
        })?;
        let SpaceId::Local { name } = SpaceId::parse(local_space)? else {
            return Err(Error::invalid(format!(
                "{local_space} is not a local Space"
            )));
        };
        let listener =
            tokio::net::TcpListener::bind(std::net::SocketAddr::new(listen.ip(), port)).await?;
        let bound = listener.local_addr()?.port();
        let task = tokio::spawn(forward_loop(
            listener,
            self.spaces.sandboxes().clone(),
            name,
            direct.allow_any_address,
        ));
        if let Some(old) = self
            .forwards
            .lock()
            .expect("forwards")
            .insert(local_space.to_string(), task)
        {
            old.abort();
        }
        Ok(bound)
    }

    /// The cua-spacesd token of this host's local Space `local_space`. A
    /// Space without one is never forwarded.
    async fn space_token(&self, local_space: &str) -> Result<String> {
        if let Some(t) = self
            .spaces
            .registry()
            .credential(local_space)?
            .and_then(|c| c.token)
            .filter(|t| !t.is_empty())
        {
            return Ok(t);
        }
        let SpaceId::Local { name } = SpaceId::parse(local_space)? else {
            return Err(Error::invalid(format!(
                "{local_space} is not a local Space"
            )));
        };
        let sandbox = self.spaces.sandboxes().connect(&name).await?;
        if sandbox.env_token().is_none() {
            // A bootstrap-mode spacesd: installs and records one.
            let _ = sandbox.spacesd().await;
        }
        sandbox
            .env_token()
            .filter(|t| !t.is_empty())
            .ok_or_else(|| {
                Error::invalid(format!(
                    "{local_space} has no cua-spacesd token, so it is not reachable directly"
                ))
            })
    }

    fn host(&self) -> Host {
        Host::new(self.spaces.home_dir())
    }

    fn host_dir(&self) -> std::path::PathBuf {
        self.host().paths().dir.clone()
    }

    fn audit(&self, action: &str, who: &HostCaller, space: &str, detail: &str) {
        if let Err(e) = provided::audit(&self.host_dir(), action, &who.label(), space, detail) {
            tracing::warn!("host spaces audit: {e}");
        }
    }

    /// Refuses with `err`, recording why.
    fn refuse(&self, who: &HostCaller, space: &str, err: Error) -> Error {
        self.audit("refused", who, space, &err.to_string());
        err
    }

    /// The Spaces runtime this host serves.
    pub(crate) fn spaces_ref(&self) -> &Spaces {
        &self.spaces
    }

    /// Whether this machine provides Spaces now.
    pub(crate) fn provides_spaces(&self) -> bool {
        self.policy().is_ok_and(|p| p.provide_spaces)
    }

    fn policy(&self) -> Result<cua_host::HostPolicy> {
        self.host().policy()?.ok_or_else(|| {
            Error::host(
                "host spaces",
                "this computer is not set up to provide Spaces (run `cua host setup --provide-spaces` on it)",
            )
        })
    }

    /// macOS VMs running on this Mac now: the ones it provides, and the
    /// owner's own local macOS Spaces (Apple's limit is per Mac).
    fn macos_in_use(&self, provided: &[ProvidedSpace]) -> u32 {
        let mut ids: std::collections::BTreeSet<String> = provided
            .iter()
            .filter(|s| s.is_macos())
            .map(|s| s.local_space.clone())
            .collect();
        if let Ok(records) = self.spaces.registry().list() {
            for r in records {
                let local = matches!(SpaceId::parse(&r.id), Ok(SpaceId::Local { .. }));
                if local && (r.os == "macos" || provided::is_macos_image(&r.image)) {
                    ids.insert(r.id.clone());
                }
            }
        }
        ids.len() as u32
    }

    fn capacity(
        &self,
        policy: &cua_host::HostPolicy,
        provided: &[ProvidedSpace],
    ) -> Vec<pb::HostSpacesCapacity> {
        let settings = policy.spaces_settings();
        let r = self.reserved.lock().expect("reserved");
        let mut out = vec![pb::HostSpacesCapacity {
            resource: "spaces".into(),
            used: provided.len() as u32 + r.spaces,
            limit: settings.max_spaces,
            reason: "this host's limit (`cua host config --max-spaces`)".into(),
        }];
        if cfg!(target_os = "macos") {
            out.push(pb::HostSpacesCapacity {
                resource: "macos_vms".into(),
                used: self.macos_in_use(provided) + r.macos,
                limit: settings.max_macos_vms,
                reason: provided::MACOS_LIMIT_REASON.into(),
            });
        }
        out
    }

    /// Reserves a slot for one more Space, or says which limit is full.
    fn reserve(
        &self,
        policy: &cua_host::HostPolicy,
        provided: &[ProvidedSpace],
        macos: bool,
        host_name: &str,
    ) -> Result<Slot<'_>> {
        let settings = policy.spaces_settings();
        let macos_used = if macos {
            self.macos_in_use(provided)
        } else {
            0
        };
        let mut r = self.reserved.lock().expect("reserved");
        let used = provided.len() as u32 + r.spaces;
        if settings.max_spaces > 0 && used >= settings.max_spaces {
            return Err(Error::LimitExceeded(format!(
                "{host_name} already provides {used} Spaces, its limit (on it: `cua host config \
                 --max-spaces N`); delete one first"
            )));
        }
        if macos {
            let in_use = macos_used + r.macos;
            if in_use >= settings.max_macos_vms {
                return Err(Error::LimitExceeded(format!(
                    "{host_name} already runs {in_use} macOS VM{} and allows {}: {}. Delete one \
                     (delete_space) or create a Linux Space instead",
                    if in_use == 1 { "" } else { "s" },
                    settings.max_macos_vms,
                    provided::MACOS_LIMIT_REASON
                )));
            }
            r.macos += 1;
        }
        r.spaces += 1;
        Ok(Slot {
            reserved: &self.reserved,
            macos,
        })
    }

    fn host_name(&self) -> String {
        self.host()
            .config()
            .ok()
            .flatten()
            .map(|c| c.name)
            .unwrap_or_else(cua_host::device_name)
    }

    /// Stops providing the Spaces whose sandbox is definitely gone (a
    /// container removed with `docker rm`, a VM deleted outside Cua): the
    /// record would otherwise list a Space that no longer exists for ever,
    /// and whoever placed a durable Space here would never learn it is lost.
    /// A Space that is merely stopped or whose engine does not answer stays.
    async fn forget_missing(&self) {
        let Ok(all) = provided::load_provided(&self.host_dir()) else {
            return;
        };
        for record in all {
            let Ok(SpaceId::Local { name }) = SpaceId::parse(&record.local_space) else {
                continue;
            };
            let gone = match self.spaces.sandboxes().get(&name).await {
                Ok(info) => {
                    matches!(&info.status, cua_sandbox_core::Status::Unknown(s) if s == cua_sandbox_core::MISSING)
                }
                Err(cua_sandbox_core::Error::NotFound(_)) => true,
                Err(_) => false,
            };
            if gone {
                tracing::warn!(space = %record.local_space, "a provided Space is gone from this host: no longer listing it");
                self.close_forward(&record.local_space);
                forget_provided(&self.spaces, &record.local_space).await;
            }
        }
    }

    /// `GetHostSpaces`: settings, capacity, the caller's Spaces (all of
    /// them for the owner) and, for the owner, the recent audit.
    pub async fn get(&self, caller: &HostCaller) -> Result<pb::GetHostSpacesResponse> {
        let policy = self.policy()?;
        // A direct host's driver asks when it starts: the forwarded ports
        // open again.
        self.restore_direct_forwards().await;
        self.forget_missing().await;
        let all = provided::load_provided(&self.host_dir())?;
        let settings = policy.spaces_settings();
        let mut images = vec!["linux".to_string()];
        if cfg!(target_os = "macos") {
            images.push("macos".into());
        }
        let node = self.node_status().await;
        Ok(pb::GetHostSpacesResponse {
            node,
            name: self.host_name(),
            os: std::env::consts::OS.into(),
            settings: Some(pb::HostSpacesSettings {
                share_desktop: settings.share_desktop,
                provide_spaces: settings.provide_spaces,
                max_spaces: settings.max_spaces,
                max_macos_vms: if cfg!(target_os = "macos") {
                    settings.max_macos_vms
                } else {
                    0
                },
            }),
            capacity: self.capacity(&policy, &all),
            spaces: all
                .iter()
                .filter(|s| caller.is_owner() || s.created_by_account == caller.account)
                .map(|s| self.with_egress(to_pb(s)))
                .collect(),
            audit: if caller.is_owner() {
                provided::read_audit(&self.host_dir(), 50)
                    .recent
                    .into_iter()
                    .map(|e| pb::HostSpacesAuditEvent {
                        ts_ms: e.at_ms as i64,
                        action: e.action,
                        who: e.who,
                        space: e.space,
                        detail: e.detail,
                    })
                    .collect()
            } else {
                vec![]
            },
            images,
            creating: self.creating.listed(caller),
        })
    }

    /// `CreateHostSpace`: creates one Space here and attaches it to the
    /// relay machine the caller registered for it.
    pub async fn create(
        &self,
        caller: &HostCaller,
        req: pb::CreateHostSpaceRequest,
    ) -> Result<pb::HostSpace> {
        // Host Spaces use: the kind and guest family only, never the name,
        // image, caller or machine.
        let kind = req.kind.clone();
        let image = req.image.to_ascii_lowercase();
        // Empty is the canonical Linux image.
        let guest = ["macos", "windows", "android", "linux"]
            .into_iter()
            .find(|os| image.contains(os))
            .unwrap_or(if image.is_empty() { "linux" } else { "unknown" });
        let r = self.create_provided(caller, req).await;
        cua_telemetry::capture(cua_telemetry::events::host_space_provided(
            &kind,
            guest,
            if r.is_ok() {
                cua_telemetry::Outcome::Ok
            } else {
                cua_telemetry::Outcome::Error
            },
            // The cause the error's text names (`image_pull_auth`,
            // `insufficient_disk`, ...), else the error's tag; only the
            // word is sent, never the text.
            r.as_ref().err().map(|e| {
                let message = e.to_string();
                cua_create_failure::create_failure_kind(&message, "", e.tag(), true)
            }),
        ));
        r
    }

    async fn create_provided(
        &self,
        caller: &HostCaller,
        req: pb::CreateHostSpaceRequest,
    ) -> Result<pb::HostSpace> {
        let attach = req.attach.clone().unwrap_or_default();
        // A routed Space is reached like a direct one, through a forward on
        // this host's loopback that callers open over the host's channel:
        // it needs no direct address and registers no relay machine.
        let routed = req.routed;
        let direct = req.direct || routed;
        let label = if direct || attach.machine_id.is_empty() {
            req.name.clone()
        } else {
            attach.machine_id.clone()
        };
        let policy = self.policy().map_err(|e| self.refuse(caller, &label, e))?;
        if !policy.provide_spaces {
            return Err(self.refuse(
                caller,
                &label,
                Error::host(
                    "host spaces",
                    "this computer does not provide Spaces (on it: `cua host config --provide-spaces on`)",
                ),
            ));
        }
        if !caller.may_create() {
            return Err(self.refuse(
                caller,
                &label,
                Error::Relay(cua_host::Error::PermissionDenied(
                    "only the owner and the accounts this computer is shared with (as editors) can create Spaces on it"
                        .into(),
                )),
            ));
        }
        let direct_hosting = match (direct, policy.direct.clone()) {
            _ if routed => Some(routed_hosting()),
            (true, Some(d)) => Some(d),
            (true, None) => {
                return Err(self.refuse(
                    caller,
                    &label,
                    Error::host(
                        "host spaces",
                        "this computer provides Spaces through the relay, not directly (on it: \
                         `cua host setup --direct <ip:port> --provide-spaces`)",
                    ),
                ));
            }
            (false, _) => None,
        };
        if direct && !attach.machine_id.is_empty() {
            return Err(self.refuse(
                caller,
                &label,
                Error::invalid("a direct Space is not attached to the relay; leave attach empty"),
            ));
        }
        if !direct
            && (attach.machine_id.is_empty()
                || attach.machine_token.is_empty()
                || attach.relay_url.is_empty())
        {
            return Err(self.refuse(
                caller,
                &label,
                Error::invalid(
                    "attach needs the relay URL, computer id and computer token the caller registered",
                ),
            ));
        }
        let (image, os) = resolve_image(&req.image);
        let macos = os == "macos";
        let host_name = self.host_name();
        if macos && !cfg!(target_os = "macos") {
            return Err(self.refuse(
                caller,
                &label,
                Error::host(
                    "macOS VMs",
                    format!(
                        "{host_name} runs {}; macOS Spaces need a Mac host (Apple Virtualization with Lume)",
                        std::env::consts::OS
                    ),
                ),
            ));
        }
        let place = |e: cua_sandbox_core::placement::PlacementError| {
            Error::Sandbox(cua_sandbox_core::Error::InvalidPlacement(e))
        };
        let kind = Kind::parse(&req.kind).map_err(place)?;
        let runtime = Runtime::parse(&req.runtime).map_err(place)?;
        // This host's runtime, not the caller's: an explicit one it cannot
        // run is refused naming what it can; `auto` takes its own
        // `default.runtime`.
        let settings = cua_sandbox_core::settings::Settings::load().ok();
        let (mut kind, mut runtime) = host_placement(
            kind,
            runtime,
            macos,
            settings.as_ref(),
            &cua_sandbox_core::placement::HostFacts::current(),
        )
        .map_err(|e| self.refuse(caller, &label, place(e)))?;
        let provided_now = provided::load_provided(&self.host_dir())?;
        let slot = self
            .reserve(&policy, &provided_now, macos, &host_name)
            .map_err(|e| self.refuse(caller, &label, e))?;
        // Without a Space limit (`max_spaces` 0, a cluster host), what is
        // free decides: the CPU and memory its jobs and Spaces do not
        // reserve.
        if policy.spaces_settings().max_spaces == 0
            && let Some(free) = self.node_status().await
        {
            let cpu = if req.cpus == 0 {
                2000
            } else {
                req.cpus.saturating_mul(1000)
            };
            let mem = if req.memory_mb == 0 {
                4096
            } else {
                req.memory_mb
            };
            // A `vm` Space: its vCPUs share host CPUs by the VM overcommit,
            // and it needs what a VM here uses out of the measured VM
            // memory (copy-on-write snapshot memory shared), not its whole
            // guest RAM out of the reservations.
            let measured = free.measured.unwrap_or_default();
            let (cpu, mem, free_mem) = if kind == Kind::Vm && measured.vm_memory_known {
                (
                    cua_cluster::sysinfo::vm_millicpus(cpu),
                    cua_cluster::node::expected_vm_mb(mem, measured.vm_typical_mb),
                    measured.vm_memory_free_mb,
                )
            } else if matches!(kind, Kind::Container | Kind::Auto) {
                // Containers share CPUs by the container overcommit.
                (
                    cua_cluster::sysinfo::container_millicpus(cpu),
                    mem,
                    free.memory_free_mb,
                )
            } else {
                (cpu, mem, free.memory_free_mb)
            };
            if free.millicpus_free < cpu || free_mem < mem {
                return Err(self.refuse(
                    caller,
                    &label,
                    Error::LimitExceeded(format!(
                        "{host_name} has {:.1} CPUs and {} MiB free, not the {:.1} CPUs and {mem} MiB \
                         this Space needs; delete a Space or wait for its jobs",
                        f64::from(free.millicpus_free) / 1000.0,
                        free_mem,
                        f64::from(cpu) / 1000.0,
                    )),
                ));
            }
        }
        // Listed in `GetHostSpaces` while it runs, for the caller to follow.
        let key = Some(req.create_key.trim())
            .filter(|k| !k.is_empty())
            .unwrap_or(&attach.machine_id);
        let network = match cua_sandbox_core::NetworkMode::parse(&req.network) {
            Ok(n) => n,
            Err(e) => return Err(self.refuse(caller, &label, Error::invalid(e.to_string()))),
        };
        let following = self.creating.follow(key, &caller.account);
        let fork_source = req.fork_source.trim().to_string();
        let created = if !fork_source.is_empty() {
            // A fork restores its source's guest: the policy is applied
            // to the new Space before it is handed out.
            let egress = egress_from_pb(req.egress.clone());
            match self
                .spaces
                .fork_local(
                    &fork_source,
                    Some(req.name.clone()).filter(|n| !n.trim().is_empty()),
                )
                .await
            {
                Ok(info) if !egress.is_empty() => {
                    match self.spaces.set_egress(&info.id, &egress).await {
                        Ok(()) => Ok(crate::SpaceCreated::Ready {
                            info,
                            reused: false,
                        }),
                        Err(e) => {
                            let _ = self.spaces.delete(&info.id).await;
                            Err(e)
                        }
                    }
                }
                other => other.map(|info| crate::SpaceCreated::Ready {
                    info,
                    reused: false,
                }),
            }
        } else {
            let create = SpaceCreate {
                progress: following.as_ref().map(Following::sink),
                image: Some(image.clone()),
                on: Some(On::Local),
                kind,
                runtime: runtime.clone(),
                name: Some(req.name.clone()).filter(|n| !n.trim().is_empty()),
                cpus: Some(req.cpus).filter(|c| *c > 0),
                memory_mb: Some(req.memory_mb).filter(|m| *m > 0),
                disk_gb: Some(req.disk_gb).filter(|d| *d > 0),
                wait: Some(true),
                // A provided Space joins the relay through its own driver
                // (a direct one is reached through the host's forward).
                spacesd: Some(true),
                // The caller cancels it by the machine it registered.
                create_id: (!direct).then(|| attach.machine_id.clone()),
                network,
                // Its driver's mesh node, so the caller reaches it directly
                // when it can (a direct Space has no relay to coordinate one).
                env: {
                    let mut env: std::collections::BTreeMap<String, String> =
                        req.env.clone().into_iter().collect();
                    if !direct {
                        env.extend(crate::mesh::provided_space_env());
                    }
                    env
                },
                gpu: Some(req.gpu.trim().to_string()).filter(|g| !g.is_empty()),
                headless: req.headless,
                // Enforced by this host, from the guest's first instruction.
                egress: egress_from_pb(req.egress.clone()),
                ..Default::default()
            };
            match self.spaces.create(create.clone()).await {
                // `auto` on a broken container engine (Docker failing its
                // own writes, a stopped engine): the next working runtime
                // here rather than the engine's error.
                Err(e)
                    if let Some(next) = engine_fallback(
                        kind,
                        &runtime,
                        macos,
                        &e,
                        &cua_sandbox_core::placement::HostFacts::current(),
                    ) =>
                {
                    tracing::warn!(
                        error = %e,
                        runtime = %next,
                        "the container engine failed; creating the Space on the next runtime"
                    );
                    (kind, runtime) = (next.builtin_kind().unwrap_or(Kind::Auto), next);
                    self.spaces
                        .create(SpaceCreate {
                            kind,
                            runtime: runtime.clone(),
                            ..create
                        })
                        .await
                        .map_err(|e2| {
                            Error::host(
                                "host spaces",
                                format!(
                                    "the container engine on {host_name} failed ({e}), and so did \
                                     {runtime} ({e2})"
                                ),
                            )
                        })
                }
                r => r,
            }
        };
        let info = match created {
            Ok(c) => match c.ready() {
                Ok(info) => info,
                Err(p) => {
                    return Err(self.refuse(
                        caller,
                        &label,
                        Error::invalid(format!("{} is still starting", p.id)),
                    ));
                }
            },
            Err(e) => {
                self.audit("failed", caller, &label, &format!("create {image}: {e}"));
                return Err(e);
            }
        };
        // Without the relay: a port on the host's direct address forwarded
        // to the new Space's cua-spacesd, and its token for the caller.
        if let Some(d) = direct_hosting {
            let opened = async {
                let token = self.space_token(&info.id).await?;
                let port = self.open_forward(&d, &info.id, 0).await?;
                Ok::<_, Error>((token, port))
            }
            .await;
            let (token, port) = match opened {
                Ok(v) => v,
                Err(e) => {
                    self.close_forward(&info.id);
                    let _ = self.spaces.delete(&info.id).await;
                    self.audit(
                        "failed",
                        caller,
                        &label,
                        &format!("forward a port to {}: {e}", info.id),
                    );
                    return Err(e);
                }
            };
            let record = ProvidedSpace {
                relay_machine: String::new(),
                local_space: info.id.clone(),
                name: info.name.clone(),
                image: image.clone(),
                os: if info.os.is_empty() {
                    os
                } else {
                    info.os.clone()
                },
                kind: info.kind.clone(),
                runtime: runtime.as_str().to_string(),
                created_by: caller.label(),
                created_by_account: caller.account.clone(),
                created_at_ms: now_ms(),
                machine_token: String::new(),
                relay_url: String::new(),
                direct_port: port,
                cpus: req.cpus,
                memory_mb: req.memory_mb,
                routed,
            };
            provided::update_provided(&self.host_dir(), |all| all.push(record.clone()))?;
            drop(slot);
            self.audit(
                "create",
                caller,
                &record.local_space,
                &format!(
                    "{image}, {}, {} on port {port}",
                    record.kind,
                    if routed { "routed" } else { "direct" }
                ),
            );
            let mut out = self.with_egress(to_pb(&record));
            out.direct_token = token;
            // What this host applied, so the caller can tell it from a
            // host that never read `network` ([`host_applied_network`]).
            out.network = network_word(network).into();
            return Ok(out);
        }
        // Attach the new Space's own driver to the relay machine the caller
        // registered (the second half of `relay_register`).
        let attached = async {
            let s = self.spaces.space(&info.id).await?;
            s.require(RELAY_ATTACH_FEATURE)?;
            s.spacesd()?
                .system()
                .attach_relay(pb::AttachRelayRequest {
                    relay_url: guest_relay_url(&attach.relay_url, &info.kind),
                    ..attach.clone()
                })
                .await
                .map_err(|e| Error::Env(cua_spacesd_client::Error::from(e)))?;
            Ok::<_, Error>(())
        }
        .await;
        if let Err(e) = attached {
            let e = too_old_for_the_relay(e, &image);
            let _ = self.spaces.delete(&info.id).await;
            self.audit(
                "failed",
                caller,
                &label,
                &format!("attach {} to the relay: {e}", info.id),
            );
            return Err(e);
        }
        let record = ProvidedSpace {
            relay_machine: attach.machine_id.clone(),
            local_space: info.id.clone(),
            name: info.name.clone(),
            image: image.clone(),
            os: if info.os.is_empty() {
                os
            } else {
                info.os.clone()
            },
            kind: info.kind.clone(),
            runtime: runtime.as_str().to_string(),
            created_by: caller.label(),
            created_by_account: caller.account.clone(),
            created_at_ms: now_ms(),
            machine_token: attach.machine_token.clone(),
            relay_url: attach.relay_url.clone(),
            direct_port: 0,
            cpus: req.cpus,
            memory_mb: req.memory_mb,
            routed: false,
        };
        provided::update_provided(&self.host_dir(), |all| all.push(record.clone()))?;
        drop(slot);
        self.audit(
            "create",
            caller,
            &record.relay_machine,
            &format!("{} ({image}, {})", record.local_space, record.kind),
        );
        Ok(self.with_egress(pb::HostSpace {
            network: network_word(network).into(),
            ..to_pb(&record)
        }))
    }

    /// `h` with where its egress policy is enforced here.
    fn with_egress(&self, mut h: pb::HostSpace) -> pb::HostSpace {
        h.egress_enforced = enforced_word(self.spaces.egress_enforced(&h.local_space));
        h
    }

    /// `SetHostSpaceEgress`: replaces the egress policy of a Space this
    /// host provides; this host enforces it outside the guest. The owner
    /// sets any; another account only on the ones it created.
    pub async fn set_egress(
        &self,
        caller: &HostCaller,
        space: &str,
        policy: Option<pb::HostEgressPolicy>,
    ) -> Result<pb::SetHostSpaceEgressResponse> {
        let space = space
            .trim()
            .strip_prefix("relay:")
            .unwrap_or(space.trim())
            .to_string();
        let Some(record) = provided::load_provided(&self.host_dir())?
            .into_iter()
            .find(|s| !space.is_empty() && (s.relay_machine == space || s.local_space == space))
        else {
            return Err(self.refuse(
                caller,
                &space,
                Error::NotFound(format!("{space} is not a Space this host provides")),
            ));
        };
        if !caller.is_owner() && record.created_by_account != caller.account {
            return Err(self.refuse(
                caller,
                &space,
                Error::Relay(cua_host::Error::PermissionDenied(format!(
                    "{space} was created by someone else; only they or the host's owner can \
                     change its egress policy"
                ))),
            ));
        }
        let policy = egress_from_pb(policy);
        let subject = if record.is_direct() {
            record.local_space.clone()
        } else {
            record.relay_machine.clone()
        };
        if let Err(e) = self.spaces.set_egress(&record.local_space, &policy).await {
            self.audit("failed", caller, &subject, &format!("egress: {e}"));
            return Err(e);
        }
        self.audit(
            "egress",
            caller,
            &subject,
            &format!(
                "{}: allow {:?}, deny {:?}",
                record.local_space, policy.allow, policy.deny
            ),
        );
        Ok(pb::SetHostSpaceEgressResponse {
            enforced: enforced_word(self.spaces.egress_enforced(&record.local_space)),
        })
    }

    /// `CancelHostSpace`: stops a create a caller started here (by the
    /// relay machine it registered), removing what it made; a Space that
    /// already finished is deleted as `DeleteHostSpace` would. Idempotent.
    pub async fn cancel(&self, caller: &HostCaller, space: &str) -> Result<String> {
        let machine = space
            .trim()
            .strip_prefix("relay:")
            .unwrap_or(space.trim())
            .to_string();
        if !caller.may_create() {
            return Err(self.refuse(
                caller,
                &machine,
                Error::Relay(cua_host::Error::PermissionDenied(
                    "only the owner and the accounts this computer is shared with (as editors) can cancel creates on it"
                        .into(),
                )),
            ));
        }
        let outcome = self.spaces.cancel_create(&machine).await?;
        if outcome.state == crate::CancelState::AlreadyCreated {
            // The Space was made before the cancel reached it, but this
            // host may still be recording it (a port forward, its record):
            // wait for that, so the Space is found and deleted below.
            let deadline = tokio::time::Instant::now() + CANCEL_RECORD_WAIT;
            while self.creating.running(&machine) && tokio::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
        // A finished create is found by the relay machine the caller
        // registered, or, for a direct Space (no relay machine), by the
        // Space the create key or name resolved to.
        let finished = provided::load_provided(&self.host_dir())?
            .into_iter()
            .find(|s| {
                (!s.relay_machine.is_empty() && s.relay_machine == machine)
                    || s.local_space == machine
                    || (!outcome.id.is_empty() && s.local_space == outcome.id)
            });
        if let Some(record) = finished {
            // It finished before the cancel reached it.
            let space = if record.relay_machine.is_empty() {
                record.local_space
            } else {
                record.relay_machine
            };
            return self.delete(caller, &space).await;
        }
        self.audit("cancel", caller, &machine, &outcome.message);
        Ok(outcome.message)
    }

    /// `DeleteCloudSpace`: deletes a Space in the owner's own cloud that
    /// this machine created (by the relay machine it joined as): its cloud
    /// resources through this machine's ownership records, and its relay
    /// machine. Only the owner; audited.
    pub async fn delete_cloud(&self, caller: &HostCaller, space: &str) -> Result<String> {
        let machine = space
            .trim()
            .strip_prefix("relay:")
            .unwrap_or(space.trim())
            .to_string();
        // Only the owner: the cloud account is theirs, and it costs money.
        if !caller.is_owner() {
            return Err(self.refuse(
                caller,
                &machine,
                Error::Relay(cua_host::Error::PermissionDenied(
                    "only the owner can delete a Space in their own cloud".into(),
                )),
            ));
        }
        let id = format!("relay:{machine}");
        if self.spaces.cloud_sandbox_of(&id)?.is_none() {
            return Err(self.refuse(
                caller,
                &machine,
                Error::NotFound(format!(
                    "{id} is not a Space in your cloud that this computer created"
                )),
            ));
        }
        match self.spaces.delete(&id).await {
            Ok(message) => {
                self.audit("delete-cloud", caller, &machine, &message);
                Ok(message)
            }
            Err(e) => {
                self.audit("failed", caller, &machine, &format!("delete-cloud: {e}"));
                Err(e)
            }
        }
    }

    /// `DeleteHostSpace`: deletes a Space this host provides (its sandbox
    /// and its relay machine). The owner deletes any; another account only
    /// the ones it created.
    pub async fn delete(&self, caller: &HostCaller, space: &str) -> Result<String> {
        let space = space
            .trim()
            .strip_prefix("relay:")
            .unwrap_or(space.trim())
            .to_string();
        let all = provided::load_provided(&self.host_dir())?;
        let Some(i) = all.iter().position(|s| {
            !space.is_empty() && (s.relay_machine == space || s.local_space == space)
        }) else {
            return Err(self.refuse(
                caller,
                &space,
                Error::NotFound(format!("{space} is not a Space this host provides")),
            ));
        };
        let record = all[i].clone();
        if !caller.is_owner() && record.created_by_account != caller.account {
            return Err(self.refuse(
                caller,
                &space,
                Error::Relay(cua_host::Error::PermissionDenied(format!(
                    "{space} was created by someone else; only they or the host's owner can delete it"
                ))),
            ));
        }
        // Recorded as this caller's delete first, so the local delete below
        // (which forgets any provided Space it removes) finds nothing left.
        provided::update_provided(&self.host_dir(), |all| {
            all.retain(|s| s.local_space != record.local_space)
        })?;
        self.close_forward(&record.local_space);
        match self.spaces.delete(&record.local_space).await {
            Ok(_) | Err(Error::NotFound(_)) => {}
            Err(e) => {
                // Still running: keep providing it.
                let _ = provided::update_provided(&self.host_dir(), |now| now.push(record.clone()));
                self.audit("failed", caller, &space, &format!("delete: {e}"));
                return Err(e);
            }
        }
        // The Space's machine token removes its relay machine (best effort:
        // the client removes it with the account too).
        if !record.machine_token.is_empty()
            && !record.relay_url.is_empty()
            && let Ok(client) = RelayClient::new(&record.relay_url)
        {
            let _ = client
                .delete(&record.machine_token, &record.relay_machine)
                .await;
        }
        if record.is_direct() {
            self.audit(
                "delete",
                caller,
                &record.local_space,
                &format!("direct on port {}", record.direct_port),
            );
            return Ok(format!(
                "Deleted {} on {} (it was forwarded on port {}).",
                record.local_space,
                self.host_name(),
                record.direct_port
            ));
        }
        self.audit("delete", caller, &record.relay_machine, &record.local_space);
        Ok(format!(
            "Deleted relay:{} ({} on {}).",
            record.relay_machine,
            record.local_space,
            self.host_name()
        ))
    }

    /// `SetHostSpacePower`: turns a Space this host provides off or on.
    /// Off stops it with its disk kept, so it frees the host's memory; on
    /// boots it again and attaches it to its relay machine
    /// again, with the machine token the host keeps. The owner powers any;
    /// another account only the ones it created.
    pub async fn set_power(
        &self,
        caller: &HostCaller,
        space: &str,
        on: bool,
    ) -> Result<pb::SetHostSpacePowerResponse> {
        let space = space
            .trim()
            .strip_prefix("relay:")
            .unwrap_or(space.trim())
            .to_string();
        let Some(record) = provided::load_provided(&self.host_dir())?
            .into_iter()
            .find(|s| !space.is_empty() && (s.relay_machine == space || s.local_space == space))
        else {
            return Err(self.refuse(
                caller,
                &space,
                Error::NotFound(format!("{space} is not a Space this host provides")),
            ));
        };
        if !caller.is_owner() && record.created_by_account != caller.account {
            return Err(self.refuse(
                caller,
                &space,
                Error::Relay(cua_host::Error::PermissionDenied(format!(
                    "{space} was created by someone else; only they or the host's owner can turn it off or on"
                ))),
            ));
        }
        let id = SpaceId::parse(&record.local_space)?;
        let SpaceId::Local { name } = &id else {
            return Err(Error::invalid(format!(
                "{} is not a local Space",
                record.local_space
            )));
        };
        let sandboxes = self.spaces.sandboxes();
        if sandboxes.power_control(name).is_none() {
            // The core's refusal names the runtime that cannot.
            let e = sandboxes
                .power_off(name)
                .await
                .err()
                .map(Error::from)
                .unwrap_or_else(|| Error::invalid("no power control"));
            return Err(self.refuse(caller, &space, e));
        }
        let control = cua_sandbox_core::PowerControl::Stop;
        self.spaces.drop_connection(&id).await;
        let label = record.label();
        // What the audit names it by: its relay machine, or for a direct
        // Space its id on this host.
        let subject = if record.is_direct() {
            record.local_space.clone()
        } else {
            record.relay_machine.clone()
        };
        let state = if on {
            let started = async {
                sandboxes.power_on(name).await?;
                if record.is_direct() {
                    // Reached through this host's forward again (each
                    // connection finds the Space's new address).
                    self.ensure_forward(&record).await
                } else {
                    self.reattach(&record).await
                }
            }
            .await;
            if let Err(e) = started {
                self.audit("failed", caller, &subject, &format!("start: {e}"));
                return Err(e);
            }
            cua_sandbox_core::PowerState::Running
        } else {
            match sandboxes.power_off_as(name, control).await {
                Ok(state) => state,
                Err(e) => {
                    let e = Error::from(e);
                    self.audit("failed", caller, &subject, &format!("stop: {e}"));
                    return Err(e);
                }
            }
        };
        let report = crate::SpacePower::new(&label, control, state);
        self.audit(
            if on { "start" } else { "stop" },
            caller,
            &subject,
            &record.local_space,
        );
        Ok(pb::SetHostSpacePowerResponse {
            state: report.state,
            power: report.power,
            message: format!(
                "{} ({} on {})",
                report.message.trim_end_matches('.'),
                record.local_space,
                self.host_name()
            ),
        })
    }

    /// The forwarded port of a direct Space this host provides, opened
    /// again on its recorded port when it is not open.
    async fn ensure_forward(&self, record: &ProvidedSpace) -> Result<()> {
        if self.forward_open(&record.local_space) {
            return Ok(());
        }
        let direct = if record.routed {
            routed_hosting()
        } else {
            self.policy()?.direct.ok_or_else(|| {
                Error::host(
                    "host spaces",
                    format!(
                        "{} is a direct Space, but this host is no longer set up with --direct; delete it",
                        record.local_space
                    ),
                )
            })?
        };
        self.open_forward(&direct, &record.local_space, record.direct_port)
            .await
            .map(|_| ())
    }

    /// Attaches a provided Space that booted again to its relay machine:
    /// its driver forgot the attachment when it stopped. The relay's keys
    /// and the machine's owner are read again (the keys may have rotated).
    async fn reattach(&self, record: &ProvidedSpace) -> Result<()> {
        if record.machine_token.is_empty() || record.relay_url.is_empty() {
            return Err(Error::invalid(format!(
                "relay:{} has no computer token on this host, so it cannot rejoin the relay; delete it and create a new one",
                record.relay_machine
            )));
        }
        let client = RelayClient::new(&record.relay_url)?;
        let jwks = client.jwks().await?;
        let machine = client
            .machine(&record.machine_token, &record.relay_machine)
            .await?;
        let s = self.spaces.space(&record.local_space).await?;
        s.require(RELAY_ATTACH_FEATURE)?;
        s.spacesd()?
            .system()
            .attach_relay(pb::AttachRelayRequest {
                relay_url: guest_relay_url(&record.relay_url, &record.kind),
                machine_token: record.machine_token.clone(),
                machine_id: record.relay_machine.clone(),
                relay_jwks_json: jwks.to_string(),
                owner: machine.owner.id.clone(),
                owner_email: machine.owner.email.clone().unwrap_or_default(),
            })
            .await
            .map_err(|e| Error::Env(cua_spacesd_client::Error::from(e)))?;
        Ok(())
    }
}

/// Where a routed Space's forward listens: this host's loopback (its own
/// channel reaches it; nothing else does).
fn routed_hosting() -> cua_host::DirectHosting {
    cua_host::DirectHosting {
        listen: "127.0.0.1:0".into(),
        allow_any_address: false,
    }
}

/// Accepts connections on a direct Space's forwarded port and splices each
/// to the Space's cua-spacesd, looked up per connection.
async fn forward_loop(
    listener: tokio::net::TcpListener,
    sandboxes: cua_sandbox_core::Sandboxes,
    name: String,
    allow_any_address: bool,
) {
    loop {
        let (mut inbound, peer) = match listener.accept().await {
            Ok(pair) => pair,
            Err(e) => {
                tracing::debug!(error = %e, space = %name, "direct Space forward: accept failed");
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
        };
        if !allow_any_address && !cua_host::direct::is_private_address(peer.ip()) {
            tracing::warn!(
                %peer,
                space = %name,
                "refused a connection to a direct Space from outside loopback, Tailscale and the LAN"
            );
            continue;
        }
        let sandboxes = sandboxes.clone();
        let name = name.clone();
        tokio::spawn(async move {
            let target = match spacesd_target(&sandboxes, &name).await {
                Ok(t) => t,
                Err(e) => {
                    tracing::debug!(error = %e, space = %name, "direct Space forward: no target");
                    return;
                }
            };
            if let Ok(mut outbound) = tokio::net::TcpStream::connect(&target).await {
                let _ = inbound.set_nodelay(true);
                let _ = outbound.set_nodelay(true);
                let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
            }
        });
    }
}

/// Where this host reaches the cua-spacesd of its local Space `name` now.
async fn spacesd_target(sandboxes: &cua_sandbox_core::Sandboxes, name: &str) -> Result<String> {
    let sandbox = sandboxes.connect(name).await?;
    let port = sandbox
        .services()
        .get("env")
        .copied()
        .filter(|p| *p != 0)
        .unwrap_or(cua_proto::SPACESD_DEFAULT_PORT);
    match sandbox.port(port)? {
        cua_sandbox_core::PortTarget::Addr { host, port } => Ok(crate::id::authority(&host, port)),
        cua_sandbox_core::PortTarget::Url(url) => Err(Error::invalid(format!(
            "{name}'s cua-spacesd is at {url}, not a TCP address this host can forward"
        ))),
    }
}

/// After `space` (a `local:` id) was deleted on this machine: when it was a
/// Space this machine provided, its relay machine is removed (with the
/// Space's own machine token) and it leaves the host's list, audited as a
/// local delete. Best effort; nothing when it was not provided.
/// Asks host `host` (a relay machine id) to cancel the Space it is
/// creating for relay machine `machine` ([`HostSpacesServer::cancel`]).
pub(crate) async fn cancel_on_host(spaces: &Spaces, host: &str, machine: &str) -> Result<String> {
    let relay = spaces.relay()?;
    let token = relay.tokens.access_token().await?;
    let client = relay.client().await?;
    let row = client.machine(&token, host).await?;
    let space = spaces.host_space(&row).await?;
    space
        .spacesd()?
        .host_spaces()
        .cancel_host_space(pb::CancelHostSpaceRequest {
            space: machine.to_string(),
        })
        .await
        .map(|r| r.into_inner().message)
        .map_err(|e| from_host(cua_spacesd_client::Error::from(e)))
}

pub(crate) async fn forget_provided(spaces: &Spaces, space: &str) {
    let dir = Host::new(spaces.home_dir()).paths().dir.clone();
    let removed = provided::update_provided(&dir, |all| {
        let i = all.iter().position(|s| s.local_space == space)?;
        Some(all.remove(i))
    });
    let record = match removed {
        Ok(Some(r)) => r,
        Ok(None) => return,
        Err(e) => {
            tracing::warn!("provided spaces: {e}");
            return;
        }
    };
    if !record.machine_token.is_empty()
        && let Ok(client) = RelayClient::new(&record.relay_url)
    {
        let _ = client
            .delete(&record.machine_token, &record.relay_machine)
            .await;
    }
    // A direct Space has no relay machine: the audit names it by its id on this host.
    let subject = if record.relay_machine.is_empty() {
        space
    } else {
        &record.relay_machine
    };
    let _ = provided::audit(&dir, "delete", "local", subject, space);
}

/// A Space whose cua-spacesd predates `relay_attach` cannot join the relay,
/// so nothing off this machine can reach it: say that, and that the image
/// (not the caller) has to change. The catalog and the image labels do not
/// say which cua-spacesd an image carries, so this is known only once the
/// Space is up.
fn too_old_for_the_relay(e: Error, image: &str) -> Error {
    match e {
        Error::CapabilityMissing {
            space,
            feature,
            limitation,
        } if feature == RELAY_ATTACH_FEATURE => {
            let why = if limitation.is_empty() {
                String::new()
            } else {
                format!(" ({limitation})")
            };
            Error::CapabilityMissing {
                space,
                feature,
                limitation: format!(
                    "this image's cua-spacesd is too old to be reached from your other devices{why}; \
                     {image} needs republishing with a current cua-spacesd. Update the image (or \
                     choose another one) and create the Space again"
                ),
            }
        }
        e => e,
    }
}

/// A machine whose owner stopped sharing it ("Stop sharing" in the app,
/// `cua host stop`): the relay refuses every call to it until they resume.
pub(crate) fn stopped_sharing(name: &str) -> Error {
    Error::Relay(cua_host::Error::PermissionDenied(format!(
        "{name} stopped sharing: ask its owner to Resume sharing (or run `cua host start` there)"
    )))
}

/// Whether a failed connection is a machine that stopped sharing refusing
/// it: the relay ("the host stopped sharing this machine", its directory
/// says so) or the machine itself ("relay assertion refused: this machine
/// stopped sharing", its own policy says so while the relay still lists it
/// as sharing, as after a set up again).
pub(crate) fn is_stopped_sharing(reason: &str) -> bool {
    reason.contains("stopped sharing")
}

/// [`HostLimit::resource`] of a host the relay sees connected whose owner
/// stopped sharing it (it refuses every call). Not a count: `used` and
/// `limit` are 0 and `reason` is the line to show. It travels with the
/// limits because those are the one open list [`Spaces::hosts`] hands to
/// every shell (the daemon, the SDK's bindings, the apps' FFI) without
/// changing their shapes; the app core's wizard reads it
/// (`HOST_SHARING_STOPPED`).
pub const STOPPED_SHARING: &str = "sharing";

/// What a refusal by a machine that stopped sharing said, from the error
/// a call to it failed with.
fn stopped_sharing_reason(e: &Error) -> Option<String> {
    match e {
        Error::Relay(cua_host::Error::PermissionDenied(m)) if is_stopped_sharing(m) => {
            Some(m.clone())
        }
        _ => None,
    }
}

/// What the Spaces this host provides reserve, for the job runner's
/// capacity: their recorded size is not kept, so each counts as a default
/// local Space (2 vCPUs, 4 GiB).
/// How this machine's daemon reaches the machine's own cua-spacesd (the
/// host service): where it listens (the direct listener, `CUA_ENV_LISTEN`,
/// else loopback on `CUA_ENV_PORT` or the channel's port, 3211 or 3221 for
/// Cua Spaces Local) and its local env token.
fn host_spacesd(host: &Host) -> std::result::Result<cua_cluster::machine::HostSpacesd, String> {
    host_spacesd_on(host, cua_home::channel::CuaChannel::current())
}

fn host_spacesd_on(
    host: &Host,
    channel: cua_home::channel::CuaChannel,
) -> std::result::Result<cua_cluster::machine::HostSpacesd, String> {
    let listen = host
        .config()
        .ok()
        .flatten()
        .and_then(|c| c.listen)
        .filter(|l| !l.is_empty())
        .or_else(|| {
            std::env::var("CUA_ENV_LISTEN")
                .ok()
                .filter(|l| !l.is_empty())
        })
        .unwrap_or_else(|| {
            let port = std::env::var("CUA_ENV_PORT")
                .ok()
                .and_then(|p| p.trim().parse::<u16>().ok())
                .unwrap_or(channel.ports().tcp);
            format!("127.0.0.1:{port}")
        });
    // A listener on every interface is reached on loopback.
    let listen = listen
        .replace("0.0.0.0:", "127.0.0.1:")
        .replace("[::]:", "127.0.0.1:");
    let path = host.paths().env_token();
    let token = std::fs::read_to_string(&path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .trim()
        .to_string();
    Ok(cua_cluster::machine::HostSpacesd {
        url: format!("http://{listen}"),
        token,
    })
}

/// What `cand` is short of for `need`, in the Space's own CPU and memory
/// (what [`Spaces::reclaim`] frees), or `None` when it is short of neither.
fn shortfall_of(
    cand: &cua_cluster::placement::Candidate,
    need: &cua_cluster::placement::Need,
) -> Option<cua_sandbox_core::Shortfall> {
    let cpu = (i64::from(need.millicpus) - cand.free_millicpus()).max(0) as u32;
    let mem = (need.memory_mb as i64 - cand.free_memory_mb()).max(0) as u64;
    (cpu > 0 || mem > 0).then_some(cua_sandbox_core::Shortfall {
        millicpus: cpu,
        memory_mb: mem,
    })
}

fn provided_reservation(host_dir: &std::path::Path) -> cua_cluster::External {
    let mut ext = cua_cluster::External::default();
    let vms_measured = cua_cluster::sysinfo::vm_overcommit().is_some();
    for s in provided::load_provided(host_dir).unwrap_or_default() {
        // What the Space was admitted with (a default Space when the record
        // does not say), so many small Spaces (forks of a world) are not
        // counted as many large ones.
        let cpu_m = if s.cpus == 0 {
            2000
        } else {
            s.cpus.saturating_mul(1000)
        };
        let mem_mb = if s.memory_mb == 0 { 4096 } else { s.memory_mb };
        let kind = if s.kind.is_empty() {
            "container".to_string()
        } else {
            s.kind.clone()
        };
        if kind == "vm" && vms_measured {
            // A VM's vCPUs share host CPUs by the VM overcommit; its memory
            // is measured (`HostMeasured.vm_memory_free_mb`).
            ext.millicpus += cua_cluster::sysinfo::vm_millicpus(cpu_m);
        } else if kind == "container" {
            ext.millicpus = ext
                .millicpus
                .saturating_add(cua_cluster::sysinfo::container_millicpus(cpu_m));
            ext.memory_mb += mem_mb;
        } else {
            ext.millicpus = ext.millicpus.saturating_add(cpu_m);
            ext.memory_mb += mem_mb;
        }
        *ext.running.entry(kind).or_default() += 1;
    }
    ext
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// A `tonic::Status` carrying a `cua.env.v1.ErrorInfo` for `e`, so the
/// client decodes the same kind (the daemon's `HostSpacesService`).
pub fn to_status(e: &Error) -> tonic::Status {
    use prost::Message as _;
    let (code, reason) = match e.tag() {
        "invalid_argument" | "invalid_placement" => {
            (tonic::Code::InvalidArgument, pb::ErrorReason::Unspecified)
        }
        "not_found" => (tonic::Code::NotFound, pb::ErrorReason::Unspecified),
        "wrong_provider" => (tonic::Code::Unimplemented, pb::ErrorReason::Unspecified),
        "permission_denied" => (
            tonic::Code::PermissionDenied,
            pb::ErrorReason::PermissionDenied,
        ),
        "limit_exceeded" | "host_capacity" => (
            tonic::Code::ResourceExhausted,
            pb::ErrorReason::LimitExceeded,
        ),
        "host_capability_missing" | "capability_missing" => (
            tonic::Code::FailedPrecondition,
            pb::ErrorReason::FeatureUnsupported,
        ),
        "unauthenticated" => (
            tonic::Code::Unauthenticated,
            pb::ErrorReason::Unauthenticated,
        ),
        "timeout" => (tonic::Code::DeadlineExceeded, pb::ErrorReason::Unspecified),
        _ => (tonic::Code::Internal, pb::ErrorReason::Internal),
    };
    let message = e.to_string();
    let info = pb::ErrorInfo {
        reason: reason as i32,
        message: message.clone(),
        feature: String::new(),
        metadata: [("kind".to_string(), e.tag().to_string())].into(),
    };
    let rpc = cua_spacesd_client::error::RpcStatus {
        code: code as i32,
        message: message.clone(),
        details: vec![cua_proto::wkt::Any {
            type_url: "type.googleapis.com/cua.env.v1.ErrorInfo".into(),
            value: info.encode_to_vec().into(),
        }],
    };
    tonic::Status::with_details(code, message, rpc.encode_to_vec().into())
}

/// Checks that a host applied the guest network it was asked for:
/// `applied` is what its `CreateHostSpace` response says
/// (`HostSpace.network`). A host whose cua predates the field never reads
/// `network`, creates the Space with outbound network and says nothing, so
/// `none` without that answer is refused (the caller deletes the Space).
pub(crate) fn host_applied_network(
    asked: cua_sandbox_core::NetworkMode,
    applied: &str,
    host: &str,
) -> Result<()> {
    if asked == cua_sandbox_core::NetworkMode::Default || applied == network_word(asked) {
        return Ok(());
    }
    Err(Error::CapabilityMissing {
        space: host.to_string(),
        feature: "network none".into(),
        limitation: "this machine's cua is too old to enforce it (the Space would have \
                     outbound network), so the Space it made was deleted; update cua on it"
            .into(),
    })
}

/// A host's error, as this crate's kind (the `kind` the host put in the
/// error's metadata, else the spacesd error).
/// A host's error for `SetHostSpaceEgress`: a host whose cua predates it
/// says so.
fn from_host_egress(e: cua_spacesd_client::Error, host: &str) -> Error {
    let unimplemented = match &e {
        cua_spacesd_client::Error::FeatureUnsupported { .. } => true,
        other => cua_spacesd_client::old_host::is_unknown_rpc(other),
    };
    if unimplemented {
        return crate::egress::refused(
            host,
            "this machine's cua does not enforce egress policies yet: update cua on it",
        );
    }
    from_host(e)
}

/// Checks that a host enforces a policy it was given (`enforced`): one
/// whose cua predates egress policies ignores them, which must never pass
/// for enforcement.
pub(crate) fn host_enforces(
    enforced: &str,
    policy: &crate::egress::EgressPolicy,
    host: &str,
) -> Result<()> {
    if policy.is_empty() || enforced == crate::egress::Enforcement::Host.as_str() {
        return Ok(());
    }
    Err(crate::egress::refused(
        host,
        "this machine's cua does not enforce egress policies yet: update cua on it",
    ))
}

/// Refuses `what` (for example "checkpoints of Spaces") on host `host`
/// when its `space` (a connection to it) reports a protocol revision from
/// before the RPC existed. Called before every RPC that Cua 0.8 added to a
/// host (see [`cua_proto::HOST_SERVICES_MIN_REVISION`]): an older host's
/// access layer rejects them as "unauthenticated".
pub(crate) fn guard_host_rpc(space: &crate::Space, host: &str, what: &str) -> Result<()> {
    match cua_spacesd_client::old_host::too_old(space.capabilities()) {
        Some(old) => Err(Error::HostTooOld(cua_spacesd_client::old_host::message(
            host,
            Some(&old),
            what,
        ))),
        None => Ok(()),
    }
}

/// [`from_host`] for the answer to an RPC that Cua 0.8 added: a host that
/// answers it as unknown (`unimplemented`, or `unauthenticated` from an
/// access layer that rejects unknown methods) runs an older Cua.
pub(crate) fn from_new_host_rpc(
    e: impl Into<cua_spacesd_client::Error>,
    host: &str,
    what: &str,
) -> Error {
    let e = e.into();
    if cua_spacesd_client::old_host::is_unknown_rpc(&e) {
        return Error::HostTooOld(cua_spacesd_client::old_host::message(host, None, what));
    }
    from_host(e)
}

pub(crate) fn from_host(e: cua_spacesd_client::Error) -> Error {
    use cua_spacesd_client::Error as E;
    let kind = match &e {
        E::LimitExceeded(d)
        | E::PermissionDenied(d)
        | E::Internal(d)
        | E::Rpc(d)
        | E::NotInitialized(d) => d.metadata.get("kind").cloned(),
        E::FeatureUnsupported { details, .. } => details.metadata.get("kind").cloned(),
        _ => None,
    };
    let msg = match &e {
        E::LimitExceeded(d) | E::PermissionDenied(d) | E::Internal(d) | E::Rpc(d) => {
            d.message.clone()
        }
        E::FeatureUnsupported { details, .. } => details.message.clone(),
        other => other.to_string(),
    };
    match (kind.as_deref(), &e) {
        (Some("host_capacity"), _) => Error::Sandbox(cua_sandbox_core::Error::Runtime(
            cua_sandbox_core::RuntimeError::HostCapacity(msg),
        )),
        (Some("limit_exceeded"), _) | (_, E::LimitExceeded(_)) => Error::LimitExceeded(msg),
        (Some("permission_denied"), _) | (_, E::PermissionDenied(_)) => {
            Error::Relay(cua_host::Error::PermissionDenied(msg))
        }
        (Some("host_capability_missing"), _) | (_, E::FeatureUnsupported { .. }) => {
            Error::host("host spaces", msg)
        }
        (Some("not_found"), _) => Error::NotFound(msg),
        (Some("invalid_argument" | "invalid_placement"), _) => Error::invalid(msg),
        _ => Error::Env(e),
    }
}

/// The kind and runtime a Space created on this host runs with, from what
/// the caller asked (`kind`, `runtime`) and this host's own configuration.
///
/// - An explicit built-in runtime must run here ([`placement::select`] on
///   this host's facts); otherwise the refusal names the runtimes it has.
/// - `auto` takes this host's `default.runtime` (`cua config set
///   default.runtime`) when it fits the kind and runs here: the caller's
///   machine knows nothing about which engine works on this one. A macOS
///   guest keeps its own rule (Lume).
///
/// [`placement::select`]: cua_sandbox_core::placement::select
pub(crate) fn host_placement(
    kind: Kind,
    runtime: Runtime,
    macos_guest: bool,
    settings: Option<&cua_sandbox_core::settings::Settings>,
    host: &cua_sandbox_core::placement::HostFacts,
) -> std::result::Result<(Kind, Runtime), cua_sandbox_core::placement::PlacementError> {
    use cua_sandbox_core::placement::{ImageFacts, select};
    let image = ImageFacts {
        os: macos_guest.then(|| "macos".to_string()),
        variants: vec![],
    };
    if runtime != Runtime::Auto {
        if runtime.builtin_kind().is_some() {
            select(&On::Local, kind, &runtime, &image, host).map_err(|mut e| {
                e.message = format!("this host cannot run {runtime}: {}", e.message);
                e
            })?;
        }
        return Ok((kind, runtime));
    }
    if macos_guest {
        return Ok((kind, runtime));
    }
    let Some(resolved) = settings.and_then(|s| {
        s.resolve(Some(On::Local), (kind != Kind::Auto).then_some(kind), None)
            .ok()
    }) else {
        return Ok((kind, runtime));
    };
    if resolved.runtime == Runtime::Auto
        || select(&On::Local, kind, &resolved.runtime, &image, host).is_err()
    {
        return Ok((kind, runtime));
    }
    Ok((kind, resolved.runtime))
}

/// The runtime to try when an `auto` create failed on a broken container
/// engine, or `None` to keep the error: a microVM runtime this host has
/// (cua's own monitor, Apple's Containerization, Firecracker). Only for
/// `auto` (an explicit kind or runtime is kept), never for a macOS guest,
/// and never for an image fault (no such image, a refused pull).
pub(crate) fn engine_fallback(
    kind: Kind,
    runtime: &Runtime,
    macos_guest: bool,
    err: &Error,
    host: &cua_sandbox_core::placement::HostFacts,
) -> Option<Runtime> {
    if kind != Kind::Auto || *runtime != Runtime::Auto || macos_guest || !engine_broken(err) {
        return None;
    }
    Kind::parse("microvm").ok()?;
    [
        (host.vz, Runtime::Vz),
        (host.apple_containers, Runtime::Containerization),
        (host.kvm, Runtime::Firecracker),
    ]
    .into_iter()
    .find_map(|(has, r)| has.then_some(r))
}

/// Whether `err` says the container engine itself failed (an I/O error in
/// its store, a 5xx, a stopped daemon), not the image.
fn engine_broken(err: &Error) -> bool {
    let m = err.to_string().to_ascii_lowercase();
    let engine = m.contains("container engine") || m.contains("docker");
    let image_fault = [
        "manifest unknown",
        "pull access denied",
        "toomanyrequests",
        "unauthorized",
        "no such image",
        "invalid reference",
        "not found: manifest",
    ]
    .iter()
    .any(|f| m.contains(f));
    engine && !image_fault
}

/// A machine that provides Spaces, as [`Spaces::resolve_host`] found it.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)] // resolved once per call, never stored in bulk
pub enum ResolvedHost {
    /// One of the account's machines on the relay.
    Relay(crate::relay::RelayMachine),
    /// A machine added with `cua spaces add <addr> --host`, reached at its
    /// direct (Tailscale or LAN) address without the relay.
    Direct(DirectHost),
}

impl ResolvedHost {
    /// `relay` or `direct`: how the host is reached.
    pub fn via(&self) -> &'static str {
        match self {
            Self::Relay(_) => "relay",
            Self::Direct(_) => "direct",
        }
    }

    /// Its display name (a relay machine without one: its id).
    pub fn name(&self) -> String {
        match self {
            Self::Relay(m) if m.name.is_empty() => m.id.clone(),
            Self::Relay(m) => m.name.clone(),
            Self::Direct(h) => h.name.clone(),
        }
    }
}

/// How a direct host takes part in matching: its name is its id.
fn direct_as_machine(h: &DirectHost) -> cua_host::Machine {
    cua_host::Machine {
        id: h.name.clone(),
        name: h.name.clone(),
        online: true,
        ..Default::default()
    }
}

fn ambiguous_host(query: &str, choices: &[cua_host::Machine]) -> Error {
    Error::AmbiguousHost {
        query: query.to_string(),
        choices: choices.iter().map(|m| format!("host:{}", m.id)).collect(),
        message: format!(
            "{query:?} matches several of your computers: {}; say which (on=\"host:<id>\")",
            provided::describe_choices(choices)
        ),
    }
}

/// How long [`Spaces::hosts`] waits for each machine to answer.
const HOST_PROBE: Duration = Duration::from_secs(4);

/// One limit of a host and how much of it is used (`HostSpacesCapacity`).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct HostLimit {
    /// `spaces` (every Space it provides), `macos_vms`, or
    /// [`STOPPED_SHARING`] (not a limit: its owner stopped sharing it).
    pub resource: String,
    /// In use now.
    pub used: u32,
    /// The limit (0: none).
    pub limit: u32,
    /// Why the limit exists, for people.
    pub reason: String,
}

/// One of your machines that provides Spaces, as [`Spaces::hosts`] lists
/// it: what `on="host:<id>"` creates on.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct HostOffer {
    /// What `on="host:<id>"` takes: the relay machine id, or the name a
    /// direct host was added as.
    pub id: String,
    /// Its name.
    pub name: String,
    /// `relay` or `direct`.
    pub via: String,
    /// It answered, or the relay sees it connected (as the Machines page
    /// says it is online).
    pub online: bool,
    /// Its operating system (`macos`, `linux`, `windows`), when it answered.
    pub os: String,
    /// Its limits, when it answered; a [`STOPPED_SHARING`] entry when it
    /// refused because its owner stopped sharing it.
    pub limits: Vec<HostLimit>,
    /// Its capacity and what it creates, when it answered and reports
    /// them (what `on="auto"` places by).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node: Option<HostCapacity>,
}

/// What a host offers now ([`pb::HostNodeStatus`], abridged).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct HostCapacity {
    /// Kinds it creates (`container`, `vm`, `microvm`).
    pub kinds: Vec<String>,
    /// Runtimes it offers.
    pub runtimes: Vec<String>,
    /// CPU it offers, in thousandths.
    pub millicpus_total: u32,
    /// Of that, free.
    pub millicpus_free: u32,
    /// Memory it offers (MiB).
    pub memory_total_mb: u64,
    /// Of that, free.
    pub memory_free_mb: u64,
    /// Spaces running there by kind.
    pub running: std::collections::BTreeMap<String, u32>,
    /// Why it takes nothing now (empty: it does).
    pub problem: String,
    /// What the OS says is available (MiB; 0: unknown).
    #[serde(default)]
    pub memory_available_mb: u64,
    /// 0 normal, 1 warn, 2 critical.
    #[serde(default)]
    pub memory_pressure: u32,
    /// vCPUs per host CPU for `vm` Spaces, in hundredths (800: 8x).
    #[serde(default)]
    pub vcpu_overcommit_pct: u32,
    /// The VM fields below are measured.
    #[serde(default)]
    pub vm_memory_known: bool,
    /// VM memory it takes now (MiB), snapshot memory shared once.
    #[serde(default)]
    pub vm_memory_free_mb: u64,
    /// What a running VM there uses (MiB; 0: none runs).
    #[serde(default)]
    pub vm_typical_mb: u64,
    /// Checkpoints it holds (name to manifest digest; empty when never
    /// exported).
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub snapshots: std::collections::BTreeMap<String, String>,
    /// Checkpoint ABIs it restores.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub snapshot_abis: Vec<String>,
    /// CPU architecture (`arm64`, `amd64`; empty: not reported).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub arch: String,
    /// Accelerators.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accelerators: Vec<String>,
    /// It is draining (takes no new Spaces).
    #[serde(default)]
    pub draining: bool,
}

impl HostCapacity {
    fn from_pb(n: &pb::HostNodeStatus) -> Self {
        let m = n.measured.unwrap_or_default();
        Self {
            kinds: n.kinds.clone(),
            runtimes: n.runtimes.clone(),
            millicpus_total: n.millicpus_total,
            millicpus_free: n.millicpus_free,
            memory_total_mb: n.memory_total_mb,
            memory_free_mb: n.memory_free_mb,
            running: n.running.iter().map(|(k, v)| (k.clone(), *v)).collect(),
            problem: n.problem.clone(),
            memory_available_mb: m.memory_available_mb,
            memory_pressure: m.memory_pressure,
            vcpu_overcommit_pct: (m.vcpu_overcommit.max(0.0) * 100.0).round() as u32,
            vm_memory_known: m.vm_memory_known,
            vm_memory_free_mb: m.vm_memory_free_mb,
            vm_typical_mb: m.vm_typical_mb,
            snapshots: n
                .snapshots
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            snapshot_abis: n.snapshot_abis.clone(),
            arch: n.arch.clone(),
            accelerators: n.accelerators.clone(),
            draining: n.draining,
        }
    }

    /// As the wire status (what placement scores).
    pub(crate) fn to_pb(&self) -> pb::HostNodeStatus {
        pb::HostNodeStatus {
            kinds: self.kinds.clone(),
            runtimes: self.runtimes.clone(),
            millicpus_total: self.millicpus_total,
            millicpus_free: self.millicpus_free,
            memory_total_mb: self.memory_total_mb,
            memory_free_mb: self.memory_free_mb,
            running: self.running.clone().into_iter().collect(),
            problem: self.problem.clone(),
            measured: Some(pb::HostMeasured {
                memory_available_mb: self.memory_available_mb,
                memory_pressure: self.memory_pressure,
                vcpu_overcommit: self.vcpu_overcommit_pct as f32 / 100.0,
                vm_memory_known: self.vm_memory_known,
                vm_memory_free_mb: self.vm_memory_free_mb,
                vm_typical_mb: self.vm_typical_mb,
            }),
            snapshots: self.snapshots.clone().into_iter().collect(),
            snapshot_abis: self.snapshot_abis.clone(),
            arch: self.arch.clone(),
            accelerators: self.accelerators.clone(),
            draining: self.draining,
            ..Default::default()
        }
    }

    /// As the scheduler reads it.
    pub fn status(&self) -> pb::HostNodeStatus {
        self.to_pb()
    }
}

/// What a host said about itself.
async fn host_answer(space: &crate::Space) -> Result<pb::GetHostSpacesResponse> {
    space
        .spacesd()?
        .host_spaces()
        .get_host_spaces(pb::GetHostSpacesRequest {})
        .await
        .map(tonic::Response::into_inner)
        .map_err(|e| from_host(cua_spacesd_client::Error::from(e)))
}

/// A machine as [`Spaces::hosts`] lists it: one that answered and provides
/// Spaces; one that does not provide them is left out; one that did not
/// answer is listed offline when `keep_offline` (it provided a Space
/// before, or was added as a host).
fn host_offer(
    id: String,
    name: String,
    via: &str,
    answer: Option<Result<pb::GetHostSpacesResponse>>,
    keep_offline: bool,
) -> Option<HostOffer> {
    match answer {
        Some(Ok(r)) if r.settings.as_ref().is_some_and(|s| !s.provide_spaces) => None,
        Some(Ok(r)) => Some(HostOffer {
            id,
            name,
            via: via.into(),
            online: true,
            node: r.node.as_ref().map(HostCapacity::from_pb),
            os: r.os,
            limits: r
                .capacity
                .into_iter()
                .map(|c| HostLimit {
                    resource: c.resource,
                    used: c.used,
                    limit: c.limit,
                    reason: c.reason,
                })
                .collect(),
        }),
        // It answered without providing Spaces (or no longer does).
        Some(Err(e)) if e.tag() == "host_capability_missing" => None,
        _ => keep_offline.then(|| HostOffer {
            id,
            name,
            via: via.into(),
            online: false,
            node: None,
            os: String::new(),
            limits: vec![],
        }),
    }
}

/// A relay machine as [`Spaces::hosts`] lists it: [`host_offer`], except
/// that one the relay sees connected (`present`) is listed online, as the
/// Machines page lists it, whatever its answer:
///
/// - one whose answer did not come in time has its limits unknown. The
///   relay reaches it (`cua spaces ls` lists it), so "Run on" must not call
///   it offline; a create there says what is wrong if it really is.
/// - one that refused because its owner stopped sharing it says so, as a
///   [`STOPPED_SHARING`] entry among its limits: "Run on" lists it as not
///   sharing, not offline.
fn relay_offer(
    id: String,
    name: String,
    present: bool,
    os: &str,
    answer: Option<Result<pb::GetHostSpacesResponse>>,
    keep_offline: bool,
) -> Option<HostOffer> {
    if present
        && let Some(Err(e)) = &answer
        && let Some(reason) = stopped_sharing_reason(e)
    {
        return keep_offline.then(|| HostOffer {
            id,
            name,
            via: "relay".into(),
            online: true,
            os: os.to_string(),
            limits: vec![HostLimit {
                resource: STOPPED_SHARING.into(),
                used: 0,
                limit: 0,
                reason,
            }],
            node: None,
        });
    }
    let timed_out = present && answer.is_none();
    let mut offer = host_offer(id, name, "relay", answer, keep_offline)?;
    if timed_out {
        offer.online = true;
        if offer.os.is_empty() {
            offer.os = os.to_string();
        }
    }
    Some(offer)
}

impl Spaces {
    /// Your machines that provide Spaces, for a "Run on" menu: the signed-in
    /// account's machines on the relay (owned, or shared with you as an
    /// editor; not Spaces a host provides, nor ones in your cloud), then the
    /// hosts added by their direct address. Each is asked for its limits
    /// (at most [`HOST_PROBE`] each, all at once). One that answers without
    /// providing Spaces is left out; one that does not answer is listed
    /// offline when it provided one of your Spaces before (relay) or was
    /// added as a host (direct), or is yours and set up to provide Spaces
    /// (its relay meta, [`cua_host::META_PROVIDES_SPACES`]). Without a relay account, or when the relay
    /// cannot be asked, only the direct hosts are listed.
    pub async fn hosts(&self) -> Result<Vec<HostOffer>> {
        let relay: Vec<crate::relay::RelayMachine> = if self.relay_account().is_some() {
            self.relay_machines().await.unwrap_or_default()
        } else {
            vec![]
        };
        // Hosts that provided one of your Spaces.
        let known: std::collections::HashSet<String> =
            relay.iter().filter_map(|m| m.host.clone()).collect();
        let known = &known;
        // This machine is "This Mac" in the menu, not one of the hosts.
        let this_machine = self.this_relay_machine_id();
        let relay_offers = futures_util::future::join_all(
            relay
                .iter()
                .filter(|m| {
                    m.host.is_none()
                        && m.role != "viewer"
                        && !m.meta.contains_key(cua_sandbox_core::byoc::meta::PROVIDER)
                        && this_machine.as_deref() != Some(m.id.as_str())
                })
                .map(|m| async move {
                    let name = if m.name.is_empty() {
                        m.id.clone()
                    } else {
                        m.name.clone()
                    };
                    let answer = if m.online {
                        tokio::time::timeout(HOST_PROBE, async {
                            let s = self.host_space(m).await?;
                            host_answer(&s).await
                        })
                        .await
                        .ok()
                    } else {
                        None
                    };
                    // Offline, it is listed (with why) when it provided one
                    // of your Spaces before, or its setup said it provides
                    // Spaces (or predates saying so): a Mac whose service
                    // is down must not just vanish from "Run on".
                    let offers_spaces = m
                        .meta
                        .get(cua_host::META_PROVIDES_SPACES)
                        .is_none_or(|v| v != "off");
                    let keep = known.contains(&m.id) || (m.role == "owner" && offers_spaces);
                    let os = m.meta.get(cua_host::META_OS).cloned().unwrap_or_default();
                    relay_offer(m.id.clone(), name, m.online, &os, answer, keep)
                }),
        )
        .await;
        let mut out: Vec<HostOffer> = relay_offers.into_iter().flatten().collect();
        // The same machine set up both ways is listed once (through the relay).
        // A relay host that routes Spaces is recorded there too; the relay
        // listed it above (or it is gone).
        let direct: Vec<DirectHost> = self
            .registry()
            .direct_hosts()
            .unwrap_or_default()
            .into_iter()
            .filter(|h| !h.space.starts_with("relay:"))
            .filter(|h| !out.iter().any(|o| o.name.eq_ignore_ascii_case(&h.name)))
            .collect();
        let direct_offers = futures_util::future::join_all(direct.iter().map(|h| async move {
            let answer = tokio::time::timeout(HOST_PROBE, async {
                let s = self.direct_host_space(h).await?;
                host_answer(&s).await
            })
            .await
            .ok();
            host_offer(h.name.clone(), h.name.clone(), "direct", answer, true)
        }))
        .await;
        out.extend(direct_offers.into_iter().flatten());
        Ok(out)
    }
}

/// The host part of a `host:port` authority, without IPv6 brackets.
fn authority_host(authority: &str) -> &str {
    let host = authority
        .rsplit_once(':')
        .map_or(authority, |(host, _)| host);
    host.trim_start_matches('[').trim_end_matches(']')
}

impl Spaces {
    /// Resolves `query` (a machine id or name, "spare mac mini") to one of
    /// the account's machines on the relay, else (the relay does not know
    /// it, or there is no relay account) to a machine added with `cua
    /// spaces add <addr> --host`. Spaces other hosts provide are never
    /// hosts themselves. Several equally good matches fail with
    /// [`Error::AmbiguousHost`] listing them, for direct hosts as for relay
    /// ones.
    pub async fn resolve_host(&self, query: &str) -> Result<ResolvedHost> {
        let mut relay_error = None;
        let machines: Vec<crate::relay::RelayMachine> = if self.relay_account().is_some() {
            match self.relay_machines().await {
                Ok(all) => all.into_iter().filter(|m| m.host.is_none()).collect(),
                Err(e) => {
                    relay_error = Some(e);
                    vec![]
                }
            }
        } else {
            vec![]
        };
        match cua_host::match_machine(query, &machines) {
            MachineMatch::One(m) => return Ok(ResolvedHost::Relay(*m)),
            MachineMatch::Ambiguous(choices) => return Err(ambiguous_host(query, &choices)),
            MachineMatch::None => {}
        }
        // The machines added by their direct address.
        // (A relay host that routes Spaces is recorded here too; the relay
        // answered for it above.)
        let direct: Vec<DirectHost> = self
            .registry()
            .direct_hosts()?
            .into_iter()
            .filter(|h| !h.space.starts_with("relay:"))
            .collect();
        let as_machines: Vec<cua_host::Machine> = direct.iter().map(direct_as_machine).collect();
        match cua_host::match_machine(query, &as_machines) {
            MachineMatch::One(m) => {
                if let Some(h) = direct.iter().find(|h| h.name == m.id) {
                    return Ok(ResolvedHost::Direct(h.clone()));
                }
            }
            MachineMatch::Ambiguous(choices) => return Err(ambiguous_host(query, &choices)),
            MachineMatch::None => {}
        }
        // Nothing here and the relay could not be asked: say why.
        if let Some(e) = relay_error
            && direct.is_empty()
        {
            return Err(e);
        }
        if machines.is_empty() && direct.is_empty() && self.relay_account().is_none() {
            return Err(Error::NotFound(format!(
                "a computer matching {query:?}: you have no computers (sign in to cua.ai to use \
                 the ones on the relay, or add one by its Tailscale or LAN address with `cua \
                 spaces add <addr> --host --token <token>`)"
            )));
        }
        let mut all = machines;
        all.extend(as_machines);
        Err(Error::NotFound(format!(
            "a computer matching {query:?} among your computers ({}); set one up with \
             `cua host setup --provide-spaces` on it, or add one by its Tailscale or LAN address \
             with `cua spaces add <addr> --host --token <token>`",
            if all.is_empty() {
                "none on the relay".to_string()
            } else {
                provided::describe_choices(&all)
            }
        )))
    }

    /// Adds a machine that provides Spaces over its direct listener (`cua
    /// host setup --direct <ip:port> --provide-spaces` on it, which prints
    /// this step): registers it as the Space `direct:<addr>` with its env
    /// token (stored like any direct Space's), and lists it as a direct
    /// host, so `on="host:<name>"` finds it. `name` defaults to the name
    /// the host reports.
    pub async fn add_direct_host(
        &self,
        url: &str,
        token: Option<String>,
        name: Option<String>,
    ) -> Result<SpaceInfo> {
        let info = self.add(url, token, name.clone()).await?;
        if !matches!(SpaceId::parse(&info.id)?, SpaceId::Direct { .. }) {
            return Err(Error::invalid(format!(
                "{} is not reached by a direct address; a host to add with --host is \
                 `<tailscale or LAN address>:<port>`",
                info.id
            )));
        }
        let space = self.space(&info.id).await?;
        // Its own name for itself, unless one was given.
        let host_name = match name.filter(|n| !n.trim().is_empty()) {
            Some(n) => n.trim().to_string(),
            None => match space
                .spacesd()?
                .host_spaces()
                .get_host_spaces(pb::GetHostSpacesRequest {})
                .await
            {
                Ok(r) if !r.get_ref().name.is_empty() => r.into_inner().name,
                _ => info.name.clone(),
            },
        };
        if !space
            .capabilities()
            .features
            .iter()
            .any(|f| f.name == HOST_SPACES_FEATURE && f.supported)
        {
            tracing::warn!(
                host = %host_name,
                "{} does not provide Spaces yet (on it: `cua host config --provide-spaces on`)",
                info.id
            );
        }
        if let Some(other) = self
            .registry()
            .direct_hosts()?
            .into_iter()
            .find(|h| h.space != info.id && h.name.eq_ignore_ascii_case(&host_name))
        {
            return Err(Error::invalid(format!(
                "another host is already called {host_name:?} ({}); pass --name",
                other.space
            )));
        }
        self.registry().upsert_direct_host(&host_name, &info.id)?;
        Ok(info)
    }

    /// What a host provides: its settings, capacity and your Spaces on it
    /// (all of them, with its audit, when you own it).
    pub async fn host_spaces(&self, host: &str) -> Result<pb::GetHostSpacesResponse> {
        let space = match self.resolve_host(host).await? {
            ResolvedHost::Relay(m) => self.host_space(&m).await?,
            ResolvedHost::Direct(h) => self.direct_host_space(&h).await?,
        };
        space
            .spacesd()?
            .host_spaces()
            .get_host_spaces(pb::GetHostSpacesRequest {})
            .await
            .map(tonic::Response::into_inner)
            .map_err(|e| from_host(cua_spacesd_client::Error::from(e)))
    }

    /// Another of your machines' health as its own cua daemon checks it
    /// (`HostSpacesService.DiagnoseHost`): Local Network access, disk,
    /// virtualization, memory, permissions. `machine` is an id or a name,
    /// as [`Spaces::resolve_host`] takes it; reached over the relay (or the
    /// mesh), or at its direct address. Fails plainly when it is offline,
    /// not yours, or runs a Cua that predates the check. Bounded (30 s).
    pub async fn machine_health(&self, machine: &str) -> Result<pb::DiagnoseHostResponse> {
        let (space, name) = match self.resolve_host(machine).await? {
            ResolvedHost::Relay(m) => {
                let name = if m.name.is_empty() {
                    m.id.clone()
                } else {
                    m.name.clone()
                };
                if !m.online {
                    return Err(Error::host(
                        "machine health",
                        format!("{name} is offline, so it can't be checked now"),
                    ));
                }
                if m.role != "owner" {
                    return Err(Error::Relay(cua_host::Error::PermissionDenied(format!(
                        "{name} is not yours: only its owner can check its health"
                    ))));
                }
                (self.space(&format!("relay:{}", m.id)).await?, name)
            }
            ResolvedHost::Direct(h) => {
                let name = h.name.clone();
                (self.direct_host_space(&h).await?, name)
            }
        };
        let mut client = space.spacesd()?.host_spaces();
        let call = client.diagnose_host(pb::DiagnoseHostRequest::default());
        match tokio::time::timeout(Duration::from_secs(30), call).await {
            Ok(Ok(r)) => Ok(r.into_inner()),
            Ok(Err(status)) if status.code() == tonic::Code::Unimplemented => Err(Error::host(
                "machine health",
                format!("{name} runs an older Cua that can't be checked; update Cua on it"),
            )),
            Ok(Err(status)) => Err(from_host(cua_spacesd_client::Error::from(status))),
            Err(_) => Err(Error::host(
                "machine health",
                format!("{name} did not answer within 30 s"),
            )),
        }
    }

    async fn host_space(&self, m: &crate::relay::RelayMachine) -> Result<crate::Space> {
        if !m.online {
            return Err(Error::host(
                "host spaces",
                format!(
                    "{} is offline (its cua-spacesd is not connected to the relay)",
                    if m.name.is_empty() { &m.id } else { &m.name }
                ),
            ));
        }
        if !m.sharing {
            return Err(stopped_sharing(if m.name.is_empty() {
                &m.id
            } else {
                &m.name
            }));
        }
        if m.role == "viewer" {
            return Err(Error::Relay(cua_host::Error::PermissionDenied(format!(
                "{} is shared with you to watch only; ask its owner to share it as an editor",
                m.name
            ))));
        }
        let id = format!("relay:{}", m.id);
        let s = self.space(&id).await?;
        if !s
            .capabilities()
            .features
            .iter()
            .any(|f| f.name == HOST_SPACES_FEATURE && f.supported)
        {
            // Capabilities may be cached from before the host changed its
            // settings: check once more.
            self.forget_connection(&id).await?;
            let s = self.space(&id).await?;
            s.require(HOST_SPACES_FEATURE)
                .map_err(|_| not_providing(&s, if m.name.is_empty() { &m.id } else { &m.name }))?;
            return Ok(s);
        }
        Ok(s)
    }

    /// A direct host's own Space, checked to provide Spaces.
    async fn direct_host_space(&self, host: &DirectHost) -> Result<crate::Space> {
        let provides = |s: &crate::Space| {
            s.capabilities()
                .features
                .iter()
                .any(|f| f.name == HOST_SPACES_FEATURE && f.supported)
        };
        let s = self.space(&host.space).await?;
        if provides(&s) {
            return Ok(s);
        }
        // Capabilities may be cached from before the host changed its
        // settings: check once more.
        self.forget_connection(&host.space).await?;
        let s = self.space(&host.space).await?;
        s.require(HOST_SPACES_FEATURE)
            .map_err(|_| not_providing(&s, &host.name))?;
        Ok(s)
    }

    /// How to call machine `query` in a message: its name, else `query`.
    pub(crate) async fn host_label(&self, query: &str) -> String {
        self.resolve_host(query)
            .await
            .map(|h| h.name())
            .unwrap_or_else(|_| query.to_string())
    }

    /// The Space of the host `query` names (to call its host services),
    /// and the base URL of its snapshot files when it is reached directly.
    pub(crate) async fn host_of(&self, query: &str) -> Result<(crate::Space, Option<String>)> {
        match self.resolve_host(query).await? {
            ResolvedHost::Relay(m) => Ok((self.host_space(&m).await?, None)),
            ResolvedHost::Direct(h) => {
                let s = self.direct_host_space(&h).await?;
                let url = s
                    .spacesd()
                    .ok()
                    .map(|c| c.endpoint().http_url(cua_snapshot_sync::HTTP_BASE));
                Ok((s, url))
            }
        }
    }

    /// Creates a Space on the machine `query` names (see the module docs).
    pub(crate) async fn create_on_host(&self, query: &str, opts: SpaceCreate) -> Result<SpaceInfo> {
        self.create_on_host_from(query, opts, None).await
    }

    /// [`Spaces::create_on_host`]; with `fork`, the host forks that
    /// checkpoint of its own instead of creating from an image.
    pub(crate) async fn create_on_host_from(
        &self,
        query: &str,
        opts: SpaceCreate,
        fork: Option<&str>,
    ) -> Result<SpaceInfo> {
        let host = match self.resolve_host(query).await? {
            ResolvedHost::Relay(m) => m,
            ResolvedHost::Direct(h) => {
                return self.create_on_direct_host(h, opts, fork, false).await;
            }
        };
        let space = self.host_space(&host).await?;
        let relay = self.relay()?;
        let token = relay.tokens.access_token().await?;
        let client = relay.client().await?;
        let machine = format!("space-{:016x}", rand::random::<u64>());
        let host_label = if host.name.is_empty() {
            host.id.clone()
        } else {
            host.name.clone()
        };
        let name = opts
            .name
            .clone()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| machine.clone());
        let reg = client
            .register(
                &token,
                &cua_host::relay::RegisterRequest {
                    id: machine.clone(),
                    name: name.clone(),
                    allow: vec![],
                    host: Some(host.id.clone()),
                    meta: Default::default(),
                },
            )
            .await?;
        // What a cancel removes: the relay machine, then the Space the
        // host is creating for it.
        crate::creating::record(crate::creating::Made::RelayMachine {
            id: machine.clone(),
        });
        crate::creating::record(crate::creating::Made::HostSpace {
            host: host.id.clone(),
            machine: machine.clone(),
        });
        let request = pb::CreateHostSpaceRequest {
            direct: false,
            network: network_word(opts.network).into(),
            egress: egress_to_pb(&opts.egress),
            attach: Some(pb::AttachRelayRequest {
                relay_url: relay.url.clone(),
                machine_token: reg.machine_token.clone(),
                machine_id: machine.clone(),
                relay_jwks_json: reg.jwks.to_string(),
                owner: reg.machine.owner.id.clone(),
                owner_email: reg.machine.owner.email.clone().unwrap_or_default(),
            }),
            create_key: machine.clone(),
            routed: false,
            ..host_request(&opts, fork)
        };
        let created = async {
            let host = space.spacesd()?;
            following_host(
                host,
                &machine,
                host.host_spaces().create_host_space(request),
            )
            .await
            .map_err(|e| from_host(cua_spacesd_client::Error::from(e)))
        }
        .await;
        // A host whose cua predates egress policies or `network` created the
        // Space without them: it must not be handed out as if it had them.
        let created = match created {
            Ok(r) => {
                let created_space = r.into_inner().space;
                let enforced = created_space
                    .as_ref()
                    .map(|s| s.egress_enforced.clone())
                    .unwrap_or_default();
                let applied = created_space.map(|s| s.network).unwrap_or_default();
                match host_enforces(&enforced, &opts.egress, &host_label)
                    .and_then(|()| host_applied_network(opts.network, &applied, &host_label))
                {
                    Ok(()) => Ok(()),
                    Err(e) => {
                        if let Ok(h) = space.spacesd() {
                            let _ = h
                                .host_spaces()
                                .delete_host_space(pb::DeleteHostSpaceRequest {
                                    space: machine.clone(),
                                })
                                .await;
                        }
                        Err(e)
                    }
                }
            }
            Err(e) => Err(e),
        };
        if let Err(e) = created {
            // Nothing runs for it: take the machine off the relay again, or
            // it lists as a Space forever (with a fresh token and device
            // session: the host may have taken long to fail).
            if let Err(gone) = self.remove_relay_machine(&machine).await {
                tracing::warn!(
                    machine,
                    "relay machine of the failed create not removed: {gone}"
                );
            }
            return Err(e);
        }
        // Its driver dials out on its own: wait (bounded) until the relay
        // lists it online.
        cua_sandbox_core::progress::report(cua_sandbox_core::progress::Progress::phase(
            cua_sandbox_core::progress::Phase::Connecting,
        ));
        let deadline = tokio::time::Instant::now() + ONLINE_WAIT;
        let mut row = None;
        while tokio::time::Instant::now() < deadline {
            if let Ok(m) = client.machine(&token, &machine).await
                && m.online
            {
                row = Some(m);
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        let m = match row {
            Some(m) => m,
            None => client.machine(&token, &machine).await?,
        };
        let mut info = crate::spaces::relay_info(&m);
        info.host_name = host_label;
        if !m.online {
            tracing::warn!(machine, "the new Space has not connected to the relay yet");
        }
        Ok(info)
    }

    /// `on="auto"`: creates the Space on the least-loaded machine that can
    /// take it. The candidates are this machine and your machines that
    /// provide Spaces (yours, and those shared with you to use; each asked
    /// what it offers and has free), ranked by the share of CPU and memory
    /// left after the Space ([`cua_cluster::placement`], spread); the best
    /// is asked and may refuse (it filled up meanwhile), and the next is
    /// tried. With more than [`P2C_ABOVE`] that fit, the first choice is the
    /// less loaded of two picked at random (power of two choices), so many
    /// clients choosing from the same view do not all pick one machine.
    /// Modal takes it only when you connected Modal and no machine could.
    pub(crate) async fn create_on_auto(&self, opts: SpaceCreate) -> Result<crate::SpaceCreated> {
        use cua_cluster::placement::{self, Candidate, Need, Policy};
        let hosts = self.hosts().await.unwrap_or_default();
        let image = opts.image.clone().unwrap_or_default();
        let macos = provided::is_macos_image(&image) || resolve_image(&image).1 == "macos";
        let need = Need {
            private_image: cua_cluster::placement::image_is_private(&image),
            kind: match opts.kind {
                Kind::Auto => String::new(),
                k => k.as_str().to_string(),
            },
            runtime: match &opts.runtime {
                Runtime::Auto => String::new(),
                r => r.as_str().to_string(),
            },
            host_os: if macos { "macos".into() } else { String::new() },
            arch: String::new(),
            on: String::new(),
            shape: cua_cluster::placement::warm_key(
                &image,
                opts.kind.as_str(),
                opts.runtime.as_str(),
                opts.cpus.unwrap_or(0),
                opts.memory_mb.unwrap_or(0),
            ),
            millicpus: opts.cpus.unwrap_or(2) * 1000,
            memory_mb: opts.memory_mb.unwrap_or(4096),
            accelerator: String::new(),
            services: false,
        };
        // This machine first in the list (index 0), then the hosts.
        let (local, mut held) = self.local_candidate(&opts, macos).await;
        let mut cands: Vec<Candidate> = vec![local];
        cands.extend(hosts.iter().map(|h| Candidate {
            name: h.name.clone(),
            ready: h.online,
            os: h.os.clone(),
            status: h.node.as_ref().map(HostCapacity::to_pb),
            ..Default::default()
        }));
        let mut order = placement::rank(&cands, &need, Policy::Spread);
        // Nothing has room, and this machine is the one short: take back
        // what nobody uses (idle warm-pool members, VMs whose lifetime
        // passed or whose owner is gone), then look again before refusing.
        if order.is_empty()
            && cands[0].lacks_room(&need)
            && let Some(short) = shortfall_of(&cands[0], &need)
        {
            let freed = self.reclaim(short).await;
            if freed.any() {
                tracing::info!(
                    warm_members = freed.warm_members,
                    ended = freed.ended.len(),
                    "freed capacity before refusing on=auto"
                );
                let (local, now) = self.local_candidate(&opts, macos).await;
                cands[0] = local;
                held = now;
                order = placement::rank(&cands, &need, Policy::Spread);
            }
        }
        if order.len() > P2C_ABOVE {
            let a = rand::random::<u64>() as usize % order.len();
            let mut b = rand::random::<u64>() as usize % (order.len() - 1);
            if b >= a {
                b += 1;
            }
            // `order` is best first: the better of the two is the earlier.
            let pick = order.remove(a.min(b));
            order.insert(0, pick);
        }
        let mut refused = Vec::new();
        for i in order.iter().copied() {
            let r = if i == 0 {
                let mut local = opts.clone();
                local.on = Some(On::Local);
                Box::pin(self.create_inner(local)).await
            } else {
                self.create_on_host(&hosts[i - 1].id, opts.clone())
                    .await
                    .map(|info| crate::SpaceCreated::Ready {
                        info,
                        reused: false,
                    })
            };
            match r {
                Ok(created) => return Ok(created),
                Err(e) => {
                    tracing::info!(machine = %cands[i].name, "on=auto tries the next machine: {e}");
                    refused.push(format!("{}: {e}", cands[i].name));
                }
            }
        }
        if !need.private_image
            && !macos
            && opts.network == cua_sandbox_core::NetworkMode::Default
            // A policy is enforced by your machines, never by a cloud.
            && opts.egress.is_empty()
            && self.cloud_connected("modal")
        {
            let mut modal = opts.clone();
            modal.on = Some(On::Provider("modal".into()));
            return Box::pin(self.create_inner(modal)).await;
        }
        if placement::nothing_has_room(&cands, &need) {
            // Machines match but each is full: say so (and what to do),
            // never "not found".
            return Err(Error::Sandbox(cua_sandbox_core::Error::Runtime(
                cua_sandbox_core::RuntimeError::HostCapacity(placement::no_room_message(
                    &cands,
                    &need,
                    THIS_MACHINE,
                    held,
                )),
            )));
        }
        if order.is_empty() {
            return Err(Error::NotFound(format!(
                "a machine that can take this Space: {}",
                placement::why_nothing_fits(&cands, &need)
            )));
        }
        Err(Error::LimitExceeded(format!(
            "none of your machines took the Space ({})",
            refused.join("; ")
        )))
    }

    /// This machine as an `on="auto"` candidate: its CPUs and memory less
    /// what its live Spaces have reserved (never what the load average
    /// leaves: other programs' busy CPUs share, and the load only lowers its
    /// rank), and what they hold, for the refusal's message.
    async fn local_candidate(
        &self,
        opts: &SpaceCreate,
        macos: bool,
    ) -> (
        cua_cluster::placement::Candidate,
        cua_cluster::sysinfo::Reserved,
    ) {
        let reservations = self.local_reservations().await;
        let (status, held) = cua_cluster::sysinfo::this_machine_status_reserving(
            &reservations,
            // A container's vCPUs share CPUs; a VM's each book one.
            if opts.kind != Kind::Vm && !macos {
                f64::from(cua_cluster::sysinfo::container_overcommit())
            } else {
                cua_cluster::sysinfo::vm_overcommit().map_or(1.0, f64::from)
            },
        );
        let cand = cua_cluster::placement::Candidate {
            name: THIS_MACHINE.into(),
            ready: cua_sandbox_core::placement::validate(&On::Local, opts.kind, &opts.runtime)
                .is_ok(),
            os: std::env::consts::OS.into(),
            status: Some(status),
            busy: cua_cluster::sysinfo::load_share(
                cua_cluster::sysinfo::load_average(),
                cua_cluster::sysinfo::cpu_count(),
            ),
            ..Default::default()
        };
        (cand, held)
    }

    /// What the Spaces on this machine reserve of it: each running or
    /// starting local sandbox by the size it was created with (the default
    /// size when its record has none), and the creates that have not made
    /// theirs yet. Counted once: a create whose sandbox is listed is its
    /// sandbox.
    pub(crate) async fn local_reservations(&self) -> Vec<cua_cluster::sysinfo::LocalReservation> {
        use cua_cluster::sysinfo::LocalReservation;
        use cua_sandbox_core::{ProviderKind, Status};
        let sandboxes = self.sandboxes();
        let mut out = Vec::new();
        let mut counted = std::collections::HashSet::new();
        let rows: Vec<(String, String, bool, bool)> = match sandboxes.list().await {
            Ok(rows) => rows
                .into_iter()
                .filter(|r| r.provider == ProviderKind::Local)
                .map(|r| {
                    let starting = r.status == Status::Provisioning;
                    let holds = starting || r.status == Status::Running;
                    (r.name, r.runtime_type, holds, starting)
                })
                .collect(),
            // The engine does not answer: the records say what ran.
            Err(_) => sandboxes
                .state()
                .list_all()
                .into_iter()
                .filter_map(|s| match s {
                    cua_sandbox_core::state::SandboxState::Local(l)
                        if l.runtime_type != "direct" =>
                    {
                        let holds = l.status == "running";
                        Some((l.name, l.runtime_type, holds, false))
                    }
                    _ => None,
                })
                .collect(),
        };
        for (name, runtime, holds, starting) in rows {
            if !holds {
                continue;
            }
            let (cpus, memory_mb) = match sandboxes.state().load(&name) {
                Some(cua_sandbox_core::state::SandboxState::Local(l)) => (
                    l.cpu_count.unwrap_or(crate::local_load::DEFAULT_CPUS),
                    l.memory_mb.unwrap_or(crate::local_load::DEFAULT_MEMORY_MB),
                ),
                _ => (
                    crate::local_load::DEFAULT_CPUS,
                    crate::local_load::DEFAULT_MEMORY_MB,
                ),
            };
            out.push(LocalReservation {
                container: crate::local_load::is_container_runtime(&runtime),
                millicpus: cpus.saturating_mul(1000),
                memory_mb,
                starting,
            });
            counted.insert(name);
        }
        // Idle warm-pool members are not Spaces until claimed, but they hold
        // CPU: counted here, they are what `reclaim` gives back before a
        // create is refused.
        for w in sandboxes.warm_held().await {
            out.push(LocalReservation {
                container: w.container,
                millicpus: w.cpus.saturating_mul(1000),
                memory_mb: w.memory_mb,
                starting: false,
            });
        }
        for (name, r) in crate::local_load::pending(self.home_dir()) {
            if !counted.contains(&name) {
                out.push(r);
            }
        }
        out
    }

    /// Whether the user connected their own `provider` cloud account here
    /// (`cua cloud connect <provider>`).
    pub(crate) fn cloud_connected(&self, provider: &str) -> bool {
        let path = self.registry().dir().join("cloud").join("state.json");
        let Ok(raw) = std::fs::read(&path) else {
            return false;
        };
        serde_json::from_slice::<serde_json::Value>(&raw).is_ok_and(|v| {
            v["connections"]
                .as_array()
                .is_some_and(|a| a.iter().any(|c| c["provider"] == provider))
        })
    }

    /// Creates a Space on a direct host: the host creates it and forwards a
    /// port on its address to it, and this device adds it as
    /// `direct:<host address>:<port>` with the Space's token, grouped under
    /// the host. Nothing is registered with the relay.
    /// Creates a Space on relay host `machine` that is reached through the
    /// host ([`crate::id::routed_authority`]): it registers no relay machine
    /// of its own, so a pool of many Spaces stays within the account's
    /// machine limit. The host is recorded like a direct host whose own
    /// Space is `relay:<machine>`.
    pub(crate) async fn create_on_host_routed(
        &self,
        machine: &str,
        opts: SpaceCreate,
        fork: Option<&str>,
    ) -> Result<SpaceInfo> {
        let m = match self.resolve_host(machine).await? {
            ResolvedHost::Relay(m) => m,
            ResolvedHost::Direct(h) => {
                return self.create_on_direct_host(h, opts, fork, false).await;
            }
        };
        let space = format!("relay:{}", m.id);
        let name = if m.name.is_empty() {
            m.id.clone()
        } else {
            m.name.clone()
        };
        if !self
            .registry()
            .direct_hosts()?
            .iter()
            .any(|h| h.space == space)
        {
            self.registry().upsert_direct_host(&name, &space)?;
        }
        let host = self
            .registry()
            .direct_hosts()?
            .into_iter()
            .find(|h| h.space == space)
            .ok_or_else(|| Error::NotFound(format!("routed host {name}")))?;
        self.create_on_direct_host(host, opts, fork, true).await
    }

    async fn create_on_direct_host(
        &self,
        host: DirectHost,
        opts: SpaceCreate,
        fork: Option<&str>,
        routed: bool,
    ) -> Result<SpaceInfo> {
        let space = self.direct_host_space(&host).await?;
        let key = format!("create-{:016x}", rand::random::<u64>());
        let request = pb::CreateHostSpaceRequest {
            attach: None,
            direct: !routed,
            routed,
            create_key: key.clone(),
            ..host_request(&opts, fork)
        };
        let host_client = space.spacesd()?;
        let made = following_host(
            host_client,
            &key,
            host_client.host_spaces().create_host_space(request),
        )
        .await
        .map(tonic::Response::into_inner)
        .map_err(|e| from_host(cua_spacesd_client::Error::from(e)))?
        .space
        .ok_or_else(|| {
            Error::host(
                "host spaces",
                format!("{} answered without the Space it created", host.name),
            )
        })?;
        if let Err(e) = host_enforces(&made.egress_enforced, &opts.egress, &host.name)
            .and_then(|()| host_applied_network(opts.network, &made.network, &host.name))
        {
            let _ = self.delete_on_direct_host(&host, &made.local_space).await;
            return Err(e);
        }
        let port = u16::try_from(made.direct_port).unwrap_or(0);
        if port == 0 || made.direct_token.is_empty() {
            let _ = self.delete_on_direct_host(&host, &made.local_space).await;
            return Err(Error::host(
                "host spaces",
                format!(
                    "{} created {} but did not forward it (its cua is older than this one; update it)",
                    host.name, made.local_space
                ),
            ));
        }
        let id = match SpaceId::parse(&host.space)? {
            SpaceId::Relay { machine_id } if routed => {
                format!("direct:{}", crate::id::routed_authority(&machine_id, port))
            }
            SpaceId::Direct { authority } => format!(
                "direct:{}",
                crate::id::authority(authority_host(&authority), port)
            ),
            _ => {
                let _ = self.delete_on_direct_host(&host, &made.local_space).await;
                return Err(Error::invalid(format!(
                    "{} is not a host Space",
                    host.space
                )));
            }
        };
        let name = opts
            .name
            .clone()
            .filter(|n| !n.trim().is_empty())
            .or_else(|| Some(made.name.clone()).filter(|n| !n.is_empty()));
        let info = match self.add(&id, Some(made.direct_token.clone()), name).await {
            Ok(info) => info,
            Err(e) => {
                // Nobody could use it: the host deletes it again.
                let _ = self.delete_on_direct_host(&host, &made.local_space).await;
                return Err(e);
            }
        };
        // Which host made it (deleted and turned off there), and its id
        // there; lists group it under the host.
        self.registry().update_direct_hosts(|hosts| {
            if let Some(h) = hosts.iter_mut().find(|h| h.space == host.space) {
                h.spaces.insert(info.id.clone(), made.local_space.clone());
            }
        })?;
        if let Some(mut record) = self.registry().get(&info.id)? {
            let credential = self.registry().credential(&info.id)?.unwrap_or_default();
            record.host = host.space.clone();
            record.host_name = host.name.clone();
            if record.image.is_empty() {
                record.image = made.image.clone();
            }
            if record.kind.is_empty() {
                record.kind = made.kind.clone();
            }
            self.registry().upsert(record, credential)?;
        }
        self.info(&SpaceId::parse(&info.id)?)
    }

    /// Asks direct host `host` to delete its Space `on_host` (`local:<name>`
    /// there).
    async fn delete_on_direct_host(&self, host: &DirectHost, on_host: &str) -> Result<String> {
        let space = self.direct_host_space(host).await?;
        space
            .spacesd()?
            .host_spaces()
            .delete_host_space(pb::DeleteHostSpaceRequest {
                space: on_host.to_string(),
            })
            .await
            .map(|r| r.into_inner().message)
            .map_err(|e| from_host(cua_spacesd_client::Error::from(e)))
    }

    /// Deletes Space `id`, which direct host `host` created (`on_host`
    /// there): the host deletes it, and it leaves the host's list here.
    pub(crate) async fn delete_on_direct_host_space(
        &self,
        id: &SpaceId,
        host: &DirectHost,
        on_host: &str,
    ) -> Result<String> {
        let message = match self.delete_on_direct_host(host, on_host).await {
            Ok(m) => m,
            // A pool host its cloud ended: the Space went with it, so it is
            // only forgotten here (nothing to ask; a retry would never end).
            Err(e) if self.direct_host_lost(host).await => {
                tracing::warn!(space = %id, host = host.name, "deleting a Space of a lost host: {e}");
                format!(
                    "Forgot {id}: its host {} is gone (its cloud ended it), and the Space with it.",
                    host.name
                )
            }
            Err(e) => {
                tracing::warn!(
                    space = %id,
                    host = host.name,
                    "delete not done: {e} (if the host is gone for good, `cua spaces rm {}` \
                     forgets it with the Spaces it provided)",
                    host.name
                );
                return Err(e);
            }
        };
        let _ = self.registry().forget_direct(&id.to_string());
        Ok(message)
    }

    /// Whether direct host `host` is confirmed lost: a pool host (reached
    /// through its relay machine) whose cloud ended it. A host added by
    /// address has no cloud to ask; removing it (`cua spaces rm <host>`)
    /// is what says it is gone.
    async fn direct_host_lost(&self, host: &DirectHost) -> bool {
        match host.space.strip_prefix("relay:") {
            Some(machine) => self.pool_host_lost(machine).await,
            None => false,
        }
    }

    /// Forgets direct host `host_space` (`direct:<addr>:<port>`, or a pool
    /// host's `relay:<machine>` route) with every Space it created here:
    /// their rows, credentials, connections and thumbnails. The host was
    /// removed or is gone; nothing reaches those Spaces through it any more.
    /// Returns the Spaces forgotten.
    pub(crate) async fn forget_host_spaces(&self, host_space: &str) -> Vec<String> {
        let gone = match self.registry().forget_direct_host(host_space) {
            Ok(g) => g,
            Err(e) => {
                tracing::warn!(host = host_space, "host's Spaces not forgotten: {e}");
                return vec![];
            }
        };
        for id in &gone {
            if let Ok(sid) = SpaceId::parse(id) {
                self.drop_connection(&sid).await;
            }
            self.inner.thumbnails.remove(id);
        }
        if !gone.is_empty() {
            tracing::info!(host = host_space, spaces = ?gone, "forgot the Spaces of a removed host");
        }
        gone
    }

    /// Turns Space `id`, which direct host `host` created (`on_host`
    /// there), off or on, on that host.
    pub(crate) async fn power_on_direct_host(
        &self,
        id: &SpaceId,
        host: &DirectHost,
        on_host: &str,
        on: bool,
    ) -> Result<crate::SpacePower> {
        let space = self.direct_host_space(host).await?;
        let r = space
            .spacesd()?
            .host_spaces()
            .set_host_space_power(pb::SetHostSpacePowerRequest {
                space: on_host.to_string(),
                on,
            })
            .await
            .map(|r| r.into_inner())
            .map_err(|e| from_host(cua_spacesd_client::Error::from(e)))?;
        Ok(crate::SpacePower {
            space: id.to_string(),
            state: r.state,
            power: r.power,
            message: r.message,
        })
    }

    /// Sets the egress policy of a Space a direct host created, on that
    /// host ([`HostSpacesServer::set_egress`]).
    pub(crate) async fn egress_on_direct_host(
        &self,
        host: &DirectHost,
        on_host: &str,
        policy: &crate::egress::EgressPolicy,
    ) -> Result<()> {
        let space = self.direct_host_space(host).await?;
        guard_host_rpc(&space, &host.name, "egress policies of Spaces")?;
        let r = space
            .spacesd()?
            .host_spaces()
            .set_host_space_egress(pb::SetHostSpaceEgressRequest {
                space: on_host.to_string(),
                egress: egress_to_pb(policy),
            })
            .await
            .map(|r| r.into_inner())
            .map_err(|e| from_host_egress(cua_spacesd_client::Error::from(e), &host.name))?;
        host_enforces(&r.enforced, policy, &host.name)
    }

    /// Sets the egress policy of a Space a host provides, on that host
    /// ([`HostSpacesServer::set_egress`]).
    pub(crate) async fn egress_on_host(
        &self,
        machine: &crate::relay::RelayMachine,
        host_id: &str,
        policy: &crate::egress::EgressPolicy,
    ) -> Result<()> {
        let relay = self.relay()?;
        let token = relay.tokens.access_token().await?;
        let client = relay.client().await?;
        let host = client.machine(&token, host_id).await.map_err(|e| match e {
            cua_host::Error::NotFound(_) => Error::NotFound(format!(
                "the host of relay:{} (machine {host_id}) is no longer on the relay",
                machine.id
            )),
            other => other.into(),
        })?;
        let label = if host.name.is_empty() {
            host.id.clone()
        } else {
            host.name.clone()
        };
        let space = self.host_space(&host).await?;
        guard_host_rpc(&space, &label, "egress policies of Spaces")?;
        let r = space
            .spacesd()?
            .host_spaces()
            .set_host_space_egress(pb::SetHostSpaceEgressRequest {
                space: machine.id.clone(),
                egress: egress_to_pb(policy),
            })
            .await
            .map(|r| r.into_inner())
            .map_err(|e| from_host_egress(cua_spacesd_client::Error::from(e), &label))?;
        host_enforces(&r.enforced, policy, &label)
    }

    /// Deletes a Space a host provides: the host deletes its sandbox and
    /// relay machine; this device removes the machine too (best effort).
    pub(crate) async fn delete_on_host(
        &self,
        machine: &crate::relay::RelayMachine,
        host_id: &str,
    ) -> Result<String> {
        let relay = self.relay()?;
        let token = relay.tokens.access_token().await?;
        let client = relay.client().await?;
        let host = match client.machine(&token, host_id).await {
            Ok(h) => h,
            // Its host is gone (removed, or set up again as a new machine):
            // nothing provides the Space any more, only its relay entry is
            // left.
            Err(cua_host::Error::NotFound(_)) => {
                self.remove_relay_machine(&machine.id).await?;
                return Ok(format!(
                    "Removed relay:{}: a stale entry (its host, computer {host_id}, is no longer \
                     on the relay).",
                    machine.id
                ));
            }
            Err(e) => return Err(e.into()),
        };
        let deleted = match self.host_space(&host).await {
            Ok(space) => match space.spacesd() {
                Ok(c) => c
                    .host_spaces()
                    .delete_host_space(pb::DeleteHostSpaceRequest {
                        space: machine.id.clone(),
                    })
                    .await
                    .map(|r| r.into_inner().message)
                    .map_err(|e| from_host(cua_spacesd_client::Error::from(e))),
                Err(e) => Err(e),
            },
            Err(e) => Err(e),
        };
        let message = match deleted {
            Ok(m) => m,
            // A pool host its cloud ended: the Space went with it; only its
            // relay entry is left (a retry would never end).
            Err(e) if !matches!(e, Error::NotFound(_)) && self.pool_host_lost(host_id).await => {
                tracing::warn!(machine = %machine.id, host = host_id, "deleting a Space of a lost host: {e}");
                self.remove_relay_machine(&machine.id).await?;
                return Ok(format!(
                    "Removed relay:{}: its host {} is gone (its cloud ended it), and the Space \
                     with it.",
                    machine.id,
                    if host.name.is_empty() {
                        &host.id
                    } else {
                        &host.name
                    }
                ));
            }
            // The host has no such Space (a create that failed before it
            // existed, or the host was set up again): only the relay entry
            // is left.
            Err(Error::NotFound(_)) => {
                self.remove_relay_machine(&machine.id).await?;
                return Ok(format!(
                    "Removed relay:{}: a stale entry ({} no longer has this Space).",
                    machine.id,
                    if host.name.is_empty() {
                        &host.id
                    } else {
                        &host.name
                    }
                ));
            }
            Err(e) => return Err(e),
        };
        match client.delete(&token, &machine.id).await {
            Ok(()) | Err(cua_host::Error::NotFound(_)) => {}
            Err(e) => tracing::warn!(machine = %machine.id, "relay machine not removed: {e}"),
        }
        Ok(message)
    }

    /// Removes relay machine `machine` as the signed-in account, with a
    /// fresh token and device session; one already gone counts as removed.
    pub(crate) async fn remove_relay_machine(&self, machine: &str) -> Result<()> {
        let relay = self.relay()?;
        let token = relay.tokens.access_token().await?;
        match relay.client().await?.delete(&token, machine).await {
            Ok(()) | Err(cua_host::Error::NotFound(_)) => {}
            Err(e) => return Err(e.into()),
        }
        self.relay_forget(machine);
        Ok(())
    }

    /// Turns a Space a host provides off or on, on that host
    /// ([`HostSpacesServer::set_power`]).
    pub(crate) async fn power_on_host(
        &self,
        machine: &crate::relay::RelayMachine,
        host_id: &str,
        on: bool,
    ) -> Result<crate::SpacePower> {
        let relay = self.relay()?;
        let token = relay.tokens.access_token().await?;
        let client = relay.client().await?;
        let host = client.machine(&token, host_id).await.map_err(|e| match e {
            cua_host::Error::NotFound(_) => Error::NotFound(format!(
                "the host of relay:{} (computer {host_id}) is no longer on the relay",
                machine.id
            )),
            other => other.into(),
        })?;
        let space = self.host_space(&host).await?;
        let r = space
            .spacesd()?
            .host_spaces()
            .set_host_space_power(pb::SetHostSpacePowerRequest {
                space: machine.id.clone(),
                on,
            })
            .await
            .map(|r| r.into_inner())
            .map_err(|e| from_host(cua_spacesd_client::Error::from(e)))?;
        Ok(crate::SpacePower {
            space: SpaceId::Relay {
                machine_id: machine.id.clone(),
            }
            .to_string(),
            state: r.state,
            power: r.power,
            message: r.message,
        })
    }
}

/// Who took each checkpoint a host holds through `CreateHostSnapshot`
/// (`<host dir>/snapshot-takers.json`, checkpoint name to account), so an
/// editor may delete the ones it took and no others.
mod snapshot_takers {
    use super::HostCaller;
    use std::collections::BTreeMap;
    use std::path::Path;

    const FILE: &str = "snapshot-takers.json";

    fn load(dir: &Path) -> BTreeMap<String, String> {
        std::fs::read(dir.join(FILE))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn save(dir: &Path, all: &BTreeMap<String, String>) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join(format!("{FILE}.tmp"));
        std::fs::write(&tmp, serde_json::to_vec_pretty(all)?)?;
        std::fs::rename(tmp, dir.join(FILE))
    }

    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    pub(super) fn record(dir: &Path, checkpoint: &str, caller: &HostCaller) -> std::io::Result<()> {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut all = load(dir);
        all.insert(checkpoint.to_string(), caller.account.clone());
        save(dir, &all)
    }

    pub(super) fn taker(dir: &Path, checkpoint: &str) -> Option<String> {
        load(dir).remove(checkpoint)
    }

    pub(super) fn forget(dir: &Path, checkpoint: &str) -> std::io::Result<()> {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut all = load(dir);
        if all.remove(checkpoint).is_some() {
            save(dir, &all)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    /// An old host (a Cua 0.7 machine, protocol revision 8) is refused up
    /// front, by name, with how to update; a current one is not.
    #[test]
    fn a_host_older_than_the_rpc_is_refused_with_how_to_update() {
        let space = |rev: u32| {
            crate::Space::new(
                SpaceId::Relay {
                    machine_id: "3e1f".into(),
                },
                "host".into(),
                None,
                pb::GetCapabilitiesResponse {
                    version: "0.6.0".into(),
                    protocol_revision: rev,
                    ..Default::default()
                },
                None,
                None,
                Default::default(),
            )
        };
        let e = guard_host_rpc(&space(8), "ip-172-31-46-222", "checkpoints of Spaces")
            .expect_err("revision 8 has no HostSnapshotsService");
        assert_eq!(e.tag(), "capability_missing");
        let m = e.to_string();
        assert!(
            m.starts_with("ip-172-31-46-222 runs an older Cua (cua-spacesd 0.6.0 / revision 8); checkpoints of Spaces there need Cua 0.8 or newer"),
            "{m}"
        );
        assert!(
            m.contains("curl -fsSL https://cua.ai/install.sh | sh"),
            "{m}"
        );
        assert!(!m.contains("unauthenticated"), "{m}");
        for rev in [
            cua_proto::HOST_SERVICES_MIN_REVISION,
            cua_proto::ENV_PROTOCOL_REVISION,
        ] {
            guard_host_rpc(&space(rev), "h", "checkpoints of Spaces").unwrap();
        }
        // No capabilities (no spacesd): not judged here.
        guard_host_rpc(&space(0), "h", "checkpoints of Spaces").unwrap();
    }

    /// What an old host answers for a new RPC (its access layer calls an
    /// unknown method "unauthenticated") is the same refusal, not a
    /// permissions error; other failures keep their own meaning.
    #[test]
    fn an_old_hosts_unauthenticated_answer_for_a_new_rpc_is_the_update_message() {
        for status in [
            tonic::Status::unauthenticated(
                "this computer does not share its desktop (it only provides Spaces): \
                 Ada cannot call /cua.env.v1.HostSnapshotsService/CreateHostSnapshot",
            ),
            tonic::Status::unimplemented("unknown service"),
        ] {
            let e = from_new_host_rpc(status, "spare", "checkpoints of Spaces");
            assert_eq!(e.tag(), "capability_missing", "{e}");
            let m = e.to_string();
            assert!(
                m.starts_with("spare runs an older Cua; checkpoints of Spaces there need Cua 0.8"),
                "{m}"
            );
            assert!(!m.contains("does not share its desktop"), "{m}");
        }
        let e = from_new_host_rpc(tonic::Status::not_found("no such checkpoint"), "spare", "x");
        assert!(!e.to_string().contains("older Cua"), "{e}");
        assert_ne!(e.tag(), "capability_missing", "{e}");
    }

    /// J: env and GPU of a create reach the host in `CreateHostSpace`.
    #[test]
    fn a_host_create_carries_env_and_gpu() {
        let opts = SpaceCreate {
            env: [("API_KEY".to_string(), "s3cret".to_string())].into(),
            gpu: Some("nvidia".into()),
            ..Default::default()
        };
        let r = host_request(&opts, None);
        assert_eq!(r.env.get("API_KEY").map(String::as_str), Some("s3cret"));
        assert_eq!(r.gpu, "nvidia");
        let r = host_request(&SpaceCreate::default(), Some("ck"));
        assert!(r.env.is_empty() && r.gpu.is_empty());
        assert_eq!(r.fork_source, "ck");
    }

    /// A fork booked small is reserved small: 64 forks of 1 vCPU / 1 GiB do
    /// not count as 64 default Spaces.
    #[test]
    fn provided_spaces_reserve_what_they_were_admitted_with() {
        let dir = tempfile::tempdir().unwrap();
        let sized = |n: u32| ProvidedSpace {
            local_space: format!("local:f{n}"),
            kind: "vm".into(),
            cpus: 1,
            memory_mb: 1024,
            ..Default::default()
        };
        let unsized_one = ProvidedSpace {
            local_space: "local:old".into(),
            kind: "vm".into(),
            ..Default::default()
        };
        provided::save_provided(dir.path(), &[sized(1), sized(2), unsized_one]).unwrap();
        let ext = provided_reservation(dir.path());
        assert_eq!(ext.millicpus, 1000 + 1000 + 2000);
        assert_eq!(ext.memory_mb, 1024 + 1024 + 4096);
        assert_eq!(ext.running["vm"], 3);
    }

    use super::*;

    /// The daemon reaches its own host service on the channel's port:
    /// Cua Spaces Local's is not Stable's 3211.
    #[test]
    fn the_daemon_reaches_its_host_spacesd_on_the_channels_port() {
        use cua_home::channel::CuaChannel;
        if std::env::var_os("CUA_ENV_LISTEN").is_some()
            || std::env::var_os("CUA_ENV_PORT").is_some()
        {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let host = Host::new(dir.path());
        std::fs::create_dir_all(host.paths().env_token().parent().unwrap()).unwrap();
        std::fs::write(host.paths().env_token(), "secret\n").unwrap();
        let on = |channel| host_spacesd_on(&host, channel).unwrap();
        assert_eq!(on(CuaChannel::Stable).url, "http://127.0.0.1:3211");
        assert_eq!(on(CuaChannel::Local).url, "http://127.0.0.1:3221");
        assert_eq!(on(CuaChannel::Local).token, "secret");
        // The mesh media port is every channel's, by design: guest Spaces
        // and older cua-spacesd serve it there, and it is not an OS port.
        for channel in CuaChannel::ALL {
            assert_eq!(channel.ports().mesh_udp, cua_mesh::MEDIA_UDP_PORT);
        }
    }

    /// What a create on a host reports to its caller, by elapsed minute,
    /// when the host call takes `call` and the host answers `ask`.
    async fn reports_while_following(
        call: Duration,
        ask: impl Fn(Duration) -> Option<Vec<pb::HostSpaceCreating>>,
    ) -> Vec<(u64, crate::CreateProgress)> {
        let start = tokio::time::Instant::now();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s = seen.clone();
        let sink: cua_sandbox_core::progress::Sink = Arc::new(move |p: &crate::CreateProgress| {
            s.lock()
                .unwrap()
                .push((start.elapsed().as_secs() / 60, p.clone()))
        });
        let ask = |d: Duration| std::future::ready(ask(d));
        cua_sandbox_core::progress::scope(
            sink,
            following_host_with(|| ask(start.elapsed()), "space-1", tokio::time::sleep(call)),
        )
        .await;
        seen.lock().unwrap().clone()
    }

    /// A host that never lists the create (an older one, or one whose
    /// answers do not get through) still keeps the caller's create alive:
    /// "preparing" again every minute, so the apps' 15 min silent-stream
    /// limit never fails a 20+ GB macOS download on another machine.
    #[tokio::test(start_paused = true)]
    async fn a_host_that_lists_nothing_still_keeps_a_long_create_alive() {
        let seen = reports_while_following(Duration::from_secs(40 * 60), |_| None).await;
        let minutes: Vec<u64> = seen.iter().map(|(m, _)| *m).collect();
        assert!(seen.len() >= 39, "{minutes:?}");
        assert!(
            seen.iter()
                .all(|(_, p)| p.phase == cua_sandbox_core::progress::Phase::Preparing),
            "{seen:?}"
        );
        assert!(
            minutes.windows(2).all(|w| w[1] - w[0] <= 1),
            "never silent for more than a minute: {minutes:?}"
        );
    }

    /// A host that lists the create only after a while (past the first
    /// 20 s) is still followed: its download shows, then the heartbeat
    /// repeats the last report while nothing new comes.
    #[tokio::test(start_paused = true)]
    async fn a_host_that_lists_the_create_late_is_still_followed() {
        let seen = reports_while_following(Duration::from_secs(20 * 60), |d| {
            (d > Duration::from_secs(90)).then(|| {
                vec![pb::HostSpaceCreating {
                    key: "space-1".into(),
                    phase: "pulling".into(),
                    bytes_done: Some(5),
                    bytes_total: Some(10),
                    ..Default::default()
                }]
            })
        })
        .await;
        let pulls = seen
            .iter()
            .filter(|(_, p)| p.phase == cua_sandbox_core::progress::Phase::Pulling)
            .count();
        assert!(pulls >= 15, "the download, then said again: {seen:?}");
        assert!(
            seen.iter()
                .skip_while(|(_, p)| p.phase != cua_sandbox_core::progress::Phase::Pulling)
                .all(|(_, p)| p.phase == cua_sandbox_core::progress::Phase::Pulling),
            "never back to preparing: {seen:?}"
        );
    }

    #[test]
    fn a_loopback_relay_is_reached_at_the_host_from_a_guest() {
        assert_eq!(
            guest_relay_url("https://relay.cua.ai", "container"),
            "https://relay.cua.ai"
        );
        assert_eq!(
            guest_relay_url("http://127.0.0.1:8080", "container"),
            "http://host.docker.internal:8080"
        );
        assert_eq!(
            guest_relay_url("http://localhost:9/base", "container"),
            "http://host.docker.internal:9/base"
        );
        if cfg!(target_os = "macos") {
            for kind in ["vm", "microvm"] {
                assert_eq!(
                    guest_relay_url("http://127.0.0.1:8080", kind),
                    "http://192.168.64.1:8080"
                );
            }
        }
    }

    #[test]
    fn aliases_resolve_and_the_os_is_known() {
        let (r, os) = resolve_image("macos:26");
        assert!(r.contains("macos"), "{r}");
        assert_eq!(os, "macos");
        let (_, os) = resolve_image("linux");
        assert_eq!(os, "linux");
        let (r, os) = resolve_image("ghcr.io/trycua/macos-sequoia-cua:latest");
        assert_eq!(
            (r.as_str(), os.as_str()),
            ("ghcr.io/trycua/macos-sequoia-cua:latest", "macos")
        );
        let (r, os) = resolve_image("");
        assert!(r.contains("linux"));
        assert_eq!(os, "linux");
    }

    #[test]
    fn callers() {
        let c = HostCaller::from_metadata(Some(
            r#"{"account":"ada","email":"ada@x.io","name":"Ada","role":"owner","via":"relay"}"#,
        ))
        .unwrap();
        assert_eq!(c.label(), "Ada <ada@x.io> (ada)");
        assert!(c.is_owner() && c.may_create());
        let local = HostCaller::from_metadata(None).unwrap();
        assert_eq!(local.label(), "local");
        assert!(local.is_owner());
        let viewer = HostCaller {
            role: "viewer".into(),
            ..Default::default()
        };
        assert!(!viewer.may_create());
        assert!(HostCaller::from_metadata(Some("nope")).is_err());
    }

    /// A Unicode display name arrives as ASCII-escaped JSON (current
    /// cua-spacesd) or as raw UTF-8 bytes (older cua-spacesd); both parse,
    /// and bytes that are not UTF-8 are refused.
    #[test]
    fn callers_from_grpc_metadata() {
        use tonic::metadata::{MetadataMap, MetadataValue};
        let with = |v: MetadataValue<tonic::metadata::Ascii>| {
            let mut m = MetadataMap::new();
            m.insert(CALLER_METADATA, v);
            m
        };
        let name = "Jos\u{e9} \u{65e5}\u{672c} \u{1f680}";

        let escaped = r#"{"account":"ada","name":"Jos\u00e9 \u65e5\u672c \ud83d\ude80","role":"owner","via":"relay"}"#;
        assert!(escaped.is_ascii());
        let c = HostCaller::from_metadata(Some(escaped)).unwrap();
        assert_eq!(c.name.as_deref(), Some(name));
        let c = HostCaller::from_grpc_metadata(&with(escaped.parse().unwrap())).unwrap();
        assert_eq!(c.name.as_deref(), Some(name));
        assert!(c.is_owner());

        let raw = format!(r#"{{"account":"ada","name":"{name}","role":"owner","via":"relay"}}"#);
        // What cua-spacesd up to 0.3.0 sent: `str::parse` keeps the bytes.
        let value: MetadataValue<tonic::metadata::Ascii> = raw.parse().unwrap();
        assert!(value.to_str().is_err());
        let c = HostCaller::from_grpc_metadata(&with(value)).unwrap();
        assert_eq!(c.name.as_deref(), Some(name));
        assert_eq!(c.account, "ada");

        let not_utf8 = MetadataValue::try_from(&b"{\"account\":\"\xff\"}"[..]).unwrap();
        let err = HostCaller::from_grpc_metadata(&with(not_utf8)).unwrap_err();
        assert!(err.to_string().contains("not UTF-8"), "{err}");

        assert_eq!(
            HostCaller::from_grpc_metadata(&MetadataMap::new()).unwrap(),
            HostCaller::local()
        );
    }

    /// A host that answered is listed with its limits unless it does not
    /// provide Spaces; one that did not answer only when it is known.
    #[test]
    fn host_offers_follow_the_answer() {
        let answer = |provide: bool| pb::GetHostSpacesResponse {
            os: "macos".into(),
            settings: Some(pb::HostSpacesSettings {
                provide_spaces: provide,
                ..Default::default()
            }),
            capacity: vec![pb::HostSpacesCapacity {
                resource: "macos_vms".into(),
                used: 2,
                limit: 2,
                reason: "two per Mac".into(),
            }],
            ..Default::default()
        };
        let offer = |a, keep| host_offer("m1".into(), "Mac mini".into(), "relay", a, keep);
        let on = offer(Some(Ok(answer(true))), false).unwrap();
        assert!(on.online && on.os == "macos");
        assert_eq!(
            on.limits,
            vec![HostLimit {
                resource: "macos_vms".into(),
                used: 2,
                limit: 2,
                reason: "two per Mac".into(),
            }]
        );
        assert_eq!(
            offer(Some(Ok(answer(false))), true),
            None,
            "does not provide"
        );
        assert_eq!(
            offer(Some(Err(Error::host("host spaces", "no"))), true),
            None,
            "answered without providing"
        );
        let silent = offer(None, true).unwrap();
        assert!(!silent.online && silent.limits.is_empty());
        assert_eq!(offer(None, false), None, "unknown and silent");

        // Connected to the relay but slow to answer: online, limits unknown.
        let slow = relay_offer("m1".into(), "Mac mini".into(), true, "macos", None, true).unwrap();
        assert!(slow.online && slow.os == "macos" && slow.limits.is_empty());
        // Not connected: offline, as before.
        let gone = relay_offer("m1".into(), "Mac mini".into(), false, "macos", None, true).unwrap();
        assert!(!gone.online);
        // Connected, and it said it does not provide Spaces: left out.
        assert_eq!(
            relay_offer(
                "m1".into(),
                "Mac mini".into(),
                true,
                "",
                Some(Ok(answer(false))),
                true
            ),
            None
        );
        // Connected, and the answer failed: offline (it was asked and said why).
        let failed = relay_offer(
            "m1".into(),
            "Mac mini".into(),
            true,
            "",
            Some(Err(Error::Timeout("hello".into()))),
            true,
        )
        .unwrap();
        assert!(!failed.online);
    }

    /// The Machines page and "Run on" read the same signal: a machine the
    /// relay sees connected is online. One that refused because its owner
    /// stopped sharing it (its relay record or its own policy says so) is
    /// online and says so, not offline.
    #[test]
    fn a_connected_machine_that_stopped_sharing_is_online_and_says_so() {
        let refused = |keep| {
            relay_offer(
                "m1".into(),
                "gamma-4 Mac Studio".into(),
                true,
                "macos",
                Some(Err(stopped_sharing("gamma-4 Mac Studio"))),
                keep,
            )
        };
        let offer = refused(true).unwrap();
        assert!(offer.online, "the relay sees it connected");
        assert_eq!(offer.os, "macos", "what its setup registered");
        assert_eq!(
            offer.limits,
            vec![HostLimit {
                resource: STOPPED_SHARING.into(),
                used: 0,
                limit: 0,
                reason: "gamma-4 Mac Studio stopped sharing: ask its owner to Resume sharing \
                         (or run `cua host start` there)"
                    .into(),
            }]
        );
        // Listed under the rule an offline machine is: yours, or provided.
        assert_eq!(refused(false), None);
        // Not connected: just offline (the relay would not have been asked).
        let gone = relay_offer(
            "m1".into(),
            "gamma-4 Mac Studio".into(),
            false,
            "macos",
            Some(Err(stopped_sharing("gamma-4 Mac Studio"))),
            true,
        )
        .unwrap();
        assert!(!gone.online && gone.limits.is_empty());
        // Another refusal stays an offline machine.
        let denied = relay_offer(
            "m1".into(),
            "Mac mini".into(),
            true,
            "",
            Some(Err(Error::Relay(cua_host::Error::PermissionDenied(
                "not yours".into(),
            )))),
            true,
        )
        .unwrap();
        assert!(!denied.online && denied.limits.is_empty());
    }

    /// Both wordings of "stopped sharing" are recognized: the relay's, and
    /// the machine's own when its policy says so while the relay still
    /// lists it as sharing (gamma-4, whose create said
    /// "cua-spacesd is not available").
    #[test]
    fn both_wordings_of_stopped_sharing_are_recognized() {
        for reason in [
            "the host stopped sharing this computer",
            "relay:96fe has no cua-spacesd (gRPC: relay assertion refused: this computer \
             stopped sharing; gRPC-Web: relay assertion refused: this computer stopped sharing)",
        ] {
            assert!(is_stopped_sharing(reason), "{reason}");
        }
        assert!(!is_stopped_sharing("the computer is not connected"));
        assert!(!is_stopped_sharing(
            "relay assertion refused: not your computer"
        ));
    }

    /// Without a relay account only the hosts added by address are listed;
    /// one that does not answer is offline.
    #[tokio::test]
    async fn a_direct_host_that_does_not_answer_is_listed_offline() {
        let dir = tempfile::tempdir().unwrap();
        let spaces = Spaces::builder().home(dir.path().join("cua")).build();
        assert!(spaces.hosts().await.unwrap().is_empty());
        spaces
            .registry()
            .upsert_direct_host("Studio", "direct:127.0.0.1:1")
            .unwrap();
        assert_eq!(
            spaces.hosts().await.unwrap(),
            vec![HostOffer {
                id: "Studio".into(),
                name: "Studio".into(),
                via: "direct".into(),
                online: false,
                node: None,
                os: String::new(),
                limits: vec![],
            }]
        );
    }

    /// With no relay account, `host:<name>` finds the hosts added with
    /// `cua spaces add <addr> --host`, with the same matching as relay
    /// hosts: the words of the name, and ambiguous_host for a tie.
    #[tokio::test]
    async fn direct_hosts_resolve_by_name_and_ties_are_ambiguous() {
        let dir = tempfile::tempdir().unwrap();
        let spaces = Spaces::builder().home(dir.path().join("cua")).build();
        let e = spaces.resolve_host("spare mac mini").await.unwrap_err();
        assert_eq!(e.tag(), "not_found", "{e}");
        assert!(e.to_string().contains("--host"), "{e}");

        let reg = spaces.registry();
        reg.upsert_direct_host("Mac mini (spare)", "direct:100.64.0.9:3211")
            .unwrap();
        reg.upsert_direct_host("Mac mini (office)", "direct:100.64.0.10:3211")
            .unwrap();
        reg.upsert_direct_host("Studio", "direct:192.168.1.20:3211")
            .unwrap();
        match spaces.resolve_host("spare mac mini").await.unwrap() {
            ResolvedHost::Direct(h) => assert_eq!(h.space, "direct:100.64.0.9:3211"),
            other => panic!("{other:?}"),
        }
        let studio = spaces.resolve_host("host:studio").await.unwrap();
        assert_eq!((studio.via(), studio.name().as_str()), ("direct", "Studio"));
        match spaces.resolve_host("mac mini").await.unwrap_err() {
            Error::AmbiguousHost { choices, .. } => {
                assert!(
                    choices.contains(&"host:Mac mini (spare)".to_string())
                        && choices.contains(&"host:Mac mini (office)".to_string()),
                    "{choices:?}"
                );
            }
            other => panic!("{other}"),
        }
        let e = spaces.resolve_host("windows laptop").await.unwrap_err();
        assert_eq!(e.tag(), "not_found", "{e}");
        assert!(e.to_string().contains("Studio"), "{e}");
    }

    /// `on="auto"` with nothing that can take the Space says why: your
    /// machines are full (an unreachable host is not one of them) and no
    /// cloud is linked to add one.
    #[tokio::test]
    async fn auto_says_why_no_machine_takes_a_space() {
        let dir = tempfile::tempdir().unwrap();
        let spaces = Spaces::builder().home(dir.path().join("cua")).build();
        spaces
            .registry()
            .upsert_direct_host("Studio", "direct:127.0.0.1:1")
            .unwrap();
        // More memory than any machine has.
        let big = SpaceCreate {
            memory_mb: Some(1 << 40),
            ..Default::default()
        };
        // No cloud linked: on="auto" places on your machines only.
        assert!(!spaces.elastic_has_clouds().await);
        let e = match spaces.create_on_auto(big).await {
            Err(e) => e,
            Ok(_) => panic!("a Space no machine can take was created"),
        };
        // A machine that matches but cannot take it is a capacity refusal
        // (#126: never "not found"), saying why and what to do.
        assert_eq!(e.tag(), "host_capacity", "{e}");
        assert!(e.to_string().contains("this machine has no room"), "{e}");
        assert!(e.to_string().contains("fix:"), "{e}");
        assert!(!spaces.cloud_connected("modal"));
    }

    #[test]
    fn host_addresses_lose_their_port_and_brackets() {
        assert_eq!(authority_host("100.64.0.9:3211"), "100.64.0.9");
        assert_eq!(
            authority_host("[fd7a:115c:a1e0::9]:3211"),
            "fd7a:115c:a1e0::9"
        );
        assert_eq!(
            authority_host("mini.tailnet.ts.net:3211"),
            "mini.tailnet.ts.net"
        );
    }

    #[test]
    fn host_errors_keep_their_kind_across_the_wire() {
        for e in [
            Error::LimitExceeded("full".into()),
            Error::Relay(cua_host::Error::PermissionDenied("no".into())),
            Error::host("host spaces", "off"),
            Error::NotFound("x".into()),
        ] {
            let status = to_status(&e);
            let back = from_host(cua_spacesd_client::Error::from(status));
            assert_eq!(back.tag(), e.tag(), "{e}");
        }
    }
}

#[cfg(test)]
mod host_runtime_tests {
    use super::*;
    use cua_sandbox_core::placement::HostFacts;
    use cua_sandbox_core::settings::Settings;

    const MAC_26: HostFacts = HostFacts {
        gvisor: None,
        apple_silicon: true,
        kvm: false,
        apple_containers: true,
        vz: false,
    };
    const MAC_15: HostFacts = HostFacts {
        gvisor: None,
        apple_silicon: true,
        kvm: false,
        apple_containers: false,
        vz: false,
    };

    fn settings(toml: &str) -> (tempfile::TempDir, Settings) {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("config.toml"), toml).unwrap();
        let s = Settings::load_with(d.path().join("config.toml"), |_| None).unwrap();
        (d, s)
    }

    /// A Space created on this host from another device with `auto` runs
    /// on this host's `default.runtime`, not the engine `auto` would pick.
    #[test]
    fn auto_takes_the_hosts_default_runtime() {
        let (_d, s) = settings("[default]\nruntime = \"containerization\"\n");
        let got = host_placement(Kind::Auto, Runtime::Auto, false, Some(&s), &MAC_26).unwrap();
        assert_eq!(got, (Kind::Auto, Runtime::Containerization));
        // Without a default, `auto` stays `auto` (this host picks at start).
        let (_d, none) = settings("");
        let got = host_placement(Kind::Auto, Runtime::Auto, false, Some(&none), &MAC_26).unwrap();
        assert_eq!(got, (Kind::Auto, Runtime::Auto));
        let got = host_placement(Kind::Auto, Runtime::Auto, false, None, &MAC_26).unwrap();
        assert_eq!(got, (Kind::Auto, Runtime::Auto));
    }

    /// The host's default is skipped where it cannot apply: a macOS guest
    /// (Lume), a kind it does not run, a host that cannot run it.
    #[test]
    fn the_hosts_default_is_skipped_where_it_does_not_fit() {
        let (_d, s) = settings("[default]\nruntime = \"containerization\"\n");
        assert_eq!(
            host_placement(Kind::Auto, Runtime::Auto, true, Some(&s), &MAC_26).unwrap(),
            (Kind::Auto, Runtime::Auto)
        );
        assert_eq!(
            host_placement(Kind::Container, Runtime::Auto, false, Some(&s), &MAC_26).unwrap(),
            (Kind::Container, Runtime::Auto)
        );
        assert_eq!(
            host_placement(Kind::Auto, Runtime::Auto, false, Some(&s), &MAC_15).unwrap(),
            (Kind::Auto, Runtime::Auto)
        );
    }

    /// An explicit runtime this host runs is kept; one it cannot run is
    /// refused naming the runtimes it has (never a transport error).
    #[test]
    fn an_explicit_runtime_is_kept_or_refused_with_the_valid_ones() {
        let got = host_placement(Kind::Auto, Runtime::Containerization, false, None, &MAC_26);
        assert_eq!(got.unwrap(), (Kind::Auto, Runtime::Containerization));
        let e = host_placement(Kind::Auto, Runtime::Containerization, false, None, &MAC_15)
            .unwrap_err();
        assert!(
            e.message
                .starts_with("this host cannot run containerization: "),
            "{}",
            e.message
        );
        assert!(e.valid.iter().any(|v| v == "auto"), "{:?}", e.valid);
        assert!(
            !e.valid.iter().any(|v| v == "containerization"),
            "{:?}",
            e.valid
        );
        assert!(e.message.contains("valid runtime: auto"), "{}", e.message);
        // Through the RPC and back it stays a clear invalid-argument error.
        let status = to_status(&Error::Sandbox(cua_sandbox_core::Error::InvalidPlacement(
            e.clone(),
        )));
        assert_eq!(status.code(), tonic::Code::InvalidArgument);
        let back = from_host(cua_spacesd_client::Error::from(status));
        assert!(matches!(back, Error::InvalidArgument(_)), "{back:?}");
        assert!(
            back.to_string()
                .contains("this host cannot run containerization")
        );
        // A third-party runtime is the provider's to check.
        let other = Runtime::Other("acme".into());
        assert_eq!(
            host_placement(Kind::Auto, other.clone(), false, None, &MAC_15).unwrap(),
            (Kind::Auto, other)
        );
    }

    /// `auto` on a broken container engine moves to a microVM runtime this
    /// host has; an image fault, an explicit choice and a macOS guest keep
    /// the error.
    #[test]
    fn a_broken_engine_falls_back_for_auto_only() {
        let broken = Error::Sandbox(cua_sandbox_core::Error::Runtime(
            cua_sandbox_core::RuntimeError::Other(
                "container engine: pull ghcr.io/trycua/linux@sha256:9a: Docker responded with \
                 status code 500: write /var/lib/containerd/io.containerd.metadata.v1.bolt/meta.db: \
                 input/output error"
                    .into(),
            ),
        ));
        let fb = |k, r: &Runtime, mac, e: &Error, h: &HostFacts| engine_fallback(k, r, mac, e, h);
        assert_eq!(
            fb(Kind::Auto, &Runtime::Auto, false, &broken, &MAC_26),
            Some(Runtime::Containerization)
        );
        assert_eq!(
            fb(Kind::Auto, &Runtime::Auto, false, &broken, &MAC_15),
            None
        );
        assert_eq!(
            fb(Kind::Container, &Runtime::Auto, false, &broken, &MAC_26),
            None
        );
        assert_eq!(
            fb(Kind::Auto, &Runtime::Gvisor, false, &broken, &MAC_26),
            None
        );
        assert_eq!(fb(Kind::Auto, &Runtime::Auto, true, &broken, &MAC_26), None);
        let image = Error::Sandbox(cua_sandbox_core::Error::Runtime(
            cua_sandbox_core::RuntimeError::Other(
                "container engine: pull ghcr.io/x/y:1: manifest unknown".into(),
            ),
        ));
        assert_eq!(fb(Kind::Auto, &Runtime::Auto, false, &image, &MAC_26), None);
        let vz = HostFacts { vz: true, ..MAC_26 };
        assert_eq!(
            fb(Kind::Auto, &Runtime::Auto, false, &broken, &vz),
            Some(Runtime::Vz)
        );
    }

    /// Drift guard: a create on this host goes through [`host_placement`]
    /// (its runtime, not the caller's) and keeps [`engine_fallback`].
    #[test]
    fn host_creates_resolve_the_runtime_here() {
        let src = include_str!("host_spaces.rs");
        let body = src
            .split("async fn create_provided(")
            .nth(1)
            .and_then(|b| b.split("\n    fn with_egress(").next())
            .expect("create_provided");
        assert!(
            body.contains("host_placement("),
            "create_provided skips host_placement"
        );
        assert!(
            body.contains("engine_fallback("),
            "create_provided skips engine_fallback"
        );
    }
}
