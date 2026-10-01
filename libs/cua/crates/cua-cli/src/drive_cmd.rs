//! `cua volume`: Cua Volume, the versioned volume every Space and agent of this
//! account shares (`public/`, `agents/<agent>/`, `spaces/<space>/`).
//!
//! Every subcommand is one Spaces tool through the SDK handle (the daemon
//! when one runs, else embedded), so access is checked by the runtime that
//! owns the drive, and grants and approvals ask for the same user presence
//! a Keyvault consent does. `--as agent:<name>` views the drive exactly as
//! that agent does; it only ever narrows.

use std::io::{Read as _, Write};
use std::sync::Arc;

use clap::{Args, Subcommand};
use cua_sdk::{Cua, CuaError, DriveView};

use crate::util::line;

/// Whose view of the drive a command takes.
#[derive(Args, Debug, Clone, Default)]
pub struct ViewArgs {
    /// View the drive as this persistent agent does (`agent:ada` or `ada`).
    #[arg(long = "as", value_name = "AGENT")]
    pub as_agent: Option<String>,
    /// With --as: the Space the agent is in (its `spaces/<space>/` folder).
    #[arg(long = "space", value_name = "SPACE", requires = "as_agent")]
    pub space: Option<String>,
}

impl ViewArgs {
    fn view(&self) -> Option<DriveView> {
        self.as_agent.as_ref().map(|a| DriveView {
            as_agent: Some(a.strip_prefix("agent:").unwrap_or(a).to_string()),
            in_space: self.space.clone(),
        })
    }
}

/// `cua volume` subcommands.
#[derive(Subcommand, Debug, Clone)]
pub enum DriveCmd {
    /// List a folder (default: the root).
    #[command(after_help = "Examples:
  cua volume ls
  cua volume ls agents/ada/
  cua volume ls --as agent:ada --space local:work")]
    Ls {
        path: Option<String>,
        #[command(flatten)]
        view: ViewArgs,
    },
    /// Print a file (or one of its versions) to stdout.
    #[command(after_help = "Examples:
  cua volume cat agents/ada/memory/MEMORY.md
  cua volume cat public/rules.md --version 0001790683200000-1a2b3c4d")]
    Cat {
        path: String,
        #[arg(long)]
        version: Option<String>,
        #[command(flatten)]
        view: ViewArgs,
    },
    /// Write a file from a local file, or from stdin with `-`.
    #[command(after_help = "Examples:
  cua volume put public/rules.md ./rules.md
  cua volume put public/rules.md -
  cua volume put agents/ada/inbox/brief.md ./brief.md --create-only")]
    Put {
        path: String,
        /// Local file, or `-` for stdin.
        source: String,
        /// Write only if the current version has this etag.
        #[arg(long, conflicts_with = "create_only")]
        if_etag: Option<String>,
        /// Write only if nothing is at the path yet.
        #[arg(long)]
        create_only: bool,
        #[command(flatten)]
        view: ViewArgs,
    },
    /// Delete a file (its history stays).
    #[command(after_help = "Examples:
  cua volume rm spaces/local-work/scratch.txt")]
    Rm {
        path: String,
        #[arg(long)]
        if_etag: Option<String>,
        #[command(flatten)]
        view: ViewArgs,
    },
    /// A file's versions, newest first.
    #[command(after_help = "Examples:
  cua volume history agents/ada/memory/MEMORY.md")]
    History {
        path: String,
        #[command(flatten)]
        view: ViewArgs,
    },
    /// Make an old version current again (a new version).
    #[command(after_help = "Examples:
  cua volume restore agents/ada/memory/MEMORY.md 0001790683200000-1a2b3c4d")]
    Restore {
        path: String,
        version: String,
        #[command(flatten)]
        view: ViewArgs,
    },
    /// Let an agent or a Space read (or, with --rw, write) a folder beyond
    /// its defaults. Asks for Touch ID or your passphrase.
    #[command(after_help = "Examples:
  cua volume grant agent:researcher agents/writer/outputs/
  cua volume grant space:local-build public/datasets/ --rw --for 2h")]
    Grant {
        /// `agent:<name>` or `space:<id>`.
        principal: String,
        /// A folder (ending in /) or one file.
        prefix: String,
        /// Read and write (default: read only).
        #[arg(long)]
        rw: bool,
        /// Lifetime (`30m`, `2h`, `7d`). Default: until revoked.
        #[arg(long = "for", value_name = "DURATION")]
        duration: Option<String>,
        /// A note shown next to the grant.
        #[arg(long)]
        note: Option<String>,
    },
    /// Revoke a grant.
    #[command(after_help = "Examples:
  cua volume revoke 5f2c9a0b1d3e4f67")]
    Revoke { grant_id: String },
    /// The grants (live ones; --all includes expired and revoked).
    #[command(after_help = "Examples:
  cua volume grants")]
    Grants {
        #[arg(long)]
        all: bool,
    },
    /// Access requests agents filed, waiting for you.
    #[command(after_help = "Examples:
  cua volume requests")]
    Requests,
    /// Turn a request into a grant. Asks for Touch ID or your passphrase.
    #[command(after_help = "Examples:
  cua volume approve 5f2c9a0b1d3e4f67
  cua volume approve 5f2c9a0b1d3e4f67 --for 1d")]
    Approve {
        request_id: String,
        #[arg(long = "for", value_name = "DURATION")]
        duration: Option<String>,
    },
    /// Decline a request.
    #[command(after_help = "Examples:
  cua volume deny 5f2c9a0b1d3e4f67")]
    Deny { request_id: String },
    /// The audit log, newest first, and whether its hash chain verified.
    #[command(after_help = "Examples:
  cua volume audit
  cua volume audit --limit 200 --json")]
    Audit {
        #[arg(long, default_value_t = 50)]
        limit: u32,
    },
    /// Show the drive as a volume (Finder on macOS, FUSE on Linux). Off
    /// until turned on; stays on across restarts.
    #[command(after_help = "Examples:
  cua volume mount
  cua volume mount --json")]
    Mount,
    /// Turn the volume off (files still uploading land first).
    #[command(after_help = "Examples:
  cua volume unmount")]
    Unmount,
    /// The volume, sync across devices, and the block cache.
    #[command(after_help = "Examples:
  cua volume status
  cua volume status --json")]
    Status,
    /// Which store the drive uses (fs, s3, cloud).
    #[command(subcommand)]
    Config(ConfigCmd),
}

/// `cua volume config`.
#[derive(Subcommand, Debug, Clone)]
pub enum ConfigCmd {
    /// The backend and bucket settings in effect (never keys).
    #[command(after_help = "Examples:
  cua volume config show")]
    Show,
    /// Choose the backend. Keys are never arguments: see set-keys.
    #[command(after_help = "Examples:
  cua volume config set --backend fs
  cua volume config set --backend s3 --endpoint http://127.0.0.1:9000 --bucket cua-volume --path-style")]
    Set {
        /// `fs` (default), `s3`, or `cloud` (off unless the Cua cloud enables it).
        #[arg(long)]
        backend: String,
        #[arg(long)]
        endpoint: Option<String>,
        #[arg(long)]
        region: Option<String>,
        #[arg(long)]
        bucket: Option<String>,
        /// A key prefix every drive key lives under.
        #[arg(long)]
        root: Option<String>,
        #[arg(long)]
        path_style: bool,
    },
    /// Save S3 keys in the credential store: the access key id and the
    /// secret, one per line on stdin.
    #[command(after_help = "Examples:
  cua volume config set-keys")]
    SetKeys,
}

fn duration_secs(d: &Option<String>) -> Result<Option<u64>, CuaError> {
    d.as_deref()
        .map(|s| {
            humantime::parse_duration(s.trim())
                .map(|d| d.as_secs())
                .map_err(|e| CuaError::InvalidArgument(format!("--for {s:?}: {e}")))
        })
        .transpose()
}

fn when(ms: u64) -> String {
    if ms == 0 {
        return String::new();
    }
    humantime::format_rfc3339_seconds(std::time::UNIX_EPOCH + std::time::Duration::from_millis(ms))
        .to_string()
}

/// Runs `cua volume <cmd>`.
fn mount_line(m: &cua_sdk::DriveMountStatus) -> String {
    match (m.state.as_str(), &m.path) {
        ("mounted", Some(p)) => format!("{} is mounted at {p} ({})", m.volume_name, m.method),
        (state, _) => match &m.detail {
            Some(d) => format!("{}: {state} ({d})", m.volume_name),
            None => format!("{}: {state}", m.volume_name),
        },
    }
}

pub async fn run(
    cua: &Arc<Cua>,
    cmd: DriveCmd,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    use serde_json::json;
    let spaces = cua.spaces();
    match cmd {
        DriveCmd::Ls { path, view } => {
            let l = spaces.volume_ls(path, view.view()).await?;
            if json {
                let entries: Vec<_> = l
                    .entries
                    .iter()
                    .map(|e| json!({"path": e.path, "name": e.name, "folder": e.folder,
                        "size": e.size, "modified_ms": e.modified_ms, "etag": e.etag, "mode": e.mode}))
                    .collect();
                line(
                    out,
                    json!({"path": l.path, "principal": l.principal, "entries": entries})
                        .to_string(),
                );
            } else if l.entries.is_empty() {
                line(
                    out,
                    format!("{} is empty", if l.path.is_empty() { "/" } else { &l.path }),
                );
            } else {
                for e in &l.entries {
                    if e.folder {
                        line(out, format!("{:<3} {:>10}  {}", e.mode, "", e.path));
                    } else {
                        line(
                            out,
                            format!(
                                "{:<3} {:>10}  {}  {}",
                                e.mode,
                                e.size,
                                e.path,
                                when(e.modified_ms)
                            ),
                        );
                    }
                }
            }
        }
        DriveCmd::Cat {
            path,
            version,
            view,
        } => {
            let f = spaces.volume_read(path, version, view.view()).await?;
            if json {
                let text = String::from_utf8(f.content.clone());
                line(
                    out,
                    json!({"path": f.path, "size": f.size, "etag": f.etag, "version": f.version,
                        "modified_ms": f.modified_ms,
                        "content": text.as_ref().ok(),
                        "binary": text.is_err()})
                    .to_string(),
                );
            } else {
                out.write_all(&f.content)
                    .map_err(|e| CuaError::Internal(e.to_string()))?;
            }
        }
        DriveCmd::Put {
            path,
            source,
            if_etag,
            create_only,
            view,
        } => {
            let bytes = if source == "-" {
                let mut b = vec![];
                std::io::stdin()
                    .read_to_end(&mut b)
                    .map_err(|e| CuaError::Internal(format!("stdin: {e}")))?;
                b
            } else {
                std::fs::read(&source)
                    .map_err(|e| CuaError::InvalidArgument(format!("{source}: {e}")))?
            };
            let o = spaces
                .volume_write(path, bytes, if_etag, create_only, view.view())
                .await?;
            if json {
                line(
                    out,
                    json!({"key": o.key, "size": o.size, "etag": o.etag, "version": o.version})
                        .to_string(),
                );
            } else {
                line(
                    out,
                    format!("wrote {} ({} bytes, version {})", o.key, o.size, o.version),
                );
            }
        }
        DriveCmd::Rm {
            path,
            if_etag,
            view,
        } => {
            spaces
                .volume_delete(path.clone(), if_etag, view.view())
                .await?;
            if json {
                line(out, json!({"deleted": path}).to_string());
            } else {
                line(out, format!("deleted {path} (its history stays)"));
            }
        }
        DriveCmd::History { path, view } => {
            let vs = spaces.volume_history(path, view.view()).await?;
            if json {
                let rows: Vec<_> = vs
                    .iter()
                    .map(|v| {
                        json!({"version": v.version, "size": v.size, "modified_ms": v.modified_ms,
                        "deleted": v.deleted, "latest": v.latest})
                    })
                    .collect();
                line(out, json!({"versions": rows}).to_string());
            } else {
                for v in &vs {
                    let tag = if v.deleted {
                        "deleted"
                    } else if v.latest {
                        "current"
                    } else {
                        ""
                    };
                    line(
                        out,
                        format!(
                            "{}  {:>10}  {}  {tag}",
                            v.version,
                            v.size,
                            when(v.modified_ms)
                        ),
                    );
                }
            }
        }
        DriveCmd::Restore {
            path,
            version,
            view,
        } => {
            let o = spaces
                .volume_restore(path, version.clone(), view.view())
                .await?;
            if json {
                line(
                    out,
                    json!({"key": o.key, "version": o.version, "restored_from": version})
                        .to_string(),
                );
            } else {
                line(
                    out,
                    format!(
                        "restored {} from {version} (now version {})",
                        o.key, o.version
                    ),
                );
            }
        }
        DriveCmd::Grant {
            principal,
            prefix,
            rw,
            duration,
            note,
        } => {
            let secs = duration_secs(&duration)?;
            let mode = if rw { "rw" } else { "r" };
            let g = spaces
                .volume_grant(principal, prefix, mode.into(), secs, note)
                .await?;
            print_grants(&[g], json, out);
        }
        DriveCmd::Revoke { grant_id } => {
            let g = spaces.volume_revoke(grant_id).await?;
            print_grants(&[g], json, out);
        }
        DriveCmd::Grants { all } => {
            let gs = spaces.volume_grants(all).await?;
            if !json && gs.is_empty() {
                line(out, "no grants: every agent has only its defaults");
            } else {
                print_grants(&gs, json, out);
            }
        }
        DriveCmd::Requests => {
            let rs = spaces.volume_requests().await?;
            if json {
                let rows: Vec<_> = rs
                    .iter()
                    .map(|r| {
                        json!({"id": r.id, "principal": r.principal, "prefix": r.prefix,
                        "mode": r.mode, "reason": r.reason, "created_ms": r.created_ms})
                    })
                    .collect();
                line(out, json!({"requests": rows}).to_string());
            } else if rs.is_empty() {
                line(out, "no requests waiting");
            } else {
                for r in &rs {
                    line(
                        out,
                        format!(
                            "{}  {} wants {} on {}: {}",
                            r.id, r.principal, r.mode, r.prefix, r.reason
                        ),
                    );
                }
            }
        }
        DriveCmd::Approve {
            request_id,
            duration,
        } => {
            let g = spaces
                .volume_approve(request_id, duration_secs(&duration)?)
                .await?;
            print_grants(&[g], json, out);
        }
        DriveCmd::Deny { request_id } => {
            spaces.volume_deny(request_id.clone()).await?;
            if json {
                line(out, json!({"denied": request_id}).to_string());
            } else {
                line(out, format!("declined {request_id}"));
            }
        }
        DriveCmd::Mount | DriveCmd::Unmount => {
            let m = if matches!(cmd, DriveCmd::Mount) {
                spaces.volume_mount().await?
            } else {
                spaces.volume_unmount().await?
            };
            if json {
                line(
                    out,
                    json!({"enabled": m.enabled, "state": m.state, "method": m.method,
                        "path": m.path, "volume_name": m.volume_name, "detail": m.detail})
                    .to_string(),
                );
            } else {
                line(out, mount_line(&m));
            }
            if m.state == "error" {
                return Ok(1);
            }
        }
        DriveCmd::Status => {
            let m = spaces.volume_mount_status().await?;
            let s = spaces.volume_sync_status().await?;
            let c = spaces.volume_cache_stats().await?;
            // Spaces that connected without a volume, and why (the tool's
            // `volume_errors`).
            let volume_errors: Vec<(String, String)> = spaces
                .call_tool_json("volume_sync_status".into(), None)
                .await
                .ok()
                .filter(|r| !r.is_error)
                .and_then(|r| {
                    serde_json::from_str::<serde_json::Value>(
                        r.structured_json.as_deref().unwrap_or(&r.text),
                    )
                    .ok()
                })
                .and_then(|v| v.get("volume_errors").and_then(|e| e.as_array()).cloned())
                .unwrap_or_default()
                .iter()
                .map(|e| {
                    let f = |k: &str| e.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
                    (f("space"), f("error"))
                })
                .collect();
            if json {
                let devices: Vec<_> = s
                    .devices
                    .iter()
                    .map(|d| {
                        json!({"id": d.id, "name": d.name, "this_device": d.this_device,
                        "last_seen_ms": d.last_seen_ms, "last_change_ms": d.last_change_ms})
                    })
                    .collect();
                let conflicts: Vec<_> = s
                    .conflicts
                    .iter()
                    .map(|x| json!({"path": x.path, "conflict_path": x.conflict_path}))
                    .collect();
                line(
                    out,
                    json!({
                        "mount": {"enabled": m.enabled, "state": m.state, "method": m.method,
                            "path": m.path},
                        "sync": {"feed": s.feed, "device_id": s.device_id,
                            "pending_uploads": s.pending_uploads, "pending_bytes": s.pending_bytes,
                            "devices": devices, "conflicts": conflicts, "last_error": s.last_error},
                        "cache": {"size_bytes": c.size_bytes, "capacity_bytes": c.capacity_bytes,
                            "hit_rate": c.hit_rate},
                        "volumes": s.volumes.iter().map(|v| json!({"space": v.space,
                            "mount_path": v.mount_path, "backend": v.backend,
                            "principal": v.principal})).collect::<Vec<_>>(),
                        "volume_errors": volume_errors.iter().map(|(space, error)|
                            json!({"space": space, "error": error})).collect::<Vec<_>>(),
                    })
                    .to_string(),
                );
            } else {
                line(out, mount_line(&m));
                line(
                    out,
                    format!(
                        "sync: {} on {} ({}), {} upload(s) pending, {} device(s), {} conflict(s)",
                        s.feed,
                        s.device_name,
                        s.device_id,
                        s.pending_uploads,
                        s.devices.len(),
                        s.conflicts.len()
                    ),
                );
                for x in &s.conflicts {
                    line(
                        out,
                        format!("  conflict: {} kept as {}", x.path, x.conflict_path),
                    );
                }
                for v in &s.volumes {
                    line(
                        out,
                        format!("{}: mounted at {} ({})", v.space, v.mount_path, v.backend),
                    );
                }
                for (space, error) in &volume_errors {
                    line(out, format!("{space}: {error}"));
                }
                line(
                    out,
                    format!(
                        "cache: {} MiB of {} MiB, hit rate {:.0}%",
                        c.size_bytes >> 20,
                        c.capacity_bytes >> 20,
                        c.hit_rate * 100.0
                    ),
                );
            }
        }
        DriveCmd::Audit { limit } => {
            let a = spaces.volume_audit(Some(limit)).await?;
            if json {
                let rows: Vec<_> = a
                    .events
                    .iter()
                    .map(|e| {
                        json!({"seq": e.seq, "ts_ms": e.ts_ms, "principal": e.principal,
                        "action": e.action, "path": e.path, "detail": e.detail})
                    })
                    .collect();
                line(
                    out,
                    json!({"events": rows, "verified": a.verified, "error": a.error}).to_string(),
                );
            } else {
                for e in &a.events {
                    line(
                        out,
                        format!(
                            "{}  {:<18} {:<14} {}  {}",
                            when(e.ts_ms),
                            e.principal,
                            e.action,
                            e.path,
                            e.detail
                        ),
                    );
                }
                match a.error {
                    None => line(out, "audit log verified"),
                    Some(err) => line(out, format!("AUDIT LOG DOES NOT VERIFY: {err}")),
                }
            }
            if !a.verified {
                return Ok(1);
            }
        }
        DriveCmd::Config(c) => return config(c, json, out),
    }
    Ok(0)
}

fn print_grants(gs: &[cua_sdk::DriveGrant], json: bool, out: &mut dyn Write) {
    use serde_json::json;
    if json {
        let rows: Vec<_> = gs
            .iter()
            .map(|g| json!({"id": g.id, "principal": g.principal, "prefix": g.prefix, "mode": g.mode,
                "created_ms": g.created_ms, "expires_ms": g.expires_ms, "revoked": g.revoked, "note": g.note}))
            .collect();
        line(out, json!({"grants": rows}).to_string());
        return;
    }
    for g in gs {
        let until = g
            .expires_ms
            .map(|e| format!(" until {}", when(e)))
            .unwrap_or_default();
        let state = if g.revoked { " (revoked)" } else { "" };
        line(
            out,
            format!(
                "{}  {} {} {}{until}{state}",
                g.id, g.principal, g.mode, g.prefix
            ),
        );
    }
}

/// `cua volume config`: the drive's backend lives with the Cua Volume, which
/// ships with Cua Spaces ([`crate::extension`]).
fn config(cmd: ConfigCmd, json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    crate::extension::drive_config(cmd, json, out)
}
