//! Async MCP stdio server loop.

use std::sync::{Arc, OnceLock};
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{debug, error, warn};

use crate::protocol::{initialize_result, InitializeMetadata, Request, Response, ResponseBody};
use crate::tool::ToolRegistry;

/// Receives privacy-bounded observations from the stdio MCP transport.
///
/// The observer never receives tool arguments, text/image content, response
/// bodies, arbitrary MCP metadata, or raw errors. HTTP dispatch reuses the
/// bounded tool timer directly; HTTP session-start telemetry remains deferred
/// until the transport has explicit session semantics.
pub trait StdioObserver: Send + Sync + 'static {
    fn on_session_started(&self, metadata: InitializeMetadata);
    fn on_tool_completed(&self, outcome: ToolCompletionObservation);
}

static STDIO_OBSERVER: OnceLock<Arc<dyn StdioObserver>> = OnceLock::new();

/// Register the process-wide stdio observer. Registration is intentionally
/// one-shot so a second subsystem cannot replace the privacy policy at runtime.
pub fn set_stdio_observer(observer: Arc<dyn StdioObserver>) -> bool {
    STDIO_OBSERVER.set(observer).is_ok()
}

/// Coarse duration buckets used by tool-completion observations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurationBucket {
    Under10Ms,
    Ms10To49,
    Ms50To249,
    Ms250To999,
    Ms1000To4999,
    Ms5000OrMore,
}

impl DurationBucket {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Under10Ms => "lt_10ms",
            Self::Ms10To49 => "10_49ms",
            Self::Ms50To249 => "50_249ms",
            Self::Ms250To999 => "250_999ms",
            Self::Ms1000To4999 => "1000_4999ms",
            Self::Ms5000OrMore => "gte_5000ms",
        }
    }
}

/// Content shape only. No content bytes or strings cross the observer seam.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputType {
    Empty,
    Text,
    Image,
    Mixed,
    Unknown,
}

impl OutputType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::Text => "text",
            Self::Image => "image",
            Self::Mixed => "mixed",
            Self::Unknown => "unknown",
        }
    }
}

/// Coarse serialized-result size buckets. The serialized value is measured in
/// place and immediately discarded; only this enum reaches the observer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputSizeBucket {
    Empty,
    Under1KiB,
    KiB1To9,
    KiB10To99,
    KiB100To1023,
    MiB1OrMore,
}

impl OutputSizeBucket {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Empty => "0",
            Self::Under1KiB => "lt_1kib",
            Self::KiB1To9 => "1_9kib",
            Self::KiB10To99 => "10_99kib",
            Self::KiB100To1023 => "100_1023kib",
            Self::MiB1OrMore => "gte_1mib",
        }
    }
}

/// Fixed tool failure classes. Arbitrary error messages are never inspected or
/// forwarded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolErrorClass {
    None,
    InvalidParams,
    UnknownTool,
    PermissionDenied,
    BackgroundUnavailable,
    TransportError,
    InternalError,
}

impl ToolErrorClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::InvalidParams => "invalid_params",
            Self::UnknownTool => "unknown_tool",
            Self::PermissionDenied => "permission_denied",
            Self::BackgroundUnavailable => "background_unavailable",
            Self::TransportError => "transport_error",
            Self::InternalError => "internal_error",
        }
    }
}

/// Privacy-bounded completion record for a known tool call on any transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCompletionObservation {
    /// Still self-reported input at this layer. The telemetry consumer must map
    /// this through the registry allowlist and use an `unknown` fallback.
    pub tool_name: String,
    pub operation: ToolOperation,
    pub success: bool,
    pub error_class: ToolErrorClass,
    pub duration_bucket: DurationBucket,
    pub output_type: OutputType,
    pub output_size_bucket: OutputSizeBucket,
}

/// Closed sub-operation vocabulary for compound tools. This enum is derived
/// at the dispatch boundary and cannot retain selectors, scripts, typed text,
/// or any other argument content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolOperation {
    NotApplicable,
    ExecuteJavascript,
    GetText,
    QueryDom,
    ClickElement,
    InsertText,
    TypeKeystrokes,
    EnableJavascriptAppleEvents,
    Other,
}

impl ToolOperation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotApplicable => "not_applicable",
            Self::ExecuteJavascript => "execute_javascript",
            Self::GetText => "get_text",
            Self::QueryDom => "query_dom",
            Self::ClickElement => "click_element",
            Self::InsertText => "insert_text",
            Self::TypeKeystrokes => "type_keystrokes",
            Self::EnableJavascriptAppleEvents => "enable_javascript_apple_events",
            Self::Other => "other",
        }
    }
}

pub fn tool_operation(tool_name: &str, args: Option<&serde_json::Value>) -> ToolOperation {
    if tool_name != "page" {
        return ToolOperation::NotApplicable;
    }
    match args
        .and_then(|value| value.get("action"))
        .and_then(serde_json::Value::as_str)
    {
        Some("execute_javascript") => ToolOperation::ExecuteJavascript,
        Some("get_text") => ToolOperation::GetText,
        Some("query_dom") => ToolOperation::QueryDom,
        Some("click_element") => ToolOperation::ClickElement,
        Some("insert_text") => ToolOperation::InsertText,
        Some("type_keystrokes") => ToolOperation::TypeKeystrokes,
        Some("enable_javascript_apple_events") => ToolOperation::EnableJavascriptAppleEvents,
        _ => ToolOperation::Other,
    }
}

/// Which stdio execution path produced a response. This only affects the
/// classification of JSON-RPC internal errors and is not emitted directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdioExecutionPath {
    InProcess,
    DaemonProxy,
}

/// Start-state for one tool call. Public so the daemon-proxy transport can use
/// exactly the same privacy and bucketing logic as the in-process transport.
pub struct ToolObservationTimer {
    tool_name: String,
    operation: ToolOperation,
    known_tool: bool,
    valid_params: bool,
    path: StdioExecutionPath,
    started: Instant,
}

impl ToolObservationTimer {
    pub fn start(
        tool_name: String,
        known_tool: bool,
        valid_params: bool,
        path: StdioExecutionPath,
    ) -> Self {
        Self::start_with_operation(
            tool_name,
            ToolOperation::NotApplicable,
            known_tool,
            valid_params,
            path,
        )
    }

    pub fn start_with_operation(
        tool_name: String,
        operation: ToolOperation,
        known_tool: bool,
        valid_params: bool,
        path: StdioExecutionPath,
    ) -> Self {
        Self {
            tool_name,
            operation,
            known_tool,
            valid_params,
            path,
            started: Instant::now(),
        }
    }

    pub fn finish(self, response: &Response) -> ToolCompletionObservation {
        classify_tool_completion(self, response)
    }
}

/// Notify the registered observer about a proxy-path initialize request.
/// Direct stdio calls this internally from [`run`].
pub fn observe_proxy_session_started(metadata: InitializeMetadata) {
    notify_session_started(metadata);
}

/// Notify the registered observer about a proxy-path tool result.
/// Direct stdio calls this internally from [`run`].
pub fn observe_proxy_tool_completed(outcome: ToolCompletionObservation) {
    notify_tool_completed(outcome);
}

/// Run the MCP server, reading JSON-RPC lines from stdin and writing
/// responses to stdout. Exits when stdin reaches EOF or a fatal I/O
/// error occurs.
pub async fn run(registry: Arc<ToolRegistry>) -> anyhow::Result<()> {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let mut reader = BufReader::new(stdin);
    let mut writer = tokio::io::BufWriter::new(stdout);
    let mut line = String::new();
    let mut session_observed = false;

    loop {
        line.clear();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            // EOF
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        debug!(raw = trimmed, "→ request");

        let response = match serde_json::from_str::<Request>(trimmed) {
            Err(e) => {
                error!("JSON parse error: {e}");
                Response::parse_error()
            }
            Ok(req) if req.is_notification() => {
                // Notifications are silently dropped.
                continue;
            }
            Ok(req) => {
                let initialize_metadata = (!session_observed)
                    .then(|| req.initialize_metadata())
                    .flatten();
                let session_context = session_tool_context(
                    &req,
                    &registry,
                    crate::session::SessionTransport::McpStdio,
                );
                let tool_timer = tool_observation_timer(
                    &req,
                    |name| name == "type_text_chars" || registry.get_def(name).is_some(),
                    StdioExecutionPath::InProcess,
                );
                let id = req.id.clone().unwrap_or(serde_json::Value::Null);
                let response = handle_request(req, id, &registry).await;
                if let Some(metadata) = initialize_metadata {
                    // A parsed initialize request always receives the static
                    // initialize result from handle_request. Mark it once only
                    // after that successful dispatch.
                    notify_session_started(metadata);
                    session_observed = true;
                }
                if let Some(timer) = tool_timer {
                    let outcome = timer.finish(&response);
                    if let Some(context) = session_context {
                        context.complete(&outcome);
                    }
                    notify_tool_completed(outcome);
                }
                response
            }
        };

        let serialized = serde_json::to_string(&response)
            .unwrap_or_else(|e| format!(r#"{{"jsonrpc":"2.0","id":null,"error":{{"code":-32603,"message":"serialize error: {e}"}}}}"#));
        debug!(raw = %serialized, "← response");

        writer.write_all(serialized.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;
    }

    Ok(())
}

/// Build a completion timer from a `tools/call` request while extracting only
/// the declared tool name and validity/allowlist booleans. Tool arguments are
/// neither cloned nor retained.
pub fn tool_observation_timer(
    req: &Request,
    is_known_tool: impl FnOnce(&str) -> bool,
    path: StdioExecutionPath,
) -> Option<ToolObservationTimer> {
    if req.method != "tools/call" {
        return None;
    }
    let parsed = req.tool_call();
    let (tool_name, operation, valid_params) = match parsed {
        Ok(call) => {
            let tool_name = bounded_tool_name(&call.name);
            let operation = tool_operation(&tool_name, Some(&call.args));
            (tool_name, operation, true)
        }
        Err(_) => {
            let name = req
                .params
                .as_ref()
                .and_then(|p| p.get("name"))
                .and_then(serde_json::Value::as_str)
                .map(bounded_tool_name)
                .unwrap_or_else(|| "unknown".to_owned());
            let operation = tool_operation(&name, None);
            (name, operation, false)
        }
    };
    let known_tool = valid_params && is_known_tool(&tool_name);
    Some(ToolObservationTimer::start_with_operation(
        tool_name,
        operation,
        known_tool,
        valid_params,
        path,
    ))
}

/// Build a private aggregate-session context from a valid, known tool call.
/// Only the public `session` argument is considered; reserved fallback keys
/// and all other arguments remain outside the observer seam.
pub fn session_tool_context(
    req: &Request,
    registry: &ToolRegistry,
    transport: crate::session::SessionTransport,
) -> Option<crate::session::SessionToolContext> {
    if req.method != "tools/call" {
        return None;
    }
    let call = req.tool_call().ok()?;
    let known_tool = call.name == "type_text_chars" || registry.get_def(&call.name).is_some();
    crate::session::begin_tool_call(&call.name, &call.args, known_tool, transport)
}

fn notify_session_started(metadata: InitializeMetadata) {
    if let Some(observer) = STDIO_OBSERVER.get() {
        observer.on_session_started(metadata);
    }
}

fn notify_tool_completed(outcome: ToolCompletionObservation) {
    if let Some(observer) = STDIO_OBSERVER.get() {
        observer.on_tool_completed(outcome);
    }
}

const MAX_TOOL_NAME_CHARS: usize = 64;

fn bounded_tool_name(name: &str) -> String {
    let value: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .take(MAX_TOOL_NAME_CHARS)
        .collect();
    if value.is_empty() {
        "unknown".to_owned()
    } else {
        value
    }
}

fn classify_tool_completion(
    timer: ToolObservationTimer,
    response: &Response,
) -> ToolCompletionObservation {
    let elapsed_ms = timer.started.elapsed().as_millis();
    let duration_bucket = match elapsed_ms {
        0..=9 => DurationBucket::Under10Ms,
        10..=49 => DurationBucket::Ms10To49,
        50..=249 => DurationBucket::Ms50To249,
        250..=999 => DurationBucket::Ms250To999,
        1000..=4999 => DurationBucket::Ms1000To4999,
        _ => DurationBucket::Ms5000OrMore,
    };

    let (result, rpc_error_code) = match &response.body {
        ResponseBody::Result { result } => (Some(result), None),
        ResponseBody::Error { error } => (None, Some(error.code)),
    };
    let tool_reported_error = result
        .and_then(|value| value.get("isError"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let success = timer.valid_params
        && timer.known_tool
        && rpc_error_code.is_none()
        && !tool_reported_error;

    let error_class = if success {
        ToolErrorClass::None
    } else if !timer.valid_params || rpc_error_code == Some(-32602) {
        ToolErrorClass::InvalidParams
    } else if !timer.known_tool {
        ToolErrorClass::UnknownTool
    } else if rpc_error_code == Some(-32603)
        && timer.path == StdioExecutionPath::DaemonProxy
    {
        ToolErrorClass::TransportError
    } else if rpc_error_code.is_some() {
        ToolErrorClass::InternalError
    } else {
        structured_error_class(result)
    };

    let output_type = classify_output_type(result);
    let output_size_bucket = result
        .and_then(serialized_size_without_retaining)
        .map(size_bucket)
        .unwrap_or(OutputSizeBucket::Empty);

    ToolCompletionObservation {
        tool_name: timer.tool_name,
        operation: timer.operation,
        success,
        error_class,
        duration_bucket,
        output_type,
        output_size_bucket,
    }
}

fn serialized_size_without_retaining(value: &serde_json::Value) -> Option<usize> {
    struct ByteCounter(usize);
    impl std::io::Write for ByteCounter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.saturating_add(bytes.len());
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let mut counter = ByteCounter(0);
    serde_json::to_writer(&mut counter, value).ok()?;
    Some(counter.0)
}

fn structured_error_class(result: Option<&serde_json::Value>) -> ToolErrorClass {
    let code = result
        .and_then(|value| value.get("structuredContent"))
        .and_then(|value| value.get("code"))
        .and_then(serde_json::Value::as_str);
    match code {
        Some("background_unavailable" | "background_occluded") => {
            ToolErrorClass::BackgroundUnavailable
        }
        Some(
            "permission_denied"
            | "accessibility_permission_denied"
            | "screen_recording_permission_denied"
            | "tcc_permission_denied",
        ) => ToolErrorClass::PermissionDenied,
        _ => ToolErrorClass::InternalError,
    }
}

fn classify_output_type(result: Option<&serde_json::Value>) -> OutputType {
    let Some(content) = result
        .and_then(|value| value.get("content"))
        .and_then(serde_json::Value::as_array)
    else {
        return OutputType::Empty;
    };
    if content.is_empty() {
        return OutputType::Empty;
    }

    let mut has_text = false;
    let mut has_image = false;
    let mut has_unknown = false;
    for item in content {
        match item.get("type").and_then(serde_json::Value::as_str) {
            Some("text") => has_text = true,
            Some("image") => has_image = true,
            _ => has_unknown = true,
        }
    }
    match (has_text, has_image, has_unknown) {
        (true, false, false) => OutputType::Text,
        (false, true, false) => OutputType::Image,
        (true, true, false) => OutputType::Mixed,
        _ => OutputType::Unknown,
    }
}

fn size_bucket(size: usize) -> OutputSizeBucket {
    match size {
        0 => OutputSizeBucket::Empty,
        1..=1023 => OutputSizeBucket::Under1KiB,
        1024..=10_239 => OutputSizeBucket::KiB1To9,
        10_240..=102_399 => OutputSizeBucket::KiB10To99,
        102_400..=1_048_575 => OutputSizeBucket::KiB100To1023,
        _ => OutputSizeBucket::MiB1OrMore,
    }
}

/// Dispatch one MCP JSON-RPC request against the registry (initialize /
/// tools/list / tools/call). Shared by the stdio loop above and the
/// daemon's HTTP transport (`cua-driver`'s `mcp_http`) so both speak the
/// exact same MCP semantics.
pub async fn handle_request(req: Request, id: serde_json::Value, registry: &Arc<ToolRegistry>) -> Response {
    match req.method.as_str() {
        "initialize" => Response::ok(id, initialize_result()),

        "tools/list" => Response::ok(id, registry.tools_list()),

        "tools/call" => match req.tool_call() {
            Err(e) => Response::error(id, -32602, format!("Invalid params: {e}")),
            Ok(call) => {
                let result = registry.invoke(&call.name, call.args).await;
                match serde_json::to_value(result) {
                    Ok(v) => Response::ok(id, v),
                    Err(e) => Response::error(id, -32603, format!("Serialize error: {e}")),
                }
            }
        },

        other => {
            warn!(method = other, "unknown method");
            Response::method_not_found(id, other)
        }
    }
}

#[cfg(test)]
mod observation_tests {
    use super::*;

    fn timer(known: bool, valid: bool, path: StdioExecutionPath) -> ToolObservationTimer {
        ToolObservationTimer::start("click".to_owned(), known, valid, path)
    }

    #[test]
    fn successful_mixed_output_is_shape_only() {
        let response = Response::ok(
            serde_json::json!(1),
            serde_json::json!({
                "content": [
                    {"type":"text", "text":"private task text"},
                    {"type":"image", "data":"private-base64", "mimeType":"image/png"}
                ],
                "isError": false,
                "structuredContent": {"private": "result body"}
            }),
        );
        let observation = timer(true, true, StdioExecutionPath::InProcess).finish(&response);
        assert!(observation.success);
        assert_eq!(observation.error_class, ToolErrorClass::None);
        assert_eq!(observation.output_type, OutputType::Mixed);
        assert_ne!(observation.output_size_bucket, OutputSizeBucket::Empty);

        let debug = format!("{observation:?}");
        for forbidden in ["private task text", "private-base64", "result body"] {
            assert!(!debug.contains(forbidden), "observer leaked content: {debug}");
        }
    }

    #[test]
    fn page_operation_is_closed_and_retains_no_argument_content() {
        for (action, expected) in [
            ("execute_javascript", ToolOperation::ExecuteJavascript),
            ("get_text", ToolOperation::GetText),
            ("query_dom", ToolOperation::QueryDom),
            ("click_element", ToolOperation::ClickElement),
            ("insert_text", ToolOperation::InsertText),
            ("type_keystrokes", ToolOperation::TypeKeystrokes),
            (
                "enable_javascript_apple_events",
                ToolOperation::EnableJavascriptAppleEvents,
            ),
            ("private-custom-action", ToolOperation::Other),
        ] {
            let args = serde_json::json!({
                "action": action,
                "selector": "private selector",
                "script": "private script",
                "text": "private typed text"
            });
            let operation = tool_operation("page", Some(&args));
            assert_eq!(operation, expected);
            let debug = format!("{operation:?}");
            for forbidden in ["private selector", "private script", "private typed text"] {
                assert!(!debug.contains(forbidden));
            }
        }
        assert_eq!(
            tool_operation("click", Some(&serde_json::json!({"action": "private"}))),
            ToolOperation::NotApplicable
        );
    }

    #[test]
    fn invalid_params_and_unknown_tool_are_distinct() {
        let invalid = Response::error(serde_json::json!(1), -32602, "private invalid params");
        let invalid_observation =
            timer(false, false, StdioExecutionPath::InProcess).finish(&invalid);
        assert!(!invalid_observation.success);
        assert_eq!(invalid_observation.error_class, ToolErrorClass::InvalidParams);
        assert_eq!(invalid_observation.output_type, OutputType::Empty);

        let unknown = Response::ok(
            serde_json::json!(2),
            serde_json::json!({
                "content": [{"type":"text", "text":"private unknown-tool error"}],
                "isError": true
            }),
        );
        let unknown_observation =
            timer(false, true, StdioExecutionPath::InProcess).finish(&unknown);
        assert_eq!(unknown_observation.error_class, ToolErrorClass::UnknownTool);
        assert!(!format!("{unknown_observation:?}").contains("private unknown-tool error"));
    }

    #[test]
    fn proxy_internal_rpc_error_is_transport_error() {
        let response = Response::error(serde_json::json!(1), -32603, "private daemon failure");
        let proxy = timer(true, true, StdioExecutionPath::DaemonProxy).finish(&response);
        let direct = timer(true, true, StdioExecutionPath::InProcess).finish(&response);
        assert_eq!(proxy.error_class, ToolErrorClass::TransportError);
        assert_eq!(direct.error_class, ToolErrorClass::InternalError);
        assert!(!format!("{proxy:?}").contains("private daemon failure"));
    }

    #[test]
    fn structured_codes_map_without_error_text() {
        for (code, expected) in [
            ("background_unavailable", ToolErrorClass::BackgroundUnavailable),
            ("permission_denied", ToolErrorClass::PermissionDenied),
            ("private_custom_error", ToolErrorClass::InternalError),
        ] {
            let response = Response::ok(
                serde_json::json!(1),
                serde_json::json!({
                    "content": [{"type":"text", "text":"private prose"}],
                    "isError": true,
                    "structuredContent": {"code": code, "detail": "private detail"}
                }),
            );
            let observation =
                timer(true, true, StdioExecutionPath::InProcess).finish(&response);
            assert_eq!(observation.error_class, expected, "code={code}");
            let debug = format!("{observation:?}");
            assert!(!debug.contains("private prose"));
            assert!(!debug.contains("private detail"));
            assert!(!debug.contains("private_custom_error"));
        }
    }

    #[test]
    fn tool_timer_retains_no_arguments() {
        let req: Request = serde_json::from_value(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "type_text",
                "arguments": {"text":"secret typed text", "path":"/secret/path"}
            }
        }))
        .unwrap();
        let timer = tool_observation_timer(
            &req,
            |name| name == "type_text",
            StdioExecutionPath::InProcess,
        )
        .unwrap();
        assert_eq!(timer.tool_name, "type_text");
        let debug = format!("{}:{}", timer.tool_name, timer.valid_params);
        assert!(!debug.contains("secret typed text"));
        assert!(!debug.contains("/secret/path"));
    }

    #[test]
    fn observer_enum_labels_are_fixed() {
        assert_eq!(DurationBucket::Under10Ms.as_str(), "lt_10ms");
        assert_eq!(DurationBucket::Ms5000OrMore.as_str(), "gte_5000ms");
        assert_eq!(OutputType::Empty.as_str(), "empty");
        assert_eq!(OutputSizeBucket::MiB1OrMore.as_str(), "gte_1mib");
        assert_eq!(ToolErrorClass::TransportError.as_str(), "transport_error");
    }
}
