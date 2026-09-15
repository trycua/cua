#![cfg(target_os = "linux")]

#[path = "support/keyboard_queue.rs"]
mod keyboard_queue;

use cua_driver_core::action_record::ActionEffect;
use cua_driver_core::tool::ToolRegistry;
use cua_driver_testkit::{
    keyboard_fixture::{wait_for_x11_focus, X11KeyboardObserver},
    spawn_in_job, ChildReaper,
};
use keyboard_queue::OverlayBarrier;
use serde_json::{json, Value};
use std::net::UdpSocket;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::Arc;
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

    fn assert_quiet(&self) {
        self.socket
            .set_read_timeout(Some(Duration::from_millis(150)))
            .unwrap();
        let mut extra = [0; 128];
        let result = self.socket.recv(&mut extra);
        self.socket
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        assert!(
            matches!(result, Err(ref error) if matches!(error.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut)),
            "unexpected terminal input: {result:?} {extra:?}"
        );
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
    terminal.assert_quiet();
    assert!(
        !observer.events().is_empty(),
        "element-targeted keyboard input bypassed the native GTK entry"
    );
}

async fn token_replaced_while_queued(tool: &'static str, fields: Value, receipt: &str) {
    let terminal = Arc::new(Terminal::new());
    for session in ["keyboard-held", "keyboard-queued"] {
        let result = terminal
            .registry
            .invoke_with_context(
                "start_session",
                json!({"session":session}),
                terminal.context.clone(),
            )
            .await;
        assert_ne!(result.is_error, Some(true), "{result:?}");
    }
    let token = terminal.entry_token().await;
    let observer = X11KeyboardObserver::start();
    let barrier = OverlayBarrier::install();
    let first_terminal = terminal.clone();
    let first_token = token.clone();
    let first = tokio::spawn(async move {
        first_terminal.call("press_key", json!({"session":"keyboard-held", "element_token":first_token, "key":"return", "delivery_mode":"foreground"})).await
    });
    tokio::time::timeout(Duration::from_secs(10), barrier.finished.notified())
        .await
        .unwrap();
    assert_eq!(terminal.receive(), "gui");
    assert_eq!(terminal.receive(), "pty");
    assert!(!observer.events().is_empty());
    let mut queued_args = fields.clone();
    queued_args["session"] = json!("keyboard-queued");
    queued_args["element_token"] = json!(token);
    queued_args["delivery_mode"] = json!("foreground");
    let queued_terminal = terminal.clone();
    let queued = tokio::spawn(async move { queued_terminal.call(tool, queued_args).await });
    tokio::time::timeout(Duration::from_secs(10), barrier.queued.notified())
        .await
        .unwrap();
    terminal.assert_quiet();
    let fresh_token = tokio::time::timeout(Duration::from_secs(10), terminal.entry_token())
        .await
        .unwrap();
    assert_ne!(fresh_token, token);
    barrier.release();
    let first_result = tokio::time::timeout(Duration::from_secs(10), first)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(first_result.is_error, Some(true), "{first_result:?}");
    let result = tokio::time::timeout(Duration::from_secs(10), queued)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.is_error, Some(true), "{result:?}");
    assert_eq!(
        result.structured_content.as_ref().unwrap()["refusal"]["code"],
        "stale_element_token",
        "{result:?}"
    );
    let record = result
        .action_record
        .as_ref()
        .expect("pre-input refusal record");
    assert_eq!(record.effect, ActionEffect::Refused, "{result:?}");
    assert!(record.actual_delivery.is_none(), "{result:?}");
    assert!(record.public_result().unwrap().evidence.is_none());
    terminal.assert_quiet();
    assert!(
        observer.events().is_empty(),
        "retired queued target emitted native input"
    );
    let mut recovery = fields;
    recovery["element_token"] = json!(fresh_token);
    recovery["delivery_mode"] = json!("foreground");
    let result = terminal.call(tool, recovery).await;
    assert_ne!(result.is_error, Some(true), "{result:?}");
    assert_eq!(terminal.receive(), receipt);
    assert_eq!(terminal.receive(), "pty");
    terminal.assert_quiet();
    assert!(!observer.events().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
#[ignore = "requires isolated X11/Openbox, D-Bus/AT-SPI, and Python GTK3 introspection"]
async fn queued_press_key_rejects_a_replaced_token_before_native_admission() {
    token_replaced_while_queued("press_key", json!({"key":"return"}), "gui").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
#[ignore = "requires isolated X11/Openbox, D-Bus/AT-SPI, and Python GTK3 introspection"]
async fn queued_hotkey_rejects_a_replaced_token_before_native_admission() {
    token_replaced_while_queued("hotkey", json!({"keys":["ctrl","return"]}), "gui_ctrl").await;
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
