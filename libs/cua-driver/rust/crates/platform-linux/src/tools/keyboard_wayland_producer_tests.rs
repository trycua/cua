use super::*;
use cua_driver_core::action_record::{ActionEffect, ActualDelivery};
use cua_driver_testkit::keyboard_fixture::KeyboardFixture;
use serde_json::json;
use std::time::{Duration, Instant};

async fn fixture() -> KeyboardFixture {
    assert!(crate::wayland::wayland_input_enabled());
    assert!(std::env::var_os("SWAYSOCK").is_some());
    let mut command = std::process::Command::new("python3");
    command.args([
        "-c",
        include_str!("../../tests/fixtures/keyboard_wayland.py"),
    ]);
    let mut fixture = KeyboardFixture::spawn_command(command);
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut previous = None;
    let mut stable = 0;
    loop {
        let windows = crate::wayland::list_windows_dispatch(Some(fixture.pid()));
        let identity = (windows.len() == 1).then(|| windows[0].xid);
        stable = if identity.is_some() && identity == previous {
            stable + 1
        } else {
            0
        };
        previous = identity;
        if stable == 2 {
            fixture.window_id = identity.unwrap();
            assert_ne!(fixture.window_id, 0);
            fixture.assert_quiet();
            return fixture;
        }
        assert!(
            Instant::now() < deadline,
            "native Wayland fixture identity did not stabilize: {windows:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
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

fn probe(sentinel: &KeyboardFixture) {
    crate::wayland::press_key_focused("f6").unwrap();
    assert_eq!(
        sentinel.key_event("down", 0xffc3)["flags"]
            .as_u64()
            .unwrap()
            & 4,
        0
    );
    sentinel.key_event("up", 0xffc3);
}

async fn observed_record(tool: &str, mut args: serde_json::Value, key: u64, control: bool) {
    let fixture = fixture().await;
    let sentinel = self::fixture().await;
    probe(&sentinel);
    args["pid"] = json!(fixture.pid());
    args["window_id"] = json!(fixture.window_id);
    args["delivery_mode"] = json!("foreground");
    let result = producer(tool).invoke(args).await;
    eprintln!("native Wayland producer result: {result:?}");
    let down = fixture.key_event("down", key);
    assert_eq!(down["flags"].as_u64().unwrap() & 4 != 0, control);
    assert_eq!(down["synthetic"], false);
    fixture.assert_single_release(key);
    sentinel.assert_quiet();
    probe(&sentinel);
    assert_ne!(result.is_error, Some(true), "{result:?}");
    let record = result.action_record.as_ref().unwrap_or_else(|| {
        panic!("native Wayland producer must supply its execution record: {result:?}")
    });
    assert_eq!(record.effect, ActionEffect::Unverifiable);
    assert_eq!(record.actual_delivery, Some(ActualDelivery::Foreground));
    assert!(record.delivered_count.is_none());
    let public = serde_json::to_value(record.public_result().unwrap()).unwrap();
    assert_eq!(public["route"], "global_input");
    assert!(public.get("evidence").is_none());
}

async fn background_refusal(tool: &str, mut args: serde_json::Value) {
    let fixture = fixture().await;
    let sentinel = self::fixture().await;
    probe(&sentinel);
    args["pid"] = json!(fixture.pid());
    args["window_id"] = json!(fixture.window_id);
    args["delivery_mode"] = json!("background");
    let result = producer(tool).invoke(args).await;
    assert_eq!(result.is_error, Some(true), "{result:?}");
    fixture.assert_quiet();
    sentinel.assert_quiet();
    probe(&sentinel);
    let record = result
        .action_record
        .as_ref()
        .unwrap_or_else(|| panic!("native Wayland refusal record: {result:?}"));
    assert_eq!(record.effect, ActionEffect::Refused);
    assert!(record.actual_delivery.is_none());
    assert!(record.delivered_count.is_none());
    assert!(record.public_result().unwrap().evidence.is_none());
}

#[tokio::test]
#[ignore = "requires an isolated Sway session and native Wayland GTK3"]
async fn native_wayland_press_key_has_a_producer_record_after_receipt() {
    observed_record("press_key", json!({"key":"f5"}), 0xffc2, false).await;
}

#[tokio::test]
#[ignore = "requires an isolated Sway session and native Wayland GTK3"]
async fn native_wayland_modified_press_key_preserves_the_chord() {
    observed_record(
        "press_key",
        json!({"key":"h", "modifiers":["ctrl"]}),
        0x68,
        true,
    )
    .await;
}

#[tokio::test]
#[ignore = "requires an isolated Sway session and native Wayland GTK3"]
async fn native_wayland_hotkey_has_a_producer_record_after_receipt() {
    observed_record("hotkey", json!({"keys":["ctrl","h"]}), 0x68, true).await;
}

#[tokio::test]
#[ignore = "requires an isolated Sway session and native Wayland GTK3"]
async fn native_wayland_background_press_key_is_refused_without_input() {
    background_refusal("press_key", json!({"key":"f5"})).await;
}

#[tokio::test]
#[ignore = "requires an isolated Sway session and native Wayland GTK3"]
async fn native_wayland_background_hotkey_is_refused_without_input() {
    background_refusal("hotkey", json!({"keys":["ctrl","h"]})).await;
}
