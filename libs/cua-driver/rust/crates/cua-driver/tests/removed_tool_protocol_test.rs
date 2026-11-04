//! Cross-platform protocol coverage for tools removed from the registry.

#![cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]

use cua_driver_testkit::RawDriver;

#[test]
fn standalone_screenshot_tool_is_removed() {
    let Some(mut driver) = RawDriver::spawn() else {
        return;
    };

    driver.send(&serde_json::json!({
        "jsonrpc":"2.0","id":1,"method":"initialize","params":{}
    }));
    driver.recv();

    driver.send(&serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"screenshot","arguments":{}}
    }));
    let response = driver.recv();
    assert_eq!(
        response["result"]["isError"],
        serde_json::json!(true),
        "{response:?}"
    );
    let text = response["result"]["content"]
        .as_array()
        .and_then(|items| items.iter().find_map(|item| item["text"].as_str()))
        .unwrap_or_default();
    assert_eq!(text, "Unknown tool: screenshot");
}
