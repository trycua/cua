//! One embedded daemon, two public client models.
//!
//! The SDK route uses the generated UniFFI implementation crate directly; the
//! agent route uses the ordinary stdio MCP proxy returned by the same host.

use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use cua_driver_contract::{CaptureScope, GetSessionStateInput, StartSessionInput};
use cua_driver_sdk::{CuaDriver, EmbeddedCuaDriverHost, EmbeddedDriverHostState};
use cua_driver_testkit::{spawn_in_job, ChildReaper};
use serde_json::{json, Value};

fn read_json(reader: &mut BufReader<std::process::ChildStdout>) -> Value {
    let mut line = String::new();
    reader.read_line(&mut line).expect("read MCP response");
    serde_json::from_str(line.trim()).expect("parse MCP response")
}

fn tool_names(value: &Value, field: &str) -> BTreeSet<String> {
    value[field]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|tool| tool["name"].as_str().expect("tool name").to_owned())
        .collect()
}

#[cfg(unix)]
fn process_is_alive(pid: u32) -> bool {
    // Signal zero performs existence/permission validation without delivering
    // a signal. The child has the same uid, so EPERM cannot mask a live child.
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(not(unix))]
fn process_is_alive(_pid: u32) -> bool {
    // Named-pipe disappearance remains the portable assertion. Windows CI
    // additionally runs the host in a kill-on-close Job Object.
    false
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn embedded_host_serves_sdk_and_mcp_with_one_contract() {
    let host = EmbeddedCuaDriverHost::new(
        env!("CARGO_BIN_EXE_cua-driver").into(),
        "com.trycua.embedded-contract-test".into(),
    )
    .expect("construct host");
    let connection = host.clone().start().await.expect("start embedded host");
    assert_eq!(host.state(), EmbeddedDriverHostState::Ready);

    let sdk = CuaDriver::connect(Some(connection.socket_path.clone())).expect("connect SDK");
    let metadata = sdk.metadata().expect("SDK metadata handshake");
    assert!(metadata.embedded);
    assert_eq!(metadata.pid, connection.pid);
    assert_eq!(metadata.contract_version, connection.contract_version);
    assert_eq!(
        metadata.host_bundle_id.as_deref(),
        Some("com.trycua.embedded-contract-test")
    );
    let sdk_tools: Value =
        serde_json::from_str(&sdk.list_tools_json().expect("SDK tools/list")).unwrap();

    let mut proxy_command = Command::new(&connection.mcp.command);
    proxy_command
        .args(&connection.mcp.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for variable in &connection.mcp.environment {
        proxy_command.env(&variable.name, &variable.value);
    }
    let mut proxy = spawn_in_job(&mut proxy_command).expect("spawn MCP proxy");
    let mut stdin = proxy.stdin.take().unwrap();
    let mut stdout = BufReader::new(proxy.stdout.take().unwrap());

    writeln!(
        stdin,
        "{}",
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": connection.mcp_protocol_version,
                "capabilities": {},
                "clientInfo": {"name": "embedded-contract-test", "version": "1"}
            }
        })
    )
    .unwrap();
    stdin.flush().unwrap();
    let initialize = read_json(&mut stdout);
    assert_eq!(initialize["id"], 1);
    assert!(initialize.get("result").is_some(), "{initialize:?}");

    writeln!(
        stdin,
        "{}",
        json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}})
    )
    .unwrap();
    writeln!(
        stdin,
        "{}",
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}})
    )
    .unwrap();
    stdin.flush().unwrap();
    let listed = read_json(&mut stdout);
    let mcp_tools = tool_names(&listed["result"], "tools");
    assert_eq!(mcp_tools, tool_names(&sdk_tools, "tools"));
    assert!(mcp_tools.contains("get_desktop_state"));
    assert!(mcp_tools.contains("start_session"));

    let sdk_session = sdk
        .start_session(StartSessionInput {
            session: "embedded-sdk-window".into(),
            capture_scope: Some(CaptureScope::Window),
        })
        .expect("start SDK-owned session");
    assert_eq!(sdk_session.state.capture_scope, CaptureScope::Window);

    writeln!(
        stdin,
        "{}",
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "start_session",
                "arguments": {
                    "session": "embedded-mcp-desktop",
                    "capture_scope": "desktop"
                }
            }
        })
    )
    .unwrap();
    stdin.flush().unwrap();
    let mcp_started = read_json(&mut stdout);
    assert_eq!(
        mcp_started["result"]["structuredContent"]["capture_scope"],
        "desktop"
    );

    // Session policy belongs to the session, not the daemon or transport:
    // one SDK session can remain strict-window while an MCP session sharing
    // the same embedded host is strict-desktop.
    let sdk_state = sdk
        .get_session_state(GetSessionStateInput {
            session: "embedded-sdk-window".into(),
        })
        .expect("read SDK session");
    let mcp_state = sdk
        .get_session_state(GetSessionStateInput {
            session: "embedded-mcp-desktop".into(),
        })
        .expect("read MCP-created session through SDK");
    assert_eq!(sdk_state.capture_scope, CaptureScope::Window);
    assert_eq!(mcp_state.capture_scope, CaptureScope::Desktop);

    drop(stdin);
    let mut reaper = ChildReaper::new();
    reaper.push(proxy);
    host.clone().stop().await.expect("stop embedded host");
    assert_eq!(host.state(), EmbeddedDriverHostState::Stopped);
    assert!(!std::path::Path::new(&connection.socket_path).exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_the_host_cannot_orphan_its_daemon() {
    let host = EmbeddedCuaDriverHost::new(
        env!("CARGO_BIN_EXE_cua-driver").into(),
        "com.trycua.embedded-drop-test".into(),
    )
    .expect("construct host");
    let connection = host.clone().start().await.expect("start embedded host");
    drop(host);

    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if !CuaDriver::connect(Some(connection.socket_path.clone()))
            .unwrap()
            .is_available()
            && !process_is_alive(connection.pid)
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!(
        "embedded daemon survived host drop: pid={}, endpoint={}",
        connection.pid, connection.socket_path
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_start_coalesces_and_restart_rotates_generation() {
    let host = EmbeddedCuaDriverHost::new(
        env!("CARGO_BIN_EXE_cua-driver").into(),
        "com.trycua.embedded-lifecycle-test".into(),
    )
    .expect("construct host");

    let (left, right, third) = tokio::join!(
        host.clone().start(),
        host.clone().start(),
        host.clone().start()
    );
    let left = left.expect("first start");
    let right = right.expect("second start");
    let third = third.expect("third start");
    assert_eq!(left, right);
    assert_eq!(left, third);

    let first_generation = left.generation.clone();
    let restarted = host.clone().restart().await.expect("restart host");
    assert_ne!(restarted.generation, first_generation);
    assert_eq!(host.connection().as_ref(), Some(&restarted));

    host.clone().stop().await.expect("stop host");
    let exit = host
        .clone()
        .wait_for_exit(restarted.generation.clone())
        .await
        .expect("observe stopped generation");
    assert_eq!(exit.generation, restarted.generation);
    assert!(
        exit.success,
        "graceful lifetime-pipe shutdown should succeed"
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_cancels_startup_and_unblocks_every_waiter() {
    use std::os::unix::fs::PermissionsExt as _;

    let directory = tempfile::tempdir().unwrap();
    let binary = directory.path().join("never-ready-driver");
    std::fs::write(&binary, "#!/bin/sh\nread _\n").unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
    let host = EmbeddedCuaDriverHost::new(
        binary.to_string_lossy().into_owned(),
        "com.trycua.embedded-cancel-test".into(),
    )
    .expect("construct host");

    let starting = tokio::spawn(host.clone().start());
    let deadline = Instant::now() + Duration::from_secs(2);
    while host.state() != EmbeddedDriverHostState::Starting && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(host.state(), EmbeddedDriverHostState::Starting);

    host.clone().stop().await.expect("cancel startup");
    let start_error = starting.await.unwrap().unwrap_err();
    assert!(matches!(
        start_error,
        cua_driver_sdk::EmbeddedDriverError::StartupCancelled
    ));
    assert_eq!(host.state(), EmbeddedDriverHostState::Stopped);
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn early_child_exit_resets_the_host_to_stopped() {
    use std::os::unix::fs::PermissionsExt as _;

    let directory = tempfile::tempdir().unwrap();
    let binary = directory.path().join("early-exit-driver");
    std::fs::write(&binary, "#!/bin/sh\nexit 7\n").unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
    let host = EmbeddedCuaDriverHost::new(
        binary.to_string_lossy().into_owned(),
        "com.trycua.embedded-early-exit-test".into(),
    )
    .expect("construct host");
    let error = host.clone().start().await.unwrap_err();
    assert!(matches!(
        error,
        cua_driver_sdk::EmbeddedDriverError::ExitedBeforeReady { code: Some(7) }
    ));
    assert_eq!(host.state(), EmbeddedDriverHostState::Stopped);
}
