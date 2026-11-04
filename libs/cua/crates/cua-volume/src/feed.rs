// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The change feed: how devices sharing one bucket learn about each
//! other's changes in seconds, with no service besides the bucket.
//!
//! A device is a cua home. Every change a device makes through a
//! [`Session`](crate::Session) is published as a tiny object:
//!
//! ```text
//! .cua-feed/<unix ms, 13 digits>-<device id>-<random>.json   {"op":"put","key":...,"version":...}
//! .cua-devices/<device id>.json                               {"name":...,"last_seen_ms":...}
//! ```
//!
//! Feed keys sort by time, so a device polls with one `ListObjectsV2`
//! `start-after` its cursor: an idle poll returns nothing and costs one
//! request. The poll interval starts at [`MIN_POLL`] and backs off to
//! [`MAX_POLL`] while nothing changes; any change (here or remote) snaps it
//! back. The cursor trails the newest entry by [`SKEW`] so a device whose
//! clock runs behind is not skipped; entries already seen are remembered
//! until they fall out of that window. Entries older than [`RETAIN`] are
//! deleted by whichever device notices first (on a versioned bucket, add a
//! lifecycle rule that expires noncurrent versions under `.cua-feed/`).
//!
//! Works the same on the local backend (two homes pointed at one data
//! directory) and on any S3-compatible store (AWS S3, R2, MinIO).

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::backend::Condition;
use crate::drive::{Change, ChangeSink};
use crate::{Drive, Error, Result, new_id, now_ms};

/// Where feed entries live.
pub const FEED_PREFIX: &str = ".cua-feed/";
/// Where device records live.
pub const DEVICES_PREFIX: &str = ".cua-devices/";
/// Fastest poll.
pub const MIN_POLL: Duration = Duration::from_millis(500);
/// Slowest poll while idle.
pub const MAX_POLL: Duration = Duration::from_secs(5);
/// How far the cursor trails the newest entry (clock skew between devices).
pub const SKEW: Duration = Duration::from_secs(30);
/// How long entries are kept.
pub const RETAIN: Duration = Duration::from_secs(24 * 3600);
/// How often a device refreshes its own record.
pub const HEARTBEAT: Duration = Duration::from_secs(30);
/// Failed polls in a row before the feed reports `offline`.
pub const OFFLINE_AFTER: u32 = 3;
/// Events kept for [`Feed::events`].
const MAX_EVENTS: usize = 2000;
/// Conflicts kept.
const MAX_CONFLICTS: usize = 100;

/// One published change.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeedEntry {
    pub ts_ms: u64,
    pub device: String,
    /// `put` or `delete`.
    pub op: String,
    pub key: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub etag: String,
    #[serde(default)]
    pub size: u64,
}

/// A device record in the bucket.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceRecord {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub last_seen_ms: u64,
    #[serde(default)]
    pub last_change_ms: u64,
}

/// One device as this daemon sees it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceStatus {
    pub id: String,
    pub name: String,
    pub this_device: bool,
    pub last_seen_ms: u64,
    pub last_change_ms: u64,
    pub changes: u64,
}

/// A write that lost to a later one. Its content is in the file's history
/// and in the visible copy at `conflict_path`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conflict {
    pub path: String,
    pub conflict_path: String,
    pub winner_device: String,
    pub loser_device: String,
    pub winner_version: String,
    pub loser_version: String,
    pub ts_ms: u64,
}

/// One sync event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncEvent {
    pub seq: u64,
    pub ts_ms: u64,
    /// `remote_change`, `remote_delete`, `upload_started`, `upload_done`,
    /// `upload_failed`, `conflict`, `device_seen`, `error`.
    pub kind: String,
    pub path: String,
    pub device: String,
    pub size: u64,
    pub version: String,
    pub detail: String,
}

/// The sync state for the app.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SyncStatus {
    pub device_id: String,
    pub device_name: String,
    /// `live` (polling a bucket), `off` (not running, or a store on this
    /// machine) or `offline` (the bucket stopped answering; see
    /// `last_error`).
    pub feed: String,
    pub poll_interval_ms: u64,
    pub last_poll_ms: u64,
    pub last_remote_change_ms: u64,
    pub pending_uploads: u32,
    pub pending_bytes: u64,
    pub conflicts: Vec<Conflict>,
    pub devices: Vec<DeviceStatus>,
    pub last_error: Option<String>,
    /// The files waiting to upload (other devices do not see them yet),
    /// largest first, at most 100.
    #[serde(default)]
    pub pending: Vec<PendingUpload>,
    /// `fs` (This Mac) or `s3` (your bucket). Set by the service.
    #[serde(default)]
    pub backend: String,
    /// This machine's mount: `off`, `mounting`, `mounted`,
    /// `needs_approval`, `unsupported` or `error`. Set by the service.
    #[serde(default)]
    pub mount: String,
    /// The block cache in front of a bucket. Set by the service.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache: Option<crate::cache::CacheStats>,
}

/// A file waiting to upload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingUpload {
    pub path: String,
    pub bytes: u64,
}

/// One file's sync state, where it is cheap to know.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSync {
    /// `synced`; `pending_upload` (written here, not yet in storage, so
    /// other devices do not see it); `conflict` (a write to it lost to a
    /// later one, kept at `conflict_path`); `conflict_copy` (this file is
    /// the kept copy of a losing write).
    pub state: String,
    /// The device that last wrote it, when that is another device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub written_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflict_path: Option<String>,
}

impl FileSync {
    /// Nothing to say beyond `synced` by this device.
    pub fn is_plain(&self) -> bool {
        self.state == "synced" && self.written_by.is_none()
    }
}

/// This cua home's identity as a device (`<home>/volume/device.json`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceId {
    pub id: String,
    pub name: String,
}

/// Names this device in the feed instead of the host name (a throwaway
/// home, a demo, a second daemon on one machine).
pub const DEVICE_NAME_ENV: &str = "CUA_DRIVE_DEVICE_NAME";

impl DeviceId {
    /// Reads the id, creating it (random, named after the host) once.
    /// [`DEVICE_NAME_ENV`] overrides the name.
    pub fn load_or_create(state: &Path) -> Result<DeviceId> {
        Self::load_with(state, std::env::var(DEVICE_NAME_ENV).ok())
    }

    fn load_with(state: &Path, name: Option<String>) -> Result<DeviceId> {
        let name = name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
        let p = state.join("device.json");
        if let Ok(b) = std::fs::read(&p)
            && let Ok(mut d) = serde_json::from_slice::<DeviceId>(&b)
        {
            if let Some(n) = name {
                d.name = n;
            }
            return Ok(d);
        }
        let d = DeviceId {
            id: new_id()[..12].to_string(),
            name: name.unwrap_or_else(host_name),
        };
        std::fs::create_dir_all(state)?;
        cua_home::guard_write(&p)?;
        std::fs::write(&p, serde_json::to_vec_pretty(&d)?)?;
        Ok(d)
    }
}

fn host_name() -> String {
    #[cfg(unix)]
    {
        let mut buf = [0u8; 256];
        // SAFETY: the buffer is valid for its length.
        if unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } == 0 {
            let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
            let name = String::from_utf8_lossy(&buf[..end]).into_owned();
            // `Dillons-MBP.local`, `host.localdomain`: the first label.
            let short = name.split('.').next().unwrap_or("").to_string();
            if !short.is_empty() {
                return short;
            }
        }
    }
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "device".into())
}

/// Told about remote changes (the mount invalidates its caches).
pub trait RemoteListener: Send + Sync {
    fn remote_changed(&self, entry: &FeedEntry);
}

struct State {
    events: VecDeque<SyncEvent>,
    seq: u64,
    conflicts: Vec<Conflict>,
    devices: HashMap<String, DeviceStatus>,
    seen: HashSet<String>,
    cursor_ms: u64,
    last_poll_ms: u64,
    last_remote_change_ms: u64,
    last_error: Option<String>,
    poll_ms: u64,
    /// Polls that failed in a row (reset by a successful one).
    failures: u32,
    /// Who last wrote each key (from the feed), for conflict reports.
    writers: HashMap<String, String>,
}

/// The feed of one drive on one device.
pub struct Feed {
    drive: Drive,
    device: DeviceId,
    state: Mutex<State>,
    wake: tokio::sync::Notify,
    events_ready: tokio::sync::Notify,
    listeners: Mutex<Vec<Arc<dyn RemoteListener>>>,
    publish_tx: tokio::sync::mpsc::UnboundedSender<FeedEntry>,
    publish_rx: Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<FeedEntry>>>,
    pending: Mutex<HashMap<String, u64>>,
    running: AtomicBool,
    published: AtomicU64,
}

impl std::fmt::Debug for Feed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Feed")
            .field("device", &self.device)
            .finish()
    }
}

/// Publishes a drive's local changes into its feed.
struct Publisher(std::sync::Weak<Feed>);

impl ChangeSink for Publisher {
    fn changed(&self, change: Change) {
        if let Some(f) = self.0.upgrade() {
            f.local_change(change);
        }
    }
}

fn entry_key(e: &FeedEntry) -> String {
    format!(
        "{FEED_PREFIX}{:013}-{}-{}.json",
        e.ts_ms,
        e.device,
        &new_id()[..8]
    )
}

fn ts_of(key: &str) -> Option<u64> {
    key.strip_prefix(FEED_PREFIX)?.get(..13)?.parse().ok()
}

fn device_of(key: &str) -> Option<&str> {
    key.strip_prefix(FEED_PREFIX)?.get(14..)?.split('-').next()
}

impl Feed {
    /// A feed for `drive` as `device`, registered as the drive's change
    /// sink. Nothing runs until [`Feed::spawn`].
    pub fn new(drive: &Drive, device: DeviceId) -> Arc<Feed> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let now = now_ms();
        let mut devices = HashMap::new();
        devices.insert(
            device.id.clone(),
            DeviceStatus {
                id: device.id.clone(),
                name: device.name.clone(),
                this_device: true,
                last_seen_ms: now,
                ..Default::default()
            },
        );
        let feed = Arc::new(Feed {
            drive: drive.clone(),
            device,
            state: Mutex::new(State {
                events: VecDeque::new(),
                seq: 0,
                conflicts: vec![],
                devices,
                seen: HashSet::new(),
                cursor_ms: now.saturating_sub(SKEW.as_millis() as u64),
                last_poll_ms: 0,
                last_remote_change_ms: 0,
                last_error: None,
                poll_ms: MIN_POLL.as_millis() as u64,
                failures: 0,
                writers: HashMap::new(),
            }),
            wake: tokio::sync::Notify::new(),
            events_ready: tokio::sync::Notify::new(),
            listeners: Mutex::new(vec![]),
            publish_tx: tx,
            publish_rx: Mutex::new(Some(rx)),
            pending: Mutex::new(HashMap::new()),
            running: AtomicBool::new(false),
            published: AtomicU64::new(0),
        });
        drive.add_sink(Arc::new(Publisher(Arc::downgrade(&feed))));
        feed
    }

    pub fn device(&self) -> &DeviceId {
        &self.device
    }

    /// Feed entries this device has published.
    pub fn published(&self) -> u64 {
        self.published.load(Ordering::Relaxed)
    }

    pub fn add_listener(&self, l: Arc<dyn RemoteListener>) {
        self.listeners.lock().unwrap().push(l);
    }

    fn local_change(&self, change: Change) {
        let e = match change {
            Change::Put(m) => FeedEntry {
                ts_ms: now_ms(),
                device: self.device.id.clone(),
                op: "put".into(),
                key: m.key,
                version: m.version,
                etag: m.etag,
                size: m.size,
            },
            Change::Delete(key) => FeedEntry {
                ts_ms: now_ms(),
                device: self.device.id.clone(),
                op: "delete".into(),
                key,
                version: String::new(),
                etag: String::new(),
                size: 0,
            },
        };
        {
            let mut st = self.state.lock().unwrap();
            if let Some(d) = st.devices.get_mut(&self.device.id) {
                d.last_change_ms = e.ts_ms;
                d.changes += 1;
            }
            st.writers.insert(e.key.clone(), self.device.id.clone());
            st.poll_ms = MIN_POLL.as_millis() as u64;
        }
        let _ = self.publish_tx.send(e);
        self.wake.notify_one();
    }

    /// Records an event and wakes long pollers.
    pub fn event(
        &self,
        kind: &str,
        path: &str,
        device: &str,
        size: u64,
        version: &str,
        detail: &str,
    ) {
        {
            let mut st = self.state.lock().unwrap();
            st.seq += 1;
            let ev = SyncEvent {
                seq: st.seq,
                ts_ms: now_ms(),
                kind: kind.into(),
                path: path.into(),
                device: device.into(),
                size,
                version: version.into(),
                detail: detail.into(),
            };
            st.events.push_back(ev);
            while st.events.len() > MAX_EVENTS {
                st.events.pop_front();
            }
            if kind == "error" || kind == "upload_failed" {
                st.last_error = Some(if detail.is_empty() {
                    path.to_string()
                } else {
                    format!("{path}: {detail}")
                });
            }
        }
        self.events_ready.notify_waiters();
    }

    /// Records a conflict (and its event).
    pub fn conflict(&self, c: Conflict) {
        self.event(
            "conflict",
            &c.path,
            &c.loser_device,
            0,
            &c.loser_version,
            &format!("kept as {}", c.conflict_path),
        );
        let mut st = self.state.lock().unwrap();
        st.conflicts.insert(0, c);
        st.conflicts.truncate(MAX_CONFLICTS);
    }

    /// Clears a conflict from the list (the files stay).
    pub fn resolve(&self, path: &str) -> bool {
        let mut st = self.state.lock().unwrap();
        let before = st.conflicts.len();
        st.conflicts
            .retain(|c| c.path != path && c.conflict_path != path);
        before != st.conflicts.len()
    }

    /// The device that last wrote `key`, as far as the feed knows.
    pub fn last_writer(&self, key: &str) -> Option<String> {
        self.state.lock().unwrap().writers.get(key).cloned()
    }

    /// The device's display name, or its id.
    pub fn device_name(&self, id: &str) -> String {
        self.state
            .lock()
            .unwrap()
            .devices
            .get(id)
            .map(|d| d.name.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| id.to_string())
    }

    /// Marks `key` as waiting to upload (`bytes`) or done (`None`).
    pub fn set_pending(&self, key: &str, bytes: Option<u64>) {
        let mut p = self.pending.lock().unwrap();
        match bytes {
            Some(b) => {
                p.insert(key.to_string(), b);
            }
            None => {
                p.remove(key);
            }
        }
    }

    /// `key`'s sync state (a file key, no leading slash).
    pub fn file_sync(&self, key: &str) -> FileSync {
        let pending = self.pending.lock().unwrap().contains_key(key);
        let st = self.state.lock().unwrap();
        let written_by = st
            .writers
            .get(key)
            .filter(|d| **d != self.device.id)
            .map(|d| {
                st.devices
                    .get(d)
                    .map(|x| x.name.clone())
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| d.clone())
            });
        let (state, conflict_path) = if pending {
            ("pending_upload", None)
        } else if let Some(c) = st.conflicts.iter().find(|c| c.path == key) {
            ("conflict", Some(c.conflict_path.clone()))
        } else if st.conflicts.iter().any(|c| c.conflict_path == key) {
            ("conflict_copy", None)
        } else {
            ("synced", None)
        };
        FileSync {
            state: state.into(),
            written_by,
            conflict_path,
        }
    }

    pub fn status(&self) -> SyncStatus {
        let (pending_uploads, pending_bytes, pending) = {
            let p = self.pending.lock().unwrap();
            let mut list: Vec<PendingUpload> = p
                .iter()
                .map(|(k, b)| PendingUpload {
                    path: k.clone(),
                    bytes: *b,
                })
                .collect();
            list.sort_by(|a, b| b.bytes.cmp(&a.bytes).then(a.path.cmp(&b.path)));
            list.truncate(100);
            (p.len() as u32, p.values().sum(), list)
        };
        let st = self.state.lock().unwrap();
        let mut devices: Vec<DeviceStatus> = st.devices.values().cloned().collect();
        devices.sort_by(|a, b| {
            b.this_device
                .cmp(&a.this_device)
                .then(b.last_seen_ms.cmp(&a.last_seen_ms))
        });
        SyncStatus {
            device_id: self.device.id.clone(),
            device_name: self.device.name.clone(),
            // `off`: not running, or a store on this machine (no other
            // device to hear from). `offline`: the bucket stopped answering
            // (OFFLINE_AFTER polls in a row failed; `last_error` says why).
            feed: if !self.running.load(Ordering::Relaxed) || !self.drive.backend().remote() {
                "off".into()
            } else if st.failures >= OFFLINE_AFTER {
                "offline".into()
            } else {
                "live".into()
            },
            poll_interval_ms: st.poll_ms,
            last_poll_ms: st.last_poll_ms,
            last_remote_change_ms: st.last_remote_change_ms,
            pending_uploads,
            pending_bytes,
            conflicts: st.conflicts.clone(),
            devices,
            last_error: st.last_error.clone(),
            pending,
            ..Default::default()
        }
    }

    /// Events after `since`, waiting up to `wait` for one.
    pub async fn events(&self, since: u64, wait: Duration) -> (Vec<SyncEvent>, u64) {
        let deadline = tokio::time::Instant::now() + wait.min(Duration::from_secs(30));
        loop {
            let notified = self.events_ready.notified();
            {
                let st = self.state.lock().unwrap();
                let out: Vec<SyncEvent> = st
                    .events
                    .iter()
                    .filter(|e| e.seq > since)
                    .cloned()
                    .collect();
                if !out.is_empty() || tokio::time::Instant::now() >= deadline {
                    return (out, st.seq);
                }
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                let st = self.state.lock().unwrap();
                return (vec![], st.seq);
            }
        }
    }

    /// Stops polling (the publisher stops with the last reference).
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
        self.wake.notify_one();
    }

    /// Starts the publisher, the poller and the heartbeat, until
    /// [`Feed::stop`].
    pub fn spawn(self: &Arc<Self>) {
        if self.running.swap(true, Ordering::SeqCst) {
            return;
        }
        let rx = self.publish_rx.lock().unwrap().take();
        if let Some(mut rx) = rx {
            let weak = Arc::downgrade(self);
            tokio::spawn(async move {
                while let Some(e) = rx.recv().await {
                    let Some(feed) = weak.upgrade() else { break };
                    feed.publish(e).await;
                }
            });
        }
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut last_beat = 0u64;
            let mut last_compact = now_ms();
            loop {
                let Some(feed) = weak.upgrade() else { break };
                let now = now_ms();
                if now.saturating_sub(last_beat) >= HEARTBEAT.as_millis() as u64 {
                    last_beat = now;
                    feed.heartbeat().await;
                }
                if now.saturating_sub(last_compact) >= 600_000 {
                    last_compact = now;
                    feed.compact().await;
                }
                let found = feed.poll_once().await;
                let wait = {
                    let mut st = feed.state.lock().unwrap();
                    st.poll_ms = if found > 0 {
                        MIN_POLL.as_millis() as u64
                    } else {
                        ((st.poll_ms as f64 * 1.5) as u64).min(MAX_POLL.as_millis() as u64)
                    };
                    Duration::from_millis(st.poll_ms)
                };
                tokio::select! {
                    _ = tokio::time::sleep(wait) => {}
                    _ = feed.wake.notified() => {}
                }
                if !feed.running.load(Ordering::SeqCst) {
                    break;
                }
            }
        });
    }

    async fn publish(&self, e: FeedEntry) {
        let key = entry_key(&e);
        let body = match serde_json::to_vec(&e) {
            Ok(b) => b,
            Err(_) => return,
        };
        // A few tries: a lost entry only delays another device until its
        // listing refreshes, but it should not be lost to a blip.
        for attempt in 0..3u64 {
            match self
                .drive
                .backend()
                .put(&key, body.clone(), Condition::None)
                .await
            {
                Ok(_) => {
                    self.published.fetch_add(1, Ordering::Relaxed);
                    self.state.lock().unwrap().seen.insert(key);
                    return;
                }
                Err(err) if attempt == 2 => {
                    self.event("error", &e.key, "", 0, "", &format!("publish: {err}"));
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(200 * (attempt + 1))).await,
            }
        }
    }

    async fn heartbeat(&self) {
        let rec = {
            let st = self.state.lock().unwrap();
            let me = st.devices.get(&self.device.id);
            DeviceRecord {
                id: self.device.id.clone(),
                name: self.device.name.clone(),
                last_seen_ms: now_ms(),
                last_change_ms: me.map(|d| d.last_change_ms).unwrap_or(0),
            }
        };
        let b = self.drive.backend();
        if let Ok(body) = serde_json::to_vec(&rec) {
            let _ = b
                .put(
                    &format!("{DEVICES_PREFIX}{}.json", self.device.id),
                    body,
                    Condition::None,
                )
                .await;
        }
        // Learn the others.
        if let Ok(list) = b.list(DEVICES_PREFIX).await {
            for m in list {
                if m.key.ends_with(&format!("/{}.json", self.device.id)) {
                    continue;
                }
                if let Ok((bytes, _)) = b.get(&m.key, None).await
                    && let Ok(r) = serde_json::from_slice::<DeviceRecord>(&bytes)
                {
                    let mut st = self.state.lock().unwrap();
                    let d = st.devices.entry(r.id.clone()).or_default();
                    d.id = r.id;
                    d.name = r.name;
                    d.last_seen_ms = d.last_seen_ms.max(r.last_seen_ms);
                    d.last_change_ms = d.last_change_ms.max(r.last_change_ms);
                }
            }
        }
    }

    async fn compact(&self) {
        let cutoff = now_ms().saturating_sub(RETAIN.as_millis() as u64);
        let b = self.drive.backend();
        let Ok(old) = b.list_after(FEED_PREFIX, "", 1000).await else {
            return;
        };
        for m in old {
            if ts_of(&m.key).is_some_and(|t| t < cutoff) {
                let _ = b.delete(&m.key, Condition::None).await;
            } else {
                break;
            }
        }
    }

    /// One poll: fetches entries after the cursor, applies the remote ones,
    /// and returns how many were new.
    pub async fn poll_once(&self) -> usize {
        let start_after = {
            let st = self.state.lock().unwrap();
            format!("{FEED_PREFIX}{:013}", st.cursor_ms)
        };
        let b = self.drive.backend();
        let listed = match b.list_after(FEED_PREFIX, &start_after, 1000).await {
            Ok(l) => l,
            Err(e) => {
                let mut st = self.state.lock().unwrap();
                st.last_error = Some(format!("poll: {e}"));
                st.failures += 1;
                return 0;
            }
        };
        let fresh: Vec<String> = {
            let st = self.state.lock().unwrap();
            listed
                .into_iter()
                .map(|m| m.key)
                .filter(|k| !st.seen.contains(k))
                .collect()
        };
        let mut remote = vec![];
        for key in &fresh {
            let mine = device_of(key) == Some(self.device.id.as_str());
            if mine {
                continue;
            }
            match b.get(key, None).await {
                Ok((bytes, _)) => {
                    if let Ok(e) = serde_json::from_slice::<FeedEntry>(&bytes) {
                        remote.push(e);
                    }
                }
                Err(Error::NotFound(_)) => {}
                Err(e) => {
                    let mut st = self.state.lock().unwrap();
                    st.last_error = Some(format!("poll: {e}"));
                    st.failures += 1;
                    return 0;
                }
            }
        }
        let now = now_ms();
        {
            let mut st = self.state.lock().unwrap();
            let newest = fresh.iter().filter_map(|k| ts_of(k)).max();
            for k in &fresh {
                st.seen.insert(k.clone());
            }
            if let Some(t) = newest {
                st.cursor_ms = st.cursor_ms.max(t.saturating_sub(SKEW.as_millis() as u64));
            }
            // Forget seen keys that fell behind the cursor.
            let cursor = st.cursor_ms;
            st.seen.retain(|k| ts_of(k).is_none_or(|t| t >= cursor));
            st.last_poll_ms = now;
            st.failures = 0;
            if st
                .last_error
                .as_deref()
                .is_some_and(|e| e.starts_with("poll:"))
            {
                st.last_error = None;
            }
            for e in &remote {
                st.last_remote_change_ms = now;
                st.writers.insert(e.key.clone(), e.device.clone());
                let d = st.devices.entry(e.device.clone()).or_default();
                if d.id.is_empty() {
                    d.id = e.device.clone();
                }
                d.last_change_ms = d.last_change_ms.max(e.ts_ms);
                d.last_seen_ms = d.last_seen_ms.max(e.ts_ms);
                d.changes += 1;
            }
        }
        let listeners = self.listeners.lock().unwrap().clone();
        for e in &remote {
            for l in &listeners {
                l.remote_changed(e);
            }
            if !crate::drive::is_internal(&e.key) {
                let kind = if e.op == "delete" {
                    "remote_delete"
                } else {
                    "remote_change"
                };
                self.event(kind, &e.key, &e.device, e.size, &e.version, "");
            }
        }
        remote.len()
    }
}

/// The visible name for the losing copy of `key`:
/// `notes (conflict from laptop 2026-09-29 17.03.12).md`.
pub fn conflict_path(key: &str, device_name: &str, ts_ms: u64) -> String {
    let (dir, name) = match key.rfind('/') {
        Some(i) => (&key[..=i], &key[i + 1..]),
        None => ("", key),
    };
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    let device: String = device_name
        .chars()
        .map(|c| if c == '/' { '-' } else { c })
        .take(40)
        .collect();
    format!("{dir}{stem} (conflict from {device} {}){ext}", stamp(ts_ms))
}

/// `yyyy-mm-dd hh.mm.ss` (UTC) for a unix ms time.
fn stamp(ms: u64) -> String {
    let secs = ms / 1000;
    let (h, m, s) = ((secs / 3600) % 24, (secs / 60) % 60, secs % 60);
    // Civil date from days since the epoch (Howard Hinnant's algorithm).
    let z = (secs / 86400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if mo <= 2 { 1 } else { 0 };
    format!("{y:04}-{mo:02}-{d:02} {h:02}.{m:02}.{s:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::FsBackend;
    use crate::{Context, Drive};

    struct Seen(Mutex<Vec<FeedEntry>>);
    impl RemoteListener for Seen {
        fn remote_changed(&self, e: &FeedEntry) {
            self.0.lock().unwrap().push(e.clone());
        }
    }

    #[test]
    fn the_device_keeps_its_id_and_takes_a_name_override() {
        let dir = tempfile::tempdir().unwrap();
        let a = DeviceId::load_with(dir.path(), None).unwrap();
        assert!(!a.name.is_empty() && a.id.len() == 12);
        let b = DeviceId::load_with(dir.path(), Some("maya-mbp".into())).unwrap();
        assert_eq!(
            (b.id.as_str(), b.name.as_str()),
            (a.id.as_str(), "maya-mbp")
        );
        assert_eq!(
            DeviceId::load_with(dir.path(), Some(" ".into()))
                .unwrap()
                .name,
            a.name
        );
    }

    #[test]
    fn conflict_names_and_stamps() {
        assert_eq!(stamp(1_790_683_200_000), "2026-09-29 12.00.00");
        assert_eq!(
            conflict_path("public/notes.md", "laptop", 1_790_683_200_000),
            "public/notes (conflict from laptop 2026-09-29 12.00.00).md"
        );
        assert_eq!(
            conflict_path("Makefile", "a/b", 0),
            "Makefile (conflict from a-b 1970-01-01 00.00.00)"
        );
        assert_eq!(
            ts_of(".cua-feed/1790683200000-abc-12345678.json"),
            Some(1_790_683_200_000)
        );
        assert_eq!(
            device_of(".cua-feed/1790683200000-abc-12345678.json"),
            Some("abc")
        );
    }

    /// A bucket that stopped answering.
    struct Down;
    #[async_trait::async_trait]
    impl crate::Backend for Down {
        fn kind(&self) -> &'static str {
            "down"
        }
        fn remote(&self) -> bool {
            true
        }
        async fn put(&self, _: &str, _: Vec<u8>, _: Condition) -> Result<crate::ObjectMeta> {
            Err(Error::Backend("connection refused".into()))
        }
        async fn get(&self, _: &str, _: Option<&str>) -> Result<(Vec<u8>, crate::ObjectMeta)> {
            Err(Error::Backend("connection refused".into()))
        }
        async fn head(&self, _: &str) -> Result<Option<crate::ObjectMeta>> {
            Err(Error::Backend("connection refused".into()))
        }
        async fn list(&self, _: &str) -> Result<Vec<crate::ObjectMeta>> {
            Err(Error::Backend("connection refused".into()))
        }
        async fn delete(&self, _: &str, _: Condition) -> Result<()> {
            Err(Error::Backend("connection refused".into()))
        }
        async fn versions(&self, _: &str) -> Result<Vec<crate::VersionInfo>> {
            Err(Error::Backend("connection refused".into()))
        }
    }

    #[tokio::test]
    async fn an_unreachable_bucket_reports_offline_with_the_reason() {
        let dir = tempfile::tempdir().unwrap();
        let d = Drive::new(Arc::new(Down), dir.path());
        let f = Feed::new(
            &d,
            DeviceId {
                id: "deva".into(),
                name: "A".into(),
            },
        );
        f.spawn();
        let t0 = std::time::Instant::now();
        loop {
            let st = f.status();
            if st.feed == "offline" {
                assert!(st.last_error.unwrap().contains("connection refused"));
                break;
            }
            assert!(t0.elapsed() < Duration::from_secs(10), "{st:?}");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        f.stop();
    }

    #[tokio::test]
    async fn two_homes_on_one_store_see_each_others_changes() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("shared");
        let a = Drive::new(Arc::new(FsBackend::new(&data)), dir.path().join("a"));
        let b = Drive::new(Arc::new(FsBackend::new(&data)), dir.path().join("b"));
        let fa = Feed::new(
            &a,
            DeviceId {
                id: "deva".into(),
                name: "A".into(),
            },
        );
        let fb = Feed::new(
            &b,
            DeviceId {
                id: "devb".into(),
                name: "B".into(),
            },
        );
        let seen = Arc::new(Seen(Mutex::new(vec![])));
        fb.add_listener(seen.clone());
        fa.spawn();
        fb.spawn();
        let t0 = std::time::Instant::now();
        a.session(Context::user())
            .write("public/x.md", b"hi".to_vec(), Condition::None)
            .await
            .unwrap();
        let (events, next) = fb.events(0, Duration::from_secs(10)).await;
        assert!(t0.elapsed() < Duration::from_secs(5), "{:?}", t0.elapsed());
        let ev = events.iter().find(|e| e.kind == "remote_change").unwrap();
        assert_eq!(
            (ev.path.as_str(), ev.device.as_str()),
            ("public/x.md", "deva")
        );
        assert_eq!(seen.0.lock().unwrap()[0].key, "public/x.md");
        assert_eq!(fb.last_writer("public/x.md").as_deref(), Some("deva"));
        // Per file: B knows another device wrote it; A has nothing to say.
        let on_b = fb.file_sync("public/x.md");
        assert_eq!(on_b.state, "synced");
        assert!(on_b.written_by.is_some(), "{on_b:?}");
        assert!(fa.file_sync("public/x.md").is_plain());
        fb.set_pending("public/y.md", Some(3));
        assert_eq!(fb.file_sync("public/y.md").state, "pending_upload");
        assert_eq!(
            fb.status().pending,
            [PendingUpload {
                path: "public/y.md".into(),
                bytes: 3
            }]
        );
        fb.set_pending("public/y.md", None);
        // A's own change is not echoed back to A.
        let (mine, _) = fa.events(0, Duration::from_millis(1200)).await;
        assert!(mine.iter().all(|e| e.kind != "remote_change"), "{mine:?}");
        // Deletes travel too; the feed's own objects stay hidden.
        a.session(Context::user())
            .delete("public/x.md", Condition::None)
            .await
            .unwrap();
        let (events, _) = fb.events(next, Duration::from_secs(10)).await;
        assert_eq!(events[0].kind, "remote_delete");
        let names: Vec<String> = a
            .session(Context::user())
            .ls("")
            .await
            .unwrap()
            .into_iter()
            .map(|e| e.path)
            .collect();
        assert_eq!(names, ["agents/", "public/", "spaces/"]);
        let st = fb.status();
        // A store on this machine has no remote to hear from.
        assert_eq!(st.feed, "off");
        assert!(
            st.devices.iter().any(|d| d.id == "deva" && d.changes == 2),
            "{st:?}"
        );
    }
}
