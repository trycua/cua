//! `cua share`: one link to a port, a Space port, a file or a folder, that
//! opens on your other signed-in devices (phone included), never a
//! localhost link. The daemon owns the shares (see `cua-daemon`'s share
//! hub), so a link keeps working after this process exits.

use std::io::{IsTerminal as _, Write};
use std::time::{Duration, SystemTime};

use clap::{Args, Subcommand};
use cua_daemon::{ShareAccess, ShareInfo, ShareTarget};
use cua_sdk::CuaError;

use crate::util::line;

#[derive(Args, Debug, Clone)]
#[command(
    long_about = "Share a link to a port, a file or a folder that opens on your other signed-in devices (phone included), never a localhost link.\n\nThe link opens only for signed-in people (you; your team with --team) unless you pass --public. It keeps working until its --ttl, `cua share stop`, or the Space it points into is deleted. This machine must be on your cua.ai relay (`cua host setup`).",
    args_conflicts_with_subcommands = true,
    subcommand_negates_reqs = true,
    after_help = "Examples:
  # A dev server on this machine, for your own devices (phone included)
  cua share 3000
  # A port inside a Space
  cua share space:dev:3000
  # A report or a folder, read-only
  cua share ./report.html
  cua share ./build/
  # Members of your team
  cua share 3000 --team
  # Anyone with the link, for an hour
  cua share ./report.html --public --ttl 1h
  # What is shared, and stop it
  cua share ls
  cua share stop share-1a2b3c4d5e6f
  cua share stop all"
)]
pub struct ShareArgs {
    /// What to share: a port (`3000`), a port in a Space (`space:NAME:3000`),
    /// or a file or folder (`./report.html`, `./build/`). Paths are
    /// read-only; use `./ls` for a file called `ls`.
    #[arg(value_name = "TARGET", required = true)]
    target: Option<String>,
    /// Share with the members of your team.
    #[arg(long, conflicts_with = "public")]
    team: bool,
    /// Anyone with the link can open it. Expires (default 1h, 60s to 24h).
    #[arg(long)]
    public: bool,
    /// How long the share lasts (`10m`, `1h`, 60s to 24h). Public shares
    /// default to 1h; the others have no end unless you set one.
    #[arg(long, value_name = "DURATION", value_parser = parse_ttl)]
    ttl: Option<Duration>,
    /// A name for `cua share ls` (and `cua share stop NAME`).
    #[arg(long)]
    name: Option<String>,
    #[command(subcommand)]
    cmd: Option<ShareCmd>,
}

pub(crate) fn parse_ttl(s: &str) -> Result<Duration, String> {
    let d = humantime::parse_duration(s).map_err(|e| format!("{e} (try 10m, 1h, 24h)"))?;
    if d < Duration::from_secs(60) || d > Duration::from_secs(86_400) {
        return Err("must be between 60s and 24h".into());
    }
    Ok(d)
}

#[derive(Subcommand, Debug, Clone)]
enum ShareCmd {
    /// List the active shares.
    #[command(after_help = "Examples:
  # What is shared right now
  cua share ls
  # The same as JSON
  cua share ls --json")]
    Ls,
    /// Stop a share (its link stops working at once), or `all`.
    #[command(after_help = "Examples:
  # Stop one share by its id
  cua share stop share-1a2b3c4d5e6f
  # Stop everything this machine shares
  cua share stop all")]
    Stop {
        /// The share's id or name from `cua share ls`, or `all`.
        id: String,
    },
    /// Open a share's link once, as its owner, and say what came back.
    #[command(after_help = "Examples:
  # Does the link load?
  cua share check share-1a2b3c4d5e6f
  # Open a page below the link
  cua share check share-1a2b3c4d5e6f /index.html")]
    Check {
        /// The share's id or name.
        id: String,
        /// A path below the link to open.
        #[arg(default_value = "")]
        path: String,
    },
}

fn secs(t: SystemTime) -> i64 {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn target_json(t: &ShareTarget) -> serde_json::Value {
    match t {
        ShareTarget::Port(p) => serde_json::json!({"kind": "port", "port": p}),
        ShareTarget::SpacePort { space, port } => {
            serde_json::json!({"kind": "space_port", "space": space, "port": port})
        }
        ShareTarget::File(p) => serde_json::json!({"kind": "file", "path": p}),
        ShareTarget::Dir(p) => serde_json::json!({"kind": "dir", "path": p}),
    }
}

/// The JSON row of a share (the CLI's and the MCP tools' shape).
pub fn share_json(s: &ShareInfo) -> serde_json::Value {
    let mut v = serde_json::json!({
        "id": s.id,
        "url": s.url,
        "name": s.name,
        "access": s.access.as_str(),
        "target": target_json(&s.target),
        "owner": s.owner,
        "created_at": s.created_at.map(secs),
        "expires_at": s.expires_at.map(secs),
    });
    if !s.notices.is_empty() {
        v["notices"] = serde_json::json!(s.notices);
        v["notice"] = serde_json::json!(s.notices.join(". "));
    }
    v
}

fn ends(s: &ShareInfo) -> String {
    match s.remaining() {
        Some(d) => {
            let d = Duration::from_secs(d.as_secs());
            humantime::format_duration(d).to_string()
        }
        None => "never".into(),
    }
}

/// `cua share ...`.
pub async fn run(args: ShareArgs, json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    match args.cmd {
        Some(ShareCmd::Ls) => ls(json, out).await,
        Some(ShareCmd::Stop { id }) => stop(&id, json, out).await,
        Some(ShareCmd::Check { id, path }) => check(&id, &path, json, out).await,
        None => {
            let target = args.target.ok_or_else(|| {
                CuaError::InvalidArgument(
                    "share what? give a port, space:NAME:PORT or a path".into(),
                )
            })?;
            create(
                &target,
                if args.public {
                    ShareAccess::Public
                } else if args.team {
                    ShareAccess::Team
                } else {
                    ShareAccess::Private
                },
                args.ttl,
                args.name,
                json,
                out,
            )
            .await
        }
    }
}

async fn create(
    input: &str,
    access: ShareAccess,
    ttl: Option<Duration>,
    name: Option<String>,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let cwd = std::env::current_dir().map_err(|e| CuaError::InvalidArgument(e.to_string()))?;
    let target = ShareTarget::parse(input, &cwd)?;
    let d = cua_daemon::client::ensure_daemon().await?;
    let s = d.create_share(&target, access, ttl, name).await?;
    if json {
        line(out, share_json(&s).to_string());
    } else {
        line(out, &s.url);
        // Limits of this link (stdout stays only the link), once, TTY or not.
        for n in &s.notices {
            eprintln!("{n}");
        }
        if std::io::stderr().is_terminal() {
            let note = match s.access {
                ShareAccess::Private => "Opens on your signed-in devices.".to_string(),
                ShareAccess::Team => "Opens for your team, signed in.".to_string(),
                ShareAccess::Public => {
                    format!("Anyone with the link can open it, for {}.", ends(&s))
                }
            };
            eprintln!(
                "{}",
                crate::util::hint(&format!("{note} Stop it with: cua share stop {}", s.id))
            );
        }
    }
    Ok(0)
}

async fn ls(json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    let d = cua_daemon::client::ensure_daemon().await?;
    let shares = d.list_shares().await?;
    if json {
        line(
            out,
            serde_json::Value::Array(shares.iter().map(share_json).collect()).to_string(),
        );
        return Ok(0);
    }
    if shares.is_empty() {
        eprintln!(
            "{}",
            crate::util::hint("Nothing is shared. Share something with: cua share 3000")
        );
        return Ok(0);
    }
    line(
        out,
        format!(
            "{:<20} {:<8} {:<10} {:<28} LINK",
            "ID", "ACCESS", "ENDS IN", "TARGET"
        ),
    );
    for s in &shares {
        let label = s.name.clone().unwrap_or_else(|| s.target.describe());
        let label = if label.chars().count() > 28 {
            format!("{}...", label.chars().take(25).collect::<String>())
        } else {
            label
        };
        line(
            out,
            format!(
                "{:<20} {:<8} {:<10} {:<28} {}",
                s.id,
                s.access.as_str(),
                ends(s),
                label,
                s.url
            ),
        );
    }
    Ok(0)
}

async fn stop(id: &str, json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    let d = cua_daemon::client::ensure_daemon().await?;
    let all = id == "all";
    let stopped = d.stop_share(if all { None } else { Some(id) }, all).await?;
    if json {
        line(out, serde_json::json!({ "stopped": stopped }).to_string());
    } else if all {
        line(
            out,
            format!(
                "Stopped {} share{}",
                stopped.len(),
                if stopped.len() == 1 { "" } else { "s" }
            ),
        );
    } else {
        for s in &stopped {
            line(out, format!("Stopped {s}"));
        }
    }
    Ok(0)
}

async fn check(id: &str, path: &str, json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    let d = cua_daemon::client::ensure_daemon().await?;
    let c = d.check_share(id, Some(path)).await?;
    if json {
        line(
            out,
            serde_json::json!({"ok": c.ok, "status": c.status, "via": c.via, "detail": c.detail})
                .to_string(),
        );
    } else {
        line(
            out,
            format!("{}: {}", if c.ok { "ok" } else { "not ok" }, c.detail),
        );
    }
    Ok(if c.ok { 0 } else { 1 })
}
