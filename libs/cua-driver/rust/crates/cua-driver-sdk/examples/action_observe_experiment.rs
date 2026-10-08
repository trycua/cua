//! Experimental SDK host for RFC #2794; no production tool registration.
//! A composite response preserves action and observation results independently.
use cua_driver_sdk::CuaDriver;
use serde_json::{json, Value};
#[path = "action_observe/operation.rs"]
mod operation;
use operation::{child, Cancellation, Options};
#[cfg(test)]
use operation::{child_envelope, composite_dispatch, observation_arguments};
use std::io::{self, BufRead, Write};

async fn run_loop(driver: &CuaDriver) -> Result<(), Box<dyn std::error::Error>> {
    for line in io::stdin().lock().lines() {
        let request: Value = serde_json::from_str(&line?)?;
        let Some(id) = request.get("id") else {
            continue;
        };
        let result = match request["method"].as_str().unwrap_or("") {
            "initialize" => {
                let metadata =
                    tokio::time::timeout(std::time::Duration::from_secs(5), driver.metadata())
                        .await??;
                json!({"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"action-observe-experiment","version":metadata.driver_version},"_meta":{"driver_metadata":metadata,"sdk_version":env!("CARGO_PKG_VERSION")}})
            }
            "tools/list" => {
                let mut list: Value = serde_json::from_str(&driver.list_tools_json().await?)?;
                for tool in list["tools"].as_array_mut().ok_or("invalid inventory")? {
                    if let Some(object) = tool.as_object_mut() {
                        if let Some(schema) = object.remove("input_schema") {
                            object.insert("inputSchema".to_owned(), schema);
                        }
                        if let Some(schema) = object.remove("output_schema") {
                            object.insert("outputSchema".to_owned(), schema);
                        }
                    }
                }
                list["tools"].as_array_mut().ok_or("invalid inventory")?.push(json!({"name":"experiment_action_observe","description":"Experimental one exact element action followed by same-window AX observation; inspect both results, never retry input from observation failure.","inputSchema":{"type":"object","properties":{"tool":{"type":"string","enum":["click","set_value","type_text"]},"arguments":{"type":"object"}},"required":["tool","arguments"],"additionalProperties":false}}));
                list
            }
            "tools/call" => {
                let name = request["params"]["name"].as_str().unwrap_or("");
                let args = request["params"]["arguments"].clone();
                if name == "experiment_action_observe" {
                    operation::action_observe(
                        driver,
                        args,
                        Options::default(),
                        Cancellation::default(),
                    )
                    .await
                } else {
                    child(&driver, name, args).await
                }
            }
            "ping" => json!({}),
            _ => json!({"error":"unsupported experimental method"}),
        };
        println!("{}", json!({"jsonrpc":"2.0","id":id,"result":result}));
        io::stdout().flush()?;
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let driver = if std::env::var("ACTION_OBSERVE_BACKEND").as_deref() == Ok("daemon") {
        CuaDriver::connect(None)?
    } else {
        CuaDriver::create(None)?
    };
    let outcome = run_loop(&driver).await;
    let shutdown = driver.shutdown().await;
    outcome?;
    shutdown?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_child_results_are_not_acknowledgments() {
        for raw in [
            "null",
            "true",
            "42",
            "{}",
            "not json",
            r#"{"content":[],"isError":"false"}"#,
            r#"{"content":[null]}"#,
            r#"{"content":[{"type":"text"}]}"#,
        ] {
            assert_eq!(child_envelope(raw)["isError"], true);
            assert_eq!(
                child_envelope(raw)["content"][0]["text"],
                "unreadable child result"
            );
        }
    }
    #[tokio::test]
    async fn observation_failure_preserves_single_action_dispatch() {
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let result = composite_dispatch(
            json!({"tool":"click","arguments":{"pid":7,"window_id":9,"element_token":"s1:0"}}),
            Options::default(),
            Cancellation::default(),
            |name, _| {
                let name = name.to_owned();
                let seen = seen.clone();
                async move {
                    seen.lock().unwrap().push(name.clone());
                    json!({"content":[],"isError":name == "get_window_state"})
                }
            },
        )
        .await;
        assert_eq!(*seen.lock().unwrap(), vec!["click", "get_window_state"]);
        assert_eq!(result["structuredContent"]["action_acknowledged"], true);
        assert_eq!(result["structuredContent"]["observation_available"], false);
        assert_eq!(
            result["structuredContent"]["action_result"]["isError"],
            false
        );
    }
    #[test]
    fn exact_observation_is_derived_from_action_owner() {
        let args = json!({"tool":"click","arguments":{"pid":7,"window_id":9,"element_token":"s1:0","session":"trial"}});
        let (_, _, read) = observation_arguments(&args).unwrap();
        assert_eq!(
            read,
            json!({"pid":7,"window_id":9,"include_screenshot":false,"session":"trial"})
        );
    }
    #[test]
    fn unbound_or_non_element_input_is_rejected() {
        for args in [
            json!({"tool":"press_key","arguments":{"pid":7,"window_id":9,"element_token":"s1:0"}}),
            json!({"tool":"click","arguments":{"pid":7,"element_token":"s1:0"}}),
            json!({"tool":"click","arguments":{"pid":7,"window_id":9}}),
        ] {
            assert!(observation_arguments(&args).is_err());
        }
    }
}
