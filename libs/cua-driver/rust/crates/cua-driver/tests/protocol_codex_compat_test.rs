//! Wire-level contract for `--codex-computer-use-compat`.

#![cfg(target_os = "macos")]

use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

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
        let mut command = Command::new(env!("CARGO_BIN_EXE_cua-driver"));
        command
            .arg("serve")
            .arg("--socket")
            .arg(&socket)
            .arg("--no-permissions-gate")
            .arg("--no-overlay")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if compat {
            command.arg("--codex-computer-use-compat");
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

#[test]
fn compat_tools_list_is_exact_v829_contract() {
    let mut driver = CompatDriver::spawn(true);
    driver.send(serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/list"
    }));
    let response = driver.recv();
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
        ("perform_secondary_action", &["app", "element_index", "action"]),
        ("set_value", &["app", "element_index", "value"]),
        ("select_text", &["app", "element_index", "text"]),
        ("scroll", &["app", "element_index", "direction"]),
        ("drag", &["app", "from_x", "from_y", "to_x", "to_y"]),
        ("press_key", &["app", "key"]),
        ("type_text", &["app", "text"]),
    ];
    for (name, expected) in expected_properties {
        let tool = tools.iter().find(|tool| tool["name"] == *name).unwrap();
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
        for (property, schema) in tool["inputSchema"]["properties"]
            .as_object()
            .unwrap()
        {
            let expected_type = match property.as_str() {
                "click_count" => "integer",
                "x" | "y" | "from_x" | "from_y" | "to_x" | "to_y" | "pages" => {
                    "number"
                }
                "app" | "element_index" | "mouse_button" | "action" | "value"
                | "text" | "prefix" | "suffix" | "selection" | "direction" | "key" => {
                    "string"
                }
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
