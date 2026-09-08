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
//! Cursor-overlay flags (--cursor-theme, --no-overlay, etc.) are consumed by
//! `CursorConfig::from_args()` and are ignored here.

use std::process;

/// Which CLI command was requested.
pub enum Command {
    Mcp {
        /// Select an explicit daemon socket/pipe path. Without one, Windows
        /// and Linux own a direct runtime while macOS uses the default app
        /// daemon to preserve TCC attribution.
        socket: Option<String>,
        /// Own the runtime in this MCP process. On macOS this is an explicit
        /// TCC-attribution choice; it is mutually exclusive with `--socket`.
        direct: bool,
        /// `--claude-code-computer-use-compat`: register the compat
        /// `screenshot` tool (window-scoped, JPEG @ 85%, pid + window_id
        /// both required) instead of the full-featured one. Used when
        /// the MCP server is wired up as `cua-computer-use` in Claude
        /// Code, where this is the documented best-practice install.
        claude_code_compat: bool,
        /// Repeatable trusted launch grants for residual standard-mode
        /// boundaries, for example `--grant existing-profile`.
        grants: Vec<String>,
    },
    ListTools,
    Describe(String),
    Call {
        tool: String,
        json_args: Option<serde_json::Value>,
        screenshot_out_file: Option<String>,
        /// Override the required daemon socket/pipe path (matches `--socket`
        /// semantics for `serve` / `status` / `stop`). Defaults to
        /// `serve::default_socket_path()` when None.
        socket: Option<String>,
    },
    McpConfig {
        client: Option<String>,
    },
    Serve {
        socket: Option<String>,
        /// Immutable agent-authorization mode selected at trusted daemon
        /// startup. This is distinct from the macOS OS-permissions gate.
        permission_mode: Option<String>,
        /// Deliberate unrestricted-mode selector and risk acknowledgement.
        dangerously_bypass_approvals: bool,
        /// Immutable narrow-only capability manifest selected by the trusted
        /// launcher. Required in bounded mode and optional in other profiles.
        capability_manifest: Option<String>,
        /// Deliberate launch-time confirmation that the manifest was reviewed.
        approve_capability_manifest: bool,
        /// True when `--no-permissions-gate` is on argv.  The env-var
        /// `CUA_DRIVER_RS_PERMISSIONS_GATE=0` short-circuits the gate too
        /// (checked inside the gate itself), so the flag is only one of
        /// two opt-out signals.
        no_permissions_gate: bool,
        /// True when `--claude-code-computer-use-compat` is on argv. The MCP
        /// proxy forwards this flag to a daemon it auto-launches (see
        /// `launch_daemon_and_wait`) so that daemon registers the requested
        /// compatibility surface.
        claude_code_compat: bool,
        /// Repeatable trusted launch grants.
        grants: Vec<String>,
        /// Admit the encrypted local Computer History early preview for this
        /// daemon generation. Capture still requires separate persisted opt-in.
        experimental_history: bool,
    },
    Stop {
        socket: Option<String>,
        expected_pid: Option<u32>,
    },
    Revoke {
        socket: Option<String>,
        session: Option<String>,
        all: bool,
    },
    Status {
        socket: Option<String>,
    },
    /// `cua-driver sessions list [--json]` — content-free operator view of
    /// the live sessions owned by the selected daemon runtime.
    Sessions {
        json: bool,
        socket: Option<String>,
    },
    Recording {
        subcommand: String,
        args: Vec<String>,
        socket: Option<String>,
    },
    History {
        subcommand: String,
        args: Vec<String>,
        socket: Option<String>,
        json: bool,
        confirmed: bool,
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
    /// Persist or inspect the stable/nightly release preference.
    Channel {
        subcommand: String,
        value: Option<String>,
        json: bool,
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
    /// dirs (Claude Code / Codex / Prime Agent / OpenClaw / OpenCode); the install
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
    /// Trusted local cursor-theme authoring and installation workflow. The
    /// actual parser/compiler is a separate short-lived executable so Lottie,
    /// ZIP, and JSON are not linked into the privileged daemon.
    CursorTheme {
        args: Vec<String>,
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
    "--cursor-theme",
    "--cursor-reduced-motion",
    "--glide-ms",
    "--dwell-ms",
    "--idle-hide-ms",
    "--screenshot-out-file",
    "--client",
    "--socket",
    "--permission-mode",
    "--grant",
    "--session-policy",
    "--capability-manifest",
    "--pid-file",
    "--expected-pid",
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

/// Authorization selectors are trusted-daemon startup inputs. Direct MCP
/// accepts their environment-variable equivalents, while a daemon-backed MCP
/// client inherits the already-fixed profile through `--socket`. Silently
/// consuming these flags on `mcp` would leave the default profile active while
/// telling the operator nothing.
const SERVE_ONLY_AUTHORIZATION_FLAGS: &[&str] = &[
    "--permission-mode",
    "--capability-manifest",
    "--approve-capability-manifest",
    "--session-policy",
    "--approve-session-policy",
    "--dangerously-bypass-approvals",
    "--no-permissions-gate",
];

fn serve_only_authorization_flag(args: &[String]) -> Option<&'static str> {
    SERVE_ONLY_AUTHORIZATION_FLAGS.iter().copied().find(|flag| {
        args.iter().any(|arg| {
            arg == flag
                || arg
                    .strip_prefix(flag)
                    .is_some_and(|remainder| remainder.starts_with('='))
        })
    })
}

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
        Some("revoke") => Some("revoke"),
        Some("status") => Some("status"),
        Some("sessions") => Some("sessions"),
        Some("recording") => Some("recording"),
        Some("history") => Some("history"),
        Some("dump-docs") => Some("dump_docs"),
        Some("update") => Some("update"),
        Some("check-update") => Some("check_update"),
        Some("channel") => Some("channel"),
        Some("doctor") => Some("doctor"),
        Some("diagnose") => Some("diagnose"),
        Some("permissions") => Some("permissions"),
        Some("autostart") => Some("autostart"),
        Some("skills") => Some("skills"),
        Some("cursor-theme") => Some("cursor_theme"),
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
        Some("history") => match subcommand.unwrap_or("status") {
            "enable" => "enable",
            "disable" => "disable",
            "pause" => "pause",
            "resume" => "resume",
            "status" => "status",
            "flush" => "flush",
            "list" => "list",
            "show" => "show",
            "delete" => "delete",
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
        Some("sessions") => match subcommand.unwrap_or("list") {
            "list" => "list",
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
        Some("channel") => match subcommand.unwrap_or("status") {
            "status" => "status",
            "set" => "set",
            _ => "other",
        },
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
        "prime-agent" => "prime_agent",
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
        println!("Subcommands: mcp, list-tools, describe, call, serve, stop, revoke, status, config, telemetry, recording, update, check-update, doctor, diagnose, permissions, autostart, skills, manifest, channel, cursor-theme, sessions, history");
        println!();
        println!("permissions options (macOS):");
        println!("  cua-driver permissions status   Report Accessibility + Screen Recording status. Read-only (no prompt).");
        println!("                                  Answers via a running daemon, so the result carries the CuaDriver");
        println!("                                  identity (com.trycua.driver). If no daemon is running it reports");
        println!("                                  `unknown` rather than your terminal's grants. Add --json for the payload.");
        println!("  cua-driver permissions grant    Launch CuaDriver via LaunchServices so dialogs attribute to the app,");
        println!("                                  explain and request Accessibility, Screen Recording, and Tahoe's");
        println!("                                  direct-capture consent, then verify live capture. This is the correct");
        println!("                                  way to grant; the read-only status command never triggers that probe.");
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
        println!("  cua-driver channel status      Show the saved stable/nightly update channel.");
        println!("  cua-driver channel set <name>  Save stable or nightly; run update --apply to switch binaries.");
        println!("    --json                        Emit machine-readable channel state.");
        println!();
        println!("autostart options (Windows-only today):");
        println!("  cua-driver autostart enable     Register a logon Scheduled Task so serve starts at every interactive logon.");
        println!("  cua-driver autostart disable    Remove the autostart entry. No-op if not registered.");
        println!("  cua-driver autostart status     Print whether the entry is registered + whether the daemon is running.");
        println!("  cua-driver autostart kick       Start the entry now without re-logging.");
        println!();
        println!("skills options (agent skill-pack management, opt-in):");
        println!("  cua-driver skills install       Fetch the versioned skill pack from GitHub Releases and symlink it");
        println!("                                  into each detected agent's skills/ dir (Claude Code, Codex, Prime Agent,");
        println!("                                  OpenClaw, OpenCode). Idempotent. Never overwrites existing user links.");
        println!("  cua-driver skills update        Re-fetch the skill pack from GitHub, refreshing the local copy + links.");
        println!("  cua-driver skills uninstall     Remove the agent symlinks. Add --all to also delete the local copy.");
        println!("  cua-driver skills status        Report local install state + per-agent link state. Read-only.");
        println!("  cua-driver skills path          Print where the local skill pack lives.");
        println!("  --from main                     (install only) Fetch latest from main branch instead of the tagged release.");
        println!();
        println!("agent authorization (serve only):");
        println!("  --permission-mode <mode>        standard (default), bounded, or unrestricted.");
        println!(
            "  --grant existing-profile       Pre-authorize existing logged-in Chromium attachment"
        );
        println!("                                  for this runtime. Repeatable for future grant types.");
        println!(
            "  --dangerously-bypass-approvals  Select unrestricted mode and acknowledge its risk."
        );
        println!("                                  The mode is fixed for the daemon lifetime and cannot");
        println!("                                  be changed by a tool call.");
        println!("                                  This is separate from --no-permissions-gate, which only");
        println!(
            "                                  controls the macOS OS-permission onboarding UI."
        );
        println!(
            "  --capability-manifest <path>    Narrow-only tool/resource manifest; required in bounded mode."
        );
        println!("  --approve-capability-manifest   Required with --capability-manifest; the trusted launcher asserts");
        println!("                                  that the human reviewed this exact manifest at startup.");
        println!("  --session-policy <path>         Deprecated alias for --capability-manifest.");
        println!(
            "  --approve-session-policy        Deprecated alias for --approve-capability-manifest."
        );
        println!();
        println!("authorization revocation:");
        println!("  cua-driver revoke --session <id>  Stop and revoke one session's grants.");
        println!("  cua-driver revoke --all           Stop and revoke every live session.");
        println!("                                      Revocation is deny-only and never needs a token.");
        println!();
        println!("mcp options:");
        println!("  --direct                Own the runtime in this MCP process. On macOS this");
        println!("                          deliberately attributes TCC to the invoking host.");
        println!("                          Mutually exclusive with --socket.");
        println!("  --embedded              Declare embedding-host mode (also:");
        println!("                          CUA_DRIVER_EMBEDDED=1). Without --direct, the host");
        println!("                          must start `cua-driver serve --embedded` and pass");
        println!("                          its private endpoint with --socket.");
        println!("                          See Skills/cua-driver/EMBEDDING.md.");
        println!(
            "  --host-bundle-id <id>   Advisory host bundle id label for check_permissions output."
        );
        println!("  --socket <path>         Select an explicit daemon socket/pipe endpoint.");
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
        println!("agent cursor overlay (serve only — needs the daemon UI runloop):");
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
        println!("  --cursor-theme <id>     Select an installed theme (default: cua.default).");
        println!("  --cursor-reduced-motion <auto|on|off>");
        println!("                          Follow the OS setting, force stills, or allow motion.");
        println!("  Set these on `cua-driver serve`; MCP and one-shot CLI processes are clients");
        println!("  and do not own the daemon's overlay configuration or UI runloop.");
        println!();
        println!("cursor-theme options (trusted local workflow):");
        println!("  cua-driver cursor-theme validate <source.lottie>");
        println!("  cua-driver cursor-theme build <source.lottie> --output <theme.cua-theme>");
        println!("  cua-driver cursor-theme inspect <theme.cua-theme> [--json]");
        println!("  cua-driver cursor-theme preview <theme.cua-theme> --output <directory>");
        println!("  cua-driver cursor-theme install <theme.cua-theme>");
        println!("  cua-driver cursor-theme list [--json]");
        println!("  cua-driver cursor-theme uninstall <theme-id>");
        println!("                                  Theme installation is local-only and is never an agent tool.");
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
        println!(
            "  --experimental-history      Admit encrypted local Computer History for this daemon."
        );
        println!(
            "                              Capture remains off until `cua-driver history enable`."
        );
        println!("  cua-driver history enable   Opt in and initialize encrypted local history.");
        println!("  cua-driver history status|pause|resume|flush|list|show|disable|delete");
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
    let approval_session = flag_value(&args, "--session");
    let grants = flag_values(&args, "--grant");

    // `--embedded` / `--host-bundle-id` export to the environment rather
    // than threading through `Command`: all consumers read
    // `cua_driver_core::embedded_mode()` and children inherit the mode.
    if args.iter().any(|a| a == "--embedded") {
        std::env::set_var(cua_driver_core::EMBEDDED_ENV, "1");
    }
    if let Some(id) = flag_value(&args, "--host-bundle-id") {
        std::env::set_var(cua_driver_core::HOST_BUNDLE_ID_ENV, id);
    }
    // Internal host/daemon lifetime contract. The daemon reads from a
    // dedicated inherited stdin pipe and shuts down on EOF. The core helper
    // additionally requires embedded mode, so this can never reinterpret an
    // ordinary MCP proxy's JSON-RPC stdin.
    if args.iter().any(|a| a == "--parent-liveness-stdio") {
        std::env::set_var(cua_driver_core::PARENT_LIVENESS_STDIN_ENV, "1");
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

    let expected_stop_pid = parse_expected_stop_pid(&args, positionals.first().copied());

    if matches!(positionals.first().copied(), None | Some("mcp")) {
        if let Some(flag) = serve_only_authorization_flag(&args) {
            eprintln!("cua-driver mcp does not accept {flag}; authorization flags belong to `cua-driver serve`.");
            eprintln!("For direct MCP, use CUA_DRIVER_PERMISSION_MODE and the related CUA_DRIVER_* environment variables.");
            eprintln!("Otherwise start a configured daemon and connect with `cua-driver mcp --socket <path>`." );
            process::exit(64);
        }
    }

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
                socket: socket.clone(),
                direct: args.iter().any(|a| a == "--direct"),
                claude_code_compat,
                grants: grants.clone(),
            }
        }
        Some("mcp") => Command::Mcp {
            socket: socket.clone(),
            direct: args.iter().any(|a| a == "--direct"),
            claude_code_compat,
            grants: grants.clone(),
        },
        Some("list-tools") => Command::ListTools,
        Some("mcp-config") => Command::McpConfig { client: mcp_client },
        Some("serve") => Command::Serve {
            socket,
            permission_mode: flag_value(&args, "--permission-mode"),
            dangerously_bypass_approvals: args
                .iter()
                .any(|a| a == "--dangerously-bypass-approvals"),
            capability_manifest: aliased_flag_value(
                &args,
                "--capability-manifest",
                "--session-policy",
            ),
            approve_capability_manifest: args
                .iter()
                .any(|a| a == "--approve-capability-manifest" || a == "--approve-session-policy"),
            // Bare flag — present anywhere on argv counts as "skip the gate".
            no_permissions_gate: args.iter().any(|a| a == "--no-permissions-gate"),
            claude_code_compat,
            grants,
            experimental_history: args.iter().any(|a| a == "--experimental-history"),
        },
        Some("stop") => Command::Stop {
            socket,
            expected_pid: expected_stop_pid,
        },
        Some("revoke") => {
            let all = args.iter().any(|a| a == "--all");
            if all == approval_session.is_some() {
                eprintln!("revoke requires exactly one of --session <id> or --all");
                process::exit(64);
            }
            Command::Revoke {
                socket,
                session: approval_session,
                all,
            }
        }
        Some("status") => Command::Status { socket },
        Some("sessions") => {
            let subcommand = pos.next().unwrap_or("list");
            if subcommand != "list" {
                eprintln!("Unknown sessions subcommand '{subcommand}'. Valid: list");
                process::exit(64);
            }
            Command::Sessions {
                json: args.iter().any(|arg| arg == "--json"),
                socket,
            }
        }
        Some("recording") => {
            let subcommand = pos.next().unwrap_or("status").to_string();
            let rest: Vec<String> = pos.map(str::to_owned).collect();
            Command::Recording {
                subcommand,
                args: rest,
                socket,
            }
        }
        Some("history") => {
            let subcommand = pos.next().unwrap_or("status").to_string();
            let rest: Vec<String> = pos.map(str::to_owned).collect();
            Command::History {
                subcommand,
                args: rest,
                socket,
                json: args.iter().any(|arg| arg == "--json"),
                confirmed: args.iter().any(|arg| arg == "--yes"),
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
        Some("channel") => {
            let subcommand = pos.next().unwrap_or("status").to_owned();
            let value = pos.next().map(str::to_owned);
            if !matches!(subcommand.as_str(), "status" | "set") {
                eprintln!("Unknown channel subcommand '{subcommand}'. Valid: status, set");
                process::exit(64);
            }
            if subcommand == "set" && value.is_none() {
                eprintln!("Usage: cua-driver channel set <stable|nightly>");
                process::exit(64);
            }
            Command::Channel {
                subcommand,
                value,
                json: args.iter().any(|arg| arg == "--json"),
            }
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
        Some("cursor-theme") => {
            let index = args
                .iter()
                .position(|value| value == "cursor-theme")
                .expect("cursor-theme positional is present");
            Command::CursorTheme {
                args: args[index + 1..].to_vec(),
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

fn parse_expected_stop_pid(args: &[String], command: Option<&str>) -> Option<u32> {
    let raw = flag_value(args, "--expected-pid")?;
    if command != Some("stop") {
        eprintln!("--expected-pid is valid only with `cua-driver stop`");
        process::exit(64);
    }
    match raw.parse::<u32>() {
        Ok(pid) if pid != 0 => Some(pid),
        _ => {
            eprintln!("--expected-pid requires a positive integer PID");
            process::exit(64);
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

fn aliased_flag_value(args: &[String], preferred: &str, deprecated: &str) -> Option<String> {
    let preferred_value = flag_value(args, preferred);
    let deprecated_value = flag_value(args, deprecated);
    if preferred_value.is_some()
        && deprecated_value.is_some()
        && preferred_value != deprecated_value
    {
        eprintln!("{preferred} conflicts with deprecated {deprecated}");
        process::exit(64);
    }
    preferred_value.or(deprecated_value)
}

/// Return every value of a repeatable `--flag value` or `--flag=value`.
fn flag_values(args: &[String], flag: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let argument = &args[index];
        if argument == flag {
            if let Some(value) = args.get(index + 1) {
                values.push(value.clone());
            }
            index += 2;
            continue;
        }
        if let Some(value) = argument
            .strip_prefix(flag)
            .and_then(|rest| rest.strip_prefix('='))
        {
            values.push(value.to_owned());
        }
        index += 1;
    }
    values
}

/// Print all tools in the registry, one per line: `name: first sentence`.
pub fn run_list_tools(tools_list: &serde_json::Value) {
    // Sort alphabetically by name to match Swift's
    // `ListToolsCommand.run()` `tools.sorted(by: { $0.name < $1.name })`.
    let mut entries = tools_list
        .get("tools")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    entries.sort_by_key(|tool| tool.get("name").and_then(serde_json::Value::as_str));
    for tool in entries {
        let Some(name) = tool.get("name").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let description = tool
            .get("description")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let summary = first_sentence(description);
        if summary.is_empty() {
            println!("{name}");
        } else {
            println!("{name}: {summary}");
        }
    }
}

/// Print a tool's full description and JSON input schema.
/// Exits 64 (EX_USAGE) if the tool is unknown.
pub fn run_describe(tools_list: &serde_json::Value, name: &str) {
    let tools = tools_list
        .get("tools")
        .and_then(serde_json::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    match tools
        .iter()
        .find(|tool| tool.get("name").and_then(serde_json::Value::as_str) == Some(name))
    {
        None => {
            eprintln!("Unknown tool: {name}");
            eprintln!("Available tools:");
            // Sort alphabetically to match Swift's `printUnknownTool`
            // (`registry.allTools.map(\.name).sorted()`).
            let mut names = tools
                .iter()
                .filter_map(|tool| tool.get("name").and_then(serde_json::Value::as_str))
                .collect::<Vec<_>>();
            names.sort();
            for n in names {
                eprintln!("  {n}");
            }
            process::exit(64);
        }
        Some(tool) => {
            let description = tool
                .get("description")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            println!("name: {name}");
            if !description.is_empty() {
                print!("\ndescription:\n{description}");
                if !description.ends_with('\n') {
                    println!();
                }
            }
            print!("\ninput_schema:\n");
            let input_schema = tool
                .get("inputSchema")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({"type": "object"}));
            let pretty = serde_json::to_string_pretty(&input_schema)
                .unwrap_or_else(|_| input_schema.to_string());
            println!("{pretty}");
        }
    }
}

/// Launch the release or local app through LaunchServices so the daemon uses
/// the matching TCC identity and namespace.
/// the daemon under `LaunchServices` (so it inherits the bundle's
/// TCC attribution), then poll the socket for up to `timeout_secs`
/// seconds. Returns Err with a diagnostic message if `open` failed
/// or the daemon never came up.
///
/// Mirror of Swift `MCPCommand.launchDaemonViaOpen` +
/// `waitForDaemon`. Split into one Rust function because we don't
/// need the post-launch probe separation Swift has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchDaemonErrorKind {
    Failed,
    Timeout,
}

#[derive(Debug)]
pub struct LaunchDaemonError {
    pub kind: LaunchDaemonErrorKind,
    message: String,
}

impl std::fmt::Display for LaunchDaemonError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for LaunchDaemonError {}

#[cfg(target_os = "macos")]
pub fn launch_daemon_and_wait(
    socket_path: &str,
    timeout_secs: u64,
    claude_code_compat: bool,
    grants: &[String],
    experimental_history: bool,
) -> Result<(), LaunchDaemonError> {
    let state = crate::history_runtime::DaemonLaunchState {
        claude_code_compat,
        grants: grants.to_vec(),
        ..Default::default()
    };
    launch_daemon_with_state_and_wait(
        socket_path,
        timeout_secs,
        &state,
        experimental_history,
        true,
    )
}

#[cfg(target_os = "macos")]
fn launch_daemon_with_state_and_wait(
    socket_path: &str,
    timeout_secs: u64,
    state: &crate::history_runtime::DaemonLaunchState,
    experimental_history: bool,
    _allow_managed_restart: bool,
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
    let app_name = crate::bundle::app_name();
    let app_path = crate::bundle::app_bundle_path();
    let pass_socket = socket_path != crate::serve::default_socket_path();
    let open_args = daemon_launch_arguments(app_name, socket_path, state, experimental_history);
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
        message: format!("failed to exec `/usr/bin/open`: {error}"),
    })?;

    if !status.success() {
        return Err(LaunchDaemonError {
            kind: LaunchDaemonErrorKind::Failed,
            message: format!(
                "`open -n -g -a {app_name} --args serve{}` exited {:?}. \
             Check that `{app_path}` is installed.",
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
         {app_name}.app in System Settings and retry."
        ),
    })
}

#[cfg(not(target_os = "macos"))]
pub fn launch_daemon_and_wait(
    socket_path: &str,
    timeout_secs: u64,
    claude_code_compat: bool,
    grants: &[String],
    experimental_history: bool,
) -> Result<(), LaunchDaemonError> {
    let state = crate::history_runtime::DaemonLaunchState {
        claude_code_compat,
        grants: grants.to_vec(),
        ..Default::default()
    };
    launch_daemon_with_state_and_wait(
        socket_path,
        timeout_secs,
        &state,
        experimental_history,
        true,
    )
}

#[cfg(not(target_os = "macos"))]
fn launch_daemon_with_state_and_wait(
    socket_path: &str,
    timeout_secs: u64,
    state: &crate::history_runtime::DaemonLaunchState,
    experimental_history: bool,
    allow_managed_restart: bool,
) -> Result<(), LaunchDaemonError> {
    use std::time::{Duration, Instant};

    let executable = std::env::current_exe().map_err(|error| LaunchDaemonError {
        kind: LaunchDaemonErrorKind::Failed,
        message: format!("current Cua Driver executable is unavailable: {error}"),
    })?;
    let managed = allow_managed_restart
        && socket_path == crate::serve::default_socket_path()
        && restart_managed_daemon_if_present(&executable);
    if !managed {
        let arguments = daemon_process_arguments(socket_path, state, experimental_history);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt as _;
            use std::process::{Command as Cmd, Stdio};

            let mut command = Cmd::new(&executable);
            command
                .args(&arguments)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            command.process_group(0);
            command.spawn().map_err(|error| LaunchDaemonError {
                kind: LaunchDaemonErrorKind::Failed,
                message: format!("failed to launch {} serve: {error}", executable.display()),
            })?;
        }
        #[cfg(target_os = "windows")]
        spawn_detached_windows_daemon(&executable, &arguments)?;
    }

    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    while Instant::now() < deadline {
        if crate::serve::is_daemon_listening(socket_path) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(LaunchDaemonError {
        kind: LaunchDaemonErrorKind::Timeout,
        message: format!("daemon did not appear on {socket_path} within {timeout_secs}s"),
    })
}

#[cfg(target_os = "windows")]
fn spawn_detached_windows_daemon(
    executable: &std::path::Path,
    arguments: &[String],
) -> Result<(), LaunchDaemonError> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        CreateProcessW, CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS, PROCESS_INFORMATION,
        STARTUPINFOW,
    };

    let executable_wide = executable
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut command_line = windows_command_line(executable, arguments);
    let mut startup = STARTUPINFOW {
        cb: std::mem::size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    let mut process = PROCESS_INFORMATION::default();
    // Rust's Command::output gives this short-lived CLI inheritable capture
    // handles. A normal Command::spawn can leak those unrelated handles into
    // the long-lived daemon, so the caller never observes EOF. CreateProcess
    // with handle inheritance disabled is the Windows process boundary here.
    unsafe {
        CreateProcessW(
            PCWSTR(executable_wide.as_ptr()),
            Some(PWSTR(command_line.as_mut_ptr())),
            None,
            None,
            false,
            CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS,
            None,
            PCWSTR::null(),
            &mut startup,
            &mut process,
        )
    }
    .map_err(|error| LaunchDaemonError {
        kind: LaunchDaemonErrorKind::Failed,
        message: format!("failed to launch {} serve: {error}", executable.display()),
    })?;
    let _ = unsafe { CloseHandle(process.hThread) };
    let _ = unsafe { CloseHandle(process.hProcess) };
    Ok(())
}

#[cfg(target_os = "windows")]
fn windows_command_line(executable: &std::path::Path, arguments: &[String]) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt as _;

    let mut command_line = Vec::new();
    for argument in
        std::iter::once(executable.as_os_str()).chain(arguments.iter().map(std::ffi::OsStr::new))
    {
        if !command_line.is_empty() {
            command_line.push(' ' as u16);
        }
        append_windows_argument(&mut command_line, argument.encode_wide());
    }
    command_line.push(0);
    command_line
}

#[cfg(target_os = "windows")]
fn append_windows_argument(command_line: &mut Vec<u16>, argument: impl IntoIterator<Item = u16>) {
    let argument = argument.into_iter().collect::<Vec<_>>();
    let quote = argument.is_empty()
        || argument
            .iter()
            .any(|value| matches!(*value, 0x20 | 0x09 | 0x22));
    if !quote {
        command_line.extend(argument);
        return;
    }
    command_line.push('"' as u16);
    let mut backslashes = 0;
    for value in argument {
        if value == '\\' as u16 {
            backslashes += 1;
        } else if value == '"' as u16 {
            command_line.extend(std::iter::repeat_n('\\' as u16, backslashes * 2 + 1));
            command_line.push(value);
            backslashes = 0;
        } else {
            command_line.extend(std::iter::repeat_n('\\' as u16, backslashes));
            command_line.push(value);
            backslashes = 0;
        }
    }
    command_line.extend(std::iter::repeat_n('\\' as u16, backslashes * 2));
    command_line.push('"' as u16);
}

#[cfg(not(target_os = "macos"))]
fn daemon_process_arguments(
    socket_path: &str,
    state: &crate::history_runtime::DaemonLaunchState,
    experimental_history: bool,
) -> Vec<String> {
    let mut args = vec!["serve".to_owned()];
    if socket_path != crate::serve::default_socket_path() {
        args.extend(["--socket".to_owned(), socket_path.to_owned()]);
    }
    append_daemon_launch_state(&mut args, state, experimental_history);
    args
}

#[cfg(target_os = "windows")]
fn restart_managed_daemon_if_present(executable: &std::path::Path) -> bool {
    use std::process::{Command as Cmd, Stdio};
    let task = crate::bundle::autostart_task_name();
    let exists = Cmd::new("schtasks.exe")
        .args(["/Query", "/TN", task])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    exists
        && Cmd::new(executable)
            .args(["autostart", "kick"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
}

#[cfg(target_os = "linux")]
fn restart_managed_daemon_if_present(_executable: &std::path::Path) -> bool {
    use std::process::{Command as Cmd, Stdio};
    let unit = if crate::bundle::is_local_installation() {
        "cua-driver-local.service"
    } else {
        "cua-driver.service"
    };
    let exists = Cmd::new("systemctl")
        .args(["--user", "is-enabled", unit])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    exists
        && Cmd::new("systemctl")
            .args(["--user", "restart", unit])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
}

#[cfg(target_os = "macos")]
fn daemon_launch_arguments(
    app_name: &str,
    socket_path: &str,
    state: &crate::history_runtime::DaemonLaunchState,
    experimental_history: bool,
) -> Vec<String> {
    let mut args = vec![
        "-n".to_owned(),
        "-g".to_owned(),
        "-a".to_owned(),
        app_name.to_owned(),
        "--args".to_owned(),
        "serve".to_owned(),
    ];
    if socket_path != crate::serve::default_socket_path() {
        args.extend(["--socket".to_owned(), socket_path.to_owned()]);
    }
    append_daemon_launch_state(&mut args, state, experimental_history);
    args
}

fn append_daemon_launch_state(
    args: &mut Vec<String>,
    state: &crate::history_runtime::DaemonLaunchState,
    experimental_history: bool,
) {
    if let Some(mode) = &state.permission_mode {
        args.extend(["--permission-mode".to_owned(), mode.clone()]);
    }
    if state.dangerously_bypass_approvals {
        args.push("--dangerously-bypass-approvals".to_owned());
    }
    if let Some(manifest) = &state.capability_manifest {
        args.extend(["--capability-manifest".to_owned(), manifest.clone()]);
    }
    if state.approve_capability_manifest {
        args.push("--approve-capability-manifest".to_owned());
    }
    if state.no_permissions_gate {
        args.push("--no-permissions-gate".to_owned());
    }
    if state.claude_code_compat {
        args.push("--claude-code-computer-use-compat".to_owned());
    }
    if experimental_history {
        args.push("--experimental-history".to_owned());
    }
    for grant in &state.grants {
        args.extend(["--grant".to_owned(), grant.clone()]);
    }
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
    grants: &[String],
    on_startup: F,
) -> anyhow::Result<()>
where
    F: FnOnce(McpDaemonStartup, bool),
{
    let mut on_startup = Some(on_startup);
    // The UIAccess helper is a daemon-internal privilege boundary. Public MCP
    // clients always enter through the canonical service authorization path;
    // they must never select the helper merely because its pipe exists.
    let socket_path = socket.unwrap_or_else(crate::serve::default_socket_path);

    let already_running = crate::serve::is_daemon_listening(&socket_path);
    if already_running && !grants.is_empty() {
        if let Some(on_startup) = on_startup.take() {
            on_startup(McpDaemonStartup::AlreadyRunning, false);
        }
        anyhow::bail!(
            "--grant configures a newly launched runtime and cannot modify the daemon already listening on {socket_path}; restart it with the same --grant option"
        );
    }
    let mut daemon = McpDaemonStartup::AlreadyRunning;
    if !already_running {
        // Never replace an embedded host's TCC identity by launching the
        // standalone CuaDriver.app daemon.
        if cua_driver_core::embedded_mode() {
            if let Some(on_startup) = on_startup.take() {
                on_startup(McpDaemonStartup::Unreachable, false);
            }
            anyhow::bail!(
                "no Cua Driver daemon listening on {socket_path}. Start one with \
                 `cua-driver serve --socket {socket_path}` and retry. Embedded hosts \
                 must spawn `cua-driver serve --embedded` before starting the MCP proxy."
            );
        }
        #[cfg(target_os = "macos")]
        {
            let app_name = crate::bundle::app_name();
            let socket_suffix = if socket_path != crate::serve::default_socket_path() {
                format!(" --socket {socket_path}")
            } else {
                String::new()
            };
            eprintln!(
                "{}: mcp launched without {app_name}.app's TCC grants; \
                 auto-launching the daemon via `open -n -g -a {app_name} --args serve{socket_suffix}` \
                 and proxying MCP requests through it.",
                crate::bundle::cli_name()
            );
            if let Err(error) = launch_daemon_and_wait(
                &socket_path,
                10,
                claude_code_compat,
                grants,
                crate::history_runtime::preview_admitted_preference(),
            ) {
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
        {
            if !crate::history_runtime::preview_admitted_preference() {
                if let Some(on_startup) = on_startup.take() {
                    on_startup(McpDaemonStartup::UnsupportedRelaunch, false);
                }
                anyhow::bail!(
                    "no Cua Driver daemon listening on {socket_path}. Start one in \
                     your interactive session — on Windows run \
                     `cua-driver autostart enable && cua-driver autostart kick`; \
                     on Linux run `cua-driver serve &` in the user's session. \
                     Then re-run `cua-driver mcp`."
                );
            }
            if let Err(error) =
                launch_daemon_and_wait(&socket_path, 10, claude_code_compat, grants, true)
            {
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

fn manifest_feature_flags(
    target_is_linux: bool,
    portal_input_enabled: bool,
    portal_capture_enabled: bool,
) -> (bool, bool, bool) {
    (
        target_is_linux,
        target_is_linux && portal_input_enabled,
        target_is_linux && portal_capture_enabled,
    )
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

    let (wayland_native, portal_input, portal_capture) = manifest_feature_flags(
        cfg!(target_os = "linux"),
        cfg!(feature = "portal-input"),
        cfg!(feature = "portal-capture"),
    );

    serde_json::json!({
        // `schema_version` is bumped only on a breaking change to the
        // manifest shape itself. Additive field changes don't bump it.
        // Consumers branch on this when reading the document.
        "schema_version": "1",
        "binary_version": env!("CARGO_PKG_VERSION"),
        "binary_path": binary,
        // A release version cannot prove which optional Linux features were
        // compiled into this artifact. Downstream hosts use this additive,
        // machine-readable map to decide whether native Wayland may be
        // auto-enabled safely.
        "features": {
            "wayland_native": wayland_native,
            "portal_input": portal_input,
            "portal_capture": portal_capture,
        },
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
              "description": "Run the MCP stdio server: direct runtime on Windows/Linux, app-daemon proxy on macOS, or explicit service with --socket.",
              "args": [
                  { "name": "--socket", "type": "string", "description": "Select an explicit daemon socket or named-pipe endpoint." },
                  { "name": "--direct", "type": "flag", "description": "Own the runtime in the MCP process; on macOS this explicitly accepts host TCC attribution. Mutually exclusive with --socket." },
                  { "name": "--claude-code-computer-use-compat", "type": "flag", "description": "Select the Claude Code computer-use compat tool surface." },
                  { "name": "--embedded", "type": "flag", "description": "Declare embedding-host mode. Without --direct, requires the host's private service through --socket instead of auto-launching the standalone app." },
                  { "name": "--host-bundle-id", "type": "string", "description": "Advisory host bundle id label echoed in check_permissions output." },
                  { "name": "--grant", "type": "repeatable-string", "description": "Pre-authorize a residual standard-mode boundary for a newly launched runtime. Supported value: existing-profile." }
              ] },
            { "name": "serve",
              "description": "Run the long-lived daemon — backs the proxy/auto-relaunch path on macOS and the autostart Session 1+ daemon on Windows.",
              "args": [
                  { "name": "--socket", "type": "string", "description": "Override the listen socket path." },
                  { "name": "--permission-mode", "type": "string", "description": "Immutable daemon authorization mode: standard, bounded, or unrestricted." },
                  { "name": "--grant", "type": "repeatable-string", "description": "Pre-authorize a residual standard-mode boundary. Supported value: existing-profile." },
                  { "name": "--dangerously-bypass-approvals", "type": "flag", "description": "Select unrestricted mode and acknowledge its risk." },
                  { "name": "--capability-manifest", "type": "string", "description": "Optional narrow-only tool/resource ceiling; required in bounded mode." },
                  { "name": "--approve-capability-manifest", "type": "flag", "description": "Trusted-launcher confirmation that the exact capability manifest was reviewed." },
                  { "name": "--session-policy", "type": "string", "description": "Deprecated alias for --capability-manifest." },
                  { "name": "--approve-session-policy", "type": "flag", "description": "Deprecated alias for --approve-capability-manifest." },
                  { "name": "--no-permissions-gate", "type": "flag", "description": "Skip the macOS TCC first-launch gate." },
                  { "name": "--claude-code-computer-use-compat", "type": "flag", "description": "Forwarded by the MCP proxy when the client asked for the compat surface." },
                  { "name": "--embedded", "type": "flag", "description": "Run embedded inside a host app: inherit the host's TCC grants, never prompt or relaunch. Also CUA_DRIVER_EMBEDDED=1." },
                  { "name": "--host-bundle-id", "type": "string", "description": "Advisory host bundle id label echoed in check_permissions output." }
                  ,{ "name": "--experimental-history", "type": "flag", "description": "Admit the encrypted local Computer History early preview for this daemon launch." }
              ] },
            { "name": "stop",
              "description": "Stop a running daemon by sending it a shutdown request.",
              "args": [ { "name": "--socket", "type": "string", "description": "Override the daemon socket path." } ] },
            { "name": "revoke",
              "description": "Revoke one session or every live authorization scope without minting new authority.",
              "args": [
                  { "name": "--session", "type": "string", "description": "Exact session id to stop and revoke." },
                  { "name": "--all", "type": "flag", "description": "Stop and revoke every live session." },
                  { "name": "--socket", "type": "string", "description": "Override the daemon socket path." }
              ] },
            { "name": "status",
              "description": "Report daemon status (running / not / unhealthy).",
              "args": [ { "name": "--socket", "type": "string", "description": "Override the daemon socket path." } ] },
            { "name": "sessions",
              "description": "List content-free lifecycle summaries for sessions owned by the daemon runtime.",
              "args": [
                  { "name": "subcommand", "type": "positional-string", "description": "Only: list. Default: list." },
                  { "name": "--json", "type": "flag", "description": "Emit the machine-readable session summary." },
                  { "name": "--socket", "type": "string", "description": "Override the daemon socket path." }
              ] },
            { "name": "list-tools",
              "description": "Print the canonical tool name + one-line summary for every registered MCP tool.",
              "args": [] },
            { "name": "describe",
              "description": "Print a single tool's full description + JSON input schema.",
              "args": [ { "name": "tool", "type": "positional-string", "description": "Tool name." } ] },
            { "name": "call",
              "description": "Invoke a single tool through the required running daemon.",
              "args": [
                  { "name": "tool", "type": "positional-string", "description": "Tool name." },
                  { "name": "json-args", "type": "positional-json", "description": "Tool input JSON (or read from stdin)." },
                  { "name": "--screenshot-out-file", "type": "string", "description": "Write image content to this path instead of emitting base64." },
                  { "name": "--socket", "type": "string", "description": "Override the required daemon socket path." }
              ] },
            { "name": "mcp-config",
              "description": "Print client-specific connection guidance (MCP config where supported).",
              "args": [ { "name": "--client", "type": "string", "description": "One of: claude, codex, cursor, hermes, antigravity, openclaw, opencode, pi, prime-agent, qwen, droid, zcode. Omit for the generic snippet." } ] },
            { "name": "manifest",
              "description": "Emit this machine-readable description of the CLI surface.",
              "args": [ { "name": "--pretty", "type": "flag", "description": "Pretty-print the JSON." } ] },
            { "name": "recording",
              "description": "Recording sub-API: start | stop | status | render.",
              "args": [
                  { "name": "subcommand", "type": "positional-string", "description": "One of: start, stop, status, render. Default: status." },
                  { "name": "--socket", "type": "string", "description": "Override the daemon socket path." }
              ] },
            { "name": "history",
              "description": "Encrypted, metadata-only Computer History early-preview lifecycle and local inspection controls.",
              "args": [
                  { "name": "subcommand", "type": "positional-string", "description": "enable | disable | pause | resume | status | flush | list [limit] | show <sequence> | delete --yes" },
                  { "name": "--socket", "type": "string", "description": "Override the daemon socket path." },
                  { "name": "--json", "type": "flag", "description": "Emit machine-readable output." },
                  { "name": "--yes", "type": "flag", "description": "Confirm irreversible deletion of encrypted chunks and their native credential-store key." }
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
            { "name": "channel",
              "description": "Inspect or persist the stable/nightly release channel.",
              "args": [
                  { "name": "subcommand", "type": "positional-string", "description": "status | set. Default: status." },
                  { "name": "channel", "type": "positional-string", "description": "stable | nightly (required for set)." },
                  { "name": "--json", "type": "flag", "description": "Emit machine-readable channel state." }
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

/// Print client-specific connection guidance (MCP config where supported).
///
/// `--client <name>` selects one of: claude, codex, cursor, hermes,
/// antigravity, openclaw, opencode, pi, prime-agent, qwen, droid, zcode.
/// Omit for the generic JSON snippet.
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
        Some("prime-agent") => {
            println!(
                "Prime Agent loads Agent Skills and can call cua-driver directly from its\n\
                 persistent IPython control environment. No MCP registration is required.\n\n\
                 Install and verify the Cua Driver skill pack:\n\n\
                     {binary} skills install\n\
                     {binary} skills status\n\n\
                 Then run /reload in Prime Agent (or start a new session) and ask it to\n\
                 use the Cua Driver skill. Use /skill:cua-driver to invoke it explicitly.\n\
                 The skill calls the cua-driver CLI with a snapshot/action/verify workflow."
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
            eprintln!("Unknown client '{other}'. Valid: claude, codex, cursor, antigravity, openclaw, opencode, hermes, pi, prime-agent, qwen, droid, zcode.");
            process::exit(2);
        }
    }
}

/// Invoke a tool through the required running daemon. Prints result to stdout
/// on success and stderr on failure. Exits non-zero when the daemon is absent,
/// unreachable, or the tool returns an error.
/// When `screenshot_out_file` is provided, image content is written there
/// instead of emitted as base64 on stdout.
///
/// `socket` — override the daemon socket path (from --socket flag).
fn ensure_compatible_daemon(socket_path: &str) -> Result<(), String> {
    let driver = cua_driver_sdk::CuaDriver::connect(Some(socket_path.to_owned()))
        .map_err(|error| error.to_string())?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("create compatibility runtime: {error}"))?;
    runtime
        .block_on(driver.metadata())
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn require_compatible_daemon(socket_path: &str) {
    if let Err(error) = ensure_compatible_daemon(socket_path) {
        eprintln!("Cua Driver daemon on {socket_path} is incompatible: {error}");
        process::exit(1);
    }
}

#[cfg(all(test, unix))]
mod daemon_compatibility_tests {
    use super::ensure_compatible_daemon;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;

    #[test]
    fn incompatible_daemon_is_refused_before_a_cli_action_can_dispatch() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("driver.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut request_line = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut request_line)
                .unwrap();
            let request: serde_json::Value = serde_json::from_str(&request_line).unwrap();
            assert_eq!(request["method"], "metadata");

            let mut metadata = cua_driver_core::daemon::current_daemon_metadata();
            metadata.contract_version = "incompatible-test-contract".into();
            let mut writer = stream;
            writeln!(
                writer,
                "{}",
                serde_json::json!({"ok": true, "result": metadata})
            )
            .unwrap();
        });

        let error = ensure_compatible_daemon(socket.to_str().unwrap()).unwrap_err();
        assert!(error.contains("incompatible daemon"), "{error}");
        server.join().unwrap();
    }
}

pub fn run_call(
    tool: &str,
    json_args: Option<serde_json::Value>,
    screenshot_out_file: Option<String>,
    socket_override: Option<String>,
) {
    // One-shot public calls remain service-backed so policy, session state,
    // AppStateEngine caches, and platform identity have one enforcement point.
    // The Windows UIAccess helper is daemon-internal: routing an untrusted CLI
    // directly to it would bypass this authorization path.
    //
    // When `socket_override` is Some (i.e. caller passed `--socket <path>`),
    // route directly to that path and skip the platform default. Used by
    // integration tests to drive a tempfile-socketed daemon.
    let socket_path = socket_override.unwrap_or_else(crate::serve::default_socket_path);
    if !crate::serve::is_daemon_listening(&socket_path) {
        eprintln!(
            "Cua Driver daemon is not running on {socket_path}.\n\
             Start it first with: cua-driver serve --socket {socket_path}"
        );
        process::exit(1);
    }
    require_compatible_daemon(&socket_path);

    {
        let mut args_for_daemon = json_args
            .clone()
            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
        cua_driver_core::tool_args::sanitize_reserved_args(&mut args_for_daemon);
        let named_session = args_for_daemon
            .get("session")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|session| !session.is_empty() && session != "default");
        // One-shot CLI processes share one daemon-scoped ownership namespace
        // for explicit public labels. The daemon adds its runtime prefix, so
        // this cannot attach to another daemon generation or transport kind.
        // Anonymous calls keep their disposable per-process lease below.
        let transport_session = if named_session {
            "cli-explicit".to_owned()
        } else {
            format!("cli-{}", uuid::Uuid::new_v4())
        };
        let req = crate::serve::DaemonRequest {
            method: "call".into(),
            name: Some(tool.to_owned()),
            args: Some(args_for_daemon),
            // Every one-shot call owns one disposable implicit transport
            // session. The daemon closes all lifecycle state attached to it
            // synchronously after the response is received.
            session_id: Some(transport_session.clone()),
            observation_origin: Some(crate::serve::ToolObservationOrigin::Direct),
            client_kind: Some(cua_driver_core::daemon::DaemonClientKind::Cli),
        };
        let response = crate::serve::send_request(&socket_path, &req);
        if !named_session {
            let cleanup = crate::serve::DaemonRequest {
                method: "session_end".into(),
                name: None,
                args: None,
                session_id: Some(transport_session),
                observation_origin: None,
                client_kind: Some(cua_driver_core::daemon::DaemonClientKind::Cli),
            };
            let cleanup_result = crate::serve::send_request(&socket_path, &cleanup);
            if let Err(error) = cleanup_result {
                eprintln!("warning: disposable session cleanup failed: {error}");
            }
        }
        match response {
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
                            // Merge image data into the structured payload so
                            // `cua-driver call screenshot` over
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
                } else {
                    if let Some(err) = resp.error {
                        eprintln!("{err}");
                    }
                    let exit_code = resp.exit_code.unwrap_or(1);
                    process::exit(exit_code);
                }
            }
            Err(e) => {
                eprintln!("Cua Driver daemon request on {socket_path} failed: {e}");
                process::exit(1);
            }
        }
    }
}

/// Operator-only lifecycle and inspection surface for encrypted Computer
/// History. Mutation is sent over a daemon-private method and is never
/// registered as an MCP tool.
pub fn run_history_cmd(
    subcommand: &str,
    args: &[String],
    socket: Option<&str>,
    json: bool,
    confirmed: bool,
) {
    let mut enabled_preview_for_this_command = false;
    let mut prior_daemon_was_running = false;
    let mut prior_daemon_state = crate::history_runtime::DaemonLaunchState::default();
    let prior_preview_admitted_preference = crate::history_runtime::preview_admitted_preference();
    let valid = matches!(
        subcommand,
        "enable" | "disable" | "pause" | "resume" | "status" | "flush" | "list" | "show" | "delete"
    );
    if !valid {
        eprintln!("Unknown history subcommand '{subcommand}'. Valid: enable, disable, pause, resume, status, flush, list, show <sequence>, delete --yes");
        process::exit(64);
    }
    if subcommand == "delete" && !confirmed {
        eprintln!("history delete destroys the encrypted files and their native credential-store key. Re-run with --yes.");
        process::exit(64);
    }
    let socket_path = socket
        .map(str::to_owned)
        .unwrap_or_else(crate::serve::default_socket_path);

    if subcommand == "enable" {
        let admitted = history_daemon_status(&socket_path)
            .and_then(|value| value.get("admitted").and_then(serde_json::Value::as_bool))
            == Some(true);
        if !admitted {
            #[cfg(target_os = "macos")]
            if crate::bundle::is_local_installation() {
                eprintln!(
                    "This local-development daemon is not admitted for Computer History. Restart it with:\n  {} serve --experimental-history\nThen run `{} history enable` again.",
                    crate::bundle::cli_name(),
                    crate::bundle::cli_name(),
                );
                process::exit(1);
            }
            if let Err(error) = crate::history_runtime::verify_installed_product_for_history() {
                eprintln!("history enable: installed product verification failed: {error}");
                process::exit(1);
            }
            if crate::serve::is_daemon_listening(&socket_path) {
                prior_daemon_was_running = true;
                prior_daemon_state = match history_daemon_relaunch_state(&socket_path) {
                    Ok(state) => state,
                    Err(error) => {
                        eprintln!(
                            "history enable: cannot preserve the running daemon mode: {error}. Stop the daemon and retry."
                        );
                        process::exit(1);
                    }
                };
            }
            if let Err(error) = crate::history_runtime::set_preview_admitted_preference(true) {
                eprintln!("history enable: could not persist preview admission: {error}");
                process::exit(1);
            }
            enabled_preview_for_this_command = true;
            if crate::serve::is_daemon_listening(&socket_path) {
                if let Err(error) = stop_history_daemon(&socket_path) {
                    let _ = crate::history_runtime::set_preview_admitted_preference(false);
                    eprintln!(
                        "history enable: could not stop the existing daemon for preview admission: {error}"
                    );
                    process::exit(1);
                }
            }
            if let Err(error) = launch_daemon_with_state_and_wait(
                &socket_path,
                15,
                &prior_daemon_state,
                true,
                false,
            ) {
                rollback_history_preview(
                    &socket_path,
                    prior_daemon_was_running,
                    &prior_daemon_state,
                    prior_preview_admitted_preference,
                );
                eprintln!("history enable: could not relaunch the installed daemon with preview admission: {error}");
                process::exit(1);
            }
        }
    }

    if !crate::serve::is_daemon_listening(&socket_path) {
        eprintln!(
            "Cua Driver daemon is not running. Start it with: {} serve{}",
            crate::bundle::cli_name(),
            if crate::history_runtime::preview_admitted_preference() {
                " --experimental-history"
            } else {
                ""
            }
        );
        process::exit(1);
    }
    if let Err(error) = ensure_compatible_daemon(&socket_path) {
        if enabled_preview_for_this_command {
            rollback_history_preview(
                &socket_path,
                prior_daemon_was_running,
                &prior_daemon_state,
                prior_preview_admitted_preference,
            );
        }
        eprintln!("Cua Driver daemon on {socket_path} is incompatible: {error}");
        process::exit(1);
    }

    let mut request_args = serde_json::json!({"operation": subcommand});
    if subcommand == "list" {
        if let Some(limit) = args.first().and_then(|value| value.parse::<u64>().ok()) {
            request_args["limit"] = serde_json::json!(limit);
        }
    }
    if subcommand == "show" {
        let Some(sequence) = args.first().and_then(|value| value.parse::<u64>().ok()) else {
            eprintln!(
                "Usage: {} history show <sequence>",
                crate::bundle::cli_name()
            );
            process::exit(64);
        };
        request_args["sequence"] = serde_json::json!(sequence);
    }
    let request = crate::serve::DaemonRequest {
        method: "history_control".to_owned(),
        name: None,
        args: Some(request_args),
        session_id: None,
        observation_origin: Some(crate::serve::ToolObservationOrigin::Direct),
        client_kind: Some(cua_driver_core::daemon::DaemonClientKind::Cli),
    };
    match crate::serve::send_request(&socket_path, &request) {
        Ok(response) if response.ok => {
            let value = response.result.unwrap_or_else(|| serde_json::json!({}));
            if json || matches!(subcommand, "list" | "show") {
                println!("{}", serde_json::to_string_pretty(&value).unwrap());
            } else if subcommand == "enable" {
                println!("Computer History preview enabled.");
                println!("Stored fields: time, opaque session/action ids, fixed capability, app name/bundle id, and fixed action outcome metadata.");
                println!("Never stored: screenshots, typed text, clipboard contents, raw arguments/results, accessibility trees, paths, titles, URLs, or free-form diagnostics.");
                println!(
                    "Encryption: CBOR Sequence + COSE_Encrypt0 (ChaCha20-Poly1305), with the key protected by {}.",
                    crate::history_runtime::platform_key_store_name()
                );
                println!(
                    "Retention: {} days. Quota: {} MiB.",
                    value
                        .get("retention_days")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(7),
                    value
                        .get("quota_bytes")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0)
                        / 1024
                        / 1024
                );
                println!(
                    "Controls: history pause | resume | status | list | disable | delete --yes"
                );
                // An MCP host that connected while history was unadmitted owns a
                // direct in-process runtime for its whole lifetime, and that
                // runtime never registers the history hook. Its actions are
                // dropped silently while every status surface reports ready, so
                // say so here rather than let the operator infer it. See #3220.
                println!(
                    "Reconnect any agent session that was already connected: sessions started before now do not record actions."
                );
            } else {
                println!("{}", serde_json::to_string_pretty(&value).unwrap());
            }
        }
        Ok(response) => {
            if enabled_preview_for_this_command {
                rollback_history_preview(
                    &socket_path,
                    prior_daemon_was_running,
                    &prior_daemon_state,
                    prior_preview_admitted_preference,
                );
            }
            eprintln!(
                "history {subcommand}: {}",
                response
                    .error
                    .unwrap_or_else(|| "operation failed".to_owned())
            );
            process::exit(response.exit_code.unwrap_or(1));
        }
        Err(error) => {
            if enabled_preview_for_this_command {
                rollback_history_preview(
                    &socket_path,
                    prior_daemon_was_running,
                    &prior_daemon_state,
                    prior_preview_admitted_preference,
                );
            }
            eprintln!("history {subcommand}: {error}");
            process::exit(1);
        }
    }
}

fn rollback_history_preview(
    socket_path: &str,
    prior_daemon_was_running: bool,
    prior_daemon_state: &crate::history_runtime::DaemonLaunchState,
    prior_preview_admitted_preference: bool,
) {
    let _ =
        crate::history_runtime::set_preview_admitted_preference(prior_preview_admitted_preference);
    let _ = stop_history_daemon(socket_path);
    if prior_daemon_was_running && !crate::serve::is_daemon_listening(socket_path) {
        let _ =
            launch_daemon_with_state_and_wait(socket_path, 15, prior_daemon_state, false, false);
    }
}

fn stop_history_daemon(socket_path: &str) -> Result<(), String> {
    if !crate::serve::is_daemon_listening(socket_path) {
        return Ok(());
    }
    let response = crate::serve::send_request(
        socket_path,
        &crate::serve::DaemonRequest {
            method: "shutdown".to_owned(),
            name: None,
            args: None,
            session_id: None,
            observation_origin: None,
            client_kind: None,
        },
    )
    .map_err(|error| error.to_string())?;
    if !response.ok {
        return Err(response
            .error
            .unwrap_or_else(|| "daemon refused shutdown".to_owned()));
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        if !crate::serve::is_daemon_listening(socket_path) {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    Err("daemon did not stop within 2 seconds".to_owned())
}

fn history_daemon_relaunch_state(
    socket_path: &str,
) -> Result<crate::history_runtime::DaemonLaunchState, String> {
    let response = crate::serve::send_request(
        socket_path,
        &crate::serve::DaemonRequest {
            method: "history_relaunch_state".to_owned(),
            name: None,
            args: None,
            session_id: None,
            observation_origin: Some(crate::serve::ToolObservationOrigin::Direct),
            client_kind: Some(cua_driver_core::daemon::DaemonClientKind::Cli),
        },
    )
    .map_err(|error| error.to_string())?;
    if !response.ok {
        return Err(response
            .error
            .unwrap_or_else(|| "daemon refused relaunch-state inspection".to_owned()));
    }
    serde_json::from_value(
        response
            .result
            .ok_or_else(|| "daemon omitted relaunch-state metadata".to_owned())?,
    )
    .map_err(|error| format!("invalid relaunch-state metadata: {error}"))
}

fn history_daemon_status(socket_path: &str) -> Option<serde_json::Value> {
    if !crate::serve::is_daemon_listening(socket_path) {
        return None;
    }
    let request = crate::serve::DaemonRequest {
        method: "history_control".to_owned(),
        name: None,
        args: Some(serde_json::json!({"operation": "status"})),
        session_id: None,
        observation_origin: Some(crate::serve::ToolObservationOrigin::Direct),
        client_kind: Some(cua_driver_core::daemon::DaemonClientKind::Cli),
    };
    crate::serve::send_request(socket_path, &request)
        .ok()?
        .result
}

/// `cua-driver recording <start|stop|status>` — wrapper around
/// `start_recording` / `stop_recording` / `get_recording_state` tools
/// on the running daemon.
///
/// Requires a running daemon (`cua-driver serve`) because recording
/// state lives in the daemon.
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
    require_compatible_daemon(&socket_path);

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
                client_kind: Some(cua_driver_core::daemon::DaemonClientKind::Cli),
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
                        client_kind: Some(cua_driver_core::daemon::DaemonClientKind::Cli),
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
                client_kind: Some(cua_driver_core::daemon::DaemonClientKind::Cli),
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
                client_kind: Some(cua_driver_core::daemon::DaemonClientKind::Cli),
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
    let input_dir = match positionals.first() {
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
    if apply && crate::bundle::is_local_installation() {
        eprintln!(
            "cua-driver-local is managed by scripts/install-local.sh (or install-local.ps1); \
             refusing to run the release installer from the local product."
        );
        process::exit(2);
    }
    let apply_started_at = std::time::Instant::now();
    let daemon_was_running = apply && crate::updater::daemon_is_running();
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
            crate::version_check::capture_update_state(
                &state,
                crate::telemetry::UpdateCheckSource::Cli,
            );
            return;
        }
    }

    let current = env!("CARGO_PKG_VERSION");
    let selected_channel = crate::release_channel::selected().unwrap_or_else(|error| {
        eprintln!("Cannot read release channel: {error}");
        eprintln!(
            "Repair it with `cua-driver channel set stable` or `cua-driver channel set nightly`."
        );
        process::exit(1);
    });
    let current_channel = crate::release_channel::ReleaseChannel::from_version(current);
    if !json {
        println!("Current version: {current}");
        println!("Checking for updates…");
    }

    let latest = crate::version_check::fetch_latest_version();
    match latest {
        Err(e) => {
            crate::telemetry::capture_update_checked(
                crate::telemetry::UpdateCheckSource::Cli,
                crate::telemetry::UpdateCheckOutcome::Unavailable,
                None,
                false,
            );
            if apply {
                crate::telemetry::capture_update_apply_completed(
                    None,
                    crate::telemetry::UpdateApplyOutcome::Failed,
                    crate::telemetry::UpdateFailureClass::CheckFailed,
                    daemon_was_running,
                    apply_started_at.elapsed(),
                );
            }
            // The shared helper returns a human-readable error string for
            // the CLI surface — pass it through so the user can see why
            // (timeout, parse error, etc.) instead of just "unreachable".
            tracing::debug!(target: "cua_driver::update", "fetch failed: {e}");
            if !json {
                println!("Could not reach GitHub — check your connection and try again.");
            }
            process::exit(1);
        }
        Ok(v)
            if !crate::version_check::update_is_available(
                &v,
                current,
                current_channel,
                selected_channel,
            ) =>
        {
            crate::telemetry::capture_update_checked(
                crate::telemetry::UpdateCheckSource::Cli,
                crate::telemetry::UpdateCheckOutcome::UpToDate,
                Some(&v),
                false,
            );
            if apply {
                crate::telemetry::capture_update_apply_completed(
                    Some(&v),
                    crate::telemetry::UpdateApplyOutcome::AlreadyCurrent,
                    crate::telemetry::UpdateFailureClass::None,
                    daemon_was_running,
                    apply_started_at.elapsed(),
                );
            }
            if !json {
                println!("Already up to date.");
            }
        }
        Ok(v) => {
            crate::telemetry::capture_update_checked(
                crate::telemetry::UpdateCheckSource::Cli,
                crate::telemetry::UpdateCheckOutcome::Available,
                Some(&v),
                false,
            );
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
            crate::telemetry::capture_update_apply_started(&v, daemon_was_running);
            match crate::updater::run_install_script(&v) {
                Ok(s) if s.success() => {
                    crate::telemetry::capture_update_apply_completed(
                        Some(&v),
                        crate::telemetry::UpdateApplyOutcome::Installed,
                        crate::telemetry::UpdateFailureClass::None,
                        daemon_was_running,
                        apply_started_at.elapsed(),
                    );
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
                    crate::telemetry::capture_update_apply_completed(
                        Some(&v),
                        crate::telemetry::UpdateApplyOutcome::Failed,
                        crate::telemetry::UpdateFailureClass::InstallerExit,
                        daemon_was_running,
                        apply_started_at.elapsed(),
                    );
                    eprintln!(
                        "Installation failed (exit {}). Re-run install manually:",
                        s.code().unwrap_or(1)
                    );
                    eprintln!("  {}", crate::updater::manual_install_one_liner());
                    process::exit(s.code().unwrap_or(1));
                }
                Err(e) => {
                    crate::telemetry::capture_update_apply_completed(
                        Some(&v),
                        crate::telemetry::UpdateApplyOutcome::Failed,
                        crate::telemetry::UpdateFailureClass::InstallerLaunch,
                        daemon_was_running,
                        apply_started_at.elapsed(),
                    );
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
pub fn run_permissions_cmd(subcommand: &str, json: bool) {
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
    let cli_name = crate::bundle::cli_name();
    let app_name = crate::bundle::app_name();
    let bundle_id = crate::bundle::bundle_id();

    // Only a listening daemon can answer for com.trycua.driver. A failed/!ok
    // response (e.g. daemon still inside its first-launch permission gate) is
    // treated the same as "no daemon" → unknown.
    let daemon_status: Option<serde_json::Value> = if crate::serve::is_daemon_listening(&socket) {
        let req = crate::serve::DaemonRequest {
            method: "call".into(),
            name: Some("check_permissions".into()),
            args: Some(serde_json::json!({ "prompt": false })),
            session_id: None,
            observation_origin: Some(crate::serve::ToolObservationOrigin::Direct),
            client_kind: Some(cua_driver_core::daemon::DaemonClientKind::Cli),
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
                "reason": format!("no {app_name} daemon is running under the driver's own identity \
                           ({bundle_id}), so its real TCC status can't be read from this \
                           process. Run `{cli_name} permissions grant` to grant + verify."),
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
            "No {app_name} daemon is running under the driver's own identity ({bundle_id}), \
             so its real TCC status can't be read."
        );
        println!(
            "(A status check from this terminal would report the terminal's grants, not the \
             driver's.)"
        );
        println!("  → Run `{cli_name} permissions grant` to grant + verify, or start the daemon");
        println!("    (`open -n -g -a {app_name} --args serve`) and re-run this command.");
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
    let cap = structured
        .get("screen_recording_capturable")
        .and_then(|v| v.as_bool());
    let direct_capture_verification = structured.get("direct_capture_verification");
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
    match cap {
        Some(true) => println!("Direct Capture:     ✅ ready"),
        Some(false) => {
            println!("Direct Capture:     ❌ unavailable");
            if sr {
                println!(
                    "  ⚠️  preflight reports granted, but the explicit live capture probe failed."
                );
            }
        }
        None => {
            if let Some(verification) = direct_capture_verification {
                let source = verification["source"].as_str().unwrap_or("unknown source");
                let verified_at = verification["verified_at"]
                    .as_str()
                    .unwrap_or("unknown time");
                let bundle_id = verification["bundle_id"]
                    .as_str()
                    .unwrap_or("unknown identity");
                println!(
                    "Direct Capture:     ✅ previously verified ({source}, {verified_at}, {bundle_id})"
                );
                println!(
                    "  ℹ️  historical observation; this read-only status did not run a live probe."
                );
            } else {
                println!(
                    "Direct Capture:     ❓ not checked (status is read-only; run `{cli_name} permissions grant`)"
                );
            }
        }
    }
    println!("Source: {attribution}");
    if !(ax && sr) {
        println!("  → To grant for the driver, run: {cli_name} permissions grant");
    }
}

fn permission_flag(structured: &serde_json::Value, key: &str) -> bool {
    structured
        .get(key)
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

fn permission_grant_is_ready(structured: &serde_json::Value) -> bool {
    permission_flag(structured, "accessibility")
        && permission_flag(structured, "screen_recording")
        && permission_flag(structured, "screen_recording_capturable")
        && structured
            .get("direct_capture_verification_error")
            .is_none()
        && structured
            .get("direct_capture_verification")
            .is_some_and(serde_json::Value::is_object)
}

fn permission_grant_needs_direct_capture(structured: &serde_json::Value) -> bool {
    permission_flag(structured, "screen_recording")
        && !permission_flag(structured, "screen_recording_capturable")
}

fn permission_status_request() -> crate::serve::DaemonRequest {
    crate::serve::DaemonRequest {
        method: "call".into(),
        name: Some("check_permissions".into()),
        args: Some(serde_json::json!({
            "prompt": false,
            "probe_direct_capture": false,
        })),
        session_id: None,
        observation_origin: Some(crate::serve::ToolObservationOrigin::Direct),
        client_kind: Some(cua_driver_core::daemon::DaemonClientKind::Cli),
    }
}

/// Handle the private LaunchServices child used by `permissions grant`.
///
/// This runs before ordinary CLI wrapping and never opens a daemon socket. The
/// app bundle asks macOS for the grants under its own responsible-process
/// identity, and macOS remains the only surface that can approve them.
#[cfg(target_os = "macos")]
pub fn run_permissions_host_request_if_requested() -> Option<i32> {
    use std::io::Write as _;
    use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};

    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.first().map(String::as_str) != Some(platform_macos::tools::PERMISSIONS_HOST_REQUEST_ARG)
    {
        return None;
    }
    if !crate::bundle::is_executable_inside_cuadriver_app() {
        eprintln!("permission host request requires the installed CuaDriver app bundle");
        return Some(77);
    }
    let result_file = args
        .windows(2)
        .find(|pair| pair[0] == "--result-file")
        .map(|pair| pair[1].clone());
    let Some(result_file) = result_file else {
        eprintln!("permission host request omitted --result-file");
        return Some(64);
    };
    let result_path = std::path::Path::new(&result_file);
    let expected_parent = std::fs::canonicalize(std::env::temp_dir()).ok();
    let actual_parent = result_path.parent().and_then(|parent| {
        std::fs::canonicalize(parent)
            .ok()
            .or_else(|| Some(parent.to_path_buf()))
    });
    let valid_name = result_path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("cua-driver-permissions-") && name.ends_with(".json"));
    if !valid_name || expected_parent != actual_parent {
        eprintln!("permission host result path is outside the private temporary-file namespace");
        return Some(64);
    }
    let probe_direct_capture = args.iter().any(|arg| arg == "--probe-direct-capture");
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("permission host runtime failed: {error}");
            return Some(70);
        }
    };
    let result = runtime.block_on(
        platform_macos::tools::request_permissions_from_launchservices_host(probe_direct_capture),
    );
    let payload = match serde_json::to_vec(&result) {
        Ok(payload) => payload,
        Err(error) => {
            eprintln!("permission host result serialization failed: {error}");
            return Some(70);
        }
    };
    let write_result = std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(result_path)
        .and_then(|mut file| {
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
            file.write_all(&payload)
        });
    match write_result {
        Ok(()) => Some(0),
        Err(error) => {
            eprintln!("permission host result write failed: {error}");
            Some(74)
        }
    }
}

#[cfg(target_os = "macos")]
fn request_permissions_via_launchservices(
    probe_direct_capture: bool,
) -> Result<serde_json::Value, String> {
    use std::fs::OpenOptions;
    use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
    use std::process::{Command as ProcessCommand, Stdio};
    use std::time::{SystemTime, UNIX_EPOCH};

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("system clock unavailable: {error}"))?
        .as_nanos();
    let result_file = std::env::temp_dir().join(format!(
        "cua-driver-permissions-{}-{nonce}.json",
        std::process::id()
    ));
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&result_file)
        .map_err(|error| format!("create permission result file: {error}"))?;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("secure permission result file: {error}"))?;

    let app_name = crate::bundle::app_name();
    let app_path = crate::bundle::app_bundle_path();
    let mut args = vec![
        "-n".to_owned(),
        "-W".to_owned(),
        "-g".to_owned(),
        app_path.to_owned(),
        "--args".to_owned(),
        platform_macos::tools::PERMISSIONS_HOST_REQUEST_ARG.to_owned(),
        "--result-file".to_owned(),
        result_file.to_string_lossy().into_owned(),
    ];
    if probe_direct_capture {
        args.push("--probe-direct-capture".to_owned());
    }
    let mut child = ProcessCommand::new("/usr/bin/open")
        .args(&args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("launch {app_name} permission host: {error}"))?;
    let status = {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(180);
        loop {
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break Err(format!(
                        "{app_name} permission host timed out after 180 seconds"
                    ));
                }
                Err(error) => break Err(format!("wait for {app_name} permission host: {error}")),
            }
        }
    };
    let payload = status.and_then(|status| {
        if !status.success() {
            return Err(format!(
                "{app_name} permission host exited with {:?}",
                status.code()
            ));
        }
        std::fs::read(&result_file).map_err(|error| format!("read permission host result: {error}"))
    });
    let _ = std::fs::remove_file(&result_file);
    let payload = payload?;
    let result: serde_json::Value = serde_json::from_slice(&payload)
        .map_err(|error| format!("parse permission host result: {error}"))?;
    result
        .get("structuredContent")
        .cloned()
        .ok_or_else(|| "permission host returned no structured status".to_owned())
}

/// Launch CuaDriver via LaunchServices so the permission prompt attributes to
/// com.trycua.driver, wait (user-paced) for the daemon to come up — its socket
/// only appears once the permissions gate passes, i.e. the grant was given —
/// then report the driver's own status.
fn run_permissions_grant() {
    #[cfg(target_os = "macos")]
    {
        let cli_name = crate::bundle::cli_name();
        let app_name = crate::bundle::app_name();
        let app_path = crate::bundle::app_bundle_path();
        let bundle_id = crate::bundle::bundle_id();
        let socket = crate::serve::default_socket_path();
        let daemon_already_running = crate::serve::is_daemon_listening(&socket);
        if daemon_already_running {
            println!("{app_name} daemon already running — checking its permissions…");
            println!(
                "Requesting any missing Accessibility and Screen Recording grants now. \
                 macOS may show a prompt or add {app_name} to System Settings."
            );
        } else {
            println!("Launching {app_name} to request permissions.");
            println!(
                "A dialog for {app_name} will appear — approve Accessibility \
                 and Screen Recording in System Settings, then this command continues."
            );
            // Preserve explicit Computer History admission across the
            // permission host's daemon launch cycle.
            if let Err(e) = launch_daemon_and_wait(
                &socket,
                180,
                false,
                &[],
                crate::history_runtime::preview_admitted_preference(),
            ) {
                eprintln!("\nDidn't detect the {app_name} daemon: {e}");
                eprintln!(
                    "If you haven't yet, grant Accessibility + Screen Recording to {app_name} \
                     in System Settings, then re-run `{cli_name} permissions grant`."
                );
                process::exit(1);
            }
        }
        // Since #1761 the daemon binds its socket IMMEDIATELY — before the
        // permissions gate completes — so the first `check_permissions`
        // query returns "pending" while the grant is still missing. First poll
        // only the non-prompting TCC preflight booleans. Direct
        // ScreenCaptureKit access has its own Tahoe consent and is requested
        // explicitly below, after we explain the system dialog.
        //
        // The gate uses short-lived probes because `AXIsProcessTrusted` is
        // cached per process. While those probes are pending, the stable daemon
        // rejects tool calls with a retryable response; tolerate that state
        // rather than bailing on the first non-success response.
        let req = permission_status_request();
        // A dedicated LaunchServices child requests the grants under the
        // CuaDriver app identity. No prompt-capable method exists on the
        // agent-reachable daemon socket.
        let staged_status = request_permissions_via_launchservices(false).ok();
        let poll_deadline = std::time::Instant::now() + std::time::Duration::from_secs(180);
        let mut ax = staged_status
            .as_ref()
            .is_some_and(|status| permission_flag(status, "accessibility"));
        let mut sr = staged_status
            .as_ref()
            .is_some_and(|status| permission_flag(status, "screen_recording"));
        loop {
            if let Some(structured) = crate::serve::send_request(&socket, &req)
                .ok()
                .filter(|r| r.ok)
                .and_then(|r| r.result)
                .and_then(|res| res.get("structuredContent").cloned())
            {
                ax = permission_flag(&structured, "accessibility");
                sr = permission_flag(&structured, "screen_recording");
                if ax && sr {
                    break;
                }
            }
            // `send_request` returning None / !ok means the daemon is still
            // gated or briefly unavailable — keep polling.
            if std::time::Instant::now() >= poll_deadline {
                break;
            }
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
        if !(ax && sr) {
            let missing = match (ax, sr) {
                (false, false) => "Accessibility + Screen Recording",
                (false, true) => "Accessibility",
                (true, false) => "Screen Recording",
                (true, true) => unreachable!(),
            };
            println!("\n⚠️  Timed out waiting on: {missing}.");
            println!(
                "Approve {app_name} in System Settings \u{2192} Privacy & Security, then \
                 re-run `{cli_name} permissions grant`."
            );
            if !sr {
                println!(
                    "If {app_name} is missing from Screen & System Audio Recording, click +, \
                     add {app_path}, enable it, then re-run the command."
                );
            }
            process::exit(1);
        }

        println!("\nAccessibility and Screen Recording are granted.");
        println!(
            "macOS may now ask {app_name} to bypass the system private window picker and \
             directly access your screen and audio. This is the expected consent for exact \
             screenshots and recordings without a picker. Cua Driver's current recorder \
             captures screen video only; it does not enable system-audio capture. This \
             consent does not authorize browser profiles, browser data, or CDP attachment."
        );
        println!("Choose Allow to request and verify direct capture now…");

        let direct_status = request_permissions_via_launchservices(true).ok();
        if let Some((status, error)) = direct_status.as_ref().and_then(|status| {
            status
                .get("direct_capture_verification_error")
                .and_then(|error| error.get("message"))
                .and_then(serde_json::Value::as_str)
                .map(|error| (status, error))
        }) {
            if permission_flag(status, "screen_recording_capturable") {
                eprintln!(
                    "\n❌ Direct capture worked, but its verification could not be recorded: {error}"
                );
            } else {
                eprintln!(
                    "\n❌ Direct capture failed, and the previous verification could not be cleared: {error}"
                );
            }
            process::exit(1);
        }

        if direct_status
            .as_ref()
            .is_some_and(permission_grant_is_ready)
        {
            println!(
                "\n✅ {app_name} has Accessibility, Screen Recording, and direct capture access. You're set."
            );
            println!(
                "macOS verified the explicit request but does not report whether consent was newly granted or already present."
            );
            return;
        }

        eprintln!("\n❌ {app_name} still cannot use direct ScreenCaptureKit capture.");
        if direct_status
            .as_ref()
            .is_some_and(permission_grant_needs_direct_capture)
        {
            eprintln!(
                "Screen Recording is granted, but the private-window-picker bypass consent \
                 was denied or the live probe failed."
            );
        }
        eprintln!(
            "In System Settings \u{2192} Privacy & Security \u{2192} Screen & System Audio Recording, \
             allow {app_name} ({bundle_id}), then re-run `{cli_name} permissions grant`."
        );
        eprintln!("If it is already allowed, reset the stale decision and retry:");
        eprintln!("  tccutil reset ScreenCapture {bundle_id}");
        eprintln!("  {cli_name} stop");
        eprintln!("  {cli_name} permissions grant");
        process::exit(1);
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
    crate::version_check::capture_update_state(&state, crate::telemetry::UpdateCheckSource::Cli);

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

/// Inspect or persist the release channel. Selection never installs by itself;
/// replacement remains explicit through `cua-driver update --apply`.
pub fn run_channel_cmd(subcommand: &str, value: Option<&str>, json: bool) {
    let result = match subcommand {
        "status" => crate::release_channel::selected(),
        "set" => {
            let channel = value
                .unwrap_or_default()
                .parse::<crate::release_channel::ReleaseChannel>()
                .unwrap_or_else(|error| {
                    eprintln!("{error}");
                    process::exit(64);
                });
            if let Err(error) = crate::release_channel::set(channel) {
                eprintln!("Failed to save release channel: {error}");
                process::exit(1);
            }
            Ok(channel)
        }
        _ => unreachable!("validated by parse_command"),
    };

    let selected = result.unwrap_or_else(|error| {
        eprintln!("Failed to read release channel: {error}");
        eprintln!(
            "Repair it with `cua-driver channel set stable` or `cua-driver channel set nightly`."
        );
        process::exit(1);
    });
    let current = crate::release_channel::ReleaseChannel::from_version(env!("CARGO_PKG_VERSION"));

    if json {
        println!(
            "{}",
            serde_json::json!({
                "selected_channel": selected.as_str(),
                "current_channel": current.map(|channel| channel.as_str()),
                "current_version": env!("CARGO_PKG_VERSION"),
            })
        );
    } else {
        println!("Selected channel: {selected}");
        match current {
            Some(channel) => println!("Current channel:  {channel}"),
            None => println!("Current channel:  development"),
        }
        if subcommand == "set" && current != Some(selected) {
            println!("Run `cua-driver update --apply` to install the latest {selected} release.");
        }
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
                "discussion": "On Windows and Linux, bare cua-driver mcp owns its runtime directly and shuts it down on stdin EOF. On macOS it proxies to CuaDriver.app so desktop permissions retain the app identity. Pass --direct to make the macOS MCP process own the runtime and TCC attribution, or --socket to select an explicit daemon endpoint.",
                "arguments": no_args,
                "options": [
                    {"name":"socket","short_name":null,"help":"Select an explicit daemon socket or named-pipe endpoint.","type":"String","default_value":null,"is_optional":true},
                    {"name":"host-bundle-id","short_name":null,"help":"Advisory host bundle id label echoed in check_permissions output (embedded mode).","type":"String","default_value":null,"is_optional":true},
                    {"name":"cursor-theme","short_name":null,"help":"Select an installed cursor theme id.","type":"String","default_value":"cua.default","is_optional":true},
                    {"name":"cursor-reduced-motion","short_name":null,"help":"Follow the OS setting, force still frames, or allow animation: auto, on, or off.","type":"String","default_value":"auto","is_optional":true},
                    {"name":"grant","short_name":null,"help":"Pre-authorize a residual standard-mode boundary for a newly launched runtime. Repeatable; supported value: existing-profile.","type":"String","default_value":null,"is_optional":true,"is_repeatable":true}
                ],
                "flags": [
                    {"name":"direct","short_name":null,"help":"Own the runtime in this MCP process; mutually exclusive with --socket.","default_value":false},
                    {"name":"claude-code-computer-use-compat","short_name":null,"help":"Accepted for older Claude Code setup snippets; no standalone screenshot tool — use get_window_state for window screenshots.","default_value":false},
                    {"name":"embedded","short_name":null,"help":"Declare embedding-host mode. Without --direct, require the host's private service through --socket instead of auto-launching the standalone app.","default_value":false}
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
                "abstract": "Invoke an MCP tool through the running daemon.",
                "discussion": "Requires a Cua Driver daemon. JSON arguments may be passed as a positional JSON object or through stdin.",
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
                    {"name":"permission-mode","short_name":null,"help":"Immutable agent authorization mode: standard, bounded, or unrestricted.","type":"String","default_value":"standard","is_optional":true},
                    {"name":"grant","short_name":null,"help":"Pre-authorize a residual standard-mode boundary. Repeatable; supported value: existing-profile.","type":"String","default_value":null,"is_optional":true,"is_repeatable":true},
                    {"name":"capability-manifest","short_name":null,"help":"Optional narrow-only tool/resource ceiling; required in bounded mode.","type":"String","default_value":null,"is_optional":true},
                    {"name":"session-policy","short_name":null,"help":"Deprecated alias for capability-manifest.","type":"String","default_value":null,"is_optional":true},
                    {"name":"host-bundle-id","short_name":null,"help":"Advisory host bundle id label echoed in check_permissions output (embedded mode).","type":"String","default_value":null,"is_optional":true}
                ],
                "flags": [
                    {"name":"dangerously-bypass-approvals","short_name":null,"help":"Select unrestricted mode and acknowledge its risk.","default_value":false},
                    {"name":"approve-capability-manifest","short_name":null,"help":"Trusted-launcher confirmation that the exact capability manifest was reviewed.","default_value":false},
                    {"name":"approve-session-policy","short_name":null,"help":"Deprecated alias for approve-capability-manifest.","default_value":false},
                    {"name":"no-permissions-gate","short_name":null,"help":"Skip the macOS first-launch permissions gate.","default_value":false},
                    {"name":"embedded","short_name":null,"help":"Run embedded inside a host app: inherit the host's TCC grants, never prompt or relaunch. Also CUA_DRIVER_EMBEDDED=1.","default_value":false},
                    {"name":"no-overlay","short_name":null,"help":"Disable the agent cursor overlay for this daemon.","default_value":false}
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
                "name": "revoke",
                "abstract": "Revoke one or all live authorization/session scopes.",
                "discussion": "Revocation is deny-only and never accepts an approval token.",
                "arguments": no_args,
                "options": [
                    {"name":"session","short_name":null,"help":"Exact session id to stop and revoke.","type":"String","default_value":null,"is_optional":true},
                    {"name":"socket","short_name":null,"help":"Override the daemon socket or named-pipe path.","type":"String","default_value":null,"is_optional":true}
                ],
                "flags": [
                    {"name":"all","short_name":null,"help":"Stop and revoke every live session.","default_value":false}
                ],
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
                "abstract": "Print client-specific connection guidance (MCP config where supported).",
                "discussion": "Supported clients include claude, codex, cursor, antigravity, openclaw, opencode, hermes, pi, prime-agent, qwen, droid, and zcode.",
                "arguments": no_args,
                "options": [{"name":"client","short_name":null,"help":"Client name to print configuration for.","type":"String","default_value":null,"is_optional":true}],
                "flags": no_flags,
                "subcommands": no_subcommands
            },
            {
                "name": "recording",
                "abstract": "Control trajectory recording on a running daemon.",
                "discussion": "Recording state lives in the required daemon and survives client reconnects.",
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
                "name": "channel",
                "abstract": "Inspect or change the stable/nightly update channel.",
                "discussion": "Selection is persistent but never installs by itself; use cua-driver update --apply after changing it.",
                "arguments": no_args,
                "options": no_options,
                "flags": no_flags,
                "subcommands": [
                    {"name":"status","abstract":"Show selected and current release channels.","discussion":"","arguments":[],"options":[],"flags":[{"name":"json","short_name":null,"help":"Emit machine-readable channel state.","default_value":false}],"subcommands":[]},
                    {"name":"set","abstract":"Save stable or nightly as the update channel.","discussion":"","arguments":[{"name":"channel","help":"stable or nightly","type":"String","is_optional":false}],"options":[],"flags":[{"name":"json","short_name":null,"help":"Emit machine-readable channel state.","default_value":false}],"subcommands":[]}
                ]
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
                    {"name":"status","abstract":"Print whether autostart is registered and running.","discussion":"`not-registered` is emitted only when Task Scheduler explicitly reports that the named task does not exist. If the task cannot be inspected, the command exits non-zero and reports `permission-denied` or `unknown` together with the original diagnostic.","arguments":[],"options":[],"flags":[],"subcommands":[]},
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
                "name": "manifest",
                "abstract": "Emit a stable JSON description of the CLI surface.",
                "discussion": "Consumers can use this instead of hardcoding launch arguments such as the MCP invocation.",
                "arguments": no_args,
                "options": no_options,
                "flags": [{"name":"pretty","short_name":"p","help":"Pretty-print JSON.","default_value":false}],
                "subcommands": no_subcommands
            },
            {
                "name": "cursor-theme",
                "abstract": "Validate, compile, inspect, preview, install, or remove a local cursor theme.",
                "discussion": "This is a trusted local authoring workflow. Agent-facing tools may select an installed theme id, but cannot install source or compiled theme data.",
                "arguments": no_args,
                "options": no_options,
                "flags": no_flags,
                "subcommands": [
                    {"name":"validate","abstract":"Validate a bounded dotLottie source archive.","discussion":"","arguments":[{"name":"source","help":"Path to the source .lottie archive.","type":"String","is_optional":false}],"options":[],"flags":[{"name":"development","short_name":null,"help":"Allow the reserved com.example development namespace.","default_value":false}],"subcommands":[]},
                    {"name":"build","abstract":"Compile a validated dotLottie archive into a bounded .cua-theme artifact.","discussion":"","arguments":[{"name":"source","help":"Path to the source .lottie archive.","type":"String","is_optional":false}],"options":[{"name":"output","short_name":null,"help":"Output .cua-theme path.","type":"String","default_value":null,"is_optional":false}],"flags":[{"name":"development","short_name":null,"help":"Allow the reserved com.example development namespace.","default_value":false}],"subcommands":[]},
                    {"name":"inspect","abstract":"Inspect metadata in a compiled .cua-theme artifact.","discussion":"","arguments":[{"name":"theme","help":"Path to the compiled .cua-theme artifact.","type":"String","is_optional":false}],"options":[],"flags":[{"name":"json","short_name":null,"help":"Emit machine-readable JSON.","default_value":false}],"subcommands":[]},
                    {"name":"preview","abstract":"Render a compiled theme's representative still frames to a directory.","discussion":"","arguments":[{"name":"theme","help":"Path to the compiled .cua-theme artifact.","type":"String","is_optional":false}],"options":[{"name":"output","short_name":null,"help":"Preview output directory.","type":"String","default_value":null,"is_optional":false}],"flags":[],"subcommands":[]},
                    {"name":"install","abstract":"Install a compiled theme into the current user's theme store.","discussion":"","arguments":[{"name":"theme","help":"Path to the compiled .cua-theme artifact.","type":"String","is_optional":false}],"options":[],"flags":[],"subcommands":[]},
                    {"name":"list","abstract":"List the built-in and installed cursor themes.","discussion":"","arguments":[],"options":[],"flags":[{"name":"json","short_name":null,"help":"Emit machine-readable JSON.","default_value":false}],"subcommands":[]},
                    {"name":"uninstall","abstract":"Remove a custom theme from the current user's theme store.","discussion":"The built-in cua.default theme cannot be removed.","arguments":[{"name":"theme-id","help":"Installed custom theme id.","type":"String","is_optional":false}],"options":[],"flags":[],"subcommands":[]}
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
pub fn run_dump_docs_with_type(tools_list: &serde_json::Value, pretty: bool, doc_type: &str) {
    // Each MCP tool: `{name, description, input_schema}` (Swift's MCPToolDoc
    // shape — Rust adds read_only/destructive/idempotent as intentional
    // extras).
    let tools: Vec<serde_json::Value> = tools_list
        .get("tools")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .map(|tool| {
            let annotations = tool.get("annotations").unwrap_or(&serde_json::Value::Null);
            serde_json::json!({
                "name":         tool.get("name").cloned().unwrap_or(serde_json::Value::Null),
                "description":  tool.get("description").cloned().unwrap_or(serde_json::Value::String(String::new())),
                "input_schema": tool.get("inputSchema").cloned().unwrap_or_else(|| serde_json::json!({"type": "object"})),
                "read_only":    annotations.get("readOnlyHint").cloned().unwrap_or(serde_json::Value::Bool(false)),
                "destructive":  annotations.get("destructiveHint").cloned().unwrap_or(serde_json::Value::Bool(false)),
                "idempotent":   annotations.get("idempotentHint").cloned().unwrap_or(serde_json::Value::Bool(false)),
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
pub fn run_diagnose_cmd() {
    let sections = [
        diagnose_runtime_section(),
        diagnose_signature_section(),
        diagnose_tcc_section(),
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

fn diagnose_tcc_section() -> String {
    let socket = crate::serve::default_socket_path();
    let status = crate::serve::is_daemon_listening(&socket)
        .then(|| crate::serve::DaemonRequest {
            method: "call".into(),
            name: Some("check_permissions".into()),
            args: Some(serde_json::json!({ "prompt": false })),
            session_id: None,
            observation_origin: Some(crate::serve::ToolObservationOrigin::Direct),
            client_kind: Some(cua_driver_core::daemon::DaemonClientKind::Cli),
        })
        .and_then(|request| crate::serve::send_request(&socket, &request).ok())
        .filter(|response| response.ok)
        .and_then(|response| response.result)
        .and_then(|result| result.get("structuredContent").cloned());
    let display = |key: &str| {
        status
            .as_ref()
            .and_then(|value| value.get(key))
            .and_then(serde_json::Value::as_bool)
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unknown (daemon unavailable)".to_owned())
    };
    let ax = display("accessibility");
    let sr = display("screen_recording");
    let direct = status
        .as_ref()
        .and_then(|value| value.get("direct_capture_status"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown (daemon unavailable)");

    format!(
        "## tcc probes (daemon)\n\
         accessibility     (AXIsProcessTrusted): {ax}\n\
         screen recording  (CGPreflightScreenCaptureAccess): {sr}\n\
         direct capture    (prompt-capable probe): {direct}\n\n\
         diagnose is read-only and never runs the direct ScreenCaptureKit probe;\n\
         use `cua-driver permissions grant` to request and verify it explicitly."
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
        (
            "user data dir",
            format!("{home}/{}", crate::bundle::user_home_subdirectory()),
        ),
        (
            "config cache",
            format!("{home}/Library/Caches/{}", crate::bundle::state_namespace()),
        ),
        (
            "telemetry id",
            format!(
                "{home}/{}/.telemetry_id",
                crate::bundle::user_home_subdirectory()
            ),
        ),
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

/// `cua-driver config [show|get|set|reset] [key] [value]`
///
/// Thin daemon-only wrapper around the `get_config` / `set_config` tools.
pub fn run_config_cmd(
    subcommand: Option<&str>,
    key: Option<&str>,
    value: Option<&str>,
    socket: Option<&str>,
) {
    let socket_path = socket
        .map(str::to_owned)
        .unwrap_or_else(crate::serve::default_socket_path);

    if !crate::serve::is_daemon_listening(&socket_path) {
        eprintln!(
            "Cua Driver daemon is not running on {socket_path}.\n\
             Start it first with: cua-driver serve --socket {socket_path}"
        );
        process::exit(1);
    }
    require_compatible_daemon(&socket_path);

    let call = |tool: &str, args: serde_json::Value| -> serde_json::Value {
        let req = crate::serve::DaemonRequest {
            method: "call".into(),
            name: Some(tool.to_owned()),
            args: Some(args),
            session_id: None,
            observation_origin: Some(crate::serve::ToolObservationOrigin::Direct),
            client_kind: Some(cua_driver_core::daemon::DaemonClientKind::Cli),
        };
        let response = crate::serve::send_request(&socket_path, &req).unwrap_or_else(|error| {
            eprintln!("Cua Driver daemon request on {socket_path} failed: {error}");
            process::exit(1);
        });
        if !response.ok {
            eprintln!(
                "{}",
                response
                    .error
                    .unwrap_or_else(|| format!("daemon rejected {tool}"))
            );
            process::exit(response.exit_code.unwrap_or(1));
        }
        response.result.unwrap_or_else(|| {
            eprintln!("{tool}: daemon returned no result");
            process::exit(1);
        })
    };

    let get_config = || -> serde_json::Value {
        let result = call("get_config", serde_json::json!({}));
        result.get("structuredContent").cloned().unwrap_or_else(|| {
            eprintln!("get_config: no structured content returned");
            process::exit(1);
        })
    };

    match subcommand.unwrap_or("show") {
        "show" | "" => {
            let config = get_config();
            println!(
                "{}",
                serde_json::to_string_pretty(&config).unwrap_or_else(|_| config.to_string())
            );
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
                    "config key 'capture_scope' is retired; select a window or desktop target on each action"
                );
                process::exit(64);
            }
            let config = get_config();
            // Support dotted key paths like "agent_cursor.enabled".
            let v = if key.contains('.') {
                let (parent, child) = key.split_once('.').unwrap();

                config
                    .get(parent)
                    .and_then(|object| object.get(child))
                    .cloned()
            } else {
                config.get(key).cloned()
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
                    "config key 'capture_scope' is retired; select a window or desktop target on each action"
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
            call("set_config", serde_json::json!({ key: parsed_value }));
            println!("Config updated.");
            let config = get_config();
            println!(
                "{}",
                serde_json::to_string_pretty(&config).unwrap_or_else(|_| config.to_string())
            );
        }

        "reset" => {
            // Reset to defaults by calling set_config with empty args.
            // The set_config tool keeps existing values if keys are absent,
            // so we send the known defaults explicitly.
            let defaults = serde_json::json!({
                "capture_mode": "ax",
                "max_image_dimension": 0
            });
            call("set_config", defaults);
            println!("Config reset to defaults.");
            let config = get_config();
            println!(
                "{}",
                serde_json::to_string_pretty(&config).unwrap_or_else(|_| config.to_string())
            );
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
    fn history_relaunch_preserves_authorization_mode_and_every_grant() {
        let grants = args(&["capability:a", "capability:b"]);
        let state = crate::history_runtime::DaemonLaunchState {
            permission_mode: Some("bounded".to_owned()),
            dangerously_bypass_approvals: false,
            capability_manifest: Some("/tmp/capabilities.yaml".to_owned()),
            approve_capability_manifest: true,
            no_permissions_gate: true,
            claude_code_compat: true,
            grants,
        };
        #[cfg(target_os = "macos")]
        let launch = daemon_launch_arguments("CuaDriver", "/tmp/history-test.sock", &state, true);
        #[cfg(not(target_os = "macos"))]
        let launch = daemon_process_arguments("history-test.sock", &state, true);
        assert!(launch
            .windows(2)
            .any(|pair| pair == ["--permission-mode", "bounded"]));
        assert!(launch
            .windows(2)
            .any(|pair| { pair == ["--capability-manifest", "/tmp/capabilities.yaml"] }));
        assert!(launch.contains(&"--approve-capability-manifest".to_owned()));
        assert!(launch.contains(&"--no-permissions-gate".to_owned()));
        assert!(launch.contains(&"--claude-code-computer-use-compat".to_owned()));
        assert!(launch.contains(&"--experimental-history".to_owned()));
        assert!(launch
            .windows(2)
            .any(|pair| pair == ["--grant", "capability:a"]));
        assert!(launch
            .windows(2)
            .any(|pair| pair == ["--grant", "capability:b"]));
        #[cfg(target_os = "macos")]
        assert!(launch
            .windows(2)
            .any(|pair| pair == ["--socket", "/tmp/history-test.sock"]));
    }

    #[test]
    fn history_relaunch_preserves_explicit_unrestricted_approval() {
        let state = crate::history_runtime::DaemonLaunchState {
            permission_mode: Some("unrestricted".to_owned()),
            dangerously_bypass_approvals: true,
            ..Default::default()
        };
        let mut launch = vec!["serve".to_owned()];
        append_daemon_launch_state(&mut launch, &state, true);
        assert!(launch
            .windows(2)
            .any(|pair| { pair == ["--permission-mode", "unrestricted"] }));
        assert!(launch.contains(&"--dangerously-bypass-approvals".to_owned()));
    }

    #[test]
    fn deprecated_session_policy_flag_remains_a_capability_manifest_alias() {
        let argv = args(&["serve", "--session-policy", "/tmp/legacy.yaml"]);
        assert_eq!(
            aliased_flag_value(&argv, "--capability-manifest", "--session-policy"),
            Some("/tmp/legacy.yaml".to_owned())
        );

        let identical = args(&[
            "serve",
            "--capability-manifest=/tmp/shared.yaml",
            "--session-policy",
            "/tmp/shared.yaml",
        ]);
        assert_eq!(
            aliased_flag_value(&identical, "--capability-manifest", "--session-policy"),
            Some("/tmp/shared.yaml".to_owned())
        );
    }

    #[test]
    fn expected_pid_keeps_stop_as_the_subcommand() {
        let argv = args(&["--expected-pid", "42", "stop"]);
        assert_eq!(positional_args(&argv), vec!["stop"]);
        assert_eq!(parse_expected_stop_pid(&argv, Some("stop")), Some(42));

        let with_socket = args(&["--socket", "/tmp/cua.sock", "--expected-pid", "42", "stop"]);
        assert_eq!(positional_args(&with_socket), vec!["stop"]);
        assert_eq!(
            parse_expected_stop_pid(&with_socket, Some("stop")),
            Some(42)
        );
    }

    #[test]
    fn expected_pid_is_absent_for_an_ordinary_stop() {
        let argv = args(&["stop"]);
        assert_eq!(parse_expected_stop_pid(&argv, Some("stop")), None);
    }

    #[test]
    fn expected_pid_does_not_shadow_other_subcommands() {
        let argv = args(&["--expected-pid", "42", "status"]);
        assert_eq!(positional_args(&argv), vec!["status"]);
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
        assert_eq!(finite_operation_from_args(&args(&["sessions"])), "list");
        assert_eq!(
            finite_operation_from_args(&args(&["sessions", "private-value"])),
            "other"
        );
        assert_eq!(
            finite_operation_from_args(&args(&["update", "--apply"])),
            "apply"
        );
        assert_eq!(finite_operation_from_args(&args(&["update"])), "check_only");
        assert_eq!(finite_operation_from_args(&args(&["channel"])), "status");
        assert_eq!(
            finite_operation_from_args(&args(&["channel", "set", "private-value"])),
            "set"
        );
        assert_eq!(finite_operation_from_args(&args(&["history"])), "status");
        assert_eq!(
            finite_operation_from_args(&args(&["history", "show", "private-value"])),
            "show"
        );
        assert_eq!(
            finite_operation_from_args(&args(&["history", "private-value"])),
            "other"
        );
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
            finite_client_kind_from_args(&args(&["mcp-config", "--client", "prime-agent"])),
            "prime_agent"
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
    fn permission_grant_requires_live_capture_probe() {
        let stale = serde_json::json!({
            "accessibility": true,
            "screen_recording": true,
            "screen_recording_capturable": false
        });

        assert!(!permission_grant_is_ready(&stale));
        assert!(permission_grant_needs_direct_capture(&stale));
    }

    #[test]
    fn permission_grant_accepts_all_live_checks() {
        let ready = serde_json::json!({
            "accessibility": true,
            "screen_recording": true,
            "screen_recording_capturable": true,
            "direct_capture_verification": {}
        });

        assert!(permission_grant_is_ready(&ready));
        assert!(!permission_grant_needs_direct_capture(&ready));
    }

    #[test]
    fn permission_grant_rejects_verification_errors_with_stale_evidence() {
        let failed = serde_json::json!({
            "accessibility": true,
            "screen_recording": true,
            "screen_recording_capturable": true,
            "direct_capture_verification": {},
            "direct_capture_verification_error": {
                "code": "direct_capture_verification_store_failed",
                "message": "read-only evidence store"
            }
        });

        assert!(!permission_grant_is_ready(&failed));
    }

    #[test]
    fn permission_grant_missing_live_probe_is_not_ready() {
        let incomplete = serde_json::json!({
            "accessibility": true,
            "screen_recording": true
        });

        assert!(!permission_grant_is_ready(&incomplete));
    }

    #[test]
    fn permission_status_request_is_read_only_and_uses_the_public_tool_route() {
        let status = permission_status_request();
        assert_eq!(status.method, "call");
        assert_eq!(status.name.as_deref(), Some("check_permissions"));
        let args = status.args.expect("status request args");
        assert_eq!(args.get("prompt"), Some(&serde_json::json!(false)));
        assert_eq!(
            args.get("probe_direct_capture"),
            Some(&serde_json::json!(false))
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

        // Explicit build-time capability claims let integrations decide whether
        // a Linux artifact can safely auto-enable native Wayland.
        let features = obj
            .get("features")
            .and_then(|v| v.as_object())
            .expect("features is an object");
        for key in ["wayland_native", "portal_input", "portal_capture"] {
            assert!(
                features.get(key).and_then(|v| v.as_bool()).is_some(),
                "features.{key} must be a boolean"
            );
        }
        assert_eq!(
            features.get("wayland_native").and_then(|v| v.as_bool()),
            Some(cfg!(target_os = "linux"))
        );
        assert_eq!(
            features.get("portal_input").and_then(|v| v.as_bool()),
            Some(cfg!(all(target_os = "linux", feature = "portal-input")))
        );
        assert_eq!(
            features.get("portal_capture").and_then(|v| v.as_bool()),
            Some(cfg!(all(target_os = "linux", feature = "portal-capture")))
        );

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
            "revoke",
            "status",
            "mcp-config",
            "manifest",
            "history",
        ] {
            assert!(names.contains(&need), "missing subcommand '{need}'");
        }
    }

    #[test]
    fn manifest_never_advertises_linux_portal_features_on_non_linux_targets() {
        // Simulate a non-Linux build with both Cargo features enabled. This
        // runs on every CI host, so the exact cross-target regression is
        // covered even when Windows only compiles the broader test suite.
        assert_eq!(
            manifest_feature_flags(false, true, true),
            (false, false, false)
        );
    }

    /// Every subcommand entry has the same JSON shape — name + description
    /// + args[] — so consumers can render the catalog uniformly without
    ///
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
        .split("\n\n")
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
