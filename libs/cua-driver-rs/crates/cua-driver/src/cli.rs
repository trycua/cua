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
    Mcp,
    ListTools,
    Describe(String),
    Call { tool: String, json_args: Option<serde_json::Value>, screenshot_out_file: Option<String> },
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
    Doctor,
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
        println!("Subcommands: mcp, list-tools, describe, call, serve, stop, status, config, recording, update, doctor, diagnose");
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

    let mut pos = positionals.into_iter();
    match pos.next() {
        None | Some("mcp") => Command::Mcp,
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
        Some("doctor") => Command::Doctor,
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
            let json_args = pos.next()
                .and_then(|s| serde_json::from_str(s).ok())
                .or_else(|| read_stdin_json());
            Command::Call { tool, json_args, screenshot_out_file }
        }
        Some(first) => {
            // Implicit call: unrecognised first positional → treat as tool name.
            let tool = first.to_string();
            let json_args = pos.next()
                .and_then(|s| serde_json::from_str(s).ok())
                .or_else(|| read_stdin_json());
            Command::Call { tool, json_args, screenshot_out_file }
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
        Some("claude") => {
            println!("claude mcp add --transport stdio cua-driver -- {binary} mcp");
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
) {
    // Daemon forwarding: if a daemon is listening, proxy the request
    // through it so AppStateEngine's element_index cache is shared.
    let socket_path = crate::serve::default_socket_path();
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
                        // Handle image write if --screenshot-out-file was given.
                        let mut printed = false;
                        if let Some(content) = result.get("content").and_then(|v| v.as_array()) {
                            for item in content {
                                if item.get("type").and_then(|v| v.as_str()) == Some("image") {
                                    if let Some(ref path) = screenshot_out_file {
                                        if let Some(b64) = item.get("data").and_then(|v| v.as_str()) {
                                            use base64::Engine as _;
                                            match base64::engine::general_purpose::STANDARD.decode(b64) {
                                                Ok(bytes) => { let _ = std::fs::write(path, &bytes); }
                                                Err(e) => eprintln!("--screenshot-out-file: {e}"),
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        if let Some(sc) = result.get("structuredContent") {
                            let pretty = serde_json::to_string_pretty(sc)
                                .unwrap_or_else(|_| sc.to_string());
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
/// Uses `curl` to query the GitHub releases API (same as Swift reference).
/// Pass `--apply` to download and install via the canonical install.sh.
pub fn run_update_cmd(apply: bool) {
    let current = env!("CARGO_PKG_VERSION");
    println!("Current version: {current}");
    println!("Checking for updates…");

    let latest = fetch_latest_version();
    match latest {
        None => {
            println!("Could not reach GitHub — check your connection and try again.");
            process::exit(1);
        }
        Some(v) if !is_version_newer(&v, current) => {
            println!("Already up to date.");
        }
        Some(v) => {
            println!("New version available: {v}");

            if !apply {
                println!();
                println!("Run with --apply to download and install it:");
                println!("  cua-driver update --apply");
                println!();
                println!("Or reinstall directly:");
                println!("  curl -fsSL https://raw.githubusercontent.com/trycua/cua/main/libs/cua-driver-rs/scripts/install.sh | bash");
                return;
            }

            println!("Downloading and installing cua-driver {v}…");
            let status = std::process::Command::new("bash")
                .arg("-c")
                .arg("curl -fsSL https://raw.githubusercontent.com/trycua/cua/main/libs/cua-driver-rs/scripts/install.sh | bash")
                .status();
            match status {
                Ok(s) if s.success() => {}
                Ok(s) => {
                    println!("Installation failed (exit {}). Run the command above manually.", s.code().unwrap_or(1));
                    process::exit(s.code().unwrap_or(1));
                }
                Err(e) => {
                    eprintln!("Failed to run installer: {e}");
                    process::exit(1);
                }
            }
        }
    }
}

/// Fetch the latest `cua-driver-v*` release tag from GitHub using curl.
/// Returns the version string (e.g. "0.1.2") or `None` on any error.
fn fetch_latest_version() -> Option<String> {
    let out = std::process::Command::new("curl")
        .args(["-s", "--max-time", "4",
               "-H", "Accept: application/vnd.github+json",
               "https://api.github.com/repos/trycua/cua/releases?per_page=40"])
        .output()
        .ok()?;
    if !out.status.success() { return None; }
    let text = String::from_utf8_lossy(&out.stdout);
    let releases: serde_json::Value = serde_json::from_str(&text).ok()?;
    // `cua-driver-rs-v*` releases are distinct from the Swift `cua-driver-v*`
    // releases on the same repo — must filter by the Rust-port prefix or
    // we'd recommend the Swift binary instead.
    let prefix = "cua-driver-rs-v";
    let mut versions: Vec<String> = releases.as_array()?.iter()
        .filter_map(|r| {
            let tag = r["tag_name"].as_str()?;
            if !tag.starts_with(prefix) { return None; }
            if r["draft"].as_bool().unwrap_or(false) { return None; }
            if r["prerelease"].as_bool().unwrap_or(false) { return None; }
            Some(tag[prefix.len()..].to_owned())
        })
        .collect();
    // Sort descending (newest first).
    versions.sort_by(|a, b| compare_versions(b, a));
    versions.into_iter().next()
}

/// True when `candidate` is strictly newer than `current` (semver compare).
fn is_version_newer(candidate: &str, current: &str) -> bool {
    compare_versions(candidate, current) == std::cmp::Ordering::Greater
}

fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let parts = |s: &str| -> Vec<u64> {
        s.split('.').filter_map(|p| p.parse().ok()).collect()
    };
    let pa = parts(a);
    let pb = parts(b);
    for (x, y) in pa.iter().zip(pb.iter()) {
        match x.cmp(y) {
            std::cmp::Ordering::Equal => continue,
            other => return other,
        }
    }
    pa.len().cmp(&pb.len())
}

/// `cua-driver dump-docs [--pretty]` — output all MCP tool schemas as JSON.
pub fn run_dump_docs(registry: &ToolRegistry, pretty: bool) {
    run_dump_docs_with_type(registry, pretty, "all")
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

/// `cua-driver doctor` — clean up stale install bits left from older cua-driver versions.
///
/// Mirrors Swift `DoctorCommand`:
///   - Removes the legacy LaunchAgent plist (~/Library/LaunchAgents/com.trycua.cua_driver_updater.plist)
///     via `launchctl unload` then `fs::remove_file`.
///   - Removes the legacy update script (/usr/local/bin/cua-driver-update) if writable;
///     otherwise prints the `sudo rm` command the user must run manually.
pub fn run_doctor_cmd() {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let legacy_plist = format!("{home}/Library/LaunchAgents/com.trycua.cua_driver_updater.plist");
    let legacy_script = "/usr/local/bin/cua-driver-update";

    let mut cleaned_anything = false;

    // — Legacy LaunchAgent plist —
    if std::path::Path::new(&legacy_plist).exists() {
        cleaned_anything = true;
        println!("Removing legacy LaunchAgent: {legacy_plist}");

        // Try to unload it first (ignore errors — may not be loaded).
        let _ = std::process::Command::new("launchctl")
            .args(["unload", &legacy_plist])
            .status();

        match std::fs::remove_file(&legacy_plist) {
            Ok(()) => println!("  ✓ Removed."),
            Err(e) => eprintln!("  ✗ Failed to remove {legacy_plist}: {e}"),
        }
    }

    // — Legacy update script —
    if std::path::Path::new(legacy_script).exists() {
        cleaned_anything = true;
        println!("Removing legacy update script: {legacy_script}");
        match std::fs::remove_file(legacy_script) {
            Ok(()) => println!("  ✓ Removed."),
            Err(_) => {
                // Root-owned — user must sudo.
                println!("  Could not remove (root-owned). Run manually:");
                println!("    sudo rm -f {legacy_script}");
            }
        }
    }

    if !cleaned_anything {
        println!("Nothing to clean — install is up to date.");
    }
}

// ── helpers ──────────────────────────────────────────────────────────────────

/// Read JSON from stdin when stdin is a pipe (non-interactive). Returns `None`
/// when stdin is a terminal or the input isn't valid JSON.
/// Matches Swift's "If omitted, reads from stdin when stdin is a pipe."
fn read_stdin_json() -> Option<serde_json::Value> {
    use std::io::{self, IsTerminal, Read};
    let stdin = io::stdin();
    if stdin.is_terminal() {
        return None;
    }
    let mut buf = String::new();
    stdin.lock().read_to_string(&mut buf).ok()?;
    serde_json::from_str(buf.trim()).ok()
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
