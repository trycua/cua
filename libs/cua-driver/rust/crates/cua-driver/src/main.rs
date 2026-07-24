#![recursion_limit = "256"]

//! cua-driver-rs — cross-platform background computer-use automation daemon.
//!
//! Runs a daemon-backed MCP JSON-RPC 2.0 proxy over stdio. The platform
//! backend lives in the `serve` daemon selected at compile time.
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
//! On macOS, `serve` keeps AppKit work on the main thread while its socket loop
//! runs in the background. MCP and CLI client processes never initialize the
//! platform tool registry.

mod autostart;
mod bundle;
mod check_update_tool;
mod cli;
mod doctor;
mod mcp_http;
mod proxy;
mod responsibility;
mod sdk_adapter;
mod serve;
mod skills;
mod telemetry;
mod updater;
mod version_check;

use std::sync::Arc;

fn init_logging() {
    use tracing_subscriber::EnvFilter;
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(EnvFilter::from_env("CUA_LOG").add_directive(tracing::Level::WARN.into()))
        .init();
    telemetry::register_stdio_observer();
}

fn configure_startup_permission_mode(
    permission_mode: Option<&str>,
    dangerously_bypass_approvals: bool,
    allow_legacy_existing_profile_approval: bool,
    session_policy: Option<&str>,
    approve_session_policy: bool,
) -> anyhow::Result<()> {
    if let Some(mode) = permission_mode {
        std::env::set_var(cua_driver_core::authorization::PERMISSION_MODE_ENV, mode);
    } else if dangerously_bypass_approvals
        && std::env::var_os(cua_driver_core::authorization::PERMISSION_MODE_ENV).is_none()
    {
        // The alarming CLI flag is both the unrestricted-mode selector and
        // the user's explicit launch-time risk acknowledgement. Embedded and
        // environment-driven launchers retain the two-part mode + acceptance
        // contract because they do not pass through this CLI normalization.
        std::env::set_var(
            cua_driver_core::authorization::PERMISSION_MODE_ENV,
            "unrestricted",
        );
    }
    if dangerously_bypass_approvals {
        std::env::set_var(cua_driver_core::authorization::DANGEROUS_BYPASS_ENV, "1");
    }
    if allow_legacy_existing_profile_approval {
        std::env::set_var(
            cua_driver_core::authorization::LEGACY_EXISTING_PROFILE_APPROVAL_ENV,
            "1",
        );
    }
    if let Some(path) = session_policy {
        std::env::set_var(
            cua_driver_core::session_manifest::SESSION_POLICY_FILE_ENV,
            path,
        );
    }
    if approve_session_policy {
        std::env::set_var(
            cua_driver_core::session_manifest::SESSION_POLICY_APPROVED_ENV,
            "1",
        );
    }
    cua_driver_core::authorization::validate_startup_authorization()?;
    if cua_driver_core::authorization::configured_permission_mode()
        .is_ok_and(|mode| mode == cua_driver_core::authorization::PermissionMode::Unrestricted)
    {
        eprintln!(
            "DANGER: Cua Driver is running in unrestricted mode. Runtime approval prompts are disabled; prompt injection or unintended input may act with every capability allowed by the built-in, managed, and user policy ceilings. Use only in a disposable or fully trusted environment."
        );
    }
    Ok(())
}

/// Execute finite commands in a child so the parent can observe every exit,
/// including validation failures and legacy `process::exit` paths. Delivery is
/// delegated to a detached, no-output worker after the child exits, so network
/// latency is never added to the foreground command.
fn maybe_wrap_finite_command() {
    if telemetry::is_wrapped_cli_child() {
        return;
    }
    let Some(command_name) = cli::finite_command_name_from_argv() else {
        return;
    };
    let tool_name = cli::finite_tool_name_from_argv();
    let computer_action = cli::finite_computer_action_from_argv();
    let operation = cli::finite_operation_from_argv();
    let client_kind = cli::finite_client_kind_from_argv();
    telemetry::spawn_first_run_registration_worker();
    let Ok(executable) = std::env::current_exe() else {
        return;
    };
    let started_at = std::time::Instant::now();
    let status = std::process::Command::new(executable)
        .args(std::env::args_os().skip(1))
        .env(telemetry::cli_wrapped_child_env(), "1")
        .status();
    let Ok(status) = status else {
        return;
    };
    let exit_code = status.code().unwrap_or(1);
    telemetry::spawn_cli_completion_worker(
        command_name,
        tool_name.as_deref(),
        computer_action,
        operation,
        client_kind,
        exit_code,
        started_at.elapsed(),
    );
    std::process::exit(exit_code);
}

fn run_telemetry_command(command: cli::TelemetryCommand) {
    match command {
        cli::TelemetryCommand::InstallEvent => telemetry::capture_install(),
        cli::TelemetryCommand::Enable => match telemetry::set_enabled(true) {
            Ok(()) => println!("Telemetry enabled. The retained installation ID will be reused."),
            Err(error) => {
                eprintln!("cua-driver: failed to enable telemetry: {error}");
                std::process::exit(1);
            }
        },
        cli::TelemetryCommand::Disable => match telemetry::set_enabled(false) {
            Ok(()) => println!("Telemetry disabled. The local installation ID was retained; run `cua-driver telemetry reset-id` to erase it."),
            Err(error) => {
                eprintln!("cua-driver: failed to disable telemetry: {error}");
                std::process::exit(1);
            }
        },
        cli::TelemetryCommand::Status { json } => {
            let status = telemetry::status();
            if json {
                println!("{}", serde_json::to_string_pretty(&status).expect("serialize telemetry status"));
            } else {
                println!("Telemetry: {} (source: {})", if status.enabled { "enabled" } else { "disabled" }, status.source);
                println!("Installation ID: {}", status.installation_id.as_deref().unwrap_or("not created"));
                println!("Registration recorded: {}", status.registration_recorded);
                println!("Current release recorded: {}", status.current_release_recorded);
            }
        }
        cli::TelemetryCommand::ResetId => match telemetry::reset_id() {
            Ok(()) => println!("Telemetry installation ID and event markers erased. The enable/disable preference was retained."),
            Err(error) => {
                eprintln!("cua-driver: failed to reset telemetry ID: {error}");
                std::process::exit(1);
            }
        },
        cli::TelemetryCommand::Inspect { event } => match telemetry::inspect_event(&event) {
            Ok(payload) => println!("{}", serde_json::to_string_pretty(&payload).expect("serialize telemetry payload")),
            Err(error) => {
                eprintln!("cua-driver: {error}");
                std::process::exit(64);
            }
        },
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
    pip_preview::set_pip_backend_factory(Box::new(platform_macos::pip::MacosPipBackendFactory));
    #[cfg(target_os = "windows")]
    pip_preview::set_pip_backend_factory(Box::new(platform_windows::pip::WindowsPipBackendFactory));
    #[cfg(target_os = "linux")]
    pip_preview::set_pip_backend_factory(Box::new(platform_linux::pip::LinuxPipBackendFactory));

    match pip_preview::start_pip(&cfg) {
        Ok(backend) => {
            // Bridge: when the tool dispatcher in cua-driver-core wants
            // to push a frame, forward to the live backend handle.
            // We move the Box into a static Mutex<Option<...>> so the
            // closure can re-borrow on every call without taking
            // ownership of the trait object.
            use std::sync::Mutex as StdMutex;
            static BACKEND: std::sync::OnceLock<
                StdMutex<Option<Box<dyn pip_preview::PipBackend>>>,
            > = std::sync::OnceLock::new();
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

// ── Public SDK runtime host ──────────────────────────────────────────────

/// Construct the canonical SDK-owned runtime for the CLI or daemon host.
/// The private socket and MCP layers consume this object downstream.
fn build_driver(
    cursor: cursor_overlay::CursorConfig,
    compatibility_mode: bool,
) -> Arc<cua_driver_sdk::CuaDriver> {
    cua_driver_sdk::CuaDriver::create_for_host(cua_driver_sdk::DriverHostOptions {
        cursor,
        claude_code_compatibility: compatibility_mode,
        prepare_desktop_environment: true,
        register_host_tools: Some(check_update_tool::register_into),
    })
}

fn build_driver_without_cursor() -> Arc<cua_driver_sdk::CuaDriver> {
    build_driver(
        cursor_overlay::CursorConfig {
            enabled: false,
            ..cursor_overlay::CursorConfig::default()
        },
        false,
    )
}

fn run_mcp_direct(compatibility_mode: bool) -> anyhow::Result<()> {
    let driver = build_driver(
        cursor_overlay::CursorConfig::from_args(),
        compatibility_mode,
    );
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(proxy::run_direct(driver))
}

fn sdk_tool_inventory(driver: Arc<cua_driver_sdk::CuaDriver>) -> serde_json::Value {
    match sdk_adapter::SdkAdapter::load_blocking(driver) {
        Ok(sdk) => sdk.tools_list(),
        Err(error) => {
            eprintln!("Could not load Cua Driver SDK tool inventory: {error}");
            std::process::exit(1);
        }
    }
}

// ── macOS entry-point ─────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn main() {
    init_logging();
    if telemetry::run_cli_completion_worker_if_requested() {
        return;
    }
    if telemetry::run_lifecycle_worker_if_requested() {
        return;
    }
    if telemetry::run_update_event_worker_if_requested() {
        return;
    }
    maybe_wrap_finite_command();

    // ── CLI subcommand dispatch ──────────────────────────────────────────────
    // Handled before AppKit init so `list-tools` / `describe` / `call` exit
    // cleanly without starting the overlay or NSApplication.
    let command = cli::parse_command();
    if !telemetry::is_wrapped_cli_child() && !matches!(&command, cli::Command::Telemetry(_)) {
        telemetry::spawn_first_run_registration_worker();
    }
    match command {
        cli::Command::Telemetry(command) => {
            run_telemetry_command(command);
        }
        cli::Command::ListTools => {
            let tools = sdk_tool_inventory(build_driver_without_cursor());
            cli::run_list_tools(&tools);
        }
        cli::Command::Describe(name) => {
            let tools = sdk_tool_inventory(build_driver_without_cursor());
            cli::run_describe(&tools, &name);
        }
        cli::Command::McpConfig { client } => {
            cli::run_mcp_config(client.as_deref());
        }
        cli::Command::Manifest { pretty } => {
            // Surface 8: machine-readable CLI manifest. Read-only — no
            // registry build needed, no daemon contact.
            cli::run_manifest(pretty);
        }
        cli::Command::Call {
            tool,
            json_args,
            screenshot_out_file,
            socket,
        } => {
            cli::run_call(&tool, json_args, screenshot_out_file, socket);
        }
        cli::Command::Serve {
            socket,
            permission_mode,
            dangerously_bypass_approvals,
            allow_legacy_existing_profile_approval,
            session_policy,
            approve_session_policy,
            no_permissions_gate,
            claude_code_compat,
        } => {
            if let Err(error) = configure_startup_permission_mode(
                permission_mode.as_deref(),
                dangerously_bypass_approvals,
                allow_legacy_existing_profile_approval,
                session_policy.as_deref(),
                approve_session_policy,
            ) {
                eprintln!("cua-driver: authorization startup error: {error}");
                std::process::exit(64);
            }
            responsibility::reexec_disclaimed_if_needed();
            let gate_opts =
                platform_macos::permissions::GateOpts::from_env_and_flag(no_permissions_gate);
            if let Some((progress, context)) =
                platform_macos::permissions::gate::prepare_telemetry_context(gate_opts.opt_out)
            {
                if progress == platform_macos::permissions::GateProgress::Started {
                    telemetry::capture_permissions_gate_started(
                        context.missing_accessibility,
                        context.missing_screen_recording,
                    );
                }
            }
            if !platform_macos::permissions::gate::is_gate_reexec() {
                telemetry::capture_start(
                    telemetry::event::SERVE_START_LEGACY,
                    telemetry::Transport::Daemon,
                );
            }
            // Long-running daemon — kick off the background update check
            // before any blocking work so the banner can land on stderr
            // early in the serve lifecycle.
            version_check::maybe_announce_update();
            let pip_cfg = match pip_preview::default_config_path() {
                Some(p) => pip_preview::PipConfig::from_args_and_file(&p),
                None => pip_preview::PipConfig::from_args(),
            };
            maybe_init_pip();

            // Agent-cursor overlay. The DAEMON is the process that actually
            // performs clicks / AX presses, so the overlay NSWindow + render
            // loop must run HERE. The MCP proxy never renders, so the daemon
            // owns every cursor command and window. Init the channel before spawning
            // the serve thread so `run_on_main_thread()` always finds it ready.
            let cursor_cfg = cursor_overlay::CursorConfig::from_args();

            // Honour the compat flag forwarded by the MCP proxy
            // (launch_daemon_and_wait passes `serve
            // --claude-code-computer-use-compat`). The Serve arm is the daemon
            // the proxy talks to, so without this the proxy path always served
            // the full screenshot tool regardless of the client's request.
            let driver = build_driver(cursor_cfg.clone(), claude_code_compat);
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
                    serve::run_serve_cmd(driver, &sp, Some(&pid_path));
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
            let gate_result = platform_macos::permissions::run_if_needed_with_observer(
                gate_opts,
                |progress, context| match progress {
                    platform_macos::permissions::GateProgress::Started => {
                        telemetry::capture_permissions_gate_started(
                            context.missing_accessibility,
                            context.missing_screen_recording,
                        );
                    }
                    platform_macos::permissions::GateProgress::Dismissed => {
                        telemetry::capture_permissions_gate_dismissed(
                            context.missing_accessibility,
                            context.missing_screen_recording,
                            context.elapsed,
                        );
                    }
                },
            );
            let gate_context = platform_macos::permissions::gate::telemetry_context();
            if gate_context.engaged {
                telemetry::capture_permissions_gate_completed(
                    gate_context.missing_accessibility,
                    gate_context.missing_screen_recording,
                    gate_context.panel_shown,
                    gate_context.dismissed,
                    telemetry::permissions_gate_resolution(
                        gate_result.is_err(),
                        gate_context.dismissed,
                    ),
                    gate_context.elapsed,
                );
            }
            if let Err(e) = gate_result {
                eprintln!("[cua-driver] permissions gate: {e}");
                eprintln!(
                    "[cua-driver] continuing — tool calls touching AX or \
                           Screen Recording fail until you grant the missing TCC \
                           permissions."
                );
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
        }
        cli::Command::Stop { socket } => {
            let sp = socket.unwrap_or_else(serve::default_socket_path);
            serve::run_stop_cmd(&sp);
        }
        cli::Command::Revoke {
            socket,
            session,
            all,
        } => {
            let sp = socket.unwrap_or_else(serve::default_socket_path);
            serve::run_revoke_cmd(&sp, session.as_deref(), all);
        }
        cli::Command::Status { socket } => {
            let sp = socket.unwrap_or_else(serve::default_socket_path);
            let pid_path = serve::default_pid_file_path();
            serve::run_status_cmd(&sp, &pid_path);
        }
        cli::Command::Recording {
            subcommand,
            args,
            socket,
        } => {
            cli::run_recording_cmd(&subcommand, &args, socket.as_deref());
        }
        cli::Command::DumpDocs { pretty, doc_type } => {
            let tools = sdk_tool_inventory(build_driver_without_cursor());
            cli::run_dump_docs_with_type(&tools, pretty, &doc_type);
        }
        cli::Command::Update { apply, json } => {
            cli::run_update_cmd(apply, json);
        }
        cli::Command::CheckUpdate { json, no_cache } => {
            cli::run_check_update_cmd(json, no_cache);
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
        }
        cli::Command::Diagnose => {
            cli::run_diagnose_cmd();
        }
        cli::Command::Permissions { subcommand, json } => {
            cli::run_permissions_cmd(&subcommand, json);
        }
        cli::Command::Autostart { subcommand } => {
            autostart::run_autostart_cmd(&subcommand);
        }
        cli::Command::Skills { subcommand, flags } => {
            skills::run(&subcommand, &flags);
        }
        cli::Command::BrowserApprove {
            pid,
            strategy,
            window_id,
            session,
            profile_mode,
            profile_name,
        } => {
            cli::run_browser_approve(
                pid,
                strategy.as_deref(),
                window_id,
                session.as_deref(),
                profile_mode.as_deref(),
                profile_name.as_deref(),
            );
        }
        cli::Command::Config {
            subcommand,
            key,
            value,
            socket,
        } => {
            cli::run_config_cmd(
                subcommand.as_deref(),
                key.as_deref(),
                value.as_deref(),
                socket.as_deref(),
            );
        }
        cli::Command::Mcp {
            socket,
            claude_code_compat,
        } => {
            let startup_started = std::time::Instant::now();
            // Long-running MCP proxy — kick off the background update check
            // before connecting to or launching the daemon.
            version_check::maybe_announce_update();
            let direct = cua_driver_core::embedded_mode() && socket.is_none();
            let result = if direct {
                telemetry::capture_mcp_startup_completed(
                    "sdk_owned_runtime",
                    "not_applicable",
                    true,
                    startup_started.elapsed(),
                );
                run_mcp_direct(claude_code_compat)
            } else {
                cli::run_mcp_via_daemon_proxy(socket, claude_code_compat, |daemon, success| {
                    telemetry::capture_mcp_startup_completed(
                        "daemon_proxy",
                        daemon.telemetry_value(),
                        success,
                        startup_started.elapsed(),
                    )
                })
            };
            if let Err(e) = result {
                eprintln!("cua-driver-rs: {e}");
                telemetry::flush_pending(std::time::Duration::from_millis(750));
                std::process::exit(1);
            }
            telemetry::flush_pending(std::time::Duration::from_millis(750));
        }
    }
}

// ── Non-macOS entry-point ─────────────────────────────────────────────────

#[cfg(not(target_os = "macos"))]
fn main() -> anyhow::Result<()> {
    init_logging();
    if telemetry::run_cli_completion_worker_if_requested() {
        return Ok(());
    }
    if telemetry::run_lifecycle_worker_if_requested() {
        return Ok(());
    }
    if telemetry::run_update_event_worker_if_requested() {
        return Ok(());
    }
    maybe_wrap_finite_command();

    // ── CLI subcommand dispatch ──────────────────────────────────────────────
    // These commands create their own tokio runtimes internally, so they must
    // run on a plain OS thread — not inside a #[tokio::main] context which
    // would cause nested block_on panics.
    let command = cli::parse_command();
    if !telemetry::is_wrapped_cli_child() && !matches!(&command, cli::Command::Telemetry(_)) {
        telemetry::spawn_first_run_registration_worker();
    }
    match command {
        cli::Command::Telemetry(command) => {
            run_telemetry_command(command);
            return Ok(());
        }
        cli::Command::ListTools => {
            let tools = sdk_tool_inventory(build_driver_without_cursor());
            cli::run_list_tools(&tools);
            return Ok(());
        }
        cli::Command::Describe(name) => {
            let tools = sdk_tool_inventory(build_driver_without_cursor());
            cli::run_describe(&tools, &name);
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
        cli::Command::Call {
            tool,
            json_args,
            screenshot_out_file,
            socket,
        } => {
            cli::run_call(&tool, json_args, screenshot_out_file, socket);
            return Ok(());
        }
        cli::Command::Serve {
            socket,
            permission_mode,
            dangerously_bypass_approvals,
            allow_legacy_existing_profile_approval,
            session_policy,
            approve_session_policy,
            no_permissions_gate,
            claude_code_compat,
        } => {
            configure_startup_permission_mode(
                permission_mode.as_deref(),
                dangerously_bypass_approvals,
                allow_legacy_existing_profile_approval,
                session_policy.as_deref(),
                approve_session_policy,
            )?;
            responsibility::reexec_disclaimed_if_needed();
            telemetry::capture_start(
                telemetry::event::SERVE_START_LEGACY,
                telemetry::Transport::Daemon,
            );
            // Long-running daemon — kick off the background update check
            // before any blocking work so the banner can land on stderr.
            version_check::maybe_announce_update();
            // The Rust permissions gate is macOS-only (TCC concept).
            // On Windows / Linux the flag is silently accepted for
            // CLI uniformity and ignored. The Claude-Code compat screenshot
            // surface is accepted on every platform for CLI uniformity.
            let _ = no_permissions_gate;
            // Serve mode needs the cursor overlay just like MCP mode.
            let cursor_cfg = cursor_overlay::CursorConfig::from_args();
            let driver = build_driver(cursor_cfg, claude_code_compat);
            maybe_init_pip();
            let sp = socket.unwrap_or_else(serve::default_socket_path);
            let pid_path = serve::default_pid_file_path();
            // run_serve_cmd builds its own runtime; must run on a fresh thread.
            std::thread::spawn(move || {
                serve::run_serve_cmd(driver, &sp, Some(&pid_path));
            })
            .join()
            .ok();
            return Ok(());
        }
        cli::Command::Stop { socket } => {
            let sp = socket.unwrap_or_else(serve::default_socket_path);
            serve::run_stop_cmd(&sp);
            return Ok(());
        }
        cli::Command::Revoke {
            socket,
            session,
            all,
        } => {
            let sp = socket.unwrap_or_else(serve::default_socket_path);
            serve::run_revoke_cmd(&sp, session.as_deref(), all);
            return Ok(());
        }
        cli::Command::Status { socket } => {
            let sp = socket.unwrap_or_else(serve::default_socket_path);
            let pid_path = serve::default_pid_file_path();
            serve::run_status_cmd(&sp, &pid_path);
            return Ok(());
        }
        cli::Command::Recording {
            subcommand,
            args,
            socket,
        } => {
            cli::run_recording_cmd(&subcommand, &args, socket.as_deref());
            return Ok(());
        }
        cli::Command::DumpDocs { pretty, doc_type } => {
            let tools = sdk_tool_inventory(build_driver_without_cursor());
            cli::run_dump_docs_with_type(&tools, pretty, &doc_type);
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
            cli::run_diagnose_cmd();
            return Ok(());
        }
        cli::Command::Permissions { subcommand, json } => {
            cli::run_permissions_cmd(&subcommand, json);
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
        cli::Command::BrowserApprove {
            pid,
            strategy,
            window_id,
            session,
            profile_mode,
            profile_name,
        } => {
            cli::run_browser_approve(
                pid,
                strategy.as_deref(),
                window_id,
                session.as_deref(),
                profile_mode.as_deref(),
                profile_name.as_deref(),
            );
            return Ok(());
        }
        cli::Command::Config {
            subcommand,
            key,
            value,
            socket,
        } => {
            cli::run_config_cmd(
                subcommand.as_deref(),
                key.as_deref(),
                value.as_deref(),
                socket.as_deref(),
            );
            return Ok(());
        }
        cli::Command::Mcp {
            socket,
            claude_code_compat,
        } => {
            let startup_started = std::time::Instant::now();
            // Long-running MCP proxy — kick off the background update check
            // before connecting to the daemon.
            version_check::maybe_announce_update();
            let direct = socket.is_none();
            let result = if direct {
                telemetry::capture_mcp_startup_completed(
                    "sdk_owned_runtime",
                    "not_applicable",
                    true,
                    startup_started.elapsed(),
                );
                run_mcp_direct(claude_code_compat)
            } else {
                cli::run_mcp_via_daemon_proxy(socket, claude_code_compat, |daemon, success| {
                    telemetry::capture_mcp_startup_completed(
                        "daemon_proxy",
                        daemon.telemetry_value(),
                        success,
                        startup_started.elapsed(),
                    )
                })
            };
            if let Err(e) = result {
                eprintln!("cua-driver-rs: {e}");
                telemetry::flush_pending(std::time::Duration::from_millis(750));
                std::process::exit(1);
            }
            telemetry::flush_pending(std::time::Duration::from_millis(750));
            return Ok(());
        }
    }
}
