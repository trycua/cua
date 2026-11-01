//! Batched forks: many Spaces from checkpoints at once, on this machine or
//! across your machines (`SandboxService.ForkSandboxBatch`).
//!
//! 1. **Place.** The batch's groups (the forks of one checkpoint that
//!    belong together) are placed with [`cua_cluster::batch::plan_batch`]:
//!    each group whole on one machine, groups of one checkpoint together,
//!    checkpoints spread over the least-loaded machines, only where the
//!    checkpoint's ABI restores. Each placement is reported.
//! 2. **Copy.** A machine that does not hold a group's checkpoint gets a
//!    copy first, once per batch ([`crate::snapshots`]): straight from the
//!    machine that holds it over the LAN, else from the fallback (S3, or a
//!    folder every host mounts). Progress is reported.
//! 3. **Fork.** At most `max_parallel_per_host` forks start at once on one
//!    machine. Each fork is reported as soon as it is ready, or failed.
//! 4. **Recover.** A fork whose machine failed (it stopped answering) or
//!    refused it (full) is placed again on the other machines, up to
//!    `retries` times; the failed machine gets no more forks of this
//!    batch. Any other failure is reported for that fork alone: the rest of
//!    the batch goes on.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Instant;

use cua_cluster::batch::{BatchGroup, plan_batch};
use cua_cluster::placement::{Candidate, Need, Policy};
use cua_proto::daemon::v1 as dpb;
use cua_proto::env::v1 as pb;
use futures_util::StreamExt as _;
use futures_util::stream::FuturesUnordered;
use tokio::sync::{OnceCell, Semaphore, mpsc};

use crate::{Error, Result};

/// Forks started at once on one machine by default.
pub const DEFAULT_PARALLEL_PER_HOST: u32 = 4;
/// vCPUs each fork is counted as when placing, by default: forks of one
/// world are mostly idle and share its pages, so a fork books one.
pub const DEFAULT_FORK_CPUS: u32 = 1;
/// Memory (MiB) each fork is counted as when placing, by default.
pub const DEFAULT_FORK_MEMORY_MB: u64 = 2048;
/// Why nothing took a fork, with what to do when the machines are out of CPU.
fn no_host_message(reason: &str, cpus: u32) -> String {
    let mut m = format!("no machine can take it: {reason}");
    if reason.contains("CPU") {
        m.push_str(&format!(
            "; each fork is counted as {cpus} vCPU (--cpus): fork fewer at once, \
             or add machines and use --on auto"
        ));
    }
    m
}

/// The machine the batch runs on, in reports.
pub const THIS_MACHINE: &str = "local";

/// A batch, as the daemon received it.
#[derive(Clone, Debug, Default)]
pub struct BatchSpec {
    /// The groups, each of one checkpoint, with their names.
    pub groups: Vec<dpb::ForkBatchGroup>,
    /// `""`/`local`, `auto` or `host:<machine>`.
    pub on: String,
    /// Forks started at once per machine.
    pub max_parallel_per_host: u32,
    /// vCPUs each fork is counted as (`None`: [`DEFAULT_FORK_CPUS`]).
    pub cpus: Option<u32>,
    /// Memory each fork is counted as, MiB (`None`: its checkpoint's).
    pub memory_mb: Option<u64>,
    /// The size of each checkpoint's forks when not asked for: vCPUs and
    /// MiB (filled from the checkpoints' records).
    pub sizes: BTreeMap<String, (u32, u64)>,
    /// Where machines fetch a checkpoint when its holder cannot be reached.
    pub fallback: Option<pb::SnapshotFallback>,
    /// Times forks of a failed machine are placed again.
    pub retries: u32,
    /// A group of one fork is named exactly its `name_prefix` (a single
    /// fork with a name of its own, as `Spaces::create` from a checkpoint
    /// starts), not `<prefix>-0`.
    pub exact_names: bool,
}

/// How a batch on `on` grows the elastic pool when your machines have no
/// room for it: `Some(false)` with any machine (`auto`), `Some(true)` with
/// cloud machines only (no location given and `overflow` allowed: a
/// budget, a linked cloud, `placement.overflow` on), `None` not at all (an
/// explicit `local` or `host:<machine>`).
pub(crate) fn batch_growth(on: &str, overflow: bool) -> Option<bool> {
    match on {
        "auto" => Some(false),
        "" if overflow => Some(true),
        _ => None,
    }
}

/// The name of fork `index` of a group, in the form it is registered under:
/// a Space name is lowercase letters, digits and `-`, 63 characters at most
/// (`sanitize_label`). The batch reports this very name, so the item's
/// `name` is what `connect` and `delete` take. A long prefix gives way to
/// the number, so the forks of a group never share a name.
fn fork_name(prefix: &str, index: u32, exact: bool) -> String {
    let clean = crate::sanitize_label(prefix);
    if exact {
        return clean;
    }
    let suffix = format!("-{index}");
    let room = 63 - suffix.len();
    let head = &clean[..clean.len().min(room)];
    format!("{}{suffix}", head.trim_end_matches('-'))
}

fn hex6() -> String {
    format!("{:06x}", rand::random::<u32>() & 0xff_ffff)
}

impl BatchSpec {
    /// The batch a `ForkSandboxBatchRequest` asks for.
    pub fn from_request(r: dpb::ForkSandboxBatchRequest) -> Result<BatchSpec> {
        let mut groups = r.groups;
        if !r.source.trim().is_empty() {
            if r.count == 0 {
                return Err(Error::invalid("count must be at least 1"));
            }
            for n in cua_cluster::batch::split_into_groups(r.count, r.group_size) {
                groups.push(dpb::ForkBatchGroup {
                    source: r.source.trim().to_string(),
                    count: n,
                    name_prefix: String::new(),
                });
            }
        }
        if groups.is_empty() {
            return Err(Error::invalid(
                "a batch needs a source and count, or groups",
            ));
        }
        let total: u64 = groups.iter().map(|g| u64::from(g.count)).sum();
        if total == 0 || total > 4096 {
            return Err(Error::invalid(format!(
                "a batch forks 1 to 4096 sandboxes, not {total}"
            )));
        }
        let batch_prefix = r.name_prefix.trim().to_string();
        for (gi, g) in groups.iter_mut().enumerate() {
            if g.source.trim().is_empty() || g.count == 0 {
                return Err(Error::invalid(format!(
                    "group {gi} needs a source and a count"
                )));
            }
            if g.name_prefix.trim().is_empty() {
                g.name_prefix = if batch_prefix.is_empty() {
                    format!("{}-{}", g.source, hex6())
                } else {
                    format!("{batch_prefix}-g{gi}")
                };
            }
        }
        let on = r.on.trim().to_string();
        if !(on.is_empty() || on == "local" || on == "auto" || on.starts_with("host:")) {
            return Err(Error::invalid(format!(
                "on must be local, auto or host:<machine>, not {on:?}"
            )));
        }
        Ok(BatchSpec {
            groups,
            on,
            max_parallel_per_host: if r.max_parallel_per_host == 0 {
                DEFAULT_PARALLEL_PER_HOST
            } else {
                r.max_parallel_per_host.min(64)
            },
            cpus: (r.cpus > 0).then_some(r.cpus),
            memory_mb: (r.memory_mb > 0).then_some(r.memory_mb),
            sizes: BTreeMap::new(),
            fallback: r.fallback,
            retries: if r.no_retry {
                0
            } else if r.retries == 0 {
                1
            } else {
                r.retries.min(8)
            },
            exact_names: false,
        })
    }

    /// What one fork of `world` is counted as: vCPUs and MiB.
    pub fn size_of(&self, world: &str) -> (u32, u64) {
        let known = self.sizes.get(world).copied();
        (
            self.cpus.unwrap_or(DEFAULT_FORK_CPUS),
            self.memory_mb
                .or(known.map(|k| k.1))
                .unwrap_or(DEFAULT_FORK_MEMORY_MB),
        )
    }

    /// Every fork of the batch: `(group, source, name)`, in index order.
    pub fn items(&self) -> Vec<(u32, String, String)> {
        let mut out = Vec::new();
        for (gi, g) in self.groups.iter().enumerate() {
            for i in 0..g.count {
                out.push((
                    gi as u32,
                    g.source.clone(),
                    fork_name(&g.name_prefix, i, self.exact_names && g.count == 1),
                ));
            }
        }
        out
    }
}

/// Where a checkpoint's files come from, for a machine that lacks it.
#[derive(Clone, Debug, Default)]
pub struct SnapshotFrom {
    /// The manifest (empty: read it from the fallback by name).
    pub manifest: Vec<u8>,
    /// Machines that serve the files, in order.
    pub sources: Vec<pb::SnapshotSource>,
    /// The fallback.
    pub fallback: Option<pb::SnapshotFallback>,
}

/// Reports of a copy.
pub type SyncReport = Arc<dyn Fn(pb::SnapshotSyncProgress) + Send + Sync>;

/// A machine a batch places forks on.
#[async_trait::async_trait]
pub trait BatchHost: Send + Sync {
    /// Its name in reports ([`THIS_MACHINE`] for this one).
    fn name(&self) -> String;
    /// What placement reads about it now.
    fn candidate(&self) -> Candidate;
    /// Forks `source` into a new Space `name`, counted as `cpus` and
    /// `memory_mb` there. Returns the Space's id.
    async fn fork(&self, source: &str, name: &str, cpus: u32, memory_mb: u64) -> Result<String>;
    /// Copies checkpoint `source` here from `from`.
    async fn pull(&self, source: &str, from: &SnapshotFrom, report: SyncReport) -> Result<()>;
}

/// Where the checkpoints of a batch are, for copying them.
#[async_trait::async_trait]
pub trait SnapshotOrigins: Send + Sync {
    /// Prepares copying `world` to other machines (called once per
    /// checkpoint per batch).
    async fn locate(&self, world: &str) -> Result<SnapshotFrom>;
}

/// Why a fork failed, and whether it may go to another machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// Its machine stopped answering: placed again elsewhere; the machine
    /// gets no more forks of the batch.
    HostFailed,
    /// Its machine refused it (full): placed again elsewhere.
    NoCapacity,
    /// The checkpoint could not be copied there.
    SyncFailed,
    /// The fork itself failed.
    ForkFailed,
    /// No machine could take it.
    NoHost,
}

impl Failure {
    /// The `ForkBatchItem.error_code`.
    pub fn code(self) -> &'static str {
        match self {
            Failure::HostFailed => "host_failed",
            Failure::NoCapacity => "no_capacity",
            Failure::SyncFailed => "sync_failed",
            Failure::ForkFailed => "fork_failed",
            Failure::NoHost => "no_host",
        }
    }

    fn retried(self) -> bool {
        matches!(self, Failure::HostFailed | Failure::NoCapacity)
    }
}

/// What an error from a machine means for its fork.
pub fn classify(e: &Error) -> Failure {
    use cua_spacesd_client::Error as E;
    match e {
        Error::LimitExceeded(_) => Failure::NoCapacity,
        Error::Env(E::LimitExceeded(_) | E::RateLimited { .. }) => Failure::NoCapacity,
        Error::Env(
            E::Transport(_)
            | E::Timeout(_)
            | E::TargetUnavailable(_)
            | E::SpacesdNotAvailable { .. },
        ) => Failure::HostFailed,
        Error::Env(e) if e.is_retryable() => Failure::HostFailed,
        Error::Timeout(_) | Error::Io(_) => Failure::HostFailed,
        // The fork was made but its machine stopped answering (reset,
        // refused, timed out): the machine is gone, not the checkpoint.
        Error::SpacesdNotAvailable { reason, .. }
            if ["refused", "reset", "deadline", "timed out", "broken pipe"]
                .iter()
                .any(|w| reason.contains(w)) =>
        {
            Failure::HostFailed
        }
        _ => Failure::ForkFailed,
    }
}

struct Item {
    index: u32,
    group: u32,
    source: String,
    name: String,
    attempt: u32,
}

/// A step done once per batch (a checkpoint's copy to a machine, its
/// export), shared by every fork waiting for it.
type Once<T> = Arc<OnceCell<std::result::Result<T, String>>>;

struct HostState {
    host: Arc<dyn BatchHost>,
    sem: Arc<Semaphore>,
    failed: AtomicBool,
    inflight: AtomicU32,
    peak: AtomicU32,
    /// One copy of each checkpoint per batch.
    synced: tokio::sync::Mutex<HashMap<String, Once<()>>>,
}

/// Runs a batch over `hosts` (the first one is this machine when it is a
/// candidate), sending every event to `tx` (the last one `done`) and
/// returning the summary.
pub async fn run(
    hosts: Vec<Arc<dyn BatchHost>>,
    origins: Arc<dyn SnapshotOrigins>,
    abis: BTreeMap<String, String>,
    spec: BatchSpec,
    tx: mpsc::Sender<dpb::ForkSandboxBatchResponse>,
) -> dpb::ForkBatchSummary {
    use dpb::fork_sandbox_batch_response::Event;
    let started = Instant::now();
    let send = |e: Event| {
        let tx = tx.clone();
        async move {
            let _ = tx
                .send(dpb::ForkSandboxBatchResponse {
                    event: Some(e),
                    sandbox: None,
                })
                .await;
        }
    };
    let states: Vec<Arc<HostState>> = hosts
        .iter()
        .map(|h| {
            Arc::new(HostState {
                host: h.clone(),
                sem: Arc::new(Semaphore::new(spec.max_parallel_per_host.max(1) as usize)),
                failed: AtomicBool::new(false),
                inflight: AtomicU32::new(0),
                peak: AtomicU32::new(0),
                synced: Default::default(),
            })
        })
        .collect();
    let mut cands: Vec<Candidate> = hosts.iter().map(|h| h.candidate()).collect();
    let origin_cache: Arc<tokio::sync::Mutex<HashMap<String, Once<SnapshotFrom>>>> =
        Default::default();
    let mut pending: Vec<Item> = spec
        .items()
        .into_iter()
        .enumerate()
        .map(|(i, (group, source, name))| Item {
            index: i as u32,
            group,
            source,
            name,
            attempt: 1,
        })
        .collect();
    let mut ok = 0u32;
    let mut failed = 0u32;
    let mut per_host: BTreeMap<String, u32> = BTreeMap::new();
    let mut excluded: HashSet<usize> = HashSet::new();
    let mut round = 1u32;
    let fail_item = |it: &Item, host: &str, why: String, f: Failure| dpb::ForkBatchItem {
        index: it.index,
        group: it.group,
        source: it.source.clone(),
        name: it.name.clone(),
        host: host.to_string(),
        error: why,
        error_code: f.code().into(),
        attempt: it.attempt,
        elapsed_ms: started.elapsed().as_millis() as u64,
        ..Default::default()
    };
    while !pending.is_empty() {
        // Place what is pending: groups by (group, source).
        let mut by_group: BTreeMap<u32, Vec<Item>> = BTreeMap::new();
        for it in pending.drain(..) {
            by_group.entry(it.group).or_default().push(it);
        }
        let keys: Vec<u32> = by_group.keys().copied().collect();
        let groups: Vec<BatchGroup> = keys
            .iter()
            .map(|g| {
                let items = &by_group[g];
                let world = items[0].source.clone();
                let (cpus, memory_mb) = spec.size_of(&world);
                BatchGroup {
                    abi: abis.get(&world).cloned().unwrap_or_default(),
                    world,
                    count: items.len() as u32,
                    member: Need {
                        kind: "vm".into(),
                        millicpus: cpus * 1000,
                        memory_mb,
                        ..Default::default()
                    },
                    shared_memory_mb: 0,
                }
            })
            .collect();
        let mut view = cands.clone();
        for i in &excluded {
            view[*i].ready = false;
        }
        let plan = plan_batch(&view, &groups, Policy::Spread);
        let mut tasks = FuturesUnordered::new();
        for part in &plan.parts {
            let gkey = keys[part.group];
            let st = states[part.host].clone();
            let world = groups[part.group].world.clone();
            send(Event::Placement(dpb::ForkBatchPlacement {
                group: gkey,
                source: world.clone(),
                host: st.host.name(),
                count: part.count,
                attempt: round,
                needs_sync: part.needs_sync,
                reason: part.reason.clone(),
            }))
            .await;
            let items: Vec<Item> = {
                let v = by_group.get_mut(&gkey).expect("placed group");
                let n = (part.count as usize).min(v.len());
                v.drain(..n).collect()
            };
            // The copy this part needs, shared by every part of the world
            // placed on that host.
            let sync_cell = if part.needs_sync {
                Some(
                    st.synced
                        .lock()
                        .await
                        .entry(world.clone())
                        .or_default()
                        .clone(),
                )
            } else {
                None
            };
            let origin_cell = origin_cache
                .lock()
                .await
                .entry(world.clone())
                .or_default()
                .clone();
            for it in items {
                let st = st.clone();
                let sync_cell = sync_cell.clone();
                let origin_cell = origin_cell.clone();
                let origins = origins.clone();
                let tx = tx.clone();
                let spec = &spec;
                let world = world.clone();
                let host_index = part.host;
                tasks.push(async move {
                    let host = st.host.name();
                    if let Some(cell) = sync_cell {
                        let r = cell
                            .get_or_init(|| async {
                                let from = origin_cell
                                    .get_or_init(|| async {
                                        origins.locate(&world).await.map_err(|e| e.to_string())
                                    })
                                    .await
                                    .clone()?;
                                let tx = tx.clone();
                                let h = host.clone();
                                let report: SyncReport = Arc::new(move |mut p| {
                                    if p.from.is_empty() {
                                        p.from = h.clone();
                                    }
                                    let _ = tx.try_send(dpb::ForkSandboxBatchResponse {
                                        sandbox: None,
                                        event: Some(Event::Sync(p)),
                                    });
                                });
                                st.host
                                    .pull(&world, &from, report)
                                    .await
                                    .map_err(|e| e.to_string())
                            })
                            .await
                            .clone();
                        if let Err(why) = r {
                            return (host_index, it, Err((Failure::SyncFailed, why)), 0);
                        }
                    }
                    let _permit = st.sem.clone().acquire_owned().await;
                    if st.failed.load(Ordering::SeqCst) {
                        return (
                            host_index,
                            it,
                            Err((
                                Failure::HostFailed,
                                format!("{host} failed earlier in this batch"),
                            )),
                            0,
                        );
                    }
                    let now = st.inflight.fetch_add(1, Ordering::SeqCst) + 1;
                    st.peak.fetch_max(now, Ordering::SeqCst);
                    let t = Instant::now();
                    let (cpus, memory_mb) = spec.size_of(&it.source);
                    let r = st.host.fork(&it.source, &it.name, cpus, memory_mb).await;
                    st.inflight.fetch_sub(1, Ordering::SeqCst);
                    let ms = t.elapsed().as_millis() as u64;
                    match r {
                        Ok(space) => (host_index, it, Ok(space), ms),
                        Err(e) => {
                            let f = classify(&e);
                            if f == Failure::HostFailed {
                                st.failed.store(true, Ordering::SeqCst);
                            }
                            (host_index, it, Err((f, e.to_string())), ms)
                        }
                    }
                });
            }
        }
        // What fits nowhere.
        for u in &plan.unplaced {
            let gkey = keys[u.group];
            if let Some(v) = by_group.get_mut(&gkey) {
                for it in v.drain(..) {
                    failed += 1;
                    send(Event::Item(fail_item(
                        &it,
                        "",
                        no_host_message(&u.reason, spec.size_of(&it.source).0),
                        Failure::NoHost,
                    )))
                    .await;
                }
            }
        }
        let mut retry = Vec::new();
        while let Some((hi, it, r, fork_ms)) = tasks.next().await {
            let host = states[hi].host.name();
            match r {
                Ok(space) => {
                    ok += 1;
                    *per_host.entry(host.clone()).or_default() += 1;
                    // Placed for good: counted against the host from now on.
                    let (cpus, memory_mb) = spec.size_of(&it.source);
                    cands[hi].pending_millicpus += u64::from(cpus) * 1000;
                    cands[hi].pending_memory_mb += memory_mb;
                    cands[hi].pending_jobs += 1;
                    send(Event::Item(dpb::ForkBatchItem {
                        index: it.index,
                        group: it.group,
                        source: it.source.clone(),
                        name: it.name.clone(),
                        host,
                        space,
                        attempt: it.attempt,
                        elapsed_ms: started.elapsed().as_millis() as u64,
                        fork_ms,
                        ..Default::default()
                    }))
                    .await;
                }
                Err((f, why)) => {
                    // A machine that failed, or refused for lack of room,
                    // gets no more forks of this batch.
                    if f.retried() {
                        excluded.insert(hi);
                    }
                    if f.retried() && round <= spec.retries {
                        tracing::info!(fork = %it.name, %host, "placing the fork again: {why}");
                        retry.push(Item {
                            attempt: it.attempt + 1,
                            ..it
                        });
                    } else {
                        failed += 1;
                        send(Event::Item(fail_item(&it, &host, why, f))).await;
                    }
                }
            }
        }
        pending = retry;
        pending.sort_by_key(|i| i.index);
        round += 1;
    }
    let summary = dpb::ForkBatchSummary {
        ok,
        failed,
        elapsed_ms: started.elapsed().as_millis() as u64,
        per_host: per_host.into_iter().collect(),
        peak_parallel: states
            .iter()
            .filter(|s| s.peak.load(Ordering::SeqCst) > 0)
            .map(|s| (s.host.name(), s.peak.load(Ordering::SeqCst)))
            .collect(),
    };
    send(Event::Done(summary.clone())).await;
    summary
}

/// This machine, as a batch host.
struct LocalHost {
    spaces: crate::spaces::Spaces,
    cand: Candidate,
}

fn manifest_of(b: &[u8]) -> Result<Option<cua_snapshot_sync::Manifest>> {
    if b.is_empty() {
        return Ok(None);
    }
    cua_snapshot_sync::Manifest::from_bytes(b)
        .map(Some)
        .map_err(|e| Error::invalid(format!("snapshot manifest: {e}")))
}

#[async_trait::async_trait]
impl BatchHost for LocalHost {
    fn name(&self) -> String {
        THIS_MACHINE.into()
    }
    fn candidate(&self) -> Candidate {
        self.cand.clone()
    }
    async fn fork(&self, source: &str, name: &str, _: u32, _: u64) -> Result<String> {
        Ok(self
            .spaces
            .fork_local(source, Some(name.to_string()))
            .await?
            .id)
    }
    async fn pull(&self, source: &str, from: &SnapshotFrom, report: SyncReport) -> Result<()> {
        let progress: cua_snapshot_sync::ProgressFn =
            Arc::new(move |p| report(crate::snapshots::progress_to_pb(&p)));
        self.spaces
            .pull_snapshot(
                manifest_of(&from.manifest)?,
                source,
                &from.sources,
                from.fallback.as_ref(),
                Some(progress),
            )
            .await
            .map(|_| ())
    }
}

/// One of your other machines, as a batch host.
struct RemoteHost {
    spaces: crate::spaces::Spaces,
    /// What `on="host:<id>"` takes.
    id: String,
    cand: Candidate,
}

#[async_trait::async_trait]
impl BatchHost for RemoteHost {
    fn name(&self) -> String {
        self.cand.name.clone()
    }
    fn candidate(&self) -> Candidate {
        self.cand.clone()
    }
    async fn fork(&self, source: &str, name: &str, cpus: u32, memory_mb: u64) -> Result<String> {
        Ok(self
            .spaces
            .create_on_host_from(
                &self.id,
                crate::SpaceCreate {
                    name: Some(name.to_string()),
                    cpus: Some(cpus),
                    memory_mb: Some(memory_mb),
                    ..Default::default()
                },
                Some(source),
            )
            .await?
            .id)
    }
    async fn pull(&self, source: &str, from: &SnapshotFrom, report: SyncReport) -> Result<()> {
        let (space, _) = self.spaces.host_of(&self.id).await?;
        let label = self.name();
        crate::host_spaces::guard_host_rpc(&space, &label, "copying checkpoints")?;
        let mut stream = space
            .spacesd()?
            .host_snapshots()
            .pull_host_snapshot(pb::PullHostSnapshotRequest {
                manifest: from.manifest.clone(),
                sources: from.sources.clone(),
                fallback: from.fallback.clone(),
                name: source.to_string(),
            })
            .await
            .map_err(|e| crate::host_spaces::from_new_host_rpc(e, &label, "copying checkpoints"))?
            .into_inner();
        loop {
            match stream.message().await {
                Ok(Some(m)) => {
                    if let Some(p) = m.progress {
                        let mut p = p;
                        if p.from.is_empty() {
                            p.from = self.cand.name.clone();
                        }
                        report(p);
                    }
                }
                Ok(None) => return Ok(()),
                Err(e) => return Err(crate::host_spaces::from_host(e.into())),
            }
        }
    }
}

/// Where checkpoints are: this machine or another that reported holding
/// it, else the fallback.
struct Origins {
    spaces: crate::spaces::Spaces,
    /// Holders of each checkpoint: `None` this machine, `Some(id)` another.
    holders: BTreeMap<String, Vec<Option<String>>>,
    fallback: Option<pb::SnapshotFallback>,
}

#[async_trait::async_trait]
impl SnapshotOrigins for Origins {
    async fn locate(&self, world: &str) -> Result<SnapshotFrom> {
        let fb = self.fallback.clone();
        let holders = self.holders.get(world).cloned().unwrap_or_default();
        let Some(holder) = holders.first().cloned() else {
            // Only in the fallback: hosts read it there by name.
            if crate::snapshots::has_fallback(fb.as_ref()) {
                return Ok(SnapshotFrom {
                    manifest: vec![],
                    sources: vec![],
                    fallback: fb,
                });
            }
            return Err(Error::NotFound(format!(
                "checkpoint {world} (no machine of yours holds it)"
            )));
        };
        let (manifest, grant, urls, label) = match &holder {
            None => {
                let (m, d) = self.spaces.export_snapshot(world).await?;
                let grant = self.spaces.snapshot_grant(&d, 3600)?;
                (
                    m.to_bytes(),
                    grant,
                    crate::host_spaces::snapshot_urls_of(self.spaces.home_dir()),
                    THIS_MACHINE.to_string(),
                )
            }
            Some(id) => {
                let (space, ours) = self.spaces.host_of(id).await?;
                let label = self.spaces.host_label(id).await;
                crate::host_spaces::guard_host_rpc(&space, &label, "copying checkpoints")?;
                let r = space
                    .spacesd()?
                    .host_snapshots()
                    .export_host_snapshot(pb::ExportHostSnapshotRequest {
                        name: world.to_string(),
                        grant_ttl_secs: 3600,
                    })
                    .await
                    .map_err(|e| {
                        crate::host_spaces::from_new_host_rpc(e, &label, "copying checkpoints")
                    })?
                    .into_inner();
                let mut urls = r.urls;
                // The address this machine reaches it at, too.
                if let Some(u) = ours
                    && !urls.contains(&u)
                {
                    urls.push(u);
                }
                (r.manifest, r.grant, urls, id.clone())
            }
        };
        if urls.is_empty() && crate::snapshots::has_fallback(fb.as_ref()) {
            // The holder serves no files to other machines (it is not set
            // up with `--direct`): it publishes to the fallback first.
            let fallback = fb.clone().unwrap_or_default();
            match &holder {
                None => {
                    self.spaces.publish_snapshot(world, &fallback, None).await?;
                }
                Some(id) => {
                    let (space, _) = self.spaces.host_of(id).await?;
                    let label = self.spaces.host_label(id).await;
                    crate::host_spaces::guard_host_rpc(&space, &label, "copying checkpoints")?;
                    let mut s = space
                        .spacesd()?
                        .host_snapshots()
                        .publish_host_snapshot(pb::PublishHostSnapshotRequest {
                            name: world.to_string(),
                            fallback: Some(fallback),
                        })
                        .await
                        .map_err(|e| {
                            crate::host_spaces::from_new_host_rpc(e, &label, "copying checkpoints")
                        })?
                        .into_inner();
                    while s
                        .message()
                        .await
                        .map_err(|e| crate::host_spaces::from_host(e.into()))?
                        .is_some()
                    {}
                }
            }
        }
        Ok(SnapshotFrom {
            manifest,
            sources: urls
                .into_iter()
                .map(|url| pb::SnapshotSource {
                    url,
                    grant: grant.clone(),
                    name: label.clone(),
                })
                .collect(),
            fallback: fb,
        })
    }
}

impl crate::spaces::Spaces {
    /// Runs a batch of forks ([`run`]) where `spec.on` says: this machine,
    /// your machines that provide Spaces (`auto`, this one included), or
    /// one of them (`host:<machine>`). Every event goes to `tx`.
    pub async fn fork_batch(
        &self,
        spec: BatchSpec,
        tx: mpsc::Sender<dpb::ForkSandboxBatchResponse>,
    ) -> Result<dpb::ForkBatchSummary> {
        let (snaps, abis) = self.local_snapshots().await;
        let mut status = cua_cluster::sysinfo::this_machine_status();
        status.snapshots = snaps.into_iter().collect();
        status.snapshot_abis = abis;
        let local = Candidate {
            name: THIS_MACHINE.into(),
            ready: true,
            os: std::env::consts::OS.into(),
            status: Some(status),
            ..Default::default()
        };
        let mut hosts: Vec<Arc<dyn BatchHost>> = Vec::new();
        // `None` for this machine, else what `host_of` takes.
        let mut ids: Vec<Option<String>> = Vec::new();
        // Machines the batch does not place on (`host:<machine>` names
        // another) that can still be a checkpoint's origin.
        let mut origins_only: Vec<(String, Candidate)> = Vec::new();
        let on_owned = spec.on.clone();
        let on = on_owned.as_str();
        if on.is_empty() || on == "local" || on == "auto" {
            hosts.push(Arc::new(LocalHost {
                spaces: self.clone(),
                cand: local,
            }));
            ids.push(None);
        }
        // On this machine only, a checkpoint it lacks is copied from the
        // machine of yours that holds it: those are origins, not targets.
        let local_only = on.is_empty() || on == "local";
        let lacks_one = local_only
            && spec
                .groups
                .iter()
                .any(|g| self.sandboxes().checkpoint_record(&g.source).is_none());
        if on == "auto" || on.starts_with("host:") || lacks_one {
            let want = on.strip_prefix("host:").map(str::trim);
            // Boxed as a `Send` future: the host listing's future is deep,
            // and the batch's own future is spawned.
            let listing: std::pin::Pin<
                Box<
                    dyn std::future::Future<Output = Result<Vec<crate::host_spaces::HostOffer>>>
                        + Send
                        + '_,
                >,
            > = Box::pin(self.hosts());
            let offers = listing.await.unwrap_or_default();
            for h in offers {
                if local_only
                    || (want.is_some_and(|w| h.id != w && !h.name.eq_ignore_ascii_case(w)))
                {
                    origins_only.push((
                        h.id.clone(),
                        Candidate {
                            name: h.name.clone(),
                            ready: h.online,
                            os: h.os.clone(),
                            status: h.node.as_ref().map(|n| n.status()),
                            ..Default::default()
                        },
                    ));
                    continue;
                }
                ids.push(Some(h.id.clone()));
                hosts.push(Arc::new(RemoteHost {
                    spaces: self.clone(),
                    id: h.id.clone(),
                    cand: Candidate {
                        name: h.name.clone(),
                        ready: h.online,
                        os: h.os.clone(),
                        status: h.node.as_ref().map(|n| n.status()),
                        ..Default::default()
                    },
                }));
            }
            if let Some(w) = want
                && hosts.is_empty()
                && !local_only
            {
                return Err(Error::NotFound(format!(
                    "a machine named {w} that provides Spaces"
                )));
            }
        }
        let mut spec = spec;
        // A running (or stopped) sandbox as a group's source: one implied
        // checkpoint for all its forks, held by them and deleted with the
        // last (E2B `fork(count=n)`, Daytona `fork`).
        let mut implied: Vec<String> = Vec::new();
        for g in spec.groups.iter_mut() {
            if self.sandboxes().checkpoint_record(&g.source).is_none()
                && self.sandboxes().state().load(&g.source).is_some()
            {
                let rec = self.sandboxes().checkpoint_for_forks(&g.source).await?;
                g.source = rec.name.clone();
                implied.push(rec.name);
            }
        }
        for g in &spec.groups.clone() {
            if let Some(rec) = self.sandboxes().checkpoint_record(&g.source) {
                spec.sizes.insert(
                    g.source.clone(),
                    (
                        DEFAULT_FORK_CPUS,
                        rec.template.memory_mb.unwrap_or(DEFAULT_FORK_MEMORY_MB),
                    ),
                );
            }
        }
        // Who holds each checkpoint, and its ABI (the holder's).
        let mut holders: BTreeMap<String, Vec<Option<String>>> = BTreeMap::new();
        let mut abis: BTreeMap<String, String> = BTreeMap::new();
        let worlds: std::collections::BTreeSet<String> =
            spec.groups.iter().map(|g| g.source.clone()).collect();
        for w in &worlds {
            for (h, id) in hosts.iter().zip(&ids) {
                let c = h.candidate();
                let Some(st) = c.status.as_ref() else {
                    continue;
                };
                if st.snapshots.contains_key(w) {
                    holders.entry(w.clone()).or_default().push(id.clone());
                    if let Some(a) = st.snapshot_abis.first() {
                        abis.entry(w.clone()).or_insert_with(|| a.clone());
                    }
                }
            }
            for (id, c) in &origins_only {
                if let Some(st) = c.status.as_ref()
                    && c.ready
                    && st.snapshots.contains_key(w)
                {
                    holders.entry(w.clone()).or_default().push(Some(id.clone()));
                    if let Some(a) = st.snapshot_abis.first() {
                        abis.entry(w.clone()).or_insert_with(|| a.clone());
                    }
                }
            }
            // This machine holds it even when it is not a candidate
            // (`host:<machine>`): it is still the origin.
            if !holders.contains_key(w) && self.sandboxes().checkpoint_record(w).is_some() {
                holders.entry(w.clone()).or_default().push(None);
                if let Some(a) = self.sandboxes().snapshot_abis().await.first() {
                    abis.insert(w.clone(), a.clone());
                }
            }
            if !abis.contains_key(w)
                && let Ok(Some(fb)) = crate::snapshots::fallback_store(spec.fallback.as_ref()).await
                && let Ok(m) = fb.manifest_by_name(w).await
            {
                abis.insert(w.clone(), m.abi);
            }
        }
        // No location given: this machine, overflowing into your linked
        // clouds when it has no room and a budget is set (the rule of every
        // create without a location; an explicit `local` never overflows).
        // Boxed as a `Send` future, like the host listing above.
        let overflow = if on.is_empty() {
            let allowed: std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send + '_>> =
                Box::pin(self.overflow_allowed());
            allowed.await
        } else {
            false
        };
        let growth = batch_growth(on, overflow);
        // No room on your machines: the elastic pool adds the ones the
        // batch needs (the same solve as every `on="auto"` placement; an
        // overflow buys cloud machines only).
        if let Some(clouds_only) = growth {
            let mut need: BTreeMap<String, u32> = BTreeMap::new();
            for g in &spec.groups {
                *need.entry(g.source.clone()).or_default() += g.count;
            }
            let worlds: Vec<(String, String, u32, u32, u64)> = need
                .into_iter()
                .filter_map(|(w, n)| {
                    let abi = abis.get(&w)?.clone();
                    let (cpus, mem) = spec.size_of(&w);
                    Some((w, abi, n, cpus, mem))
                })
                .collect();
            let fits = cua_cluster::batch::plan_batch(
                &hosts.iter().map(|h| h.candidate()).collect::<Vec<_>>(),
                &worlds
                    .iter()
                    .map(|(w, abi, n, cpus, mem)| BatchGroup {
                        world: w.clone(),
                        count: *n,
                        member: Need {
                            kind: "vm".into(),
                            millicpus: cpus * 1000,
                            memory_mb: *mem,
                            ..Default::default()
                        },
                        shared_memory_mb: 0,
                        abi: abi.clone(),
                    })
                    .collect::<Vec<_>>(),
                Policy::Spread,
            )
            .unplaced
            .is_empty();
            if !fits {
                let joined = self
                    .elastic_grow_for_forks(&worlds, clouds_only)
                    .await
                    .unwrap_or_else(|e| {
                        tracing::warn!("the elastic pool added no machine for the forks: {e}");
                        vec![]
                    });
                if !joined.is_empty() {
                    for h in self.hosts().await.unwrap_or_default() {
                        if joined.contains(&h.id) && !ids.contains(&Some(h.id.clone())) {
                            ids.push(Some(h.id.clone()));
                            hosts.push(Arc::new(RemoteHost {
                                spaces: self.clone(),
                                id: h.id.clone(),
                                cand: Candidate {
                                    name: h.name.clone(),
                                    ready: h.online,
                                    os: h.os.clone(),
                                    status: h.node.as_ref().map(|n| n.status()),
                                    ..Default::default()
                                },
                            }));
                        }
                    }
                }
            }
        }
        let origins = Arc::new(Origins {
            spaces: self.clone(),
            holders,
            fallback: spec.fallback.clone(),
        });
        let summary = run(hosts, origins, abis, spec, tx).await;
        // Forks hold an implied checkpoint; one none of them holds (every
        // fork failed, or they all went to other machines, which copied it)
        // goes now.
        for name in implied {
            self.sandboxes().release_if_unheld(&name).await;
        }
        Ok(summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::Duration;

    const ABI: &str = "qemu/hvf/aarch64";

    /// A batch with no location overflows into the clouds (cloud machines
    /// only) when that is allowed, like a create; `auto` grows the pool
    /// with any machine; an explicit `local` or host never grows it.
    #[test]
    fn a_batch_with_no_location_overflows_like_a_create() {
        assert_eq!(batch_growth("", true), Some(true));
        assert_eq!(batch_growth("", false), None);
        assert_eq!(batch_growth("auto", false), Some(false));
        assert_eq!(batch_growth("auto", true), Some(false));
        assert_eq!(batch_growth("local", true), None);
        assert_eq!(batch_growth("host:kvm-a", true), None);
    }

    #[derive(Default)]
    struct Fake {
        name: String,
        holds: Vec<String>,
        mem_gb: u64,
        /// Fails every fork with a transport error after this many.
        dies_after: Option<u32>,
        /// Refuses (full) after this many.
        full_after: Option<u32>,
        /// Forks that fail on their own (by name).
        bad: Vec<String>,
        forks: Mutex<Vec<String>>,
        pulls: Mutex<Vec<String>>,
        inflight: AtomicU32,
        peak: AtomicU32,
    }

    #[async_trait::async_trait]
    impl BatchHost for Fake {
        fn name(&self) -> String {
            self.name.clone()
        }
        fn candidate(&self) -> Candidate {
            Candidate {
                name: self.name.clone(),
                ready: true,
                os: "macos".into(),
                status: Some(pb::HostNodeStatus {
                    millicpus_total: 64_000,
                    millicpus_free: 64_000,
                    memory_total_mb: self.mem_gb * 1024,
                    memory_free_mb: self.mem_gb * 1024,
                    kinds: vec!["vm".into()],
                    snapshot_abis: vec![ABI.into()],
                    snapshots: self
                        .holds
                        .iter()
                        .map(|w| (w.clone(), String::new()))
                        .collect(),
                    ..Default::default()
                }),
                ..Default::default()
            }
        }
        async fn fork(&self, source: &str, name: &str, _: u32, _: u64) -> Result<String> {
            let n = self.inflight.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(n, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(20)).await;
            self.inflight.fetch_sub(1, Ordering::SeqCst);
            let done = self.forks.lock().unwrap().len() as u32;
            if self.dies_after.is_some_and(|d| done >= d) {
                return Err(Error::Env(cua_spacesd_client::Error::Transport(
                    "connection refused".into(),
                )));
            }
            if self.full_after.is_some_and(|d| done >= d) {
                return Err(Error::LimitExceeded(format!("{} is full", self.name)));
            }
            if self.bad.iter().any(|b| b == name) {
                return Err(Error::invalid(format!("{name} did not start")));
            }
            assert!(
                self.holds.iter().any(|w| w == source)
                    || self.pulls.lock().unwrap().iter().any(|w| w == source),
                "{} forked {source} without holding it",
                self.name
            );
            self.forks.lock().unwrap().push(name.to_string());
            Ok(format!("direct:{}:{name}", self.name))
        }
        async fn pull(&self, source: &str, from: &SnapshotFrom, report: SyncReport) -> Result<()> {
            assert_eq!(from.sources[0].name, "origin");
            self.pulls.lock().unwrap().push(source.to_string());
            report(pb::SnapshotSyncProgress {
                name: source.into(),
                bytes_done: 10,
                bytes_total: 10,
                done: true,
                verified: true,
                ..Default::default()
            });
            Ok(())
        }
    }

    struct Origins(Mutex<Vec<String>>);
    #[async_trait::async_trait]
    impl SnapshotOrigins for Origins {
        async fn locate(&self, world: &str) -> Result<SnapshotFrom> {
            self.0.lock().unwrap().push(world.to_string());
            Ok(SnapshotFrom {
                sources: vec![pb::SnapshotSource {
                    name: "origin".into(),
                    url: "http://origin".into(),
                    grant: "g".into(),
                }],
                ..Default::default()
            })
        }
    }

    fn spec(worlds: &[&str], per: u32) -> BatchSpec {
        BatchSpec::from_request(dpb::ForkSandboxBatchRequest {
            groups: worlds
                .iter()
                .map(|w| dpb::ForkBatchGroup {
                    source: (*w).into(),
                    count: per,
                    name_prefix: (*w).into(),
                })
                .collect(),
            on: "auto".into(),
            max_parallel_per_host: 3,
            memory_mb: 1024,
            cpus: 1,
            ..Default::default()
        })
        .unwrap()
    }

    async fn go(
        hosts: Vec<Arc<Fake>>,
        spec: BatchSpec,
    ) -> (
        dpb::ForkBatchSummary,
        Vec<dpb::fork_sandbox_batch_response::Event>,
        Arc<Origins>,
    ) {
        let (tx, mut rx) = mpsc::channel(1024);
        let origins = Arc::new(Origins(Mutex::new(vec![])));
        let abis = spec
            .groups
            .iter()
            .map(|g| (g.source.clone(), ABI.to_string()))
            .collect();
        let dyn_hosts: Vec<Arc<dyn BatchHost>> = hosts
            .iter()
            .map(|h| h.clone() as Arc<dyn BatchHost>)
            .collect();
        let summary = run(dyn_hosts, origins.clone(), abis, spec, tx).await;
        let mut events = vec![];
        while let Ok(e) = rx.try_recv() {
            events.push(e.event.unwrap());
        }
        (summary, events, origins)
    }

    fn items(events: &[dpb::fork_sandbox_batch_response::Event]) -> Vec<dpb::ForkBatchItem> {
        events
            .iter()
            .filter_map(|e| match e {
                dpb::fork_sandbox_batch_response::Event::Item(i) => Some(i.clone()),
                _ => None,
            })
            .collect()
    }

    /// 8 worlds x 8 forks over two Macs: each world's forks on one Mac,
    /// worlds split evenly, at most 3 forks at once per Mac, each world
    /// copied once to the Mac that lacked it, every fork streamed, `done`
    /// last.
    #[tokio::test]
    async fn sixty_four_forks_keep_groups_whole_and_bound_parallelism() {
        let worlds: Vec<String> = (0..8).map(|i| format!("w{i}")).collect();
        let a = Arc::new(Fake {
            name: "mac-a".into(),
            holds: worlds.clone(),
            mem_gb: 64,
            ..Default::default()
        });
        let b = Arc::new(Fake {
            name: "mac-b".into(),
            mem_gb: 64,
            ..Default::default()
        });
        let wrefs: Vec<&str> = worlds.iter().map(String::as_str).collect();
        let (sum, events, origins) = go(vec![a.clone(), b.clone()], spec(&wrefs, 8)).await;
        assert_eq!((sum.ok, sum.failed), (64, 0), "{sum:?}");
        let its = items(&events);
        assert_eq!(its.len(), 64);
        // Group affinity: every world on exactly one host.
        let mut host_of: BTreeMap<String, HashSet<String>> = BTreeMap::new();
        for i in &its {
            host_of
                .entry(i.source.clone())
                .or_default()
                .insert(i.host.clone());
        }
        assert!(host_of.values().all(|h| h.len() == 1), "{host_of:?}");
        // Worlds spread: both machines used.
        assert!(sum.per_host.len() == 2, "{sum:?}");
        // Bounded parallelism, measured on the hosts themselves too.
        assert!(a.peak.load(Ordering::SeqCst) <= 3 && b.peak.load(Ordering::SeqCst) <= 3);
        assert!(sum.peak_parallel.values().all(|p| *p <= 3));
        // Each world b runs was copied there once, and located once.
        let pulls = b.pulls.lock().unwrap().clone();
        let mut uniq = pulls.clone();
        uniq.dedup();
        assert_eq!(pulls.len(), uniq.len());
        assert_eq!(origins.0.lock().unwrap().len(), pulls.len());
        assert!(a.pulls.lock().unwrap().is_empty());
        // `done` is the last event.
        assert!(matches!(
            events.last(),
            Some(dpb::fork_sandbox_batch_response::Event::Done(_))
        ));
        // Indexes are each reported once.
        let mut idx: Vec<u32> = its.iter().map(|i| i.index).collect();
        idx.sort();
        assert_eq!(idx, (0..64).collect::<Vec<_>>());
    }

    /// A machine that dies mid-batch: its forks go to the other machine
    /// (attempt 2), nothing more is sent to it, and the batch completes.
    #[tokio::test]
    async fn a_failed_host_has_its_forks_placed_again() {
        let worlds = ["w0", "w1", "w2", "w3"];
        let a = Arc::new(Fake {
            name: "mac-a".into(),
            holds: worlds.iter().map(|s| s.to_string()).collect(),
            mem_gb: 64,
            ..Default::default()
        });
        let b = Arc::new(Fake {
            name: "mac-b".into(),
            holds: worlds.iter().map(|s| s.to_string()).collect(),
            mem_gb: 64,
            dies_after: Some(2),
            ..Default::default()
        });
        let (sum, events, _) = go(vec![a.clone(), b.clone()], spec(&worlds, 8)).await;
        assert_eq!((sum.ok, sum.failed), (32, 0), "{sum:?}");
        let its = items(&events);
        let moved: Vec<_> = its.iter().filter(|i| i.attempt == 2).collect();
        assert!(!moved.is_empty());
        assert!(moved.iter().all(|i| i.host == "mac-a"));
        assert_eq!(b.forks.lock().unwrap().len(), 2);
        // The re-placement was announced.
        assert!(events.iter().any(|e| matches!(e,
            dpb::fork_sandbox_batch_response::Event::Placement(p) if p.attempt == 2 && p.host == "mac-a")));
        // Without retries the failures are reported per fork instead.
        let b2 = Arc::new(Fake {
            name: "mac-b".into(),
            holds: worlds.iter().map(|s| s.to_string()).collect(),
            mem_gb: 64,
            dies_after: Some(0),
            ..Default::default()
        });
        let mut s = spec(&worlds, 8);
        s.retries = 0;
        let (sum, events, _) = go(vec![b2], s).await;
        assert_eq!((sum.ok, sum.failed), (0, 32));
        let its = items(&events);
        assert!(its.iter().all(|i| i.error_code == "host_failed"), "{its:?}");
    }

    /// One fork that fails on its own is reported alone; the rest of the
    /// batch is ready.
    #[tokio::test]
    async fn a_failed_fork_does_not_stop_the_batch() {
        let a = Arc::new(Fake {
            name: "mac-a".into(),
            holds: vec!["w0".into()],
            mem_gb: 64,
            bad: vec!["w0-3".into()],
            ..Default::default()
        });
        let (sum, events, _) = go(vec![a], spec(&["w0"], 8)).await;
        assert_eq!((sum.ok, sum.failed), (7, 1));
        let bad: Vec<_> = items(&events)
            .into_iter()
            .filter(|i| !i.error.is_empty())
            .collect();
        assert_eq!(bad.len(), 1);
        assert_eq!(bad[0].name, "w0-3");
        assert_eq!(bad[0].error_code, "fork_failed");
        assert!(bad[0].space.is_empty());
    }

    /// A full machine (admission refused) has the rest go elsewhere.
    #[tokio::test]
    async fn a_full_host_spills_to_another() {
        let a = Arc::new(Fake {
            name: "mac-a".into(),
            holds: vec!["w0".into()],
            mem_gb: 64,
            full_after: Some(5),
            ..Default::default()
        });
        let b = Arc::new(Fake {
            name: "mac-b".into(),
            mem_gb: 64,
            ..Default::default()
        });
        let (sum, events, _) = go(vec![a, b.clone()], spec(&["w0"], 8)).await;
        assert_eq!((sum.ok, sum.failed), (8, 0), "{sum:?}");
        assert_eq!(sum.per_host["mac-a"], 5);
        assert_eq!(sum.per_host["mac-b"], 3);
        assert_eq!(b.pulls.lock().unwrap().as_slice(), ["w0"]);
        assert!(
            items(&events)
                .iter()
                .filter(|i| i.host == "mac-b")
                .all(|i| i.attempt == 2)
        );
    }

    /// Nothing fits: every fork is reported with why.
    #[tokio::test]
    async fn no_host_is_reported_per_fork() {
        let tiny = Arc::new(Fake {
            name: "tiny".into(),
            holds: vec!["w0".into()],
            mem_gb: 2,
            ..Default::default()
        });
        let (sum, events, _) = go(vec![tiny], spec(&["w0"], 4)).await;
        assert_eq!((sum.ok, sum.failed), (2, 2), "{sum:?}");
        let failed: Vec<_> = items(&events)
            .into_iter()
            .filter(|i| i.error_code == "no_host")
            .collect();
        assert_eq!(failed.len(), 2);
        assert!(
            failed[0].error.contains("not enough memory"),
            "{}",
            failed[0].error
        );
    }

    /// A fork books one vCPU unless asked otherwise; an out-of-CPU report
    /// says what to do.
    /// A fork whose new Space never answers because its machine died is a
    /// machine failure (placed again), not a failed fork.
    #[test]
    fn a_space_that_stops_answering_is_a_host_failure() {
        let gone = Error::SpacesdNotAvailable {
            space: "direct:10.0.0.1:4000".into(),
            reason: "gRPC: tcp connect error (Connection refused (os error 61))".into(),
        };
        assert_eq!(classify(&gone), Failure::HostFailed);
        let no_spacesd = Error::SpacesdNotAvailable {
            space: "x".into(),
            reason: "this image does not run it".into(),
        };
        assert_eq!(classify(&no_spacesd), Failure::ForkFailed);
    }

    #[test]
    fn forks_book_one_vcpu_by_default_and_cpu_shortage_says_what_to_do() {
        let mut s = spec(&["w0"], 4);
        s.cpus = None;
        s.memory_mb = None;
        s.sizes.insert("w0".into(), (8, 4096));
        assert_eq!(
            s.size_of("w0"),
            (1, 4096),
            "the checkpoint's vCPUs are not booked"
        );
        s.cpus = Some(3);
        assert_eq!(s.size_of("w0").0, 3);
        let m = no_host_message("local has not enough CPU free", 1);
        assert!(
            m.contains("--cpus") && m.contains("--on auto") && m.contains("1 vCPU"),
            "{m}"
        );
        assert!(!no_host_message("local has not enough memory free", 1).contains("--cpus"));
    }

    #[test]
    fn a_single_named_fork_keeps_its_name() {
        let mut s = spec(&["w"], 1);
        s.groups[0].name_prefix = "agent-7".into();
        s.exact_names = true;
        assert_eq!(s.items(), vec![(0, "w".to_string(), "agent-7".to_string())]);
        s.exact_names = false;
        assert_eq!(s.items()[0].2, "agent-7-0");
        // Groups of more than one are numbered either way.
        let mut s = spec(&["w"], 2);
        s.exact_names = true;
        assert_eq!(s.items()[1].2, "w-1");
    }

    #[test]
    fn fork_names_are_the_names_they_are_registered_under() {
        // What `fork_local` registers is `sanitize_label(name)`: the batch
        // reports that name, so connect and delete find it.
        for prefix in [
            "Run_A",
            "fm-",
            "py-ab12c-fm",
            "weird name!",
            &"x".repeat(80),
        ] {
            let mut s = spec(&["w"], 12);
            s.groups[0].name_prefix = prefix.into();
            let names: Vec<String> = s.items().into_iter().map(|i| i.2).collect();
            for n in &names {
                assert_eq!(&crate::sanitize_label(n), n, "{prefix:?} -> {n:?}");
                assert!(n.len() <= 63);
            }
            let unique: std::collections::BTreeSet<_> = names.iter().collect();
            assert_eq!(unique.len(), names.len(), "{prefix:?}: distinct names");
        }
        let mut s = spec(&["w"], 1);
        s.groups[0].name_prefix = "Run_A".into();
        assert_eq!(s.items()[0].2, "run-a-0");
        s.exact_names = true;
        assert_eq!(s.items()[0].2, "run-a");
    }

    #[test]
    fn requests_expand_and_validate() {
        let s = BatchSpec::from_request(dpb::ForkSandboxBatchRequest {
            source: "w".into(),
            count: 20,
            group_size: 8,
            name_prefix: "run".into(),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(
            s.groups.iter().map(|g| g.count).collect::<Vec<_>>(),
            [8, 8, 4]
        );
        assert_eq!(s.groups[1].name_prefix, "run-g1");
        assert_eq!(s.items()[9].2, "run-g1-1");
        assert_eq!(s.max_parallel_per_host, DEFAULT_PARALLEL_PER_HOST);
        assert_eq!(s.retries, 1);
        for bad in [
            dpb::ForkSandboxBatchRequest::default(),
            dpb::ForkSandboxBatchRequest {
                source: "w".into(),
                ..Default::default()
            },
            dpb::ForkSandboxBatchRequest {
                source: "w".into(),
                count: 5000,
                ..Default::default()
            },
            dpb::ForkSandboxBatchRequest {
                source: "w".into(),
                count: 1,
                on: "cloud".into(),
                ..Default::default()
            },
        ] {
            assert!(BatchSpec::from_request(bad).is_err());
        }
        assert_eq!(
            classify(&Error::Env(cua_spacesd_client::Error::Transport(
                "x".into()
            ))),
            Failure::HostFailed
        );
        assert_eq!(
            classify(&Error::LimitExceeded("full".into())),
            Failure::NoCapacity
        );
        assert_eq!(classify(&Error::invalid("x")), Failure::ForkFailed);
    }
}
