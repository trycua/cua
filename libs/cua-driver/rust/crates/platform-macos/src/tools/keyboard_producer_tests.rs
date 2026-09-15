use super::*;
use cua_driver_core::action_record::{ActionEffect, ActionTransport, ActualDelivery};
use cua_driver_testkit::keyboard_fixture::KeyboardFixture;
use serde_json::json;

#[tokio::test]
#[ignore = "requires a native macOS desktop"]
async fn native_registry_refuses_ambiguous_pid_only_keyboard_targets() {
    let fixture = KeyboardFixture::spawn_with_companion();
    let mut registry = ToolRegistry::new();
    super::register_all(&mut registry, false, false, true, None);
    for (tool, fields) in [
        ("press_key", json!({"key":"f5"})),
        ("hotkey", json!({"keys":["ctrl","h"]})),
    ] {
        let mut args = fields;
        args["pid"] = json!(fixture.pid());
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
    producer_with_state(name, Arc::new(ToolState::default()))
}

fn producer_with_state(name: &str, state: Arc<ToolState>) -> Box<dyn Tool> {
    match name {
        "press_key" => Box::new(press_key::PressKeyTool::new(state)),
        "hotkey" => Box::new(hotkey::HotkeyTool::new(state)),
        _ => unreachable!(),
    }
}

fn retained_ax_pid(pointer: usize) -> i32 {
    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXUIElementGetPid(element: crate::ax::bindings::AXUIElementRef, pid: *mut i32) -> i32;
    }
    let mut pid = 0;
    let error =
        unsafe { AXUIElementGetPid(pointer as crate::ax::bindings::AXUIElementRef, &mut pid) };
    assert_eq!(
        error, 0,
        "retained AX element became invalid after cache retirement"
    );
    pid
}

async fn admitted_target_survives_cache_clear(tool: &'static str) {
    use cua_driver_testkit::keyboard_native_barrier::NativeFocusBarrier;
    assert!(
        unsafe { crate::ax::bindings::AXIsProcessTrusted() },
        "environment blocked: native producer test host lacks Accessibility permission"
    );
    let fixture = KeyboardFixture::spawn(false);
    let state = Arc::new(ToolState::default());
    let snapshot = get_window_state::GetWindowStateTool::new(state.clone())
        .invoke(json!({
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
    let mut args = json!({"pid":fixture.pid(), "window_id":fixture.window_id, "element_token":token, "delivery_mode":"background"});
    let key = if tool == "press_key" {
        args["key"] = json!("f5");
        96
    } else {
        args["keys"] = json!(["ctrl", "h"]);
        4
    };
    let barrier = NativeFocusBarrier::install("macos");
    let pending_tool = producer_with_state(tool, state.clone());
    let pending_args = args.clone();
    let pending = tokio::spawn(async move { pending_tool.invoke(pending_args).await });
    let pointer = barrier.wait();
    fixture.assert_quiet();
    assert!(state.element_cache.clear() > 0);
    assert_eq!(retained_ax_pid(pointer), fixture.pid() as i32);
    assert_eq!(
        unsafe {
            crate::ax::bindings::copy_string_attr(
                pointer as crate::ax::bindings::AXUIElementRef,
                "AXValue",
            )
        }
        .as_deref(),
        Some("unchanged")
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
    assert_eq!(record.transport, ActionTransport::MacosCgEventPid);
    assert_eq!(record.actual_delivery, Some(ActualDelivery::Background));
    assert!(record.delivered_count.is_none());
    assert!(record.public_result().unwrap().evidence.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
#[ignore = "requires a native macOS desktop and permission attributed to the test host"]
async fn native_producer_press_key_retains_the_admitted_ax_target_after_cache_clear() {
    admitted_target_survives_cache_clear("press_key").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
#[ignore = "requires a native macOS desktop and permission attributed to the test host"]
async fn native_producer_hotkey_retains_the_admitted_ax_target_after_cache_clear() {
    admitted_target_survives_cache_clear("hotkey").await;
}

async fn focus_probe(fixture: &KeyboardFixture) {
    let result = producer("press_key")
        .invoke(json!({"scope":"desktop", "key":"f6"}))
        .await;
    assert_ne!(result.is_error, Some(true), "{result:?}");
    fixture.key_event("down", 97);
    fixture.key_event("up", 97);
}

async fn observes_record(name: &str, foreground: bool, close: bool) {
    assert!(unsafe { crate::ax::bindings::AXIsProcessTrusted() }, "environment blocked: native producer test host lacks Accessibility permission; do not treat an unobserved post as delivery");
    let fixture = KeyboardFixture::spawn(close);
    let sentinel = (!close).then(|| KeyboardFixture::spawn(false));
    if let Some(sentinel) = &sentinel {
        focus_probe(sentinel).await;
    }
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
    fixture.key_event("down", key);
    if close {
        fixture.event("closed");
    } else {
        fixture.key_event("up", key);
        assert_ne!(result.is_error, Some(true), "{result:?}");
    }
    if let Some(sentinel) = &sentinel {
        sentinel.assert_quiet();
        focus_probe(sentinel).await;
    }
    let record = result.action_record.as_ref().unwrap_or_else(|| panic!("keyboard producer must supply its execution record before dispatch reconstruction: {result:?}"));
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
#[ignore = "requires a native macOS desktop and permission attributed to the test host"]
async fn native_producer_hotkey_target_closes_after_key_post_without_false_confirmation() {
    observes_record("hotkey", false, true).await;
}

#[tokio::test]
#[ignore = "requires a native macOS desktop and permission attributed to the test host"]
async fn native_producer_confirms_only_after_the_same_ax_control_changes() {
    assert!(
        unsafe { crate::ax::bindings::AXIsProcessTrusted() },
        "environment blocked: native producer test host lacks Accessibility permission"
    );
    let fixture = KeyboardFixture::spawn(false);
    let result = producer("press_key")
        .invoke(json!({"pid":fixture.pid(), "key":"x"}))
        .await;
    assert_ne!(result.is_error, Some(true), "{result:?}");
    fixture.key_event("down", 7);
    assert_eq!(fixture.event("value")["value"], "x");
    let record = result.action_record.expect("native AX readback record");
    assert_eq!(record.effect, ActionEffect::Confirmed);
    assert_eq!(record.transport, ActionTransport::MacosCgEventPid);
    assert_eq!(record.actual_delivery, Some(ActualDelivery::Background));
    assert!(record.delivered_count.is_none());
    let public = serde_json::to_value(record.public_result().unwrap()).unwrap();
    assert_eq!(public["evidence"], json!([{"kind":"value_readback"}]));
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

#[tokio::test]
#[ignore = "requires a native macOS desktop and permission attributed to the test host"]
async fn native_producer_duplicate_modifiers_form_one_chord_and_release() {
    assert!(
        unsafe { crate::ax::bindings::AXIsProcessTrusted() },
        "environment blocked: native producer test host lacks Accessibility permission"
    );
    let fixture = KeyboardFixture::spawn(false);
    let result = producer("hotkey")
        .invoke(json!({"pid":fixture.pid(), "keys":["ctrl","ctrl","h"]}))
        .await;
    assert_ne!(result.is_error, Some(true), "{result:?}");
    fixture.key_event("down", 4);
    fixture.assert_single_release(4);
    let recovery = producer("press_key")
        .invoke(json!({"pid":fixture.pid(), "key":"f5"}))
        .await;
    assert_ne!(recovery.is_error, Some(true), "{recovery:?}");
    assert_eq!(
        fixture.key_event("down", 96)["flags"].as_u64().unwrap() & (1 << 18),
        0
    );
    fixture.key_event("up", 96);
}
