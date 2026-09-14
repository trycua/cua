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

async fn observes_record(name: &str, foreground: bool, close: bool) {
    let fixture = KeyboardFixture::spawn(close);
    let mut args = serde_json::json!({"pid": fixture.pid(), "window_id": fixture.window_id,
        "delivery_mode": if foreground { "foreground" } else { "background" }});
    let key = if name == "press_key" {
        args["key"] = serde_json::json!("f5");
        0x74
    } else {
        args["keys"] = serde_json::json!(["ctrl", "h"]);
        0x48
    };
    let result = producer(name).invoke(args).await;
    if !close {
        assert_ne!(result.is_error, Some(true), "{result:?}");
    }
    fixture.key_event("down", key);
    if close {
        fixture.event("closed");
    } else {
        fixture.key_event("up", key);
    }
    let record = result.action_record.as_ref().expect(
        "keyboard producer must supply its execution record before dispatch reconstruction",
    );
    assert_eq!(
        record.effect,
        ActionEffect::Unverifiable,
        "native posting alone cannot confirm, and an observed key cannot be refused: {result:?}"
    );
    assert_eq!(
        record.transport,
        if foreground {
            ActionTransport::WindowsSendInput
        } else {
            ActionTransport::WindowsPostMessage
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
#[ignore = "requires an interactive Windows desktop"]
async fn native_producer_background_press_key_record() {
    observes_record("press_key", false, false).await;
}

#[tokio::test]
#[ignore = "requires an interactive Windows desktop"]
async fn native_producer_foreground_press_key_record() {
    observes_record("press_key", true, false).await;
}

#[tokio::test]
#[ignore = "requires an interactive Windows desktop"]
async fn native_producer_background_hotkey_record() {
    observes_record("hotkey", false, false).await;
}

#[tokio::test]
#[ignore = "requires an interactive Windows desktop"]
async fn native_producer_foreground_hotkey_record() {
    observes_record("hotkey", true, false).await;
}

#[tokio::test]
#[ignore = "requires an interactive Windows desktop"]
async fn native_producer_target_destruction_after_key_down_is_not_a_clean_refusal() {
    observes_record("press_key", false, true).await;
}

#[tokio::test]
#[ignore = "requires an interactive Windows desktop"]
async fn native_producer_invalid_keys_never_reach_the_target() {
    let fixture = KeyboardFixture::spawn(false);
    for (tool, fields) in [
        ("press_key", serde_json::json!({"key":"not-a-key"})),
        ("hotkey", serde_json::json!({"keys":[]})),
        ("hotkey", serde_json::json!({"keys":["ctrl","shift"]})),
    ] {
        let mut args = fields;
        args["pid"] = serde_json::json!(fixture.pid());
        args["window_id"] = serde_json::json!(fixture.window_id);
        let result = producer(tool).invoke(args).await;
        assert_eq!(result.is_error, Some(true), "{result:?}");
        fixture.assert_quiet();
    }
}
