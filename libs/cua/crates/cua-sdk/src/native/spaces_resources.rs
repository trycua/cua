// What this Mac, its extra Spaces and your other computers use: processor,
// memory and disk (`Spaces.resources`, `Spaces.watch_resources`).
//
// The numbers are measured once, by the runtime that owns the Spaces (the
// daemon, or this process when embedded), so every window and the menu bar
// read the same sample. Numbers only: counts, bytes and percentages.

/// How low the free disk is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiskLevel {
    /// Plenty.
    Ok,
    /// Under 50 GB or 15% of the volume, whichever line is higher (the
    /// line never rises above 100 GB).
    Low,
    /// Under the floor where creating a Space already fails (5 GiB).
    Critical,
}

/// The low-disk signal for the app and the menu bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskWarning {
    /// How low it is.
    pub level: DiskLevel,
    /// Free bytes on the volume that holds Spaces.
    pub free: u64,
    /// Size of that volume.
    pub total: u64,
    /// The line that was crossed (0 when `ok`).
    pub threshold: u64,
}

/// A computer: this Mac, or one of your other computers (from the summary
/// it published).
#[derive(Debug, Clone, PartialEq, uniffi::Record, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComputerResources {
    /// `this-mac`, or the relay machine id.
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
#[derive(Debug, Clone, Copy, PartialEq, uniffi::Record, Serialize, Deserialize)]
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
    /// Disk size, bytes; absent when the guest's file system has no size
    /// of its own (gVisor's root overlay) or did not say. Never a sentinel.
    pub disk_total: Option<u64>,
    /// `disk_total` is the guest's own disk.
    pub disk_limited: bool,
}

/// What a Space costs this computer's disk, as opposed to what the guest
/// sees.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskOnHost {
    /// Bytes only this Space holds (what deleting it frees); absent when
    /// they could not be measured.
    pub bytes: Option<u64>,
    /// Blocks its files occupy, clones counted in full: an upper bound.
    pub allocated: u64,
    /// Part of it is shared with other copies.
    pub shares_space: bool,
}

/// One extra Space on this Mac.
#[derive(Debug, Clone, PartialEq, uniffi::Record, Serialize, Deserialize)]
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
    /// The guest's own numbers; absent while it is off or unreachable.
    pub usage: Option<GuestUsage>,
    /// What it costs this Mac: its own data only, never the base it
    /// shares; absent when there is nothing to measure.
    pub disk_on_host: Option<DiskOnHost>,
    /// The base image it is a copy-on-write copy of, counted once for all
    /// the Spaces that share it; absent when it has none or the runtime
    /// cannot tell it apart.
    #[serde(default)]
    pub base: Option<SharedBase>,
}

/// A base image several Spaces share copy-on-write (a container image's
/// layers, the image a VM disk was cloned from).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedBase {
    /// Which base: Spaces with the same key share the same blocks.
    pub key: String,
    /// The image reference it came from (`ghcr.io/trycua/linux:24.04`).
    pub image: String,
    /// What it takes on this computer, once for all its Spaces; absent when
    /// the runtime cannot tell it apart from the Spaces' own data.
    pub bytes: Option<u64>,
    /// Local Spaces that use it, this one included.
    pub shared_with: u32,
}

/// One sample of everything.
#[derive(Debug, Clone, PartialEq, uniffi::Record, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourcesSnapshot {
    /// Grows by one per sample.
    pub sequence: u64,
    /// Unix ms.
    pub sampled_at_ms: u64,
    /// This Mac.
    pub this_mac: ComputerResources,
    /// Extra Spaces on this Mac.
    pub spaces: Vec<SpaceResources>,
    /// Your other computers, from the summaries they published.
    pub computers: Vec<ComputerResources>,
    /// `this_mac.warning`, for the menu bar.
    pub warning: DiskWarning,
}

/// What changed since the last event: only entries that moved by more than
/// 1 point or 256 MB. Replace entries by `id`; drop the `removed` ids.
#[derive(Debug, Clone, PartialEq, uniffi::Record, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourcesUpdate {
    /// The sample it brings you to.
    pub sequence: u64,
    /// Unix ms.
    pub sampled_at_ms: u64,
    /// This Mac, when it changed.
    pub this_mac: Option<ComputerResources>,
    /// Extra Spaces that changed or appeared.
    pub spaces: Vec<SpaceResources>,
    /// Other computers that changed or appeared.
    pub computers: Vec<ComputerResources>,
    /// Ids (Spaces or computers) that are gone.
    pub removed: Vec<String>,
}

/// What [`Spaces::watch_resources`] reports.
#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum ResourcesEvent {
    /// Everything, at the start (and again after a reconnect).
    Snapshot {
        /// The sample.
        snapshot: ResourcesSnapshot,
    },
    /// Only what changed.
    Update {
        /// The changes.
        update: ResourcesUpdate,
    },
    /// The low-disk level changed (including back to `ok`).
    LowDisk {
        /// The warning now.
        warning: DiskWarning,
    },
}

/// Receives [`ResourcesEvent`]s from [`Spaces::watch_resources`], in order.
/// Called on an SDK worker thread; return quickly.
#[uniffi::export(with_foreign)]
pub trait ResourcesListener: Send + Sync {
    /// Something moved.
    fn on_event(&self, event: ResourcesEvent);
}

/// A running [`Spaces::watch_resources`]: tell it whether a window shows
/// the numbers, and stop it when done. Dropping it also stops it.
#[derive(uniffi::Object)]
pub struct ResourcesWatch {
    control: tokio::sync::watch::Sender<Control>,
    task: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

#[derive(Clone, Copy)]
struct Control {
    visible: bool,
    stopped: bool,
}

#[uniffi::export]
impl ResourcesWatch {
    /// Whether a window shows the numbers: visible watchers are sampled
    /// every 10 seconds, hidden ones every 5 minutes (and still hear
    /// `LowDisk`, for the menu bar).
    pub fn set_visible(&self, visible: bool) {
        self.control.send_modify(|c| c.visible = visible);
    }

    /// Ends the watch. No event is delivered after this returns.
    pub fn stop(&self) {
        self.control.send_modify(|c| c.stopped = true);
        if let Some(t) = self.task.lock().expect("watch task").take() {
            t.abort();
        }
    }
}

impl Drop for ResourcesWatch {
    fn drop(&mut self) {
        self.stop();
    }
}

/// A runtime value as the SDK's record (same JSON, same names).
fn record<T: serde::de::DeserializeOwned>(v: &impl serde::Serialize) -> Result<T> {
    serde_json::to_value(v)
        .and_then(serde_json::from_value)
        .map_err(|e| CuaError::Internal(e.to_string()))
}

/// A snapshot as the SDK's record.
pub(crate) fn snapshot_of(r: &cua_spaces::resources::Resources) -> Result<ResourcesSnapshot> {
    record(r)
}

fn event_of(e: &cua_spaces::resources::ResourcesEvent) -> Result<ResourcesEvent> {
    use cua_spaces::resources::ResourcesEvent as E;
    Ok(match e {
        E::Snapshot(s) => ResourcesEvent::Snapshot {
            snapshot: record(s)?,
        },
        E::Update(u) => ResourcesEvent::Update { update: record(u)? },
        E::LowDisk(w) => ResourcesEvent::LowDisk {
            warning: record(w)?,
        },
    })
}

/// How long a dropped daemon stream waits before it is opened again.
const REOPEN_AFTER: Duration = Duration::from_secs(5);

#[uniffi::export]
impl Spaces {
    /// What this Mac, its extra Spaces and your other computers use now:
    /// processor, memory and disk, as one snapshot (a few seconds old at
    /// most). With `visible`, the call also says a window shows the numbers
    /// for the next 30 seconds, so the runtime samples every 10 seconds
    /// instead of every 5 minutes: call it every 10 seconds while a window
    /// is visible and stop when it is hidden.
    pub async fn resources(&self, visible: bool) -> Result<ResourcesSnapshot> {
        let host = self.host.clone();
        run(async move {
            let r = match &host {
                Host::Embedded(s) => s.resources(visible).await?,
                Host::Daemon(d) => d
                    .spaces()
                    .get_resources(dpb::GetResourcesRequest { visible })
                    .await
                    .map_err(daemon_err)?
                    .into_inner()
                    .snapshot
                    .map(cua_spaces::resources::Resources::from)
                    .ok_or_else(|| CuaError::Internal("the daemon sent no snapshot".into()))?,
            };
            snapshot_of(&r)
        })
        .await
    }

    /// The same numbers as events for `listener`: a snapshot first, then an
    /// update with only the entries that changed by more than 1 point or
    /// 256 MB, and `LowDisk` whenever the warning level changes. Reopens by
    /// itself if the daemon restarts (the snapshot comes again). Stop it
    /// with [`ResourcesWatch::stop`].
    pub fn watch_resources(
        &self,
        visible: bool,
        listener: Arc<dyn ResourcesListener>,
    ) -> Arc<ResourcesWatch> {
        let (control, rx) = tokio::sync::watch::channel(Control {
            visible,
            stopped: false,
        });
        let host = self.host.clone();
        let task = super::runtime().spawn(async move {
            match host {
                Host::Embedded(s) => embedded_watch(s, rx, listener).await,
                Host::Daemon(d) => daemon_watch(d, rx, listener).await,
            }
        });
        Arc::new(ResourcesWatch {
            control,
            task: std::sync::Mutex::new(Some(task)),
        })
    }
}

async fn embedded_watch(
    spaces: cua_spaces::Spaces,
    mut control: tokio::sync::watch::Receiver<Control>,
    listener: Arc<dyn ResourcesListener>,
) {
    let mut watch = spaces.watch_resources(control.borrow().visible);
    loop {
        tokio::select! {
            event = watch.next() => {
                let Some(event) = event else { return };
                if let Ok(e) = event_of(&event) {
                    listener.on_event(e);
                }
            }
            changed = control.changed() => {
                if changed.is_err() || control.borrow().stopped {
                    return;
                }
                watch.set_visible(control.borrow().visible);
            }
        }
    }
}

async fn daemon_watch(
    daemon: cua_daemon::client::DaemonClient,
    mut control: tokio::sync::watch::Receiver<Control>,
    listener: Arc<dyn ResourcesListener>,
) {
    use dpb::watch_resources_response::Event;
    loop {
        let visible = control.borrow().visible;
        let opened = daemon
            .spaces()
            .watch_resources(dpb::WatchResourcesRequest { visible })
            .await;
        let mut stream = match opened {
            Ok(s) => s.into_inner(),
            Err(_) => {
                // A daemon that is not up, or predates this: try again.
                tokio::select! {
                    _ = tokio::time::sleep(REOPEN_AFTER) => continue,
                    changed = control.changed() => {
                        if changed.is_err() || control.borrow().stopped {
                            return;
                        }
                        continue;
                    }
                }
            }
        };
        loop {
            tokio::select! {
                msg = stream.message() => match msg {
                    Ok(Some(m)) => {
                        let event = match m.event {
                            Some(Event::Snapshot(s)) => {
                                event_of(&cua_spaces::resources::ResourcesEvent::Snapshot(s.into()))
                            }
                            Some(Event::Update(u)) => {
                                event_of(&cua_spaces::resources::ResourcesEvent::Update(u.into()))
                            }
                            Some(Event::LowDisk(w)) => {
                                event_of(&cua_spaces::resources::ResourcesEvent::LowDisk(w.into()))
                            }
                            None => continue,
                        };
                        if let Ok(e) = event {
                            listener.on_event(e);
                        }
                    }
                    // The daemon ended the stream or went away.
                    Ok(None) | Err(_) => break,
                },
                changed = control.changed() => {
                    if changed.is_err() || control.borrow().stopped {
                        return;
                    }
                    // The visibility changed: the server takes it as a new
                    // call (the snapshot comes again).
                    if control.borrow().visible != visible {
                        break;
                    }
                }
            }
        }
        // Reopen (at once for a visibility change, after a pause when the
        // daemon went away).
        if control.borrow().visible == visible {
            tokio::time::sleep(REOPEN_AFTER).await;
        }
    }
}

#[cfg(test)]
mod resources_tests {
    use super::*;
    use cua_spaces::resources as core;

    fn core_snapshot() -> core::Resources {
        let warning = core::DiskWarning {
            level: core::DiskLevel::Low,
            free: 90 << 30,
            total: 1 << 40,
            threshold: 100 << 30,
        };
        core::Resources {
            sequence: 4,
            sampled_at_ms: 99,
            this_mac: core::ComputerResources {
                id: "this-mac".into(),
                online: true,
                cpu_count: 12,
                cpu_percent: 38.5,
                memory_total: 64 << 30,
                memory_used: 41 << 30,
                disk_total: 1 << 40,
                disk_free: 90 << 30,
                spaces_count: 1,
                warning,
                ..Default::default()
            },
            spaces: vec![core::SpaceResources {
                id: "local:a".into(),
                power_state: "running".into(),
                cpu_allocated: 4,
                memory_allocated: 8 << 30,
                disk_allocated: 80 << 30,
                usage: Some(core::GuestUsage {
                    cpu_count: 4,
                    cpu_percent: 22.0,
                    memory_limited: true,
                    ..Default::default()
                }),
                disk_on_host: Some(core::DiskOnHost {
                    bytes: None,
                    allocated: 31 << 30,
                    shares_space: true,
                }),
                base: None,
            }],
            computers: vec![],
            warning,
        }
    }

    #[test]
    fn the_sdk_records_carry_every_field_of_the_snapshot() {
        let r = core_snapshot();
        let s = snapshot_of(&r).unwrap();
        assert_eq!(s.sequence, 4);
        assert_eq!(s.this_mac.cpu_count, 12);
        assert_eq!(s.this_mac.warning.level, DiskLevel::Low);
        assert_eq!(s.warning.threshold, 100 << 30);
        let space = &s.spaces[0];
        assert_eq!(space.id, "local:a");
        assert_eq!((space.cpu_allocated, space.memory_allocated), (4, 8 << 30));
        assert_eq!(space.usage.unwrap().cpu_percent, 22.0);
        let d = space.disk_on_host.unwrap();
        assert_eq!(
            (d.bytes, d.allocated, d.shares_space),
            (None, 31 << 30, true)
        );
        // Nothing dropped: the record serializes back to the same JSON.
        let back = serde_json::to_value(&r).unwrap();
        assert_eq!(back["thisMac"]["loadAverage1m"], 0.0);
    }

    #[test]
    fn events_become_records() {
        let r = core_snapshot();
        match event_of(&core::ResourcesEvent::Snapshot(r.clone())).unwrap() {
            ResourcesEvent::Snapshot { snapshot } => assert_eq!(snapshot.sequence, 4),
            other => panic!("{other:?}"),
        }
        let update = core::ResourcesUpdate {
            sequence: 5,
            sampled_at_ms: 100,
            this_mac: None,
            spaces: vec![r.spaces[0].clone()],
            computers: vec![],
            removed: vec!["local:gone".into()],
        };
        match event_of(&core::ResourcesEvent::Update(update)).unwrap() {
            ResourcesEvent::Update { update } => {
                assert_eq!(update.sequence, 5);
                assert!(update.this_mac.is_none());
                assert_eq!(update.spaces.len(), 1);
                assert_eq!(update.removed, ["local:gone"]);
            }
            other => panic!("{other:?}"),
        }
        match event_of(&core::ResourcesEvent::LowDisk(r.warning)).unwrap() {
            ResourcesEvent::LowDisk { warning } => {
                assert_eq!(warning.level, DiskLevel::Low);
                assert_eq!(warning.free, 90 << 30);
            }
            other => panic!("{other:?}"),
        }
    }
}
