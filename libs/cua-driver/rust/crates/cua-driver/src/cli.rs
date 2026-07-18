//! CLI subcommand parsing and execution.
//!
//! Subcommand dispatch (mirrors the Swift cua-driver CLI):
//!
//!   cua-driver                              → mcp server (default)
//!   cua-driver mcp                          → mcp server (explicit)
//!   cua-driver list-tools                   → print all tool names + descriptions
//!   cua-driver describe <tool>              → print tool schema
//!   cua-driver call <tool> [json-args]      → invoke tool, print result
//!   cua-driver <tool> [json-args]           → shorthand for call (snake_case names)
//!
//! Cursor-overlay flags (--cursor-id, --no-overlay, etc.) are consumed by
//! `CursorConfig::from_args()` and are ignored here.

use cua_driver_core::{protocol::Content, tool::ToolRegistry};
use std::process;

/// Which CLI command was requested.
pub enum Command {
    Mcp {
        /// Force in-process MCP execution — skip the TCC auto-relaunch
        /// path that would spawn a daemon via `open -n -g -a CuaDriver
        /// --args serve` and proxy stdio MCP requests through its Unix
        /// socket. Useful when the calling context already has the right
        /// TCC grants (CuaDriver.app launched us directly), or when
        /// diagnosing in-process failures. Also toggleable via
        /// `CUA_DRIVER_RS_MCP_NO_RELAUNCH=1`.
        no_daemon_relaunch: bool,
        /// Override the daemon Unix socket path used by the proxy
        /// fallback. Defaults to `serve::default_socket_path()`.
        socket: Option<String>,
        /// `--claude-code-computer-use-compat`: register the compat
        /// `screenshot` tool (window-scoped, JPEG @ 85%, pid + window_id
        /// both required) instead of the full-featured one. Used when
        /// the MCP server is wired up as `cua-computer-use` in Claude
        /// Code, where this is the documented best-practice install.
        claude_code_compat: bool,
    },
    ListTools,
    Describe(String),
    Call {
        tool: String,
        json_args: Option<serde_json::Value>,
        screenshot_out_file: Option<String>,
        /// Override the daemon socket/pipe path used by the in-process
        /// forwarding fallback (matches `--socket` semantics for `serve` /
        /// `status` / `stop`). Defaults to `serve::default_socket_path()`
        /// when None — i.e. `cua-driver call X` looks for the user's
        /// default-path daemon. Surfaced to make integration tests able
        /// to spin up a tempfile-socketed daemon and route calls
        /// through it.
        socket: Option<String>,
    },
    McpConfig {
        client: Option<String>,
    },
    Serve {
        socket: Option<String>,
        /// True when `--no-permissions-gate` is on argv.  The env-var
        /// `CUA_DRIVER_RS_PERMISSIONS_GATE=0` short-circuits the gate too
        /// (checked inside the gate itself), so the flag is only one of
        /// two opt-out signals.
        no_permissions_gate: bool,
        /// True when `--claude-code-computer-use-compat` is on argv. The MCP
        /// proxy forwards this flag to the daemon it auto-launches (see
        /// `launch_daemon_and_wait`) so the proxy path registers the compat
        /// `screenshot` surface, not just the in-process path. Without it the
        /// flag was a no-op for `cua-driver mcp --claude-code-computer-use-compat`,
        /// which always routes through the proxy on an installed bundle.
        claude_code_compat: bool,
    },
    Stop {
        socket: Option<String>,
    },
    Status {
        socket: Option<String>,
    },
    Recording {
        subcommand: String,
        args: Vec<String>,
        socket: Option<String>,
    },
    DumpDocs {
        pretty: bool,
        doc_type: String,
    },
    Update {
        apply: bool,
        json: bool,
    },
    /// `cua-driver check-update [--json] [--no-cache]` — pure check verb.
    /// Never installs; the apply path stays on `update --apply` so the
    /// "did anything change on disk?" question is unambiguous from argv.
    /// Mirror of the `check_for_update` MCP tool — both routes share
    /// `crate::version_check::check_update_state`.
    CheckUpdate {
        json: bool,
        no_cache: bool,
    },
    Doctor {
        json: bool,
    },
    Diagnose,
    /// `cua-driver permissions status|grant [--json]` — report TCC status
    /// (with source attribution + a live capture probe) or raise the
    /// correctly-attributed grant by launching CuaDriver via LaunchServices.
    Permissions {
        subcommand: String,
        json: bool,
    },
    Config {
        /// `show` | `get` | `set` | `reset` (None → show)
        subcommand: Option<String>,
        /// key arg for `get`/`set`
        key: Option<String>,
        /// value arg for `set`
        value: Option<String>,
        socket: Option<String>,
    },
    /// Content-free telemetry preference, inspection, and installer hooks.
    Telemetry(TelemetryCommand),
    /// `cua-driver autostart {enable|disable|status|kick}` —
    /// platform-native auto-start so `cua-driver serve` comes up on
    /// every logon. Windows: Scheduled Task with LogonType=Interactive
    /// (lands in Session 1+). macOS / Linux: not yet implemented; the
    /// stub returns a helpful "use install-local.sh --autostart"
    /// message. See `crates/cua-driver/src/autostart.rs`.
    Autostart {
        subcommand: String,
    },
    /// `cua-driver manifest` — emit a stable JSON description of the CLI
    /// surface (subcommands, args, MCP invocation, version).
    ///
    /// Designed for downstream consumers (Hermes, Claude Code, future
    /// SDKs) so they can drop hardcoded launch argv such as
    /// `_CUA_DRIVER_ARGS = ["mcp"]` and read the canonical invocation
    /// from the binary itself. `schema_version` keys the manifest shape
    /// so consumers can branch on additive changes.
    ///
    /// Mirrors the existing `dump-docs` shape (read-only inspection
    /// subcommand) and is purely additive: never removes a field,
    /// never renames an existing one.
    Manifest {
        pretty: bool,
    },
    /// `cua-driver skills {install|update|uninstall|status|path}` —
    /// agent skill-pack management. The verb is the ONLY way a user
    /// installs or updates the cua-driver skill pack into their agent
    /// dirs (Claude Code / Codex / OpenClaw / OpenCode); the install
    /// scripts never touch ~/.claude/skills/ etc. directly. `install`
    /// fetches the matching versioned release asset
    /// (`cua-driver-rs-v<v>-skills.tar.gz` — the asset filename keeps
    /// the legacy `-rs` for backward-compat with pinned URLs) from
    /// GitHub, places it under `<HomeDir>/skills/cua-driver/`, and
    /// symlinks into each detected agent's `skills/` dir. See
    /// `crates/cua-driver/src/skills.rs`.
    Skills {
        subcommand: String,
        flags: Vec<String>,
    },
    /// Mint a short-lived, single-use approval token for a direct/raw
    /// `browser_prepare` call. This command requires an interactive terminal.
    BrowserApprove {
        pid: i64,
        strategy: Option<String>,
        window_id: Option<u64>,
        session: Option<String>,
        profile_mode: Option<String>,
        profile_name: Option<String>,
    },
}

pub enum TelemetryCommand {
    InstallEvent,
    Enable,
    Disable,
    Status { json: bool },
    ResetId,
    Inspect { event: String },
}

/// Flags whose next token is a value (not a subcommand).
/// We skip both the flag and its value when scanning for the subcommand.
const VALUE_FLAGS: &[&str] = &[
    "--cursor-icon",
    "--cursor-id",
    "--cursor-palette",
    "--cursor-shape",
    "--glide-ms",
    "--dwell-ms",
    "--idle-hide-ms",
    "--screenshot-out-file",
    "--client",
    "--socket",
    "--pid-file",
    "--type",
    "--host-bundle-id",
    "--pid",
    "--strategy",
    "--window-id",
    "--session",
    "--profile-mode",
    "--profile-name",
    // Experimental PiP preview — value flag for the optional geometry
    // override (--experimental-pip itself is a bare flag and doesn't
    // need to be listed here).
    "--experimental-pip-geometry",
];

/// Classify the requested finite command without parsing its arguments. The
/// parent process uses this before `parse_command` so invalid JSON and other
/// parser exits are still observed as completed failures.
pub fn finite_command_name_from_argv() -> Option<&'static str> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    finite_command_name_from_args(&args)
}

fn positional_args(args: &[String]) -> Vec<&str> {
    let mut positionals = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        if VALUE_FLAGS.contains(&arg) {
            index += 2;
        } else if arg.starts_with('-') {
            index += 1;
        } else {
            positionals.push(arg);
            index += 1;
        }
    }
    positionals
}

fn finite_command_name_from_args(args: &[String]) -> Option<&'static str> {
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "--help" | "-h" | "--version" | "-V"))
    {
        return None;
    }
    let positionals = positional_args(args);
    match positionals.first().copied() {
        None | Some("mcp" | "serve" | "telemetry") => None,
        Some("list-tools") => Some("list_tools"),
        Some("describe") => Some("describe"),
        Some("mcp-config") => Some("mcp_config"),
        Some("manifest") => Some("manifest"),
        Some("call") => Some("call"),
        Some("stop") => Some("stop"),
        Some("status") => Some("status"),
        Some("recording") => Some("recording"),
        Some("dump-docs") => Some("dump_docs"),
        Some("update") => Some("update"),
        Some("check-update") => Some("check_update"),
        Some("doctor") => Some("doctor"),
        Some("diagnose") => Some("diagnose"),
        Some("permissions") => Some("permissions"),
        Some("autostart") => Some("autostart"),
        Some("skills") => Some("skills"),
        Some("browser-approve") => Some("browser_approve"),
        Some("config") => Some("config"),
        Some(_) => Some("call"),
    }
}

/// Return the candidate tool for a finite `call` command. The telemetry layer
/// maps this through its fixed registry allowlist before anything is emitted.
pub fn finite_tool_name_from_argv() -> Option<String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    finite_tool_name_from_args(&args)
}

/// Return whether a finite `call` targets a fixed computer-action category.
/// JSON is inspected only long enough to classify the closed `page.action`
/// vocabulary and is never retained or passed to telemetry.
pub fn finite_computer_action_from_argv() -> bool {
    let args: Vec<String> = std::env::args().skip(1).collect();
    finite_computer_action_from_args(&args)
}

fn finite_computer_action_from_args(args: &[String]) -> bool {
    let Some(tool_name) = finite_tool_name_from_args(args) else {
        return false;
    };
    let positionals = positional_args(args);
    let json_arg = match positionals.as_slice() {
        ["call", _, json, ..] | [_, json, ..] => Some(*json),
        _ => None,
    };
    let parsed_args = json_arg.and_then(|json| serde_json::from_str(json).ok());
    let operation = cua_driver_core::server::tool_operation(&tool_name, parsed_args.as_ref());
    cua_driver_core::server::is_computer_action(&tool_name, operation)
}

/// Return the bounded sub-operation for a finite command. This classifier
/// reads only the command verb, a reviewed subcommand, and the presence of
/// `--apply`; arbitrary values never leave this function.
pub fn finite_operation_from_argv() -> &'static str {
    let args: Vec<String> = std::env::args().skip(1).collect();
    finite_operation_from_args(&args)
}

fn finite_operation_from_args(args: &[String]) -> &'static str {
    let command = finite_command_name_from_args(args);
    let positionals = positional_args(args);
    let subcommand = positionals.get(1).copied();
    match command {
        Some("recording") => match subcommand.unwrap_or("status") {
            "start" => "start",
            "stop" => "stop",
            "status" => "status",
            "render" => "render",
            _ => "other",
        },
        Some("permissions") => match subcommand.unwrap_or("status") {
            "status" => "status",
            "grant" => "grant",
            _ => "other",
        },
        Some("config") => match subcommand.unwrap_or("show") {
            "show" => "show",
            "get" => "get",
            "set" => "set",
            "reset" => "reset",
            _ => "other",
        },
        Some("autostart") => match subcommand.unwrap_or("") {
            "enable" => "enable",
            "disable" => "disable",
            "status" => "status",
            "kick" => "kick",
            _ => "other",
        },
        Some("skills") => match subcommand.unwrap_or("status") {
            "install" => "install",
            "update" => "update",
            "uninstall" => "uninstall",
            "status" => "status",
            "path" => "path",
            _ => "other",
        },
        Some("update") if args.iter().any(|arg| arg == "--apply") => "apply",
        Some("update") => "check_only",
        _ => "not_applicable",
    }
}

/// Return the configured MCP client as a closed category. Raw `--client`
/// values are mapped to `other` before the detached worker is spawned.
pub fn finite_client_kind_from_argv() -> &'static str {
    let args: Vec<String> = std::env::args().skip(1).collect();
    finite_client_kind_from_args(&args)
}

fn finite_client_kind_from_args(args: &[String]) -> &'static str {
    if finite_command_name_from_args(args) != Some("mcp_config") {
        return "not_applicable";
    }
    let value = args
        .iter()
        .position(|arg| arg == "--client")
        .and_then(|index| args.get(index + 1))
        .map(String::as_str)
        .unwrap_or("");
    match value {
        "" => "generic",
        "claude" | "claude-code" => "claude_code",
        "codex" => "codex",
        "cursor" => "cursor",
        "openclaw" => "openclaw",
        "opencode" => "opencode",
        "hermes" => "hermes",
        "pi" => "pi",
        "antigravity" | "gemini" => "antigravity",
        "qwen" | "qwen-code" => "qwen_code",
        "droid" | "factory" => "factory_droid",
        "zcode" => "zcode",
        _ => "other",
    }
}

fn finite_tool_name_from_args(args: &[String]) -> Option<String> {
    if finite_command_name_from_args(args) != Some("call") {
        return None;
    }
    let positionals = positional_args(args);
    match positionals.as_slice() {
        ["call", tool, ..] => Some((*tool).to_owned()),
        [tool, ..] => Some((*tool).to_owned()),
        _ => None,
    }
}

/// Parse the first non-flag positional argument from argv to determine which
/// subcommand to run.  Cursor-overlay flags are consumed by `CursorConfig`
/// independently; we only care about the first non-`--` arg here.
pub fn parse_command() -> Command {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // Handle --version / -V before any other parsing so they are never
    // silently stripped as "bare flags" and swallowed by MCP mode.
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("cua-driver {}", env!("CARGO_PKG_VERSION"));
        std::process::exit(0);
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "cua-driver {} — cross-platform computer-use automation driver",
            env!("CARGO_PKG_VERSION")
        );
        println!("Usage: cua-driver [SUBCOMMAND] [OPTIONS]");
        println!("Subcommands: mcp, list-tools, describe, call, serve, stop, status, config, telemetry, recording, update, check-update, doctor, diagnose, permissions, autostart, skills, browser-approve, manifest");
        println!();
        println!("permissions options (macOS):");
        println!("  cua-driver permissions status   Report Accessibility + Screen Recording status. Read-only (no prompt).");
        println!("                                  Answers via a running daemon, so the result carries the CuaDriver");
        println!("                                  identity (com.trycua.driver). If no daemon is running it reports");
        println!("                                  `unknown` rather than your terminal's grants. Add --json for the payload.");
        println!("  cua-driver permissions grant    Launch CuaDriver via LaunchServices so the permission dialog attributes");
        println!("                                  to com.trycua.driver (not your terminal), wait for the grant, then");
        println!("                                  confirm the driver's own status. This is the correct way to grant.");
        println!();
        println!("Updating cua-driver:");
        println!("  cua-driver check-update         Ask GitHub whether a newer release is available. Read-only.");
        println!("                                  Default output is human-friendly text.");
        println!("    --json                        Emit a machine-readable JSON payload (same shape as the");
        println!("                                  check_for_update MCP tool). Hermes branches on update_available.");
        println!("    --no-cache                    Skip the 20h on-disk cache and force a fresh GitHub round-trip.");
        println!("  cua-driver update               Same check as above, then suggest --apply if outdated.");
        println!("    --apply                       Download + install the latest release via the canonical installer.");
        println!("    --json                        Emit the structured check payload (does not change --apply behaviour).");
        println!();
        println!("autostart options (Windows-only today):");
        println!("  cua-driver autostart enable     Register a logon Scheduled Task so serve starts at every interactive logon.");
        println!("  cua-driver autostart disable    Remove the autostart entry. No-op if not registered.");
        println!("  cua-driver autostart status     Print whether the entry is registered + whether the daemon is running.");
        println!("  cua-driver autostart kick       Start the entry now without re-logging.");
        println!();
        println!("skills options (agent skill-pack management, opt-in):");
        println!("  cua-driver skills install       Fetch the versioned skill pack from GitHub Releases and symlink it");
        println!("                                  into each detected agent's skills/ dir (Claude Code, Codex, OpenClaw,");
        println!("                                  OpenCode). Idempotent. Never overwrites existing user links.");
        println!("  cua-driver skills update        Re-fetch the skill pack from GitHub, refreshing the local copy + links.");
        println!("  cua-driver skills uninstall     Remove the agent symlinks. Add --all to also delete the local copy.");
        println!("  cua-driver skills status        Report local install state + per-agent link state. Read-only.");
        println!("  cua-driver skills path          Print where the local skill pack lives.");
        println!("  --from main                     (install only) Fetch latest from main branch instead of the tagged release.");
        println!();
        println!("browser preparation approval:");
        println!("  cua-driver browser-approve --pid <pid> --profile-mode isolated_new");
        println!("  cua-driver browser-approve --pid <pid> --profile-mode isolated_named --profile-name <name>");
        println!("                                  Interactively mint a five-minute, single-use token for a");
        println!("                                  direct CLI/raw browser_prepare call. MCP hosts use their");
        println!("                                  destructive-tool approval flow instead.");
        println!("  cua-driver browser-approve --strategy existing_profile --pid <pid>");
        println!("                                  --window-id <window_id> --session <session>");
        println!("                                  Approve attachment to one exact existing browser request.");
        println!();
        println!("mcp options (macOS):");
        println!(
            "  --no-daemon-relaunch    Stay in-process; skip auto-launching the CuaDriver daemon."
        );
        println!("                          Also: CUA_DRIVER_RS_MCP_NO_RELAUNCH=1");
        println!("  --embedded              Run embedded inside a host app (also: CUA_DRIVER_EMBEDDED=1).");
        println!("                          Inherits the host's TCC grants; never prompts or relaunches.");
        println!("                          See Skills/cua-driver/EMBEDDING.md.");
        println!(
            "  --host-bundle-id <id>   Advisory host bundle id label for check_permissions output."
        );
        println!(
            "  --socket <path>         Override the daemon UDS path used by the proxy fallback."
        );
        println!("  --claude-code-computer-use-compat");
        println!("                          Select the Claude Code computer-use compat surface.");
        println!(
            "                          Now forwarded to the proxy-launched daemon (was a no-op"
        );
        println!(
            "                          on the proxy path — the path you actually run — because"
        );
        println!("                          the daemon hardcoded compat=false). Note: the compat");
        println!(
            "                          screenshot tool itself was removed in #1692, so the flag"
        );
        println!(
            "                          has no tool-surface effect today; the wiring is in place"
        );
        println!("                          for any future compat-gated tool.");
        println!();
        println!("agent cursor overlay (serve / mcp only — needs the daemon UI runloop):");
        println!("  The overlay is ON by default: every MCP session automatically gets its own");
        println!(
            "  cursor (keyed by session id) that shows where the agent acts without moving the"
        );
        println!("  real pointer. It is removed when the session ends. A pure accessibility (AX)");
        println!(
            "  action snaps the cursor with a brief pulse on its first action instead of a long"
        );
        println!("  glide, so it can be easy to miss — do a pixel click or move_cursor first");
        println!("  for a visibly gliding demo. These flags tune the overlay on `serve`/`mcp`:");
        println!("  --no-overlay            Disable the cursor overlay entirely for this daemon.");
        println!(
            "  --cursor-id <id>        Name the default cursor instance (default: 'default')."
        );
        println!("  --cursor-icon <path>    Use a custom PNG / JPEG / SVG / ICO cursor asset.");
        println!(
            "  --cursor-shape <name>   Built-in silhouette: {} ('teardrop' is the default —",
            cursor_overlay::BuiltinShape::names_help()
        );
        println!(
            "                          embedded cursor-up SVG; 'arrow' is the procedural gradient"
        );
        println!("                          diamond). Same vocabulary as MCP `cursor_icon`.");
        println!("  --cursor-palette <name> Pick a built-in colour palette for the cursor.");
        println!("  (These are no-ops for one-shot CLI calls like `cua-driver call` — the overlay");
        println!("   needs the long-lived AppKit runloop that only `serve` / `mcp` keep alive.)");
        println!();
        println!("manifest options:");
        println!("  cua-driver manifest             Emit a stable JSON description of this CLI's surface");
        println!("                                  (subcommands, args, MCP invocation, version). Read-only.");
        println!(
            "                                  Consumers (Hermes, Claude Code, …) read it to drop"
        );
        println!("                                  hardcoded launch argv like _CUA_DRIVER_ARGS = [\"mcp\"].");
        println!("    --pretty / -p                 Pretty-print the JSON.");
        println!();
        println!("doctor options:");
        println!("  --json                  Emit the probe report as JSON for scripting.");
        println!();
        println!("experimental options (default: off):");
        println!("  --experimental-pip          Show a small always-on-top window with the latest");
        println!(
            "                              post-action screenshot + a 1-line label. macOS only"
        );
        println!(
            "                              today; Win/Linux print a not-yet-implemented notice."
        );
        println!(
            "  --experimental-pip-geometry WxH[+X+Y]   Override window size (and optional top-left"
        );
        println!("                                          origin). Defaults to 480x360 in the top-right");
        println!("                                          corner of the main display.");
        std::process::exit(0);
    }

    // Collect named flag values we care about.
    let screenshot_out_file = flag_value(&args, "--screenshot-out-file");
    let mcp_client = flag_value(&args, "--client");
    let socket = flag_value(&args, "--socket");
    let approval_pid = flag_value(&args, "--pid");
    let approval_strategy = flag_value(&args, "--strategy");
    let approval_window_id = flag_value(&args, "--window-id");
    let approval_session = flag_value(&args, "--session");
    let approval_profile_mode = flag_value(&args, "--profile-mode");
    let approval_profile_name = flag_value(&args, "--profile-name");

    // `--embedded` / `--host-bundle-id` export to the environment rather
    // than threading through `Command`: all consumers read
    // `cua_driver_core::embedded_mode()` and children inherit the mode.
    if args.iter().any(|a| a == "--embedded") {
        std::env::set_var(cua_driver_core::EMBEDDED_ENV, "1");
    }
    if let Some(id) = flag_value(&args, "--host-bundle-id") {
        std::env::set_var(cua_driver_core::HOST_BUNDLE_ID_ENV, id);
    }

    // Strip cursor-overlay flags (and their values) to expose the subcommand.
    let mut positionals: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if VALUE_FLAGS.contains(&a) {
            i += 2; // skip flag + value
        } else if a.starts_with('-') {
            i += 1; // skip bare flag
        } else {
            positionals.push(a);
            i += 1;
        }
    }

    let no_daemon_relaunch = args.iter().any(|a| a == "--no-daemon-relaunch");
    let claude_code_compat = args
        .iter()
        .any(|a| a == "--claude-code-computer-use-compat");

    let mut pos = positionals.into_iter();
    match pos.next() {
        None => {
            // Bare `cua-driver` defaults to MCP, which reads JSON-RPC from
            // stdin forever. From a terminal that looks like a hang. If
            // stdin is a TTY (i.e. interactive shell, no client piping
            // stdio), surface a hint and exit. Piped / redirected stdin —
            // the normal MCP client case — falls through to MCP mode.
            // Explicit `cua-driver mcp` bypasses the check entirely.
            use std::io::IsTerminal as _;
            if std::io::stdin().is_terminal() {
                eprintln!("cua-driver: bare invocation defaults to the MCP server, which reads");
                eprintln!("JSON-RPC from stdin. From a terminal that looks like a hang.");
                eprintln!();
                eprintln!("You probably meant one of:");
                eprintln!("  cua-driver list-tools                           # available tools");
                eprintln!("  cua-driver status                               # check the daemon");
                eprintln!("  cua-driver mcp-config --client claude-code      # wire into a client");
                eprintln!("  cua-driver --help                               # everything else");
                eprintln!();
                eprintln!("To run the MCP server explicitly (and pipe JSON-RPC by hand):");
                eprintln!("  cua-driver mcp");
                std::process::exit(0);
            }
            Command::Mcp {
                no_daemon_relaunch,
                socket: socket.clone(),
                claude_code_compat,
            }
        }
        Some("mcp") => Command::Mcp {
            no_daemon_relaunch,
            socket: socket.clone(),
            claude_code_compat,
        },
        Some("list-tools") => Command::ListTools,
        Some("mcp-config") => Command::McpConfig { client: mcp_client },
        Some("serve") => Command::Serve {
            socket,
            // Bare flag — present anywhere on argv counts as "skip the gate".
            no_permissions_gate: args.iter().any(|a| a == "--no-permissions-gate"),
            claude_code_compat,
        },
        Some("stop") => Command::Stop { socket },
        Some("status") => Command::Status { socket },
        Some("recording") => {
            let subcommand = pos.next().unwrap_or("status").to_string();
            let rest: Vec<String> = pos.map(str::to_owned).collect();
            Command::Recording {
                subcommand,
                args: rest,
                socket,
            }
        }
        Some("dump-docs") => {
            let pretty = args.iter().any(|a| a == "--pretty" || a == "-p");
            let doc_type = flag_value(&args, "--type").unwrap_or_else(|| "all".to_owned());
            Command::DumpDocs { pretty, doc_type }
        }
        Some("manifest") => {
            // Default to compact output to match other JSON-emitting commands
            // (`check-update --json`, `doctor --json`); `--pretty` is opt-in for
            // shell-debug use.
            let pretty = args.iter().any(|a| a == "--pretty" || a == "-p");
            Command::Manifest { pretty }
        }
        Some("update") => {
            let apply = args.iter().any(|a| a == "--apply");
            let json = args.iter().any(|a| a == "--json");
            Command::Update { apply, json }
        }
        Some("check-update") => {
            let json = args.iter().any(|a| a == "--json");
            let no_cache = args.iter().any(|a| a == "--no-cache");
            Command::CheckUpdate { json, no_cache }
        }
        Some("doctor") => {
            // `--json` switches to machine-readable output for scripting.
            // Bare flag — no value, position-independent.
            let json = args.iter().any(|a| a == "--json");
            Command::Doctor { json }
        }
        Some("diagnose") => Command::Diagnose,
        Some("permissions") => {
            let subcommand = pos.next().unwrap_or("status").to_string();
            let json = args.iter().any(|a| a == "--json");
            Command::Permissions { subcommand, json }
        }
        Some("config") => {
            let subcommand = pos.next().map(str::to_owned);
            let key = pos.next().map(str::to_owned);
            let value = pos.next().map(str::to_owned);
            Command::Config {
                subcommand,
                key,
                value,
                socket,
            }
        }
        Some("describe") => {
            let name = pos.next().unwrap_or("").to_string();
            Command::Describe(name)
        }
        Some("call") => {
            let tool = pos.next().unwrap_or("").to_string();
            // Differentiate "no positional arg" (fall back to stdin) from
            // "positional arg given but didn't parse as JSON" (surface the
            // error instead of silently falling back to stdin and letting
            // the tool's required-field validator emit a misleading
            // "missing field X" later). See #1637.
            //
            // The common cause of an unparseable positional arg is
            // PowerShell 5.1's native-command-arg quote-stripping on
            // multi-field JSON — `'{"a":1,"b":2}'` arrives as `{a:1,b:2}`
            // which serde_json rejects. The error message points users at
            // the stdin-pipe workaround.
            let json_args = match pos.next() {
                Some(s) => match serde_json::from_str(s) {
                    Ok(v) => Some(v),
                    Err(e) => {
                        eprintln!(
                            "error: positional JSON arg to 'cua-driver call' did not parse: {e}"
                        );
                        eprintln!("       received: {s}");
                        eprintln!();
                        eprintln!("hint: PowerShell 5.1 strips quotes around JSON field names in");
                        eprintln!("      multi-field args. Pipe the JSON via stdin instead:");
                        eprintln!(
                            "        '{{\"pid\":1234,\"window_id\":5678}}' | cua-driver call {}",
                            tool
                        );
                        eprintln!();
                        eprintln!("      Or use PowerShell 7+ (pwsh) which preserves the quotes.");
                        process::exit(2);
                    }
                },
                None => read_stdin_json(),
            };
            Command::Call {
                tool,
                json_args,
                screenshot_out_file,
                socket: socket.clone(),
            }
        }
        Some("telemetry") => match pos.next() {
            Some("install-event") => Command::Telemetry(TelemetryCommand::InstallEvent),
            Some("enable") => Command::Telemetry(TelemetryCommand::Enable),
            Some("disable") => Command::Telemetry(TelemetryCommand::Disable),
            Some("status") => Command::Telemetry(TelemetryCommand::Status {
                json: args.iter().any(|arg| arg == "--json"),
            }),
            Some("reset-id") => Command::Telemetry(TelemetryCommand::ResetId),
            Some("inspect") => {
                let event = pos.next().unwrap_or("").to_owned();
                if event.is_empty() {
                    eprintln!("Usage: cua-driver telemetry inspect <event> --json");
                    process::exit(64);
                }
                Command::Telemetry(TelemetryCommand::Inspect { event })
            }
            _ => {
                eprintln!("Usage: cua-driver telemetry {{enable|disable|status [--json]|reset-id|inspect <event> --json}}");
                process::exit(64);
            }
        },
        Some("autostart") => {
            // No `cua-driver autostart` (no subcommand) shortcut today —
            // every operation is destructive enough that we want the
            // user to be explicit about which one.
            let subcommand = pos.next().unwrap_or("").to_string();
            if subcommand.is_empty() {
                eprintln!("Usage: cua-driver autostart {{enable|disable|status|kick}}");
                process::exit(64);
            }
            Command::Autostart { subcommand }
        }
        Some("skills") => {
            // Skills subcommand. Default is `status` so plain `cua-driver
            // skills` is a read-only probe — won't ever modify user state.
            let subcommand = pos.next().unwrap_or("status").to_string();
            // Pass through any other flags / args after the subcommand for
            // the verb's own parsing (e.g. `--force`, `--from main`,
            // `--agent claude-code`, `--local`, `--all`). Collect from `pos`
            // and dotted long-form flags from the raw args too.
            let mut flags: Vec<String> = pos.map(str::to_owned).collect();
            for a in &args {
                if a.starts_with("--") && !flags.contains(a) {
                    flags.push(a.clone());
                }
            }
            Command::Skills { subcommand, flags }
        }
        Some("browser-approve") => {
            let pid = approval_pid
                .as_deref()
                .and_then(|value| value.parse::<i64>().ok())
                .filter(|pid| *pid > 0)
                .unwrap_or_else(|| {
                    eprintln!("browser-approve requires --pid <positive integer>");
                    process::exit(64);
                });
            Command::BrowserApprove {
                pid,
                strategy: approval_strategy,
                window_id: approval_window_id.and_then(|value| value.parse::<u64>().ok()),
                session: approval_session,
                profile_mode: approval_profile_mode,
                profile_name: approval_profile_name,
            }
        }
        Some(first) => {
            // Implicit call: unrecognised first positional → treat as tool name.
            // Same parse-error handling as the explicit `call` branch above. See #1637.
            let tool = first.to_string();
            let json_args = match pos.next() {
                Some(s) => match serde_json::from_str(s) {
                    Ok(v) => Some(v),
                    Err(e) => {
                        eprintln!(
                            "error: positional JSON arg to 'cua-driver {tool}' did not parse: {e}"
                        );
                        eprintln!("       received: {s}");
                        eprintln!();
                        eprintln!("hint: PowerShell 5.1 strips quotes around JSON field names in");
                        eprintln!("      multi-field args. Pipe the JSON via stdin instead:");
                        eprintln!(
                            "        '{{\"pid\":1234,\"window_id\":5678}}' | cua-driver {}",
                            tool
                        );
                        eprintln!();
                        eprintln!("      Or use PowerShell 7+ (pwsh) which preserves the quotes.");
                        process::exit(2);
                    }
                },
                None => read_stdin_json(),
            };
            Command::Call {
                tool,
                json_args,
                screenshot_out_file,
                socket: socket.clone(),
            }
        }
    }
}

pub fn run_browser_approve(
    pid: i64,
    strategy: Option<&str>,
    window_id: Option<u64>,
    session: Option<&str>,
    profile_mode: Option<&str>,
    profile_name: Option<&str>,
) {
    use std::io::{IsTerminal as _, Write as _};

    if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        eprintln!("browser-approve requires an interactive terminal; approval cannot be piped or scripted");
        process::exit(1);
    }
    if let Some(strategy) = strategy {
        if strategy != "existing_profile" {
            eprintln!("unsupported browser approval strategy {strategy:?}; use existing_profile");
            process::exit(64);
        }
        if profile_mode.is_some() || profile_name.is_some() {
            eprintln!("--strategy existing_profile cannot be combined with profile flags");
            process::exit(64);
        }
        let Some(window_id) = window_id.filter(|window_id| *window_id > 0) else {
            eprintln!("existing-profile approval requires --window-id <positive integer>");
            process::exit(64);
        };
        let Some(session) = session.filter(|session| !session.trim().is_empty()) else {
            eprintln!("existing-profile approval requires --session <explicit session>");
            process::exit(64);
        };
        eprintln!("Approve CUA Driver to attach to this existing Chromium profile?");
        eprintln!("  browser pid: {pid}");
        eprintln!("  native window id: {window_id}");
        eprintln!("  caller session: {session}");
        eprintln!(
            "This grant stays in daemon memory, expires, and never authorizes arbitrary dialogs."
        );
        eprint!("Type APPROVE to continue: ");
        let _ = std::io::stderr().flush();
        let mut confirmation = String::new();
        if std::io::stdin().read_line(&mut confirmation).is_err()
            || confirmation.trim() != "APPROVE"
        {
            eprintln!("browser attachment approval declined; no artifact was created");
            process::exit(1);
        }
        let scope = cua_driver_core::browser::approval::ExistingProfileApprovalScope {
            pid,
            window_id,
            session: session.to_owned(),
        };
        match cua_driver_core::browser::approval::mint_existing_profile_approval(scope) {
            Ok(token) => println!("{token}"),
            Err(error) => {
                eprintln!("{}", error.message);
                process::exit(1);
            }
        }
        return;
    }
    if window_id.is_some() || session.is_some() {
        eprintln!("--window-id/--session require --strategy existing_profile");
        process::exit(64);
    }
    let profile_mode = profile_mode.unwrap_or_else(|| {
        eprintln!("browser-approve requires --profile-mode isolated_new|isolated_named");
        process::exit(64);
    });
    let mode = match profile_mode {
        "isolated_new" => cua_driver_core::browser::PrepareProfileMode::IsolatedNew,
        "isolated_named" => cua_driver_core::browser::PrepareProfileMode::IsolatedNamed,
        other => {
            eprintln!("unsupported profile mode {other:?}; use isolated_new or isolated_named");
            process::exit(64);
        }
    };
    let profile = cua_driver_core::browser::PrepareProfile {
        mode,
        name: profile_name.map(str::to_owned),
    };
    if let Err(error) = cua_driver_core::browser::approval::validate_profile(&profile) {
        eprintln!("{}", error.message);
        process::exit(64);
    }
    eprintln!("Approve CUA Driver to launch a separate driver-owned Chromium profile?");
    eprintln!("  source pid: {pid}");
    eprintln!("  profile mode: {profile_mode}");
    if let Some(name) = profile_name {
        eprintln!("  profile name: {name}");
    }
    eprintln!("The existing browser process and its profile will not be modified or terminated.");
    eprint!("Type APPROVE to continue: ");
    let _ = std::io::stderr().flush();
    let mut confirmation = String::new();
    if std::io::stdin().read_line(&mut confirmation).is_err() || confirmation.trim() != "APPROVE" {
        eprintln!("browser preparation approval declined; no artifact was created");
        process::exit(1);
    }
    match cua_driver_core::browser::approval::mint_prepare_approval(pid, profile) {
        Ok(token) => println!("{token}"),
        Err(error) => {
            eprintln!("{}", error.message);
            process::exit(1);
        }
    }
}

/// Return the value of `--flag value` from argv, or `None`.
fn flag_value(args: &[String], flag: &str) -> Option<String> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == flag {
            return it.next().cloned();
        }
        // Also handle --flag=value form.
        if let Some(rest) = a.strip_prefix(flag) {
            if let Some(val) = rest.strip_prefix('=') {
                return Some(val.to_owned());
            }
        }
    }
    None
}

/// Print all tools in the registry, one per line: `name: first sentence`.
pub fn run_list_tools(registry: &ToolRegistry) {
    // Sort alphabetically by name to match Swift's
    // `ListToolsCommand.run()` `tools.sorted(by: { $0.name < $1.name })`.
    let mut entries: Vec<(&str, &cua_driver_core::tool::ToolDef)> = registry.iter_defs().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    for (name, def) in entries {
        let summary = first_sentence(&def.description);
        if summary.is_empty() {
            println!("{name}");
        } else {
            println!("{name}: {summary}");
        }
    }
}

/// Print a tool's full description and JSON input schema.
/// Exits 64 (EX_USAGE) if the tool is unknown.
pub fn run_describe(registry: &ToolRegistry, name: &str) {
    match registry.get_def(name) {
        None => {
            eprintln!("Unknown tool: {name}");
            eprintln!("Available tools:");
            // Sort alphabetically to match Swift's `printUnknownTool`
            // (`registry.allTools.map(\.name).sorted()`).
            let mut names: Vec<&str> = registry.tool_names().collect();
            names.sort();
            for n in names {
                eprintln!("  {n}");
            }
            process::exit(64);
        }
        Some(def) => {
            print!("name: {}\n", def.name);
            if !def.description.is_empty() {
                print!("\ndescription:\n{}", def.description);
                if !def.description.ends_with('\n') {
                    println!();
                }
            }
            print!("\ninput_schema:\n");
            let pretty = serde_json::to_string_pretty(&def.input_schema)
                .unwrap_or_else(|_| def.input_schema.to_string());
            println!("{pretty}");
        }
    }
}

/// Decide whether `mcp` should auto-launch a daemon and proxy MCP
/// requests through its Unix socket instead of running in-process.
///
/// Mirrors Swift `MCPCommand.shouldUseDaemonProxy` in spirit:
/// the trigger is "shell-spawned bare binary that resolves into an
/// installed `CuaDriver.app` bundle, with a non-launchd parent".
/// When any of those conditions fails — explicit opt-out, dev-mode
/// `cargo run` invocation, already-relaunched-via-launchd — we stay
/// in-process. The proxy path is purely additive.
///
/// `false` on non-macOS targets: TCC is a macOS-only concern and
/// there's no `open -a` equivalent on Linux / Windows.
#[cfg(target_os = "macos")]
pub fn should_use_daemon_proxy(no_daemon_relaunch: bool) -> bool {
    use crate::bundle::{is_env_truthy, is_executable_inside_cuadriver_app, parent_is_not_launchd};
    // Embedded mode stays in-process: relaunching via `open -a CuaDriver`
    // would leave the host's TCC responsibility chain and could prompt
    // for com.trycua.driver.
    if cua_driver_core::embedded_mode() {
        return false;
    }
    if no_daemon_relaunch {
        return false;
    }
    if is_env_truthy("CUA_DRIVER_RS_MCP_NO_RELAUNCH") {
        return false;
    }
    // Hidden test/escape hook: force proxy mode without requiring the
    // executable to live inside CuaDriver.app. Used by the
    // integration test (which spawns a daemon manually) and by users
    // who've wrapped the binary in a custom bundle. Skips the
    // launch_daemon_and_wait `open -a` step too — caller is expected
    // to have a daemon already running on the chosen socket.
    if is_env_truthy("CUA_DRIVER_RS_MCP_FORCE_PROXY") {
        return true;
    }
    if !is_executable_inside_cuadriver_app() {
        // Raw `cargo run` / dev binary — no installed bundle to land
        // in, so relaunching would fail. Stay in-process.
        return false;
    }
    if !parent_is_not_launchd() {
        // ppid == 1 — already running as the LaunchServices-spawned
        // daemon. TCC context is already correct.
        return false;
    }
    true
}

/// Non-macOS targets don't have TCC, but they DO have the equivalent
/// problem of session attribution on Windows (Session 0 vs the user's
/// interactive Session 1+). When the CLI is spawned via SSH or a
/// Windows service, it lands in Session 0 where the desktop, window
/// APIs, and UI Automation return empty. A daemon running in the
/// interactive session (via `cua-driver autostart enable && kick`,
/// or any other Session-1+ launch) can answer tool calls correctly —
/// so when one is up, we proxy through it.
///
/// Behaviour:
///   * `--no-daemon-relaunch` or `CUA_DRIVER_RS_MCP_NO_RELAUNCH=1`
///     forces in-process (matches macOS opt-out).
///   * `CUA_DRIVER_RS_MCP_FORCE_PROXY=1` always proxies, even with
///     no daemon up — the caller is responsible for having one
///     already.
///   * Otherwise we probe `is_daemon_listening` on the default
///     socket: a live daemon means proxy through it; nothing
///     listening means run in-process (no autospawn equivalent on
///     Linux/Windows — there's no `open -a CuaDriver` analog).
#[cfg(not(target_os = "macos"))]
pub fn should_use_daemon_proxy(no_daemon_relaunch: bool) -> bool {
    use crate::bundle::is_env_truthy;
    // Same rule as macOS: an embedded driver answers in-process.
    if cua_driver_core::embedded_mode() {
        return false;
    }
    if no_daemon_relaunch {
        return false;
    }
    if is_env_truthy("CUA_DRIVER_RS_MCP_NO_RELAUNCH") {
        return false;
    }
    if is_env_truthy("CUA_DRIVER_RS_MCP_FORCE_PROXY") {
        return true;
    }
    // Either the regular daemon (`\\.\pipe\cua-driver`) OR the uiAccess'd
    // worker (`\\.\pipe\cua-driver-uia`) is a valid proxy target on Windows:
    // both speak the same line-delimited JSON protocol. Preferring proxy mode
    // when only the uia worker is up means MCP tool calls land in a process
    // that bypasses UIPI for UWP apps. See #1602 / the cua-driver-uia crate.
    #[cfg(target_os = "windows")]
    {
        if crate::serve::is_daemon_listening(&crate::serve::default_uia_pipe_path()) {
            return true;
        }
    }
    crate::serve::is_daemon_listening(&crate::serve::default_socket_path())
}

/// Spawn `/usr/bin/open -n -g -a CuaDriver --args serve` to launch
/// the daemon under `LaunchServices` (so it inherits the bundle's
/// TCC attribution), then poll the socket for up to `timeout_secs`
/// seconds. Returns Err with a diagnostic message if `open` failed
/// or the daemon never came up.
///
/// Mirror of Swift `MCPCommand.launchDaemonViaOpen` +
/// `waitForDaemon`. Split into one Rust function because we don't
/// need the post-launch probe separation Swift has.
#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchDaemonErrorKind {
    Failed,
    Timeout,
}

#[cfg(target_os = "macos")]
#[derive(Debug)]
pub struct LaunchDaemonError {
    pub kind: LaunchDaemonErrorKind,
    message: String,
}

#[cfg(target_os = "macos")]
impl std::fmt::Display for LaunchDaemonError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

#[cfg(target_os = "macos")]
impl std::error::Error for LaunchDaemonError {}

#[cfg(target_os = "macos")]
pub fn launch_daemon_and_wait(
    socket_path: &str,
    timeout_secs: u64,
    claude_code_compat: bool,
) -> Result<(), LaunchDaemonError> {
    use std::process::{Command as Cmd, Stdio};
    use std::time::{Duration, Instant};

    // Forward `--socket <path>` to the relaunched daemon when the caller
    // passed a non-default socket via `cua-driver mcp --socket /path`.
    // Without this the daemon would listen on `default_socket_path()`,
    // and the proxy would block forever waiting for a daemon on the
    // user-supplied path that never comes up. Only added when the path
    // actually differs from the default, so the common case keeps the
    // shorter `open` argv (and matches Swift's invocation byte-for-byte).
    let pass_socket = socket_path != crate::serve::default_socket_path();
    let mut open_args: Vec<&str> = vec!["-n", "-g", "-a", "CuaDriver", "--args", "serve"];
    if pass_socket {
        open_args.push("--socket");
        open_args.push(socket_path);
    }
    // Thread the Claude-Code compat flag through to the daemon. Without this
    // the proxy-spawned daemon always called build_macos_registry() (compat
    // hardcoded false), so `cua-driver mcp --claude-code-computer-use-compat`
    // SILENTLY DROPPED the flag on the proxy path — the path users actually
    // run on an installed bundle. Today this is latent: the compat screenshot
    // tool was removed in #1692, so `register_all(compat)` ignores the flag and
    // the served surface is identical either way. But the flag was being lost
    // before reaching the daemon at all, so the moment any compat-gated tool is
    // re-introduced the proxy path would not honour it. This makes the flag
    // travel end-to-end. Only honoured on a freshly-launched daemon — a
    // pre-existing daemon keeps whatever surface it launched with.
    if claude_code_compat {
        open_args.push("--claude-code-computer-use-compat");
    }

    let status = Cmd::new("/usr/bin/open")
        // `-n` forces a new instance: CuaDriver.app might already be
        // running from a previous MCP session, and without `-n`, `open
        // -a` would re-use it and drop our `--args serve`, leaving no
        // daemon up. `-g` keeps the new instance backgrounded —
        // LSUIElement=true in Info.plist already does this but the
        // flag makes it explicit and matches Swift's invocation.
        .args(&open_args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    let status = status.map_err(|error| LaunchDaemonError {
        kind: LaunchDaemonErrorKind::Failed,
        message: format!(
            "failed to exec `/usr/bin/open`: {error}. Pass --no-daemon-relaunch to bypass."
        ),
    })?;

    if !status.success() {
        return Err(LaunchDaemonError {
            kind: LaunchDaemonErrorKind::Failed,
            message: format!(
                "`open -n -g -a CuaDriver --args serve{}` exited {:?}. \
             Check that `/Applications/CuaDriver.app` is installed, or \
             pass --no-daemon-relaunch to bypass.",
                if pass_socket {
                    format!(" --socket {socket_path}")
                } else {
                    String::new()
                },
                status.code()
            ),
        });
    }

    // Poll the UDS until the daemon answers a probe or we time out.
    // 100ms tick matches Swift's `usleep(100_000)`.
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    while Instant::now() < deadline {
        if crate::serve::is_daemon_listening(socket_path) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    Err(LaunchDaemonError {
        kind: LaunchDaemonErrorKind::Timeout,
        message: format!(
            "daemon did not appear on {socket_path} within {timeout_secs}s. If this \
         is the first launch, grant Accessibility + Screen Recording to \
         CuaDriver.app in System Settings and retry. Pass --no-daemon-relaunch \
         to stay in-process."
        ),
    })
}

/// Run the MCP proxy path: ensure a daemon is up (spawning via
/// `open` if needed), then `crate::proxy::run_proxy` against its
/// socket. Builds its own tokio runtime — same shape as the other
/// `run_*` helpers in this file that own their event loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // Some outcomes are platform-specific.
pub enum McpDaemonStartup {
    AlreadyRunning,
    Launched,
    LaunchFailed,
    LaunchTimeout,
    Unreachable,
    UnsupportedRelaunch,
}

impl McpDaemonStartup {
    pub const fn telemetry_value(self) -> &'static str {
        match self {
            Self::AlreadyRunning => "already_running",
            Self::Launched => "launched",
            Self::LaunchFailed => "launch_failed",
            Self::LaunchTimeout => "launch_timeout",
            Self::Unreachable => "unreachable",
            Self::UnsupportedRelaunch => "unsupported_relaunch",
        }
    }
}

pub fn run_mcp_via_daemon_proxy<F>(
    socket: Option<String>,
    claude_code_compat: bool,
    on_startup: F,
) -> anyhow::Result<()>
where
    F: FnOnce(McpDaemonStartup, bool),
{
    let mut on_startup = Some(on_startup);
    // Windows: prefer the uiAccess'd worker pipe over the regular daemon pipe
    // when both are running, so MCP tool calls land in a process that can
    // bypass UIPI for UWP apps. The protocol on both pipes is identical so
    // the proxy doesn't need to know which one it's talking to. See #1602.
    let socket_path = if let Some(s) = socket {
        s
    } else {
        #[cfg(target_os = "windows")]
        {
            let uia = crate::serve::default_uia_pipe_path();
            if crate::serve::is_daemon_listening(&uia) {
                uia
            } else {
                crate::serve::default_socket_path()
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            crate::serve::default_socket_path()
        }
    };

    let already_running = crate::serve::is_daemon_listening(&socket_path);
    let mut daemon = McpDaemonStartup::AlreadyRunning;
    if !already_running {
        // CUA_DRIVER_RS_MCP_FORCE_PROXY callers (test harness, custom
        // bundle setups) supply their own daemon — skip the auto-
        // launch step, since they don't have an installed
        // CuaDriver.app to relaunch into. Fail fast if no daemon is
        // up at this point.
        if crate::bundle::is_env_truthy("CUA_DRIVER_RS_MCP_FORCE_PROXY") {
            if let Some(on_startup) = on_startup.take() {
                on_startup(McpDaemonStartup::Unreachable, false);
            }
            anyhow::bail!(
                "CUA_DRIVER_RS_MCP_FORCE_PROXY=1 but no daemon listening on \
                 {socket_path}. Start one with `cua-driver serve --socket {socket_path}` \
                 and retry."
            );
        }
        #[cfg(target_os = "macos")]
        {
            let socket_suffix = if socket_path != crate::serve::default_socket_path() {
                format!(" --socket {socket_path}")
            } else {
                String::new()
            };
            eprintln!(
                "cua-driver-rs: mcp launched without CuaDriver.app's TCC grants; \
                 auto-launching the daemon via `open -n -g -a CuaDriver --args serve{socket_suffix}` \
                 and proxying MCP requests through it. Pass --no-daemon-relaunch to stay in-process."
            );
            if let Err(error) = launch_daemon_and_wait(&socket_path, 10, claude_code_compat) {
                if let Some(on_startup) = on_startup.take() {
                    on_startup(
                        if error.kind == LaunchDaemonErrorKind::Timeout {
                            McpDaemonStartup::LaunchTimeout
                        } else {
                            McpDaemonStartup::LaunchFailed
                        },
                        false,
                    );
                }
                return Err(error.into());
            }
            daemon = McpDaemonStartup::Launched;
        }
        #[cfg(not(target_os = "macos"))]
        let _ = claude_code_compat;
        // On Linux / Windows there's no equivalent `open -a CuaDriver`
        // mechanism to spawn a daemon attributed to the user's
        // interactive session. The caller is expected to have one
        // running already (e.g. via `cua-driver autostart enable && kick`
        // on Windows). Bail with an actionable error rather than
        // silently falling back to an in-process server that would
        // be attributed to whatever session spawned us (typically
        // Session 0 over SSH).
        #[cfg(not(target_os = "macos"))]
        {
            if let Some(on_startup) = on_startup.take() {
                on_startup(McpDaemonStartup::UnsupportedRelaunch, false);
            }
            anyhow::bail!(
                "no Cua Driver daemon listening on {socket_path}. Start one in \
                 your interactive session — on Windows run \
                 `cua-driver autostart enable && cua-driver autostart kick`; \
                 on Linux run `cua-driver serve &` in the user's session. \
                 Then re-run `cua-driver mcp`. To skip the proxy and run \
                 in-process anyway (Session 0 attribution, GUI tools will \
                 return empty), pass --no-daemon-relaunch."
            );
        }
    }

    if let Some(on_startup) = on_startup.take() {
        on_startup(daemon, true);
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    rt.block_on(crate::proxy::run_proxy(socket_path))
}

/// Emit a stable, machine-readable JSON description of the cua-driver CLI
/// surface — subcommands, their args, the canonical MCP invocation, version.
///
/// The shape is purely additive — `schema_version` is bumped on breaking
/// changes; new fields appear without a bump. Designed so downstream
/// consumers (Hermes, Claude Code, future SDKs) can drop their hardcoded
/// `_CUA_DRIVER_ARGS = ["mcp"]` and read the canonical invocation from the
/// binary itself.
///
/// Mirrors the read-only inspection shape of `dump-docs` and `mcp-config`.
pub fn run_manifest(pretty: bool) {
    let manifest = build_manifest();
    let out = if pretty {
        serde_json::to_string_pretty(&manifest).unwrap_or_else(|_| manifest.to_string())
    } else {
        manifest.to_string()
    };
    println!("{out}");
}

/// Build the JSON manifest document. Pure function — surfaced separately
/// from `run_manifest` so tests can introspect the shape without going
/// through stdout.
pub fn build_manifest() -> serde_json::Value {
    // Resolve the binary path the way `run_mcp_config` already does so the
    // emitted `mcp_invocation.command` is the actually-runnable path, not
    // a bare "cua-driver" the caller has to resolve.
    let binary = std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(str::to_owned))
        .unwrap_or_else(|| "cua-driver".to_owned());

    serde_json::json!({
        // `schema_version` is bumped only on a breaking change to the
        // manifest shape itself. Additive field changes don't bump it.
        // Consumers branch on this when reading the document.
        "schema_version": "1",
        "binary_version": env!("CARGO_PKG_VERSION"),
        "binary_path": binary,
        "mcp_invocation": {
            "command": binary,
            "args": ["mcp"]
        },
        // Subcommand catalog — keep in sync with `parse_command` above.
        // `args` is a hint shape for consumers; the canonical source is
        // `--help` text. Each entry follows the same JSON shape so the
        // consumer can render uniformly.
        "subcommands": [
            { "name": "mcp",
              "description": "Run the MCP JSON-RPC server over stdio (the default invocation).",
              "args": [
                  { "name": "--no-daemon-relaunch", "type": "flag", "description": "Skip the bundle-based TCC auto-relaunch and stay in-process." },
                  { "name": "--socket", "type": "string", "description": "Override the daemon proxy UDS path." },
                  { "name": "--claude-code-computer-use-compat", "type": "flag", "description": "Select the Claude Code computer-use compat tool surface." },
                  { "name": "--embedded", "type": "flag", "description": "Run embedded inside a host app: inherit the host's TCC grants, never prompt or relaunch. Also CUA_DRIVER_EMBEDDED=1." },
                  { "name": "--host-bundle-id", "type": "string", "description": "Advisory host bundle id label echoed in check_permissions output." }
              ] },
            { "name": "serve",
              "description": "Run the long-lived daemon — backs the proxy/auto-relaunch path on macOS and the autostart Session 1+ daemon on Windows.",
              "args": [
                  { "name": "--socket", "type": "string", "description": "Override the listen socket path." },
                  { "name": "--no-permissions-gate", "type": "flag", "description": "Skip the macOS TCC first-launch gate." },
                  { "name": "--claude-code-computer-use-compat", "type": "flag", "description": "Forwarded by the MCP proxy when the client asked for the compat surface." },
                  { "name": "--embedded", "type": "flag", "description": "Run embedded inside a host app: inherit the host's TCC grants, never prompt or relaunch. Also CUA_DRIVER_EMBEDDED=1." },
                  { "name": "--host-bundle-id", "type": "string", "description": "Advisory host bundle id label echoed in check_permissions output." }
              ] },
            { "name": "stop",
              "description": "Stop a running daemon by sending it a shutdown request.",
              "args": [ { "name": "--socket", "type": "string", "description": "Override the daemon socket path." } ] },
            { "name": "status",
              "description": "Report daemon status (running / not / unhealthy).",
              "args": [ { "name": "--socket", "type": "string", "description": "Override the daemon socket path." } ] },
            { "name": "list-tools",
              "description": "Print the canonical tool name + one-line summary for every registered MCP tool.",
              "args": [] },
            { "name": "describe",
              "description": "Print a single tool's full description + JSON input schema.",
              "args": [ { "name": "tool", "type": "positional-string", "description": "Tool name." } ] },
            { "name": "call",
              "description": "Invoke a single tool one-shot — proxies to a running daemon when one is up, otherwise runs in-process.",
              "args": [
                  { "name": "tool", "type": "positional-string", "description": "Tool name." },
                  { "name": "json-args", "type": "positional-json", "description": "Tool input JSON (or read from stdin)." },
                  { "name": "--screenshot-out-file", "type": "string", "description": "Write image content to this path instead of emitting base64." },
                  { "name": "--socket", "type": "string", "description": "Override the daemon socket path used by the in-process forwarding fallback." }
              ] },
            { "name": "mcp-config",
              "description": "Print the MCP server config snippet or a client-specific install command.",
              "args": [ { "name": "--client", "type": "string", "description": "One of: claude, codex, cursor, hermes, antigravity, openclaw, opencode, pi, qwen, droid, zcode. Omit for the generic snippet." } ] },
            { "name": "manifest",
              "description": "Emit this machine-readable description of the CLI surface.",
              "args": [ { "name": "--pretty", "type": "flag", "description": "Pretty-print the JSON." } ] },
            { "name": "recording",
              "description": "Recording sub-API: start | stop | status | render.",
              "args": [
                  { "name": "subcommand", "type": "positional-string", "description": "One of: start, stop, status, render. Default: status." },
                  { "name": "--socket", "type": "string", "description": "Override the daemon socket path." }
              ] },
            { "name": "dump-docs",
              "description": "Dump every registered tool's docs as one document (markdown by default, JSON with --type json).",
              "args": [
                  { "name": "--pretty", "type": "flag", "description": "Pretty-print." },
                  { "name": "--type", "type": "string", "description": "Output type." }
              ] },
            { "name": "update",
              "description": "Check GitHub for a newer release; with --apply, download and install via the canonical installer.",
              "args": [
                  { "name": "--apply", "type": "flag", "description": "Apply the update." },
                  { "name": "--json", "type": "flag", "description": "Emit the structured check payload." }
              ] },
            { "name": "check-update",
              "description": "Read-only release-check verb (mirror of the check_for_update MCP tool).",
              "args": [
                  { "name": "--json", "type": "flag", "description": "Emit the structured check payload." },
                  { "name": "--no-cache", "type": "flag", "description": "Force a fresh GitHub round-trip." }
              ] },
            { "name": "doctor",
              "description": "Self-diagnose probes for runtime prerequisites (permissions, accessibility, capture, etc.).",
              "args": [ { "name": "--json", "type": "flag", "description": "Machine-readable doctor report." } ] },
            { "name": "diagnose",
              "description": "Emit a developer-focused diagnostic dump suitable for bug reports.",
              "args": [] },
            { "name": "permissions",
              "description": "Inspect / raise TCC permission grants (macOS).",
              "args": [
                  { "name": "subcommand", "type": "positional-string", "description": "status | grant" },
                  { "name": "--json", "type": "flag", "description": "Machine-readable payload." }
              ] },
            { "name": "config",
              "description": "Read / write the persistent driver config.",
              "args": [
                  { "name": "subcommand", "type": "positional-string", "description": "show | get | set | reset" },
                  { "name": "key", "type": "positional-string", "description": "Config key (for get/set)." },
                  { "name": "value", "type": "positional-string", "description": "Config value (for set)." },
                  { "name": "--socket", "type": "string", "description": "Override the daemon socket path." }
              ] },
            { "name": "telemetry",
              "description": "Inspect or change content-free telemetry and its pseudonymous installation identity.",
              "args": [
                  { "name": "subcommand", "type": "positional-string", "description": "enable | disable | status | reset-id | inspect" },
                  { "name": "event", "type": "positional-string", "description": "Fixed event name for inspect." },
                  { "name": "--json", "type": "flag", "description": "Emit machine-readable status or inspection output." }
              ] },
            { "name": "autostart",
              "description": "Platform-native auto-start so `cua-driver serve` comes up on every logon.",
              "args": [ { "name": "subcommand", "type": "positional-string", "description": "enable | disable | status | kick" } ] },
            { "name": "skills",
              "description": "Manage the cua-driver agent skill pack (install / update / uninstall / status / path).",
              "args": [ { "name": "subcommand", "type": "positional-string", "description": "install | update | uninstall | status | path. Default: status." } ] }
        ]
    })
}

/// Print the MCP server config snippet or a client-specific install command.
///
/// `--client <name>` selects one of: claude, codex, cursor, hermes,
/// antigravity, openclaw, opencode, pi. Omit for the generic JSON snippet.
pub fn run_mcp_config(client: Option<&str>) {
    let binary = std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(str::to_owned))
        .unwrap_or_else(|| "cua-driver".to_owned());

    match client {
        None | Some("") => {
            println!(
                r#"{{
  "mcpServers": {{
    "cua-driver": {{
      "command": "{binary}",
      "args": ["mcp"]
    }}
  }}
}}"#
            );
        }
        Some("claude") | Some("claude-code") => {
            // Claude Code wants the MCP server registered as
            // `cua-computer-use` — the bare key "computer-use" is
            // reserved, so external stdio registrations use a distinct
            // key. We emit `--scope user` so the entry lands in
            // `~/.claude.json` and is visible from every Claude Code
            // session regardless of cwd. Without it, `claude mcp add`
            // defaults to the per-project config (`<cwd>/.claude.json`),
            // which is the source of the "registered cua-driver but
            // Claude Code doesn't see it" surprise users hit.
            //
            // The `--claude-code-computer-use-compat` flag is NOT
            // emitted: the only tool it ever gated (the compat
            // screenshot) was removed in #1692, so the flag is a no-op
            // today on every code path. Carrying it confused Claude
            // Code's tool indexer in observed sessions; dropping it
            // makes the registration shape match what `--client codex`,
            // `--client cursor`, etc. already produce.
            //
            // Why `add-json` instead of `add -- BIN --flag`? PowerShell's
            // native-command arg parser mangles long flags after a bare
            // `--`, so the canonical `claude mcp add NAME -- BIN mcp
            // --extra-flag` form errors with "unknown option …" on
            // Windows. `add-json` takes the whole config as one JSON
            // string, sidestepping the bare-dash issue.
            //
            // Why a per-OS escape on the JSON literal? PowerShell 5.1's
            // native-command arg passing strips the inner `"` characters
            // when crossing the PS → exe boundary unless they're
            // backslash-escaped inside the single-quoted literal. The
            // bash form keeps the raw quotes. The two escapings are
            // mutually exclusive — verified by piping each into the
            // other shell — so we emit per-host.
            //
            // Forward slashes in the binary path because Windows accepts
            // them and avoid backslash-soup inside the JSON literal.
            let normalised = binary.replace('\\', "/");
            let cfg = serde_json::json!({
                "command": normalised,
                "args": ["mcp"],
            });
            let json = cfg.to_string();
            #[cfg(windows)]
            let json = json.replace('"', "\\\"");
            println!(
                "claude mcp add-json --scope user cua-computer-use '{}'",
                json
            );
        }
        Some("codex") => {
            println!("codex mcp add cua-driver -- {binary} mcp");
        }
        Some("cursor") => {
            println!(
                r#"{{
  "mcpServers": {{
    "cua-driver": {{
      "command": "{binary}",
      "args": ["mcp"],
      "type": "stdio"
    }}
  }}
}}"#
            );
        }
        Some("openclaw") => {
            println!(
                "openclaw mcp set cua-driver '{{\"command\":\"{binary}\",\"args\":[\"mcp\"]}}'"
            );
        }
        Some("opencode") => {
            println!(
                r#"// paste under "mcp" in opencode.json:
{{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {{
    "cua-driver": {{
      "type": "local",
      "command": ["{binary}", "mcp"],
      "enabled": true
    }}
  }}
}}"#
            );
        }
        Some("hermes") => {
            println!("# paste under mcp_servers in ~/.hermes/config.yaml,");
            println!("# then run /reload-mcp inside Hermes:");
            println!("mcp_servers:");
            println!("  cua-driver:");
            println!("    command: \"{binary}\"");
            println!("    args: [\"mcp\"]");
        }
        Some("antigravity") | Some("gemini") => {
            // Google Antigravity CLI (the `agy` binary, successor to Gemini
            // CLI as of May 2026 — Gemini CLI support sunsets 2026-06-18 for
            // consumers per the developers.googleblog.com transition post)
            // has no `agy mcp add` subcommand: MCP servers are registered by
            // editing JSON directly. Both Antigravity CLI and Antigravity
            // IDE read from the SAME mcp_config.json at:
            //
            //   Unix:    ~/.gemini/config/mcp_config.json
            //   Windows: %USERPROFILE%\.gemini\config\mcp_config.json
            //
            // (Antigravity inherited the `.gemini` directory tree from the
            // old Gemini CLI install path on purpose — same config carries
            // over.) An additional CLI-only path at
            // ~/.gemini/antigravity-cli/mcp_config.json takes precedence
            // for CLI when present; we register at the shared path so the
            // IDE picks the same server up.
            //
            // Reload after edit: restart `agy` (Antigravity CLI has no
            // mid-session config-reload hook).
            //
            // The `gemini` client alias points at the same instructions
            // so anyone with old muscle memory typing `--client gemini`
            // gets the right (forward-compatible) config.
            let normalised = binary.replace('\\', "/");
            // Emit the full mcp_config.json envelope so the user can paste
            // it verbatim into a fresh file (or merge under "mcpServers" in
            // an existing one). Single pretty-printed JSON object keeps
            // both shapes — full file and merge fragment — useful.
            let full = serde_json::json!({
                "mcpServers": {
                    "cua-driver": {
                        "command": normalised,
                        "args": ["mcp"],
                    }
                }
            });
            let pretty = serde_json::to_string_pretty(&full).unwrap_or_else(|_| full.to_string());
            println!(
                "# Antigravity CLI (the `agy` binary) reads MCP server configs from:\n\
                 #   ~/.gemini/config/mcp_config.json   (Unix)\n\
                 #   %USERPROFILE%\\.gemini\\config\\mcp_config.json   (Windows)\n\
                 #\n\
                 # No `agy` subcommand for this — drop the JSON below into that file (or\n\
                 # merge under the existing top-level \"mcpServers\" object if it already\n\
                 # exists), then restart `agy` to pick up the change.\n\
                 #\n\
                 # The same file is shared with the Antigravity IDE.\n\
                 {pretty}",
            );
        }
        Some("pi") => {
            println!(
                "Pi (badlogic/pi-mono) does not support MCP natively — the author\n\
                 has stated MCP support will not be added for context-budget reasons.\n\n\
                 Use cua-driver as a plain CLI from inside Pi instead:\n\n\
                     {binary} list_apps\n\
                     {binary} click  '{{\"pid\": 1234, \"x\": 100, \"y\": 200}}'\n\
                     {binary} --help        # full tool catalog\n\n\
                 Each call is one-shot and returns JSON / text on stdout, which is\n\
                 exactly the shape Pi is designed around."
            );
        }
        Some("qwen") | Some("qwen-code") => {
            // Qwen Code (Alibaba's open-source coding CLI, a Gemini-CLI fork).
            // Config: ~/.qwen/settings.json (user) or .qwen/settings.json
            // (project), top-level "mcpServers" keyed by name. It also ships a
            // CLI: `qwen mcp add <name> <command> [args...]`.
            println!("qwen mcp add cua-driver {binary} mcp");
        }
        Some("droid") | Some("factory") => {
            // Factory Droid CLI. Config: ~/.factory/mcp.json (user) or
            // .factory/mcp.json (folder/project), top-level "mcpServers" with
            // "type":"stdio". The CLI takes command+args as one quoted string.
            println!("droid mcp add cua-driver \"{binary} mcp\"");
        }
        Some("zcode") => {
            // ZCode by Z.ai (GLM coding harness) — a GUI app. MCP servers are
            // added in Settings -> MCP Servers -> New MCP Server (type: stdio),
            // or by pasting JSON under "Full configuration". No CLI and no
            // documented config-file path, so emit the JSON to paste. (Z.ai's
            // separate `zai` CLI does have `zai mcp add` — noted below.)
            let normalised = binary.replace('\\', "/");
            let full = serde_json::json!({
                "mcpServers": {
                    "cua-driver": {
                        "command": normalised,
                        "args": ["mcp"],
                        "type": "stdio",
                    }
                }
            });
            let pretty = serde_json::to_string_pretty(&full).unwrap_or_else(|_| full.to_string());
            println!(
                "# ZCode (Z.ai) is a GUI app — add via Settings -> MCP Servers ->\n\
                 # New MCP Server (type: stdio), or paste this under \"Full\n\
                 # configuration\". If you use Z.ai's `zai` CLI instead, run:\n\
                 #   zai mcp add cua-driver --transport stdio --command \"{binary}\" --args mcp\n\
                 {pretty}",
            );
        }
        Some(other) => {
            eprintln!("Unknown client '{other}'. Valid: claude, codex, cursor, antigravity, openclaw, opencode, hermes, pi, qwen, droid, zcode.");
            process::exit(2);
        }
    }
}

/// Invoke a tool, forwarding to a running daemon if one is reachable;
/// otherwise runs in-process. Prints result to stdout on success, error
/// to stderr on failure. Exits 1 if the tool returned an error result.
/// When `screenshot_out_file` is provided, image content is written there
/// instead of emitted as base64 on stdout.
///
/// `socket` — override the daemon socket path (from --socket flag).
pub fn run_call(
    registry: std::sync::Arc<ToolRegistry>,
    tool: &str,
    json_args: Option<serde_json::Value>,
    screenshot_out_file: Option<String>,
    socket_override: Option<String>,
) {
    // Daemon forwarding: if a daemon is listening, proxy the request
    // through it so AppStateEngine's element_index cache is shared.
    //
    // On Windows, prefer the uiAccess-elevated worker (cua-driver-uia.exe) when
    // present — it runs at UIAccess integrity and bypasses UIPI for UWP apps
    // like Calculator / modern Notepad / Settings. The regular daemon at
    // `\\.\pipe\cua-driver` is Medium integrity and gets ERROR_ACCESS_DENIED on
    // SendInput into AppContainer'd processes. See #1602.
    //
    // When `socket_override` is Some (i.e. caller passed `--socket <path>`),
    // route directly to that path and skip the platform default + uia worker
    // search. Used by integration tests to drive a tempfile-socketed daemon.
    let socket_path = if let Some(s) = socket_override {
        s
    } else {
        #[cfg(target_os = "windows")]
        {
            let uia = crate::serve::default_uia_pipe_path();
            if crate::serve::is_daemon_listening(&uia) {
                uia
            } else {
                crate::serve::default_socket_path()
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            crate::serve::default_socket_path()
        }
    };
    // macOS: `check_permissions` with prompt:true raises a TCC dialog. Run
    // in-process from a terminal, that dialog attributes to the *terminal*
    // (LaunchServices' "responsible" process), not to com.trycua.driver —
    // so the grant lands on the wrong app and never sticks for the driver.
    // When we're a bundle CLI spawned from a terminal (should_use_daemon_proxy)
    // and there's no daemon to route through, DON'T raise the mis-attributed
    // prompt: degrade to report-only and tell the user the one launch that
    // grants correctly (`open … CuaDriver --args serve`, which raises the
    // dialog as CuaDriver and waits for the grant). We deliberately do NOT
    // auto-spawn that daemon here — a `call` shouldn't leave a background
    // daemon behind, and the first-launch gate can lag socket creation.
    #[cfg(target_os = "macos")]
    let json_args = {
        let mut effective = json_args;
        let wants_prompt = effective
            .as_ref()
            .and_then(|v| v.get("prompt"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true); // check_permissions defaults prompt:true
        if tool == "check_permissions"
            && wants_prompt
            && !crate::serve::is_daemon_listening(&socket_path)
            && should_use_daemon_proxy(false)
        {
            eprintln!(
                "cua-driver-rs: reporting permission status only. A prompt raised from \
                 this terminal would attribute to the terminal, not CuaDriver, so the \
                 grant wouldn't apply to the driver. To grant correctly, launch the \
                 driver as its own app:\n  open -n -g -a CuaDriver --args serve\n\
                 then approve the CuaDriver dialog in System Settings."
            );
            effective = Some(serde_json::json!({ "prompt": false }));
        }
        effective
    };
    if crate::serve::is_daemon_listening(&socket_path) {
        let mut args_for_daemon = json_args
            .clone()
            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
        cua_driver_core::tool_args::sanitize_reserved_args(&mut args_for_daemon);
        let req = crate::serve::DaemonRequest {
            method: "call".into(),
            name: Some(tool.to_owned()),
            args: Some(args_for_daemon),
            // CLI one-shot is its own ephemeral, anonymous/global session.
            session_id: None,
            observation_origin: Some(crate::serve::ToolObservationOrigin::Direct),
        };
        match crate::serve::send_request(&socket_path, &req) {
            Ok(resp) => {
                if resp.ok {
                    if let Some(result) = resp.result {
                        // Walk the content array once: pick up any Image
                        // payloads (either to write to --screenshot-out-file
                        // or to merge into structuredContent below).
                        let mut printed = false;
                        let mut image_b64: Option<(String, String)> = None;
                        if let Some(content) = result.get("content").and_then(|v| v.as_array()) {
                            for item in content {
                                if item.get("type").and_then(|v| v.as_str()) == Some("image") {
                                    let b64 = item
                                        .get("data")
                                        .and_then(|v| v.as_str())
                                        .map(str::to_owned);
                                    let mime = item
                                        .get("mimeType")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("image/png")
                                        .to_owned();
                                    if let Some(b64) = b64 {
                                        if let Some(ref path) = screenshot_out_file {
                                            use base64::Engine as _;
                                            match base64::engine::general_purpose::STANDARD
                                                .decode(&b64)
                                            {
                                                Ok(bytes) => {
                                                    if let Err(e) = std::fs::write(path, &bytes) {
                                                        eprintln!("--screenshot-out-file: failed to write {path}: {e}");
                                                    }
                                                }
                                                Err(e) => {
                                                    eprintln!("--screenshot-out-file: base64 decode failed: {e}");
                                                }
                                            }
                                        } else {
                                            // Stash for the structuredContent merge below.
                                            image_b64 = Some((b64, mime));
                                        }
                                    }
                                }
                            }
                        }
                        if let Some(sc) = result.get("structuredContent") {
                            // Merge image data into the structured payload
                            // (matches in-process behaviour at the bottom of
                            // this fn) so `cua-driver call screenshot` over
                            // the daemon socket still emits
                            // `screenshot_png_b64`. Previously this path
                            // dropped the image entirely when no
                            // --screenshot-out-file was given.
                            let mut obj = sc.clone();
                            if let Some((b64, mime)) = image_b64 {
                                if let serde_json::Value::Object(ref mut map) = obj {
                                    map.insert(
                                        "screenshot_png_b64".into(),
                                        serde_json::Value::String(b64),
                                    );
                                    map.insert(
                                        "screenshot_mime_type".into(),
                                        serde_json::Value::String(mime),
                                    );
                                }
                            }
                            let pretty = serde_json::to_string_pretty(&obj)
                                .unwrap_or_else(|_| obj.to_string());
                            println!("{pretty}");
                            printed = true;
                        }
                        if !printed {
                            if let Some(content) = result.get("content").and_then(|v| v.as_array())
                            {
                                for item in content {
                                    if item.get("type").and_then(|v| v.as_str()) == Some("text") {
                                        if let Some(text) =
                                            item.get("text").and_then(|v| v.as_str())
                                        {
                                            println!("{text}");
                                        }
                                    }
                                }
                            }
                        }
                    }
                    return;
                } else {
                    if let Some(err) = resp.error {
                        eprintln!("{err}");
                    }
                    let exit_code = resp.exit_code.unwrap_or(1);
                    process::exit(exit_code);
                }
            }
            Err(e) => {
                // Daemon became unreachable mid-call — fall through to in-process.
                // Promoted from `tracing::debug!` to `eprintln!` so callers see
                // the degradation: in-process execution gets a FRESH ToolState,
                // which means state-dependent tools (`click`, `type_text`,
                // `set_value` — anything that reads the element_index cache)
                // will fail with "Element N not in cache" even when a prior
                // `get_window_state` populated the daemon's cache, because the
                // daemon's cache and the in-process cache are different.
                eprintln!(
                    "[cua-driver] WARNING: daemon proxy to {socket_path} failed ({e}); \
                     running '{tool}' in-process. State-dependent tools may misbehave."
                );
            }
        }
    }
    if registry.get_def(tool).is_none() {
        eprintln!("Unknown tool: {tool}");
        eprintln!("Run `cua-driver list-tools` to see available tools.");
        process::exit(64);
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    let mut args = json_args.unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
    cua_driver_core::tool_args::sanitize_reserved_args(&mut args);
    let session_context = cua_driver_core::session::begin_tool_call(
        tool,
        &args,
        true,
        cua_driver_core::session::SessionTransport::Cli,
    );
    let operation = cua_driver_core::server::tool_operation(tool, Some(&args));
    let observation_timer = cua_driver_core::server::ToolObservationTimer::start_with_operation(
        tool.to_owned(),
        operation,
        true,
        true,
        cua_driver_core::server::StdioExecutionPath::InProcess,
    );
    let tool_name = tool.to_string();
    let out_path = screenshot_out_file;
    let is_error = rt.block_on(async move {
        let result = registry.invoke(&tool_name, args).await;
        let is_err = result.is_error.unwrap_or(false);
        if let Ok(value) = serde_json::to_value(&result) {
            let response = cua_driver_core::protocol::Response::ok(serde_json::Value::Null, value);
            let outcome = observation_timer.finish(&response);
            if let Some(context) = session_context {
                context.complete(&outcome);
            }
            crate::telemetry::capture_tool_completed(outcome, crate::telemetry::Transport::Cli);
        }

        // Emit content.
        let mut has_printed = false;
        let mut image_b64: Option<(String, String)> = None; // (base64, mime)
        for item in &result.content {
            match item {
                Content::Text { text, .. } => {
                    if is_err {
                        eprintln!("{text}");
                    } else {
                        // Only print text when there is no structuredContent
                        // (structuredContent path prints below).
                        if result.structured_content.is_none() {
                            println!("{text}");
                            has_printed = true;
                        }
                    }
                }
                Content::Image {
                    data, mime_type, ..
                } => {
                    image_b64 = Some((data.clone(), mime_type.clone()));
                }
            }
        }

        // If --screenshot-out-file was provided, write the image there
        // and suppress it from the JSON output (same as Swift reference).
        if let Some(ref path) = out_path {
            if let Some((b64, _mime)) = image_b64.take() {
                use base64::Engine as _;
                match base64::engine::general_purpose::STANDARD.decode(&b64) {
                    Ok(bytes) => {
                        if let Err(e) = std::fs::write(path, &bytes) {
                            eprintln!("--screenshot-out-file: failed to write {path}: {e}");
                        }
                    }
                    Err(e) => {
                        eprintln!("--screenshot-out-file: base64 decode failed: {e}");
                    }
                }
            } else {
                eprintln!(
                    "--screenshot-out-file: no image content in tool response; file not written"
                );
            }
        }

        // If there's structuredContent, print it as JSON (with image merged in if no out_path).
        if let Some(sc) = &result.structured_content {
            if !is_err {
                let mut obj = sc.clone();
                if out_path.is_none() {
                    if let Some((b64, mime)) = image_b64 {
                        if let serde_json::Value::Object(ref mut map) = obj {
                            map.insert("screenshot_png_b64".into(), serde_json::Value::String(b64));
                            map.insert(
                                "screenshot_mime_type".into(),
                                serde_json::Value::String(mime),
                            );
                        }
                    }
                }
                let pretty = serde_json::to_string_pretty(&obj).unwrap_or_else(|_| obj.to_string());
                println!("{pretty}");
                has_printed = true;
            }
        }

        let _ = has_printed;
        is_err
    });

    if is_error {
        process::exit(1);
    }
}

/// `cua-driver recording <start|stop|status>` — wrapper around
/// `start_recording` / `stop_recording` / `get_recording_state` tools
/// on the running daemon.
///
/// Requires a running daemon (`cua-driver serve`) because recording
/// state lives in-process.
pub fn run_recording_cmd(subcommand: &str, args: &[String], socket: Option<&str>) {
    // `render` is pure file-to-file work that doesn't need the daemon;
    // dispatch it before the daemon-running check so it works without
    // a running `cua-driver serve`.
    if subcommand == "render" {
        run_recording_render(args);
        return;
    }

    let socket_path = socket
        .map(str::to_owned)
        .unwrap_or_else(crate::serve::default_socket_path);

    if !crate::serve::is_daemon_listening(&socket_path) {
        eprintln!(
            "Cua Driver daemon is not running.\n\
             Start it first with: cua-driver serve"
        );
        process::exit(1);
    }

    match subcommand {
        "start" => {
            let output_dir = match args.first() {
                Some(d) => {
                    // Expand ~ manually.
                    if d.starts_with('~') {
                        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
                        d.replacen('~', &home, 1)
                    } else {
                        d.clone()
                    }
                }
                None => {
                    eprintln!("Usage: cua-driver recording start <output-dir>");
                    process::exit(64);
                }
            };

            // Create the directory if it doesn't exist.
            if let Err(e) = std::fs::create_dir_all(&output_dir) {
                eprintln!("Failed to create output directory {output_dir}: {e}");
                process::exit(1);
            }

            let req = crate::serve::DaemonRequest {
                method: "call".into(),
                name: Some("start_recording".into()),
                args: Some(serde_json::json!({
                    "output_dir": output_dir
                })),
                // CLI `recording start` is anonymous — the recording is owned by
                // nobody, so only an unconditional stop (CLI / manual) reaps it.
                session_id: None,
                observation_origin: Some(crate::serve::ToolObservationOrigin::Direct),
            };
            match crate::serve::send_request(&socket_path, &req) {
                Ok(resp) if resp.ok => {
                    println!("Recording started → {output_dir}");
                    // Query state to show next_turn.
                    let state_req = crate::serve::DaemonRequest {
                        method: "call".into(),
                        name: Some("get_recording_state".into()),
                        args: Some(serde_json::json!({})),
                        session_id: None,
                        observation_origin: Some(crate::serve::ToolObservationOrigin::Direct),
                    };
                    if let Ok(sr) = crate::serve::send_request(&socket_path, &state_req) {
                        if let Some(result) = sr.result {
                            let sc = result
                                .get("structuredContent")
                                .or_else(|| result.get("structured_content"));
                            if let Some(next_turn) =
                                sc.and_then(|s| s.get("next_turn")).and_then(|v| v.as_u64())
                            {
                                println!("Next turn: {next_turn:05}");
                            }
                        }
                    }
                }
                Ok(resp) => {
                    if let Some(e) = resp.error {
                        eprintln!("{e}");
                    }
                    process::exit(1);
                }
                Err(e) => {
                    eprintln!("recording start: {e}");
                    process::exit(1);
                }
            }
        }

        "stop" => {
            let req = crate::serve::DaemonRequest {
                method: "call".into(),
                name: Some("stop_recording".into()),
                args: Some(serde_json::json!({})),
                session_id: None,
                observation_origin: Some(crate::serve::ToolObservationOrigin::Direct),
            };
            match crate::serve::send_request(&socket_path, &req) {
                Ok(resp) if resp.ok => println!("Recording stopped."),
                Ok(resp) => {
                    if let Some(e) = resp.error {
                        eprintln!("{e}");
                    }
                    process::exit(1);
                }
                Err(e) => {
                    eprintln!("recording stop: {e}");
                    process::exit(1);
                }
            }
        }

        "status" | "" => {
            let req = crate::serve::DaemonRequest {
                method: "call".into(),
                name: Some("get_recording_state".into()),
                args: Some(serde_json::json!({})),
                session_id: None,
                observation_origin: Some(crate::serve::ToolObservationOrigin::Direct),
            };
            match crate::serve::send_request(&socket_path, &req) {
                Ok(resp) if resp.ok => {
                    if let Some(result) = resp.result {
                        let sc = result
                            .get("structuredContent")
                            .or_else(|| result.get("structured_content"))
                            .cloned()
                            .unwrap_or(serde_json::json!({}));
                        let enabled = sc.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
                        let out_dir = sc
                            .get("output_dir")
                            .and_then(|v| v.as_str())
                            .unwrap_or("(none)");
                        let next_turn = sc.get("next_turn").and_then(|v| v.as_u64()).unwrap_or(0);
                        println!(
                            "Recording: {}",
                            if enabled { "enabled" } else { "disabled" }
                        );
                        if enabled {
                            println!("  output_dir: {out_dir}");
                            println!("  next_turn:  {next_turn:05}");
                        }
                    }
                }
                Ok(resp) => {
                    if let Some(e) = resp.error {
                        eprintln!("{e}");
                    }
                    process::exit(1);
                }
                Err(e) => {
                    eprintln!("recording status: {e}");
                    process::exit(1);
                }
            }
        }

        other => {
            eprintln!("Unknown recording subcommand '{other}'. Valid: start <dir>, stop, status, render <dir> --output <out.mp4>");
            process::exit(64);
        }
    }
}

/// `cua-driver recording render <input-dir> <out.mp4> [--no-zoom] [--scale N]`
/// Pure file-to-file work — does NOT go through the daemon.
///
/// Note on flag parsing: the global cua-driver CLI parser strips
/// recognised flags before this subcommand sees them, leaving only
/// positionals. So instead of `--output <out>` (which the parser
/// would consume and lose) we accept the output path as the second
/// positional. `--no-zoom` and `--scale N` survive because their
/// values are inline (no separate value token).
fn run_recording_render(args: &[String]) {
    // First positional = input dir, second positional = output mp4.
    let positionals: Vec<&String> = args.iter().filter(|s| !s.starts_with("--")).collect();
    let input_dir = match positionals.get(0) {
        Some(s) if !s.is_empty() => std::path::PathBuf::from(s),
        _ => {
            eprintln!(
                "Usage: cua-driver recording render <input-dir> <out.mp4> [--no-zoom] [--scale N]"
            );
            process::exit(64);
        }
    };
    let output_path = match positionals.get(1) {
        Some(s) if !s.is_empty() => std::path::PathBuf::from(s),
        _ => {
            eprintln!(
                "Usage: cua-driver recording render <input-dir> <out.mp4> [--no-zoom] [--scale N]"
            );
            eprintln!("(second positional argument is the output path)");
            process::exit(64);
        }
    };
    let mut no_zoom = false;
    let mut scale: f64 = 2.0;
    let mut iter = args.iter();
    while let Some(a) = iter.next() {
        match a.as_str() {
            "--no-zoom" => no_zoom = true,
            "--scale" => {
                if let Some(v) = iter.next() {
                    if let Ok(f) = v.parse::<f64>() {
                        scale = f;
                    }
                }
            }
            _ => {}
        }
    }

    let opts = cua_driver_core::recording_render::RenderOptions {
        no_zoom,
        default_scale: scale,
    };
    println!(
        "Rendering {} -> {}{}",
        input_dir.display(),
        output_path.display(),
        if no_zoom { " (no-zoom baseline)" } else { "" }
    );
    match cua_driver_core::recording_render::render(&input_dir, &output_path, &opts) {
        Ok(res) => {
            println!("✅ Wrote {}", res.output_path.display());
            println!("   input_duration_ms: {:.0}", res.input_duration_ms);
            println!("   zoom_region_count: {}", res.zoom_region_count);
        }
        Err(e) => {
            eprintln!("Render failed: {e}");
            process::exit(1);
        }
    }
}

/// `cua-driver update [--apply]` — check for a newer release and optionally apply it.
///
/// Shares the GitHub releases fetch with the startup banner via
/// [`crate::version_check::fetch_latest_version`] so both code paths agree on
/// tag filtering and HTTP semantics. `--apply` delegates to the canonical
/// installer script — see [`crate::updater`] for why we go through the script
/// instead of re-implementing the asset resolution + atomic swap + GC in Rust.
pub fn run_update_cmd(apply: bool, json: bool) {
    // `--json` short-circuits the text path entirely so scripted callers
    // get a parseable payload regardless of `--apply`. The check itself
    // routes through the same `check_update_state` the `check-update`
    // verb and the MCP tool use, so all three surfaces agree.
    if json {
        let state = crate::version_check::check_update_state(false);
        let val = serde_json::to_value(&state).unwrap_or_else(|_| serde_json::json!({}));
        let pretty = serde_json::to_string_pretty(&val).unwrap_or_else(|_| val.to_string());
        println!("{pretty}");
        // `--apply` still installs when JSON is on — the JSON is just the
        // pre-install snapshot. Returning here when apply is false keeps
        // the existing "check + suggest" behaviour off the JSON path.
        if !apply {
            return;
        }
    }

    let current = env!("CARGO_PKG_VERSION");
    if !json {
        println!("Current version: {current}");
        println!("Checking for updates…");
    }

    let latest = crate::version_check::fetch_latest_version();
    match latest {
        Err(e) => {
            // The shared helper returns a human-readable error string for
            // the CLI surface — pass it through so the user can see why
            // (timeout, parse error, etc.) instead of just "unreachable".
            tracing::debug!(target: "cua_driver::update", "fetch failed: {e}");
            if !json {
                println!("Could not reach GitHub — check your connection and try again.");
            }
            process::exit(1);
        }
        Ok(v) if !crate::version_check::is_newer(&v, current) => {
            if !json {
                println!("Already up to date.");
            }
        }
        Ok(v) => {
            if !json {
                println!("New version available: {v}");
            }

            if !apply {
                if !json {
                    println!();
                    println!("Run with --apply to download and install it:");
                    println!("  cua-driver update --apply");
                    println!();
                    println!("Or reinstall directly:");
                    println!("  {}", crate::updater::manual_install_one_liner());
                }
                return;
            }

            if !json {
                println!("Downloading and installing cua-driver {v}…");
            }
            let daemon_was_running = crate::updater::daemon_is_running();
            match crate::updater::run_install_script(&v) {
                Ok(s) if s.success() => {
                    if !json {
                        println!("Installed cua-driver {v}.");
                    }
                    if daemon_was_running {
                        // The atomic swap (symlink retarget / junction flip)
                        // means the running daemon kept executing the old
                        // binary — restart picks up the new one.
                        println!();
                        println!("A daemon was running before the install. Restart it to pick up the new binary:");
                        println!("  cua-driver stop && cua-driver serve");
                    }
                }
                Ok(s) => {
                    eprintln!(
                        "Installation failed (exit {}). Re-run install manually:",
                        s.code().unwrap_or(1)
                    );
                    eprintln!("  {}", crate::updater::manual_install_one_liner());
                    process::exit(s.code().unwrap_or(1));
                }
                Err(e) => {
                    eprintln!("Failed to launch installer: {e}");
                    #[cfg(windows)]
                    eprintln!("  (is powershell.exe on PATH?)");
                    #[cfg(not(windows))]
                    eprintln!("  (is bash + curl on PATH?)");
                    process::exit(1);
                }
            }
        }
    }
}

/// `cua-driver permissions status|grant`.
pub fn run_permissions_cmd(_registry: std::sync::Arc<ToolRegistry>, subcommand: &str, json: bool) {
    match subcommand {
        "status" => run_permissions_status(json),
        "grant" => run_permissions_grant(),
        other => {
            eprintln!("unknown permissions subcommand '{other}'. Valid: status, grant.");
            process::exit(2);
        }
    }
}

/// Report the CuaDriver daemon's TCC status — reliably, or not at all.
///
/// macOS attributes Accessibility / Screen-Recording to the *responsible
/// process*, so the ONLY process that can read `com.trycua.driver`'s real
/// grants is the daemon running as its own responsible process. When the
/// daemon is up we query it and report its
/// `driver-daemon`-attributed answer. When it is NOT up we deliberately
/// report `unknown` rather than fall back to an in-process check — that
/// fallback would report the *calling terminal's* grants and could print
/// `✅ granted` while the driver itself has none. An honest "unknown" beats a
/// confident lie. To grant + verify, use `cua-driver permissions grant`.
/// Never raises a prompt.
fn run_permissions_status(json: bool) {
    let socket = crate::serve::default_socket_path();

    // Only a listening daemon can answer for com.trycua.driver. A failed/!ok
    // response (e.g. daemon mid-re-exec during the gate's recheck window) is
    // treated the same as "no daemon" → unknown.
    let daemon_status: Option<serde_json::Value> = if crate::serve::is_daemon_listening(&socket) {
        let req = crate::serve::DaemonRequest {
            method: "call".into(),
            name: Some("check_permissions".into()),
            args: Some(serde_json::json!({ "prompt": false })),
            session_id: None,
            observation_origin: Some(crate::serve::ToolObservationOrigin::Direct),
        };
        crate::serve::send_request(&socket, &req)
            .ok()
            .filter(|r| r.ok)
            .and_then(|r| r.result)
            .and_then(|res| res.get("structuredContent").cloned())
            // Trust the booleans ONLY when the answering daemon is its own
            // responsible process (`driver-daemon`). Otherwise those grants
            // belong to the launching app, so we discard them and fall through
            // to `unknown`. A missing `source` (non-macOS, no TCC) is trusted
            // as-is.
            .filter(|s| {
                s.get("source")
                    .and_then(|src| src.get("attribution"))
                    .and_then(|v| v.as_str())
                    .map(|a| a == "driver-daemon")
                    .unwrap_or(true)
            })
    } else {
        None
    };

    let Some(structured) = daemon_status else {
        // No reliable answer. Emit NO accessibility/screen_recording booleans —
        // nothing downstream can misread a false `granted: true`.
        if json {
            let payload = serde_json::json!({
                "daemon_running": false,
                "status": "unknown",
                "reason": "no CuaDriver daemon is running under the driver's own identity \
                           (com.trycua.driver), so its real TCC status can't be read from this \
                           process. Run `cua-driver permissions grant` to grant + verify.",
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string())
            );
            return;
        }
        println!("Accessibility:    ❓ unknown");
        println!("Screen Recording: ❓ unknown");
        println!(
            "No CuaDriver daemon is running under the driver's own identity (com.trycua.driver), \
             so its real TCC status can't be read."
        );
        println!(
            "(A status check from this terminal would report the terminal's grants, not the \
             driver's.)"
        );
        println!("  → Run `cua-driver permissions grant` to grant + verify, or start the daemon");
        println!("    (`open -n -g -a CuaDriver --args serve`) and re-run this command.");
        return;
    };

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&structured).unwrap_or_else(|_| structured.to_string())
        );
        return;
    }

    let b = |k: &str| structured.get(k).and_then(|v| v.as_bool()).unwrap_or(false);
    let ax = b("accessibility");
    let sr = b("screen_recording");
    let cap = b("screen_recording_capturable");
    let attribution = structured
        .get("source")
        .and_then(|s| s.get("attribution"))
        .and_then(|v| v.as_str())
        .unwrap_or("driver-daemon");

    println!(
        "Accessibility:    {}",
        if ax { "✅ granted" } else { "❌ not granted" }
    );
    println!(
        "Screen Recording: {}",
        if sr { "✅ granted" } else { "❌ not granted" }
    );
    if sr && !cap {
        println!(
            "  ⚠️  preflight reports granted, but a live capture probe failed — the grant \
             likely belongs to another process, not this one."
        );
    }
    println!("Source: {attribution}");
    if !(ax && sr) {
        println!("  → To grant for the driver, run: cua-driver permissions grant");
    }
}

/// Launch CuaDriver via LaunchServices so the permission prompt attributes to
/// com.trycua.driver, wait (user-paced) for the daemon to come up — its socket
/// only appears once the permissions gate passes, i.e. the grant was given —
/// then report the driver's own status.
fn run_permissions_grant() {
    #[cfg(target_os = "macos")]
    {
        let socket = crate::serve::default_socket_path();
        if crate::serve::is_daemon_listening(&socket) {
            println!("CuaDriver daemon already running — checking its permissions…");
        } else {
            println!("Launching CuaDriver to request permissions.");
            println!(
                "A dialog titled \u{201c}Cua Driver\u{201d} will appear — approve Accessibility \
                 and Screen Recording in System Settings, then this command continues."
            );
            // Permissions-grant launch never needs the compat screenshot surface.
            if let Err(e) = launch_daemon_and_wait(&socket, 180, false) {
                eprintln!("\nDidn't detect the CuaDriver daemon: {e}");
                eprintln!(
                    "If you haven't yet, grant Accessibility + Screen Recording to CuaDriver \
                     in System Settings, then re-run `cua-driver permissions grant`."
                );
                process::exit(1);
            }
        }
        // Since #1761 the daemon binds its socket IMMEDIATELY — before the
        // permissions gate completes — so the first `check_permissions`
        // query returns "pending" while the grant is still missing. Poll
        // the daemon until both grants flip true (success) or we time out.
        //
        // The gate re-execs the daemon (~every 25s) to pick up an
        // Accessibility grant — `AXIsProcessTrusted` is cached per process
        // and only a fresh process image sees a later grant. During each
        // restart the socket briefly disappears, so tolerate transient
        // connection failures rather than bailing on the first one.
        let req = crate::serve::DaemonRequest {
            method: "call".into(),
            name: Some("check_permissions".into()),
            args: Some(serde_json::json!({ "prompt": false })),
            session_id: None,
            observation_origin: Some(crate::serve::ToolObservationOrigin::Direct),
        };
        let poll_deadline = std::time::Instant::now() + std::time::Duration::from_secs(180);
        let mut ax = false;
        let mut sr = false;
        loop {
            if let Some(structured) = crate::serve::send_request(&socket, &req)
                .ok()
                .filter(|r| r.ok)
                .and_then(|r| r.result)
                .and_then(|res| res.get("structuredContent").cloned())
            {
                ax = structured
                    .get("accessibility")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                sr = structured
                    .get("screen_recording")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if ax && sr {
                    break;
                }
            }
            // `send_request` failing (None / !ok) means the daemon is
            // mid-restart (re-exec) or briefly down — keep polling.
            if std::time::Instant::now() >= poll_deadline {
                break;
            }
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
        if ax && sr {
            println!("\n✅ CuaDriver has Accessibility + Screen Recording. You're set.");
        } else {
            let missing = match (ax, sr) {
                (false, false) => "Accessibility + Screen Recording",
                (false, true) => "Accessibility",
                (true, false) => "Screen Recording",
                (true, true) => unreachable!(),
            };
            println!("\n⚠️  Timed out waiting on: {missing}.");
            println!(
                "Approve CuaDriver for \u{201c}Cua Driver\u{201d} in System Settings \u{2192} \
                 Privacy & Security, then re-run `cua-driver permissions grant`."
            );
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        eprintln!("`cua-driver permissions grant` is macOS-only.");
        process::exit(1);
    }
}

/// `cua-driver check-update [--json] [--no-cache]` — pure check, never installs.
///
/// Mirror of the `check_for_update` MCP tool. Both routes call into
/// [`crate::version_check::check_update_state`] so the CLI and MCP
/// surfaces never disagree on which release is "latest".
///
/// Exit codes (mirror `brew outdated` / `npm outdated`):
///   * `0` — the check itself succeeded (regardless of `update_available`)
///   * `1` — the check failed (network down, parse error, GitHub 5xx)
///
/// We deliberately do NOT use a non-zero exit to mean "outdated" — that
/// would conflict with every shell script's "non-zero means error"
/// assumption. Hermes parses JSON; humans read text; the signal lives in
/// the payload.
pub fn run_check_update_cmd(json: bool, no_cache: bool) {
    let state = crate::version_check::check_update_state(no_cache);

    if json {
        let val = serde_json::to_value(&state).unwrap_or_else(|_| serde_json::json!({}));
        let pretty = serde_json::to_string_pretty(&val).unwrap_or_else(|_| val.to_string());
        println!("{pretty}");
    } else {
        println!("Current: {}", state.current_version);
        match (&state.latest_version, &state.error) {
            (Some(latest), _) => {
                println!("Latest:  {latest}");
                if state.update_available {
                    println!();
                    println!("Update available. Run `cua-driver update --apply` to install.");
                    if let Some(url) = &state.release_notes_url {
                        println!("Release notes: {url}");
                    }
                } else {
                    println!();
                    println!("You're on the latest release.");
                }
            }
            (None, Some(err)) => {
                println!("Latest:  <unavailable>");
                println!();
                println!("Could not reach GitHub: {err}");
            }
            (None, None) => {
                // Network failed AND no cache existed — `error` should be set;
                // fall through with a generic message in case it isn't.
                println!("Latest:  <unavailable>");
            }
        }
    }

    if state.error.is_some() && state.latest_version.is_none() {
        process::exit(1);
    }
}

fn cli_docs_json() -> serde_json::Value {
    let no_args: Vec<serde_json::Value> = Vec::new();
    let no_options: Vec<serde_json::Value> = Vec::new();
    let no_flags: Vec<serde_json::Value> = Vec::new();
    let no_subcommands: Vec<serde_json::Value> = Vec::new();

    serde_json::json!({
        "name": "cua-driver",
        "version": env!("CARGO_PKG_VERSION"),
        "abstract": "Cross-platform computer-use automation driver.",
        "commands": [
            {
                "name": "mcp",
                "abstract": "Run the stdio MCP server.",
                "discussion": "On macOS, shell-spawned MCP processes can auto-launch and proxy through a CuaDriver.app daemon so TCC grants attach to the bundle. On Windows and Linux, MCP proxies through an already-running daemon when one is listening.",
                "arguments": no_args,
                "options": [
                    {"name":"socket","short_name":null,"help":"Override the daemon socket or named-pipe path used by the proxy fallback.","type":"String","default_value":null,"is_optional":true},
                    {"name":"host-bundle-id","short_name":null,"help":"Advisory host bundle id label echoed in check_permissions output (embedded mode).","type":"String","default_value":null,"is_optional":true}
                ],
                "flags": [
                    {"name":"no-daemon-relaunch","short_name":null,"help":"Stay in-process instead of proxying through a daemon.","default_value":false},
                    {"name":"claude-code-computer-use-compat","short_name":null,"help":"Expose the Claude Code computer-use compatibility screenshot surface.","default_value":false},
                    {"name":"embedded","short_name":null,"help":"Run embedded inside a host app: inherit the host's TCC grants, never prompt or relaunch. Also CUA_DRIVER_EMBEDDED=1.","default_value":false}
                ],
                "subcommands": no_subcommands
            },
            {
                "name": "list-tools",
                "abstract": "List every registered MCP tool with a one-line description.",
                "discussion": "",
                "arguments": no_args,
                "options": no_options,
                "flags": no_flags,
                "subcommands": no_subcommands
            },
            {
                "name": "describe",
                "abstract": "Print a tool's full description and JSON input schema.",
                "discussion": "",
                "arguments": [{"name":"tool-name","help":"Name of the MCP tool to describe.","type":"String","is_optional":false}],
                "options": no_options,
                "flags": no_flags,
                "subcommands": no_subcommands
            },
            {
                "name": "call",
                "abstract": "Invoke an MCP tool directly from the shell.",
                "discussion": "Runs the same handler the MCP server uses. JSON arguments may be passed as a positional JSON object or through stdin.",
                "arguments": [
                    {"name":"tool-name","help":"Name of the MCP tool to invoke.","type":"String","is_optional":false},
                    {"name":"json-args","help":"JSON object for the tool input schema. If omitted, stdin is read when piped.","type":"String","is_optional":true}
                ],
                "options": [
                    {"name":"screenshot-out-file","short_name":null,"help":"Write the first image content block from the response to this path.","type":"String","default_value":null,"is_optional":true},
                    {"name":"socket","short_name":null,"help":"Override the daemon socket or named-pipe path.","type":"String","default_value":null,"is_optional":true}
                ],
                "flags": no_flags,
                "subcommands": no_subcommands
            },
            {
                "name": "serve",
                "abstract": "Run Cua Driver as a long-running daemon.",
                "discussion": "The daemon owns per-process state such as element-index caches, recording state, and cursor overlay state.",
                "arguments": no_args,
                "options": [
                    {"name":"socket","short_name":null,"help":"Override the daemon socket or named-pipe path.","type":"String","default_value":null,"is_optional":true},
                    {"name":"pid-file","short_name":null,"help":"Override the pid-file path on Unix targets.","type":"String","default_value":null,"is_optional":true},
                    {"name":"host-bundle-id","short_name":null,"help":"Advisory host bundle id label echoed in check_permissions output (embedded mode).","type":"String","default_value":null,"is_optional":true}
                ],
                "flags": [
                    {"name":"no-permissions-gate","short_name":null,"help":"Skip the macOS first-launch permissions gate.","default_value":false},
                    {"name":"embedded","short_name":null,"help":"Run embedded inside a host app: inherit the host's TCC grants, never prompt or relaunch. Also CUA_DRIVER_EMBEDDED=1.","default_value":false}
                ],
                "subcommands": no_subcommands
            },
            {
                "name": "stop",
                "abstract": "Ask the running daemon to exit gracefully.",
                "discussion": "",
                "arguments": no_args,
                "options": [{"name":"socket","short_name":null,"help":"Override the daemon socket or named-pipe path.","type":"String","default_value":null,"is_optional":true}],
                "flags": no_flags,
                "subcommands": no_subcommands
            },
            {
                "name": "status",
                "abstract": "Report whether a Cua Driver daemon is running.",
                "discussion": "",
                "arguments": no_args,
                "options": [
                    {"name":"socket","short_name":null,"help":"Override the daemon socket or named-pipe path.","type":"String","default_value":null,"is_optional":true},
                    {"name":"pid-file","short_name":null,"help":"Override the pid-file path on Unix targets.","type":"String","default_value":null,"is_optional":true}
                ],
                "flags": no_flags,
                "subcommands": no_subcommands
            },
            {
                "name": "mcp-config",
                "abstract": "Print MCP server config or a client-specific install command.",
                "discussion": "Supported clients include claude, codex, cursor, antigravity, openclaw, opencode, hermes, pi, qwen, droid, and zcode.",
                "arguments": no_args,
                "options": [{"name":"client","short_name":null,"help":"Client name to print configuration for.","type":"String","default_value":null,"is_optional":true}],
                "flags": no_flags,
                "subcommands": no_subcommands
            },
            {
                "name": "recording",
                "abstract": "Control trajectory recording on a running daemon.",
                "discussion": "Recording state lives in-process, so use a daemon for multi-call sessions.",
                "arguments": no_args,
                "options": [{"name":"socket","short_name":null,"help":"Override the daemon socket or named-pipe path.","type":"String","default_value":null,"is_optional":true}],
                "flags": no_flags,
                "subcommands": [
                    {
                        "name":"start",
                        "abstract":"Start trajectory recording to a directory.",
                        "discussion":"",
                        "arguments":[{"name":"output-dir","help":"Directory to write turn folders into.","type":"String","is_optional":false}],
                        "options":[],
                        "flags":[],
                        "subcommands":[]
                    },
                    {
                        "name":"stop",
                        "abstract":"Stop trajectory recording.",
                        "discussion":"",
                        "arguments":[],
                        "options":[],
                        "flags":[],
                        "subcommands":[]
                    },
                    {
                        "name":"status",
                        "abstract":"Print the current recording state.",
                        "discussion":"",
                        "arguments":[],
                        "options":[],
                        "flags":[],
                        "subcommands":[]
                    },
                    {
                        "name":"render",
                        "abstract":"Render a recorded trajectory directory to an MP4.",
                        "discussion":"This pure file-to-file path does not require a running daemon.",
                        "arguments":[
                            {"name":"input-dir","help":"Trajectory directory containing recorded turn folders.","type":"String","is_optional":false},
                            {"name":"out-mp4","help":"Output MP4 path.","type":"String","is_optional":false}
                        ],
                        "options":[{"name":"scale","short_name":null,"help":"Scale factor for rendered frames.","type":"Number","default_value":null,"is_optional":true}],
                        "flags":[{"name":"no-zoom","short_name":null,"help":"Disable cursor/action zoom effects in the rendered video.","default_value":false}],
                        "subcommands":[]
                    }
                ]
            },
            {
                "name": "config",
                "abstract": "Read or mutate persistent driver configuration.",
                "discussion": "Without a subcommand, prints the full config.",
                "arguments": no_args,
                "options": [{"name":"socket","short_name":null,"help":"Override the daemon socket or named-pipe path.","type":"String","default_value":null,"is_optional":true}],
                "flags": no_flags,
                "subcommands": [
                    {"name":"show","abstract":"Print the full config.","discussion":"","arguments":[],"options":[],"flags":[],"subcommands":[]},
                    {"name":"get","abstract":"Print one config key.","discussion":"","arguments":[{"name":"key","help":"Config key to read.","type":"String","is_optional":false}],"options":[],"flags":[],"subcommands":[]},
                    {"name":"set","abstract":"Set one config key.","discussion":"","arguments":[{"name":"key","help":"Config key to write.","type":"String","is_optional":false},{"name":"value","help":"Value to store.","type":"String","is_optional":false}],"options":[],"flags":[],"subcommands":[]},
                    {"name":"reset","abstract":"Reset config to defaults.","discussion":"","arguments":[],"options":[],"flags":[],"subcommands":[]}
                ]
            },
            {
                "name": "telemetry",
                "abstract": "Inspect or change content-free product telemetry.",
                "discussion": "Telemetry is default-on. Disable retains the pseudonymous installation ID; reset-id erases the ID and event markers while preserving the preference.",
                "arguments": no_args,
                "options": no_options,
                "flags": no_flags,
                "subcommands": [
                    {"name":"enable","abstract":"Persistently enable telemetry.","discussion":"","arguments":[],"options":[],"flags":[],"subcommands":[]},
                    {"name":"disable","abstract":"Persistently disable every telemetry request.","discussion":"Retains the local installation ID.","arguments":[],"options":[],"flags":[],"subcommands":[]},
                    {"name":"status","abstract":"Show the effective setting and redacted identity state.","discussion":"","arguments":[],"options":[],"flags":[{"name":"json","short_name":null,"help":"Emit JSON.","default_value":false}],"subcommands":[]},
                    {"name":"reset-id","abstract":"Erase the installation ID and event markers.","discussion":"The persisted enabled/disabled preference is retained.","arguments":[],"options":[],"flags":[],"subcommands":[]},
                    {"name":"inspect","abstract":"Build a fixed event payload without sending it.","discussion":"The distinct ID is replaced with a redacted placeholder.","arguments":[{"name":"event","help":"Fixed telemetry event name.","type":"String","is_optional":false}],"options":[],"flags":[{"name":"json","short_name":null,"help":"Emit JSON.","default_value":true}],"subcommands":[]}
                ]
            },
            {
                "name": "check-update",
                "abstract": "Check whether a newer cua-driver release is available.",
                "discussion": "Read-only. Uses the same update-state payload as the check_for_update MCP tool.",
                "arguments": no_args,
                "options": no_options,
                "flags": [
                    {"name":"json","short_name":null,"help":"Emit a machine-readable JSON payload.","default_value":false},
                    {"name":"no-cache","short_name":null,"help":"Skip the 20-hour on-disk cache and force a GitHub request.","default_value":false}
                ],
                "subcommands": no_subcommands
            },
            {
                "name": "update",
                "abstract": "Check for an update and optionally apply it.",
                "discussion": "The apply path delegates to the canonical platform installer scripts.",
                "arguments": no_args,
                "options": no_options,
                "flags": [
                    {"name":"apply","short_name":null,"help":"Download and install the latest release when one is available.","default_value":false},
                    {"name":"json","short_name":null,"help":"Emit the structured update-state payload.","default_value":false}
                ],
                "subcommands": no_subcommands
            },
            {
                "name": "doctor",
                "abstract": "Run platform-aware diagnostic probes.",
                "discussion": "Exit code is non-zero when any probe is an error.",
                "arguments": no_args,
                "options": no_options,
                "flags": [{"name":"json","short_name":null,"help":"Emit the probe report as JSON.","default_value":false}],
                "subcommands": no_subcommands
            },
            {
                "name": "diagnose",
                "abstract": "Print a pasteable install-layout and permission-attribution report.",
                "discussion": "",
                "arguments": no_args,
                "options": no_options,
                "flags": no_flags,
                "subcommands": no_subcommands
            },
            {
                "name": "autostart",
                "abstract": "Manage platform-native daemon autostart.",
                "discussion": "Windows registers a logon Scheduled Task. macOS and Linux currently print manual-recipe guidance.",
                "arguments": no_args,
                "options": no_options,
                "flags": no_flags,
                "subcommands": [
                    {"name":"enable","abstract":"Register the autostart entry.","discussion":"","arguments":[],"options":[],"flags":[],"subcommands":[]},
                    {"name":"disable","abstract":"Remove the autostart entry.","discussion":"","arguments":[],"options":[],"flags":[],"subcommands":[]},
                    {"name":"status","abstract":"Print whether autostart is registered and running.","discussion":"","arguments":[],"options":[],"flags":[],"subcommands":[]},
                    {"name":"kick","abstract":"Start the autostart entry now without re-logging.","discussion":"","arguments":[],"options":[],"flags":[],"subcommands":[]}
                ]
            },
            {
                "name": "skills",
                "abstract": "Install, update, inspect, or remove the optional agent skill pack.",
                "discussion": "The install script never touches agent skill directories automatically.",
                "arguments": no_args,
                "options": no_options,
                "flags": no_flags,
                "subcommands": [
                    {"name":"install","abstract":"Fetch the versioned skill pack and link detected agents.","discussion":"","arguments":[],"options":[{"name":"agent","short_name":null,"help":"Restrict linking to one agent.","type":"String","default_value":null,"is_optional":true},{"name":"from","short_name":null,"help":"Fetch from a source such as main instead of the tagged release.","type":"String","default_value":null,"is_optional":true}],"flags":[{"name":"all-platforms","short_name":null,"help":"Keep platform-specific skill files for every platform.","default_value":false}],"subcommands":[]},
                    {"name":"update","abstract":"Refresh the local skill pack and links.","discussion":"","arguments":[],"options":[],"flags":[],"subcommands":[]},
                    {"name":"uninstall","abstract":"Remove agent skill links.","discussion":"","arguments":[],"options":[],"flags":[{"name":"all","short_name":null,"help":"Also delete the local skill-pack copy.","default_value":false}],"subcommands":[]},
                    {"name":"status","abstract":"Report local skill-pack and per-agent link state.","discussion":"","arguments":[],"options":[],"flags":[],"subcommands":[]},
                    {"name":"path","abstract":"Print the local skill-pack path.","discussion":"","arguments":[],"options":[],"flags":[],"subcommands":[]}
                ]
            },
            {
                "name": "dump-docs",
                "abstract": "Output machine-readable CLI and MCP documentation JSON.",
                "discussion": "Used by the docs generator to keep reference pages in sync with the live binary.",
                "arguments": no_args,
                "options": [{"name":"type","short_name":null,"help":"Which docs to emit: all, cli, or mcp.","type":"String","default_value":"all","is_optional":true}],
                "flags": [{"name":"pretty","short_name":"p","help":"Pretty-print JSON.","default_value":false}],
                "subcommands": no_subcommands
            }
        ]
    })
}

/// Output documentation as JSON.  `doc_type` is one of:
/// - `"mcp"` — only MCP tool docs (`{version, tools: [...]}`)
/// - `"cli"` — CLI docs
/// - `"all"` — `{cli, mcp}` matching Swift `CombinedDocs`
pub fn run_dump_docs_with_type(registry: &ToolRegistry, pretty: bool, doc_type: &str) {
    // Each MCP tool: `{name, description, input_schema}` (Swift's MCPToolDoc
    // shape — Rust adds read_only/destructive/idempotent as intentional
    // extras).
    let tools: Vec<serde_json::Value> = registry
        .iter_defs()
        .map(|(_, def)| {
            serde_json::json!({
                "name":         def.name,
                "description":  def.description,
                "input_schema": def.input_schema,
                "read_only":    def.read_only,
                "destructive":  def.destructive,
                "idempotent":   def.idempotent,
            })
        })
        .collect();
    let mcp = serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "tools":   tools,
    });

    let cli_docs = cli_docs_json();

    let doc = match doc_type {
        "cli" => cli_docs,
        "mcp" => mcp,
        _ => serde_json::json!({ "cli": cli_docs, "mcp": mcp }),
    };
    let out = if pretty {
        serde_json::to_string_pretty(&doc)
    } else {
        serde_json::to_string(&doc)
    };
    println!("{}", out.unwrap_or_else(|_| "{}".into()));
}

/// `cua-driver diagnose` — print a paste-able bundle-path / install-layout / TCC report.
///
/// Mirrors Swift `DiagnoseCommand`. Covers:
///   - running process identity (path, pid, version)
///   - codesign info (cdhash, team-id, authority) via `codesign -dvvv`
///   - AX + screen recording TCC status (check_permissions tool)
///   - install layout (/Applications/CuaDriver.app, ~/.local/bin/cua-driver)
///   - TCC DB rows for com.trycua.driver (sqlite3, best-effort)
///   - config + state paths with existence booleans
pub fn run_diagnose_cmd(registry: std::sync::Arc<ToolRegistry>) {
    let sections = [
        diagnose_runtime_section(),
        diagnose_signature_section(),
        diagnose_tcc_section(registry),
        diagnose_install_layout_section(),
        diagnose_tcc_db_section(),
        diagnose_config_paths_section(),
    ];
    println!("{}", sections.join("\n\n"));
}

fn diagnose_runtime_section() -> String {
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(str::to_owned))
        .unwrap_or_else(|| "<unknown>".into());
    let argv0 = std::env::args()
        .next()
        .unwrap_or_else(|| "<unknown>".into());
    let pid = std::process::id();
    let version = env!("CARGO_PKG_VERSION");
    format!(
        "## running process\n\
         version:        {version}\n\
         argv[0]:        {argv0}\n\
         executablePath: {exe}\n\
         pid:            {pid}"
    )
}

fn diagnose_signature_section() -> String {
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(str::to_owned))
        .unwrap_or_default();

    let out = std::process::Command::new("codesign")
        .args(["-dvvv", &exe])
        .output();

    let mut cdhash = "<unknown>".to_owned();
    let mut team_id = "<none — ad-hoc signed?>".to_owned();
    let mut authority = "<none — ad-hoc signed?>".to_owned();

    if let Ok(out) = out {
        // codesign prints to stderr
        let text = String::from_utf8_lossy(&out.stderr);
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("CDHash=") {
                cdhash = rest.trim().to_owned();
            } else if let Some(rest) = line.strip_prefix("TeamIdentifier=") {
                team_id = rest.trim().to_owned();
            } else if let Some(rest) = line.strip_prefix("Authority=") {
                // Only take the first (leaf certificate) authority line.
                if authority == "<none — ad-hoc signed?>" {
                    authority = rest.trim().to_owned();
                }
            }
        }
    }

    format!(
        "## running process signature\n\
         cdhash:    {cdhash}\n\
         teamID:    {team_id}\n\
         authority: {authority}"
    )
}

fn diagnose_tcc_section(registry: std::sync::Arc<ToolRegistry>) -> String {
    // Call check_permissions in-process (quick, read-only).
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build();

    let (ax, sr) = if let Ok(rt) = rt {
        rt.block_on(async {
            let result = registry
                .invoke(
                    "check_permissions",
                    serde_json::Value::Object(Default::default()),
                )
                .await;
            if let Some(sc) = &result.structured_content {
                let ax = sc
                    .get("accessibility")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let sr = sc
                    .get("screen_recording")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                (ax, sr)
            } else {
                (false, false)
            }
        })
    } else {
        (false, false)
    };

    format!(
        "## tcc probes (live process)\n\
         accessibility     (AXIsProcessTrusted): {ax}\n\
         screen recording  (SCShareableContent):  {sr}\n\n\
         if the UI disagrees with these booleans the live process is fine —\n\
         the issue is elsewhere (wrong bundle granted, stale cdhash, etc)."
    )
}

fn diagnose_install_layout_section() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let mut lines = vec!["## install layout".to_owned()];

    let app_path = "/Applications/CuaDriver.app";
    let app_exists = std::path::Path::new(app_path).exists();
    lines.push(format!("bundle:  {app_path}   exists={app_exists}"));
    if app_exists {
        // Codesign info for the bundle.
        if let Ok(out) = std::process::Command::new("codesign")
            .args(["-dvvv", app_path])
            .output()
        {
            let text = String::from_utf8_lossy(&out.stderr);
            let mut cdhash = "<unknown>";
            let mut team_id = "<none>";
            let mut authority = "<none>";
            for line in text.lines() {
                if let Some(r) = line.strip_prefix("CDHash=") {
                    cdhash = r.trim();
                } else if let Some(r) = line.strip_prefix("TeamIdentifier=") {
                    team_id = r.trim();
                } else if line.starts_with("Authority=") && authority == "<none>" {
                    authority = line.trim_start_matches("Authority=").trim();
                }
            }
            lines.push(format!("  cdhash:    {cdhash}"));
            lines.push(format!("  teamID:    {team_id}"));
            lines.push(format!("  authority: {authority}"));
        }
    }

    let cli_paths = [
        ("symlink", format!("{home}/.local/bin/cua-driver")),
        ("legacy symlink", "/usr/local/bin/cua-driver".to_owned()),
    ];
    for (label, path) in &cli_paths {
        let exists = std::path::Path::new(path).exists();
        lines.push(format!("{label}: {path}   exists={exists}"));
        if exists {
            if let Ok(target) = std::fs::read_link(path) {
                lines.push(format!("  resolves to: {}", target.display()));
            }
        }
    }

    let stale = format!("{home}/Applications/CuaDriver.app");
    if std::path::Path::new(&stale).exists() {
        lines.push(format!(
            "stale:   {stale}   \u{2190} old install-local.sh path, consider removing"
        ));
    }

    lines.join("\n")
}

fn diagnose_tcc_db_section() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let db = format!("{home}/Library/Application Support/com.apple.TCC/TCC.db");
    let sql = "SELECT service, client, client_type, auth_value, auth_reason, \
               hex(csreq) AS csreq_hex FROM access WHERE client='com.trycua.driver';";

    let mut lines = vec!["## tcc database rows for com.trycua.driver".to_owned()];
    lines.push(format!(
        "(reading {db} — best-effort; system TCC DB requires FDA)"
    ));
    lines.push(String::new());

    match std::process::Command::new("sqlite3")
        .args(["-header", "-column", &db, sql])
        .output()
    {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            let trimmed = stdout.trim();
            if !trimmed.is_empty() {
                lines.push(trimmed.to_owned());
            } else if out.status.code() != Some(0) && !stderr.trim().is_empty() {
                lines.push(format!("(sqlite3 failed: {})", stderr.trim()));
            } else {
                lines.push("(no rows — either grants never made it to TCC, or they live".into());
                lines.push(" in the system DB which requires Full Disk Access to read.)".into());
            }
        }
        Err(e) => {
            lines.push(format!("(could not launch sqlite3: {e})"));
        }
    }

    lines.push(String::new());
    lines.push("# auth_value legend: 0=denied  2=allowed".into());
    lines.push("# services: kTCCServiceAccessibility, kTCCServiceScreenCapture".into());
    lines.join("\n")
}

fn diagnose_config_paths_section() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let paths: &[(&str, String)] = &[
        ("user data dir", format!("{home}/.cua-driver")),
        ("config cache", format!("{home}/Library/Caches/cua-driver")),
        ("telemetry id", format!("{home}/.cua-driver/.telemetry_id")),
        (
            "updater plist",
            format!("{home}/Library/LaunchAgents/com.trycua.cua_driver_updater.plist"),
        ),
        (
            "daemon plist",
            format!("{home}/Library/LaunchAgents/com.trycua.cua_driver_daemon.plist"),
        ),
    ];
    let mut lines = vec!["## config + state paths".to_owned()];
    for (label, path) in paths {
        let exists = std::path::Path::new(path).exists();
        lines.push(format!("{:<18} exists={exists}   {path}", label));
    }
    lines.join("\n")
}

/// Path to the persistent JSON config file (`~/.cua-driver/config.json`).
fn config_file_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    std::path::PathBuf::from(format!("{home}/.cua-driver/config.json"))
}

/// Read persisted config from disk.  Returns an empty object if absent/unreadable.
fn read_config_file() -> serde_json::Value {
    let path = config_file_path();
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::json!({}))
}

/// Write a single key/value into the persisted config file.
fn write_config_file(key: &str, value: &serde_json::Value) {
    let path = config_file_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut cfg = read_config_file();
    if let serde_json::Value::Object(ref mut map) = cfg {
        map.insert(key.to_owned(), value.clone());
    }
    if let Ok(json) = serde_json::to_string_pretty(&cfg) {
        let _ = std::fs::write(&path, json);
    }
}

/// `cua-driver config [show|get|set|reset] [key] [value]`
///
/// Thin wrapper around the `get_config` / `set_config` MCP tools.
/// Forwards to the daemon when one is reachable, otherwise runs in-process.
pub fn run_config_cmd(
    registry: std::sync::Arc<ToolRegistry>,
    subcommand: Option<&str>,
    key: Option<&str>,
    value: Option<&str>,
    socket: Option<&str>,
) {
    let socket_path = socket
        .map(str::to_owned)
        .unwrap_or_else(crate::serve::default_socket_path);

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    match subcommand.unwrap_or("show") {
        "show" | "" => {
            // Print full config as pretty JSON.
            let config =
                rt.block_on(async { registry.invoke("get_config", serde_json::json!({})).await });
            if let Some(sc) = config.structured_content {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&sc).unwrap_or_else(|_| sc.to_string())
                );
            } else {
                eprintln!("get_config: no structured content returned");
                process::exit(1);
            }
        }

        "get" => {
            let key = match key {
                Some(k) => k,
                None => {
                    eprintln!("Usage: cua-driver config get <key>");
                    eprintln!("Keys: capture_mode, max_image_dimension, version, platform");
                    process::exit(64);
                }
            };
            if key == "capture_scope" {
                eprintln!(
                    "config key 'capture_scope' is retired; use start_session(capture_scope=auto|window|desktop)"
                );
                process::exit(64);
            }
            // Try daemon first.
            if crate::serve::is_daemon_listening(&socket_path) {
                let req = crate::serve::DaemonRequest {
                    method: "call".into(),
                    name: Some("get_config".into()),
                    args: Some(serde_json::json!({})),
                    // CLI `config get` reads the persisted global (anonymous).
                    session_id: None,
                    observation_origin: Some(crate::serve::ToolObservationOrigin::Direct),
                };
                if let Ok(resp) = crate::serve::send_request(&socket_path, &req) {
                    if resp.ok {
                        if let Some(result) = resp.result {
                            if let Some(sc) = result.get("structuredContent") {
                                if let Some(v) = sc.get(key) {
                                    println!("{v}");
                                    return;
                                }
                            }
                        }
                    }
                }
            }
            // In-process: merge persisted file config over in-memory defaults.
            let config =
                rt.block_on(async { registry.invoke("get_config", serde_json::json!({})).await });
            let mut sc = match config.structured_content {
                Some(v) => v,
                None => {
                    eprintln!("get_config: no structured content returned");
                    process::exit(1);
                }
            };
            // Overlay persisted values from the config file.
            let file_cfg = read_config_file();
            if let (serde_json::Value::Object(sc_map), serde_json::Value::Object(file_map)) =
                (&mut sc, file_cfg)
            {
                for (k, v) in file_map {
                    if k != "capture_scope" {
                        sc_map.insert(k, v);
                    }
                }
            }
            // Support dotted key paths like "agent_cursor.enabled".
            let v = if key.contains('.') {
                let mut parts = key.splitn(2, '.');
                let parent = parts.next().unwrap();
                let child = parts.next().unwrap();
                sc.get(parent).and_then(|obj| obj.get(child)).cloned()
            } else {
                sc.get(key).cloned()
            };
            if let Some(v) = v {
                println!(
                    "{}",
                    match &v {
                        serde_json::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    }
                );
            } else {
                eprintln!("Unknown config key: {key}");
                eprintln!("Available keys: capture_mode, max_image_dimension, version, platform, agent_cursor.enabled");
                process::exit(64);
            }
        }

        "set" => {
            let key = match key {
                Some(k) => k,
                None => {
                    eprintln!("Usage: cua-driver config set <key> <value>");
                    process::exit(64);
                }
            };
            if key == "capture_scope" {
                eprintln!(
                    "config key 'capture_scope' is retired; use start_session(capture_scope=auto|window|desktop)"
                );
                process::exit(64);
            }
            let value = match value {
                Some(v) => v,
                None => {
                    eprintln!("Usage: cua-driver config set {key} <value>");
                    process::exit(64);
                }
            };
            // Parse value: try JSON, fall back to string.
            let parsed_value: serde_json::Value = serde_json::from_str(value)
                .unwrap_or_else(|_| serde_json::Value::String(value.to_owned()));
            let args = serde_json::json!({ key: parsed_value });

            // Try daemon first.
            if crate::serve::is_daemon_listening(&socket_path) {
                let req = crate::serve::DaemonRequest {
                    method: "call".into(),
                    name: Some("set_config".into()),
                    args: Some(args.clone()),
                    // CLI `config set` is anonymous → writes the persisted
                    // global default (the only writer of the on-disk config).
                    session_id: None,
                    observation_origin: Some(crate::serve::ToolObservationOrigin::Direct),
                };
                if let Ok(resp) = crate::serve::send_request(&socket_path, &req) {
                    if resp.ok {
                        println!("Config updated.");
                        // Show current state.
                        let req2 = crate::serve::DaemonRequest {
                            method: "call".into(),
                            name: Some("get_config".into()),
                            args: Some(serde_json::json!({})),
                            session_id: None,
                            observation_origin: Some(crate::serve::ToolObservationOrigin::Direct),
                        };
                        if let Ok(r2) = crate::serve::send_request(&socket_path, &req2) {
                            if let Some(result) = r2.result {
                                if let Some(sc) = result.get("structuredContent") {
                                    println!(
                                        "{}",
                                        serde_json::to_string_pretty(sc).unwrap_or_default()
                                    );
                                }
                            }
                        }
                        return;
                    } else if let Some(e) = resp.error {
                        eprintln!("{e}");
                        process::exit(1);
                    }
                }
            }
            // In-process.
            let result = rt.block_on(async { registry.invoke("set_config", args).await });
            if result.is_error.unwrap_or(false) {
                for item in &result.content {
                    if let cua_driver_core::protocol::Content::Text { text, .. } = item {
                        eprintln!("{text}");
                    }
                }
                process::exit(1);
            }
            // Persist the value to disk so future CLI invocations can read it.
            write_config_file(key, &parsed_value);
            // Print updated config.
            let config =
                rt.block_on(async { registry.invoke("get_config", serde_json::json!({})).await });
            if let Some(sc) = config.structured_content {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&sc).unwrap_or_else(|_| sc.to_string())
                );
            }
        }

        "reset" => {
            // Reset to defaults by calling set_config with empty args.
            // The set_config tool keeps existing values if keys are absent,
            // so we send the known defaults explicitly.
            let defaults = serde_json::json!({
                "capture_mode": "ax",
                "max_image_dimension": 0
            });
            let result = rt.block_on(async { registry.invoke("set_config", defaults).await });
            if result.is_error.unwrap_or(false) {
                eprintln!("config reset failed");
                process::exit(1);
            }
            println!("Config reset to defaults.");
            let config =
                rt.block_on(async { registry.invoke("get_config", serde_json::json!({})).await });
            if let Some(sc) = config.structured_content {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&sc).unwrap_or_else(|_| sc.to_string())
                );
            }
        }

        other => {
            eprintln!("Unknown config subcommand '{other}'. Valid: show, get <key>, set <key> <value>, reset");
            process::exit(64);
        }
    }
}

/// `cua-driver doctor` — run platform-aware diagnostic probes and emit a
/// structured report.
///
/// See [`crate::doctor`] for the probe catalog. Output is plain text by
/// default; `--json` switches to a machine-readable shape for scripting.
/// Exit code is `0` when every probe is `[ok]` or `[warn]`, non-zero when
/// at least one `[err]` probe failed (e.g. the binary cannot resolve its
/// own install dir).
pub fn run_doctor_cmd(json: bool) {
    let report = crate::doctor::run();

    if json {
        let val = report.to_json();
        let out = serde_json::to_string_pretty(&val).unwrap_or_else(|_| val.to_string());
        println!("{out}");
    } else {
        print!("{}", report.to_text());
    }

    if report.has_errors() {
        process::exit(1);
    }
}

// ── helpers ──────────────────────────────────────────────────────────────────

/// Read JSON from stdin when stdin is a pipe (non-interactive). Returns `None`
/// when stdin is a terminal or the input isn't valid JSON.
/// Matches Swift's "If omitted, reads from stdin when stdin is a pipe."
///
/// Strips a leading UTF-8 BOM (`U+FEFF`, bytes `EF BB BF`) before parsing.
/// Without this, payloads written via PowerShell 5.1's
/// `Set-Content -Encoding utf8` (which silently prepends a BOM) parse as
/// invalid JSON and the call falls through to default-args, producing
/// confusing "Missing required integer field" errors despite the caller
/// having sent a valid-looking payload. See the 2026-05-23 dogfood journal.
fn read_stdin_json() -> Option<serde_json::Value> {
    use std::io::{self, IsTerminal, Read};
    let stdin = io::stdin();
    if stdin.is_terminal() {
        return None;
    }
    let mut buf = String::new();
    stdin.lock().read_to_string(&mut buf).ok()?;
    let trimmed = buf.trim();
    // U+FEFF is one character (3 bytes UTF-8) — `str::strip_prefix` matches by
    // chars, so a single `'\u{feff}'` is the right comparand.
    let stripped = trimmed.strip_prefix('\u{feff}').unwrap_or(trimmed);
    serde_json::from_str(stripped).ok()
}

#[cfg(test)]
mod stdin_bom_tests {
    /// Manual cross-check that the BOM-stripping logic round-trips correctly
    /// without needing a real stdin pipe.
    #[test]
    fn strip_prefix_handles_utf8_bom() {
        let with_bom = "\u{feff}{\"pid\":42}";
        let stripped = with_bom.strip_prefix('\u{feff}').unwrap_or(with_bom);
        assert_eq!(stripped, "{\"pid\":42}");
        let v: serde_json::Value = serde_json::from_str(stripped).unwrap();
        assert_eq!(v["pid"], 42);
    }

    #[test]
    fn strip_prefix_no_op_when_no_bom() {
        let plain = "{\"pid\":7}";
        let stripped = plain.strip_prefix('\u{feff}').unwrap_or(plain);
        assert_eq!(stripped, plain);
    }
}

/// Normalise a user-provided tool name into a safe PostHog event suffix.
///
/// Tool names are concatenated onto `cua_driver_api_` to build per-tool
/// telemetry event names. The raw string is user-controlled (any CLI
/// arg or MCP request can specify it), so we:
///
/// 1. ASCII-lowercase
/// 2. Keep only `[a-z0-9_]` — drop punctuation, slashes, dots, anything else
/// 3. Truncate to 64 chars (event names are a dashboard axis, not free text)
/// 4. Fall back to `"unknown"` when the result is empty (e.g. all non-ASCII
///    input), so we still record *that* a call happened without inventing
///    a per-payload event name.
#[cfg(test)]
fn sanitize_tool_name(name: &str) -> String {
    const MAX_LEN: usize = 64;
    const FALLBACK: &str = "unknown";

    let cleaned: String = name
        .chars()
        .filter_map(|c| {
            let lc = c.to_ascii_lowercase();
            if lc.is_ascii_alphanumeric() || lc == '_' {
                Some(lc)
            } else {
                None
            }
        })
        .take(MAX_LEN)
        .collect();

    if cleaned.is_empty() {
        FALLBACK.to_owned()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn finite_call_tool_extraction_supports_subcommand_and_legacy_forms() {
        assert_eq!(
            finite_tool_name_from_args(&args(&["call", "click", r#"{\"x\":1}"#])),
            Some("click".into())
        );
        assert_eq!(
            finite_tool_name_from_args(&args(&["--socket", "/tmp/test", "click", "{}"])),
            Some("click".into())
        );
    }

    #[test]
    fn finite_call_tool_extraction_ignores_non_call_commands() {
        assert_eq!(
            finite_tool_name_from_args(&args(&["describe", "click"])),
            None
        );
        assert_eq!(finite_tool_name_from_args(&args(&["mcp"])), None);
    }

    #[test]
    fn finite_computer_action_discards_arguments_after_fixed_classification() {
        assert!(finite_computer_action_from_args(&args(&[
            "call",
            "click",
            r#"{"x":1,"private":"discarded"}"#,
        ])));
        assert!(finite_computer_action_from_args(&args(&[
            "call",
            "page",
            r#"{"action":"insert_text","text":"private"}"#,
        ])));
        assert!(!finite_computer_action_from_args(&args(&[
            "call",
            "page",
            r#"{"action":"query_dom","selector":"private"}"#,
        ])));
        assert!(!finite_computer_action_from_args(&args(&[
            "call", "page", "not-json",
        ])));
    }

    #[test]
    fn finite_operations_are_closed_and_ignore_values() {
        assert_eq!(
            finite_operation_from_args(&args(&["recording", "start", "/private/path"])),
            "start"
        );
        assert_eq!(
            finite_operation_from_args(&args(&["config", "set", "private.key", "private-value"])),
            "set"
        );
        assert_eq!(finite_operation_from_args(&args(&["skills"])), "status");
        assert_eq!(
            finite_operation_from_args(&args(&["update", "--apply"])),
            "apply"
        );
        assert_eq!(finite_operation_from_args(&args(&["update"])), "check_only");
        assert_eq!(
            finite_operation_from_args(&args(&["doctor", "private-value"])),
            "not_applicable"
        );
        assert_eq!(
            finite_operation_from_args(&args(&["recording", "private-value"])),
            "other"
        );
    }

    #[test]
    fn finite_mcp_config_clients_are_closed_before_worker_handoff() {
        assert_eq!(
            finite_client_kind_from_args(&args(&["mcp-config"])),
            "generic"
        );
        assert_eq!(
            finite_client_kind_from_args(&args(&["mcp-config", "--client", "claude-code"])),
            "claude_code"
        );
        assert_eq!(
            finite_client_kind_from_args(&args(&["mcp-config", "--client", "antigravity"])),
            "antigravity"
        );
        assert_eq!(
            finite_client_kind_from_args(&args(&["mcp-config", "--client", "/private/client"])),
            "other"
        );
        assert_eq!(
            finite_client_kind_from_args(&args(&["doctor", "--client", "claude"])),
            "not_applicable"
        );
    }

    #[test]
    fn sanitize_tool_name_passes_through_canonical_names() {
        assert_eq!(sanitize_tool_name("click"), "click");
        assert_eq!(sanitize_tool_name("move_mouse"), "move_mouse");
        assert_eq!(sanitize_tool_name("ScrollUp"), "scrollup");
    }

    #[test]
    fn sanitize_tool_name_strips_punctuation_and_path_separators() {
        // Path-like input would otherwise leak directory names into event
        // names — strip everything that's not [a-z0-9_].
        assert_eq!(sanitize_tool_name("foo.bar/baz"), "foobarbaz");
        assert_eq!(sanitize_tool_name("../etc/passwd"), "etcpasswd");
        assert_eq!(sanitize_tool_name("click-element!"), "clickelement");
    }

    #[test]
    fn sanitize_tool_name_falls_back_when_non_ascii() {
        // Non-ASCII characters are dropped entirely — without a fallback
        // we'd emit `cua_driver_api_` (empty suffix), which collides with
        // the bare `cua_driver_call` event.
        assert_eq!(sanitize_tool_name("クリック"), "unknown");
        assert_eq!(sanitize_tool_name("🚀"), "unknown");
    }

    #[test]
    fn sanitize_tool_name_falls_back_on_empty_or_all_stripped() {
        assert_eq!(sanitize_tool_name(""), "unknown");
        assert_eq!(sanitize_tool_name("---"), "unknown");
        assert_eq!(sanitize_tool_name("///"), "unknown");
    }

    #[test]
    fn sanitize_tool_name_caps_length_at_64() {
        let long_name = "a".repeat(200);
        let sanitized = sanitize_tool_name(&long_name);
        assert_eq!(sanitized.len(), 64);
        assert!(sanitized.chars().all(|c| c == 'a'));
    }

    // ── Surface 8: manifest shape ───────────────────────────────────────────

    /// The manifest must carry the four documented top-level keys so a
    /// consumer can branch on `schema_version` and read the canonical
    /// MCP invocation without sniffing argv defaults.
    #[test]
    fn manifest_has_documented_top_level_shape() {
        let m = build_manifest();
        let obj = m.as_object().expect("manifest is an object");

        // schema_version — stable string; consumers branch on this.
        assert_eq!(
            obj.get("schema_version").and_then(|v| v.as_str()),
            Some("1")
        );

        // binary_version — must equal CARGO_PKG_VERSION (current build).
        let bv = obj
            .get("binary_version")
            .and_then(|v| v.as_str())
            .expect("binary_version present and a string");
        assert_eq!(bv, env!("CARGO_PKG_VERSION"));

        // mcp_invocation — { command: <bin path>, args: ["mcp"] }
        let inv = obj
            .get("mcp_invocation")
            .and_then(|v| v.as_object())
            .expect("mcp_invocation is an object");
        assert!(
            inv.get("command").and_then(|v| v.as_str()).is_some(),
            "mcp_invocation.command must be a string"
        );
        let args = inv
            .get("args")
            .and_then(|v| v.as_array())
            .expect("mcp_invocation.args is an array");
        assert_eq!(args.len(), 1);
        assert_eq!(args[0].as_str(), Some("mcp"));

        // subcommands — non-empty array with the canonical entries.
        let subs = obj
            .get("subcommands")
            .and_then(|v| v.as_array())
            .expect("subcommands is an array");
        let names: Vec<&str> = subs
            .iter()
            .filter_map(|s| s.get("name").and_then(|v| v.as_str()))
            .collect();
        for need in [
            "mcp",
            "list-tools",
            "describe",
            "call",
            "serve",
            "stop",
            "status",
            "mcp-config",
            "manifest",
        ] {
            assert!(names.contains(&need), "missing subcommand '{need}'");
        }
    }

    /// Every subcommand entry has the same JSON shape — name + description
    /// + args[] — so consumers can render the catalog uniformly without
    /// per-subcommand branching.
    #[test]
    fn manifest_subcommands_have_uniform_shape() {
        let m = build_manifest();
        let subs = m
            .get("subcommands")
            .and_then(|v| v.as_array())
            .expect("subcommands");
        for entry in subs {
            let obj = entry.as_object().expect("each subcommand is an object");
            assert!(
                obj.get("name").and_then(|v| v.as_str()).is_some(),
                "subcommand missing name: {entry}"
            );
            assert!(
                obj.get("description").and_then(|v| v.as_str()).is_some(),
                "subcommand missing description: {entry}"
            );
            assert!(
                obj.get("args").and_then(|v| v.as_array()).is_some(),
                "subcommand missing args[]: {entry}"
            );
        }
    }

    /// Hermes / Codex / Claude Code can read `mcp_invocation` and drop
    /// their hardcoded `["mcp"]` defaults. The invocation must point at
    /// an executable path, and the `args` array MUST be `["mcp"]` — no
    /// `--something` flag drift, no rename, no removal.
    #[test]
    fn manifest_mcp_invocation_is_stable() {
        let m = build_manifest();
        let inv = m.get("mcp_invocation").expect("mcp_invocation");
        let args: Vec<&str> = inv
            .get("args")
            .and_then(|v| v.as_array())
            .expect("args[] array")
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert_eq!(args, vec!["mcp"]);
    }
}

fn first_sentence(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let flat: String = trimmed
        .splitn(3, "\n\n")
        .next()
        .unwrap_or(trimmed)
        .split('\n')
        .collect::<Vec<_>>()
        .join(" ");
    let mut sentence = String::new();
    let mut prev = ' ';
    for ch in flat.chars() {
        if (prev == '.' || prev == '?' || prev == '!') && ch == ' ' {
            break;
        }
        sentence.push(ch);
        prev = ch;
    }
    let mut s = sentence.trim().to_string();
    if s.ends_with('.') {
        s.pop();
    }
    s
}
