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

mod cli;
mod serve;

use std::sync::Arc;

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

// ── macOS entry-point ─────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn main() {
    init_logging();

    // ── CLI subcommand dispatch ──────────────────────────────────────────────
    // Handled before AppKit init so `list-tools` / `describe` / `call` exit
    // cleanly without starting the overlay or NSApplication.
    match cli::parse_command() {
        cli::Command::ListTools => {
            let reg = Arc::new(platform_macos::register_tools());
            cli::run_list_tools(&reg);
            return;
        }
        cli::Command::Describe(name) => {
            let reg = Arc::new(platform_macos::register_tools());
            cli::run_describe(&reg, &name);
            return;
        }
        cli::Command::McpConfig { client } => {
            cli::run_mcp_config(client.as_deref());
            return;
        }
        cli::Command::Call { tool, json_args, screenshot_out_file } => {
            // Register callbacks (needed if the tool does screenshots/recording).
            mcp_server::recording::set_screenshot_fn(|window_id, pid| {
                if let Some(wid) = window_id {
                    platform_macos::capture::screenshot_window_bytes(wid as u32).ok()
                } else if let Some(p) = pid {
                    platform_macos::windows::resolve_main_window_id(p as i32).ok()
                        .and_then(|wid| platform_macos::capture::screenshot_window_bytes(wid).ok())
                } else {
                    platform_macos::capture::screenshot_display_bytes().ok()
                }
            });
            mcp_server::recording::set_click_marker_fn(|png_bytes, cx, cy| {
                platform_macos::capture::crosshair_png_bytes(png_bytes, cx, cy).ok()
            });
            let reg = Arc::new(platform_macos::register_tools());
            reg.init_self_weak();
            cli::run_call(reg, &tool, json_args, screenshot_out_file);
            return;
        }
        cli::Command::Serve { socket } => {
            mcp_server::recording::set_screenshot_fn(|window_id, pid| {
                if let Some(wid) = window_id {
                    platform_macos::capture::screenshot_window_bytes(wid as u32).ok()
                } else if let Some(p) = pid {
                    platform_macos::windows::resolve_main_window_id(p as i32).ok()
                        .and_then(|wid| platform_macos::capture::screenshot_window_bytes(wid).ok())
                } else {
                    platform_macos::capture::screenshot_display_bytes().ok()
                }
            });
            mcp_server::recording::set_click_marker_fn(|png_bytes, cx, cy| {
                platform_macos::capture::crosshair_png_bytes(png_bytes, cx, cy).ok()
            });
            let reg = Arc::new(platform_macos::register_tools());
            reg.init_self_weak();
            let sp = socket.unwrap_or_else(serve::default_socket_path);
            let pid_path = serve::default_pid_file_path();
            serve::run_serve_cmd(reg, &sp, Some(&pid_path));
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
            let reg = Arc::new(platform_macos::register_tools());
            cli::run_dump_docs_with_type(&reg, pretty, &doc_type);
            return;
        }
        cli::Command::Update { apply } => {
            cli::run_update_cmd(apply);
            return;
        }
        cli::Command::Doctor => {
            cli::run_doctor_cmd();
            return;
        }
        cli::Command::Diagnose => {
            let reg = Arc::new(platform_macos::register_tools());
            cli::run_diagnose_cmd(reg);
            return;
        }
        cli::Command::Config { subcommand, key, value, socket } => {
            let reg = Arc::new(platform_macos::register_tools());
            reg.init_self_weak();
            cli::run_config_cmd(reg, subcommand.as_deref(), key.as_deref(), value.as_deref(), socket.as_deref());
            return;
        }
        cli::Command::Mcp => {} // fall through to MCP server startup below
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
    mcp_server::recording::set_screenshot_fn(|window_id, pid| {
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
    mcp_server::recording::set_click_marker_fn(|png_bytes, cx, cy| {
        platform_macos::capture::crosshair_png_bytes(png_bytes, cx, cy).ok()
    });

    std::thread::Builder::new()
        .name("cua-mcp".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            rt.block_on(async move {
                // Register tools; overlay init has already happened above.
                let registry = Arc::new(platform_macos::register_tools());
                // Wire up replay tool's back-reference to the registry.
                registry.init_self_weak();
                if let Err(e) = mcp_server::server::run(registry).await {
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
    match cli::parse_command() {
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
        cli::Command::Call { tool, json_args, screenshot_out_file } => {
            let reg = Arc::new(build_registry_no_cursor());
            reg.init_self_weak();
            // run_call builds its own tokio runtime; must run on a fresh thread.
            std::thread::spawn(move || {
                cli::run_call(reg, &tool, json_args, screenshot_out_file);
            }).join().ok();
            return Ok(());
        }
        cli::Command::Serve { socket } => {
            // Serve mode needs the cursor overlay just like MCP mode.
            let cursor_cfg = cursor_overlay::CursorConfig::from_args();
            let reg = Arc::new(build_registry(cursor_cfg));
            reg.init_self_weak();
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
        cli::Command::Update { apply } => {
            cli::run_update_cmd(apply);
            return Ok(());
        }
        cli::Command::Doctor => {
            cli::run_doctor_cmd();
            return Ok(());
        }
        cli::Command::Diagnose => {
            let reg = Arc::new(build_registry_no_cursor());
            cli::run_diagnose_cmd(reg);
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
        cli::Command::Mcp => {} // fall through to MCP server startup below
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
    mcp_server::server::run(registry).await?;
    Ok(())
}

// ── Registry builder (non-macOS) ──────────────────────────────────────────

#[cfg(not(target_os = "macos"))]
fn build_registry(cursor_cfg: cursor_overlay::CursorConfig) -> mcp_server::tool::ToolRegistry {
    #[cfg(target_os = "windows")]
    {
        mcp_server::recording::set_screenshot_fn(|window_id, pid| {
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
        mcp_server::recording::set_click_marker_fn(|png_bytes, cx, cy| {
            platform_windows::capture::crosshair_png_bytes(png_bytes, cx, cy).ok()
        });
        platform_windows::register_tools_with_cursor(cursor_cfg)
    }
    #[cfg(target_os = "linux")]
    {
        mcp_server::recording::set_screenshot_fn(|window_id, pid| {
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
        mcp_server::recording::set_click_marker_fn(|png_bytes, cx, cy| {
            platform_linux::capture::crosshair_png_bytes(png_bytes, cx, cy).ok()
        });
        platform_linux::register_tools_with_cursor(cursor_cfg)
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        let _ = cursor_cfg;
        let mut r = mcp_server::tool::ToolRegistry::new();
        r.register(Box::new(crate::stub::UnsupportedPlatformTool));
        r
    }
}

/// Build a registry without initialising the cursor overlay.
/// Used by CLI subcommands (list-tools / describe / call) that don't need the overlay.
#[cfg(not(target_os = "macos"))]
fn build_registry_no_cursor() -> mcp_server::tool::ToolRegistry {
    #[cfg(target_os = "windows")]
    {
        mcp_server::recording::set_screenshot_fn(|window_id, pid| {
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
        mcp_server::recording::set_click_marker_fn(|png_bytes, cx, cy| {
            platform_windows::capture::crosshair_png_bytes(png_bytes, cx, cy).ok()
        });
        platform_windows::register_tools_with_cursor(cursor_overlay::CursorConfig { enabled: false, ..Default::default() })
    }
    #[cfg(target_os = "linux")]
    {
        mcp_server::recording::set_screenshot_fn(|window_id, pid| {
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
        mcp_server::recording::set_click_marker_fn(|png_bytes, cx, cy| {
            platform_linux::capture::crosshair_png_bytes(png_bytes, cx, cy).ok()
        });
        platform_linux::register_tools_with_cursor(cursor_overlay::CursorConfig { enabled: false, ..Default::default() })
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        let mut r = mcp_server::tool::ToolRegistry::new();
        r.register(Box::new(crate::stub::UnsupportedPlatformTool));
        r
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
mod stub {
    use async_trait::async_trait;
    use mcp_server::tool::{Tool, ToolDef, ToolResult};
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
