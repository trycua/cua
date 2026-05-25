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

use std::process;
use mcp_server::{protocol::Content, tool::ToolRegistry};

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
    McpConfig { client: Option<String> },
    Serve {
        socket: Option<String>,
        /// True when `--no-permissions-gate` is on argv.  The env-var
        /// `CUA_DRIVER_RS_PERMISSIONS_GATE=0` short-circuits the gate too
        /// (checked inside the gate itself), so the flag is only one of
        /// two opt-out signals.
        no_permissions_gate: bool,
    },
    Stop { socket: Option<String> },
    Status { socket: Option<String> },
    Recording { subcommand: String, args: Vec<String>, socket: Option<String> },
    DumpDocs { pretty: bool, doc_type: String },
    Update { apply: bool },
    Doctor { json: bool },
    Diagnose,
    Config {
        /// `show` | `get` | `set` | `reset` (None → show)
        subcommand: Option<String>,
        /// key arg for `get`/`set`
        key: Option<String>,
        /// value arg for `set`
        value: Option<String>,
        socket: Option<String>,
    },
    /// Hidden subcommand used by `scripts/install.sh` to emit the
    /// one-shot `cua_driver_install` PostHog event. Bypasses the
    /// opt-out flag (the only call site that does so) so we get a
    /// usable adoption signal even from users who opt out immediately
    /// after install. Subsequent runs see the `.installation_recorded`
    /// marker file and become no-ops.
    TelemetryInstallEvent,
    /// `cua-driver autostart {enable|disable|status|kick}` —
    /// platform-native auto-start so `cua-driver serve` comes up on
    /// every logon. Windows: Scheduled Task with LogonType=Interactive
    /// (lands in Session 1+). macOS / Linux: not yet implemented; the
    /// stub returns a helpful "use install-local.sh --autostart"
    /// message. See `crates/cua-driver/src/autostart.rs`.
    Autostart { subcommand: String },
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
    Skills { subcommand: String, flags: Vec<String> },
}

/// Flags whose next token is a value (not a subcommand).
/// We skip both the flag and its value when scanning for the subcommand.
const VALUE_FLAGS: &[&str] = &[
    "--cursor-icon", "--cursor-id", "--cursor-palette",
    "--glide-ms", "--dwell-ms", "--idle-hide-ms",
    "--screenshot-out-file", "--client", "--socket", "--pid-file", "--type",
];

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
        println!("cua-driver {} — cross-platform computer-use automation driver", env!("CARGO_PKG_VERSION"));
        println!("Usage: cua-driver [SUBCOMMAND] [OPTIONS]");
        println!("Subcommands: mcp, list-tools, describe, call, serve, stop, status, config, recording, update, doctor, diagnose, autostart, skills");
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
        println!("mcp options (macOS):");
        println!("  --no-daemon-relaunch    Stay in-process; skip auto-launching the CuaDriver daemon.");
        println!("                          Also: CUA_DRIVER_RS_MCP_NO_RELAUNCH=1");
        println!("  --socket <path>         Override the daemon UDS path used by the proxy fallback.");
        println!();
        println!("doctor options:");
        println!("  --json                  Emit the probe report as JSON for scripting.");
        std::process::exit(0);
    }

    // Collect named flag values we care about.
    let screenshot_out_file = flag_value(&args, "--screenshot-out-file");
    let mcp_client = flag_value(&args, "--client");
    let socket = flag_value(&args, "--socket");

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
    let claude_code_compat = args.iter().any(|a| a == "--claude-code-computer-use-compat");

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
        },
        Some("stop") => Command::Stop { socket },
        Some("status") => Command::Status { socket },
        Some("recording") => {
            let subcommand = pos.next().unwrap_or("status").to_string();
            let rest: Vec<String> = pos.map(str::to_owned).collect();
            Command::Recording { subcommand, args: rest, socket }
        }
        Some("dump-docs") => {
            let pretty = args.iter().any(|a| a == "--pretty" || a == "-p");
            let doc_type = flag_value(&args, "--type").unwrap_or_else(|| "all".to_owned());
            Command::DumpDocs { pretty, doc_type }
        }
        Some("update") => {
            let apply = args.iter().any(|a| a == "--apply");
            Command::Update { apply }
        }
        Some("doctor") => {
            // `--json` switches to machine-readable output for scripting.
            // Bare flag — no value, position-independent.
            let json = args.iter().any(|a| a == "--json");
            Command::Doctor { json }
        }
        Some("diagnose") => Command::Diagnose,
        Some("config") => {
            let subcommand = pos.next().map(str::to_owned);
            let key = pos.next().map(str::to_owned);
            let value = pos.next().map(str::to_owned);
            Command::Config { subcommand, key, value, socket }
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
                        eprintln!("error: positional JSON arg to 'cua-driver call' did not parse: {e}");
                        eprintln!("       received: {s}");
                        eprintln!();
                        eprintln!("hint: PowerShell 5.1 strips quotes around JSON field names in");
                        eprintln!("      multi-field args. Pipe the JSON via stdin instead:");
                        eprintln!("        '{{\"pid\":1234,\"window_id\":5678}}' | cua-driver call {}", tool);
                        eprintln!();
                        eprintln!("      Or use PowerShell 7+ (pwsh) which preserves the quotes.");
                        process::exit(2);
                    }
                },
                None => read_stdin_json(),
            };
            Command::Call { tool, json_args, screenshot_out_file, socket: socket.clone() }
        }
        Some("telemetry") => {
            // Hidden — used by install.sh. Only supports `install-event`
            // today; left extensible (e.g. future `telemetry status`).
            match pos.next() {
                Some("install-event") => Command::TelemetryInstallEvent,
                _ => {
                    eprintln!("Usage: cua-driver telemetry install-event");
                    process::exit(64);
                }
            }
        }
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
        Some(first) => {
            // Implicit call: unrecognised first positional → treat as tool name.
            // Same parse-error handling as the explicit `call` branch above. See #1637.
            let tool = first.to_string();
            let json_args = match pos.next() {
                Some(s) => match serde_json::from_str(s) {
                    Ok(v) => Some(v),
                    Err(e) => {
                        eprintln!("error: positional JSON arg to 'cua-driver {tool}' did not parse: {e}");
                        eprintln!("       received: {s}");
                        eprintln!();
                        eprintln!("hint: PowerShell 5.1 strips quotes around JSON field names in");
                        eprintln!("      multi-field args. Pipe the JSON via stdin instead:");
                        eprintln!("        '{{\"pid\":1234,\"window_id\":5678}}' | cua-driver {}", tool);
                        eprintln!();
                        eprintln!("      Or use PowerShell 7+ (pwsh) which preserves the quotes.");
                        process::exit(2);
                    }
                },
                None => read_stdin_json(),
            };
            Command::Call { tool, json_args, screenshot_out_file, socket: socket.clone() }
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
    let mut entries: Vec<(&str, &mcp_server::tool::ToolDef)> = registry.iter_defs().collect();
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
                if !def.description.ends_with('\n') { println!(); }
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
pub fn launch_daemon_and_wait(socket_path: &str, timeout_secs: u64) -> anyhow::Result<()> {
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

    let status = status.map_err(|e| {
        anyhow::anyhow!(
            "failed to exec `/usr/bin/open`: {e}. Pass --no-daemon-relaunch to bypass."
        )
    })?;

    if !status.success() {
        anyhow::bail!(
            "`open -n -g -a CuaDriver --args serve{}` exited {:?}. \
             Check that `/Applications/CuaDriver.app` is installed, or \
             pass --no-daemon-relaunch to bypass.",
            if pass_socket { format!(" --socket {socket_path}") } else { String::new() },
            status.code()
        );
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

    anyhow::bail!(
        "daemon did not appear on {socket_path} within {timeout_secs}s. If this \
         is the first launch, grant Accessibility + Screen Recording to \
         CuaDriver.app in System Settings and retry. Pass --no-daemon-relaunch \
         to stay in-process."
    );
}

/// Run the MCP proxy path: ensure a daemon is up (spawning via
/// `open` if needed), then `crate::proxy::run_proxy` against its
/// socket. Builds its own tokio runtime — same shape as the other
/// `run_*` helpers in this file that own their event loop.
pub fn run_mcp_via_daemon_proxy(socket: Option<String>) -> anyhow::Result<()> {
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

    if !crate::serve::is_daemon_listening(&socket_path) {
        // CUA_DRIVER_RS_MCP_FORCE_PROXY callers (test harness, custom
        // bundle setups) supply their own daemon — skip the auto-
        // launch step, since they don't have an installed
        // CuaDriver.app to relaunch into. Fail fast if no daemon is
        // up at this point.
        if crate::bundle::is_env_truthy("CUA_DRIVER_RS_MCP_FORCE_PROXY") {
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
            launch_daemon_and_wait(&socket_path, 10)?;
        }
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
            anyhow::bail!(
                "no cua-driver daemon listening on {socket_path}. Start one in \
                 your interactive session — on Windows run \
                 `cua-driver autostart enable && cua-driver autostart kick`; \
                 on Linux run `cua-driver serve &` in the user's session. \
                 Then re-run `cua-driver mcp`. To skip the proxy and run \
                 in-process anyway (Session 0 attribution, GUI tools will \
                 return empty), pass --no-daemon-relaunch."
            );
        }
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    rt.block_on(crate::proxy::run_proxy(socket_path))
}

/// Print the MCP server config snippet or a client-specific install command.
///
/// `--client <name>` selects one of: claude, codex, cursor, hermes, openclaw,
/// opencode. Omit for the generic JSON snippet.
pub fn run_mcp_config(client: Option<&str>) {
    let binary = std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(str::to_owned))
        .unwrap_or_else(|| "cua-driver".to_owned());

    match client {
        None | Some("") => {
            println!(r#"{{
  "mcpServers": {{
    "cua-driver": {{
      "command": "{binary}",
      "args": ["mcp"]
    }}
  }}
}}"#);
        }
        Some("claude") | Some("claude-code") => {
            // Claude Code wants the MCP server registered as
            // `cua-computer-use` and the binary invoked with
            // `--claude-code-computer-use-compat` so the regular
            // `screenshot` tool is replaced by a window-scoped variant
            // (pid + window_id required, JPEG @ 85%, text note pointing
            // at pixel tools).
            //
            // Observed Claude Code behaviour: the exact config key
            // "computer-use" is reserved, so external stdio
            // registrations use a distinct key — hence `cua-computer-use`.
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
                "args": ["mcp", "--claude-code-computer-use-compat"],
            });
            let json = cfg.to_string();
            #[cfg(windows)]
            let json = json.replace('"', "\\\"");
            println!("claude mcp add-json cua-computer-use '{}'", json);
        }
        Some("codex") => {
            println!("codex mcp add cua-driver -- {binary} mcp");
        }
        Some("cursor") => {
            println!(r#"{{
  "mcpServers": {{
    "cua-driver": {{
      "command": "{binary}",
      "args": ["mcp"],
      "type": "stdio"
    }}
  }}
}}"#);
        }
        Some("openclaw") => {
            println!("openclaw mcp set cua-driver '{{\"command\":\"{binary}\",\"args\":[\"mcp\"]}}'");
        }
        Some("opencode") => {
            println!(r#"// paste under "mcp" in opencode.json:
{{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {{
    "cua-driver": {{
      "type": "local",
      "command": ["{binary}", "mcp"],
      "enabled": true
    }}
  }}
}}"#);
        }
        Some("hermes") => {
            println!("# paste under mcp_servers in ~/.hermes/config.yaml,");
            println!("# then run /reload-mcp inside Hermes:");
            println!("mcp_servers:");
            println!("  cua-driver:");
            println!("    command: \"{binary}\"");
            println!("    args: [\"mcp\"]");
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
        Some(other) => {
            eprintln!("Unknown client '{other}'. Valid: claude, codex, cursor, openclaw, opencode, hermes, pi.");
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
    if crate::serve::is_daemon_listening(&socket_path) {
        let args_for_daemon = json_args.clone()
            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
        let req = crate::serve::DaemonRequest {
            method: "call".into(),
            name: Some(tool.to_owned()),
            args: Some(args_for_daemon),
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
                                    let b64 = item.get("data").and_then(|v| v.as_str()).map(str::to_owned);
                                    let mime = item.get("mimeType").and_then(|v| v.as_str())
                                        .unwrap_or("image/png").to_owned();
                                    if let Some(b64) = b64 {
                                        if let Some(ref path) = screenshot_out_file {
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
                                    map.insert("screenshot_png_b64".into(), serde_json::Value::String(b64));
                                    map.insert("screenshot_mime_type".into(), serde_json::Value::String(mime));
                                }
                            }
                            let pretty = serde_json::to_string_pretty(&obj)
                                .unwrap_or_else(|_| obj.to_string());
                            println!("{pretty}");
                            printed = true;
                        }
                        if !printed {
                            if let Some(content) = result.get("content").and_then(|v| v.as_array()) {
                                for item in content {
                                    if item.get("type").and_then(|v| v.as_str()) == Some("text") {
                                        if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
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
                    process::exit(resp.exit_code.unwrap_or(1));
                }
            }
            Err(e) => {
                // Daemon became unreachable mid-call — fall through to in-process.
                tracing::debug!("Daemon forwarding failed ({e}), running in-process");
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

    let args = json_args.unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
    let tool_name = tool.to_string();
    let out_path = screenshot_out_file;
    let is_error = rt.block_on(async move {
        let result = registry.invoke(&tool_name, args).await;
        let is_err = result.is_error.unwrap_or(false);

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
                Content::Image { data, mime_type, .. } => {
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
                eprintln!("--screenshot-out-file: no image content in tool response; file not written");
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
                            map.insert("screenshot_mime_type".into(), serde_json::Value::String(mime));
                        }
                    }
                }
                let pretty = serde_json::to_string_pretty(&obj)
                    .unwrap_or_else(|_| obj.to_string());
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
/// `set_recording` / `get_recording_state` tools on the running daemon.
///
/// Requires a running daemon (`cua-driver serve`) because recording
/// state lives in-process.
pub fn run_recording_cmd(subcommand: &str, args: &[String], socket: Option<&str>) {
    let socket_path = socket
        .map(str::to_owned)
        .unwrap_or_else(crate::serve::default_socket_path);

    if !crate::serve::is_daemon_listening(&socket_path) {
        eprintln!(
            "cua-driver daemon is not running.\n\
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
                name: Some("set_recording".into()),
                args: Some(serde_json::json!({
                    "enabled": true,
                    "output_dir": output_dir
                })),
            };
            match crate::serve::send_request(&socket_path, &req) {
                Ok(resp) if resp.ok => {
                    println!("Recording started → {output_dir}");
                    // Query state to show next_turn.
                    let state_req = crate::serve::DaemonRequest {
                        method: "call".into(),
                        name: Some("get_recording_state".into()),
                        args: Some(serde_json::json!({})),
                    };
                    if let Ok(sr) = crate::serve::send_request(&socket_path, &state_req) {
                        if let Some(result) = sr.result {
                            let sc = result.get("structuredContent")
                                .or_else(|| result.get("structured_content"));
                            if let Some(next_turn) = sc.and_then(|s| s.get("next_turn")).and_then(|v| v.as_u64()) {
                                println!("Next turn: {next_turn:05}");
                            }
                        }
                    }
                }
                Ok(resp) => {
                    if let Some(e) = resp.error { eprintln!("{e}"); }
                    process::exit(1);
                }
                Err(e) => { eprintln!("recording start: {e}"); process::exit(1); }
            }
        }

        "stop" => {
            let req = crate::serve::DaemonRequest {
                method: "call".into(),
                name: Some("set_recording".into()),
                args: Some(serde_json::json!({ "enabled": false })),
            };
            match crate::serve::send_request(&socket_path, &req) {
                Ok(resp) if resp.ok => println!("Recording stopped."),
                Ok(resp) => {
                    if let Some(e) = resp.error { eprintln!("{e}"); }
                    process::exit(1);
                }
                Err(e) => { eprintln!("recording stop: {e}"); process::exit(1); }
            }
        }

        "status" | "" => {
            let req = crate::serve::DaemonRequest {
                method: "call".into(),
                name: Some("get_recording_state".into()),
                args: Some(serde_json::json!({})),
            };
            match crate::serve::send_request(&socket_path, &req) {
                Ok(resp) if resp.ok => {
                    if let Some(result) = resp.result {
                        let sc = result.get("structuredContent")
                            .or_else(|| result.get("structured_content"))
                            .cloned()
                            .unwrap_or(serde_json::json!({}));
                        let enabled = sc.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
                        let out_dir = sc.get("output_dir").and_then(|v| v.as_str()).unwrap_or("(none)");
                        let next_turn = sc.get("next_turn").and_then(|v| v.as_u64()).unwrap_or(0);
                        println!("Recording: {}", if enabled { "enabled" } else { "disabled" });
                        if enabled {
                            println!("  output_dir: {out_dir}");
                            println!("  next_turn:  {next_turn:05}");
                        }
                    }
                }
                Ok(resp) => {
                    if let Some(e) = resp.error { eprintln!("{e}"); }
                    process::exit(1);
                }
                Err(e) => { eprintln!("recording status: {e}"); process::exit(1); }
            }
        }

        other => {
            eprintln!("Unknown recording subcommand '{other}'. Valid: start <dir>, stop, status");
            process::exit(64);
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
pub fn run_update_cmd(apply: bool) {
    let current = env!("CARGO_PKG_VERSION");
    println!("Current version: {current}");
    println!("Checking for updates…");

    let latest = crate::version_check::fetch_latest_version();
    match latest {
        Err(e) => {
            // The shared helper returns a human-readable error string for
            // the CLI surface — pass it through so the user can see why
            // (timeout, parse error, etc.) instead of just "unreachable".
            tracing::debug!(target: "cua_driver::update", "fetch failed: {e}");
            println!("Could not reach GitHub — check your connection and try again.");
            process::exit(1);
        }
        Ok(v) if !crate::version_check::is_newer(&v, current) => {
            println!("Already up to date.");
        }
        Ok(v) => {
            println!("New version available: {v}");

            if !apply {
                println!();
                println!("Run with --apply to download and install it:");
                println!("  cua-driver update --apply");
                println!();
                println!("Or reinstall directly:");
                println!("  {}", crate::updater::manual_install_one_liner());
                return;
            }

            println!("Downloading and installing cua-driver {v}…");
            let daemon_was_running = crate::updater::daemon_is_running();
            match crate::updater::run_install_script(&v) {
                Ok(s) if s.success() => {
                    println!("Installed cua-driver {v}.");
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

/// Output documentation as JSON.  `doc_type` is one of:
/// - `"mcp"` — only MCP tool docs (`{version, tools: [...]}`)
/// - `"cli"` — CLI docs (stub on Rust — Swift extracts via swift-argument-parser
///   introspection which has no clap analogue without a bigger refactor)
/// - `"all"` — `{cli, mcp}` matching Swift `CombinedDocs`
pub fn run_dump_docs_with_type(registry: &ToolRegistry, pretty: bool, doc_type: &str) {
    // Each MCP tool: `{name, description, input_schema}` (Swift's MCPToolDoc
    // shape — Rust adds read_only/destructive/idempotent as intentional
    // extras documented in PARITY.md).
    let tools: Vec<serde_json::Value> = registry.iter_defs()
        .map(|(_, def)| serde_json::json!({
            "name":         def.name,
            "description":  def.description,
            "input_schema": def.input_schema,
            "read_only":    def.read_only,
            "destructive":  def.destructive,
            "idempotent":   def.idempotent,
        }))
        .collect();
    let mcp = serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "tools":   tools,
    });

    // CLI docs are intentionally a thin stub on Rust — full extraction
    // would require introspecting clap's command graph which we don't use
    // (cli.rs uses hand-rolled arg matching).  Documented in PARITY.md.
    let cli_stub = serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "commands": [],
        "_note": "CLI introspection not implemented on Rust port. Use `cua-driver --help` for CLI docs.",
    });

    let doc = match doc_type {
        "cli" => cli_stub,
        "mcp" => mcp,
        _     => serde_json::json!({ "cli": cli_stub, "mcp": mcp }),
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
    let argv0 = std::env::args().next().unwrap_or_else(|| "<unknown>".into());
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
            let result = registry.invoke("check_permissions",
                serde_json::Value::Object(Default::default())).await;
            if let Some(sc) = &result.structured_content {
                let ax = sc.get("accessibility")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let sr = sc.get("screen_recording")
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
    let db = format!(
        "{home}/Library/Application Support/com.apple.TCC/TCC.db"
    );
    let sql = "SELECT service, client, client_type, auth_value, auth_reason, \
               hex(csreq) AS csreq_hex FROM access WHERE client='com.trycua.driver';";

    let mut lines = vec!["## tcc database rows for com.trycua.driver".to_owned()];
    lines.push(format!("(reading {db} — best-effort; system TCC DB requires FDA)"));
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
        ("user data dir",   format!("{home}/.cua-driver")),
        ("config cache",    format!("{home}/Library/Caches/cua-driver")),
        ("telemetry id",    format!("{home}/.cua-driver/.telemetry_id")),
        ("updater plist",   format!("{home}/Library/LaunchAgents/com.trycua.cua_driver_updater.plist")),
        ("daemon plist",    format!("{home}/Library/LaunchAgents/com.trycua.cua_driver_daemon.plist")),
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
            let config = rt.block_on(async {
                registry.invoke("get_config", serde_json::json!({})).await
            });
            if let Some(sc) = config.structured_content {
                println!("{}", serde_json::to_string_pretty(&sc).unwrap_or_else(|_| sc.to_string()));
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
            // Try daemon first.
            if crate::serve::is_daemon_listening(&socket_path) {
                let req = crate::serve::DaemonRequest {
                    method: "call".into(),
                    name: Some("get_config".into()),
                    args: Some(serde_json::json!({})),
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
            let config = rt.block_on(async {
                registry.invoke("get_config", serde_json::json!({})).await
            });
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
                    sc_map.insert(k, v);
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
                println!("{}", match &v { serde_json::Value::String(s) => s.clone(), other => other.to_string() });
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
            let value = match value {
                Some(v) => v,
                None => {
                    eprintln!("Usage: cua-driver config set {key} <value>");
                    process::exit(64);
                }
            };
            // Parse value: try JSON, fall back to string.
            let parsed_value: serde_json::Value =
                serde_json::from_str(value).unwrap_or_else(|_| serde_json::Value::String(value.to_owned()));
            let args = serde_json::json!({ key: parsed_value });

            // Try daemon first.
            if crate::serve::is_daemon_listening(&socket_path) {
                let req = crate::serve::DaemonRequest {
                    method: "call".into(),
                    name: Some("set_config".into()),
                    args: Some(args.clone()),
                };
                if let Ok(resp) = crate::serve::send_request(&socket_path, &req) {
                    if resp.ok {
                        println!("Config updated.");
                        // Show current state.
                        let req2 = crate::serve::DaemonRequest {
                            method: "call".into(),
                            name: Some("get_config".into()),
                            args: Some(serde_json::json!({})),
                        };
                        if let Ok(r2) = crate::serve::send_request(&socket_path, &req2) {
                            if let Some(result) = r2.result {
                                if let Some(sc) = result.get("structuredContent") {
                                    println!("{}", serde_json::to_string_pretty(sc).unwrap_or_default());
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
            let result = rt.block_on(async {
                registry.invoke("set_config", args).await
            });
            if result.is_error.unwrap_or(false) {
                for item in &result.content {
                    if let mcp_server::protocol::Content::Text { text, .. } = item {
                        eprintln!("{text}");
                    }
                }
                process::exit(1);
            }
            // Persist the value to disk so future CLI invocations can read it.
            write_config_file(key, &parsed_value);
            // Print updated config.
            let config = rt.block_on(async {
                registry.invoke("get_config", serde_json::json!({})).await
            });
            if let Some(sc) = config.structured_content {
                println!("{}", serde_json::to_string_pretty(&sc).unwrap_or_else(|_| sc.to_string()));
            }
        }

        "reset" => {
            // Reset to defaults by calling set_config with empty args.
            // The set_config tool keeps existing values if keys are absent,
            // so we send the known defaults explicitly.
            let defaults = serde_json::json!({
                "capture_mode": "som",
                "max_image_dimension": 0
            });
            let result = rt.block_on(async {
                registry.invoke("set_config", defaults).await
            });
            if result.is_error.unwrap_or(false) {
                eprintln!("config reset failed");
                process::exit(1);
            }
            println!("Config reset to defaults.");
            let config = rt.block_on(async {
                registry.invoke("get_config", serde_json::json!({})).await
            });
            if let Some(sc) = config.structured_content {
                println!("{}", serde_json::to_string_pretty(&sc).unwrap_or_else(|_| sc.to_string()));
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

/// Map a parsed [`Command`] to its canonical telemetry event name.
///
/// Mirrors Swift's `CuaDriverCommand.telemetryEntryEvent(for:)`. Per-tool
/// `call <tool>` invocations report as `cua_driver_api_<tool>` so per-tool
/// usage shows up in aggregate without our ever recording the args.
///
/// Returns `None` for [`Command::TelemetryInstallEvent`] — that path emits
/// the install event directly via [`crate::telemetry::capture_install`]
/// instead of a per-entry event.
pub fn telemetry_entry_event(cmd: &Command) -> Option<String> {
    use crate::telemetry::event;
    let name = match cmd {
        Command::Mcp { .. } => event::MCP.to_owned(),
        Command::Serve { .. } => event::SERVE.to_owned(),
        Command::Stop { .. } => event::STOP.to_owned(),
        Command::Status { .. } => event::STATUS.to_owned(),
        Command::ListTools => event::LIST_TOOLS.to_owned(),
        Command::Describe(_) => event::DESCRIBE.to_owned(),
        // `call <tool>` → per-tool event (no args, just the tool name).
        // The tool name flows into the PostHog event name, so we sanitize
        // it aggressively (see `sanitize_tool_name`) before concatenation —
        // otherwise weird / path-like / non-ASCII tool names would pollute
        // dashboards and could even leak user-controlled strings into
        // telemetry event names.
        Command::Call { tool, .. } => {
            if tool.is_empty() {
                event::CALL.to_owned()
            } else {
                format!("{}{}", event::API_PREFIX, sanitize_tool_name(tool))
            }
        }
        Command::McpConfig { .. } => "cua_driver_mcp_config".to_owned(),
        Command::Recording { .. } => event::RECORDING.to_owned(),
        Command::Config { .. } => event::CONFIG.to_owned(),
        Command::DumpDocs { .. } => "cua_driver_dump_docs".to_owned(),
        Command::Update { .. } => "cua_driver_update".to_owned(),
        Command::Doctor { .. } => "cua_driver_doctor".to_owned(),
        Command::Diagnose => "cua_driver_diagnose".to_owned(),
        // Per-subcommand event so dashboards can split enable / disable /
        // status / kick separately — they have very different meanings
        // for adoption. `sanitize_tool_name` already does what we want
        // (lowercase ASCII / underscore-only / max 64 chars / fallback).
        Command::Autostart { subcommand } => {
            format!("cua_driver_autostart_{}", sanitize_tool_name(subcommand))
        }
        Command::Skills { subcommand, .. } => {
            format!("cua_driver_skills_{}", sanitize_tool_name(subcommand))
        }
        Command::TelemetryInstallEvent => return None,
    };
    Some(name)
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
}

fn first_sentence(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() { return String::new(); }
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
    if s.ends_with('.') { s.pop(); }
    s
}
