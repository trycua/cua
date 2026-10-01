use super::{focus_by_pixel_click_args, ToolState, ZoomTool};
use cua_driver_core::tool::Tool;

#[test]
fn schema_keeps_pid_optional_for_window_owned_lookup() {
    let tool = ZoomTool {
        state: ToolState::new(None),
    };
    let required = tool.def().input_schema["required"].as_array().unwrap();
    assert!(!required.iter().any(|field| field == "pid"));
    assert!(tool.def().input_schema["properties"].get("pid").is_some());
}

#[test]
fn press_key_native_element_focus_skips_second_screenshot_scaling() {
    let native = focus_by_pixel_click_args(
        42,
        Some(7),
        120.0,
        80.0,
        false,
        None,
        Some("client-a".to_owned()),
        false,
        true,
    );
    assert_eq!(native["_native_coordinates"], true);

    let screenshot = focus_by_pixel_click_args(
        42,
        Some(7),
        60.0,
        40.0,
        false,
        None,
        Some("client-a".to_owned()),
        false,
        false,
    );
    assert!(screenshot.get("_native_coordinates").is_none());
}
