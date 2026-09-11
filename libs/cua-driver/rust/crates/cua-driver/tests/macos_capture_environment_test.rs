#![cfg(target_os = "macos")]

use cua_driver_testkit::RawDriver;
use serde_json::{json, Value};

fn call(driver: &mut RawDriver, name: &str, arguments: Value) -> Value {
    driver.send(&json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": name, "arguments": arguments}
    }));
    let response = driver.recv();
    assert!(response.get("error").is_none(), "{name}: {response}");
    assert_ne!(response["result"]["isError"], true, "{name}: {response}");
    response["result"].clone()
}

#[test]
#[ignore = "requires an unlocked macOS desktop and a TCC-authorized installed driver"]
fn desktop_capture_does_not_require_sbin_on_path() {
    for path in ["/usr/bin:/bin:/usr/sbin:/sbin", "/usr/bin:/bin"] {
        eprintln!("desktop capture with daemon PATH={path}");
        let mut driver = RawDriver::spawn_with_env(&[
            ("PATH", path),
            ("CUA_DRIVER_PERMISSION_MODE", "unrestricted"),
            ("CUA_DRIVER_DANGEROUSLY_BYPASS_APPROVALS", "1"),
        ])
        .expect("start the installed driver with the selected PATH");
        driver.send(&json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {}}));
        assert!(driver.recv().get("result").is_some());
        call(
            &mut driver,
            "start_session",
            json!({"session": "capture-path", "capture_scope": "desktop"}),
        );
        let output = tempfile::tempdir().unwrap();
        let png = output.path().join("desktop.png");
        let result = call(
            &mut driver,
            "get_desktop_state",
            json!({"session": "capture-path", "screenshot_out_file": png}),
        );
        let image = image::open(&png).expect("decode the captured desktop PNG");
        let metadata = &result["structuredContent"];
        assert_eq!(metadata["screenshot_mime_type"], "image/png");
        assert_eq!(metadata["screenshot_width"], image.width());
        assert_eq!(metadata["screenshot_height"], image.height());
        eprintln!("decoded desktop PNG: {}x{}", image.width(), image.height());
    }
}
