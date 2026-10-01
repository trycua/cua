//! `cua host …` (set this machine up for unattended access, over cua-host)
//! and `cua spaces` (`ls`: registered Spaces plus the account's relay
//! machines; `add` / `rm`: register or forget a Space).

use crate::auth;
use crate::util::line;
use clap::Subcommand;
use cua_host::{
    AccountTokens, DeviceAuth, Host, HostStatus, MachinePatch, RelayClient, RunnerKind,
    ServiceManager, SetupOptions,
};
use cua_sdk::CuaError;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Subcommand, Debug)]
pub enum HostCmd {
    /// Set this machine up for unattended access: install cua-spacesd as
    /// a service that joins the cua.ai relay as your account (default) or
    /// serves on a direct ip:port.
    #[command(after_help = "Examples:
  # Join the cua.ai relay as your account (shares this desktop)
  cua host setup
  # A spare machine: do not share its desktop, run Spaces for your devices
  cua host setup --profile spare --name \"Mac mini (spare)\"
  cua host setup --no-desktop --provide-spaces
  # Serve on the LAN instead of the relay
  cua host setup --direct 0.0.0.0:3211 --name studio-mac
  # A spare Mac on your tailnet that runs Spaces, without the relay
  # (prints the command that adds it on your laptop)
  cua host setup --direct 100.101.102.103:3211 --profile spare")]
    Setup {
        /// Relay URL (default $CUA_RELAY_URL, else https://relay.cua.ai).
        #[arg(long, conflicts_with = "direct")]
        relay: Option<String>,
        /// Serve directly on ip:port with a local env token (LAN /
        /// port-forwarded) instead of joining a relay.
        #[arg(long)]
        direct: Option<String>,
        /// Display name (default: the host name).
        #[arg(long)]
        name: Option<String>,
        /// Also allow this account (id or email). Repeatable.
        #[arg(long = "allow")]
        allow: Vec<String>,
        /// cua-spacesd binary (default $CUA_SPACESD_BIN, a bundled
        /// one, or the release download).
        #[arg(long)]
        driver_bin: Option<PathBuf>,
        /// auto | systemd | launchd | windows-task | process.
        #[arg(long, default_value = "auto")]
        runner: String,
        /// What this machine is for: `desktop` (share this desktop; the
        /// default) or `spare` (do not share the desktop; provide Spaces).
        #[arg(long, default_value = "desktop", value_parser = ["desktop", "spare"])]
        profile: String,
        /// Share this machine's own desktop as a Space.
        #[arg(long, overrides_with = "no_desktop")]
        desktop: bool,
        /// Do not share this machine's own desktop (screen, input, shell,
        /// files): relayed callers reach only the Spaces it provides.
        #[arg(long)]
        no_desktop: bool,
        /// Create Spaces for your enrolled devices on this machine's
        /// runtimes (Lume macOS VMs, Docker Linux containers).
        #[arg(long, overrides_with = "no_provide_spaces")]
        provide_spaces: bool,
        /// Do not create Spaces for other devices.
        #[arg(long)]
        no_provide_spaces: bool,
        /// Provided Spaces at once (default 4; 0: no limit).
        #[arg(long)]
        max_spaces: Option<u32>,
        /// With --direct and Spaces: take host calls, and connections to
        /// the Spaces this machine forwards, from any address (and allow a
        /// public --direct address). By default only loopback, Tailscale
        /// and private LAN addresses may. The direct listener is plaintext
        /// and its token is a shell on this machine: prefer Tailscale.
        #[arg(long, requires = "direct")]
        allow_any_address: bool,
    },
    /// Show or change what this machine shares: its own desktop, and
    /// whether it creates Spaces for your other devices.
    #[command(after_help = "Examples:
  cua host config
  # Stop sharing this desktop; keep providing Spaces
  cua host config --desktop off --provide-spaces on
  cua host config --max-spaces 6")]
    Config {
        /// Share this machine's own desktop: on | off.
        #[arg(long, value_parser = ["on", "off"])]
        desktop: Option<String>,
        /// Create Spaces for your enrolled devices: on | off.
        #[arg(long, value_parser = ["on", "off"])]
        provide_spaces: Option<String>,
        /// Provided Spaces at once (0: no limit).
        #[arg(long)]
        max_spaces: Option<u32>,
        /// macOS VMs at once, 0 to 2 (Apple's license allows two per Mac).
        #[arg(long)]
        max_macos_vms: Option<u32>,
    },
    /// Show whether this machine is set up, the service, and who is
    /// connected.
    #[command(after_help = "Examples:
  cua host status")]
    Status,
    /// Stop sharing: disconnect everyone and refuse new connections.
    #[command(after_help = "Examples:
  cua host stop")]
    Stop,
    /// Start sharing again after `stop`.
    #[command(after_help = "Examples:
  cua host start")]
    Start,
    /// Share a relay machine (this one, or `--machine`) with other cua.ai
    /// accounts (id or verified email). Run from an enrolled device; every
    /// change is in the owner's audit log (`cua devices audit`).
    #[command(after_help = "Examples:
  cua host share friend@example.com
  cua host share friend@example.com --machine 0123abcd4567ef89 --yes")]
    Share {
        /// Accounts to add. Repeatable.
        #[arg(required = true)]
        who: Vec<String>,
        /// Machine id (default: this machine).
        #[arg(long)]
        machine: Option<String>,
        /// Relay URL (default: the one this machine joined, else
        /// $CUA_RELAY_URL / https://relay.cua.ai).
        #[arg(long)]
        relay: Option<String>,
        /// Do not ask for confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// Stop sharing a relay machine with accounts; their open connections
    /// are cut.
    #[command(after_help = "Examples:
  cua host unshare friend@example.com")]
    Unshare {
        /// Accounts to remove. Repeatable.
        #[arg(required = true)]
        who: Vec<String>,
        /// Machine id (default: this machine).
        #[arg(long)]
        machine: Option<String>,
        /// Relay URL.
        #[arg(long)]
        relay: Option<String>,
    },
    /// Unregister from the relay, uninstall the service and delete the host
    /// configuration (asks first).
    #[command(after_help = "Examples:
  cua host remove
  # Without the prompt (required in scripts and agents)
  cua host remove --force")]
    Remove {
        /// Skip the confirmation prompt. Required when there is no terminal
        /// to ask on (scripts, agents): without it nothing is removed.
        #[arg(long, short)]
        force: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum SpacesCmd {
    /// Registered Spaces and your machines on the relay.
    #[command(
        visible_alias = "list",
        after_help = "Examples:
  cua spaces ls
  cua spaces ls --json"
    )]
    Ls {
        /// Relay URL (default $CUA_RELAY_URL, else https://relay.cua.ai).
        #[arg(long)]
        relay: Option<String>,
    },
    /// Register a Space after a capabilities handshake with its cua-spacesd
    /// (or, for another URL, an MCP initialize). Re-adding an address
    /// updates it. The token is stored apart from the registry, 0600.
    #[command(after_help = "Examples:
  cua spaces add 10.0.0.5:3211 --token \"$TOKEN\" --name studio
  cua spaces add local:my-sandbox
  cua spaces add http://127.0.0.1:8765/mcp --service tools
  # A machine that provides Spaces over Tailscale, without the relay
  # (`cua host setup --direct ... --provide-spaces` on it prints this)
  cua spaces add 100.101.102.103:3211 --host --name \"Mac mini (spare)\" --token \"$TOKEN\"")]
    Add {
        /// The Space's cua-spacesd (`http(s)://host:port`, `host:port`,
        /// default port 3211), a Space id (`local:<name>`, `cloud:<name>`),
        /// or any MCP endpoint URL.
        url: String,
        /// spacesd token (or the bearer an MCP endpoint needs).
        #[arg(long, env = "CUA_ENV_TOKEN", hide_env_values = true)]
        token: Option<String>,
        /// Display name (default: the guest's hostname).
        #[arg(long)]
        name: Option<String>,
        /// For an MCP endpoint URL: the service name to register it under
        /// (default `mcp`).
        #[arg(long)]
        service: Option<String>,
        /// It is a machine that provides Spaces over its direct address
        /// (Tailscale or LAN): `--on host:<name>` finds it without the
        /// relay. Its token is stored like any direct Space's.
        #[arg(long, conflicts_with = "service")]
        host: bool,
    },
    /// Create Spaces: on this machine, on one of your machines that
    /// provides Spaces (`cua host setup --provide-spaces` there), or in the
    /// cloud.
    #[command(after_help = "Examples:
  cua spaces create linux
  # Two macOS VMs on your spare Mac mini (matched by name)
  cua spaces create macos:26 --on \"spare mac mini\" --count 2
  cua spaces create --on host:0123abcd4567ef89 linux --json")]
    Create {
        /// Image: an alias (`linux`, `macos`, `macos:26`) or a registry
        /// reference. Default: the canonical Linux image.
        image: Option<String>,
        /// Where: `local`, `cloud`, `host:<machine>`, or a machine name
        /// (`mac-mini`, "spare mac mini"). Default: your default location.
        #[arg(long)]
        on: Option<String>,
        /// How many (1 to 8).
        #[arg(long, default_value_t = 1)]
        count: u32,
        /// Name (with --count, `-2`, `-3`, ... are appended).
        #[arg(long)]
        name: Option<String>,
        /// auto | container | vm.
        #[arg(long)]
        kind: Option<String>,
        /// auto, or an engine the location offers (lume, runc, gvisor, qemu).
        #[arg(long)]
        runtime: Option<String>,
        /// vCPUs.
        #[arg(long)]
        cpus: Option<u32>,
        /// Memory in MiB.
        #[arg(long)]
        memory_mb: Option<u64>,
        /// Grow a VM's disk to this many GiB.
        #[arg(long)]
        disk_gb: Option<u32>,
        /// A GPU: the runtime's own (`--gpu`), or an option `cua spaces
        /// gpus` lists. macOS VMs: GPU acceleration (experimental, Apple
        /// silicon; https://cua.ai/docs/lume/guides/gpu-passthrough).
        #[arg(long, num_args = 0..=1, default_missing_value = "auto", value_name = "OPTION")]
        gpu: Option<String>,
    },
    /// Cancel a create that is still running (here, in the daemon, or in
    /// another terminal): it stops and what it made is removed. Finished
    /// image downloads stay cached. Ctrl-C during `cua spaces create` does
    /// the same.
    #[command(after_help = "Examples:
  cua spaces cancel local:space-1a2b3c
  cua spaces cancel studio")]
    Cancel {
        /// The id the Space will have (`local:<name>`, printed when the
        /// create starts) or its name.
        space: String,
    },
    /// The GPU options each runtime offers on this machine (or in the
    /// cloud).
    #[command(after_help = "Examples:
  cua spaces gpus
  cua spaces gpus --json")]
    Gpus {
        /// `local` (default) or `cloud`.
        #[arg(long)]
        on: Option<String>,
    },
    /// Delete a Space's sandbox (on this machine, on the machine that
    /// provides it, or in the cloud) and forget it (asks first).
    #[command(after_help = "Examples:
  cua spaces delete relay:space-0123456789abcdef
  cua spaces delete local:studio --force")]
    Delete {
        /// Space id or name.
        space: String,
        /// Skip the confirmation (required without a terminal).
        #[arg(long, short)]
        force: bool,
    },
    /// Turn a Space off: a local container or QEMU VM is suspended (its
    /// memory is kept), a Lume VM or a Space one of your machines provides
    /// is stopped (its disk is kept), and so is a Space in your own cloud
    /// (`--on aws`, `gcp`; compute billing stops). Fleet cloud Spaces,
    /// Modal sandboxes and Spaces added by address cannot be turned off.
    #[command(after_help = "Examples:
  cua spaces stop local:studio
  cua spaces stop relay:space-0123456789abcdef --json")]
    Stop {
        /// Space id or name.
        space: String,
    },
    /// Turn a Space back on: resume a suspended one or boot a stopped one.
    #[command(after_help = "Examples:
  cua spaces start local:studio
  cua spaces start studio --json")]
    Start {
        /// Space id or name.
        space: String,
    },
    /// Forget a registered Space and its stored token. The sandbox keeps
    /// running.
    #[command(
        visible_alias = "remove",
        after_help = "Examples:
  cua spaces rm direct:10.0.0.5:3211
  cua spaces rm studio"
    )]
    Rm {
        /// Space id, address or display name (as `cua spaces ls` shows).
        space: String,
    },
    /// Let another cua.ai account watch (viewer) or use (editor) a Space,
    /// through the relay. They open it as `relay:<machine>`.
    #[command(after_help = "Examples:
  cua spaces share local:studio bob@example.com
  cua spaces share local:studio bob@example.com --role editor
  cua spaces share relay:0123abcd4567ef89 acct-42 --role viewer")]
    Share {
        /// Space id, address or display name.
        space: String,
        /// Their email (verified by cua.ai) or account id.
        who: String,
        /// `viewer`: presence and a view-only stream. `editor`: full use.
        #[arg(long, default_value = "viewer", value_parser = ["viewer", "editor"])]
        role: String,
        /// Confirm without a prompt (needed without a terminal when no
        /// daemon runs; a running daemon asks with Touch ID or your
        /// password instead).
        #[arg(long)]
        yes: bool,
    },
    /// Stop sharing a Space with one account at once, or with everyone.
    #[command(after_help = "Examples:
  cua spaces unshare local:studio bob@example.com
  cua spaces unshare local:studio --all")]
    Unshare {
        /// Space id, address or display name.
        space: String,
        /// Their email or account id.
        #[arg(required_unless_present = "all", conflicts_with = "all")]
        who: Option<String>,
        /// Stop sharing with everyone (the Space leaves the relay).
        #[arg(long)]
        all: bool,
    },
    /// Who a Space is shared with, and who of them is connected now.
    #[command(after_help = "Examples:
  cua spaces shares local:studio
  cua spaces shares local:studio --json")]
    Shares {
        /// Space id, address or display name.
        space: String,
    },
    /// Publish a Space on the relay for your other devices (a phone off
    /// this network): its own driver dials out, as `cua host setup` does
    /// for this computer. Nobody else gets in until you share it.
    #[command(
        name = "relay-register",
        after_help = "Examples:
  cua spaces relay-register local:studio"
    )]
    RelayRegister {
        /// Space id, address or display name.
        space: String,
    },
    /// Take a Space off the relay (every share with it ends).
    #[command(
        name = "relay-unregister",
        after_help = "Examples:
  cua spaces relay-unregister local:studio"
    )]
    RelayUnregister {
        /// Space id, address or display name.
        space: String,
    },
}

/// Confirms a share on the terminal (the `cua` CLI without a daemon).
pub struct TerminalConsent {
    /// `--yes`: confirmed up front.
    pub yes: bool,
}

impl cua_spaces::share::ShareConsent for TerminalConsent {
    fn confirm(&self, reason: &str) -> std::result::Result<(), String> {
        use std::io::{BufRead as _, IsTerminal as _};
        if self.yes {
            return Ok(());
        }
        if !std::io::stdin().is_terminal() {
            return Err("no terminal to confirm on; pass --yes".into());
        }
        eprint!("{reason}? [y/N] ");
        let mut answer = String::new();
        std::io::stdin()
            .lock()
            .read_line(&mut answer)
            .map_err(|e| e.to_string())?;
        if matches!(answer.trim(), "y" | "Y" | "yes") {
            Ok(())
        } else {
            Err("you declined".into())
        }
    }
}

fn print_shares(s: &cua_sdk::SpaceShares, json: bool, out: &mut dyn Write) {
    if json {
        let shares: Vec<serde_json::Value> = s
            .shares
            .iter()
            .map(|e| serde_json::json!({"who": e.who, "role": e.role, "connected": e.connected}))
            .collect();
        line(
            out,
            serde_json::json!({
                "space": s.space, "machine": s.machine, "invitee_space": s.invitee_space,
                "url": s.url, "online": s.online, "shares": shares,
            })
            .to_string(),
        );
        return;
    }
    if s.machine.is_empty() || s.shares.is_empty() {
        line(out, format!("{} is not shared.", s.space));
        return;
    }
    line(
        out,
        format!("{} is shared as {}:", s.space, s.invitee_space),
    );
    for e in &s.shares {
        line(
            out,
            format!(
                "  {:<32} {:<7}{}",
                e.who,
                e.role,
                if e.connected { " connected" } else { "" }
            ),
        );
    }
}

fn space_json(s: &cua_sdk::SpaceInfo) -> serde_json::Value {
    serde_json::json!({
        "id": s.id,
        "name": s.name,
        "provider": s.provider,
        "spacesd_version": s.spacesd_version,
        "features": s.features,
        "os": s.os,
        "services": s.services,
        "added_at": s.added_at,
        "host": s.host,
        "host_name": s.host_name,
        "power": s.power,
        "power_state": s.power_state,
    })
}

/// What `cua spaces stop` / `start` did: the message, or one JSON line.
fn power_line(r: cua_sdk::SpacePowerReport, json: bool, out: &mut dyn Write) {
    if json {
        line(
            out,
            serde_json::json!({"space": r.space, "state": r.state, "power": r.power,
                "message": r.message})
            .to_string(),
        );
    } else {
        line(out, r.message);
    }
}

/// `cua spaces add` / `cua spaces rm`, through the SDK handle (the daemon
/// when one runs, else embedded), so they share the registry the daemon's
/// Spaces runtime and MCP use.
pub async fn run_spaces_sdk(
    cua: &Arc<cua_sdk::Cua>,
    cmd: SpacesCmd,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let spaces = cua.spaces();
    match cmd {
        SpacesCmd::Add {
            url,
            token,
            name,
            service,
            host: _,
        } => {
            let info = spaces
                .add_with_service(url, token.filter(|t| !t.is_empty()), name, service)
                .await?;
            if json {
                line(out, space_json(&info).to_string());
            } else {
                line(
                    out,
                    format!(
                        "Added {} ({}, {}{})",
                        info.id,
                        info.name,
                        info.provider,
                        if info.spacesd_version.is_empty() {
                            ", no cua-spacesd".to_string()
                        } else {
                            format!(", cua-spacesd {}", info.spacesd_version)
                        }
                    ),
                );
            }
        }
        SpacesCmd::Rm { space } => {
            let info = spaces.resolve(space.clone()).await?;
            spaces.remove(space).await?;
            if json {
                line(
                    out,
                    serde_json::json!({ "removed": space_json(&info) }).to_string(),
                );
            } else {
                line(out, format!("Removed {} ({})", info.id, info.name));
            }
        }
        SpacesCmd::Share {
            space, who, role, ..
        } => {
            let s = spaces.space(space).await?;
            print_shares(&s.share(who, Some(role)).await?, json, out);
        }
        SpacesCmd::Unshare { space, who, all } => {
            let s = spaces.space(space).await?;
            let who = if all { None } else { who };
            print_shares(&s.unshare(who).await?, json, out);
        }
        SpacesCmd::Shares { space } => {
            let s = spaces.space(space).await?;
            print_shares(&s.shares().await?, json, out);
        }
        SpacesCmd::RelayRegister { space } => {
            let r = spaces.relay_register(space).await?;
            if json {
                line(
                    out,
                    serde_json::json!({"space": r.space, "relay_space": r.relay_space,
                        "machine": r.machine, "url": r.url, "online": r.online})
                    .to_string(),
                );
            } else {
                line(
                    out,
                    format!(
                        "{} is on the relay as {}{}",
                        r.space,
                        r.relay_space,
                        if r.online { "" } else { " (not connected yet)" }
                    ),
                );
            }
        }
        SpacesCmd::RelayUnregister { space } => {
            let gone = spaces.relay_unregister(space.clone()).await?;
            if json {
                line(
                    out,
                    serde_json::json!({"space": space, "unregistered": gone}).to_string(),
                );
            } else if gone {
                line(out, format!("{space} is off the relay."));
            } else {
                line(out, format!("{space} was not on the relay."));
            }
        }
        SpacesCmd::Stop { space } => power_line(spaces.stop(space).await?, json, out),
        SpacesCmd::Start { space } => power_line(spaces.start(space).await?, json, out),
        SpacesCmd::Cancel { space } => {
            let o = spaces.cancel_create(space).await?;
            if json {
                line(
                    out,
                    serde_json::json!({"id": o.id, "state": o.state, "message": o.message})
                        .to_string(),
                );
            } else {
                line(out, o.message);
            }
        }
        SpacesCmd::Gpus { on } => {
            let all = spaces.gpu_support(on).await?;
            if json {
                let rows: Vec<serde_json::Value> = all
                    .iter()
                    .map(|g| {
                        serde_json::json!({
                            "runtime": g.runtime,
                            "reason": g.reason,
                            "options": g.options.iter().map(|o| serde_json::json!({
                                "id": o.id, "label": o.label, "experimental": o.experimental,
                                "supported": o.supported, "reason": o.reason,
                                "learn_more": o.learn_more, "usd_per_hour": o.usd_per_hour,
                            })).collect::<Vec<_>>(),
                        })
                    })
                    .collect();
                line(out, serde_json::json!({ "runtimes": rows }).to_string());
            } else {
                for g in &all {
                    if g.options.is_empty() {
                        line(out, format!("{}: none ({})", g.runtime, g.reason));
                    }
                    for o in &g.options {
                        let state = if o.supported {
                            "available".to_string()
                        } else {
                            format!("unavailable: {}", o.reason)
                        };
                        line(
                            out,
                            format!(
                                "{}: {} ({}{}), {state}",
                                g.runtime,
                                o.id,
                                o.label,
                                if o.experimental { ", experimental" } else { "" }
                            ),
                        );
                    }
                }
            }
        }
        SpacesCmd::Create {
            image,
            on,
            count,
            name,
            kind,
            runtime,
            cpus,
            memory_mb,
            disk_gb,
            gpu,
        } => {
            if !(1..=8).contains(&count) {
                return Err(CuaError::InvalidArgument(format!(
                    "--count {count}: create 1 to 8 Spaces at a time"
                )));
            }
            // A plain word that names no location is one of your machines.
            let on = on
                .map(|o| {
                    cua_sandbox_core::placement::On::parse_or_host(&o)
                        .map(|on| on.to_string())
                        .map_err(|e| CuaError::InvalidPlacement(e.message))
                })
                .transpose()?;
            // Each create has its own key, so Ctrl-C can cancel it.
            let run = std::process::id();
            let one = |n: u32, name: Option<String>| cua_sdk::SpaceCreateOptions {
                image: image.clone(),
                on: on.clone(),
                kind: kind.clone(),
                runtime: runtime.clone(),
                name,
                cpus,
                memory_mb,
                disk_gb,
                gpu: gpu.clone(),
                create_id: Some(format!("cli:{run}:{n}")),
                ..Default::default()
            };
            let names: Vec<Option<String>> = (1..=count)
                .map(|n| {
                    name.as_ref().map(|b| {
                        if n == 1 {
                            b.clone()
                        } else {
                            format!("{b}-{n}")
                        }
                    })
                })
                .collect();
            use std::io::IsTerminal as _;
            // What each create is doing, on one updating line of the
            // terminal (never in --json output).
            let show = !json && std::io::stderr().is_terminal() && count == 1;
            let creates = names.into_iter().enumerate().map(|(i, n)| {
                let options = one(i as u32 + 1, n);
                let spaces = spaces.clone();
                async move {
                    if show {
                        Box::pin(
                            spaces.create_with_progress(options, Arc::new(ProgressLine::default())),
                        )
                        .await
                    } else {
                        Box::pin(spaces.create(options)).await
                    }
                }
            });
            let mut all = Box::pin(futures_util::future::join_all(creates));
            let results = tokio::select! {
                r = &mut all => r,
                _ = tokio::signal::ctrl_c() => {
                    // Ctrl-C: stop every create and remove what it made.
                    eprintln!("\nCancelling...");
                    let mut messages = Vec::new();
                    for n in 1..=count {
                        let o = spaces.cancel_create(format!("cli:{run}:{n}")).await?;
                        if o.state != "not_creating" {
                            messages.push(o.message);
                        }
                    }
                    // The creates end with `Cancelled` once cleaned up.
                    let _ = tokio::time::timeout(std::time::Duration::from_secs(120), all).await;
                    let message = if messages.is_empty() {
                        "Cancelled.".to_string()
                    } else {
                        messages.join(" ")
                    };
                    return Err(CuaError::Cancelled(message));
                }
            };
            crate::util::exit_on_ctrl_c();
            if show {
                eprint!("\r\x1b[2K");
            }
            let mut failed = 0;
            let mut rows = Vec::new();
            for r in results {
                match r {
                    Ok(created) => {
                        let Some(info) = created.space else {
                            continue;
                        };
                        if !json {
                            line(
                                out,
                                format!(
                                    "Created {} ({}{})",
                                    info.id,
                                    info.name,
                                    if info.host_name.is_empty() {
                                        String::new()
                                    } else {
                                        format!(", on {}", info.host_name)
                                    }
                                ),
                            );
                        }
                        rows.push(space_json(&info));
                    }
                    Err(e) => {
                        failed += 1;
                        if !json {
                            eprintln!("cua: {e}");
                        }
                        rows.push(serde_json::json!({"error": e.to_string()}));
                    }
                }
            }
            if json {
                line(out, serde_json::json!({ "spaces": rows }).to_string());
            }
            if failed == count as usize {
                return Ok(1);
            }
        }
        SpacesCmd::Delete { space, force } => {
            use crate::sandbox::{DeleteConfirmation, confirm_delete};
            use std::io::IsTerminal as _;
            match confirm_delete(force, std::io::stdin().is_terminal(), || {
                crate::util::confirm(&format!("Delete {space} and everything in it?"), false)
            }) {
                DeleteConfirmation::Proceed => {}
                DeleteConfirmation::Declined => {
                    line(out, "Aborted.");
                    return Ok(0);
                }
                DeleteConfirmation::NeedsForce => {
                    return Err(crate::sandbox::needs_force(&format!("delete {space}")));
                }
            }
            let message = spaces.delete(space.clone()).await?;
            if json {
                line(
                    out,
                    serde_json::json!({"deleted": space, "message": message}).to_string(),
                );
            } else {
                line(out, message);
            }
        }
        SpacesCmd::Ls { .. } => {
            return Err(CuaError::Internal(
                "`cua spaces ls` runs over the registry directly".into(),
            ));
        }
    }
    Ok(0)
}

/// Account tokens for `cua host setup`: the `cua auth login` session when
/// this machine has one; otherwise a sign-in held in memory for this command
/// only and never stored, so a dedicated host keeps no account session (just
/// its machine token).
pub struct SetupTokens {
    stored: Arc<dyn AccountTokens>,
    login: Box<dyn Fn() -> LoginFuture + Send + Sync>,
    cached: tokio::sync::Mutex<Option<String>>,
    ephemeral: std::sync::atomic::AtomicBool,
}

/// A sign-in returning an access token.
pub type LoginFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = cua_host::Result<String>> + Send>>;

impl SetupTokens {
    /// `stored` first, else `login` (once per command).
    pub fn new(
        stored: Arc<dyn AccountTokens>,
        login: impl Fn() -> LoginFuture + Send + Sync + 'static,
    ) -> Self {
        Self {
            stored,
            login: Box::new(login),
            cached: tokio::sync::Mutex::new(None),
            ephemeral: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// The stored session, else an interactive sign-in that is not saved.
    pub fn session_or_sign_in() -> Self {
        Self::new(Arc::new(SessionTokens), || Box::pin(sign_in_for_setup()))
    }

    /// Whether the sign-in was held in memory only.
    pub fn used_ephemeral(&self) -> bool {
        self.ephemeral.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl AccountTokens for SetupTokens {
    async fn access_token(&self) -> cua_host::Result<String> {
        let mut cached = self.cached.lock().await;
        if let Some(t) = cached.as_ref() {
            return Ok(t.clone());
        }
        match self.stored.access_token().await {
            Ok(t) => Ok(t),
            Err(cua_host::Error::Unauthenticated(_)) => {
                let t = (self.login)().await?;
                self.ephemeral
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                *cached = Some(t.clone());
                Ok(t)
            }
            Err(e) => Err(e),
        }
    }
}

/// Signs in for `cua host setup` without storing the session.
async fn sign_in_for_setup() -> cua_host::Result<String> {
    let unauth = |e: cua_auth::Error| cua_host::Error::Unauthenticated(e.to_string());
    let pending = auth::session()
        .begin_login(cua_auth::Flow::Auto)
        .await
        .map_err(unauth)?;
    eprintln!("Sign in to cua.ai to register this machine (the session is not stored here):");
    eprintln!("  {}", pending.url);
    if let Some(code) = &pending.user_code {
        eprintln!("Enter this code if prompted: {code}");
    }
    let creds = pending.complete().await.map_err(unauth)?;
    Ok(creds.access_token)
}

/// The `cua auth login` session as account tokens (refreshed on use).
pub struct SessionTokens;

#[async_trait::async_trait]
impl AccountTokens for SessionTokens {
    async fn access_token(&self) -> cua_host::Result<String> {
        match auth::session_token(&auth::Store::from_env()).await {
            Ok(Some(t)) => Ok(t),
            Ok(None) => Err(cua_host::Error::Unauthenticated(
                "not signed in to cua.ai; run `cua auth login` first".into(),
            )),
            Err(e) => Err(cua_host::Error::Unauthenticated(e.to_string())),
        }
    }
}

pub fn host_err(e: cua_host::Error) -> CuaError {
    use cua_host::Error as E;
    let m = e.to_string();
    match e {
        E::InvalidArgument(_) | E::Conflict(_) => CuaError::InvalidArgument(m),
        E::Unauthenticated(_) => CuaError::Unauthenticated(m),
        E::PermissionDenied(_) => CuaError::PermissionDenied(m),
        E::NotFound(_) => CuaError::NotFound(m),
        E::Relay(_) | E::Download(_) => CuaError::Http(m),
        E::Service(_) => CuaError::Runtime(m),
        E::Io(_) | E::Internal(_) => CuaError::Internal(m),
    }
}

fn render(s: &HostStatus, out: &mut dyn Write) {
    if !s.configured {
        line(
            out,
            "This machine is not set up for access (run `cua host setup`).",
        );
        return;
    }
    let name = s.name.clone().unwrap_or_default();
    match s.mode.as_deref() {
        Some("relay") => line(
            out,
            format!(
                "{name}: relay {} as machine {}",
                s.relay_url.clone().unwrap_or_default(),
                s.machine_id.clone().unwrap_or_default()
            ),
        ),
        _ => line(
            out,
            format!(
                "{name}: direct {} (env token in {})",
                s.direct_url.clone().unwrap_or_default(),
                s.env_token_path.clone().unwrap_or_default()
            ),
        ),
    }
    line(
        out,
        format!(
            "  service: {} {}{}",
            s.service.kind,
            if s.service.running {
                "running"
            } else if s.service.installed {
                "installed, not running"
            } else {
                "not installed"
            },
            if s.service.detail.is_empty() {
                String::new()
            } else {
                format!(" ({})", s.service.detail)
            }
        ),
    );
    line(
        out,
        format!(
            "  sharing: {}{}",
            if s.sharing { "on" } else { "off" },
            match s.online {
                Some(true) => ", online",
                Some(false) => ", offline",
                None => "",
            }
        ),
    );
    render_settings(s, out);
    if !s.allow.is_empty() {
        line(out, format!("  also allowed: {}", s.allow.join(", ")));
    }
    if s.clients.is_empty() {
        if s.mode.as_deref() == Some("relay") {
            line(out, "  connected: nobody");
        }
    } else {
        for c in &s.clients {
            let who = c.name.clone().or(c.email.clone()).unwrap_or(c.id.clone());
            line(out, format!("  connected: {who} ({} stream(s))", c.streams));
        }
    }
    if let Some(e) = &s.error {
        line(out, format!("  relay: {e}"));
    }
    render_access(s, now_ms(), out);
    render_provided(s, now_ms(), out);
    if !s.permissions.is_empty() && s.share_desktop {
        line(
            out,
            "  macOS permissions (grant them yourself; Cua never changes them):",
        );
        for p in &s.permissions {
            line(out, format!("    - {}: {}", p.title, p.instructions));
            line(out, format!("      open {}", p.settings_url));
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn ago(now_ms: u64, at_ms: u64) -> String {
    let s = now_ms.saturating_sub(at_ms) / 1000;
    match s {
        0..=59 => "just now".into(),
        60..=3599 => format!("{}m ago", s / 60),
        3600..=86_399 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86_400),
    }
}

/// The two settings: the desktop, and Spaces for your other devices.
fn render_settings(s: &HostStatus, out: &mut dyn Write) {
    line(
        out,
        format!(
            "  desktop: {}",
            if s.share_desktop {
                "shared (this machine is a Space)"
            } else {
                "not shared (its screen, input, shell and files are off limits)"
            }
        ),
    );
    if s.provide_spaces {
        let limit = |n: u32| {
            if n == 0 {
                "no limit".to_string()
            } else {
                format!("at most {n}")
            }
        };
        line(
            out,
            format!(
                "  provides Spaces: on ({} now, {}{})",
                s.provided_spaces.len(),
                limit(s.max_spaces),
                if cfg!(target_os = "macos") {
                    format!("; macOS VMs {}", limit(s.max_macos_vms))
                } else {
                    String::new()
                }
            ),
        );
    } else {
        line(out, "  provides Spaces: off");
    }
}

/// The Spaces this machine provides and the audit of remote creates and
/// deletes.
fn render_provided(s: &HostStatus, now_ms: u64, out: &mut dyn Write) {
    for p in &s.provided_spaces {
        if p.is_direct() {
            line(
                out,
                format!(
                    "    {} on port {} ({}, {}) for {}",
                    p.local_space, p.direct_port, p.image, p.kind, p.created_by
                ),
            );
            continue;
        }
        line(
            out,
            format!(
                "    relay:{} {} ({}, {}) for {}",
                p.relay_machine, p.local_space, p.image, p.kind, p.created_by
            ),
        );
    }
    if let Some(e) = &s.spaces_audit_error {
        line(
            out,
            format!("  spaces audit: does not verify ({e}); it may have been altered"),
        );
    }
    if s.spaces_audit.is_empty() {
        return;
    }
    line(out, "  spaces audit:");
    for r in s.spaces_audit.iter().take(8) {
        line(
            out,
            format!(
                "    {:<9} {} {} {}{}",
                ago(now_ms, r.at_ms),
                r.who,
                r.action,
                r.space,
                if r.detail.is_empty() {
                    String::new()
                } else {
                    format!(": {}", r.detail)
                }
            ),
        );
    }
}

/// Who reached this machine recently (the driver's access log).
fn render_access(s: &HostStatus, now_ms: u64, out: &mut dyn Write) {
    if let Some(e) = &s.access_log_error {
        line(
            out,
            format!("  access log: does not verify ({e}); it may have been altered"),
        );
    }
    if s.recent_access.is_empty() {
        return;
    }
    line(out, "  recent access:");
    for r in s.recent_access.iter().take(8) {
        line(
            out,
            format!(
                "    {:<9} {} via {}: {}",
                ago(now_ms, r.at_ms),
                r.who,
                r.via,
                r.what
            ),
        );
    }
}

fn emit(s: &HostStatus, json: bool, out: &mut dyn Write) -> Result<(), CuaError> {
    if json {
        line(
            out,
            serde_json::to_string(s).map_err(|e| CuaError::Internal(e.to_string()))?,
        );
    } else {
        render(s, out);
    }
    Ok(())
}

/// Runs a host command against `home` with `tokens` (`manager` overrides the
/// OS service runner; tests pass a fake).
pub async fn run_host(
    cmd: HostCmd,
    home: &Path,
    tokens: &dyn AccountTokens,
    device: Option<&DeviceAuth>,
    manager: Option<Arc<dyn ServiceManager>>,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let mut host = Host::new(home);
    if let Some(m) = manager {
        host = host.with_service_manager(m);
    }
    match cmd {
        HostCmd::Setup {
            relay,
            direct,
            name,
            allow,
            driver_bin,
            runner,
            profile,
            desktop,
            no_desktop,
            provide_spaces,
            no_provide_spaces,
            max_spaces,
            allow_any_address,
        } => {
            let mut opts = match direct {
                Some(addr) => SetupOptions::direct(addr.parse().map_err(|_| {
                    CuaError::InvalidArgument(format!("--direct {addr:?} is not ip:port"))
                })?),
                None => SetupOptions::relay(relay.unwrap_or_else(cua_host::relay_url_from_env)),
            };
            opts.name = name;
            opts.allow = allow;
            opts.driver_bin = driver_bin;
            opts.runner = RunnerKind::parse(&runner).map_err(host_err)?;
            opts = opts.profile(cua_host::HostProfile::parse(&profile).map_err(host_err)?);
            if desktop {
                opts.share_desktop = true;
            }
            if no_desktop {
                opts.share_desktop = false;
            }
            if provide_spaces {
                opts.provide_spaces = true;
            }
            if no_provide_spaces {
                opts.provide_spaces = false;
            }
            opts.max_spaces = max_spaces;
            opts.allow_any_address = allow_any_address;
            let s = host.setup(opts, tokens).await.map_err(host_err)?;
            emit(&s, json, out)?;
            if allow_any_address && s.provide_spaces {
                eprintln!(
                    "warning: --allow-any-address: host calls are accepted from any address; {}.",
                    cua_host::direct::PLAINTEXT_WARNING
                );
            }
            // Direct mode: how a laptop adds this machine (with the token,
            // so to stderr when the output is JSON for a program).
            if let Some(pair) = host.pairing_command().map_err(host_err)? {
                let lines = [
                    "On your laptop, add this machine (the token is a shell on this machine: \
                     keep it private):"
                        .to_string(),
                    format!("  {pair}"),
                ];
                for l in lines {
                    if json {
                        eprintln!("{l}");
                    } else {
                        line(out, l);
                    }
                }
            }
        }
        HostCmd::Status => {
            let s = host.status().await.map_err(host_err)?;
            emit(&s, json, out)?;
            if !s.configured {
                return Ok(1);
            }
        }
        HostCmd::Config {
            desktop,
            provide_spaces,
            max_spaces,
            max_macos_vms,
        } => {
            let on = |v: Option<String>| v.map(|v| v == "on");
            let change = cua_host::HostSettingsChange {
                share_desktop: on(desktop),
                provide_spaces: on(provide_spaces),
                max_spaces,
                max_macos_vms,
                cua_bin: None,
            };
            let s = if change == cua_host::HostSettingsChange::default() {
                host.status().await.map_err(host_err)?
            } else {
                host.configure(change).await.map_err(host_err)?
            };
            if json {
                emit(&s, true, out)?;
            } else if !s.configured {
                render(&s, out);
                return Ok(1);
            } else {
                render_settings(&s, out);
                render_provided(&s, now_ms(), out);
            }
        }
        HostCmd::Stop => emit(&host.stop_sharing().await.map_err(host_err)?, json, out)?,
        HostCmd::Start => emit(&host.start_sharing().await.map_err(host_err)?, json, out)?,
        HostCmd::Share {
            who,
            machine,
            relay,
            yes,
        } => {
            // Sharing widens who reaches the machine: ask.
            if !yes
                && !(crate::util::interactive()
                    && crate::util::confirm(
                        &format!("Let {} reach this machine?", who.join(", ")),
                        false,
                    ))
            {
                return Err(CuaError::PermissionDenied(
                    "not shared (pass --yes to share without a prompt)".into(),
                ));
            }
            let allow = change_allow(&host, home, tokens, device, machine, relay, &who, true)
                .await
                .map_err(host_err)?;
            print_allow(&allow, json, out);
        }
        HostCmd::Unshare {
            who,
            machine,
            relay,
        } => {
            let allow = change_allow(&host, home, tokens, device, machine, relay, &who, false)
                .await
                .map_err(host_err)?;
            print_allow(&allow, json, out);
        }
        HostCmd::Remove { force } => {
            use crate::sandbox::{DeleteConfirmation, confirm_delete};
            use std::io::IsTerminal as _;
            match confirm_delete(force, std::io::stdin().is_terminal(), || {
                crate::util::confirm(
                    "Remove this machine's access setup (unregister it and delete its tokens)?",
                    false,
                )
            }) {
                DeleteConfirmation::Proceed => {}
                DeleteConfirmation::Declined => {
                    line(out, "Aborted.");
                    return Ok(0);
                }
                DeleteConfirmation::NeedsForce => {
                    return Err(crate::sandbox::needs_force(
                        "remove this machine's access setup",
                    ));
                }
            }
            host.remove().await.map_err(host_err)?;
            if json {
                line(out, r#"{"removed":true}"#);
            } else {
                line(out, "This machine is no longer set up for access.");
            }
        }
    }
    Ok(0)
}

fn print_allow(allow: &[String], json: bool, out: &mut dyn Write) {
    if json {
        line(out, serde_json::json!({ "allow": allow }).to_string());
    } else if allow.is_empty() {
        line(out, "Shared with: nobody besides the owner.");
    } else {
        line(out, format!("Shared with: {}", allow.join(", ")));
    }
}

/// Adds (`add`) or removes `who` on the allowlist of `machine` (default:
/// this machine) through the relay, as the account from this device, and
/// mirrors it into this machine's driver policy when it is the target.
#[allow(clippy::too_many_arguments)]
async fn change_allow(
    host: &Host,
    home: &Path,
    tokens: &dyn AccountTokens,
    device: Option<&DeviceAuth>,
    machine: Option<String>,
    relay: Option<String>,
    who: &[String],
    add: bool,
) -> cua_host::Result<Vec<String>> {
    let config = host.config()?;
    let local_id = config.as_ref().and_then(|c| c.machine_id.clone());
    let id = machine.or(local_id.clone()).ok_or_else(|| {
        cua_host::Error::InvalidArgument(
            "this machine is not on a relay; pass --machine <id> (`cua spaces ls`)".into(),
        )
    })?;
    let url = relay
        .or_else(|| config.as_ref().and_then(|c| c.relay_url.clone()))
        .unwrap_or_else(|| relay_url(None, home));
    let session = match device {
        Some(d) => d.try_session().await,
        None => None,
    };
    let client = RelayClient::new(&url)?.with_device_session(session);
    let token = tokens.access_token().await?;
    let current = client.machine(&token, &id).await?;
    let mut allow = current.allow.clone();
    for w in who {
        let w = w.trim().to_string();
        if add {
            if !allow.iter().any(|a| a.eq_ignore_ascii_case(&w)) {
                allow.push(w);
            }
        } else {
            allow.retain(|a| !a.eq_ignore_ascii_case(&w));
        }
    }
    let updated = client
        .patch(
            &token,
            &id,
            &MachinePatch {
                allow: Some(allow),
                ..Default::default()
            },
        )
        .await?;
    if local_id.as_deref() == Some(id.as_str()) {
        host.sync_policy_allow(&updated.allow)?;
    }
    Ok(updated.allow)
}

/// The relay account `cua daemon` serves Spaces with: the `cua auth login`
/// session (read per call, so signing in later works without a restart) on
/// the configured relay, and this device's enrollment.
pub fn daemon_relay_account(home: &Path) -> cua_spaces::RelayAccount {
    let url = relay_url(None, home);
    let account = cua_spaces::RelayAccount::new(url.clone(), Arc::new(SessionTokens));
    match crate::devices_cmd::device_auth(&url) {
        Ok(device) => account.with_device(device),
        Err(_) => account,
    }
}

/// Gives an embedded `cua` (no daemon running, or `--embedded`) the
/// account's relay machines, as `cua daemon` does for its own runtime:
/// without it `cua mcp` lists no relay machines and refuses `relay:<id>`
/// Spaces. A daemon-backed `cua` is left alone (the daemon lists them), as
/// is a runtime that already has an account. `CUA_DAEMON_NO_RELAY=1` turns
/// it off here too. Returns whether an account was attached.
pub fn attach_relay_account(
    cua: &cua_sdk::Cua,
    account: impl FnOnce() -> cua_spaces::RelayAccount,
) -> bool {
    let Some(runtime) = cua.embedded_runtime() else {
        return false;
    };
    if std::env::var_os("CUA_DAEMON_NO_RELAY").is_some()
        || runtime.spaces().relay_account().is_some()
    {
        return false;
    }
    runtime.spaces().set_relay(Some(account()));
    true
}

/// `--relay`, else `CUA_RELAY_URL`, else the relay this machine is set up
/// with, else the default relay.
pub fn relay_url(flag: Option<String>, home: &Path) -> String {
    flag.filter(|u| !u.trim().is_empty())
        .or_else(|| {
            std::env::var("CUA_RELAY_URL")
                .ok()
                .filter(|u| !u.trim().is_empty())
        })
        .or_else(|| Host::new(home).config().ok().flatten()?.relay_url)
        .unwrap_or_else(cua_host::relay_url_from_env)
}

/// `cua spaces ls` rows: every Space, with the Spaces a machine provides
/// indented under it (or, when that machine is not listed, marked with its
/// name).
fn grouped_lines(list: &[cua_spaces::SpaceInfo]) -> Vec<String> {
    let row = |s: &cua_spaces::SpaceInfo, indent: &str| {
        let line = format!(
            "{indent}{:<width$} {:<24} {:<7} {}",
            s.id,
            s.name,
            s.provider.as_str(),
            s.spacesd_version,
            width = 48usize.saturating_sub(indent.len()),
        );
        // A Space turned off says so (`cua spaces start` turns it on).
        match s.power_state.as_str() {
            "suspended" | "stopped" => format!("{} ({})", line.trim_end(), s.power_state),
            _ => line,
        }
    };
    let mut out = Vec::new();
    for s in list.iter().filter(|s| s.host.is_empty()) {
        // A Space in your cloud names its cloud ("AWS · us-west-2").
        if s.host_name.is_empty() {
            out.push(row(s, ""));
        } else {
            out.push(format!("{} (on {})", row(s, ""), s.host_name));
        }
        // A relay host's Spaces name its machine id; a direct host's name
        // its Space id.
        let machine = s.id.strip_prefix("relay:").unwrap_or("");
        for c in list.iter().filter(|c| {
            !c.host.is_empty() && ((!machine.is_empty() && c.host == machine) || c.host == s.id)
        }) {
            out.push(row(c, "  "));
        }
    }
    for c in list.iter().filter(|c| {
        !c.host.is_empty()
            && !list
                .iter()
                .any(|s| s.id == format!("relay:{}", c.host) || s.id == c.host)
    }) {
        out.push(format!(
            "{} (on {})",
            row(c, ""),
            if c.host_name.is_empty() {
                &c.host
            } else {
                &c.host_name
            }
        ));
    }
    out
}

/// `cua spaces add <addr> --host`: registers a machine that provides
/// Spaces over its direct address, in this cua home's registry (the one the
/// daemon and `cua mcp` use).
pub async fn run_add_host(
    cmd: SpacesCmd,
    home: &Path,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let SpacesCmd::Add {
        url, token, name, ..
    } = cmd
    else {
        return Err(CuaError::Internal(
            "only `cua spaces add --host` adds a host".into(),
        ));
    };
    let spaces = cua_spaces::Spaces::builder().home(home).build();
    let info = spaces
        .add_direct_host(&url, token.filter(|t| !t.is_empty()), name)
        .await
        .map_err(CuaError::from)?;
    let host = spaces
        .registry()
        .direct_hosts()
        .map_err(CuaError::from)?
        .into_iter()
        .find(|h| h.space == info.id)
        .map(|h| h.name)
        .unwrap_or_else(|| info.name.clone());
    if json {
        line(
            out,
            serde_json::json!({ "host": host, "space": info.id, "via": "direct" }).to_string(),
        );
    } else {
        line(
            out,
            format!(
                "Added host {host:?} ({}, direct). Create Spaces on it with `cua spaces create --on \"host:{host}\"`.",
                info.id
            ),
        );
    }
    Ok(0)
}

/// `cua spaces ls`.
pub async fn run_spaces(
    cmd: SpacesCmd,
    home: &Path,
    tokens: Option<Arc<dyn AccountTokens>>,
    device: Option<Arc<DeviceAuth>>,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let SpacesCmd::Ls { relay } = cmd else {
        return Err(CuaError::Internal(
            "only `cua spaces ls` runs over the registry directly".into(),
        ));
    };
    let mut b = cua_spaces::Spaces::builder().home(home);
    if let Some(t) = tokens {
        let mut account = cua_spaces::RelayAccount::new(relay_url(relay, home), t);
        if let Some(d) = device {
            account = account.with_device(d);
        }
        b = b.relay(account);
    }
    let spaces = b.build();
    let mut relay_error = None;
    if spaces.relay_account().is_some()
        && let Err(e) = spaces.relay_machines().await
    {
        relay_error = Some(e.to_string());
    }
    let list = spaces
        .list()
        .map_err(|e| CuaError::Internal(e.to_string()))?;
    if json {
        line(
            out,
            serde_json::json!({ "spaces": list, "relay_error": relay_error }).to_string(),
        );
        return Ok(0);
    }
    if list.is_empty() {
        line(out, "No Spaces.");
    }
    for l in grouped_lines(&list) {
        line(out, l);
    }
    if let Some(e) = relay_error {
        line(out, format!("(relay machines unavailable: {e})"));
    }
    Ok(0)
}

/// Prints a create's progress on one updating line of stderr:
/// "Downloading image 4.2 of 22.1 GB · 85 MB/s · about 4 min".
#[derive(Default)]
struct ProgressLine {
    last: std::sync::Mutex<String>,
}

impl cua_sdk::SpaceCreateListener for ProgressLine {
    fn on_progress(&self, p: cua_sdk::SpaceCreateProgress) {
        let words = match p.phase.as_str() {
            "preparing" => "Preparing",
            "pulling" => "Downloading image",
            "creating" => "Creating",
            "booting" => "Booting",
            "waiting_for_services" => "Starting services",
            "connecting" => "Connecting",
            _ => return,
        };
        let detail = match (p.bytes_done, p.bytes_total) {
            (Some(done), total) => cua_sandbox_core::progress::Transfer {
                done,
                total: total.unwrap_or(0),
                per_second: p.bytes_per_second,
            }
            .describe(),
            _ => p
                .fraction
                .map(|f| format!("{:.0}%", f * 100.0))
                .unwrap_or_default(),
        };
        let text = if p.space.is_empty() {
            format!("{words} {detail}")
        } else {
            format!("{}: {words} {detail}", p.space)
        };
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        if *last != text {
            eprint!("\r\x1b[2K{}", text.trim_end());
            *last = text;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_host::StaticToken;
    use cua_host::service::FakeServiceManager;
    use cua_host::testing::FakeRelay;

    fn parse(args: &[&str]) -> HostCmd {
        #[derive(clap::Parser)]
        struct W {
            #[command(subcommand)]
            c: HostCmd,
        }
        <W as clap::Parser>::try_parse_from(std::iter::once("host").chain(args.iter().copied()))
            .unwrap()
            .c
    }

    fn parse_spaces(args: &[&str]) -> Result<SpacesCmd, clap::Error> {
        #[derive(clap::Parser)]
        struct W {
            #[command(subcommand)]
            c: SpacesCmd,
        }
        <W as clap::Parser>::try_parse_from(std::iter::once("spaces").chain(args.iter().copied()))
            .map(|w| w.c)
    }

    #[test]
    fn spare_machines_and_spaces_on_them_parse() {
        match parse(&["setup", "--profile", "spare", "--max-spaces", "6"]) {
            HostCmd::Setup {
                profile,
                max_spaces,
                ..
            } => assert_eq!((profile.as_str(), max_spaces), ("spare", Some(6))),
            other => panic!("{other:?}"),
        }
        match parse(&["setup", "--no-desktop", "--provide-spaces"]) {
            HostCmd::Setup {
                profile,
                no_desktop,
                provide_spaces,
                ..
            } => assert_eq!(
                (profile.as_str(), no_desktop, provide_spaces),
                ("desktop", true, true)
            ),
            other => panic!("{other:?}"),
        }
        match parse(&["config", "--desktop", "off", "--provide-spaces", "on"]) {
            HostCmd::Config {
                desktop,
                provide_spaces,
                ..
            } => assert_eq!(
                (desktop.as_deref(), provide_spaces.as_deref()),
                (Some("off"), Some("on"))
            ),
            other => panic!("{other:?}"),
        }
        match parse_spaces(&[
            "create",
            "macos:26",
            "--on",
            "spare mac mini",
            "--count",
            "2",
        ])
        .unwrap()
        {
            SpacesCmd::Create {
                image, on, count, ..
            } => assert_eq!(
                (image.as_deref(), on.as_deref(), count),
                (Some("macos:26"), Some("spare mac mini"), 2)
            ),
            other => panic!("{other:?}"),
        }
        assert!(parse_spaces(&["delete", "relay:space-1", "--force"]).is_ok());
        // A direct host: added with --host, set up with --direct.
        match parse_spaces(&["add", "100.64.0.9:3211", "--host", "--token", "t"]).unwrap() {
            SpacesCmd::Add { host, token, .. } => {
                assert!(host);
                assert_eq!(token.as_deref(), Some("t"));
            }
            other => panic!("{other:?}"),
        }
        assert!(parse_spaces(&["add", "h:1", "--host", "--service", "x"]).is_err());
        match parse(&[
            "setup",
            "--direct",
            "0.0.0.0:3211",
            "--profile",
            "spare",
            "--allow-any-address",
        ]) {
            HostCmd::Setup {
                direct,
                allow_any_address,
                ..
            } => assert_eq!(
                (direct.as_deref(), allow_any_address),
                (Some("0.0.0.0:3211"), true)
            ),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn spaces_on_a_host_list_under_it() {
        let space = |id: &str, host: &str| cua_spaces::SpaceInfo {
            id: id.into(),
            name: id.into(),
            provider: cua_spaces::Provider::Relay,
            spacesd_version: String::new(),
            features: vec![],
            os: String::new(),
            os_name: String::new(),
            os_pretty_name: String::new(),
            image: String::new(),
            image_digest: String::new(),
            kind: String::new(),
            arch: String::new(),
            services: vec![],
            added_at: None,
            host: host.into(),
            host_name: if host.is_empty() {
                String::new()
            } else {
                "Mac mini (spare)".into()
            },
            power: String::new(),
            power_state: String::new(),
            cloud: String::new(),
            cloud_place: String::new(),
            cloud_delete: String::new(),
        };
        let lines = grouped_lines(&[
            space("relay:space-1", "mini1234"),
            space("relay:studio99", ""),
            space("relay:mini1234", ""),
            space("relay:space-2", "gone5678"),
        ]);
        assert!(lines[0].starts_with("relay:studio99"), "{lines:?}");
        assert!(lines[1].starts_with("relay:mini1234"), "{lines:?}");
        assert!(lines[2].starts_with("  relay:space-1"), "{lines:?}");
        assert!(
            lines[3].starts_with("relay:space-2") && lines[3].ends_with("(on Mac mini (spare))"),
            "{lines:?}"
        );
        // A direct host's Spaces name its Space id.
        let direct = |id: &str, host: &str| {
            let mut s = space(id, host);
            s.provider = cua_spaces::Provider::Direct;
            s
        };
        let lines = grouped_lines(&[
            direct("direct:100.64.0.9:41001", "direct:100.64.0.9:3211"),
            direct("direct:100.64.0.9:3211", ""),
        ]);
        assert!(lines[0].starts_with("direct:100.64.0.9:3211"), "{lines:?}");
        assert!(
            lines[1].starts_with("  direct:100.64.0.9:41001"),
            "{lines:?}"
        );
        assert_eq!(lines.len(), 2, "{lines:?}");
    }

    #[test]
    fn spaces_share_parses_roles_and_unshare_needs_who_or_all() {
        match parse_spaces(&["share", "local:studio", "bob@example.com"]).unwrap() {
            SpacesCmd::Share { role, yes, .. } => {
                assert_eq!((role.as_str(), yes), ("viewer", false))
            }
            other => panic!("{other:?}"),
        }
        match parse_spaces(&["share", "s", "bob@example.com", "--role", "editor", "--yes"]).unwrap()
        {
            SpacesCmd::Share { role, yes, .. } => {
                assert_eq!((role.as_str(), yes), ("editor", true))
            }
            other => panic!("{other:?}"),
        }
        assert!(parse_spaces(&["share", "s", "bob@example.com", "--role", "owner"]).is_err());
        assert!(parse_spaces(&["unshare", "s"]).is_err(), "who or --all");
        assert!(parse_spaces(&["unshare", "s", "bob@example.com", "--all"]).is_err());
        assert!(matches!(
            parse_spaces(&["unshare", "s", "--all"]).unwrap(),
            SpacesCmd::Unshare {
                all: true,
                who: None,
                ..
            }
        ));
        use cua_spaces::share::ShareConsent as _;
        // Without a terminal (as under `cargo test`) it refuses rather
        // than saying yes on the user's behalf.
        if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
            assert_eq!(
                TerminalConsent { yes: false }.confirm("x").unwrap_err(),
                "no terminal to confirm on; pass --yes"
            );
        }
        assert!(TerminalConsent { yes: true }.confirm("x").is_ok());
    }

    #[test]
    fn shares_print_one_line_per_account() {
        let s = cua_sdk::SpaceShares {
            space: "local:studio".into(),
            machine: "space-00000000000000aa".into(),
            invitee_space: "relay:space-00000000000000aa".into(),
            url: String::new(),
            online: true,
            shares: vec![
                cua_sdk::SpaceShareEntry {
                    who: "bob@example.com".into(),
                    role: "editor".into(),
                    connected: true,
                },
                cua_sdk::SpaceShareEntry {
                    who: "carol@example.com".into(),
                    role: "viewer".into(),
                    connected: false,
                },
            ],
        };
        let mut out = Vec::new();
        print_shares(&s, false, &mut out);
        let text = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines[0],
            "local:studio is shared as relay:space-00000000000000aa:"
        );
        assert!(
            lines[1].contains("bob@example.com") && lines[1].ends_with("editor  connected"),
            "{text}"
        );
        assert!(lines[2].trim_end().ends_with("viewer"), "{text}");
        let mut out = Vec::new();
        print_shares(
            &cua_sdk::SpaceShares {
                shares: vec![],
                ..s
            },
            false,
            &mut out,
        );
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "local:studio is not shared.\n"
        );
    }

    #[test]
    fn status_lists_recent_access_and_flags_an_altered_log() {
        let s = HostStatus {
            recent_access: vec![cua_host::access::AccessRecord {
                at_ms: 1_000_000 - 120_000,
                via: "relay".into(),
                who: "Ada (acct-1)".into(),
                what: "ProcessService".into(),
            }],
            access_log_error: Some("line 3: altered".into()),
            ..HostStatus::default()
        };
        let mut out = Vec::new();
        render_access(&s, 1_000_000, &mut out);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("does not verify"), "{text}");
        assert!(
            text.contains("2m ago    Ada (acct-1) via relay: ProcessService"),
            "{text}"
        );
    }

    #[tokio::test]
    async fn host_setup_status_stop_remove_against_a_fake_relay() {
        let relay = FakeRelay::start().await;
        relay.add_account("acct", "user-1", Some("ada@example.com"));
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join(".cua");
        let driver = dir.path().join("drv");
        std::fs::write(&driver, b"x").unwrap();
        let manager = Arc::new(FakeServiceManager::default());
        let tokens = StaticToken("acct".into());
        let mut out = Vec::new();
        let code = run_host(
            parse(&[
                "setup",
                "--relay",
                &relay.url,
                "--name",
                "lab-box",
                "--allow",
                "friend@example.com",
                "--driver-bin",
                driver.to_str().unwrap(),
            ]),
            &home,
            &tokens,
            None,
            Some(manager.clone()),
            false,
            &mut out,
        )
        .await
        .unwrap();
        assert_eq!(code, 0);
        let text = String::from_utf8(out).unwrap();
        assert!(text.starts_with("lab-box: relay "), "{text}");
        assert!(text.contains("sharing: on, offline"), "{text}");
        assert!(text.contains("also allowed: friend@example.com"), "{text}");

        let mut out = Vec::new();
        run_host(
            parse(&["status"]),
            &home,
            &tokens,
            None,
            Some(manager.clone()),
            true,
            &mut out,
        )
        .await
        .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["configured"], true);
        assert_eq!(v["mode"], "relay");
        let id = v["machineId"].as_str().unwrap().to_string();

        // `cua spaces ls` as the owner shows the machine.
        let mut out = Vec::new();
        // No --relay: the relay this machine joined.
        run_spaces(
            SpacesCmd::Ls { relay: None },
            &home,
            Some(Arc::new(StaticToken("acct".into()))),
            None,
            false,
            &mut out,
        )
        .await
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains(&format!("relay:{id}")), "{text}");
        assert!(text.contains("lab-box") && text.contains("relay"), "{text}");

        let mut out = Vec::new();
        run_host(
            parse(&["stop"]),
            &home,
            &tokens,
            None,
            Some(manager.clone()),
            false,
            &mut out,
        )
        .await
        .unwrap();
        assert!(String::from_utf8(out).unwrap().contains("sharing: off"));
        assert!(!relay.machine(&id).unwrap().sharing);

        let mut out = Vec::new();
        run_host(
            parse(&["remove", "--force"]),
            &home,
            &tokens,
            None,
            Some(manager.clone()),
            false,
            &mut out,
        )
        .await
        .unwrap();
        assert!(relay.machine(&id).is_none());
        let mut out = Vec::new();
        let code = run_host(
            parse(&["status"]),
            &home,
            &tokens,
            None,
            Some(manager),
            false,
            &mut out,
        )
        .await
        .unwrap();
        assert_eq!(code, 1);
    }

    /// `cua host setup` on a machine with no stored session signs in for
    /// the command only: the relay registration works and no account
    /// session is written anywhere under the cua home.
    #[tokio::test]
    async fn setup_without_a_stored_session_signs_in_in_memory_only() {
        let relay = FakeRelay::start().await;
        relay.add_account("fresh-login-token", "user-1", None);
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join(".cua");
        let driver = dir.path().join("drv");
        std::fs::write(&driver, b"x").unwrap();
        let logins = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let counter = logins.clone();
        let tokens = SetupTokens::new(Arc::new(cua_host::NoAccount), move || {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async { Ok("fresh-login-token".to_string()) })
        });
        let mut out = Vec::new();
        run_host(
            parse(&[
                "setup",
                "--relay",
                &relay.url,
                "--driver-bin",
                driver.to_str().unwrap(),
            ]),
            &home,
            &tokens,
            None,
            Some(Arc::new(FakeServiceManager::default())),
            true,
            &mut out,
        )
        .await
        .unwrap();
        assert!(tokens.used_ephemeral());
        assert_eq!(logins.load(std::sync::atomic::Ordering::SeqCst), 1);
        let mut stack = vec![home.clone()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                let text = String::from_utf8_lossy(&std::fs::read(&p).unwrap()).into_owned();
                assert!(!text.contains("fresh-login-token"), "{}", p.display());
                let name = p.file_name().unwrap().to_string_lossy().into_owned();
                assert!(
                    name != "credentials.json" && name != "session.json",
                    "{}",
                    p.display()
                );
            }
        }
        // A stored session is used as is, without a sign-in.
        let stored = SetupTokens::new(Arc::new(StaticToken("acct".into())), || {
            Box::pin(async { panic!("no sign-in with a stored session") })
        });
        assert_eq!(stored.access_token().await.unwrap(), "acct");
        assert!(!stored.used_ephemeral());
    }

    /// `--allow` sharing stays, is confirmed before it widens access, and is
    /// revocable with `unshare`; the driver policy follows.
    #[tokio::test]
    async fn share_needs_consent_and_unshare_revokes() {
        let relay = FakeRelay::start().await;
        relay.add_account("acct", "user-1", Some("ada@example.com"));
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join(".cua");
        let driver = dir.path().join("drv");
        std::fs::write(&driver, b"x").unwrap();
        let manager = Arc::new(FakeServiceManager::default());
        let tokens = StaticToken("acct".into());
        let run = |args: Vec<String>| {
            let home = home.clone();
            let manager = manager.clone();
            let tokens = tokens.clone();
            async move {
                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                let mut out = Vec::new();
                run_host(
                    parse(&args),
                    &home,
                    &tokens,
                    None,
                    Some(manager),
                    true,
                    &mut out,
                )
                .await
                .map(|_| String::from_utf8(out).unwrap())
            }
        };
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        run(s(&[
            "setup",
            "--relay",
            &relay.url,
            "--allow",
            "friend@example.com",
            "--driver-bin",
            driver.to_str().unwrap(),
        ]))
        .await
        .unwrap();
        let id = Host::new(&home)
            .config()
            .unwrap()
            .unwrap()
            .machine_id
            .unwrap();
        // Not interactive and no --yes: refused.
        assert!(matches!(
            run(s(&["share", "bob@example.com"])).await,
            Err(CuaError::PermissionDenied(_))
        ));
        assert_eq!(relay.machine(&id).unwrap().allow, ["friend@example.com"]);
        let text = run(s(&["share", "bob@example.com", "--yes"]))
            .await
            .unwrap();
        assert!(text.contains("bob@example.com"), "{text}");
        assert_eq!(
            relay.machine(&id).unwrap().allow,
            ["friend@example.com", "bob@example.com"]
        );
        run(s(&["unshare", "FRIEND@example.com"])).await.unwrap();
        assert_eq!(relay.machine(&id).unwrap().allow, ["bob@example.com"]);
        assert_eq!(
            Host::new(&home).policy().unwrap().unwrap().allow,
            ["bob@example.com"]
        );
    }

    #[tokio::test]
    async fn direct_setup_needs_no_account_and_rejects_bad_addresses() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join(".cua");
        let driver = dir.path().join("drv");
        std::fs::write(&driver, b"x").unwrap();
        let manager = Arc::new(FakeServiceManager::default());
        let mut out = Vec::new();
        let err = run_host(
            parse(&["setup", "--direct", "nope"]),
            &home,
            &cua_host::NoAccount,
            None,
            Some(manager.clone()),
            false,
            &mut out,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, CuaError::InvalidArgument(_)));
        run_host(
            parse(&[
                "setup",
                "--direct",
                "127.0.0.1:3299",
                "--driver-bin",
                driver.to_str().unwrap(),
            ]),
            &home,
            &cua_host::NoAccount,
            None,
            Some(manager),
            false,
            &mut out,
        )
        .await
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("direct http://127.0.0.1:3299"), "{text}");
    }

    #[test]
    fn relay_and_direct_conflict() {
        #[derive(clap::Parser)]
        struct W {
            #[command(subcommand)]
            c: HostCmd,
        }
        assert!(
            <W as clap::Parser>::try_parse_from([
                "host",
                "setup",
                "--relay",
                "https://r",
                "--direct",
                "1.2.3.4:1"
            ])
            .is_err()
        );
    }

    /// `cua mcp` without a daemon (embedded) sees the account's relay
    /// machines, as the daemon's Spaces do; it attaches only once and never
    /// to a daemon-backed `cua`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn embedded_mcp_spaces_list_the_accounts_relay_machines() {
        let relay = FakeRelay::start().await;
        relay.add_account("acct", "user-1", None);
        cua_host::RelayClient::new(&relay.url)
            .unwrap()
            .register(
                "acct",
                &cua_host::relay::RegisterRequest {
                    id: "0123abcd4567ef89".into(),
                    name: "lab-box".into(),
                    allow: vec![],
                    host: None,
                    meta: Default::default(),
                },
            )
            .await
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let cua = cua_sdk::Cua::embedded(cua_sdk::CuaConfig {
            state_dir: Some(dir.path().join("sandboxes").display().to_string()),
            spaces_home: Some(dir.path().join("cua").display().to_string()),
            fleet_pool_home: Some(dir.path().join("pools").display().to_string()),
            fleet_from_env: false,
            fleet_from_session: false,
            ..Default::default()
        })
        .unwrap();
        let list = |cua: Arc<cua_sdk::Cua>| async move {
            cua.spaces()
                .call_tool_json("list_spaces".into(), None)
                .await
                .unwrap()
                .text
        };
        assert!(!list(cua.clone()).await.contains("relay:0123abcd4567ef89"));

        let account = || {
            cua_spaces::RelayAccount::new(relay.url.clone(), Arc::new(StaticToken("acct".into())))
        };
        assert!(attach_relay_account(&cua, account));
        let text = list(cua.clone()).await;
        assert!(text.contains("relay:0123abcd4567ef89"), "{text}");
        assert!(text.contains("lab-box"), "{text}");
        // Already attached: left alone.
        assert!(!attach_relay_account(&cua, || unreachable!()));
    }
}
