// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The health of the relay's state volume (CUA-1297).
//!
//! Every state store (devices, audit log, device sessions, machine
//! directory, mesh registry, pool, usage, teams, pairing codes, grants)
//! reports each write here. A failed write
//! is counted, logged with a stable line (`state persist failed`, field
//! `alert = "state_persist_failed"`) and remembered as the store's last
//! error, so a full or read-only volume shows up on `GET /readyz` and
//! `GET /metrics` within one write, not only in the logs.
//!
//! - **Budget.** Pool writes (the only state an account can grow at will)
//!   must leave a reserve of free space on the state volume, measured with
//!   `statvfs`: at least [`StorageLimits::reserve_percent`] of the volume
//!   and [`StorageLimits::reserve_min_bytes`]. The other stores' writes
//!   are never held to it, so they keep working while pool appends are
//!   refused.
//! - **Probe.** [`Storage::probe`] writes, fsyncs and removes a small file
//!   in the state directory: the readiness check of the volume itself.
//! - **Temp files.** [`sweep_temps`] removes the `.<name>.<uuid>.tmp` files
//!   an interrupted [`crate::assertion::write_private`] leaves behind.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::assertion::now_secs;

/// A store whose last failed write is younger than this (and not followed
/// by a successful one) makes `/readyz` unhealthy.
pub const FAILING_WINDOW_SECS: u64 = 300;
/// Free space under this share of the volume is reported as `low_space`
/// (the alert threshold).
pub const LOW_SPACE_PERCENT: u64 = 20;
/// The probe's result is reused for this long (a burst of `/readyz` calls
/// writes once).
const PROBE_CACHE: Duration = Duration::from_secs(1);
/// At most one error line per store per this interval; the rest are
/// counted and reported with the next line.
const LOG_EVERY: Duration = Duration::from_secs(1);

/// The state stores, as named in logs and metrics.
pub mod store {
    /// `devices.json`: enrolled devices.
    pub const DEVICES: &str = "devices";
    /// `devices.json.audit.jsonl`: the audit log.
    pub const AUDIT: &str = "audit";
    /// `devices.json.sessions.jsonl`: live device sessions.
    pub const SESSIONS: &str = "sessions";
    /// The machine directory (the state file).
    pub const DIRECTORY: &str = "directory";
    /// The mesh registry.
    pub const MESH: &str = "mesh";
    /// The pools (lease, log, snapshot, terminal keys, keyring).
    pub const POOL: &str = "pool";
    /// `<state>.usage.json` (or `CUA_RELAY_USAGE_FILE`): this month's
    /// relay usage per user.
    pub const USAGE: &str = "usage";
    /// `<state>.teams.json`: teams, invites, team machines and approvals.
    pub const TEAMS: &str = "teams";
    /// `<state>.pairing.json`: one-time pairing codes.
    pub const PAIRING: &str = "pairing";
    /// `<state>.grants.json`: who reached each machine and whose access was
    /// revoked (old tickets refused).
    pub const GRANTS: &str = "grants";
    /// Every store, in report order.
    pub const ALL: [&str; 10] = [
        DEVICES, AUDIT, SESSIONS, DIRECTORY, MESH, POOL, USAGE, TEAMS, PAIRING, GRANTS,
    ];
}

/// Free-space limits for pool writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StorageLimits {
    /// Pool writes must leave at least this share of the volume free.
    pub reserve_percent: u64,
    /// ... and at least this many bytes.
    pub reserve_min_bytes: u64,
    /// Most bytes one account's pool may keep on disk (log, terminal keys,
    /// snapshot, keyring).
    pub pool_account_quota_bytes: u64,
}

impl Default for StorageLimits {
    fn default() -> Self {
        Self {
            reserve_percent: 15,
            reserve_min_bytes: 256 << 20,
            pool_account_quota_bytes: 512 << 20,
        }
    }
}

/// Size and free space of a volume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Space {
    /// Bytes on the volume.
    pub total: u64,
    /// Bytes an unprivileged writer may still use.
    pub available: u64,
}

impl Space {
    /// Free share of the volume, in percent.
    pub fn free_percent(&self) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        self.available as f64 * 100.0 / self.total as f64
    }
}

/// `statvfs` of the volume holding `dir`.
#[cfg(unix)]
pub fn space(dir: &Path) -> std::io::Result<Space> {
    use std::os::unix::ffi::OsStrExt as _;
    let c = std::ffi::CString::new(dir.as_os_str().as_bytes())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c` is a valid NUL-terminated path and `st` a writable
    // statvfs the call fills in.
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let unit = if st.f_frsize > 0 {
        st.f_frsize as u64
    } else {
        st.f_bsize as u64
    };
    Ok(Space {
        total: (st.f_blocks as u64).saturating_mul(unit),
        available: (st.f_bavail as u64).saturating_mul(unit),
    })
}

/// No `statvfs`: no budget is enforced.
#[cfg(not(unix))]
pub fn space(_dir: &Path) -> std::io::Result<Space> {
    Err(std::io::Error::other(
        "free space is not measured on this platform",
    ))
}

/// Whether `name` is a temp file [`crate::assertion::write_private`] (or
/// the probe) makes: `.<name>.<32 hex>.tmp`.
fn is_write_temp(name: &str) -> bool {
    let Some(rest) = name.strip_prefix('.').and_then(|r| r.strip_suffix(".tmp")) else {
        return false;
    };
    let Some((_, id)) = rest.rsplit_once('.') else {
        return false;
    };
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Removes the temp files interrupted writes left in `dir` and, `depth`
/// levels down, its subdirectories (the pools). Call it at startup, before
/// anything writes there (one relay per state directory). Returns the files
/// and bytes removed.
pub fn sweep_temps(dir: &Path, depth: usize) -> std::io::Result<(u64, u64)> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((0, 0)),
        Err(e) => return Err(e),
    };
    let (mut files, mut bytes) = (0u64, 0u64);
    for entry in entries {
        let entry = entry?;
        // A temp file can vanish between the listing and the stat or the
        // remove: its writer finished and renamed it, or gave up. Gone is
        // what the sweep wants, so it is not an error.
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        if meta.is_dir() {
            if depth > 0 {
                let (f, b) = sweep_temps(&entry.path(), depth - 1)?;
                files += f;
                bytes += b;
            }
            continue;
        }
        if meta.is_file() && is_write_temp(&entry.file_name().to_string_lossy()) {
            // Another sweep may be deleting it too (Windows reports that
            // as access denied until it finishes): gone either way.
            if crate::assertion::remove_if_present(&entry.path())? {
                files += 1;
                bytes += meta.len();
            }
        }
    }
    if files > 0 {
        tracing::warn!(
            dir = %dir.display(),
            files,
            bytes,
            "removed temp files left by interrupted writes"
        );
    }
    Ok((files, bytes))
}

#[derive(Debug, Default, Clone)]
struct StoreStatus {
    writes: u64,
    failures: u64,
    last_ok_at: u64,
    last_error_at: u64,
    last_error: Option<String>,
    /// The last write failed.
    failed_last: bool,
    /// Failures not logged yet (rate limit).
    unlogged: u64,
    last_logged: Option<Instant>,
}

impl StoreStatus {
    fn failing(&self, now: u64) -> bool {
        self.failed_last && now.saturating_sub(self.last_error_at) < FAILING_WINDOW_SECS
    }
}

/// Why a pool write was refused before it reached the disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The account's pool is at its quota (`quota`).
    Quota {
        /// Bytes it keeps now.
        used: u64,
        /// Its quota.
        quota: u64,
    },
    /// The volume is down to its reserve (`disk_reserve`).
    Reserve {
        /// Bytes free.
        available: u64,
        /// Bytes kept free.
        reserve: u64,
    },
}

impl Refusal {
    /// The error code clients see.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Quota { .. } => "quota",
            Self::Reserve { .. } => "disk_reserve",
        }
    }

    /// What clients are told.
    pub fn message(&self) -> String {
        match self {
            Self::Quota { used, quota } => format!(
                "the pool keeps {used} of its {quota} bytes on this relay: write a snapshot, then retry"
            ),
            Self::Reserve { available, reserve } => format!(
                "the relay's state volume is down to its reserve ({available} bytes free, {reserve} kept for devices and sign-ins): retry later"
            ),
        }
    }
}

/// The state volume's health, shared by every store of a relay.
#[derive(Debug)]
pub struct Storage {
    dir: Option<PathBuf>,
    limits: StorageLimits,
    stores: Mutex<BTreeMap<&'static str, StoreStatus>>,
    audit_dropped: AtomicU64,
    temp_files_removed: AtomicU64,
    temp_bytes_removed: AtomicU64,
    pool_refused: Mutex<BTreeMap<&'static str, u64>>,
    probe: Mutex<Option<(Instant, Result<Duration, String>)>>,
    probes_failed: AtomicU64,
}

impl Default for Storage {
    fn default() -> Self {
        Self::new(None, StorageLimits::default())
    }
}

impl Storage {
    /// The health of state kept in `dir` (`None`: in memory).
    pub fn new(dir: Option<PathBuf>, limits: StorageLimits) -> Self {
        Self {
            dir,
            limits,
            stores: Mutex::default(),
            audit_dropped: AtomicU64::new(0),
            temp_files_removed: AtomicU64::new(0),
            temp_bytes_removed: AtomicU64::new(0),
            pool_refused: Mutex::default(),
            probe: Mutex::new(None),
            probes_failed: AtomicU64::new(0),
        }
    }

    /// The state directory, if state is persisted.
    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    /// The limits.
    pub fn limits(&self) -> StorageLimits {
        self.limits
    }

    /// Records the outcome of a write of `store`, logging a failure.
    pub fn record<T, E: std::fmt::Display>(&self, store: &'static str, result: &Result<T, E>) {
        match result {
            Ok(_) => self.ok(store),
            Err(e) => self.failed(store, &e.to_string()),
        }
    }

    /// A write of `store` succeeded.
    pub fn ok(&self, store: &'static str) {
        let mut stores = self.stores.lock().expect("storage");
        let s = stores.entry(store).or_default();
        let recovered = s.failed_last;
        s.failed_last = false;
        s.writes += 1;
        s.last_ok_at = now_secs();
        if recovered {
            tracing::info!(store, failures = s.failures, "state persist recovered");
        }
    }

    /// A write of `store` failed with `error`.
    pub fn failed(&self, store: &'static str, error: &str) {
        let mut stores = self.stores.lock().expect("storage");
        let s = stores.entry(store).or_default();
        s.writes += 1;
        s.failures += 1;
        s.failed_last = true;
        s.last_error_at = now_secs();
        s.last_error = Some(error.chars().take(300).collect());
        s.unlogged += 1;
        if s.last_logged.is_none_or(|t| t.elapsed() >= LOG_EVERY) {
            tracing::error!(
                alert = "state_persist_failed",
                store,
                error,
                failures = s.failures,
                since_last_line = s.unlogged,
                "state persist failed"
            );
            s.unlogged = 0;
            s.last_logged = Some(Instant::now());
        }
    }

    /// An audit event was not recorded (its write failed).
    pub fn audit_dropped(&self) {
        let n = self.audit_dropped.fetch_add(1, Ordering::Relaxed) + 1;
        if n.is_power_of_two() {
            tracing::error!(
                alert = "audit_event_dropped",
                dropped_total = n,
                "audit events dropped"
            );
        }
    }

    /// Audit events dropped since start.
    pub fn audit_dropped_total(&self) -> u64 {
        self.audit_dropped.load(Ordering::Relaxed)
    }

    /// Temp files removed at startup.
    pub fn temps_removed(&self, files: u64, bytes: u64) {
        self.temp_files_removed.fetch_add(files, Ordering::Relaxed);
        self.temp_bytes_removed.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Persist failures of `store` since start.
    pub fn failures(&self, store: &str) -> u64 {
        self.stores
            .lock()
            .expect("storage")
            .get(store)
            .map_or(0, |s| s.failures)
    }

    /// Whether `store`'s last write succeeded (true before any write).
    pub fn store_ok(&self, store: &str) -> bool {
        !self
            .stores
            .lock()
            .expect("storage")
            .get(store)
            .is_some_and(|s| s.failing(now_secs()))
    }

    /// Whether no store is failing.
    pub fn writes_ok(&self) -> bool {
        let now = now_secs();
        !self
            .stores
            .lock()
            .expect("storage")
            .values()
            .any(|s| s.failing(now))
    }

    /// The state volume's size and free space (`None`: in memory, or not
    /// measurable).
    pub fn space(&self) -> Option<Space> {
        self.dir.as_deref().and_then(|d| space(d).ok())
    }

    /// Bytes pool writes must leave free on a volume of `total` bytes.
    pub fn reserve_for(&self, total: u64) -> u64 {
        (total / 100)
            .saturating_mul(self.limits.reserve_percent)
            .max(self.limits.reserve_min_bytes)
    }

    /// Whether the pool of an account using `used` bytes may write `bytes`
    /// more. `snapshot` writes (which shrink the log) may use half of the
    /// reserve and ignore the quota, so a full log can always be compacted.
    pub fn admit_pool(&self, used: u64, bytes: u64, snapshot: bool) -> Result<(), Refusal> {
        let quota = self.limits.pool_account_quota_bytes;
        if !snapshot && used.saturating_add(bytes) > quota {
            return Err(self.refused(Refusal::Quota { used, quota }));
        }
        if let Some(space) = self.space() {
            let mut reserve = self.reserve_for(space.total);
            if snapshot {
                reserve /= 2;
            }
            if space.available < reserve.saturating_add(bytes) {
                return Err(self.refused(Refusal::Reserve {
                    available: space.available,
                    reserve,
                }));
            }
        }
        Ok(())
    }

    fn refused(&self, r: Refusal) -> Refusal {
        let n = {
            let mut m = self.pool_refused.lock().expect("refused");
            let n = m.entry(r.code()).or_default();
            *n += 1;
            *n
        };
        if n.is_power_of_two() {
            tracing::warn!(
                alert = "pool_write_refused",
                reason = r.code(),
                refused_total = n,
                "pool write refused: {}",
                r.message()
            );
        }
        r
    }

    /// Pool writes refused for `reason` since start.
    pub fn pool_refused(&self, reason: &str) -> u64 {
        self.pool_refused
            .lock()
            .expect("refused")
            .get(reason)
            .copied()
            .unwrap_or(0)
    }

    /// Writes, fsyncs and removes a small file in the state directory
    /// (blocking; at most once per second, else the last result).
    /// `Ok(None)`: state is in memory.
    pub fn probe(&self) -> Result<Option<Duration>, String> {
        let Some(dir) = &self.dir else {
            return Ok(None);
        };
        let mut last = self.probe.lock().expect("probe");
        if let Some((at, r)) = last.as_ref() {
            if at.elapsed() < PROBE_CACHE {
                return r.clone().map(Some);
            }
        }
        let r = probe_dir(dir);
        if let Err(e) = &r {
            self.probes_failed.fetch_add(1, Ordering::Relaxed);
            tracing::error!(
                alert = "state_probe_failed",
                dir = %dir.display(),
                error = %e,
                "state volume write probe failed"
            );
        }
        *last = Some((Instant::now(), r.clone()));
        r.map(Some)
    }

    fn last_probe_ok(&self) -> Option<bool> {
        self.probe
            .lock()
            .expect("probe")
            .as_ref()
            .map(|(_, r)| r.is_ok())
    }

    /// The `/readyz` body and whether the state is ready: the probe passed
    /// and no store is failing. Low free space is reported, not failed on
    /// (pool writes are refused before the volume fills).
    pub fn report(&self) -> (bool, Value) {
        let probe = self.probe();
        let now = now_secs();
        let stores: serde_json::Map<String, Value> = {
            let map = self.stores.lock().expect("storage");
            store::ALL
                .iter()
                .map(|name| {
                    let s = map.get(name).cloned().unwrap_or_default();
                    (
                        name.to_string(),
                        json!({
                            "ok": !s.failing(now),
                            "writes": s.writes,
                            "failures": s.failures,
                            "last_ok_at": (s.last_ok_at > 0).then_some(s.last_ok_at),
                            "last_error": s.last_error,
                            "last_error_at": (s.last_error_at > 0).then_some(s.last_error_at),
                        }),
                    )
                })
                .collect()
        };
        let failing: Vec<String> = stores
            .iter()
            .filter(|(_, v)| v["ok"] == json!(false))
            .map(|(k, _)| k.clone())
            .collect();
        let disk = self.space().map(|s| {
            let reserve = self.reserve_for(s.total);
            json!({
                "total_bytes": s.total,
                "available_bytes": s.available,
                "free_percent": (s.free_percent() * 10.0).round() / 10.0,
                "pool_reserve_bytes": reserve,
                "low_space": s.free_percent() < LOW_SPACE_PERCENT as f64,
                "pool_writes_refused": s.available < reserve,
            })
        });
        let ready = probe.is_ok() && failing.is_empty();
        let body = json!({
            "ready": ready,
            "persistent": self.dir.is_some(),
            "probe": match &probe {
                Ok(Some(d)) => json!({"ok": true, "ms": d.as_secs_f64() * 1000.0}),
                Ok(None) => json!({"ok": true, "skipped": "state is in memory"}),
                Err(e) => json!({"ok": false, "error": e}),
            },
            "disk": disk,
            "stores": stores,
            "failing": failing,
            "audit_events_dropped": self.audit_dropped_total(),
            "pool_writes_refused": {
                "quota": self.pool_refused("quota"),
                "disk_reserve": self.pool_refused("disk_reserve"),
            },
        });
        (ready, body)
    }

    /// Prometheus text exposition of the counters.
    pub fn metrics(&self) -> String {
        use std::fmt::Write as _;
        fn head(out: &mut String, name: &str, kind: &str, help: &str) {
            let _ = writeln!(out, "# HELP {name} {help}\n# TYPE {name} {kind}");
        }
        fn one(out: &mut String, name: &str, kind: &str, help: &str, v: u64) {
            head(out, name, kind, help);
            let _ = writeln!(out, "{name} {v}");
        }
        let now = now_secs();
        let stores = self.stores.lock().expect("storage").clone();
        let status = |name: &str| stores.get(name).cloned().unwrap_or_default();
        let mut out = String::new();
        head(
            &mut out,
            "cua_relay_state_writes_total",
            "counter",
            "State file writes, by store.",
        );
        for name in store::ALL {
            let _ = writeln!(
                out,
                "cua_relay_state_writes_total{{store=\"{name}\"}} {}",
                status(name).writes
            );
        }
        head(
            &mut out,
            "cua_relay_state_persist_failures_total",
            "counter",
            "Failed state file writes, by store.",
        );
        for name in store::ALL {
            let _ = writeln!(
                out,
                "cua_relay_state_persist_failures_total{{store=\"{name}\"}} {}",
                status(name).failures
            );
        }
        head(
            &mut out,
            "cua_relay_state_store_ok",
            "gauge",
            "1 unless the store's last write failed (within 300 s).",
        );
        for name in store::ALL {
            let _ = writeln!(
                out,
                "cua_relay_state_store_ok{{store=\"{name}\"}} {}",
                u8::from(!status(name).failing(now))
            );
        }
        one(
            &mut out,
            "cua_relay_audit_events_dropped_total",
            "counter",
            "Audit events not recorded because their write failed.",
            self.audit_dropped_total(),
        );
        one(
            &mut out,
            "cua_relay_state_temp_files_removed_total",
            "counter",
            "Temp files of interrupted writes removed at startup.",
            self.temp_files_removed.load(Ordering::Relaxed),
        );
        one(
            &mut out,
            "cua_relay_state_temp_bytes_removed_total",
            "counter",
            "Bytes of temp files of interrupted writes removed at startup.",
            self.temp_bytes_removed.load(Ordering::Relaxed),
        );
        one(
            &mut out,
            "cua_relay_state_probe_failures_total",
            "counter",
            "Failed write probes of the state volume.",
            self.probes_failed.load(Ordering::Relaxed),
        );
        if let Some(ok) = self.last_probe_ok() {
            one(
                &mut out,
                "cua_relay_state_probe_ok",
                "gauge",
                "1 if the last write probe of the state volume passed.",
                u64::from(ok),
            );
        }
        head(
            &mut out,
            "cua_relay_pool_writes_refused_total",
            "counter",
            "Pool writes refused before reaching the disk, by reason.",
        );
        for reason in ["quota", "disk_reserve"] {
            let _ = writeln!(
                out,
                "cua_relay_pool_writes_refused_total{{reason=\"{reason}\"}} {}",
                self.pool_refused(reason)
            );
        }
        if let Some(s) = self.space() {
            one(
                &mut out,
                "cua_relay_state_volume_bytes",
                "gauge",
                "Size of the state volume.",
                s.total,
            );
            one(
                &mut out,
                "cua_relay_state_volume_available_bytes",
                "gauge",
                "Free bytes on the state volume.",
                s.available,
            );
            one(
                &mut out,
                "cua_relay_state_pool_reserve_bytes",
                "gauge",
                "Free bytes pool writes leave for devices and sign-ins.",
                self.reserve_for(s.total),
            );
        }
        out
    }
}

/// Writes, fsyncs and removes a 4 KiB file in `dir`.
fn probe_dir(dir: &Path) -> Result<Duration, String> {
    use std::io::Write as _;
    let start = Instant::now();
    let path = dir.join(format!(
        ".readyz-probe.{}.tmp",
        uuid::Uuid::new_v4().simple()
    ));
    let result = (|| {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        f.write_all(&[0u8; 4096])?;
        f.sync_all()
    })();
    let removed = std::fs::remove_file(&path);
    result.map_err(|e| format!("write probe: {e}"))?;
    removed.map_err(|e| format!("write probe cleanup: {e}"))?;
    Ok(start.elapsed())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_temps_are_recognized_and_swept_two_levels_deep() {
        let dir = tempfile::tempdir().unwrap();
        let pool = dir.path().join("relay.pool").join("abcd");
        std::fs::create_dir_all(&pool).unwrap();
        let id = uuid::Uuid::new_v4().simple().to_string();
        for (d, name) in [
            (
                dir.path().to_path_buf(),
                format!(".relay.devices.json.{id}.tmp"),
            ),
            (dir.path().to_path_buf(), format!(".readyz-probe.{id}.tmp")),
            (pool.clone(), format!(".log.jsonl.{id}.tmp")),
        ] {
            std::fs::write(d.join(name), "partial").unwrap();
        }
        for (d, name) in [
            (dir.path().to_path_buf(), "relay.devices.json".to_string()),
            (dir.path().to_path_buf(), ".notes.tmp".to_string()),
            (dir.path().to_path_buf(), ".x.short.tmp".to_string()),
            (pool.clone(), "log.jsonl".to_string()),
        ] {
            std::fs::write(d.join(name), "keep").unwrap();
        }
        assert_eq!(sweep_temps(dir.path(), 2).unwrap(), (3, 21));
        assert_eq!(sweep_temps(dir.path(), 2).unwrap(), (0, 0));
        assert!(pool.join("log.jsonl").exists());
        assert!(dir.path().join(".notes.tmp").exists());
    }

    /// A previous relay in the same process (a restart in a test, a handover)
    /// can still be writing while the next one sweeps: a temp file that is
    /// renamed or removed between the listing and the stat or the remove must
    /// not fail the start.
    #[test]
    fn a_sweep_racing_the_removal_of_its_temps_does_not_fail() {
        use std::sync::Arc;
        let dir = tempfile::tempdir().unwrap();
        let names: Vec<_> = (0..4000)
            .map(|_| {
                let id = uuid::Uuid::new_v4().simple();
                dir.path().join(format!(".relay.mesh.json.{id}.tmp"))
            })
            .collect();
        for name in &names {
            std::fs::write(name, "partial").unwrap();
        }
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let remover = {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                for name in &names {
                    let _ = std::fs::remove_file(name);
                }
            })
        };
        barrier.wait();
        let swept = sweep_temps(dir.path(), 0);
        remover.join().unwrap();
        swept.unwrap();
    }

    #[test]
    fn failures_are_counted_reported_and_cleared_by_a_success() {
        let dir = tempfile::tempdir().unwrap();
        let s = Storage::new(Some(dir.path().into()), StorageLimits::default());
        assert!(s.writes_ok());
        s.record(store::MESH, &Err::<(), _>("No space left on device"));
        assert!(!s.writes_ok());
        assert!(!s.store_ok(store::MESH));
        assert_eq!(s.failures(store::MESH), 1);
        let (ready, body) = s.report();
        assert!(!ready);
        assert_eq!(body["failing"], json!(["mesh"]));
        assert_eq!(
            body["stores"]["mesh"]["last_error"],
            "No space left on device"
        );
        assert!(s
            .metrics()
            .contains("cua_relay_state_persist_failures_total{store=\"mesh\"} 1"));
        s.record(store::MESH, &Ok::<(), String>(()));
        assert!(s.writes_ok());
        let (ready, body) = s.report();
        assert!(ready, "{body}");
        assert_eq!(body["probe"]["ok"], true);
        // Free space is measured with statvfs (Unix only).
        #[cfg(unix)]
        assert!(body["disk"]["total_bytes"].as_u64().unwrap() > 0);
    }

    #[test]
    fn the_quota_and_the_reserve_refuse_pool_writes() {
        let dir = tempfile::tempdir().unwrap();
        // No reserve here, so the quota is all that refuses whatever the
        // test machine's free space (a CI runner's disk is often near full).
        let limits = StorageLimits {
            pool_account_quota_bytes: 1000,
            reserve_percent: 0,
            reserve_min_bytes: 0,
        };
        let s = Storage::new(Some(dir.path().into()), limits);
        assert!(s.admit_pool(0, 1000, false).is_ok());
        assert_eq!(s.admit_pool(500, 501, false).unwrap_err().code(), "quota");
        // A snapshot compacts: never held to the quota.
        assert!(s.admit_pool(5000, 10, true).is_ok());
        // A reserve larger than the volume refuses every pool write (free
        // space is measured with statvfs, on Unix only).
        #[cfg(unix)]
        {
            let s = Storage::new(
                Some(dir.path().into()),
                StorageLimits {
                    reserve_percent: 100,
                    ..StorageLimits::default()
                },
            );
            assert_eq!(
                s.admit_pool(0, 1, false).unwrap_err().code(),
                "disk_reserve"
            );
            assert_eq!(s.pool_refused("disk_reserve"), 1);
        }
        // In memory: only the quota applies.
        let s = Storage::new(None, StorageLimits::default());
        assert!(s.admit_pool(0, 1 << 20, false).is_ok());
        assert_eq!(s.probe(), Ok(None));
    }
}
