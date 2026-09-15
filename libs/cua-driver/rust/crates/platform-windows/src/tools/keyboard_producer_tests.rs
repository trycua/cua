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
    producer_with_state(name, ToolState::new())
}

fn producer_with_state(name: &str, state: Arc<ToolState>) -> Box<dyn Tool> {
    match name {
        "press_key" => Box::new(PressKeyTool { state }),
        "hotkey" => Box::new(HotkeyTool { state }),
        _ => unreachable!(),
    }
}

fn retained_uia_identity(pointer: usize) -> (i32, String) {
    use std::mem::ManuallyDrop;
    use windows::core::Interface;
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
    use windows::Win32::UI::Accessibility::IUIAutomationElement;
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok().unwrap() };
    let element = ManuallyDrop::new(unsafe { IUIAutomationElement::from_raw(pointer as *mut _) });
    let pid = unsafe { element.CurrentProcessId() };
    let name = unsafe { element.CurrentName() };
    unsafe { CoUninitialize() };
    (pid.unwrap(), name.unwrap().to_string())
}

async fn admitted_target_survives_cache_clear(tool: &'static str) {
    use cua_driver_testkit::keyboard_native_barrier::NativeFocusBarrier;
    let fixture = KeyboardFixture::spawn(false);
    let state = ToolState::new();
    let snapshot = GetWindowStateTool {
        state: state.clone(),
    }
    .invoke(serde_json::json!({
        "pid":fixture.pid(), "window_id":fixture.window_id, "include_screenshot":false
    }))
    .await;
    assert_ne!(snapshot.is_error, Some(true), "{snapshot:?}");
    let elements = snapshot.structured_content.as_ref().unwrap()["elements"]
        .as_array()
        .unwrap();
    let targets = elements
        .iter()
        .filter(|element| element["label"] == "Keyboard input")
        .collect::<Vec<_>>();
    assert_eq!(targets.len(), 1, "{snapshot:?}");
    let token = targets[0]["element_token"].as_str().unwrap();
    let mut args = serde_json::json!({"pid":fixture.pid(), "window_id":fixture.window_id, "element_token":token, "delivery_mode":"foreground"});
    let key = if tool == "press_key" {
        args["key"] = serde_json::json!("f5");
        0x74
    } else {
        args["keys"] = serde_json::json!(["ctrl", "h"]);
        0x48
    };
    let barrier = NativeFocusBarrier::install("windows");
    let pending_tool = producer_with_state(tool, state.clone());
    let pending_args = args.clone();
    let pending = tokio::spawn(async move { pending_tool.invoke(pending_args).await });
    let pointer = barrier.wait();
    fixture.assert_quiet();
    assert!(state.element_cache.clear() > 0);
    assert_eq!(
        retained_uia_identity(pointer),
        (fixture.pid() as i32, "Keyboard input".to_owned())
    );
    let refused = producer_with_state(tool, state).invoke(args).await;
    assert_eq!(refused.is_error, Some(true), "{refused:?}");
    assert_eq!(
        refused.structured_content.as_ref().unwrap()["refusal"]["code"],
        "stale_element_token"
    );
    fixture.assert_quiet();
    barrier.release();
    let result = tokio::time::timeout(std::time::Duration::from_secs(15), pending)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(result.is_error, Some(true), "{result:?}");
    fixture.key_event("down", key);
    fixture.assert_single_release(key);
    let record = result
        .action_record
        .as_ref()
        .unwrap_or_else(|| panic!("admitted keyboard producer record: {result:?}"));
    assert_eq!(record.effect, ActionEffect::Unverifiable);
    assert_eq!(record.transport, ActionTransport::WindowsSendInput);
    assert_eq!(record.actual_delivery, Some(ActualDelivery::Foreground));
    assert!(record.delivered_count.is_none());
    assert!(record.public_result().unwrap().evidence.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
#[ignore = "requires an interactive Windows desktop and real UIA entry"]
async fn native_producer_press_key_retains_the_admitted_uia_target_after_cache_clear() {
    admitted_target_survives_cache_clear("press_key").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
#[ignore = "requires an interactive Windows desktop and real UIA entry"]
async fn native_producer_hotkey_retains_the_admitted_uia_target_after_cache_clear() {
    admitted_target_survives_cache_clear("hotkey").await;
}

async fn partial_native_acceptance(tool: &str) {
    use crate::input::keyboard::partial_input::PartialInputGuard;
    use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
    let fixture = KeyboardFixture::spawn(false);
    let sentinel = KeyboardFixture::spawn(false);
    focus_probe(&sentinel).await;
    let fault = PartialInputGuard::install();
    let mut args = serde_json::json!({"pid":fixture.pid(), "window_id":fixture.window_id, "delivery_mode":"foreground"});
    let key = if tool == "press_key" {
        args["key"] = serde_json::json!("f5");
        0x74
    } else {
        args["keys"] = serde_json::json!(["ctrl", "h"]);
        0x11
    };
    let result = producer(tool).invoke(args).await;
    eprintln!("partial native acceptance result: {result:?}");
    fixture.key_event("down", key);
    assert!(
        fault.release(),
        "could not release the independently observed partial input"
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while unsafe { GetAsyncKeyState(key as i32) } as u16 & 0x8000 != 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "native key remained pressed after cleanup"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    fixture.assert_no_key_down();
    assert_eq!(
        fixture.recorded_key_down_count(),
        1,
        "partial input was replayed"
    );
    sentinel.assert_no_key_down();
    focus_probe(&sentinel).await;
    assert_eq!(result.is_error, Some(true), "{result:?}");
    assert!(serde_json::to_string(&result)
        .unwrap()
        .contains("SendInput inserted only 1 of"));
    let record = result.action_record.as_ref().unwrap_or_else(|| {
        panic!("post-input native error requires an execution record: {result:?}")
    });
    assert_eq!(record.effect, ActionEffect::Unverifiable);
    assert_eq!(record.transport, ActionTransport::WindowsSendInput);
    assert_eq!(record.actual_delivery, Some(ActualDelivery::Foreground));
    assert!(record.delivered_count.is_none());
    assert!(record.public_result().unwrap().evidence.is_none());
}

#[tokio::test]
#[ignore = "requires an interactive Windows desktop; injects partial acceptance at the SendInput boundary"]
async fn native_producer_press_key_partial_acceptance_is_not_a_clean_refusal() {
    partial_native_acceptance("press_key").await;
}

#[tokio::test]
#[ignore = "requires an interactive Windows desktop; injects partial acceptance at the SendInput boundary"]
async fn native_producer_hotkey_partial_acceptance_is_not_a_clean_refusal() {
    partial_native_acceptance("hotkey").await;
}

pub(super) async fn focus_probe(fixture: &KeyboardFixture) {
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
