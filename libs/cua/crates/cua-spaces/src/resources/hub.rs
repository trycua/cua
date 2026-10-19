//! The sampler, its cache and its watchers.
//!
//! One [`Hub`] per runtime. Nothing runs until somebody is interested: a
//! watcher ([`Spaces::watch_resources`]) or a poll that says a window is
//! visible ([`Spaces::resources`] with `visible`). While interest lasts, a
//! task samples every [`VISIBLE_TICK`] (a visible window) or [`HIDDEN_TICK`]
//! (only hidden watchers, for the menu bar warning), then ends.

use super::disk::{self, Footprint, LumeFacts};
use super::host::{self, HostSampler};
use super::{
    ComputerResources, DISK_MEASURE_EVERY, DiskLevel, DiskOnHost, DiskWarning, GuestUsage,
    HIDDEN_TICK, MAX_AGE, POLL_LEASE, Resources, ResourcesEvent, SpaceResources, THIS_MAC,
    VISIBLE_TICK, now_ms, summary,
};
use crate::Spaces;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{Notify, watch};

/// How long one Space gets to answer `Metrics` in a sample.
const METRICS_TIMEOUT: Duration = Duration::from_secs(3);
/// How long Lume's list is reused.
const LUME_REUSE: Duration = Duration::from_secs(30);
/// A snapshot this young is reused by whoever asks next.
const SAME_SAMPLE: Duration = Duration::from_millis(500);

/// What one sample asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Measure {
    /// Measure what each Space costs this computer's disk.
    pub disk: bool,
    /// Ask each running guest what it uses (`SystemService.Metrics`).
    pub guests: bool,
}

impl Measure {
    /// Everything: what the daemon's sampler does.
    pub const ALL: Measure = Measure {
        disk: true,
        guests: true,
    };
}

/// Who wants fresh numbers, and the last snapshot.
pub(crate) struct Hub {
    shared: Mutex<Shared>,
    wake: Notify,
    tx: watch::Sender<Option<Arc<Resources>>>,
    sampler: tokio::sync::Mutex<Sampler>,
}

#[derive(Default)]
struct Shared {
    visible: usize,
    hidden: usize,
    lease_until: Option<Instant>,
    running: bool,
    last: Option<(Arc<Resources>, Instant)>,
}

impl Shared {
    /// How long until the next sample, or `None` when nobody is
    /// interested.
    fn tick(&self, now: Instant) -> Option<Duration> {
        let leased = self.lease_until.is_some_and(|t| t > now);
        if self.visible > 0 || leased {
            Some(VISIBLE_TICK)
        } else if self.hidden > 0 {
            Some(HIDDEN_TICK)
        } else {
            None
        }
    }
}

struct Sampler {
    host: Arc<Mutex<HostSampler>>,
    sequence: u64,
    level: DiskLevel,
    disks: HashMap<String, Measured>,
    lume: Option<(Instant, Vec<LumeFacts>)>,
}

impl Sampler {
    fn new() -> Self {
        Sampler {
            host: Arc::new(Mutex::new(HostSampler::new())),
            sequence: 0,
            level: DiskLevel::Ok,
            disks: HashMap::new(),
            lume: None,
        }
    }
}

struct Measured {
    at: Instant,
    power_state: String,
    value: Option<DiskOnHost>,
}

impl Hub {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            shared: Mutex::new(Shared::default()),
            wake: Notify::new(),
            tx: watch::channel(None).0,
            sampler: tokio::sync::Mutex::new(Sampler::new()),
        })
    }

    fn cached(&self) -> Option<(Arc<Resources>, Instant)> {
        self.shared.lock().expect("hub").last.clone()
    }

    fn publish(&self, snap: Arc<Resources>) {
        self.shared.lock().expect("hub").last = Some((snap.clone(), Instant::now()));
        self.tx.send_replace(Some(snap));
    }
}

/// A subscription to [`ResourcesEvent`]s: a snapshot first, then updates
/// that moved past the thresholds, and a [`ResourcesEvent::LowDisk`] each
/// time the warning level changes. Dropping it ends the interest.
pub struct ResourceWatch {
    hub: Arc<Hub>,
    rx: watch::Receiver<Option<Arc<Resources>>>,
    visible: bool,
    /// What the receiver holds after everything sent so far.
    sent: Option<Resources>,
    level: DiskLevel,
    pending: VecDeque<ResourcesEvent>,
    first: bool,
}

impl ResourceWatch {
    /// The next event; `None` when the runtime is gone.
    pub async fn next(&mut self) -> Option<ResourcesEvent> {
        loop {
            if let Some(e) = self.pending.pop_front() {
                return Some(e);
            }
            let snap = if self.first {
                self.first = false;
                // A snapshot a moment old is the start; otherwise the one
                // the sample this watcher woke is about to publish.
                let fresh = self
                    .hub
                    .cached()
                    .filter(|(_, at)| at.elapsed() < VISIBLE_TICK + MAX_AGE)
                    .map(|(s, _)| s);
                match fresh {
                    Some(s) => {
                        self.rx.borrow_and_update();
                        s
                    }
                    None => self.next_published().await?,
                }
            } else {
                self.next_published().await?
            };
            match self.sent.as_mut() {
                None => {
                    self.level = snap.warning.level;
                    self.sent = Some((*snap).clone());
                    self.pending
                        .push_back(ResourcesEvent::Snapshot((*snap).clone()));
                }
                Some(sent) => {
                    let update = snap.update_since(sent);
                    if !update.is_empty() {
                        sent.apply(&update);
                        self.pending.push_back(ResourcesEvent::Update(update));
                    }
                    if snap.warning.level != self.level {
                        self.level = snap.warning.level;
                        self.pending
                            .push_back(ResourcesEvent::LowDisk(snap.warning));
                    }
                }
            }
        }
    }

    async fn next_published(&mut self) -> Option<Arc<Resources>> {
        loop {
            self.rx.changed().await.ok()?;
            if let Some(s) = self.rx.borrow_and_update().clone() {
                return Some(s);
            }
        }
    }

    /// Whether a window shows the numbers: visible watchers get a sample
    /// every [`VISIBLE_TICK`], hidden ones every [`HIDDEN_TICK`] (and still
    /// every warning change).
    pub fn set_visible(&mut self, visible: bool) {
        if visible == self.visible {
            return;
        }
        {
            let mut s = self.hub.shared.lock().expect("hub");
            if visible {
                s.hidden = s.hidden.saturating_sub(1);
                s.visible += 1;
            } else {
                s.visible = s.visible.saturating_sub(1);
                s.hidden += 1;
            }
        }
        self.visible = visible;
        self.hub.wake.notify_one();
    }
}

impl Drop for ResourceWatch {
    fn drop(&mut self) {
        {
            let mut s = self.hub.shared.lock().expect("hub");
            if self.visible {
                s.visible = s.visible.saturating_sub(1);
            } else {
                s.hidden = s.hidden.saturating_sub(1);
            }
        }
        self.hub.wake.notify_one();
    }
}

impl Spaces {
    /// What this Mac, its extra Spaces and your other computers use now.
    /// A snapshot a few seconds old is returned as it is. With `visible`,
    /// the call also says a window shows the numbers for the next
    /// [`POLL_LEASE`], so the daemon samples every [`VISIBLE_TICK`]: call
    /// it every 10 s while the window is visible.
    pub async fn resources(&self, visible: bool) -> crate::Result<Resources> {
        let hub = self.inner.resources.clone();
        if visible {
            hub.shared.lock().expect("hub").lease_until = Some(Instant::now() + POLL_LEASE);
            self.ensure_resources_loop();
        }
        let reuse = if visible {
            VISIBLE_TICK + MAX_AGE
        } else {
            MAX_AGE
        };
        if let Some((snap, at)) = hub.cached()
            && at.elapsed() < reuse
        {
            return Ok((*snap).clone());
        }
        Ok((*self.sample_resources().await).clone())
    }

    /// One sample taken now, outside the shared sampler: for a command that
    /// runs once (the CLI). Nothing is cached or kept running. With
    /// [`Measure::disk`] false the Spaces' disks are not measured (their
    /// `disk_on_host` is empty: `cua spaces ls --no-size`), and with
    /// [`Measure::guests`] false no guest is asked what it uses (`usage`
    /// is empty), which keeps a command fast when a Space does not answer.
    pub async fn resources_now(&self, ask: Measure) -> Resources {
        let mut st = Sampler::new();
        self.collect_resources(&mut st, ask).await
    }

    /// A subscription to resource events ([`ResourceWatch`]). `visible` says
    /// a window shows the numbers now ([`ResourceWatch::set_visible`]).
    pub fn watch_resources(&self, visible: bool) -> ResourceWatch {
        let hub = self.inner.resources.clone();
        {
            let mut s = hub.shared.lock().expect("hub");
            if visible {
                s.visible += 1;
            } else {
                s.hidden += 1;
            }
        }
        self.ensure_resources_loop();
        ResourceWatch {
            rx: hub.tx.subscribe(),
            hub,
            visible,
            sent: None,
            level: DiskLevel::Ok,
            pending: VecDeque::new(),
            first: true,
        }
    }

    /// Starts the sampling task when none runs; wakes it when one does.
    fn ensure_resources_loop(&self) {
        let hub = self.inner.resources.clone();
        {
            let mut s = hub.shared.lock().expect("hub");
            if s.running {
                drop(s);
                hub.wake.notify_one();
                return;
            }
            s.running = true;
        }
        let weak = Arc::downgrade(&self.inner);
        tokio::spawn(async move {
            loop {
                let tick = {
                    let mut s = hub.shared.lock().expect("hub");
                    match s.tick(Instant::now()) {
                        Some(t) => t,
                        None => {
                            s.running = false;
                            return;
                        }
                    }
                };
                {
                    let Some(inner) = weak.upgrade() else { return };
                    Spaces { inner }.sample_resources().await;
                }
                tokio::select! {
                    _ = tokio::time::sleep(tick) => {}
                    _ = hub.wake.notified() => {}
                }
            }
        });
    }

    /// Samples now (one sample at a time; a caller that waited gets the
    /// sample taken meanwhile).
    pub(crate) async fn sample_resources(&self) -> Arc<Resources> {
        let hub = self.inner.resources.clone();
        let mut st = hub.sampler.lock().await;
        if let Some((snap, at)) = hub.cached()
            && at.elapsed() < SAME_SAMPLE
        {
            return snap;
        }
        let snap = Arc::new(self.collect_resources(&mut st, Measure::ALL).await);
        hub.publish(snap.clone());
        snap
    }

    async fn lume_facts(&self, st: &mut Sampler) -> Vec<LumeFacts> {
        if let Some((at, facts)) = &st.lume
            && at.elapsed() < LUME_REUSE
        {
            return facts.clone();
        }
        let facts = lume_facts().await;
        st.lume = Some((Instant::now(), facts.clone()));
        facts
    }

    /// One sample of everything. Degrades instead of failing: a Space that
    /// does not answer simply has no `usage`.
    async fn collect_resources(&self, st: &mut Sampler, ask: Measure) -> Resources {
        let home = self.home_dir().to_path_buf();
        // This computer.
        let hs = st.host.clone();
        let host = tokio::task::spawn_blocking(move || hs.lock().expect("host sampler").sample())
            .await
            .unwrap_or_default();
        let paths = host::space_paths(&home);
        let volume = tokio::task::spawn_blocking(move || host::most_constrained(&paths))
            .await
            .ok()
            .flatten()
            .unwrap_or_default();
        let min_free = cua_vmm::disk::CacheConfig::load().min_free;
        let warning = DiskWarning::assess(volume.free, volume.total, min_free, st.level);
        st.level = warning.level;

        // Extra Spaces: the local ones.
        let infos: Vec<_> = self
            .list()
            .unwrap_or_default()
            .into_iter()
            .filter(|i| i.id.starts_with("local:") && i.host.is_empty())
            .collect();
        let lume = self.lume_facts(st).await;
        let store = self.inner.sandboxes.state().clone();
        let layout = cua_disk::Layout::new(&home);
        let lume_home = host::lume_home();
        let facts = {
            let (store, layout, lume_home) = (store.clone(), layout.clone(), lume_home.clone());
            let lume = lume.clone();
            tokio::task::spawn_blocking(move || {
                disk::footprints(&store, &layout, &lume, lume_home.as_deref())
            })
            .await
            .unwrap_or_default()
        };
        let now = Instant::now();
        let ids: Vec<String> = infos.iter().map(|i| i.id.clone()).collect();
        st.disks.retain(|id, _| ids.contains(id));
        // Disks: measured when new, after a power change, or when old.
        let mut due: Vec<(String, Footprint, String)> = Vec::new();
        for info in &infos {
            let name = info.id.trim_start_matches("local:");
            let Some(fp) = facts.iter().find(|f| f.name == name) else {
                continue;
            };
            let stale = st.disks.get(&info.id).is_none_or(|m| {
                m.power_state != info.power_state || now.duration_since(m.at) >= DISK_MEASURE_EVERY
            });
            if stale && ask.disk {
                due.push((info.id.clone(), fp.clone(), info.power_state.clone()));
            }
        }
        let measured =
            futures_util::future::join_all(due.into_iter().map(|(id, fp, power)| async move {
                let value = tokio::task::spawn_blocking(move || measure_footprint(&fp))
                    .await
                    .unwrap_or(None);
                (id, power, value)
            }))
            .await;
        for (id, power_state, value) in measured {
            st.disks.insert(
                id,
                Measured {
                    at: Instant::now(),
                    power_state,
                    value,
                },
            );
        }
        // Guests: only the ones that run, each asked at once, each bounded.
        let usages = futures_util::future::join_all(infos.iter().map(|info| async move {
            let off = matches!(info.power_state.as_str(), "suspended" | "stopped");
            if !ask.guests || off || info.spacesd_version.is_empty() {
                return None;
            }
            let ask = async {
                let space = self.space(&info.id).await.ok()?;
                space.usage().await.ok()
            };
            tokio::time::timeout(METRICS_TIMEOUT, ask)
                .await
                .ok()
                .flatten()
                .map(GuestUsage::from)
        }))
        .await;
        let spaces: Vec<SpaceResources> = infos
            .iter()
            .zip(usages)
            .map(|(info, usage)| {
                let name = info.id.trim_start_matches("local:");
                let fp = facts.iter().find(|f| f.name == name);
                // A guest that reports its own limits fills what the state
                // file did not record.
                let own = usage.filter(|u| u.memory_limited || u.disk_limited);
                SpaceResources {
                    id: info.id.clone(),
                    power_state: info.power_state.clone(),
                    cpu_allocated: fp
                        .map(|f| f.cpus)
                        .filter(|c| *c > 0)
                        .or(own.map(|u| u.cpu_count))
                        .unwrap_or(0),
                    memory_allocated: fp
                        .map(|f| f.memory)
                        .filter(|m| *m > 0)
                        .or(own.filter(|u| u.memory_limited).map(|u| u.memory_total))
                        .unwrap_or(0),
                    disk_allocated: fp
                        .map(|f| f.disk)
                        .filter(|d| *d > 0)
                        .or(own.filter(|u| u.disk_limited).map(|u| u.disk_total))
                        .unwrap_or(0),
                    usage,
                    disk_on_host: st.disks.get(&info.id).and_then(|m| m.value),
                }
            })
            .collect();

        let mut this_mac = ComputerResources {
            id: THIS_MAC.into(),
            online: true,
            sampled_at_ms: now_ms(),
            cpu_count: host.cpu_count,
            cpu_percent: host.cpu_percent,
            load_average_1m: host.load_average_1m,
            memory_total: host.memory_total,
            memory_used: host.memory_used,
            disk_total: volume.total,
            disk_free: volume.free,
            warning,
            spaces_disk_exact: true,
            ..Default::default()
        };
        totals(&mut this_mac, &spaces);

        st.sequence += 1;
        Resources {
            sequence: st.sequence,
            sampled_at_ms: this_mac.sampled_at_ms,
            warning,
            computers: self.other_computers(),
            this_mac,
            spaces,
        }
    }

    /// Your other computers, from the summary each published in its relay
    /// machine record. Reads the machine list already fetched
    /// ([`Spaces::relay_machines`]): no relay call of its own.
    pub fn other_computers(&self) -> Vec<ComputerResources> {
        let own = self.this_relay_machine_id();
        let cache = self.inner.relay_cache.lock().expect("relay cache");
        cache
            .iter()
            .filter(|m| own.as_deref() != Some(m.id.as_str()))
            .filter_map(|m| {
                let value = m.meta.get(summary::META_KEY)?;
                summary::decode(&m.id, &m.name, m.online, value)
            })
            .collect()
    }
}

/// The extra-Space totals of a computer.
pub(crate) fn totals(c: &mut ComputerResources, spaces: &[SpaceResources]) {
    c.spaces_count = spaces.len() as u32;
    c.spaces_disk_exact = true;
    for s in spaces {
        let running = s.power_state == "running" || (s.power_state.is_empty() && s.usage.is_some());
        if running {
            c.spaces_running += 1;
            c.spaces_cpu_allocated += s.cpu_allocated;
            c.spaces_memory_allocated += s.memory_allocated;
            c.spaces_memory_used += s.usage.map(|u| u.memory_used).unwrap_or(0);
        }
        if let Some(d) = &s.disk_on_host {
            match d.bytes {
                Some(b) => c.spaces_disk_bytes += b,
                None => {
                    c.spaces_disk_bytes += d.allocated;
                    c.spaces_disk_exact = false;
                }
            }
        }
    }
}

/// What the Space with `fp` costs this computer's disk.
fn measure_footprint(fp: &Footprint) -> Option<DiskOnHost> {
    let dir = fp.dir.as_deref()?;
    disk::measure(dir).or_else(|| {
        // Lume's directory is somewhere else: only Lume's own count, which
        // clones count in full.
        (fp.reported > 0).then_some(DiskOnHost {
            bytes: None,
            allocated: fp.reported,
            shares_space: true,
        })
    })
}

/// What `lume serve` says about its VMs, when it already answers (macOS
/// only; never started).
async fn lume_facts() -> Vec<LumeFacts> {
    if !cfg!(target_os = "macos") {
        return Vec::new();
    }
    let client = cua_vmm::lume::LumeClient::new(cua_vmm::lume::LumeConfig::default().url);
    let read = async {
        if !client.reachable().await {
            return Vec::new();
        }
        client.list().await.unwrap_or_default()
    };
    tokio::time::timeout(Duration::from_secs(2), read)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|v| LumeFacts {
            cpus: v.cpu_count,
            memory: v.memory_size,
            disk_total: cua_vmm::lume::client::disk_total_bytes(&v.disk_size).unwrap_or(0),
            reported: v
                .disk_size
                .get("allocated")
                .and_then(|a| a.as_u64())
                .unwrap_or(0),
            name: v.name,
        })
        .collect()
}
