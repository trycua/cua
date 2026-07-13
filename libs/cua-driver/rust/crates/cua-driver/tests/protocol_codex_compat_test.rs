//! Wire-level contract for `--codex-computer-use-compat`.

#![cfg(target_os = "macos")]

use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

struct CompatDriver {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl CompatDriver {
    fn spawn(compat: bool) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cua-driver"));
        command
            .arg("mcp")
            .arg("--no-daemon-relaunch")
            .arg("--no-overlay")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if compat {
            command.arg("--codex-computer-use-compat");
        }
        let mut child = command.spawn().expect("spawn cua-driver");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let mut driver = Self {
            child,
            stdin,
            stdout,
        };
        driver.send(serde_json::json!({
            "jsonrpc":"2.0","id":1,"method":"initialize","params":{}
        }));
        let initialized = driver.recv();
        assert_eq!(initialized["id"], 1);
        if compat {
            assert_eq!(initialized["result"]["serverInfo"]["name"], "Computer Use");
            assert_eq!(
                initialized["result"]["capabilities"]["tools"]["listChanged"],
                false
            );
            assert!(initialized["result"].get("instructions").is_none());
        } else {
            assert_eq!(initialized["result"]["serverInfo"]["name"], "cua-driver");
            assert!(initialized["result"]["instructions"].is_string());
        }
        driver
    }

    fn send(&mut self, value: Value) {
        writeln!(self.stdin, "{}", value).unwrap();
        self.stdin.flush().unwrap();
    }

    fn recv(&mut self) -> Value {
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        serde_json::from_str(line.trim()).unwrap()
    }
}

impl Drop for CompatDriver {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct DaemonDriver {
    child: Child,
    socket: PathBuf,
}

impl DaemonDriver {
    fn spawn(socket: PathBuf, compat: bool) -> Self {
        Self::spawn_with_unverified_client_override(socket, compat, compat)
    }

    fn spawn_with_unverified_client_override(
        socket: PathBuf,
        compat: bool,
        allow_unverified_client: bool,
    ) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cua-driver"));
        command
            .arg("serve")
            .arg("--socket")
            .arg(&socket)
            .arg("--no-permissions-gate")
            .arg("--no-overlay")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .env_remove("CUA_DRIVER_CODEX_ALLOW_UNVERIFIED_CLIENT");
        if compat {
            command.arg("--codex-computer-use-compat");
        }
        if allow_unverified_client {
            command.env("CUA_DRIVER_CODEX_ALLOW_UNVERIFIED_CLIENT", "1");
        }
        let mut child = command.spawn().expect("spawn cua-driver daemon");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if UnixStream::connect(&socket).is_ok() {
                break;
            }
            assert!(
                child.try_wait().unwrap().is_none(),
                "daemon exited before binding {}",
                socket.display()
            );
            assert!(
                std::time::Instant::now() < deadline,
                "daemon did not bind {}",
                socket.display()
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        Self { child, socket }
    }

    fn tool_names(&self) -> Vec<String> {
        let mut stream = UnixStream::connect(&self.socket).expect("connect daemon");
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        writeln!(stream, "{}", serde_json::json!({"method":"list"})).unwrap();
        stream.flush().unwrap();
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).unwrap();
        let response: Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(response["ok"], true);
        response["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap().to_owned())
            .collect()
    }
}

fn send_raw_daemon_request(socket: &std::path::Path, request: Value) -> Value {
    let mut stream = UnixStream::connect(socket).expect("connect daemon");
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .unwrap();
    writeln!(stream, "{request}").unwrap();
    stream.flush().unwrap();
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();
    serde_json::from_str(line.trim()).unwrap()
}

impl Drop for DaemonDriver {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn unique_daemon_root() -> PathBuf {
    PathBuf::from("/tmp").join(format!(
        "cua-cdx-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn mcp_tool_names(driver: &mut CompatDriver) -> Vec<String> {
    driver.send(serde_json::json!({
        "jsonrpc":"2.0","id":99,"method":"tools/list"
    }));
    driver.recv()["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_owned())
        .collect()
}

fn run_proxy_to_explicit_socket(socket: &PathBuf, compat: bool) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cua-driver"));
    command
        .arg("mcp")
        .arg("--socket")
        .arg(socket)
        .arg("--no-overlay")
        .env("CUA_DRIVER_RS_MCP_FORCE_PROXY", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if compat {
        command.arg("--codex-computer-use-compat");
    }
    command.output().expect("run explicit-socket MCP proxy")
}

fn default_profile_socket(home: &std::path::Path, compat: bool) -> PathBuf {
    home.join("Library/Caches/cua-driver").join(if compat {
        "cua-driver-codex-computer-use.sock"
    } else {
        "cua-driver.sock"
    })
}

fn spawn_default_profile_daemon(home: &std::path::Path, compat: bool) -> DaemonDriver {
    let socket = default_profile_socket(home, compat);
    let mut command = Command::new(env!("CARGO_BIN_EXE_cua-driver"));
    command
        .arg("serve")
        .arg("--no-permissions-gate")
        .arg("--no-overlay")
        .env("HOME", home)
        .env_remove("CUA_DRIVER_RS_MCP_HTTP_PORT")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    if compat {
        command.arg("--codex-computer-use-compat");
    }
    let mut child = command.spawn().expect("spawn default-profile daemon");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if UnixStream::connect(&socket).is_ok() {
            break;
        }
        if let Some(status) = child.try_wait().unwrap() {
            use std::io::Read as _;
            let mut stderr = String::new();
            if let Some(mut pipe) = child.stderr.take() {
                let _ = pipe.read_to_string(&mut stderr);
            }
            panic!(
                "default-profile daemon exited before binding {} ({status}): {stderr}",
                socket.display()
            );
        }
        assert!(
            std::time::Instant::now() < deadline,
            "default-profile daemon did not bind {}",
            socket.display()
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    DaemonDriver { child, socket }
}

fn run_profile_lifecycle_command(
    home: &std::path::Path,
    subcommand: &str,
    compat: bool,
) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cua-driver"));
    command
        .arg(subcommand)
        .env("HOME", home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if compat {
        command.arg("--codex-computer-use-compat");
    }
    command.output().expect("run daemon lifecycle command")
}

#[test]
fn compat_tools_list_is_exact_v829_contract() {
    let mut driver = CompatDriver::spawn(true);
    driver.send(serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/list"
    }));
    let response = driver.recv();
    assert_eq!(
        response["result"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        vec!["tools"]
    );
    let tools = response["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        vec![
            "list_apps",
            "get_app_state",
            "click",
            "perform_secondary_action",
            "set_value",
            "select_text",
            "scroll",
            "drag",
            "press_key",
            "type_text",
        ]
    );

    let expected_properties: &[(&str, &[&str])] = &[
        ("list_apps", &[]),
        ("get_app_state", &["app"]),
        (
            "click",
            &[
                "app",
                "click_count",
                "element_index",
                "mouse_button",
                "x",
                "y",
            ],
        ),
        (
            "perform_secondary_action",
            &["action", "app", "element_index"],
        ),
        ("set_value", &["app", "element_index", "value"]),
        (
            "select_text",
            &[
                "app",
                "element_index",
                "prefix",
                "selection",
                "suffix",
                "text",
            ],
        ),
        ("scroll", &["app", "direction", "element_index", "pages"]),
        ("drag", &["app", "from_x", "from_y", "to_x", "to_y"]),
        ("press_key", &["app", "key"]),
        ("type_text", &["app", "text"]),
    ];
    let expected_descriptions = [
        ("list_apps", "List the apps on this computer. Returns the set of apps that are currently running, as well as any that have been used in the last 14 days, including details on usage frequency"),
        ("get_app_state", "Start an app use session if needed, then get the state of the app's key window and return a screenshot and accessibility tree. This must be called once per assistant turn before interacting with the app"),
        ("click", "Click an element by index or pixel coordinates from screenshot"),
        ("perform_secondary_action", "Invoke a secondary accessibility action exposed by an element"),
        ("set_value", "Set the value of a settable accessibility element"),
        ("select_text", "Select text inside a text element, or place the text cursor before or after it. Provide text exactly as it appears in the accessibility tree, including any Markdown formatting. If the text is not unique, provide surrounding prefix or suffix text to disambiguate it."),
        ("scroll", "Scroll an element in a direction by a number of pages"),
        ("drag", "Drag from one point to another using pixel coordinates"),
        ("press_key", "Press a key or key-combination on the keyboard, including modifier and navigation keys.\n  - This supports xdotool's `key` syntax.\n  - Examples: \"a\", \"Return\", \"Tab\", \"super+c\", \"Up\", \"KP_0\" (for the numpad 0 key)."),
        ("type_text", "Type literal text using keyboard input"),
    ];
    let expected_required: &[(&str, &[&str])] = &[
        ("list_apps", &[]),
        ("get_app_state", &["app"]),
        ("click", &["app"]),
        (
            "perform_secondary_action",
            &["app", "element_index", "action"],
        ),
        ("set_value", &["app", "element_index", "value"]),
        ("select_text", &["app", "element_index", "text"]),
        ("scroll", &["app", "element_index", "direction"]),
        ("drag", &["app", "from_x", "from_y", "to_x", "to_y"]),
        ("press_key", &["app", "key"]),
        ("type_text", &["app", "text"]),
    ];
    for (name, expected) in expected_properties {
        let tool = tools.iter().find(|tool| tool["name"] == *name).unwrap();
        assert_eq!(
            tool.as_object().unwrap().keys().collect::<Vec<_>>(),
            vec!["annotations", "description", "inputSchema", "name"],
            "wire field mismatch for {name}"
        );
        let mut properties: Vec<&str> = tool["inputSchema"]["properties"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        properties.sort_unstable();
        let mut expected = expected.to_vec();
        expected.sort_unstable();
        assert_eq!(properties, expected, "property mismatch for {name}");
        assert_eq!(tool["inputSchema"]["additionalProperties"], false);
        let expected_read_only = matches!(*name, "list_apps" | "get_app_state");
        assert_eq!(tool["annotations"]["readOnlyHint"], expected_read_only);
        assert_eq!(tool["annotations"]["idempotentHint"], expected_read_only);
        assert_eq!(tool["annotations"]["destructiveHint"], false);
        assert_eq!(tool["annotations"]["openWorldHint"], false);
        assert_eq!(
            tool["description"],
            expected_descriptions
                .iter()
                .find(|(expected_name, _)| expected_name == name)
                .unwrap()
                .1
        );
        let mut required: Vec<&str> = tool["inputSchema"]["required"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|value| value.as_str().unwrap())
            .collect();
        required.sort_unstable();
        let mut expected = expected_required
            .iter()
            .find(|(expected_name, _)| expected_name == name)
            .unwrap()
            .1
            .to_vec();
        expected.sort_unstable();
        assert_eq!(required, expected, "required mismatch for {name}");
        for (property, schema) in tool["inputSchema"]["properties"].as_object().unwrap() {
            let expected_type = match property.as_str() {
                "click_count" => "integer",
                "x" | "y" | "from_x" | "from_y" | "to_x" | "to_y" | "pages" => "number",
                "app" | "element_index" | "mouse_button" | "action" | "value" | "text"
                | "prefix" | "suffix" | "selection" | "direction" | "key" => "string",
                other => panic!("missing expected type for {name}.{other}"),
            };
            assert_eq!(
                schema["type"], expected_type,
                "type mismatch for {name}.{property}"
            );
            let expected_description = match (*name, property.as_str()) {
                ("select_text", "app") => "App name or bundle identifier",
                (_, "app") => "App name, full app path, or unambiguous bundle identifier",
                ("click", "element_index") => "Element index to click",
                ("select_text", "element_index") => "Text element identifier",
                (_, "element_index") => "Element identifier",
                (_, "x") => "X coordinate in screenshot pixel coordinates",
                (_, "y") => "Y coordinate in screenshot pixel coordinates",
                (_, "click_count") => "Number of clicks. Defaults to 1",
                (_, "mouse_button") => "Mouse button to click. Defaults to left.",
                (_, "from_x") => "Start X coordinate",
                (_, "from_y") => "Start Y coordinate",
                (_, "to_x") => "End X coordinate",
                (_, "to_y") => "End Y coordinate",
                (_, "action") => "Secondary accessibility action name",
                (_, "key") => "Key or key combination to press",
                (_, "direction") => "Scroll direction: up, down, left, or right",
                (_, "pages") => {
                    "Number of pages to scroll. Fractional values are supported. Defaults to 1"
                }
                (_, "prefix") => "Optional text immediately before the target, used to disambiguate repeated matches",
                (_, "suffix") => "Optional text immediately after the target, used to disambiguate repeated matches",
                (_, "selection") => "Whether to select the text or place the cursor before or after it. Defaults to text.",
                ("select_text", "text") => "Target text as shown in the accessibility tree",
                ("type_text", "text") => "Literal text to type",
                (_, "value") => "Value to assign",
                other => panic!("missing expected description for {other:?}"),
            };
            assert_eq!(
                schema["description"], expected_description,
                "description mismatch for {name}.{property}"
            );
        }
    }
    let click = tools.iter().find(|tool| tool["name"] == "click").unwrap();
    assert_eq!(
        click["inputSchema"]["properties"]["mouse_button"]["enum"],
        serde_json::json!(["left", "right", "middle"])
    );
    assert!(click["inputSchema"]["properties"]["click_count"]
        .get("minimum")
        .is_none());
    let scroll = tools.iter().find(|tool| tool["name"] == "scroll").unwrap();
    assert!(scroll["inputSchema"]["properties"]["pages"]
        .get("exclusiveMinimum")
        .is_none());
    let select_text = tools
        .iter()
        .find(|tool| tool["name"] == "select_text")
        .unwrap();
    assert_eq!(
        select_text["inputSchema"]["properties"]["selection"]["enum"],
        serde_json::json!(["text", "cursor_before", "cursor_after"])
    );
}

#[test]
fn compat_action_requires_current_app_snapshot() {
    let mut driver = CompatDriver::spawn(true);
    driver.send(serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"click","arguments":{"app":"Calculator","x":10,"y":10}}
    }));
    let response = driver.recv();
    assert_eq!(response["result"]["isError"], true);
    assert_eq!(
        response["result"]["structuredContent"]["code"],
        "no_active_app_session"
    );
}

#[test]
fn absent_flag_preserves_native_surface() {
    let mut driver = CompatDriver::spawn(false);
    driver.send(serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/list"
    }));
    let response = driver.recv();
    assert!(response["result"].get("capability_version").is_some());
    assert!(response["result"].get("schema_version").is_some());
    assert!(response["result"]["tools"][0].get("capabilities").is_some());
    let names: Vec<&str> = response["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    assert!(names.contains(&"list_windows"));
    assert!(names.contains(&"get_window_state"));
    assert!(!names.contains(&"get_app_state"));
}

#[test]
fn native_and_codex_compat_daemons_coexist_with_isolated_rosters() {
    use std::os::unix::fs::PermissionsExt;

    let root = unique_daemon_root();
    std::fs::create_dir_all(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut native = DaemonDriver::spawn(root.join("native.sock"), false);
    let mut compat = DaemonDriver::spawn(root.join("codex.sock"), true);

    let mut native_mcp = CompatDriver::spawn(false);
    let expected_native = mcp_tool_names(&mut native_mcp);
    assert_eq!(native.tool_names(), expected_native);
    assert_eq!(
        compat.tool_names(),
        vec![
            "list_apps",
            "get_app_state",
            "click",
            "perform_secondary_action",
            "set_value",
            "select_text",
            "scroll",
            "drag",
            "press_key",
            "type_text",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>()
    );
    assert!(native.child.try_wait().unwrap().is_none());
    assert!(compat.child.try_wait().unwrap().is_none());

    drop(native_mcp);
    drop(compat);
    drop(native);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn explicit_socket_proxy_rejects_mismatched_daemon_profile() {
    use std::os::unix::fs::PermissionsExt;

    let root = unique_daemon_root();
    std::fs::create_dir_all(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let native_socket = root.join("native.sock");
    let compat_socket = root.join("compat.sock");
    let native = DaemonDriver::spawn(native_socket.clone(), false);
    let compat = DaemonDriver::spawn(compat_socket.clone(), true);

    let matching_compat = run_proxy_to_explicit_socket(&compat_socket, true);
    assert!(
        matching_compat.status.success(),
        "matching compat proxy must authenticate its approval broker: {}",
        String::from_utf8_lossy(&matching_compat.stderr)
    );

    let compat_to_native = run_proxy_to_explicit_socket(&native_socket, true);
    assert!(!compat_to_native.status.success());
    let stderr = String::from_utf8_lossy(&compat_to_native.stderr);
    assert!(
        stderr.contains("daemon profile mismatch")
            && stderr.contains("MCP requested `codex-computer-use-compat`")
            && stderr.contains("daemon reports `native`"),
        "unexpected compat-to-native error: {stderr}"
    );

    let native_to_compat = run_proxy_to_explicit_socket(&compat_socket, false);
    assert!(!native_to_compat.status.success());
    let stderr = String::from_utf8_lossy(&native_to_compat.stderr);
    assert!(
        stderr.contains("daemon profile mismatch")
            && stderr.contains("MCP requested `native`")
            && stderr.contains("daemon reports `codex-computer-use-compat`"),
        "unexpected native-to-compat error: {stderr}"
    );

    drop(compat);
    drop(native);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn compat_approval_broker_fails_closed_without_signed_codex_parent() {
    use std::os::unix::fs::PermissionsExt;

    let root = unique_daemon_root();
    std::fs::create_dir_all(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let socket = root.join("compat.sock");
    let daemon = DaemonDriver::spawn_with_unverified_client_override(socket.clone(), true, false);

    let output = run_proxy_to_explicit_socket(&socket, true);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Only the signed OpenAI Codex app may use this profile"),
        "unexpected unsigned-parent rejection: {stderr}"
    );

    drop(daemon);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn raw_client_cannot_reuse_a_live_compat_session_without_its_broker_token() {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::mpsc;

    let root = unique_daemon_root();
    std::fs::create_dir_all(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let socket = root.join("compat.sock");
    let daemon = DaemonDriver::spawn(socket.clone(), true);

    let mut proxy = Command::new(env!("CARGO_BIN_EXE_cua-driver"))
        .arg("mcp")
        .arg("--socket")
        .arg(&socket)
        .arg("--codex-computer-use-compat")
        .arg("--no-overlay")
        .env("CUA_DRIVER_RS_MCP_FORCE_PROXY", "1")
        .env("CUA_LOG", "cua_driver::proxy=debug")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut proxy_stdin = proxy.stdin.take().unwrap();
    let mut proxy_stdout = BufReader::new(proxy.stdout.take().unwrap());
    let proxy_stderr = proxy.stderr.take().unwrap();
    let (session_tx, session_rx) = mpsc::channel();
    let stderr_reader = std::thread::spawn(move || {
        let mut session_id = None;
        let mut sent = false;
        for line in BufReader::new(proxy_stderr).lines().map_while(Result::ok) {
            if let Some(field) = line.split_whitespace().find_map(|field| {
                field
                    .strip_prefix("session_id=")
                    .map(|value| value.trim_matches('"').to_owned())
            }) {
                session_id = Some(field);
            }
            if !sent && line.contains("control connection established") {
                let _ = session_tx.send(
                    session_id
                        .clone()
                        .expect("control log includes session id"),
                );
                sent = true;
            }
        }
    });
    let session_id = session_rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("proxy established authenticated control session");

    writeln!(
        proxy_stdin,
        "{}",
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {"name": "not_a_real_tool", "arguments": {}},
        })
    )
    .unwrap();
    proxy_stdin.flush().unwrap();
    let mut proxy_line = String::new();
    proxy_stdout.read_line(&mut proxy_line).unwrap();
    assert!(
        proxy_line.contains("Unknown tool: not_a_real_tool"),
        "authenticated proxy call did not reach the registry: {proxy_line}"
    );

    for args in [
        serde_json::json!({}),
        serde_json::json!({"_cua_approval_broker_token": "caller-forged"}),
    ] {
        let response = send_raw_daemon_request(
            &socket,
            serde_json::json!({
                "method": "call",
                "name": "list_apps",
                "args": args,
                "session_id": session_id,
            }),
        );
        assert_eq!(response["ok"], false);
        assert_eq!(response["exit_code"], 77);
        assert!(
            response["error"]
                .as_str()
                .unwrap()
                .contains("authenticated MCP control session")
                || response["error"]
                    .as_str()
                    .unwrap()
                    .contains("broker authentication failed"),
            "unexpected raw-client rejection: {response}"
        );
    }

    let _ = proxy.kill();
    let _ = proxy.wait();
    stderr_reader.join().unwrap();
    drop(daemon);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn status_and_stop_manage_native_and_compat_daemons_independently() {
    let home = tempfile::Builder::new()
        .prefix("cua-lc-")
        .tempdir_in("/tmp")
        .expect("short isolated HOME");
    let mut native = spawn_default_profile_daemon(home.path(), false);
    let mut compat = spawn_default_profile_daemon(home.path(), true);

    let native_status = run_profile_lifecycle_command(home.path(), "status", false);
    assert!(
        native_status.status.success(),
        "native status: {native_status:?}"
    );
    assert!(
        String::from_utf8_lossy(&native_status.stdout).contains(native.socket.to_str().unwrap())
    );

    let compat_status = run_profile_lifecycle_command(home.path(), "status", true);
    assert!(
        compat_status.status.success(),
        "compat status: {compat_status:?}"
    );
    assert!(
        String::from_utf8_lossy(&compat_status.stdout).contains(compat.socket.to_str().unwrap())
    );

    let stop_compat = run_profile_lifecycle_command(home.path(), "stop", true);
    assert!(stop_compat.status.success(), "compat stop: {stop_compat:?}");
    assert!(compat
        .child
        .wait()
        .expect("wait for compat daemon")
        .success());
    assert!(
        native.child.try_wait().unwrap().is_none(),
        "compat stop must not terminate the native daemon"
    );
    assert!(run_profile_lifecycle_command(home.path(), "status", false)
        .status
        .success());
    assert!(!run_profile_lifecycle_command(home.path(), "status", true)
        .status
        .success());

    let stop_native = run_profile_lifecycle_command(home.path(), "stop", false);
    assert!(stop_native.status.success(), "native stop: {stop_native:?}");
    assert!(native
        .child
        .wait()
        .expect("wait for native daemon")
        .success());
}

#[test]
fn compat_proxy_brokers_nested_app_approval_and_retries_once() {
    let root = unique_daemon_root();
    std::fs::create_dir_all(&root).unwrap();
    let socket = root.join("approval.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let stopping = Arc::new(AtomicBool::new(false));
    let calls = Arc::new(AtomicUsize::new(0));
    let resolutions = Arc::new(AtomicUsize::new(0));
    let server = {
        let stopping = stopping.clone();
        let calls = calls.clone();
        let resolutions = resolutions.clone();
        std::thread::spawn(move || {
            let mut workers = Vec::new();
            while !stopping.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let calls = calls.clone();
                        let resolutions = resolutions.clone();
                        workers.push(std::thread::spawn(move || {
                            let mut reader = BufReader::new(stream.try_clone().unwrap());
                            let mut line = String::new();
                            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                                return;
                            }
                            let request: Value = serde_json::from_str(line.trim()).unwrap();
                            let method = request["method"].as_str().unwrap_or("");
                            let response = match method {
                                "session_begin" => serde_json::json!({
                                    "ok": true,
                                    "result": {
                                        "session_begin": true,
                                        "profile": "codex-computer-use-compat",
                                        "approval_broker_token": "daemon-broker-token",
                                    }
                                }),
                                "list" => {
                                    let names = [
                                        "list_apps",
                                        "get_app_state",
                                        "click",
                                        "perform_secondary_action",
                                        "set_value",
                                        "select_text",
                                        "scroll",
                                        "drag",
                                        "press_key",
                                        "type_text",
                                    ];
                                    serde_json::json!({
                                        "ok": true,
                                        "result": {
                                            "profile": "codex-computer-use-compat",
                                            "capability_version": "1",
                                            "schema_version": "1",
                                            "tools": names.iter().map(|name| serde_json::json!({
                                                "name": name,
                                                "description": name,
                                                "input_schema": {"type":"object","properties":{}},
                                                "read_only": false,
                                                "destructive": false,
                                                "idempotent": false,
                                                "open_world": false,
                                            })).collect::<Vec<_>>(),
                                        }
                                    })
                                }
                                "call" => {
                                    assert_eq!(
                                        request["args"]["_cua_approval_broker_token"],
                                        "daemon-broker-token"
                                    );
                                    let attempt = calls.fetch_add(1, Ordering::SeqCst);
                                    if attempt == 0 {
                                        serde_json::json!({
                                            "ok": false,
                                            "error": "approval required",
                                            "exit_code": 1,
                                            "result": {
                                                "content": [{"type":"text","text":"approval required"}],
                                                "isError": true,
                                                "structuredContent": {
                                                    "code": "app_approval_required",
                                                    "message": "approval required",
                                                    "approval": {
                                                        "challenge": "challenge-1",
                                                        "app": "com.apple.calculator",
                                                        "displayName": "Calculator",
                                                        "allowPersistentApproval": true,
                                                    }
                                                }
                                            }
                                        })
                                    } else {
                                        serde_json::json!({
                                            "ok": true,
                                            "result": {
                                                "content": [{"type":"text","text":"approved state"}],
                                                "isError": false,
                                            }
                                        })
                                    }
                                }
                                "app_approval_resolve" => {
                                    resolutions.fetch_add(1, Ordering::SeqCst);
                                    assert_eq!(request["args"]["challenge"], "challenge-1");
                                    assert_eq!(
                                        request["args"]["broker_token"],
                                        "daemon-broker-token"
                                    );
                                    assert_eq!(request["args"]["action"], "accept");
                                    assert_eq!(request["args"]["persist"], "session");
                                    serde_json::json!({
                                        "ok": true,
                                        "result": {"resolution":"approved","persistent":false}
                                    })
                                }
                                other => panic!("unexpected fake daemon method {other}"),
                            };
                            writeln!(stream, "{response}").unwrap();
                            stream.flush().unwrap();
                            if method == "session_begin" {
                                loop {
                                    line.clear();
                                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                                        break;
                                    }
                                }
                            }
                        }));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    Err(error) => panic!("fake daemon accept failed: {error}"),
                }
            }
            for worker in workers {
                let _ = worker.join();
            }
        })
    };

    let mut child = Command::new(env!("CARGO_BIN_EXE_cua-driver"))
        .arg("mcp")
        .arg("--socket")
        .arg(&socket)
        .arg("--codex-computer-use-compat")
        .arg("--no-overlay")
        .env("CUA_DRIVER_RS_MCP_FORCE_PROXY", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    writeln!(
        stdin,
        "{}",
        serde_json::json!({
            "jsonrpc":"2.0",
            "id":1,
            "method":"initialize",
            "params":{"capabilities":{"elicitation":{}}},
        })
    )
    .unwrap();
    stdin.flush().unwrap();
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    assert_eq!(serde_json::from_str::<Value>(line.trim()).unwrap()["id"], 1);

    writeln!(
        stdin,
        "{}",
        serde_json::json!({"jsonrpc":"2.0","id":10,"method":"tools/list"})
    )
    .unwrap();
    stdin.flush().unwrap();
    line.clear();
    stdout.read_line(&mut line).unwrap();
    let tools_response: Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(
        tools_response["result"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        vec!["tools"]
    );
    for tool in tools_response["result"]["tools"].as_array().unwrap() {
        assert_eq!(
            tool.as_object().unwrap().keys().collect::<Vec<_>>(),
            vec!["annotations", "description", "inputSchema", "name"]
        );
    }

    writeln!(
        stdin,
        "{}",
        serde_json::json!({
            "jsonrpc":"2.0",
            "id":2,
            "method":"tools/call",
            "params":{"name":"get_app_state","arguments":{"app":"Calculator"}},
        })
    )
    .unwrap();
    stdin.flush().unwrap();
    line.clear();
    stdout.read_line(&mut line).unwrap();
    let elicitation: Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(elicitation["method"], "elicitation/create");
    assert_eq!(
        elicitation["params"]["message"],
        "Allow Computer Use to use \"Calculator\"?"
    );
    assert_eq!(
        elicitation["params"]["_meta"]["persist"],
        serde_json::json!(["session", "always"])
    );
    writeln!(
        stdin,
        "{}",
        serde_json::json!({
            "jsonrpc":"2.0",
            "id":elicitation["id"],
            "result":{"action":"accept"},
        })
    )
    .unwrap();
    stdin.flush().unwrap();
    line.clear();
    stdout.read_line(&mut line).unwrap();
    let response: Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(response["id"], 2);
    assert_eq!(response["result"]["isError"], false);

    writeln!(
        stdin,
        "{}",
        serde_json::json!({
            "jsonrpc":"2.0",
            "id":3,
            "method":"tools/call",
            "params":{"name":"click","arguments":{"app":"Calculator","x":10,"y":10}},
        })
    )
    .unwrap();
    stdin.flush().unwrap();
    line.clear();
    stdout.read_line(&mut line).unwrap();
    let action_response: Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(action_response["id"], 3);
    assert_eq!(action_response["result"]["isError"], false);

    assert_eq!(calls.load(Ordering::SeqCst), 3);
    assert_eq!(resolutions.load(Ordering::SeqCst), 1);

    let _ = child.kill();
    let _ = child.wait();
    stopping.store(true, Ordering::SeqCst);
    server.join().unwrap();
    let _ = std::fs::remove_dir_all(root);
}
