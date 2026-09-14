#![cfg(target_os = "linux")]

use cua_driver_core::tool::ToolRegistry;
use cua_driver_testkit::{
    keyboard_fixture::{wait_for_x11_focus, X11KeyboardObserver},
    spawn_in_job, ChildReaper,
};
use serde_json::{json, Value};
use std::net::UdpSocket;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

struct Terminal {
    pid: u32,
    window: u64,
    socket: UdpSocket,
    registry: ToolRegistry,
    context: std::sync::Arc<cua_driver_core::session_authorization::EffectiveAuthorizationContext>,
    _reaper: ChildReaper,
    _directory: tempfile::TempDir,
}

impl Terminal {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("terminal.py");
        std::fs::write(&script, include_str!("fixtures/keyboard_terminal.py")).unwrap();
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let child = spawn_in_job(
            Command::new("/usr/bin/python3")
                .arg0("xterm")
                .arg("-u")
                .arg(script)
                .arg(socket.local_addr().unwrap().port().to_string())
                .env("GDK_BACKEND", "x11")
                .env("NO_AT_BRIDGE", "0")
                .stdin(Stdio::null())
                .stdout(Stdio::null()),
        )
        .unwrap();
        let pid = child.id();
        let mut reaper = ChildReaper::new();
        reaper.push(child);
        let context = cua_driver_core::session_authorization::configured_registry()
            .unwrap()
            .legacy_context()
            .unwrap();
        let registry =
            cua_driver_core::tool::with_runtime_scope(context.runtime_scope_key(), || {
                platform_linux::tools::build_registry(false)
            });
        let mut fixture = Self {
            pid,
            window: 0,
            socket,
            registry,
            context,
            _reaper: reaper,
            _directory: directory,
        };
        let mut pty_ready = false;
        while fixture.window == 0 || !pty_ready {
            let message = fixture.receive();
            if message == "pty_ready" {
                pty_ready = true;
            } else if let Some(window) = message.strip_prefix("gui_ready:") {
                fixture.window = window.parse().unwrap();
            } else {
                panic!("unexpected fixture startup: {message}");
            }
        }
        wait_for_x11_focus(fixture.window);
        fixture
    }

    fn receive(&self) -> String {
        let mut buffer = [0; 128];
        let size = self
            .socket
            .recv(&mut buffer)
            .expect("native terminal oracle receipt");
        String::from_utf8(buffer[..size].to_vec()).unwrap()
    }

    async fn call(&self, tool: &str, mut fields: Value) -> cua_driver_core::protocol::ToolResult {
        fields["pid"] = json!(self.pid);
        fields["window_id"] = json!(self.window);
        self.registry
            .invoke_with_context(tool, fields, self.context.clone())
            .await
    }

    async fn entry_token(&self) -> String {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let snapshot = self
                .call("get_window_state", json!({"include_screenshot":false}))
                .await;
            if let Some(elements) = snapshot
                .structured_content
                .as_ref()
                .and_then(|value| value["elements"].as_array())
            {
                if let Some(token) = elements
                    .iter()
                    .find(|element| element["label"] == "Terminal input")
                    .and_then(|element| element["element_token"].as_str())
                {
                    return token.into();
                }
            }
            assert!(
                Instant::now() < deadline,
                "native terminal entry never became accessible: {snapshot:?}"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

async fn targeted_input(tool: &str, fields: Value, gui_event: &str) {
    let terminal = Terminal::new();
    let observer = X11KeyboardObserver::start();
    let control = terminal
        .call(
            "press_key",
            json!({"key":"return", "delivery_mode":"foreground"}),
        )
        .await;
    assert_ne!(control.is_error, Some(true), "{control:?}");
    assert_eq!(terminal.receive(), "pty");
    assert!(
        observer.events().is_empty(),
        "bare Return did not take the eligible PTY shortcut"
    );
    let token = terminal.entry_token().await;
    let mut args = fields;
    args["element_token"] = json!(token);
    args["delivery_mode"] = json!("foreground");
    let result = terminal.call(tool, args).await;
    assert_ne!(result.is_error, Some(true), "{result:?}");
    assert_eq!(terminal.receive(), gui_event);
    assert_eq!(terminal.receive(), "pty");
    terminal
        .socket
        .set_read_timeout(Some(Duration::from_millis(150)))
        .unwrap();
    let mut extra = [0; 128];
    assert!(
        matches!(terminal.socket.recv(&mut extra), Err(error) if matches!(error.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut)),
        "duplicate terminal keyboard delivery"
    );
    assert!(
        !observer.events().is_empty(),
        "element-targeted keyboard input bypassed the native GTK entry"
    );
}

#[tokio::test]
#[ignore = "requires isolated X11/Openbox, D-Bus/AT-SPI, and Python GTK3 introspection"]
async fn valid_terminal_press_key_element_bypasses_the_pty_shortcut() {
    targeted_input("press_key", json!({"key":"return"}), "gui").await;
}

#[tokio::test]
#[ignore = "requires isolated X11/Openbox, D-Bus/AT-SPI, and Python GTK3 introspection"]
async fn valid_terminal_hotkey_element_reaches_the_exact_native_entry() {
    targeted_input("hotkey", json!({"keys":["ctrl","return"]}), "gui_ctrl").await;
}
