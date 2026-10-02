//! A scripted mock LLM provider.
//!
//! Real agent harnesses (Claude Code, Codex, OpenCode, Goose, ...) run
//! against it through their base-URL settings, so a test exercises the real
//! harness, its real tool execution in the sandbox and real credential
//! delivery, with deterministic model output. It is a **mock provider
//! (scripted)**: nothing here is a model, and every reply says so unless a
//! `mock:` directive scripts it (see [`scenario`]).
//!
//! Wire formats, each checked against the provider's API reference and
//! against requests captured from the real harnesses:
//!
//! - Anthropic Messages: `POST /v1/messages` (JSON or SSE with
//!   `message_start`, `content_block_start`/`delta`/`stop`, `message_delta`,
//!   `message_stop`; `text`, `thinking` and `tool_use` blocks),
//!   `POST /v1/messages/count_tokens`, `GET /v1/models`, and the
//!   `{"type":"error","error":{...}}` error shape;
//! - OpenAI Responses: `POST /v1/responses` (SSE `response.*` events with
//!   `message` and `function_call` output items);
//! - Gemini: `generateContent`, `streamGenerateContent?alt=sse`, `countTokens`,
//!   `models` (`x-goog-api-key` or `?key=`);
//! - OpenAI Chat Completions: `POST /v1/chat/completions` (JSON or SSE
//!   `chat.completion.chunk`s with `tool_calls`, then `[DONE]`),
//!   `GET /v1/models`.
//!
//! Auth is checked like the real APIs: `x-api-key` or `Authorization:
//! Bearer` for Anthropic, `Authorization: Bearer` for OpenAI, with the
//! configured mock key; anything else is a 401 in the provider's shape.

pub mod anthropic;
pub mod gemini;
pub mod openai;
pub mod scenario;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::mpsc;

/// Largest request body accepted (harness system prompts are large).
pub const MAX_BODY: usize = 32 * 1024 * 1024;

/// Server settings.
#[derive(Clone, Debug)]
pub struct Config {
    /// The key a client must present.
    pub api_key: String,
    /// Write every request (auth redacted) here as `NNNN-<api>.json`.
    pub capture_dir: Option<PathBuf>,
    /// Scripts by prompt: a user message with no `mock:` directive that
    /// contains a rule's text runs that rule's script, so a demo prompt can
    /// read like a person wrote it.
    pub rules: Vec<scenario::Rule>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            api_key: "mock-key".into(),
            capture_dir: None,
            rules: vec![],
        }
    }
}

/// Shared server state.
pub struct Mock {
    config: Config,
    failures: scenario::Failures,
    seq: AtomicU64,
}

impl Mock {
    /// A mock with `config`.
    pub fn new(config: Config) -> Arc<Self> {
        Arc::new(Mock {
            config,
            failures: Default::default(),
            seq: AtomicU64::new(0),
        })
    }

    pub(crate) fn plan(&self, convo: &scenario::Convo) -> scenario::Reply {
        scenario::plan_with(convo, &self.failures, &self.config.rules)
    }

    pub(crate) fn id(&self, prefix: &str) -> String {
        format!("{prefix}{:016x}", rand::random::<u64>())
    }
}

/// The router: every endpoint, with or without the `/v1` prefix.
pub fn router(mock: Arc<Mock>) -> Router {
    Router::new().fallback(dispatch).with_state(mock)
}

/// Binds `addr` and serves until the process ends.
pub async fn serve(addr: &str, config: Config) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, router(Mock::new(config))).await
}

/// Which API a request is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Api {
    Anthropic,
    OpenAi,
    Gemini,
}

async fn dispatch(State(mock): State<Arc<Mock>>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let path = parts.uri.path().trim_end_matches('/').to_string();
    let path = path.strip_prefix("/v1").unwrap_or(&path).to_string();
    let bytes = match axum::body::to_bytes(body, MAX_BODY).await {
        Ok(b) => b,
        Err(_) => return (StatusCode::PAYLOAD_TOO_LARGE, "body too large").into_response(),
    };
    let anthropic_headers =
        parts.headers.contains_key("anthropic-version") || parts.headers.contains_key("x-api-key");
    let gemini = path.starts_with("beta/") || parts.uri.path().starts_with("/v1beta/");
    let api = match path.as_str() {
        _ if gemini => Api::Gemini,
        "/messages" | "/messages/count_tokens" => Api::Anthropic,
        "/models" if anthropic_headers => Api::Anthropic,
        _ => Api::OpenAi,
    };
    capture(
        &mock,
        api,
        &parts.method,
        parts.uri.to_string(),
        &parts.headers,
        &bytes,
    )
    .await;
    // Claude Code's unauthenticated reachability probe (`HEAD /api/hello`).
    if path == "/api/hello" {
        return (StatusCode::OK, "{}").into_response();
    }
    if parts.method == Method::GET && path == "/health" {
        return (StatusCode::OK, "ok").into_response();
    }
    let query_key = parts
        .uri
        .query()
        .unwrap_or("")
        .split('&')
        .find_map(|kv| kv.strip_prefix("key="))
        .unwrap_or("")
        .to_string();
    if let Err(r) = check_auth(&mock.config.api_key, api, &parts.headers, &query_key) {
        return *r;
    }
    let body: Value = if bytes.is_empty() {
        Value::Null
    } else {
        match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            Err(e) => return invalid(api, &format!("request body is not JSON: {e}")),
        }
    };
    if api == Api::Gemini {
        // `/v1beta/models/{model}:{method}`
        let rest = parts.uri.path().trim_start_matches("/v1beta/");
        return match (
            parts.method.clone(),
            rest.strip_prefix("models/").and_then(|r| r.split_once(':')),
        ) {
            (Method::POST, Some((model, "generateContent"))) => {
                gemini::generate(&mock, model, &body, false).await
            }
            (Method::POST, Some((model, "streamGenerateContent"))) => {
                gemini::generate(&mock, model, &body, true).await
            }
            (Method::POST, Some((_, "countTokens"))) => gemini::count_tokens(&body),
            (Method::GET, None) if rest.trim_end_matches('/') == "models" => gemini::models(),
            _ => not_found(api, &format!("{} {}", parts.method, parts.uri.path())),
        };
    }
    match (parts.method.clone(), path.as_str()) {
        (Method::POST, "/messages") => anthropic::messages(&mock, &body).await,
        (Method::POST, "/messages/count_tokens") => anthropic::count_tokens(&body),
        (Method::GET, "/models") if api == Api::Anthropic => anthropic::models(),
        (Method::GET, "/models") => openai::models(),
        (Method::POST, "/responses") => openai::responses(&mock, &body).await,
        (Method::POST, "/chat/completions") => openai::chat(&mock, &body).await,
        _ => not_found(api, &format!("{} {}", parts.method, parts.uri.path())),
    }
}

fn check_auth(key: &str, api: Api, h: &HeaderMap, query_key: &str) -> Result<(), Box<Response>> {
    let get = |k: &str| h.get(k).and_then(|v| v.to_str().ok()).unwrap_or("");
    let bearer = get("authorization")
        .strip_prefix("Bearer ")
        .map(str::trim)
        .unwrap_or("");
    let ok = match api {
        Api::Anthropic => get("x-api-key") == key || bearer == key,
        Api::OpenAi => bearer == key,
        Api::Gemini => get("x-goog-api-key") == key || query_key == key,
    };
    if ok {
        return Ok(());
    }
    Err(Box::new(match api {
        Api::Gemini => json_response(
            StatusCode::BAD_REQUEST,
            gemini::error(
                400,
                "API key not valid. Please pass a valid API key.",
                "INVALID_ARGUMENT",
            ),
        ),
        Api::Anthropic => json_response(
            StatusCode::UNAUTHORIZED,
            anthropic::error("authentication_error", "invalid x-api-key"),
        ),
        Api::OpenAi => json_response(
            StatusCode::UNAUTHORIZED,
            openai::error(
                "Incorrect API key provided. You can find your API key at https://platform.openai.com/account/api-keys.",
                "invalid_request_error",
                Some("invalid_api_key"),
            ),
        ),
    }))
}

pub(crate) fn invalid(api: Api, msg: &str) -> Response {
    json_response(
        StatusCode::BAD_REQUEST,
        match api {
            Api::Anthropic => anthropic::error("invalid_request_error", msg),
            Api::OpenAi => openai::error(msg, "invalid_request_error", None),
            Api::Gemini => gemini::error(400, msg, "INVALID_ARGUMENT"),
        },
    )
}

fn not_found(api: Api, what: &str) -> Response {
    json_response(
        StatusCode::NOT_FOUND,
        match api {
            Api::Anthropic => anthropic::error("not_found_error", &format!("{what} not found")),
            Api::OpenAi => {
                openai::error(&format!("{what} not found"), "invalid_request_error", None)
            }
            Api::Gemini => gemini::error(404, &format!("{what} not found"), "NOT_FOUND"),
        },
    )
}

pub(crate) fn json_response(status: StatusCode, v: Value) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "application/json")],
        v.to_string(),
    )
        .into_response()
}

/// A 429 or 500 in `api`'s shape.
pub(crate) fn failure(api: Api, f: scenario::Failure) -> Response {
    let (status, body) = match (api, f) {
        (Api::Anthropic, scenario::Failure::RateLimited) => (
            StatusCode::TOO_MANY_REQUESTS,
            anthropic::error("rate_limit_error", "mock: scripted rate limit"),
        ),
        (Api::Anthropic, scenario::Failure::Server) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            anthropic::error("api_error", "mock: scripted internal error"),
        ),
        (Api::OpenAi, scenario::Failure::RateLimited) => (
            StatusCode::TOO_MANY_REQUESTS,
            openai::error(
                "mock: scripted rate limit",
                "requests",
                Some("rate_limit_exceeded"),
            ),
        ),
        (Api::Gemini, scenario::Failure::RateLimited) => (
            StatusCode::TOO_MANY_REQUESTS,
            gemini::error(429, "mock: scripted rate limit", "RESOURCE_EXHAUSTED"),
        ),
        (Api::Gemini, scenario::Failure::Server) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            gemini::error(500, "mock: scripted internal error", "INTERNAL"),
        ),
        (Api::OpenAi, scenario::Failure::Server) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            openai::error("mock: scripted internal error", "server_error", None),
        ),
    };
    let mut r = json_response(status, body);
    r.headers_mut()
        .insert("retry-after", header::HeaderValue::from_static("1"));
    r
}

/// An SSE response fed by the returned sender.
pub(crate) fn sse() -> (Sse, Response) {
    let (tx, rx) = mpsc::channel::<Result<Bytes, Infallible>>(64);
    let body = Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(rx));
    let r = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(body)
        .expect("static response");
    (Sse(tx), r)
}

/// Writes SSE frames; a gone client just ends the stream.
pub(crate) struct Sse(mpsc::Sender<Result<Bytes, Infallible>>);

impl Sse {
    /// `event: <name>` + `data: <json>`.
    pub async fn event(&self, name: &str, data: &Value) -> bool {
        self.raw(format!("event: {name}\ndata: {data}\n\n")).await
    }

    /// `data: <json>` only (Chat Completions).
    pub async fn data(&self, data: &str) -> bool {
        self.raw(format!("data: {data}\n\n")).await
    }

    async fn raw(&self, s: String) -> bool {
        self.0.send(Ok(Bytes::from(s))).await.is_ok()
    }
}

async fn capture(mock: &Mock, api: Api, method: &Method, uri: String, h: &HeaderMap, body: &Bytes) {
    let Some(dir) = mock.config.capture_dir.as_ref() else {
        return;
    };
    let n = mock.seq.fetch_add(1, Ordering::SeqCst);
    let headers: serde_json::Map<String, Value> = h
        .iter()
        .map(|(k, v)| {
            let name = k.as_str().to_string();
            let value = v.to_str().unwrap_or("<binary>");
            let shown = if matches!(
                name.as_str(),
                "authorization" | "x-api-key" | "x-goog-api-key" | "cookie"
            ) {
                format!("<redacted, {} bytes>", value.len())
            } else {
                value.to_string()
            };
            (name, Value::String(shown))
        })
        .collect();
    let body: Value = serde_json::from_slice(body).unwrap_or_else(|_| {
        Value::String(String::from_utf8_lossy(&body[..body.len().min(4096)]).into_owned())
    });
    let record = json!({
        "note": "request recorded by cua-mock-llm (mock provider, scripted); auth values redacted",
        "api": format!("{api:?}"),
        "method": method.as_str(),
        "uri": uri,
        "headers": headers,
        "body": body,
    });
    let _ = tokio::fs::create_dir_all(dir).await;
    let name = format!("{n:04}-{}.json", format!("{api:?}").to_lowercase());
    let _ = tokio::fs::write(
        dir.join(name),
        serde_json::to_vec_pretty(&record).unwrap_or_default(),
    )
    .await;
}

/// Text of an OpenAI or Anthropic content value: a string, or an array of
/// blocks with `text`.
pub(crate) fn content_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(a) => a
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str).or_else(|| b.as_str()))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}
