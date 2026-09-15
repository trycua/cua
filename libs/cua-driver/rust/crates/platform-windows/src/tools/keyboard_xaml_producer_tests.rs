use super::*;
use cua_driver_core::action_record::{ActionEffect, ActionTransport, ActualDelivery};
use cua_driver_testkit::keyboard_fixture::KeyboardFixture;
use serde_json::json;
use std::time::{Duration, Instant};

async fn fixture() -> KeyboardFixture {
    let executable = cua_driver_testkit::harness_app("harness-winui3", "CuaTestHarness.WinUI3.exe");
    assert!(
        executable.is_file(),
        "environment blocked: WinUI3 fixture missing at {executable:?}"
    );
    let mut command = std::process::Command::new(executable);
    command.env("CUA_KEYBOARD_JOURNAL", "1");
    let fixture = KeyboardFixture::spawn_command(command);
    let state = ToolState::new();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let result = GetWindowStateTool {
            state: state.clone(),
        }
        .invoke(json!({
            "pid":fixture.pid(), "window_id":fixture.window_id, "include_screenshot":false
        }))
        .await;
        let text = serde_json::to_string(&result).unwrap();
        if result.is_error != Some(true)
            && text.contains("HARNESS_TEXT_MARKER_v1")
            && text.contains("counter=0")
            && text.contains("chk-agreed")
        {
            fixture.assert_quiet();
            return fixture;
        }
        assert!(
            Instant::now() < deadline,
            "WinUI3 keyboard fixture not ready: {result:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn probe(sentinel: &KeyboardFixture) {
    let result = PressKeyTool {
        state: ToolState::new(),
    }
    .invoke(json!({"key":"f6"}))
    .await;
    assert_ne!(result.is_error, Some(true), "{result:?}");
    sentinel.key_event("down", 0x75);
    sentinel.key_event("up", 0x75);
}

async fn semantic_hotkey(
    key: &str,
    event: &str,
    expected: serde_json::Value,
    transport: ActionTransport,
    delivery: &str,
) {
    let fixture = fixture().await;
    let sentinel = KeyboardFixture::spawn(false);
    probe(&sentinel).await;
    let result = HotkeyTool {
        state: ToolState::new(),
    }
    .invoke(json!({
        "pid":fixture.pid(), "window_id":fixture.window_id,
        "keys":["ctrl","shift",key], "delivery_mode":delivery
    }))
    .await;
    assert_ne!(result.is_error, Some(true), "{result:?}");
    assert_eq!(fixture.event(event)["value"], expected);
    fixture.assert_quiet();
    assert_eq!(
        fixture.recorded_key_down_count(),
        0,
        "UIA action must not be replayed as native keys"
    );
    sentinel.assert_quiet();
    probe(&sentinel).await;
    let record = result
        .action_record
        .as_ref()
        .unwrap_or_else(|| panic!("XAML keyboard producer record: {result:?}"));
    assert_eq!(record.effect, ActionEffect::Unverifiable);
    assert_eq!(record.transport, transport);
    assert_eq!(record.actual_delivery, Some(ActualDelivery::Background));
    assert!(record.delivered_count.is_none());
    assert!(record.public_result().unwrap().evidence.is_none());
}

#[tokio::test]
#[ignore = "requires an interactive Windows desktop and built WinUI3 fixture"]
async fn native_xaml_hotkey_reports_the_invoke_pattern_without_native_key_replay() {
    semantic_hotkey(
        "h",
        "counter",
        json!(1),
        ActionTransport::WindowsUiaInvoke,
        "background",
    )
    .await;
}

#[tokio::test]
#[ignore = "requires an interactive Windows desktop and built WinUI3 fixture"]
async fn native_xaml_hotkey_reports_actual_toggle_delivery_despite_foreground_request() {
    semantic_hotkey(
        "g",
        "toggle",
        json!(true),
        ActionTransport::WindowsUiaToggle,
        "foreground",
    )
    .await;
}

#[tokio::test]
#[ignore = "requires an interactive Windows desktop and built WinUI3 fixture"]
async fn native_xaml_missing_accelerator_is_refused_without_input() {
    let fixture = fixture().await;
    let sentinel = KeyboardFixture::spawn(false);
    probe(&sentinel).await;
    let result = HotkeyTool {
        state: ToolState::new(),
    }
    .invoke(json!({
        "pid":fixture.pid(), "window_id":fixture.window_id,
        "keys":["ctrl","shift","f12"], "delivery_mode":"background"
    }))
    .await;
    assert_eq!(result.is_error, Some(true), "{result:?}");
    fixture.assert_quiet();
    assert_eq!(fixture.recorded_key_down_count(), 0);
    sentinel.assert_quiet();
    probe(&sentinel).await;
    let record = result
        .action_record
        .as_ref()
        .unwrap_or_else(|| panic!("XAML keyboard refusal record: {result:?}"));
    assert_eq!(record.effect, ActionEffect::Refused);
    assert!(record.actual_delivery.is_none());
    assert!(record.attempts.is_empty());
    assert!(record.delivered_count.is_none());
    assert!(record.public_result().unwrap().evidence.is_none());
}

#[tokio::test]
#[ignore = "requires an interactive Windows desktop and built WinUI3 fixture"]
async fn native_xaml_keyboard_journal_observes_a_real_shortcut_control() {
    let fixture = fixture().await;
    assert_eq!(fixture.recorded_key_down_count(), 0);
    crate::input::keyboard::send_key_synthesized(fixture.window_id, "h", &["ctrl", "shift"])
        .unwrap();
    assert_eq!(fixture.event("counter")["value"], 1);
    assert!(
        fixture.recorded_key_down_count() > 0,
        "native key observer did not see the physical shortcut"
    );
}
