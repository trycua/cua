// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `cua-spacesd`: the in-sandbox daemon. One port (default 3211) serves
//! the `cua.env.v1` contract over native gRPC and gRPC-Web plus the HTTP side
//! channels; `join` additionally exposes it through a `cua-relay`.

mod doctor;
mod legacy;
#[cfg(target_os = "linux")]
mod volume_mount;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::{Args, Parser, Subcommand};
use cua_spacesd_server::config::{self, ServerConfig};
use cua_spacesd_server::{ServerBuilder, ServerContext};

/// cua-spacesd: the daemon inside every cua sandbox.
#[derive(Parser, Debug)]
#[command(
    name = "cua-spacesd",
    version,
    about,
    args_conflicts_with_subcommands = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    #[command(flatten)]
    serve: ServeArgs,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Serve on the local port (the default).
    Serve(ServeArgs),
    /// Serve locally and publish through a cua-relay (for machines behind NAT).
    Join(JoinArgs),
    /// Privileged helper for --await-token-file: mirror a root-only token
    /// file (the Fleet claim Secret) to a file the driver's user can read.
    #[cfg(unix)]
    TokenSync(TokenSyncArgs),
    /// Mount Cua Volume for the driver's VolumeService (Linux; run as root
    /// by the driver through `sudo -n`).
    #[cfg(target_os = "linux")]
    #[command(hide = true)]
    VolumeMount(volume_mount::VolumeMountArgs),
    /// The root service that mounts Cua Volume for the driver (Linux; run
    /// by the image's init as root).
    #[cfg(target_os = "linux")]
    #[command(hide = true)]
    VolumeHelper(volume_mount::VolumeHelperArgs),
    /// Check this guest against its image's claims and print one report
    /// (exit 0 pass, 1 fail, 2 the doctor itself failed).
    Doctor(doctor::DoctorArgs),
    /// Print what this binary was built with (version, protocol revision,
    /// linked cua-driver, tool schema hash, compiled encoders) as JSON. Image
    /// builds record it in /etc/cua-image/manifest.json.
    BuildInfo,
    /// This process's own live Screen Recording and Accessibility TCC
    /// status (macOS only), as JSON: `{"accessibility":bool,
    /// "screen_recording":bool}`. Those grants are per executable, so only
    /// cua-spacesd can answer for itself; `cua host setup` and `cua
    /// doctor` call this instead of guessing from another process.
    /// Nothing here grants or opens anything.
    #[cfg(target_os = "macos")]
    #[command(hide = true)]
    CheckPermissions,
    /// Pre-consolidation daemon modes (Unix socket, QUIC, remote shares).
    #[command(disable_help_flag = true)]
    Legacy {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

#[derive(Args, Debug, Clone)]
struct ServeArgs {
    /// Listen address. Default: 0.0.0.0:<port> with a token, 127.0.0.1:<port> without.
    #[arg(long, env = "CUA_ENV_LISTEN")]
    listen: Option<SocketAddr>,
    /// Port used when --listen is not given.
    #[arg(long, env = "CUA_ENV_PORT", default_value_t = cua_proto::SPACESD_DEFAULT_PORT)]
    port: u16,
    /// Access token (prefer CUA_ENV_TOKEN or the token file).
    #[arg(long, hide_env_values = true)]
    token: Option<String>,
    /// Token file.
    #[arg(long, env = "CUA_ENV_TOKEN_FILE", default_value = config::DEFAULT_TOKEN_FILE)]
    token_file: PathBuf,
    /// Allow a non-loopback bind without a token: only GetCapabilities,
    /// Health and Init answer until Init installs one (first-claim bootstrap
    /// behind an authenticated gateway).
    #[arg(
        long,
        env = "CUA_ENV_INSECURE_BOOTSTRAP",
        value_parser = clap::builder::BoolishValueParser::new()
    )]
    insecure_bootstrap: bool,
    /// Take the token only from --token-file, watching it: bind even while
    /// it is empty (serving only GetCapabilities and Health), install or
    /// rotate when it changes, revoke when it is emptied or removed. Tokens
    /// are never accepted over the network in this mode.
    #[arg(
        long,
        env = "CUA_ENV_AWAIT_TOKEN_FILE",
        value_parser = clap::builder::BoolishValueParser::new(),
        conflicts_with_all = ["token", "insecure_bootstrap"]
    )]
    await_token_file: bool,
    /// Token file poll interval in milliseconds (--await-token-file).
    #[arg(
        long,
        env = "CUA_ENV_TOKEN_POLL_MS",
        default_value_t = 500,
        hide = true
    )]
    token_poll_ms: u64,
    /// State directory (upload staging, CA bundle, teleport staging).
    #[arg(long, env = "CUA_ENV_DATA_DIR")]
    data_dir: Option<PathBuf>,
    /// Report this runtime instead of detecting it.
    #[arg(long, env = "CUA_ENV_RUNTIME")]
    runtime: Option<String>,
    /// Allow Shutdown(GUEST_POWEROFF / GUEST_REBOOT).
    #[arg(long, env = "CUA_ENV_ALLOW_GUEST_POWER")]
    allow_guest_power: bool,
    /// Presence cursor-shape probe: `false` never moves the idle guest
    /// pointer to read a participant's real cursor shape (presence then uses
    /// the accessibility hit-test only). `SystemService.Init` can change it.
    #[arg(long, env = "CUA_SPACESD_CURSOR_PROBE", default_value_t = true, action = clap::ArgAction::Set, value_parser = clap::builder::BoolishValueParser::new())]
    cursor_probe: bool,
    /// Do not serve /mcp.
    #[arg(long, env = "CUA_ENV_NO_MCP")]
    no_mcp: bool,
    /// Do not record remote accesses in `<data-dir>/access.log`.
    #[arg(long, env = "CUA_SPACESD_NO_ACCESS_LOG")]
    no_access_log: bool,
    /// Do not link the cua-driver tool registry (Driver service and /mcp
    /// report unsupported).
    #[arg(long, env = "CUA_ENV_NO_DRIVER")]
    no_driver: bool,
    /// Do not serve the desktop services (Computer, Windows, Accessibility,
    /// Stream, Presence, /media).
    #[arg(long, env = "CUA_ENV_NO_DESKTOP")]
    no_desktop: bool,
    /// Teleport destination (default ~/Downloads).
    #[arg(long, env = "CUA_ENV_DOWNLOADS_DIR")]
    downloads_dir: Option<PathBuf>,
    /// Always use the polling filesystem watcher.
    #[arg(long, env = "CUA_ENV_POLL_WATCHER")]
    poll_watcher: bool,
    /// UDP port of the direct QUIC media listener, bound on the --listen
    /// address (0 = no QUIC; media stays on the /media WebSocket).
    #[arg(long, env = "CUA_ENV_QUIC_PORT", default_value_t = cua_proto::SPACESD_DEFAULT_QUIC_PORT)]
    quic_port: u16,
    /// Default per-process scrollback in bytes.
    #[arg(long, env = "CUA_ENV_SCROLLBACK_BYTES", default_value_t = config::DEFAULT_SCROLLBACK_BYTES)]
    scrollback_bytes: u64,
    /// Serve `HostSpacesService` on this listener from this host policy
    /// (written by `cua host setup --direct ... --provide-spaces`, reloaded
    /// on every call): the token holder creates Spaces on this machine,
    /// from loopback, Tailscale and private LAN addresses unless the
    /// policy allows any address.
    #[arg(long, env = "CUA_ENV_DIRECT_HOST_POLICY")]
    direct_host_policy: Option<PathBuf>,
    /// Print the effective configuration as JSON (never the token) and exit.
    #[arg(long)]
    print_config: bool,
}

#[cfg(unix)]
#[derive(Args, Debug, Clone)]
struct TokenSyncArgs {
    /// Root-only source (the claim Secret file).
    #[arg(long, default_value = config::DEFAULT_TOKEN_FILE)]
    from: PathBuf,
    /// Target the driver reads with --await-token-file. Its directory is
    /// created root-owned 0755 so the driver's user cannot replace the file.
    #[arg(long)]
    to: PathBuf,
    /// User that owns the target (0600).
    #[arg(long, default_value = "cua")]
    owner: String,
    /// Poll interval in milliseconds.
    #[arg(long, default_value_t = 250)]
    interval_ms: u64,
    /// Accept a world-accessible source (default: only as root in a container).
    #[arg(long)]
    allow_world_readable: bool,
    /// Sync once and exit.
    #[arg(long)]
    once: bool,
}

#[derive(Args, Debug, Clone)]
struct JoinArgs {
    /// Relay URL, e.g. wss://relay.example.
    #[arg(long, env = "CUA_ENV_RELAY_URL")]
    relay: String,
    /// Machine credential for the relay (not the env token): a static relay
    /// token, or the machine token from `cua host setup` (account mode).
    #[arg(
        long,
        env = "CUA_RELAY_TOKEN",
        hide_env_values = true,
        required_unless_present = "relay_token_file"
    )]
    relay_token: Option<String>,
    /// File holding the relay credential; re-read before every reconnect.
    #[arg(long, env = "CUA_RELAY_TOKEN_FILE")]
    relay_token_file: Option<PathBuf>,
    /// Host policy (owner, allowlist, sharing) for account mode, reloaded
    /// when it changes. Written by `cua host setup`.
    #[arg(long, env = "CUA_HOST_POLICY")]
    host_policy: Option<PathBuf>,
    /// Pin the relay's assertion keys (JWKS file) instead of trusting the
    /// keys the relay sends on join.
    #[arg(long, env = "CUA_RELAY_JWKS")]
    relay_jwks: Option<PathBuf>,
    /// Where the persistent machine id lives.
    #[arg(long, env = "CUA_ENV_MACHINE_ID_FILE")]
    machine_id_file: Option<PathBuf>,
    /// Join as this machine id (a machine another device registered, for
    /// example a Space in your cloud): written to the machine id file, so
    /// a restart joins as the same machine.
    #[arg(long, env = "CUA_ENV_MACHINE_ID")]
    machine_id: Option<String>,
    /// The relay's assertion keys as JSON (`--relay-jwks` from the
    /// environment, for a guest that has no files yet).
    #[arg(
        long,
        env = "CUA_RELAY_JWKS_JSON",
        hide_env_values = true,
        conflicts_with = "relay_jwks"
    )]
    relay_jwks_json: Option<String>,
    /// Host policy as JSON (`--host-policy` from the environment): kept in
    /// the data directory 0600 and reloaded from there.
    #[arg(
        long,
        env = "CUA_HOST_POLICY_JSON",
        hide_env_values = true,
        conflicts_with = "host_policy"
    )]
    host_policy_json: Option<String>,
    /// Heartbeat interval in seconds.
    #[arg(long, default_value_t = 15)]
    heartbeat_secs: u64,
    #[command(flatten)]
    serve: ServeArgs,
}

/// Accepts the `CUA_SPACESD_*` spelling of every `CUA_ENV_*` variable (the
/// original names keep working), and the interim `CUA_GUESTD_*` spelling.
/// Precedence: `CUA_SPACESD_*`, then `CUA_GUESTD_*`, then `CUA_ENV_*`. Runs
/// first in `main`, while the process is still single-threaded.
fn promote_spacesd_env_vars() {
    let vars: Vec<_> = std::env::vars_os().collect();
    for prefix in ["CUA_GUESTD_", "CUA_SPACESD_"] {
        for (key, value) in &vars {
            if let Some(rest) = key.to_str().and_then(|k| k.strip_prefix(prefix)) {
                if !rest.is_empty() {
                    std::env::set_var(format!("CUA_ENV_{rest}"), value);
                }
            }
        }
    }
}

fn main() {
    cua_driver_core::build_info::init(GIT_SHA);
    promote_spacesd_env_vars();
    let cli = Cli::parse();
    let command = cli.command.unwrap_or(Command::Serve(cli.serve));
    if let Command::Legacy { args } = command {
        legacy::main(args);
        return;
    }
    if let Command::BuildInfo = command {
        println!(
            "{}",
            serde_json::to_string_pretty(&build_info()).expect("build info serializes")
        );
        return;
    }
    #[cfg(target_os = "macos")]
    if let Command::CheckPermissions = command {
        let status = platform_macos::permissions::current_status();
        println!(
            "{}",
            serde_json::to_string(&status).expect("permissions status serializes")
        );
        return;
    }
    // `serve` and `join` build the desktop provider, whose macOS driver
    // registry initializes the agent-cursor overlay when there is a GUI
    // session. That overlay only runs (and only fires the arrivals an
    // animated click waits for) on the OS main thread, so the daemon runs on
    // a spawned thread and main enters the AppKit loop once the registry
    // exists, the same shape as `legacy::main`.
    #[cfg(target_os = "macos")]
    if matches!(command, Command::Serve(_) | Command::Join(_)) {
        std::thread::Builder::new()
            .name("cua-spacesd-serve".into())
            .spawn(move || finish(run(command)))
            .expect("spawn the cua-spacesd serve thread");
        cua_spacesd_desktop::wait_for_overlay_ready();
        platform_macos::cursor::overlay::run_on_main_thread();
        // Without a GUI session (or with the overlay off) that parks; the
        // serve thread owns the process exit.
        loop {
            std::thread::park();
        }
    }
    finish(run(command));
}

/// Runs a daemon command on a fresh multi-threaded runtime.
fn run(command: Command) -> Result<Exit, BoxError> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    runtime.block_on(async move {
        match command {
            #[cfg(unix)]
            Command::TokenSync(args) => token_sync(args).await,
            Command::Serve(args) => serve(args, None).await,
            Command::Join(join) => {
                let serve_args = join.serve.clone();
                serve(serve_args, Some(join)).await
            }
            Command::Doctor(args) => {
                let code = doctor::run(args).await;
                std::process::exit(code);
            }
            #[cfg(target_os = "linux")]
            Command::VolumeMount(args) => {
                let code = volume_mount::run(args).await;
                std::process::exit(code);
            }
            #[cfg(target_os = "linux")]
            Command::VolumeHelper(args) => {
                let code = volume_mount::run_helper(args).await;
                std::process::exit(code);
            }
            #[cfg(target_os = "macos")]
            Command::CheckPermissions => unreachable!(),
            Command::Legacy { .. } | Command::BuildInfo => unreachable!(),
        }
    })
}

/// Ends the process for a finished daemon command.
fn finish(result: Result<Exit, BoxError>) {
    match result {
        Ok(Exit::Done) => std::process::exit(0),
        Ok(Exit::Restart) => restart(),
        Err(error) => {
            eprintln!("cua-spacesd: {error}");
            std::process::exit(1);
        }
    }
}

enum Exit {
    Done,
    Restart,
}

type BoxError = Box<dyn std::error::Error + Send + Sync>;

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("CUA_ENV_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
        .try_init();
}

/// Resolves the token from every source (`--token`, `CUA_ENV_TOKEN`,
/// `--token-file` / `/run/cua/env-token`) before applying the bind policy,
/// so `--listen 0.0.0.0:3211` works with a token from any of them.
fn build_config(
    args: &ServeArgs,
    join: bool,
    env_token: Option<&str>,
) -> Result<(ServerConfig, Option<String>), BoxError> {
    if args.await_token_file {
        return build_await_config(args, join, env_token);
    }
    let mut resolved = config::resolve_token(args.token.as_deref(), env_token, &args.token_file)
        .map_err(|e| format!("reading {}: {e}", args.token_file.display()))?;
    // A joined machine is reachable through the relay, so it serves on
    // loopback only and always has a token. Without one (account mode: the
    // relay asserts callers' identity) a random local token is generated;
    // it never leaves this host.
    if join && resolved.is_none() {
        resolved = Some(config::ResolvedToken {
            token: random_token(),
            source: "generated (join)".into(),
        });
    }
    let has_token = resolved.is_some();
    let listen = match (args.listen, join) {
        (Some(listen), _) => listen,
        (None, true) => SocketAddr::from(([127, 0, 0, 1], args.port)),
        (None, false) => config::default_listen(has_token, args.port),
    };
    let config = ServerConfig {
        listen,
        token_source: resolved
            .as_ref()
            .map(|r| r.source.clone())
            .unwrap_or_else(|| "none".into()),
        insecure_bootstrap: args.insecure_bootstrap,
        // The first bootstrap Init persists its token where a restart reads it.
        bootstrap_token_file: (args.insecure_bootstrap && !has_token)
            .then(|| args.token_file.clone()),
        data_dir: args
            .data_dir
            .clone()
            .unwrap_or_else(config::default_data_dir),
        default_scrollback_bytes: args.scrollback_bytes,
        enable_mcp: !args.no_mcp,
        allow_guest_power: args.allow_guest_power,
        runtime_override: args.runtime.clone(),
        media_quic_port: args.quic_port,
        downloads_dir: args.downloads_dir.clone(),
        force_poll_watcher: args.poll_watcher,
        cursor_probe: args.cursor_probe,
        access_log: !args.no_access_log,
        ..ServerConfig::default()
    };
    config::check_bind_policy(config.listen, has_token, config.insecure_bootstrap)?;
    Ok((config, resolved.map(|r| r.token)))
}

/// `--await-token-file`: no token at start-up (the server reads and watches
/// the file itself); binds every interface by default.
fn build_await_config(
    args: &ServeArgs,
    join: bool,
    env_token: Option<&str>,
) -> Result<(ServerConfig, Option<String>), BoxError> {
    if join {
        return Err("--await-token-file is not supported with join".into());
    }
    if env_token.is_some_and(|t| !t.trim().is_empty())
        || args.token.as_deref().is_some_and(|t| !t.trim().is_empty())
    {
        return Err(
            "--await-token-file takes the token only from --token-file; unset CUA_ENV_TOKEN / --token"
                .into(),
        );
    }
    let config = ServerConfig {
        listen: args
            .listen
            .unwrap_or_else(|| config::default_listen(true, args.port)),
        token_source: format!("await-file:{}", args.token_file.display()),
        await_token_file: Some(args.token_file.clone()),
        token_poll_interval: Duration::from_millis(args.token_poll_ms.clamp(10, 60_000)),
        token_file_allow_world_readable:
            cua_spacesd_server::token_file::default_allow_world_readable(),
        data_dir: args
            .data_dir
            .clone()
            .unwrap_or_else(config::default_data_dir),
        default_scrollback_bytes: args.scrollback_bytes,
        enable_mcp: !args.no_mcp,
        allow_guest_power: args.allow_guest_power,
        runtime_override: args.runtime.clone(),
        media_quic_port: args.quic_port,
        downloads_dir: args.downloads_dir.clone(),
        force_poll_watcher: args.poll_watcher,
        cursor_probe: args.cursor_probe,
        access_log: !args.no_access_log,
        ..ServerConfig::default()
    };
    config::check_bind_policy(config.listen, false, true)?;
    Ok((config, None))
}

/// `token-sync`: runs as root next to an unprivileged driver.
#[cfg(unix)]
async fn token_sync(args: TokenSyncArgs) -> Result<Exit, BoxError> {
    use cua_spacesd_server::token_file::{default_allow_world_readable, TokenFileSync};
    init_tracing();
    let user = cua_spacesd_server::process::spawn::lookup_user(&args.owner)
        .map_err(|e| format!("owner {:?}: {e}", args.owner))?;
    if let Some(dir) = args.to.parent() {
        if !dir.exists() {
            std::fs::create_dir_all(dir)?;
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755))?;
        }
    }
    let sync = TokenFileSync {
        from: args.from.clone(),
        to: args.to.clone(),
        owner: Some((user.uid, user.gid)),
        interval: Duration::from_millis(args.interval_ms.clamp(10, 60_000)),
        allow_world_readable: args.allow_world_readable || default_allow_world_readable(),
    };
    if args.once {
        sync.sync_once(None)?;
        return Ok(Exit::Done);
    }
    tracing::info!(from = %args.from.display(), to = %args.to.display(), owner = %args.owner, "token sync running");
    let stop = tokio_util::sync::CancellationToken::new();
    {
        let stop = stop.clone();
        tokio::spawn(async move {
            wait_for_signal().await;
            stop.cancel();
        });
    }
    sync.run(stop).await;
    Ok(Exit::Done)
}

/// 64 hex characters from the OS RNG (two v4 UUIDs: 244 random bits).
fn random_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

/// Source revision baked in at build time (`CUA_SPACESD_GIT_SHA`, set by
/// build.rs from the build environment or `git rev-parse HEAD`).
pub(crate) const GIT_SHA: &str = env!("CUA_SPACESD_GIT_SHA");

/// `cua-spacesd build-info`.
fn build_info() -> serde_json::Value {
    let tools = platform_tools()
        .map(|t| cua_spacesd_server::services::driver::tool_infos(&t.tools_list()))
        .unwrap_or_default();
    let driver_version = cua_driver_core::protocol::initialize_result()["serverInfo"]["version"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "protocol_version": cua_proto::ENV_PROTOCOL_VERSION,
        "protocol_revision": cua_proto::ENV_PROTOCOL_REVISION,
        "git_sha": GIT_SHA,
        "exe_sha256": cua_driver_core::build_info::exe_sha256().unwrap_or_default(),
        "cua_driver_version": driver_version,
        "tools_sha256": cua_spacesd_client::diagnose::tools_sha256(&tools),
        "tools_count": tools.len(),
        "codecs_compiled": cua_media_codec::probe::compiled_backends()
            .iter()
            .map(|b| format!("{b:?}").to_lowercase())
            .collect::<Vec<_>>(),
        "target": format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
    })
}

/// The cua-driver tool registry for this platform.
pub(crate) fn platform_tools() -> Option<Arc<dyn cua_driver_core::server::ToolProvider>> {
    #[cfg(target_os = "linux")]
    let registry = platform_linux::register_tools();
    #[cfg(target_os = "macos")]
    let registry = platform_macos::register_tools();
    #[cfg(target_os = "windows")]
    let registry = platform_windows::register_tools();
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    return None;
    #[allow(unreachable_code)]
    {
        let registry = Arc::new(registry);
        registry.init_self_weak();
        Some(registry)
    }
}

async fn serve(args: ServeArgs, join: Option<JoinArgs>) -> Result<Exit, BoxError> {
    let env_token = std::env::var("CUA_ENV_TOKEN").ok();
    let (config, token) = build_config(&args, join.is_some(), env_token.as_deref())?;
    if args.print_config {
        let machine_id_file = join.as_ref().map(|j| {
            j.machine_id_file
                .clone()
                .unwrap_or_else(|| config.data_dir.join("id"))
        });
        let printed = serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "protocol_version": cua_proto::ENV_PROTOCOL_VERSION,
            "protocol_revision": cua_proto::ENV_PROTOCOL_REVISION,
            "server": config,
            "driver_registry": !args.no_driver,
            "direct_host_policy": args.direct_host_policy,
            "join": join.as_ref().map(|j| serde_json::json!({
                "relay": j.relay,
                "relay_token_file": j.relay_token_file,
                "host_policy": j.host_policy,
                "relay_jwks": j.relay_jwks,
                "machine_id_file": machine_id_file,
                "heartbeat_secs": j.heartbeat_secs,
            })),
        });
        println!("{}", serde_json::to_string_pretty(&printed)?);
        return Ok(Exit::Done);
    }
    init_tracing();
    let mut config = config;
    // Bind the QUIC media socket before publishing the config so
    // `media_quic_port` is the port actually in use (0 when unavailable).
    let quic_socket = if args.no_desktop || config.media_quic_port == 0 {
        config.media_quic_port = 0;
        None
    } else {
        let address = SocketAddr::new(config.listen.ip(), config.media_quic_port);
        match std::net::UdpSocket::bind(address) {
            Ok(socket) => {
                config.media_quic_port = socket
                    .local_addr()
                    .map(|a| a.port())
                    .unwrap_or(config.media_quic_port);
                Some(socket)
            }
            Err(error) => {
                tracing::warn!(%address, %error, "QUIC media disabled; media stays on the /media WebSocket");
                config.media_quic_port = 0;
                None
            }
        }
    };
    let ctx = ServerContext::new(config, token);
    let mut builder = ServerBuilder::new(ctx.clone());
    // The desktop services (Computer, Windows, Accessibility, Stream,
    // Presence and the /media socket). The provider also supplies the
    // cua-driver registry it drives, so the Driver service and /mcp share one
    // registry (and one cursor overlay) with it.
    let desktop = if args.no_desktop {
        None
    } else {
        match cua_spacesd_desktop::grpc::DesktopServiceProvider::for_host(ctx.clone()) {
            Ok(provider) => Some(provider),
            Err(error) => {
                tracing::warn!(%error, "desktop services unavailable; serving the core services only");
                None
            }
        }
    };
    let desktop_tools = desktop.is_some();
    if let Some(provider) = desktop {
        if let Some(socket) = quic_socket {
            if let Err(error) = provider.start_quic_on(socket) {
                tracing::warn!(%error, "QUIC media listener failed; media stays on the /media WebSocket");
            }
        }
        builder = builder.provider(Arc::new(provider));
    }
    if !args.no_driver && !desktop_tools {
        if let Some(tools) = platform_tools() {
            builder = builder.tools(tools);
        }
    }
    // A host (`join` with a host policy, or `serve` with a direct host
    // policy) serves `HostSpacesService`: it provides Spaces when its
    // policy says so, through its cua daemon.
    let direct_host_policy = match &join {
        Some(_) => None,
        None => args.direct_host_policy.clone(),
    };
    if let Some(policy) = join
        .as_ref()
        .and_then(|j| j.host_policy.clone())
        .or_else(|| direct_host_policy.clone())
    {
        builder = builder.provider(Arc::new(
            cua_spacesd_server::host_spaces::HostSpacesProvider::new(policy),
        ));
    }
    // SystemService.Diagnose runs the doctor against this server.
    builder = builder.diagnoser(Arc::new(cua_spacesd_doctor::ServerDiagnoser {
        manifest_path: None,
        artifacts_dir: None,
    }));
    let server = builder.build();
    let listener = cua_spacesd_server::bind(&ctx).await?;
    let addr = listener.local_addr()?;
    tracing::info!(
        %addr,
        token = %ctx.config().token_source,
        stubbed = ?server.manifest().stubbed_services,
        "cua-spacesd {} serving",
        env!("CARGO_PKG_VERSION")
    );

    let shutdown = ctx.shutdown_token();
    // A direct host starts its cua daemon, which reopens the ports it
    // forwards to the Spaces it provides.
    if let Some(policy) = direct_host_policy {
        tokio::spawn(cua_spacesd_server::host_spaces::wake_daemon(policy));
    }
    {
        let ctx = ctx.clone();
        tokio::spawn(async move {
            wait_for_signal().await;
            tracing::info!("signal received; shutting down");
            ctx.request_shutdown(false);
        });
    }
    if let Some(join) = join {
        let id_file = join
            .machine_id_file
            .clone()
            .unwrap_or_else(|| ctx.config().data_dir.join("id"));
        if let Some(id) = join
            .machine_id
            .as_deref()
            .map(str::trim)
            .filter(|i| !i.is_empty())
        {
            if !cua_relay::valid_machine_id(id) {
                return Err(format!("CUA_ENV_MACHINE_ID: invalid machine id {id:?}").into());
            }
            write_private_file(&id_file, format!("{id}\n").as_bytes())?;
        }
        let machine_id = cua_relay::client::load_or_create_machine_id(&id_file)?;
        let host_policy = match join
            .host_policy_json
            .as_deref()
            .filter(|j| !j.trim().is_empty())
        {
            Some(json) => {
                serde_json::from_str::<serde_json::Value>(json)
                    .map_err(|e| format!("CUA_HOST_POLICY_JSON: {e}"))?;
                let path = ctx.config().data_dir.join("host-policy.json");
                write_private_file(&path, json.as_bytes())?;
                Some(path)
            }
            None => join.host_policy.clone(),
        };
        let mut join_config = cua_relay::client::JoinConfig::new(
            join.relay.clone(),
            join.relay_token.clone().unwrap_or_default(),
            machine_id.clone(),
            addr,
        );
        join_config.version = env!("CARGO_PKG_VERSION").into();
        join_config.heartbeat = Duration::from_secs(join.heartbeat_secs.max(2));
        join_config.relay_token_file = join.relay_token_file.clone();
        if let Some(pinned) = &join.relay_jwks {
            join_config.account.pin_jwks_file(pinned)?;
        }
        if let Some(json) = join
            .relay_jwks_json
            .as_deref()
            .filter(|j| !j.trim().is_empty())
        {
            join_config
                .account
                .pin_jwks_json(json)
                .map_err(|e| format!("CUA_RELAY_JWKS_JSON: {e}"))?;
        }
        // Account mode: relay-signed principal assertions authorize
        // callers (in addition to the local token).
        ctx.auth().set_external(Arc::new(
            cua_spacesd_server::relay_account::RelayAssertionAuth::new(
                machine_id.clone(),
                join_config.account.clone(),
                host_policy,
            ),
        ));
        ctx.mark_joined_at_start();
        tracing::info!(relay = %join.relay, machine = %machine_id, "joining relay; clients connect to <relay>/m/{machine_id}");
        tokio::spawn(cua_relay::client::run(join_config, shutdown.clone()));
    }
    server.serve(listener).await?;
    Ok(if ctx.restart_requested() {
        Exit::Restart
    } else {
        Exit::Done
    })
}

async fn wait_for_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
        tokio::select! {
            _ = term.recv() => {}
            _ = tokio::signal::ctrl_c() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// Re-executes the current binary with the same arguments.
fn restart() {
    let exe = std::env::current_exe().expect("current executable");
    let args: Vec<String> = std::env::args().skip(1).collect();
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        let error = std::process::Command::new(exe).args(args).exec();
        eprintln!("cua-spacesd: re-exec failed: {error}");
        std::process::exit(1);
    }
    #[cfg(not(unix))]
    {
        match std::process::Command::new(exe).args(args).spawn() {
            Ok(_) => std::process::exit(0),
            Err(error) => {
                eprintln!("cua-spacesd: restart failed: {error}");
                std::process::exit(1);
            }
        }
    }
}

/// Writes `bytes` to `path` readable by this user only (0600 on Unix),
/// creating its directory.
fn write_private_file(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    {
        use std::io::Write as _;
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            o.mode(0o600);
        }
        let mut f = o.open(&tmp)?;
        f.write_all(bytes)?;
    }
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(std::iter::once("cua-spacesd").chain(args.iter().copied())).unwrap()
    }

    #[test]
    fn default_command_is_serve_with_flags() {
        let cli = parse(&["--port", "4000", "--print-config"]);
        assert!(cli.command.is_none());
        assert_eq!(cli.serve.port, 4000);
        assert!(cli.serve.print_config);
    }

    #[test]
    fn join_binds_loopback_with_a_token() {
        let dir = tempfile::tempdir().unwrap();
        let cli = parse(&[
            "join",
            "--relay",
            "wss://relay.example",
            "--relay-token",
            "r",
            "--token-file",
            dir.path().join("missing").to_str().unwrap(),
        ]);
        let Some(Command::Join(join)) = cli.command else {
            panic!()
        };
        // No token: a random local one is generated.
        let (config, token) = build_config(&join.serve, true, None).unwrap();
        assert!(config.listen.ip().is_loopback());
        assert_eq!(token.as_deref().map(str::len), Some(64));
        assert_eq!(config.token_source, "generated (join)");
        let (config, token) = build_config(&join.serve, true, Some("t")).unwrap();
        assert!(config.listen.ip().is_loopback());
        assert_eq!(token.as_deref(), Some("t"));
    }

    #[test]
    fn serve_takes_a_direct_host_policy() {
        let cli = parse(&["--direct-host-policy", "/x/host/host.json"]);
        assert_eq!(
            cli.serve.direct_host_policy.as_deref(),
            Some(std::path::Path::new("/x/host/host.json"))
        );
        assert_eq!(parse(&[]).serve.direct_host_policy, None);
    }

    fn public_listen(dir: &tempfile::TempDir) -> ServeArgs {
        let mut args = parse(&["--listen", "0.0.0.0:3211"]).serve;
        args.token_file = dir.path().join("env-token");
        args
    }

    #[test]
    fn non_loopback_without_any_token_is_refused_unless_bootstrapping() {
        let dir = tempfile::tempdir().unwrap();
        let mut args = public_listen(&dir);
        let error = build_config(&args, false, None).unwrap_err().to_string();
        assert!(error.contains("without a token"), "{error}");
        // Empty values do not count as a token.
        std::fs::write(&args.token_file, "\n").unwrap();
        assert!(build_config(&args, false, Some("  ")).is_err());
        args.insecure_bootstrap = true;
        let (config, _) = build_config(&args, false, None).unwrap();
        assert_eq!(
            config.bootstrap_token_file.as_deref(),
            Some(args.token_file.as_path())
        );
        // With a token there is nothing to bootstrap or persist.
        let (config, _) = build_config(&args, false, Some("t")).unwrap();
        assert!(config.bootstrap_token_file.is_none());
    }

    #[test]
    fn non_loopback_accepts_a_token_from_the_flag() {
        let dir = tempfile::tempdir().unwrap();
        let mut args = public_listen(&dir);
        args.token = Some("flag-token".into());
        let (config, token) = build_config(&args, false, None).unwrap();
        assert_eq!(config.listen.to_string(), "0.0.0.0:3211");
        assert_eq!(
            (token.as_deref(), config.token_source.as_str()),
            (Some("flag-token"), "flag")
        );
    }

    #[test]
    fn non_loopback_accepts_a_token_from_cua_env_token() {
        let dir = tempfile::tempdir().unwrap();
        let args = public_listen(&dir);
        let (config, token) = build_config(&args, false, Some("env-token")).unwrap();
        assert_eq!(token.as_deref(), Some("env-token"));
        assert_eq!(config.token_source, "env:CUA_ENV_TOKEN");
    }

    #[test]
    fn non_loopback_accepts_a_token_from_the_token_file() {
        let dir = tempfile::tempdir().unwrap();
        let args = public_listen(&dir);
        std::fs::write(&args.token_file, "file-token\n").unwrap();
        let (config, token) = build_config(&args, false, None).unwrap();
        assert_eq!(token.as_deref(), Some("file-token"));
        assert!(
            config.token_source.starts_with("file:"),
            "{}",
            config.token_source
        );
    }

    #[test]
    fn token_file_defaults_to_run_cua_env_token_and_listen_defaults_follow_the_token() {
        let args = parse(&[]).serve;
        assert_eq!(
            args.token_file,
            std::path::PathBuf::from("/run/cua/env-token")
        );
        let dir = tempfile::tempdir().unwrap();
        let mut args = args;
        args.token_file = dir.path().join("env-token");
        let (open, _) = build_config(&args, false, None).unwrap();
        assert_eq!(open.listen.to_string(), "127.0.0.1:3211");
        let (public, _) = build_config(&args, false, Some("t")).unwrap();
        assert_eq!(public.listen.to_string(), "0.0.0.0:3211");
    }

    #[test]
    fn await_token_file_binds_public_without_a_token() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("env-token");
        let args = parse(&["--await-token-file", "--token-file", file.to_str().unwrap()]).serve;
        let (config, token) = build_config(&args, false, None).unwrap();
        assert!(token.is_none());
        assert_eq!(config.listen.to_string(), "0.0.0.0:3211");
        assert_eq!(config.await_token_file.as_deref(), Some(file.as_path()));
        assert!(config.token_source.starts_with("await-file:"));
        assert_eq!(config.token_poll_interval, Duration::from_millis(500));
        // Even a token already in the file is left to the server's watcher.
        std::fs::write(&file, "0123456789abcdef0123").unwrap();
        assert!(build_config(&args, false, None).unwrap().1.is_none());
        // Empty CUA_ENV_TOKEN is fine; a real one is a configuration error.
        assert!(build_config(&args, false, Some(" ")).is_ok());
        let error = build_config(&args, false, Some("t"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("--await-token-file"), "{error}");
        assert!(build_config(&args, true, None).is_err(), "join");
        // Flag conflicts.
        assert!(Cli::try_parse_from(["x", "--await-token-file", "--token", "t"]).is_err());
        assert!(Cli::try_parse_from(["x", "--await-token-file", "--insecure-bootstrap"]).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn token_sync_parses() {
        let cli = parse(&["token-sync", "--to", "/run/cua-env/env-token", "--once"]);
        let Some(Command::TokenSync(args)) = cli.command else {
            panic!()
        };
        assert_eq!(args.from, std::path::PathBuf::from("/run/cua/env-token"));
        assert_eq!(args.owner, "cua");
        assert!(args.once);
    }

    #[test]
    fn doctor_parses() {
        let cli = parse(&[
            "doctor",
            "--strict",
            "--format",
            "junit",
            "--effects",
            "virtual",
            "--only",
            "stream,auth",
            "--skip",
            "audio",
            "--timeout",
            "90",
        ]);
        let Some(Command::Doctor(args)) = cli.command else {
            panic!()
        };
        assert!(args.strict);
        assert_eq!(args.format(), doctor::Format::Junit);
        assert_eq!(args.only, ["stream", "auth"]);
        assert_eq!(args.skip, ["audio"]);
        assert_eq!(args.timeout, 90);
        let cli = parse(&["doctor", "--json"]);
        let Some(Command::Doctor(args)) = cli.command else {
            panic!()
        };
        assert_eq!(args.format(), doctor::Format::Json);
        assert!(Cli::try_parse_from(["x", "doctor", "--effects", "any"]).is_err());
        let cli = parse(&[
            "doctor",
            "--expect",
            "cua-driver=git:abcdef1",
            "--expect",
            "cua-spacesd=0.1.0",
        ]);
        let Some(Command::Doctor(args)) = cli.command else {
            panic!()
        };
        assert_eq!(args.expect, ["cua-driver=git:abcdef1", "cua-spacesd=0.1.0"]);
    }

    #[test]
    fn build_info_names_the_build() {
        let info = build_info();
        assert_eq!(info["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(info["protocol_revision"], cua_proto::ENV_PROTOCOL_REVISION);
        assert_eq!(info["tools_sha256"].as_str().unwrap().len(), 64);
        assert!(info["codecs_compiled"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c == "openh264"));
    }

    #[test]
    fn legacy_passes_arguments_through() {
        let cli = parse(&["legacy", "--socket", "/tmp/x.sock"]);
        let Some(Command::Legacy { args }) = cli.command else {
            panic!()
        };
        assert_eq!(args, ["--socket", "/tmp/x.sock"]);
    }
}
