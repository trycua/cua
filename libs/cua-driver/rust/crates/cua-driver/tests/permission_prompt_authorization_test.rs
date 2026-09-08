// SPDX-License-Identifier: MIT

//! Cross-platform socket proof for protected operating-system permission UI.

use cua_driver_testkit::RawDriver;

#[test]
fn standard_mode_refuses_protected_permission_prompt_over_real_socket() {
    let mut driver = RawDriver::spawn_with_env(&[("CUA_DRIVER_PERMISSION_MODE", "standard")])
        .expect("the permission-authorization proof requires a built, spawnable driver");

    driver.send(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {}
    }));
    driver.recv();
    driver.send(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "check_permissions",
            "arguments": {"prompt": true}
        }
    }));
    let response = driver.recv();
    assert_eq!(response["id"], 2);
    assert!(
        response["result"]["isError"].as_bool().unwrap_or(false)
            && response["result"]["structuredContent"]["refusal"]["code"]
                == "os_permission_prompt_requires_trusted_host",
        "standard mode did not fail closed: {response}"
    );
}
