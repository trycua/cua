//! `cua`: the Cua command line, on the cua SDK (`cua-sdk` as a Rust
//! library).
//!
//! - `auth`: Cua sign-in (browser or device login, client credentials) and
//!   contrib provider keys;
//! - `agents`: set up AI coding agents (cua skills, the cua MCP server);
//! - `sandbox` (`sb`): local and direct sandboxes;
//! - `do`, `do-host-consent`: one-shot computer actions for agents;
//! - `mcp` (`serve-mcp`): stdio MCP server for AI assistants;
//! - `skills`, `trajectory` (`traj`): recorded demonstrations and action
//!   trajectories;
//! - `image` (`img`): local OCI images;
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

#![recursion_limit = "512"]

#[cfg(feature = "byoc")]
mod add_aws;
mod agent_cmd;
mod agent_keys_cmd;
mod agent_session_cmd;
mod agents;
mod auth;
mod bench_cmd;
mod browse;
mod build_cmd;
mod cache;
mod catalog;
mod cloud_cmd;
mod compat_cmd;
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
mod jobs_cmd;
pub mod keyvault_cmd;
mod mcp;
mod member_cmd;
pub mod onboard;
mod persistent_cmd;
pub mod prewarm;
mod providers;
mod resources_view;
mod risky;
mod runs;
pub mod sandbox;
mod share_cmd;
mod shell;
mod skills;
#[doc(hidden)]
pub mod stream_probe;
mod team_cmd;
mod telemetry_cmd;
pub mod teleport;
pub mod teleport_session;
mod trajectory;
#[cfg(test)]
mod upgrade_070_tests;
mod usage_cmd;
pub mod util;

use clap::{ArgAction, CommandFactory, FromArgMatches, Parser, Subcommand};
use cua_sdk::{Cua, CuaConfig, CuaError, SpacesdClient};
use std::{io::Write, path::PathBuf, sync::Arc, time::Duration};
use util::line;

/// The log filter spec for `CUA_LOG` (`None`: not set): the spec, else
/// warnings (info in the background daemon), without the WireGuard library's
/// own timer messages (`boringtun`) unless the spec names it. It logs a
/// warning every few seconds while a mesh peer that went away is retried and
/// an error when it gives up: normal churn that says nothing a user can act
/// on. `CUA_LOG=warn,boringtun=debug` brings them back.
fn log_filter_spec(env: Option<&str>, daemon_log: bool) -> String {
    let chosen = env.map(str::trim).filter(|s| !s.is_empty());
    let base = chosen.unwrap_or(if daemon_log { "info" } else { "warn" });
    let mut spec = if base.contains("boringtun") {
        base.to_owned()
    } else {
        format!("{base},boringtun=off")
    };
    // A fallback to the relay's HTTPS path is shown once as a notice
    // (`fallback_notice`); the default log need not repeat it.
    if chosen.is_none() && !daemon_log {
        spec.push_str(&format!(",{}=error", cua_spaces::mesh::FALLBACK_LOG_TARGET));
    }
    spec
}

#[derive(Parser, Debug)]
#[command(
    name = "cua",
    version = cua_sdk::VERSION,
    disable_version_flag = true,
    about = "Cua: sandboxes, computer actions, MCP, skills and the cua daemon",
    after_help = "Remote Spaces use the end-to-end encrypted mesh when both ends and the relay\nsupport it, and the relay's HTTPS path otherwise. CUA_MESH=off turns the mesh\noff; CUA_MESH=require refuses any other path.\n\nDocs: https://cua.ai/docs/cua-cli/reference/cli"
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

/// Former command names that still work, and the command each is now
/// (`cua dump-docs` marks them `deprecated` with `replaced_by`).
pub(crate) const DEPRECATED_COMMANDS: &[(&str, &str)] = &[
    ("cua agents", "cua connect"),
    ("cua agent create", "cua agent add"),
    ("cua agent persistent", "cua agent ls"),
    ("cua agent tell", "cua agent send"),
    ("cua agent forget", "cua agent rm"),
    ("cua agent routine", "cua agent schedule"),
];

#[derive(Subcommand, Debug)]
enum Command {
    /// Log in to Cua and manage the signed-in session.
    #[command(subcommand)]
    Auth(AuthCmd),
    /// Connect the AI agents on this computer to Cua: install the cua skills
    /// and configure the cua MCP server.
    #[command(subcommand)]
    Connect(agents::AgentsCmd),
    /// Deprecated alias of `connect`.
    #[command(subcommand)]
    Agents(agents::AgentsCmd),
    /// Share a link to a port, a file or a folder that opens on your other
    /// signed-in devices (never localhost): `cua share 3000`, `cua share
    /// ./report.html`, `cua share ls`, `cua share stop ID`.
    Share(share_cmd::ShareArgs),
    /// Agents in a Space: add, send, ls, rm, schedule; run, logs, stop.
    #[command(subcommand)]
    Agent(agent_cmd::AgentCmd),
    /// Sandboxes: local or an existing computer (direct).
    #[command(subcommand, visible_alias = "sb")]
    Sandbox(Box<sandbox::SandboxCmd>),
    /// User defaults (~/.cua/config.toml): where sandboxes run when --on is
    /// not given (default.on), the default kind and runtime.
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
  cua do screenshot
  # A relay Space (`cua spaces ls`)
  cua do switch relay:0123abcd4567ef89")]
    Do(do_cmd::DoArgs),
    /// Grant consent for `cua do switch host`.
    #[command(
        name = "do-host-consent",
        after_help = "Examples:
  # Allow `cua do` to act on this computer's desktop
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
  cua mcp --permissions sandbox:readonly,computer:all --sandbox dev
  # An agent that joined a Space with cua join: its thread tools only
  cua mcp --member ubuntu-build/codex"
    )]
    Mcp {
        /// Comma-separated permissions or groups (default: all).
        #[arg(long, env = "CUA_MCP_PERMISSIONS", default_value = "")]
        permissions: String,
        /// Default sandbox for computer tools.
        #[arg(long, env = "CUA_SANDBOX", default_value = "")]
        sandbox: String,
        /// Serve only the Space thread tools (thread_join, thread_inbox,
        /// thread_read, thread_post, agent_members) as the agent this
        /// computer joined with `cua join` (`<space>/<handle>`). Its key
        /// stays in the keychain.
        #[arg(long, conflicts_with_all = ["permissions", "sandbox"])]
        member: Option<String>,
    },
    /// Join a Space as an agent on this computer, with the code from a
    /// "Join my Cua Space" link. Needs your Cua sign-in; the code works once.
    Join(member_cmd::JoinArgs),
    /// A Space's thread, as the agent this computer joined with `cua join`:
    /// say who you are, read your inbox, post.
    #[command(subcommand)]
    Thread(member_cmd::ThreadCmd),
    /// Recorded demonstrations (skills) for agents.
    #[command(subcommand)]
    Skills(SkillsCmd),
    /// Recorded `cua do` trajectories.
    #[command(subcommand, visible_alias = "traj")]
    Trajectory(TrajCmd),
    /// Removed: Cua Cloud has ended.
    #[command(name = "wif-token", hide = true, disable_help_flag = true)]
    WifToken(util::EndedArgs),
    /// Removed: Cua Cloud has ended.
    #[command(hide = true, disable_help_flag = true)]
    Fleet(util::EndedArgs),
    /// Removed: Cua Cloud has ended.
    #[command(hide = true, disable_help_flag = true)]
    Pools(util::EndedArgs),
    /// Build an image from a Dockerfile, in a Space, as pinned checkpoints
    /// that Spaces fork from in under a second. Works in any directory;
    /// layers already built are reused.
    Build(build_cmd::BuildArgs),
    /// Images: local OCI pull/build/push.
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
    /// Run another provider's SDK unchanged on Cua Spaces: a local gateway
    /// that speaks its wire protocol (`cua compat serve --provider e2b`).
    #[command(subcommand)]
    Compat(compat_cmd::CompatCmd),
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
  cua doctor local:dev --strict --junit doctor.xml")]
    Doctor(Box<doctor::DoctorCmd>),
    /// Run a Python function on your pool: `FILE::FN`, decorated with
    /// `@app.function(...)` (the `cua` jobs shim). Each member runs in a
    /// Space (a macOS VM with GPU on a Mac, the computer's own Space on a
    /// dedicated host, a container elsewhere); a lost computer re-places the
    /// gang and the job resumes from its checkpoints. Rank 0's result is
    /// saved in ./cua-runs/<run>/.
    #[command(after_help = "Examples:
  cua run train.py::finetune
  cua run train.py::finetune --nodes 1:2 --accelerator gpu
  cua run train.py::finetune --on host:aws-1 --detach")]
    Run(jobs_cmd::RunArgs),
    /// Runs on your pool: the list, one run's members and cost, its logs,
    /// cancel, and its result.
    #[command(after_help = "Examples:
  cua jobs
  cua jobs p-finetune-1a2b3c4d
  cua jobs logs -f p-finetune-1a2b3c4d --rank 0
  cua jobs cancel p-finetune-1a2b3c4d
  cua jobs result p-finetune-1a2b3c4d -o ./adapter")]
    Jobs(jobs_cmd::JobsArgs),
    /// The computers in your pool, as its coordinator sees them now: state,
    /// OS/arch, load, jobs, accelerators and $/h, with the queue.
    #[command(after_help = "Examples:
  cua computers
  cua computers --json")]
    Computers,
    /// Unattended access to this computer (setup, status, stop, start, remove).
    #[command(subcommand)]
    Host(host::HostCmd),
    /// Spaces: registered ones and your computers on the relay.
    #[command(subcommand)]
    Spaces(host::SpacesCmd),
    /// Spaces in your own cloud account (AWS, Google Cloud, Hetzner, Modal):
    /// connect, status, test, disconnect, sweep; and pool hosts there
    /// (`cua cloud add aws`).
    #[command(
        subcommand,
        after_help = "Examples:
  cua cloud status
  cua cloud connect aws --region us-west-2 --default
  cua spaces create --on aws
  cua cloud add aws --count 2 --type g6.xlarge
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
    /// Teams (organizations of your cua.ai account) share computers with
    /// every member and their agents. Owners and admins invite by email,
    /// approve member devices and add computers.
    #[command(
        subcommand,
        after_help = "Examples:
  cua team create AIGIS
  cua team invite AIGIS dillon@example.com
  cua team add-machine AIGIS local:build-box
  cua team approve AIGIS dev_0123456789abcdef01234567
  cua team show AIGIS"
    )]
    Team(team_cmd::TeamCmd),
    /// Your remote use this month: live sessions, remote hours, and your
    /// plan. Remote hours are time spent using your computers from
    /// somewhere else; using a computer from itself is free.
    #[command(after_help = "Examples:
  cua usage
  cua usage --json")]
    Usage {
        /// Relay URL (default $CUA_RELAY_URL, else the CUA_ENV relay, https://relay.cua.ai in production).
        #[arg(long, hide = true)]
        relay: Option<String>,
    },
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
    /// Spaces performance benchmarks: list them, run one in Spaces of its
    /// own (writes result.json and samples.csv in the bench-result v1
    /// format), and check a run against a committed baseline.
    #[command(
        subcommand,
        after_help = "Examples:
  cua bench list
  cua bench run microvm-fork --n 5 --out ./mf
  cua bench check ./mf/result.json --baseline microvm-fork/baseline.json"
    )]
    Bench(bench_cmd::BenchCmd),
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
        /// computers without a browser).
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
    /// Who is signed in: the account, its plan and its teams.
    #[command(after_help = "Examples:
  cua auth whoami
  cua auth whoami --json")]
    Whoami,
    /// Whether this cua can read the Cua items in the macOS keychain
    /// without a prompt (JSON; for the Spaces app, which asks before it
    /// starts anything that reads the session). A Cua-signed cua gives
    /// every item it reads the Cua access list, so later Cua builds read it
    /// without a prompt.
    #[command(hide = true)]
    Keychain {
        /// Let macOS ask for access (its keychain prompt) where needed.
        #[arg(long)]
        prompt: bool,
        /// Remove the stored session without reading it (to sign in again).
        #[arg(long, conflicts_with = "prompt")]
        forget: bool,
    },
    /// Removed: Cua Cloud has ended.
    #[command(hide = true, disable_help_flag = true)]
    Keys(util::EndedArgs),
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
        #[arg(value_name = "TARGET")]
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
        #[arg(long, visible_alias = "computer", value_name = "COMPUTER")]
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
  cua spacesd exec http://127.0.0.1:3211 uname -a
  # A long-lived stdio stream (stdin forwarded, no time limit, killed with this command)
  cua spacesd exec -i http://127.0.0.1:3211 cat")]
    Exec {
        #[command(flatten)]
        flags: shell::ExecFlags,
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
    /// End the VMs nobody uses: a Space whose lifetime passed, a warm-pool
    /// member of a daemon that is gone, a VM nothing records or whose home
    /// was deleted. Runs without a daemon (a service manager can run it on
    /// a timer); a home with a running daemon is left to that daemon.
    #[command(after_help = "Examples:
  # See what would be ended
  cua daemon reap --dry-run
  cua daemon reap")]
    Reap {
        /// Only report what would be ended.
        #[arg(long)]
        dry_run: bool,
    },
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
    /// Inspect local runtimes (read-only). Exit 1 if any check is Error; warnings exit 0.
    #[command(after_help = "Examples:
  cua runtime doctor")]
    Doctor,
    /// Provision local runtimes. Exit 1 if any step is Error; warnings exit 0.
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
    /// Removed: Cua Cloud has ended.
    #[command(alias = "list", hide = true, disable_help_flag = true)]
    Ls(util::EndedArgs),
    /// Removed: Cua Cloud has ended.
    #[command(hide = true, disable_help_flag = true)]
    Info(util::EndedArgs),
    /// Removed: Cua Cloud has ended.
    #[command(alias = "delete", hide = true, disable_help_flag = true)]
    Rm(util::EndedArgs),
    /// Removed: Cua Cloud has ended.
    #[command(hide = true, disable_help_flag = true)]
    Create(util::EndedArgs),
}

#[derive(Subcommand, Debug)]
enum ImagesCmd {
    /// List the catalog: ref, OS, container or VM, local runtime,
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
        "Invalid argument, or an ambiguous sandbox name (qualify it: `local:NAME`)",
    ),
    (
        3,
        "Not found: sandbox, window, skill or image (or an image not published yet)",
    ),
    (
        4,
        "Not supported, or not configured (for example no provider credentials)",
    ),
    (5, "No cua-spacesd answered, or a transport failure"),
    (
        6,
        "Unauthenticated or permission denied (by Cua or your cloud account)",
    ),
    (7, "Not enough free disk space (see `cua cache`)"),
    (
        8,
        "No room on the machine: its VMs are at their memory or CPU limit, or it is low on memory",
    ),
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
        CuaError::Unauthenticated(_) | CuaError::PermissionDenied(_) | CuaError::Cloud(_) => 6,
        CuaError::InsufficientDisk(_) => 7,
        CuaError::HostCapacity(_) => 8,
        CuaError::Cancelled(_) => 130,
        _ => 1,
    }
}

/// For a `cua` inside an app bundle: what to do with the running daemon
/// ([`cua_daemon::identity::verdict`]: its own, another app's of the same or
/// a newer version to keep, one this app yields to, or a stranger to
/// replace), or `None` when this
/// `cua` is not in a bundle or no daemon answers.
async fn stranger_daemon() -> Option<cua_daemon::identity::Verdict> {
    let own = cua_daemon::identity::bundled_cua()?;
    let info = cua_daemon::client::existing_daemon()
        .await
        .ok()?
        .info()
        .await
        .ok()?;
    Some(cua_daemon::identity::verdict(
        &info.version,
        &info.executable,
        &info.build_id,
        &own,
    ))
}

/// The SDK handle. `with_session` passes the `cua auth login` session to
/// an embedded runtime (relay and hosted Spaces features); the token
/// refreshes itself.
async fn open(cli: &Cli, with_session: bool) -> Result<Arc<Cua>, CuaError> {
    let embedded = move || {
        Cua::embedded(CuaConfig {
            state_dir: cli.state_dir.clone(),
            account_from_session: with_session,
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
    // `cua auth keychain` is the Spaces app's own launch check: no usage
    // event, and never the first-run notice (the app shows its own).
    !matches!(
        c,
        Command::Telemetry(_) | Command::DumpDocs { .. } | Command::Auth(AuthCmd::Keychain { .. })
    )
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
    // Teams turned off (CUA_EXPERIMENTAL_TEAMS=0): refused before any
    // sign-in, daemon or relay call.
    if uses_teams(&cli.command) && !cua_home::experimental::teams_enabled() {
        return Err(CuaError::InvalidArgument(
            cua_home::experimental::TEAMS_UNAVAILABLE.into(),
        ));
    }
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
                        caps: None,
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
                AuthCmd::Keychain { prompt, forget } => auth::keychain(*prompt, *forget, out),
                AuthCmd::Keys(_) => Err(util::cloud_ended()),
                AuthCmd::Provider(ProviderCmd::Ls) => providers::list(json, out),
                AuthCmd::Provider(ProviderCmd::Set { name, var }) => {
                    providers::set(name, var.as_deref(), out)
                }
                AuthCmd::Provider(ProviderCmd::Rm { name }) => providers::remove(name, out),
            };
        }
        Command::Connect(a) | Command::Agents(a) => {
            return match a {
                agents::AgentsCmd::Detect => agents::cmd_detect(json, out),
                agents::AgentsCmd::Setup(args) => agents::cmd_setup(args, json, out),
                agents::AgentsCmd::Status => agents::cmd_status(json, out),
                agents::AgentsCmd::Remove(args) => agents::cmd_remove(args, json, out),
                agents::AgentsCmd::Update => agents::cmd_update(json, out),
            };
        }
        // Removed Cua Cloud commands (hidden): the one ended message, before
        // any sign-in, daemon or network call.
        Command::Fleet(_)
        | Command::Pools(_)
        | Command::WifToken(_)
        | Command::Image(
            ImageCmd::Ls(_) | ImageCmd::Info(_) | ImageCmd::Rm(_) | ImageCmd::Create(_),
        ) => return Err(util::cloud_ended()),
        Command::Sandbox(cmd) if matches!(**cmd, sandbox::SandboxCmd::KeepAlive(_)) => {
            return Err(util::cloud_ended());
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
        Command::Bench(cmd) => return bench_cmd::run(cmd.clone(), json, out),
        Command::Keyvault(cmd) => return extension::keyvault(cmd.clone(), json, out).await,
        Command::Share(args) => return share_cmd::run(args.clone(), json, out).await,
        Command::Compat(cmd) => return compat_cmd::run(cmd.clone(), json, out).await,
        Command::Cache(cmd) => {
            let state_dir = cli.state_dir.clone();
            return cache::run(cmd.clone(), state_dir.as_deref(), json, out).await;
        }
        Command::Daemon(DaemonCmd::Start {
            foreground,
            socket,
            loopback,
        }) => return daemon_start(&cli, *foreground, socket.clone(), loopback, out).await,
        Command::Daemon(DaemonCmd::Reap { dry_run }) => {
            let home = cua_daemon::cua_home();
            let serving = cua_daemon::guard::daemon_alive(&home);
            let ended = cua_daemon::guard::sweep(
                &home,
                serving,
                &cua_daemon::guard::Policy::default(),
                *dry_run,
            )
            .await;
            print(
                out,
                serde_json::json!({ "dry_run": dry_run, "ended": ended }),
                json,
                || {
                    if ended.is_empty() {
                        return "no VM to end".to_string();
                    }
                    ended
                        .iter()
                        .map(|e| {
                            format!(
                                "{} {} (pid {}, {:?}){}",
                                if *dry_run { "would end" } else { "ended" },
                                e.name,
                                e.pid,
                                e.reason,
                                e.error
                                    .as_ref()
                                    .map(|x| format!(": {x}"))
                                    .unwrap_or_default()
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                },
            );
            return Ok(i32::from(ended.iter().any(|e| e.error.is_some())));
        }
        Command::Teleport(
            cmd @ (teleport::TeleportCmd::Providers | teleport::TeleportCmd::Manifest(_)),
        ) => {
            return extension::teleport(None, cmd.clone(), json, out).await;
        }
        _ => {}
    }
    match cli.command {
        Command::Run(a) => return jobs_cmd::run(a, json, out).await,
        Command::Jobs(a) => return jobs_cmd::jobs(a, json, out).await,
        Command::Computers => return runs::computers(json, out).await,
        Command::Host(cmd) => {
            let home = util::cua_home();
            // An uninstall turns off the Spaces this machine provides first
            // (each is on the relay as its own machine), best effort.
            if matches!(
                cmd,
                host::HostCmd::Remove {
                    uninstalling: true,
                    ..
                }
            ) {
                let rest = Cli {
                    command: Command::Spaces(host::SpacesCmd::Ls {
                        relay: None,
                        label: vec![],
                        no_size: false,
                    }),
                    ..cli
                };
                host::turn_off_provided_spaces(&home, || open(&rest, false)).await;
            }
            let flow = if matches!(&cmd, host::HostCmd::Setup { remote: true, .. }) {
                cua_auth::Flow::Device
            } else {
                cua_auth::Flow::Auto
            };
            let tokens = host::SetupTokens::session_or_sign_in(flow);
            let device = devices_cmd::device_auth(&host::relay_url(None, &home)).ok();
            let code =
                host::run_host(cmd, &home, &tokens, device.as_deref(), None, json, out).await?;
            if tokens.used_ephemeral() && !json {
                eprintln!(
                    "Signed in for this command only: this computer keeps no cua.ai session, only its own access token."
                );
            }
            Ok(code)
        }
        Command::Devices(cmd) => {
            return devices_cmd::run(cmd, &util::cua_home(), json, out).await;
        }
        // Adding a machine is sharing the Space with the team: the same
        // path as `cua spaces share <space> team:<id>` (attach, confirm).
        Command::Team(team_cmd::TeamCmd::AddMachine { team, space, yes }) => {
            let id = team_cmd::resolve_team(&util::cua_home(), &team).await?;
            let share = Cli {
                command: Command::Spaces(host::SpacesCmd::Share {
                    space,
                    who: format!("team:{id}"),
                    role: "editor".into(),
                    yes,
                }),
                ..cli
            };
            return Box::pin(run(share, out)).await;
        }
        Command::Team(cmd) => {
            return team_cmd::run(cmd, &util::cua_home(), json, out).await;
        }
        Command::Usage { relay } => {
            return usage_cmd::run(relay, &util::cua_home(), json, out).await;
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
            | host::SpacesCmd::Suspend { .. }
            | host::SpacesCmd::Resize { .. }
            | host::SpacesCmd::Start { .. }
            | host::SpacesCmd::Egress { .. }
            | host::SpacesCmd::Prewarm { .. }
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
                    | host::SpacesCmd::Egress { .. }
                    | host::SpacesCmd::Prewarm { .. }
            );
            let rest = Cli {
                command: Command::Spaces(host::SpacesCmd::Ls {
                    relay: None,
                    label: vec![],
                    no_size: false,
                }),
                ..cli
            };
            let cua = open(&rest, false).await?;
            if sharing {
                // Sharing goes through the account's relay (the daemon
                // already has it; an embedded runtime gets it here), and
                // an embedded runtime confirms on the terminal.
                host::attach_relay_account(&cua, || host::daemon_relay_account(&util::cua_home()));
                let yes = matches!(cmd, host::SpacesCmd::Share { yes: true, .. });
                if let Some(rt) = cua.embedded_runtime() {
                    if !rt.spaces().has_share_consent() {
                        rt.spaces()
                            .set_share_consent(Some(Arc::new(host::TerminalConsent { yes })));
                    }
                } else if matches!(cmd, host::SpacesCmd::Share { .. }) {
                    // The daemon asks for user presence on this computer.
                    use std::io::IsTerminal as _;
                    host::daemon_share_gate(yes, std::io::stdin().is_terminal())?;
                    eprintln!("{}", host::share_confirm_prompt());
                }
            }
            return host::run_spaces_sdk(&cua, cmd, json, out).await;
        }
        Command::Build(a) => {
            let rest = Cli {
                command: Command::Spaces(host::SpacesCmd::Ls {
                    relay: None,
                    label: vec![],
                    no_size: false,
                }),
                ..cli
            };
            let cua = open(&rest, false).await?;
            return build_cmd::run(&cua, a, json, out).await;
        }
        Command::Cloud(cloud_cmd::CloudCmd::Add {
            cloud: cloud_cmd::AddCloud::Aws(a),
        }) => return add_aws(a, json, out).await,
        Command::Cloud(cloud_cmd::CloudCmd::Trust { cloud }) => {
            return cloud_trust(&cloud, out).await;
        }
        Command::Cloud(cmd) => {
            let rest = Cli {
                command: Command::Spaces(host::SpacesCmd::Ls {
                    relay: None,
                    label: vec![],
                    no_size: false,
                }),
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
    let needs_session = matches!(
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
            None => d.args.needs_session(),
        },
        _ => false,
    };
    // `cua sb ls` lists what this machine knows: it never reads the OS
    // credential vault.
    let needs_session = needs_session
        && !matches!(&cli.command, Command::Sandbox(cmd) if matches!(cmd.as_ref(), sandbox::SandboxCmd::Ls { .. }));
    if let Some(d) = &cli.state_dir {
        let _ = sandbox::STATE_DIR.set(PathBuf::from(d));
    }
    // Ephemeral sandboxes of a process that died go before new ones come.
    if matches!(cli.command, Command::Sandbox(_)) {
        cache::quick_reap(cli.state_dir.as_deref()).await;
    }
    // `cua daemon mcp` serves the daemon's Spaces runtime: start one when
    // none runs (its chatter goes to stderr; stdout is the MCP channel).
    if matches!(cli.command, Command::Daemon(DaemonCmd::Mcp { .. }))
        && !cli.embedded
        && cli.daemon.is_none()
    {
        daemon_start(&cli, false, None, "127.0.0.1:0", &mut std::io::stderr()).await?;
    }
    let cua = open(&cli, needs_session).await?;
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
            member: Some(member),
            ..
        } => {
            return mcp::serve_stdio(member_cmd::member_server(cua, &member)?).await;
        }
        Command::Join(args) => return member_cmd::join(&cua, args, json, out).await,
        Command::Thread(cmd) => return member_cmd::thread(&cua, cmd, json, out).await,
        Command::Mcp {
            permissions,
            sandbox,
            member: None,
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
                flags,
                url,
                token,
                command,
            } => {
                let env = env_at(&cua, &url, token).await?;
                if flags.stdin {
                    return shell::stream(
                        &env,
                        &command,
                        flags.opts(None),
                        tokio::io::stdin(),
                        shell::os_signals(),
                        out,
                        &mut std::io::stderr(),
                    )
                    .await;
                }
                return shell::exec_with(&env, &command, flags.opts(None), out).await;
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
                        env.download_file(remote.to_string(), dst.clone(), None)
                            .await?
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
                line(out, env.call_json(method, body, None).await?);
            }
        },
        Command::Daemon(DaemonCmd::Status) => {
            let i = cua.info().await?;
            // The warm pool, from the daemon itself.
            let warm = match i.daemon_pid {
                Some(_) => match cua_daemon::client::existing_daemon().await {
                    Ok(d) => d
                        .info()
                        .await
                        .ok()
                        .map(|r| (r.warm_spaces, r.warm_memory_bytes, r.warm_reason)),
                    Err(_) => None,
                },
                None => None,
            };
            print(
                out,
                serde_json::json!({
                    "mode": format!("{:?}", i.mode).to_lowercase(),
                    "daemon_version": i.daemon_version,
                    "pid": i.daemon_pid,
                    "socket": i.socket_path,
                    "loopback": i.loopback_url,
                    "warm_spaces": warm.as_ref().map(|w| w.0),
                    "warm_memory_bytes": warm.as_ref().map(|w| w.1),
                    "warm_reason": warm.as_ref().map(|w| w.2.clone()),
                }),
                json,
                || match i.daemon_pid {
                    Some(pid) => {
                        let mut s = format!(
                            "cua daemon {} (pid {pid}) socket={} loopback={}",
                            i.daemon_version.clone().unwrap_or_default(),
                            i.socket_path.clone().unwrap_or_default(),
                            i.loopback_url.clone().unwrap_or_default()
                        );
                        match &warm {
                            Some((n, bytes, why)) if *n > 0 => s.push_str(&format!(
                                "\nwarm: {n} Space{}, {:.1} GB ({why})",
                                if *n == 1 { "" } else { "s" },
                                *bytes as f64 / 1e9
                            )),
                            Some((_, _, why)) if !why.is_empty() => {
                                s.push_str(&format!("\nwarm: 0 ({why})"))
                            }
                            _ => {}
                        }
                        s
                    }
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
            // A Linux runtime VM that is up stops with the daemon (unless
            // Spaces still run in it): that takes a while.
            let runtime_up = cua_vmm::managed::running();
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
            let exit_by =
                std::time::Instant::now() + Duration::from_secs(if runtime_up { 210 } else { 5 });
            while cua_host::service::process_alive(pid) && std::time::Instant::now() < exit_by {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            line(out, "stopped");
            if let Some(note) = cua_vmm::managed::take_shutdown_note() {
                line(out, note);
            }
        }
        Command::Runtime(RuntimeCmd::Doctor) => {
            let report = cua.local().doctor().await?;
            return Ok(runtime_doctor_output_with(
                out,
                &report,
                json,
                &runtime_optional(&report.report_json),
            ));
        }
        Command::Runtime(RuntimeCmd::Setup {
            dry_run,
            components,
        }) => {
            let steps = cua.local().setup(components.clone(), dry_run).await?;
            let report = cua.local().doctor().await?;
            let optional = runtime_optional(&report.report_json);
            return Ok(runtime_setup_output(out, steps, &optional, &components));
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
                ImageCmd::Ls(_) | ImageCmd::Info(_) | ImageCmd::Rm(_) | ImageCmd::Create(_) => {
                    unreachable!("handled above")
                }
            }
        }
        Command::Auth(_)
        | Command::Connect(_)
        | Command::Agents(_)
        | Command::WifToken(_)
        | Command::Trajectory(_)
        | Command::Skills(_)
        | Command::Daemon(DaemonCmd::Start { .. } | DaemonCmd::Reap { .. })
        | Command::Host(_)
        | Command::Run(_)
        | Command::Build(_)
        | Command::Jobs(_)
        | Command::Computers
        | Command::Spaces(_)
        | Command::Cloud(_)
        | Command::Devices(_)
        | Command::Team(_)
        | Command::Usage { .. }
        | Command::Fleet(_)
        | Command::Pools(_)
        | Command::Teleport(_)
        | Command::Cache(_)
        | Command::Share(_)
        | Command::Compat(_)
        | Command::Config(_)
        | Command::Telemetry(_)
        | Command::Bench(_)
        | Command::Keyvault(_)
        | Command::Images(_)
        | Command::DumpDocs { .. } => unreachable!("handled above"),
    }
    Ok(0)
}

/// `cua cloud add aws` (builds with AWS: the `byoc` feature).
#[cfg(feature = "byoc")]
async fn add_aws(a: onboard::AwsArgs, json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    add_aws::run(a, json, out).await
}

/// `cua cloud trust <cloud>`: the trust setup file this cua ships.
#[cfg(feature = "byoc")]
async fn cloud_trust(cloud: &str, out: &mut dyn Write) -> Result<i32, CuaError> {
    let cloud = cloud.trim().to_ascii_lowercase();
    let file = cua_byoc::trust::file(&cloud).ok_or_else(|| {
        CuaError::InvalidArgument(format!(
            "{cloud:?} has no trust file: aws, gcp or azure (Hetzner and Modal take an API token)"
        ))
    })?;
    out.write_all(file.as_bytes())
        .map_err(|e| CuaError::Internal(e.to_string()))?;
    Ok(0)
}

#[cfg(not(feature = "byoc"))]
async fn cloud_trust(_cloud: &str, _out: &mut dyn Write) -> Result<i32, CuaError> {
    Err(CuaError::Unsupported(
        "this cua was built without your own clouds (the byoc feature); install the Cua Spaces cua"
            .into(),
    ))
}

#[cfg(not(feature = "byoc"))]
async fn add_aws(_a: onboard::AwsArgs, _json: bool, _out: &mut dyn Write) -> Result<i32, CuaError> {
    Err(CuaError::Unsupported(
        "this cua was built without AWS (the byoc feature); install the Cua Spaces cua".into(),
    ))
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
    if foreground {
        // Job members that ask for `compat:e2b` / `compat:daytona` reach a
        // cua-compat gateway this daemon runs (shared, started on first use).
        compat_cmd::register_job_services();
    }
    if !foreground {
        if let Some(d) = cua_daemon::Discovery::read(&discovery)
            && Cua::connect(None, None)?.info().await.is_ok()
        {
            // A daemon without an extension this `cua` carries (an MIT
            // daemon when this is the Cua Spaces build) is replaced, so the
            // Spaces apps and their tools find what they ship with. So is,
            // for a `cua` inside an app bundle, another app's daemon older
            // than this build (or of an unknown version), and its own
            // before a rebuild or update: its Spaces would run with that
            // build and permissions. Another app's of the same or a newer
            // version is kept, so two apps never keep replacing each other's,
            // and so is one this app replaced once that its app started
            // again (`CUA_DAEMON_KEEP`): an older app restarts its own.
            use cua_daemon::identity::Verdict;
            let stranger = stranger_daemon().await;
            let missing = if matches!(stranger, Some(Verdict::Yield(_))) {
                Vec::new()
            } else {
                extension::missing_daemon_extensions().await
            };
            let why = if missing.is_empty() {
                stranger
            } else if let Some(theirs) = extension::newer_daemon_version().await {
                // Never a downgrade: a daemon newer than this `cua` stays,
                // although it lacks what this build carries. Those
                // commands say so when they are used.
                Some(Verdict::Keep(format!(
                    "cua {theirs} is newer than this cua {}, so it is not replaced, \
                     although it lacks {}",
                    cua_daemon::VERSION,
                    missing.join(", ")
                )))
            } else {
                Some(Verdict::Replace(format!("it lacks {}", missing.join(", "))))
            };
            match why {
                None | Some(Verdict::Own) => {
                    line(out, format!("cua daemon already running (pid {})", d.pid));
                    return Ok(0);
                }
                Some(Verdict::Keep(whose)) => {
                    line(
                        out,
                        format!(
                            "cua daemon already running (pid {}): {whose}; using it",
                            d.pid
                        ),
                    );
                    return Ok(0);
                }
                Some(Verdict::Yield(whose)) => {
                    line(
                        out,
                        format!(
                            "cua daemon already running (pid {}): {whose}; using it",
                            d.pid
                        ),
                    );
                    return Ok(0);
                }
                Some(Verdict::Replace(why)) => {
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
        let spaces = if cua_daemon::extension::registered().is_empty() {
            match extension::spaces_cli_not_older() {
                Ok(found) => found,
                Err(why) => {
                    line(out, why);
                    None
                }
            }
        } else {
            None
        };
        let exe = match spaces {
            Some(spaces) => spaces,
            None => std::env::current_exe().map_err(util::internal)?,
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
    // One daemon per home: one still winding down (stopped, or an app
    // restarting it) is waited for, never run next to.
    let lock_path = discovery
        .parent()
        .map(|d| d.join("daemon.lock"))
        .unwrap_or_else(cua_daemon::InstanceLock::path);
    let _instance = {
        let path = lock_path.clone();
        let wait = cua_daemon::server::SHUTDOWN_GRACE + Duration::from_secs(5);
        tokio::task::spawn_blocking(move || cua_daemon::InstanceLock::acquire(&path, wait))
            .await
            .map_err(util::internal)?
            .map_err(|held| CuaError::InvalidArgument(held.to_string()))?
    };
    // A daemon holds a descriptor per Space, client and stream: dozens of
    // concurrent creates exhausted macOS's default soft limit of 256.
    if let Some((before, after)) = util::raise_open_file_limit() {
        eprintln!("cua daemon: open-file limit {before} -> {after}");
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
            account_from_session: true,
            ..Default::default()
        };
        let cua = Cua::embedded(cfg)?;
        let runtime = cua
            .embedded_runtime()
            .cloned()
            .ok_or_else(|| CuaError::Internal("embedded runtime".into()))?;
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
                        .list_filtered(None)
                        .await
                        .map(|l| l.sandboxes.len() as u64)
                        .unwrap_or(0);
                    let t = cua_telemetry::global();
                    t.capture(events::daemon_health(started.elapsed(), n));
                    t.flush(Duration::from_secs(3));
                }
            });
        } else {
            cua_telemetry::global().shutdown(EXIT_TELEMETRY_BUDGET);
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
    // SIGTERM and Ctrl-C stop it cleanly, within a bounded time.
    let trigger = h.shutdown_trigger();
    tokio::spawn(async move {
        cua_daemon::server::stop_signal().await;
        let _ = trigger.send(true);
    });
    if !h.wait_bounded(cua_daemon::server::SHUTDOWN_GRACE).await {
        eprintln!("cua daemon: shutdown took too long; exiting");
    }
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

/// How this process shows the first-run telemetry notice. A daemon the
/// Spaces app starts (`CUA_DAEMON_STARTED_BY=app`) leaves it to the app:
/// its stderr is a pipe nobody reads, so printing the notice there and
/// marking it shown would let it (and the app) send before the person saw
/// the app's Welcome page and its usage-data switch. The app acknowledges
/// the notice when Welcome is left with the switch on; until then this
/// daemon sends nothing.
fn notice_mode(started_by: Option<&str>) -> cua_telemetry::NoticeMode {
    match started_by {
        Some("app") => cua_telemetry::NoticeMode::External,
        _ => cua_telemetry::NoticeMode::Stderr,
    }
}

/// Whether `cmd` is a Teams command: `cua team`, `cua host pair --team`,
/// or sharing a Space with (or unsharing it from) `team:<id>`.
fn uses_teams(cmd: &Command) -> bool {
    let team = |who: &str| who.trim().starts_with("team:");
    match cmd {
        Command::Team(_) => true,
        Command::Host(host::HostCmd::Pair { team: Some(_), .. }) => true,
        Command::Spaces(host::SpacesCmd::Share { who, .. }) => team(who),
        Command::Spaces(host::SpacesCmd::Unshare { who: Some(who), .. }) => team(who),
        _ => false,
    }
}

/// The clap command `cua` parses with. Teams are experimental: without
/// them, `cua team` and `cua host pair --team` stay out of help (they still
/// parse, and are refused with a short reason).
fn cli_command(teams: bool) -> clap::Command {
    let cmd = Cli::command();
    if teams {
        return cmd;
    }
    cmd.mut_subcommand("team", |c| c.hide(true))
        .mut_subcommand("host", |h| {
            h.mut_subcommand("pair", |p| {
                // Drop the Teams example too.
                let examples = p.get_after_help().map(|h| {
                    h.to_string()
                        .lines()
                        .filter(|l| !l.contains("team"))
                        .collect::<Vec<_>>()
                        .join("\n")
                });
                let p = p
                    .mut_arg("team", |a| a.hide(true))
                    .mut_arg("no_wait", |a| a.hide(true));
                match examples {
                    Some(e) => p.after_help(e),
                    None => p,
                }
            })
        })
}

/// The `cua` command: parses this process's arguments, runs them and exits.
/// The Cua Spaces build registers its [`extension`] first.
/// `cua help advanced`: the options most people never need, kept out of
/// every other `--help` (the flags still work everywhere).
pub const ADVANCED_HELP: &str = "\
Advanced options

For self-hosting teams that run their own Cua relay, or connect machines
directly without one. Most people use Cua's relay and need none of this.

  --relay <URL>
      Use your own relay instead of Cua's. Works with cua host setup,
      host pair, host share, host unshare, spaces ls, devices and usage.
      Default: $CUA_RELAY_URL, else https://relay.cua.ai.

  cua host setup --direct <IP:PORT>
      Serve on ip:port with a local access token instead of joining a
      relay (your network, a VPN or a forwarded port). Add
      --allow-any-address to accept callers from any address.

Examples:
  cua host setup --relay https://relay.example.com
  cua host setup --direct 0.0.0.0:3211 --name studio-mac
  # A spare Mac on your tailnet that runs Spaces, without a relay
  # (prints the command that adds it on your laptop)
  cua host setup --direct 100.101.102.103:3211 --profile spare
";

/// The command `cua` parses with, named for `channel`. Stable's is `cli`
/// untouched. A Local build ("Cua Spaces Local", CUA-1776) installs its CLI as
/// `cua-local`, so its help says so: usage lines print `cua-local` (the
/// bundled copy is a file named `cua` inside the app, which clap would name
/// `cua`), and an example that starts with `cua` starts with `cua-local`, so
/// pasting it does not run the Stable CLI.
fn channel_command(cli: clap::Command, channel: cua_home::channel::CuaChannel) -> clap::Command {
    if !channel.is_local() {
        return cli;
    }
    let verbs = util::verbs_of(&cli);
    localize_help(cli.bin_name(channel.cli_name()), channel, &verbs)
}

/// `cmd` and its subcommands, their arguments included, with every command
/// their help tells the reader to run named for `channel`'s CLI: the
/// examples in the after-help (see [`localize_examples`]) and a `cua <verb>`
/// anywhere in the about, long about, after-help and argument help (see
/// [`util::hint`]). `verbs` are the CLI's command names.
fn localize_help(
    cmd: clap::Command,
    channel: cua_home::channel::CuaChannel,
    verbs: &std::collections::HashSet<String>,
) -> clap::Command {
    let name = channel.cli_name();
    let text = |s: &clap::builder::StyledStr| {
        util::localize_for(channel, &localize_examples(&s.to_string(), name), verbs)
    };
    let about = cmd.get_about().map(text);
    let long_about = cmd.get_long_about().map(text);
    let after = cmd.get_after_help().map(text);
    let after_long = cmd.get_after_long_help().map(text);
    let mut cmd = cmd;
    if let Some(h) = about {
        cmd = cmd.about(h);
    }
    if let Some(h) = long_about {
        cmd = cmd.long_about(h);
    }
    if let Some(h) = after {
        cmd = cmd.after_help(h);
    }
    if let Some(h) = after_long {
        cmd = cmd.after_long_help(h);
    }
    cmd.mut_args(|arg| {
        let help = arg.get_help().map(text);
        let long_help = arg.get_long_help().map(text);
        let mut arg = arg;
        if let Some(h) = help {
            arg = arg.help(h);
        }
        if let Some(h) = long_help {
            arg = arg.long_help(h);
        }
        arg
    })
    .mut_subcommands(|sub| localize_help(sub, channel, verbs))
}

/// `text` with every line that is a `cua ...` example (it starts with `cua `
/// once indented) starting with `name` instead.
fn localize_examples(text: &str, name: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let body = line.trim_start();
        match body.strip_prefix("cua ") {
            Some(rest) => {
                out.push_str(&line[..line.len() - body.len()]);
                out.push_str(name);
                out.push(' ');
                out.push_str(rest);
            }
            None => out.push_str(line),
        }
    }
    out
}

/// The text of `cua help advanced` for `channel`: Local's names its own CLI in
/// the examples and wherever a command is named in a sentence.
fn advanced_help_for(channel: cua_home::channel::CuaChannel) -> String {
    if !channel.is_local() {
        return ADVANCED_HELP.to_string();
    }
    let examples = localize_examples(ADVANCED_HELP, channel.cli_name());
    util::localize_for(channel, &examples, &util::verbs_of(&cli_command(true)))
}

/// `cua help advanced`, answered before clap: it is a help topic, not a
/// command.
fn is_advanced_help(args: &[std::ffi::OsString]) -> bool {
    args.len() == 2 && args[0] == "help" && args[1] == "advanced"
}

/// Shows a fallback to the relay's HTTPS path on stderr (never stdout, so
/// `--json` output and pipes stay clean).
fn fallback_notice(n: &cua_spaces::mesh::FallbackNotice) {
    eprintln!("{}: {}", util::cli(), n.message());
}

pub fn main() {
    #[cfg(windows)]
    keep_stdio_out_of_children();
    let channel = cua_home::channel::CuaChannel::current();
    if is_advanced_help(&std::env::args_os().skip(1).collect::<Vec<_>>()) {
        print!("{}", advanced_help_for(channel));
        return;
    }
    promote_spacesd_env_vars();
    // `CUA_CHANNEL` and `CUA_ENV` pick every endpoint and the cua home (Local
    // is `cua-local`: staging and `~/.cua-local` unless told otherwise): a
    // typo must not fall back to production silently.
    if let Err(e) = cua_home::env::CuaEnv::try_current() {
        eprintln!("error: {e}");
        std::process::exit(2);
    }
    // SAFETY: still first in `main`, before any other thread exists. Pins
    // `CUA_HOME` for the Local channel or a non-production `CUA_ENV` (and
    // `CUA_CHANNEL` and `CUA_ENV` for Local) so every component and child
    // process (the daemon) uses that channel's and environment's home.
    unsafe { cua_home::env::pin_home() };
    let matches = channel_command(
        cli_command(cua_home::experimental::teams_enabled()),
        channel,
    )
    .get_matches();
    let command = command_path(&matches);
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());
    let daemon_process = matches!(cli.command, Command::Daemon(DaemonCmd::Start { .. }));
    let telemetry = cua_telemetry::init(
        if daemon_process { "daemon" } else { "cli" },
        cua_sdk::VERSION,
    );
    let record = records_telemetry(&cli.command);
    let json_output = cli.json;
    telemetry.set_notice_mode(notice_mode(
        std::env::var("CUA_DAEMON_STARTED_BY").ok().as_deref(),
    ));
    // Crash reports (opt-in, only when the Spaces shell passed a DSN) come
    // from the daemon process itself, not the `cua daemon start` launcher.
    let crash_guard = matches!(
        cli.command,
        Command::Daemon(DaemonCmd::Start {
            foreground: true,
            ..
        })
    )
    .then(|| cua_crash::init_from_env("daemon"))
    .flatten();
    // The first-run notice goes to stderr before anything could be sent
    // (not for `cua telemetry` / `cua config`, which are how you answer it).
    if record && !matches!(cli.command, Command::Config(_)) {
        telemetry.show_notice_if_needed();
    }
    // A Space that falls back from the mesh to the relay's HTTPS path says
    // so once, on stderr (the daemon logs a warning instead).
    if !daemon_process {
        cua_spaces::mesh::on_fallback(fallback_notice);
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
        tracing_subscriber::EnvFilter::try_new(log_filter_spec(
            std::env::var("CUA_LOG").ok().as_deref(),
            daemon_log,
        ))
        .unwrap_or_else(|_| log_filter_spec(None, daemon_log).into())
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
    // A signed-in machine links usage data even when it signed in before
    // the server offered linking (CUA-1881): at most every few hours, in
    // the background. Not around `cua auth`, which links or unlinks itself.
    let refresh_link = record && !matches!(cli.command, Command::Auth(_) | Command::Config(_));
    let code = rt.block_on(async {
        if refresh_link {
            tokio::spawn(auth::refresh_usage_link());
        }
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
            // A command turned down over a plan limit prints the sentence and
            // the upgrade link: `cua_plan_limit_shown` (CUA-1881).
            if let Err(e) = &result
                && let Some(ev) = plan_limit_shown(&e.to_string())
            {
                telemetry.capture(ev);
            }
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
                eprintln!("{}: {}", channel.cli_name(), util::hint(&e.to_string()));
                exit_code(&e)
            }
        }
    });
    let _ = std::io::stdout().flush();
    // `process::exit` skips destructors: flush crash reports explicitly.
    drop(crash_guard);
    // Telemetry never holds the exit up for more than this; what is left
    // goes to the offline spool, which the cua daemon (or the next command)
    // sends. A round trip to the collector often takes longer than this, and
    // agents run commands back to back: every one used to wait out 400 ms.
    telemetry.shutdown(EXIT_TELEMETRY_BUDGET);
    std::process::exit(code);
}

/// `cua_plan_limit_shown` for an error that is the relay's plan-limit
/// refusal (the sentence the CLI prints, with its upgrade link); none for
/// any other error.
fn plan_limit_shown(error: &str) -> Option<cua_telemetry::Event> {
    let l = cua_home::plan_limit::PlanLimit::parse(error)?;
    Some(cua_telemetry::events::plan_limit_shown(
        "cli_limit",
        l.kind.word(),
        &l.plan,
    ))
}

/// The longest a command waits at exit to send its telemetry.
const EXIT_TELEMETRY_BUDGET: Duration = Duration::from_millis(60);

fn runtime_optional(report_json: &str) -> Vec<String> {
    runtime_optional_with(
        report_json,
        cua_vmm::managed::supported(),
        cua_vmm::managed::LinuxSource::current(),
    )
}

fn runtime_optional_with(
    report_json: &str,
    managed_supported: bool,
    source: cua_vmm::managed::LinuxSource,
) -> Vec<String> {
    use cua_vmm::{BackendKind as B, host::HostOs, managed::LinuxSource};
    let Ok(r) = serde_json::from_str::<cua_vmm::auto::DoctorReport>(report_json) else {
        return Vec::new();
    };
    let linux_ready = r.backends.iter().any(|b| {
        b.ready && matches!(b.backend, B::Qemu | B::Container)
            || b.backend == B::Managed && r.host.os == HostOs::Macos && (b.ready || b.provisionable)
    });
    let mut optional: Vec<String> = r
        .backends
        .iter()
        .filter(|b| {
            !b.ready
                && match b.backend {
                    B::Lume => !r.lume.supported_host,
                    B::Managed => {
                        linux_ready || !managed_supported || source == LinuxSource::System
                    }
                    _ => linux_ready,
                }
        })
        .map(|b| b.backend.as_str().to_string())
        .collect();
    if linux_ready && !r.backends.iter().any(|b| b.backend == B::Qemu && b.ready) {
        optional.push("kvm".into());
    }
    optional
}

fn runtime_doctor_output_with(
    out: &mut dyn std::io::Write,
    report: &cua_sdk::DoctorReport,
    json: bool,
    optional: &[String],
) -> i32 {
    use cua_sdk::RuntimeCheckStatus as S;
    let mut checks = report.checks.clone();
    for check in &mut checks {
        if check.status == S::Error && optional.contains(&check.name) {
            check.status = S::NotApplicable;
        }
    }
    if json {
        line(out, &report.report_json);
    } else {
        for c in &checks {
            line(
                out,
                format!(
                    "{:<10} {:<12} {}{}",
                    c.name,
                    format!("{:?}", c.status).to_lowercase(),
                    c.version
                        .as_ref()
                        .map(|v| format!("[{v}] "))
                        .unwrap_or_default(),
                    c.detail
                ),
            );
        }
    }
    i32::from(checks.iter().any(|c| c.status == S::Error))
}

fn runtime_setup_output(
    out: &mut dyn std::io::Write,
    mut steps: Vec<cua_sdk::SetupStep>,
    optional: &[String],
    components: &[String],
) -> i32 {
    use cua_sdk::RuntimeCheckStatus as S;
    for s in &mut steps {
        if s.status == S::Error
            && optional.contains(&s.name)
            && !components.contains(&s.name)
            && !(s.name == "kvm" && components.iter().any(|c| c == "qemu"))
        {
            s.status = S::NotApplicable;
        }
        line(out, format!("{:<10} {:?}: {}", s.name, s.status, s.detail));
    }
    i32::from(steps.iter().any(|s| s.status == S::Error))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CUA-1881: a command turned down over a plan limit is
    /// `cua_plan_limit_shown` (`cli_limit`, its cap and plan); other errors
    /// are not.
    #[test]
    fn a_plan_limit_error_is_a_plan_limit_shown() {
        let e = crate::host::host_err(cua_host::Error::PlanLimit(
            "You've used 20 of 20 remote hours this month on the free plan. Upgrade to Pro for more.".into(),
        ));
        let ev = plan_limit_shown(&e.to_string()).unwrap();
        assert_eq!(ev.name, "cua_plan_limit_shown");
        assert_eq!(ev.props["surface"], "cli_limit");
        assert_eq!(ev.props["kind"], "hours");
        assert_eq!(ev.props["plan"], "free");
        assert!(e.to_string().contains("/buy/pro?from=cli_limit"), "{e}");
        assert!(plan_limit_shown("relay: timed out").is_none());
    }

    /// The WireGuard library's retry/expiry messages stay out of the log
    /// (also with `CUA_LOG=info`, as a service sets), unless
    /// the filter names them.
    #[test]
    fn the_log_filter_hides_the_wireguard_timers_unless_asked_for() {
        use std::io::Write;
        #[derive(Clone, Default)]
        struct Buf(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
        impl Write for Buf {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Buf {
            type Writer = Buf;
            fn make_writer(&'a self) -> Buf {
                self.clone()
            }
        }
        let run = |spec: Option<&str>, daemon: bool| {
            let buf = Buf::default();
            let subscriber = tracing_subscriber::fmt()
                .with_env_filter(tracing_subscriber::EnvFilter::new(log_filter_spec(
                    spec, daemon,
                )))
                .with_writer(buf.clone())
                .finish();
            tracing::subscriber::with_default(subscriber, || {
                tracing::warn!(target: "boringtun::noise::timers", "HANDSHAKE(REKEY_TIMEOUT)");
                tracing::error!(target: "boringtun::noise::timers", "CONNECTION_EXPIRED(REKEY_ATTEMPT_TIME)");
                tracing::warn!(target: "cua_mesh::node", "kept warning");
            });
            let bytes = buf.0.lock().unwrap().clone();
            String::from_utf8(bytes).unwrap()
        };
        for daemon in [false, true] {
            for spec in [None, Some(""), Some("info"), Some("debug"), Some(" warn ")] {
                let out = run(spec, daemon);
                assert!(
                    !out.contains("HANDSHAKE") && !out.contains("CONNECTION_EXPIRED"),
                    "{spec:?}: {out}"
                );
                assert!(out.contains("kept warning"), "{spec:?}: {out}");
            }
        }
        let out = run(Some("warn,boringtun=debug"), false);
        assert!(
            out.contains("HANDSHAKE") && out.contains("CONNECTION_EXPIRED"),
            "{out}"
        );
    }

    /// Self-hosting stays out of every `--help` but `cua help advanced`;
    /// the flags still parse.
    #[test]
    fn self_hosting_flags_are_hidden_but_work() {
        let help = |path: &[&str]| {
            let mut cmd = cli_command(false);
            let mut sub = &mut cmd;
            for p in path {
                sub = sub.find_subcommand_mut(p).expect(p);
            }
            sub.render_long_help().to_string()
        };
        for path in [
            &["host", "setup"][..],
            &["host", "pair"],
            &["usage"],
            &["spaces", "ls"],
        ] {
            let text = help(path);
            assert!(!text.contains("--relay"), "{path:?}: {text}");
            assert!(!text.contains("--direct"), "{path:?}: {text}");
        }
        assert!(!help(&[]).contains("--relay"));
        assert!(
            Cli::try_parse_from(["cua", "usage", "--relay", "https://relay.example.com"]).is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "cua",
                "host",
                "setup",
                "--relay",
                "https://relay.example.com"
            ])
            .is_ok()
        );
        assert!(Cli::try_parse_from(["cua", "host", "setup", "--direct", "0.0.0.0:3211"]).is_ok());
        let args = |a: &[&str]| a.iter().map(std::ffi::OsString::from).collect::<Vec<_>>();
        assert!(is_advanced_help(&args(&["help", "advanced"])));
        assert!(!is_advanced_help(&args(&["help"])));
        assert!(ADVANCED_HELP.contains("--relay <URL>"));
        assert!(ADVANCED_HELP.contains("--direct <IP:PORT>"));
    }

    /// The Spaces app's launch check (`cua auth keychain`) records no usage
    /// event and never shows the first-run notice; other auth commands do.
    #[test]
    fn the_app_keychain_check_records_nothing() {
        let cli = Cli::try_parse_from(["cua", "auth", "keychain", "--prompt"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Auth(AuthCmd::Keychain {
                prompt: true,
                forget: false
            })
        ));
        assert!(!records_telemetry(&cli.command));
        assert!(Cli::try_parse_from(["cua", "auth", "keychain", "--prompt", "--forget"]).is_err());
        let status = Cli::try_parse_from(["cua", "auth", "status"]).unwrap();
        assert!(records_telemetry(&status.command));
        // One compact JSON line (the app reads it). A test build is not
        // Cua-signed, so its store is a file and no keychain is touched.
        if std::env::var_os("CUA_CREDENTIAL_STORE").is_none() {
            let mut out = Vec::new();
            assert_eq!(auth::keychain(false, false, &mut out).unwrap(), 0);
            let text = String::from_utf8(out).unwrap();
            assert_eq!(text.lines().count(), 1, "{text}");
            let v: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(v["state"], "not_used");
        }
    }

    /// A daemon the Spaces app starts never shows or acknowledges the
    /// first-run notice (the app's Welcome page does), so it sends nothing
    /// before the person answered it; every other process prints it.
    #[test]
    fn an_app_started_daemon_leaves_the_notice_to_the_app() {
        use cua_telemetry::{NoticeMode, Telemetry};
        assert_eq!(notice_mode(Some("app")), NoticeMode::External);
        for other in [None, Some("cli"), Some("launchd")] {
            assert_eq!(notice_mode(other), NoticeMode::Stderr, "{other:?}");
        }
        let home = tempfile::tempdir().unwrap();
        let t = Telemetry::builder()
            .home(home.path())
            .env(|k| (k == "CUA_TELEMETRY").then(|| "1".to_string()))
            .build();
        t.set_notice_mode(notice_mode(Some("app")));
        if !t.is_enabled() {
            return; // telemetry forced off in this environment
        }
        assert!(!t.show_notice_if_needed());
        assert!(!t.notice_shown(), "the app's Welcome page acknowledges it");
        t.set_notice_mode(notice_mode(None));
        assert!(t.show_notice_if_needed());
        assert!(t.notice_shown());
    }

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
            CuaError::Cloud(s()),
            CuaError::InsufficientDisk(s()),
            CuaError::HostCapacity(s()),
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

    /// A full machine exits 8, apart from "not found" (3): a script retries
    /// or frees a Space on one and not the other.
    #[test]
    fn a_full_machine_exits_8_and_not_found_exits_3() {
        assert_eq!(exit_code(&CuaError::HostCapacity("no room".into())), 8);
        assert_eq!(exit_code(&CuaError::NotFound("x".into())), 3);
        assert!(
            EXIT_CODES
                .iter()
                .any(|(c, d)| *c == 8 && d.starts_with("No room"))
        );
    }

    /// `cua computers` is the one command that lists the pool's computers;
    /// `cua machines` and `cua sb runs --machines` (old spellings) are gone, not aliases.
    #[test]
    fn computers_is_its_own_command_and_machines_is_gone() {
        let parse =
            |a: &[&str]| Cli::try_parse_from(std::iter::once("cua").chain(a.iter().copied()));
        assert!(matches!(
            parse(&["computers"]).unwrap().command,
            Command::Computers
        ));
        let j = parse(&["computers", "--json"]).unwrap();
        assert!(j.json && matches!(j.command, Command::Computers));
        assert!(parse(&["computers", "extra"]).is_err());
        assert!(parse(&["machines"]).is_err());
        for sb in ["sb", "sandbox"] {
            let e = parse(&[sb, "runs", "--machines"]).unwrap_err();
            assert_eq!(
                e.kind(),
                clap::error::ErrorKind::UnknownArgument,
                "{sb}: {e}"
            );
        }
        assert!(parse(&["sb", "runs"]).is_ok());
        assert!(parse(&["sb", "runs", "p-1a2b3c4d", "--state", "failed"]).is_ok());
    }

    #[test]
    fn build_takes_a_context_anywhere_and_the_docker_flags() {
        let parse =
            |a: &[&str]| Cli::try_parse_from(std::iter::once("cua").chain(a.iter().copied()));
        let Command::Build(a) = parse(&["build"]).unwrap().command else {
            panic!()
        };
        assert_eq!(a.context, std::path::PathBuf::from("."));
        assert!(a.file.is_none() && a.tag.is_none() && !a.no_cache && !a.desktop);
        let Command::Build(a) = parse(&[
            "build",
            "-f",
            "docker/Dockerfile.dev",
            "-t",
            "app",
            "--build-arg",
            "A=1",
            "--build-arg",
            "B=2",
            "--kind",
            "vm",
            "--cpus",
            "4",
            "--memory",
            "8192",
            "--no-cache",
            "--step-timeout",
            "30m",
            "/tmp/ctx",
        ])
        .unwrap()
        .command
        else {
            panic!()
        };
        assert_eq!(a.context, std::path::PathBuf::from("/tmp/ctx"));
        assert_eq!(a.file.as_deref(), Some("docker/Dockerfile.dev"));
        assert_eq!(
            (a.tag.as_deref(), a.cpus, a.memory_mb),
            (Some("app"), Some(4), Some(8192))
        );
        assert_eq!(a.build_arg, ["A=1", "B=2"]);
        assert!(a.no_cache);
        assert!(parse(&["build", "--json", "-f", "-"]).unwrap().json);
        assert!(parse(&["build", "--cpus", "many"]).is_err());
    }

    #[test]
    fn local_pool_commands_parse() {
        let parse =
            |a: &[&str]| Cli::try_parse_from(std::iter::once("cua").chain(a.iter().copied()));
        assert!(parse(&["sandbox", "create", "--pool", "p", "--name", "n"]).is_ok());
        assert!(
            parse(&[
                "sandbox", "pool", "create", "linux", "--name", "agents", "--min", "2", "--max",
                "8", "--idle", "30m"
            ])
            .is_ok()
        );
        assert!(parse(&["sandbox", "pool", "ls"]).is_ok());
        assert!(parse(&["sandbox", "pool", "rm", "agents"]).is_ok());
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
        // The streaming form parses with the flags that go with it (it runs
        // against the real binary in tests/sandbox_do.rs: it reads stdin).
        for args in [
            &[
                "cua", "spacesd", "exec", "-i", "--user", "u", "--cwd", "/w", "URL", "cat",
            ][..],
            &[
                "cua",
                "sandbox",
                "exec",
                "-i",
                "--timeout",
                "0",
                "dev",
                "--",
                "cat",
            ],
        ] {
            assert!(Cli::try_parse_from(args).is_ok(), "{args:?}");
        }
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

    #[test]
    fn runtime_optional_backends_preserve_sdk_contract() {
        use cua_vmm::{
            BackendKind as B,
            auto::{BackendStatus, DoctorReport, HostReport},
            host::HostOs,
            managed::LinuxSource,
        };
        for (os, ready, supported, source, running) in [
            (HostOs::Linux, B::Qemu, false, LinuxSource::Auto, true),
            (HostOs::Linux, B::Container, false, LinuxSource::Auto, true),
            (HostOs::Macos, B::Container, true, LinuxSource::System, true),
            (HostOs::Macos, B::Managed, true, LinuxSource::Builtin, true),
            (HostOs::Macos, B::Managed, true, LinuxSource::Builtin, false),
        ] {
            let mut r = DoctorReport {
                host: HostReport {
                    os,
                    arch: cua_vmm::Arch::X86_64,
                    kvm: true,
                    accel: Default::default(),
                },
                backends: [B::Qemu, B::Lume, B::Managed, B::Container]
                    .into_iter()
                    .map(|backend| BackendStatus {
                        backend,
                        ready: backend == ready && running,
                        provisionable: matches!(backend, B::Lume | B::Managed),
                        detail: String::new(),
                        missing: vec![],
                        provisioning: vec![],
                    })
                    .collect(),
                qemu: Default::default(),
                lume: Default::default(),
                container: Default::default(),
                containerization: Default::default(),
                microvm: Default::default(),
                vz: Default::default(),
            };
            let raw = cua_daemon::local::doctor_checks(&r);
            assert_eq!(
                raw[1].status,
                cua_proto::daemon::v1::CheckStatus::Installable as i32
            );
            if ready != B::Qemu {
                assert_eq!(
                    raw[0].status,
                    cua_proto::daemon::v1::CheckStatus::Error as i32
                );
            }
            let optional = super::runtime_optional_with(
                &serde_json::to_string(&r).unwrap(),
                supported,
                source,
            );
            for (b, c) in r.backends.iter().zip(&raw) {
                assert!(b.ready || optional.contains(&c.name));
            }
            let report = cua_sdk::DoctorReport {
                checks: raw.into_iter().map(Into::into).collect(),
                report_json: serde_json::to_string(&r).unwrap(),
            };
            for json in [false, true] {
                let mut out = Vec::new();
                assert_eq!(
                    super::runtime_doctor_output_with(&mut out, &report, json, &optional),
                    0
                );
                if !json {
                    let text = String::from_utf8(out).unwrap();
                    assert!(!text.contains("error"));
                    assert!(
                        text.lines()
                            .any(|l| l.starts_with("lume") && l.contains("installable"))
                    );
                }
            }
            if os == HostOs::Macos && ready == B::Container {
                let mut mac = r.clone();
                mac.lume.supported_host = true;
                mac.backends
                    .iter_mut()
                    .find(|b| b.backend == B::Lume)
                    .unwrap()
                    .provisionable = false;
                let report_json = serde_json::to_string(&mac).unwrap();
                let optional = super::runtime_optional_with(&report_json, supported, source);
                let report = cua_sdk::DoctorReport {
                    checks: cua_daemon::local::doctor_checks(&mac)
                        .into_iter()
                        .map(Into::into)
                        .collect(),
                    report_json,
                };
                for json in [false, true] {
                    assert_eq!(
                        super::runtime_doctor_output_with(
                            &mut Vec::new(),
                            &report,
                            json,
                            &optional
                        ),
                        1
                    );
                }
                assert_eq!(
                    super::runtime_setup_output(
                        &mut Vec::new(),
                        vec![cua_sdk::SetupStep {
                            name: "lume".into(),
                            status: cua_sdk::RuntimeCheckStatus::Error,
                            detail: "unavailable".into(),
                        }],
                        &optional,
                        &[]
                    ),
                    1
                );
            }
            if ready == B::Container {
                assert_eq!(
                    super::runtime_setup_output(
                        &mut Vec::new(),
                        vec![cua_sdk::SetupStep {
                            name: "kvm".into(),
                            status: cua_sdk::RuntimeCheckStatus::Error,
                            detail: "permission denied".into(),
                        }],
                        &optional,
                        &["qemu".into()]
                    ),
                    1
                );
                let steps = || {
                    vec![cua_sdk::SetupStep {
                        name: "qemu".into(),
                        status: cua_sdk::RuntimeCheckStatus::Error,
                        detail: "install failed".into(),
                    }]
                };
                let mut out = Vec::new();
                assert_eq!(
                    super::runtime_setup_output(&mut out, steps(), &optional, &["qemu".into()]),
                    1
                );
                assert_eq!(
                    super::runtime_setup_output(&mut out, steps(), &optional, &[]),
                    0
                );
            }
            r.backends.iter_mut().for_each(|b| {
                b.ready = false;
                b.provisionable = false;
            });
            let optional = super::runtime_optional_with(
                &serde_json::to_string(&r).unwrap(),
                supported,
                source,
            );
            assert!(!optional.contains(&"qemu".into()));
            assert!(!optional.contains(&"kvm".into()));
            let report = cua_sdk::DoctorReport {
                checks: cua_daemon::local::doctor_checks(&r)
                    .into_iter()
                    .map(Into::into)
                    .collect(),
                report_json: serde_json::to_string(&r).unwrap(),
            };
            let mut out = Vec::new();
            assert_eq!(
                super::runtime_doctor_output_with(&mut out, &report, false, &optional),
                1
            );
            assert!(String::from_utf8(out).unwrap().contains("error"));
        }
    }

    #[test]
    fn runtime_output_exit_codes() {
        use cua_sdk::{DoctorReport, RuntimeCheck, RuntimeCheckStatus as S, SetupStep};
        for (status, expected) in [(S::Ok, 0), (S::Installable, 0), (S::Error, 1)] {
            let checks = vec![
                RuntimeCheck {
                    name: "kvm".into(),
                    status,
                    version: None,
                    detail: "first".into(),
                },
                RuntimeCheck {
                    name: "later".into(),
                    status: S::Ok,
                    version: None,
                    detail: "last".into(),
                },
            ];
            let report = DoctorReport {
                checks,
                report_json: "{\"checks\":[]}".into(),
            };
            for json in [false, true] {
                let mut out = Vec::new();
                assert_eq!(
                    super::runtime_doctor_output_with(&mut out, &report, json, &[]),
                    expected
                );
                if json {
                    serde_json::from_slice::<serde_json::Value>(&out).unwrap();
                } else {
                    assert!(String::from_utf8(out).unwrap().contains("last"));
                }
            }
            let steps = report
                .checks
                .iter()
                .map(|c| SetupStep {
                    name: c.name.clone(),
                    status: c.status,
                    detail: c.detail.clone(),
                })
                .collect::<Vec<_>>();
            let mut out = Vec::new();
            assert_eq!(
                super::runtime_setup_output(&mut out, steps, &[], &[]),
                expected
            );
            assert!(String::from_utf8(out).unwrap().contains("last"));
        }
    }

    /// Every command of the tree with its rendered long help.
    fn all_help(cmd: &mut clap::Command) -> Vec<(String, String)> {
        let mut out = vec![(
            cmd.get_bin_name().unwrap_or(cmd.get_name()).to_string(),
            cmd.render_long_help().to_string(),
        )];
        for name in cmd
            .get_subcommands()
            .map(|c| c.get_name().to_string())
            .collect::<Vec<_>>()
        {
            let sub = cmd.find_subcommand_mut(&name).unwrap();
            out.extend(all_help(sub));
        }
        out
    }

    /// CUA-1776: the Local CLI (`cua-local`) says `cua-local` in its usage
    /// and examples, so nothing it prints sends the user to Stable's `cua`.
    #[test]
    fn local_help_says_cua_local() {
        use cua_home::channel::CuaChannel;
        let mut local = channel_command(cli_command(true), CuaChannel::Local);
        local.build();
        let help = all_help(&mut local);
        assert!(help.len() > 20, "the whole tree is rendered");
        assert!(help[0].1.contains("Usage: cua-local "), "{}", help[0].1);
        for (name, text) in &help {
            assert!(name.starts_with("cua-local"), "{name}");
            for line in text.lines() {
                let l = line.trim_start();
                assert!(
                    !l.starts_with("cua ") && !l.starts_with("Usage: cua "),
                    "{name}: {line}"
                );
            }
        }
        // The examples are still there, renamed.
        assert!(
            help.iter().any(|(_, t)| t
                .lines()
                .any(|l| l.trim_start().starts_with("cua-local host setup"))),
            "host setup example"
        );
        // The version line is still `cua <version>`: the app checks it to
        // recognise its CLI.
        assert_eq!(
            local.render_version().trim(),
            format!("cua {}", cua_sdk::VERSION)
        );
    }

    /// Everything in Local's help that tells the reader to run a command is
    /// already named `cua-local`: running the hint rewriter over the rendered
    /// help (after-help, about, argument help) finds nothing left to change.
    /// A new help text that spells `cua host setup` in the middle of a line, or
    /// in backticks, is caught here, not by a user who pastes it.
    #[test]
    fn local_help_has_no_stable_command_left() {
        use cua_home::channel::CuaChannel;
        let verbs = util::verbs_of(&cli_command(true));
        let mut local = channel_command(cli_command(true), CuaChannel::Local);
        local.build();
        for (name, text) in all_help(&mut local) {
            assert_eq!(
                util::localize_for(CuaChannel::Local, &text, &verbs),
                text,
                "{name} still names the Stable `cua`"
            );
        }
        // And the rewrite is not a no-op: Stable's help still has them.
        let mut stable = cli_command(true);
        stable.build();
        assert!(
            all_help(&mut stable)
                .iter()
                .any(|(_, t)| t.contains("`cua ")),
            "Stable help has backticked commands"
        );
    }

    /// `cua help advanced` is a help topic printed before clap runs, so the help
    /// tree above does not cover it: Local's names `cua-local` in its examples
    /// and in its sentences, Stable's is the text as written.
    #[test]
    fn help_advanced_follows_the_channel() {
        use cua_home::channel::CuaChannel;
        assert_eq!(advanced_help_for(CuaChannel::Stable), ADVANCED_HELP);
        let local = advanced_help_for(CuaChannel::Local);
        assert!(local.contains("Works with cua-local host setup"), "{local}");
        let verbs = util::verbs_of(&cli_command(true));
        assert_eq!(
            util::localize_for(CuaChannel::Local, &local, &verbs),
            local,
            "nothing left that names the Stable `cua`"
        );
        assert!(
            !local.lines().any(|l| l.trim_start().starts_with("cua ")),
            "{local}"
        );
    }

    /// Stable's command and its help are exactly what they were.
    #[test]
    fn stable_help_is_unchanged() {
        use cua_home::channel::CuaChannel;
        for teams in [true, false] {
            let mut plain = cli_command(teams);
            let mut stable = channel_command(cli_command(teams), CuaChannel::Stable);
            plain.build();
            stable.build();
            assert_eq!(all_help(&mut plain), all_help(&mut stable));
        }
    }

    #[test]
    fn examples_are_renamed_line_by_line() {
        let text = "Examples:\n  cua host setup\n  # cua is a comment\nthe cua daemon\n    cua sb ls  # list\ncua\n";
        assert_eq!(
            localize_examples(text, "cua-local"),
            "Examples:\n  cua-local host setup\n  # cua is a comment\nthe cua daemon\n    cua-local sb ls  # list\ncua\n"
        );
        assert_eq!(localize_examples("", "cua-local"), "");
    }

    #[tokio::test]
    #[ignore = "host: probes the developer's lume, qemu and Docker; run with --ignored"]
    async fn runtime_doctor_is_read_only_and_reports_backends() {
        let (r, out) = cli(&["--embedded", "runtime", "doctor"]).await;
        assert!([0, 1].contains(&r.unwrap()));
        assert!(out.contains("qemu") && out.contains("container"), "{out}");
        let (r, out) = cli(&["--embedded", "runtime", "setup", "--dry-run", "qemu"]).await;
        assert_eq!(r.unwrap(), 0);
        assert!(out.starts_with("qemu"), "{out}");
    }
}
