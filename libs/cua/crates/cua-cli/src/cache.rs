//! `cua cache ls|du|prune|config`: what the SDK keeps on disk and its
//! cleanup (`cua-disk`).

use crate::util::{self, line};
use clap::Subcommand;
use cua_disk::{Budget, CacheConfig, Category, GcOptions, Layout, Scanner, format_size};
use cua_sdk::CuaError;
use cua_vmm::disk::CacheConfigFile;
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Subcommand, Debug, Clone)]
pub enum CacheCmd {
    /// List what the SDK keeps on disk, one row per image, sandbox, build
    /// output, install archive or log.
    #[command(
        visible_alias = "list",
        after_help = "Examples:
  cua cache ls
  # Only cached images
  cua cache ls --category images"
    )]
    Ls {
        /// Only this category (images, docker-images, lume-bases, builds,
        /// installables, icons, sandboxes, checkpoints, logs, data, legacy).
        #[arg(long)]
        category: Option<String>,
    },
    /// Disk usage per category, the cache budget and free space.
    #[command(after_help = "Examples:
  cua cache du
  cua cache du --json")]
    Du,
    /// Reap orphaned ephemeral sandboxes, remove orphans (abandoned
    /// downloads, unused layers) and evict least-recently-used cache down
    /// to the budget. Never removes a sandbox or anything one uses.
    #[command(after_help = "Examples:
  # See what a cleanup would remove
  cua cache prune --dry-run
  # Evict every unused cache entry
  cua cache prune --all")]
    Prune {
        /// Evict every unused cache entry, not only down to the budget.
        #[arg(long)]
        all: bool,
        /// Show what would be removed; remove nothing.
        #[arg(long)]
        dry_run: bool,
        /// Budget for this run (`auto`, `off` or a size such as `20G`).
        #[arg(long)]
        budget: Option<String>,
    },
    /// Show or set the cache settings (`$CUA_HOME/cache.json`; the
    /// CUA_CACHE_BUDGET, CUA_DISK_MIN_FREE, CUA_DISK_WARN_FREE and
    /// CUA_ORPHAN_REAP_MINUTES environment variables win).
    #[command(after_help = "Examples:
  # Show the settings
  cua cache config
  # Cap the cache at 20 GB and keep 10 GB free
  cua cache config --budget 20G --min-free 10G")]
    Config {
        /// Cache budget: `auto` (min of 30 GiB and 10% of free space plus
        /// cache), `off`, or a size.
        #[arg(long)]
        budget: Option<String>,
        /// Free space pulls, builds and VM creates always leave (size).
        #[arg(long)]
        min_free: Option<String>,
        /// Warn when free space drops under this (size).
        #[arg(long)]
        warn_free: Option<String>,
        /// Minutes before an ephemeral sandbox whose process died is reaped.
        #[arg(long)]
        orphan_reap_minutes: Option<u64>,
        /// Clean up automatically after pulls and builds and on daemon idle.
        #[arg(long)]
        auto_gc: Option<bool>,
    },
}

fn ago(t: Option<u64>) -> String {
    let Some(t) = t else { return "-".into() };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let s = now.saturating_sub(t);
    match s {
        0..60 => "just now".into(),
        60..3600 => format!("{}m ago", s / 60),
        3600..86_400 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86_400),
    }
}

fn category(name: &str) -> Result<Category, CuaError> {
    Category::ALL
        .into_iter()
        .find(|c| c.as_str() == name)
        .ok_or_else(|| {
            CuaError::InvalidArgument(format!(
                "unknown category {name:?} (one of: {})",
                Category::ALL.map(|c| c.as_str()).join(", ")
            ))
        })
}

fn state_store(state_dir: Option<&str>) -> cua_sandbox_core::StateStore {
    match state_dir {
        Some(d) => cua_sandbox_core::StateStore::new(PathBuf::from(d)),
        None => cua_sandbox_core::StateStore::default(),
    }
}

/// Reaps ephemeral sandboxes whose process died (leases only; cheap). Runs
/// before sandbox and cache commands, so a crashed script's sandboxes do
/// not outlive the next `cua` run. Never fails the command.
pub async fn quick_reap(state_dir: Option<&str>) {
    let state = state_store(state_dir);
    let config = CacheConfig::load();
    let now = SystemTime::now();
    // Only build the runtimes when some lease is actually stale.
    let stale = state.leases().into_iter().any(|l| {
        l.pid != std::process::id()
            && !cua_vmm::host::pid_alive(l.pid)
            && now
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
                .saturating_sub(l.created_at)
                >= config.orphan_reap_after.as_secs()
    });
    if !stale {
        return;
    }
    let local = cua_daemon::local::VmmLocal::default();
    let reaped = cua_daemon::maintenance::reap_leases(
        &local,
        &state,
        config.orphan_reap_after,
        now,
        false,
        &cua_vmm::host::pid_alive,
    )
    .await;
    for r in reaped {
        match r.error {
            None => eprintln!("cua: removed orphaned sandbox {} ({})", r.name, r.reason),
            Some(e) => eprintln!("cua: could not remove orphaned sandbox {}: {e}", r.name),
        }
    }
}

pub async fn run(
    cmd: CacheCmd,
    state_dir: Option<&str>,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let layout = Layout::default();
    match cmd {
        CacheCmd::Du => {
            let r = Scanner::system(layout).await.scan().await;
            if json {
                util::json_line(
                    out,
                    &serde_json::json!({
                        "home": r.home,
                        "categories": r.totals(),
                        "cache_bytes": r.cache_bytes,
                        "budget": r.budget,
                        "budget_bytes": r.budget_bytes,
                        "total_bytes": r.total_bytes(),
                        "space": r.space,
                        "notes": r.notes,
                    }),
                );
                return Ok(0);
            }
            let rows: Vec<Vec<String>> = r
                .totals()
                .iter()
                .map(|t| {
                    vec![
                        t.category.to_string(),
                        format_size(t.bytes),
                        t.items.to_string(),
                        if t.category.is_cache() {
                            "cache"
                        } else {
                            "kept"
                        }
                        .into(),
                    ]
                })
                .collect();
            util::table(out, &["CATEGORY", "SIZE", "ITEMS", "POLICY"], &rows);
            line(out, "");
            line(out, format!("home:   {}", r.home.display()));
            line(out, format!("total:  {}", format_size(r.total_bytes())));
            line(
                out,
                format!(
                    "cache:  {} (budget {}{})",
                    format_size(r.cache_bytes),
                    r.budget,
                    r.budget_bytes
                        .map(|b| format!(" = {}", format_size(b)))
                        .unwrap_or_default()
                ),
            );
            if let Some(s) = r.space {
                line(
                    out,
                    format!(
                        "free:   {} of {}",
                        format_size(s.available),
                        format_size(s.total)
                    ),
                );
            }
            for n in &r.notes {
                line(out, format!("note:   {n}"));
            }
            Ok(0)
        }
        CacheCmd::Ls { category: only } => {
            let only = only.as_deref().map(category).transpose()?;
            let r = Scanner::system(layout).await.scan().await;
            let items: Vec<&cua_disk::Item> = r
                .items
                .iter()
                .filter(|i| only.is_none_or(|c| i.category == c))
                .collect();
            if json {
                util::json_line(out, &serde_json::to_value(&items).map_err(util::internal)?);
                return Ok(0);
            }
            if items.is_empty() {
                line(out, "Nothing cached.");
                return Ok(0);
            }
            let rows: Vec<Vec<String>> = items
                .iter()
                .map(|i| {
                    let mut notes = Vec::new();
                    if let Some(s) = &i.status {
                        notes.push(s.clone());
                    }
                    if !i.referenced_by.is_empty() {
                        notes.push(format!("used by {}", i.referenced_by.join(", ")));
                    }
                    if i.ephemeral {
                        notes.push("ephemeral".into());
                    }
                    if i.orphan {
                        notes.push("orphan".into());
                    }
                    if i.in_progress {
                        notes.push("in progress".into());
                    }
                    vec![
                        i.category.to_string(),
                        i.kind.clone(),
                        i.name.clone(),
                        format_size(i.bytes),
                        ago(i.last_used),
                        notes.join("; "),
                    ]
                })
                .collect();
            util::table(
                out,
                &["CATEGORY", "KIND", "NAME", "SIZE", "LAST USED", "NOTES"],
                &rows,
            );
            for n in &r.notes {
                line(out, format!("note: {n}"));
            }
            Ok(0)
        }
        CacheCmd::Prune {
            all,
            dry_run,
            budget,
        } => {
            let budget = budget
                .map(|b| {
                    Budget::parse(&b).ok_or_else(|| {
                        CuaError::InvalidArgument(format!(
                            "bad budget {b:?} (auto, off, or a size such as 20G)"
                        ))
                    })
                })
                .transpose()?;
            let local = cua_daemon::local::VmmLocal::default();
            let state = state_store(state_dir);
            let r = cua_daemon::maintenance::run(
                &local,
                &state,
                layout,
                Some(GcOptions {
                    all,
                    dry_run,
                    budget,
                    ..Default::default()
                }),
                dry_run,
            )
            .await;
            if json {
                util::json_line(out, &serde_json::to_value(&r).map_err(util::internal)?);
                return Ok(0);
            }
            let verb = if dry_run { "would remove" } else { "removed" };
            for x in &r.reaped {
                match &x.error {
                    None => line(out, format!("{verb} sandbox {} ({})", x.name, x.reason)),
                    Some(e) => line(out, format!("could not remove sandbox {}: {e}", x.name)),
                }
            }
            let Some(g) = r.gc else {
                return Ok(0);
            };
            if let Some(s) = &g.skipped {
                line(out, format!("skipped: {s}"));
                return Ok(0);
            }
            for x in &g.removed {
                let what = format!(
                    "{} {} ({}, {})",
                    x.item.kind,
                    x.item.name,
                    format_size(x.item.bytes),
                    x.reason
                );
                match &x.error {
                    None => line(out, format!("{verb} {what}")),
                    Some(e) => line(out, format!("could not remove {what}: {e}")),
                }
            }
            line(
                out,
                format!(
                    "{} {}; cache {} -> {}{}",
                    if dry_run { "would free" } else { "freed" },
                    format_size(g.freed),
                    format_size(g.before),
                    format_size(g.after),
                    g.budget
                        .map(|b| format!(" (budget {})", format_size(b)))
                        .unwrap_or_default()
                ),
            );
            if g.kept_referenced > 0 {
                line(
                    out,
                    format!(
                        "kept {} cache entr{} used by sandboxes (cua sb rm frees them)",
                        g.kept_referenced,
                        if g.kept_referenced == 1 { "y" } else { "ies" }
                    ),
                );
            }
            Ok(0)
        }
        CacheCmd::Config {
            budget,
            min_free,
            warn_free,
            orphan_reap_minutes,
            auto_gc,
        } => {
            let path = layout.config();
            let mut f = CacheConfigFile::read(&path);
            let changed = budget.is_some()
                || min_free.is_some()
                || warn_free.is_some()
                || orphan_reap_minutes.is_some()
                || auto_gc.is_some();
            if let Some(b) = budget {
                Budget::parse(&b).ok_or_else(|| {
                    CuaError::InvalidArgument(format!("bad budget {b:?} (auto, off or a size)"))
                })?;
                f.budget = Some(b);
            }
            for (v, slot, name) in [
                (min_free, &mut f.min_free, "min-free"),
                (warn_free, &mut f.warn_free, "warn-free"),
            ] {
                if let Some(v) = v {
                    cua_disk::parse_size(&v).ok_or_else(|| {
                        CuaError::InvalidArgument(format!("bad --{name} {v:?} (a size such as 5G)"))
                    })?;
                    *slot = Some(v);
                }
            }
            if let Some(m) = orphan_reap_minutes {
                f.orphan_reap_minutes = Some(m);
            }
            if let Some(a) = auto_gc {
                f.auto_gc = Some(a);
            }
            if changed {
                f.write(&path).map_err(util::internal)?;
            }
            let c = CacheConfig::load();
            if json {
                util::json_line(
                    out,
                    &serde_json::json!({
                        "file": path,
                        "budget": c.budget.to_string(),
                        "min_free": c.min_free,
                        "warn_free": c.warn_free,
                        "orphan_reap_minutes": c.orphan_reap_after.as_secs() / 60,
                        "auto_gc": c.auto_gc,
                    }),
                );
                return Ok(0);
            }
            line(out, format!("budget:              {}", c.budget));
            line(
                out,
                format!("min free:            {}", format_size(c.min_free)),
            );
            line(
                out,
                format!("warn free:           {}", format_size(c.warn_free)),
            );
            line(
                out,
                format!(
                    "orphan reap after:   {}",
                    humantime_minutes(c.orphan_reap_after)
                ),
            );
            line(
                out,
                format!(
                    "automatic cleanup:   {}",
                    if c.auto_gc { "on" } else { "off" }
                ),
            );
            line(out, format!("file:                {}", path.display()));
            Ok(0)
        }
    }
}

fn humantime_minutes(d: Duration) -> String {
    format!("{} min", d.as_secs() / 60)
}
