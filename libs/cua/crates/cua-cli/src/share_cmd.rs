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
  # A game server in a Space, at a public address (Labs: Public TCP shares)
  cua share space:mc:25565 --tcp --public --service minecraft
  # The same from an agent inside the Space (its own port)
  cua share 25565 --tcp --public --service minecraft
  # Through your own cloud only, for two friends' addresses
  cua share space:mc:25565 --tcp --public --route edge --allow 203.0.113.7 --allow 198.51.100.0/24
  # A share an agent asked for, waiting for you
  cua share approve share-1a2b3c4d5e6f
  # Where public TCP shares go, the daily data cap and the default allowlist
  cua share settings
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
    /// Share a raw TCP service in a Space (a game server, for example) at a
    /// public address instead of a link: anyone with the address can
    /// connect, for 24h unless --ttl says less. Needs --public and a port in
    /// a Space (inside a Space, a bare port is its own). An experiment:
    /// turn on Public TCP shares in Settings, Labs.
    #[arg(long, requires = "public")]
    tcp: bool,
    /// With --tcp: the game or service (`minecraft`), for a name people can
    /// type instead of the address.
    #[arg(long, requires = "tcp")]
    service: Option<String>,
    /// With --tcp: how people reach it. `auto` (default) takes the Space's
    /// own host in your cloud, else a small server in your linked cloud
    /// (a share edge), else Cua's relay, within `cua share settings`.
    #[arg(long, requires = "tcp", value_parser = ["auto", "host", "edge", "relay"])]
    route: Option<String>,
    /// With --tcp: only these addresses or CIDRs may connect (repeat it).
    /// Default: the settings' allowlist, usually anyone.
    #[arg(long = "allow", value_name = "CIDR", requires = "tcp")]
    allow: Vec<String>,
    /// With --tcp --service minecraft: drop connections that did not dial
    /// this share's address (scanners, stale addresses).
    #[arg(long, requires = "service")]
    check_host: bool,
    /// With --tcp: agree to the share edge's cost without asking (the first
    /// share through your cloud creates a small server there).
    #[arg(long, short = 'y', requires = "tcp")]
    yes: bool,
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

impl ShareArgs {
    /// Whether this call is part of the Public TCP shares experiment: a
    /// `--tcp` share, or the commands that decide on one.
    pub fn uses_tcp_shares(&self) -> bool {
        self.tcp
            || matches!(
                self.cmd,
                Some(ShareCmd::Approve { .. } | ShareCmd::Deny { .. } | ShareCmd::Settings { .. })
            )
    }
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
    /// Let a waiting public TCP share go live: one an agent in a Space
    /// asked for, or the first through a share edge in your cloud.
    #[command(after_help = "Examples:
  # See what waits, then approve it
  cua share ls
  cua share approve share-1a2b3c4d5e6f
  # And let that Space share without asking from now on
  cua share approve share-1a2b3c4d5e6f --always")]
    Approve {
        /// The waiting share's id.
        id: String,
        /// Let its Space's agents share without asking from now on.
        #[arg(long)]
        always: bool,
    },
    /// Turn down a waiting public TCP share.
    #[command(after_help = "Examples:
  cua share deny share-1a2b3c4d5e6f")]
    Deny {
        /// The waiting share's id.
        id: String,
    },
    /// Where public TCP shares go, the daily data cap, the default
    /// allowlist and the approval rule; with no flags, shows them.
    #[command(after_help = "Examples:
  # Show them
  cua share settings
  # Never through Cua: only your own cloud
  cua share settings --land only-my-cloud
  # A 5 GB daily data cap, or none
  cua share settings --daily-cap 5GB
  cua share settings --daily-cap off
  # New shares only for these addresses
  cua share settings --allow 203.0.113.0/24
  # Remove the share edge from your cloud now
  cua share settings --remove-edge")]
    Settings {
        /// `auto`, `only-my-cloud` or `allow-relay`.
        #[arg(long, value_parser = ["auto", "only-my-cloud", "allow-relay"])]
        land: Option<String>,
        /// Whether Cua's relay may carry public TCP shares at all.
        #[arg(long, value_parser = ["on", "off"])]
        relay: Option<String>,
        /// The most data public TCP shares move in a day (`2GB`, `500MB`,
        /// `off`).
        #[arg(long, value_name = "SIZE")]
        daily_cap: Option<String>,
        /// The allowlist new shares start with (repeat it).
        #[arg(long = "allow", value_name = "CIDR", conflicts_with = "allow_anyone")]
        allow: Vec<String>,
        /// New shares are open to anyone (clears the allowlist).
        #[arg(long)]
        allow_anyone: bool,
        /// Ask before an agent's share goes public (`on`, the default).
        #[arg(long, value_parser = ["on", "off"])]
        approve_agents: Option<String>,
        /// Remove the share edge from your cloud now; its shares end.
        #[arg(long)]
        remove_edge: bool,
    },
}

/// `2GB`, `500MB`, `512` (MB) or `off` in MiB (0: no cap).
pub(crate) fn parse_cap(s: &str) -> Result<u64, String> {
    let t = s.trim().to_ascii_lowercase().replace(' ', "");
    if t == "off" || t == "none" || t == "0" {
        return Ok(0);
    }
    let (num, mult) = if let Some(n) = t.strip_suffix("gb").or_else(|| t.strip_suffix("g")) {
        (n, 1024)
    } else if let Some(n) = t.strip_suffix("mb").or_else(|| t.strip_suffix("m")) {
        (n, 1)
    } else {
        (t.as_str(), 1)
    };
    let v: f64 = num
        .parse()
        .map_err(|_| format!("{s:?}: use a size like 2GB or 500MB, or off"))?;
    if v.is_nan() || v <= 0.0 || v * mult as f64 > (1u64 << 30) as f64 {
        return Err(format!("{s:?}: use a size like 2GB or 500MB, or off"));
    }
    Ok((v * mult as f64).round() as u64)
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
    if let Some(t) = &s.tcp {
        v["tcp"] = serde_json::json!({
            "address": t.address,
            "port": t.port,
            "srv_name": t.srv_name,
            "service": t.service,
            "connect_with": t.connect_with(),
            "route": t.route,
            "route_line": t.route_line,
            "cloud": t.cloud,
            "allow": t.allow,
        });
        v["route"] = serde_json::json!(t.route);
    }
    if let Some(w) = &s.waiting {
        v["waiting"] = serde_json::json!({
            "reason": w.reason,
            "text": w.text,
            "route_line": w.route_line,
        });
    }
    if let Some(o) = &s.origin {
        v["origin"] = serde_json::json!(o);
    }
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
        Some(ShareCmd::Approve { id, always }) => approve(&id, always, json, out).await,
        Some(ShareCmd::Deny { id }) => {
            let d = cua_daemon::client::ensure_daemon().await?;
            d.deny_share(&id).await?;
            if json {
                line(out, serde_json::json!({ "denied": id }).to_string());
            } else {
                line(out, format!("Turned down {id}"));
            }
            Ok(0)
        }
        Some(ShareCmd::Settings {
            land,
            relay,
            daily_cap,
            allow,
            allow_anyone,
            approve_agents,
            remove_edge,
        }) => {
            settings(
                SettingsChange {
                    land,
                    relay,
                    daily_cap,
                    allow,
                    allow_anyone,
                    approve_agents,
                    remove_edge,
                },
                json,
                out,
            )
            .await
        }
        None => {
            let target = args.target.ok_or_else(|| {
                CuaError::InvalidArgument(
                    "share what? give a port, space:NAME:PORT or a path".into(),
                )
            })?;
            if args.tcp {
                let opts = cua_daemon::TcpShareOptions {
                    service: args.service,
                    route: args.route.filter(|r| r != "auto"),
                    allow: args.allow,
                    check_host: args.check_host,
                    consent: args.yes,
                };
                return create_tcp(&target, args.ttl, args.name, &opts, json, out).await;
            }
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

/// The daemon to ask: inside a Space with a spawn channel, the channel
/// (it answers for the Space's own shares only); else this computer's.
async fn daemon() -> Result<cua_daemon::client::DaemonClient, CuaError> {
    if let Some(addr) = cua_daemon::client::spawn_channel() {
        return Ok(cua_daemon::client::DaemonClient::new(addr)?);
    }
    Ok(cua_daemon::client::ensure_daemon().await?)
}

async fn create_tcp(
    input: &str,
    ttl: Option<Duration>,
    name: Option<String>,
    opts: &cua_daemon::TcpShareOptions,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let cwd = std::env::current_dir().map_err(|e| CuaError::InvalidArgument(e.to_string()))?;
    let target = ShareTarget::parse(input, &cwd)?;
    if matches!(target, ShareTarget::File(_) | ShareTarget::Dir(_)) {
        return Err(CuaError::InvalidArgument(
            "--tcp shares a port in a Space (space:NAME:PORT), not a file".into(),
        ));
    }
    let in_space = cua_daemon::client::spawn_channel().is_some();
    if !in_space && matches!(target, ShareTarget::Port(_)) {
        return Err(CuaError::InvalidArgument(
            "--tcp shares a port in a Space, never this computer: use space:NAME:PORT".into(),
        ));
    }
    let d = daemon().await?;
    let mut s = d.create_tcp_share(&target, ttl, name, opts).await?;
    if let Some(w) = s.waiting.clone() {
        // The first share edge in a cloud: the owner, at a terminal, can
        // agree right here.
        let ask = w.reason == "consent" && !json && !in_space && std::io::stdin().is_terminal();
        if ask {
            eprintln!("{}", w.text);
            eprint!("Create it? [y/N] ");
            let mut answer = String::new();
            let _ = std::io::stdin().read_line(&mut answer);
            if matches!(answer.trim(), "y" | "Y" | "yes" | "Yes") {
                s = d.approve_share(&s.id, false).await?;
            } else {
                d.deny_share(&s.id).await?;
                eprintln!("Not shared.");
                return Ok(1);
            }
        } else {
            if json {
                line(out, share_json(&s).to_string());
                return Ok(0);
            }
            line(out, format!("Waiting: {}", w.text));
            let how = if in_space {
                "The owner approves it on their computer (Cua, Shared links, or `cua share approve`)."
                    .to_string()
            } else {
                format!("Approve it with: cua share approve {} (or add --yes)", s.id)
            };
            eprintln!("{}", crate::util::hint(&how));
            return Ok(0);
        }
    }
    if json {
        line(out, share_json(&s).to_string());
        return Ok(0);
    }
    let tcp = s.tcp.clone();
    match &tcp {
        Some(t) => line(out, t.connect_with()),
        None => line(out, &s.url),
    }
    if std::io::stderr().is_terminal() {
        let also = match &tcp {
            Some(t) if t.srv_name.is_some() => format!(" (or {})", t.address),
            _ => String::new(),
        };
        let route = tcp
            .as_ref()
            .map(|t| format!("{}. ", t.route_line))
            .unwrap_or_default();
        let who = match &tcp {
            Some(t) if !t.allow.is_empty() => format!("Only {} can connect", t.allow.join(", ")),
            _ => format!("Anyone with the address{also} can connect"),
        };
        eprintln!(
            "{}",
            crate::util::hint(&format!(
                "{route}{who}, for {}. Stop it with: cua share stop {}",
                ends(&s),
                s.id
            ))
        );
    }
    Ok(0)
}

async fn approve(id: &str, always: bool, json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    let d = cua_daemon::client::ensure_daemon().await?;
    let s = d.approve_share(id, always).await?;
    if json {
        line(out, share_json(&s).to_string());
        return Ok(0);
    }
    match &s.tcp {
        Some(t) => {
            line(out, t.connect_with());
            eprintln!(
                "{}",
                crate::util::hint(&format!(
                    "{}. Stop it with: cua share stop {}",
                    t.route_line, s.id
                ))
            );
        }
        None => line(out, &s.url),
    }
    Ok(0)
}

struct SettingsChange {
    land: Option<String>,
    relay: Option<String>,
    daily_cap: Option<String>,
    allow: Vec<String>,
    allow_anyone: bool,
    approve_agents: Option<String>,
    remove_edge: bool,
}

async fn settings(c: SettingsChange, json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    use cua_daemon::share_route::Landing;
    let d = cua_daemon::client::ensure_daemon().await?;
    if c.remove_edge {
        let stopped = d.remove_share_edge().await?;
        if !json {
            line(
                out,
                format!(
                    "Removed the share edge ({} share{} ended)",
                    stopped.len(),
                    if stopped.len() == 1 { "" } else { "s" }
                ),
            );
        }
    }
    let mut info = d.share_settings().await?;
    let changed = c.land.is_some()
        || c.relay.is_some()
        || c.daily_cap.is_some()
        || !c.allow.is_empty()
        || c.allow_anyone
        || c.approve_agents.is_some();
    if changed {
        let mut s = info.settings.clone();
        if let Some(l) = &c.land {
            s.landing = match l.as_str() {
                "only-my-cloud" => Landing::OnlyMyCloud,
                "allow-relay" => Landing::AllowRelay,
                _ => Landing::Auto,
            };
        }
        if let Some(r) = &c.relay {
            s.relay_fallback = r == "on";
        }
        if let Some(cap) = &c.daily_cap {
            s.daily_cap_mib = parse_cap(cap).map_err(CuaError::InvalidArgument)?;
        }
        if !c.allow.is_empty() {
            s.default_allow = c.allow.clone();
        }
        if c.allow_anyone {
            s.default_allow.clear();
        }
        if let Some(a) = &c.approve_agents {
            s.approve_agent_shares = a == "on";
        }
        info.settings = d.set_share_settings(&s).await?;
    }
    let s = &info.settings;
    let land = match s.landing {
        Landing::Auto => "auto",
        Landing::OnlyMyCloud => "only-my-cloud",
        Landing::AllowRelay => "allow-relay",
    };
    if json {
        line(
            out,
            serde_json::json!({
                "land": land,
                "relay": s.relay_fallback,
                "daily_cap_mib": s.daily_cap_mib,
                "used_today": info.used_today,
                "default_allow": s.default_allow,
                "approve_agent_shares": s.approve_agent_shares,
                "always_allow_spaces": s.always_allow_spaces,
                "edge_consent": s.edge_consent,
                "edge": info.edge.as_ref().map(|(c, h, usd)| serde_json::json!({
                    "cloud": c, "host": h, "monthly_usd": usd,
                })),
            })
            .to_string(),
        );
        return Ok(0);
    }
    let cap = if s.daily_cap_mib == 0 {
        "no cap".to_string()
    } else {
        format!(
            "{} a day ({} used today)",
            cua_daemon::share_route::human_mib(s.daily_cap_mib),
            cua_daemon::share_route::human_mib(info.used_today >> 20)
        )
    };
    line(out, format!("Shares land:     {land}"));
    line(
        out,
        format!(
            "Cua relay:       {}",
            if s.relay_fallback { "allowed" } else { "off" }
        ),
    );
    line(out, format!("Daily data cap:  {cap}"));
    line(
        out,
        format!(
            "Who can connect: {}",
            if s.default_allow.is_empty() {
                "anyone with the address".to_string()
            } else {
                s.default_allow.join(", ")
            }
        ),
    );
    line(
        out,
        format!(
            "Agent shares:    {}",
            if s.approve_agent_shares {
                "ask me first"
            } else {
                "go public at once"
            }
        ),
    );
    match &info.edge {
        Some((cloud, host, usd)) => line(
            out,
            format!(
                "Share edge:      {host} in {} (about ${usd:.0} a month)",
                cloud_title(cloud)
            ),
        ),
        None => line(out, "Share edge:      none"),
    }
    Ok(0)
}

/// `AWS` for `aws`.
fn cloud_title(cloud: &str) -> String {
    match cloud {
        "aws" => "AWS".into(),
        "gcp" => "Google Cloud".into(),
        "azure" => "Azure".into(),
        "hetzner" => "Hetzner".into(),
        other => other.to_owned(),
    }
}

async fn ls(json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    let d = daemon().await?;
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
            "{:<20} {:<8} {:<10} {:<28} {:<8} LINK",
            "ID", "ACCESS", "ENDS IN", "TARGET", "ROUTE"
        ),
    );
    for s in &shares {
        let label = s.name.clone().unwrap_or_else(|| s.target.describe());
        let label = if label.chars().count() > 28 {
            format!("{}...", label.chars().take(25).collect::<String>())
        } else {
            label
        };
        let route = match (&s.waiting, &s.tcp) {
            (Some(_), _) => "waiting",
            (None, Some(t)) => t.route.as_str(),
            (None, None) => "-",
        };
        let link = match (&s.waiting, &s.tcp) {
            (Some(w), _) => format!("{} (cua share approve {})", w.route_line, s.id),
            (None, Some(t)) => t.connect_with().to_owned(),
            (None, None) => s.url.clone(),
        };
        line(
            out,
            format!(
                "{:<20} {:<8} {:<10} {:<28} {:<8} {}",
                s.id,
                s.access.as_str(),
                ends(s),
                label,
                route,
                link
            ),
        );
    }
    Ok(0)
}

async fn stop(id: &str, json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    let d = daemon().await?;
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

#[cfg(test)]
mod cap_tests {
    use super::parse_cap;

    #[test]
    fn caps_read_as_mib() {
        assert_eq!(parse_cap("2GB"), Ok(2048));
        assert_eq!(parse_cap("500 MB"), Ok(500));
        assert_eq!(parse_cap("1.5g"), Ok(1536));
        assert_eq!(parse_cap("512"), Ok(512));
        assert_eq!(parse_cap("off"), Ok(0));
        assert!(parse_cap("lots").is_err());
        assert!(parse_cap("-1GB").is_err());
    }
}
