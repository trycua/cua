// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The pre-consolidation daemon modes (Unix-socket RCDP, the Fleet-facing
//! WebSocket, QUIC, Tailscale remote shares). Reachable as
//! `cua-spacesd legacy …` until the desktop provider serves the same
//! functionality through `cua-spacesd-server`.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use cua_media_protocol::SessionPolicy;
use cua_spacesd::presence::PresenceHub;
use cua_spacesd::ws::WsDaemon;
use cua_spacesd::{RemoteShareConfig, DEFAULT_PORT};
use cua_spacesd_desktop::{ApplicationSelector, CuaProviderBundle, CuaProviderConfig};
use cua_spacesd_session::ServerRuntime;

/// Give AppKit the OS main thread and run the daemon on a spawned thread.
///
/// The cursor overlay that draws remote and agent cursors on the *origin*
/// desktop is an AppKit `NSWindow` with a run loop, and
/// `cursor::overlay::run_on_main_thread()` must own the OS main thread and
/// never returns. While `main` was `#[tokio::main]` the main thread was the
/// tokio runtime driver, so that call had nowhere to go and the origin-desktop
/// overlay could not exist on macOS at all.
///
/// This is the shape cua-driver's own `main.rs` uses for its daemon (build the
/// driver, which initializes the overlay channel; spawn the async work on a
/// plain thread; that thread owns process exit; park main in the AppKit loop).
/// Following that precedent rather than inventing one matters because the
/// ordering is load-bearing: the overlay channel must be initialized *before*
/// main parks, or `run_on_main_thread` finds no receiver and parks forever
/// without rendering. Here `run()` constructs the provider bundle (and with it
/// the driver registry that calls `overlay::init`) before it ever serves, and
/// the ready handshake below blocks main until that has happened.
///
/// `run_on_main_thread` self-guards: with the overlay disabled, or on a host
/// with no Window Server session, it parks the main thread instead of touching
/// AppKit (`sharedApplication` aborts without a GUI session). So a headless
/// cua-spacesd keeps serving exactly as before -- it simply parks main rather than
/// running it.
#[cfg(target_os = "macos")]
pub fn main(args: Vec<String>) {
    std::thread::Builder::new()
        .name("cua-spacesd-serve".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    eprintln!("cua-spacesd: cannot create the async runtime: {error}");
                    std::process::exit(1);
                }
            };
            let code = match runtime.block_on(run(args)) {
                Ok(()) => 0,
                Err(error) => {
                    eprintln!("cua-spacesd: {error}");
                    1
                }
            };
            // The AppKit loop is process-long and never returns, so the daemon
            // thread owns the exit code.
            std::process::exit(code);
        })
        .expect("spawn the cua-spacesd serve thread");

    // Block until the provider bundle (and with it `cursor::overlay::init`)
    // exists. Not a sleep and not a poll: the provider signals this from the
    // same function that initializes the overlay, so the ordering holds in
    // every daemon mode. Entering the AppKit loop first would take the command
    // receiver before it was installed and park forever without rendering.
    cua_spacesd_desktop::wait_for_overlay_ready();
    platform_macos::cursor::overlay::run_on_main_thread();
}

/// Platforms whose overlay needs no main-thread dispatch. Windows and Linux
/// overlays spawn their own loop thread from inside registry construction
/// (`overlay::run_on_thread()`), so the daemon keeps the ordinary shape.
#[cfg(not(target_os = "macos"))]
pub fn main(args: Vec<String>) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    if let Err(error) = runtime.block_on(run(args)) {
        eprintln!("cua-spacesd legacy: {error}");
        std::process::exit(1);
    }
}

/// The daemon proper.
async fn run(args: Vec<String>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let diagnostics = cua_logging::init("host")?;
    tracing::info!(log_path = %diagnostics.path().display(), "persistent diagnostics enabled");

    let mut arguments = args.into_iter();
    let mut socket: Option<PathBuf> = None;
    let mut listen: Option<SocketAddr> = None;
    let mut quic_listen: Option<SocketAddr> = None;
    let mut quic_identity: Option<PathBuf> = None;
    let mut apps_path: Option<PathBuf> = None;
    let mut token: Option<String> = None;
    let mut remote_listen: Option<SocketAddr> = None;
    let mut share_name = None;
    let mut application_id = None;
    let mut allowed_users = Vec::new();
    let mut required_capability = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--socket" => {
                socket = Some(
                    arguments
                        .next()
                        .map(PathBuf::from)
                        .ok_or("--socket requires a path")?,
                );
            }
            "--listen" => {
                listen = Some(
                    arguments
                        .next()
                        .ok_or("--listen requires an address like 127.0.0.1:3211")?
                        .parse()?,
                );
            }
            "--quic-listen" => {
                quic_listen = Some(
                    arguments
                        .next()
                        .ok_or("--quic-listen requires an address like 0.0.0.0:8766")?
                        .parse()?,
                );
            }
            "--quic-identity" => {
                quic_identity = Some(
                    arguments
                        .next()
                        .map(PathBuf::from)
                        .ok_or("--quic-identity requires a directory")?,
                );
            }
            "--apps" => {
                apps_path = Some(
                    arguments
                        .next()
                        .map(PathBuf::from)
                        .ok_or("--apps requires a JSON file path")?,
                );
            }
            "--token" => token = Some(arguments.next().ok_or("--token requires a value")?),
            "--remote-listen" => {
                remote_listen = Some(
                    arguments
                        .next()
                        .ok_or("--remote-listen requires an address")?
                        .parse()?,
                );
            }
            "--share" => share_name = Some(arguments.next().ok_or("--share requires a name")?),
            "--application-id" => {
                application_id = Some(
                    arguments
                        .next()
                        .ok_or("--application-id requires a value")?,
                );
            }
            "--allow-tailscale-user" => allowed_users.push(
                arguments
                    .next()
                    .ok_or("--allow-tailscale-user requires a login")?,
            ),
            "--require-tailscale-capability" => {
                required_capability = Some(
                    arguments
                        .next()
                        .ok_or("--require-tailscale-capability requires a name")?,
                );
            }
            "--help" | "-h" => {
                print_help();
                return Ok(());
            }
            "--version" | "-V" => {
                println!("cua-spacesd {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }

    if let Some(address) = remote_listen {
        if socket.is_some()
            || listen.is_some()
            || quic_listen.is_some()
            || quic_identity.is_some()
            || apps_path.is_some()
            || token.is_some()
        {
            return Err(
                "--remote-listen is exclusive with --socket, --listen, --apps, and --token".into(),
            );
        }
        let share_name = share_name.ok_or("remote mode requires --share")?;
        let application_id = application_id.ok_or("remote mode requires --application-id")?;
        let providers = CuaProviderBundle::with_config(CuaProviderConfig {
            application: Some(ApplicationSelector::Id(application_id.clone())),
            agent_cursor: None,
            encoder: None,
        })?;
        let runtime = Arc::new(
            ServerRuntime::new_with_policy_ceiling_and_interactive_input(
                providers.targets,
                providers.captures,
                providers.actions,
                providers.accessibility,
                providers.geometry,
                providers.inputs,
                SessionPolicy::AllowActivation,
            ),
        );
        let config = RemoteShareConfig {
            name: share_name.clone(),
            application_id,
            allowed_users,
            required_capability,
        };
        config.validate()?;
        eprintln!(
            "cua-spacesd serving share {share_name:?} on {address} with foreground input enabled"
        );
        return cua_spacesd::serve_remote(address, runtime, config).await;
    }

    if share_name.is_some() || !allowed_users.is_empty() || required_capability.is_some() {
        return Err("Tailscale share options require --remote-listen".into());
    }

    // Linux guests default to a Fleet-facing WebSocket. The daemon sits behind
    // the Fleet gateway (wss://run.cua.ai/api/svc/{ns}/{sandbox}-rcdp/ws), which
    // authenticates at the edge, so anonymous connections are accepted unless a
    // shared token is configured. Any explicit transport flag opts back out into
    // the shared CLI handling below.
    #[cfg(target_os = "linux")]
    if socket.is_none() && listen.is_none() && quic_listen.is_none() {
        return serve_linux_default(apps_path, application_id).await;
    }

    if quic_identity.is_some() && quic_listen.is_none() {
        return Err("--quic-identity requires --quic-listen".into());
    }
    if quic_listen.is_some() && quic_identity.is_none() {
        return Err("--quic-listen requires --quic-identity".into());
    }

    let network_requested = listen.is_some()
        || quic_listen.is_some()
        || apps_path.is_some()
        || token.is_some()
        || application_id.is_some()
        || cfg!(not(unix));
    if network_requested {
        let apps = match &apps_path {
            Some(path) => cua_spacesd::presence::load_apps(path)?,
            None => Vec::new(),
        };
        // The agent-cursor sink: window actions push a CursorState here, drained
        // below into the presence broadcast so viewers render the agent cursor.
        let (agent_cursor_tx, mut agent_cursor_rx) = tokio::sync::mpsc::unbounded_channel();
        let providers = CuaProviderBundle::with_config(CuaProviderConfig {
            application: application_id.clone().map(ApplicationSelector::Id),
            agent_cursor: Some(agent_cursor_tx),
            encoder: None,
        })?;
        let targets = providers.targets.clone();
        let overlay = providers.presence.clone();
        let mcp_registry = providers.registry.clone();
        let runtime = Arc::new(ServerRuntime::new_with_interactive_input(
            providers.targets,
            providers.captures,
            providers.actions,
            providers.accessibility,
            providers.geometry,
            providers.inputs,
        ));
        let presence = Arc::new(PresenceHub::new(
            providers.presence,
            providers.launcher,
            apps,
        ));
        cua_spacesd::presence::spawn_desktop_watchers(presence.clone(), targets, overlay);

        // Drain agent-cursor updates into the presence broadcast so every viewer
        // renders the CUA agent's cursor as an overlay (no video of it is sent).
        {
            let presence = presence.clone();
            tokio::spawn(async move {
                while let Some(cursor) = agent_cursor_rx.recv().await {
                    presence.broadcast_all(cua_media_protocol::ServerMessage::RemoteCursor(cursor));
                }
            });
        }

        // Serve cua-driver's own MCP over the embedded registry so agents drive
        // THIS daemon's cua-driver (its cursor moves fire the hook above),
        // instead of a separate driver + a routing layer. Same bind exposure and
        // token gate as the rcdp WS.
        {
            let registry = mcp_registry;
            let mcp_token = token.clone();
            // Same interface as the rcdp listener (loopback unless one was
            // requested); a non-loopback bind requires the token.
            let mcp_ip = listen
                .or(quic_listen)
                .map(|address| address.ip())
                .unwrap_or(std::net::IpAddr::from([127, 0, 0, 1]));
            let mcp_addr = std::net::SocketAddr::new(mcp_ip, 8801);
            tokio::spawn(async move {
                if let Err(error) =
                    cua_spacesd::mcp_serve::serve_mcp(mcp_addr, registry, mcp_token).await
                {
                    tracing::error!(%error, "cua-driver MCP transport failed");
                }
            });
        }

        #[cfg(unix)]
        if let Some(socket) = socket {
            let runtime = runtime.clone();
            tokio::spawn(async move {
                if let Err(error) = cua_spacesd::serve_unix(&socket, runtime).await {
                    tracing::error!(%error, "unix listener failed");
                }
            });
        }
        #[cfg(not(unix))]
        if socket.is_some() {
            return Err("--socket is only supported on Unix".into());
        }

        let listen = if listen.is_some() || quic_listen.is_none() {
            Some(listen.unwrap_or_else(|| SocketAddr::from(([127, 0, 0, 1], DEFAULT_PORT))))
        } else {
            None
        };
        if listen.is_some_and(|address| !address.ip().is_loopback())
            && token.as_deref().is_none_or(str::is_empty)
        {
            return Err("a non-loopback --listen address requires --token".into());
        }
        let daemon = Arc::new(WsDaemon {
            runtime,
            presence,
            token,
        });
        if let Some(listen) = listen {
            if let Some(application_id) = &application_id {
                eprintln!(
                    "cua-spacesd listening on ws://{listen} for application {application_id:?} with foreground input enabled"
                );
            } else {
                eprintln!("cua-spacesd listening on ws://{listen}");
            }
        }
        return match (listen, quic_listen, quic_identity) {
            (Some(websocket), Some(quic), Some(identity)) => {
                tokio::try_join!(
                    cua_spacesd::ws::serve_ws(websocket, daemon.clone()),
                    cua_spacesd::quic::serve_quic(quic, daemon, identity),
                )?;
                Ok(())
            }
            (Some(websocket), None, None) => cua_spacesd::ws::serve_ws(websocket, daemon).await,
            (None, Some(quic), Some(identity)) => {
                cua_spacesd::quic::serve_quic(quic, daemon, identity).await
            }
            _ => unreachable!("QUIC arguments were validated above"),
        };
    }

    #[cfg(unix)]
    {
        let socket = socket.unwrap_or_else(|| PathBuf::from("/tmp/cua-spacesd.sock"));
        let providers = CuaProviderBundle::new()?;
        let runtime = Arc::new(ServerRuntime::new_with_interactive_input(
            providers.targets,
            providers.captures,
            providers.actions,
            providers.accessibility,
            providers.geometry,
            providers.inputs,
        ));
        eprintln!("cua-spacesd listening on {}", socket.display());
        cua_spacesd::serve_unix(&socket, runtime).await
    }
    #[cfg(not(unix))]
    unreachable!("non-Unix platforms select the WebSocket transport")
}

/// Serve the Fleet-facing WebSocket used on Linux guests.
///
/// Binds `0.0.0.0:$CUA_ENV_PORT` (default `DEFAULT_PORT`, 3211). The `tokio-tungstenite` acceptor
/// upgrades any request path, so clients reach it at `/ws` (or root) through the
/// gateway. Authentication: a non-empty `CUA_ENV_TOKEN` is required from clients;
/// otherwise anonymous connections are accepted unless `CUA_ENV_ALLOW_ANONYMOUS`
/// is explicitly disabled.
#[cfg(target_os = "linux")]
async fn serve_linux_default(
    apps_path: Option<PathBuf>,
    application_id: Option<String>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let port: u16 = std::env::var("CUA_ENV_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_PORT);
    let allow_anonymous = std::env::var("CUA_ENV_ALLOW_ANONYMOUS")
        .map(|value| value != "0" && !value.eq_ignore_ascii_case("false"))
        .unwrap_or(true);
    let env_token = std::env::var("CUA_ENV_TOKEN")
        .ok()
        .filter(|t| !t.is_empty());
    let token = match env_token {
        Some(token) => Some(token),
        None if allow_anonymous => None,
        None => return Err("CUA_ENV_ALLOW_ANONYMOUS=0 requires a non-empty CUA_ENV_TOKEN".into()),
    };
    let address: SocketAddr = format!("0.0.0.0:{port}").parse()?;

    let apps = match &apps_path {
        Some(path) => cua_spacesd::presence::load_apps(path)?,
        None => Vec::new(),
    };
    let providers = match &application_id {
        Some(application_id) => CuaProviderBundle::with_config(CuaProviderConfig {
            application: Some(ApplicationSelector::Id(application_id.clone())),
            agent_cursor: None,
            encoder: None,
        })?,
        None => CuaProviderBundle::new()?,
    };
    let targets = providers.targets.clone();
    let overlay = providers.presence.clone();
    let runtime = Arc::new(ServerRuntime::new_with_interactive_input(
        providers.targets,
        providers.captures,
        providers.actions,
        providers.accessibility,
        providers.geometry,
        providers.inputs,
    ));
    let presence = Arc::new(PresenceHub::new(
        providers.presence,
        providers.launcher,
        apps,
    ));
    cua_spacesd::presence::spawn_desktop_watchers(presence.clone(), targets, overlay);

    let daemon = Arc::new(WsDaemon {
        runtime,
        presence,
        token: token.clone(),
    });
    eprintln!(
        "cua-spacesd listening on ws://{address}/ws ({})",
        if token.is_some() {
            "shared-token auth"
        } else {
            "anonymous"
        }
    );
    cua_spacesd::ws::serve_ws(address, daemon).await
}

fn print_help() {
    println!(concat!(
        "Legacy window-streaming daemon modes (superseded by `cua-spacesd serve`).\n\n",
        "Usage:\n",
        "  cua-spacesd legacy [--socket PATH]\n",
        "  cua-spacesd --listen 127.0.0.1:3211 [--apps APPS.json] [--token TOKEN] ",
        "[--socket PATH]\n",
        "  cua-spacesd --listen ADDRESS --application-id ID --token TOKEN\n",
        "  cua-spacesd --quic-listen ADDRESS --quic-identity DIRECTORY ",
        "--application-id ID --token TOKEN [--listen ADDRESS]\n",
        "  cua-spacesd --remote-listen 127.0.0.1:7443 --share NAME --application-id ID \\\n",
        "    [--allow-tailscale-user LOGIN] [--require-tailscale-capability NAME]\n\n",
        "Both app-scoped modes enable foreground input. Non-loopback direct listeners require a token. ",
        "QUIC keeps control reliable and carries replaceable video in datagrams."
    ));
}
