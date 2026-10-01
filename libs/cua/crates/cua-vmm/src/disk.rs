//! Disk-space safety shared by every crate that writes large files.
//!
//! * [`ensure_space`]: the low-disk guard. Pulls, builds and VM creates call
//!   it with the bytes they expect to write; it fails with a typed
//!   [`InsufficientDisk`] (hint: `cua cache prune`) instead of filling the
//!   volume, and warns when free space drops under the warning threshold.
//! * [`CacheConfig`]: the cache budget and thresholds, from
//!   `$CUA_HOME/cache.json` and `CUA_CACHE_BUDGET` / `CUA_DISK_MIN_FREE` /
//!   `CUA_DISK_WARN_FREE` / `CUA_ORPHAN_REAP_MINUTES`.
//! * [`allocated_size`], [`mark_used`], [`last_used`]: accounting and LRU
//!   bookkeeping (allocated blocks, so sparse qcow2 files and APFS clones
//!   count what they really use).
//! * [`logs`]: size-capped log rotation.
//!
//! Free space comes from `statvfs` (Unix) or `GetDiskFreeSpaceExW`
//! (Windows). [`set_probe`] replaces it (tests, embedders);
//! `CUA_DISK_FAKE_AVAILABLE=<size>` does the same for a whole process (CLI
//! tests).

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

/// One GiB.
pub const GIB: u64 = 1 << 30;
/// Default free space kept on the volume: an operation that would leave
/// less fails ([`InsufficientDisk`]).
pub const DEFAULT_MIN_FREE: u64 = 5 * GIB;
/// Default warning threshold: operations still run, with a warning.
pub const DEFAULT_WARN_FREE: u64 = 20 * GIB;
/// Ceiling of the automatic cache budget.
pub const AUTO_BUDGET_CAP: u64 = 30 * GIB;
/// Share of the space available to the cache (free space plus what the
/// cache already holds) that the automatic budget allows, in percent.
pub const AUTO_BUDGET_PERCENT: u64 = 10;
/// Default age after which an ephemeral sandbox whose process is gone is
/// reaped.
pub const DEFAULT_ORPHAN_REAP_MINUTES: u64 = 10;

/// What a container image pull is assumed to write when its size is not
/// known up front (the engine reports it only after the pull).
pub const CONTAINER_PULL_ESTIMATE: u64 = 2 * GIB;
/// What a new VM instance (overlay, firmware vars, first boot writes) is
/// assumed to write.
pub const VM_CREATE_ESTIMATE: u64 = 4 * GIB;
/// What a local image build (build container export, pack and import, or a
/// VM build's flattened disk) is assumed to write on top of its base.
pub const BUILD_ESTIMATE: u64 = 4 * GIB;
/// What pulling a Lume (macOS) base VM is assumed to write.
pub const LUME_PULL_ESTIMATE: u64 = 30 * GIB;

/// `CUA_CACHE_BUDGET`: `auto` (default), `off`, or a size (`50G`).
pub const ENV_BUDGET: &str = "CUA_CACHE_BUDGET";
/// `CUA_DISK_MIN_FREE`: free space kept on the volume (size).
pub const ENV_MIN_FREE: &str = "CUA_DISK_MIN_FREE";
/// `CUA_DISK_WARN_FREE`: warn when free space would drop under this (size).
pub const ENV_WARN_FREE: &str = "CUA_DISK_WARN_FREE";
/// `CUA_ORPHAN_REAP_MINUTES`: minutes before an orphaned ephemeral sandbox
/// is reaped.
pub const ENV_ORPHAN_REAP_MINUTES: &str = "CUA_ORPHAN_REAP_MINUTES";
/// `CUA_CACHE_AUTO_GC`: `0` turns the automatic garbage collection off.
pub const ENV_AUTO_GC: &str = "CUA_CACHE_AUTO_GC";
/// Test hook: report this much available space (size) on every volume.
pub const ENV_FAKE_AVAILABLE: &str = "CUA_DISK_FAKE_AVAILABLE";
/// File name of the cache configuration under the cua home.
pub const CONFIG_FILE: &str = "cache.json";
/// Marker file whose mtime records when a cache directory was last used.
pub const LAST_USED: &str = ".last-used";

// ------------------------------------------------------------------ space

/// Space on the volume holding a path.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Space {
    /// Bytes available to this user.
    pub available: u64,
    /// Volume size in bytes.
    pub total: u64,
}

/// Where free-space numbers come from (`statvfs` by default).
pub trait SpaceProbe: Send + Sync {
    /// Space on the volume holding `path` (which may not exist yet).
    fn space(&self, path: &Path) -> std::io::Result<Space>;
}

/// The host's filesystem (`statvfs` / `GetDiskFreeSpaceExW`).
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemProbe;

impl SpaceProbe for SystemProbe {
    fn space(&self, path: &Path) -> std::io::Result<Space> {
        system_space(&nearest_existing(path))
    }
}

/// The closest ancestor of `path` that exists (free space is per volume).
fn nearest_existing(path: &Path) -> PathBuf {
    let mut p = path.to_path_buf();
    loop {
        if p.exists() {
            return p;
        }
        if !p.pop() {
            return PathBuf::from(".");
        }
    }
}

#[cfg(unix)]
fn system_space(path: &Path) -> std::io::Result<Space> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    // SAFETY: `c` is a valid NUL-terminated path and `st` is plain data the
    // call fills in.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let frsize = if st.f_frsize > 0 {
        st.f_frsize as u64
    } else {
        st.f_bsize as u64
    };
    Ok(Space {
        available: (st.f_bavail as u64).saturating_mul(frsize),
        total: (st.f_blocks as u64).saturating_mul(frsize),
    })
}

#[cfg(windows)]
fn system_space(path: &Path) -> std::io::Result<Space> {
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
    // SAFETY: `wide` is NUL-terminated and the out pointers are valid.
    let ok = unsafe {
        windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut avail,
            &mut total,
            &mut free,
        )
    };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(Space {
        available: avail,
        total,
    })
}

#[cfg(not(any(unix, windows)))]
fn system_space(_: &Path) -> std::io::Result<Space> {
    Err(std::io::Error::other("free space is not available here"))
}

static PROBE: RwLock<Option<Arc<dyn SpaceProbe>>> = RwLock::new(None);

/// Replaces the free-space source for this process (`None` restores
/// `statvfs`). Tests use it to simulate a full disk.
pub fn set_probe(probe: Option<Arc<dyn SpaceProbe>>) {
    *PROBE.write().unwrap_or_else(|e| e.into_inner()) = probe;
}

/// Space on the volume holding `path`: the [`set_probe`] override, then
/// `CUA_DISK_FAKE_AVAILABLE`, then the filesystem.
pub fn space(path: &Path) -> std::io::Result<Space> {
    if let Some(p) = PROBE.read().unwrap_or_else(|e| e.into_inner()).clone() {
        return p.space(path);
    }
    if let Some(avail) = std::env::var(ENV_FAKE_AVAILABLE)
        .ok()
        .and_then(|v| parse_size(&v))
    {
        let total = SystemProbe
            .space(path)
            .map(|s| s.total)
            .unwrap_or(avail)
            .max(avail);
        return Ok(Space {
            available: avail,
            total,
        });
    }
    SystemProbe.space(path)
}

// ------------------------------------------------------------------ guard

/// An operation would leave less than the configured minimum free space.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error(
    "not enough disk space to {what}: it needs about {} and cua keeps {} free, but only {} is \
     available on {} (run `cua cache prune` to free space, or lower CUA_DISK_MIN_FREE)",
    format_size(*.needed),
    format_size(*.min_free),
    format_size(*.available),
    .path.display()
)]
pub struct InsufficientDisk {
    /// What was about to happen (`pull ghcr.io/...`).
    pub what: String,
    /// Volume checked (the path the operation writes to).
    pub path: PathBuf,
    /// Bytes the operation expects to write.
    pub needed: u64,
    /// Bytes available.
    pub available: u64,
    /// Free space kept on the volume.
    pub min_free: u64,
}

/// Checks that writing `needed` bytes under `path` leaves at least the
/// configured minimum free (`CUA_DISK_MIN_FREE`, default 5 GiB), and warns
/// when it would leave less than the warning threshold. An unknown free
/// space never blocks.
pub fn ensure_space(path: &Path, needed: u64, what: &str) -> Result<(), InsufficientDisk> {
    let cfg = CacheConfig::load();
    ensure_space_with(path, needed, what, cfg.min_free, cfg.warn_free)
}

/// [`ensure_space`] with explicit thresholds.
pub fn ensure_space_with(
    path: &Path,
    needed: u64,
    what: &str,
    min_free: u64,
    warn_free: u64,
) -> Result<(), InsufficientDisk> {
    ensure_space_in(&GlobalProbe, path, needed, what, min_free, warn_free)
}

/// The process-wide source [`space`] reads.
struct GlobalProbe;

impl SpaceProbe for GlobalProbe {
    fn space(&self, path: &Path) -> std::io::Result<Space> {
        space(path)
    }
}

/// [`ensure_space_with`] against an explicit free-space source.
pub fn ensure_space_in(
    probe: &dyn SpaceProbe,
    path: &Path,
    needed: u64,
    what: &str,
    min_free: u64,
    warn_free: u64,
) -> Result<(), InsufficientDisk> {
    let s = match probe.space(path) {
        Ok(s) => s,
        Err(e) => {
            tracing::debug!(path = %path.display(), error = %e, "free space unknown; not checking");
            return Ok(());
        }
    };
    if s.available < needed.saturating_add(min_free) {
        return Err(InsufficientDisk {
            what: what.to_string(),
            path: path.to_path_buf(),
            needed,
            available: s.available,
            min_free,
        });
    }
    let after = s.available - needed;
    if after < warn_free {
        tracing::warn!(
            path = %path.display(),
            free_after = %format_size(after),
            "disk space is low: {what} leaves {} free; run `cua cache prune`",
            format_size(after)
        );
    }
    Ok(())
}

// ------------------------------------------------------------------ config

/// How much the cua cache may hold before garbage collection evicts the
/// least recently used unreferenced entries.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Budget {
    /// `min(30 GiB, 10% of (free space + current cache size))`.
    #[default]
    Auto,
    /// A fixed number of bytes.
    Bytes(u64),
    /// No automatic eviction (`cua cache prune` still works).
    Off,
}

impl Budget {
    /// Parses `auto`, `off` or a size.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "auto" => Some(Budget::Auto),
            "off" | "none" | "unlimited" => Some(Budget::Off),
            other => parse_size(other).map(Budget::Bytes),
        }
    }

    /// Resolved bytes, given the space available and what the cache holds
    /// (`None`: no limit).
    pub fn resolve(self, available: u64, cache_bytes: u64) -> Option<u64> {
        match self {
            Budget::Auto => Some(auto_budget(available, cache_bytes)),
            Budget::Bytes(b) => Some(b),
            Budget::Off => None,
        }
    }
}

impl std::fmt::Display for Budget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Budget::Auto => f.write_str("auto"),
            Budget::Bytes(b) => f.write_str(&format_size(*b)),
            Budget::Off => f.write_str("off"),
        }
    }
}

/// The automatic budget: a tenth of what the cache could grow into (free
/// space plus its own size, so the cache never shrinks its own budget by
/// existing), capped at 30 GiB.
pub fn auto_budget(available: u64, cache_bytes: u64) -> u64 {
    (available.saturating_add(cache_bytes) / 100 * AUTO_BUDGET_PERCENT).min(AUTO_BUDGET_CAP)
}

/// `$CUA_HOME/cache.json` (every field optional).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheConfigFile {
    /// `auto`, `off` or a size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<String>,
    /// Size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_free: Option<String>,
    /// Size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warn_free: Option<String>,
    /// Minutes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orphan_reap_minutes: Option<u64>,
    /// Automatic garbage collection after pulls and builds and on daemon
    /// idle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_gc: Option<bool>,
}

impl CacheConfigFile {
    /// Reads `path` (a missing or unreadable file is empty).
    pub fn read(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// Writes `path` atomically.
    pub fn write(&self, path: &Path) -> std::io::Result<()> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let tmp = path.with_extension(format!("json.tmp.{}", std::process::id()));
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(tmp, path)
    }
}

/// Effective cache settings: environment over `cache.json` over defaults.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CacheConfig {
    /// Cache budget.
    pub budget: Budget,
    /// Free space kept on the volume.
    pub min_free: u64,
    /// Warning threshold.
    pub warn_free: u64,
    /// Orphaned ephemeral sandboxes are reaped after this long.
    pub orphan_reap_after: Duration,
    /// Garbage-collect automatically.
    pub auto_gc: bool,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            budget: Budget::Auto,
            min_free: DEFAULT_MIN_FREE,
            warn_free: DEFAULT_WARN_FREE,
            orphan_reap_after: Duration::from_secs(DEFAULT_ORPHAN_REAP_MINUTES * 60),
            auto_gc: true,
        }
    }
}

impl CacheConfig {
    /// `$CUA_HOME/cache.json` plus the environment.
    pub fn load() -> Self {
        Self::load_from(&crate::host::cua_home().join(CONFIG_FILE), |k| {
            std::env::var(k).ok()
        })
    }

    /// Settings from `file` and an environment lookup.
    pub fn load_from(file: &Path, env: impl Fn(&str) -> Option<String>) -> Self {
        let f = CacheConfigFile::read(file);
        let d = Self::default();
        let pick =
            |k: &str, v: &Option<String>| env(k).filter(|s| !s.trim().is_empty()).or(v.clone());
        Self {
            budget: pick(ENV_BUDGET, &f.budget)
                .and_then(|s| Budget::parse(&s))
                .unwrap_or(d.budget),
            min_free: pick(ENV_MIN_FREE, &f.min_free)
                .and_then(|s| parse_size(&s))
                .unwrap_or(d.min_free),
            warn_free: pick(ENV_WARN_FREE, &f.warn_free)
                .and_then(|s| parse_size(&s))
                .unwrap_or(d.warn_free),
            orphan_reap_after: env(ENV_ORPHAN_REAP_MINUTES)
                .and_then(|s| s.trim().parse::<u64>().ok())
                .or(f.orphan_reap_minutes)
                .map(|m| Duration::from_secs(m * 60))
                .unwrap_or(d.orphan_reap_after),
            auto_gc: env(ENV_AUTO_GC)
                .map(|v| !matches!(v.trim(), "0" | "false" | "off" | "no"))
                .or(f.auto_gc)
                .unwrap_or(d.auto_gc),
        }
    }
}

// ------------------------------------------------------------------ sizes

/// Parses `123`, `500M`, `20G`, `20GiB`, `1.5T` (binary units; `k` is KiB).
pub fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim();
    let split = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let n: f64 = num.parse().ok()?;
    if !n.is_finite() || n < 0.0 {
        return None;
    }
    let mult: u64 = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1,
        "k" | "kb" | "kib" => 1 << 10,
        "m" | "mb" | "mib" => 1 << 20,
        "g" | "gb" | "gib" => 1 << 30,
        "t" | "tb" | "tib" => 1 << 40,
        _ => return None,
    };
    Some((n * mult as f64) as u64)
}

/// `1.2 GiB`, `512 MiB`, `17 B`.
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{bytes} B")
    } else if v >= 100.0 {
        format!("{v:.0} {}", UNITS[i])
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

// ------------------------------------------------------------------ usage

/// Bytes a file or tree occupies on disk (allocated blocks, so sparse files
/// and copy-on-write clones count what they use). Symlinks are not
/// followed. A missing path is 0.
pub fn allocated_size(path: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    let mut total = allocated_of(&meta);
    if meta.is_dir() {
        let mut stack = vec![path.to_path_buf()];
        // Bounded: every directory is visited once (no symlinks followed).
        while let Some(dir) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in rd.flatten() {
                let Ok(m) = e.metadata() else { continue };
                total = total.saturating_add(allocated_of(&m));
                if m.is_dir() {
                    stack.push(e.path());
                }
            }
        }
    }
    total
}

fn allocated_of(meta: &std::fs::Metadata) -> u64 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        meta.blocks().saturating_mul(512)
    }
    #[cfg(not(unix))]
    {
        meta.len()
    }
}

/// Records that `path` (a cache directory or file) was just used: touches
/// `<dir>/.last-used`, or the file's mtime.
pub fn mark_used(path: &Path) {
    mark_used_at(path, SystemTime::now());
}

/// [`mark_used`] at a given time (tests use a fake clock).
pub fn mark_used_at(path: &Path, at: SystemTime) {
    let target = if path.is_dir() {
        let m = path.join(LAST_USED);
        if !m.exists() {
            let _ = std::fs::write(&m, b"");
        }
        m
    } else {
        path.to_path_buf()
    };
    let _ = filetime::set_file_mtime(&target, filetime::FileTime::from_system_time(at));
}

/// When `path` was last used: `<dir>/.last-used`, else the mtime.
pub fn last_used(path: &Path) -> Option<SystemTime> {
    let marker = path.join(LAST_USED);
    std::fs::metadata(&marker)
        .or_else(|_| std::fs::metadata(path))
        .and_then(|m| m.modified())
        .ok()
}

// ------------------------------------------------------------------ logs

/// Size-capped log rotation.
pub mod logs {
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::path::{Path, PathBuf};

    /// Default cap of one log file.
    pub const DEFAULT_MAX_BYTES: u64 = 10 << 20;
    /// Default number of rotated files kept (`.1` .. `.N`).
    pub const DEFAULT_KEEP: usize = 2;

    fn numbered(path: &Path, n: usize) -> PathBuf {
        let mut s = path.as_os_str().to_owned();
        s.push(format!(".{n}"));
        PathBuf::from(s)
    }

    fn shift(path: &Path, keep: usize) {
        let _ = std::fs::remove_file(numbered(path, keep.max(1)));
        for n in (1..keep).rev() {
            let _ = std::fs::rename(numbered(path, n), numbered(path, n + 1));
        }
    }

    /// Rotated files of `path` (`path.1`, `path.2`, ...), for accounting.
    pub fn rotated(path: &Path, keep: usize) -> Vec<PathBuf> {
        (1..=keep.max(1))
            .map(|n| numbered(path, n))
            .filter(|p| p.exists())
            .collect()
    }

    /// Renames `path` to `path.1` (shifting older files, dropping the
    /// oldest past `keep`) when it is larger than `max_bytes`. For files no
    /// other process holds open.
    pub fn rotate(path: &Path, max_bytes: u64, keep: usize) -> std::io::Result<bool> {
        let len = match std::fs::metadata(path) {
            Ok(m) => m.len(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e),
        };
        if len <= max_bytes {
            return Ok(false);
        }
        if keep == 0 {
            std::fs::remove_file(path)?;
            return Ok(true);
        }
        shift(path, keep);
        std::fs::rename(path, numbered(path, 1))?;
        Ok(true)
    }

    /// For a log another process appends to (launchd/systemd stdout,
    /// `>>` redirects): copies it to `path.1` and truncates it in place when
    /// it is larger than `max_bytes`, keeping at most the last `max_bytes`
    /// in the copy. The writer keeps its descriptor.
    pub fn copy_truncate(path: &Path, max_bytes: u64, keep: usize) -> std::io::Result<bool> {
        let len = match std::fs::metadata(path) {
            Ok(m) => m.len(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e),
        };
        if len <= max_bytes {
            return Ok(false);
        }
        if keep > 0 {
            shift(path, keep);
            let mut src = std::fs::File::open(path)?;
            src.seek(SeekFrom::Start(len.saturating_sub(max_bytes)))?;
            let mut dst = std::fs::File::create(numbered(path, 1))?;
            std::io::copy(&mut src.take(max_bytes), &mut dst)?;
        }
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)?
            .set_len(0)?;
        Ok(true)
    }

    /// Keeps only the last `keep_tail` bytes of `path` when it is larger
    /// than `max_bytes` (a stopped VM's console log).
    pub fn trim_tail(path: &Path, max_bytes: u64, keep_tail: u64) -> std::io::Result<bool> {
        let len = match std::fs::metadata(path) {
            Ok(m) => m.len(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e),
        };
        if len <= max_bytes {
            return Ok(false);
        }
        let mut src = std::fs::File::open(path)?;
        src.seek(SeekFrom::Start(len.saturating_sub(keep_tail)))?;
        let mut tail = Vec::new();
        src.take(keep_tail).read_to_end(&mut tail)?;
        let tmp = numbered(path, 0);
        std::fs::write(&tmp, &tail)?;
        std::fs::rename(tmp, path)?;
        Ok(true)
    }

    /// An append-only log file that rotates itself at `max_bytes`, keeping
    /// `keep` older files (`daemon.log`, `daemon.log.1`, ...).
    pub struct RotatingFile {
        path: PathBuf,
        max_bytes: u64,
        keep: usize,
        file: Option<std::fs::File>,
        written: u64,
    }

    impl RotatingFile {
        /// Opens (appends to) `path`, rotating first when it is already
        /// over the cap.
        pub fn open(
            path: impl Into<PathBuf>,
            max_bytes: u64,
            keep: usize,
        ) -> std::io::Result<Self> {
            let path = path.into();
            if let Some(d) = path.parent() {
                std::fs::create_dir_all(d)?;
            }
            rotate(&path, max_bytes, keep)?;
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)?;
            let written = file.metadata().map(|m| m.len()).unwrap_or(0);
            Ok(Self {
                path,
                max_bytes,
                keep,
                file: Some(file),
                written,
            })
        }

        /// The current file.
        pub fn path(&self) -> &Path {
            &self.path
        }

        fn roll(&mut self) -> std::io::Result<()> {
            self.file = None;
            if self.keep == 0 {
                let _ = std::fs::remove_file(&self.path);
            } else {
                shift(&self.path, self.keep);
                let _ = std::fs::rename(&self.path, numbered(&self.path, 1));
            }
            self.file = Some(
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&self.path)?,
            );
            self.written = 0;
            Ok(())
        }
    }

    impl Write for RotatingFile {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if self.written > 0 && self.written.saturating_add(buf.len() as u64) > self.max_bytes {
                self.roll()?;
            }
            let f = match self.file.as_mut() {
                Some(f) => f,
                None => {
                    self.roll()?;
                    self.file.as_mut().expect("opened by roll")
                }
            };
            let n = f.write(buf)?;
            self.written = self.written.saturating_add(n as u64);
            Ok(n)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            match self.file.as_mut() {
                Some(f) => f.flush(),
                None => Ok(()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    struct Fake(u64);
    impl SpaceProbe for Fake {
        fn space(&self, _: &Path) -> std::io::Result<Space> {
            Ok(Space {
                available: self.0,
                total: 1000 * GIB,
            })
        }
    }

    #[test]
    fn sizes_parse_and_format() {
        assert_eq!(parse_size("20G"), Some(20 * GIB));
        assert_eq!(parse_size("20GiB"), Some(20 * GIB));
        assert_eq!(parse_size(" 1.5 t "), Some(3 * (1 << 39)));
        assert_eq!(parse_size("512m"), Some(512 << 20));
        assert_eq!(parse_size("17"), Some(17));
        assert_eq!(parse_size("x"), None);
        assert_eq!(parse_size("5Q"), None);
        assert_eq!(format_size(17), "17 B");
        assert_eq!(format_size(3 * GIB / 2), "1.5 GiB");
        assert_eq!(format_size(300 * GIB), "300 GiB");
    }

    #[test]
    fn guard_fails_below_the_floor_and_passes_above() {
        let p = Path::new("/nonexistent/cua/images");
        let e =
            ensure_space_in(&Fake(6 * GIB), p, 2 * GIB, "pull x", 5 * GIB, 20 * GIB).unwrap_err();
        assert_eq!(
            (e.needed, e.available, e.min_free),
            (2 * GIB, 6 * GIB, 5 * GIB)
        );
        assert_eq!(e.what, "pull x");
        // Enough room (a warning only, under 20 GiB after the write).
        ensure_space_in(&Fake(8 * GIB), p, 2 * GIB, "pull x", 5 * GIB, 20 * GIB).unwrap();
        ensure_space_in(&Fake(100 * GIB), p, 2 * GIB, "pull x", 5 * GIB, 20 * GIB).unwrap();
        // An unknown free space never blocks.
        struct Broken;
        impl SpaceProbe for Broken {
            fn space(&self, _: &Path) -> std::io::Result<Space> {
                Err(std::io::Error::other("no statvfs"))
            }
        }
        ensure_space_in(&Broken, p, u64::MAX, "pull x", 5 * GIB, 20 * GIB).unwrap();
        // The real probe answers for a path that does not exist yet.
        assert!(SystemProbe.space(p).unwrap().total > 0);
    }

    #[test]
    fn insufficient_disk_message_names_the_fix() {
        let e = InsufficientDisk {
            what: "pull ghcr.io/trycua/linux:latest".into(),
            path: "/Users/a/.cua/images".into(),
            needed: 3 * GIB,
            available: 4 * GIB,
            min_free: 5 * GIB,
        };
        let m = e.to_string();
        assert!(m.contains("cua cache prune"), "{m}");
        assert!(m.contains("3.0 GiB") && m.contains("4.0 GiB"), "{m}");
    }

    #[test]
    fn budgets_resolve() {
        assert_eq!(Budget::parse("auto"), Some(Budget::Auto));
        assert_eq!(Budget::parse("off"), Some(Budget::Off));
        assert_eq!(Budget::parse("50G"), Some(Budget::Bytes(50 * GIB)));
        // Large disks: capped at 30 GiB.
        assert_eq!(Budget::Auto.resolve(1000 * GIB, 0), Some(30 * GIB));
        // Small disks: a tenth of free + cache.
        assert_eq!(Budget::Auto.resolve(80 * GIB, 20 * GIB), Some(10 * GIB));
        assert_eq!(Budget::Off.resolve(1, 1), None);
    }

    #[test]
    fn config_env_overrides_file() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join(CONFIG_FILE);
        CacheConfigFile {
            budget: Some("40G".into()),
            min_free: Some("1G".into()),
            orphan_reap_minutes: Some(3),
            ..Default::default()
        }
        .write(&f)
        .unwrap();
        let c = CacheConfig::load_from(&f, |_| None);
        assert_eq!(c.budget, Budget::Bytes(40 * GIB));
        assert_eq!(c.min_free, GIB);
        assert_eq!(c.warn_free, DEFAULT_WARN_FREE);
        assert_eq!(c.orphan_reap_after, Duration::from_secs(180));
        let c = CacheConfig::load_from(&f, |k| match k {
            ENV_BUDGET => Some("off".into()),
            ENV_AUTO_GC => Some("0".into()),
            _ => None,
        });
        assert_eq!(c.budget, Budget::Off);
        assert!(!c.auto_gc);
    }

    #[test]
    fn allocated_size_counts_trees_and_markers_track_use() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("a/b")).unwrap();
        std::fs::write(d.path().join("a/b/f"), vec![1u8; 64 << 10]).unwrap();
        assert!(allocated_size(d.path()) >= 64 << 10);
        assert_eq!(allocated_size(&d.path().join("missing")), 0);
        let t = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        mark_used_at(&d.path().join("a"), t);
        assert_eq!(last_used(&d.path().join("a")), Some(t));
        mark_used_at(&d.path().join("a/b/f"), t);
        assert_eq!(last_used(&d.path().join("a/b/f")), Some(t));
    }

    #[test]
    fn logs_rotate_with_caps() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("x.log");
        let mut w = logs::RotatingFile::open(&p, 100, 2).unwrap();
        for _ in 0..50 {
            w.write_all(&[b'a'; 30]).unwrap();
        }
        w.flush().unwrap();
        assert!(std::fs::metadata(&p).unwrap().len() <= 100);
        assert_eq!(logs::rotated(&p, 2).len(), 2);
        assert!(!d.path().join("x.log.3").exists());

        let q = d.path().join("held.log");
        std::fs::write(&q, vec![b'b'; 500]).unwrap();
        assert!(logs::copy_truncate(&q, 100, 1).unwrap());
        assert_eq!(std::fs::metadata(&q).unwrap().len(), 0);
        assert_eq!(
            std::fs::metadata(d.path().join("held.log.1"))
                .unwrap()
                .len(),
            100
        );
        assert!(!logs::copy_truncate(&q, 100, 1).unwrap());

        let s = d.path().join("serial.log");
        std::fs::write(&s, vec![b'c'; 500]).unwrap();
        assert!(logs::trim_tail(&s, 100, 40).unwrap());
        assert_eq!(std::fs::metadata(&s).unwrap().len(), 40);
    }
}
