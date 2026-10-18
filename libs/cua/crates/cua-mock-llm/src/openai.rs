//! OpenAI Responses (https://platform.openai.com/docs/api-reference/responses,
//! streaming events: .../responses-streaming) and Chat Completions
//! (https://platform.openai.com/docs/api-reference/chat).

use crate::scenario::{self, Convo, Msg, Part, Reply, Role, Step, Tool};
use crate::{Api, Mock, content_text, failure, json_response};
use axum::http::StatusCode;
use axum::response::Response;
use serde_json::{Value, json};
use std::time::{SystemTime, UNIX_EPOCH};

/// `{"error":{"message","type","param","code"}}`.
pub fn error(message: &str, kind: &str, code: Option<&str>) -> Value {
    json!({"error": {"message": message, "type": kind, "param": null, "code": code}})
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Models offered: whatever a harness asks for works; these are listed.
pub const MODELS: &[&str] = &["gpt-mock-1", "gpt-5-codex", "gpt-5"];

/// `GET /v1/models`.
pub fn models() -> Response {
    let data: Vec<Value> = MODELS
        .iter()
        .map(|m| json!({"id": m, "object": "model", "created": 0, "owned_by": "cua-mock-llm"}))
        .collect();
    json_response(StatusCode::OK, json!({"object": "list", "data": data}))
}

fn push(messages: &mut Vec<Msg>, role: Role, part: Part) {
    match messages.last_mut() {
        Some(m) if m.role == role => m.parts.push(part),
        _ => messages.push(Msg {
            role,
            parts: vec![part],
        }),
    }
}

/// Parses a Responses request.
pub fn responses_convo(body: &Value) -> Convo {
    let mut messages = vec![];
    match &body["input"] {
        Value::String(s) => push(&mut messages, Role::User, Part::Text(s.clone())),
        Value::Array(items) => {
            for it in items {
                let kind = it["type"].as_str().unwrap_or("message");
                match kind {
                    "message" => {
                        let role = if it["role"] == "assistant" {
                            Role::Assistant
                        } else {
                            Role::User
                        };
                        // `developer`/`system` input is instructions, not the user.
                        if matches!(it["role"].as_str(), Some("developer" | "system")) {
                            continue;
                        }
                        push(
                            &mut messages,
                            role,
                            Part::Text(content_text(&it["content"])),
                        );
                    }
                    "function_call" | "custom_tool_call" | "local_shell_call" => push(
                        &mut messages,
                        Role::Assistant,
                        Part::ToolUse {
                            id: it["call_id"].as_str().unwrap_or("").into(),
                            name: it["name"].as_str().unwrap_or("").into(),
                        },
                    ),
                    "function_call_output"
                    | "custom_tool_call_output"
                    | "local_shell_call_output" => push(
                        &mut messages,
                        Role::User,
                        Part::ToolResult {
                            text: content_text(&it["output"]),
                        },
                    ),
                    _ => {}
                }
            }
        }
        _ => {}
    }
    // Functions, and functions grouped under a `namespace` tool (Codex's
    // MCP servers); a namespaced function is named `<ns>\u{1f}<name>` here
    // and called back with its `namespace` field.
    let mut tools = vec![];
    for t in body["tools"].as_array().into_iter().flatten() {
        let fun = |t: &Value, ns: Option<&str>| {
            let name = t["name"].as_str()?;
            Some(Tool {
                name: match ns {
                    Some(ns) => format!("{ns}{NS_SEP}{name}"),
                    None => name.into(),
                },
                schema: t.get("parameters").cloned().unwrap_or(json!({})),
            })
        };
        match t["type"].as_str() {
            Some("function") => tools.extend(fun(t, None)),
            Some("namespace") => {
                let ns = t["name"].as_str().unwrap_or("");
                for inner in t["tools"].as_array().into_iter().flatten() {
                    tools.extend(fun(inner, Some(ns)));
                }
            }
            _ => {}
        }
    }
    Convo { messages, tools }
}

/// Separates a namespace from a function name in [`Tool::name`].
pub const NS_SEP: char = '\u{1f}';

fn call_item(mock: &Mock, name: &str, input: &Value) -> Value {
    let mut item = json!({"id": mock.id("fc_mock_"), "type": "function_call",
        "status": "completed", "call_id": mock.id("call_mock_"), "arguments": input.to_string()});
    match name.split_once(NS_SEP) {
        Some((ns, n)) => {
            item["namespace"] = json!(ns);
            item["name"] = json!(n);
        }
        None => item["name"] = json!(name),
    }
    item
}

/// Parses a Chat Completions request.
pub fn chat_convo(body: &Value) -> Convo {
    let mut messages = vec![];
    for m in body["messages"].as_array().into_iter().flatten() {
        match m["role"].as_str() {
            Some("user") => push(
                &mut messages,
                Role::User,
                Part::Text(content_text(&m["content"])),
            ),
            Some("assistant") => {
                let text = content_text(&m["content"]);
                if !text.is_empty() {
                    push(&mut messages, Role::Assistant, Part::Text(text));
                }
                for c in m["tool_calls"].as_array().into_iter().flatten() {
                    push(
                        &mut messages,
                        Role::Assistant,
                        Part::ToolUse {
                            id: c["id"].as_str().unwrap_or("").into(),
                            name: c["function"]["name"].as_str().unwrap_or("").into(),
                        },
                    );
                }
            }
            Some("tool") => push(
                &mut messages,
                Role::User,
                Part::ToolResult {
                    text: content_text(&m["content"]),
                },
            ),
            _ => {}
        }
    }
    let tools = body["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| {
            let f = &t["function"];
            Some(Tool {
                name: f["name"].as_str()?.into(),
                schema: f.get("parameters").cloned().unwrap_or(json!({})),
            })
        })
        .collect();
    Convo { messages, tools }
}

fn visible(reply: &Reply) -> Vec<Step> {
    reply.steps.clone()
}

/// `POST /v1/responses` (Codex's wire API).
#[allow(unused_assignments)] // the last `seq += 1` in `ev!`
pub async fn responses(mock: &Mock, body: &Value) -> Response {
    let Some(model) = body["model"].as_str().map(str::to_string) else {
        return crate::invalid(Api::OpenAi, "Missing required parameter: 'model'.");
    };
    let reply = mock.plan(&responses_convo(body));
    if let Some(f) = reply.fail {
        return failure(Api::OpenAi, f);
    }
    let rid = mock.id("resp_mock_");
    let input_tokens = scenario::tokens(&body["input"].to_string());
    // Output items, completed.
    let items: Vec<Value> = visible(&reply)
        .iter()
        .map(|s| match s {
            Step::Text(t) => json!({"id": mock.id("msg_mock_"), "type": "message",
                "status": "completed", "role": "assistant",
                "content": [{"type": "output_text", "text": t, "annotations": []}]}),
            Step::Thinking(t) => json!({"id": mock.id("rs_mock_"), "type": "reasoning",
                "summary": [{"type": "summary_text", "text": t}]}),
            Step::Call { name, input } => call_item(mock, name, input),
        })
        .collect();
    let output_tokens: u64 = items.iter().map(|i| scenario::tokens(&i.to_string())).sum();
    let usage = json!({"input_tokens": input_tokens,
        "input_tokens_details": {"cached_tokens": 0},
        "output_tokens": output_tokens,
        "output_tokens_details": {"reasoning_tokens": 0},
        "total_tokens": input_tokens + output_tokens});
    let response = |status: &str, output: &[Value], usage: Value| {
        json!({"id": rid, "object": "response", "created_at": now(), "status": status,
               "model": model, "output": output, "usage": usage,
               "error": null, "incomplete_details": null})
    };
    if body["stream"] != true {
        return json_response(StatusCode::OK, response("completed", &items, usage));
    }
    let created = response("in_progress", &[], Value::Null);
    let completed = response("completed", &items, usage);
    let (tx, resp) = crate::sse();
    tokio::spawn(async move {
        let mut seq = 0u64;
        macro_rules! ev {
            ($name:expr, $v:expr) => {{
                let mut v: Value = $v;
                v["type"] = json!($name);
                v["sequence_number"] = json!(seq);
                seq += 1;
                if !tx.event($name, &v).await {
                    return;
                }
            }};
        }
        ev!("response.created", json!({"response": created.clone()}));
        ev!("response.in_progress", json!({"response": created}));
        for (oi, item) in items.iter().enumerate() {
            let id = item["id"].clone();
            match item["type"].as_str() {
                Some("message") => {
                    let text = item["content"][0]["text"]
                        .as_str()
                        .unwrap_or("")
                        .to_string();
                    let mut open = item.clone();
                    open["status"] = json!("in_progress");
                    open["content"] = json!([]);
                    ev!(
                        "response.output_item.added",
                        json!({"output_index": oi, "item": open})
                    );
                    ev!(
                        "response.content_part.added",
                        json!({"item_id": id, "output_index": oi,
                        "content_index": 0, "part": {"type": "output_text", "text": "", "annotations": []}})
                    );
                    for c in scenario::chunks(&text, reply.chunks) {
                        tokio::time::sleep(reply.pace).await;
                        ev!(
                            "response.output_text.delta",
                            json!({"item_id": id, "output_index": oi,
                            "content_index": 0, "delta": c, "logprobs": []})
                        );
                    }
                    ev!(
                        "response.output_text.done",
                        json!({"item_id": id, "output_index": oi,
                        "content_index": 0, "text": text, "logprobs": []})
                    );
                    ev!(
                        "response.content_part.done",
                        json!({"item_id": id, "output_index": oi,
                        "content_index": 0, "part": item["content"][0]})
                    );
                }
                Some("reasoning") => {
                    let text = item["summary"][0]["text"].clone();
                    let mut open = item.clone();
                    open["summary"] = json!([]);
                    ev!(
                        "response.output_item.added",
                        json!({"output_index": oi, "item": open})
                    );
                    ev!(
                        "response.reasoning_summary_part.added",
                        json!({"item_id": id,
                        "output_index": oi, "summary_index": 0, "part": {"type": "summary_text", "text": ""}})
                    );
                    ev!(
                        "response.reasoning_summary_text.delta",
                        json!({"item_id": id,
                        "output_index": oi, "summary_index": 0, "delta": text})
                    );
                    ev!(
                        "response.reasoning_summary_text.done",
                        json!({"item_id": id,
                        "output_index": oi, "summary_index": 0, "text": text})
                    );
                    ev!(
                        "response.reasoning_summary_part.done",
                        json!({"item_id": id,
                        "output_index": oi, "summary_index": 0, "part": item["summary"][0]})
                    );
                }
                _ => {
                    let args = item["arguments"].as_str().unwrap_or("").to_string();
                    let mut open = item.clone();
                    open["status"] = json!("in_progress");
                    open["arguments"] = json!("");
                    ev!(
                        "response.output_item.added",
                        json!({"output_index": oi, "item": open})
                    );
                    for c in scenario::chunks(&args, 2) {
                        ev!(
                            "response.function_call_arguments.delta",
                            json!({"item_id": id,
                            "output_index": oi, "delta": c})
                        );
                    }
                    ev!(
                        "response.function_call_arguments.done",
                        json!({"item_id": id,
                        "output_index": oi, "arguments": args})
                    );
                }
            }
            ev!(
                "response.output_item.done",
                json!({"output_index": oi, "item": item})
            );
        }
        ev!("response.completed", json!({"response": completed}));
    });
    resp
}

/// `POST /v1/chat/completions`.
pub async fn chat(mock: &Mock, body: &Value) -> Response {
    let Some(model) = body["model"].as_str().map(str::to_string) else {
        return crate::invalid(Api::OpenAi, "Missing required parameter: 'model'.");
    };
    let reply = mock.plan(&chat_convo(body));
    if let Some(f) = reply.fail {
        return failure(Api::OpenAi, f);
    }
    let id = mock.id("chatcmpl-mock-");
    let text: String = reply
        .steps
        .iter()
        .filter_map(|s| match s {
            Step::Text(t) => Some(t.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let reasoning: String = reply
        .steps
        .iter()
        .filter_map(|s| match s {
            Step::Thinking(t) => Some(t.as_str()),
            _ => None,
        })
        .collect();
    let calls: Vec<Value> = reply
        .steps
        .iter()
        .filter_map(|s| match s {
            Step::Call { name, input } => Some(json!({"id": mock.id("call_mock_"),
                "type": "function", "function": {"name": name, "arguments": input.to_string()}})),
            _ => None,
        })
        .collect();
    let finish = if calls.is_empty() {
        "stop"
    } else {
        "tool_calls"
    };
    let prompt_tokens = scenario::tokens(&body["messages"].to_string());
    let completion_tokens = scenario::tokens(&text) + scenario::tokens(&json!(calls).to_string());
    let usage = json!({"prompt_tokens": prompt_tokens, "completion_tokens": completion_tokens,
                       "total_tokens": prompt_tokens + completion_tokens});
    if body["stream"] != true {
        let mut message = json!({"role": "assistant",
            "content": if text.is_empty() { Value::Null } else { json!(text) }});
        if !calls.is_empty() {
            message["tool_calls"] = json!(calls);
        }
        return json_response(
            StatusCode::OK,
            json!({"id": id, "object": "chat.completion", "created": now(), "model": model,
                   "choices": [{"index": 0, "message": message, "finish_reason": finish}],
                   "usage": usage}),
        );
    }
    let include_usage = body["stream_options"]["include_usage"] == true;
    let (tx, resp) = crate::sse();
    tokio::spawn(async move {
        let chunk = |delta: Value, finish: Value| {
            json!({"id": id, "object": "chat.completion.chunk", "created": now(),
                   "model": model, "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]})
            .to_string()
        };
        if !tx
            .data(&chunk(
                json!({"role": "assistant", "content": ""}),
                Value::Null,
            ))
            .await
        {
            return;
        }
        if !reasoning.is_empty() {
            tx.data(&chunk(json!({"reasoning_content": reasoning}), Value::Null))
                .await;
        }
        for c in scenario::chunks(&text, reply.chunks) {
            tokio::time::sleep(reply.pace).await;
            if !tx.data(&chunk(json!({"content": c}), Value::Null)).await {
                return;
            }
        }
        for (i, c) in calls.iter().enumerate() {
            let mut head = c.clone();
            head["index"] = json!(i);
            head["function"]["arguments"] = json!("");
            tx.data(&chunk(json!({"tool_calls": [head]}), Value::Null))
                .await;
            let args = c["function"]["arguments"]
                .as_str()
                .unwrap_or("")
                .to_string();
            for part in scenario::chunks(&args, 2) {
                tx.data(&chunk(
                    json!({"tool_calls": [{"index": i, "function": {"arguments": part}}]}),
                    Value::Null,
                ))
                .await;
            }
        }
        tx.data(&chunk(json!({}), json!(finish))).await;
        if include_usage {
            tx.data(
                &json!({"id": id, "object": "chat.completion.chunk", "created": now(),
                        "model": model, "choices": [], "usage": usage})
                .to_string(),
            )
            .await;
        }
        tx.data("[DONE]").await;
    });
    resp
}
