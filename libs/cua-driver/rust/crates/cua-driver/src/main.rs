//! cua-driver-rs — cross-platform background computer-use automation daemon.
//!
//! Runs as a MCP JSON-RPC 2.0 server over stdio.  The platform backend is
//! selected at compile time via conditional compilation.
//!
//! Extra CLI flags (consumed here, not by MCP):
//!   --cursor-icon  <path.svg|.ico|.png>   custom cursor shape
//!   --cursor-id    <id>                   multi-cursor instance id
//!   --cursor-palette <name>               named colour palette
//!   --no-overlay                          start with overlay disabled
//!   --glide-ms     <f64>                  glide duration override
//!   --dwell-ms     <f64>                  post-click dwell override
//!   --idle-hide-ms <f64>                  idle-hide timeout override
//!
//! ## macOS threading model
//!
//! AppKit requires the main thread.  On macOS the entry-point is a plain
//! `fn main()` that:
//!  1. Initialises the cursor overlay channel synchronously (so
//!     `run_on_main_thread` always finds it ready).
//!  2. Spawns a background tokio thread for the MCP server.
//!  3. Calls `platform_macos::cursor::overlay::run_on_main_thread()` which
//!     starts `NSApplication.run()` and the 60 fps render loop.
//!
//! On all other platforms `#[tokio::main]` is used directly.

mod autostart;
mod bundle;
mod cli;
mod doctor;
mod mcp_http;
mod proxy;
mod responsibility;
mod serve;
mod skills;
mod telemetry;
mod check_update_tool;
mod updater;
mod version_check;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Set by the `Command::Mcp` arm when `--claude-code-computer-use-compat`
/// is on argv. Read by `build_registry` / `build_registry_no_cursor` to
/// pick which `screenshot` tool variant to register. Static keeps the
/// thread of dependency arrows pointed away from the platform crates —
/// they take `compat: bool` directly, but the binary crate decides what
/// to pass without making every Command variant carry the flag.
static CLAUDE_CODE_COMPAT: AtomicBool = AtomicBool::new(false);

fn init_logging() {
    use tracing_subscriber::EnvFilter;
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::from_env("CUA_LOG")
                .add_directive(tracing::Level::WARN.into()),
        )
        .init();
}

/// Fire the per-entry-point telemetry event (e.g. `cua_driver_mcp`,
/// `cua_driver_api_click`). Respects the opt-out env var — no-op when
/// telemetry is disabled. Always returns immediately: the actual POST
/// happens on a background thread or tokio task.
///
/// Mirrors Swift's `TelemetryClient.shared.record(event:)` call at the
/// top of `CuaDriverCommand.main()`. The install ping is *not* emitted
/// here — that's the dedicated `telemetry install-event` subcommand
/// fired exactly once by the post-install script.
fn emit_entry_telemetry(command: &cli::Command) {
    if let Some(event_name) = cli::telemetry_entry_event(command) {
        // Attach the bounded, anonymous entry-event properties
        // (capture_mode / mcp_client / display_server). The helper returns
        // `None` when a command contributes none, matching the prior
        // `capture(.., None)` behaviour.
        let properties = telemetry::entry_event_properties(command);
        telemetry::capture(&event_name, properties);
    }
}

/// Wire up the experimental picture-in-picture preview window.
///
/// Called from every long-running entry point (Serve and Mcp on all
/// platforms; the `Call` arm intentionally skips PiP since the
/// per-call binaries don't keep an AppKit/event loop alive long
/// enough to be useful).
///
/// No-op when `--experimental-pip` is not on argv. On Windows / Linux
/// the factory returns "not yet implemented" — we log and continue
/// without a window so the rest of the daemon keeps working.
fn maybe_init_pip() {
    let cfg = match pip_preview::default_config_path() {
        Some(p) => pip_preview::PipConfig::from_args_and_file(&p),
        None => pip_preview::PipConfig::from_args(),
    };
    if !cfg.enabled {
        return;
    }

    // Register the platform factory. The set is idempotent so multiple
    // entry points calling this in the same process is safe.
    #[cfg(target_os = "macos")]
    pip_preview::set_pip_backend_factory(
        Box::new(platform_macos::pip::MacosPipBackendFactory),
    );
    #[cfg(target_os = "windows")]
    pip_preview::set_pip_backend_factory(
        Box::new(platform_windows::pip::WindowsPipBackendFactory),
    );
    #[cfg(target_os = "linux")]
    pip_preview::set_pip_backend_factory(
        Box::new(platform_linux::pip::LinuxPipBackendFactory),
    );

    match pip_preview::start_pip(&cfg) {
        Ok(backend) => {
            // Bridge: when the tool dispatcher in cua-driver-core wants
            // to push a frame, forward to the live backend handle.
            // We move the Box into a static Mutex<Option<...>> so the
            // closure can re-borrow on every call without taking
            // ownership of the trait object.
            use std::sync::Mutex as StdMutex;
            static BACKEND: std::sync::OnceLock<StdMutex<Option<Box<dyn pip_preview::PipBackend>>>> =
                std::sync::OnceLock::new();
            let _ = BACKEND.set(StdMutex::new(Some(backend)));
            cua_driver_core::pip_hook::set_pip_push_fn(|frame| {
                if let Some(slot) = BACKEND.get() {
                    if let Some(b) = slot.lock().unwrap().as_ref() {
                        b.push_frame(pip_preview::PipFrame {
                            png_bytes: frame.png_bytes,
                            action_label: frame.action_label,
                            timestamp_ms: frame.timestamp_ms,
                        });
                    }
                }
            });
            eprintln!(
                "⚗️  PiP preview enabled (experimental — macOS only today; \
                 see https://github.com/trycua/cua/issues for follow-up)"
            );
        }
        Err(e) => {
            eprintln!("⚗️  PiP preview requested but unavailable: {e}");
        }
    }
}

// ── Registry helpers (macOS) ─────────────────────────────────────────────

/// Build the macOS tool registry and inject the platform-agnostic
/// `check_for_update` tool. Wrapper lives in the binary crate so the
/// `cua-driver-core` graph (shared with every `platform-*` crate) stays
/// free of the `ureq` + rustls + ring deps that the check tool needs.
#[cfg(target_os = "macos")]
fn build_macos_registry() -> cua_driver_core::tool::ToolRegistry {
    let mut r = platform_macos::register_tools();
    check_update_tool::register_into(&mut r);
    r
}

#[cfg(target_os = "macos")]
fn build_macos_registry_with_compat(compat: bool) -> cua_driver_core::tool::ToolRegistry {
    let mut r = platform_macos::register_tools_with_compat(compat);
    check_update_tool::register_into(&mut r);
    r
}

// ── macOS entry-point ─────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn main() {
    init_logging();

    // ── CLI subcommand dispatch ──────────────────────────────────────────────
    // Handled before AppKit init so `list-tools` / `describe` / `call` exit
    // cleanly without starting the overlay or NSApplication.
    let command = cli::parse_command();
    emit_entry_telemetry(&command);
    match command {
        cli::Command::TelemetryInstallEvent => {
            // Synchronous install ping (see `telemetry::capture_install`).
            // Blocks on the POST so the `.installation_recorded` marker
            // is only written on HTTP success — failed POST means next
            // launch retries. Installer script already runs us in the
            // background via `&`, so blocking here is fine.
            telemetry::capture_install();
            return;
        }
        cli::Command::ListTools => {
            let reg = Arc::new(build_macos_registry());
            cli::run_list_tools(&reg);
            return;
        }
        cli::Command::Describe(name) => {
            let reg = Arc::new(build_macos_registry());
            cli::run_describe(&reg, &name);
            return;
        }
        cli::Command::McpConfig { client } => {
            cli::run_mcp_config(client.as_deref());
            return;
        }
        cli::Command::Manifest { pretty } => {
            // Surface 8: machine-readable CLI manifest. Read-only — no
            // registry build needed, no daemon contact.
            cli::run_manifest(pretty);
            return;
        }
        cli::Command::Call { tool, json_args, screenshot_out_file, socket } => {
            // Register callbacks (needed if the tool does screenshots/recording).
            cua_driver_core::recording::set_screenshot_fn(|window_id, pid| {
                if let Some(wid) = window_id {
                    platform_macos::capture::screenshot_window_bytes(wid as u32).ok()
                } else if let Some(p) = pid {
                    platform_macos::windows::resolve_main_window_id(p as i32).ok()
                        .and_then(|wid| platform_macos::capture::screenshot_window_bytes(wid).ok())
                } else {
                    platform_macos::capture::screenshot_display_bytes().ok()
                }
            });
            cua_driver_core::recording::set_click_marker_fn(|png_bytes, cx, cy| {
                platform_macos::capture::crosshair_png_bytes(png_bytes, cx, cy).ok()
            });
            cua_driver_core::recording::set_ax_snapshot_fn(|window_id, pid| {
                platform_macos::recording_hooks::app_state_json_for(window_id, pid)
            });
            cua_driver_core::recording::set_element_bounds_fn(|wid, pid, idx| {
                platform_macos::recording_hooks::element_window_local_xy(wid, pid, idx)
            });
            cua_driver_core::video::set_video_backend_factory(
                Box::new(platform_macos::video_sckit::SckitVideoBackendFactory),
            );
            let reg = Arc::new(build_macos_registry());
            reg.init_self_weak();
            cli::run_call(reg, &tool, json_args, screenshot_out_file, socket);
            return;
        }
        cli::Command::Serve { socket, no_permissions_gate, claude_code_compat } => {
            responsibility::reexec_disclaimed_if_needed();
            // Long-running daemon — kick off the background update check
            // before any blocking work so the banner can land on stderr
            // early in the serve lifecycle.
            version_check::maybe_announce_update();
            let gate_opts = platform_macos::permissions::GateOpts::from_env_and_flag(
                no_permissions_gate,
            );
            cua_driver_core::recording::set_screenshot_fn(|window_id, pid| {
                if let Some(wid) = window_id {
                    platform_macos::capture::screenshot_window_bytes(wid as u32).ok()
                } else if let Some(p) = pid {
                    platform_macos::windows::resolve_main_window_id(p as i32).ok()
                        .and_then(|wid| platform_macos::capture::screenshot_window_bytes(wid).ok())
                } else {
                    platform_macos::capture::screenshot_display_bytes().ok()
                }
            });
            cua_driver_core::recording::set_click_marker_fn(|png_bytes, cx, cy| {
                platform_macos::capture::crosshair_png_bytes(png_bytes, cx, cy).ok()
            });
            cua_driver_core::recording::set_ax_snapshot_fn(|window_id, pid| {
                platform_macos::recording_hooks::app_state_json_for(window_id, pid)
            });
            cua_driver_core::recording::set_element_bounds_fn(|wid, pid, idx| {
                platform_macos::recording_hooks::element_window_local_xy(wid, pid, idx)
            });
            cua_driver_core::video::set_video_backend_factory(
                Box::new(platform_macos::video_sckit::SckitVideoBackendFactory),
            );
            let pip_cfg = match pip_preview::default_config_path() {
                Some(p) => pip_preview::PipConfig::from_args_and_file(&p),
                None => pip_preview::PipConfig::from_args(),
            };
            maybe_init_pip();

            // Agent-cursor overlay. The DAEMON is the process that actually
            // performs clicks / AX presses, so the overlay NSWindow + render
            // loop must run HERE — not only in the in-process `mcp` arm. In the
            // daemon-proxy setup (`mcp` relaunches `open -n -g … serve` and
            // proxies to it), the proxy never renders and, before this, neither
            // did the daemon — so every cursor command was a silent no-op and
            // the agent cursor never appeared. Init the channel before spawning
            // the serve thread so `run_on_main_thread()` always finds it ready
            // (mirrors the Mcp arm).
            let cursor_cfg = cursor_overlay::CursorConfig::from_args();
            if cursor_cfg.enabled {
                platform_macos::cursor::overlay::init(cursor_cfg.clone());
            }

            // Honour the compat flag forwarded by the MCP proxy
            // (launch_daemon_and_wait passes `serve
            // --claude-code-computer-use-compat`). The Serve arm is the daemon
            // the proxy talks to, so without this the proxy path always served
            // the full screenshot tool regardless of the client's request.
            let reg = Arc::new(build_macos_registry_with_compat(claude_code_compat));
            reg.init_self_weak();
            let sp = socket.unwrap_or_else(serve::default_socket_path);
            let pid_path = serve::default_pid_file_path();

            // Bind the Unix socket FIRST, on a background thread, BEFORE
            // running the (blocking) permissions gate (#1761).
            //
            // The gate's `wait_for_grants` blocks while `com.trycua.driver`
            // is ungranted — it prompts and re-exec-loops until the user
            // grants or the deadline elapses. If serve ran after the gate,
            // the daemon's socket wouldn't appear for minutes on first
            // launch, so `permissions grant` / MCP clients launched via
            // `open -n -g -a CuaDriver --args serve` (the correct-TCC-
            // attribution path) couldn't reach the daemon to even report
            // "pending". Binding the socket first makes the daemon
            // reachable within ~1s while the gate works toward the grant.
            //
            // A Unix socket + tokio accept loop has no main-thread
            // requirement, so serve runs on a background thread. The gate
            // stays on the MAIN thread: its prompt APIs
            // (`request_accessibility` / `request_screen_recording`) and
            // the NSPanel must run on main. On grant, the gate's
            // `reexec_self()` execvp's the whole daemon — the socket
            // re-binds fast on restart (run_serve unlinks the stale socket
            // file first) and stabilizes once the grant sticks.
            let serve_handle = std::thread::Builder::new()
                .name("cua-serve".into())
                .spawn(move || {
                    serve::run_serve_cmd(reg, &sp, Some(&pid_path));
                    std::process::exit(0);
                })
                .expect("spawn serve thread");

            // Socket is binding/bound now → daemon reachable while we gate.
            //
            // First-launch permissions gate (Swift PermissionsGate parity).
            // Runs on every `serve` start; no-op when both grants are
            // already active.  Honors --no-permissions-gate and
            // CUA_DRIVER_RS_PERMISSIONS_GATE=0 for CI / headless.
            //
            // Failures (e.g. deadline elapsed without grants) are logged
            // and the daemon continues to serve — individual tool calls
            // will then fail with the underlying TCC error, mirroring
            // Swift's "user closed the panel" fallback.
            if let Err(e) = platform_macos::permissions::run_if_needed(gate_opts) {
                eprintln!("[cua-driver] permissions gate: {e}");
                eprintln!("[cua-driver] continuing — tool calls touching AX or \
                           Screen Recording fail until you grant the missing TCC \
                           permissions.");
            }

            // Keep the main thread alive for the daemon.
            //
            // PiP needs the AppKit main run loop to process the
            // dispatch_async_f calls that push frames into NSImageView;
            // park main in NSApplication.run() when --experimental-pip is
            // on. Otherwise just join the serve thread so the process
            // stays up as long as the daemon does.
            if pip_cfg.enabled {
                platform_macos::pip::run_appkit_main_loop();
            } else if cursor_cfg.enabled {
                // Render the agent-cursor overlay: park the main thread in the
                // AppKit run loop so the overlay NSWindow draws. `run_on_main_thread`
                // self-guards on `has_graphic_access()` and returns immediately
                // when the daemon has no Window Server session — fall through to
                // join so the daemon still serves headless. The serve thread runs
                // on its background thread regardless.
                platform_macos::cursor::overlay::run_on_main_thread();
                let _ = serve_handle.join();
            } else {
                let _ = serve_handle.join();
            }
            return;
        }
        cli::Command::Stop { socket } => {
            let sp = socket.unwrap_or_else(serve::default_socket_path);
            serve::run_stop_cmd(&sp);
            return;
        }
        cli::Command::Status { socket } => {
            let sp = socket.unwrap_or_else(serve::default_socket_path);
            let pid_path = serve::default_pid_file_path();
            serve::run_status_cmd(&sp, &pid_path);
            return;
        }
        cli::Command::Recording { subcommand, args, socket } => {
            cli::run_recording_cmd(&subcommand, &args, socket.as_deref());
            return;
        }
        cli::Command::DumpDocs { pretty, doc_type } => {
            let reg = Arc::new(build_macos_registry());
            cli::run_dump_docs_with_type(&reg, pretty, &doc_type);
            return;
        }
        cli::Command::Update { apply, json } => {
            cli::run_update_cmd(apply, json);
            return;
        }
        cli::Command::CheckUpdate { json, no_cache } => {
            cli::run_check_update_cmd(json, no_cache);
            return;
        }
        cli::Command::Doctor { json } => {
            // Long-running interactive entry point — kick off the
            // background "new version available?" check so the banner
            // can land on stderr if the user is on an outdated install.
            // Skip the banner in --json mode so output stays parseable.
            if !json {
                version_check::maybe_announce_update();
            }
            cli::run_doctor_cmd(json);
            return;
        }
        cli::Command::Diagnose => {
            let reg = Arc::new(build_macos_registry());
            cli::run_diagnose_cmd(reg);
            return;
        }
        cli::Command::Permissions { subcommand, json } => {
            let reg = Arc::new(build_macos_registry());
            cli::run_permissions_cmd(reg, &subcommand, json);
            return;
        }
        cli::Command::Autostart { subcommand } => {
            autostart::run_autostart_cmd(&subcommand);
            return;
        }
        cli::Command::Skills { subcommand, flags } => {
            skills::run(&subcommand, &flags);
            return;
        }
        cli::Command::Config { subcommand, key, value, socket } => {
            let reg = Arc::new(build_macos_registry());
            reg.init_self_weak();
            cli::run_config_cmd(reg, subcommand.as_deref(), key.as_deref(), value.as_deref(), socket.as_deref());
            return;
        }
        cli::Command::Mcp { no_daemon_relaunch, socket, claude_code_compat } => {
            CLAUDE_CODE_COMPAT.store(claude_code_compat, Ordering::SeqCst);
            // Long-running MCP server — kick off the background update
            // check before any TCC / daemon-proxy decisions so the
            // banner can land on stderr in either dispatch path.
            version_check::maybe_announce_update();
            // TCC sidestep: if we're a shell-spawned bare binary that
            // resolves into /Applications/CuaDriver.app, run the
            // proxy path instead of the in-process MCP server. The
            // proxy ensures a daemon is up under the bundle's TCC
            // attribution and forwards stdio MCP through its socket.
            // Issue #1525 / mirror of Swift PR #1479.
            if cli::should_use_daemon_proxy(no_daemon_relaunch) {
                if let Err(e) = cli::run_mcp_via_daemon_proxy(socket, claude_code_compat) {
                    eprintln!("cua-driver-rs: {e}");
                    std::process::exit(1);
                }
                return;
            }
            // Fall through to the in-process MCP server below. The
            // `socket` flag is daemon-proxy-only; it has no meaning
            // in the in-process path, so we drop it on the floor.
            let _ = socket;
        }
    }

    let cursor_cfg = cursor_overlay::CursorConfig::from_args();

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        cursor_id = %cursor_cfg.cursor_id,
        overlay_enabled = cursor_cfg.enabled,
        has_custom_icon = cursor_cfg.shape.is_some(),
        "cua-driver-rs starting (macOS)"
    );

    let enabled = cursor_cfg.enabled;

    // Initialise overlay channel synchronously BEFORE spawning background
    // thread.  This eliminates a race where run_on_main_thread() could be
    // called before init() and find an empty channel.
    if enabled {
        platform_macos::cursor::overlay::init(cursor_cfg.clone());
    }

    // Spawn tokio + MCP server on a background thread so the main thread
    // is free for AppKit.
    // Register screenshot callback for recording (post-action screenshots).
    cua_driver_core::recording::set_screenshot_fn(|window_id, pid| {
        if let Some(wid) = window_id {
            platform_macos::capture::screenshot_window_bytes(wid as u32).ok()
        } else if let Some(p) = pid {
            platform_macos::windows::resolve_main_window_id(p as i32).ok()
                .and_then(|wid| platform_macos::capture::screenshot_window_bytes(wid).ok())
        } else {
            platform_macos::capture::screenshot_display_bytes().ok()
        }
    });

    // Register click-marker callback for recording (click.png with red crosshair).
    cua_driver_core::recording::set_click_marker_fn(|png_bytes, cx, cy| {
        platform_macos::capture::crosshair_png_bytes(png_bytes, cx, cy).ok()
    });
    cua_driver_core::recording::set_ax_snapshot_fn(|window_id, pid| {
        platform_macos::recording_hooks::app_state_json_for(window_id, pid)
    });
    cua_driver_core::recording::set_element_bounds_fn(|wid, pid, idx| {
        platform_macos::recording_hooks::element_window_local_xy(wid, pid, idx)
    });
    cua_driver_core::video::set_video_backend_factory(
        Box::new(platform_macos::video_sckit::SckitVideoBackendFactory),
    );
    maybe_init_pip();

    std::thread::Builder::new()
        .name("cua-mcp".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            let compat = CLAUDE_CODE_COMPAT.load(Ordering::SeqCst);
            rt.block_on(async move {
                // Register tools; overlay init has already happened above.
                let registry = Arc::new(build_macos_registry_with_compat(compat));
                // Wire up replay tool's back-reference to the registry.
                registry.init_self_weak();
                if let Err(e) = cua_driver_core::server::run(registry).await {
                    tracing::error!("MCP server error: {e}");
                }
            });
            // MCP server exited (stdin closed / client disconnected).
            // The main thread is blocked in NSApplication.run() and won't
            // exit on its own — force-exit the process cleanly.
            std::process::exit(0);
        })
        .expect("spawn mcp thread");

    // Main thread: AppKit overlay (blocks until the process exits).
    if enabled {
        platform_macos::cursor::overlay::run_on_main_thread();
    }
    // Overlay disabled: park the main thread while the MCP background thread
    // keeps running.
    loop { std::thread::park(); }
}

// ── Non-macOS entry-point ─────────────────────────────────────────────────

#[cfg(not(target_os = "macos"))]
fn main() -> anyhow::Result<()> {
    init_logging();

    // ── CLI subcommand dispatch ──────────────────────────────────────────────
    // These commands create their own tokio runtimes internally, so they must
    // run on a plain OS thread — not inside a #[tokio::main] context which
    // would cause nested block_on panics.
    let command = cli::parse_command();
    emit_entry_telemetry(&command);
    match command {
        cli::Command::TelemetryInstallEvent => {
            // Synchronous install ping (see `telemetry::capture_install`).
            // Blocks on the POST so the `.installation_recorded` marker
            // is only written on HTTP success — failed POST means next
            // launch retries. Installer script already runs us in the
            // background via `&`, so blocking here is fine.
            telemetry::capture_install();
            return Ok(());
        }
        cli::Command::ListTools => {
            let reg = Arc::new(build_registry_no_cursor());
            cli::run_list_tools(&reg);
            return Ok(());
        }
        cli::Command::Describe(name) => {
            let reg = Arc::new(build_registry_no_cursor());
            cli::run_describe(&reg, &name);
            return Ok(());
        }
        cli::Command::McpConfig { client } => {
            cli::run_mcp_config(client.as_deref());
            return Ok(());
        }
        cli::Command::Manifest { pretty } => {
            // Surface 8: machine-readable CLI manifest. Read-only — no
            // registry build needed.
            cli::run_manifest(pretty);
            return Ok(());
        }
        cli::Command::Call { tool, json_args, screenshot_out_file, socket } => {
            let reg = Arc::new(build_registry_no_cursor());
            reg.init_self_weak();
            // run_call builds its own tokio runtime; must run on a fresh thread.
            std::thread::spawn(move || {
                cli::run_call(reg, &tool, json_args, screenshot_out_file, socket);
            }).join().ok();
            return Ok(());
        }
        cli::Command::Serve { socket, no_permissions_gate, claude_code_compat } => {
            responsibility::reexec_disclaimed_if_needed();
            // Long-running daemon — kick off the background update check
            // before any blocking work so the banner can land on stderr.
            version_check::maybe_announce_update();
            // The Rust permissions gate is macOS-only (TCC concept).
            // On Windows / Linux the flag is silently accepted for
            // CLI uniformity and ignored. The Claude-Code compat screenshot
            // surface is likewise macOS-only (register_tools_with_compat),
            // so the flag is accepted-and-ignored here for CLI uniformity.
            let _ = no_permissions_gate;
            let _ = claude_code_compat;
            // Serve mode needs the cursor overlay just like MCP mode.
            let cursor_cfg = cursor_overlay::CursorConfig::from_args();
            let reg = Arc::new(build_registry(cursor_cfg));
            reg.init_self_weak();
            maybe_init_pip();
            let sp = socket.unwrap_or_else(serve::default_socket_path);
            let pid_path = serve::default_pid_file_path();
            // run_serve_cmd builds its own runtime; must run on a fresh thread.
            std::thread::spawn(move || {
                serve::run_serve_cmd(reg, &sp, Some(&pid_path));
            }).join().ok();
            return Ok(());
        }
        cli::Command::Stop { socket } => {
            let sp = socket.unwrap_or_else(serve::default_socket_path);
            serve::run_stop_cmd(&sp);
            return Ok(());
        }
        cli::Command::Status { socket } => {
            let sp = socket.unwrap_or_else(serve::default_socket_path);
            let pid_path = serve::default_pid_file_path();
            serve::run_status_cmd(&sp, &pid_path);
            return Ok(());
        }
        cli::Command::Recording { subcommand, args, socket } => {
            cli::run_recording_cmd(&subcommand, &args, socket.as_deref());
            return Ok(());
        }
        cli::Command::DumpDocs { pretty, doc_type } => {
            let reg = Arc::new(build_registry_no_cursor());
            cli::run_dump_docs_with_type(&reg, pretty, &doc_type);
            return Ok(());
        }
        cli::Command::Update { apply, json } => {
            cli::run_update_cmd(apply, json);
            return Ok(());
        }
        cli::Command::CheckUpdate { json, no_cache } => {
            cli::run_check_update_cmd(json, no_cache);
            return Ok(());
        }
        cli::Command::Doctor { json } => {
            // Long-running interactive entry point — kick off the
            // background update check so the banner can land on stderr.
            // Skip the banner in --json mode so output stays parseable.
            if !json {
                version_check::maybe_announce_update();
            }
            cli::run_doctor_cmd(json);
            return Ok(());
        }
        cli::Command::Diagnose => {
            let reg = Arc::new(build_registry_no_cursor());
            cli::run_diagnose_cmd(reg);
            return Ok(());
        }
        cli::Command::Permissions { subcommand, json } => {
            let reg = Arc::new(build_registry_no_cursor());
            cli::run_permissions_cmd(reg, &subcommand, json);
            return Ok(());
        }
        cli::Command::Autostart { subcommand } => {
            autostart::run_autostart_cmd(&subcommand);
            return Ok(());
        }
        cli::Command::Skills { subcommand, flags } => {
            skills::run(&subcommand, &flags);
            return Ok(());
        }
        cli::Command::Config { subcommand, key, value, socket } => {
            let reg = Arc::new(build_registry_no_cursor());
            reg.init_self_weak();
            std::thread::spawn(move || {
                cli::run_config_cmd(reg, subcommand.as_deref(), key.as_deref(), value.as_deref(), socket.as_deref());
            }).join().ok();
            return Ok(());
        }
        cli::Command::Mcp { no_daemon_relaunch, socket, claude_code_compat } => {
            CLAUDE_CODE_COMPAT.store(claude_code_compat, Ordering::SeqCst);
            // Long-running MCP server — kick off the background update
            // check before any daemon-proxy decisions.
            version_check::maybe_announce_update();
            // Daemon-proxy sidestep for Windows Session 0 attribution
            // (and equivalent on Linux when a daemon is up): if a
            // daemon is listening on the default socket, forward
            // stdio MCP through it instead of running the server
            // in-process. The proxy preserves the daemon's session
            // identity (typically Session 1+ on Windows) so window /
            // UIA / screen tools see the user's actual desktop —
            // without this, an `cua-driver mcp` spawned by Claude
            // Code over SSH lands in Session 0 and every desktop
            // tool returns empty. See `cli::should_use_daemon_proxy`.
            if cli::should_use_daemon_proxy(no_daemon_relaunch) {
                if let Err(e) = cli::run_mcp_via_daemon_proxy(socket, claude_code_compat) {
                    eprintln!("cua-driver-rs: {e}");
                    std::process::exit(1);
                }
                return Ok(());
            }
            // Fall through to the in-process MCP server below. The
            // `socket` flag is daemon-proxy-only; ignored on the
            // in-process path (mirrors the macOS arm's drop-on-floor
            // behaviour).
            let _ = socket;
        }
    }

    // MCP server mode: this needs a full async tokio runtime.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    rt.block_on(async_main())?;
    Ok(())
}

#[cfg(not(target_os = "macos"))]
async fn async_main() -> anyhow::Result<()> {

    let cursor_cfg = cursor_overlay::CursorConfig::from_args();

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        os = std::env::consts::OS,
        cursor_id = %cursor_cfg.cursor_id,
        overlay_enabled = cursor_cfg.enabled,
        has_custom_icon = cursor_cfg.shape.is_some(),
        "cua-driver-rs starting"
    );

    let registry = Arc::new(build_registry(cursor_cfg));
    registry.init_self_weak();
    maybe_init_pip();
    let result = cua_driver_core::server::run(registry).await;
    if let Err(e) = &result {
        tracing::error!("MCP server error: {e}");
    }

    // The stdio MCP server loop has ended — the client disconnected (stdin
    // EOF) or a fatal I/O error occurred. The cursor overlay runs on its own
    // detached thread with an independent Win32 message loop (and we raised the
    // multimedia timer resolution via `timeBeginPeriod`), so simply returning
    // is not guaranteed to tear it down promptly: that thread is never joined
    // and would otherwise keep its render loop alive as an orphan, accumulating
    // CPU after the client is gone (issue #1808). Force a clean process exit so
    // the overlay thread dies with us the moment the transport closes — mirrors
    // the macOS `std::process::exit(0)` after `server::run`.
    std::process::exit(if result.is_ok() { 0 } else { 1 });
}

// ── Registry builder (non-macOS) ──────────────────────────────────────────

#[cfg(not(target_os = "macos"))]
fn build_registry(cursor_cfg: cursor_overlay::CursorConfig) -> cua_driver_core::tool::ToolRegistry {
    let compat = CLAUDE_CODE_COMPAT.load(Ordering::SeqCst);
    #[cfg(target_os = "windows")]
    {
        cua_driver_core::recording::set_screenshot_fn(|window_id, pid| {
            if let Some(hwnd) = window_id {
                platform_windows::capture::screenshot_window_bytes(hwnd).ok()
            } else if let Some(p) = pid {
                let wins = platform_windows::win32::list_windows(Some(p as u32));
                wins.first().and_then(|w| {
                    platform_windows::capture::screenshot_window_bytes(w.hwnd).ok()
                })
            } else {
                platform_windows::capture::screenshot_display_bytes().ok()
            }
        });
        cua_driver_core::recording::set_click_marker_fn(|png_bytes, cx, cy| {
            platform_windows::capture::crosshair_png_bytes(png_bytes, cx, cy).ok()
        });
        cua_driver_core::recording::set_ax_snapshot_fn(|window_id, pid| {
            platform_windows::recording_hooks::app_state_json_for(window_id, pid)
        });
        cua_driver_core::recording::set_element_bounds_fn(|wid, pid, idx| {
            platform_windows::recording_hooks::element_window_local_xy(wid, pid, idx)
        });
        cua_driver_core::video::set_video_backend_factory(
            Box::new(cua_driver_core::video_ffmpeg::FfmpegVideoBackendFactory),
        );
        { let mut r = platform_windows::register_tools_with_cursor(cursor_cfg, compat); check_update_tool::register_into(&mut r); r }
    }
    #[cfg(target_os = "linux")]
    {
        cua_driver_core::recording::set_screenshot_fn(|window_id, pid| {
            if let Some(xid) = window_id {
                platform_linux::capture::screenshot_window_bytes(xid).ok()
            } else if let Some(p) = pid {
                let wins = platform_linux::x11::list_windows(Some(p as u32));
                wins.first().and_then(|w| {
                    platform_linux::capture::screenshot_window_bytes(w.xid).ok()
                })
            } else {
                platform_linux::capture::screenshot_display_bytes().ok()
            }
        });
        cua_driver_core::recording::set_click_marker_fn(|png_bytes, cx, cy| {
            platform_linux::capture::crosshair_png_bytes(png_bytes, cx, cy).ok()
        });
        cua_driver_core::video::set_video_backend_factory(
            Box::new(cua_driver_core::video_ffmpeg::FfmpegVideoBackendFactory),
        );
        // SSH-driven Wayland+Xwayland sessions inherit DISPLAY but not
        // XAUTHORITY; adopt the running X server's auth cookie so X11 tools
        // don't all fail "Authorization required" (#1926). No-op when
        // XAUTHORITY is already set or there's no DISPLAY.
        platform_linux::xauth::ensure_xauthority_discovered();
        // AT-SPI lives on the session bus; when the daemon is started outside
        // the desktop session (container, headless, runuser, systemd system
        // unit) DBUS_SESSION_BUS_ADDRESS is unset and the AT-SPI tree comes back
        // empty. Recover it from /run/user/<uid>/bus or a running session
        // process before the a11y advertise (which itself needs the bus). No-op
        // when already set.
        platform_linux::session_bus::ensure_session_bus_discovered();
        // Turn on Chromium/Electron (and GTK/Qt) accessibility for the session
        // so their AT-SPI trees are visible to get_window_state. Best-effort and
        // idempotent; only on the serve path, not for short-lived CLI calls.
        platform_linux::a11y::ensure_chromium_accessibility_enabled();
        { let mut r = platform_linux::register_tools_with_cursor(cursor_cfg, compat); check_update_tool::register_into(&mut r); r }
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        let _ = cursor_cfg;
        let _ = compat;
        let mut r = cua_driver_core::tool::ToolRegistry::new();
        r.register(Box::new(crate::stub::UnsupportedPlatformTool));
        r
    }
}

/// Build a registry without initialising the cursor overlay.
/// Used by CLI subcommands (list-tools / describe / call) that don't need the overlay.
#[cfg(not(target_os = "macos"))]
fn build_registry_no_cursor() -> cua_driver_core::tool::ToolRegistry {
    let compat = CLAUDE_CODE_COMPAT.load(Ordering::SeqCst);
    #[cfg(target_os = "windows")]
    {
        cua_driver_core::recording::set_screenshot_fn(|window_id, pid| {
            if let Some(hwnd) = window_id {
                platform_windows::capture::screenshot_window_bytes(hwnd).ok()
            } else if let Some(p) = pid {
                let wins = platform_windows::win32::list_windows(Some(p as u32));
                wins.first().and_then(|w| {
                    platform_windows::capture::screenshot_window_bytes(w.hwnd).ok()
                })
            } else {
                platform_windows::capture::screenshot_display_bytes().ok()
            }
        });
        cua_driver_core::recording::set_click_marker_fn(|png_bytes, cx, cy| {
            platform_windows::capture::crosshair_png_bytes(png_bytes, cx, cy).ok()
        });
        cua_driver_core::recording::set_ax_snapshot_fn(|window_id, pid| {
            platform_windows::recording_hooks::app_state_json_for(window_id, pid)
        });
        cua_driver_core::recording::set_element_bounds_fn(|wid, pid, idx| {
            platform_windows::recording_hooks::element_window_local_xy(wid, pid, idx)
        });
        cua_driver_core::video::set_video_backend_factory(
            Box::new(cua_driver_core::video_ffmpeg::FfmpegVideoBackendFactory),
        );
        {
            let mut r = platform_windows::register_tools_with_cursor(
                cursor_overlay::CursorConfig { enabled: false, ..Default::default() },
                compat,
            );
            check_update_tool::register_into(&mut r);
            r
        }
    }
    #[cfg(target_os = "linux")]
    {
        cua_driver_core::recording::set_screenshot_fn(|window_id, pid| {
            if let Some(xid) = window_id {
                platform_linux::capture::screenshot_window_bytes(xid).ok()
            } else if let Some(p) = pid {
                let wins = platform_linux::x11::list_windows(Some(p as u32));
                wins.first().and_then(|w| {
                    platform_linux::capture::screenshot_window_bytes(w.xid).ok()
                })
            } else {
                platform_linux::capture::screenshot_display_bytes().ok()
            }
        });
        cua_driver_core::recording::set_click_marker_fn(|png_bytes, cx, cy| {
            platform_linux::capture::crosshair_png_bytes(png_bytes, cx, cy).ok()
        });
        cua_driver_core::video::set_video_backend_factory(
            Box::new(cua_driver_core::video_ffmpeg::FfmpegVideoBackendFactory),
        );
        {
            let mut r = platform_linux::register_tools_with_cursor(
                cursor_overlay::CursorConfig { enabled: false, ..Default::default() },
                compat,
            );
            check_update_tool::register_into(&mut r);
            r
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        let _ = compat;
        let mut r = cua_driver_core::tool::ToolRegistry::new();
        r.register(Box::new(crate::stub::UnsupportedPlatformTool));
        r
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
mod stub {
    use async_trait::async_trait;
    use cua_driver_core::tool::{Tool, ToolDef, ToolResult};
    use serde_json::Value;

    pub struct UnsupportedPlatformTool;

    #[async_trait]
    impl Tool for UnsupportedPlatformTool {
        fn def(&self) -> &ToolDef {
            static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();
            DEF.get_or_init(|| ToolDef {
                name: "unsupported_platform".into(),
                description: "This platform is not supported.".into(),
                input_schema: serde_json::json!({"type":"object","properties":{}}),
                read_only: true,
                destructive: false,
                idempotent: true,
                open_world: false,
            })
        }
        async fn invoke(&self, _args: Value) -> ToolResult {
            ToolResult::error("Unsupported platform")
        }
    }
}
