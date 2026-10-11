//! Linux wire-argument compatibility, without any desktop input.
//! Missing element tokens force the real handler to refuse before AT-SPI or
//! keyboard delivery. This certifies argument admission, not value delivery.
#![cfg(target_os = "linux")]

use cua_driver_testkit::RawDriver;
use serde_json::{json, Value};

#[test]
fn numeric_set_value_reaches_the_same_handler_directly_and_in_a_batch() {
    let mut driver =
        RawDriver::spawn_explicit_direct().expect("spawn source-built direct MCP driver");
    driver.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    assert!(driver.recv()["result"].is_object());
    let expected = "set_value requires element_token to address the target element.";
    for value in [
        json!(0),
        json!(-12),
        json!(4.5),
        json!(18446744073709551615u64),
        json!(" 004.50 "),
    ] {
        let arguments = json!({"pid":1,"window_id":1,"value":value});
        driver.send(
            &json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
                "name":"set_value","arguments":arguments
            }}),
        );
        let direct = driver.recv();
        assert_eq!(direct["result"]["isError"], true, "{direct}");
        assert_eq!(direct["result"]["content"][0]["text"], expected, "{direct}");
        for batch_tool in ["run_steps", "run_actions"] {
            driver.send(
                &json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{
                    "name":batch_tool,"arguments":{"steps":[{"tool":"set_value","args":arguments}]}
                }}),
            );
            let batch = driver.recv();
            let result = &batch["result"]["structuredContent"];
            assert_ne!(result["code"], "invalid_batch", "{batch}");
            assert_eq!(result["steps"][0]["phase"], "action", "{batch}");
            assert_eq!(result["steps"][0]["message"], expected, "{batch}");
        }
    }
    for value in [Value::Null, json!(true), json!([]), json!({})] {
        driver.send(
            &json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{
                "name":"set_value","arguments":{"pid":1,"window_id":1,"value":value}
            }}),
        );
        let direct = driver.recv();
        assert_eq!(
            direct["result"]["structuredContent"]["code"], "invalid_arguments",
            "{direct}"
        );
        assert_eq!(
            direct["result"]["content"][0]["text"],
            "set_value: 'value' must be a string or a number.",
            "{direct}"
        );
    }
}
