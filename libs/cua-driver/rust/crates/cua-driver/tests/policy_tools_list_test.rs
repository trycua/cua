//! Permission policy must shape the MCP tool roster as well as invocation.

#![cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]

use std::collections::HashSet;

use cua_driver_testkit::RawDriver;
use serde_json::json;

#[test]
fn tools_list_hides_policy_denied_tools_and_calls_stay_denied() {
    let directory = tempfile::tempdir().expect("temporary policy directory");
    let policy_path = directory.path().join("policy.yaml");
    // `get_config` is unconditionally allowed via `allow.tools`.
    // `screenshot` is conditionally allowed via `allow.rules` with a
    // constraint.  Both must appear in `tools/list` because `tools/list`
    // should not hide tools that are *potentially* allowed.
    // `list_apps` is explicitly denied and must be absent.
    std::fs::write(
        &policy_path,
        "allow:\n  tools: [get_config]\n  rules:\n    - tool: screenshot\n      constraints:\n        display_id: {\"const\": 0}\ndeny:\n  tools: [list_apps]\n",
    )
    .expect("write permission policy");
    let policy = policy_path.display().to_string();

    let Some(mut driver) = RawDriver::spawn_with_env(&[("CUA_DRIVER_POLICY_FILE", &policy)]) else {
        return;
    };

    driver.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    driver.recv();

    driver.send(&json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}));
    let response = driver.recv();
    let names: HashSet<&str> = response["result"]["tools"]
        .as_array()
        .expect("tools/list tools array")
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    assert!(
        names.contains("get_config"),
        "unconditionally allowed tool must remain listed"
    );
    assert!(
        names.contains("screenshot"),
        "rule-conditionally allowed tool must remain listed even though empty-arg evaluation would deny it"
    );
    assert!(
        !names.contains("list_apps"),
        "policy-denied tool must not be advertised"
    );

    driver.send(&json!({
        "jsonrpc":"2.0",
        "id":3,
        "method":"tools/call",
        "params":{"name":"list_apps","arguments":{}}
    }));
    let response = driver.recv();
    assert_eq!(response["result"]["isError"], true);
    assert!(
        response["result"]["content"][0]["text"]
            .as_str()
            .is_some_and(|message| message.contains("Permission denied")),
        "policy-denied invocation must remain rejected: {response}"
    );
}
