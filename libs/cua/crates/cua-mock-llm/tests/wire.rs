//! The wire formats over real HTTP (hermetic: loopback only).

use serde_json::{Value, json};

async fn start() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = cua_mock_llm::router(cua_mock_llm::Mock::new(Default::default()));
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

/// SSE frames as (event, data) pairs, bounded.
fn frames(body: &str) -> Vec<(String, Value)> {
    body.split("\n\n")
        .take(10_000)
        .filter(|f| !f.trim().is_empty())
        .map(|f| {
            let mut ev = String::new();
            let mut data = String::new();
            for line in f.lines() {
                if let Some(e) = line.strip_prefix("event: ") {
                    ev = e.into();
                }
                if let Some(d) = line.strip_prefix("data: ") {
                    data = d.into();
                }
            }
            (
                ev,
                serde_json::from_str(&data).unwrap_or(Value::String(data)),
            )
        })
        .collect()
}

#[tokio::test]
async fn anthropic_auth_is_checked_in_the_real_error_shape() {
    let base = start().await;
    let c = reqwest::Client::new();
    let r = c
        .post(format!("{base}/v1/messages"))
        .header("x-api-key", "wrong")
        .header("anthropic-version", "2023-06-01")
        .json(&json!({"model": "m", "max_tokens": 5, "messages": []}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["type"], "error");
    assert_eq!(v["error"]["type"], "authentication_error");
    // Bearer (ANTHROPIC_AUTH_TOKEN) is accepted too.
    let r = c
        .post(format!("{base}/v1/messages"))
        .bearer_auth("mock-key")
        .json(&json!({"model": "m", "max_tokens": 5,
                      "messages": [{"role": "user", "content": "hi"}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["content"][0]["text"], "mock reply (scripted): hi");
}

#[tokio::test]
async fn anthropic_streams_a_tool_use_in_the_documented_event_order() {
    let base = start().await;
    let body = json!({"model": "claude-x", "max_tokens": 100, "stream": true,
        "tools": [{"name": "Bash", "input_schema": {"type": "object",
            "properties": {"command": {"type": "string"}}, "required": ["command"]}}],
        "messages": [{"role": "user", "content": [{"type": "text", "text": "go mock: say ok; shell ls"}]}]});
    let text = reqwest::Client::new()
        .post(format!("{base}/v1/messages"))
        .header("x-api-key", "mock-key")
        .json(&body)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let f = frames(&text);
    let names: Vec<&str> = f.iter().map(|(e, _)| e.as_str()).collect();
    assert_eq!(names.first(), Some(&"message_start"));
    assert_eq!(names.last(), Some(&"message_stop"));
    let tool = f
        .iter()
        .find(|(e, d)| e == "content_block_start" && d["content_block"]["type"] == "tool_use")
        .expect("tool_use block");
    assert_eq!(tool.1["content_block"]["name"], "Bash");
    let json: String = f
        .iter()
        .filter(|(_, d)| d["delta"]["type"] == "input_json_delta")
        .map(|(_, d)| d["delta"]["partial_json"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        serde_json::from_str::<Value>(&json).unwrap(),
        json!({"command": "ls"})
    );
    let delta = f.iter().find(|(e, _)| e == "message_delta").unwrap();
    assert_eq!(delta.1["delta"]["stop_reason"], "tool_use");
}

#[tokio::test]
async fn responses_stream_a_function_call_and_complete() {
    let base = start().await;
    let body = json!({"model": "gpt-5-codex", "stream": true,
        "tools": [{"type": "function", "name": "shell", "parameters": {"type": "object",
            "properties": {"command": {"type": "array", "items": {"type": "string"}}},
            "required": ["command"]}}],
        "input": [{"type": "message", "role": "user",
                   "content": [{"type": "input_text", "text": "mock: shell echo hi"}]}]});
    let c = reqwest::Client::new();
    let r = c
        .post(format!("{base}/v1/responses"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401, "no bearer, no service");
    let text = c
        .post(format!("{base}/v1/responses"))
        .bearer_auth("mock-key")
        .json(&body)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let f = frames(&text);
    assert_eq!(f[0].0, "response.created");
    let done = f
        .iter()
        .find(|(e, _)| e == "response.output_item.done")
        .unwrap();
    assert_eq!(done.1["item"]["type"], "function_call");
    let args: Value = serde_json::from_str(done.1["item"]["arguments"].as_str().unwrap()).unwrap();
    assert_eq!(args, json!({"command": ["bash", "-lc", "echo hi"]}));
    assert_eq!(f.last().unwrap().0, "response.completed");
    // The tool result comes back as function_call_output: the script ends.
    let call_id = done.1["item"]["call_id"].clone();
    let body2 = json!({"model": "gpt-5-codex", "stream": false, "input": [
        {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "mock: shell echo hi"}]},
        {"type": "function_call", "call_id": call_id, "name": "shell", "arguments": "{}"},
        {"type": "function_call_output", "call_id": call_id, "output": "hi\n"}]});
    let v: Value = c
        .post(format!("{base}/v1/responses"))
        .bearer_auth("mock-key")
        .json(&body2)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        v["output"][0]["content"][0]["text"],
        "Done. Last tool output: hi"
    );
}

#[tokio::test]
async fn chat_completions_stream_tool_calls_then_done_and_a_scripted_429() {
    let base = start().await;
    let c = reqwest::Client::new();
    let body = json!({"model": "m", "stream": true, "stream_options": {"include_usage": true},
        "tools": [{"type": "function", "function": {"name": "bash", "parameters": {"type": "object",
            "properties": {"command": {"type": "string"}}, "required": ["command"]}}}],
        "messages": [{"role": "user", "content": "mock: fail429; shell pwd"}]});
    let r = c
        .post(format!("{base}/v1/chat/completions"))
        .bearer_auth("mock-key")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 429);
    assert_eq!(r.headers()["retry-after"], "1");
    let text = c
        .post(format!("{base}/v1/chat/completions"))
        .bearer_auth("mock-key")
        .json(&body)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let f = frames(&text);
    assert_eq!(f.last().unwrap().1, Value::String("[DONE]".into()));
    assert!(
        f.iter()
            .any(|(_, d)| d["choices"][0]["finish_reason"] == "tool_calls")
    );
    assert!(
        f.iter()
            .any(|(_, d)| d["usage"]["total_tokens"].is_number())
    );
    let name = f
        .iter()
        .find_map(|(_, d)| d["choices"][0]["delta"]["tool_calls"][0]["function"]["name"].as_str())
        .unwrap();
    assert_eq!(name, "bash");
}

#[tokio::test]
async fn gemini_streams_whole_responses_with_a_function_call() {
    let base = start().await;
    let c = reqwest::Client::new();
    let body = json!({"contents": [{"role": "user", "parts": [{"text": "mock: shell uname"}]}],
        "tools": [{"functionDeclarations": [{"name": "run_shell_command",
            "parameters": {"type": "OBJECT", "properties": {"command": {"type": "STRING"}},
                           "required": ["command"]}}]}]});
    let url = format!("{base}/v1beta/models/gemini-2.5-pro:streamGenerateContent?alt=sse");
    let r = c.post(&url).json(&body).send().await.unwrap();
    assert_eq!(r.status(), 400);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["error"]["status"], "INVALID_ARGUMENT");
    let text = c
        .post(&url)
        .header("x-goog-api-key", "mock-key")
        .json(&body)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let f = frames(&text);
    let call = f
        .iter()
        .find_map(|(_, d)| {
            d["candidates"][0]["content"]["parts"][0]
                .get("functionCall")
                .cloned()
        })
        .expect("functionCall part");
    assert_eq!(call["name"], "run_shell_command");
    assert_eq!(call["args"], json!({"command": "uname"}));
    assert_eq!(f.last().unwrap().1["candidates"][0]["finishReason"], "STOP");
}

/// Codex groups an MCP server's tools under a `namespace` tool; the call
/// comes back with the function's own name and its `namespace`.
#[tokio::test]
async fn responses_call_namespaced_mcp_tools() {
    let base = start().await;
    let body = json!({"model": "m", "stream": false,
        "tools": [{"type": "namespace", "name": "mcp__cua_driver", "description": "x",
                   "tools": [{"type": "function", "name": "list_windows",
                              "parameters": {"type": "object", "properties": {}}}]}],
        "input": [{"type": "message", "role": "user",
                   "content": [{"type": "input_text", "text": "mock: tool list_windows"}]}]});
    let v: Value = reqwest::Client::new()
        .post(format!("{base}/v1/responses"))
        .bearer_auth("mock-key")
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let call = &v["output"][0];
    assert_eq!(call["type"], "function_call");
    assert_eq!(call["name"], "list_windows");
    assert_eq!(call["namespace"], "mcp__cua_driver");
}
