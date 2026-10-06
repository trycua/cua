//! Opt-in composition recipe; not a production SDK/MCP contract.
use cua_driver_sdk::CuaDriver;
use serde_json::{json, Value};
use std::{
    future::Future,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

/// Cooperative cancellation does not undo an input already dispatched.
#[derive(Clone, Default)]
pub struct Cancellation(Arc<AtomicBool>);
impl Cancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
    async fn wait(&self) {
        while !self.is_cancelled() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}
#[derive(Clone, Copy)]
pub struct Options {
    pub action_timeout: Duration,
    pub observation_timeout: Duration,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            action_timeout: Duration::from_secs(30),
            observation_timeout: Duration::from_secs(5),
        }
    }
}

/// Preserve child receipt independently from observation and task success.
pub async fn action_observe(
    driver: &CuaDriver,
    request: Value,
    options: Options,
    cancellation: Cancellation,
) -> Value {
    composite_dispatch(request, options, cancellation, |name, args| {
        let name = name.to_owned();
        async move { child(driver, &name, args).await }
    })
    .await
}
pub fn observation_arguments(args: &Value) -> Result<(&str, Value, Value), &'static str> {
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

pub async fn child(driver: &CuaDriver, name: &str, args: Value) -> Value {
    match driver.call_tool(name.to_owned(), args.to_string()).await {
        Ok(result) => child_envelope(&result.raw_json),
        Err(_) => {
            json!({"isError":true,"content":[{"type":"text","text":"child dispatch failed; inspect effects before retrying"}]})
        }
    }
}

fn valid_content(content: &Value) -> bool {
    content.as_array().is_some_and(|blocks| {
        blocks.iter().all(|block| match block["type"].as_str() {
            Some("text") => block["text"].is_string(),
            Some("image" | "audio") => block["data"].is_string() && block["mimeType"].is_string(),
            Some("resource") => block["resource"].is_object(),
            Some("resource_link") => block["uri"].is_string() && block["name"].is_string(),
            _ => false,
        })
    })
}

pub fn child_envelope(raw: &str) -> Value {
    match serde_json::from_str::<Value>(raw) {
        Ok(result)
            if result.get("content").is_some_and(valid_content)
                && result.get("isError").is_none_or(Value::is_boolean) =>
        {
            result
        }
        _ => json!({"isError":true,"content":[{"type":"text","text":"unreadable child result"}]}),
    }
}

fn failed(reason: &str) -> Value {
    json!({"isError":true,"content":[{"type":"text","text":reason}]})
}
fn valid_snapshot(result: &Value, owner: &Value) -> bool {
    if result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return false;
    }
    let s = &result["structuredContent"];
    s["pid"] == owner["pid"]
        && s["window_id"] == owner["window_id"]
        && s["snapshot_id"].as_str().is_some_and(|v| !v.is_empty())
        && s["elements"].is_array()
}
async fn bounded<F: Future<Output = Value>>(
    future: F,
    timeout: Duration,
    cancellation: &Cancellation,
) -> Result<Value, &'static str> {
    tokio::select! {
        biased;
        _ = cancellation.wait() => Err("cancelled"),
        result = tokio::time::timeout(timeout, future) => result.map_err(|_| "timeout"),
    }
}
pub async fn composite_dispatch<F, Fut>(
    args: Value,
    options: Options,
    cancellation: Cancellation,
    mut dispatch: F,
) -> Value
where
    F: FnMut(&str, Value) -> Fut,
    Fut: Future<Output = Value>,
{
    let (name, action_args, read_args) = match observation_arguments(&args) {
        Ok(parts) => parts,
        Err(reason) => return failed(reason),
    };
    if options.action_timeout.is_zero()
        || options.observation_timeout.is_zero()
        || options.action_timeout > Duration::from_secs(60)
        || options.observation_timeout > Duration::from_secs(60)
    {
        return failed("timeouts must be positive and at most 60 seconds");
    }
    if cancellation.is_cancelled() {
        return failed("cancelled before dispatch; no input sent");
    }
    // From this point, interruption cannot establish whether input had an effect.
    let action = bounded(
        dispatch(name, action_args),
        options.action_timeout,
        &cancellation,
    )
    .await;
    let (action_result, action_receipt_available, interrupted) = match action {
        Ok(result) => (child_envelope(&result.to_string()), true, None),
        Err(reason) => (
            failed("action receipt unavailable; effects unknown; observe before any retry"),
            false,
            Some(reason),
        ),
    };
    let (observation_result, observation_attempted, interruption) =
        if interrupted.is_some() || cancellation.is_cancelled() {
            (
                failed("observation skipped after interruption"),
                false,
                interrupted.or(Some("cancelled")),
            )
        } else {
            match bounded(
                dispatch("get_window_state", read_args.clone()),
                options.observation_timeout,
                &cancellation,
            )
            .await
            {
                Ok(result) => (child_envelope(&result.to_string()), true, None),
                Err(reason) => (
                    failed("observation receipt unavailable; do not repeat input"),
                    true,
                    Some(reason),
                ),
            }
        };
    let observation_available = valid_snapshot(&observation_result, &read_args);
    let payload = json!({"action_result": action_result, "observation_result": observation_result,
        "child_dispatches": if observation_attempted { vec![name,"get_window_state"] } else {vec![name]}, "action_dispatch_attempted":true, "action_receipt_available":action_receipt_available,
        "action_acknowledged":action_receipt_available && !action_result["isError"].as_bool().unwrap_or(false),
        "action_effect":"not_verified", "observation_attempted":observation_attempted,
        "observation_available":observation_available, "interruption":interruption});
    // Child refusals/pending results remain intact. Neither receipt establishes task completion.
    json!({"content":[{"type":"text","text":payload.to_string()}],"structuredContent":payload})
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> Value {
        json!({"tool":"click","arguments":{"pid":7,"window_id":9,"element_token":"fresh:0"}})
    }
    fn snapshot(pid: i64) -> Value {
        json!({"content":[],"structuredContent":{"pid":pid,"window_id":9,"snapshot_id":"new","elements":[]}})
    }
    #[tokio::test]
    async fn cancellation_before_dispatch_sends_nothing() {
        let cancel = Cancellation::default();
        cancel.cancel();
        let r = composite_dispatch(request(), Options::default(), cancel, |_, _| async {
            panic!("dispatch forbidden")
        })
        .await;
        assert_eq!(r["isError"], true);
    }
    #[tokio::test]
    async fn cancellation_during_action_is_unknown_and_never_replayed() {
        let cancel = Cancellation::default();
        let trigger = cancel.clone();
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = calls.clone();
        let r = composite_dispatch(request(), Options::default(), cancel, move |name, _| {
            let name = name.to_owned();
            let calls = seen.clone();
            let trigger = trigger.clone();
            async move {
                calls.lock().unwrap().push(name);
                trigger.cancel();
                std::future::pending::<Value>().await
            }
        })
        .await;
        assert_eq!(*calls.lock().unwrap(), vec!["click"]);
        let s = &r["structuredContent"];
        assert_eq!(s["action_dispatch_attempted"], true);
        assert_eq!(s["action_receipt_available"], false);
        assert_eq!(s["observation_attempted"], false);
        assert_eq!(s["interruption"], "cancelled");
    }
    #[tokio::test]
    async fn observation_timeout_keeps_original_pending_receipt() {
        let receipt = json!({"content":[],"structuredContent":{"status":"pending","marker":17}});
        let original = receipt.clone();
        let r = composite_dispatch(
            request(),
            Options {
                observation_timeout: Duration::from_millis(1),
                ..Options::default()
            },
            Cancellation::default(),
            move |name, _| {
                let receipt = receipt.clone();
                let read = name == "get_window_state";
                async move {
                    if read {
                        std::future::pending::<Value>().await
                    } else {
                        receipt
                    }
                }
            },
        )
        .await;
        let s = &r["structuredContent"];
        assert_eq!(s["action_result"], original);
        assert_eq!(s["action_effect"], "not_verified");
        assert_eq!(s["observation_available"], false);
        assert_eq!(s["interruption"], "timeout");
    }
    #[tokio::test]
    async fn owner_and_schema_required_for_observation_availability() {
        for read in [
            snapshot(8),
            json!({"content":[],"structuredContent":{"pid":7,"window_id":9,"elements":[]}}),
            json!({"content":[]}),
            json!(false),
        ] {
            let r = composite_dispatch(
                request(),
                Options::default(),
                Cancellation::default(),
                move |name, _| {
                    let r = if name == "get_window_state" {
                        read.clone()
                    } else {
                        json!({"content":[]})
                    };
                    async move { r }
                },
            )
            .await;
            assert_eq!(r["structuredContent"]["observation_available"], false);
        }
        assert!(valid_snapshot(
            &snapshot(7),
            &json!({"pid":7,"window_id":9})
        ));
    }
    #[tokio::test]
    async fn typed_action_refusal_survives_successful_read() {
        let refusal = json!({"isError":true,"content":[],"structuredContent":{"status":"element_outside_target_window"}});
        let original = refusal.clone();
        let r = composite_dispatch(
            request(),
            Options::default(),
            Cancellation::default(),
            move |name, _| {
                let r = if name == "get_window_state" {
                    snapshot(7)
                } else {
                    refusal.clone()
                };
                async move { r }
            },
        )
        .await;
        assert_eq!(r["structuredContent"]["action_result"], original);
        assert_eq!(r["structuredContent"]["action_acknowledged"], false);
        assert_eq!(r["structuredContent"]["observation_available"], true);
    }
    #[tokio::test]
    async fn cancellation_after_receipt_preserves_it_without_read() {
        let cancellation = Cancellation::default();
        let trigger = cancellation.clone();
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = calls.clone();
        let receipt = json!({"content":[],"structuredContent":{"status":"pending","marker":23}});
        let expected = receipt.clone();
        let r = composite_dispatch(
            request(),
            Options::default(),
            cancellation,
            move |name, _| {
                let name = name.to_owned();
                let seen = seen.clone();
                let trigger = trigger.clone();
                let receipt = receipt.clone();
                async move {
                    seen.lock().unwrap().push(name);
                    trigger.cancel();
                    receipt
                }
            },
        )
        .await;
        assert_eq!(*calls.lock().unwrap(), vec!["click"]);
        assert_eq!(r["structuredContent"]["action_result"], expected);
        assert_eq!(r["structuredContent"]["action_receipt_available"], true);
        assert_eq!(r["structuredContent"]["observation_attempted"], false);
    }
    #[tokio::test]
    async fn timeout_during_action_records_single_unknown_attempt() {
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = calls.clone();
        let r = composite_dispatch(
            request(),
            Options {
                action_timeout: Duration::from_millis(1),
                ..Options::default()
            },
            Cancellation::default(),
            move |name, _| {
                seen.lock().unwrap().push(name.to_owned());
                async { std::future::pending::<Value>().await }
            },
        )
        .await;
        assert_eq!(*calls.lock().unwrap(), vec!["click"]);
        assert_eq!(r["structuredContent"]["action_receipt_available"], false);
        assert_eq!(r["structuredContent"]["observation_attempted"], false);
        assert_eq!(r["structuredContent"]["interruption"], "timeout");
    }
}
