//! Gemini API (https://ai.google.dev/api/generate-content): `generateContent`,
//! `streamGenerateContent?alt=sse` (each SSE `data:` is a whole
//! `GenerateContentResponse`), `countTokens` and `models`.

use crate::scenario::{self, Convo, Msg, Part, Role, Step, Tool};
use crate::{Mock, json_response};
use axum::http::StatusCode;
use axum::response::Response;
use serde_json::{Value, json};

/// `{"error":{"code","message","status"}}`.
pub fn error(code: u16, message: &str, status: &str) -> Value {
    json!({"error": {"code": code, "message": message, "status": status}})
}

/// Gemini's OpenAPI schemas spell types in upper case (`OBJECT`, `STRING`).
fn lower_types(v: &Value) -> Value {
    match v {
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, v)| {
                    let v = match (k.as_str(), v) {
                        ("type", Value::String(s)) => Value::String(s.to_ascii_lowercase()),
                        _ => lower_types(v),
                    };
                    (k.clone(), v)
                })
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(lower_types).collect()),
        other => other.clone(),
    }
}

/// Parses a `generateContent` request.
pub fn convo(body: &Value) -> Convo {
    let mut messages = vec![];
    for c in body["contents"].as_array().into_iter().flatten() {
        let role = if c["role"] == "model" {
            Role::Assistant
        } else {
            Role::User
        };
        let mut parts = vec![];
        for p in c["parts"].as_array().into_iter().flatten() {
            if p["thought"] == true {
                continue;
            }
            if let Some(t) = p["text"].as_str() {
                parts.push(Part::Text(t.into()));
            } else if let Some(f) = p.get("functionCall") {
                parts.push(Part::ToolUse {
                    id: f["id"].as_str().unwrap_or("").into(),
                    name: f["name"].as_str().unwrap_or("").into(),
                });
            } else if let Some(f) = p.get("functionResponse") {
                parts.push(Part::ToolResult {
                    text: f["response"].to_string(),
                });
            }
        }
        messages.push(Msg { role, parts });
    }
    let tools = body["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|t| {
            t["functionDeclarations"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .filter_map(|f| {
            let schema = f
                .get("parametersJsonSchema")
                .or_else(|| f.get("parameters"))
                .map(lower_types)
                .unwrap_or(json!({}));
            Some(Tool {
                name: f["name"].as_str()?.into(),
                schema,
            })
        })
        .collect();
    Convo { messages, tools }
}

/// `POST /v1beta/models/{model}:generateContent` or `:streamGenerateContent`.
pub async fn generate(mock: &Mock, model: &str, body: &Value, stream: bool) -> Response {
    let reply = mock.plan(&convo(body));
    if let Some(f) = reply.fail {
        let (status, v) = match f {
            scenario::Failure::RateLimited => (
                StatusCode::TOO_MANY_REQUESTS,
                error(429, "mock: scripted rate limit", "RESOURCE_EXHAUSTED"),
            ),
            scenario::Failure::Server => (
                StatusCode::INTERNAL_SERVER_ERROR,
                error(500, "mock: scripted internal error", "INTERNAL"),
            ),
        };
        return json_response(status, v);
    }
    let prompt_tokens = scenario::tokens(&body["contents"].to_string());
    let parts: Vec<Value> = reply
        .steps
        .iter()
        .map(|s| match s {
            Step::Text(t) => json!({"text": t}),
            Step::Thinking(t) => json!({"text": t, "thought": true}),
            Step::Call { name, input } => {
                json!({"functionCall": {"name": name, "args": input, "id": mock.id("call_mock_")}})
            }
        })
        .collect();
    let out_tokens: u64 = parts.iter().map(|p| scenario::tokens(&p.to_string())).sum();
    let rid = mock.id("mock-");
    let model = model.to_string();
    let response = move |parts: Vec<Value>, finish: Option<&str>| {
        let mut cand = json!({"content": {"role": "model", "parts": parts}, "index": 0});
        if let Some(f) = finish {
            cand["finishReason"] = json!(f);
        }
        json!({"candidates": [cand],
               "usageMetadata": {"promptTokenCount": prompt_tokens,
                   "candidatesTokenCount": out_tokens,
                   "totalTokenCount": prompt_tokens + out_tokens},
               "modelVersion": model, "responseId": rid})
    };
    if !stream {
        return json_response(StatusCode::OK, response(parts, Some("STOP")));
    }
    let (tx, resp) = crate::sse();
    tokio::spawn(async move {
        let n = parts.len();
        for (i, p) in parts.into_iter().enumerate() {
            let last = i + 1 == n;
            // Text streams in chunks; calls arrive whole, as Gemini sends them.
            let pieces: Vec<Value> = match p["text"].as_str() {
                Some(t) if p["thought"] != true => scenario::chunks(t, reply.chunks)
                    .into_iter()
                    .map(|c| json!({"text": c}))
                    .collect(),
                _ => vec![p],
            };
            let k = pieces.len();
            for (j, piece) in pieces.into_iter().enumerate() {
                tokio::time::sleep(reply.pace).await;
                let fin = (last && j + 1 == k).then_some("STOP");
                if !tx.data(&response(vec![piece], fin).to_string()).await {
                    return;
                }
            }
        }
        if n == 0 {
            tx.data(&response(vec![json!({"text": ""})], Some("STOP")).to_string())
                .await;
        }
    });
    resp
}

/// `POST /v1beta/models/{model}:countTokens`.
pub fn count_tokens(body: &Value) -> Response {
    json_response(
        StatusCode::OK,
        json!({"totalTokens": scenario::tokens(&body.to_string())}),
    )
}

/// Models offered: whatever a harness asks for works; these are listed.
pub const MODELS: &[&str] = &["gemini-mock-1", "gemini-2.5-pro", "gemini-2.5-flash"];

/// `GET /v1beta/models`.
pub fn models() -> Response {
    let data: Vec<Value> = MODELS
        .iter()
        .map(|m| json!({"name": format!("models/{m}"), "displayName": format!("{m} (mock, scripted)"),
            "supportedGenerationMethods": ["generateContent", "streamGenerateContent", "countTokens"]}))
        .collect();
    json_response(StatusCode::OK, json!({"models": data}))
}
