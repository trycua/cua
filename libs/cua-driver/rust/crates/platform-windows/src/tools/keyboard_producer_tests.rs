use super::*;
use cua_driver_core::action_record::{ActionEffect, ActionTransport, ActualDelivery};
use cua_driver_testkit::keyboard_fixture::KeyboardFixture;

#[tokio::test]
#[ignore = "requires an interactive Windows desktop"]
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
    fixture.key_event("down", 0x75);
    fixture.key_event("up", 0x75);
}

async fn observes_record(name: &str, foreground: bool, close: bool) {
    let fixture = KeyboardFixture::spawn(close);
    let sentinel = (!close).then(|| KeyboardFixture::spawn(false));
    if let Some(sentinel) = &sentinel {
        focus_probe(sentinel).await;
    }
    let mut args = serde_json::json!({"pid": fixture.pid(), "window_id": fixture.window_id,
        "delivery_mode": if foreground { "foreground" } else { "background" }});
    let key = if name == "press_key" {
        args["key"] = serde_json::json!("f5");
        0x74
    } else {
        args["keys"] = serde_json::json!(["ctrl", "h"]);
        if close {
            0x11
        } else {
            0x48
        }
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
    if let Some(sentinel) = &sentinel {
        sentinel.assert_quiet();
        focus_probe(sentinel).await;
    }
    let record = result.action_record.as_ref().unwrap_or_else(|| panic!("keyboard producer must supply its execution record before dispatch reconstruction: {result:?}"));
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
async fn native_producer_hotkey_target_destruction_after_modifier_down_is_not_a_clean_refusal() {
    observes_record("hotkey", false, true).await;
}

#[tokio::test]
#[ignore = "requires an interactive Windows desktop"]
async fn native_producer_malformed_keys_never_reach_the_target() {
    let fixture = KeyboardFixture::spawn(false);
    for (tool, fields) in [
        ("press_key", serde_json::json!({"key":""})),
        ("press_key", serde_json::json!({"key":17})),
        ("hotkey", serde_json::json!({"keys":[]})),
    ] {
        let mut args = fields;
        args["pid"] = serde_json::json!(fixture.pid());
        args["window_id"] = serde_json::json!(fixture.window_id);
        let result = producer(tool).invoke(args).await;
        assert_eq!(result.is_error, Some(true), "{result:?}");
        fixture.assert_quiet();
    }
}

#[tokio::test]
#[ignore = "requires an interactive Windows desktop"]
async fn native_producer_retains_first_character_key_name_compatibility() {
    let fixture = KeyboardFixture::spawn(false);
    let result = producer("press_key")
        .invoke(serde_json::json!({
            "pid":fixture.pid(), "window_id":fixture.window_id, "key":"not-a-key"
        }))
        .await;
    assert_ne!(result.is_error, Some(true), "{result:?}");
    fixture.key_event("down", 0x4e);
    fixture.key_event("up", 0x4e);
}

#[tokio::test]
#[ignore = "requires an interactive Windows desktop"]
async fn native_producer_rejects_modifier_only_hotkeys_before_input() {
    let fixture = KeyboardFixture::spawn(false);
    let result = producer("hotkey")
        .invoke(serde_json::json!({
            "pid":fixture.pid(), "window_id":fixture.window_id, "keys":["ctrl","shift"]
        }))
        .await;
    assert_eq!(result.is_error, Some(true), "{result:?}");
    fixture.assert_quiet();
}

#[tokio::test]
#[ignore = "requires an interactive Windows desktop"]
async fn native_producer_duplicate_modifiers_form_one_chord_and_release() {
    let fixture = KeyboardFixture::spawn(false);
    let result = producer("hotkey")
        .invoke(serde_json::json!({
            "pid":fixture.pid(), "window_id":fixture.window_id,
            "keys":["ctrl","ctrl","h"], "delivery_mode":"foreground"
        }))
        .await;
    assert_ne!(result.is_error, Some(true), "{result:?}");
    fixture.key_event("down", 0x48);
    fixture.assert_single_release(0x48);
    let recovery = producer("press_key").invoke(serde_json::json!({
        "pid":fixture.pid(), "window_id":fixture.window_id, "key":"f5", "delivery_mode":"foreground"
    })).await;
    assert_ne!(recovery.is_error, Some(true), "{recovery:?}");
    assert_eq!(fixture.key_event("down", 0x74)["flags"], 0);
    fixture.key_event("up", 0x74);
}
