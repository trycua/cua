//! Cache budget and least-recently-used garbage collection.
//!
//! Rules, in order:
//!
//! 1. Nothing a sandbox references is removed (running, stopped or named:
//!    an overlay's backing disk, an image a container uses, a volume a
//!    container mounts). Sandboxes and checkpoints themselves are never
//!    evicted; `cua sb rm` removes them.
//! 2. Nothing another process is writing is removed (`.lock` files, fresh
//!    `.partial` downloads, a Lume base still pulling).
//! 3. Docker objects are removed only when the SDK created them
//!    (`ai.cua.managed=true`, the `cua-vmm/` repositories) or pulled them
//!    (the pull ledger); an image is removed by the SDK's own reference and
//!    never forced.
//! 4. Orphans are always removed: abandoned partial downloads, frozen QEMU
//!    layers no entry uses, records of objects deleted elsewhere.
//! 5. Then, while the cache is over budget, the least recently used
//!    evictable entry goes first. Entries used within the grace period stay
//!    (a sandbox being created right now may be about to use them).
//!    `--all` evicts every eligible entry regardless of the budget.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::scan::{Item, Report, Scanner, Target};
use cua_vmm::disk::{Budget, CacheConfig, format_size};

/// Grace period of automatic collections.
pub const AUTO_GRACE: Duration = Duration::from_secs(10 * 60);
/// Grace period of `cua cache prune`.
pub const MANUAL_GRACE: Duration = Duration::from_secs(2 * 60);
/// Minimum spacing of automatic collections.
pub const AUTO_MIN_INTERVAL: Duration = Duration::from_secs(30);
/// A GC lock older than this is broken.
const STALE_LOCK: Duration = Duration::from_secs(3600);

/// Options of one collection.
#[derive(Clone, Copy, Debug)]
pub struct GcOptions {
    /// Evict every eligible entry, not just enough to meet the budget.
    pub all: bool,
    /// Report only.
    pub dry_run: bool,
    /// Budget override (default: the configured one).
    pub budget: Option<Budget>,
    /// Entries used more recently than this stay.
    pub grace: Duration,
    /// The clock.
    pub now: SystemTime,
}

impl Default for GcOptions {
    fn default() -> Self {
        Self {
            all: false,
            dry_run: false,
            budget: None,
            grace: MANUAL_GRACE,
            now: SystemTime::now(),
        }
    }
}

/// One removal.
#[derive(Clone, Debug, Serialize)]
pub struct Removed {
    /// What.
    pub item: Item,
    /// Why (`orphan`, `budget`, `all`).
    pub reason: String,
    /// The removal failed (the entry stays).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// What a collection did.
#[derive(Clone, Debug, Serialize)]
pub struct GcReport {
    /// Cache bytes before.
    pub before: u64,
    /// Cache bytes after (planned, for a dry run).
    pub after: u64,
    /// Bytes freed (planned, for a dry run), orphans included.
    pub freed: u64,
    /// Budget in bytes (`None`: no limit).
    pub budget: Option<u64>,
    /// Removals.
    pub removed: Vec<Removed>,
    /// Cache entries kept because a sandbox uses them.
    pub kept_referenced: usize,
    /// Report only.
    pub dry_run: bool,
    /// Why nothing ran (lock held, automatic GC off).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
}

impl GcReport {
    fn skipped(why: impl Into<String>) -> Self {
        Self {
            before: 0,
            after: 0,
            freed: 0,
            budget: None,
            removed: vec![],
            kept_referenced: 0,
            dry_run: false,
            skipped: Some(why.into()),
        }
    }
}

fn unix(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Whether GC may evict `item` at `now` (cache, unreferenced, idle for the
/// grace period, not being written).
pub fn eligible(item: &Item, now: SystemTime, grace: Duration) -> bool {
    let idle = match item.last_used {
        Some(t) => unix(now).saturating_sub(t) >= grace.as_secs(),
        None => true,
    };
    item.evictable
        && item.category.is_cache()
        && item.referenced_by.is_empty()
        && !item.in_progress
        && item.target != Target::None
        && idle
}

/// The removals for `report`: indices into `report.items` with a reason.
/// Pure: the clock and the budget come in, nothing is touched.
pub fn plan(
    report: &Report,
    budget: Option<u64>,
    all: bool,
    now: SystemTime,
    grace: Duration,
) -> Vec<(usize, &'static str)> {
    let mut out: Vec<(usize, &'static str)> = report
        .items
        .iter()
        .enumerate()
        .filter(|(_, i)| i.orphan && i.referenced_by.is_empty() && i.target != Target::None)
        .map(|(n, _)| (n, "orphan"))
        .collect();
    let mut cands: Vec<(usize, &Item)> = report
        .items
        .iter()
        .enumerate()
        .filter(|(_, i)| !i.orphan && eligible(i, now, grace))
        .collect();
    // Least recently used first; unknown use counts as oldest; ties by
    // size (larger first frees more).
    cands.sort_by(|(_, a), (_, b)| {
        a.last_used
            .unwrap_or(0)
            .cmp(&b.last_used.unwrap_or(0))
            .then(b.bytes.cmp(&a.bytes))
    });
    if all {
        out.extend(cands.iter().map(|(n, _)| (*n, "all")));
        return out;
    }
    let Some(budget) = budget else {
        return out;
    };
    let mut total = report.cache_bytes;
    for (n, i) in cands {
        if total <= budget {
            break;
        }
        total = total.saturating_sub(i.bytes);
        out.push((n, "budget"));
    }
    out
}

struct Lock(std::path::PathBuf);

impl Lock {
    fn take(path: &std::path::Path) -> Option<Self> {
        if let Some(d) = path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        for _ in 0..2 {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
            {
                Ok(_) => return Some(Self(path.to_path_buf())),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let stale = std::fs::metadata(path)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.elapsed().ok())
                        .is_some_and(|a| a > STALE_LOCK);
                    if !stale {
                        return None;
                    }
                    let _ = std::fs::remove_file(path);
                }
                Err(_) => return None,
            }
        }
        None
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

async fn remove(scanner: &Scanner, item: &Item) -> Result<(), String> {
    match &item.target {
        Target::None => Err("not removable".into()),
        Target::Path(p) | Target::Record(p) => {
            let r = if p.is_dir() {
                std::fs::remove_dir_all(p)
            } else {
                std::fs::remove_file(p)
            };
            match r {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.to_string()),
            }
            // A removed image directory: drop the refs that named it.
            if item.kind == "containerdisk" || item.kind == "rootfs" {
                drop_refs(scanner, p);
            }
            Ok(())
        }
        Target::DockerImage(r) => {
            let d = scanner.docker().ok_or("container engine not reachable")?;
            d.remove_image(r).await?;
            if item.kind == "pulled-image" {
                cua_vmm::container::ledger::PullLedger::new(scanner.layout().docker_pulls())
                    .forget(r);
            }
            Ok(())
        }
        Target::DockerVolume(v) => {
            let d = scanner.docker().ok_or("container engine not reachable")?;
            d.remove_volume(v).await
        }
        Target::LumeVm(name) => {
            let l = scanner.lume().ok_or("lume serve not running")?;
            l.delete(name).await?;
            cua_vmm::lume::OwnedVms::new(scanner.layout().lume_owned()).forget(name);
            let dir = scanner.layout().lume().join(name);
            if dir.exists() {
                let _ = std::fs::remove_dir_all(dir);
            }
            Ok(())
        }
    }
}

fn drop_refs(scanner: &Scanner, dir: &std::path::Path) {
    let Some(hex) = dir.file_name().map(|n| n.to_string_lossy().into_owned()) else {
        return;
    };
    let refs = scanner.layout().images().join("refs");
    let Ok(rd) = std::fs::read_dir(&refs) else {
        return;
    };
    for e in rd.flatten() {
        let hit = std::fs::read(e.path())
            .ok()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
            .and_then(|v| v.get("digest")?.as_str().map(|d| d.ends_with(&hex)))
            .unwrap_or(false);
        if hit {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Scans, plans and (unless `dry_run`) removes. Takes the GC lock; when
/// another process holds it, returns a skipped report.
pub async fn collect(scanner: &Scanner, opts: GcOptions) -> GcReport {
    let _lock = if opts.dry_run {
        None
    } else {
        match Lock::take(&scanner.layout().gc_lock()) {
            Some(l) => Some(l),
            None => return GcReport::skipped("another cua cache cleanup is running"),
        }
    };
    let mut report = scanner.scan_at(opts.now).await;
    if let Some(b) = opts.budget {
        report.budget_bytes = b.resolve(
            report.space.map(|s| s.available).unwrap_or(u64::MAX / 4),
            report.cache_bytes,
        );
    }
    let planned = plan(&report, report.budget_bytes, opts.all, opts.now, opts.grace);
    let kept_referenced = report
        .items
        .iter()
        .filter(|i| i.category.is_cache() && i.evictable && !i.referenced_by.is_empty())
        .count();
    let mut removed = Vec::new();
    let mut freed = 0u64;
    let mut cache_freed = 0u64;
    for (n, reason) in planned {
        let item = report.items[n].clone();
        let error = if opts.dry_run {
            None
        } else {
            remove(scanner, &item).await.err()
        };
        if error.is_none() {
            freed = freed.saturating_add(item.bytes);
            if item.category.is_cache() {
                cache_freed = cache_freed.saturating_add(item.bytes);
            }
        } else {
            tracing::warn!(item = %item.name, error = ?error, "cache cleanup could not remove an entry");
        }
        removed.push(Removed {
            item,
            reason: reason.into(),
            error,
        });
    }
    if !opts.dry_run && freed > 0 {
        tracing::info!(freed = %format_size(freed), "cua cache cleanup");
    }
    GcReport {
        before: report.cache_bytes,
        after: report.cache_bytes.saturating_sub(cache_freed),
        freed,
        budget: report.budget_bytes,
        removed,
        kept_referenced,
        dry_run: opts.dry_run,
        skipped: None,
    }
}

/// The automatic collection after pulls and builds and on daemon idle:
/// budget eviction with the long grace period, at most every 30 s, unless
/// `auto_gc` is off or the budget is `off`. Never fails the caller.
pub async fn auto_gc(reason: &str) -> Option<GcReport> {
    let config = CacheConfig::load();
    if !config.auto_gc {
        return None;
    }
    let layout = crate::Layout::default();
    let stamp = layout.gc_stamp();
    let recent = std::fs::metadata(&stamp)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|a| a < AUTO_MIN_INTERVAL);
    if recent {
        return None;
    }
    let _ = std::fs::create_dir_all(layout.home());
    let _ = std::fs::write(&stamp, reason.as_bytes());
    let scanner = Scanner::system(layout).await;
    let budget_off = config.budget == Budget::Off;
    let r = collect(
        &scanner,
        GcOptions {
            all: false,
            dry_run: false,
            // Orphans still go when the budget is off.
            budget: budget_off.then_some(Budget::Off),
            grace: AUTO_GRACE,
            now: SystemTime::now(),
        },
    )
    .await;
    if r.freed > 0 {
        tracing::info!(reason, freed = %format_size(r.freed), removed = r.removed.len(), "automatic cache cleanup");
    }
    Some(r)
}
