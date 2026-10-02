//! `cua sandbox` (alias `sb`): create/launch, list, inspect, lifecycle,
//! exec, shell, screenshot and display URLs for Fleet, local and direct
//! sandboxes. Keeps the flags and aliases of the former Python CLI
//! (`launch`, `ls`/`list`, `info`/`get`, `delete`/`rm`, `--local`, `--json`).

use crate::{
    auth,
    shell::{self, exit_code},
    util::{self, internal, line},
};
use clap::{Args, Subcommand};
use cua_sandbox_core::placement::{Kind, On, Runtime};
use cua_sdk::{Cua, CuaError, ReadinessProbe, SandboxCreateOptions, SandboxInfo};
use std::{collections::HashMap, io::Write, path::PathBuf, sync::Arc};

/// `--on`: where it runs (`local`, `cloud`, `direct:<addr>`, or a
/// registered provider).
pub fn parse_on(s: &str) -> Result<On, String> {
    On::parse(s).map_err(|e| e.message)
}

/// `--kind`: `auto`, `container` or `vm`.
pub fn parse_kind(s: &str) -> Result<Kind, String> {
    Kind::parse(s).map_err(|e| e.message)
}

/// `--runtime`: `auto` or an engine (`gvisor`, `runc`, `qemu`, `lume`,
/// `kubevirt`).
pub fn parse_runtime(s: &str) -> Result<Runtime, String> {
    Runtime::parse(s).map_err(|e| e.message)
}

/// The user settings (`~/.cua/config.toml` and the `CUA_DEFAULT_*`
/// environment).
pub fn settings() -> Result<cua_sandbox_core::settings::Settings, CuaError> {
    cua_sandbox_core::settings::Settings::load().map_err(|e| CuaError::InvalidArgument(e.0))
}

/// `--local` / `--cloud`: narrow a bare NAME to one location. NAME can
/// also be a ref (`local:<name>`, `cloud:<name>`, `direct:<host:port>`); a
/// bare name must be unique across locations.
#[derive(Args, Clone, Copy, Debug, Default)]
pub struct Narrow {
    /// Look NAME up among local sandboxes only (same as `local:NAME`).
    #[arg(long, conflicts_with = "cloud")]
    local: bool,
    /// Look NAME up among cloud sandboxes only (same as `cloud:NAME`).
    #[arg(long)]
    cloud: bool,
}

impl Narrow {
    /// NAME, qualified when a flag narrows it.
    pub fn apply(self, name: &str) -> Result<String, CuaError> {
        let local = if self.local {
            Some(true)
        } else if self.cloud {
            Some(false)
        } else {
            None
        };
        if local.is_none() {
            return Ok(name.to_string());
        }
        cua_sdk::qualify_sandbox_ref(name.to_string(), local)
    }
}

#[derive(Args, Debug, Default)]
pub struct CreateArgs {
    /// Image: a registry reference (`ghcr.io/org/image:tag`), an alias
    /// (`linux`, `ubuntu`, `windows`, `macos[:tahoe|sequoia]`, `omarchy`),
    /// or locally
    /// `pool:<name>` to run that cloud pool's template image with its
    /// firmware, services and readiness probe (`fleet:<name>` is the
    /// deprecated spelling).
    image: Option<String>,
    /// Where it runs: local, cloud, direct:<addr> (an existing machine
    /// running cua-spacesd), or a registered provider. Default: `cua config
    /// get default.on` (`CUA_DEFAULT_ON`), else local.
    #[arg(long, value_parser = parse_on, value_name = "WHERE")]
    on: Option<On>,
    /// What kind of machine: auto (from the image: a container when it has
    /// a container rootfs; macOS, Windows and disk-only images are VMs),
    /// container or vm.
    #[arg(long, value_parser = parse_kind)]
    kind: Option<Kind>,
    /// Which engine: auto (the safest available) or one the location offers
    /// for the kind. Local: gvisor or runc (containers), qemu or lume (VMs).
    /// Cloud: gvisor (containers), kubevirt (VMs). A local sandbox with
    /// --sidecar needs `--runtime runc` where gVisor would run (separate
    /// gVisor containers cannot share a network namespace).
    #[arg(long, value_parser = parse_runtime)]
    runtime: Option<Runtime>,
    /// Image tier for an alias: `slim`, `full` (default) or macOS `xcode`.
    #[arg(long, value_name = "TIER")]
    tier: Option<String>,
    /// Sandbox name (generated when omitted).
    #[arg(long)]
    name: Option<String>,
    /// Start Chromium in it for the cua-driver browser tools (IMAGE
    /// defaults to `linux`).
    #[arg(long)]
    browser: bool,
    /// With --browser: open this URL.
    #[arg(long, value_name = "URL", requires = "browser")]
    open: Option<String>,
    /// Install these into the sandbox once it is up: harness ids
    /// (`claude-code`) or installables (`blender`, `vscode`, `node`), pinned
    /// and checksum-verified (`cua agent ensure` later does the same).
    #[arg(long = "install", value_name = "ID")]
    install: Vec<String>,
    /// vCPUs.
    #[arg(long, visible_alias = "cpus")]
    cpu: Option<u32>,
    /// Memory, e.g. 8GB or 4096MB (a bare number is GB).
    #[arg(long)]
    memory: Option<String>,
    /// Guest network: `default` (outbound network, like a Docker container)
    /// or `none` (no egress; published ports still work). `none` needs a
    /// local QEMU VM; containers, Lume and cloud sandboxes reject it.
    #[arg(long, value_parser = ["default", "none"])]
    network: Option<String>,
    /// Disk size (accepted; the image decides today).
    #[arg(long)]
    disk: Option<String>,
    /// Expose a guest port: NAME=PORT (a named service) or PORT (repeatable).
    #[arg(long = "port", visible_alias = "service", value_parser = parse_port_spec)]
    ports: Vec<(Option<String>, u16)>,
    /// Wait until ready: tcp:SERVICE, http:SERVICE/path, a guest port
    /// (tcp:PORT, http:PORT/path), or `desktop` (the desktop session takes
    /// input: display, window manager and cua-driver) (repeatable).
    #[arg(long = "wait", visible_alias = "wait-for", value_parser = parse_wait)]
    wait: Vec<(WaitTarget, Option<String>)>,
    /// Readiness budget in seconds, the wait for the image's cua-spacesd
    /// included (default 600, with at most 120 of it for cua-spacesd).
    #[arg(long)]
    ready_timeout: Option<u32>,
    /// Inject a freshly built binary once the sandbox is up:
    /// NAME=PATH (`cua-driver`, `cua-spacesd`) or NAME=PATH:GUEST_PATH
    /// (repeatable). It replaces the guest file atomically, is recorded with
    /// its sha256 (checked by `cua doctor`) and what runs it is restarted.
    /// Needs cua-spacesd in the image.
    #[arg(long = "overlay", value_name = "NAME=PATH[:GUEST_PATH]", value_parser = parse_overlay)]
    overlays: Vec<cua_sdk::Overlay>,
    /// Environment KEY=VALUE for the sandbox (repeatable).
    #[arg(long = "env", value_parser = parse_kv)]
    env: Vec<(String, String)>,
    /// The sandbox's command, after `--` (replaces the image's entrypoint):
    /// `cua sb create python:3.12-slim --service mcp=8765 -- python -m srv`.
    #[arg(last = true, value_name = "COMMAND")]
    command: Vec<String>,
    /// A sidecar container, reachable from the sandbox at its name (it
    /// reaches the sandbox at `main`):
    /// IMAGE[,port=PORT][,env=K=V][,name=NAME] (repeatable; `port=` and
    /// `env=` repeat too). Local containers and every cloud sandbox; locally
    /// it needs `--runtime runc` where gVisor would run. With sidecars the
    /// service names main, sidecars and sc are reserved.
    #[arg(long = "sidecar", value_parser = parse_sidecar)]
    sidecars: Vec<cua_sdk::Container>,
    /// Credentials for a private image: env:USER_VAR:PASSWORD_VAR (read from
    /// this environment) or aws-ecr[:REGION] (the AWS CLI's login token).
    #[arg(long, value_parser = parse_registry_secret)]
    registry_secret: Option<cua_sdk::RegistrySecret>,
    /// Cloud: keep one warm sandbox of this image ready (the default for the
    /// canonical linux/windows/macos images).
    #[arg(long, help_heading = "Cloud (advanced)", conflicts_with = "no_warm")]
    warm: bool,
    /// Cloud: no warm sandbox, even for a canonical image.
    #[arg(long, help_heading = "Cloud (advanced)")]
    no_warm: bool,
    /// Cloud: most sandboxes of this image at once.
    #[arg(long, help_heading = "Cloud (advanced)")]
    max_pool_size: Option<u32>,
    /// Cloud: how long the sandbox outlives this command without a
    /// keep-alive (`15m`, `900`); renewed while the daemon holds it.
    #[arg(long, value_parser = parse_secs, help_heading = "Cloud (advanced)")]
    claim_ttl: Option<u32>,
    /// Cloud: use this dedicated pool instead of shared capacity. The
    /// sandbox fields given (IMAGE, --cmd, --env, --port, --sidecar, --cpu,
    /// --memory) must match its template, or creation fails with the diff.
    #[arg(long, help_heading = "Cloud (advanced)")]
    pool: Option<String>,
    /// Cloud, with --pool: update the pool's template to the given fields
    /// instead of failing when they differ.
    #[arg(long, requires = "pool", help_heading = "Cloud (advanced)")]
    apply: bool,
    /// spacesd token (direct: sandboxes).
    #[arg(long, env = "CUA_ENV_TOKEN", hide_env_values = true)]
    token: Option<String>,
    /// Deprecated: use IMAGE.
    #[arg(long = "image", hide = true)]
    image_flag: Option<String>,
    /// Deprecated: use --wait tcp:PORT.
    #[arg(long = "wait-tcp", hide = true)]
    wait_tcp: Vec<u16>,
    /// Deprecated: use --wait http:PORT/path.
    #[arg(long = "wait-http", value_parser = parse_http_probe, hide = true)]
    wait_http: Vec<(u16, String)>,
    /// Open the desktop in the HTML5 viewer once the sandbox is ready
    /// (`cua sb view`).
    #[arg(long)]
    view: bool,
    /// Keep the sandbox (its cloud claim, VM or container) when it fails to
    /// become ready, to debug it; the ref and how to delete it are printed.
    /// Default: a failed create deletes it and releases its claim.
    #[arg(long)]
    keep_on_failure: bool,
    /// A GPU: the runtime's own (`--gpu`), or an option `cua spaces gpus`
    /// lists (`paravirtual`: GPU acceleration for a macOS VM on Lume,
    /// experimental; `virgl`: QEMU on Linux; `nvidia`: a runc container on
    /// Linux; a provider's GPU type).
    #[arg(long, num_args = 0..=1, default_missing_value = "auto", value_name = "OPTION")]
    gpu: Option<String>,
}

#[derive(Args, Debug)]
pub struct LaunchArgs {
    /// Image: `linux`/`ubuntu`, `macos[:tahoe|sequoia]`, or a registry
    /// reference such as `ghcr.io/org/image:tag`.
    image: Option<String>,
    /// Use this dedicated cloud pool (requires --name). With --on local, run
    /// that pool's template locally instead.
    #[arg(long)]
    pool: Option<String>,
    /// Deprecated: use `--on local`.
    #[arg(long, hide = true)]
    local: bool,
    /// Where it runs (default cloud for launch; see `sb create --on`).
    #[arg(long, value_parser = parse_on, value_name = "WHERE", conflicts_with = "local")]
    on: Option<On>,
    /// Sandbox name.
    #[arg(long)]
    name: Option<String>,
    /// Deprecated: use `--kind vm`.
    #[arg(long, hide = true, conflicts_with = "kind")]
    vm: bool,
    /// What kind of machine: auto, container or vm.
    #[arg(long, value_parser = parse_kind)]
    kind: Option<Kind>,
    /// Which engine (see `sb create --runtime`).
    #[arg(long, value_parser = parse_runtime)]
    runtime: Option<Runtime>,
    /// vCPUs.
    #[arg(long, visible_alias = "cpus")]
    cpu: Option<u32>,
    /// Memory, e.g. 8GB or 4096MB (a bare number is GB).
    #[arg(long)]
    memory: Option<String>,
    /// Disk size (accepted for compatibility; sized by the image).
    #[arg(long, hide = true)]
    disk: Option<String>,
    /// Cloud region (accepted for compatibility; the cloud places sandboxes).
    #[arg(long, hide = true)]
    region: Option<String>,
    /// Expose a guest port: NAME=PORT or PORT (repeatable).
    #[arg(long = "port", visible_alias = "service", value_parser = parse_port_spec)]
    ports: Vec<(Option<String>, u16)>,
    /// Cloud: keep one warm sandbox of this image ready.
    #[arg(long)]
    warm: bool,
    /// Cloud: most sandboxes of this image at once.
    #[arg(long)]
    max_pool_size: Option<u32>,
    /// Cloud: how long the sandbox outlives this command without a
    /// keep-alive (`15m`, `900`).
    #[arg(long, value_parser = parse_secs)]
    claim_ttl: Option<u32>,
    /// Keep the sandbox when it fails to become ready (see `sb create
    /// --keep-on-failure`).
    #[arg(long)]
    keep_on_failure: bool,
}

impl LaunchArgs {
    /// The equivalent `sb create` arguments (launch defaults to the cloud).
    fn into_create(self) -> Result<CreateArgs, CuaError> {
        if self.image.is_some() == self.pool.is_some() {
            return Err(CuaError::InvalidArgument(
                "specify exactly one of IMAGE or --pool".into(),
            ));
        }
        let on = if self.local {
            eprintln!("warning: `--local` is deprecated; use `--on local`");
            On::Local
        } else {
            self.on.unwrap_or(On::Cloud)
        };
        // A cloud pool sandbox needs a name to reattach; running the pool's
        // template locally does not.
        if self.pool.is_some() && self.name.is_none() && on != On::Local {
            return Err(CuaError::InvalidArgument(
                "--name is required with --pool".into(),
            ));
        }
        if self.region.is_some() {
            eprintln!("note: --region is ignored (the cloud places sandboxes)");
        }
        let kind = if self.vm {
            eprintln!("warning: `--vm` is deprecated; use `--kind vm`");
            Some(Kind::Vm)
        } else {
            self.kind
        };
        Ok(CreateArgs {
            image: self.image,
            on: Some(on),
            kind,
            runtime: self.runtime,
            name: self.name,
            cpu: self.cpu,
            memory: self.memory,
            disk: self.disk,
            ports: self.ports,
            warm: self.warm,
            max_pool_size: self.max_pool_size,
            claim_ttl: self.claim_ttl,
            pool: self.pool,
            keep_on_failure: self.keep_on_failure,
            ..Default::default()
        })
    }
}

#[derive(Subcommand, Debug)]
pub enum SandboxCmd {
    /// Create a sandbox: `cua sb create IMAGE [--on local|cloud|direct:<addr>]
    /// [--kind auto|container|vm] [--runtime auto|gvisor|runc|qemu|lume|kubevirt]`.
    #[command(after_help = "Examples:
  # A Linux desktop on this machine (a gVisor container)
  cua sb create linux --name dev
  # A web server container, ready when its HTTP service answers
  cua sb create python:3.12-slim --service web=8000 --wait http:web/ -- python -m http.server 8000
  # The same Linux image in the cloud instead, as a VM
  cua sb create linux --on cloud --kind vm --name dev")]
    Create(Box<CreateArgs>),
    /// Deprecated alias of `create` that defaults to `--on cloud`.
    #[command(after_help = "Examples:
  # A cloud Linux sandbox (same as `cua sb create linux --on cloud`)
  cua sb launch linux --name dev")]
    Launch(LaunchArgs),
    /// Reattach to a sandbox (NAME or a ref such as `cloud:NAME`) and print
    /// it.
    #[command(after_help = "Examples:
  cua sb connect dev
  cua sb connect cloud:dev")]
    Connect {
        /// Sandbox name or ref (`local:NAME`, `cloud:NAME`).
        name: String,
        #[command(flatten)]
        at: Narrow,
    },
    /// List sandboxes: local, direct and cloud (live Fleet claims) by
    /// default, each with its `location`; `--local` or `--cloud` filters.
    #[command(
        visible_alias = "list",
        after_help = "Examples:
  cua sb ls
  cua sb ls --cloud --json"
    )]
    Ls {
        /// Only local sandboxes.
        #[arg(long, conflicts_with = "cloud")]
        local: bool,
        /// Only cloud sandboxes, listed live from Fleet.
        #[arg(long)]
        cloud: bool,
        /// Deprecated: everything is the default.
        #[arg(long, hide = true)]
        all: bool,
        /// Skip the SIZE column (disk used by each local sandbox), which
        /// reads the VM, container and Lume stores.
        #[arg(long)]
        no_size: bool,
    },
    /// Show one sandbox (or a Fleet pool of that name).
    #[command(
        visible_alias = "get",
        after_help = "Examples:
  cua sb info dev
  cua sb info cloud:dev --json"
    )]
    Info {
        /// Sandbox name or ref (`local:NAME`, `cloud:NAME`).
        name: String,
        #[command(flatten)]
        at: Narrow,
    },
    /// Suspend (local: pause/snapshot; Fleet managed pool: release the
    /// claim and keep the record; explicit Fleet pool: scale to zero).
    #[command(after_help = "Examples:
  cua sb suspend dev")]
    Suspend {
        /// Sandbox name or ref (`local:NAME`, `cloud:NAME`).
        name: String,
        #[command(flatten)]
        at: Narrow,
    },
    /// Resume (Fleet managed pool: claim a fresh sandbox of the same shape).
    #[command(after_help = "Examples:
  cua sb resume dev")]
    Resume {
        /// Sandbox name or ref (`local:NAME`, `cloud:NAME`).
        name: String,
        #[command(flatten)]
        at: Narrow,
    },
    /// Restart.
    #[command(after_help = "Examples:
  cua sb restart dev")]
    Restart {
        /// Sandbox name or ref (`local:NAME`, `cloud:NAME`).
        name: String,
        #[command(flatten)]
        at: Narrow,
    },
    /// Extend a Fleet claim's lease.
    #[command(
        name = "keep-alive",
        after_help = "Examples:
  # Keep the claim for two more hours
  cua sb keep-alive cloud:dev --for 2h"
    )]
    KeepAlive {
        /// Sandbox name or ref (`local:NAME`, `cloud:NAME`).
        name: String,
        #[command(flatten)]
        at: Narrow,
        /// Lease from now (`15m`, `2h`, `900`).
        #[arg(long = "for", default_value = "15m", value_parser = parse_secs)]
        duration: u32,
    },
    /// Delete a sandbox (Fleet: release the claim; managed pools are kept
    /// for reuse).
    #[command(
        visible_alias = "delete",
        after_help = "Examples:
  cua sb rm dev
  # Without the prompt (required in scripts and agents)
  cua sb rm cloud:dev --force"
    )]
    Rm {
        /// Sandbox name or ref (`local:NAME`, `cloud:NAME`).
        name: String,
        /// Skip the confirmation prompt. Required when there is no terminal
        /// to ask on (scripts, agents): without it nothing is deleted.
        #[arg(long, short)]
        force: bool,
        #[command(flatten)]
        at: Narrow,
    },
    /// Run a command (joined into one shell line, as `sh -c`). Exits with
    /// the command's own status. A local Lume sandbox without cua-spacesd
    /// runs it via SSH (`lume ssh`).
    #[command(after_help = "Examples:
  cua sb exec dev uname -a
  cua sb exec dev 'ls /tmp | wc -l'")]
    Exec {
        #[command(flatten)]
        at: Narrow,
        /// Sandbox name or ref (`local:NAME`, `cloud:NAME`).
        name: String,
        /// Command and arguments.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        command: Vec<String>,
    },
    /// Interactive shell (PTY), or run a command in a PTY.
    #[command(after_help = "Examples:
  cua sb shell dev
  cua sb shell dev top")]
    Shell {
        #[command(flatten)]
        at: Narrow,
        /// Terminal width (default: this terminal's).
        #[arg(long)]
        cols: Option<u32>,
        /// Terminal height (default: this terminal's).
        #[arg(long)]
        rows: Option<u32>,
        /// Sandbox name or ref (`local:NAME`, `cloud:NAME`).
        name: String,
        /// Command to run instead of the login shell.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// Copy files: `cp NAME:/guest/path LOCAL` or `cp LOCAL NAME:/guest/path`
    /// (NAME may be a ref: `cp local:box:/tmp/x .`).
    #[command(after_help = "Examples:
  # Upload
  cua sb cp ./notes.txt dev:/tmp/notes.txt
  # Download
  cua sb cp dev:/tmp/notes.txt .")]
    Cp {
        /// Source: a local path or `NAME:/guest/path`.
        src: String,
        /// Destination: a local path or `NAME:/guest/path`.
        dst: String,
        #[command(flatten)]
        at: Narrow,
    },
    /// Inject freshly built binaries into a running sandbox (see
    /// `create --overlay`): NAME=PATH or NAME=PATH:GUEST_PATH.
    #[command(after_help = "Examples:
  # The cua-driver under test, then check that it is what runs
  cua sb overlay dev cua-driver=./target/release/cua-driver
  cua doctor dev --expect cua-driver=sha256:<sha256>
  # The daemon too (restarts it and waits for the new build)
  cua sb overlay dev cua-spacesd=./target/release/cua-spacesd")]
    Overlay {
        /// Sandbox name or ref (`local:NAME`, `cloud:NAME`).
        name: String,
        /// NAME=PATH[:GUEST_PATH] (repeatable).
        #[arg(required = true, value_parser = parse_overlay)]
        overlays: Vec<cua_sdk::Overlay>,
        #[command(flatten)]
        at: Narrow,
        /// Budget for a cua-spacesd restart, in seconds.
        #[arg(long, default_value_t = 180)]
        timeout: u32,
    },
    /// Show logs: the backend console (QEMU serial, container) when there
    /// is one, else the guest system log through the spacesd.
    #[command(after_help = "Examples:
  cua sb logs dev
  cua sb logs dev -n 50 --source guest")]
    Logs {
        /// Sandbox name or ref (`local:NAME`, `cloud:NAME`).
        name: String,
        #[command(flatten)]
        at: Narrow,
        /// Lines from the end.
        #[arg(long, short = 'n', default_value_t = 200)]
        tail: u32,
        /// auto, console or guest.
        #[arg(long, default_value = "auto")]
        source: String,
    },
    /// Forward a guest port: `port-forward NAME PORT[:LOCAL]` (a local
    /// sandbox's port not published at create, and cloud sandboxes, tunnel
    /// through cua-spacesd when the image has it; cloud images without it
    /// get HTTP and WebSocket through a loopback proxy to the cloud gateway).
    #[command(
        name = "port-forward",
        visible_alias = "forward",
        after_help = "Examples:
  # Guest port 8080 on localhost:8080 until Ctrl-C
  cua sb port-forward dev 8080
  # Guest port 5432 on localhost:15432
  cua sb port-forward cloud:dev 5432:15432"
    )]
    PortForward {
        /// Sandbox name or ref (`local:NAME`, `cloud:NAME`).
        name: String,
        #[command(flatten)]
        at: Narrow,
        /// PORT or PORT:LOCAL_PORT.
        spec: String,
        /// Print the target and exit instead of forwarding until Ctrl-C.
        #[arg(long)]
        no_wait: bool,
    },
    /// Print a URL for a named service: usable from this machine (default),
    /// or `--public` for a shareable URL that expires (`--ttl`, default 1h;
    /// cloud: a signed URL; local: a token URL served by the cua daemon).
    #[command(after_help = "Examples:
  cua sb url dev web
  # A shareable URL for 10 minutes
  cua sb url cloud:dev web --public --ttl 10m")]
    Url {
        /// Sandbox name or ref (`local:NAME`, `cloud:NAME`).
        name: String,
        /// Service name (from `--service` at create time).
        service: String,
        #[command(flatten)]
        at: Narrow,
        /// A shareable URL that expires.
        #[arg(long)]
        public: bool,
        /// Lifetime of a --public URL (`10m`, `1h`, `3600`; 60 s to 24 h).
        #[arg(long, value_parser = parse_secs)]
        ttl: Option<u32>,
    },
    /// Save a screenshot (PNG). A local Lume sandbox without cua-spacesd is
    /// captured via VNC.
    #[command(after_help = "Examples:
  cua sb screenshot dev -o desktop.png")]
    Screenshot {
        /// Sandbox name or ref (`local:NAME`, `cloud:NAME`).
        name: String,
        #[command(flatten)]
        at: Narrow,
        /// Output file (PNG).
        #[arg(short, long, default_value = "screenshot.png")]
        output: PathBuf,
    },
    /// Talk to an MCP server a sandbox serves: `mcp NAME SERVICE tools`,
    /// `mcp NAME SERVICE call TOOL '{"a":1}'`. Generic streamable HTTP over
    /// the service (local or cloud); no cua-spacesd needed.
    #[command(after_help = "Examples:
  cua sb mcp dev mcp tools
  cua sb mcp dev mcp call echo '{\"text\":\"hi\"}'")]
    Mcp {
        /// Sandbox name or ref (`local:NAME`, `cloud:NAME`).
        name: String,
        /// Service name (for example `mcp`, or `port-8765`).
        service: String,
        #[command(flatten)]
        at: Narrow,
        /// Endpoint path on the service.
        #[arg(long, default_value = "/mcp")]
        path: String,
        #[command(subcommand)]
        action: McpAction,
    },
    /// Open the sandbox desktop in the browser (the cua-spacesd HTML5 viewer).
    /// A local Lume sandbox without cua-spacesd opens its VNC display with
    /// `lume attach`.
    #[command(after_help = "Examples:
  cua sb view dev
  # Watch only, print the link instead of opening it
  cua sb view dev --view-only --no-open
  # An image that ships its own web display instead of cua-spacesd
  cua sb view old-box --service web")]
    View(ViewArgs),
    /// Deprecated spelling of `view` (it opens the HTML5 viewer).
    #[command(hide = true)]
    Vnc(ViewArgs),
    /// Headless stream viewer for image gates and measurements: opens a
    /// media session, sends `--action` input on it and prints timings as
    /// JSON.
    #[command(hide = true)]
    StreamProbe {
        #[command(flatten)]
        args: crate::stream_probe::StreamProbeArgs,
        #[command(flatten)]
        at: Narrow,
    },
}

/// `cua sb view`.
#[derive(Args, Debug, Clone)]
pub struct ViewArgs {
    /// Sandbox name or ref (`local:NAME`, `cloud:NAME`).
    pub name: String,
    /// Open this web service of the sandbox instead of the viewer (for
    /// images without cua-spacesd that serve their own display page).
    #[arg(long)]
    pub service: Option<String>,
    /// Watch only: no input, clipboard, files or microphone.
    #[arg(long)]
    pub view_only: bool,
    /// Guest folder for uploads and folder sharing (default `~`; `none`
    /// turns files off).
    #[arg(long)]
    pub files: Option<String>,
    /// Link lifetime (for example `1h`, `30m`). Default 1h.
    #[arg(long)]
    pub ttl: Option<String>,
    /// Print the link without opening a browser.
    #[arg(long)]
    pub no_open: bool,
    #[command(flatten)]
    pub at: Narrow,
}

/// `cua sb mcp NAME SERVICE <action>`.
#[derive(Subcommand, Debug)]
pub enum McpAction {
    /// The endpoint URL and headers, for any MCP client. Credential headers
    /// print masked (`Bearer ****`) unless `--show-secrets`; Fleet bearers
    /// are short-lived.
    #[command(after_help = "Examples:
  cua sb mcp dev mcp config")]
    Config {
        /// Print credential header values in full.
        #[arg(long)]
        show_secrets: bool,
    },
    /// Server name, version and negotiated protocol revision.
    #[command(after_help = "Examples:
  cua sb mcp dev mcp info")]
    Info,
    /// List tools.
    #[command(after_help = "Examples:
  cua sb mcp dev mcp tools")]
    Tools,
    /// Call a tool with a JSON object of arguments.
    #[command(after_help = "Examples:
  cua sb mcp dev mcp call echo '{\"text\":\"hi\"}'
  cua sb mcp dev mcp call screenshot --out ./shots")]
    Call {
        /// Tool name.
        tool: String,
        /// Arguments (JSON object). Default `{}`.
        arguments: Option<String>,
        /// Save image, audio and blob content here instead of summarizing.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// List resources.
    #[command(after_help = "Examples:
  cua sb mcp dev mcp resources")]
    Resources,
    /// List resource templates.
    #[command(after_help = "Examples:
  cua sb mcp dev mcp templates")]
    Templates,
    /// Read a resource.
    #[command(after_help = "Examples:
  cua sb mcp dev mcp read file:///tmp/report.txt")]
    Read {
        /// Resource URI.
        uri: String,
        /// Save blob contents here instead of summarizing.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// List prompts.
    #[command(after_help = "Examples:
  cua sb mcp dev mcp prompts")]
    Prompts,
    /// Render a prompt with a JSON object of string arguments.
    #[command(after_help = "Examples:
  cua sb mcp dev mcp prompt summarize '{\"topic\":\"logs\"}'")]
    Prompt {
        /// Prompt name.
        name: String,
        /// Arguments (JSON object of strings). Default `{}`.
        arguments: Option<String>,
    },
    /// Any other method (for example `skills/list`), printing the result.
    #[command(after_help = "Examples:
  cua sb mcp dev mcp request skills/list")]
    Request {
        /// JSON-RPC method.
        method: String,
        /// Params (JSON object).
        params: Option<String>,
    },
}

/// Renders MCP content blocks for a terminal: text as is; image, audio and
/// blob contents summarized, or saved under `out`; everything else as JSON.
fn render_blocks(blocks: &[serde_json::Value], out: Option<&PathBuf>) -> Result<String, CuaError> {
    use base64::Engine;
    let mut lines = Vec::new();
    let mut saved = 0usize;
    let mut save = |data: &str, mime: &str, hint: &str| -> Result<String, CuaError> {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|e| CuaError::Internal(format!("base64 content: {e}")))?;
        let Some(dir) = out else {
            return Ok(format!("[{hint} {mime}, {} bytes]", bytes.len()));
        };
        std::fs::create_dir_all(dir).map_err(internal)?;
        saved += 1;
        let ext = mime.rsplit('/').next().unwrap_or("bin");
        let path = dir.join(format!("{hint}-{saved}.{ext}"));
        std::fs::write(&path, &bytes).map_err(internal)?;
        Ok(format!(
            "[{hint} {mime}, {} bytes: {}]",
            bytes.len(),
            path.display()
        ))
    };
    for b in blocks {
        let s = |k: &str| b.get(k).and_then(|v| v.as_str()).unwrap_or_default();
        let r = |k: &str| {
            b.pointer(&format!("/resource/{k}"))
                .and_then(|v| v.as_str())
                .unwrap_or_default()
        };
        lines.push(match s("type") {
            "text" => s("text").to_string(),
            "image" | "audio" => save(s("data"), s("mimeType"), s("type"))?,
            "resource_link" => format!("[link {} ({})]", s("uri"), s("mimeType")),
            "resource" if b.pointer("/resource/text").is_some() => r("text").to_string(),
            "resource" => save(r("blob"), r("mimeType"), "blob")?,
            // resources/read contents: {uri, mimeType, text | blob}.
            "" if b.get("text").is_some() => s("text").to_string(),
            "" if b.get("blob").is_some() => save(s("blob"), s("mimeType"), "blob")?,
            _ => b.to_string(),
        });
    }
    Ok(lines.join("\n"))
}

async fn mcp(
    cua: &Arc<Cua>,
    name: &str,
    service: &str,
    path: &str,
    action: McpAction,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    // Direct sandboxes carry their registered spacesd token, which
    // cua-spacesd's `/mcp` requires.
    let sb = sandbox_with_token(cua, name).await?;
    let parse = |s: &str| serde_json::from_str::<serde_json::Value>(s).unwrap_or_default();
    if let McpAction::Config { show_secrets } = action {
        let c = sb
            .mcp_config(service.to_string(), Some(path.to_string()))
            .await?;
        let headers: serde_json::Map<String, serde_json::Value> = c
            .headers
            .iter()
            .map(|h| {
                let v = util::header_for_display(&h.name, &h.value, show_secrets);
                (h.name.clone(), serde_json::Value::String(v))
            })
            .collect();
        let v = serde_json::json!({"type": "http", "url": c.url, "headers": headers});
        line(out, serde_json::to_string_pretty(&v).unwrap_or_default());
        return Ok(0);
    }
    let client = sb.mcp(service.to_string(), Some(path.to_string())).await?;
    let list = |v: &serde_json::Value, key: &str, extra: &str| {
        v.as_array()
            .map(|a| {
                a.iter()
                    .map(|t| {
                        let first = t[extra]
                            .as_str()
                            .unwrap_or_default()
                            .lines()
                            .next()
                            .unwrap_or_default()
                            .to_string();
                        format!("{:<32} {first}", t[key].as_str().unwrap_or_default())
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default()
    };
    let result: Result<i32, CuaError> = async {
        match action {
            McpAction::Config { .. } => unreachable!(),
            McpAction::Info => {
                let v = client
                    .server_info_json()
                    .map(|s| parse(&s))
                    .unwrap_or_default();
                print(out, v.clone(), json, || {
                    format!(
                        "{} {} (MCP {})",
                        v.pointer("/serverInfo/name")
                            .and_then(|x| x.as_str())
                            .unwrap_or("?"),
                        v.pointer("/serverInfo/version")
                            .and_then(|x| x.as_str())
                            .unwrap_or(""),
                        v["protocolVersion"].as_str().unwrap_or("?")
                    )
                });
                Ok(0)
            }
            McpAction::Tools => {
                let v = parse(&client.list_tools().await?);
                print(out, v.clone(), json, || list(&v, "name", "description"));
                Ok(0)
            }
            McpAction::Call {
                tool,
                arguments,
                out: dir,
            } => {
                let v = parse(&client.call_tool(tool, arguments).await?);
                let is_error = v["isError"].as_bool().unwrap_or(false);
                if json {
                    line(out, v.to_string());
                } else {
                    let blocks = v["content"].as_array().cloned().unwrap_or_default();
                    let mut text = render_blocks(&blocks, dir.as_ref())?;
                    if text.is_empty()
                        && let Some(s) = v.get("structuredContent")
                    {
                        text = s.to_string();
                    }
                    line(out, text);
                }
                Ok(i32::from(is_error))
            }
            McpAction::Resources => {
                let v = parse(&client.list_resources().await?);
                print(out, v.clone(), json, || list(&v, "uri", "name"));
                Ok(0)
            }
            McpAction::Templates => {
                let v = parse(&client.list_resource_templates().await?);
                print(out, v.clone(), json, || list(&v, "uriTemplate", "name"));
                Ok(0)
            }
            McpAction::Read { uri, out: dir } => {
                let v = parse(&client.read_resource(uri).await?);
                if json {
                    line(out, v.to_string());
                } else {
                    let blocks = v["contents"].as_array().cloned().unwrap_or_default();
                    line(out, render_blocks(&blocks, dir.as_ref())?);
                }
                Ok(0)
            }
            McpAction::Prompts => {
                let v = parse(&client.list_prompts().await?);
                print(out, v.clone(), json, || list(&v, "name", "description"));
                Ok(0)
            }
            McpAction::Prompt { name, arguments } => {
                let v = parse(&client.get_prompt(name, arguments).await?);
                print(out, v.clone(), json, || {
                    serde_json::to_string_pretty(&v).unwrap_or_default()
                });
                Ok(0)
            }
            McpAction::Request { method, params } => {
                let v = parse(&client.request(method, params).await?);
                print(out, v.clone(), json, || {
                    serde_json::to_string_pretty(&v).unwrap_or_default()
                });
                Ok(0)
            }
        }
    }
    .await;
    let _ = client.close().await;
    result
}

fn parse_port_spec(s: &str) -> Result<(Option<String>, u16), String> {
    match s.split_once('=') {
        Some((k, v)) => Ok((Some(k.to_string()), v.parse().map_err(|_| "bad port")?)),
        None => Ok((None, s.parse().map_err(|_| "expected NAME=PORT or PORT")?)),
    }
}

/// `--wait desktop` budget without `--ready-timeout` (seconds).
const DESKTOP_READY_TIMEOUT_SECS: u32 = 180;

/// A readiness probe target: a guest port or a declared service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WaitTarget {
    /// Guest port.
    Port(u16),
    /// Declared service name.
    Service(String),
    /// The desktop session takes input (cua-spacesd's `desktop` health).
    Desktop,
}

impl WaitTarget {
    fn parse(s: &str) -> Result<Self, String> {
        if s.is_empty() {
            return Err("expected a port or a service name".into());
        }
        match s.parse::<u16>() {
            Ok(0) => Err("bad port".into()),
            Ok(p) => Ok(WaitTarget::Port(p)),
            Err(_)
                if s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') =>
            {
                Ok(WaitTarget::Service(s.to_string()))
            }
            Err(_) => Err(format!("bad port or service name {s:?}")),
        }
    }

    fn probe(&self, http_path: Option<String>) -> Option<ReadinessProbe> {
        Some(match self {
            WaitTarget::Desktop => return None,
            WaitTarget::Port(port) => ReadinessProbe {
                port: *port,
                http_path,
                http_status: None,
                service: None,
            },
            WaitTarget::Service(name) => ReadinessProbe {
                port: 0,
                http_path,
                http_status: None,
                service: Some(name.clone()),
            },
        })
    }
}

fn parse_wait(s: &str) -> Result<(WaitTarget, Option<String>), String> {
    if s == "desktop" {
        return Ok((WaitTarget::Desktop, None));
    }
    if let Some(p) = s.strip_prefix("tcp:") {
        return Ok((WaitTarget::parse(p)?, None));
    }
    if let Some(r) = s.strip_prefix("http:") {
        let (p, path) = match r.find('/') {
            Some(i) => (&r[..i], &r[i..]),
            None => (r, "/"),
        };
        return Ok((WaitTarget::parse(p)?, Some(path.to_string())));
    }
    Err("expected tcp:SERVICE, http:SERVICE/path, tcp:PORT, http:PORT/path or desktop".into())
}

/// `IMAGE[,port=PORT][,env=K=V][,name=NAME]`.
fn parse_sidecar(s: &str) -> Result<cua_sdk::Container, String> {
    let mut parts = s.split(',');
    let image = parts.next().unwrap_or_default().trim();
    if image.is_empty() || image.contains('=') {
        return Err("expected IMAGE[,port=PORT][,env=K=V][,name=NAME]".into());
    }
    let mut c = cua_sdk::Container {
        image: image.to_string(),
        command: None,
        env: Default::default(),
        ports: vec![],
        name: None,
    };
    for part in parts {
        let (k, v) = part
            .split_once('=')
            .ok_or_else(|| format!("sidecar option {part:?}: expected key=value"))?;
        match k.trim() {
            "port" => c.ports.push(
                v.trim()
                    .parse()
                    .map_err(|_| format!("bad sidecar port {v:?}"))?,
            ),
            "env" => {
                let (ek, ev) = v
                    .split_once('=')
                    .ok_or_else(|| format!("sidecar env {v:?}: expected env=K=V"))?;
                c.env.insert(ek.to_string(), ev.to_string());
            }
            "name" => c.name = Some(v.trim().to_string()),
            other => {
                return Err(format!(
                    "unknown sidecar option {other:?} (port, env, name)"
                ));
            }
        }
    }
    Ok(c)
}

/// `env:USER_VAR:PASSWORD_VAR` or `aws-ecr[:REGION]`. Passwords never go
/// on the command line.
fn parse_registry_secret(s: &str) -> Result<cua_sdk::RegistrySecret, String> {
    if let Some(rest) = s.strip_prefix("env:") {
        let (u, p) = rest
            .split_once(':')
            .ok_or("expected env:USER_VAR:PASSWORD_VAR")?;
        return Ok(cua_sdk::RegistrySecret::FromEnv {
            username_var: u.to_string(),
            password_var: p.to_string(),
            registry: None,
        });
    }
    if s == "aws-ecr" || s.starts_with("aws-ecr:") {
        return Ok(cua_sdk::RegistrySecret::AwsEcr {
            region: s.strip_prefix("aws-ecr:").map(str::to_string),
        });
    }
    Err("expected env:USER_VAR:PASSWORD_VAR or aws-ecr[:REGION]".into())
}

fn parse_overlay(s: &str) -> Result<cua_sdk::Overlay, String> {
    cua_sdk::Overlay::parse(s).map_err(|e| e.to_string())
}

/// Prints what overlays did (stderr) and returns them as JSON.
fn report_overlays(results: &[cua_sdk::OverlayResult]) -> serde_json::Value {
    for r in results {
        eprintln!(
            "overlay {} -> {} sha256:{}{}",
            r.name,
            r.target,
            r.sha256,
            if r.restarted.is_empty() {
                String::new()
            } else {
                format!(" (restarted {})", r.restarted)
            }
        );
    }
    serde_json::Value::Array(
        results
            .iter()
            .map(|r| {
                serde_json::json!({
                    "name": r.name,
                    "target": r.target,
                    "sha256": r.sha256,
                    "previous_sha256": r.previous_sha256,
                    "size": r.size,
                    "restarted": r.restarted,
                    "expect": format!("{}=sha256:{}", r.name, r.sha256),
                })
            })
            .collect(),
    )
}

fn parse_kv(s: &str) -> Result<(String, String), String> {
    let (k, v) = s.split_once('=').ok_or("expected KEY=VALUE")?;
    Ok((k.to_string(), v.to_string()))
}

/// Seconds from `900`, `15m`, `2h`.
fn parse_secs(s: &str) -> Result<u32, String> {
    let d = match s.trim().parse::<u64>() {
        Ok(n) => std::time::Duration::from_secs(n),
        Err(_) => humantime::parse_duration(s.trim()).map_err(|e| e.to_string())?,
    };
    u32::try_from(d.as_secs()).map_err(|_| "too long".into())
}

fn parse_http_probe(s: &str) -> Result<(u16, String), String> {
    let (p, path) = s.split_once(':').ok_or("expected PORT:/path")?;
    Ok((p.parse().map_err(|_| "bad port")?, path.to_string()))
}

/// Memory string to MiB: `8GB` → 8192, `4096MB` → 4096, `8` → 8192.
///
/// A bare number is GB, so `4096` (meant as MiB) would ask for 4 TiB, which
/// QEMU refuses to start. Bare numbers above [`MAX_BARE_MEMORY_GB`] are
/// rejected as ambiguous, and so is anything above [`MAX_MEMORY_MIB`].
pub fn parse_memory(s: &str) -> Result<u64, CuaError> {
    let t = s.trim().to_ascii_uppercase();
    let bad = || CuaError::InvalidArgument(format!("bad memory {s:?} (use 8GB or 4096MB)"));
    let (num, mul, bare) = if let Some(n) = t.strip_suffix("GB").or_else(|| t.strip_suffix('G')) {
        (n, 1024.0, false)
    } else if let Some(n) = t.strip_suffix("MB").or_else(|| t.strip_suffix('M')) {
        (n, 1.0, false)
    } else {
        (t.as_str(), 1024.0, true)
    };
    let v: f64 = num.trim().parse().map_err(|_| bad())?;
    if !v.is_finite() || v <= 0.0 {
        return Err(bad());
    }
    if bare && v > MAX_BARE_MEMORY_GB {
        return Err(CuaError::InvalidArgument(format!(
            "memory {s:?} is ambiguous: a bare number is GB ({v} GB); write {v}MB or {}GB",
            (v / 1024.0).max(1.0).round()
        )));
    }
    let mib = v * mul;
    if mib > MAX_MEMORY_MIB as f64 {
        return Err(CuaError::InvalidArgument(format!(
            "memory {s:?} is {} GiB, above the {} GiB limit (use 8GB or 4096MB)",
            (mib / 1024.0).round(),
            MAX_MEMORY_MIB / 1024
        )));
    }
    Ok(mib as u64)
}

/// Largest bare (unit-less, GB) memory number accepted.
pub const MAX_BARE_MEMORY_GB: f64 = 512.0;

/// Largest memory a sandbox can ask for: 1 TiB.
pub const MAX_MEMORY_MIB: u64 = 1024 * 1024;

/// Resolves an image alias to (image reference, guest OS). The aliases
/// `linux`, `windows` and `macos` (optionally `:<version>`, e.g.
/// `macos:tahoe`) and the bare word `ubuntu` name the canonical images `ghcr.io/trycua/{linux:24.04,
/// windows:2022,macos:26}` (override with `CUA_IMAGE_LINUX` /
/// `CUA_IMAGE_WINDOWS` / `CUA_IMAGE_MACOS`), `tier` picks `-slim` or
/// `-xcode`, and `omarchy[:<channel>]` names `ghcr.io/trycua/omarchy:edge`;
/// the one resolver then picks the variant the backend runs. Anything else
/// is an image reference. Catalog images CI has not published yet fail
/// with `ImageNotPublished`.
pub fn resolve_image(
    image: &str,
    tier: Option<&str>,
) -> Result<(String, Option<String>), CuaError> {
    resolve_image_with(image, tier, |name| std::env::var(name).ok())
}

/// [`resolve_image`] with an explicit environment lookup (for tests).
pub fn resolve_image_with(
    image: &str,
    tier: Option<&str>,
    env: impl Fn(&str) -> Option<String>,
) -> Result<(String, Option<String>), CuaError> {
    Ok(
        match cua_image::canonical::resolve_word_with(image, tier, &env).map_err(word_error)? {
            Some((r, os)) => (r, Some(os.as_str().to_string())),
            None => (image.to_string(), None),
        },
    )
}

fn word_error(e: cua_image::ImageError) -> CuaError {
    match e {
        cua_image::ImageError::NotPublished { .. } => CuaError::ImageNotPublished(e.to_string()),
        other => CuaError::InvalidArgument(other.to_string()),
    }
}

/// Sandbox info as JSON.
pub fn info_json(i: &SandboxInfo) -> serde_json::Value {
    serde_json::json!({
        "id": i.id,
        "name": i.name,
        "status": row_status(i),
        "state": status_of(i),
        "location": i.location,
        "kind": i.kind,
        "runtime": i.runtime,
        "expires_at": i.expires_at_unix.map(|t| {
            humantime::format_rfc3339_seconds(
                std::time::UNIX_EPOCH + std::time::Duration::from_secs(t.max(0) as u64),
            )
            .to_string()
        }),
        "runtime_type": i.runtime_type,
        "ephemeral": i.ephemeral,
        "image": i.image,
        "services": i.services,
        // Service base URLs from this machine (cloud: gateway URLs that need
        // the Fleet bearer).
        "endpoints": i.endpoints,
        "provider_details": i.provider_details,
    })
}

/// The status a listing shows: the portable phase, or `missing` for a
/// record whose VM or container is gone.
fn row_status(i: &SandboxInfo) -> &'static str {
    if i.status_detail.as_deref() == Some(cua_sandbox_core::MISSING) {
        cua_sandbox_core::MISSING
    } else {
        phase_of(i)
    }
}

/// `-` for an empty cell.
fn or_dash(s: &str) -> String {
    if s.is_empty() { "-".into() } else { s.into() }
}

/// The portable status word (provisioning, starting, ready, stopped).
fn phase_of(i: &SandboxInfo) -> &'static str {
    match i.phase {
        cua_sdk::SandboxPhase::Provisioning => "provisioning",
        cua_sdk::SandboxPhase::Starting => "starting",
        cua_sdk::SandboxPhase::Ready => "ready",
        cua_sdk::SandboxPhase::Stopped => "stopped",
    }
}

/// `EXPIRES` cell: time left, or `-`.
fn expires_of(unix: Option<i64>) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    match unix {
        Some(t) => format!(
            "in {}",
            humantime::format_duration(std::time::Duration::from_secs((t - now).max(0) as u64))
        ),
        None => "-".into(),
    }
}

/// The portable status of a listing row's provider word.
fn portable_status(word: &str) -> &'static str {
    match word {
        "running" | "bound" | "ready" => "ready",
        "provisioning" | "pending" | "creating" => "provisioning",
        "starting" | "unknown" => "starting",
        "expired" => "expired",
        // A record whose VM or container is gone (`cua sb rm` clears it).
        "missing" => "missing",
        _ => "stopped",
    }
}

fn status_of(i: &SandboxInfo) -> String {
    i.status_detail
        .clone()
        .unwrap_or_else(|| format!("{:?}", i.status).to_lowercase())
}

fn print(out: &mut dyn Write, v: serde_json::Value, json: bool, text: impl FnOnce() -> String) {
    if json {
        line(out, v.to_string());
    } else {
        line(out, text());
    }
}

/// Where, what kind and which engine `sb create` / `sb launch` use: the
/// flags, else `--pool` means the cloud, else the user defaults
/// (`cua config`, `CUA_DEFAULT_*`), each with its source.
pub fn resolve_placement(a: &CreateArgs) -> Result<cua_sandbox_core::settings::Resolved, CuaError> {
    let on = a.on.clone().or_else(|| a.pool.as_ref().map(|_| On::Cloud));
    let r = settings()?
        .resolve(on, a.kind, a.runtime.clone())
        .map_err(|e| CuaError::InvalidArgument(e.0))?;
    cua_sandbox_core::placement::validate(&r.on, r.kind, &r.runtime)
        .map_err(|e| CuaError::InvalidPlacement(e.message))?;
    Ok(r)
}

/// Whether this command should run through a (possibly auto-started)
/// daemon: cloud creations, so the sandbox's keep-alive outlives the CLI.
pub fn wants_daemon(cmd: &SandboxCmd) -> bool {
    match cmd {
        SandboxCmd::Create(a) => resolve_placement(a).is_ok_and(|r| r.on == On::Cloud),
        SandboxCmd::Launch(a) => !a.local && a.on.as_ref().is_none_or(|o| *o == On::Cloud),
        _ => false,
    }
}

/// Prefixes an error with the backend it came from, keeping its kind.
pub fn on_backend(e: CuaError, backend: &str) -> CuaError {
    use CuaError as E;
    let f = |m: String| format!("[{backend}] {m}");
    match e {
        E::InvalidArgument(m) => E::InvalidArgument(f(m)),
        E::InvalidPlacement(m) => E::InvalidPlacement(f(m)),
        E::NotFound(m) => E::NotFound(f(m)),
        E::ProviderNotConfigured(m) => E::ProviderNotConfigured(f(m)),
        E::Unsupported(m) => E::Unsupported(f(m)),
        E::SpacesdNotAvailable(m) => E::SpacesdNotAvailable(f(m)),
        E::Timeout(m) => E::Timeout(f(m)),
        E::Fleet(m) => E::Fleet(f(m)),
        E::FleetAdmissionDenied(m) => E::FleetAdmissionDenied(f(m)),
        E::CloudCreditExhausted(m) => E::CloudCreditExhausted(f(m)),
        E::Runtime(m) => E::Runtime(f(m)),
        E::Env(m) => E::Env(f(m)),
        E::Http(m) => E::Http(f(m)),
        E::Unauthenticated(m) => E::Unauthenticated(f(m)),
        E::PermissionDenied(m) => E::PermissionDenied(f(m)),
        E::Transport(m) => E::Transport(f(m)),
        E::DaemonNotRunning(m) => E::DaemonNotRunning(f(m)),
        E::Closed(m) => E::Closed(f(m)),
        E::CapabilityMissing(m) => E::CapabilityMissing(f(m)),
        E::HostCapabilityMissing(m) => E::HostCapabilityMissing(f(m)),
        E::TeleportRefused(m) => E::TeleportRefused(f(m)),
        E::PoolSpecMismatch(m) => E::PoolSpecMismatch(f(m)),
        E::ClaimSecretsNotDelivered(m) => E::ClaimSecretsNotDelivered(f(m)),
        E::Internal(m) => E::Internal(f(m)),
        E::AmbiguousSandbox(m) => E::AmbiguousSandbox(f(m)),
        E::InsufficientDisk(m) => E::InsufficientDisk(f(m)),
        E::ImageNotPublished(m) => E::ImageNotPublished(f(m)),
        E::Cloud(m) => E::Cloud(f(m)),
        E::Cancelled(m) => E::Cancelled(f(m)),
    }
}

/// What a failed create kept: the sandbox ref, its cloud claim (pool and
/// namespace) when it has one, and how to delete it.
fn kept_message(i: &SandboxInfo) -> String {
    let d = |k: &str| i.provider_details.get(k).filter(|v| !v.is_empty());
    let mut claim = String::new();
    if let Some(c) = d("claim").or(i.location.eq("cloud").then_some(&i.name)) {
        claim.push_str(&format!(" (claim {c}"));
        if let Some(p) = d("pool") {
            claim.push_str(&format!(", pool {p}"));
        }
        if let Some(n) = d("namespace").filter(|n| Some(*n) != d("pool")) {
            claim.push_str(&format!(", namespace {n}"));
        }
        claim.push(')');
    }
    format!(
        "kept sandbox {}{claim} after the failure (--keep-on-failure); delete it with `cua sb rm {} --force`",
        i.id, i.id
    )
}

/// A create that succeeded but failed a later readiness step (overlays,
/// `--wait desktop`, `--browser`): deletes the sandbox (releasing its
/// claim) unless `keep`, and returns `e`.
async fn failed_after_create(sb: &Arc<cua_sdk::Sandbox>, keep: bool, e: CuaError) -> CuaError {
    if keep {
        eprintln!("{}", kept_message(&sb.info()));
    } else if let Err(d) = sb.delete().await {
        eprintln!(
            "warning: could not delete sandbox {} after the failure: {d} (delete it with `cua sb rm {} --force`)",
            sb.id(),
            sb.id()
        );
    }
    e
}

/// A generated sandbox name (`sb-<8 hex>`).
fn generated_name() -> String {
    format!("sb-{:08x}", rand::random::<u32>())
}

/// `sb create` (and `sb launch`, `compat` keeps its output).
async fn create(
    cua: &Arc<Cua>,
    a: CreateArgs,
    compat: bool,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let placement = resolve_placement(&a)?;
    let on = placement.on.clone();
    let label = on.to_string();
    let mut o = SandboxCreateOptions::auto("");
    o.on = Some(label.clone());
    o.kind = Some(placement.kind.to_string());
    o.runtime = Some(placement.runtime.to_string());
    o.name = a.name.clone();
    if let On::Direct(_) = &on {
        if a.pool.is_some() || a.image.is_some() || a.image_flag.is_some() {
            return Err(CuaError::InvalidArgument(format!(
                "--on {label} connects to an existing machine: IMAGE and --pool do not apply"
            )));
        }
        o.token = a.token.clone();
    } else {
        let image = a
            .image
            .clone()
            .or(a.image_flag.clone())
            .or_else(|| (a.browser && a.pool.is_none()).then(|| "linux".to_string()));
        if image.is_none() && a.pool.is_none() {
            return Err(CuaError::InvalidArgument(format!(
                "IMAGE is required for --on {label}"
            )));
        }
        // Locally, --pool (without an image) runs that cloud pool's
        // template: its image, firmware, services and readiness probe.
        if a.pool.is_some() && !matches!(on, On::Cloud | On::Local) {
            return Err(CuaError::InvalidArgument(
                "--pool is a cloud pool: use it (--on cloud) or run its template locally \
                 (--on local)"
                    .into(),
            ));
        }
        if a.pool.is_some() && on == On::Local && image.is_some() {
            return Err(CuaError::InvalidArgument(
                "pass IMAGE or --pool, not both (locally --pool runs that pool's template)".into(),
            ));
        }
        if let Some(image) = image {
            let (resolved, os) = resolve_image(&image, a.tier.as_deref())?;
            o.image = resolved;
            o.os = os;
        }
        if o.name.is_none() {
            o.name = Some(generated_name());
        }
    }
    o.pool = a.pool.clone();
    o.cpus = a.cpu;
    o.memory_mb = a.memory.as_deref().map(parse_memory).transpose()?;
    o.network = a.network.clone();
    if a.disk.is_some() {
        eprintln!("note: --disk is ignored (the image decides the disk size)");
    }
    for (name, port) in &a.ports {
        match name {
            Some(n) => {
                o.services.insert(n.clone(), *port);
            }
            None => o.ports.push(*port),
        }
    }
    let mut waits: Vec<(WaitTarget, Option<String>)> = a.wait.clone();
    waits.extend(a.wait_tcp.iter().map(|p| (WaitTarget::Port(*p), None)));
    waits.extend(
        a.wait_http
            .iter()
            .map(|(p, path)| (WaitTarget::Port(*p), Some(path.clone()))),
    );
    let wait_desktop = waits.iter().any(|(t, _)| *t == WaitTarget::Desktop) || a.browser;
    if a.browser
        && !waits
            .iter()
            .any(|(t, _)| *t == WaitTarget::Port(cua_proto::SPACESD_DEFAULT_PORT))
    {
        // The browser preset drives the image's cua-spacesd.
        waits.push((WaitTarget::Port(cua_proto::SPACESD_DEFAULT_PORT), None));
    }
    o.wait_for = waits
        .into_iter()
        .filter_map(|(target, http_path)| target.probe(http_path))
        .collect();
    o.command = (!a.command.is_empty()).then(|| a.command.clone());
    o.ready_timeout_ms = a.ready_timeout.map(|s| s.saturating_mul(1000));
    o.env = a.env.iter().cloned().collect();
    o.warm = if a.no_warm {
        Some(false)
    } else {
        a.warm.then_some(true)
    };
    o.sidecars = a.sidecars.clone();
    o.registry_secret = a.registry_secret.clone();
    o.max_pool_size = a.max_pool_size;
    o.fleet_ttl_seconds = a.claim_ttl;
    o.keep_on_failure = a.keep_on_failure;
    o.gpu = a.gpu.clone();
    if a.apply {
        o.cloud = Some(cua_sdk::CloudOptions {
            apply: true,
            ..Default::default()
        });
    }
    let token = o.token.clone();
    if on == On::Cloud {
        eprintln!("Starting a cloud sandbox (first start of an image can take a few minutes)");
    }
    let requested_name = o.name.clone();
    let created = {
        let sandboxes = cua.sandboxes();
        let mut create = Box::pin(sandboxes.create(o));
        tokio::select! {
            r = &mut create => r,
            _ = tokio::signal::ctrl_c() => {
                // Ctrl-C: stop the create and delete what it made.
                eprintln!("\nCancelling...");
                let what = match &requested_name {
                    Some(n) => cua.sandboxes().cancel_create(n.clone()).await?,
                    None => None,
                };
                // It ends with `Cancelled` once cleaned up.
                let _ = tokio::time::timeout(std::time::Duration::from_secs(120), create).await;
                return Err(CuaError::Cancelled(
                    what.unwrap_or_else(|| "the create stopped".into()),
                ));
            }
        }
    };
    crate::util::exit_on_ctrl_c();
    if created.is_err()
        && a.keep_on_failure
        && let Some(n) = &requested_name
        && let Ok(info) = cua.sandboxes().get(n.clone()).await
    {
        eprintln!("{}", kept_message(&info));
    }
    let sb = created
        .map_err(|e| match e {
            // The cloud came from a default: say how to sign in or go back.
            CuaError::ProviderNotConfigured(m) => CuaError::ProviderNotConfigured(
                match cua_sandbox_core::settings::cloud_default_hint(&placement.on_source) {
                    Some(hint) => format!("{m}; {hint}"),
                    None => m,
                },
            ),
            other => other,
        })
        .map_err(|e| on_backend(e, &label))?;
    let i = sb.info();
    if i.location == "direct" {
        save_env_token(&i.name, token.as_deref())?;
        save_env_token(&i.id, token.as_deref())?;
    }
    let overlays = if a.overlays.is_empty() {
        None
    } else {
        let results = sb
            .clone()
            .overlay(
                a.overlays.clone(),
                a.ready_timeout.map(|s| s.saturating_mul(1000)),
            )
            .await
            .map_err(|e| {
                CuaError::Runtime(format!(
                    "sandbox {} was created, but injecting the overlays failed: {e}",
                    i.id
                ))
            });
        let results = match results {
            Ok(r) => r,
            Err(e) => return Err(failed_after_create(&sb, a.keep_on_failure, e).await),
        };
        Some(report_overlays(&results))
    };
    if wait_desktop {
        let budget = std::time::Duration::from_secs(u64::from(
            a.ready_timeout.unwrap_or(DESKTOP_READY_TIMEOUT_SECS),
        ));
        // A container reports ready before its cua-spacesd listens: keep
        // probing within the same budget instead of failing on the first try.
        let started = std::time::Instant::now();
        let desktop = async {
            let env = loop {
                match sb.spacesd(None).await {
                    Ok(env) => break env,
                    Err(CuaError::SpacesdNotAvailable(_)) if started.elapsed() < budget => {
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    }
                    Err(e) => return Err(e),
                }
            };
            crate::computer::Computer::new(env)
                .ensure_desktop(budget)
                .await
        }
        .await;
        // `--wait desktop` is readiness too: a failure releases the sandbox.
        if let Err(e) = desktop {
            return Err(failed_after_create(&sb, a.keep_on_failure, e).await);
        }
    }
    let browser = if a.browser {
        let opened = async {
            crate::browse::register_space(cua, &i.id).await?;
            crate::browse::start(
                cua,
                &i.id,
                a.open.as_deref(),
                crate::browse::DEFAULT_SESSION,
            )
            .await
        }
        .await
        .map_err(|e| {
            CuaError::Runtime(format!(
                "sandbox {} was created, but its browser did not start: {e}",
                i.id
            ))
        });
        match opened {
            Ok(o) => Some(o),
            Err(e) => return Err(failed_after_create(&sb, a.keep_on_failure, e).await),
        }
    } else {
        None
    };
    if !a.install.is_empty() {
        eprintln!("installing {} ...", a.install.join(", "));
        for p in sb.agents().await?.ensure(a.install.clone()).await? {
            eprintln!("  {p}");
        }
    }
    if on == On::Cloud && a.pool.is_none() && cua.mode() == cua_sdk::CuaMode::Embedded {
        let ttl = a.claim_ttl.unwrap_or_else(|| {
            cua_sandbox_core::settings::auto_pool_config()
                .claim_ttl
                .as_secs() as u32
        });
        eprintln!(
            "note: no cua daemon holds this sandbox, so it expires {} from now unless you run \
             `cua sb keep-alive {}` (or start `cua daemon start` and `cua sb connect {}`)",
            humantime::format_duration(std::time::Duration::from_secs(u64::from(ttl))),
            i.id,
            i.id
        );
    }
    if compat {
        print(
            out,
            serde_json::json!({"name": i.name, "status": "ready"}),
            json,
            || format!("Sandbox '{}' is ready", i.name),
        );
    } else {
        let mut v = info_json(&i);
        if let Some(overlays) = overlays {
            v["overlays"] = overlays;
        }
        if let Some(b) = &browser {
            v["browser"] = b["browser"].clone();
            v["space"] = b["space"].clone();
            v["session"] = b["session"].clone();
            v["url"] = b["url"].clone();
        }
        if json {
            line(out, v.to_string());
        } else {
            util::table(
                out,
                &["ID", "STATUS", "LOCATION", "KIND", "RUNTIME", "EXPIRES"],
                &[vec![
                    i.id.clone(),
                    phase_of(&i).into(),
                    i.location.clone(),
                    or_dash(&i.kind),
                    or_dash(&i.runtime),
                    expires_of(i.expires_at_unix),
                ]],
            );
            if let Some(b) = &browser {
                let br = &b["browser"];
                line(
                    out,
                    format!(
                        "\nBrowser ready ({}): session {}, target_id {}, tab_id {}\n\
                         Drive it from an agent with the cua MCP server: call_tool {{\"space\": \"{}\", \"tool\": \"browser_navigate\", ...}}\n\
                         (see the cua-sandboxes skill), watch it with `cua sb view {}`.",
                        b["url"].as_str().unwrap_or("about:blank"),
                        b["session"].as_str().unwrap_or_default(),
                        br["target_id"].as_str().unwrap_or_default(),
                        br["tab_id"].as_str().unwrap_or_default(),
                        i.id,
                        i.id,
                    ),
                );
            }
        }
    }
    if a.view {
        let link = viewer_link(cua, &i.id, None, false, None).await?;
        eprintln!("Viewer: {}", link.url);
        util::open_browser(&link.url);
    }
    Ok(0)
}

/// Runs a sandbox subcommand.
pub async fn run(
    cua: &Arc<Cua>,
    cmd: SandboxCmd,
    json_global: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let sbx = cua.sandboxes();
    match cmd {
        SandboxCmd::Create(a) => return create(cua, *a, false, json_global, out).await,
        SandboxCmd::Launch(a) => {
            return create(cua, a.into_create()?, true, json_global, out).await;
        }
        SandboxCmd::Connect { name, at } => {
            let i = sbx.connect(at.apply(&name)?).await?.info();
            print(out, info_json(&i), json_global, || {
                format!("{} ({}, {})", i.name, i.location, status_of(&i))
            });
        }
        SandboxCmd::Ls {
            local,
            cloud,
            all,
            no_size,
        } => {
            let _ = all; // deprecated no-op: everything is the default
            return ls(cua, local, cloud, !no_size, json_global, out).await;
        }
        SandboxCmd::KeepAlive { name, at, duration } => {
            let name = at.apply(&name)?;
            let sb = sbx.connect(name.clone()).await?;
            sb.keep_alive(duration).await?;
            let until =
                std::time::SystemTime::now() + std::time::Duration::from_secs(u64::from(duration));
            let until = humantime::format_rfc3339_seconds(until).to_string();
            print(
                out,
                serde_json::json!({"name": name, "expires_at": until}),
                json_global,
                || format!("Sandbox '{name}' is kept alive until {until}"),
            );
        }
        SandboxCmd::Cp { src, dst, at } => return cp(cua, &src, &dst, at, json_global, out).await,
        SandboxCmd::Url {
            name,
            service,
            at,
            public,
            ttl,
        } => {
            let name = at.apply(&name)?;
            let sb = sbx.connect(name.clone()).await?;
            if public {
                let u = sb.public_url(service.clone(), ttl, None).await?;
                let expires = humantime::format_rfc3339_seconds(
                    std::time::UNIX_EPOCH
                        + std::time::Duration::from_secs(u.expires_at_unix.max(0) as u64),
                )
                .to_string();
                print(
                    out,
                    serde_json::json!({"name": name, "service": service, "url": u.url,
                        "id": u.id, "expires_at": expires}),
                    json_global,
                    || format!("{} (expires {expires})", u.url),
                );
            } else {
                let url = sb.service(service.clone())?.url().await?;
                print(
                    out,
                    serde_json::json!({"name": name, "service": service, "url": url}),
                    json_global,
                    || url.clone(),
                );
            }
        }
        SandboxCmd::Logs {
            name,
            at,
            tail,
            source,
        } => {
            return logs(cua, &at.apply(&name)?, tail, &source, out).await;
        }
        SandboxCmd::PortForward {
            name,
            at,
            spec,
            no_wait,
        } => {
            let name = at.apply(&name)?;
            return port_forward(cua, &name, &spec, no_wait, json_global, out).await;
        }
        SandboxCmd::Info { name, at } => {
            let name = at.apply(&name)?;
            let json = json_global;
            match sbx.get(name.clone()).await {
                Ok(i) => {
                    if json {
                        line(out, info_json(&i).to_string());
                    } else {
                        line(out, format!("Name:     {}", i.name));
                        line(out, format!("Id:       {}", i.id));
                        line(out, format!("Status:   {}", row_status(&i)));
                        line(out, format!("Location: {}", i.location));
                        line(out, format!("Expires:  {}", expires_of(i.expires_at_unix)));
                        if let Some(img) = &i.image {
                            line(out, format!("Image:    {img}"));
                        }
                        let mut s: Vec<_> = i.services.iter().collect();
                        s.sort();
                        for (k, p) in s {
                            line(out, format!("Service:  {k} ({p})"));
                        }
                    }
                }
                Err(CuaError::NotFound(missing)) => {
                    // Not a sandbox: maybe a cloud pool. A name that is
                    // neither stays "not found" (a local name must not
                    // surface a Fleet error, e.g. without cloud access).
                    let p = match fleet_pool(&name).await {
                        Ok(p) => p,
                        Err(_) => return Err(CuaError::NotFound(missing)),
                    };
                    if json {
                        line(out, p.to_string());
                    } else {
                        line(out, format!("Name:     {name} (dedicated cloud capacity)"));
                        line(
                            out,
                            format!(
                                "Replicas: {} ready / {} desired",
                                p["status"]["readyReplicas"].as_u64().unwrap_or(0),
                                p["spec"]["replicas"].as_u64().unwrap_or(0)
                            ),
                        );
                    }
                }
                Err(e) => return Err(e),
            }
        }
        SandboxCmd::Suspend { name, at } => {
            let name = at.apply(&name)?;
            lifecycle(cua, &name, "suspend").await?;
            line(out, format!("Sandbox '{name}' is suspending."));
        }
        SandboxCmd::Resume { name, at } => {
            let name = at.apply(&name)?;
            lifecycle(cua, &name, "resume").await?;
            line(out, format!("Sandbox '{name}' is resuming."));
        }
        SandboxCmd::Restart { name, at } => {
            let name = at.apply(&name)?;
            lifecycle(cua, &name, "restart").await?;
            line(out, format!("Sandbox '{name}' is restarting."));
        }
        SandboxCmd::Rm { name, force, at } => {
            let name = at.apply(&name)?;
            use std::io::IsTerminal as _;
            match confirm_delete(force, std::io::stdin().is_terminal(), || {
                util::confirm(&format!("Delete sandbox '{name}'?"), false)
            }) {
                DeleteConfirmation::Proceed => {}
                DeleteConfirmation::Declined => {
                    line(out, "Aborted.");
                    return Ok(0);
                }
                DeleteConfirmation::NeedsForce => {
                    return Err(needs_force(&format!("delete sandbox '{name}'")));
                }
            }
            // A record whose VM or container is already gone still deletes;
            // say so rather than claim a teardown.
            let info = sbx.get(name.clone()).await.ok();
            let missing = info
                .as_ref()
                .is_some_and(|i| i.status_detail.as_deref() == Some(cua_sandbox_core::MISSING));
            sbx.delete(name.clone()).await?;
            // A direct sandbox's remembered token (saved under its name and
            // its `direct:<host:port>` ref at create) goes with it.
            if let Some(i) = info.filter(|i| i.location == "direct") {
                save_env_token(&i.name, None)?;
                save_env_token(&i.id, None)?;
            }
            print(
                out,
                serde_json::json!({"deleted": name, "missing": missing}),
                json_global,
                || {
                    if missing {
                        format!(
                            "Sandbox '{name}' was already gone (no VM or container); removed its record."
                        )
                    } else {
                        format!("Sandbox '{name}' is being deleted.")
                    }
                },
            );
        }
        SandboxCmd::Overlay {
            name,
            overlays,
            at,
            timeout,
        } => {
            let sb = sandbox_with_token(cua, &at.apply(&name)?).await?;
            let results = sb
                .overlay(overlays, Some(timeout.saturating_mul(1000)))
                .await?;
            let v = report_overlays(&results);
            if json_global {
                line(
                    out,
                    serde_json::json!({ "name": name, "overlays": v }).to_string(),
                );
            } else {
                for r in &results {
                    line(out, format!("{}=sha256:{}", r.name, r.sha256));
                }
            }
            return Ok(0);
        }
        SandboxCmd::Exec { name, command, at } => {
            let name = at.apply(&name)?;
            let env = match env_of(cua, &name).await {
                Err(CuaError::SpacesdNotAvailable(m)) => match agentless(cua, &name).await? {
                    Some(sb) => {
                        return exec_agentless(&sb, &name, &command.join(" "), json_global, out)
                            .await;
                    }
                    None => return Err(CuaError::SpacesdNotAvailable(m)),
                },
                other => other?,
            };
            let o = env.sh(command.join(" "), Some(120_000)).await?;
            let code = exit_code(&o);
            if json_global {
                line(
                    out,
                    serde_json::json!({
                        "stdout": String::from_utf8_lossy(&o.stdout),
                        "stderr": String::from_utf8_lossy(&o.stderr),
                        "returncode": code,
                    })
                    .to_string(),
                );
            } else {
                out.write_all(&o.stdout).ok();
                std::io::stderr().write_all(&o.stderr).ok();
            }
            return Ok(code);
        }
        SandboxCmd::Shell {
            name,
            command,
            cols,
            rows,
            at,
        } => {
            let env = env_of(cua, &at.apply(&name)?).await?;
            if !command.is_empty() && !util::interactive() {
                return shell::exec(&env, &sh_argv(&command.join(" ")), out).await;
            }
            let cmd = (!command.is_empty()).then(|| sh_argv(&command.join(" ")));
            return shell::interactive(&env, cmd, cols, rows).await;
        }
        SandboxCmd::Screenshot { name, at, output } => {
            let name = at.apply(&name)?;
            let (s, via) = match env_of(cua, &name).await {
                Ok(env) => (env.screenshot(None).await?, None),
                Err(CuaError::SpacesdNotAvailable(m)) => match agentless(cua, &name).await? {
                    Some(sb) => (sb.guest_screenshot().await?, Some("vnc")),
                    None => return Err(CuaError::SpacesdNotAvailable(m)),
                },
                Err(e) => return Err(e),
            };
            std::fs::write(&output, &s.image).map_err(internal)?;
            let mut v = serde_json::json!({"path": output, "width": s.width, "height": s.height});
            if let Some(via) = via {
                v["via"] = via.into();
            }
            print(out, v, json_global, || {
                format!(
                    "wrote {} ({}x{}){}",
                    output.display(),
                    s.width,
                    s.height,
                    if via.is_some() { " via VNC" } else { "" }
                )
            });
        }
        SandboxCmd::Mcp {
            name,
            service,
            at,
            path,
            action,
        } => {
            let name = at.apply(&name)?;
            return mcp(cua, &name, &service, &path, action, json_global, out).await;
        }
        SandboxCmd::View(v) => return view(cua, v, json_global, out).await,
        SandboxCmd::Vnc(v) => {
            eprintln!("warning: `cua sb vnc` is deprecated; use `cua sb view` (the HTML5 viewer).");
            return view(cua, v, json_global, out).await;
        }
        SandboxCmd::StreamProbe { args, at } => {
            let env = env_of(cua, &at.apply(&args.name)?).await?;
            let report = crate::stream_probe::run(env, &args).await?;
            line(out, report.to_string());
        }
    }
    Ok(0)
}

// ------------------------------------------------------ direct env tokens

fn tokens_path() -> PathBuf {
    util::cua_home().join("env-tokens.json")
}

fn load_tokens() -> serde_json::Map<String, serde_json::Value> {
    std::fs::read_to_string(tokens_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Remembers the spacesd token of a named direct sandbox (the SDK keeps
/// it in memory only; each CLI invocation is a new process). Stored 0600 in
/// `~/.cua/env-tokens.json`.
pub fn save_env_token(name: &str, token: Option<&str>) -> Result<(), CuaError> {
    let mut t = load_tokens();
    match token {
        Some(v) => {
            t.insert(name.into(), serde_json::Value::String(v.into()));
        }
        None => {
            if t.remove(name).is_none() {
                return Ok(());
            }
        }
    }
    util::write_private(
        &tokens_path(),
        serde_json::Value::Object(t).to_string().as_bytes(),
    )
}

/// A spacesd client for a named sandbox. Direct sandboxes use the
/// remembered token (or `CUA_ENV_TOKEN`).
pub async fn env_of(cua: &Cua, name: &str) -> Result<Arc<cua_sdk::SpacesdClient>, CuaError> {
    let sbx = cua.sandboxes();
    let info = sbx.get(name.to_string()).await?;
    if info.location == "direct"
        && let Some(url) = info.endpoints.get("env")
    {
        return cua.spacesd(url.clone(), direct_token(&info)).await;
    }
    sbx.connect(info.id.clone()).await?.spacesd(None).await
}

/// The remembered spacesd token of a direct sandbox (saved under its name
/// and its `direct:<host:port>` ref), or `CUA_ENV_TOKEN`.
fn direct_token(info: &SandboxInfo) -> Option<String> {
    let tokens = load_tokens();
    [&info.id, &info.name]
        .iter()
        .find_map(|k| tokens.get(k.as_str()).and_then(|v| v.as_str()))
        .map(str::to_string)
        .or_else(|| {
            std::env::var("CUA_ENV_TOKEN")
                .ok()
                .filter(|t| !t.is_empty())
        })
}

/// A handle for `name` that carries its spacesd token: direct sandboxes
/// keep the token outside the SDK state, so they are reattached by URL.
pub(crate) async fn sandbox_with_token(
    cua: &Arc<Cua>,
    name: &str,
) -> Result<Arc<cua_sdk::Sandbox>, CuaError> {
    let sbx = cua.sandboxes();
    let info = sbx.get(name.to_string()).await?;
    if info.location == "direct"
        && let Some(url) = info.endpoints.get("env")
    {
        return sbx
            .connect_url(url.clone(), direct_token(&info), Some(info.name.clone()))
            .await;
    }
    sbx.connect(info.id).await
}

fn sh_argv(line: &str) -> Vec<String> {
    vec!["/bin/sh".into(), "-c".into(), line.into()]
}

async fn fleet_pool(name: &str) -> Result<serde_json::Value, CuaError> {
    let (c, _) = auth::fleet_client()
        .await
        .map_err(|_| CuaError::NotFound(format!("no sandbox named {name}")))?;
    let h = c.get_pool(name).await.map_err(auth::fleet_err)?;
    Ok(serde_json::to_value(&h.pool)?)
}

/// suspend / resume / restart by sandbox name, falling back to a Fleet
/// pool of that name (as the former CLI did).
async fn lifecycle(cua: &Arc<Cua>, name: &str, op: &str) -> Result<(), CuaError> {
    match cua.sandboxes().by_name(name.to_string()).await {
        Ok(sb) => match op {
            "suspend" => sb.suspend().await,
            "resume" => sb.resume().await,
            _ => sb.restart().await,
        },
        Err(CuaError::NotFound(m)) => {
            let Ok((c, _)) = auth::fleet_client().await else {
                return Err(CuaError::NotFound(m));
            };
            let mut h = c
                .get_pool(name)
                .await
                .map_err(|e| match auth::fleet_err(e) {
                    CuaError::NotFound(_) => CuaError::NotFound(m.clone()),
                    e => e,
                })?;
            let reps: &[u32] = match op {
                "suspend" => &[0],
                "resume" => &[1],
                _ => &[0, 1],
            };
            for r in reps {
                c.set_pool_replicas(&mut h, *r)
                    .await
                    .map_err(auth::fleet_err)?;
            }
            Ok(())
        }
        Err(e) => Err(e),
    }
}

/// `--state-dir`, when given (set once by main).
pub static STATE_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

fn state_dir() -> PathBuf {
    STATE_DIR
        .get()
        .cloned()
        .unwrap_or_else(|| util::cua_home().join("sandboxes"))
}

/// The raw state file of a named sandbox, if any.
fn state_of(name: &str) -> Option<serde_json::Value> {
    if name.is_empty() || name.contains(['/', '\\']) || name.starts_with('.') {
        return None;
    }
    std::fs::read(state_dir().join(format!("{name}.json")))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
}

/// Backend label of a listing row (`--on` vocabulary).
/// A listing row in portable words: `status` (provisioning, starting,
/// ready, stopped, expired) with the provider's word in `state`, `location`
/// (local, cloud, direct), and cloud internals moved into
/// `provider_details`.
fn portable_row(r: &mut serde_json::Value) {
    let word = r["status"].as_str().unwrap_or("unknown").to_string();
    if r.get("state").is_none() {
        r["state"] = serde_json::Value::String(word.clone());
        r["status"] = serde_json::Value::String(portable_status(&word).into());
    }
    if r["location"].as_str().is_none_or(str::is_empty) {
        r["location"] = serde_json::Value::String("local".into());
    }
    let Some(obj) = r.as_object_mut() else {
        return;
    };
    let mut details = obj
        .remove("provider_details")
        .and_then(|d| d.as_object().cloned())
        .unwrap_or_default();
    for k in ["pool", "namespace", "managed"] {
        if let Some(v) = obj.remove(k) {
            details.insert(k.into(), v);
        }
    }
    obj.insert(
        "provider_details".into(),
        serde_json::Value::Object(details),
    );
}

/// Adds `pool`, `managed` and `expires_in_seconds` to Fleet rows (live
/// claim `shutdownTime`). Best effort.
async fn annotate_fleet(rows: &mut [serde_json::Value]) {
    let fleet_rows: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, r)| r["location"] == "cloud")
        .map(|(i, _)| i)
        .collect();
    if fleet_rows.is_empty() {
        return;
    }
    let mut by_pool: std::collections::BTreeMap<String, Vec<usize>> = Default::default();
    for i in fleet_rows {
        let name = rows[i]["name"].as_str().unwrap_or_default().to_string();
        let pool = rows[i]["namespace"]
            .as_str()
            .map(str::to_string)
            .or_else(|| state_of(&name).and_then(|s| s["pool_name"].as_str().map(str::to_string)));
        if let Some(p) = pool {
            rows[i]["pool"] = serde_json::Value::String(p.clone());
            rows[i]["managed"] = serde_json::Value::Bool(p.starts_with("cua-auto-"));
            by_pool.entry(p).or_default().push(i);
        }
    }
    let Ok((c, _)) = auth::fleet_client().await else {
        return;
    };
    let now = std::time::SystemTime::now();
    for (pool, idx) in by_pool {
        let Ok(claims) = c.list_claims(&pool).await else {
            continue;
        };
        // The pool's runtime says the kind and engine of its sandboxes.
        let runtime = match c.get_pool(&pool).await {
            Ok(p) => c.pool_runtime(&p.pool).await,
            Err(_) => None,
        };
        let placement = match runtime {
            Some(cua_fleet::RuntimeKind::Gvisor) => Some(("container", "gvisor")),
            Some(cua_fleet::RuntimeKind::Kubevirt) => Some(("vm", "kubevirt")),
            _ => None,
        };
        for i in idx {
            if let Some((kind, runtime)) = placement {
                if rows[i]["kind"].as_str().is_none_or(str::is_empty) {
                    rows[i]["kind"] = serde_json::Value::String(kind.into());
                }
                if rows[i]["runtime"].as_str().is_none_or(str::is_empty) {
                    rows[i]["runtime"] = serde_json::Value::String(runtime.into());
                }
            }
            let name = rows[i]["name"].as_str().unwrap_or_default().to_string();
            let Some(cl) = claims.iter().find(|c| c.metadata.name == name) else {
                let suspended = rows[i]["state"] == "suspended" || rows[i]["status"] == "suspended";
                if !suspended {
                    rows[i]["state"] = serde_json::Value::String("expired".into());
                    rows[i]["status"] = serde_json::Value::String("expired".into());
                }
                continue;
            };
            if let Some(t) = cl
                .spec
                .lifecycle
                .as_ref()
                .and_then(|l| l.shutdown_time.as_deref())
                .and_then(|t| humantime::parse_rfc3339_weak(t).ok())
            {
                let left = t.duration_since(now).map(|d| d.as_secs()).unwrap_or(0);
                rows[i]["expires_in_seconds"] = serde_json::Value::from(left);
            }
        }
    }
}

/// Every sandbox this machine knows, of every provider (local, direct and
/// the Fleet claims it holds); no live Fleet listing (`cua do ls`).
pub async fn list_known(cua: &Arc<Cua>) -> Result<Vec<SandboxInfo>, CuaError> {
    cua.sandboxes().list_known(None).await
}

async fn ls(
    cua: &Arc<Cua>,
    local: bool,
    cloud: bool,
    sizes: bool,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    // Known sandboxes of the filter; live cloud sandboxes unless local only.
    let known = if cloud {
        Some("cloud")
    } else if local {
        Some("local")
    } else {
        None
    };
    let mut rows: Vec<serde_json::Value> = cua
        .sandboxes()
        .list_known(known)
        .await?
        .iter()
        .map(info_json)
        .collect();
    let mut warnings: Vec<String> = vec![];
    // The default listing never reads the OS credential vault to find out
    // whether a session exists (the session marker says so).
    if cloud || (known.is_none() && auth::fleet_maybe_configured()) {
        let live = tokio::time::timeout(cua_daemon::LIST_CLOUD_TIMEOUT, fleet_claims()).await;
        match live {
            Ok(Ok(claims)) => {
                // Known Fleet sandboxes already listed stay once.
                for c in claims {
                    if !rows.iter().any(|r| r["id"] == c["id"]) {
                        rows.push(c);
                    }
                }
            }
            // No Fleet credentials: nothing to list in the cloud.
            Ok(Err(CuaError::ProviderNotConfigured(_))) if !cloud => {}
            Ok(Err(e)) => warnings.push(format!("cloud sandboxes not listed: {e}")),
            Err(_) => warnings.push(format!(
                "cloud sandboxes not listed: Fleet did not answer within {}s",
                cua_daemon::LIST_CLOUD_TIMEOUT.as_secs()
            )),
        }
    }
    if warnings.is_empty() {
        // Remaining TTLs of cloud rows (bounded like the listing).
        let _ =
            tokio::time::timeout(cua_daemon::LIST_CLOUD_TIMEOUT, annotate_fleet(&mut rows)).await;
    }
    for r in rows.iter_mut() {
        portable_row(r);
    }
    // Disk used by each local sandbox (cloud and direct ones use none here).
    if sizes && rows.iter().any(|r| r["location"] == "local") {
        let by_name = cua_disk::Scanner::system(cua_disk::Layout::default())
            .await
            .sandbox_sizes()
            .await;
        for r in rows.iter_mut().filter(|r| r["location"] == "local") {
            if let Some(b) = r["name"].as_str().and_then(|n| by_name.get(n)) {
                r["size_bytes"] = serde_json::json!(b);
            }
        }
    }
    for w in &warnings {
        eprintln!("warning: {w}");
    }
    if json {
        line(out, serde_json::Value::Array(rows).to_string());
        return Ok(0);
    }
    if rows.is_empty() {
        line(out, "No sandboxes found.");
    } else {
        let s = |v: &serde_json::Value| v.as_str().unwrap_or_default().to_string();
        let dash = |v: &serde_json::Value| {
            v.as_str()
                .filter(|v| !v.is_empty())
                .unwrap_or("-")
                .to_string()
        };
        let table: Vec<Vec<String>> = rows
            .iter()
            .map(|r| {
                let mut row = vec![
                    s(&r["id"]),
                    s(&r["status"]),
                    s(&r["location"]),
                    dash(&r["kind"]),
                    dash(&r["runtime"]),
                    r["expires_in_seconds"]
                        .as_u64()
                        .map(|t| {
                            format!(
                                "in {}",
                                humantime::format_duration(std::time::Duration::from_secs(t))
                            )
                        })
                        .unwrap_or_else(|| "-".into()),
                ];
                if sizes {
                    row.push(
                        r["size_bytes"]
                            .as_u64()
                            .map(cua_disk::format_size)
                            .unwrap_or_else(|| "-".into()),
                    );
                }
                row
            })
            .collect();
        let mut headers = vec!["ID", "STATUS", "LOCATION", "KIND", "RUNTIME", "EXPIRES"];
        if sizes {
            headers.push("SIZE");
        }
        util::table(out, &headers, &table);
    }
    Ok(0)
}

/// `NAME:/path` -> (name, path); local paths (`./x`, `/x`, `C:\x`) -> None.
/// NAME may be a ref (`local:box:/tmp/x`, `direct:10.0.0.5:3211:/tmp/x`):
/// the split is at the first `:` followed by a guest path.
fn remote(spec: &str) -> Option<(&str, &str)> {
    let guest_path = |p: &str| {
        let b = p.as_bytes();
        p.starts_with('/')
            || p.starts_with('~')
            || (b.len() >= 3
                && b[0].is_ascii_alphabetic()
                && b[1] == b':'
                && matches!(b[2], b'\\' | b'/'))
    };
    let plausible = |n: &str| {
        n.len() >= 2
            && !n.contains(['/', '\\'])
            && (!n.contains('.') || n.contains(':'))
            && cua_sdk::parse_sandbox_ref(n.to_string()).is_ok()
    };
    for (i, _) in spec.match_indices(':') {
        let (name, path) = (&spec[..i], &spec[i + 1..]);
        if guest_path(path) && plausible(name) {
            return Some((name, path));
        }
    }
    let (name, path) = spec.split_once(':')?;
    (plausible(name) && !name.contains(':') && !path.is_empty()).then_some((name, path))
}

/// `sb cp`.
async fn cp(
    cua: &Arc<Cua>,
    src: &str,
    dst: &str,
    at: Narrow,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let (r, direction) = match (remote(src), remote(dst)) {
        (Some(r), None) => (r, "download"),
        (None, Some(r)) => (r, "upload"),
        (Some(_), Some(_)) => {
            return Err(CuaError::InvalidArgument(
                "copy between two sandboxes is not supported; copy through a local file".into(),
            ));
        }
        (None, None) => {
            return Err(CuaError::InvalidArgument(
                "one side must be NAME:/path in a sandbox".into(),
            ));
        }
    };
    let (name, guest) = r;
    let env = env_of(cua, &at.apply(name)?).await?;
    let t = if direction == "download" {
        let mut local = PathBuf::from(dst);
        if local.is_dir() {
            local = local.join(
                std::path::Path::new(guest)
                    .file_name()
                    .unwrap_or_else(|| std::ffi::OsStr::new("download")),
            );
        }
        env.download_file(guest.to_string(), local.display().to_string())
            .await?
    } else {
        if !std::path::Path::new(src).is_file() {
            return Err(CuaError::NotFound(format!("{src}: no such file")));
        }
        let guest = if guest.ends_with('/') {
            format!(
                "{guest}{}",
                std::path::Path::new(src)
                    .file_name()
                    .map(|f| f.to_string_lossy().to_string())
                    .unwrap_or_default()
            )
        } else {
            guest.to_string()
        };
        env.upload_file(src.to_string(), guest, None).await?
    };
    print(
        out,
        serde_json::json!({"direction": direction, "size": t.size, "sha256": t.sha256,
            "src": src, "dst": dst}),
        json,
        || format!("{src} -> {dst} ({} bytes)", t.size),
    );
    Ok(0)
}

/// `sb logs`: the backend console (QEMU serial log, container logs) or the
/// guest system log through the spacesd.
async fn logs(
    cua: &Arc<Cua>,
    name: &str,
    tail: u32,
    source: &str,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    // The state file is keyed by the plain name, not the ref.
    let plain = match cua.sandboxes().get(name.to_string()).await {
        Ok(i) => i.name,
        Err(e @ CuaError::AmbiguousSandbox(_)) => return Err(e),
        Err(_) => name.to_string(),
    };
    let state = state_of(&plain).unwrap_or_default();
    let serial = state["serial_log"].as_str().map(str::to_string);
    let container = state["container_id"].as_str().map(str::to_string);
    let console = serial.is_some() || container.is_some();
    let use_console = match source {
        "console" => true,
        "guest" => false,
        "auto" => console,
        other => {
            return Err(CuaError::InvalidArgument(format!(
                "unknown --source {other:?} (auto, console, guest)"
            )));
        }
    };
    if use_console {
        if let Some(path) = serial {
            let text = std::fs::read_to_string(&path)
                .map_err(|e| CuaError::NotFound(format!("serial log {path}: {e}")))?;
            let lines: Vec<&str> = text.lines().collect();
            let start = lines.len().saturating_sub(tail as usize);
            for l in &lines[start..] {
                line(out, *l);
            }
            return Ok(0);
        }
        if let Some(id) = container {
            let o = tokio::process::Command::new("docker")
                .args(["logs", "--tail", &tail.to_string(), &id])
                .output()
                .await
                .map_err(|e| CuaError::HostCapabilityMissing(format!("docker logs: {e}")))?;
            out.write_all(&o.stdout).ok();
            out.write_all(&o.stderr).ok();
            return Ok(o.status.code().unwrap_or(1));
        }
        return Err(CuaError::Unsupported(format!(
            "sandbox {name} has no console log on its backend; use --source guest"
        )));
    }
    let env = env_of(cua, name).await?;
    let n = tail.max(1);
    let script = format!(
        "journalctl -n {n} --no-pager 2>/dev/null || tail -n {n} /var/log/syslog 2>/dev/null \
         || tail -n {n} /var/log/messages 2>/dev/null || dmesg 2>/dev/null | tail -n {n}"
    );
    let o = env.sh(script, Some(60_000)).await?;
    out.write_all(&o.stdout).ok();
    Ok(0)
}

/// `sb port-forward NAME PORT[:LOCAL]`.
async fn port_forward(
    cua: &Arc<Cua>,
    name: &str,
    spec: &str,
    no_wait: bool,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let (port, local) = match spec.split_once(':') {
        Some((p, l)) => (p, Some(l)),
        None => (spec, None),
    };
    let bad = || CuaError::InvalidArgument(format!("bad port spec {spec:?} (PORT or PORT:LOCAL)"));
    let port: u16 = port.parse().map_err(|_| bad())?;
    let local: Option<u16> = local.map(|l| l.parse().map_err(|_| bad())).transpose()?;
    let sb = sandbox_with_token(cua, name).await?;
    // Every provider gets a loopback forward: local runtimes through the
    // published port (an unpublished one over the spacesd's /tunnel), cloud
    // and url: sandboxes over the spacesd's /tunnel WebSocket, and cloud
    // images without it through a loopback HTTP proxy to the gateway. A forward without a listener (older daemons) falls
    // back to a signed HTTPS URL for the port's service.
    let fwd = sb.forward(port).await?;
    if fwd.local_addr().is_none() && sb.location() == "cloud" {
        let _ = fwd.close().await;
        // Reattached handles know service names only; the state file keeps
        // the ports a managed sandbox was created with.
        let recorded: HashMap<String, u16> = state_of(name)
            .and_then(|s| serde_json::from_value(s["services"].clone()).ok())
            .unwrap_or_default();
        let service = sb
            .services()
            .into_iter()
            .chain(recorded)
            .find(|(_, p)| *p == port)
            .map(|(n, _)| n)
            .unwrap_or_else(|| {
                if port == 3211 {
                    "env".into()
                } else {
                    format!("port-{port}")
                }
            });
        let (url, _) = service_page_url(cua, name, Some(service)).await?;
        print(
            out,
            serde_json::json!({"name": name, "guest_port": port, "url": url}),
            json,
            || format!("Fleet port {port} of {name}: {url}"),
        );
        return Ok(0);
    }
    let target = fwd.local_addr();
    let listen = match (local, &target) {
        (Some(l), Some(t)) => {
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", l))
                .await
                .map_err(|e| CuaError::InvalidArgument(format!("127.0.0.1:{l}: {e}")))?;
            let t = t.clone();
            tokio::spawn(async move {
                while let Ok((mut inbound, _)) = listener.accept().await {
                    let t = t.clone();
                    tokio::spawn(async move {
                        if let Ok(mut up) = tokio::net::TcpStream::connect(&t).await {
                            let _ = tokio::io::copy_bidirectional(&mut inbound, &mut up).await;
                        }
                    });
                }
            });
            Some(format!("127.0.0.1:{l}"))
        }
        _ => target.clone(),
    };
    let addr = listen.clone().or_else(|| fwd.url()).unwrap_or_default();
    print(
        out,
        serde_json::json!({"name": name, "guest_port": port, "local_addr": listen, "url": fwd.url()}),
        json,
        || format!("Forwarding {addr} -> {name}:{port}"),
    );
    let _ = out.flush();
    if !no_wait {
        eprintln!("Press Ctrl-C to stop.");
        let _ = tokio::signal::ctrl_c().await;
    }
    let _ = fwd.close().await;
    Ok(0)
}

/// Every Fleet claim in the account's namespaces (live, read-only).
async fn fleet_claims() -> Result<Vec<serde_json::Value>, CuaError> {
    let (c, _) = auth::fleet_client().await?;
    let sdk = c.sdk();
    let nss = sdk
        .clone()
        .list_namespaces()
        .await
        .map_err(|e| auth::fleet_err(cua_fleet::Error::Sdk(e)))?;
    let mut out = vec![];
    for ns in nss {
        let claims = match c.list_claims(&ns.name).await.map_err(auth::fleet_err) {
            Ok(c) => c,
            Err(CuaError::PermissionDenied(_)) => continue,
            Err(e) => return Err(e),
        };
        for cl in claims {
            let v = serde_json::to_value(&cl)?;
            out.push(serde_json::json!({
                "id": format!("cloud:{}", v["metadata"]["name"].as_str().unwrap_or_default()),
                "name": v["metadata"]["name"],
                "location": "cloud",
                "kind": "",
                "runtime": "",
                "runtime_type": format!("fleet/{}", ns.name),
                "status": v["status"]["phase"].as_str().unwrap_or("unknown").to_lowercase(),
                "namespace": ns.name,
            }));
        }
    }
    Ok(out)
}

/// Web display services of images without cua-spacesd, in preference order
/// (`cua sb view --service` picks one explicitly).
pub const LEGACY_DISPLAY_SERVICES: [&str; 3] = ["novnc", "display", "web"];

/// `cua sb view`: open the sandbox desktop in the HTML5 viewer.
pub async fn view(
    cua: &Arc<Cua>,
    v: ViewArgs,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let name = v.at.apply(&v.name)?;
    let ttl = v
        .ttl
        .as_deref()
        .map(parse_secs)
        .transpose()
        .map_err(CuaError::InvalidArgument)?;
    let (url, expires, forward) = match v.service {
        Some(service) => {
            let (url, forward) = service_page_url(cua, &name, Some(service)).await?;
            (url, None, forward)
        }
        None => {
            let files = match v.files.as_deref() {
                Some("none") => Some(String::new()),
                other => other.map(str::to_owned),
            };
            match viewer_link(cua, &name, ttl, v.view_only, files).await {
                Ok(link) => (link.url, Some(link.expires_at_unix), None),
                Err(CuaError::SpacesdNotAvailable(m)) => match agentless(cua, &name).await? {
                    Some(sb) => return view_agentless(&sb, &name, &v, json, out).await,
                    None => return Err(CuaError::SpacesdNotAvailable(m)),
                },
                Err(e) => return Err(e),
            }
        }
    };
    print(
        out,
        serde_json::json!({"name": name, "url": url, "expires_at_unix": expires}),
        json,
        || format!("Viewer for {name}: {url}"),
    );
    let _ = out.flush();
    if !v.no_open {
        util::open_browser(&url);
    }
    if let Some(f) = forward {
        eprintln!("Forwarding to {name}; press Ctrl-C to stop.");
        let _ = tokio::signal::ctrl_c().await;
        let _ = f.close().await;
    }
    Ok(0)
}

/// After cua-spacesd did not answer: the sandbox, when it is a local Lume
/// sandbox, which `exec`, `screenshot` and `view` then reach over SSH and
/// VNC instead (`Sandbox.guest_sh` / `guest_screenshot` /
/// `guest_display`). `None` for every other sandbox, which keeps the
/// cua-spacesd error.
async fn agentless(cua: &Arc<Cua>, name: &str) -> Result<Option<Arc<cua_sdk::Sandbox>>, CuaError> {
    let sb = cua.sandboxes().connect(name.to_string()).await?;
    let info = sb.info();
    Ok((info.location == "local" && info.runtime == "lume").then_some(sb))
}

/// `cua sb exec` without cua-spacesd: over `lume ssh`, output streamed.
async fn exec_agentless(
    sb: &cua_sdk::Sandbox,
    name: &str,
    script: &str,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    use std::io::IsTerminal;
    if !json && std::io::stderr().is_terminal() {
        eprintln!("{name}: no cua-spacesd; running via SSH (lume ssh)");
    }
    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    let run = sb.guest_sh_streaming(script.to_string(), Some(120_000), tx);
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let forward = async {
        while let Some(c) = rx.recv().await {
            match c {
                cua_sandbox_core::GuestOutput::Stdout(b) if json => stdout.extend(b),
                cua_sandbox_core::GuestOutput::Stderr(b) if json => stderr.extend(b),
                cua_sandbox_core::GuestOutput::Stdout(b) => {
                    out.write_all(&b).ok();
                    out.flush().ok();
                }
                cua_sandbox_core::GuestOutput::Stderr(b) => {
                    std::io::stderr().write_all(&b).ok();
                }
            }
        }
    };
    let (code, ()) = tokio::join!(run, forward);
    let code = i32::try_from(code?).unwrap_or(1);
    if json {
        line(
            out,
            serde_json::json!({
                "stdout": String::from_utf8_lossy(&stdout),
                "stderr": String::from_utf8_lossy(&stderr),
                "returncode": code,
                "via": "ssh",
            })
            .to_string(),
        );
    }
    Ok(code)
}

/// `cua sb view` without cua-spacesd: the VM's own display over VNC,
/// opened with `lume attach` (what `lume attach` shows: the native display,
/// VNC as its fallback). The HTML5 viewer needs cua-spacesd.
async fn view_agentless(
    sb: &cua_sdk::Sandbox,
    name: &str,
    v: &ViewArgs,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    if v.view_only {
        return Err(CuaError::Unsupported(format!(
            "--view-only needs cua-spacesd; {name} has none, and its VNC display takes input"
        )));
    }
    if v.files.is_some() || v.ttl.is_some() {
        eprintln!("warning: --files and --ttl apply to the cua-spacesd viewer; ignored over VNC");
    }
    let display = sb.guest_display().await?;
    // Only the masked URL: the password never reaches the output.
    let (url, via, open_command) = (display.url(), display.via(), display.open_command());
    // `lume attach NAME`, with the program by its file name.
    let shown = open_command
        .iter()
        .enumerate()
        .map(|(i, a)| match i {
            0 => std::path::Path::new(a)
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_else(|| a.clone()),
            _ => a.clone(),
        })
        .collect::<Vec<_>>()
        .join(" ");
    let can_open = !open_command.is_empty()
        && std::env::var_os("CUA_NO_BROWSER").is_none()
        && util::interactive();
    let open = !v.no_open && can_open;
    print(
        out,
        serde_json::json!({
            "name": name,
            "url": url,
            "via": via,
            "open_command": open_command,
            "opened": open,
        }),
        json,
        || {
            let how = if open {
                format!(" (opening with `{shown}`)")
            } else if open_command.is_empty() {
                String::new()
            } else {
                format!("; open it with `{shown}`")
            };
            format!("Display of {name} via VNC: {}{how}", url)
        },
    );
    let _ = out.flush();
    if open {
        // `lume attach` hands the display to a viewer app and exits.
        let (program, args) = open_command.split_first().expect("non-empty");
        let run = tokio::process::Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output();
        let o = tokio::time::timeout(std::time::Duration::from_secs(60), run)
            .await
            .map_err(|_| CuaError::Timeout(format!("`{}` did not return", open_command.join(" "))))?
            .map_err(|e| CuaError::Internal(format!("running {program}: {e}")))?;
        if !o.status.success() {
            return Err(CuaError::Runtime(format!(
                "`{}` failed: {}",
                open_command.join(" "),
                String::from_utf8_lossy(&o.stderr).trim()
            )));
        }
    }
    Ok(0)
}

/// A viewer link (`Sandbox.viewer_url`) for `name`.
pub async fn viewer_link(
    cua: &Arc<Cua>,
    name: &str,
    ttl_secs: Option<u32>,
    view_only: bool,
    files_root: Option<String>,
) -> Result<cua_sdk::ViewerLink, CuaError> {
    let sandbox = sandbox_with_token(cua, name).await?;
    sandbox
        .viewer_url(Some(cua_sdk::ViewerOptions {
            ttl_seconds: ttl_secs,
            view_only,
            files_root,
            ..Default::default()
        }))
        .await
        .map_err(|e| match e {
            CuaError::SpacesdNotAvailable(m) => CuaError::SpacesdNotAvailable(format!(
                "{m}. The viewer needs cua-spacesd in the sandbox; for an image that serves its own display page pass --service NAME"
            )),
            other => other,
        })
}

/// A browser URL for a web service of the sandbox (no credentials): the
/// loopback port locally, a signed service URL (1 h) on Fleet. With no
/// `service`, the first of the legacy display services an image declares
/// (`novnc`, `display`, `web`).
pub async fn service_page_url(
    cua: &Arc<Cua>,
    name: &str,
    service: Option<String>,
) -> Result<(String, Option<Arc<cua_sdk::PortForward>>), CuaError> {
    let i = cua.sandboxes().get(name.to_string()).await?;
    let known: Vec<&String> = i.services.keys().chain(i.endpoints.keys()).collect();
    let svc = match service {
        Some(s) => s,
        None => LEGACY_DISPLAY_SERVICES
            .iter()
            .find(|s| known.iter().any(|k| k.as_str() == **s))
            .map(|s| s.to_string())
            .ok_or_else(|| {
                CuaError::NotFound(format!(
                    "sandbox {name} declares no web display service (novnc, display, web); pass --service"
                ))
            })?,
    };
    match i.location.as_str() {
        "cloud" => {
            let ep = i
                .endpoints
                .values()
                .next()
                .cloned()
                .ok_or_else(|| CuaError::NotFound(format!("no Fleet endpoint for {name}")))?;
            let ns = ep
                .split("/api/svc/")
                .nth(1)
                .and_then(|r| r.split('/').next())
                .ok_or_else(|| CuaError::Internal(format!("unexpected Fleet endpoint {ep}")))?
                .to_string();
            let (c, _) = auth::fleet_client().await?;
            let bound = c.attach_claim(&ns, name).await.map_err(auth::fleet_err)?;
            let s = c
                .create_signed_service_url(
                    &bound,
                    &svc,
                    Some("cua sb view".into()),
                    std::time::Duration::from_secs(3600),
                )
                .await
                .map_err(auth::fleet_err)?;
            Ok((s.url, None))
        }
        _ => {
            if let Some(u) = i.endpoints.get(&svc) {
                return Ok((u.clone(), None));
            }
            let port = *i.services.get(&svc).ok_or_else(|| {
                CuaError::NotFound(format!("sandbox {name} has no service {svc}"))
            })?;
            let f = cua
                .sandboxes()
                .connect(name.to_string())
                .await?
                .forward(port)
                .await?;
            let url = f
                .url()
                .or_else(|| f.local_addr().map(|a| format!("http://{a}")))
                .ok_or_else(|| CuaError::Internal("port forward has no address".into()))?;
            // The forward lives as long as this process.
            Ok((url, Some(f)))
        }
    }
}

/// Outcome of a destructive command's confirmation (`cua sandbox rm`,
/// `cua host remove`).
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DeleteConfirmation {
    Proceed,
    Declined,
    /// No terminal to ask on and no `--force`: deleting is never implied.
    NeedsForce,
}

/// The error when a destructive command has no terminal to confirm on
/// and no `--force` (exit 2, `--json` included): deleting is never implied,
/// and the message names the flag.
pub(crate) fn needs_force(action: &str) -> CuaError {
    CuaError::InvalidArgument(format!(
        "not confirmed: stdin is not a terminal, so cua cannot ask before it would {action}; \
         pass --force (-f) to {action} without a prompt"
    ))
}

pub(crate) fn confirm_delete(
    force: bool,
    interactive: bool,
    ask: impl FnOnce() -> bool,
) -> DeleteConfirmation {
    if force {
        DeleteConfirmation::Proceed
    } else if !interactive {
        DeleteConfirmation::NeedsForce
    } else if ask() {
        DeleteConfirmation::Proceed
    } else {
        DeleteConfirmation::Declined
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_and_registry_secret_flags_parse() {
        let c = parse_sidecar("redis:7-alpine,port=6379,env=A=b=c,name=db").unwrap();
        assert_eq!(c.image, "redis:7-alpine");
        assert_eq!(c.ports, vec![6379]);
        assert_eq!(c.env.get("A").map(String::as_str), Some("b=c"));
        assert_eq!(c.name.as_deref(), Some("db"));
        assert!(parse_sidecar("redis,port=x").is_err());
        assert!(parse_sidecar("redis,cpu=1").is_err());
        assert!(parse_sidecar("port=1").is_err());
        assert!(matches!(
            parse_registry_secret("env:U:P").unwrap(),
            cua_sdk::RegistrySecret::FromEnv { ref username_var, .. } if username_var == "U"
        ));
        assert!(matches!(
            parse_registry_secret("aws-ecr:us-west-2").unwrap(),
            cua_sdk::RegistrySecret::AwsEcr { region: Some(ref r) } if r == "us-west-2"
        ));
        assert!(
            parse_registry_secret("user:pass").is_err(),
            "no passwords on the command line"
        );
    }

    #[test]
    fn on_kind_and_runtime_are_separate_flags() {
        use clap::Parser;
        #[derive(Parser)]
        struct T {
            #[command(flatten)]
            a: CreateArgs,
        }
        let p = |args: &[&str]| T::try_parse_from(["t", "linux"].iter().chain(args));
        let t = p(&["--on", "cloud", "--kind", "vm", "--runtime", "kubevirt"]).unwrap();
        assert_eq!(t.a.on, Some(On::Cloud));
        assert_eq!(t.a.kind, Some(Kind::Vm));
        assert_eq!(t.a.runtime, Some(Runtime::Kubevirt));
        let t = p(&["--on", "direct:[::1]:3211"]).unwrap();
        assert_eq!(t.a.on, Some(On::Direct("[::1]:3211".into())));
        // Engines and kinds are not locations; the error points at the flag.
        for (bad, hint) in [
            ("qemu", "--runtime qemu"),
            ("lume", "--runtime lume"),
            ("docker", "--kind container"),
            ("vm", "--kind vm"),
            ("fleet", "cloud"),
        ] {
            let e = p(&["--on", bad]).err().unwrap().to_string();
            assert!(e.contains(hint), "{bad}: {e}");
        }
        assert!(
            p(&["--kind", "qemu"])
                .err()
                .unwrap()
                .to_string()
                .contains("runtime")
        );
        assert!(
            p(&["--runtime", "vm"])
                .err()
                .unwrap()
                .to_string()
                .contains("--kind vm")
        );
        // The flags `create` never shipped are gone.
        for gone in ["--vm", "--container", "--provider", "--url", "--local"] {
            assert!(p(&[gone]).is_err(), "{gone}");
        }
    }

    #[test]
    fn launch_keeps_its_shipped_flags_as_deprecated_aliases() {
        use clap::Parser;
        #[derive(Parser)]
        struct T {
            #[command(flatten)]
            a: LaunchArgs,
        }
        let p = |args: &[&str]| {
            T::try_parse_from(["t", "linux"].iter().chain(args))
                .unwrap()
                .a
        };
        let c = p(&[]).into_create().unwrap();
        assert_eq!(
            (c.on, c.kind),
            (Some(On::Cloud), None),
            "launch defaults to the cloud"
        );
        let c = p(&["--local", "--vm"]).into_create().unwrap();
        assert_eq!((c.on, c.kind), (Some(On::Local), Some(Kind::Vm)));
        let c = p(&["--on", "local", "--kind", "container"])
            .into_create()
            .unwrap();
        assert_eq!((c.on, c.kind), (Some(On::Local), Some(Kind::Container)));
    }

    #[test]
    fn placement_resolves_flags_then_pool_then_defaults() {
        let _env = crate::util::test_env::isolated();
        let a = CreateArgs {
            pool: Some("p".into()),
            ..Default::default()
        };
        assert_eq!(resolve_placement(&a).unwrap().on, On::Cloud);
        let a = CreateArgs {
            on: Some(On::Local),
            pool: Some("p".into()),
            ..Default::default()
        };
        assert_eq!(resolve_placement(&a).unwrap().on, On::Local);
        let a = CreateArgs::default();
        assert_eq!(resolve_placement(&a).unwrap().on, On::Local);
        let a = CreateArgs {
            on: Some(On::Cloud),
            runtime: Some(Runtime::Qemu),
            ..Default::default()
        };
        let e = resolve_placement(&a).unwrap_err();
        assert!(
            e.to_string()
                .contains("valid runtime: auto, gvisor, kubevirt"),
            "{e}"
        );
    }

    #[test]
    fn keep_on_failure_flag_and_kept_message_name_the_claim() {
        use clap::Parser;
        #[derive(Parser)]
        struct T {
            #[command(flatten)]
            a: CreateArgs,
        }
        let p = |args: &[&str]| T::try_parse_from(["t", "linux"].iter().chain(args));
        assert!(!p(&[]).unwrap().a.keep_on_failure);
        assert!(p(&["--keep-on-failure"]).unwrap().a.keep_on_failure);
        let launch = LaunchArgs {
            image: Some("linux".into()),
            pool: None,
            local: false,
            on: None,
            name: Some("cua-e2e-x".into()),
            vm: false,
            kind: None,
            runtime: None,
            cpu: None,
            memory: None,
            disk: None,
            region: None,
            ports: vec![],
            warm: false,
            max_pool_size: None,
            claim_ttl: None,
            keep_on_failure: true,
        };
        assert!(launch.into_create().unwrap().keep_on_failure);

        let info = SandboxInfo {
            name: "cua-e2e-kept".into(),
            kind: "container".into(),
            runtime: "gvisor".into(),
            runtime_type: "fleet".into(),
            status: cua_sdk::SandboxStatus::Provisioning,
            status_detail: None,
            ephemeral: false,
            services: HashMap::new(),
            endpoints: HashMap::new(),
            image: None,
            id: "cloud:cua-e2e-kept".into(),
            phase: cua_sdk::SandboxPhase::Provisioning,
            location: "cloud".into(),
            expires_at_unix: None,
            provider_details: HashMap::from([
                ("claim".to_string(), "cua-e2e-kept".to_string()),
                ("pool".to_string(), "cua-auto-abc".to_string()),
                ("namespace".to_string(), "cua-auto-abc".to_string()),
            ]),
            image_info: None,
        };
        let m = kept_message(&info);
        assert!(m.contains("cloud:cua-e2e-kept"), "{m}");
        assert!(m.contains("claim cua-e2e-kept, pool cua-auto-abc"), "{m}");
        assert!(m.contains("`cua sb rm cloud:cua-e2e-kept --force`"), "{m}");
        assert!(
            !m.contains("namespace"),
            "namespace equal to the pool is not repeated: {m}"
        );
    }

    #[test]
    fn network_flag_takes_default_or_none() {
        use clap::Parser;
        #[derive(Parser)]
        struct T {
            #[command(flatten)]
            a: CreateArgs,
        }
        let p = |args: &[&str]| T::try_parse_from(["t", "linux"].iter().chain(args));
        assert_eq!(p(&[]).unwrap().a.network, None);
        assert_eq!(
            p(&["--network", "none"]).unwrap().a.network.as_deref(),
            Some("none")
        );
        assert_eq!(
            p(&["--network", "default"]).unwrap().a.network.as_deref(),
            Some("default")
        );
        assert!(p(&["--network", "host"]).is_err());
    }

    #[test]
    fn narrow_qualifies_bare_names() {
        let n = |local, cloud| Narrow { local, cloud };
        assert_eq!(n(false, false).apply("box").unwrap(), "box");
        assert_eq!(n(true, false).apply("box").unwrap(), "local:box");
        assert_eq!(n(false, true).apply("box").unwrap(), "cloud:box");
        assert_eq!(n(false, true).apply("cloud:box").unwrap(), "cloud:box");
        assert!(n(true, false).apply("cloud:box").is_err());
    }

    #[test]
    fn remote_specs_and_secs() {
        assert_eq!(remote("box:/tmp/a"), Some(("box", "/tmp/a")));
        // Refs carry colons of their own.
        assert_eq!(remote("local:box:/tmp/a"), Some(("local:box", "/tmp/a")));
        assert_eq!(remote("cloud:box:~/a"), Some(("cloud:box", "~/a")));
        assert_eq!(
            remote("direct:10.0.0.5:3211:/tmp/a"),
            Some(("direct:10.0.0.5:3211", "/tmp/a"))
        );
        assert_eq!(
            remote("direct:[::1]:3211:/a"),
            Some(("direct:[::1]:3211", "/a"))
        );
        assert_eq!(remote("win:C:\\x"), Some(("win", "C:\\x")));
        assert_eq!(remote("file.txt:x"), None);
        assert_eq!(remote("./x:y"), None);
        assert_eq!(remote("C:\\x"), None);
        assert_eq!(remote("/abs"), None);
        assert_eq!(parse_secs("15m").unwrap(), 900);
        assert_eq!(parse_secs("900").unwrap(), 900);
        assert_eq!(
            parse_wait("http:8000/status").unwrap(),
            (WaitTarget::Port(8000), Some("/status".into()))
        );
        assert_eq!(parse_wait("tcp:22").unwrap(), (WaitTarget::Port(22), None));
        assert_eq!(
            parse_wait("http:mcp/health").unwrap(),
            (WaitTarget::Service("mcp".into()), Some("/health".into()))
        );
        assert_eq!(
            parse_wait("tcp:mcp").unwrap(),
            (WaitTarget::Service("mcp".into()), None)
        );
        assert!(parse_wait("tcp:0").is_err());
        assert!(parse_wait("tcp:a b").is_err());
        assert_eq!(parse_wait("desktop").unwrap(), (WaitTarget::Desktop, None));
        assert_eq!(
            parse_wait("tcp:desktop").unwrap(),
            (WaitTarget::Service("desktop".into()), None)
        );
        assert_eq!(WaitTarget::Desktop.probe(None), None);
        assert_eq!(
            parse_port_spec("novnc=6080").unwrap(),
            (Some("novnc".into()), 6080)
        );
        assert_eq!(parse_port_spec("8080").unwrap(), (None, 8080));
    }

    #[test]
    fn memory_strings() {
        assert_eq!(parse_memory("8GB").unwrap(), 8192);
        assert_eq!(parse_memory("4096MB").unwrap(), 4096);
        assert_eq!(parse_memory("8").unwrap(), 8192);
        assert!(parse_memory("lots").is_err());
        // The Omarchy harness once passed `--memory 4096` meaning MiB, which a
        // bare number (GB) turned into 4 TiB (memory_mb 4194304).
        let err = parse_memory("4096").unwrap_err().to_string();
        assert!(err.contains("ambiguous") && err.contains("4096MB"), "{err}");
        assert!(parse_memory("4194304MB").is_err());
        assert!(parse_memory("4096GB").is_err());
        assert!(parse_memory("inf").is_err());
        assert!(parse_memory("NaN").is_err());
        assert_eq!(parse_memory("1024GB").unwrap(), 1024 * 1024);
        assert_eq!(parse_memory("512").unwrap(), 512 * 1024);
    }

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn image_tiers_and_omarchy() {
        let r = |image: &str, tier: Option<&str>| resolve_image_with(image, tier, no_env);
        assert_eq!(
            r("linux", Some("full")).unwrap().0,
            "ghcr.io/trycua/linux:24.04"
        );
        assert!(matches!(
            r("linux", Some("tiny")),
            Err(CuaError::InvalidArgument(_))
        ));
        assert!(matches!(
            r("linux", Some("xcode")),
            Err(CuaError::InvalidArgument(_))
        ));
        assert!(matches!(
            r("python:3.12", Some("slim")),
            Err(CuaError::InvalidArgument(_))
        ));
        let published = |x: &str| cua_image::catalog::find(x).is_none_or(|e| e.published);
        for (got, want, os) in [
            (
                r("linux", Some("slim")),
                "ghcr.io/trycua/linux:24.04-slim",
                "linux",
            ),
            (
                r("macos", Some("xcode")),
                "ghcr.io/trycua/macos:26-xcode",
                "macos",
            ),
            (r("omarchy", None), "ghcr.io/trycua/omarchy:edge", "linux"),
        ] {
            if published(want) {
                assert_eq!(got.unwrap(), (want.to_string(), Some(os.to_string())));
            } else {
                let e = got.unwrap_err();
                assert!(
                    matches!(&e, CuaError::ImageNotPublished(m) if m.contains(want)),
                    "{e:?}"
                );
            }
        }
        // An explicit reference is never gated.
        assert_eq!(
            r("ghcr.io/trycua/omarchy:edge", None).unwrap(),
            ("ghcr.io/trycua/omarchy:edge".to_string(), None)
        );
        let a = <crate::Cli as clap::Parser>::try_parse_from([
            "cua", "sb", "create", "linux", "--tier", "slim",
        ]);
        assert!(a.is_ok());
    }

    #[test]
    fn image_aliases() {
        let resolve = |image: &str, _vm: bool| resolve_image_with(image, None, no_env);
        assert_eq!(
            resolve("linux:24.04", false).unwrap(),
            (
                "ghcr.io/trycua/linux:24.04".to_string(),
                Some("linux".into())
            )
        );
        assert_eq!(
            resolve("ubuntu", false).unwrap().0,
            "ghcr.io/trycua/linux:24.04"
        );
        // A tagged `ubuntu` is Docker Hub's image (docker.io/library/ubuntu),
        // not the canonical Linux.
        assert_eq!(
            resolve("ubuntu:24.04", false).unwrap(),
            ("ubuntu:24.04".to_string(), None)
        );
        // The kind is its own flag: an alias never carries a backend prefix.
        assert_eq!(
            resolve("linux", true).unwrap().0,
            "ghcr.io/trycua/linux:24.04"
        );
        assert_eq!(
            resolve("windows", false).unwrap(),
            (
                "ghcr.io/trycua/windows:2022".to_string(),
                Some("windows".into())
            )
        );
        let (m, os) = resolve("macos", false).unwrap();
        assert_eq!(m, "ghcr.io/trycua/macos:26");
        assert_eq!(os.as_deref(), Some("macos"));
        assert_eq!(
            resolve("macos:sequoia", true).unwrap().0,
            "ghcr.io/trycua/macos:15"
        );
        assert_eq!(
            resolve("ghcr.io/org/img:1", false).unwrap().0,
            "ghcr.io/org/img:1"
        );
        // Not aliases: plain registry refs (docker.io).
        assert_eq!(
            resolve("debian:12", false).unwrap(),
            ("debian:12".to_string(), None)
        );
        assert_eq!(resolve("python:3.12-slim", false).unwrap().1, None);
    }

    #[test]
    fn default_images_are_overridable_by_env() {
        let env = |name: &str| match name {
            "CUA_IMAGE_LINUX" => Some("registry.example/linux:1".to_string()),
            // The old name still works (deprecated).
            "CUA_DEFAULT_WINDOWS_IMAGE" => Some(" registry.example/windows:2 ".to_string()),
            _ => None,
        };
        let (linux, os) = resolve_image_with("linux", None, env).unwrap();
        assert_eq!(linux, "registry.example/linux:1");
        assert_eq!(os.as_deref(), Some("linux"));
        assert_eq!(
            resolve_image_with("ubuntu", None, env).unwrap().0,
            "registry.example/linux:1"
        );
        let (windows, os) = resolve_image_with("windows", None, env).unwrap();
        assert_eq!(windows, "registry.example/windows:2");
        assert_eq!(os.as_deref(), Some("windows"));
        // An empty override is ignored.
        let empty = |_: &str| Some("  ".to_string());
        assert_eq!(
            resolve_image_with("linux", None, empty).unwrap().0,
            "ghcr.io/trycua/linux:24.04"
        );
    }

    #[test]
    fn sandbox_rm_never_deletes_without_consent() {
        assert_eq!(
            confirm_delete(true, false, || panic!("no prompt with --force")),
            DeleteConfirmation::Proceed
        );
        assert_eq!(
            confirm_delete(false, false, || panic!("no prompt without a terminal")),
            DeleteConfirmation::NeedsForce
        );
        let e = needs_force("delete sandbox 'dev'").to_string();
        assert!(e.contains("stdin is not a terminal"), "{e}");
        assert!(
            e.contains("pass --force (-f) to delete sandbox 'dev'"),
            "{e}"
        );
        assert_eq!(
            confirm_delete(false, true, || true),
            DeleteConfirmation::Proceed
        );
        assert_eq!(
            confirm_delete(false, true, || false),
            DeleteConfirmation::Declined
        );
    }
}
