//! `cua storage`: where Cua keeps its files, what each folder holds, and
//! moving the large folders (computers' disks, images, caches) elsewhere.
//! The Settings, Storage page runs these commands.

use crate::util::{self, hint_line, line};
use clap::Subcommand;
use cua_home::storage::{self, Kind, Source};
use cua_sdk::CuaError;
use serde_json::json;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Subcommand, Debug, Clone)]
pub enum StorageCmd {
    /// Show every folder Cua uses, where it comes from and how big it is.
    #[command(
        visible_alias = "ls",
        after_help = "Examples:
  cua storage
  cua storage --json"
    )]
    Show,
    /// Check that a folder can hold a kind of data, without changing anything.
    #[command(after_help = "Examples:
  cua storage check runtimes /Volumes/Fast/cua-runtimes")]
    Check {
        /// runtimes, images, cache, logs or trajectories.
        kind: String,
        /// The new folder (a full path).
        path: PathBuf,
    },
    /// Use another folder for a kind of data. With --move the existing data is
    /// copied there first; the old copy stays until `cua storage remove`.
    #[command(after_help = "Examples:
  # Keep computers' disks on an external drive, copying what exists
  cua storage set runtimes /Volumes/Fast/cua-runtimes --move
  # Only point at a folder that already holds the data
  cua storage set images /Volumes/Fast/cua-images")]
    Set {
        /// runtimes, images, cache, logs or trajectories.
        kind: String,
        /// The new folder (a full path).
        path: PathBuf,
        /// Copy the existing data to the new folder first.
        #[arg(long = "move")]
        move_data: bool,
    },
    /// Go back to the default folder for a kind. Nothing is copied or deleted.
    #[command(after_help = "Examples:
  cua storage reset runtimes")]
    Reset {
        /// runtimes, images, cache, logs or trajectories.
        kind: String,
    },
    /// Delete one computer image or runtime from `runtimes` (see `cua storage`),
    /// or an old folder left by `cua storage set --move`. Asks nothing: the
    /// Settings page confirms first.
    #[command(after_help = "Examples:
  # Delete the old copy after `cua storage set runtimes <path> --move`
  cua storage remove ~/.cua/runtimes")]
    Remove {
        /// A folder listed by `cua storage --json` (items) or a folder left
        /// behind by a move.
        path: PathBuf,
    },
}

fn kind_arg(name: &str) -> Result<Kind, CuaError> {
    Kind::parse(name).filter(|k| k.movable()).ok_or_else(|| {
        CuaError::InvalidArgument(format!(
            "{name:?} is not a folder you can change (runtimes, images, cache, logs, trajectories)"
        ))
    })
}

fn source_name(s: Source) -> &'static str {
    match s {
        Source::Default => "default",
        Source::Settings => "settings",
        Source::Env => "environment",
    }
}

/// The runtimes folder must leave room for the Linux runtime's socket paths
/// (103 bytes). `None` when the path is fine or the kind is another one.
fn runtimes_path_problem(kind: Kind, path: &Path) -> Option<String> {
    if kind != Kind::Runtimes {
        return None;
    }
    let len = cua_vmm::managed::longest_socket_path(path);
    (len > 103).then(|| {
        format!(
            "That path is too long for computers to run from it ({len} bytes; the limit is 103). Choose a shorter folder, for example /Volumes/Drive/cua."
        )
    })
}

/// The report `cua storage --json` prints (also what the app reads).
pub fn report() -> serde_json::Value {
    let home = cua_home::cua_home();
    let env = |k: &str| std::env::var(k).ok();
    let kinds: Vec<_> = Kind::ALL
        .into_iter()
        .map(|k| {
            let dir = storage::dir_in(&home, k, env);
            json!({
                "kind": k.name(),
                "path": dir,
                "source": source_name(storage::source_in(&home, k, env)),
                "movable": k.movable(),
                "env_var": k.env_var(),
                // The home itself holds config, data and keys: counted once, as "home".
                "bytes": if k.movable() { storage::disk_usage(&dir) } else { 0 },
                "free_bytes": storage::available_space(&dir),
                "owned": std::fs::read_to_string(dir.join(storage::OWNED_MARKER)).ok()
                    .map(|t| t.contains("owned=1")),
            })
        })
        .collect();
    let layout = cua_disk::Layout::default();
    let mut items = Vec::new();
    if let Ok(rd) = std::fs::read_dir(layout.runtimes()) {
        for fam in rd.flatten() {
            let fname = fam.file_name().to_string_lossy().into_owned();
            let Ok(sub) = std::fs::read_dir(fam.path()) else {
                continue;
            };
            for e in sub.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                // Lima's download cache is cache, not an image to remove by hand.
                if fname == "linux" && name == "home" {
                    continue;
                }
                let p = e.path();
                items.push(json!({
                    "family": fname,
                    "name": name,
                    "path": p,
                    "bytes": storage::disk_usage(&p),
                }));
            }
        }
    }
    let old: Vec<_> = Kind::ALL
        .into_iter()
        .filter(|k| k.movable())
        .map(|k| k.default_in(&home))
        .filter(|p| p.join(storage::MOVED_MARKER).exists() && !storage::is_in_use(&home, p))
        .map(|p| json!({"path": p, "bytes": storage::disk_usage(&p)}))
        .collect();
    let (traj_bytes, traj_age) = cua_disk::scratch::trajectory_caps();
    json!({
        "home": home,
        "home_bytes": storage::disk_usage(&home),
        // Everything Cua keeps: the home plus each relocated folder (the home alone drops
        // when a folder moves to another drive).
        "total_bytes": storage::disk_usage(&home)
            + Kind::ALL
                .into_iter()
                .filter(|k| k.movable())
                .map(|k| storage::dir_in(&home, k, env))
                .filter(|d| !d.starts_with(&home))
                .map(|d| storage::disk_usage(&d))
                .sum::<u64>(),
        "free_bytes": storage::available_space(&home),
        "kinds": kinds,
        "items": items,
        "old_locations": old,
        "daemon_running": storage::daemon_running(&home),
        "caps": {
            "trajectories_max_bytes": traj_bytes,
            "trajectories_max_days": traj_age.as_secs() / 86_400,
        },
    })
}

fn human(b: u64) -> String {
    cua_disk::format_size(b)
}

pub fn run(cmd: Option<StorageCmd>, json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    run_inner(cmd.unwrap_or(StorageCmd::Show), json, out).map(|()| 0)
}

fn run_inner(cmd: StorageCmd, json: bool, out: &mut dyn Write) -> Result<(), CuaError> {
    let home = cua_home::cua_home();
    match cmd {
        StorageCmd::Show => {
            let r = report();
            if json {
                util::json_line(out, &r);
                return Ok(());
            }
            line(out, format!("Cua home  {}", home.display()));
            for k in r["kinds"].as_array().into_iter().flatten() {
                let name = k["kind"].as_str().unwrap_or("");
                if !k["movable"].as_bool().unwrap_or(false) {
                    continue;
                }
                line(
                    out,
                    format!(
                        "{:<13} {:>9}  {}  ({})",
                        name,
                        human(k["bytes"].as_u64().unwrap_or(0)),
                        k["path"].as_str().unwrap_or(""),
                        k["source"].as_str().unwrap_or("")
                    ),
                );
            }
            for i in r["items"].as_array().into_iter().flatten() {
                line(
                    out,
                    format!(
                        "  {}/{}  {}",
                        i["family"].as_str().unwrap_or(""),
                        i["name"].as_str().unwrap_or(""),
                        human(i["bytes"].as_u64().unwrap_or(0))
                    ),
                );
            }
            hint_line(out, "Change a folder: cua storage set <kind> <path> --move");
            Ok(())
        }
        StorageCmd::Check { kind, path } => {
            let kind = kind_arg(&kind)?;
            if let Some(e) = runtimes_path_problem(kind, &path) {
                return Err(CuaError::InvalidArgument(e));
            }
            let needed = storage::needed_bytes(&storage::dir(kind), &path);
            match storage::validate(&home, kind, &path, needed) {
                Ok(v) => {
                    if json {
                        util::json_line(
                            out,
                            &json!({"ok": true, "needed_bytes": needed, "available_bytes": v.available,
                                    "other_volume": v.other_volume, "warnings": v.warnings}),
                        );
                    } else {
                        line(out, format!("ok, {} to copy", human(needed)));
                        for w in v.warnings {
                            hint_line(out, w);
                        }
                    }
                    Ok(())
                }
                Err(e) => {
                    if json {
                        util::json_line(out, &json!({"ok": false, "error": e}));
                        Ok(())
                    } else {
                        Err(CuaError::InvalidArgument(e))
                    }
                }
            }
        }
        StorageCmd::Set {
            kind,
            path,
            move_data,
        } => {
            let kind = kind_arg(&kind)?;
            if kind
                .env_var()
                .and_then(|v| std::env::var(v).ok())
                .is_some_and(|v| !v.is_empty())
            {
                return Err(CuaError::InvalidArgument(format!(
                    "{} is set in the environment and wins over this setting; unset it first",
                    kind.env_var().unwrap_or("")
                )));
            }
            if move_data {
                let plan = storage::move_kind(&home, kind, &path, |_| {})
                    .map_err(CuaError::InvalidArgument)?;
                if plan.from == plan.to {
                    if json {
                        util::json_line(out, &json!({"ok": true, "to": plan.to, "bytes": 0}));
                    } else {
                        line(
                            out,
                            format!("{} is already in {}", kind.name(), plan.to.display()),
                        );
                    }
                    return Ok(());
                }
                if json {
                    util::json_line(
                        out,
                        &json!({"ok": true, "from": plan.from, "to": plan.to, "bytes": plan.bytes}),
                    );
                } else {
                    line(
                        out,
                        format!("Moved {} to {}", human(plan.bytes), plan.to.display()),
                    );
                    hint_line(
                        out,
                        format!(
                            "The old copy is still in {}. Remove it with: cua storage remove {}",
                            plan.from.display(),
                            plan.from.display()
                        ),
                    );
                }
            } else {
                let needed = 0;
                storage::validate(&home, kind, &path, needed).map_err(CuaError::InvalidArgument)?;
                storage::set_override(&home, kind, Some(&path)).map_err(util::internal)?;
                if json {
                    util::json_line(out, &json!({"ok": true, "to": path}));
                } else {
                    line(
                        out,
                        format!("{} now lives in {}", kind.name(), path.display()),
                    );
                }
            }
            Ok(())
        }
        StorageCmd::Reset { kind } => {
            let kind = kind_arg(&kind)?;
            storage::set_override(&home, kind, None).map_err(util::internal)?;
            if json {
                util::json_line(out, &json!({"ok": true, "path": storage::dir(kind)}));
            } else {
                line(
                    out,
                    format!(
                        "{} is back in {}",
                        kind.name(),
                        storage::dir(kind).display()
                    ),
                );
            }
            Ok(())
        }
        StorageCmd::Remove { path } => {
            let removed = remove(&home, &path).map_err(CuaError::InvalidArgument)?;
            if json {
                util::json_line(out, &json!({"ok": true, "freed_bytes": removed}));
            } else {
                line(out, format!("Removed {}", human(removed)));
            }
            Ok(())
        }
    }
}

/// Deletes a computer image or runtime below `runtimes`, or an old location
/// marked by a move. Anything else is refused.
fn remove(home: &Path, path: &Path) -> Result<u64, String> {
    let canon = path
        .canonicalize()
        .map_err(|_| "That folder does not exist.".to_string())?;
    if canon.join(storage::MOVED_MARKER).exists() {
        return storage::remove_old(&canon).map_err(|e| e.to_string());
    }
    let runtimes = storage::dir(Kind::Runtimes)
        .canonicalize()
        .unwrap_or_else(|_| storage::dir(Kind::Runtimes));
    let inside = canon.starts_with(&runtimes) && canon != runtimes;
    let depth = canon
        .strip_prefix(&runtimes)
        .map(|r| r.components().count())
        .unwrap_or(0);
    if !inside || depth != 2 {
        return Err("Only a computer image or runtime can be removed here.".into());
    }
    if storage::daemon_running(home) {
        return Err("Stop Cua's computers before removing their files.".into());
    }
    let busy = storage::processes_using(&canon);
    if !busy.is_empty() {
        return Err(format!(
            "A computer is still running from that folder ({}).",
            busy.join(", ")
        ));
    }
    cua_home::guard_write(&canon).map_err(|e| e.to_string())?;
    let bytes = storage::disk_usage(&canon);
    std::fs::remove_dir_all(&canon)
        .or_else(|_| std::fs::remove_file(&canon))
        .map_err(|e| e.to_string())?;
    Ok(bytes)
}
