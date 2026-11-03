//! Anthropic Messages API (https://docs.anthropic.com/en/api/messages,
//! streaming: https://docs.anthropic.com/en/docs/build-with-claude/streaming).

use crate::scenario::{self, Convo, Msg, Part, Role, Step, Tool};
use crate::{Api, Mock, content_text, failure, json_response};
use axum::http::StatusCode;
use axum::response::Response;
use serde_json::{Value, json};

/// `{"type":"error","error":{"type":..,"message":..}}`.
pub fn error(kind: &str, message: &str) -> Value {
    json!({"type": "error", "error": {"type": kind, "message": message}})
}

/// Parses a Messages request.
pub fn convo(body: &Value) -> Convo {
    let mut messages = vec![];
    for m in body["messages"].as_array().into_iter().flatten() {
        let role = if m["role"] == "assistant" {
            Role::Assistant
        } else {
            Role::User
        };
        let mut parts = vec![];
        match &m["content"] {
            Value::String(s) => parts.push(Part::Text(s.clone())),
            Value::Array(blocks) => {
                for b in blocks {
                    match b["type"].as_str() {
                        Some("text") => {
                            parts.push(Part::Text(b["text"].as_str().unwrap_or("").into()))
                        }
                        Some("tool_use") => parts.push(Part::ToolUse {
                            id: b["id"].as_str().unwrap_or("").into(),
                            name: b["name"].as_str().unwrap_or("").into(),
                        }),
                        Some("tool_result") => parts.push(Part::ToolResult {
                            text: content_text(&b["content"]),
                        }),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        messages.push(Msg { role, parts });
    }
    let tools = body["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| {
            Some(Tool {
                name: t["name"].as_str()?.into(),
                schema: t.get("input_schema").cloned().unwrap_or(json!({})),
            })
        })
        .collect();
    Convo { messages, tools }
}

fn usage_in(body: &Value) -> u64 {
    scenario::tokens(&body.to_string())
}

/// `POST /v1/messages`.
pub async fn messages(mock: &Mock, body: &Value) -> Response {
    let Some(model) = body["model"].as_str() else {
        return crate::invalid(Api::Anthropic, "model: Field required");
    };
    if !body["messages"].is_array() {
        return crate::invalid(Api::Anthropic, "messages: Field required");
    }
    let reply = mock.plan(&convo(body));
    if let Some(f) = reply.fail {
        return failure(Api::Anthropic, f);
    }
    let thinking_on =
        body["thinking"]["type"] == "enabled" || body["thinking"]["type"] == "adaptive";
    let steps: Vec<Step> = reply
        .steps
        .iter()
        .filter(|s| thinking_on || !matches!(s, Step::Thinking(_)))
        .cloned()
        .collect();
    let id = mock.id("msg_mock_");
    let stop = if reply.calls_tool() {
        "tool_use"
    } else {
        "end_turn"
    };
    let input_tokens = usage_in(body);
    let blocks: Vec<Value> = steps
        .iter()
        .map(|s| match s {
            Step::Text(t) => json!({"type": "text", "text": t}),
            Step::Thinking(t) => {
                json!({"type": "thinking", "thinking": t, "signature": "mock-signature"})
            }
            Step::Call { name, input } => json!({
                "type": "tool_use", "id": mock.id("toolu_mock_"), "name": name, "input": input
            }),
        })
        .collect();
    let output_tokens: u64 = blocks
        .iter()
        .map(|b| scenario::tokens(&b.to_string()))
        .sum();
    if body["stream"] != true {
        return json_response(
            StatusCode::OK,
            json!({
                "id": id, "type": "message", "role": "assistant", "model": model,
                "content": blocks, "stop_reason": stop, "stop_sequence": null,
                "usage": {"input_tokens": input_tokens, "output_tokens": output_tokens,
                          "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0},
            }),
        );
    }
    let (tx, resp) = crate::sse();
    let model = model.to_string();
    tokio::spawn(async move {
        let start = json!({"type": "message_start", "message": {
            "id": id, "type": "message", "role": "assistant", "model": model, "content": [],
            "stop_reason": null, "stop_sequence": null,
            "usage": {"input_tokens": input_tokens, "output_tokens": 1,
                      "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0}}});
        if !tx.event("message_start", &start).await {
            return;
        }
        tx.event("ping", &json!({"type": "ping"})).await;
        for (index, block) in blocks.iter().enumerate() {
            let (open, deltas): (Value, Vec<Value>) = match block["type"].as_str() {
                Some("text") => (
                    json!({"type": "text", "text": ""}),
                    scenario::chunks(block["text"].as_str().unwrap_or(""), reply.chunks)
                        .into_iter()
                        .map(|c| json!({"type": "text_delta", "text": c}))
                        .collect(),
                ),
                Some("thinking") => (
                    json!({"type": "thinking", "thinking": "", "signature": ""}),
                    scenario::chunks(block["thinking"].as_str().unwrap_or(""), 2)
                        .into_iter()
                        .map(|c| json!({"type": "thinking_delta", "thinking": c}))
                        .chain([json!({"type": "signature_delta", "signature": "mock-signature"})])
                        .collect(),
                ),
                _ => (
                    json!({"type": "tool_use", "id": block["id"], "name": block["name"], "input": {}}),
                    scenario::chunks(&block["input"].to_string(), 2)
                        .into_iter()
                        .map(|c| json!({"type": "input_json_delta", "partial_json": c}))
                        .collect(),
                ),
            };
            tx.event(
                "content_block_start",
                &json!({"type": "content_block_start", "index": index, "content_block": open}),
            )
            .await;
            for d in deltas {
                tokio::time::sleep(reply.pace).await;
                if !tx
                    .event(
                        "content_block_delta",
                        &json!({"type": "content_block_delta", "index": index, "delta": d}),
                    )
                    .await
                {
                    return;
                }
            }
            tx.event(
                "content_block_stop",
                &json!({"type": "content_block_stop", "index": index}),
            )
            .await;
        }
        tx.event(
            "message_delta",
            &json!({"type": "message_delta",
                    "delta": {"stop_reason": stop, "stop_sequence": null},
                    "usage": {"output_tokens": output_tokens}}),
        )
        .await;
        tx.event("message_stop", &json!({"type": "message_stop"}))
            .await;
    });
    resp
}

/// `POST /v1/messages/count_tokens`.
pub fn count_tokens(body: &Value) -> Response {
    json_response(StatusCode::OK, json!({"input_tokens": usage_in(body)}))
}

/// Models offered: whatever a harness asks for works; these are listed.
pub const MODELS: &[&str] = &["claude-mock-1", "claude-sonnet-4-5", "claude-haiku-4-5"];

/// `GET /v1/models`.
pub fn models() -> Response {
    let data: Vec<Value> = MODELS
        .iter()
        .map(|m| {
            json!({"type": "model", "id": m, "display_name": format!("{m} (mock, scripted)"),
                   "created_at": "2025-01-01T00:00:00Z"})
        })
        .collect();
    json_response(
        StatusCode::OK,
        json!({"data": data, "has_more": false,
               "first_id": MODELS.first(), "last_id": MODELS.last()}),
    )
}
