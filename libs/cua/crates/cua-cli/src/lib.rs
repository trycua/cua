//! `cua`: the Cua command line, on the cua SDK (`cua-sdk` as a Rust
//! library).
//!
//! - `auth`, `wif-token`: Cua / Fleet identity (browser or device login,
//!   client credentials, `FLEETS_TOKEN`, GitHub Actions OIDC, user API
//!   keys);
//! - `agents`: set up AI coding agents (cua skills, the cua MCP server);
//! - `sandbox` (`sb`): Fleet, local and direct sandboxes;
//! - `do`, `do-host-consent`: one-shot computer actions for agents;
//! - `mcp` (`serve-mcp`): stdio MCP server for AI assistants;
//! - `skills`, `trajectory` (`traj`): recorded demonstrations and action
//!   trajectories;
//! - `image` (`img`): local OCI images and Fleet image resources;
//! - `env`, `daemon`, `runtime`: spacesd debugging, the cua daemon and
//!   local runtimes;
//! - `host`, `spaces`: set this machine up for unattended access (relay or
//!   direct) and list Spaces including the account's relay machines.
//! - `teleport`, `keyvault`: move a desktop app session into a sandbox, and
//!   the Cua Keyvault; they ship with Cua Spaces ([`extension`]).
//!
//! By default the CLI uses a running `cua daemon` when one is discoverable
//! (`~/.cua/daemon.json`) and an embedded runtime otherwise; `--embedded`
//! and `--daemon <addr>` force one.

mod agent_cmd;
mod agents;
mod auth;
mod browse;
mod cache;
mod catalog;
mod cloud_cmd;
mod computer;
mod config_cmd;
mod devices_cmd;
mod do_cmd;
mod doctor;
pub mod drive_cmd;
mod dump_docs;
pub mod extension;
mod host;
mod image_build;
mod image_release;
mod images;
pub mod keyvault_cmd;
mod mcp;
mod persistent_cmd;
mod pools;
mod providers;
pub mod sandbox;
mod shell;
mod skills;
#[doc(hidden)]
pub mod stream_probe;
mod telemetry_cmd;
pub mod teleport;
pub mod teleport_session;
mod trajectory;
pub mod util;

use clap::{ArgAction, CommandFactory, FromArgMatches, Parser, Subcommand};
use cua_sdk::{Cua, CuaConfig, CuaError, SpacesdClient};
use std::{io::Write, path::PathBuf, sync::Arc, time::Duration};
use util::line;

#[derive(Parser, Debug)]
#[command(
    name = "cua",
    version = cua_sdk::VERSION,
    disable_version_flag = true,
    about = "Cua: sandboxes, computer actions, MCP, skills and the cua daemon",
    after_help = "Docs: https://cua.ai/docs/cua-cli/reference/cli"
)]
struct Cli {
    /// Print the version.
    #[arg(short = 'v', short_alias = 'V', long, action = ArgAction::Version, global = false)]
    version: Option<bool>,
    /// Use an embedded runtime even when a daemon runs.
    #[arg(long, global = true)]
    embedded: bool,
    /// Daemon address (socket path or loopback URL).
    #[arg(long, global = true, env = "CUA_DAEMON")]
    daemon: Option<String>,
    /// Daemon token (loopback URLs).
    #[arg(long, global = true, env = "CUA_DAEMON_TOKEN", hide_env_values = true)]
    daemon_token: Option<String>,
    /// Sandbox state directory (embedded; default ~/.cua/sandboxes).
    #[arg(long, global = true)]
    state_dir: Option<String>,
    /// Print JSON.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Log in to Cua and inspect Fleet identity.
    #[command(subcommand)]
    Auth(AuthCmd),
    /// AI coding agents on this machine: install cua skills and configure the cua MCP server.
    #[command(subcommand)]
    Agents(agents::AgentsCmd),
    /// Run coding agents inside a sandbox: run, ls, logs, send, interrupt, stop.
    #[command(subcommand)]
    Agent(agent_cmd::AgentCmd),
    /// Sandboxes: local, cloud or an existing machine (direct).
    #[command(subcommand, visible_alias = "sb")]
    Sandbox(Box<sandbox::SandboxCmd>),
    /// User defaults (~/.cua/config.toml): where sandboxes run when --on is
    /// not given (default.on), the default kind and runtime, cloud defaults.
    #[command(subcommand)]
    Config(config_cmd::ConfigCmd),
    /// Serve the HTML5 viewer for every sandbox on one loopback port.
    #[command(after_help = "Examples:
  # List your sandboxes in the browser and open any of them
  cua viewer
  # Fixed port, print the URL only
  cua viewer --listen 127.0.0.1:8211 --no-open")]
    Viewer {
        /// Address to listen on (loopback only).
        #[arg(long, default_value = "127.0.0.1:0")]
        listen: String,
        /// Print the URL without opening a browser.
        #[arg(long)]
        no_open: bool,
    },
    /// One-shot computer actions against the selected target.
    #[command(after_help = "Examples:
  # Act on a sandbox, then take a screenshot
  cua do switch dev
  cua do screenshot")]
    Do(do_cmd::DoArgs),
    /// Grant consent for `cua do switch host`.
    #[command(
        name = "do-host-consent",
        after_help = "Examples:
  # Allow `cua do` to act on this machine's desktop
  cua do-host-consent"
    )]
    DoHostConsent,
    /// Stdio MCP server exposing sandboxes, computer control and skills.
    #[command(
        visible_alias = "serve-mcp",
        after_help = "Examples:
  # Serve every tool over stdio (what MCP clients launch)
  cua mcp
  # Read-only sandbox tools, computer tools on the sandbox dev
  cua mcp --permissions sandbox:readonly,computer:all --sandbox dev"
    )]
    Mcp {
        /// Comma-separated permissions or groups (default: all).
        #[arg(long, env = "CUA_MCP_PERMISSIONS", default_value = "")]
        permissions: String,
        /// Default sandbox for computer tools.
        #[arg(long, env = "CUA_SANDBOX", default_value = "")]
        sandbox: String,
    },
    /// Recorded demonstrations (skills) for agents.
    #[command(subcommand)]
    Skills(SkillsCmd),
    /// Recorded `cua do` trajectories.
    #[command(subcommand, visible_alias = "traj")]
    Trajectory(TrajCmd),
    /// Workload identity federation tokens.
    #[command(subcommand, name = "wif-token")]
    WifToken(WifCmd),
    /// Fleet: managed pools behind `cua sandbox create` without `--pool`.
    #[command(subcommand)]
    Fleet(FleetCmd),
    /// Images: local OCI pull/build/push and Fleet image resources.
    #[command(subcommand, visible_alias = "img")]
    Image(ImageCmd),
    /// The sandbox image catalog (the images the docs list): which exist,
    /// their variants, and which ship cua-spacesd and a browser.
    #[command(subcommand)]
    Images(ImagesCmd),
    /// Talk to a cua-spacesd directly by URL.
    #[command(subcommand)]
    Spacesd(SpacesdCmd),
    /// The cua daemon.
    #[command(subcommand)]
    Daemon(DaemonCmd),
    /// Local runtimes (cua-vmm).
    #[command(subcommand)]
    Runtime(RuntimeCmd),
    /// Disk used by images, sandboxes, builds and logs, and its cleanup.
    #[command(subcommand)]
    Cache(cache::CacheCmd),
    /// Check the host, an image and a sandbox's guest in one report
    /// (`cua doctor [REF]`), or eval parity between two variants
    /// (`cua doctor parity A B`).
    #[command(after_help = "Examples:
  # Host checks only
  cua doctor
  # Host, image and guest checks for the sandbox dev
  cua doctor dev
  # CI: fail on warnings, write JUnit XML
  cua doctor cloud:dev --strict --junit doctor.xml")]
    Doctor(Box<doctor::DoctorCmd>),
    /// Unattended access to this machine (setup, status, stop, start, remove).
    #[command(subcommand)]
    Host(host::HostCmd),
    /// Spaces: registered ones and your machines on the relay.
    #[command(subcommand)]
    Spaces(host::SpacesCmd),
    /// Spaces in your own cloud account (AWS, Google Cloud, Modal):
    /// connect, status, test, disconnect, sweep.
    #[command(
        subcommand,
        after_help = "Examples:
  cua cloud status
  cua cloud connect aws --region us-west-2 --default
  cua spaces create --on aws
  cua cloud sweep"
    )]
    Cloud(cloud_cmd::CloudCmd),
    /// This device as a client of your cua.ai account on the relay:
    /// enroll (second factor), approve other devices, list, rename,
    /// revoke, and the audit log of who accessed what.
    #[command(
        subcommand,
        after_help = "Examples:
  cua devices enroll
  cua devices approve K7QX-M2RP
  cua devices ls
  cua devices audit"
    )]
    Devices(devices_cmd::DevicesCmd),
    /// Move a desktop app session (tabs, profile, sign-in) into a sandbox.
    #[command(subcommand)]
    Teleport(teleport::TeleportCmd),
    /// The Cua Keyvault the daemon hosts: status, init, unlock, lock, saved
    /// passwords and approvals. Passphrases are prompted for (or read from
    /// stdin), never arguments.
    #[command(
        subcommand,
        after_help = "Examples:
  cua keyvault status
  cua keyvault init --passphrase
  cua keyvault unlock --passphrase
  cua keyvault import-passwords --browser chrome
  cua keyvault requests
  cua keyvault approve <request-id>
  cua keyvault lock"
    )]
    Keyvault(keyvault_cmd::KeyvaultCmd),
    /// Cua Volume: the versioned volume every Space and agent shares
    /// (public/, agents/<agent>/, spaces/<space>/), its grants and audit.
    /// (`cua volume` still works for one release.)
    #[command(
        name = "volume",
        alias = "drive",
        subcommand,
        after_help = "Examples:
  cua volume ls agents/
  cua volume cat agents/ada/memory/MEMORY.md
  cua volume put public/rules.md ./rules.md
  cua volume grant agent:researcher agents/writer/outputs/
  cua volume audit"
    )]
    Volume(drive_cmd::DriveCmd),
    /// Anonymous usage telemetry (status, on, off, show-last, reset-id,
    /// schema). What is sent:
    /// https://cua.ai/docs/cua-sdk/concepts/telemetry
    #[command(
        subcommand,
        after_help = "Examples:
  cua telemetry status
  cua telemetry off
  cua telemetry show-last"
    )]
    Telemetry(telemetry_cmd::TelemetryCmd),
    /// (internal) The CLI and MCP surface as JSON, for the generated docs.
    #[command(name = "dump-docs", hide = true)]
    DumpDocs {
        /// What to dump.
        #[arg(long = "type", default_value = "all", value_parser = ["cli", "mcp", "all"])]
        kind: String,
        /// Pretty-print.
        #[arg(long)]
        pretty: bool,
    },
}

#[derive(Subcommand, Debug)]
enum AuthCmd {
    /// Log in (browser sign-in; `--remote` for a device code), then offer
    /// to set up your AI coding agents.
    #[command(after_help = "Examples:
  # Sign in with the browser
  cua auth login
  # Sign in over SSH with a device code
  cua auth login --remote
  # Sign in and set up Claude Code without prompts
  cua auth login --agents claude -y")]
    Login {
        /// Do not open a browser; print the URL.
        #[arg(long)]
        no_browser: bool,
        /// Sign in from another device with a code (for SSH sessions and
        /// machines without a browser).
        #[arg(long)]
        remote: bool,
        /// Force a flow: `pkce` or `device`.
        #[arg(long, hide = true, env = "CUA_AUTH_FLOW")]
        flow: Option<String>,
        /// Skip the agent onboarding prompt.
        #[arg(long)]
        no_onboarding: bool,
        #[command(flatten)]
        setup: agents::SetupArgs,
    },
    /// Revoke and remove the local session.
    #[command(after_help = "Examples:
  cua auth logout")]
    Logout,
    /// Local session state (no network).
    #[command(after_help = "Examples:
  cua auth status
  cua auth status --json")]
    Status,
    /// The active Fleet identity, verified against Fleet.
    #[command(after_help = "Examples:
  cua auth whoami")]
    Whoami,
    /// Fleet user API keys (client credentials).
    #[command(subcommand)]
    Keys(KeysCmd),
    /// Keys of the contrib sandbox providers (`--on e2b`, `--on daytona`,
    /// `--on modal`).
    #[command(subcommand)]
    Provider(ProviderCmd),
}

#[derive(Subcommand, Debug)]
enum ProviderCmd {
    /// Providers, whether this build has them and where their keys come
    /// from (never the values).
    #[command(
        visible_alias = "list",
        after_help = "Examples:
  cua auth provider ls
  cua auth provider ls --json"
    )]
    Ls,
    /// Store a provider key, read from stdin (no echo on a terminal). An
    /// environment variable of the same name wins over the stored key.
    #[command(after_help = "Examples:
  # Prompt for E2B_API_KEY (no echo)
  cua auth provider set e2b
  # One variable only
  cua auth provider set daytona --var DAYTONA_API_KEY
  # From a secret manager, without shell history:
  #   op read op://ci/e2b/key | cua auth provider set e2b")]
    Set {
        /// Provider (`e2b`, `daytona`, `modal`).
        name: String,
        /// Only this variable (for example MODAL_TOKEN_SECRET).
        #[arg(long)]
        var: Option<String>,
    },
    /// Remove a provider's stored keys.
    #[command(
        visible_alias = "delete",
        after_help = "Examples:
  cua auth provider rm e2b"
    )]
    Rm {
        /// Provider.
        name: String,
    },
}

#[derive(Subcommand, Debug)]
enum KeysCmd {
    /// List API keys.
    #[command(
        visible_alias = "list",
        after_help = "Examples:
  cua auth keys ls"
    )]
    Ls,
    /// Create an API key (prints CUA_CLIENT_ID / CUA_CLIENT_SECRET once).
    #[command(after_help = "Examples:
  # A key for CI
  cua auth keys create ci")]
    Create {
        /// Key name.
        name: String,
        /// Scopes (repeatable).
        #[arg(long = "scope")]
        scope: Vec<String>,
    },
    /// Delete an API key by id.
    #[command(
        visible_alias = "delete",
        after_help = "Examples:
  cua auth keys rm <key-id>"
    )]
    Rm {
        /// Key id (from `cua auth keys ls`).
        id: String,
    },
}

#[derive(Subcommand, Debug)]
enum SkillsCmd {
    /// List saved skills.
    #[command(
        visible_alias = "ls",
        after_help = "Examples:
  cua skills list"
    )]
    List,
    /// Print a skill.
    #[command(after_help = "Examples:
  cua skills read export-invoice
  cua skills read export-invoice --format json")]
    Read {
        /// Skill name.
        name: String,
        /// Output format.
        #[arg(short, long, default_value = "md", value_parser = ["md", "json"])]
        format: String,
    },
    /// Open a skill's video recording.
    #[command(after_help = "Examples:
  cua skills replay export-invoice")]
    Replay {
        /// Skill name.
        name: String,
    },
    /// Delete a skill.
    #[command(after_help = "Examples:
  cua skills delete export-invoice")]
    Delete {
        /// Skill name.
        name: String,
    },
    /// Delete every skill (asks first).
    #[command(after_help = "Examples:
  cua skills clean
  # Without the prompt
  cua skills clean --yes")]
    Clean {
        /// Do not ask for confirmation.
        #[arg(short, long)]
        yes: bool,
    },
    /// Record a demonstration in the HTML5 viewer (it records the screen and your input).
    #[command(after_help = "Examples:
  # Record on the sandbox dev and caption with Anthropic
  cua skills record --sandbox dev --name export-invoice")]
    Record {
        /// Sandbox whose display service to record.
        #[arg(short, long)]
        sandbox: Option<String>,
        /// Viewer URL to record (a `cua sb view --no-open` link).
        #[arg(short = 'u', long, alias = "vnc-url")]
        viewer_url: Option<String>,
        /// Captioning provider.
        #[arg(short, long, default_value = "anthropic", value_parser = ["anthropic", "openai"])]
        provider: String,
        /// Captioning model.
        #[arg(short, long)]
        model: Option<String>,
        /// API key for the captioning provider.
        #[arg(short = 'k', long, hide_env_values = true)]
        api_key: Option<String>,
        /// Skill name.
        #[arg(short, long)]
        name: Option<String>,
        /// Skill description.
        #[arg(short, long)]
        description: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
enum TrajCmd {
    /// List sessions.
    #[command(after_help = "Examples:
  cua traj ls
  # Only the sessions recorded on the sandbox dev
  cua traj ls dev")]
    Ls {
        /// Only sessions of this target (sandbox name or `host`).
        machine: Option<String>,
    },
    /// Zip, serve on loopback and open the hosted viewer.
    #[command(after_help = "Examples:
  # The latest session
  cua traj view
  cua traj view <session> --port 8090")]
    View {
        /// Session id or target name (default: the latest session).
        target: Option<String>,
        /// Loopback port of the file server.
        #[arg(short, long, default_value_t = 8089)]
        port: u16,
    },
    /// Stop the file server started by `view`.
    #[command(after_help = "Examples:
  cua traj stop")]
    Stop,
    /// Delete sessions.
    #[command(after_help = "Examples:
  # Sessions older than a week, without the prompt
  cua traj clean --older-than 7 --yes")]
    Clean {
        /// Only sessions older than this many days.
        #[arg(long, value_name = "DAYS")]
        older_than: Option<i64>,
        /// Only sessions of this target.
        #[arg(long)]
        machine: Option<String>,
        /// Do not ask for confirmation.
        #[arg(short, long)]
        yes: bool,
    },
    /// (internal) Serve a directory for the viewer.
    #[command(hide = true)]
    Serve {
        dir: PathBuf,
        #[arg(long, default_value_t = 8089)]
        port: u16,
    },
}

#[derive(Subcommand, Debug)]
enum WifCmd {
    /// Print a GitHub Actions OIDC token for Fleets.
    #[command(after_help = "Examples:
  # In a GitHub Actions job with `id-token: write`
  cua wif-token github")]
    Github {
        /// Token audience.
        #[arg(long, default_value = auth::GITHUB_WIF_AUDIENCE)]
        audience: String,
    },
}

#[derive(Subcommand, Debug)]
enum SpacesdCmd {
    /// Print capabilities.
    #[command(after_help = "Examples:
  cua spacesd caps http://127.0.0.1:3211")]
    Caps {
        /// spacesd URL (`http://host:port`).
        url: String,
        /// spacesd token.
        #[arg(long, env = "CUA_ENV_TOKEN", hide_env_values = true)]
        token: Option<String>,
    },
    /// Run a command (argv, no shell).
    #[command(after_help = "Examples:
  cua spacesd exec http://127.0.0.1:3211 uname -a")]
    Exec {
        /// spacesd URL (`http://host:port`).
        url: String,
        /// spacesd token.
        #[arg(long, env = "CUA_ENV_TOKEN", hide_env_values = true)]
        token: Option<String>,
        /// Program and arguments.
        #[arg(trailing_var_arg = true, required = true)]
        command: Vec<String>,
    },
    /// Copy files: `cua spacesd cp <URL> local :remote` uploads,
    /// `cua spacesd cp <URL> :remote local` downloads.
    #[command(after_help = "Examples:
  # Upload
  cua spacesd cp http://127.0.0.1:3211 ./notes.txt :/tmp/notes.txt
  # Download
  cua spacesd cp http://127.0.0.1:3211 :/tmp/notes.txt ./notes.txt")]
    Cp {
        /// spacesd URL (`http://host:port`).
        url: String,
        /// Source: a local path, or `:/guest/path`.
        src: String,
        /// Destination: a local path, or `:/guest/path`.
        dst: String,
        /// spacesd token.
        #[arg(long, env = "CUA_ENV_TOKEN", hide_env_values = true)]
        token: Option<String>,
    },
    /// Interactive shell (PTY).
    #[command(after_help = "Examples:
  cua spacesd shell http://127.0.0.1:3211")]
    Shell {
        /// spacesd URL (`http://host:port`).
        url: String,
        /// spacesd token.
        #[arg(long, env = "CUA_ENV_TOKEN", hide_env_values = true)]
        token: Option<String>,
    },
    /// List streamable targets (displays, and windows with --windows).
    #[command(after_help = "Examples:
  cua spacesd targets http://127.0.0.1:3211 --windows")]
    Targets {
        /// spacesd URL (`http://host:port`).
        url: String,
        /// spacesd token.
        #[arg(long, env = "CUA_ENV_TOKEN", hide_env_values = true)]
        token: Option<String>,
        /// Include window targets.
        #[arg(long)]
        windows: bool,
    },
    /// Call any cua.env.v1 RPC with a proto3-JSON body.
    #[command(after_help = "Examples:
  cua spacesd call http://127.0.0.1:3211 SystemService/GetCapabilities")]
    Call {
        /// spacesd URL (`http://host:port`).
        url: String,
        /// `Service/Method`.
        method: String,
        /// Request body (proto3 JSON).
        #[arg(default_value = "{}")]
        body: String,
        /// spacesd token.
        #[arg(long, env = "CUA_ENV_TOKEN", hide_env_values = true)]
        token: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
enum DaemonCmd {
    /// Start the daemon (in the background unless --foreground).
    #[command(after_help = "Examples:
  cua daemon start
  # In the foreground, for a service manager
  cua daemon start --foreground")]
    Start {
        /// Stay in the foreground.
        #[arg(long)]
        foreground: bool,
        /// Unix socket (default ~/.cua/cua.sock).
        #[arg(long)]
        socket: Option<PathBuf>,
        /// Loopback address (default 127.0.0.1:0; "off" disables).
        #[arg(long, default_value = "127.0.0.1:0")]
        loopback: String,
    },
    /// Stop the daemon.
    #[command(after_help = "Examples:
  cua daemon stop")]
    Stop,
    /// Show daemon status.
    #[command(after_help = "Examples:
  cua daemon status")]
    Status,
    /// Stdio MCP server backed by the daemon (started if needed): the Spaces
    /// contract tools plus the sandbox/computer/skills tools (same server as
    /// `cua mcp`).
    #[command(after_help = "Examples:
  cua daemon mcp
  cua daemon mcp --permissions spaces:all")]
    Mcp {
        /// Comma-separated permissions or groups (default: all).
        #[arg(long, env = "CUA_MCP_PERMISSIONS", default_value = "")]
        permissions: String,
        /// Default sandbox for computer tools.
        #[arg(long, env = "CUA_SANDBOX", default_value = "")]
        sandbox: String,
    },
}

#[derive(Subcommand, Debug)]
enum RuntimeCmd {
    /// Inspect local runtimes (read-only).
    #[command(after_help = "Examples:
  cua runtime doctor")]
    Doctor,
    /// Provision local runtimes.
    #[command(after_help = "Examples:
  # See what would be installed
  cua runtime setup --dry-run
  cua runtime setup qemu")]
    Setup {
        /// Only report what would be done.
        #[arg(long)]
        dry_run: bool,
        /// Components: `qemu`, `lume`, `runsc` (default: everything provisionable).
        components: Vec<String>,
    },
}

#[derive(Subcommand, Debug)]
enum FleetCmd {
    /// Managed pools (`cua-auto-*`): reused per image and shape, autoscaled
    /// from zero, garbage-collected when idle.
    #[command(subcommand)]
    Pools(PoolsCmd),
    /// One pool (any pool you own, not only managed ones).
    #[command(subcommand)]
    Pool(PoolCmd),
}

#[derive(Subcommand, Debug)]
enum PoolCmd {
    /// Print a pool's spec: the shared sandbox model as JSON, or with
    /// `--terraform` the equivalent `fleets_pool` resource block.
    #[command(after_help = "Examples:
  cua fleet pool export ci-linux
  cua fleet pool export ci-linux --terraform > pool.tf")]
    Export {
        /// Pool name.
        name: String,
        /// Emit Terraform HCL (the cyclops-cs `fleets_pool` resource).
        #[arg(long)]
        terraform: bool,
    },
}

#[derive(Subcommand, Debug)]
enum PoolsCmd {
    /// List this account's managed pools.
    #[command(
        visible_alias = "list",
        after_help = "Examples:
  cua fleet pools ls"
    )]
    Ls,
    /// Delete managed pools idle for `--idle` and stuck claims.
    #[command(after_help = "Examples:
  cua fleet pools gc
  cua fleet pools gc --idle 2h")]
    Gc {
        /// Idle threshold (`30m`, `2h`, `1d`).
        #[arg(long, default_value = "30m")]
        idle: String,
        /// Only these pools (repeatable). Default: every managed pool.
        #[arg(long = "pool")]
        pools: Vec<String>,
    },
}

#[derive(Subcommand, Debug)]
enum ImageCmd {
    /// Pull an image into the local cache.
    #[command(after_help = "Examples:
  cua image pull linux
  cua image pull ghcr.io/trycua/linux:24.04")]
    Pull {
        /// Image reference or alias.
        reference: String,
    },
    /// Build an image from an images.cua.ai/v1alpha1 Image resource (JSON).
    #[command(after_help = "Examples:
  cua image build image.json --base container:ubuntu:24.04
  cua image build image.json --base container:ubuntu:24.04 --push ghcr.io/acme/desktop:1")]
    Build {
        /// Image resource file (JSON).
        spec: PathBuf,
        /// Base image (`container:<ref>`, `vm:<ref>`, `disk:<path>`).
        #[arg(long)]
        base: String,
        /// Push the result to this reference.
        #[arg(long)]
        push: Option<String>,
    },
    /// Copy an image (host architecture) to another registry reference.
    #[command(after_help = "Examples:
  cua image push linux ghcr.io/acme/desktop:1")]
    Push {
        /// Source image reference or alias.
        reference: String,
        /// Destination registry reference.
        destination: String,
    },
    /// List Fleet image resources.
    #[command(
        visible_alias = "list",
        after_help = "Examples:
  cua image ls
  cua image ls --namespace acme"
    )]
    Ls {
        /// Namespace (default: every namespace, or CUA_FLEET_NAMESPACE).
        #[arg(long)]
        namespace: Option<String>,
    },
    /// Show a Fleet image resource.
    #[command(after_help = "Examples:
  cua image info desktop")]
    Info {
        /// Image resource name.
        name: String,
        /// Namespace (default: CUA_FLEET_NAMESPACE).
        #[arg(long)]
        namespace: Option<String>,
    },
    /// Delete a Fleet image resource.
    #[command(
        visible_alias = "delete",
        after_help = "Examples:
  cua image rm desktop"
    )]
    Rm {
        /// Image resource name.
        name: String,
        /// Namespace (default: CUA_FLEET_NAMESPACE).
        #[arg(long)]
        namespace: Option<String>,
        /// Do not ask for confirmation.
        #[arg(long)]
        force: bool,
    },
    /// Submit an Image manifest (JSON) to Fleet for a remote build.
    #[command(after_help = "Examples:
  cua image create -f image.json")]
    Create {
        /// Image manifest file (JSON).
        #[arg(short, long)]
        file: PathBuf,
        /// Namespace (default: CUA_FLEET_NAMESPACE).
        #[arg(long)]
        namespace: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
enum ImagesCmd {
    /// List the catalog: ref, OS, container or VM, local and cloud runtime,
    /// cua-spacesd, browsers, summary.
    #[command(
        visible_alias = "list",
        after_help = "Examples:
  cua images ls
  # Images whose browser the cua-driver browser tools drive
  cua images ls --browser
  cua images ls --all --json"
    )]
    Ls {
        /// Include unpublished entries (benchmark images).
        #[arg(long)]
        all: bool,
        /// Only this OS: linux, windows or macos.
        #[arg(long)]
        os: Option<String>,
        /// Only images with a browser the cua-driver browser tools drive.
        #[arg(long)]
        browser: bool,
    },
    /// Show one catalog entry (a ref or an alias such as `linux`).
    #[command(after_help = "Examples:
  cua images info linux")]
    Info {
        /// Image ref or alias.
        reference: String,
    },
    /// Build a sandbox image definition (a libs/images directory): the
    /// rootfs, the bootable VM disk and the KubeVirt containerDisk.
    #[command(
        after_help = "Outputs: rootfs <repo>:docker-<tag>-<arch>; disk <out>/<name>/<arch>/disk.img;
containerdisk <repo>:<tag>-<arch>. --push pushes the outputs the image.json
lists (a VM-only image never pushes its rootfs).

Examples:
  cua images build libs/images/omarchy --outputs rootfs,disk
  cua images build libs/images/omarchy --tag build-1a2b3c4d --outputs containerdisk --push
  cua images build libs/images/linux --target slim --tag 24.04-slim-local --outputs rootfs,disk
  cua images build libs/images/linux --platform linux/amd64,linux/arm64 --dry-run"
    )]
    Build {
        /// Image definition directory (Dockerfile, image.json).
        dir: PathBuf,
        /// Platforms, comma separated (default: the host's, or the image's only one).
        #[arg(long)]
        platform: Option<String>,
        /// Tag for the per-arch outputs.
        #[arg(long, default_value = "local")]
        tag: String,
        /// Repository (default: image.json `repository`, else cua-e2e-local/<name>).
        #[arg(long)]
        repo: Option<String>,
        /// Outputs: rootfs, disk, containerdisk (comma separated).
        #[arg(long, default_value = "rootfs")]
        outputs: String,
        /// Docker build argument KEY=VALUE (repeatable).
        #[arg(long = "build-arg")]
        build_args: Vec<String>,
        /// Named build context NAME=PATH (repeatable).
        #[arg(long = "build-context")]
        build_contexts: Vec<String>,
        /// Extra rootfs label KEY=VALUE (repeatable).
        #[arg(long = "label")]
        labels: Vec<String>,
        /// Where disks go (default: $CUA_IMAGES_OUT or ~/.cache/cua-images).
        #[arg(long)]
        out: Option<PathBuf>,
        /// Virtual size of the disk.
        #[arg(long, default_value = "20G")]
        disk_size: String,
        /// Dockerfile stage to build (a tier: `slim`, `full`); disks go to
        /// <out>/<name>-<target>/<arch>. Default: the Dockerfile's last stage.
        #[arg(long)]
        target: Option<String>,
        /// Push the published outputs.
        #[arg(long)]
        push: bool,
        /// Print the steps without running them.
        #[arg(long)]
        dry_run: bool,
    },
    /// Pack the disks `cua images build --outputs disk` wrote as KubeVirt
    /// containerDisks `<repo>:<tag>-<arch>` (push, or load into docker).
    #[command(
        after_help = "Packs exactly the disk the doctor checked, so the pushed bytes are the
tested ones.

Examples:
  cua images pack libs/images/omarchy --tag ci-1a2b3c4d --push --json"
    )]
    Pack {
        /// Image definition directory.
        dir: PathBuf,
        /// Platforms, comma separated (default: the host's, or the image's only one).
        #[arg(long)]
        platform: Option<String>,
        /// Tag for the per-arch containerDisks.
        #[arg(long, default_value = "local")]
        tag: String,
        /// Repository (default: image.json `repository`, else cua-e2e-local/<name>).
        #[arg(long)]
        repo: Option<String>,
        /// Where the disks are (default: $CUA_IMAGES_OUT or ~/.cache/cua-images).
        #[arg(long)]
        out: Option<PathBuf>,
        /// The `--target` the disks were built with (their <name>-<target> dir).
        #[arg(long)]
        target: Option<String>,
        /// Push instead of loading into docker.
        #[arg(long)]
        push: bool,
        /// Print what would be packed.
        #[arg(long)]
        dry_run: bool,
    },
    /// Publish a built image as immutable pins `<series>-<stamp>` and
    /// `<series>-disk-<stamp>`, refusing any that exist. Moves nothing.
    #[command(
        after_help = "Reads the per-arch images `cua images build --tag TAG --push` wrote
(<repo>:<tag>-<arch>, and <repo>:docker-<tag>-<arch> when the image has a rootfs
output). Writes the record `cua images promote` takes.

Examples:
  cua images publish libs/images/omarchy --tag build-1a2b3c4d --series edge --record pins.json
  cua images publish libs/images/linux --tag build-20260925-1a2b3c4 --series 24.04 --descriptor-annotation rootfs/amd64:ai.cua.doctor.status=pass
  cua images publish libs/images/omarchy --tag build-1a2b3c4d --series edge --dry-run"
    )]
    Publish {
        /// Image definition directory.
        dir: PathBuf,
        /// The build tag the per-arch images were pushed under.
        #[arg(long)]
        tag: String,
        /// Series / channel (`edge`, `24.04`).
        #[arg(long)]
        series: String,
        /// Pin stamp `<yyyymmdd>-<sha7>` (default: today and git HEAD).
        #[arg(long)]
        stamp: Option<String>,
        /// Repository (default: image.json `repository`).
        #[arg(long)]
        repo: Option<String>,
        /// Platforms (default: image.json `platforms`).
        #[arg(long)]
        platform: Option<String>,
        /// Extra index annotation KEY=VALUE (repeatable).
        #[arg(long = "annotation")]
        annotations: Vec<String>,
        /// Annotation on one child's descriptor, VARIANT/ARCH:KEY=VALUE
        /// (`rootfs/amd64:ai.cua.doctor.status=pass`; repeatable).
        #[arg(long = "descriptor-annotation")]
        descriptor_annotations: Vec<String>,
        /// Write the publish record (JSON) here.
        #[arg(long)]
        record: Option<PathBuf>,
        /// Resolve and print, push nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Run an image's release pipeline (its release.json): build, doctor
    /// lanes, content digest, save; with --publish push, publish pins and
    /// verify; with --promote move the floating tags. The same command runs
    /// locally and in CI.
    #[command(
        after_help = "Phases: prepare, build, gate (doctor lanes), stage, push, publish, verify,
promote. Push refuses unless every required gate passed (this run, or a
state-*.json merged from other jobs in <work>/evidence). Evidence, logs and
the summary go to <work>/evidence (a plain directory CI uploads as-is).

Examples:
  # Plan only
  cua images release libs/images/linux --tier slim --dry-run
  # Build and doctor amd64 locally; resume after a failure
  cua images release libs/images/linux --tier slim --arch amd64 --resume
  # Rerun from the doctor lanes (the build must have passed)
  cua images release libs/images/linux --tier slim --from gate
  # One CI job's slice
  cua images release libs/images/linux --tier full --arch arm64 --steps build,gate,stage --work release
  # Publish and promote (the gates must have passed)
  cua images release libs/images/omarchy --publish --promote --resume"
    )]
    Release {
        /// Image directory with a release.json.
        dir: PathBuf,
        /// Tier (`slim`, `full`) for images that have tiers.
        #[arg(long)]
        tier: Option<String>,
        /// Arches, comma separated (default: release.json `arches`).
        #[arg(long = "arch", value_delimiter = ',')]
        arches: Option<Vec<String>>,
        /// Pin stamp `<yyyymmdd>-<sha7>` (default: today and git HEAD).
        #[arg(long)]
        stamp: Option<String>,
        /// Work directory (default: $CUA_RELEASE_WORK, else under the cua
        /// build cache, which `cua cache prune` manages).
        #[arg(long)]
        work: Option<PathBuf>,
        /// Only these steps: phase names or step-id prefixes (comma separated).
        #[arg(long, value_delimiter = ',')]
        steps: Option<Vec<String>>,
        /// Rerun from this phase or step; the steps before it must have passed.
        #[arg(long)]
        from: Option<String>,
        /// Skip steps that passed with the same inputs and whose outputs exist.
        #[arg(long)]
        resume: bool,
        /// Print the plan; run nothing.
        #[arg(long)]
        dry_run: bool,
        /// Also push, publish immutable pins and verify them.
        #[arg(long)]
        publish: bool,
        /// Also move the floating tags to the pins.
        #[arg(long)]
        promote: bool,
        /// Variable KEY=VALUE for release.json templates (repeatable).
        #[arg(long = "var")]
        vars: Vec<String>,
        /// Treat a host capability as present (`kvm`) or absent (`!runsc`).
        #[arg(long)]
        assume: Vec<String>,
        /// This run's state file name, state-<scope>.json (default: the
        /// arches); CI jobs sharing an evidence directory use distinct scopes.
        #[arg(long)]
        scope: Option<String>,
    },
    /// Move a series' moving tags (`edge`, `edge-disk`) to the pins a
    /// `cua images publish` record names, after the gates passed.
    #[command(after_help = "Examples:
  cua images promote pins.json
  cua images promote pins.json --dry-run")]
    Promote {
        /// Record written by `cua images publish --record`.
        record: PathBuf,
        /// Check and print, move nothing.
        #[arg(long)]
        dry_run: bool,
    },
}

/// Every exit code `cua` returns, for `--help` readers and the generated
/// reference (`cua dump-docs`). Keep in step with [`exit_code`].
pub(crate) const EXIT_CODES: &[(i32, &str)] = &[
    (0, "Success"),
    (1, "Failure, or `cua do` reported an error"),
    (
        2,
        "Invalid argument, or an ambiguous sandbox name (qualify it: `local:NAME`, `cloud:NAME`)",
    ),
    (
        3,
        "Not found: sandbox, window, skill or image (or an image not published yet)",
    ),
    (
        4,
        "Not supported, or not configured (for example no Fleet credentials)",
    ),
    (5, "No cua-spacesd answered, or a transport failure"),
    (
        6,
        "Unauthenticated or permission denied (by Cua, Fleet or your cloud account)",
    ),
    (7, "Not enough free disk space (see `cua cache`)"),
    (
        130,
        "Cancelled (Ctrl-C during a create): what it made was removed",
    ),
];

/// Exit code for an SDK error.
fn exit_code(e: &CuaError) -> i32 {
    match e {
        // An ambiguous bare name is a usage error: qualify it.
        CuaError::InvalidArgument(_)
        | CuaError::InvalidPlacement(_)
        | CuaError::AmbiguousSandbox(_) => 2,
        CuaError::NotFound(_) | CuaError::ImageNotPublished(_) => 3,
        CuaError::Unsupported(_) | CuaError::ProviderNotConfigured(_) => 4,
        CuaError::SpacesdNotAvailable(_)
        | CuaError::Transport(_)
        | CuaError::DaemonNotRunning(_) => 5,
        CuaError::Unauthenticated(_)
        | CuaError::PermissionDenied(_)
        | CuaError::FleetAdmissionDenied(_)
        | CuaError::CloudCreditExhausted(_)
        | CuaError::Cloud(_) => 6,
        CuaError::InsufficientDisk(_) => 7,
        CuaError::Cancelled(_) => 130,
        _ => 1,
    }
}

/// For a `cua` inside an app bundle: why the running daemon is not the
/// bundle's own build ([`cua_daemon::identity`]), or `None` when it is (or
/// when this `cua` is not in a bundle, or no daemon answers).
async fn stranger_daemon() -> Option<String> {
    let own = cua_daemon::identity::bundled_cua()?;
    let info = cua_daemon::client::existing_daemon()
        .await
        .ok()?
        .info()
        .await
        .ok()?;
    cua_daemon::identity::stranger(&info.executable, &info.build_id, &own)
}

/// The SDK handle. `with_session` passes the `cua auth login` session to
/// an embedded runtime as its Fleet token when the environment has no
/// Fleet credentials.
async fn open(cli: &Cli, with_session: bool) -> Result<Arc<Cua>, CuaError> {
    // The signed-in session (shared credential store) drives Fleet when the
    // environment has no Fleet credentials; the token refreshes itself.
    let embedded = move || {
        Cua::embedded(CuaConfig {
            state_dir: cli.state_dir.clone(),
            fleet_from_session: with_session,
            ..Default::default()
        })
    };
    if cli.embedded {
        let _ = TOPOLOGY.set("embedded");
        return embedded();
    }
    if cli.daemon.is_some() {
        let _ = TOPOLOGY.set("daemon");
        return Cua::connect(cli.daemon.clone(), cli.daemon_token.clone());
    }
    // A daemon that is starting is waited for (bounded), never raced with
    // a runtime of this process's own (a `cua volume mount` would mount
    // here, not in the daemon).
    if cua_daemon::client::live_address().is_none() {
        tokio::task::spawn_blocking(|| {
            cua_daemon::client::wait_for_starting(Duration::from_secs(30))
        })
        .await
        .map_err(util::internal)??;
    }
    // A daemon that accepts connections; files left by one that exited are
    // cleaned up and the command runs embedded.
    if cua_daemon::client::live_address().is_some()
        && let Ok(c) = Cua::connect(None, None)
        && c.info().await.is_ok()
    {
        // A `cua` inside an app bundle never silently uses another build's
        // daemon; `daemon status` and `daemon stop` still reach it.
        if !matches!(
            cli.command,
            Command::Daemon(DaemonCmd::Status | DaemonCmd::Stop)
        ) && let Ok(d) = cua_daemon::client::existing_daemon().await
            && let Ok(info) = d.info().await
        {
            cua_daemon::identity::check(&info)?;
        }
        let _ = TOPOLOGY.set("daemon");
        return Ok(c);
    }
    let _ = TOPOLOGY.set("embedded");
    embedded()
}

/// Which runtime this command used (`embedded`, `daemon`), for the
/// `cua_cli_command` event; unset when it opened none.
static TOPOLOGY: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();

/// The command as `group.sub` (clap names, never argument values), for
/// telemetry. Deeper levels fold into their group.
fn command_path(m: &clap::ArgMatches) -> String {
    match m.subcommand() {
        Some((top, sub)) => match sub.subcommand() {
            Some((name, _)) => format!("{top}.{name}"),
            None => top.to_string(),
        },
        None => "other".into(),
    }
}

/// Every `group.sub` path of the CLI (the telemetry vocabulary).
#[cfg(test)]
fn all_command_paths() -> Vec<String> {
    let cmd = Cli::command();
    let mut out = Vec::new();
    for top in cmd.get_subcommands() {
        let name = top.get_name().to_string();
        let subs: Vec<_> = top.get_subcommands().collect();
        if subs.is_empty() || !top.is_subcommand_required_set() {
            out.push(name.clone());
        }
        for sub in subs {
            out.push(format!("{name}.{}", sub.get_name()));
        }
    }
    out.sort();
    out
}

/// Whether a command records a `cua_cli_command` event (not `cua telemetry`
/// itself, and not the internal docs dump).
fn records_telemetry(c: &Command) -> bool {
    !matches!(c, Command::Telemetry(_) | Command::DumpDocs { .. })
}

fn print(out: &mut dyn Write, v: serde_json::Value, json: bool, text: impl FnOnce() -> String) {
    if json {
        line(out, v.to_string());
    } else {
        line(out, text());
    }
}

async fn env_at(
    cua: &Cua,
    url: &str,
    token: Option<String>,
) -> Result<Arc<SpacesdClient>, CuaError> {
    cua.spacesd(url.to_string(), token).await
}

async fn run(cli: Cli, out: &mut dyn Write) -> Result<i32, CuaError> {
    let json = cli.json;
    // Commands that need no SDK handle.
    match &cli.command {
        Command::Images(cmd) => {
            return match cmd {
                ImagesCmd::Ls { all, os, browser } => catalog::print_list(
                    &catalog::Filter {
                        all: *all,
                        os: os.clone(),
                        browser: *browser,
                    },
                    json,
                    out,
                ),
                ImagesCmd::Info { reference } => catalog::print_info(reference, json, out),
                ImagesCmd::Build {
                    dir,
                    platform,
                    tag,
                    repo,
                    outputs,
                    build_args,
                    build_contexts,
                    labels,
                    out: out_dir,
                    disk_size,
                    target,
                    push,
                    dry_run,
                } => {
                    let v = image_build::build(&image_build::BuildOpts {
                        dir: dir.clone(),
                        platforms: platform.clone(),
                        tag: tag.clone(),
                        repo: repo.clone(),
                        outputs: outputs.clone(),
                        build_args: build_args.clone(),
                        build_contexts: build_contexts.clone(),
                        labels: labels.clone(),
                        out: out_dir.clone(),
                        disk_size: disk_size.clone(),
                        target: target.clone(),
                        push: *push,
                        dry_run: *dry_run,
                    })
                    .await?;
                    if json {
                        line(out, serde_json::to_string_pretty(&v).unwrap_or_default());
                    } else if let Some(steps) = v["steps"].as_array() {
                        for s in steps {
                            line(out, s.as_str().unwrap_or_default());
                        }
                    } else {
                        for c in v["containerdisks"].as_array().into_iter().flatten() {
                            line(
                                out,
                                format!(
                                    "{}\t{}",
                                    c["reference"].as_str().unwrap_or_default(),
                                    c["digest"].as_str().unwrap_or("local")
                                ),
                            );
                        }
                    }
                    Ok(0)
                }
                ImagesCmd::Pack {
                    dir,
                    platform,
                    tag,
                    repo,
                    out: out_dir,
                    target,
                    push,
                    dry_run,
                } => {
                    let v = image_build::pack_cmd(&image_build::BuildOpts {
                        dir: dir.clone(),
                        platforms: platform.clone(),
                        tag: tag.clone(),
                        repo: repo.clone(),
                        outputs: "containerdisk".into(),
                        build_args: vec![],
                        build_contexts: vec![],
                        labels: vec![],
                        out: out_dir.clone(),
                        disk_size: "20G".into(),
                        target: target.clone(),
                        push: *push,
                        dry_run: *dry_run,
                    })
                    .await?;
                    if json {
                        line(out, serde_json::to_string_pretty(&v).unwrap_or_default());
                    } else {
                        for c in v["containerdisks"].as_array().into_iter().flatten() {
                            line(
                                out,
                                format!(
                                    "{}\t{}",
                                    c["reference"].as_str().unwrap_or_default(),
                                    c["digest"].as_str().unwrap_or(if *dry_run {
                                        "dry-run"
                                    } else {
                                        "local"
                                    })
                                ),
                            );
                        }
                    }
                    Ok(0)
                }
                ImagesCmd::Publish {
                    dir,
                    tag,
                    series,
                    stamp,
                    repo,
                    platform,
                    annotations,
                    descriptor_annotations,
                    record,
                    dry_run,
                } => {
                    let rec = image_build::publish_cmd(&image_build::PublishOpts {
                        dir: dir.clone(),
                        tag: tag.clone(),
                        series: series.clone(),
                        stamp: stamp.clone(),
                        repo: repo.clone(),
                        platforms: platform.clone(),
                        annotations: annotations.clone(),
                        descriptor_annotations: descriptor_annotations.clone(),
                        dry_run: *dry_run,
                    })
                    .await?;
                    let text = serde_json::to_string_pretty(&rec).unwrap_or_default();
                    if let Some(path) = record {
                        std::fs::write(path, format!("{text}\n"))
                            .map_err(|e| CuaError::Internal(format!("{}: {e}", path.display())))?;
                    }
                    if json {
                        line(out, &text);
                    } else {
                        let verb = if *dry_run { "would push" } else { "pushed" };
                        for p in [&rec.containerdisk, &rec.primary] {
                            line(out, format!("{verb} {}:{}\t{}", rec.repo, p.pin, p.digest));
                        }
                    }
                    Ok(0)
                }
                ImagesCmd::Release {
                    dir,
                    tier,
                    arches,
                    stamp,
                    work,
                    steps,
                    from,
                    resume,
                    dry_run,
                    publish,
                    promote,
                    vars,
                    assume,
                    scope,
                } => {
                    let v = image_release::release(&image_release::ReleaseOpts {
                        dir: dir.clone(),
                        tier: tier.clone(),
                        arches: arches.clone(),
                        stamp: stamp.clone(),
                        work: work.clone(),
                        steps: steps.clone(),
                        from: from.clone(),
                        resume: *resume,
                        dry_run: *dry_run,
                        publish: *publish,
                        promote: *promote,
                        vars: vars.clone(),
                        assume: assume.clone(),
                        scope: scope.clone(),
                    })?;
                    if json {
                        util::json_line(out, &v);
                    }
                    Ok(0)
                }
                ImagesCmd::Promote { record, dry_run } => {
                    let moved = image_build::promote_cmd(record, *dry_run).await?;
                    for m in moved {
                        line(
                            out,
                            format!("{}{m}", if *dry_run { "would move " } else { "moved " }),
                        );
                    }
                    Ok(0)
                }
            };
        }
        Command::DumpDocs { kind, pretty } => {
            let v = dump_docs::dump(kind).map_err(CuaError::InvalidArgument)?;
            let s = if *pretty {
                serde_json::to_string_pretty(&v)
            } else {
                serde_json::to_string(&v)
            };
            line(out, s.unwrap_or_default());
            return Ok(0);
        }
        Command::Auth(a) => {
            return match a {
                AuthCmd::Login {
                    no_browser,
                    remote,
                    flow,
                    no_onboarding,
                    setup,
                } => {
                    let opts = auth::LoginOptions {
                        no_browser: *no_browser,
                        remote: *remote,
                        flow: flow.clone(),
                    };
                    let code = auth::login(&opts, out).await?;
                    if code != 0 {
                        return Ok(code);
                    }
                    devices_cmd::after_login(&util::cua_home(), out).await;
                    agents::after_login(setup, *no_onboarding, out)
                }
                AuthCmd::Logout => auth::logout(out).await,
                AuthCmd::Status => auth::status(json, out).await,
                AuthCmd::Whoami => auth::whoami(json, out).await,
                AuthCmd::Keys(KeysCmd::Ls) => auth::keys_list(json, out).await,
                AuthCmd::Keys(KeysCmd::Create { name, scope }) => {
                    auth::keys_create(name.clone(), scope.clone(), json, out).await
                }
                AuthCmd::Keys(KeysCmd::Rm { id }) => auth::keys_delete(id.clone(), out).await,
                AuthCmd::Provider(ProviderCmd::Ls) => providers::list(json, out),
                AuthCmd::Provider(ProviderCmd::Set { name, var }) => {
                    providers::set(name, var.as_deref(), out)
                }
                AuthCmd::Provider(ProviderCmd::Rm { name }) => providers::remove(name, out),
            };
        }
        Command::Agents(a) => {
            return match a {
                agents::AgentsCmd::Detect => agents::cmd_detect(json, out),
                agents::AgentsCmd::Setup(args) => agents::cmd_setup(args, json, out),
                agents::AgentsCmd::Status => agents::cmd_status(json, out),
                agents::AgentsCmd::Remove(args) => agents::cmd_remove(args, json, out),
                agents::AgentsCmd::Update => agents::cmd_update(json, out),
            };
        }
        Command::WifToken(WifCmd::Github { audience }) => {
            let t = auth::github_wif_token(audience).await?;
            line(out, t);
            return Ok(0);
        }
        Command::Trajectory(t) => {
            return match t {
                TrajCmd::Ls { machine } => trajectory::cmd_ls(machine.clone(), json, out),
                TrajCmd::View { target, port } => trajectory::cmd_view(target.clone(), *port, out),
                TrajCmd::Stop => {
                    trajectory::stop_server(false);
                    Ok(0)
                }
                TrajCmd::Clean {
                    older_than,
                    machine,
                    yes,
                } => trajectory::cmd_clean(*older_than, machine.clone(), *yes, out),
                TrajCmd::Serve { dir, port } => trajectory::serve(dir.clone(), *port).await,
            };
        }
        Command::Skills(s)
            if !matches!(
                s,
                SkillsCmd::Record {
                    sandbox: Some(_),
                    ..
                }
            ) =>
        {
            return match s {
                SkillsCmd::List => skills::cmd_list(json, out),
                SkillsCmd::Read { name, format } => skills::cmd_read(name, format, out),
                SkillsCmd::Replay { name } => skills::cmd_replay(name, out),
                SkillsCmd::Delete { name } => skills::cmd_delete(name, out),
                SkillsCmd::Clean { yes } => skills::cmd_clean(*yes, out),
                SkillsCmd::Record {
                    viewer_url,
                    provider,
                    model,
                    api_key,
                    name,
                    description,
                    ..
                } => {
                    let Some(u) = viewer_url.clone() else {
                        return Err(CuaError::InvalidArgument(
                            "either --sandbox or --viewer-url is required".into(),
                        ));
                    };
                    skills::cmd_record(
                        u,
                        provider.clone(),
                        model.clone(),
                        api_key.clone(),
                        name.clone(),
                        description.clone(),
                        out,
                    )
                    .await
                }
            };
        }
        Command::Config(cmd) => return config_cmd::run(cmd.clone(), json, out),
        Command::Telemetry(cmd) => return telemetry_cmd::run(cmd.clone(), json, out),
        Command::Keyvault(cmd) => return extension::keyvault(cmd.clone(), json, out).await,
        Command::Cache(cmd) => {
            let state_dir = cli.state_dir.clone();
            return cache::run(cmd.clone(), state_dir.as_deref(), json, out).await;
        }
        Command::Fleet(FleetCmd::Pool(PoolCmd::Export { name, terraform })) => {
            return pools::export(name, *terraform, out).await;
        }
        Command::Fleet(FleetCmd::Pools(cmd)) => {
            let state_dir = cli.state_dir.as_deref();
            return match cmd {
                PoolsCmd::Ls => pools::list(state_dir, json, out).await,
                PoolsCmd::Gc { idle, pools: only } => {
                    pools::gc(state_dir, idle, only, json, out).await
                }
            };
        }
        Command::Image(
            i @ (ImageCmd::Ls { .. }
            | ImageCmd::Info { .. }
            | ImageCmd::Rm { .. }
            | ImageCmd::Create { .. }),
        ) => {
            return match i {
                ImageCmd::Ls { namespace } => images::list(namespace.clone(), json, out).await,
                ImageCmd::Info { name, namespace } => {
                    images::info(name.clone(), namespace.clone(), out).await
                }
                ImageCmd::Rm {
                    name,
                    namespace,
                    force,
                } => images::delete(name.clone(), namespace.clone(), *force, out).await,
                ImageCmd::Create { file, namespace } => {
                    images::create(file.clone(), namespace.clone(), out).await
                }
                _ => unreachable!(),
            };
        }
        Command::Daemon(DaemonCmd::Start {
            foreground,
            socket,
            loopback,
        }) => return daemon_start(&cli, *foreground, socket.clone(), loopback, out).await,
        Command::Teleport(
            cmd @ (teleport::TeleportCmd::Providers | teleport::TeleportCmd::Manifest(_)),
        ) => {
            return extension::teleport(None, cmd.clone(), json, out).await;
        }
        _ => {}
    }
    match cli.command {
        Command::Host(cmd) => {
            let home = util::cua_home();
            let tokens = host::SetupTokens::session_or_sign_in();
            let device = devices_cmd::device_auth(&host::relay_url(None, &home)).ok();
            let code =
                host::run_host(cmd, &home, &tokens, device.as_deref(), None, json, out).await?;
            if tokens.used_ephemeral() && !json {
                eprintln!(
                    "Signed in for this command only: this machine keeps no cua.ai session, just its machine token."
                );
            }
            Ok(code)
        }
        Command::Devices(cmd) => {
            return devices_cmd::run(cmd, &util::cua_home(), json, out).await;
        }
        // A machine that provides Spaces over its direct address: kept in
        // this cua home's registry, which the daemon reads too.
        Command::Spaces(cmd @ host::SpacesCmd::Add { host: true, .. }) => {
            return host::run_add_host(cmd, &util::cua_home(), json, out).await;
        }
        Command::Spaces(
            cmd @ (host::SpacesCmd::Add { .. }
            | host::SpacesCmd::Rm { .. }
            | host::SpacesCmd::Create { .. }
            | host::SpacesCmd::Cancel { .. }
            | host::SpacesCmd::Gpus { .. }
            | host::SpacesCmd::Delete { .. }
            | host::SpacesCmd::Stop { .. }
            | host::SpacesCmd::Start { .. }
            | host::SpacesCmd::Share { .. }
            | host::SpacesCmd::Unshare { .. }
            | host::SpacesCmd::Shares { .. }
            | host::SpacesCmd::RelayRegister { .. }
            | host::SpacesCmd::RelayUnregister { .. }),
        ) => {
            // The SDK handle (the daemon when one runs): the same registry
            // the daemon's Spaces runtime and `cua mcp` use.
            let sharing = matches!(
                cmd,
                host::SpacesCmd::Share { .. }
                    | host::SpacesCmd::Unshare { .. }
                    | host::SpacesCmd::Shares { .. }
                    | host::SpacesCmd::RelayRegister { .. }
                    | host::SpacesCmd::RelayUnregister { .. }
                    | host::SpacesCmd::Create { .. }
                    | host::SpacesCmd::Delete { .. }
                    // A Space one of your machines provides, or one in
                    // your cloud, is turned off and on through the relay.
                    | host::SpacesCmd::Stop { .. }
                    | host::SpacesCmd::Start { .. }
            );
            let rest = Cli {
                command: Command::Spaces(host::SpacesCmd::Ls { relay: None }),
                ..cli
            };
            let cua = open(&rest, false).await?;
            if sharing {
                // Sharing goes through the account's relay (the daemon
                // already has it; an embedded runtime gets it here), and
                // an embedded runtime confirms on the terminal.
                host::attach_relay_account(&cua, || host::daemon_relay_account(&util::cua_home()));
                if let Some(rt) = cua.embedded_runtime()
                    && !rt.spaces().has_share_consent()
                {
                    let yes = matches!(cmd, host::SpacesCmd::Share { yes: true, .. });
                    rt.spaces()
                        .set_share_consent(Some(Arc::new(host::TerminalConsent { yes })));
                }
            }
            return host::run_spaces_sdk(&cua, cmd, json, out).await;
        }
        Command::Cloud(cmd) => {
            let rest = Cli {
                command: Command::Spaces(host::SpacesCmd::Ls { relay: None }),
                ..cli
            };
            let cua = open(&rest, false).await?;
            host::attach_relay_account(&cua, || host::daemon_relay_account(&util::cua_home()));
            return cloud_cmd::run(&cua, cmd, json, out).await;
        }
        Command::Spaces(cmd) => {
            // Relay machines only when signed in; the registry either way.
            let tokens: Option<Arc<dyn cua_host::AccountTokens>> =
                match auth::session_token(&auth::Store::from_env()).await {
                    Ok(Some(_)) => Some(Arc::new(host::SessionTokens)),
                    _ => None,
                };
            let home = util::cua_home();
            let device = tokens
                .as_ref()
                .and_then(|_| devices_cmd::device_auth(&host::relay_url(None, &home)).ok());
            return host::run_spaces(cmd, &home, tokens, device, json, out).await;
        }
        command => run_sdk(Cli { command, ..cli }, out).await,
    }
}

async fn run_sdk(cli: Cli, out: &mut dyn Write) -> Result<i32, CuaError> {
    let json = cli.json;
    let needs_fleet = matches!(
        cli.command,
        Command::Sandbox(_)
            | Command::Agent(_)
            | Command::Do(_)
            | Command::DoHostConsent
            | Command::Mcp { .. }
            | Command::Skills(_)
            | Command::Daemon(DaemonCmd::Mcp { .. })
            | Command::Teleport(_)
    ) || match &cli.command {
        Command::Doctor(d) => match &d.sub {
            Some(doctor::DoctorSub::Parity(p)) => [&p.a, &p.b].iter().any(|t| {
                !(t.starts_with("http") || t.starts_with("direct:") || t.starts_with("local:"))
            }),
            None => d.args.needs_fleet(),
        },
        _ => false,
    };
    // The default `cua sb ls` is an implicit Fleet call: it opens the
    // signed-in session only when the session marker says one exists, so it
    // never reads the OS credential vault otherwise.
    let needs_fleet = needs_fleet
        && match &cli.command {
            Command::Sandbox(cmd) => match cmd.as_ref() {
                sandbox::SandboxCmd::Ls { cloud, .. } => *cloud || cua_auth::may_have_session(),
                _ => true,
            },
            _ => true,
        };
    if let Some(d) = &cli.state_dir {
        let _ = sandbox::STATE_DIR.set(PathBuf::from(d));
    }
    // Ephemeral sandboxes of a process that died go before new ones come.
    if matches!(cli.command, Command::Sandbox(_)) {
        cache::quick_reap(cli.state_dir.as_deref()).await;
    }
    // Fleet sandboxes are held by the daemon so their claim heartbeat
    // outlives this command: start one when none runs.
    if let Command::Sandbox(cmd) = &cli.command
        && sandbox::wants_daemon(cmd)
        && !cli.embedded
        && cli.daemon.is_none()
        && std::env::var_os("CUA_NO_DAEMON_AUTOSTART").is_none()
        && let Err(e) = daemon_start(&cli, false, None, "127.0.0.1:0", &mut std::io::stderr()).await
    {
        eprintln!("note: could not start the cua daemon ({e}); continuing without it");
    }
    // `cua daemon mcp` serves the daemon's Spaces runtime: start one when
    // none runs (its chatter goes to stderr; stdout is the MCP channel).
    if matches!(cli.command, Command::Daemon(DaemonCmd::Mcp { .. }))
        && !cli.embedded
        && cli.daemon.is_none()
    {
        daemon_start(&cli, false, None, "127.0.0.1:0", &mut std::io::stderr()).await?;
    }
    let cua = open(&cli, needs_fleet).await?;
    match cli.command {
        Command::Sandbox(cmd) => return sandbox::run(&cua, *cmd, json, out).await,
        Command::Viewer { listen, no_open } => {
            return extension::viewer(cua.clone(), &listen, no_open, out).await;
        }
        Command::Agent(cmd) => return agent_cmd::run(&cua, cmd, json, out).await,
        Command::Volume(cmd) => return drive_cmd::run(&cua, cmd, json, out).await,
        Command::Teleport(cmd @ teleport::TeleportCmd::Push(_)) => {
            return extension::teleport(Some(&cua), cmd, json, out).await;
        }
        Command::Do(args) => return do_cmd::run(&cua, args, out).await,
        Command::Doctor(cmd) => return doctor::dispatch(&cua, *cmd, json, out).await,
        Command::DoHostConsent => return do_cmd::host_consent(&cua, out).await,
        Command::Mcp {
            permissions,
            sandbox,
        }
        | Command::Daemon(DaemonCmd::Mcp {
            permissions,
            sandbox,
        }) => {
            let perms = mcp::parse_permissions(&permissions);
            // Embedded (no daemon): the account's relay machines, like the daemon.
            host::attach_relay_account(&cua, || host::daemon_relay_account(&util::cua_home()));
            return mcp::serve_stdio(mcp::server(cua, perms, sandbox)).await;
        }
        Command::Skills(SkillsCmd::Record {
            sandbox: Some(sb),
            provider,
            model,
            api_key,
            name,
            description,
            ..
        }) => {
            let url = sandbox::viewer_link(&cua, &sb, None, false, None)
                .await?
                .url;
            return skills::cmd_record(url, provider, model, api_key, name, description, out).await;
        }
        Command::Spacesd(cmd) => match cmd {
            SpacesdCmd::Caps { url, token } => {
                let c = env_at(&cua, &url, token).await?.capabilities().await?;
                if json {
                    line(out, &c.json);
                } else {
                    line(
                        out,
                        format!(
                            "cua-spacesd {} ({} {} {}, protocol {}.{})",
                            c.version,
                            c.os_family,
                            c.os_version,
                            c.arch,
                            c.protocol_version,
                            c.protocol_revision
                        ),
                    );
                    for f in c.features {
                        line(
                            out,
                            format!(
                                "  {} {}{}",
                                if f.supported { "+" } else { "-" },
                                f.name,
                                f.limitation.map(|l| format!(" ({l})")).unwrap_or_default()
                            ),
                        );
                    }
                }
            }
            SpacesdCmd::Exec {
                url,
                token,
                command,
            } => {
                let env = env_at(&cua, &url, token).await?;
                return shell::exec(&env, &command, out).await;
            }
            SpacesdCmd::Cp {
                url,
                src,
                dst,
                token,
            } => {
                let env = env_at(&cua, &url, token).await?;
                let r = match (src.strip_prefix(':'), dst.strip_prefix(':')) {
                    (None, Some(remote)) => {
                        env.upload_file(src.clone(), remote.to_string(), None)
                            .await?
                    }
                    (Some(remote), None) => {
                        env.download_file(remote.to_string(), dst.clone()).await?
                    }
                    _ => {
                        return Err(CuaError::InvalidArgument(
                            "exactly one of src/dst must be :remote".into(),
                        ));
                    }
                };
                print(
                    out,
                    serde_json::json!({"size": r.size, "sha256": r.sha256}),
                    json,
                    || format!("{} bytes, sha256 {}", r.size, r.sha256),
                );
            }
            SpacesdCmd::Shell { url, token } => {
                let env = env_at(&cua, &url, token).await?;
                return shell::interactive(&env, None, None, None).await;
            }
            SpacesdCmd::Targets {
                url,
                token,
                windows,
            } => {
                let env = env_at(&cua, &url, token).await?;
                env_targets(&env, windows, json, out).await?;
            }
            SpacesdCmd::Call {
                url,
                method,
                body,
                token,
            } => {
                let env = env_at(&cua, &url, token).await?;
                line(out, env.call_json(method, body).await?);
            }
        },
        Command::Daemon(DaemonCmd::Status) => {
            let i = cua.info().await?;
            print(
                out,
                serde_json::json!({
                    "mode": format!("{:?}", i.mode).to_lowercase(),
                    "daemon_version": i.daemon_version,
                    "pid": i.daemon_pid,
                    "socket": i.socket_path,
                    "loopback": i.loopback_url,
                }),
                json,
                || match i.daemon_pid {
                    Some(pid) => format!(
                        "cua daemon {} (pid {pid}) socket={} loopback={}",
                        i.daemon_version.clone().unwrap_or_default(),
                        i.socket_path.clone().unwrap_or_default(),
                        i.loopback_url.clone().unwrap_or_default()
                    ),
                    None => "no cua daemon is running".into(),
                },
            );
            if i.daemon_pid.is_none() {
                return Ok(1);
            }
        }
        Command::Daemon(DaemonCmd::Stop) => {
            let Some(pid) = cua.info().await?.daemon_pid else {
                line(out, "no cua daemon is running");
                return Ok(1);
            };
            cua.shutdown_daemon().await?;
            // The daemon stops after answering: wait (bounded) until it has,
            // so a `cua daemon start` right after starts a new one instead of
            // finding this one on its way out (and the next command then
            // failing with a transport error).
            let deadline = std::time::Instant::now() + Duration::from_secs(15);
            while cua_daemon::client::live_address().is_some() {
                if std::time::Instant::now() >= deadline {
                    return Err(CuaError::Timeout(format!(
                        "the cua daemon (pid {pid}) still answers 15 s after it was asked to stop"
                    )));
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            // Then its exit (best effort: a parent that has not reaped it
            // yet keeps a zombie that looks alive).
            let exit_by = std::time::Instant::now() + Duration::from_secs(5);
            while cua_host::service::process_alive(pid) && std::time::Instant::now() < exit_by {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            line(out, "stopped");
        }
        Command::Runtime(RuntimeCmd::Doctor) => {
            let report = cua.local().doctor().await?;
            if json {
                line(out, &report.report_json);
            } else {
                for c in report.checks {
                    line(
                        out,
                        format!(
                            "{:<10} {:<12} {}{}",
                            c.name,
                            format!("{:?}", c.status).to_lowercase(),
                            c.version.map(|v| format!("[{v}] ")).unwrap_or_default(),
                            c.detail
                        ),
                    );
                }
            }
        }
        Command::Runtime(RuntimeCmd::Setup {
            dry_run,
            components,
        }) => {
            for s in cua.local().setup(components, dry_run).await? {
                line(out, format!("{:<10} {:?}: {}", s.name, s.status, s.detail));
            }
        }
        Command::Image(cmd) => {
            let local = cua.local();
            match cmd {
                ImageCmd::Pull { reference } => {
                    let i = local.pull_image(reference).await?;
                    print(
                        out,
                        serde_json::json!({"reference": i.reference, "kind": i.kind,
                            "location": i.location, "size_bytes": i.size_bytes}),
                        json,
                        || format!("{} ({}) -> {}", i.reference, i.kind, i.location),
                    );
                }
                ImageCmd::Build { spec, base, push } => {
                    let spec = std::fs::read_to_string(spec).map_err(util::internal)?;
                    line(out, local.build_image(spec, base, push).await?);
                }
                ImageCmd::Push {
                    reference,
                    destination,
                } => {
                    let d = local.push_image(reference, destination.clone()).await?;
                    line(out, format!("{destination}@{d}"));
                }
                _ => unreachable!("handled above"),
            }
        }
        Command::Auth(_)
        | Command::Agents(_)
        | Command::WifToken(_)
        | Command::Trajectory(_)
        | Command::Skills(_)
        | Command::Daemon(DaemonCmd::Start { .. })
        | Command::Host(_)
        | Command::Spaces(_)
        | Command::Cloud(_)
        | Command::Devices(_)
        | Command::Fleet(_)
        | Command::Teleport(_)
        | Command::Cache(_)
        | Command::Config(_)
        | Command::Telemetry(_)
        | Command::Keyvault(_)
        | Command::Images(_)
        | Command::DumpDocs { .. } => unreachable!("handled above"),
    }
    Ok(0)
}

/// `cua spacesd targets`: `StreamService.ListTargets`, one line per target.
async fn env_targets(
    env: &SpacesdClient,
    windows: bool,
    json: bool,
    out: &mut dyn Write,
) -> Result<(), CuaError> {
    use cua_proto::env::v1::{ListTargetsRequest, stream_target::Target};
    let r = env
        .inner()
        .stream()
        .list_targets(ListTargetsRequest {
            include_windows: windows,
            window_filter: None,
        })
        .await
        .map_err(|s| CuaError::from(cua_spacesd_client::Error::from(s)))?
        .into_inner();
    if json {
        line(
            out,
            serde_json::to_string(&r).map_err(|e| CuaError::Internal(e.to_string()))?,
        );
        return Ok(());
    }
    for t in &r.targets {
        let state = if t.available {
            String::new()
        } else {
            format!(" (unavailable: {})", t.limitation)
        };
        match &t.target {
            Some(Target::Display(d)) => {
                let size = d
                    .native_size
                    .as_ref()
                    .map(|s| format!(" {}x{}", s.width, s.height))
                    .unwrap_or_default();
                line(
                    out,
                    format!(
                        "display {}{}{}{state}",
                        d.id,
                        if d.primary { " primary" } else { "" },
                        size
                    ),
                );
            }
            Some(Target::Window(w)) => {
                let r = w.r#ref.clone().unwrap_or_default();
                let app = w.app.as_ref().map(|a| a.name.as_str()).unwrap_or("");
                line(
                    out,
                    format!("window {}@{} {app}: {:?}{state}", r.id, r.epoch, w.title),
                );
            }
            None => {}
        }
    }
    Ok(())
}

async fn daemon_start(
    cli: &Cli,
    foreground: bool,
    socket: Option<PathBuf>,
    loopback: &str,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let discovery = cua_daemon::default_discovery_path();
    if !foreground {
        if let Some(d) = cua_daemon::Discovery::read(&discovery)
            && Cua::connect(None, None)?.info().await.is_ok()
        {
            // A daemon without an extension this `cua` carries (an MIT
            // daemon when this is the Cua Spaces build) is replaced, so the
            // Spaces apps and their tools find what they ship with. So is,
            // for a `cua` inside an app bundle, any daemon but the bundle's
            // own build (another app's, or its own before a rebuild or
            // update): its Spaces would run with that app's build and
            // permissions.
            let missing = extension::missing_daemon_extensions().await;
            let why = if missing.is_empty() {
                stranger_daemon().await
            } else {
                Some(format!("it lacks {}", missing.join(", ")))
            };
            match why {
                None => {
                    line(out, format!("cua daemon already running (pid {})", d.pid));
                    return Ok(0);
                }
                Some(why) => {
                    line(
                        out,
                        format!("replacing the running cua daemon (pid {}): {why}", d.pid),
                    );
                    extension::stop_daemon(&discovery, d.pid).await?;
                }
            }
        }
        // The Cua Spaces `cua` runs the daemon when this build has no
        // extensions of its own and one is installed ([`extension`]).
        let exe = match (
            cua_daemon::extension::registered().is_empty(),
            extension::spaces_cli(),
        ) {
            (true, Some(spaces)) => spaces,
            _ => std::env::current_exe().map_err(util::internal)?,
        };
        let mut args = vec!["daemon".to_string(), "start".into(), "--foreground".into()];
        if let Some(s) = &socket {
            args.push("--socket".into());
            args.push(s.display().to_string());
        }
        args.push("--loopback".into());
        args.push(loopback.to_string());
        if let Some(d) = &cli.state_dir {
            args.push("--state-dir".into());
            args.push(d.clone());
        }
        // A discovery file of a daemon that exited goes (never a live one's);
        // the new daemon replaces a stale socket and discovery file itself.
        if let Some(d) = cua_daemon::Discovery::read(&discovery) {
            cua_daemon::remove_stale(&discovery, &d);
        }
        let child = std::process::Command::new(exe)
            .args(&args)
            // Who started it (`cua_daemon_started.mode`): the Spaces app
            // sets `app`; an autostart from the CLI is `background`.
            .env(
                "CUA_DAEMON_STARTED_BY",
                std::env::var("CUA_DAEMON_STARTED_BY").unwrap_or_else(|_| "cli".into()),
            )
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(util::internal)?;
        // Clients wait for it rather than run a runtime of their own while
        // it starts (it writes the same marker itself).
        let _starting = cua_daemon::StartingMarker::write(child.id());
        // Bounded wait for the discovery file.
        for _ in 0..100 {
            if let Some(d) = cua_daemon::Discovery::read(&discovery)
                && d.pid == child.id()
            {
                line(
                    out,
                    format!(
                        "cua daemon started (pid {}) socket={} loopback={}",
                        d.pid,
                        d.socket_path.unwrap_or_default(),
                        d.loopback_url.unwrap_or_default()
                    ),
                );
                return Ok(0);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        return Err(CuaError::Timeout(
            "the daemon did not start within 10 s".into(),
        ));
    }
    // Starting: clients wait for this daemon (until its discovery file is
    // written) instead of running a runtime of their own meanwhile.
    let starting = cua_daemon::StartingMarker::write(std::process::id());
    let runtime = {
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let cfg = CuaConfig {
            state_dir: cli.state_dir.clone(),
            // Test and CI hosts point teleport at a throwaway home; the
            // real daemon reads the user's own app sessions (on consent).
            teleport_home: env("CUA_SPACES_TELEPORT_HOME"),
            fleet_from_session: true,
            ..Default::default()
        };
        let cua = Cua::embedded(cfg)?;
        let runtime = cua
            .embedded_runtime()
            .cloned()
            .ok_or_else(|| CuaError::Internal("embedded runtime".into()))?;
        // Keep renewing the claims of managed Fleet sandboxes this machine
        // created (for example after a daemon restart).
        {
            let rt = runtime.clone();
            tokio::spawn(async move {
                let held = rt.adopt_managed_fleet_sandboxes().await;
                if !held.is_empty() {
                    eprintln!("cua daemon: holding managed Fleet sandboxes {held:?}");
                }
            });
        }
        // A local create the last daemon did not finish (it crashed or was
        // killed mid-create): register the Space or delete its sandbox, so
        // nothing runs that no Space lists.
        {
            let spaces = runtime.spaces().clone();
            tokio::spawn(async move {
                for r in spaces
                    .recover_interrupted_creates(Duration::from_secs(90))
                    .await
                {
                    eprintln!("cua daemon: interrupted create {}: {:?}", r.id, r.outcome);
                }
            });
        }
        // SpaceService / `cua daemon mcp` list the signed-in account's relay
        // machines (`CUA_DAEMON_NO_RELAY=1` turns that off).
        if env("CUA_DAEMON_NO_RELAY").is_none() {
            runtime
                .spaces()
                .set_relay(Some(host::daemon_relay_account(&util::cua_home())));
        }
        runtime
    };
    let mut cfg = cua_daemon::server::ServerConfig::default_paths();
    if let Some(s) = socket {
        cfg.socket_path = Some(s);
    }
    cfg.loopback = match loopback {
        "off" | "" => None,
        a => Some(
            a.parse()
                .map_err(|_| CuaError::InvalidArgument(format!("bad loopback address {a}")))?,
        ),
    };
    let health_runtime = runtime.clone();
    let h = cua_daemon::server::start(runtime, cfg).await;
    drop(starting);
    {
        use cua_telemetry::events::{self, Outcome};
        let mode = match std::env::var("CUA_DAEMON_STARTED_BY").as_deref() {
            Ok("app") => "app",
            Ok(_) => "background",
            Err(_) => "foreground",
        };
        let t = cua_telemetry::global();
        t.capture(events::daemon_started(
            mode,
            if h.is_ok() {
                Outcome::Ok
            } else {
                Outcome::Error
            },
        ));
        if h.is_ok() {
            // Aggregate health, at most hourly: uptime and how many
            // sandboxes it knows, both bucketed.
            tokio::spawn(async move {
                let started = std::time::Instant::now();
                let mut tick = tokio::time::interval(Duration::from_secs(3600));
                tick.tick().await;
                loop {
                    tick.tick().await;
                    let n = health_runtime
                        .list_filtered(None, false)
                        .await
                        .map(|l| l.sandboxes.len() as u64)
                        .unwrap_or(0);
                    let t = cua_telemetry::global();
                    t.capture(events::daemon_health(started.elapsed(), n));
                    t.flush(Duration::from_secs(3));
                }
            });
        } else {
            cua_telemetry::global().shutdown(Duration::from_millis(400));
        }
    }
    let h = h?;
    eprintln!(
        "cua daemon listening: socket={} loopback={}",
        h.socket_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
        h.loopback_url.clone().unwrap_or_default()
    );
    let trigger = h.shutdown_trigger();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = trigger.send(true);
    });
    h.wait().await;
    Ok(0)
}

/// Accepts the `CUA_SPACESD_*` spelling of every `CUA_ENV_*` variable (for
/// example `CUA_SPACESD_TOKEN` for `CUA_ENV_TOKEN`), and the interim
/// `CUA_GUESTD_*` spelling. Precedence: `CUA_SPACESD_*`, then
/// `CUA_GUESTD_*`, then `CUA_ENV_*`.
fn promote_spacesd_env_vars() {
    let vars: Vec<_> = std::env::vars_os().collect();
    for prefix in ["CUA_GUESTD_", "CUA_SPACESD_"] {
        for (key, value) in &vars {
            if let Some(rest) = key.to_str().and_then(|k| k.strip_prefix(prefix))
                && !rest.is_empty()
            {
                // SAFETY: runs first in `main`, before any other thread exists.
                unsafe { std::env::set_var(format!("CUA_ENV_{rest}"), value) };
            }
        }
    }
}

/// Cap of `~/.cua/logs/daemon.log` (and of each of its rotated copies).
const DAEMON_LOG_MAX: u64 = 10 << 20;
/// Rotated daemon logs kept (`daemon.log.1` .. `.3`): 40 MiB at most.
const DAEMON_LOG_KEEP: usize = 3;

/// Windows starts every child with all of the parent's inheritable handles,
/// and `cua` gets its stdio from its caller as inheritable pipes. A process
/// that outlives the command (`cua trajectory view`'s file server, a daemon
/// that `cua daemon start` spawns) then holds the caller's pipes open, and a
/// caller waiting for end of output waits until that process exits. Our own
/// stdio stops being inheritable here; a child that should share it still
/// gets it, because std duplicates inherited stdio explicitly.
#[cfg(windows)]
fn keep_stdio_out_of_children() {
    use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation};
    use windows_sys::Win32::System::Console::{
        GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };
    for id in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        // SAFETY: GetStdHandle returns this process's handle (or null or
        // INVALID_HANDLE_VALUE, which SetHandleInformation rejects harmlessly);
        // clearing its inherit flag does not close or move it.
        unsafe {
            let h = GetStdHandle(id);
            if !h.is_null() {
                SetHandleInformation(h, HANDLE_FLAG_INHERIT, 0);
            }
        }
    }
}

/// The `cua` command: parses this process's arguments, runs them and exits.
/// The Cua Spaces build registers its [`extension`] first.
pub fn main() {
    #[cfg(windows)]
    keep_stdio_out_of_children();
    promote_spacesd_env_vars();
    let matches = Cli::command().get_matches();
    let command = command_path(&matches);
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());
    let daemon_process = matches!(cli.command, Command::Daemon(DaemonCmd::Start { .. }));
    let telemetry = cua_telemetry::init(
        if daemon_process { "daemon" } else { "cli" },
        cua_sdk::VERSION,
    );
    let record = records_telemetry(&cli.command);
    let json_output = cli.json;
    // The first-run notice goes to stderr before anything could be sent
    // (not for `cua telemetry` / `cua config`, which are how you answer it).
    if record && !matches!(cli.command, Command::Config(_)) {
        telemetry.show_notice_if_needed();
    }
    let started = std::time::Instant::now();
    // The daemon logs to a size-capped, self-rotating file as well as
    // stderr (a background daemon's stderr goes nowhere).
    let daemon_log = matches!(
        cli.command,
        Command::Daemon(DaemonCmd::Start {
            foreground: true,
            ..
        })
    ) && std::env::var("CUA_DAEMON_LOG").as_deref() != Ok("off");
    let filter = || {
        tracing_subscriber::EnvFilter::try_from_env("CUA_LOG").unwrap_or_else(|_| {
            if daemon_log {
                "info".into()
            } else {
                "warn".into()
            }
        })
    };
    let file = daemon_log
        .then(|| {
            cua_vmm::disk::logs::RotatingFile::open(
                cua_disk::Layout::default().daemon_log(),
                DAEMON_LOG_MAX,
                DAEMON_LOG_KEEP,
            )
            .ok()
        })
        .flatten();
    match file {
        Some(f) => {
            use tracing_subscriber::fmt::writer::MakeWriterExt;
            let _ = tracing_subscriber::fmt()
                .with_env_filter(filter())
                .with_ansi(false)
                .with_writer(std::io::stderr.and(std::sync::Mutex::new(f)))
                .try_init();
        }
        None => {
            let _ = tracing_subscriber::fmt()
                .with_env_filter(filter())
                .with_writer(std::io::stderr)
                .try_init();
        }
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        // Tasks such as the MCP server's tool calls are deep async state
        // machines; unoptimized builds overflow tokio's 2 MiB default.
        .thread_stack_size(8 * 1024 * 1024)
        .enable_all()
        .build()
        .expect("tokio runtime");
    let code = rt.block_on(async {
        let mut out = std::io::stdout();
        let result = run(cli, &mut out).await;
        if record && !daemon_process {
            use cua_telemetry::events::{self, Outcome};
            // `cua config set telemetry off` must not report itself.
            telemetry.refresh();
            let (outcome, variant) = match &result {
                Ok(0) => (Outcome::Ok, None),
                Ok(_) => (Outcome::Error, None),
                Err(e) => (Outcome::Error, Some(e.variant())),
            };
            telemetry.capture(events::cli_command(
                &command,
                outcome,
                variant,
                started.elapsed(),
                json_output,
                TOPOLOGY.get().copied().unwrap_or("none"),
            ));
            // Retention: one `cua_app_active` per install per UTC day.
            telemetry.capture_active_day();
        }
        match result {
            Ok(c) => c,
            Err(e) => {
                let _ = out.flush();
                eprintln!("cua: {e}");
                exit_code(&e)
            }
        }
    });
    let _ = std::io::stdout().flush();
    // Telemetry never holds the exit up for more than this; what is left
    // goes to the offline spool.
    telemetry.shutdown(Duration::from_millis(400));
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every CLI command path is in the telemetry vocabulary, so a new
    /// command cannot silently report as `other` (or leak its name). With
    /// `CUA_WRITE_CLI_COMMANDS=1` the vocabulary file is regenerated.
    #[test]
    fn telemetry_command_vocabulary_matches_the_cli() {
        let paths = all_command_paths();
        let file = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../cua-telemetry/src/cli_commands.in"
        );
        if std::env::var_os("CUA_WRITE_CLI_COMMANDS").is_some() {
            let mut body = String::from(
                "// Generated by cua-cli's telemetry_command_vocabulary_matches_the_cli\n// (CUA_WRITE_CLI_COMMANDS=1 cargo test -p cua-cli telemetry_command).\n&[\n",
            );
            for p in &paths {
                body.push_str(&format!("    {p:?},\n"));
            }
            body.push_str("    \"other\",\n]\n");
            std::fs::write(file, body).unwrap();
        }
        for p in &paths {
            assert!(
                cua_telemetry::schema::CLI_COMMANDS.contains(&p.as_str()),
                "{p} is not in cua-telemetry's CLI_COMMANDS; run CUA_WRITE_CLI_COMMANDS=1 cargo test -p cua-cli telemetry_command"
            );
        }
        let m = Cli::command().get_matches_from(["cua", "auth", "status"]);
        assert_eq!(command_path(&m), "auth.status");
        let m = Cli::command().get_matches_from(["cua", "viewer"]);
        assert_eq!(command_path(&m), "viewer");
    }
    use cua_daemon::fixtures;

    /// Runs the CLI with a private `CUA_HOME`, so the embedded runtime never
    /// opens the developer's real `~/.cua` (keyvault, registry, state).
    async fn cli(args: &[&str]) -> (Result<i32, CuaError>, String) {
        let _env = util::test_env::isolated();
        let cli = Cli::try_parse_from(std::iter::once("cua").chain(args.iter().copied())).unwrap();
        let mut out = Vec::new();
        // Boxed: the whole command tree's state machine is too large for a
        // test thread's stack in an unoptimized Windows build.
        let r = Box::pin(run(cli, &mut out)).await;
        (r, String::from_utf8(out).unwrap())
    }

    #[test]
    fn cli_definition_is_consistent() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }

    #[test]
    fn documented_exit_codes_cover_every_error() {
        let documented: Vec<i32> = EXIT_CODES.iter().map(|(c, _)| *c).collect();
        let s = String::new;
        for e in [
            CuaError::InvalidArgument(s()),
            CuaError::AmbiguousSandbox(s()),
            CuaError::NotFound(s()),
            CuaError::Unsupported(s()),
            CuaError::ProviderNotConfigured(s()),
            CuaError::SpacesdNotAvailable(s()),
            CuaError::Transport(s()),
            CuaError::DaemonNotRunning(s()),
            CuaError::Unauthenticated(s()),
            CuaError::PermissionDenied(s()),
            CuaError::FleetAdmissionDenied(s()),
            CuaError::CloudCreditExhausted(s()),
            CuaError::Cloud(s()),
            CuaError::InsufficientDisk(s()),
            CuaError::ImageNotPublished(s()),
            CuaError::Cancelled(s()),
            CuaError::Timeout(s()),
        ] {
            assert!(
                documented.contains(&exit_code(&e)),
                "{e:?} exits undocumented"
            );
        }
    }

    #[test]
    fn pool_apply_and_export_parse() {
        let parse =
            |a: &[&str]| Cli::try_parse_from(std::iter::once("cua").chain(a.iter().copied()));
        assert!(
            parse(&["sandbox", "create", "img", "--apply"]).is_err(),
            "--apply needs --pool"
        );
        assert!(parse(&["sandbox", "create", "--pool", "p", "--name", "n", "--apply"]).is_ok());
        let cli = parse(&["fleet", "pool", "export", "p", "--terraform"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Fleet(FleetCmd::Pool(PoolCmd::Export { ref name, terraform: true })) if name == "p"
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn spacesd_commands_against_the_mock() {
        let env = fixtures::start_env(Some("t"), None).await;
        let dir = tempfile::tempdir().unwrap();
        let (r, out) = cli(&["--embedded", "spacesd", "caps", &env.url, "--token", "t"]).await;
        assert_eq!(r.unwrap(), 0);
        assert!(out.starts_with("cua-spacesd "), "{out}");
        // No aliases: `env` (the env-driver era) and `guest` are gone.
        for old in ["env", "guest"] {
            assert!(
                Cli::try_parse_from(["cua", old, "caps", "x"]).is_err(),
                "{old}"
            );
        }
        let (r, out) = cli(&[
            "--embedded",
            "spacesd",
            "exec",
            &env.url,
            "--token",
            "t",
            "echo",
            "hi",
        ])
        .await;
        assert_eq!(r.unwrap(), 0);
        assert_eq!(out, "hi\n");
        let (r, _) = cli(&[
            "--embedded",
            "spacesd",
            "exec",
            &env.url,
            "--token",
            "t",
            "fail",
            "7",
        ])
        .await;
        assert_eq!(r.unwrap(), 7);
        let local = dir.path().join("f.txt");
        std::fs::write(&local, b"payload").unwrap();
        let (r, out) = cli(&[
            "--embedded",
            "--json",
            "spacesd",
            "cp",
            &env.url,
            local.to_str().unwrap(),
            ":/tmp/f.txt",
            "--token",
            "t",
        ])
        .await;
        assert_eq!(r.unwrap(), 0);
        assert!(out.contains("\"size\":7"), "{out}");
        assert_eq!(env.mock.state.file("/tmp/f.txt").unwrap(), b"payload");
        let (r, _) = cli(&["--embedded", "spacesd", "targets", &env.url, "--token", "t"]).await;
        assert_eq!(r.unwrap(), 0);
        let (r, out) = cli(&[
            "--embedded",
            "--json",
            "spacesd",
            "targets",
            &env.url,
            "--token",
            "t",
            "--windows",
        ])
        .await;
        assert_eq!(r.unwrap(), 0);
        assert!(out.starts_with('{'), "{out}");
        let (r, out) = cli(&[
            "--embedded",
            "spacesd",
            "call",
            &env.url,
            "SystemService/Health",
            "--token",
            "t",
        ])
        .await;
        assert_eq!(r.unwrap(), 0);
        assert!(out.starts_with('{'));
    }

    #[tokio::test]
    #[ignore = "host: probes the developer's lume, qemu and Docker; run with --ignored"]
    async fn runtime_doctor_is_read_only_and_reports_backends() {
        let (r, out) = cli(&["--embedded", "runtime", "doctor"]).await;
        assert_eq!(r.unwrap(), 0);
        assert!(out.contains("qemu") && out.contains("container"), "{out}");
        let (r, out) = cli(&["--embedded", "runtime", "setup", "--dry-run", "qemu"]).await;
        assert_eq!(r.unwrap(), 0);
        assert!(out.starts_with("qemu"), "{out}");
    }
}
