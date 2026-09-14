use super::*;
use cua_driver_core::action_record::{ActionEffect, ActionTransport, ActualDelivery};
use cua_driver_testkit::keyboard_fixture::KeyboardFixture;

fn producer(name: &str) -> Box<dyn Tool> {
    let state = ToolState::new();
    match name {
        "press_key" => Box::new(PressKeyTool { state }),
        "hotkey" => Box::new(HotkeyTool { state }),
        _ => unreachable!(),
    }
}

async fn observes_record(name: &str, foreground: bool) {
    let fixture = KeyboardFixture::spawn(false);
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

#[tokio::test]
#[ignore = "requires an isolated X11/Openbox desktop and Python"]
async fn native_producer_target_destruction_after_key_down_is_not_a_clean_refusal() {
    let fixture = KeyboardFixture::spawn(true);
    let result = producer("press_key")
        .invoke(
            serde_json::json!({"pid": fixture.pid(), "window_id": fixture.window_id, "key":"f5"}),
        )
        .await;
    fixture.key_event("down", 0xffc2);
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
    assert!(record.actual_delivery.is_some());
}
