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
//! 6. Automatic and budget eviction follow [`BASE_POLICY`] for base images
//!    (a macOS base is ~25-30 GB, often more than the whole automatic
//!    budget): see [`BasePolicy`]. `--all` is explicit and removes every
//!    base no sandbox uses.

use std::collections::HashMap;
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
/// How automatic and budget eviction (the daemon, the cleanup after pulls
/// and builds, `cua cache prune` without `--all`) treat base images. An
/// explicit `cua cache prune --all` ignores it (it still never removes a
/// base a sandbox uses).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BasePolicy {
    /// Never evict a base that is in use: one a Space or VM was cloned from
    /// ([`Item::referenced_by`]) or the current pinned macOS or Linux base of
    /// this release. Also keep a Lume base pulled after every base in use (the
    /// base of a create that failed or was interrupted) until a create from
    /// a newer base succeeds, so a retry clones from cache instead of
    /// pulling ~25 GB again. Older, unused bases still go.
    // Open question: this or a 30-day expiry (an
    // `ExpireAfter(Duration)` variant letting a base unused for 30 days go
    // even when pinned or kept for a retry). Changing `BASE_POLICY` is the switch.
    NeverAutoEvictInUse,
}

/// The policy in force.
pub const BASE_POLICY: BasePolicy = BasePolicy::NeverAutoEvictInUse;

/// Why budget eviction kept a base image.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "reason", rename_all = "kebab-case")]
pub enum KeepReason {
    /// A Space or VM was cloned from it (their names).
    InUse {
        /// Sandbox names.
        by: Vec<String>,
    },
    /// The current pinned base of this release.
    CurrentPin {
        /// The pinned reference.
        reference: String,
    },
    /// A Lume base pulled after every base a sandbox uses: a create from it
    /// failed or was interrupted, and no later create from a newer base has
    /// succeeded.
    NoNewerBaseInUse,
}

impl std::fmt::Display for KeepReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeepReason::InUse { by } => write!(f, "in use by {}", by.join(", ")),
            KeepReason::CurrentPin { reference } => write!(f, "current pinned base {reference}"),
            KeepReason::NoNewerBaseInUse => {
                f.write_str("kept for a retry: no newer base is in use")
            }
        }
    }
}

/// One base image budget eviction kept.
#[derive(Clone, Debug, Serialize)]
pub struct KeptBase {
    /// The base.
    pub name: String,
    /// Kind (`lume-base`, `containerdisk`, `rootfs`, `pulled-image`).
    pub kind: String,
    /// Bytes.
    pub bytes: u64,
    /// Why.
    #[serde(flatten)]
    pub why: KeepReason,
}

/// A pinned base reference of this release and the digest the catalog
/// verified it at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pin {
    /// Repository and tag (`ghcr.io/trycua/macos:26`).
    pub reference: String,
    /// `sha256:..`, when the catalog records one.
    pub digest: Option<String>,
}

/// The current pinned macOS and Linux bases: the canonical default of each
/// OS (the `CUA_IMAGE_<OS>` override replaces it) and its `-disk` variant,
/// at the catalog's digest.
pub fn current_pins() -> Vec<Pin> {
    use cua_image::canonical::{CanonicalOs, canonical_for};
    let mut out = Vec::new();
    for os in [CanonicalOs::Macos, CanonicalOs::Linux] {
        let base = canonical_for(os, None);
        for r in [base.clone(), format!("{base}-disk")] {
            let digest = cua_image::catalog::find(&r).and_then(|e| e.digest.clone());
            if r == base || digest.is_some() {
                out.push(Pin {
                    reference: r,
                    digest,
                });
            }
        }
    }
    out
}

/// The pin `reference` (`repo:tag`, `repo:tag@sha256:..`, `repo@sha256:..`)
/// names: the same digest, else the same tag when either has no digest.
fn pinned_by<'a>(reference: &str, pins: &'a [Pin]) -> Option<&'a Pin> {
    let (name, digest) = match reference.split_once('@') {
        Some((n, d)) => (n, Some(d)),
        None => (reference, None),
    };
    pins.iter().find(|p| match (digest, p.digest.as_deref()) {
        (Some(d), Some(pd)) => d == pd,
        _ => name == p.reference,
    })
}

/// Kinds that are base images (what sandboxes are created from).
fn is_base(kind: &str) -> bool {
    matches!(
        kind,
        "lume-base" | "containerdisk" | "rootfs" | "pulled-image"
    )
}

/// The base images [`BASE_POLICY`] keeps from automatic and budget
/// eviction (indices into `report.items`), with why. Pure.
pub fn kept_bases(report: &Report, pins: &[Pin]) -> HashMap<usize, KeepReason> {
    let BasePolicy::NeverAutoEvictInUse = BASE_POLICY;
    let mut out = HashMap::new();
    for (n, i) in report.items.iter().enumerate() {
        if i.orphan || !i.category.is_cache() || !is_base(&i.kind) {
            continue;
        }
        if !i.referenced_by.is_empty() {
            out.insert(
                n,
                KeepReason::InUse {
                    by: i.referenced_by.clone(),
                },
            );
            continue;
        }
        // A containerDisk's name lists every reference that names it.
        let refs = std::iter::once(i.location.as_str()).chain(i.name.split(", "));
        if let Some(p) = refs.filter_map(|r| pinned_by(r, pins)).next() {
            out.insert(
                n,
                KeepReason::CurrentPin {
                    reference: p.reference.clone(),
                },
            );
        }
    }
    // A create that failed or was interrupted leaves its base unused: keep
    // every Lume base pulled after the newest one a sandbox uses (all of
    // them while none is in use), so the retry clones from cache. A later
    // create that succeeds from a newer base releases the older ones.
    let in_use_since = report
        .items
        .iter()
        .filter(|i| i.kind == "lume-base" && !i.orphan && !i.referenced_by.is_empty())
        .filter_map(|i| i.created)
        .max();
    for (n, i) in report.items.iter().enumerate() {
        let newer = match (i.created, in_use_since) {
            (_, None) => true,
            (Some(c), Some(t)) => c > t,
            (None, Some(_)) => false,
        };
        if i.kind == "lume-base" && !i.orphan && newer {
            out.entry(n).or_insert(KeepReason::NoNewerBaseInUse);
        }
    }
    out
}

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
    /// Base images kept over budget by [`BASE_POLICY`], with why.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub kept_bases: Vec<KeptBase>,
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
            kept_bases: vec![],
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

/// What one collection would do.
#[derive(Clone, Debug, Default)]
pub struct Plan {
    /// Removals: indices into `report.items` with a reason.
    pub remove: Vec<(usize, &'static str)>,
    /// Base images [`BASE_POLICY`] kept while the cache was over budget,
    /// with why (empty when under budget or for `--all`).
    pub kept: Vec<(usize, KeepReason)>,
}

/// The removals for `report`: indices into `report.items` with a reason.
/// Pure: the clock and the budget come in, nothing is touched. Uses this
/// release's [`current_pins`].
pub fn plan(
    report: &Report,
    budget: Option<u64>,
    all: bool,
    now: SystemTime,
    grace: Duration,
) -> Vec<(usize, &'static str)> {
    plan_with(report, budget, all, now, grace, &current_pins()).remove
}

/// [`plan`] with explicit pins, and the bases it kept.
pub fn plan_with(
    report: &Report,
    budget: Option<u64>,
    all: bool,
    now: SystemTime,
    grace: Duration,
    pins: &[Pin],
) -> Plan {
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
        return Plan {
            remove: out,
            kept: vec![],
        };
    }
    let Some(budget) = budget else {
        return Plan {
            remove: out,
            kept: vec![],
        };
    };
    let mut keep = kept_bases(report, pins);
    let mut total = report.cache_bytes;
    let over = total > budget;
    for (n, i) in cands.iter().filter(|(n, _)| !keep.contains_key(n)) {
        if total <= budget {
            break;
        }
        total = total.saturating_sub(i.bytes);
        out.push((*n, "budget"));
    }
    // Report what the policy kept: bases eviction would otherwise have
    // reached (eligible) and bases a sandbox uses.
    let mut kept: Vec<(usize, KeepReason)> = if over {
        let reached = |n: &usize| {
            cands.iter().any(|(c, _)| c == n) || !report.items[*n].referenced_by.is_empty()
        };
        keep.drain().filter(|(n, _)| reached(n)).collect()
    } else {
        vec![]
    };
    kept.sort_by_key(|(n, _)| *n);
    Plan { remove: out, kept }
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
    let planned = plan_with(
        &report,
        report.budget_bytes,
        opts.all,
        opts.now,
        opts.grace,
        &current_pins(),
    );
    let kept_bases: Vec<KeptBase> = planned
        .kept
        .iter()
        .map(|(n, why)| {
            let i = &report.items[*n];
            KeptBase {
                name: i.name.clone(),
                kind: i.kind.clone(),
                bytes: i.bytes,
                why: why.clone(),
            }
        })
        .collect();
    if !opts.dry_run && !kept_bases.is_empty() {
        // One line per run: which bases stayed over budget and why.
        let kept = kept_bases
            .iter()
            .map(|k| format!("{} ({}, {})", k.name, format_size(k.bytes), k.why))
            .collect::<Vec<_>>()
            .join("; ");
        tracing::info!(policy = ?BASE_POLICY, kept, "cache cleanup kept base images over budget");
    }
    let planned = planned.remove;
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
            if !opts.dry_run {
                tracing::info!(
                    item = %item.name,
                    kind = %item.kind,
                    location = %item.location,
                    reason,
                    bytes = %format_size(item.bytes),
                    "cache cleanup removed"
                );
            }
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
        kept_bases,
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

#[cfg(test)]
mod tests {
    use super::{Pin, pinned_by};

    #[test]
    fn a_reference_names_a_pin_by_digest_or_by_tag() {
        let pins = [
            Pin {
                reference: "ghcr.io/trycua/macos:26".into(),
                digest: Some("sha256:d6".into()),
            },
            Pin {
                reference: "ghcr.io/trycua/linux:24.04".into(),
                digest: None,
            },
        ];
        let hit = |r: &str| pinned_by(r, &pins).map(|p| p.reference.as_str());
        assert_eq!(
            hit("ghcr.io/trycua/macos:26"),
            Some("ghcr.io/trycua/macos:26")
        );
        assert_eq!(
            hit("ghcr.io/trycua/macos:26@sha256:d6"),
            Some("ghcr.io/trycua/macos:26")
        );
        assert_eq!(
            hit("ghcr.io/trycua/macos@sha256:d6"),
            Some("ghcr.io/trycua/macos:26")
        );
        // An older digest of the same tag is superseded.
        assert_eq!(hit("ghcr.io/trycua/macos:26@sha256:00"), None);
        assert_eq!(hit("ghcr.io/trycua/macos:15"), None);
        assert_eq!(
            hit("ghcr.io/trycua/linux:24.04@sha256:ab"),
            Some("ghcr.io/trycua/linux:24.04")
        );
    }

    #[test]
    fn the_current_pins_are_the_macos_and_linux_defaults() {
        let pins = super::current_pins();
        let refs: Vec<&str> = pins.iter().map(|p| p.reference.as_str()).collect();
        assert!(refs.iter().any(|r| r.contains("macos")), "{refs:?}");
        assert!(refs.iter().any(|r| r.contains("linux")), "{refs:?}");
    }
}
