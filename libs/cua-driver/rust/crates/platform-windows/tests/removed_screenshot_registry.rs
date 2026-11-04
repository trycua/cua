//! The non-Windows stub registry must not advertise the removed `screenshot`
//! tool. No desktop, input, clipboard, or user state is accessed.
#![cfg(not(target_os = "windows"))]

fn assert_screenshot_removed(compat: bool) {
    let registry = platform_windows::tools::build_registry(compat);
    assert!(
        registry.get_def("screenshot").is_none(),
        "removed screenshot remains resolvable (compat={compat})"
    );
    let list = registry.tools_list();
    let tools = list["tools"].as_array().expect("tools/list array");
    assert!(
        tools.iter().all(|tool| tool["name"] != "screenshot"),
        "removed screenshot remains advertised (compat={compat})"
    );
}

#[test]
fn standard_registry_does_not_expose_removed_screenshot() {
    assert_screenshot_removed(false);
}

#[test]
fn compat_registry_does_not_expose_removed_screenshot() {
    assert_screenshot_removed(true);
}
