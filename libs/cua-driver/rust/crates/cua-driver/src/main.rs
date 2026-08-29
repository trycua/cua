#![recursion_limit = "256"]

//! cua-driver-rs — cross-platform background computer-use automation daemon.
//!
//! Runs a daemon-backed MCP JSON-RPC 2.0 proxy over stdio. The platform
//! backend lives in the `serve` daemon selected at compile time.
//!
//! Extra CLI flags (consumed here, not by MCP):
//!   --cursor-theme <installed-theme-id>   installed cursor theme
//!   --cursor-reduced-motion <auto|on|off> accessibility motion preference
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
mod history_runtime;
mod mcp_http;
mod private_worker;
mod proxy;
mod release_channel;
mod responsibility;
mod sdk_adapter;
mod serve;
mod skills;
mod stop;
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
    capability_manifest: Option<&str>,
    approve_capability_manifest: bool,
    grants: &[String],
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
    if let Some(path) = capability_manifest {
        std::env::set_var(
            cua_driver_core::session_manifest::CAPABILITY_MANIFEST_FILE_ENV,
            path,
        );
    }
    if approve_capability_manifest {
        std::env::set_var(
            cua_driver_core::session_manifest::CAPABILITY_MANIFEST_APPROVED_ENV,
            "1",
        );
    }
    let mode =
        cua_driver_core::authorization::configured_permission_mode().map_err(anyhow::Error::msg)?;
    if !grants.is_empty() && mode != cua_driver_core::authorization::PermissionMode::Standard {
        anyhow::bail!("--grant is valid only in standard permission mode");
    }
    cua_driver_core::authorization::configure_launch_grants(grants).map_err(anyhow::Error::msg)?;
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

fn run_cursor_theme_command(args: &[String]) -> ! {
    let executable = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("cua-driver: cannot locate cursor-theme compiler: {error}");
            std::process::exit(1);
        }
    };
    let binary_name = if cfg!(target_os = "windows") {
        "cua-cursor-theme.exe"
    } else {
        "cua-cursor-theme"
    };
    let sidecar = executable
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join(binary_name);
    let status = match std::process::Command::new(&sidecar).args(args).status() {
        Ok(status) => status,
        Err(error) => {
            eprintln!(
                "cua-driver: cursor-theme compiler is unavailable at {}: {error}",
                sidecar.display()
            );
            eprintln!("Reinstall Cua Driver so the matching authoring sidecar is present.");
            std::process::exit(1);
        }
    };
    std::process::exit(status.code().unwrap_or(1));
}

/// Wire up the multi-target Agent View.
///
/// Called from every long-running entry point (Serve and Mcp on all
/// platforms; the `Call` arm intentionally skips PiP since the
/// per-call binaries don't keep an AppKit/event loop alive long
/// enough to be useful).
///
fn maybe_init_pip() -> bool {
    let cfg = match pip_preview::default_config_path() {
        Some(p) => pip_preview::PipConfig::from_args_and_file(&p),
        None => pip_preview::PipConfig::from_args(),
    };
    if !cfg.enabled {
        return false;
    }

    // AppKit's shared application registration aborts the process when the
    // daemon has no Window Server session (for example, an SSH-launched or
    // headless service). Do not select the Agent View run loop unless the
    // native surface can safely exist; the socket-serving thread must remain
    // the main lifetime owner in that case.
    #[cfg(target_os = "macos")]
    if !platform_macos::session::has_graphic_access() {
        eprintln!("⚗️  Agent View requested but unavailable: no Window Server graphic session");
        return false;
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
            cua_driver_core::pip_hook::set_pip_event_fn(|event| {
                if let Some(slot) = BACKEND.get() {
                    if let Some(b) = slot.lock().unwrap().as_ref() {
                        match event {
                            cua_driver_core::pip_hook::PipHookEvent::Upsert(frame) => {
                                let target_kind = match frame.target.target_kind {
                                    cua_driver_core::pip_hook::PipHookTargetKind::NativeWindow => {
                                        pip_preview::PipTargetKind::NativeWindow
                                    }
                                    cua_driver_core::pip_hook::PipHookTargetKind::BrowserTab => {
                                        pip_preview::PipTargetKind::BrowserTab
                                    }
                                };
                                b.push_frame(pip_preview::PipFrame {
                                    target: pip_preview::PipTarget {
                                        workspace_id: frame.target.workspace_id,
                                        workspace_label: frame.target.workspace_label,
                                        target_id: frame.target.target_id,
                                        identity_key: frame.target.identity_key,
                                        target_kind,
                                        target_label: frame.target.target_label,
                                        native_container: frame.target.native_container.map(
                                            |container| pip_preview::PipNativeContainer {
                                                pid: container.pid,
                                                window_id: container.window_id,
                                            },
                                        ),
                                    },
                                    png_bytes: frame.png_bytes,
                                    action_label: frame.action_label,
                                    timestamp_ms: frame.timestamp_ms,
                                    cursor_position: frame.cursor_position,
                                });
                            }
                            cua_driver_core::pip_hook::PipHookEvent::SetInputPassthrough {
                                passthrough,
                            } => {
                                return b
                                    .set_input_passthrough(passthrough)
                                    .map_err(|error| error.to_string());
                            }
                            cua_driver_core::pip_hook::PipHookEvent::RemoveWorkspace {
                                workspace_id,
                            } => b.remove_workspace(&workspace_id),
                            cua_driver_core::pip_hook::PipHookEvent::RemoveTarget {
                                workspace_id,
                                identity_key,
                            } => b.remove_target(&workspace_id, &identity_key),
                        }
                    }
                }
                Ok(())
            });
            cua_driver_core::session::register_session_end_hook(|workspace_id| {
                cua_driver_core::pip_hook::remove_pip_workspace(workspace_id);
            });
            eprintln!("⚗️  Agent View enabled (native macOS, Windows, and Linux presentation)");
            true
        }
        Err(e) => {
            eprintln!("⚗️  Agent View requested but unavailable: {e}");
            false
        }
    }
}

// ── Public SDK runtime host ──────────────────────────────────────────────

/// Construct the canonical SDK-owned runtime for the CLI or daemon host.
/// The private socket and MCP layers consume this object downstream.
fn build_driver(
    cursor: cursor_overlay::CursorConfig,
    compatibility_mode: bool,
    host_owns_permission_ux: bool,
) -> Result<Arc<cua_driver_sdk::CuaDriver>, cua_driver_sdk::DriverError> {
    cua_driver_sdk::CuaDriver::try_create_service_for_host(cua_driver_sdk::DriverHostOptions {
        cursor,
        host_owns_permission_ux,
        host_bundle_id: std::env::var(cua_driver_core::HOST_BUNDLE_ID_ENV).ok(),
        claude_code_compatibility: compatibility_mode,
        prepare_desktop_environment: true,
        register_host_tools: Some(history_runtime::register_host_tools),
        authorization_host: None,
        activity_observer: None,
    })
}

#[cfg(test)]
fn build_driver_without_cursor() -> Arc<cua_driver_sdk::CuaDriver> {
    build_driver(
        cursor_overlay::CursorConfig {
            enabled: false,
            ..cursor_overlay::CursorConfig::default()
        },
        false,
        false,
    )
    .expect("test host requires an available desktop runtime")
}

/// Load the canonical SDK inventory without constructing an action runtime.
///
/// Finite metadata commands remain usable from non-interactive Windows
/// sessions, while `serve`, MCP, and direct SDK creation still fail closed
/// before accepting desktop actions.
fn inspect_tools_without_runtime() -> serde_json::Value {
    cua_driver_sdk::CuaDriver::inspect_host_tools(cua_driver_sdk::DriverHostOptions {
        cursor: cursor_overlay::CursorConfig {
            enabled: false,
            ..cursor_overlay::CursorConfig::default()
        },
        host_owns_permission_ux: false,
        host_bundle_id: None,
        claude_code_compatibility: false,
        prepare_desktop_environment: false,
        register_host_tools: Some(history_runtime::register_host_tools),
        authorization_host: None,
        activity_observer: None,
    })
}

#[cfg(test)]
fn test_runtime_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

fn run_mcp_direct(compatibility_mode: bool) -> anyhow::Result<()> {
    // Validate immutable process policy before platform initialization. The
    // adapter repeats this check before reading stdin as defense in depth.
    cua_driver_core::authorization::validate_startup_authorization()?;
    cua_driver_core::policy::validate_configured_policy()?;
    let cursor = cursor_overlay::CursorConfig::from_args();
    // A plain stdio MCP process does not provide the required AppKit
    // main-thread host adapter. Explicit direct mode on macOS must therefore
    // expose facility_unavailable instead of initializing an overlay that can
    // report success without a usable UI owner. Private-worker and app-service
    // hosts keep the full facility.
    #[cfg(target_os = "macos")]
    let cursor = {
        let mut cursor = cursor;
        cursor.enabled = false;
        cursor
    };
    let driver = build_driver(cursor, compatibility_mode, true)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(proxy::run_direct(driver))
}

fn history_admission_requested(explicit: bool, persisted: bool) -> bool {
    explicit || persisted
}

#[cfg(target_os = "macos")]
#[derive(Debug, PartialEq, Eq)]
enum MacosMainLoopHost {
    CursorOverlay,
    AgentView,
    ServeThread,
}

#[cfg(target_os = "macos")]
fn macos_main_loop_host(cursor_enabled: bool, agent_view_enabled: bool) -> MacosMainLoopHost {
    if cursor_enabled {
        // The cursor host also owns NSApplication's main run loop. Agent View
        // posts its window updates to that same queue, so both surfaces can
        // coexist without leaving cursor animations waiting forever.
        MacosMainLoopHost::CursorOverlay
    } else if agent_view_enabled {
        MacosMainLoopHost::AgentView
    } else {
        MacosMainLoopHost::ServeThread
    }
}

#[cfg(test)]
mod history_admission_tests {
    use super::history_admission_requested;

    #[test]
    fn persisted_preview_admission_survives_a_relaunch_without_the_cli_flag() {
        assert!(history_admission_requested(false, true));
    }

    #[test]
    fn admission_requires_an_explicit_or_persisted_request() {
        assert!(!history_admission_requested(false, false));
        assert!(history_admission_requested(true, false));
        assert!(history_admission_requested(true, true));
    }
}

#[cfg(all(test, target_os = "macos"))]
mod macos_main_loop_host_tests {
    use super::{macos_main_loop_host, MacosMainLoopHost};

    #[test]
    fn cursor_overlay_hosts_the_shared_appkit_loop_when_agent_view_is_enabled() {
        assert_eq!(
            macos_main_loop_host(true, true),
            MacosMainLoopHost::CursorOverlay
        );
    }

    #[test]
    fn agent_view_hosts_appkit_only_when_the_cursor_is_disabled() {
        assert_eq!(
            macos_main_loop_host(false, true),
            MacosMainLoopHost::AgentView
        );
        assert_eq!(
            macos_main_loop_host(false, false),
            MacosMainLoopHost::ServeThread
        );
    }
}

fn mcp_uses_direct_runtime(socket: Option<&str>, direct: bool) -> anyhow::Result<bool> {
    mcp_uses_direct_runtime_for(
        cua_driver_core::embedded_mode(),
        socket,
        cfg!(target_os = "macos"),
        direct,
        history_runtime::preview_admitted_preference(),
    )
}

fn mcp_uses_direct_runtime_for(
    embedded: bool,
    socket: Option<&str>,
    macos: bool,
    direct: bool,
    history_preview_admitted: bool,
) -> anyhow::Result<bool> {
    if direct && socket.is_some() {
        anyhow::bail!("--direct and --socket are mutually exclusive");
    }
    if direct {
        return Ok(true);
    }
    if embedded && socket.is_none() {
        anyhow::bail!("embedded hosts must provide their private service endpoint with --socket");
    }
    if macos {
        // Preserve LaunchServices/TCC attribution for normal macOS clients.
        Ok(false)
    } else {
        Ok(socket.is_none() && !history_preview_admitted)
    }
}

#[cfg(test)]
mod mcp_runtime_selection_tests {
    use super::mcp_uses_direct_runtime_for;

    #[test]
    fn embedded_host_without_private_endpoint_fails_closed() {
        let error = mcp_uses_direct_runtime_for(true, None, false, false, false).unwrap_err();
        assert!(error.to_string().contains("--socket"));
        let error = mcp_uses_direct_runtime_for(true, None, true, false, false).unwrap_err();
        assert!(error.to_string().contains("--socket"));
    }

    #[test]
    fn normal_linux_and_windows_stdio_own_the_runtime() {
        assert!(mcp_uses_direct_runtime_for(false, None, false, false, false).unwrap());
        assert!(!mcp_uses_direct_runtime_for(false, Some("service"), false, false, false).unwrap());
        assert!(!mcp_uses_direct_runtime_for(false, None, false, false, true).unwrap());
    }

    #[test]
    fn normal_macos_stdio_preserves_the_service_boundary() {
        assert!(!mcp_uses_direct_runtime_for(false, None, true, false, false).unwrap());
        assert!(!mcp_uses_direct_runtime_for(false, Some("service"), true, false, false).unwrap());
    }

    #[test]
    fn explicit_direct_owns_the_runtime_on_macos_and_in_embedded_hosts() {
        assert!(mcp_uses_direct_runtime_for(false, None, true, true, false).unwrap());
        assert!(mcp_uses_direct_runtime_for(true, None, true, true, false).unwrap());
        assert!(mcp_uses_direct_runtime_for(true, None, false, true, false).unwrap());
        let error =
            mcp_uses_direct_runtime_for(false, Some("service"), true, true, false).unwrap_err();
        assert!(error.to_string().contains("mutually exclusive"));
    }
}

// ── macOS entry-point ─────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn main() {
    if let Some(code) = platform_macos::permissions::gate::run_permission_probe_if_requested() {
        std::process::exit(code);
    }
    // The packaged uninstaller needs a truly offline, pre-telemetry purge
    // path while this exact signed executable still exists on disk.
    if let Some(code) = history_runtime::run_offline_purge_if_requested() {
        std::process::exit(code);
    }
    init_logging();
    if let Some(code) = cli::run_permissions_host_request_if_requested() {
        std::process::exit(code);
    }
    if let Some(generation) = private_worker::requested_generation() {
        let (initialized_tx, initialized_rx) = std::sync::mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let code = match private_worker::run(generation, Some(initialized_tx)) {
                Ok(()) => 0,
                Err(error) => {
                    eprintln!("cua-driver private worker: {error}");
                    1
                }
            };
            // AppKit's event loop is process-long. This directly supervised
            // child has no reusable endpoint, so protocol completion owns the
            // worker process lifetime.
            std::process::exit(code);
        });
        if initialized_rx.recv().unwrap_or(false) {
            platform_macos::cursor::overlay::run_on_main_thread();
        }
        return;
    }
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
            let tools = inspect_tools_without_runtime();
            cli::run_list_tools(&tools);
        }
        cli::Command::Describe(name) => {
            let tools = inspect_tools_without_runtime();
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
            capability_manifest,
            approve_capability_manifest,
            no_permissions_gate,
            claude_code_compat,
            grants,
            experimental_history,
        } => {
            if let Err(error) = configure_startup_permission_mode(
                permission_mode.as_deref(),
                dangerously_bypass_approvals,
                capability_manifest.as_deref(),
                approve_capability_manifest,
                &grants,
            ) {
                eprintln!("cua-driver: authorization startup error: {error}");
                std::process::exit(64);
            }
            responsibility::reexec_disclaimed_if_needed();
            if let Err(error) = history_runtime::configure_admission(history_admission_requested(
                experimental_history,
                history_runtime::preview_admitted_preference(),
            )) {
                eprintln!("cua-driver: Computer History admission error: {error}");
                std::process::exit(1);
            }
            history_runtime::configure_daemon_launch_state(
                permission_mode.as_deref(),
                dangerously_bypass_approvals,
                capability_manifest.as_deref(),
                approve_capability_manifest,
                no_permissions_gate,
                claude_code_compat,
                &grants,
            );
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
            // Fail closed until a fresh helper-process probe completes. This
            // also covers a probe launch failure without letting the serving
            // process perform and cache its own negative TCC preflight.
            serve::set_permission_gate_pending(!gate_opts.opt_out);
            telemetry::capture_start(
                telemetry::event::SERVE_START_LEGACY,
                telemetry::Transport::Daemon,
            );
            // Long-running daemon — kick off the background update check
            // before any blocking work so the banner can land on stderr
            // early in the serve lifecycle.
            version_check::maybe_announce_update();
            let pip_available = maybe_init_pip();

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
            let driver = match build_driver(
                cursor_cfg.clone(),
                claude_code_compat,
                cua_driver_core::embedded_mode(),
            ) {
                Ok(driver) => driver,
                Err(error) => {
                    eprintln!("cua-driver: cannot create desktop runtime: {error}");
                    std::process::exit(1);
                }
            };
            let sp = socket.unwrap_or_else(serve::default_socket_path);
            let pid_path = serve::default_pid_file_path();

            // Bind the Unix socket FIRST, on a background thread, BEFORE
            // running the (blocking) permissions gate (#1761).
            //
            // The gate's `wait_for_grants` blocks while `com.trycua.driver`
            // is ungranted. Fresh helper processes poll TCC until the user
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
            // stays on the MAIN thread for its NSPanel; short-lived helper
            // processes own prompt and status APIs. The serving process never performs
            // a negative TCC preflight, so its socket and accepted connections
            // remain stable while helper processes refresh permission state.
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
            if gate_result.is_ok() {
                serve::set_permission_gate_pending(false);
            }
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
                    "[cua-driver] desktop tool calls remain gated; grant Accessibility and \
                     Screen Recording permissions, then restart the daemon."
                );
            }

            // Keep the main thread alive for the daemon.
            //
            // Cursor and Agent View share one AppKit application/main queue.
            // When both are enabled, the cursor host must drain its command
            // receiver as well as NSApplication events; the Agent View-only
            // loop would leave cursor-bearing actions waiting indefinitely.
            match macos_main_loop_host(cursor_cfg.enabled, pip_available) {
                MacosMainLoopHost::CursorOverlay => {
                    // `run_on_main_thread` self-guards on graphic-session
                    // access. If it returns, keep the daemon alive by joining
                    // the serve thread.
                    platform_macos::cursor::overlay::run_on_main_thread();
                    let _ = serve_handle.join();
                }
                MacosMainLoopHost::AgentView => {
                    platform_macos::pip::run_appkit_main_loop();
                }
                MacosMainLoopHost::ServeThread => {
                    let _ = serve_handle.join();
                }
            }
        }
        cli::Command::Stop {
            socket,
            expected_pid,
        } => {
            let sp = socket.unwrap_or_else(serve::default_socket_path);
            match expected_pid {
                Some(pid) => stop::run_pid_bound_stop_cmd(&sp, pid),
                None => serve::run_stop_cmd(&sp),
            }
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
        cli::Command::Sessions { json, socket } => {
            let sp = socket.unwrap_or_else(serve::default_socket_path);
            serve::run_sessions_list_cmd(&sp, json);
        }
        cli::Command::Recording {
            subcommand,
            args,
            socket,
        } => {
            cli::run_recording_cmd(&subcommand, &args, socket.as_deref());
        }
        cli::Command::History {
            subcommand,
            args,
            socket,
            json,
            confirmed,
        } => {
            cli::run_history_cmd(&subcommand, &args, socket.as_deref(), json, confirmed);
        }
        cli::Command::DumpDocs { pretty, doc_type } => {
            let tools = inspect_tools_without_runtime();
            cli::run_dump_docs_with_type(&tools, pretty, &doc_type);
        }
        cli::Command::Update { apply, json } => {
            cli::run_update_cmd(apply, json);
        }
        cli::Command::CheckUpdate { json, no_cache } => {
            cli::run_check_update_cmd(json, no_cache);
        }
        cli::Command::Channel {
            subcommand,
            value,
            json,
        } => {
            cli::run_channel_cmd(&subcommand, value.as_deref(), json);
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
        cli::Command::CursorTheme { args } => {
            run_cursor_theme_command(&args);
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
            direct,
            claude_code_compat,
            grants,
        } => {
            let startup_started = std::time::Instant::now();
            // Long-running MCP proxy — kick off the background update check
            // before connecting to or launching the daemon.
            version_check::maybe_announce_update();
            let result = match mcp_uses_direct_runtime(socket.as_deref(), direct) {
                Ok(true) => {
                    if let Err(error) =
                        configure_startup_permission_mode(None, false, None, false, &grants)
                    {
                        Err(error)
                    } else {
                        telemetry::capture_mcp_startup_completed(
                            "sdk_owned_runtime",
                            "not_applicable",
                            true,
                            startup_started.elapsed(),
                        );
                        run_mcp_direct(claude_code_compat)
                    }
                }
                Err(error) => Err(error),
                Ok(false) => cli::run_mcp_via_daemon_proxy(
                    socket,
                    claude_code_compat,
                    &grants,
                    |daemon, success| {
                        telemetry::capture_mcp_startup_completed(
                            "daemon_proxy",
                            daemon.telemetry_value(),
                            success,
                            startup_started.elapsed(),
                        )
                    },
                ),
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
    if let Some(code) = history_runtime::run_offline_purge_if_requested() {
        std::process::exit(code);
    }
    init_logging();
    if let Some(generation) = private_worker::requested_generation() {
        return private_worker::run(generation, None);
    }
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
            let tools = inspect_tools_without_runtime();
            cli::run_list_tools(&tools);
            return Ok(());
        }
        cli::Command::Describe(name) => {
            let tools = inspect_tools_without_runtime();
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
            capability_manifest,
            approve_capability_manifest,
            no_permissions_gate,
            claude_code_compat,
            grants,
            experimental_history,
        } => {
            configure_startup_permission_mode(
                permission_mode.as_deref(),
                dangerously_bypass_approvals,
                capability_manifest.as_deref(),
                approve_capability_manifest,
                &grants,
            )?;
            responsibility::reexec_disclaimed_if_needed();
            history_runtime::configure_admission(history_admission_requested(
                experimental_history,
                history_runtime::preview_admitted_preference(),
            ))?;
            history_runtime::configure_daemon_launch_state(
                permission_mode.as_deref(),
                dangerously_bypass_approvals,
                capability_manifest.as_deref(),
                approve_capability_manifest,
                no_permissions_gate,
                claude_code_compat,
                &grants,
            );
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
            let driver = build_driver(
                cursor_cfg,
                claude_code_compat,
                cua_driver_core::embedded_mode(),
            )?;
            let _ = maybe_init_pip();
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
        cli::Command::Stop {
            socket,
            expected_pid,
        } => {
            let sp = socket.unwrap_or_else(serve::default_socket_path);
            match expected_pid {
                Some(pid) => stop::run_pid_bound_stop_cmd(&sp, pid),
                None => serve::run_stop_cmd(&sp),
            }
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
        cli::Command::Sessions { json, socket } => {
            let sp = socket.unwrap_or_else(serve::default_socket_path);
            serve::run_sessions_list_cmd(&sp, json);
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
        cli::Command::History {
            subcommand,
            args,
            socket,
            json,
            confirmed,
        } => {
            cli::run_history_cmd(&subcommand, &args, socket.as_deref(), json, confirmed);
            return Ok(());
        }
        cli::Command::DumpDocs { pretty, doc_type } => {
            let tools = inspect_tools_without_runtime();
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
        cli::Command::Channel {
            subcommand,
            value,
            json,
        } => {
            cli::run_channel_cmd(&subcommand, value.as_deref(), json);
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
        cli::Command::CursorTheme { args } => {
            run_cursor_theme_command(&args);
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
            direct,
            claude_code_compat,
            grants,
        } => {
            let startup_started = std::time::Instant::now();
            // Long-running MCP proxy — kick off the background update check
            // before connecting to the daemon.
            version_check::maybe_announce_update();
            let result = match mcp_uses_direct_runtime(socket.as_deref(), direct) {
                Ok(true) => {
                    configure_startup_permission_mode(None, false, None, false, &grants)?;
                    telemetry::capture_mcp_startup_completed(
                        "sdk_owned_runtime",
                        "not_applicable",
                        true,
                        startup_started.elapsed(),
                    );
                    run_mcp_direct(claude_code_compat)
                }
                Err(error) => Err(error),
                Ok(false) => cli::run_mcp_via_daemon_proxy(
                    socket,
                    claude_code_compat,
                    &grants,
                    |daemon, success| {
                        telemetry::capture_mcp_startup_completed(
                            "daemon_proxy",
                            daemon.telemetry_value(),
                            success,
                            startup_started.elapsed(),
                        )
                    },
                ),
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
