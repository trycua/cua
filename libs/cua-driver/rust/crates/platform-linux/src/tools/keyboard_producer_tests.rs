use super::*;
use cua_driver_core::action_record::{ActionEffect, ActionTransport, ActualDelivery};
use cua_driver_testkit::keyboard_fixture::KeyboardFixture;

#[tokio::test]
#[ignore = "requires an isolated X11/Openbox desktop and Python"]
async fn native_registry_refuses_ambiguous_pid_only_keyboard_targets() {
    let fixture = KeyboardFixture::spawn_with_companion();
    let registry = crate::tools::build_registry(false);
    for (tool, fields) in [
        ("press_key", serde_json::json!({"key":"f5"})),
        ("hotkey", serde_json::json!({"keys":["ctrl","h"]})),
    ] {
        let mut args = fields;
        args["pid"] = serde_json::json!(fixture.pid());
        let result = registry.invoke(tool, args).await;
        assert_eq!(result.is_error, Some(true), "{result:?}");
        let value = result.structured_content.as_ref().unwrap();
        assert_eq!(
            value
                .pointer("/refusal/code")
                .or_else(|| value.get("code"))
                .and_then(|v| v.as_str()),
            Some("ambiguous_window_target"),
            "{result:?}"
        );
        fixture.assert_quiet();
    }
}

async fn pty_record(foreground: bool) {
    use std::net::UdpSocket;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    use x11rb::protocol::xproto::ConnectionExt;
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let script = "import socket,sys\ns=socket.socket(socket.AF_INET,socket.SOCK_DGRAM)\na=('127.0.0.1',int(sys.argv[1]))\ns.sendto(b'ready',a)\nfor line in sys.stdin:s.sendto(line.encode(),a)";
    let child = cua_driver_testkit::spawn_in_job(
        Command::new("xterm")
            .args([
                "-xrm",
                "XTerm*allowSendEvents: false",
                "-e",
                "python3",
                "-u",
                "-c",
                script,
            ])
            .arg(socket.local_addr().unwrap().port().to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null()),
    )
    .unwrap();
    let pid = child.id();
    let mut reaper = cua_driver_testkit::ChildReaper::new();
    reaper.push(child);
    let mut buffer = [0; 32];
    let ready = socket.recv(&mut buffer).unwrap();
    assert_eq!(&buffer[..ready], b"ready");
    let deadline = Instant::now() + Duration::from_secs(5);
    let window = loop {
        if let Some(window) = crate::x11::list_windows(Some(pid)).first() {
            break window.xid;
        }
        assert!(
            Instant::now() < deadline,
            "terminal window did not become ready"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    cua_driver_testkit::keyboard_fixture::wait_for_x11_focus(window);
    let sentinel = KeyboardFixture::spawn(false);
    let input = cua_driver_testkit::keyboard_fixture::X11KeyboardObserver::start();
    input.track_focus(sentinel.window_id);
    let (conn, _) = x11rb::connect(None).unwrap();
    let before = conn.get_input_focus().unwrap().reply().unwrap().focus;
    let result = producer("press_key")
        .invoke(serde_json::json!({
            "pid":pid, "window_id":window, "key":"return",
            "delivery_mode":if foreground { "foreground" } else { "background" }
        }))
        .await;
    assert_ne!(result.is_error, Some(true), "{result:?}");
    let received = socket.recv(&mut buffer).unwrap();
    assert_eq!(&buffer[..received], b"\n");
    assert_eq!(
        conn.get_input_focus().unwrap().reply().unwrap().focus,
        before
    );
    assert!(
        input.events().is_empty(),
        "PTY delivery must not generate global key or focus events"
    );
    let record = result
        .action_record
        .expect("PTY producer must supply its own execution record");
    assert_eq!(record.transport, ActionTransport::LinuxPty);
    assert_eq!(record.actual_delivery, Some(ActualDelivery::Background));
    assert_eq!(record.effect, ActionEffect::Unverifiable);
}

#[tokio::test]
#[ignore = "requires an isolated X11/Openbox desktop, xterm, and eligible descendant PTY"]
async fn native_producer_pty_background_record() {
    pty_record(false).await;
}

#[tokio::test]
#[ignore = "requires an isolated X11/Openbox desktop, xterm, and eligible descendant PTY"]
async fn native_producer_pty_foreground_request_records_actual_background_delivery() {
    pty_record(true).await;
}

fn producer(name: &str) -> Box<dyn Tool> {
    let state = ToolState::new();
    match name {
        "press_key" => Box::new(PressKeyTool { state }),
        "hotkey" => Box::new(HotkeyTool { state }),
        _ => unreachable!(),
    }
}

async fn focus_probe(fixture: &KeyboardFixture) {
    let result = producer("press_key")
        .invoke(serde_json::json!({"scope":"desktop", "key":"f6"}))
        .await;
    assert_ne!(result.is_error, Some(true), "{result:?}");
    fixture.key_event("down", 0xffc3);
    fixture.key_event("up", 0xffc3);
}

async fn observes_record(name: &str, foreground: bool) {
    let fixture = KeyboardFixture::spawn(false);
    let sentinel = KeyboardFixture::spawn(false);
    focus_probe(&sentinel).await;
    let mut args = serde_json::json!({"pid": fixture.pid(), "window_id": fixture.window_id,
        "delivery_mode": if foreground { "foreground" } else { "background" }});
    let key = if name == "press_key" {
        args["key"] = serde_json::json!("f5");
        0xffc2
    } else {
        args["keys"] = serde_json::json!(["ctrl", "h"]);
        0x68
    };
    let result = producer(name).invoke(args).await;
    assert_ne!(result.is_error, Some(true), "{result:?}");
    let down = fixture.key_event("down", key);
    let up = fixture.key_event("up", key);
    assert_eq!(down["synthetic"], !foreground);
    assert_eq!(up["synthetic"], !foreground);
    sentinel.assert_quiet();
    focus_probe(&sentinel).await;
    let record = result
        .action_record
        .expect("keyboard producer must supply a record before dispatch reconstruction");
    assert_eq!(record.effect, ActionEffect::Unverifiable);
    assert_eq!(
        record.transport,
        if foreground {
            ActionTransport::LinuxXTest
        } else {
            ActionTransport::LinuxXSendEvent
        }
    );
    assert_eq!(
        record.actual_delivery,
        Some(if foreground {
            ActualDelivery::Foreground
        } else {
            ActualDelivery::Background
        })
    );
    assert!(record.public_result().unwrap().evidence.is_none());
}

#[tokio::test]
#[ignore = "requires an isolated X11/Openbox desktop and Python"]
async fn native_producer_background_press_key_record() {
    observes_record("press_key", false).await;
}

#[tokio::test]
#[ignore = "requires an isolated X11/Openbox desktop and Python"]
async fn native_producer_foreground_press_key_record() {
    observes_record("press_key", true).await;
}

#[tokio::test]
#[ignore = "requires an isolated X11/Openbox desktop and Python"]
async fn native_producer_background_hotkey_record() {
    observes_record("hotkey", false).await;
}

#[tokio::test]
#[ignore = "requires an isolated X11/Openbox desktop and Python"]
async fn native_producer_foreground_hotkey_record() {
    observes_record("hotkey", true).await;
}

#[tokio::test]
#[ignore = "requires an isolated X11/Openbox desktop and Python"]
async fn native_producer_missing_terminal_pty_falls_back_to_observed_xsend_event() {
    let fixture = KeyboardFixture::spawn(false);
    let result = producer("press_key").invoke(serde_json::json!({"pid": fixture.pid(), "window_id": fixture.window_id, "key":"return"})).await;
    assert_ne!(result.is_error, Some(true), "{result:?}");
    assert_eq!(fixture.key_event("down", 0xff0d)["synthetic"], true);
    fixture.key_event("up", 0xff0d);
    let record = result.action_record.expect("fallback producer record");
    assert_eq!(record.transport, ActionTransport::LinuxXSendEvent);
    assert_eq!(record.effect, ActionEffect::Unverifiable);
}

async fn target_closes_after_input(tool: &str, mut fields: serde_json::Value, key: u64) {
    let fixture = KeyboardFixture::spawn(true);
    fields["pid"] = serde_json::json!(fixture.pid());
    fields["window_id"] = serde_json::json!(fixture.window_id);
    fields["delivery_mode"] = serde_json::json!("background");
    let result = producer(tool).invoke(fields).await;
    assert_eq!(fixture.key_event("down", key)["synthetic"], true);
    fixture.event("closed");
    let record = result
        .action_record
        .as_ref()
        .expect("possibly delivered input needs an execution record even on error");
    assert_ne!(
        record.effect,
        ActionEffect::Refused,
        "a key reached the target: {result:?}"
    );
    assert_ne!(record.effect, ActionEffect::Confirmed);
    assert_eq!(record.actual_delivery, Some(ActualDelivery::Background));
    assert_eq!(record.transport, ActionTransport::LinuxXSendEvent);
    assert!(record.public_result().unwrap().evidence.is_none());
}

#[tokio::test]
#[ignore = "requires an isolated X11/Openbox desktop and Python"]
async fn native_producer_target_destruction_after_key_down_is_not_a_clean_refusal() {
    target_closes_after_input("press_key", serde_json::json!({"key":"f5"}), 0xffc2).await;
}

#[tokio::test]
#[ignore = "requires an isolated X11/Openbox desktop and Python"]
async fn native_producer_hotkey_target_destruction_after_modifier_down_is_not_a_clean_refusal() {
    target_closes_after_input("hotkey", serde_json::json!({"keys":["ctrl","h"]}), 0xffe3).await;
}

#[tokio::test]
#[ignore = "requires an isolated X11/Openbox desktop and Python"]
async fn native_producer_duplicate_modifiers_form_one_chord_and_release() {
    let fixture = KeyboardFixture::spawn(false);
    let result = producer("hotkey")
        .invoke(serde_json::json!({
            "pid":fixture.pid(), "window_id":fixture.window_id,
            "keys":["ctrl","ctrl","h"], "delivery_mode":"foreground"
        }))
        .await;
    assert_ne!(result.is_error, Some(true), "{result:?}");
    fixture.key_event("down", 0x68);
    fixture.assert_single_release(0x68);
    let recovery = producer("press_key").invoke(serde_json::json!({
        "pid":fixture.pid(), "window_id":fixture.window_id, "key":"f5", "delivery_mode":"foreground"
    })).await;
    assert_ne!(recovery.is_error, Some(true), "{recovery:?}");
    assert_eq!(fixture.key_event("down", 0xffc2)["flags"], 0);
    fixture.key_event("up", 0xffc2);
}
