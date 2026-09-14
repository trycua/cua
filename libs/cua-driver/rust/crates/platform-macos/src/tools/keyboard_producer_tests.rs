use super::*;
use cua_driver_core::action_record::{ActionEffect, ActionTransport, ActualDelivery};
use cua_driver_testkit::keyboard_fixture::KeyboardFixture;
use serde_json::json;

fn producer(name: &str) -> Box<dyn Tool> {
    let state = Arc::new(ToolState::default());
    match name {
        "press_key" => Box::new(press_key::PressKeyTool::new(state)),
        "hotkey" => Box::new(hotkey::HotkeyTool::new(state)),
        _ => unreachable!(),
    }
}

async fn observes_record(name: &str, foreground: bool, close: bool) {
    assert!(unsafe { crate::ax::bindings::AXIsProcessTrusted() }, "environment blocked: native producer test host lacks Accessibility permission; do not treat an unobserved post as delivery");
    let fixture = KeyboardFixture::spawn(close);
    let mut args = json!({"pid": fixture.pid(), "delivery_mode": if foreground { "foreground" } else { "background" }});
    let key = if name == "press_key" {
        args["key"] = json!("f5");
        96
    } else {
        args["keys"] = json!(["ctrl", "h"]);
        4
    };
    if foreground {
        args["window_id"] = json!(fixture.window_id);
    }
    let result = producer(name).invoke(args).await;
    assert_ne!(
        result.is_error,
        Some(true),
        "native producer failed before receipt: {result:?}"
    );
    fixture.key_event("down", key);
    if close {
        fixture.event("closed");
    } else {
        fixture.key_event("up", key);
    }
    let record = result.action_record.expect(
        "keyboard producer must supply its execution record before dispatch reconstruction",
    );
    assert_eq!(record.effect, ActionEffect::Unverifiable);
    assert_eq!(
        record.transport,
        if foreground && name == "press_key" {
            ActionTransport::MacosCgEventHid
        } else {
            ActionTransport::MacosCgEventPid
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
#[ignore = "requires a native macOS desktop and permission attributed to the test host"]
async fn native_producer_pid_press_key_record() {
    observes_record("press_key", false, false).await;
}

#[tokio::test]
#[ignore = "requires a native macOS desktop and permission attributed to the test host"]
async fn native_producer_pid_hotkey_record() {
    observes_record("hotkey", false, false).await;
}

#[tokio::test]
#[ignore = "requires a native macOS desktop and permission attributed to the test host"]
async fn native_producer_hid_press_key_record() {
    observes_record("press_key", true, false).await;
}

#[tokio::test]
#[ignore = "requires a native macOS desktop and permission attributed to the test host"]
async fn native_producer_menu_hotkey_record() {
    observes_record("hotkey", true, false).await;
}

#[tokio::test]
#[ignore = "requires a native macOS desktop and permission attributed to the test host"]
async fn native_producer_target_closes_after_key_post_without_false_confirmation() {
    observes_record("press_key", false, true).await;
}

#[tokio::test]
#[ignore = "requires a native macOS desktop"]
async fn native_producer_invalid_keys_never_reach_the_target() {
    let fixture = KeyboardFixture::spawn(false);
    for (tool, fields) in [
        ("press_key", json!({"key":"not-a-key"})),
        ("hotkey", json!({"keys":[]})),
        ("hotkey", json!({"keys":["ctrl","shift"]})),
    ] {
        let mut args = fields;
        args["pid"] = json!(fixture.pid());
        let result = producer(tool).invoke(args).await;
        assert_eq!(result.is_error, Some(true), "{result:?}");
        fixture.assert_quiet();
    }
}
