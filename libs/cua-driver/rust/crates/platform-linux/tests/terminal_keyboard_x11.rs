#![cfg(target_os = "linux")]

use anyhow::{bail, Context, Result};
use cua_driver_core::protocol::ToolResult;
use cua_driver_core::tool::ToolRegistry;
use serde_json::{json, Value};
use std::net::UdpSocket;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use x11rb::protocol::xproto::*;

const ORACLE: &str = r#"
import socket, sys
observer = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
address = ('127.0.0.1', int(sys.argv[1]))
observer.sendto(b'ready', address)
for line in sys.stdin:
    observer.sendto(line.encode(), address)
"#;

struct Terminal {
    child: Child,
    socket: UdpSocket,
    window: u64,
    registry: ToolRegistry,
}

impl Terminal {
    fn new() -> Result<Self> {
        let socket = UdpSocket::bind("127.0.0.1:0")?;
        socket.set_read_timeout(Some(Duration::from_secs(5)))?;
        let child = Command::new("xterm")
            .args([
                "-title",
                "Cua terminal keyboard test",
                "-xrm",
                "XTerm*allowSendEvents: false",
                "-e",
                "python3",
                "-u",
                "-c",
                ORACLE,
            ])
            .arg(socket.local_addr()?.port().to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .context("requires xterm, python3, and an isolated X11 window manager")?;
        let mut terminal = Self {
            child,
            socket,
            window: 0,
            registry: platform_linux::tools::register_all(),
        };
        assert_eq!(terminal.receive()?, b"ready");
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(window) =
                platform_linux::x11::list_windows(Some(terminal.child.id())).first()
            {
                terminal.window = window.xid;
                cua_driver_testkit::keyboard_fixture::wait_for_x11_focus(window.xid);
                return Ok(terminal);
            }
            if terminal.child.try_wait()?.is_some() || Instant::now() >= deadline {
                bail!("terminal fixture did not publish its exact window");
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    async fn call(&self, tool: &str, fields: Value) -> ToolResult {
        let mut args = json!({"pid":self.child.id(),"window_id":self.window});
        args.as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        self.registry.invoke(tool, args).await
    }

    fn receive(&self) -> Result<Vec<u8>> {
        let mut bytes = [0; 4096];
        let count = self.socket.recv(&mut bytes)?;
        Ok(bytes[..count].to_vec())
    }

    fn assert_no_line(&self) -> Result<()> {
        self.socket
            .set_read_timeout(Some(Duration::from_millis(150)))?;
        let mut bytes = [0; 4096];
        match self.socket.recv(&mut bytes) {
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                Ok(())
            }
            other => bail!("unexpected terminal input: {other:?}"),
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

async fn terminal_alias(key: &str) -> Result<()> {
    let terminal = Terminal::new()?;
    let (conn, _) = x11rb::connect(None)?;
    let focus = conn.get_input_focus()?.reply()?.focus;
    let response = terminal
        .call("press_key", json!({"key":key,"delivery_mode":"background"}))
        .await;
    assert_ne!(response.is_error, Some(true), "{response:?}");
    assert_eq!(
        terminal
            .receive()
            .with_context(|| format!("terminal did not consume {key}"))?,
        b"\n"
    );
    assert_eq!(conn.get_input_focus()?.reply()?.focus, focus);
    let output = response.structured_content.unwrap();
    assert_eq!(output["effect"], "unverifiable");
    assert_eq!(output["route"], "synthetic_events");
    assert_eq!(output["delivery"]["mode"], "background");
    assert!(output["evidence"].is_null());
    Ok(())
}

#[tokio::test]
#[ignore = "requires an isolated X11 desktop, xterm, and permitted descendant PTY borrowing"]
async fn terminal_enter_reaches_the_application() -> Result<()> {
    terminal_alias("enter").await
}

#[tokio::test]
#[ignore = "requires an isolated X11 desktop, xterm, and permitted descendant PTY borrowing"]
async fn terminal_return_reaches_the_application() -> Result<()> {
    terminal_alias("return").await
}

#[tokio::test]
#[ignore = "requires an isolated X11 desktop, xterm, and permitted descendant PTY borrowing"]
async fn terminal_aliases_are_case_insensitive() -> Result<()> {
    for key in ["Enter", "ENTER", "Return", "RETURN"] {
        terminal_alias(key).await?;
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires an isolated X11 desktop, xterm, and permitted descendant PTY borrowing"]
async fn modified_return_does_not_take_the_bare_key_pty_shortcut() -> Result<()> {
    let terminal = Terminal::new()?;
    let response = terminal
        .call(
            "press_key",
            json!({"key":"return","modifiers":["ctrl"],"delivery_mode":"background"}),
        )
        .await;
    assert_ne!(response.is_error, Some(true), "{response:?}");
    terminal.assert_no_line()
}

#[tokio::test]
#[ignore = "requires an isolated X11 desktop, xterm, and permitted descendant PTY borrowing"]
async fn hotkey_return_does_not_take_the_press_key_pty_shortcut() -> Result<()> {
    let terminal = Terminal::new()?;
    let response = terminal
        .call(
            "hotkey",
            json!({"keys":["ctrl","return"],"delivery_mode":"background"}),
        )
        .await;
    assert_ne!(response.is_error, Some(true), "{response:?}");
    terminal.assert_no_line()
}

async fn rejects_without_input(tool: &str, fields: Value) -> Result<()> {
    let terminal = Terminal::new()?;
    let (conn, _) = x11rb::connect(None)?;
    let before = conn.query_keymap()?.reply()?.keys;
    let response = terminal.call(tool, fields).await;
    assert_eq!(response.is_error, Some(true), "{response:?}");
    terminal.assert_no_line()?;
    assert_eq!(
        conn.query_keymap()?.reply()?.keys,
        before,
        "rejected call left a key held"
    );
    Ok(())
}

macro_rules! rejected_keyboard_case {
    ($name:ident, $tool:literal, $fields:expr) => {
        #[tokio::test]
        #[ignore = "requires an isolated X11 desktop and xterm"]
        async fn $name() -> Result<()> {
            rejects_without_input($tool, $fields).await
        }
    };
}

rejected_keyboard_case!(missing_press_key_is_rejected, "press_key", json!({}));
rejected_keyboard_case!(
    non_string_press_key_is_rejected,
    "press_key",
    json!({"key":17})
);
rejected_keyboard_case!(
    non_array_hotkey_is_rejected,
    "hotkey",
    json!({"keys":"ctrl+return"})
);
rejected_keyboard_case!(
    non_string_hotkey_member_is_rejected,
    "hotkey",
    json!({"keys":["ctrl",17]})
);
rejected_keyboard_case!(empty_hotkey_is_rejected, "hotkey", json!({"keys":[]}));
rejected_keyboard_case!(
    modifier_only_hotkey_is_rejected,
    "hotkey",
    json!({"keys":["ctrl","shift"]})
);
rejected_keyboard_case!(
    unknown_press_key_is_rejected,
    "press_key",
    json!({"key":"not-a-real-key"})
);
rejected_keyboard_case!(
    unknown_hotkey_is_rejected,
    "hotkey",
    json!({"keys":["ctrl","not-a-real-key"]})
);
rejected_keyboard_case!(
    malformed_press_token_is_rejected,
    "press_key",
    json!({"key":"return","element_token":"invalid"})
);
rejected_keyboard_case!(
    stale_press_token_is_rejected,
    "press_key",
    json!({"key":"return","element_token":"s00000000:0"})
);
rejected_keyboard_case!(
    bare_press_index_is_rejected,
    "press_key",
    json!({"key":"return","element_index":0})
);
rejected_keyboard_case!(
    malformed_hotkey_token_is_rejected,
    "hotkey",
    json!({"keys":["ctrl","return"],"element_token":"invalid"})
);
rejected_keyboard_case!(
    stale_hotkey_token_is_rejected,
    "hotkey",
    json!({"keys":["ctrl","return"],"element_token":"s00000000:0"})
);
rejected_keyboard_case!(
    bare_hotkey_index_is_rejected,
    "hotkey",
    json!({"keys":["ctrl","return"],"element_index":0})
);

async fn foreground_sequence(tool: &str, fields: Value, expected: &[u8]) -> Result<()> {
    let terminal = Terminal::new()?;
    let input = cua_driver_testkit::keyboard_fixture::X11KeyboardObserver::start();
    let response = terminal.call(tool, fields).await;
    assert_ne!(response.is_error, Some(true), "{response:?}");
    let flush = terminal
        .call(
            "press_key",
            json!({"key":"return","delivery_mode":"background"}),
        )
        .await;
    assert_ne!(flush.is_error, Some(true), "{flush:?}");
    assert_eq!(terminal.receive()?, expected);
    assert!(
        !input.events().is_empty(),
        "XTest must reach the independent raw-key observer"
    );
    assert_eq!(
        response.structured_content,
        Some(json!({
            "effect":"unverifiable", "route":"global_input", "delivery":{"mode":"foreground"}
        }))
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires an isolated X11 desktop, xterm, and permitted descendant PTY borrowing"]
async fn foreground_f5_reports_xtest_after_terminal_observes_it() -> Result<()> {
    foreground_sequence(
        "press_key",
        json!({"key":"f5","delivery_mode":"foreground"}),
        b"\x1b[15~\n",
    )
    .await
}

#[tokio::test]
#[ignore = "requires an isolated X11 desktop, xterm, and permitted descendant PTY borrowing"]
async fn foreground_hotkey_reports_xtest_after_terminal_observes_it() -> Result<()> {
    foreground_sequence(
        "hotkey",
        json!({"keys":["ctrl","a"],"delivery_mode":"foreground"}),
        b"\x01\n",
    )
    .await
}

#[tokio::test]
#[ignore = "requires an isolated X11 desktop, xterm, and permitted descendant PTY borrowing"]
async fn pty_execution_record_names_the_executed_transport() -> Result<()> {
    let terminal = Terminal::new()?;
    let response = terminal
        .call(
            "press_key",
            json!({"key":"return","delivery_mode":"background"}),
        )
        .await;
    assert_ne!(response.is_error, Some(true), "{response:?}");
    assert_eq!(terminal.receive()?, b"\n");
    let record = response
        .action_record
        .expect("completed keyboard action must have a record");
    assert_eq!(
        record.transport,
        cua_driver_core::action_record::ActionTransport::LinuxPty
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires an isolated X11 desktop, xterm, and permitted descendant PTY borrowing"]
async fn foreground_request_does_not_relabel_background_pty_delivery() -> Result<()> {
    let terminal = Terminal::new()?;
    let sentinel = cua_driver_testkit::keyboard_fixture::KeyboardFixture::spawn(false);
    let (conn, _) = x11rb::connect(None)?;
    let before = conn.get_input_focus()?.reply()?.focus;
    let input = cua_driver_testkit::keyboard_fixture::X11KeyboardObserver::start();
    input.track_focus(sentinel.window_id);
    let response = terminal
        .call(
            "press_key",
            json!({"key":"return","delivery_mode":"foreground"}),
        )
        .await;
    assert_ne!(response.is_error, Some(true), "{response:?}");
    assert_eq!(terminal.receive()?, b"\n");
    assert_eq!(conn.get_input_focus()?.reply()?.focus, before);
    assert!(
        input.events().is_empty(),
        "PTY delivery must not generate global key or focus events"
    );
    let output = response.structured_content.unwrap();
    assert_eq!(output["route"], "synthetic_events");
    assert_eq!(output["effect"], "unverifiable");
    assert_eq!(output["delivery"]["mode"], "background");
    Ok(())
}

#[tokio::test]
#[ignore = "requires an isolated X11 desktop and xterm"]
async fn exited_terminal_cannot_accept_a_new_key() -> Result<()> {
    let mut terminal = Terminal::new()?;
    terminal.child.kill()?;
    terminal.child.wait()?;
    let response = terminal
        .call(
            "press_key",
            json!({"key":"return","delivery_mode":"foreground"}),
        )
        .await;
    assert_eq!(response.is_error, Some(true), "{response:?}");
    terminal.assert_no_line()
}
