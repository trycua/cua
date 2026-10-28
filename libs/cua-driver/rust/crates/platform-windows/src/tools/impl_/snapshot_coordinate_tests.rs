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

#[test]
fn resolver_refuses_cached_center_read_while_minimized() {
    use crate::uia::snapshot::{RetainedElement, SnapshotKind};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
        SM_YVIRTUALSCREEN,
    };
    let (x, y, w, h) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    };
    assert!(w > 0 && h > 0, "runner has no virtual-screen metrics");
    assert!(crate::input::point_on_virtual_desktop(x, y));

    let none = super::resolve_onscreen_point_with_scroll(&None, 0, 1, x, y, "clicking");
    assert_eq!(none.ok(), Some((x, y)));

    for kind in [SnapshotKind::Uia, SnapshotKind::Msaa] {
        let fresh = Some(RetainedElement::detached(kind, (x, y), false));
        let ok = super::resolve_onscreen_point_with_scroll(&fresh, 0, 1, x, y, "clicking");
        assert_eq!(ok.ok(), Some((x, y)));

        let stale = Some(RetainedElement::detached(kind, (x, y), true));
        let err = super::resolve_onscreen_point_with_scroll(&stale, 0, 1, x, y, "clicking")
            .expect_err("flagged element must be refused");
        let sc = err.structured_content.expect("structured refusal");
        assert_eq!(sc["status"], "refused");
        assert_eq!(sc["code"], "element_not_visible");
        assert_eq!(sc["refusal"]["code"], "element_not_visible");
    }
}
