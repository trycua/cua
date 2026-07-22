//! Shared MCP request dispatch and privacy-bounded observations.

use std::sync::{Arc, OnceLock};
use std::time::Instant;
use tracing::warn;

use crate::authorization::authorize_tool_call;
use crate::protocol::{initialize_result, InitializeMetadata, Request, Response, ResponseBody};
use crate::tool::ToolRegistry;

/// Runtime contract consumed by protocol adapters such as MCP.
///
/// The standalone server implements this with the public Cua Driver SDK; the
/// `ToolRegistry` implementation remains for core-level tests and embedders.
/// Keeping the protocol dependent on this small contract prevents transports
/// from reaching through the SDK into its private platform registry.
#[async_trait::async_trait]
pub trait ToolProvider: Send + Sync {
    fn tools_list(&self) -> serde_json::Value;
    async fn invoke_tool(
        &self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<serde_json::Value, String>;
}

#[async_trait::async_trait]
impl ToolProvider for ToolRegistry {
    fn tools_list(&self) -> serde_json::Value {
        ToolRegistry::tools_list(self)
    }

    async fn invoke_tool(
        &self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        serde_json::to_value(self.invoke(name, arguments).await)
            .map_err(|error| format!("Serialize error: {error}"))
    }
}

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

/// Fixed structured-refusal codes that may cross the telemetry observer seam.
///
/// This vocabulary deliberately mirrors only reviewed, content-free browser
/// refusal codes. Unknown future wire values become [`Self::Other`] until they
/// are explicitly adopted here; refusal messages and details are never read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolRefusalCode {
    None,
    BrowserRouteUnavailable,
    BrowserRequiresSetup,
    BrowserBindingAmbiguous,
    BrowserBindingStale,
    BrowserWrongTargetRefused,
    BrowserTabRequired,
    BrowserTabNotFound,
    BrowserRefStale,
    BrowserInputTrustUnavailable,
    BrowserEndpointOwnerMismatch,
    BrowserConsentRequired,
    BrowserConsentRevoked,
    BrowserReconnectExhausted,
    BrowserInputIncomplete,
    BrowserActionUnavailable,
    BrowserOriginOutsideScope,
    Other,
}

impl ToolRefusalCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::BrowserRouteUnavailable => "browser_route_unavailable",
            Self::BrowserRequiresSetup => "browser_requires_setup",
            Self::BrowserBindingAmbiguous => "browser_binding_ambiguous",
            Self::BrowserBindingStale => "browser_binding_stale",
            Self::BrowserWrongTargetRefused => "browser_wrong_target_refused",
            Self::BrowserTabRequired => "browser_tab_required",
            Self::BrowserTabNotFound => "browser_tab_not_found",
            Self::BrowserRefStale => "browser_ref_stale",
            Self::BrowserInputTrustUnavailable => "browser_input_trust_unavailable",
            Self::BrowserEndpointOwnerMismatch => "browser_endpoint_owner_mismatch",
            Self::BrowserConsentRequired => "browser_consent_required",
            Self::BrowserConsentRevoked => "browser_consent_revoked",
            Self::BrowserReconnectExhausted => "browser_reconnect_exhausted",
            Self::BrowserInputIncomplete => "browser_input_incomplete",
            Self::BrowserActionUnavailable => "browser_action_unavailable",
            Self::BrowserOriginOutsideScope => "browser_origin_outside_scope",
            Self::Other => "other",
        }
    }

    pub const fn is_refusal(self) -> bool {
        !matches!(self, Self::None)
    }

    fn from_wire_code(code: Option<&str>) -> Self {
        match code {
            Some("browser_route_unavailable") => Self::BrowserRouteUnavailable,
            Some("browser_requires_setup") => Self::BrowserRequiresSetup,
            Some("browser_binding_ambiguous") => Self::BrowserBindingAmbiguous,
            Some("browser_binding_stale") => Self::BrowserBindingStale,
            Some("browser_wrong_target_refused") => Self::BrowserWrongTargetRefused,
            Some("browser_tab_required") => Self::BrowserTabRequired,
            Some("browser_tab_not_found") => Self::BrowserTabNotFound,
            Some("browser_ref_stale") => Self::BrowserRefStale,
            Some("browser_input_trust_unavailable") => Self::BrowserInputTrustUnavailable,
            Some("browser_endpoint_owner_mismatch") => Self::BrowserEndpointOwnerMismatch,
            Some("browser_consent_required") => Self::BrowserConsentRequired,
            Some("browser_consent_revoked") => Self::BrowserConsentRevoked,
            Some("browser_reconnect_exhausted") => Self::BrowserReconnectExhausted,
            Some("browser_input_incomplete") => Self::BrowserInputIncomplete,
            Some("browser_action_unavailable") => Self::BrowserActionUnavailable,
            Some("browser_origin_outside_scope") => Self::BrowserOriginOutsideScope,
            Some(_) | None => Self::Other,
        }
    }
}

impl From<crate::browser::refusal::BrowserRefusalCode> for ToolRefusalCode {
    fn from(code: crate::browser::refusal::BrowserRefusalCode) -> Self {
        use crate::browser::refusal::BrowserRefusalCode;
        match code {
            BrowserRefusalCode::BrowserRouteUnavailable => Self::BrowserRouteUnavailable,
            BrowserRefusalCode::BrowserRequiresSetup => Self::BrowserRequiresSetup,
            BrowserRefusalCode::BrowserBindingAmbiguous => Self::BrowserBindingAmbiguous,
            BrowserRefusalCode::BrowserBindingStale => Self::BrowserBindingStale,
            BrowserRefusalCode::BrowserWrongTargetRefused => Self::BrowserWrongTargetRefused,
            BrowserRefusalCode::BrowserTabRequired => Self::BrowserTabRequired,
            BrowserRefusalCode::BrowserTabNotFound => Self::BrowserTabNotFound,
            BrowserRefusalCode::BrowserRefStale => Self::BrowserRefStale,
            BrowserRefusalCode::BrowserInputTrustUnavailable => Self::BrowserInputTrustUnavailable,
            BrowserRefusalCode::BrowserEndpointOwnerMismatch => Self::BrowserEndpointOwnerMismatch,
            BrowserRefusalCode::BrowserConsentRequired => Self::BrowserConsentRequired,
            BrowserRefusalCode::BrowserConsentRevoked => Self::BrowserConsentRevoked,
            BrowserRefusalCode::BrowserReconnectExhausted => Self::BrowserReconnectExhausted,
            BrowserRefusalCode::BrowserInputIncomplete => Self::BrowserInputIncomplete,
            BrowserRefusalCode::BrowserActionUnavailable => Self::BrowserActionUnavailable,
            BrowserRefusalCode::BrowserOriginOutsideScope => Self::BrowserOriginOutsideScope,
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
    /// Whether the fixed tool/operation category can change computer state.
    /// This is derived before argument content is discarded; no argument
    /// value is retained in the observation.
    pub computer_action: bool,
    pub success: bool,
    pub error_class: ToolErrorClass,
    pub refusal_code: ToolRefusalCode,
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
    BrowserBind,
    BrowserSnapshotDomRefsV1,
    BrowserSnapshotSemanticV2,
    BrowserClickTrusted,
    BrowserClickDomEvent,
    BrowserTypeInsertText,
    BrowserTypeKeystrokes,
    BrowserPrepareIsolated,
    BrowserPrepareExistingProfile,
    BrowserDialogInspect,
    BrowserDialogAccept,
    BrowserDialogDismiss,
    BrowserSetInputFiles,
    BrowserDownload,
    BrowserPointerHoverTrusted,
    BrowserPointerHoverDomEvent,
    BrowserPointerRightClickTrusted,
    BrowserPointerRightClickDomEvent,
    BrowserPointerDoubleClickTrusted,
    BrowserPointerDoubleClickDomEvent,
    BrowserPointerScrollTrusted,
    BrowserPointerScrollDomEvent,
    BrowserPointerDragTrusted,
    BrowserPointerDragDomEvent,
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
            Self::BrowserBind => "browser_bind",
            Self::BrowserSnapshotDomRefsV1 => "browser_snapshot_dom_refs_v1",
            Self::BrowserSnapshotSemanticV2 => "browser_snapshot_semantic_v2",
            Self::BrowserClickTrusted => "browser_click_trusted",
            Self::BrowserClickDomEvent => "browser_click_dom_event",
            Self::BrowserTypeInsertText => "browser_type_insert_text",
            Self::BrowserTypeKeystrokes => "browser_type_keystrokes",
            Self::BrowserPrepareIsolated => "browser_prepare_isolated",
            Self::BrowserPrepareExistingProfile => "browser_prepare_existing_profile",
            Self::BrowserDialogInspect => "browser_dialog_inspect",
            Self::BrowserDialogAccept => "browser_dialog_accept",
            Self::BrowserDialogDismiss => "browser_dialog_dismiss",
            Self::BrowserSetInputFiles => "browser_set_input_files",
            Self::BrowserDownload => "browser_download",
            Self::BrowserPointerHoverTrusted => "browser_pointer_hover_trusted",
            Self::BrowserPointerHoverDomEvent => "browser_pointer_hover_dom_event",
            Self::BrowserPointerRightClickTrusted => "browser_pointer_right_click_trusted",
            Self::BrowserPointerRightClickDomEvent => "browser_pointer_right_click_dom_event",
            Self::BrowserPointerDoubleClickTrusted => "browser_pointer_double_click_trusted",
            Self::BrowserPointerDoubleClickDomEvent => "browser_pointer_double_click_dom_event",
            Self::BrowserPointerScrollTrusted => "browser_pointer_scroll_trusted",
            Self::BrowserPointerScrollDomEvent => "browser_pointer_scroll_dom_event",
            Self::BrowserPointerDragTrusted => "browser_pointer_drag_trusted",
            Self::BrowserPointerDragDomEvent => "browser_pointer_drag_dom_event",
            Self::Other => "other",
        }
    }
}

pub fn tool_operation(tool_name: &str, args: Option<&serde_json::Value>) -> ToolOperation {
    let string_arg = |key: &str| {
        args.and_then(|value| value.get(key))
            .and_then(serde_json::Value::as_str)
    };
    match tool_name {
        "page" => match string_arg("action") {
            Some("execute_javascript") => ToolOperation::ExecuteJavascript,
            Some("get_text") => ToolOperation::GetText,
            Some("query_dom") => ToolOperation::QueryDom,
            Some("click_element") => ToolOperation::ClickElement,
            Some("insert_text") => ToolOperation::InsertText,
            Some("type_keystrokes") => ToolOperation::TypeKeystrokes,
            Some("enable_javascript_apple_events") => ToolOperation::EnableJavascriptAppleEvents,
            _ => ToolOperation::Other,
        },
        "get_browser_state" => {
            if string_arg("target_id").is_some() {
                match string_arg("snapshot_format") {
                    Some("semantic_v2") => ToolOperation::BrowserSnapshotSemanticV2,
                    None | Some("dom_refs_v1") => ToolOperation::BrowserSnapshotDomRefsV1,
                    Some(_) => ToolOperation::Other,
                }
            } else if args.and_then(|value| value.get("pid")).is_some()
                || args.and_then(|value| value.get("window_id")).is_some()
            {
                ToolOperation::BrowserBind
            } else {
                ToolOperation::Other
            }
        }
        "browser_click" => match string_arg("input_route") {
            None | Some("trusted") => ToolOperation::BrowserClickTrusted,
            Some("dom_event") => ToolOperation::BrowserClickDomEvent,
            Some(_) => ToolOperation::Other,
        },
        "browser_type" => match string_arg("mode") {
            None | Some("insert_text") => ToolOperation::BrowserTypeInsertText,
            Some("keystrokes") => ToolOperation::BrowserTypeKeystrokes,
            Some(_) => ToolOperation::Other,
        },
        "browser_prepare" => {
            let strategy = args
                .and_then(|value| value.pointer("/strategy/kind"))
                .and_then(serde_json::Value::as_str);
            let profile_mode = args
                .and_then(|value| value.pointer("/profile/mode"))
                .and_then(serde_json::Value::as_str);
            match (strategy, profile_mode) {
                (Some("existing_profile"), _) => ToolOperation::BrowserPrepareExistingProfile,
                (None, Some("isolated_new" | "isolated_named")) => {
                    ToolOperation::BrowserPrepareIsolated
                }
                _ => ToolOperation::Other,
            }
        }
        "browser_dialog" => match string_arg("action") {
            Some("inspect") => ToolOperation::BrowserDialogInspect,
            Some("accept") => ToolOperation::BrowserDialogAccept,
            Some("dismiss") => ToolOperation::BrowserDialogDismiss,
            _ => ToolOperation::Other,
        },
        "browser_set_input_files" => ToolOperation::BrowserSetInputFiles,
        "browser_download" => ToolOperation::BrowserDownload,
        "browser_pointer" => match (string_arg("action"), string_arg("input_route")) {
            (Some("hover"), None | Some("trusted")) => ToolOperation::BrowserPointerHoverTrusted,
            (Some("hover"), Some("dom_event")) => ToolOperation::BrowserPointerHoverDomEvent,
            (Some("right_click"), None | Some("trusted")) => {
                ToolOperation::BrowserPointerRightClickTrusted
            }
            (Some("right_click"), Some("dom_event")) => {
                ToolOperation::BrowserPointerRightClickDomEvent
            }
            (Some("double_click"), None | Some("trusted")) => {
                ToolOperation::BrowserPointerDoubleClickTrusted
            }
            (Some("double_click"), Some("dom_event")) => {
                ToolOperation::BrowserPointerDoubleClickDomEvent
            }
            (Some("scroll"), None | Some("trusted")) => ToolOperation::BrowserPointerScrollTrusted,
            (Some("scroll"), Some("dom_event")) => ToolOperation::BrowserPointerScrollDomEvent,
            (Some("drag"), None | Some("trusted")) => ToolOperation::BrowserPointerDragTrusted,
            (Some("drag"), Some("dom_event")) => ToolOperation::BrowserPointerDragDomEvent,
            _ => ToolOperation::Other,
        },
        _ => ToolOperation::NotApplicable,
    }
}

/// Classify a tool and its already-bounded operation as a computer action.
///
/// Keeping this at the dispatch boundary gives session aggregates and
/// per-call telemetry one definition without retaining selectors, text, URLs,
/// or any other tool argument.
pub fn is_computer_action(tool_name: &str, operation: ToolOperation) -> bool {
    if tool_name == "page" {
        return matches!(
            operation,
            ToolOperation::ClickElement
                | ToolOperation::InsertText
                | ToolOperation::TypeKeystrokes
                | ToolOperation::EnableJavascriptAppleEvents
        );
    }
    if tool_name == "browser_dialog" {
        return matches!(
            operation,
            ToolOperation::BrowserDialogAccept | ToolOperation::BrowserDialogDismiss
        );
    }
    if matches!(
        tool_name,
        "browser_set_input_files" | "browser_download" | "browser_pointer"
    ) {
        return true;
    }
    crate::tool::default_capabilities_for(tool_name)
        .iter()
        .any(|capability| {
            capability.starts_with("input.pointer.")
                || capability.starts_with("input.keyboard.")
                || matches!(
                    capability.as_str(),
                    "app.launch"
                        | "app.kill"
                        | "window.activate"
                        | "browser.navigate"
                        | "browser.input.click"
                        | "browser.input.type"
                )
        })
}

/// Which daemon transport path produced a response. This only affects the
/// classification of JSON-RPC internal errors and is not emitted directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdioExecutionPath {
    DirectDaemon,
    DaemonProxy,
}

/// Start-state for one tool call. Public so direct daemon and daemon-proxy
/// transports use exactly the same privacy and bucketing logic.
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
pub fn observe_proxy_session_started(metadata: InitializeMetadata) {
    notify_session_started(metadata);
}

/// Notify the registered observer about a proxy-path tool result.
pub fn observe_proxy_tool_completed(outcome: ToolCompletionObservation) {
    notify_tool_completed(outcome);
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
    is_known_tool: impl Fn(&str) -> bool,
    transport: crate::session::SessionTransport,
) -> Option<crate::session::SessionToolContext> {
    if req.method != "tools/call" {
        return None;
    }
    let call = req.tool_call().ok()?;
    let known_tool = call.name == "type_text_chars" || is_known_tool(&call.name);
    let client_kind = match transport {
        crate::session::SessionTransport::Cli => crate::session::SessionClientKind::Cli,
        crate::session::SessionTransport::Daemon => crate::session::SessionClientKind::Direct,
        crate::session::SessionTransport::McpStdio | crate::session::SessionTransport::McpHttp => {
            crate::session::SessionClientKind::Mcp
        }
    };
    crate::session::begin_tool_call(&call.name, &call.args, known_tool, transport, client_kind)
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
    let success =
        timer.valid_params && timer.known_tool && rpc_error_code.is_none() && !tool_reported_error;

    let error_class = if success {
        ToolErrorClass::None
    } else if !timer.valid_params || rpc_error_code == Some(-32602) {
        ToolErrorClass::InvalidParams
    } else if !timer.known_tool {
        ToolErrorClass::UnknownTool
    } else if rpc_error_code == Some(-32603) && timer.path == StdioExecutionPath::DaemonProxy {
        ToolErrorClass::TransportError
    } else if rpc_error_code.is_some() {
        ToolErrorClass::InternalError
    } else {
        structured_error_class(result)
    };
    let refusal_code = if success {
        structured_refusal_code(&timer.tool_name, result)
    } else {
        ToolRefusalCode::None
    };

    let output_type = classify_output_type(result);
    let output_size_bucket = result
        .and_then(serialized_size_without_retaining)
        .map(size_bucket)
        .unwrap_or(OutputSizeBucket::Empty);

    ToolCompletionObservation {
        computer_action: is_computer_action(&timer.tool_name, timer.operation),
        tool_name: timer.tool_name,
        operation: timer.operation,
        success,
        error_class,
        refusal_code,
        duration_bucket,
        output_type,
        output_size_bucket,
    }
}

fn structured_refusal_code(tool_name: &str, result: Option<&serde_json::Value>) -> ToolRefusalCode {
    if !matches!(
        tool_name,
        "get_browser_state"
            | "browser_prepare"
            | "browser_navigate"
            | "browser_click"
            | "browser_type"
            | "browser_dialog"
            | "browser_set_input_files"
            | "browser_download"
            | "browser_pointer"
    ) {
        return ToolRefusalCode::None;
    }
    let structured = result.and_then(|value| value.get("structuredContent"));
    if structured
        .and_then(|value| value.get("status"))
        .and_then(serde_json::Value::as_str)
        != Some("refused")
    {
        return ToolRefusalCode::None;
    }
    ToolRefusalCode::from_wire_code(
        structured
            .and_then(|value| value.pointer("/refusal/code"))
            .and_then(serde_json::Value::as_str),
    )
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
pub async fn handle_request(
    req: Request,
    id: serde_json::Value,
    provider: &dyn ToolProvider,
) -> Response {
    match req.method.as_str() {
        "initialize" => Response::ok(id, initialize_result()),

        "tools/list" => Response::ok(id, provider.tools_list()),

        "tools/call" => match req.tool_call() {
            Err(e) => Response::error(id, -32602, format!("Invalid params: {e}")),
            Ok(mut call) => {
                crate::tool_args::sanitize_reserved_args(&mut call.args);
                if let Err(error) = authorize_tool_call(&call.name, &call.args) {
                    return Response::error(id, -32603, error.to_string());
                }

                let public_session = call
                    .args
                    .get("session")
                    .and_then(serde_json::Value::as_str)
                    .filter(|session| !session.is_empty())
                    .map(str::to_owned);
                if let (Some(arguments), Some(session)) =
                    (call.args.as_object_mut(), public_session)
                {
                    arguments.insert(
                        "_session_id".to_owned(),
                        serde_json::Value::String(session.clone()),
                    );
                    arguments.insert(
                        "_transport_session_id".to_owned(),
                        serde_json::Value::String(session.clone()),
                    );
                    crate::session::touch_session(&session);
                }
                if call.name == "browser_prepare" {
                    if let Some(arguments) = call.args.as_object_mut() {
                        arguments.insert(
                            crate::browser::approval::MCP_HOST_APPROVAL_ARG.to_owned(),
                            serde_json::Value::Bool(true),
                        );
                    }
                }
                if call.name == "browser_download" {
                    if let Some(arguments) = call.args.as_object_mut() {
                        arguments.insert(
                            crate::browser::download::MCP_HOST_DOWNLOAD_APPROVAL_ARG.to_owned(),
                            serde_json::Value::Bool(true),
                        );
                    }
                }

                match provider.invoke_tool(&call.name, call.args).await {
                    Ok(result) => Response::ok(id, result),
                    Err(error) => Response::error(id, -32603, error),
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
        let observation = timer(true, true, StdioExecutionPath::DirectDaemon).finish(&response);
        assert!(observation.success);
        assert_eq!(observation.error_class, ToolErrorClass::None);
        assert_eq!(observation.output_type, OutputType::Mixed);
        assert_ne!(observation.output_size_bucket, OutputSizeBucket::Empty);

        let debug = format!("{observation:?}");
        for forbidden in ["private task text", "private-base64", "result body"] {
            assert!(
                !debug.contains(forbidden),
                "observer leaked content: {debug}"
            );
        }
    }

    #[test]
    fn page_operation_is_closed_and_retains_no_argument_content() {
        for (action, expected, computer_action) in [
            (
                "execute_javascript",
                ToolOperation::ExecuteJavascript,
                false,
            ),
            ("get_text", ToolOperation::GetText, false),
            ("query_dom", ToolOperation::QueryDom, false),
            ("click_element", ToolOperation::ClickElement, true),
            ("insert_text", ToolOperation::InsertText, true),
            ("type_keystrokes", ToolOperation::TypeKeystrokes, true),
            (
                "enable_javascript_apple_events",
                ToolOperation::EnableJavascriptAppleEvents,
                true,
            ),
            ("private-custom-action", ToolOperation::Other, false),
        ] {
            let args = serde_json::json!({
                "action": action,
                "selector": "private selector",
                "script": "private script",
                "text": "private typed text"
            });
            let operation = tool_operation("page", Some(&args));
            assert_eq!(operation, expected);
            assert_eq!(is_computer_action("page", operation), computer_action);
            let debug = format!("{operation:?}");
            for forbidden in ["private selector", "private script", "private typed text"] {
                assert!(!debug.contains(forbidden));
            }
        }
        assert_eq!(
            tool_operation("click", Some(&serde_json::json!({"action": "private"}))),
            ToolOperation::NotApplicable
        );
        assert!(is_computer_action("click", ToolOperation::NotApplicable));
        assert!(!is_computer_action(
            "get_window_state",
            ToolOperation::NotApplicable
        ));
    }

    #[test]
    fn browser_operations_are_closed_and_retain_no_argument_content() {
        for (tool_name, args, expected) in [
            (
                "get_browser_state",
                serde_json::json!({"pid": 42, "window_id": 7, "query": "private query"}),
                ToolOperation::BrowserBind,
            ),
            (
                "get_browser_state",
                serde_json::json!({
                    "target_id": "private-target",
                    "snapshot_format": "dom_refs_v1"
                }),
                ToolOperation::BrowserSnapshotDomRefsV1,
            ),
            (
                "get_browser_state",
                serde_json::json!({
                    "target_id": "private-target",
                    "snapshot_format": "semantic_v2"
                }),
                ToolOperation::BrowserSnapshotSemanticV2,
            ),
            (
                "browser_click",
                serde_json::json!({"input_route": "trusted", "ref": "private-ref"}),
                ToolOperation::BrowserClickTrusted,
            ),
            (
                "browser_click",
                serde_json::json!({"input_route": "dom_event", "ref": "private-ref"}),
                ToolOperation::BrowserClickDomEvent,
            ),
            (
                "browser_type",
                serde_json::json!({"mode": "insert_text", "text": "private typed text"}),
                ToolOperation::BrowserTypeInsertText,
            ),
            (
                "browser_type",
                serde_json::json!({"mode": "keystrokes", "text": "private typed text"}),
                ToolOperation::BrowserTypeKeystrokes,
            ),
            (
                "browser_prepare",
                serde_json::json!({
                    "profile": {"mode": "isolated_named", "name": "private-profile"}
                }),
                ToolOperation::BrowserPrepareIsolated,
            ),
            (
                "browser_prepare",
                serde_json::json!({
                    "strategy": {"kind": "existing_profile"},
                    "approval_token": "private-token"
                }),
                ToolOperation::BrowserPrepareExistingProfile,
            ),
            (
                "browser_dialog",
                serde_json::json!({"action": "accept", "prompt_text": "private prompt"}),
                ToolOperation::BrowserDialogAccept,
            ),
            (
                "browser_set_input_files",
                serde_json::json!({"files": ["/private/path"]}),
                ToolOperation::BrowserSetInputFiles,
            ),
            (
                "browser_download",
                serde_json::json!({"destination_root": "/private/destination"}),
                ToolOperation::BrowserDownload,
            ),
            (
                "browser_pointer",
                serde_json::json!({"action": "drag", "input_route": "dom_event", "ref": "private-ref"}),
                ToolOperation::BrowserPointerDragDomEvent,
            ),
        ] {
            let operation = tool_operation(tool_name, Some(&args));
            assert_eq!(operation, expected, "tool={tool_name}");
            let debug = format!("{operation:?}");
            for forbidden in [
                "private query",
                "private-target",
                "private-ref",
                "private typed text",
                "private-profile",
                "private-token",
                "private prompt",
                "/private/path",
                "/private/destination",
            ] {
                assert!(!debug.contains(forbidden), "observer leaked {forbidden}");
            }
        }
        assert_eq!(
            tool_operation(
                "browser_click",
                Some(&serde_json::json!({"input_route": "private-route"})),
            ),
            ToolOperation::Other
        );
        assert_eq!(
            tool_operation(
                "browser_navigate",
                Some(&serde_json::json!({"url": "https://private.example"})),
            ),
            ToolOperation::NotApplicable
        );
    }

    #[test]
    fn browser_refusals_preserve_protocol_success_without_retaining_content() {
        let response = Response::ok(
            serde_json::json!(1),
            serde_json::json!({
                "content": [{"type":"text", "text":"private refusal prose"}],
                "structuredContent": {
                    "status": "refused",
                    "refusal": {
                        "code": "browser_input_trust_unavailable",
                        "message": "private refusal message",
                        "detail": {"candidate": "private tab id"}
                    }
                }
            }),
        );
        let observation = ToolObservationTimer::start_with_operation(
            "browser_click".to_owned(),
            ToolOperation::BrowserClickTrusted,
            true,
            true,
            StdioExecutionPath::DirectDaemon,
        )
        .finish(&response);
        assert!(observation.success);
        assert_eq!(observation.error_class, ToolErrorClass::None);
        assert_eq!(
            observation.refusal_code,
            ToolRefusalCode::BrowserInputTrustUnavailable
        );
        let debug = format!("{observation:?}");
        for forbidden in [
            "private refusal prose",
            "private refusal message",
            "private tab id",
        ] {
            assert!(!debug.contains(forbidden), "observer leaked {forbidden}");
        }
    }

    #[test]
    fn browser_refusal_allowlist_matches_the_public_contract() {
        use crate::browser::refusal::BrowserRefusalCode;
        for code in [
            BrowserRefusalCode::BrowserRouteUnavailable,
            BrowserRefusalCode::BrowserRequiresSetup,
            BrowserRefusalCode::BrowserBindingAmbiguous,
            BrowserRefusalCode::BrowserBindingStale,
            BrowserRefusalCode::BrowserWrongTargetRefused,
            BrowserRefusalCode::BrowserTabRequired,
            BrowserRefusalCode::BrowserTabNotFound,
            BrowserRefusalCode::BrowserRefStale,
            BrowserRefusalCode::BrowserInputTrustUnavailable,
            BrowserRefusalCode::BrowserEndpointOwnerMismatch,
            BrowserRefusalCode::BrowserConsentRequired,
            BrowserRefusalCode::BrowserConsentRevoked,
            BrowserRefusalCode::BrowserReconnectExhausted,
            BrowserRefusalCode::BrowserInputIncomplete,
            BrowserRefusalCode::BrowserActionUnavailable,
        ] {
            assert_eq!(ToolRefusalCode::from(code).as_str(), code.as_str());
        }
        let unknown = Response::ok(
            serde_json::json!(1),
            serde_json::json!({
                "structuredContent": {
                    "status": "refused",
                    "refusal": {"code": "private_future_refusal"}
                }
            }),
        );
        assert_eq!(
            ToolObservationTimer::start(
                "browser_click".to_owned(),
                true,
                true,
                StdioExecutionPath::DirectDaemon,
            )
            .finish(&unknown)
            .refusal_code,
            ToolRefusalCode::Other
        );
    }

    #[test]
    fn invalid_params_and_unknown_tool_are_distinct() {
        let invalid = Response::error(serde_json::json!(1), -32602, "private invalid params");
        let invalid_observation =
            timer(false, false, StdioExecutionPath::DirectDaemon).finish(&invalid);
        assert!(!invalid_observation.success);
        assert_eq!(
            invalid_observation.error_class,
            ToolErrorClass::InvalidParams
        );
        assert_eq!(invalid_observation.output_type, OutputType::Empty);

        let unknown = Response::ok(
            serde_json::json!(2),
            serde_json::json!({
                "content": [{"type":"text", "text":"private unknown-tool error"}],
                "isError": true
            }),
        );
        let unknown_observation =
            timer(false, true, StdioExecutionPath::DirectDaemon).finish(&unknown);
        assert_eq!(unknown_observation.error_class, ToolErrorClass::UnknownTool);
        assert!(!format!("{unknown_observation:?}").contains("private unknown-tool error"));
    }

    #[test]
    fn proxy_internal_rpc_error_is_transport_error() {
        let response = Response::error(serde_json::json!(1), -32603, "private daemon failure");
        let proxy = timer(true, true, StdioExecutionPath::DaemonProxy).finish(&response);
        let direct = timer(true, true, StdioExecutionPath::DirectDaemon).finish(&response);
        assert_eq!(proxy.error_class, ToolErrorClass::TransportError);
        assert_eq!(direct.error_class, ToolErrorClass::InternalError);
        assert!(!format!("{proxy:?}").contains("private daemon failure"));
    }

    #[test]
    fn structured_codes_map_without_error_text() {
        for (code, expected) in [
            (
                "background_unavailable",
                ToolErrorClass::BackgroundUnavailable,
            ),
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
            let observation = timer(true, true, StdioExecutionPath::DirectDaemon).finish(&response);
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
            StdioExecutionPath::DirectDaemon,
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
        assert_eq!(
            ToolRefusalCode::BrowserConsentRevoked.as_str(),
            "browser_consent_revoked"
        );
        assert_eq!(
            ToolOperation::BrowserSnapshotSemanticV2.as_str(),
            "browser_snapshot_semantic_v2"
        );
    }
}
