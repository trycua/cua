//! Experimental SDK host for RFC #2794; no production tool registration.
//! A composite response preserves action and observation results independently.
use cua_driver_sdk::CuaDriver;
use serde_json::{json, Value};
use std::future::Future;
use std::io::{self, BufRead, Write};

fn observation_arguments(args: &Value) -> Result<(&str, Value, Value), &'static str> {
    let name = args
        .get("tool")
        .and_then(Value::as_str)
        .ok_or("missing tool")?;
    if !["click", "set_value", "type_text"].contains(&name) {
        return Err("only one element action is supported");
    }
    let action = args
        .get("arguments")
        .filter(|v| v.is_object())
        .ok_or("missing arguments")?;
    for key in ["pid", "window_id"] {
        if !action
            .get(key)
            .and_then(Value::as_i64)
            .is_some_and(|v| v > 0)
        {
            return Err("explicit positive pid and window_id required");
        }
    }
    if !action
        .get("element_token")
        .and_then(Value::as_str)
        .is_some_and(|v| !v.is_empty())
    {
        return Err("observed element_token required");
    }
    let mut read = json!({"pid": action["pid"], "window_id": action["window_id"], "include_screenshot": false});
    if let Some(session) = action.get("session") {
        read["session"] = session.clone();
    }
    Ok((name, action.clone(), read))
}

async fn child(driver: &CuaDriver, name: &str, args: Value) -> Value {
    match driver.call_tool(name.to_owned(), args.to_string()).await {
        Ok(result) => child_envelope(&result.raw_json),
        Err(_) => {
            json!({"isError":true,"content":[{"type":"text","text":"child dispatch failed; inspect effects before retrying"}]})
        }
    }
}

fn child_envelope(raw: &str) -> Value {
    match serde_json::from_str::<Value>(raw) {
        Ok(result)
            if result.get("content").is_some_and(Value::is_array)
                && result.get("isError").is_none_or(Value::is_boolean) =>
        {
            result
        }
        _ => json!({"isError":true,"content":[{"type":"text","text":"unreadable child result"}]}),
    }
}

async fn composite_dispatch<F, Fut>(args: Value, mut dispatch: F) -> Value
where
    F: FnMut(&str, Value) -> Fut,
    Fut: Future<Output = Value>,
{
    let (name, action_args, read_args) = match observation_arguments(&args) {
        Ok(parts) => parts,
        Err(reason) => return json!({"isError":true,"content":[{"type":"text","text":reason}]}),
    };
    // Ordinary SDK calls reach the canonical registry independently. The action
    // is dispatched exactly once, including when the observation later fails.
    let action = dispatch(name, action_args).await;
    let observation = dispatch("get_window_state", read_args).await;
    let payload = json!({"action_result":action,"observation_result":observation,
        "action_acknowledged": !action.get("isError").and_then(Value::as_bool).unwrap_or(false),
        "observation_available": !observation.get("isError").and_then(Value::as_bool).unwrap_or(false)});
    // A child error is data, not a request to repeat an already dispatched input.
    json!({"content":[{"type":"text","text":payload.to_string()}],"structuredContent":payload})
}

async fn run_loop(driver: &CuaDriver) -> Result<(), Box<dyn std::error::Error>> {
    for line in io::stdin().lock().lines() {
        let request: Value = serde_json::from_str(&line?)?;
        let Some(id) = request.get("id") else {
            continue;
        };
        let result = match request["method"].as_str().unwrap_or("") {
            "initialize" => {
                json!({"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"action-observe-experiment","version":env!("CARGO_PKG_VERSION")}})
            }
            "tools/list" => {
                let mut list: Value = serde_json::from_str(&driver.list_tools_json().await?)?;
                list["tools"].as_array_mut().ok_or("invalid inventory")?.push(json!({"name":"experiment_action_observe","description":"Experimental one exact element action followed by same-window AX observation; inspect both results, never retry input from observation failure.","inputSchema":{"type":"object","properties":{"tool":{"type":"string","enum":["click","set_value","type_text"]},"arguments":{"type":"object"}},"required":["tool","arguments"],"additionalProperties":false}}));
                list
            }
            "tools/call" => {
                let name = request["params"]["name"].as_str().unwrap_or("");
                let args = request["params"]["arguments"].clone();
                if name == "experiment_action_observe" {
                    composite_dispatch(args, |tool, arguments| {
                        let tool = tool.to_owned();
                        async move { child(driver, &tool, arguments).await }
                    })
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
    let driver = CuaDriver::create(None)?;
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
        ] {
            assert_eq!(child_envelope(raw)["isError"], true);
        }
    }
    #[tokio::test]
    async fn observation_failure_preserves_single_action_dispatch() {
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let result = composite_dispatch(
            json!({"tool":"click","arguments":{"pid":7,"window_id":9,"element_token":"s1:0"}}),
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
