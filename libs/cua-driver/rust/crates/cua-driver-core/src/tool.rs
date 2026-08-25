//! Tool trait and registry.

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;

thread_local! {
    static CONSTRUCTION_RUNTIME_SCOPE: std::cell::RefCell<Option<String>> =
        const { std::cell::RefCell::new(None) };
}

use crate::{
    pip_hook,
    protocol::{Content, ToolResult},
    recording::{now_ms, screenshot_for, RecordingSession},
    recording_tools::{
        GetRecordingStateTool, ReplayRegistrySlot, ReplayTrajectoryTool, StartRecordingTool,
        StopRecordingTool,
    },
    tool_args::ArgsExt,
};

tokio::task_local! {
    /// Authorization inherited by nested registry dispatch such as trajectory
    /// replay. The value is installed only by a trusted runtime/session action
    /// surface; caller arguments and public session labels cannot influence it.
    static DISPATCH_AUTHORIZATION_CONTEXT:
        Arc<crate::session_authorization::EffectiveAuthorizationContext>;
    /// Adapter-proved per-call facts inherited by nested registry dispatch.
    /// This value is never populated from the caller's public JSON object.
    static DISPATCH_TRUSTED_INVOCATION_EVIDENCE: TrustedInvocationEvidence;
    /// Opaque generation for runtime-owned mutable resources. Nested
    /// dispatches inherit this key, while public arguments can never select it.
    static DISPATCH_RUNTIME_SCOPE: String;
}

/// Return the immutable authorization context bound to the current dispatch.
///
/// Resource adapters use this instead of consulting process-global
/// compatibility configuration. The value exists only while the canonical
/// registry chokepoint is executing a tool, including nested dispatch.
pub(crate) fn current_dispatch_authorization_context(
) -> Option<Arc<crate::session_authorization::EffectiveAuthorizationContext>> {
    DISPATCH_AUTHORIZATION_CONTEXT.try_with(Arc::clone).ok()
}

#[doc(hidden)]
pub fn current_dispatch_runtime_scope() -> Option<String> {
    DISPATCH_RUNTIME_SCOPE
        .try_with(Clone::clone)
        .ok()
        .or_else(|| CONSTRUCTION_RUNTIME_SCOPE.with(|scope| scope.borrow().clone()))
}

#[doc(hidden)]
pub fn with_runtime_scope<T>(scope: String, action: impl FnOnce() -> T) -> T {
    struct RestoreScope(Option<String>);
    impl Drop for RestoreScope {
        fn drop(&mut self) {
            CONSTRUCTION_RUNTIME_SCOPE.with(|scope| {
                scope.replace(self.0.take());
            });
        }
    }
    let previous = CONSTRUCTION_RUNTIME_SCOPE.with(|current| current.replace(Some(scope)));
    let _restore = RestoreScope(previous);
    action()
}

fn desktop_action_coordinator() -> &'static tokio::sync::Mutex<()> {
    static COORDINATOR: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    COORDINATOR.get_or_init(|| tokio::sync::Mutex::new(()))
}

fn active_text_input_pids() -> &'static Mutex<HashSet<i64>> {
    static ACTIVE: OnceLock<Mutex<HashSet<i64>>> = OnceLock::new();
    ACTIVE.get_or_init(|| Mutex::new(HashSet::new()))
}

#[derive(Debug)]
struct TextInputAdmission {
    pid: i64,
}

impl Drop for TextInputAdmission {
    fn drop(&mut self) {
        active_text_input_pids()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.pid);
    }
}

fn try_admit_text_input(
    tool_name: &str,
    args: &Value,
) -> Result<Option<TextInputAdmission>, ToolResult> {
    if tool_name != "type_text" {
        return Ok(None);
    }
    let Some(pid) = args
        .get("pid")
        .and_then(Value::as_i64)
        .filter(|pid| *pid > 0)
    else {
        // Desktop-scoped input has no stable process identity. It continues to
        // use the process-wide physical action coordinator below.
        return Ok(None);
    };
    let mut active = active_text_input_pids()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !active.insert(pid) {
        let message = format!("text input is already active for pid {pid}");
        return Err(protected_refusal("input_busy", &message));
    }
    Ok(Some(TextInputAdmission { pid }))
}

pub use cua_driver_contract::{CAPABILITY_VERSION, TOOLS_LIST_SCHEMA_VERSION};

/// Metadata for a single tool.
#[derive(Debug, Clone)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub read_only: bool,
    pub destructive: bool,
    pub idempotent: bool,
    pub open_world: bool,
}

impl ToolDef {
    /// Build the runtime MCP definition from a canonical client contract.
    /// Only migrated tools use this bridge; platform-specific tools continue
    /// to own their live schemas until they can pass parity checks.
    pub fn from_contract(contract: &cua_driver_contract::ToolContract) -> Self {
        assert_eq!(
            contract.schema_mode,
            cua_driver_contract::SchemaMode::CanonicalRuntime,
            "portable subset contracts cannot replace live runtime schemas"
        );
        Self {
            name: contract.name.clone(),
            description: contract.description.clone(),
            input_schema: contract.input_schema.clone(),
            read_only: contract.annotations.read_only,
            destructive: contract.annotations.destructive,
            idempotent: contract.annotations.idempotent,
            open_world: contract.annotations.open_world,
        }
    }

    pub fn to_list_entry(&self) -> Value {
        // `capabilities` is always emitted (even when empty) so consumers
        // can rely on the key existing. Additive only — old consumers
        // that ignore the field keep working unchanged.
        //
        // Published SDK tools resolve capabilities from their typed Rust
        // contract. The legacy map remains only for runtime-only tools.
        let input_schema = advertised_runtime_input_schema(&self.name, &self.input_schema);
        let caps = advertised_capabilities_for(&self.name, &input_schema);
        let risk = crate::authorization::risk_metadata_json(&self.name);
        let mut entry = serde_json::json!({
            "name": self.name,
            "description": self.description,
            "inputSchema": input_schema,
            "annotations": {
                "readOnlyHint": self.read_only,
                "destructiveHint": self.destructive,
                "idempotentHint": self.idempotent,
                "openWorldHint": self.open_world,
            },
            "capabilities": caps,
            "risk": risk,
        });
        let output_schema = if crate::action_record::is_action_tool(&self.name) {
            Some(
                <cua_driver_contract::ActionResult as cua_driver_contract::ToolOutput>::output_schema(
                ),
            )
        } else {
            cua_driver_contract::tool_success_output_schema(&self.name)
        };
        if let Some(output_schema) = output_schema {
            // Advertise the refusal envelope alongside the success shape. MCP
            // holds every `structuredContent` we emit — refusals included — to
            // the advertised schema, and a success-only schema made strict
            // clients discard our refusal message in favour of a schema error.
            entry
                .as_object_mut()
                .expect("tool list entry is an object")
                .insert(
                    "outputSchema".into(),
                    cua_driver_contract::advertised_output_schema(output_schema),
                );
        }
        entry
    }
}

fn advertised_runtime_input_schema(tool_name: &str, schema: &Value) -> Value {
    let mut schema = schema.clone();
    if !crate::action_target::supports_typed_target(tool_name) {
        return schema;
    }
    let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) else {
        return schema;
    };
    // Reuse the portable contract's exact tagged-union schema while retaining
    // the live runtime's broader legacy `scope=window|desktop` decoder.
    if let Some(portable) = cua_driver_contract::tool_contract(tool_name) {
        if let Some(portable_properties) = portable
            .input_schema
            .get("properties")
            .and_then(Value::as_object)
        {
            if let Some(field_schema) = portable_properties.get("target") {
                properties.insert("target".into(), field_schema.clone());
            }
        }
    }
    schema
}

/// Centralised tool name → capability tokens map. Lookup is by name so
/// platform-specific tool modules don't have to declare their own
/// capabilities — keeps the additive-only contract tight and avoids
/// merge collisions with sibling agents touching the same tool files.
///
/// ### Vocabulary
/// Capability strings are dotted-namespace tokens. The canonical set
/// is (extend additively as new tools / surfaces ship — never rename
/// without bumping `CAPABILITY_VERSION`):
///
/// - `input.pointer.click`, `input.pointer.click.left`,
///   `input.pointer.click.right`, `input.pointer.click.double`,
///   `input.pointer.drag`, `input.pointer.scroll`,
///   `input.pointer.move`, `input.pointer.button` (raw down/up)
/// - `input.keyboard.type`, `input.keyboard.hotkey`,
///   `input.keyboard.press`
/// - `input.delivery_mode` (the live tool schema accepts the shared
///   `background` / `foreground` delivery ladder)
/// - `screen.capture`, `screen.capture.window`,
///   `screen.capture.region`, `screen.dimensions`,
///   `screen.cursor.position`
/// - `accessibility.tree`, `accessibility.tree.structured`,
///   `accessibility.tree.bounded`, `accessibility.window_state`,
///   `accessibility.element_tokens` (Surface 6 — tool accepts the
///   opaque `element_token` arg alongside the integer `element_index`)
/// - `app.launch`, `app.list`, `app.kill`, `window.list`,
///   `window.activate`, `window.frame.set`, `window.debug_info`
/// - `system.permissions.tcc`,
///   `system.permissions.tcc.accessibility`,
///   `system.permissions.tcc.screen_recording`
/// - `system.config.read`, `system.config.write`
/// - `session.lifecycle.start`, `session.lifecycle.read`,
///   `session.lifecycle.list`, `session.lifecycle.end`, plus the deprecated
///   `session.capture_scope`, `session.capture_scope.read`, and
///   `session.capture_scope.escalate` compatibility tokens
/// - `agent_cursor.move`, `agent_cursor.set_enabled`,
///   `agent_cursor.set_motion`, `agent_cursor.set_theme`,
///   `agent_cursor.state`
/// - `recording.start`, `recording.stop`, `recording.state`,
///   `recording.replay`, `recording.install_dependency`
/// - `page.action`
/// - `browser.state`, `browser.prepare`, `browser.navigate`,
///   `browser.input.click`, `browser.input.type`, `browser.input.files`,
///   `browser.dialog`
/// - `driver.update_check`, `driver.probe`
/// - `history.status`, `history.query`
///
/// Tools with no entry get `[]` — that's fine, it just means
/// downstream consumers fall back to matching by tool name for them.
pub fn default_capabilities_for(tool_name: &str) -> Vec<String> {
    if let Some(capabilities) = cua_driver_contract::tool_capabilities(tool_name) {
        return capabilities;
    }
    let caps: &[&str] = match tool_name {
        // ── input.pointer ────────────────────────────────────────────
        //
        // Surface 6: tools that accept the opaque `element_token` arg
        // (in addition to the integer `element_index`) claim the
        // `accessibility.element_tokens` token so consumers can branch
        // on its presence — Hermes' wrapper currently does this by name
        // for each tool; the capability token removes that coupling.
        "double_click" => &[
            "input.pointer.click",
            "input.pointer.click.left",
            "input.pointer.click.double",
            "accessibility.element_tokens",
        ],
        "right_click" => &[
            "input.pointer.click",
            "input.pointer.click.right",
            "accessibility.element_tokens",
        ],
        "mouse_drag" => &["input.pointer.drag"],
        "parallel_mouse_drag" => &["input.pointer.drag"],
        "mouse_button_down" => &["input.pointer.button"],
        "mouse_button_up" => &["input.pointer.button"],
        // ── input.keyboard ───────────────────────────────────────────
        // `type_text` claims `terminal_safe` because every platform
        // implementation detects terminal-emulator targets (bundle id
        // on macOS, WM_CLASS / process name on Linux, window class on
        // Windows) and routes past the accessibility-text channel to
        // key-event synthesis — bypassing the silent-drop that
        // otherwise affects Ghostty / iTerm2 / Terminal.app / Windows
        // Terminal / mintty / GVim, etc. See the per-platform
        // `terminal` module for the matched list and the structured
        // `path: "ax" | "key_events"` field on the response.
        // `type_text_chars` is a deprecated alias resolved at invoke
        // time on macOS/Windows. On Linux it's still registered (see
        // platform-linux/impl_.rs). The Linux implementation runs
        // XSendEvent per-character without the terminal short-circuit,
        // so we deliberately do NOT claim `terminal_safe` here — the
        // contract is intentionally narrower than `type_text`'s. It
        // still accepts `element_token`, hence the tokens claim.
        "type_text_chars" => &["input.keyboard.type", "accessibility.element_tokens"],
        "set_value" => &[
            // Bulk-set an editable field's value — semantically a
            // typing surface, even though the implementation skips
            // per-key events.
            "input.keyboard.type",
            "accessibility.element_tokens",
        ],

        // ── screen / capture ─────────────────────────────────────────
        // Note: the regular `screenshot` tool was removed from the
        // surface in PR #1692 — get_window_state's vision capture mode
        // is the canonical screenshot path. `zoom` returns a JPEG of
        // a window region, so it claims screen.capture.region.
        "zoom" => &[
            "screen.capture",
            "screen.capture.window",
            "screen.capture.region",
        ],
        // ── accessibility / window state ─────────────────────────────
        "get_accessibility_tree" => &["accessibility.tree", "accessibility.tree.structured"],
        "get_window_state" => &[
            "accessibility.window_state",
            "accessibility.tree",
            "accessibility.tree.structured",
            "accessibility.tree.bounded",
            // Surface 6: emits `element_token` on every structured
            // element entry — paired with the existing integer
            // `element_index`.
            "accessibility.element_tokens",
            // capture_mode:"vision" returns a window screenshot — see
            // platform-{macos,windows,linux}/src/tools/get_window_state.rs.
            "screen.capture",
            "screen.capture.window",
        ],

        // ── apps / windows ───────────────────────────────────────────
        "launch_app" => &["app.launch"],
        "list_apps" => &["app.list"],
        "kill_app" => &["app.kill"],
        "list_windows" => &["window.list"],
        "bring_to_front" => &["window.activate"],
        "set_window_frame" => &["window.frame.set"],
        "debug_window_info" => &["window.debug_info"],

        // ── permissions / config ─────────────────────────────────────
        // The macOS TCC tokens are claimed even on Windows/Linux —
        // `check_permissions` on those platforms still reports the
        // same accessibility/screen_recording booleans (mapped to the
        // platform's own permission model), so the capability surface
        // stays platform-agnostic.
        "check_permissions" => &[
            "system.permissions.tcc",
            "system.permissions.tcc.accessibility",
            "system.permissions.tcc.screen_recording",
        ],
        "get_config" => &["system.config.read"],
        "set_config" => &["system.config.write"],

        // ── agent cursor ─────────────────────────────────────────────
        "set_agent_cursor_enabled" => &["agent_cursor.set_enabled"],
        "set_agent_cursor_motion" => &["agent_cursor.set_motion"],
        "set_agent_cursor_theme" => &["agent_cursor.set_theme"],
        "get_agent_cursor_state" => &["agent_cursor.state"],

        // ── recording / replay ───────────────────────────────────────
        "start_recording" => &["recording.start"],
        "stop_recording" => &["recording.stop"],
        "get_recording_state" => &["recording.state"],
        "replay_trajectory" => &["recording.replay"],
        "install_ffmpeg" => &["recording.install_dependency"],

        // ── cross-platform page ──────────────────────────────────────
        "page" => &["page.action"],

        // ── browser-tool v1 (exact-or-refused CDP surface) ───────────
        // Additive tokens; see `crate::browser` for semantics. The
        // input tools claim distinct browser.* tokens (not the
        // input.pointer/keyboard families) because they act inside a
        // page via CDP, not on the OS input layer.
        "get_browser_state" => &["browser.state"],
        "browser_prepare" => &["browser.prepare"],
        "browser_navigate" => &["browser.navigate"],
        "browser_click" => &["browser.input.click"],
        "browser_type" => &["browser.input.type"],
        "browser_dialog" => &["browser.dialog"],
        "browser_set_input_files" => &["browser.input.files"],
        "browser_download" => &["browser.download"],
        "browser_pointer" => &["browser.input.pointer"],

        // ── driver self-service ──────────────────────────────────────
        "check_for_update" => &["driver.update_check"],
        "probe" => &["driver.probe"],

        // ── encrypted local Computer History ─────────────────────────
        "history_status" => &["history.status"],
        "history_query" => &["history.query"],

        // ── unsupported_platform stub & anything else ────────────────
        _ => &[],
    };
    caps.iter().map(|s| (*s).to_owned()).collect()
}

/// Capabilities advertised by a concrete runtime tool definition.
///
/// Most capabilities are stable properties of a tool name and come from the
/// typed portable contract (or the legacy runtime-only map). Delivery mode is
/// different: the typed desktop SDK intentionally exposes a narrower,
/// desktop-only input while the live platform schemas additionally accept
/// window-targeted `delivery_mode`. Deriving this one token from the concrete
/// schema keeps `tools/list` truthful on every platform and prevents either
/// overclaiming a tool that cannot accept the field or omitting support from a
/// richer live schema.
pub fn advertised_capabilities_for(tool_name: &str, input_schema: &Value) -> Vec<String> {
    let mut capabilities = default_capabilities_for(tool_name);
    let accepts_delivery_mode = schema_accepts_delivery_mode(input_schema);
    if accepts_delivery_mode
        && !capabilities
            .iter()
            .any(|capability| capability == "input.delivery_mode")
    {
        capabilities.push("input.delivery_mode".into());
    }
    capabilities
}

fn schema_accepts_delivery_mode(input_schema: &Value) -> bool {
    input_schema
        .pointer("/properties/delivery_mode")
        .is_some_and(Value::is_object)
}

/// Canonicalize the hidden pre-0.7 Windows `dispatch` compatibility alias.
///
/// This runs at the native dispatch boundary before policy, protected-resource
/// authorization, recording, and platform execution. Every downstream
/// consumer therefore sees the same modern field and one of the two supported
/// values. The live schema remains modern-only, and schema gating prevents an
/// unrelated tool's `dispatch` argument from being reinterpreted.
///
/// Presence of `delivery_mode` always wins, including when its value is null or
/// malformed. As with the platform parsers, only an explicit case-insensitive
/// `foreground` opts into foreground delivery; every other supplied value,
/// including the removed legacy `auto`, fails closed to `background`.
fn normalize_delivery_mode_args(tool: &ToolDef, args: &mut Value) {
    if !schema_accepts_delivery_mode(&tool.input_schema) {
        return;
    }
    let Some(arguments) = args.as_object_mut() else {
        return;
    };

    let supplied = if arguments.contains_key("delivery_mode") {
        arguments.get("delivery_mode")
    } else {
        arguments.get("dispatch")
    };
    let Some(supplied) = supplied else {
        return;
    };
    let mode = if supplied
        .as_str()
        .is_some_and(|value| value.eq_ignore_ascii_case("foreground"))
    {
        "foreground"
    } else {
        "background"
    };

    arguments.remove("dispatch");
    arguments.insert("delivery_mode".to_owned(), Value::String(mode.to_owned()));
}

/// Runtime-owned provenance for protected-resource admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtectedResourceOwnership {
    UserOwned,
    DriverOwned,
}

/// A callable tool handler. Object-safe — uses `Box<dyn Tool>`.
#[async_trait]
pub trait Tool: Send + Sync {
    fn def(&self) -> &ToolDef;

    /// Trusted implementation-side provenance for prompt-light disposable
    /// resources. The default is deliberately conservative: caller arguments
    /// never establish ownership, and an adapter may skip protected consent
    /// only when the concrete tool can prove it from runtime-owned state.
    async fn protected_resource_ownership(
        &self,
        _adapter_id: &str,
        _args: &Value,
    ) -> ProtectedResourceOwnership {
        ProtectedResourceOwnership::UserOwned
    }

    /// Return implementation-attested identity for an exact protected
    /// resource. Callers cannot supply this value directly. Adapters that
    /// require process or browser identity fail closed when the concrete tool
    /// cannot produce it.
    async fn protected_resource_scope(
        &self,
        _adapter_id: &str,
        _args: &Value,
    ) -> Result<Option<Value>, String> {
        Ok(None)
    }

    /// Re-prove a protected scope immediately before a destructive dispatch.
    /// The default is conservative equality against a fresh attestation.
    async fn validate_protected_resource_scope(
        &self,
        adapter_id: &str,
        args: &Value,
        approved_scope: &Value,
    ) -> Result<(), String> {
        match self.protected_resource_scope(adapter_id, args).await? {
            Some(current) if current == *approved_scope => Ok(()),
            Some(_) => Err("the protected resource identity changed before dispatch".to_owned()),
            None => Err("the protected resource identity cannot be re-proven".to_owned()),
        }
    }

    async fn invoke(&self, args: Value) -> ToolResult;
}

struct RuntimeCleanup(Option<Box<dyn FnOnce() + Send + Sync>>);

impl Drop for RuntimeCleanup {
    fn drop(&mut self) {
        if let Some(cleanup) = self.0.take() {
            cleanup();
        }
    }
}

/// Transport evidence admitted only through a trusted protocol adapter.
///
/// The values are extracted from the adapter's private argument envelope and
/// then removed before the canonical authorization boundary evaluates caller
/// input. Keeping the evidence separate lets the registry reject forged
/// underscore-prefixed fields without discarding facts proved by the host
/// transport.
#[derive(Clone, Debug, Default)]
pub struct TrustedInvocationEvidence {
    session_id: Option<String>,
    transport_session_id: Option<String>,
    browser_download_mcp_host_approved: bool,
}

impl TrustedInvocationEvidence {
    #[doc(hidden)]
    pub fn extract_from_adapter_args(args: &mut Value) -> Self {
        let mut evidence = Self::default();
        if let Some(arguments) = args.as_object_mut() {
            evidence.session_id = arguments
                .remove("_session_id")
                .and_then(|value| value.as_str().map(str::to_owned))
                .filter(|value| !value.is_empty());
            evidence.transport_session_id = arguments
                .remove("_transport_session_id")
                .and_then(|value| value.as_str().map(str::to_owned))
                .filter(|value| !value.is_empty());
            evidence.browser_download_mcp_host_approved = arguments
                .remove(crate::browser::download::MCP_HOST_DOWNLOAD_APPROVAL_ARG)
                .and_then(|value| value.as_bool())
                == Some(true);
        }
        crate::tool_args::sanitize_reserved_args(args);
        evidence
    }

    fn apply_runtime_args(&self, args: &mut Value) {
        let Some(arguments) = args.as_object_mut() else {
            return;
        };
        if let Some(session) = self.session_id.as_ref() {
            arguments.insert("_session_id".to_owned(), Value::String(session.clone()));
        }
        if let Some(session) = self.transport_session_id.as_ref() {
            arguments.insert(
                "_transport_session_id".to_owned(),
                Value::String(session.clone()),
            );
        }
        if self.browser_download_mcp_host_approved {
            arguments.insert(
                crate::browser::download::MCP_HOST_DOWNLOAD_APPROVAL_ARG.to_owned(),
                Value::Bool(true),
            );
        }
    }
}

/// Thread-safe collection of all registered tools.
pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
    /// Ordered list of tool names for `tools/list`.
    order: Vec<String>,
    /// Shared recording session — auto-records each non-read-only tool call.
    pub recording: Arc<RecordingSession>,
    /// Optional encrypted Computer History runtime. It is installed only by a
    /// trusted host that admitted the experimental preview.
    history: Option<Arc<crate::history::HistoryManager>>,
    replay_registry: ReplayRegistrySlot,
    session_end_hooks: Vec<crate::session::SessionEndHookRegistration>,
    session_revive_hooks: Vec<crate::session::SessionReviveHookRegistration>,
    cursor_outcome_readers: Vec<crate::session::CursorOutcomeReaderRegistration>,
    _recording_state_readers: Vec<crate::session::RecordingStateReaderRegistration>,
    runtime_cleanups: Vec<RuntimeCleanup>,
    /// Runtime-owned protected-consent broker shared by every resource
    /// adapter. Keeping it at the canonical dispatch boundary prevents
    /// browser, desktop, and file adapters from growing independent provider
    /// identities or grant lifecycles.
    approval_broker: Arc<crate::consent::ApprovalBroker>,
    protected_resource_grants: Arc<crate::consent::ProtectedResourceGrants>,
    protected_resource_ownership: Arc<crate::consent::ProtectedResourceOwnershipStore>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::new_with_protected_consent_provider(None)
    }

    /// Construct a registry with a provider installed by a trusted embedding
    /// host. Public tool calls and transport metadata cannot replace it.
    pub fn new_with_protected_consent_provider(
        provider: Option<Arc<dyn crate::consent::ProtectedConsentProvider>>,
    ) -> Self {
        let approval_broker = Arc::new(crate::consent::ApprovalBroker::new(provider));
        let protected_resource_grants = Arc::new(crate::consent::ProtectedResourceGrants::new(
            approval_broker.clone(),
        ));
        let protected_resource_ownership =
            Arc::new(crate::consent::ProtectedResourceOwnershipStore::default());
        let weak_grants = Arc::downgrade(&protected_resource_grants);
        let weak_ownership = Arc::downgrade(&protected_resource_ownership);
        let session_end_hook =
            crate::session::register_scoped_session_end_hook(move |session_id| {
                if let Some(ownership) = weak_ownership.upgrade() {
                    ownership.remove_session(session_id);
                }
                let Some(grants) = weak_grants.upgrade() else {
                    return;
                };
                let revoked = grants.revoke_session(session_id);
                if revoked.is_empty() {
                    return;
                }
                if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                    runtime.spawn(async move {
                        for grant in revoked {
                            grants.broker().revoke(&grant).await;
                        }
                    });
                }
            });
        let recording = Arc::new(RecordingSession::new());
        let weak_recording = Arc::downgrade(&recording);
        let recording_state_reader =
            crate::session::register_scoped_recording_state_reader(Arc::new(move |session_id| {
                weak_recording.upgrade().is_some_and(|recording| {
                    let state = recording.current_state();
                    state.enabled && state.owner.as_deref() == Some(session_id)
                })
            }));
        Self {
            tools: HashMap::new(),
            order: Vec::new(),
            recording,
            history: None,
            replay_registry: Arc::new(std::sync::Mutex::new(std::sync::Weak::new())),
            session_end_hooks: vec![session_end_hook],
            session_revive_hooks: Vec::new(),
            cursor_outcome_readers: Vec::new(),
            _recording_state_readers: vec![recording_state_reader],
            runtime_cleanups: Vec::new(),
            approval_broker,
            protected_resource_grants,
            protected_resource_ownership,
        }
    }

    /// Return the runtime-owned broker for adapter construction.
    ///
    /// This is intentionally not exposed through MCP/CLI arguments. Platform
    /// registries pass the clone to resource adapters while assembling one
    /// trusted runtime.
    pub fn approval_broker(&self) -> Arc<crate::consent::ApprovalBroker> {
        self.approval_broker.clone()
    }

    pub fn protected_resource_grants(&self) -> Arc<crate::consent::ProtectedResourceGrants> {
        self.protected_resource_grants.clone()
    }

    pub fn protected_resource_ownership(
        &self,
    ) -> Arc<crate::consent::ProtectedResourceOwnershipStore> {
        self.protected_resource_ownership.clone()
    }

    async fn snapshot_running_pids(&self) -> Option<std::collections::BTreeSet<i64>> {
        let tool = self.tools.get("list_apps")?;
        let result = tool.invoke(serde_json::json!({})).await;
        if result.is_error == Some(true) {
            return None;
        }
        let apps = result.structured_content?.get("apps")?.as_array()?.clone();
        Some(
            apps.into_iter()
                .filter_map(|app| app.get("pid").and_then(Value::as_i64))
                .filter(|pid| *pid > 0)
                .collect(),
        )
    }

    async fn attest_process_fingerprint(
        &self,
        pid: i64,
    ) -> Option<crate::browser::ProcessFingerprint> {
        let tool = self.tools.get("kill_app")?;
        let scope = tool
            .protected_resource_scope("process_control", &serde_json::json!({"pid": pid}))
            .await
            .ok()??;
        serde_json::from_value(scope.get("fingerprint")?.clone()).ok()
    }

    async fn enrich_application_resource(&self, resource: &mut Value, pid: i64) {
        if let Some(fingerprint) = self.attest_process_fingerprint(pid).await {
            resource["fingerprint"] = serde_json::to_value(fingerprint).unwrap_or(Value::Null);
        }
        let Some(tool) = self.tools.get("list_apps") else {
            return;
        };
        let result = tool.invoke(serde_json::json!({})).await;
        let Some(app) = result
            .structured_content
            .as_ref()
            .and_then(|value| value.get("apps"))
            .and_then(Value::as_array)
            .and_then(|apps| {
                apps.iter()
                    .find(|app| app.get("pid").and_then(Value::as_i64) == Some(pid))
            })
        else {
            return;
        };
        for key in ["bundle_id", "launch_path"] {
            if let Some(value) = app.get(key).filter(|value| !value.is_null()) {
                resource[key] = value.clone();
            }
        }
    }

    /// Content-free authorization status for this exact runtime.
    pub fn authorization_status_json(&self) -> Value {
        crate::authorization::status_json_with_provider(self.approval_broker.provider_id())
    }

    pub fn register(&mut self, tool: Box<dyn Tool>) {
        let name = tool.def().name.clone();
        self.order.push(name.clone());
        self.tools.insert(name, tool);
    }

    pub fn retain_session_end_hook(
        &mut self,
        registration: crate::session::SessionEndHookRegistration,
    ) {
        self.session_end_hooks.push(registration);
    }

    pub fn retain_session_revive_hook(
        &mut self,
        registration: crate::session::SessionReviveHookRegistration,
    ) {
        self.session_revive_hooks.push(registration);
    }

    pub fn retain_cursor_outcome_reader(
        &mut self,
        registration: crate::session::CursorOutcomeReaderRegistration,
    ) {
        self.cursor_outcome_readers.push(registration);
    }

    pub fn retain_runtime_cleanup(&mut self, cleanup: impl FnOnce() + Send + Sync + 'static) {
        self.runtime_cleanups
            .push(RuntimeCleanup(Some(Box::new(cleanup))));
    }

    /// Register the four platform-independent recording/replay tools.
    /// Call this after all platform tools have been registered.
    pub fn register_recording_tools(&mut self) {
        let session = self.recording.clone();
        self.register(Box::new(StartRecordingTool::new(session.clone())));
        self.register(Box::new(StopRecordingTool::new(session.clone())));
        self.register(Box::new(GetRecordingStateTool::new(session)));
        self.register(Box::new(ReplayTrajectoryTool::new(
            self.replay_registry.clone(),
        )));
        self.register(Box::new(crate::recording_tools::InstallFfmpegTool));
    }

    /// Install the encrypted history hook and its two permission-gated,
    /// read-only agent tools. Lifecycle mutation remains daemon-private.
    pub fn register_history_tools(&mut self, manager: Arc<crate::history::HistoryManager>) {
        self.history = Some(manager.clone());
        self.register(Box::new(crate::history::HistoryStatusTool::new(
            manager.clone(),
        )));
        self.register(Box::new(crate::history::HistoryQueryTool::new(manager)));
    }

    pub fn history(&self) -> Option<Arc<crate::history::HistoryManager>> {
        self.history.clone()
    }

    /// Register the platform-independent lifecycle and compatibility tools
    /// (`start_session`, `get_session`, `list_sessions`, `end_session`, and
    /// the legacy capture-scope readers). Call alongside
    /// `register_recording_tools` from each platform's `register_all`.
    pub fn register_session_tools(&mut self) {
        use crate::session_tools::{
            EndSessionTool, EscalateSessionTool, GetSessionStateTool, GetSessionTool,
            ListSessionsTool, StartSessionTool,
        };
        self.register(Box::new(StartSessionTool));
        self.register(Box::new(EscalateSessionTool));
        self.register(Box::new(GetSessionTool));
        self.register(Box::new(ListSessionsTool));
        self.register(Box::new(GetSessionStateTool));
        self.register(Box::new(EndSessionTool));
    }

    /// Wire up the replay tool's weak self-reference.
    /// Call this once, immediately after `Arc::new(registry)`.
    pub fn init_self_weak(self: &Arc<Self>) {
        *self.replay_registry.lock().unwrap() = Arc::downgrade(self);
    }

    pub fn tools_list(&self) -> Value {
        let list: Vec<Value> = self
            .order
            .iter()
            .filter(|name| crate::policy::is_tool_listable(name))
            .filter_map(|name| self.tools.get(name))
            .map(|tool| tool.def().to_list_entry())
            .collect();
        // `capability_version` is the contract version for the
        // capability tokens claimed by each tool entry. Bumped on
        // BREAKING vocabulary changes only; additive changes (new
        // tokens, new tools, new claims) keep the version. See
        // `CAPABILITY_VERSION` for the policy.
        //
        // `schema_version` is the contract version for the rest of
        // the tools/list entry shape (name/description/inputSchema/
        // annotations/capabilities). Pinned at "1" today — bumped on
        // a BREAKING change to that shape, NOT when we add a new
        // optional field (those stay backward-compatible).
        //
        // Both fields are additive: existing consumers that read only
        // `tools` keep working unchanged.
        serde_json::json!({
            "tools": list,
            "capability_version": CAPABILITY_VERSION,
            "schema_version": TOOLS_LIST_SCHEMA_VERSION,
            "enforcement_adapters": crate::authorization::enforcement_adapter_inventory_json(),
        })
    }

    /// Iterate over (name, &ToolDef) in registration order.
    pub fn iter_defs(&self) -> impl Iterator<Item = (&str, &ToolDef)> {
        self.order
            .iter()
            .filter_map(move |n| self.tools.get(n).map(|t| (n.as_str(), t.def())))
    }

    /// Get a tool's ToolDef by name, or None if unknown.
    pub fn get_def(&self, name: &str) -> Option<&ToolDef> {
        self.tools.get(name).map(|t| t.def())
    }

    /// List all tool names in registration order.
    pub fn tool_names(&self) -> impl Iterator<Item = &str> {
        self.order.iter().map(|s| s.as_str())
    }

    /// Invoke a tool by name and (if recording is enabled) write its result to disk.
    pub async fn invoke(&self, name: &str, args: Value) -> ToolResult {
        if let Ok(context) = DISPATCH_AUTHORIZATION_CONTEXT.try_with(Arc::clone) {
            let mut args = args;
            let evidence = DISPATCH_TRUSTED_INVOCATION_EVIDENCE
                .try_with(Clone::clone)
                .unwrap_or_default();
            crate::tool_args::sanitize_reserved_args(&mut args);
            if let Some(bound_session) = context.public_session() {
                let Some(arguments) = args.as_object_mut() else {
                    return permission_denied_result(
                        "session-bound nested actions require an object argument".to_owned(),
                    );
                };
                if arguments
                    .get("session")
                    .and_then(Value::as_str)
                    .is_some_and(|session| session != bound_session)
                {
                    return permission_denied_result(
                        "nested action session does not match the bound authorization context"
                            .to_owned(),
                    );
                }
                arguments.insert(
                    "session".to_owned(),
                    Value::String(bound_session.to_owned()),
                );
            }
            return self
                .invoke_authorized(name, args, context.as_ref(), &evidence)
                .await;
        }
        let context = match crate::session_authorization::configured_registry()
            .and_then(crate::session_authorization::SessionAuthorizationRegistry::legacy_context)
        {
            Ok(context) => context,
            Err(error) => {
                return permission_denied_result(format!(
                    "authorization configuration is invalid: {error}"
                ))
            }
        };
        self.invoke_with_context(name, args, context).await
    }

    /// Invoke from a protocol adapter that already stripped caller-owned
    /// reserved fields before adding its own transport evidence.
    #[doc(hidden)]
    pub async fn invoke_from_trusted_adapter(&self, name: &str, mut args: Value) -> ToolResult {
        let evidence = TrustedInvocationEvidence::extract_from_adapter_args(&mut args);
        let context = match crate::session_authorization::configured_registry()
            .and_then(crate::session_authorization::SessionAuthorizationRegistry::legacy_context)
        {
            Ok(context) => context,
            Err(error) => {
                return permission_denied_result(format!(
                    "authorization configuration is invalid: {error}"
                ))
            }
        };
        self.invoke_with_context_and_evidence(name, args, context, evidence)
            .await
    }

    /// Invoke through an immutable context chosen by the trusted runtime host.
    ///
    /// The task-local scope deliberately propagates the same authority through
    /// nested registry calls. Without this, replay or another composite tool
    /// could accidentally fall back to the process compatibility context.
    pub async fn invoke_with_context(
        &self,
        name: &str,
        args: Value,
        context: Arc<crate::session_authorization::EffectiveAuthorizationContext>,
    ) -> ToolResult {
        self.invoke_with_context_and_evidence(
            name,
            args,
            context,
            TrustedInvocationEvidence::default(),
        )
        .await
    }

    #[doc(hidden)]
    pub async fn invoke_with_context_and_evidence(
        &self,
        name: &str,
        args: Value,
        context: Arc<crate::session_authorization::EffectiveAuthorizationContext>,
        evidence: TrustedInvocationEvidence,
    ) -> ToolResult {
        let runtime_scope = context.runtime_scope_key();
        DISPATCH_RUNTIME_SCOPE
            .scope(runtime_scope, async {
                DISPATCH_TRUSTED_INVOCATION_EVIDENCE
                    .scope(evidence.clone(), async {
                        DISPATCH_AUTHORIZATION_CONTEXT
                            .scope(
                                context.clone(),
                                self.invoke_authorized(name, args, context.as_ref(), &evidence),
                            )
                            .await
                    })
                    .await
            })
            .await
    }

    async fn invoke_authorized(
        &self,
        name: &str,
        mut args: Value,
        context: &crate::session_authorization::EffectiveAuthorizationContext,
        evidence: &TrustedInvocationEvidence,
    ) -> ToolResult {
        // This function is the canonical native dispatch boundary. Keep
        // transport-side stripping as defense in depth, but never trust a
        // future same-process embedder to remember it: underscore-prefixed
        // fields carry registry-internal attestations and must not be
        // caller-forgeable here.
        crate::tool_args::sanitize_reserved_args(&mut args);

        if crate::session::is_runtime_scope_suspended(&context.runtime_scope_key()) {
            return protected_refusal(
                "authorization_suspended",
                "authorization for this Cua runtime has been suspended by revoke-all",
            );
        }
        if context.is_revoked() {
            return protected_refusal(
                "authorization_revoked",
                "this session authorization context has been revoked",
            );
        }

        // Deprecated alias: `type_text_chars` → `type_text`.  Swift's
        // ToolRegistry.swift keeps the same alias (with stderr warning) for
        // backwards compatibility with hermes-agent builds that still emit
        // the old name.  Aliased name is intentionally not registered, so it
        // never appears in tools/list.
        let resolved_name: &str = match name {
            "type_text_chars" => {
                eprintln!("[cua-driver-rs] deprecated tool name 'type_text_chars' — use 'type_text' instead.");
                "type_text"
            }
            other => other,
        };

        let Some(tool) = self.tools.get(resolved_name) else {
            return ToolResult::error(format!("Unknown tool: {name}"));
        };

        // Normalize deprecated public argument spellings before any policy,
        // consent, recording, or implementation layer interprets the call.
        normalize_delivery_mode_args(tool.def(), &mut args);
        if let Err(result) = crate::action_target::normalize_action_target(resolved_name, &mut args)
        {
            return result;
        }

        // This registry is the canonical native dispatch boundary shared by
        // the same-process SDK and every transport adapter. Authorization must
        // live here: transport-only checks leave CuaDriver::create() able to
        // invoke platform tools without policy, permission-mode, hard-
        // invariant, or reviewed-risk enforcement.
        //
        // Transports may still reject earlier for defense in depth, but those
        // checks must remain side-effect free. Active consent/grant adapters
        // run only downstream of this boundary so a call cannot prompt twice.
        if let Err(error) =
            crate::authorization::authorize_tool_call_with_context(resolved_name, &args, context)
        {
            return permission_denied_result(error.to_string());
        }

        let mut public_args = args.clone();

        // Public session labels remain part of the stable transport contract,
        // but mutable core/platform state must not be keyed by that
        // caller-chosen label alone. Translate it only after authorization so
        // policy and manifests continue to evaluate the public request.
        let runtime_prefix = namespace_runtime_args(&mut args, context, evidence);
        if session_selecting_tool(resolved_name)
            && args.get("_session_id").and_then(Value::as_str).is_none()
        {
            let implicit = args
                .get("_transport_session_id")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| format!("{runtime_prefix}implicit-direct"));
            args["_session_id"] = Value::String(implicit.clone());
            args["_transport_session_id"] = Value::String(implicit);
        }
        let runtime_session = args
            .get("_session_id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if let Some(session) = runtime_session.as_deref() {
            if !matches!(resolved_name, "start_session" | "end_session")
                && crate::session::is_session_ended(session)
            {
                let mut result = protected_refusal(
                    "session_ended",
                    "this session has ended; call start_session explicitly to reuse its label",
                );
                restore_public_runtime_result(&mut result, &runtime_prefix);
                return result;
            }
        }

        // Reject modality violations before reserving a recording turn. A
        // rejected action has no before/after evidence and must not leave a
        // pending recorder entry behind.
        if let Err(violation) = crate::capture_scope::enforce_tool(resolved_name, &args) {
            let structured =
                violation.as_json(args.get("session").and_then(Value::as_str).unwrap_or(""));
            let mut result = ToolResult::error(violation.message).with_structured(structured);
            restore_public_runtime_result(&mut result, &runtime_prefix);
            return result;
        }

        // A queued text mutation can become stale while another agent is
        // typing into the same process. Refuse that overlap before consent,
        // cursor, recording, or platform focus behavior can begin. The guard
        // is process-global so independent registries cannot interleave text
        // through separate platform workers, and RAII releases it on task
        // cancellation as well as normal completion.
        let _text_input_admission = match try_admit_text_input(resolved_name, &public_args) {
            Ok(admission) => admission,
            Err(mut refusal) => {
                restore_public_runtime_result(&mut refusal, &runtime_prefix);
                return refusal;
            }
        };

        let active_adapters =
            crate::authorization::enforcement_adapters_for_call(resolved_name, &public_args);
        let has_adapter = |id: &str| active_adapters.iter().any(|adapter| adapter.id == id);
        let current_process_fingerprint = if has_adapter("process_control") {
            match tool
                .protected_resource_scope("process_control", &public_args)
                .await
            {
                Ok(Some(scope)) => serde_json::from_value::<crate::browser::ProcessFingerprint>(
                    scope["fingerprint"].clone(),
                )
                .ok(),
                Ok(None) | Err(_) => None,
            }
        } else {
            None
        };
        let process_ownership_key =
            process_ownership_key(runtime_session.as_deref(), &runtime_prefix);
        let runtime_proves_driver_owned =
            current_process_fingerprint
                .as_ref()
                .is_some_and(|fingerprint| {
                    self.protected_resource_ownership
                        .reprove_driver_owned_process(&process_ownership_key, fingerprint)
                });

        if context.mode() == crate::authorization::PermissionMode::Standard
            && has_adapter("process_control")
            && !runtime_proves_driver_owned
        {
            return protected_refusal(
                "foreign_process_termination_denied",
                "standard mode may terminate only a process proven to have been launched by this Cua runtime",
            );
        }
        if runtime_proves_driver_owned {
            if let Some(fingerprint) = current_process_fingerprint.as_ref() {
                args["_protected_process_fingerprint"] =
                    serde_json::to_value(fingerprint).unwrap_or(Value::Null);
            }
        }

        // Raw/legacy page mutations have no exact browser binding. They are
        // not grantable through a model-time confirmation: only a trusted
        // host's unrestricted launch acknowledgement admits them, and the
        // legacy feature flag still applies downstream.
        if has_adapter("browser_unbounded_script")
            && context.mode() != crate::authorization::PermissionMode::Unrestricted
        {
            return protected_refusal(
                "unbounded_operation_requires_unrestricted",
                "legacy page mutation is unbounded and requires unrestricted mode with trusted launch-time risk acceptance",
            );
        }

        // Cua may report OS permission state, but an agent tool call never
        // manufactures a protected system prompt. Host setup happens outside
        // the model/tool stream in every permission mode.
        if has_adapter("os_permission_prompt")
            && tool
                .protected_resource_ownership("os_permission_prompt", &public_args)
                .await
                != ProtectedResourceOwnership::DriverOwned
        {
            return protected_refusal(
                "os_permission_prompt_requires_trusted_host",
                "operating-system permission prompts must be initiated by a trusted host outside the agent tool path; call check_permissions with prompt=false to inspect state",
            );
        }

        // Resource adapters run at the same canonical boundary as policy and
        // manifest admission, after session capture-scope validation but
        // before recording or the platform tool can observe user state. This
        // avoids prompting for a call the capability manifest will refuse, while
        // preserving the public arguments in the grant scope and the private
        // runtime session key in its revocation lifecycle.
        let history_observation = has_adapter("computer_history");
        if (has_adapter("private_observation") || history_observation)
            && (history_observation
                || (!runtime_proves_driver_owned
                    && tool
                        .protected_resource_ownership("private_observation", &public_args)
                        .await
                        != ProtectedResourceOwnership::DriverOwned)
                || context.capability_manifest().is_some())
        {
            if let Err(error) = self
                .authorize_private_observation(
                    tool.as_ref(),
                    resolved_name,
                    &public_args,
                    context,
                    runtime_session.as_deref(),
                    if history_observation {
                        "computer_history"
                    } else {
                        "private_observation"
                    },
                )
                .await
            {
                return protected_consent_refusal(error);
            }
        }
        if has_adapter("clipboard") {
            if let Err(error) = self
                .authorize_clipboard(
                    resolved_name,
                    &public_args,
                    context,
                    runtime_session.as_deref(),
                )
                .await
            {
                return protected_consent_refusal(error);
            }
        }
        if has_adapter("file_transfer_and_output") {
            if let Err(refusal) = self
                .authorize_file_transfer(
                    resolved_name,
                    &mut public_args,
                    context,
                    runtime_session.as_deref(),
                )
                .await
            {
                return refusal;
            }
            // File adapters rewrite exact paths to their canonical form after
            // approval. Dispatch must receive those same values, while the
            // private session namespace remains transport-internal.
            args = public_args.clone();
            namespace_runtime_args(&mut args, context, evidence);
        }
        let recording_args = recording_args_for(resolved_name, &public_args);
        if has_adapter("desktop_input")
            && (!runtime_proves_driver_owned || context.capability_manifest().is_some())
        {
            if let Err(error) = self
                .authorize_desktop_input(
                    resolved_name,
                    &public_args,
                    context,
                    runtime_session.as_deref(),
                )
                .await
            {
                return protected_consent_refusal(error);
            }
        }
        if has_adapter("browser_consequential_action")
            && (tool
                .protected_resource_ownership("browser_consequential_action", &public_args)
                .await
                != ProtectedResourceOwnership::DriverOwned
                || context.capability_manifest().is_some())
        {
            let approved_scope = match self
                .authorize_attested_resource(
                    tool.as_ref(),
                    "browser_consequential_action",
                    crate::authorization::RiskClass::R3,
                    &public_args,
                    context,
                    runtime_session.as_deref(),
                    "Allow Cua to resolve this exact page-owned browser dialog",
                    Duration::from_secs(5 * 60),
                    Duration::from_secs(30 * 60),
                )
                .await
            {
                Ok(scope) => scope,
                Err(refusal) => return refusal,
            };
            if let Err(refusal) = self
                .validate_attested_resource(
                    tool.as_ref(),
                    "browser_consequential_action",
                    &public_args,
                    context,
                    runtime_session.as_deref(),
                    &approved_scope,
                )
                .await
            {
                return refusal;
            }
        }
        if has_adapter("browser_bound_input")
            && (tool
                .protected_resource_ownership("browser_bound_input", &public_args)
                .await
                != ProtectedResourceOwnership::DriverOwned
                || context.capability_manifest().is_some())
        {
            let approved_scope = match self
                .authorize_attested_resource(
                    tool.as_ref(),
                    "browser_bound_input",
                    crate::authorization::RiskClass::R2,
                    &public_args,
                    context,
                    runtime_session.as_deref(),
                    "Allow Cua to control this exact authenticated browser tab",
                    Duration::from_secs(30 * 60),
                    Duration::from_secs(8 * 60 * 60),
                )
                .await
            {
                Ok(scope) => scope,
                Err(refusal) => return refusal,
            };
            if let Err(refusal) = self
                .validate_attested_resource(
                    tool.as_ref(),
                    "browser_bound_input",
                    &public_args,
                    context,
                    runtime_session.as_deref(),
                    &approved_scope,
                )
                .await
            {
                return refusal;
            }
        }
        if has_adapter("process_control")
            && (!runtime_proves_driver_owned || context.capability_manifest().is_some())
        {
            let approved_scope = match self
                .authorize_attested_resource(
                    tool.as_ref(),
                    "process_control",
                    crate::authorization::RiskClass::R3,
                    &public_args,
                    context,
                    runtime_session.as_deref(),
                    "Allow Cua to force-terminate this exact process instance",
                    Duration::from_secs(2 * 60),
                    Duration::from_secs(10 * 60),
                )
                .await
            {
                Ok(scope) => scope,
                Err(refusal) => return refusal,
            };
            if let Err(refusal) = self
                .validate_attested_resource(
                    tool.as_ref(),
                    "process_control",
                    &public_args,
                    context,
                    runtime_session.as_deref(),
                    &approved_scope,
                )
                .await
            {
                return refusal;
            }
            if context.mode() != crate::authorization::PermissionMode::Unrestricted {
                args["_protected_process_fingerprint"] = approved_scope["fingerprint"].clone();
            }
        }
        if has_adapter("driver_configuration") {
            if let Err(refusal) = self
                .authorize_driver_configuration(
                    tool.as_ref(),
                    &public_args,
                    context,
                    runtime_session.as_deref(),
                )
                .await
            {
                return refusal;
            }
        }

        if let Err(error) = context.commit_authorized_dispatch() {
            return permission_denied_result(error);
        }

        let lifecycle_dispatch =
            if session_requiring_tool(resolved_name) && resolved_name != "start_session" {
                let Some(session_id) = runtime_session.as_deref() else {
                    return protected_refusal(
                        "session_unavailable",
                        "an admitted session-requiring call has no lifecycle identity",
                    );
                };
                let owner = args
                    .get("_transport_session_id")
                    .and_then(Value::as_str)
                    .unwrap_or(session_id);
                let public_label = args.get("_public_session_label").and_then(Value::as_str);
                let (transport, client_kind) = crate::session::infer_transport_metadata(owner);
                let admitted = match context.lifecycle_idle_ttl_override() {
                    Some(idle_ttl) => crate::session::begin_session_dispatch_with_ttl(
                        session_id,
                        public_label,
                        owner,
                        public_label.is_none(),
                        transport,
                        client_kind,
                        idle_ttl,
                    ),
                    None => crate::session::begin_session_dispatch(
                        session_id,
                        public_label,
                        owner,
                        public_label.is_none(),
                        transport,
                        client_kind,
                    ),
                };
                match admitted {
                    Ok(guard) => Some(guard),
                    Err(message) => {
                        return protected_refusal("session_ended", message);
                    }
                }
            } else {
                None
            };

        // Capture start time for recording timestamps only after validation.
        let launch_snapshot = if resolved_name == "launch_app" {
            self.snapshot_running_pids().await
        } else {
            None
        };
        let start_ms = now_ms();
        let cursor_event = crate::cursor_events::begin_tool(resolved_name, &args);
        let pending_history = self.history.as_ref().and_then(|history| {
            history.begin_action(resolved_name, &public_args, runtime_session.as_deref())
        });

        // Reserve and capture the turn before dispatch so recorded evidence
        // shows the application immediately before the action changed it.
        let should_record = !tool.def().read_only
            && !matches!(
                resolved_name,
                "start_recording" | "stop_recording" | "get_recording_state" | "replay_trajectory"
            );
        let private_consent_turn = is_existing_profile_prepare(resolved_name, &args);
        let _desktop_action = if is_physical_desktop_action(resolved_name) {
            let coordinator = desktop_action_coordinator();
            // Avoid yielding the dispatch task when the process-wide input
            // lane is uncontended. On Windows, that yield creates a window in
            // which the foreground target can lose keyboard eligibility
            // between the fixture's focus proof and SendInput. Contended
            // runtimes still wait and serialize through the same mutex.
            Some(match coordinator.try_lock() {
                Ok(guard) => guard,
                Err(_) => coordinator.lock().await,
            })
        } else {
            None
        };
        let pending_turn = should_record
            .then(|| {
                if private_consent_turn {
                    self.recording
                        .begin_private_turn(resolved_name, &recording_args, start_ms)
                } else {
                    self.recording
                        .begin_turn(resolved_name, &recording_args, start_ms)
                }
            })
            .flatten();

        let mut result = tool.invoke(args.clone()).await;
        drop(lifecycle_dispatch);
        // The platform worker has exited, so another text operation for this
        // pid may now start even while result projection and evidence capture
        // finish for the completed call.
        drop(_text_input_admission);
        if result.action_record.is_none() {
            if let Some(structured) = result.structured_content.as_ref() {
                result.action_record = crate::action_record::ActionExecutionRecord::from_legacy(
                    resolved_name,
                    &public_args,
                    structured,
                );
            } else if result.is_error != Some(true) {
                // Some legacy successful actions return text only. Normalize
                // them through an empty payload so internal accounting exists
                // before the public contract cutover.
                result.action_record = crate::action_record::ActionExecutionRecord::from_legacy(
                    resolved_name,
                    &public_args,
                    &Value::Null,
                );
            }
        }
        if let Some(delta) = result.surface_delta.take() {
            if let Some(record) = result.action_record.as_mut() {
                record.observe_surface_delta(delta);
            } else {
                result.surface_delta = Some(delta);
            }
        }
        if resolved_name == "launch_app" && result.is_error != Some(true) {
            if let (Some(before), Some(pid)) = (
                launch_snapshot.as_ref(),
                result
                    .structured_content
                    .as_ref()
                    .and_then(|value| value.get("pid"))
                    .and_then(Value::as_i64),
            ) {
                if pid > 0 && !before.contains(&pid) {
                    if let Some(fingerprint) = self.attest_process_fingerprint(pid).await {
                        self.protected_resource_ownership
                            .mark_driver_owned_process(&process_ownership_key, fingerprint);
                    }
                }
            }
        }
        crate::cursor_events::end_tool(cursor_event);
        // Coordinate the physical action itself, not post-action evidence
        // capture or result shaping. Keeping the global desktop lock through
        // recording/PiP screenshots would unnecessarily block an unrelated
        // runtime after the input side effect has already completed.
        drop(_desktop_action);
        restore_public_runtime_result(&mut result, &runtime_prefix);
        // Preserve the producer's private summary for recording/replay before
        // the public ActionResult projection deliberately replaces legacy
        // prose. Coordinate recovery and other internal diagnostics must not
        // depend on the narrowed MCP text surface.
        let recording_result_text = result.content.iter().find_map(|content| {
            if let Content::Text { text, .. } = content {
                Some(text.clone())
            } else {
                None
            }
        });
        if result.is_error != Some(true) && crate::action_record::is_action_tool(resolved_name) {
            if let Err(error) = publish_action_result(&mut result) {
                result = ToolResult::error(format!(
                    "internal action outcome mismatch for {resolved_name}: {error}"
                ))
                .with_structured(serde_json::json!({
                    "code": "action_outcome_mismatch",
                    "tool": resolved_name,
                    "detail": error,
                }));
            }
        }
        if result.is_error != Some(true) {
            if let Some(structured) = result.structured_content.clone() {
                if let Err(error) =
                    cua_driver_contract::validate_success_output(resolved_name, structured)
                {
                    result = ToolResult::error(format!(
                        "internal typed output mismatch for {resolved_name}: {error}"
                    ))
                    .with_structured(serde_json::json!({
                        "code": "typed_output_mismatch",
                        "tool": resolved_name,
                        "detail": error,
                    }));
                }
            }
        }
        // Use the original name for downstream code paths below so the
        // exit-code matching and recording paths keep treating the alias
        // as a distinct call site.
        let name = resolved_name;

        if let (Some(history), Some(pending)) = (self.history.as_ref(), pending_history) {
            history.finish_action(
                pending,
                result.action_record.as_ref(),
                result.is_error == Some(true),
            );
        }
        if result.is_error != Some(true) && matches!(name, "start_session" | "end_session") {
            self.history.as_ref().map(|history| {
                history.session_event(
                    public_args
                        .get("session")
                        .and_then(Value::as_str)
                        .or(runtime_session.as_deref()),
                    name == "start_session",
                )
            });
        }

        // Record non-read-only, non-recording tool calls. The recording-
        // control tools themselves are excluded so the recorded turn
        // stream stays the actual user-action sequence (not the meta
        // start/stop frames).
        if let Some(pending_turn) = pending_turn {
            self.recording.finish_turn_with_outcome(
                pending_turn,
                recording_result_text.as_deref().unwrap_or(""),
                result.action_record.as_ref(),
                result.is_error == Some(true),
            );
        }

        // Experimental PiP push — only when --experimental-pip is on argv
        // (otherwise `pip_enabled()` is false and we skip the screenshot
        // entirely to avoid wasted capture work). We push for the same set
        // of action tools the recording pipeline cares about (non-read-only,
        // not the recording-control meta-tools) so the live view matches
        // what the recorder would have captured for the turn.
        if pip_hook::pip_enabled() && should_record && !private_consent_turn {
            let window_id = args.opt_u64("window_id");
            let pid = args.opt_i64("pid");
            if let Some(png_bytes) = screenshot_for(window_id, pid) {
                let label = synthesize_action_label(name, &public_args);
                pip_hook::push_pip_frame(pip_hook::PipHookFrame {
                    png_bytes,
                    action_label: label,
                    timestamp_ms: now_ms(),
                });
            }
        }

        result
    }

    async fn authorize_private_observation(
        &self,
        tool: &dyn Tool,
        tool_name: &str,
        args: &Value,
        context: &crate::session_authorization::EffectiveAuthorizationContext,
        lifecycle_session: Option<&str>,
        adapter_id: &str,
    ) -> Result<(), crate::consent::ConsentError> {
        if context.mode() == crate::authorization::PermissionMode::Unrestricted
            && context.capability_manifest().is_none()
        {
            return Ok(());
        }
        let browser_target = args.get("target_id").and_then(Value::as_str);
        let browser_tab = args.get("tab_id").and_then(Value::as_str);
        if adapter_id == "private_observation"
            && context.mode() == crate::authorization::PermissionMode::Standard
            && context.capability_manifest().is_none()
        {
            // Standard observation is promptless, but browser observations
            // still have to attest their live target before dispatch.
            if browser_target.is_some()
                && tool
                    .protected_resource_scope("private_observation", args)
                    .await
                    .map_err(crate::consent::ConsentError::Provider)?
                    .is_none()
            {
                return Err(crate::consent::ConsentError::Provider(
                    "browser observation did not attest a live top-level origin".to_owned(),
                ));
            }
            // Whole-display scope discovery is X11-specific on Linux. It is
            // not a grant boundary in standard mode and must not turn routine
            // promptless operation into a pure-Wayland failure.
            return Ok(());
        }
        let history_resource = history_observation_resource(tool_name);
        let browser_scope = if history_resource.is_none() {
            tool.protected_resource_scope("private_observation", args)
                .await
                .map_err(crate::consent::ConsentError::Provider)?
        } else {
            None
        };
        let (mut resource, summary) = if let Some((resource, summary)) = history_resource {
            (resource, summary.to_owned())
        } else if let Some(resource) = browser_scope {
            let target_id = browser_target.unwrap_or("unknown");
            (
                resource,
                match browser_tab {
                    Some(tab_id) => {
                        format!("Allow Cua to observe browser target {target_id}, tab {tab_id}")
                    }
                    None => format!("Allow Cua to observe browser target {target_id}"),
                },
            )
        } else if browser_target.is_some() {
            return Err(crate::consent::ConsentError::Provider(
                "browser observation did not attest a live top-level origin".to_owned(),
            ));
        } else if tool_name == "escalate_session" {
            (
                serde_json::json!({
                    "kind": "session_capture_scope",
                    "capture_scope": "desktop",
                }),
                "Allow Cua to expand this session's observation scope".to_owned(),
            )
        } else if let Some(window_id) = args.get("window_id").and_then(Value::as_u64) {
            let pid = args.get("pid").and_then(Value::as_i64);
            (
                serde_json::json!({
                    "kind": "window",
                    "pid": pid,
                    "window_id": window_id,
                }),
                match pid {
                    Some(pid) => {
                        format!("Allow Cua to observe window {window_id} of process {pid}")
                    }
                    None => format!("Allow Cua to observe window {window_id}"),
                },
            )
        } else if let Some(pid) = args.get("pid").and_then(Value::as_i64) {
            (
                serde_json::json!({
                    "kind": "application",
                    "pid": pid,
                }),
                format!("Allow Cua to observe process {pid}"),
            )
        } else if tool_name == "start_recording"
            && args.get("record_video").and_then(Value::as_bool) != Some(true)
        {
            // A trajectory-only recording does not capture the display when it
            // starts. Each later action still passes its own observation/input
            // adapters, and the grant key is bound to this runtime-private
            // lifecycle session. Requiring display discovery here would make
            // this safe, headless-capable mode unusable on Linux CI.
            (
                serde_json::json!({
                    "kind": "session_trajectory_recording",
                    "record_video": false,
                }),
                "Allow Cua to record separately authorized actions in this session".to_owned(),
            )
        } else {
            // Whole-desktop and unfiltered listing calls are scoped to the
            // current display geometry. Reading get_screen_size is R0 and
            // content-free; calling its platform implementation directly
            // avoids recursive authorization while making a display topology
            // change produce a different grant digest.
            let display = self
                .tools
                .get("get_screen_size")
                .ok_or_else(|| {
                    crate::consent::ConsentError::Provider(
                        "display identity is unavailable".to_owned(),
                    )
                })?
                .invoke(serde_json::json!({}))
                .await;
            if display.is_error == Some(true) {
                return Err(crate::consent::ConsentError::Provider(
                    "display identity could not be read".to_owned(),
                ));
            }
            let display = display.structured_content.ok_or_else(|| {
                crate::consent::ConsentError::Provider(
                    "display identity was not returned".to_owned(),
                )
            })?;
            (
                serde_json::json!({
                    "kind": "display",
                    "width": display.get("width").and_then(Value::as_u64),
                    "height": display.get("height").and_then(Value::as_u64),
                    "scale_factor": display.get("scale_factor").and_then(Value::as_f64),
                }),
                "Allow Cua to observe the current desktop".to_owned(),
            )
        };

        if context.capability_manifest().is_some() {
            if let Some(pid) = resource.get("pid").and_then(Value::as_i64) {
                self.enrich_application_resource(&mut resource, pid).await;
            }
        }
        self.protected_resource_grants
            .authorize(
                context,
                adapter_id,
                crate::authorization::RiskClass::R2,
                lifecycle_session,
                resource,
                &format!("{summary} using {tool_name}"),
                Duration::from_secs(30 * 60),
                Duration::from_secs(8 * 60 * 60),
            )
            .await?;
        Ok(())
    }

    async fn authorize_desktop_input(
        &self,
        tool_name: &str,
        args: &Value,
        context: &crate::session_authorization::EffectiveAuthorizationContext,
        lifecycle_session: Option<&str>,
    ) -> Result<(), crate::consent::ConsentError> {
        if context.mode() != crate::authorization::PermissionMode::Bounded
            && context.capability_manifest().is_none()
        {
            // Standard and unrestricted input do not need a protected grant.
            // Bounded mode continues below so the exact resource can be
            // checked against its approved manifest.
            return Ok(());
        }
        let delivery_mode = if tool_name == "bring_to_front" {
            "foreground"
        } else {
            args.get("delivery_mode")
                .and_then(Value::as_str)
                .unwrap_or("background")
        };
        let (mut resource, summary) = if let Some(window_id) =
            args.get("window_id").and_then(Value::as_u64)
        {
            let pid = args.get("pid").and_then(Value::as_i64);
            (
                    serde_json::json!({
                        "kind": "window_input",
                        "pid": pid,
                        "window_id": window_id,
                        "delivery_mode_ceiling": delivery_mode,
                    }),
                    match pid {
                        Some(pid) => format!(
                            "Allow Cua to control window {window_id} of process {pid} in {delivery_mode} mode"
                        ),
                        None => format!(
                            "Allow Cua to control window {window_id} in {delivery_mode} mode"
                        ),
                    },
                )
        } else if let Some(pid) = args.get("pid").and_then(Value::as_i64) {
            (
                serde_json::json!({
                    "kind": "application_input",
                    "pid": pid,
                    "delivery_mode_ceiling": delivery_mode,
                }),
                format!("Allow Cua to control process {pid} in {delivery_mode} mode"),
            )
        } else {
            let display = self
                .tools
                .get("get_screen_size")
                .ok_or_else(|| {
                    crate::consent::ConsentError::Provider(
                        "display identity is unavailable".to_owned(),
                    )
                })?
                .invoke(serde_json::json!({}))
                .await;
            if display.is_error == Some(true) {
                return Err(crate::consent::ConsentError::Provider(
                    "display identity could not be read".to_owned(),
                ));
            }
            let display = display.structured_content.ok_or_else(|| {
                crate::consent::ConsentError::Provider(
                    "display identity was not returned".to_owned(),
                )
            })?;
            (
                serde_json::json!({
                    "kind": "display_input",
                    "width": display.get("width").and_then(Value::as_u64),
                    "height": display.get("height").and_then(Value::as_u64),
                    "scale_factor": display.get("scale_factor").and_then(Value::as_f64),
                    "delivery_mode_ceiling": delivery_mode,
                }),
                format!("Allow Cua to control the current desktop in {delivery_mode} mode"),
            )
        };

        if context.capability_manifest().is_some() {
            if let Some(pid) = resource.get("pid").and_then(Value::as_i64) {
                self.enrich_application_resource(&mut resource, pid).await;
            }
        }
        self.protected_resource_grants
            .authorize(
                context,
                "desktop_input",
                crate::authorization::RiskClass::R1,
                lifecycle_session,
                resource,
                &format!("{summary} using {tool_name}"),
                Duration::from_secs(30 * 60),
                Duration::from_secs(8 * 60 * 60),
            )
            .await?;
        Ok(())
    }

    async fn authorize_clipboard(
        &self,
        tool_name: &str,
        args: &Value,
        context: &crate::session_authorization::EffectiveAuthorizationContext,
        lifecycle_session: Option<&str>,
    ) -> Result<(), crate::consent::ConsentError> {
        if context.mode() != crate::authorization::PermissionMode::Bounded
            && context.capability_manifest().is_none()
        {
            return Ok(());
        }
        let (operation, content_kind, summary) = if tool_name == "clipboard_read" {
            (
                "read",
                if args.get("include_text").and_then(Value::as_bool) == Some(true) {
                    "text_and_types"
                } else {
                    "types"
                },
                "Allow Cua to read the current system clipboard",
            )
        } else {
            let kind = if args.get("text").and_then(Value::as_str).is_some() {
                "text"
            } else if args.get("image_path").and_then(Value::as_str).is_some() {
                "image"
            } else {
                "file_url"
            };
            (
                "write",
                kind,
                "Allow Cua to replace the current system clipboard",
            )
        };
        self.protected_resource_grants
            .authorize(
                context,
                "clipboard",
                crate::authorization::RiskClass::R2,
                lifecycle_session,
                serde_json::json!({
                    "kind": "system_clipboard",
                    "operation": operation,
                    "content_kind": content_kind,
                }),
                &format!("{summary} using {tool_name}"),
                Duration::from_secs(30 * 60),
                Duration::from_secs(8 * 60 * 60),
            )
            .await
            .map(|_| ())
    }

    async fn authorize_file_transfer(
        &self,
        tool_name: &str,
        args: &mut Value,
        context: &crate::session_authorization::EffectiveAuthorizationContext,
        lifecycle_session: Option<&str>,
    ) -> Result<(), ToolResult> {
        if context.mode() == crate::authorization::PermissionMode::Unrestricted
            && context.capability_manifest().is_none()
        {
            return Ok(());
        }
        let (resource, summary) = match tool_name {
            "browser_set_input_files" => {
                let files = canonical_upload_files(args)?;
                let count = files.len();
                args["files"] = serde_json::json!(files);
                (
                    serde_json::json!({
                        "kind": "browser_upload",
                        "target_id": args.get("target_id").and_then(Value::as_str),
                        "tab_id": args.get("tab_id").and_then(Value::as_str),
                        "ref": args.get("ref").and_then(Value::as_str),
                        "direction": "local_to_browser",
                        "canonical_paths": files,
                    }),
                    format!("Allow Cua to upload {count} exact local file(s) to this browser tab"),
                )
            }
            "browser_download" => {
                let destination =
                    canonical_existing_directory(required_path_arg(args, "destination_root")?)?;
                args["destination_root"] = Value::String(destination.clone());
                (
                    serde_json::json!({
                        "kind": "browser_download",
                        "target_id": args.get("target_id").and_then(Value::as_str),
                        "tab_id": args.get("tab_id").and_then(Value::as_str),
                        "ref": args.get("ref").and_then(Value::as_str),
                        "direction": "browser_to_local",
                        "canonical_destination_root": destination,
                    }),
                    "Allow Cua to download one file from this browser tab to the exact destination directory".to_owned(),
                )
            }
            "clipboard_write" => {
                let (argument, content_kind) =
                    if args.get("image_path").and_then(Value::as_str).is_some() {
                        ("image_path", "image")
                    } else {
                        ("file_path", "file_url")
                    };
                let source = canonical_existing_file(required_path_arg(args, argument)?)?;
                args[argument] = Value::String(source.clone());
                (
                    serde_json::json!({
                        "kind": "clipboard_local_file",
                        "content_kind": content_kind,
                        "direction": "local_to_clipboard",
                        "canonical_source_path": source,
                    }),
                    format!("Allow Cua to read this exact local {content_kind} into the clipboard"),
                )
            }
            "get_desktop_state" | "get_window_state" => {
                let output =
                    canonical_proposed_path(required_path_arg(args, "screenshot_out_file")?)?;
                args["screenshot_out_file"] = Value::String(output.clone());
                (
                    serde_json::json!({
                        "kind": "screenshot_output",
                        "tool": tool_name,
                        "pid": args.get("pid").and_then(Value::as_i64),
                        "window_id": args.get("window_id").and_then(Value::as_u64),
                        "direction": "driver_to_local",
                        "canonical_output_path": output,
                    }),
                    "Allow Cua to write this screenshot to the exact local path".to_owned(),
                )
            }
            "start_recording" => {
                let output = canonical_proposed_path(required_path_arg(args, "output_dir")?)?;
                args["output_dir"] = Value::String(output.clone());
                (
                    serde_json::json!({
                        "kind": "recording_output",
                        "direction": "driver_to_local",
                        "canonical_output_directory": output,
                        "record_video": args.get("record_video").and_then(Value::as_bool).unwrap_or(false),
                    }),
                    "Allow Cua to write trajectory evidence to the exact local directory"
                        .to_owned(),
                )
            }
            "stop_recording" => {
                let state = self.recording.current_state();
                let Some(output_dir) = state.output_dir else {
                    // Stopping an already-stopped recorder is a side-effect-free
                    // no-op and must not manufacture an approval prompt.
                    return Ok(());
                };
                let output = canonical_proposed_path(&output_dir)?;
                (
                    serde_json::json!({
                        "kind": "recording_finalize",
                        "direction": "driver_to_local",
                        "canonical_output_directory": output,
                    }),
                    "Allow Cua to finalize the active recording in its exact local directory"
                        .to_owned(),
                )
            }
            "replay_trajectory" => {
                let source = canonical_existing_directory(required_path_arg(args, "dir")?)?;
                args["dir"] = Value::String(source.clone());
                (
                    serde_json::json!({
                        "kind": "trajectory_replay",
                        "direction": "local_to_driver",
                        "canonical_source_directory": source,
                    }),
                    "Allow Cua to read and replay actions from the exact trajectory directory"
                        .to_owned(),
                )
            }
            "install_ffmpeg" if args.get("confirm").and_then(Value::as_bool) == Some(true) => (
                serde_json::json!({
                    "kind": "dependency_install",
                    "dependency": "ffmpeg",
                    "direction": "network_to_system",
                }),
                "Allow Cua to install ffmpeg using the detected system package manager".to_owned(),
            ),
            _ => {
                return Err(protected_scope_refusal(
                    "the file-transfer operation has no reviewed exact scope",
                ))
            }
        };

        self.protected_resource_grants
            .authorize(
                context,
                "file_transfer_and_output",
                crate::authorization::RiskClass::R3,
                lifecycle_session,
                resource,
                &format!("{summary} using {tool_name}"),
                Duration::from_secs(30 * 60),
                Duration::from_secs(8 * 60 * 60),
            )
            .await
            .map_err(protected_consent_refusal)?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn authorize_attested_resource(
        &self,
        tool: &dyn Tool,
        adapter_id: &str,
        risk_class: crate::authorization::RiskClass,
        args: &Value,
        context: &crate::session_authorization::EffectiveAuthorizationContext,
        lifecycle_session: Option<&str>,
        human_summary: &str,
        idle_ttl: Duration,
        absolute_ttl: Duration,
    ) -> Result<Value, ToolResult> {
        if context.mode() == crate::authorization::PermissionMode::Unrestricted
            && context.capability_manifest().is_none()
        {
            return Ok(Value::Null);
        }
        let mut resource = tool
            .protected_resource_scope(adapter_id, args)
            .await
            .map_err(|message| protected_scope_refusal(&message))?
            .ok_or_else(|| {
                protected_scope_refusal(
                    "the concrete tool cannot attest an exact protected resource scope",
                )
            })?;
        if adapter_id == "process_control" {
            let driver_owned = lifecycle_session
                .zip(resource.pointer("/fingerprint/pid").and_then(Value::as_i64))
                .is_some_and(|(session, pid)| {
                    self.protected_resource_ownership
                        .is_driver_owned_pid(session, pid)
                });
            resource["driver_owned"] = Value::Bool(driver_owned);
        }
        self.protected_resource_grants
            .authorize(
                context,
                adapter_id,
                risk_class,
                lifecycle_session,
                resource.clone(),
                human_summary,
                idle_ttl,
                absolute_ttl,
            )
            .await
            .map_err(protected_consent_refusal)?;
        Ok(resource)
    }

    async fn validate_attested_resource(
        &self,
        tool: &dyn Tool,
        adapter_id: &str,
        args: &Value,
        context: &crate::session_authorization::EffectiveAuthorizationContext,
        lifecycle_session: Option<&str>,
        approved_scope: &Value,
    ) -> Result<(), ToolResult> {
        if context.mode() == crate::authorization::PermissionMode::Unrestricted
            && context.capability_manifest().is_none()
        {
            return Ok(());
        }
        let mut validation_scope = approved_scope.clone();
        if adapter_id == "process_control" {
            if let Some(scope) = validation_scope.as_object_mut() {
                scope.remove("driver_owned");
            }
        }
        if let Err(message) = tool
            .validate_protected_resource_scope(adapter_id, args, &validation_scope)
            .await
        {
            if let Some(grant) = self.protected_resource_grants.revoke_resource(
                context,
                lifecycle_session,
                adapter_id,
                approved_scope,
            ) {
                self.approval_broker.revoke(&grant).await;
            }
            return Err(protected_refusal(
                "protected_resource_scope_stale",
                &message,
            ));
        }
        Ok(())
    }

    async fn authorize_driver_configuration(
        &self,
        tool: &dyn Tool,
        args: &Value,
        context: &crate::session_authorization::EffectiveAuthorizationContext,
        lifecycle_session: Option<&str>,
    ) -> Result<(), ToolResult> {
        if args.get("capture_scope").is_some()
            || args.get("key").and_then(Value::as_str) == Some("capture_scope")
        {
            return Err(
                ToolResult::error(
                    "config key 'capture_scope' is retired; select a window or desktop target on each action",
                )
                .with_structured(serde_json::json!({
                    "code": "config_key_retired",
                    "key": "capture_scope",
                    "replacement": "action.target",
                })),
            );
        }
        if context.mode() == crate::authorization::PermissionMode::Unrestricted
            && context.capability_manifest().is_none()
        {
            return Ok(());
        }
        let properties = tool
            .def()
            .input_schema
            .get("properties")
            .and_then(Value::as_object)
            .ok_or_else(|| protected_scope_refusal("set_config has no reviewable input schema"))?;
        let object = args
            .as_object()
            .ok_or_else(|| protected_scope_refusal("set_config arguments must be an object"))?;
        for key in object.keys() {
            if key == "session" || key.starts_with('_') {
                continue;
            }
            if !properties.contains_key(key) {
                return Err(protected_scope_refusal(&format!(
                    "set_config field '{key}' is not present in the concrete tool schema"
                )));
            }
        }

        let mut exact = serde_json::Map::new();
        if let Some(key) = args.get("key").and_then(Value::as_str) {
            if matches!(key, "key" | "value" | "session" | "_session_id")
                || !properties.contains_key(key)
            {
                return Err(protected_scope_refusal(&format!(
                    "set_config key '{key}' is not present in the concrete tool schema"
                )));
            }
            let value = args
                .get("value")
                .ok_or_else(|| protected_scope_refusal("set_config key requires an exact value"))?;
            exact.insert(key.to_owned(), value.clone());
        } else if args.get("value").is_some() {
            return Err(protected_scope_refusal(
                "set_config value requires an exact key",
            ));
        }
        for (key, value) in object {
            if matches!(key.as_str(), "session" | "key" | "value") || key.starts_with('_') {
                continue;
            }
            exact.insert(key.clone(), value.clone());
        }
        if exact.is_empty() {
            return Err(protected_scope_refusal(
                "set_config did not contain a reviewed configuration field",
            ));
        }
        let keys = exact.keys().cloned().collect::<Vec<_>>().join(", ");
        self.protected_resource_grants
            .authorize(
                context,
                "driver_configuration",
                crate::authorization::RiskClass::R2,
                lifecycle_session,
                serde_json::json!({
                    "kind": "driver_configuration",
                    "exact_changes": exact,
                }),
                &format!("Allow Cua to change exact driver configuration field(s): {keys}"),
                Duration::from_secs(5 * 60),
                Duration::from_secs(30 * 60),
            )
            .await
            .map_err(protected_consent_refusal)?;
        Ok(())
    }
}

fn history_observation_resource(tool_name: &str) -> Option<(Value, &'static str)> {
    match tool_name {
        "history_status" => Some((
            serde_json::json!({"kind": "computer_history", "operation": "status"}),
            "Allow Cua to inspect Computer History status",
        )),
        "history_query" => Some((
            serde_json::json!({"kind": "computer_history", "operation": "query"}),
            "Allow Cua to read encrypted Computer History metadata",
        )),
        _ => None,
    }
}

#[cfg(test)]
mod history_observation_tests {
    use super::*;

    #[test]
    fn history_grants_are_display_independent_and_operation_specific() {
        let (status, _) = history_observation_resource("history_status").unwrap();
        let (query, _) = history_observation_resource("history_query").unwrap();
        assert_eq!(
            status,
            serde_json::json!({"kind":"computer_history","operation":"status"})
        );
        assert_eq!(
            query,
            serde_json::json!({"kind":"computer_history","operation":"query"})
        );
        assert_ne!(status, query);
        for resource in [status, query] {
            assert!(resource.get("width").is_none());
            assert!(resource.get("pid").is_none());
            assert!(resource.get("session").is_none());
            assert!(resource.get("since_sequence").is_none());
        }
    }
}

fn required_path_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str, ToolResult> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| protected_scope_refusal(&format!("missing exact path field `{key}`")))
}

fn expanded_path(raw: &str) -> Result<PathBuf, ToolResult> {
    let path = if raw == "~" {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| protected_scope_refusal("the home directory is unavailable"))?
    } else if let Some(relative) = raw.strip_prefix("~/") {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| protected_scope_refusal("the home directory is unavailable"))?
            .join(relative)
    } else {
        PathBuf::from(raw)
    };
    if path.is_absolute() {
        Ok(path)
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .map_err(|_| protected_scope_refusal("the current directory is unavailable"))
    }
}

fn canonical_existing_directory(raw: &str) -> Result<String, ToolResult> {
    let path = expanded_path(raw)?;
    let metadata = std::fs::symlink_metadata(&path)
        .map_err(|_| protected_scope_refusal("the exact directory does not exist"))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(protected_scope_refusal(
            "the exact path must name a directory directly, not a link or file",
        ));
    }
    std::fs::canonicalize(path)
        .map(|path| path.to_string_lossy().into_owned())
        .map_err(|_| protected_scope_refusal("the exact directory could not be canonicalized"))
}

fn canonical_upload_files(args: &Value) -> Result<Vec<String>, ToolResult> {
    let files = args
        .get("files")
        .and_then(Value::as_array)
        .filter(|files| !files.is_empty() && files.len() <= 32)
        .ok_or_else(|| protected_scope_refusal("files must contain between 1 and 32 paths"))?;
    files
        .iter()
        .map(|value| {
            let raw = value
                .as_str()
                .ok_or_else(|| protected_scope_refusal("every upload path must be a string"))?;
            let path = Path::new(raw);
            if !path.is_absolute() {
                return Err(protected_scope_refusal(
                    "every upload path must be absolute",
                ));
            }
            let metadata = std::fs::symlink_metadata(path)
                .map_err(|_| protected_scope_refusal("an exact upload path does not exist"))?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(protected_scope_refusal(
                    "upload paths must name regular files directly, not links or directories",
                ));
            }
            std::fs::canonicalize(path)
                .map(|path| path.to_string_lossy().into_owned())
                .map_err(|_| protected_scope_refusal("an upload path could not be canonicalized"))
        })
        .collect()
}

fn canonical_existing_file(raw: &str) -> Result<String, ToolResult> {
    let path = Path::new(raw);
    if !path.is_absolute() {
        return Err(protected_scope_refusal(
            "clipboard file paths must be absolute",
        ));
    }
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|_| protected_scope_refusal("the exact clipboard path does not exist"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(protected_scope_refusal(
            "clipboard paths must name regular files directly, not links or directories",
        ));
    }
    std::fs::canonicalize(path)
        .map(|path| path.to_string_lossy().into_owned())
        .map_err(|_| protected_scope_refusal("the clipboard path could not be canonicalized"))
}

/// Resolve a not-yet-created output without creating it before approval.
///
/// The deepest existing ancestor is canonicalized first, so symlinked parents
/// are captured in the approved identity. Only normal path components may be
/// appended after that ancestor; lexical parent traversal never enters a
/// protected-resource digest.
fn canonical_proposed_path(raw: &str) -> Result<String, ToolResult> {
    let path = expanded_path(raw)?;
    if path.exists() {
        return std::fs::canonicalize(path)
            .map(|path| path.to_string_lossy().into_owned())
            .map_err(|_| protected_scope_refusal("the exact path could not be canonicalized"));
    }

    let mut existing = path.as_path();
    let mut suffix = Vec::new();
    while !existing.exists() {
        match std::fs::symlink_metadata(existing) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(protected_scope_refusal(
                    "the output path contains a broken symbolic link",
                ))
            }
            Ok(_) => {
                return Err(protected_scope_refusal(
                    "the output path contains an unavailable filesystem entry",
                ))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {
                return Err(protected_scope_refusal(
                    "the output path could not be inspected safely",
                ))
            }
        }
        let name = existing
            .file_name()
            .ok_or_else(|| protected_scope_refusal("the output path has no existing ancestor"))?;
        suffix.push(name.to_os_string());
        existing = existing
            .parent()
            .ok_or_else(|| protected_scope_refusal("the output path has no existing ancestor"))?;
    }
    let metadata = std::fs::symlink_metadata(existing)
        .map_err(|_| protected_scope_refusal("the output ancestor is unavailable"))?;
    if !metadata.is_dir() {
        return Err(protected_scope_refusal(
            "the output path's existing ancestor is not a directory",
        ));
    }
    let mut canonical = std::fs::canonicalize(existing)
        .map_err(|_| protected_scope_refusal("the output ancestor could not be canonicalized"))?;
    for component in suffix.into_iter().rev() {
        let component_path = Path::new(&component);
        if !matches!(
            component_path.components().next(),
            Some(Component::Normal(_))
        ) || component_path.components().count() != 1
        {
            return Err(protected_scope_refusal(
                "the output path contains an unsafe component",
            ));
        }
        canonical.push(component);
    }
    Ok(canonical.to_string_lossy().into_owned())
}

fn is_physical_desktop_action(tool: &str) -> bool {
    matches!(
        tool,
        "click"
            | "double_click"
            | "right_click"
            | "scroll"
            | "drag"
            | "mouse_drag"
            | "parallel_mouse_drag"
            | "move_cursor"
            | "mouse_button_down"
            | "mouse_button_up"
            | "type_text"
            | "press_key"
            | "hotkey"
            | "set_value"
            | "bring_to_front"
            | "set_window_frame"
    )
}

/// Bucket that owns the processes a call is allowed to terminate.
///
/// A call that declares a session keys its launches to that session, so
/// concurrent agents sharing a runtime cannot kill each other's processes. A
/// sessionless call carries no such identity: bucketing its launches per
/// runtime scope keeps exactly the guarantee the refusal states — "launched
/// by this Cua runtime" — instead of recording no ownership at all, which
/// left a sessionless `launch_app` followed by `kill_app` permanently
/// refused in standard mode.
///
/// The bucket is namespaced like a session but named with a NUL, which no
/// namespaced public session label contains, so it cannot be selected by
/// passing a crafted `session` argument. Even a collision would only reach
/// processes this same runtime scope launched.
fn process_ownership_key(runtime_session: Option<&str>, runtime_prefix: &str) -> String {
    match runtime_session {
        Some(session) => session.to_owned(),
        None => format!("{runtime_prefix}\u{0}anonymous-launch"),
    }
}

fn namespace_runtime_args(
    args: &mut Value,
    context: &crate::session_authorization::EffectiveAuthorizationContext,
    evidence: &TrustedInvocationEvidence,
) -> String {
    let runtime_prefix = format!("__cua_runtime_{}:", context.runtime_scope_key());
    if !args.is_object() {
        return runtime_prefix;
    }
    evidence.apply_runtime_args(args);
    let Some(arguments) = args.as_object_mut() else {
        return runtime_prefix;
    };
    if let Some(public_session) = arguments
        .get("session")
        .and_then(Value::as_str)
        .filter(|value| {
            !value.is_empty() && *value != "default" && !value.starts_with("__cua_runtime_")
        })
        .map(str::to_owned)
    {
        arguments.insert(
            "_public_session_label".to_owned(),
            Value::String(public_session),
        );
    }
    if !arguments.contains_key("_transport_session_id") {
        if let Some(session) = context.transport_session() {
            arguments.insert(
                "_transport_session_id".to_owned(),
                Value::String(session.to_owned()),
            );
        }
    }
    if let Some(idle_ttl) = context.lifecycle_idle_ttl_override() {
        let idle_ttl_ms = idle_ttl.as_millis().min(u64::MAX as u128) as u64;
        arguments.insert(
            "_session_idle_ttl_ms".to_owned(),
            Value::Number(idle_ttl_ms.into()),
        );
    }
    // The registry is the first boundary shared by every transport and the
    // direct SDK. Transport adapters may already have injected `_session_id`,
    // but a direct runtime call only carries the public `session` field. Mint
    // the trusted ownership key here, after authorization, so recording and
    // every other session-owned resource use the same runtime-private identity
    // regardless of which adapter invoked the registry.
    if !arguments.contains_key("_session_id") {
        if let Some(session) = arguments
            .get("session")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
        {
            arguments.insert("_session_id".to_owned(), Value::String(session));
        }
    }
    for key in [
        "session",
        "_session_id",
        "_transport_session_id",
        "cursor_id",
    ] {
        let Some(public) = arguments
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty() && *value != "default")
            .map(str::to_owned)
        else {
            continue;
        };
        let internal = if public.starts_with(&runtime_prefix) {
            public
        } else {
            format!("{runtime_prefix}{public}")
        };
        arguments.insert(key.to_owned(), Value::String(internal));
    }
    runtime_prefix
}

fn session_requiring_tool(tool_name: &str) -> bool {
    !matches!(
        tool_name,
        "check_permissions"
            | "health_report"
            | "get_session"
            | "list_sessions"
            | "get_session_state"
            | "end_session"
    )
}

fn session_selecting_tool(tool_name: &str) -> bool {
    session_requiring_tool(tool_name)
        || matches!(
            tool_name,
            "get_session" | "list_sessions" | "get_session_state" | "end_session"
        )
}

fn publish_action_result(result: &mut ToolResult) -> Result<(), String> {
    let action = result
        .action_record
        .as_ref()
        .ok_or_else(|| "successful action omitted its internal execution record".to_owned())?;
    let public = action
        .public_result()
        .map_err(|error| format!("invalid internal execution record: {error:?}"))?;
    public
        .validate_invariants()
        .map_err(|error| format!("invalid public projection: {error}"))?;
    let structured =
        serde_json::to_value(public).map_err(|error| format!("projection failed: {error}"))?;
    // The breaking contract narrows machine-readable structured content. Keep
    // the outer ToolResult text/images intact for human diagnostics, refusal
    // messages, recording/replay, and clients that intentionally degrade to
    // raw content.
    result.structured_content = Some(structured);
    Ok(())
}

fn restore_public_runtime_result(result: &mut ToolResult, runtime_prefix: &str) {
    for content in &mut result.content {
        if let Content::Text { text, .. } = content {
            *text = text.replace(runtime_prefix, "");
        }
    }
    let mut key_collision = false;
    if let Some(structured) = result.structured_content.as_mut() {
        key_collision = restore_public_runtime_value(structured, runtime_prefix);
    }
    if key_collision {
        *result = ToolResult::error(
            "internal runtime namespace restoration produced a duplicate output key",
        )
        .with_structured(serde_json::json!({
            "code": "runtime_output_key_collision",
        }));
    }
}

fn restore_public_runtime_value(value: &mut Value, runtime_prefix: &str) -> bool {
    match value {
        Value::String(text) => {
            *text = text.replace(runtime_prefix, "");
            false
        }
        Value::Array(values) => {
            let mut collision = false;
            for value in values {
                collision |= restore_public_runtime_value(value, runtime_prefix);
            }
            collision
        }
        Value::Object(values) => {
            let original = std::mem::take(values);
            let mut collision = false;
            for (key, mut value) in original {
                collision |= restore_public_runtime_value(&mut value, runtime_prefix);
                collision |= values
                    .insert(key.replace(runtime_prefix, ""), value)
                    .is_some();
            }
            collision
        }
        _ => false,
    }
}

#[cfg(test)]
mod runtime_isolation_tests {
    use super::{
        canonical_proposed_path, desktop_action_coordinator, namespace_runtime_args,
        publish_action_result, restore_public_runtime_result, try_admit_text_input,
        TrustedInvocationEvidence, DISPATCH_RUNTIME_SCOPE,
    };
    use crate::{
        authorization::PermissionMode,
        consent::{ConsentAction, ConsentRequest, ProtectedConsentProvider, ProviderDecision},
        protocol::ToolResult,
        session_authorization::{SessionAuthorizationRegistry, SessionModeCeiling},
    };
    use std::io::Write;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    };
    use std::time::Duration;

    fn standard_context() -> Arc<crate::session_authorization::EffectiveAuthorizationContext> {
        let ceiling = SessionModeCeiling::for_trusted_sessions(
            [PermissionMode::Standard],
            false,
            Duration::from_secs(60),
            Duration::from_secs(30),
        )
        .unwrap();
        SessionAuthorizationRegistry::with_ceiling(ceiling)
            .compatibility_context(PermissionMode::Standard, None)
            .unwrap()
    }

    fn unrestricted_context() -> Arc<crate::session_authorization::EffectiveAuthorizationContext> {
        let ceiling = SessionModeCeiling::for_trusted_sessions(
            [PermissionMode::Unrestricted],
            true,
            Duration::from_secs(60),
            Duration::from_secs(30),
        )
        .unwrap();
        SessionAuthorizationRegistry::with_ceiling(ceiling)
            .compatibility_context(PermissionMode::Unrestricted, None)
            .unwrap()
    }

    fn bounded_context(
        manifest_source: &str,
    ) -> Arc<crate::session_authorization::EffectiveAuthorizationContext> {
        manifest_context(PermissionMode::Bounded, manifest_source)
    }

    fn manifest_context(
        mode: PermissionMode,
        manifest_source: &str,
    ) -> Arc<crate::session_authorization::EffectiveAuthorizationContext> {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(manifest_source.as_bytes()).unwrap();
        let manifest = Arc::new(
            crate::session_manifest::load_manifest(file.path())
                .expect("test bounded manifest loads"),
        );
        let ceiling = SessionModeCeiling::for_trusted_sessions(
            [mode],
            mode == PermissionMode::Unrestricted,
            Duration::from_secs(60),
            Duration::from_secs(30),
        )
        .unwrap();
        SessionAuthorizationRegistry::with_ceiling(ceiling)
            .compatibility_context(mode, Some(manifest))
            .unwrap()
    }

    struct ReplayProbe {
        hits: Arc<AtomicUsize>,
        def: super::ToolDef,
    }

    struct ObservationProbe {
        hits: Arc<AtomicUsize>,
        def: super::ToolDef,
    }

    struct ArgumentProbe {
        hits: Arc<AtomicUsize>,
        last_args: Arc<Mutex<Option<serde_json::Value>>>,
        def: super::ToolDef,
    }

    struct AttestedProbe {
        hits: Arc<AtomicUsize>,
        scope: serde_json::Value,
        stale_before_dispatch: bool,
        def: super::ToolDef,
    }

    #[async_trait::async_trait]
    impl super::Tool for AttestedProbe {
        fn def(&self) -> &super::ToolDef {
            &self.def
        }

        async fn protected_resource_scope(
            &self,
            _adapter_id: &str,
            _args: &serde_json::Value,
        ) -> Result<Option<serde_json::Value>, String> {
            Ok(Some(self.scope.clone()))
        }

        async fn validate_protected_resource_scope(
            &self,
            _adapter_id: &str,
            _args: &serde_json::Value,
            approved_scope: &serde_json::Value,
        ) -> Result<(), String> {
            if self.stale_before_dispatch {
                return Err("synthetic process identity changed".to_owned());
            }
            if *approved_scope == self.scope {
                Ok(())
            } else {
                Err("synthetic scope mismatch".to_owned())
            }
        }

        async fn invoke(&self, _args: serde_json::Value) -> crate::protocol::ToolResult {
            self.hits.fetch_add(1, Ordering::SeqCst);
            crate::protocol::ToolResult::text("attested operation ran")
        }
    }

    #[async_trait::async_trait]
    impl super::Tool for ArgumentProbe {
        fn def(&self) -> &super::ToolDef {
            &self.def
        }

        async fn invoke(&self, args: serde_json::Value) -> crate::protocol::ToolResult {
            self.hits.fetch_add(1, Ordering::SeqCst);
            self.last_args.lock().unwrap().replace(args.clone());
            crate::protocol::ToolResult::text("file operation ran")
                .with_structured(serde_json::json!({"received": args}))
        }
    }

    #[async_trait::async_trait]
    impl super::Tool for ObservationProbe {
        fn def(&self) -> &super::ToolDef {
            &self.def
        }

        async fn invoke(&self, _args: serde_json::Value) -> crate::protocol::ToolResult {
            self.hits.fetch_add(1, Ordering::SeqCst);
            crate::protocol::ToolResult::text("private state")
                .with_structured(serde_json::json!({"snapshot_id": 1}))
        }
    }

    struct AcceptingProvider {
        requests: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl ProtectedConsentProvider for AcceptingProvider {
        fn provider_id(&self) -> &'static str {
            "test.observation-provider"
        }

        async fn request_consent(
            &self,
            request: &ConsentRequest,
        ) -> Result<ProviderDecision, String> {
            self.requests.fetch_add(1, Ordering::SeqCst);
            Ok(ProviderDecision {
                action: ConsentAction::Accept,
                request_digest: request.request_digest.clone(),
            })
        }
    }

    fn observation_registry(
        provider: Option<Arc<dyn ProtectedConsentProvider>>,
        hits: Arc<AtomicUsize>,
    ) -> Arc<super::ToolRegistry> {
        observation_registry_for("get_window_state", provider, hits)
    }

    fn observation_registry_for(
        name: &str,
        provider: Option<Arc<dyn ProtectedConsentProvider>>,
        hits: Arc<AtomicUsize>,
    ) -> Arc<super::ToolRegistry> {
        let mut registry = super::ToolRegistry::new_with_protected_consent_provider(provider);
        registry.register(Box::new(ObservationProbe {
            hits,
            def: super::ToolDef {
                name: name.to_owned(),
                description: "test observation".into(),
                input_schema: serde_json::json!({"type": "object"}),
                read_only: true,
                destructive: false,
                idempotent: true,
                open_world: false,
            },
        }));
        Arc::new(registry)
    }

    fn input_registry(
        provider: Option<Arc<dyn ProtectedConsentProvider>>,
        hits: Arc<AtomicUsize>,
    ) -> Arc<super::ToolRegistry> {
        let mut registry = super::ToolRegistry::new_with_protected_consent_provider(provider);
        registry.register(Box::new(ObservationProbe {
            hits,
            def: super::ToolDef {
                name: "click".into(),
                description: "test input".into(),
                input_schema: serde_json::json!({"type": "object"}),
                read_only: false,
                destructive: false,
                idempotent: false,
                open_world: false,
            },
        }));
        Arc::new(registry)
    }

    fn file_registry(
        provider: Option<Arc<dyn ProtectedConsentProvider>>,
        hits: Arc<AtomicUsize>,
        last_args: Arc<Mutex<Option<serde_json::Value>>>,
    ) -> Arc<super::ToolRegistry> {
        let mut registry = super::ToolRegistry::new_with_protected_consent_provider(provider);
        registry.register(Box::new(ArgumentProbe {
            hits,
            last_args,
            def: super::ToolDef {
                name: "browser_set_input_files".into(),
                description: "test file transfer".into(),
                input_schema: serde_json::json!({"type": "object"}),
                read_only: false,
                destructive: false,
                idempotent: false,
                open_world: true,
            },
        }));
        Arc::new(registry)
    }

    fn attested_registry(
        name: &str,
        provider: Option<Arc<dyn ProtectedConsentProvider>>,
        hits: Arc<AtomicUsize>,
        stale_before_dispatch: bool,
    ) -> Arc<super::ToolRegistry> {
        let mut registry = super::ToolRegistry::new_with_protected_consent_provider(provider);
        let scope = match name {
            "kill_app" => serde_json::json!({
                "kind": "process_instance",
                "fingerprint": {
                    "pid": 424242,
                    "start_time": 7,
                    "executable": "/synthetic/fixture"
                }
            }),
            "get_window_state" => serde_json::json!({
                "kind": "window",
                "pid": 424242,
                "window_id": 7,
                "fingerprint": {
                    "pid": 424242,
                    "start_time": 7,
                    "executable": "/synthetic/fixture"
                }
            }),
            _ => serde_json::json!({
                "kind": "synthetic_attested_resource",
                "identity": "exact-1",
            }),
        };
        registry.register(Box::new(AttestedProbe {
            hits,
            scope,
            stale_before_dispatch,
            def: super::ToolDef {
                name: name.to_owned(),
                description: "test attested protected resource".into(),
                input_schema: serde_json::json!({"type": "object"}),
                read_only: false,
                destructive: name == "kill_app",
                idempotent: false,
                open_world: name.starts_with("browser_"),
            },
        }));
        Arc::new(registry)
    }

    fn argument_registry(
        name: &str,
        provider: Option<Arc<dyn ProtectedConsentProvider>>,
        hits: Arc<AtomicUsize>,
    ) -> Arc<super::ToolRegistry> {
        let mut registry = super::ToolRegistry::new_with_protected_consent_provider(provider);
        let input_schema = if name == "set_config" {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "key": {"type": "string"},
                    "value": {},
                    "max_image_dimension": {"type": "integer"},
                    "experimental_pip": {"type": "boolean"}
                },
                "additionalProperties": false
            })
        } else {
            serde_json::json!({"type": "object"})
        };
        registry.register(Box::new(ArgumentProbe {
            hits,
            last_args: Arc::new(Mutex::new(None)),
            def: super::ToolDef {
                name: name.to_owned(),
                description: "test typed operation".into(),
                input_schema,
                read_only: false,
                destructive: false,
                idempotent: false,
                open_world: name == "page",
            },
        }));
        Arc::new(registry)
    }

    #[async_trait::async_trait]
    impl super::Tool for ReplayProbe {
        fn def(&self) -> &super::ToolDef {
            &self.def
        }

        async fn invoke(&self, _args: serde_json::Value) -> crate::protocol::ToolResult {
            self.hits.fetch_add(1, Ordering::SeqCst);
            crate::protocol::ToolResult::text("probe ran")
        }
    }

    fn replay_registry(hits: Arc<AtomicUsize>) -> Arc<super::ToolRegistry> {
        let mut registry = super::ToolRegistry::new();
        registry.register(Box::new(ReplayProbe {
            hits,
            def: super::ToolDef {
                name: "probe".into(),
                description: "runtime-local replay probe".into(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "additionalProperties": false,
                }),
                read_only: false,
                destructive: false,
                idempotent: true,
                open_world: false,
            },
        }));
        registry.register_recording_tools();
        let registry = Arc::new(registry);
        registry.init_self_weak();
        registry
    }

    #[tokio::test]
    async fn replay_dispatches_only_through_the_owning_registry() {
        let hits_a = Arc::new(AtomicUsize::new(0));
        let hits_b = Arc::new(AtomicUsize::new(0));
        let registry_a = replay_registry(hits_a.clone());
        let registry_b = replay_registry(hits_b.clone());
        let context_b = unrestricted_context();
        let trajectory = tempfile::tempdir().unwrap();
        let turn = trajectory.path().join("turn-00001");
        std::fs::create_dir(&turn).unwrap();
        std::fs::write(
            turn.join("action.json"),
            serde_json::json!({"tool": "probe", "arguments": {}}).to_string(),
        )
        .unwrap();

        let result = registry_b
            .invoke_with_context(
                "replay_trajectory",
                serde_json::json!({"dir": trajectory.path(), "delay_ms": 0}),
                context_b,
            )
            .await;
        assert_ne!(result.is_error, Some(true));
        assert_eq!(result.structured_content.unwrap()["succeeded"], 1);
        assert_eq!(hits_a.load(Ordering::SeqCst), 0);
        assert_eq!(hits_b.load(Ordering::SeqCst), 1);

        drop(registry_a);
        drop(registry_b);
    }

    #[tokio::test]
    async fn standard_observation_runs_without_a_protected_host() {
        let hits = Arc::new(AtomicUsize::new(0));
        let registry = observation_registry(None, hits.clone());
        let result = registry
            .invoke_with_context(
                "get_window_state",
                serde_json::json!({"pid": 42, "window_id": 7, "session": "review"}),
                standard_context(),
            )
            .await;

        assert_eq!(hits.load(Ordering::SeqCst), 1);
        assert_ne!(result.is_error, Some(true));
    }

    #[tokio::test]
    async fn standard_history_requires_an_explicit_protected_host_grant() {
        let denied_hits = Arc::new(AtomicUsize::new(0));
        let denied_registry = observation_registry_for("history_query", None, denied_hits.clone());
        let denied = denied_registry
            .invoke_with_context(
                "history_query",
                serde_json::json!({"session": "review"}),
                standard_context(),
            )
            .await;
        assert_eq!(denied_hits.load(Ordering::SeqCst), 0);
        assert_eq!(
            denied
                .structured_content
                .as_ref()
                .and_then(|value| value.pointer("/refusal/code"))
                .and_then(serde_json::Value::as_str),
            Some("authorization_required")
        );

        let provider = Arc::new(AcceptingProvider {
            requests: AtomicUsize::new(0),
        });
        let allowed_hits = Arc::new(AtomicUsize::new(0));
        let allowed_registry = observation_registry_for(
            "history_query",
            Some(provider.clone()),
            allowed_hits.clone(),
        );
        let allowed = allowed_registry
            .invoke_with_context(
                "history_query",
                serde_json::json!({"session": "review"}),
                standard_context(),
            )
            .await;
        assert_ne!(allowed.is_error, Some(true));
        assert_eq!(allowed_hits.load(Ordering::SeqCst), 1);
        assert_eq!(provider.requests.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn bounded_history_requires_the_matching_operation_resource() {
        for (operations, expected_allowed) in
            [(["status"].as_slice(), false), (["query"].as_slice(), true)]
        {
            let hits = Arc::new(AtomicUsize::new(0));
            let registry = observation_registry_for("history_query", None, hits.clone());
            let operations = operations
                .iter()
                .map(|operation| format!("      - {operation}"))
                .collect::<Vec<_>>()
                .join("\n");
            let context = bounded_context(&format!(
                "version: 3\nexpires_after: 1h\nidle_timeout: 30m\nresources:\n  computer_history:\n    operations:\n{operations}\nallow:\n  tools: [history_query]\n"
            ));
            let result = registry
                .invoke_with_context(
                    "history_query",
                    serde_json::json!({"session": "review"}),
                    context,
                )
                .await;
            assert_eq!(result.is_error != Some(true), expected_allowed);
            assert_eq!(hits.load(Ordering::SeqCst), usize::from(expected_allowed));
        }
    }

    #[tokio::test]
    async fn bounded_observation_uses_only_the_manifest_without_a_protected_host() {
        let hits = Arc::new(AtomicUsize::new(0));
        let registry = attested_registry("get_window_state", None, hits.clone(), false);
        let context = bounded_context(
            r#"
version: 2
mode: bounded
expires_after: 1h
idle_timeout: 30m
allow:
  tools: [get_window_state]
resources:
  apps:
    - executable: /synthetic/fixture
      launch: false
      windows: all
      terminate: deny
"#,
        );
        let result = registry
            .invoke_with_context(
                "get_window_state",
                serde_json::json!({"pid": 42, "window_id": 7, "session": "review"}),
                context,
            )
            .await;

        assert_eq!(hits.load(Ordering::SeqCst), 1);
        assert_ne!(result.is_error, Some(true));
    }

    #[tokio::test]
    async fn capability_manifest_typed_resources_narrow_standard_and_unrestricted() {
        for mode in [PermissionMode::Standard, PermissionMode::Unrestricted] {
            let allowed_hits = Arc::new(AtomicUsize::new(0));
            let registry = attested_registry("get_window_state", None, allowed_hits.clone(), false);
            let allowed = manifest_context(
                mode,
                r#"
version: 3
allow:
  tools: [get_window_state]
resources:
  apps:
    - executable: /synthetic/fixture
      windows: all
"#,
            );
            let result = registry
                .invoke_with_context(
                    "get_window_state",
                    serde_json::json!({"pid": 424242, "window_id": 7, "session": "review"}),
                    allowed,
                )
                .await;
            assert_ne!(result.is_error, Some(true), "{mode:?} in-scope call");
            assert_eq!(allowed_hits.load(Ordering::SeqCst), 1);

            let denied_hits = Arc::new(AtomicUsize::new(0));
            let registry = attested_registry("get_window_state", None, denied_hits.clone(), false);
            let denied = manifest_context(
                mode,
                r#"
version: 3
allow:
  tools: [get_window_state]
resources:
  apps:
    - executable: /another/application
      windows: all
"#,
            );
            let result = registry
                .invoke_with_context(
                    "get_window_state",
                    serde_json::json!({"pid": 424242, "window_id": 7, "session": "review"}),
                    denied,
                )
                .await;
            assert_eq!(result.is_error, Some(true), "{mode:?} out-of-scope call");
            assert_eq!(denied_hits.load(Ordering::SeqCst), 0);
        }
    }

    #[tokio::test]
    async fn capability_manifest_tools_narrow_standard_and_unrestricted() {
        for mode in [PermissionMode::Standard, PermissionMode::Unrestricted] {
            let hits = Arc::new(AtomicUsize::new(0));
            let registry = attested_registry("get_window_state", None, hits.clone(), false);
            let context = manifest_context(
                mode,
                r#"
version: 3
allow:
  tools: [click]
"#,
            );
            let result = registry
                .invoke_with_context(
                    "get_window_state",
                    serde_json::json!({"pid": 424242, "window_id": 7, "session": "review"}),
                    context,
                )
                .await;
            assert_eq!(result.is_error, Some(true), "{mode:?} undeclared tool");
            assert_eq!(hits.load(Ordering::SeqCst), 0);
        }
    }

    #[tokio::test]
    async fn rejected_typed_resource_does_not_refresh_manifest_idle_lease() {
        let denied_hits = Arc::new(AtomicUsize::new(0));
        let allowed_hits = Arc::new(AtomicUsize::new(0));
        let mut registry = super::ToolRegistry::new();
        for (name, window_id, hits) in [
            ("get_desktop_state", 8_u64, denied_hits.clone()),
            ("get_window_state", 7_u64, allowed_hits.clone()),
        ] {
            registry.register(Box::new(AttestedProbe {
                hits,
                scope: serde_json::json!({
                    "kind": "window",
                    "pid": 42,
                    "window_id": window_id,
                }),
                stale_before_dispatch: false,
                def: super::ToolDef {
                    name: name.to_owned(),
                    description: "manifest idle lease probe".into(),
                    input_schema: serde_json::json!({"type": "object"}),
                    read_only: true,
                    destructive: false,
                    idempotent: true,
                    open_world: false,
                },
            }));
        }
        let context = manifest_context(
            PermissionMode::Standard,
            r#"
version: 3
expires_after: 1h
idle_timeout: 1s
allow:
  tools: [get_desktop_state, get_window_state]
resources:
  desktop:
    windows:
      - pid: 42
        window_id: 7
"#,
        );

        tokio::time::sleep(Duration::from_millis(650)).await;
        let denied = registry
            .invoke_with_context(
                "get_desktop_state",
                serde_json::json!({"session": "lease"}),
                context.clone(),
            )
            .await;
        assert_eq!(denied.is_error, Some(true));
        assert_eq!(denied_hits.load(Ordering::SeqCst), 0);

        tokio::time::sleep(Duration::from_millis(650)).await;
        let expired = registry
            .invoke_with_context(
                "get_window_state",
                serde_json::json!({"pid": 42, "window_id": 7, "session": "lease"}),
                context,
            )
            .await;
        assert_eq!(expired.is_error, Some(true));
        assert_eq!(allowed_hits.load(Ordering::SeqCst), 0);
        assert!(expired
            .content
            .iter()
            .any(|content| matches!(content, crate::protocol::Content::Text { text, .. } if text.contains("idle timeout"))));
    }

    #[tokio::test]
    async fn ended_session_and_runtime_suspend_latches_fail_closed_at_dispatch() {
        let ended_hits = Arc::new(AtomicUsize::new(0));
        let ended_registry = observation_registry(None, ended_hits.clone());
        let ended_context = standard_context();
        let ended_prefix = format!("__cua_runtime_{}:", ended_context.runtime_scope_key());
        let ended_runtime_session = ended_context.runtime_session_key("ended");
        crate::session::end_session(&ended_runtime_session);
        let ended = ended_registry
            .invoke_with_context(
                "get_window_state",
                serde_json::json!({"pid": 42, "window_id": 7, "session": "ended"}),
                ended_context,
            )
            .await;
        assert_eq!(ended_hits.load(Ordering::SeqCst), 0);
        assert_eq!(
            ended
                .structured_content
                .as_ref()
                .and_then(|value| value.pointer("/refusal/code"))
                .and_then(serde_json::Value::as_str),
            Some("session_ended")
        );

        let suspended_hits = Arc::new(AtomicUsize::new(0));
        let suspended_registry = observation_registry(None, suspended_hits.clone());
        let suspended_context = standard_context();
        let scope = suspended_context.runtime_scope_key();
        assert!(crate::session::suspend_runtime_scope(&scope));
        let suspended = suspended_registry
            .invoke_with_context(
                "get_window_state",
                serde_json::json!({"pid": 42, "window_id": 7, "session": "new-label"}),
                suspended_context,
            )
            .await;
        assert_eq!(suspended_hits.load(Ordering::SeqCst), 0);
        assert_eq!(
            suspended
                .structured_content
                .as_ref()
                .and_then(|value| value.pointer("/refusal/code"))
                .and_then(serde_json::Value::as_str),
            Some("authorization_suspended")
        );
        assert!(crate::session::forget_suspended_runtime_scope(&scope));
        crate::session::forget_ended_sessions_with_prefix(&ended_prefix);
    }

    #[tokio::test]
    async fn standard_observation_never_requests_per_window_grants() {
        let hits = Arc::new(AtomicUsize::new(0));
        let provider = Arc::new(AcceptingProvider {
            requests: AtomicUsize::new(0),
        });
        let registry = observation_registry(Some(provider.clone()), hits.clone());
        let context = standard_context();
        for window_id in [7, 7, 8] {
            let result = registry
                .invoke_with_context(
                    "get_window_state",
                    serde_json::json!({
                        "pid": 42,
                        "window_id": window_id,
                        "session": "review"
                    }),
                    context.clone(),
                )
                .await;
            assert_ne!(result.is_error, Some(true));
        }
        let display_wide = registry
            .invoke_with_context(
                "get_window_state",
                serde_json::json!({"session": "review"}),
                context,
            )
            .await;
        assert_ne!(
            display_wide.is_error,
            Some(true),
            "standard display-wide observation must not require get_screen_size"
        );

        assert_eq!(hits.load(Ordering::SeqCst), 4);
        assert_eq!(provider.requests.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn trajectory_only_recording_does_not_require_live_display_discovery() {
        let hits = Arc::new(AtomicUsize::new(0));
        let provider = Arc::new(AcceptingProvider {
            requests: AtomicUsize::new(0),
        });
        let mut registry =
            super::ToolRegistry::new_with_protected_consent_provider(Some(provider.clone()));
        registry.register(Box::new(ObservationProbe {
            hits: hits.clone(),
            def: super::ToolDef {
                name: "start_recording".into(),
                description: "test trajectory-only recording".into(),
                input_schema: serde_json::json!({"type": "object"}),
                read_only: false,
                destructive: false,
                idempotent: true,
                open_world: false,
            },
        }));

        let result = registry
            .invoke_with_context(
                "start_recording",
                serde_json::json!({
                    "output_dir": "/synthetic/recording",
                    "record_video": false,
                    "session": "recording"
                }),
                standard_context(),
            )
            .await;

        assert_ne!(result.is_error, Some(true));
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        assert_eq!(
            provider.requests.load(Ordering::SeqCst),
            0,
            "routine standard recording is promptless"
        );
    }

    #[tokio::test]
    async fn standard_desktop_input_is_promptless_across_delivery_modes() {
        let no_host_hits = Arc::new(AtomicUsize::new(0));
        let no_host = input_registry(None, no_host_hits.clone())
            .invoke_with_context(
                "click",
                serde_json::json!({
                    "pid": 42,
                    "window_id": 7,
                    "session": "control",
                    "x": 10,
                    "y": 20
                }),
                standard_context(),
            )
            .await;
        assert_eq!(no_host_hits.load(Ordering::SeqCst), 1);
        assert_ne!(no_host.is_error, Some(true));
        let display_wide_hits = Arc::new(AtomicUsize::new(0));
        let display_wide = input_registry(None, display_wide_hits.clone())
            .invoke_with_context(
                "click",
                serde_json::json!({
                    "session": "control",
                    "x": 10,
                    "y": 20
                }),
                standard_context(),
            )
            .await;
        assert_eq!(display_wide_hits.load(Ordering::SeqCst), 1);
        assert_ne!(
            display_wide.is_error,
            Some(true),
            "standard display-wide input must not require get_screen_size"
        );

        let hits = Arc::new(AtomicUsize::new(0));
        let provider = Arc::new(AcceptingProvider {
            requests: AtomicUsize::new(0),
        });
        let registry = input_registry(Some(provider.clone()), hits.clone());
        let context = standard_context();
        for delivery_mode in ["background", "background", "foreground"] {
            let result = registry
                .invoke_with_context(
                    "click",
                    serde_json::json!({
                        "pid": 42,
                        "window_id": 7,
                        "session": "control",
                        "delivery_mode": delivery_mode,
                        "x": 10,
                        "y": 20
                    }),
                    context.clone(),
                )
                .await;
            assert_ne!(result.is_error, Some(true));
        }
        assert_eq!(hits.load(Ordering::SeqCst), 3);
        assert_eq!(provider.requests.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn canonical_dispatch_normalizes_legacy_delivery_mode_before_execution() {
        let hits = Arc::new(AtomicUsize::new(0));
        let last_args = Arc::new(Mutex::new(None));
        let mut registry = super::ToolRegistry::new();
        registry.register(Box::new(ArgumentProbe {
            hits: hits.clone(),
            last_args: last_args.clone(),
            def: super::ToolDef {
                name: "click".into(),
                description: "test input".into(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "delivery_mode": crate::tool_schema::delivery_mode_schema()
                    }
                }),
                read_only: false,
                destructive: false,
                idempotent: false,
                open_world: false,
            },
        }));
        let registry = Arc::new(registry);

        let result = registry
            .invoke_with_context(
                "click",
                serde_json::json!({"dispatch": "foreground"}),
                standard_context(),
            )
            .await;

        assert_ne!(result.is_error, Some(true));
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        let received = last_args.lock().unwrap().clone().expect("arguments");
        assert_eq!(received["delivery_mode"], "foreground");
        assert!(received.get("dispatch").is_none());
    }

    #[tokio::test]
    async fn standard_file_transfer_is_promptless_and_still_canonicalizes_paths() {
        let files = tempfile::tempdir().unwrap();
        let first = files.path().join("first.txt");
        let second = files.path().join("second.txt");
        std::fs::write(&first, b"first").unwrap();
        std::fs::write(&second, b"second").unwrap();

        let no_host_hits = Arc::new(AtomicUsize::new(0));
        let no_host = file_registry(None, no_host_hits.clone(), Arc::new(Mutex::new(None)))
            .invoke_with_context(
                "browser_set_input_files",
                serde_json::json!({
                    "target_id": "browser",
                    "tab_id": "tab",
                    "ref": "p1:0",
                    "session": "transfer",
                    "files": [first],
                }),
                standard_context(),
            )
            .await;
        assert_eq!(no_host_hits.load(Ordering::SeqCst), 1);
        assert_ne!(no_host.is_error, Some(true));

        let hits = Arc::new(AtomicUsize::new(0));
        let last_args = Arc::new(Mutex::new(None));
        let provider = Arc::new(AcceptingProvider {
            requests: AtomicUsize::new(0),
        });
        let registry = file_registry(Some(provider.clone()), hits.clone(), last_args.clone());
        let context = standard_context();
        for path in [&first, &first, &second] {
            let result = registry
                .invoke_with_context(
                    "browser_set_input_files",
                    serde_json::json!({
                        "target_id": "browser",
                        "tab_id": "tab",
                        "ref": "p1:0",
                        "session": "transfer",
                        "files": [path],
                    }),
                    context.clone(),
                )
                .await;
            assert_ne!(result.is_error, Some(true));
        }
        assert_eq!(hits.load(Ordering::SeqCst), 3);
        assert_eq!(provider.requests.load(Ordering::SeqCst), 0);
        let expected = std::fs::canonicalize(second)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert_eq!(
            last_args.lock().unwrap().as_ref().unwrap()["files"][0],
            expected
        );
    }

    #[tokio::test]
    async fn legacy_page_mutations_require_trusted_unrestricted_launch_acceptance() {
        let hits = Arc::new(AtomicUsize::new(0));
        let registry = argument_registry("page", None, hits.clone());
        let denied = registry
            .invoke_with_context(
                "page",
                serde_json::json!({
                    "action": "execute_javascript",
                    "pid": 42,
                    "window_id": 7,
                    "javascript": "1 + 1",
                    "session": "legacy"
                }),
                standard_context(),
            )
            .await;
        assert_eq!(hits.load(Ordering::SeqCst), 0);
        assert_eq!(
            denied
                .structured_content
                .as_ref()
                .and_then(|value| value.pointer("/refusal/code"))
                .and_then(serde_json::Value::as_str),
            Some("unbounded_operation_requires_unrestricted")
        );

        let allowed = registry
            .invoke_with_context(
                "page",
                serde_json::json!({
                    "action": "execute_javascript",
                    "pid": 42,
                    "window_id": 7,
                    "javascript": "1 + 1",
                    "session": "legacy"
                }),
                unrestricted_context(),
            )
            .await;
        assert_ne!(allowed.is_error, Some(true));
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn os_permission_prompt_is_never_agent_controllable() {
        let hits = Arc::new(AtomicUsize::new(0));
        let registry = argument_registry("check_permissions", None, hits.clone());
        for context in [standard_context(), unrestricted_context()] {
            let denied = registry
                .invoke_with_context(
                    "check_permissions",
                    serde_json::json!({"prompt": true, "session": "permissions"}),
                    context,
                )
                .await;
            assert_eq!(
                denied
                    .structured_content
                    .as_ref()
                    .and_then(|value| value.pointer("/refusal/code"))
                    .and_then(serde_json::Value::as_str),
                Some("os_permission_prompt_requires_trusted_host")
            );
        }
        let inspected = registry
            .invoke_with_context(
                "check_permissions",
                serde_json::json!({"prompt": false, "session": "permissions"}),
                standard_context(),
            )
            .await;
        assert_ne!(inspected.is_error, Some(true));
        let inspected_default = registry
            .invoke_with_context(
                "check_permissions",
                serde_json::json!({"session": "permissions"}),
                standard_context(),
            )
            .await;
        assert_ne!(inspected_default.is_error, Some(true));
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn standard_browser_input_uses_the_existing_binding_without_action_grants() {
        let no_host_hits = Arc::new(AtomicUsize::new(0));
        let no_host = attested_registry("browser_click", None, no_host_hits.clone(), false)
            .invoke_with_context(
                "browser_click",
                serde_json::json!({
                    "target_id": "target-1",
                    "tab_id": "tab-1",
                    "session": "browser"
                }),
                standard_context(),
            )
            .await;
        assert_eq!(no_host_hits.load(Ordering::SeqCst), 1);
        assert_ne!(no_host.is_error, Some(true));

        let hits = Arc::new(AtomicUsize::new(0));
        let provider = Arc::new(AcceptingProvider {
            requests: AtomicUsize::new(0),
        });
        let registry =
            attested_registry("browser_click", Some(provider.clone()), hits.clone(), false);
        let context = standard_context();
        for _ in 0..2 {
            let result = registry
                .invoke_with_context(
                    "browser_click",
                    serde_json::json!({
                        "target_id": "target-1",
                        "tab_id": "tab-1",
                        "session": "browser"
                    }),
                    context.clone(),
                )
                .await;
            assert_ne!(result.is_error, Some(true));
        }
        assert_eq!(hits.load(Ordering::SeqCst), 2);
        assert_eq!(provider.requests.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn standard_foreign_process_termination_denies_without_prompting() {
        let hits = Arc::new(AtomicUsize::new(0));
        let provider = Arc::new(AcceptingProvider {
            requests: AtomicUsize::new(0),
        });
        let registry = attested_registry("kill_app", Some(provider.clone()), hits.clone(), true);
        let result = registry
            .invoke_with_context(
                "kill_app",
                serde_json::json!({"pid": 424242, "session": "process"}),
                standard_context(),
            )
            .await;
        assert_eq!(hits.load(Ordering::SeqCst), 0);
        assert_eq!(provider.requests.load(Ordering::SeqCst), 0);
        assert_eq!(
            result
                .structured_content
                .as_ref()
                .and_then(|value| value.pointer("/refusal/code"))
                .and_then(serde_json::Value::as_str),
            Some("foreign_process_termination_denied")
        );
    }

    #[tokio::test]
    async fn manifest_cannot_widen_standard_foreign_process_termination() {
        let hits = Arc::new(AtomicUsize::new(0));
        let provider = Arc::new(AcceptingProvider {
            requests: AtomicUsize::new(0),
        });
        let registry = attested_registry("kill_app", Some(provider.clone()), hits.clone(), false);
        let context = manifest_context(
            PermissionMode::Standard,
            r#"
version: 3
allow:
  tools: [kill_app]
resources:
  processes:
    terminate: [424242]
"#,
        );
        let result = registry
            .invoke_with_context(
                "kill_app",
                serde_json::json!({"pid": 424242, "session": "process"}),
                context,
            )
            .await;
        assert_eq!(hits.load(Ordering::SeqCst), 0);
        assert_eq!(provider.requests.load(Ordering::SeqCst), 0);
        assert_eq!(
            result
                .structured_content
                .as_ref()
                .and_then(|value| value.pointer("/refusal/code"))
                .and_then(serde_json::Value::as_str),
            Some("foreign_process_termination_denied")
        );
    }

    #[tokio::test]
    async fn manifest_preserves_standard_owned_process_termination_inside_scope() {
        let hits = Arc::new(AtomicUsize::new(0));
        let registry = attested_registry("kill_app", None, hits.clone(), false);
        let context = manifest_context(
            PermissionMode::Standard,
            r#"
version: 3
allow:
  tools: [kill_app]
resources:
  processes:
    terminate: [424242]
"#,
        );
        registry
            .protected_resource_ownership()
            .mark_driver_owned_process(
                &context.runtime_session_key("process"),
                crate::browser::ProcessFingerprint {
                    pid: 424242,
                    start_time: Some(7),
                    executable: Some("/synthetic/fixture".to_owned()),
                },
            );

        let result = registry
            .invoke_with_context(
                "kill_app",
                serde_json::json!({"pid": 424242, "session": "process"}),
                context,
            )
            .await;
        assert_ne!(result.is_error, Some(true));
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn standard_owned_process_termination_reproves_the_fingerprint() {
        let hits = Arc::new(AtomicUsize::new(0));
        let registry = attested_registry("kill_app", None, hits.clone(), false);
        let context = standard_context();
        let runtime_session = context.runtime_session_key("process");
        registry
            .protected_resource_ownership()
            .mark_driver_owned_process(
                &runtime_session,
                crate::browser::ProcessFingerprint {
                    pid: 424242,
                    start_time: Some(7),
                    executable: Some("/synthetic/fixture".to_owned()),
                },
            );

        let result = registry
            .invoke_with_context(
                "kill_app",
                serde_json::json!({"pid": 424242, "session": "process"}),
                context,
            )
            .await;
        assert_ne!(result.is_error, Some(true));
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn standard_sessionless_owned_process_termination_reproves_the_fingerprint() {
        // A call without a public label now receives the runtime's implicit
        // lifecycle identity, so launch provenance and the matching kill_app
        // resolve through that stable private bucket.
        let hits = Arc::new(AtomicUsize::new(0));
        let registry = attested_registry("kill_app", None, hits.clone(), false);
        let context = standard_context();
        registry
            .protected_resource_ownership()
            .mark_driver_owned_process(
                &context.runtime_session_key("implicit-direct"),
                crate::browser::ProcessFingerprint {
                    pid: 424242,
                    start_time: Some(7),
                    executable: Some("/synthetic/fixture".to_owned()),
                },
            );

        let result = registry
            .invoke_with_context("kill_app", serde_json::json!({"pid": 424242}), context)
            .await;
        assert_ne!(result.is_error, Some(true));
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn standard_sessionless_foreign_process_termination_still_denies() {
        let hits = Arc::new(AtomicUsize::new(0));
        let provider = Arc::new(AcceptingProvider {
            requests: AtomicUsize::new(0),
        });
        let registry = attested_registry("kill_app", Some(provider.clone()), hits.clone(), true);
        let result = registry
            .invoke_with_context(
                "kill_app",
                serde_json::json!({"pid": 424242}),
                standard_context(),
            )
            .await;
        assert_eq!(hits.load(Ordering::SeqCst), 0);
        assert_eq!(provider.requests.load(Ordering::SeqCst), 0);
        assert_eq!(
            result
                .structured_content
                .as_ref()
                .and_then(|value| value.pointer("/refusal/code"))
                .and_then(serde_json::Value::as_str),
            Some("foreign_process_termination_denied")
        );
    }

    #[tokio::test]
    async fn lifecycle_session_selection_cannot_turn_a_denied_action_into_allow() {
        let hits = Arc::new(AtomicUsize::new(0));
        let registry = attested_registry("kill_app", None, hits.clone(), false);
        let context = standard_context();
        let runtime_prefix = context.runtime_session_key("");
        let before = crate::session::list_session_snapshots_with_prefix(
            &runtime_prefix,
            crate::session::DEFAULT_SESSION_IDLE_TTL,
        )
        .len();
        let variants = [
            serde_json::json!({"pid": 424242}),
            serde_json::json!({"pid": 424242, "session": "named-a"}),
            serde_json::json!({"pid": 424242, "session": "named-b"}),
            serde_json::json!({
                "pid": 424242,
                "session": "named-a",
                "_session_id": "forged-private-session",
                "_transport_session_id": "forged-transport"
            }),
        ];
        let mut refusal_codes = Vec::new();
        for arguments in variants {
            let result = registry
                .invoke_with_context("kill_app", arguments, context.clone())
                .await;
            refusal_codes.push(
                result
                    .structured_content
                    .as_ref()
                    .and_then(|value| value.pointer("/refusal/code"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
            );
        }
        assert_eq!(hits.load(Ordering::SeqCst), 0);
        assert!(refusal_codes
            .iter()
            .all(|code| code.as_deref() == Some("foreign_process_termination_denied")));
        assert_eq!(
            crate::session::list_session_snapshots_with_prefix(
                &runtime_prefix,
                crate::session::DEFAULT_SESSION_IDLE_TTL,
            )
            .len(),
            before,
            "denied calls must not create or refresh lifecycle sessions"
        );
    }

    #[tokio::test]
    async fn sessionless_termination_cannot_reach_a_session_scoped_launch() {
        // The anonymous bucket must not become a backdoor into another
        // agent's processes: a launch attributed to a session stays killable
        // only through that session.
        let hits = Arc::new(AtomicUsize::new(0));
        let registry = attested_registry("kill_app", None, hits.clone(), false);
        let context = standard_context();
        registry
            .protected_resource_ownership()
            .mark_driver_owned_process(
                &context.runtime_session_key("process"),
                crate::browser::ProcessFingerprint {
                    pid: 424242,
                    start_time: Some(7),
                    executable: Some("/synthetic/fixture".to_owned()),
                },
            );

        let result = registry
            .invoke_with_context("kill_app", serde_json::json!({"pid": 424242}), context)
            .await;
        assert_eq!(hits.load(Ordering::SeqCst), 0);
        assert_eq!(
            result
                .structured_content
                .as_ref()
                .and_then(|value| value.pointer("/refusal/code"))
                .and_then(serde_json::Value::as_str),
            Some("foreign_process_termination_denied")
        );
    }

    #[test]
    fn anonymous_process_ownership_key_cannot_be_selected_by_a_session_argument() {
        let prefix = "__cua_runtime_scope:";
        let anonymous = super::process_ownership_key(None, prefix);
        assert_eq!(super::process_ownership_key(Some("s"), prefix), "s");
        assert!(anonymous.starts_with(prefix));
        // A namespaced public session label is prefix + the caller's string;
        // the NUL keeps the anonymous bucket outside that space.
        assert!(anonymous.contains('\u{0}'));
    }

    #[tokio::test]
    async fn standard_browser_identity_is_reproved_without_repeating_attach_authorization() {
        let hits = Arc::new(AtomicUsize::new(0));
        let provider = Arc::new(AcceptingProvider {
            requests: AtomicUsize::new(0),
        });
        let registry =
            attested_registry("browser_click", Some(provider.clone()), hits.clone(), true);
        let result = registry
            .invoke_with_context(
                "browser_click",
                serde_json::json!({
                    "target_id": "target-1",
                    "tab_id": "tab-1",
                    "session": "browser"
                }),
                standard_context(),
            )
            .await;
        assert_eq!(hits.load(Ordering::SeqCst), 0);
        assert_eq!(provider.requests.load(Ordering::SeqCst), 0);
        assert_eq!(
            result
                .structured_content
                .as_ref()
                .and_then(|value| value.pointer("/refusal/code"))
                .and_then(serde_json::Value::as_str),
            Some("protected_resource_scope_stale")
        );
    }

    #[tokio::test]
    async fn browser_observation_without_a_live_origin_attestation_fails_closed() {
        let hits = Arc::new(AtomicUsize::new(0));
        let provider = Arc::new(AcceptingProvider {
            requests: AtomicUsize::new(0),
        });
        let registry =
            observation_registry_for("get_browser_state", Some(provider.clone()), hits.clone());
        let result = registry
            .invoke_with_context(
                "get_browser_state",
                serde_json::json!({
                    "target_id": "caller-supplied-target",
                    "tab_id": "caller-supplied-tab",
                    "session": "browser"
                }),
                standard_context(),
            )
            .await;
        assert_eq!(result.is_error, Some(true));
        assert_eq!(hits.load(Ordering::SeqCst), 0);
        assert_eq!(provider.requests.load(Ordering::SeqCst), 0);
        assert!(result
            .content
            .iter()
            .any(|item| format!("{item:?}").contains("live top-level origin")));
    }

    #[tokio::test]
    async fn invoke_with_context_strips_reserved_caller_arguments_at_the_registry_boundary() {
        let hits = Arc::new(AtomicUsize::new(0));
        let last_args = Arc::new(Mutex::new(None));
        let registry = file_registry(None, hits.clone(), last_args.clone());
        let result = registry
            .invoke_with_context(
                "browser_set_input_files",
                serde_json::json!({
                    "_protected_process_fingerprint": {"pid": 1},
                    "_session_id": "forged",
                    "files": ["/does/not/matter"]
                }),
                unrestricted_context(),
            )
            .await;
        assert_ne!(result.is_error, Some(true));
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        let received = last_args.lock().unwrap().clone().unwrap();
        assert!(received.get("_protected_process_fingerprint").is_none());
        assert!(received.get("_session_id").is_none());
    }

    #[tokio::test]
    async fn trusted_adapter_evidence_is_restored_only_after_caller_fields_are_stripped() {
        let hits = Arc::new(AtomicUsize::new(0));
        let last_args = Arc::new(Mutex::new(None));
        let registry = file_registry(None, hits.clone(), last_args.clone());
        let mut args = serde_json::json!({
            "session": "public",
            "_session_id": "trusted-owner",
            "_transport_session_id": "transport-a",
            "_cua_browser_download_mcp_host_approved": true,
            "_protected_process_fingerprint": {"pid": 1},
            "files": ["/does/not/matter"]
        });
        let evidence = TrustedInvocationEvidence::extract_from_adapter_args(&mut args);
        assert!(args
            .as_object()
            .unwrap()
            .keys()
            .all(|key| !key.starts_with('_')));

        let result = registry
            .invoke_with_context_and_evidence(
                "browser_set_input_files",
                args,
                unrestricted_context(),
                evidence,
            )
            .await;
        assert_ne!(result.is_error, Some(true));
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        let received = last_args.lock().unwrap().clone().unwrap();
        assert!(received
            .get("_session_id")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|value| value.ends_with(":trusted-owner")));
        assert!(received
            .get("_transport_session_id")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|value| value.ends_with(":transport-a")));
        assert_eq!(
            received["_cua_browser_download_mcp_host_approved"],
            serde_json::Value::Bool(true)
        );
        assert!(received.get("_protected_process_fingerprint").is_none());
    }

    #[tokio::test]
    async fn standard_agent_adjustable_driver_configuration_is_promptless() {
        let hits = Arc::new(AtomicUsize::new(0));
        let provider = Arc::new(AcceptingProvider {
            requests: AtomicUsize::new(0),
        });
        let registry = argument_registry("set_config", Some(provider.clone()), hits.clone());
        let context = standard_context();
        for dimension in [800, 800, 1200] {
            let result = registry
                .invoke_with_context(
                    "set_config",
                    serde_json::json!({
                        "key": "max_image_dimension",
                        "value": dimension,
                        "session": "config"
                    }),
                    context.clone(),
                )
                .await;
            assert_ne!(result.is_error, Some(true));
        }
        assert_eq!(hits.load(Ordering::SeqCst), 3);
        assert_eq!(provider.requests.load(Ordering::SeqCst), 0);

        let unknown = registry
            .invoke_with_context(
                "set_config",
                serde_json::json!({
                    "key": "future_unreviewed_setting",
                    "value": true,
                    "session": "config"
                }),
                context,
            )
            .await;
        assert_eq!(unknown.is_error, Some(true));
        assert_eq!(hits.load(Ordering::SeqCst), 3);
        assert_eq!(provider.requests.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn proposed_output_scope_canonicalizes_the_existing_ancestor_without_creating_output() {
        let root = tempfile::tempdir().unwrap();
        let proposed = root.path().join("nested").join("capture.png");
        let canonical = canonical_proposed_path(proposed.to_str().unwrap()).unwrap();
        assert_eq!(
            canonical,
            std::fs::canonicalize(root.path())
                .unwrap()
                .join("nested")
                .join("capture.png")
                .to_string_lossy()
        );
        assert!(!proposed.exists());
    }

    #[tokio::test]
    async fn runtime_owned_pid_is_prompt_light_and_session_revocation_removes_provenance() {
        let hits = Arc::new(AtomicUsize::new(0));
        let registry = input_registry(None, hits.clone());
        let context = standard_context();
        let runtime_session = context.runtime_session_key("isolated");
        registry
            .protected_resource_ownership()
            .mark_driver_owned_pid(&runtime_session, 42);
        let args = serde_json::json!({
            "pid": 42,
            "window_id": 7,
            "session": "isolated",
            "x": 10,
            "y": 20
        });
        let first = registry
            .invoke_with_context("click", args.clone(), context.clone())
            .await;
        assert_ne!(first.is_error, Some(true));
        let action = first
            .action_record
            .as_ref()
            .expect("canonical dispatch should normalize legacy action truth");
        assert_eq!(
            action.effect,
            crate::action_record::ActionEffect::Unverifiable
        );
        assert!(action
            .debug_json()
            .get("requested_delivery")
            .is_some_and(|value| value == "background"));
        assert_eq!(hits.load(Ordering::SeqCst), 1);

        crate::session::fire_session_end(&runtime_session);
        let after_end = registry.invoke_with_context("click", args, context).await;
        assert_eq!(after_end.is_error, Some(true));
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn canonical_dispatch_replaces_legacy_action_payload_with_the_closed_outcome() {
        let hits = Arc::new(AtomicUsize::new(0));
        let registry = input_registry(None, hits.clone());
        let context = standard_context();
        let runtime_session = context.runtime_session_key("projection");
        registry
            .protected_resource_ownership()
            .mark_driver_owned_pid(&runtime_session, 42);

        let result = registry
            .invoke_with_context(
                "click",
                serde_json::json!({
                    "pid": 42,
                    "window_id": 7,
                    "session": "projection",
                    "x": 10,
                    "y": 20
                }),
                context,
            )
            .await;

        assert_ne!(result.is_error, Some(true));
        let structured = result
            .structured_content
            .expect("successful actions publish an ActionResult");
        let object = structured.as_object().expect("ActionResult is an object");
        assert_eq!(
            object.keys().map(String::as_str).collect::<Vec<_>>(),
            ["delivery", "effect", "route"]
        );
        assert_eq!(structured["effect"], "unverifiable");
        assert_eq!(structured["delivery"]["mode"], "unknown");
        assert!(matches!(
            structured["route"].as_str(),
            Some("accessibility" | "synthetic_events" | "global_input" | "dom" | "trusted_input")
        ));
        assert!(structured.get("snapshot_id").is_none());
        assert!(structured.get("x").is_none());
        assert!(structured.get("y").is_none());
        assert!(structured.get("scope").is_none());
        assert!(structured.get("verified").is_none());
        cua_driver_contract::validate_success_output("click", structured)
            .expect("the projected action must satisfy the public contract");
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn action_projection_keeps_browser_refusal_diagnostics_and_closes_structured_content() {
        let legacy = serde_json::json!({
            "status": "refused",
            "refusal": {
                "code": "browser_ref_stale",
                "message": "take a fresh browser snapshot"
            }
        });
        let mut result = crate::protocol::ToolResult::text(
            "refused (browser_ref_stale): take a fresh browser snapshot",
        )
        .with_structured(legacy.clone());
        result.action_record = crate::action_record::ActionExecutionRecord::from_legacy(
            "browser_click",
            &serde_json::json!({"input_route": "trusted"}),
            &legacy,
        );

        publish_action_result(&mut result).expect("browser refusal should project");
        let structured = result.structured_content.as_ref().unwrap();
        assert_eq!(structured["effect"], "refused");
        assert_eq!(structured["route"], "trusted_input");
        assert_eq!(structured["escalation"]["target"], "page");
        assert!(structured.get("refusal").is_none());
        assert!(result.content.iter().any(|content| {
            matches!(
                content,
                crate::protocol::Content::Text { text, .. }
                    if text.contains("browser_ref_stale")
            )
        }));
    }

    #[tokio::test]
    async fn element_tokens_are_bound_to_the_dispatch_runtime_generation() {
        let pid = 8_675_309;
        let token = DISPATCH_RUNTIME_SCOPE
            .scope("runtime-a".to_owned(), async {
                let snapshot = crate::element_token::global().register_snapshot(pid, 44, 1);
                crate::element_token::token_for(snapshot, 0)
            })
            .await;

        let cross_runtime = DISPATCH_RUNTIME_SCOPE
            .scope("runtime-b".to_owned(), async {
                crate::element_token::global().resolve(pid, &token)
            })
            .await;
        assert_eq!(
            cross_runtime.unwrap_err(),
            "element_token belongs to another runtime generation"
        );
        let structured = DISPATCH_RUNTIME_SCOPE
            .scope("runtime-b".to_owned(), async {
                crate::element_token::resolve_element_args(
                    pid,
                    None,
                    Some(&token),
                    None,
                    None,
                    "click",
                )
                .unwrap_err()
            })
            .await
            .structured_content
            .unwrap();
        assert_eq!(
            structured.pointer("/refusal/code"),
            Some(&serde_json::Value::String("generation_mismatch".into()))
        );

        let owner = DISPATCH_RUNTIME_SCOPE
            .scope("runtime-a".to_owned(), async {
                crate::element_token::global().resolve(pid, &token)
            })
            .await;
        assert_eq!(owner.unwrap(), (44, 0));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn physical_desktop_actions_are_admitted_one_at_a_time() {
        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let mut tasks = Vec::new();
        for _ in 0..8 {
            let active = active.clone();
            let max_active = max_active.clone();
            tasks.push(tokio::spawn(async move {
                let _admission = desktop_action_coordinator().lock().await;
                let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                max_active.fetch_max(now, Ordering::SeqCst);
                tokio::task::yield_now().await;
                active.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        for task in tasks {
            task.await.unwrap();
        }
        assert_eq!(max_active.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn overlapping_text_input_for_one_pid_fails_fast_and_releases_after_completion() {
        let pid = 8_675_410;
        let first = try_admit_text_input("type_text", &serde_json::json!({"pid": pid}))
            .expect("first text operation should be admitted")
            .expect("pid-scoped text operation should receive a guard");

        let refusal = try_admit_text_input("type_text", &serde_json::json!({"pid": pid}))
            .expect_err("overlapping text operation should fail fast");
        assert_eq!(refusal.is_error, Some(true));
        assert_eq!(
            refusal
                .structured_content
                .as_ref()
                .and_then(|value| value.pointer("/refusal/code"))
                .and_then(serde_json::Value::as_str),
            Some("input_busy")
        );

        let other_pid = try_admit_text_input("type_text", &serde_json::json!({"pid": pid + 1}))
            .expect("a different pid should have an independent input lane")
            .expect("pid-scoped text operation should receive a guard");
        drop(other_pid);
        drop(first);

        assert!(
            try_admit_text_input("type_text", &serde_json::json!({"pid": pid}))
                .expect("completed text operation should release its pid")
                .is_some()
        );
    }

    #[test]
    fn runtime_namespaces_sessions_and_explicit_cursor_ids_without_leaking_them() {
        let context = standard_context();
        let mut args = serde_json::json!({
            "session": "shared-session",
            "_session_id": "shared-session",
            "cursor_id": "shared-cursor",
        });
        let prefix = namespace_runtime_args(
            &mut args,
            context.as_ref(),
            &TrustedInvocationEvidence::default(),
        );
        assert_eq!(args["session"], format!("{prefix}shared-session"));
        assert_eq!(args["_session_id"], format!("{prefix}shared-session"));
        assert_eq!(args["cursor_id"], format!("{prefix}shared-cursor"));
        assert_eq!(args["_public_session_label"], "shared-session");

        let mut result = ToolResult::text(format!("{prefix}shared-session")).with_structured(
            serde_json::json!({
                format!("{prefix}shared-session"): {
                    "cursor_id": format!("{prefix}shared-cursor"),
                }
            }),
        );
        restore_public_runtime_result(&mut result, &prefix);
        assert!(matches!(
            &result.content[0],
            crate::protocol::Content::Text { text, .. } if text == "shared-session"
        ));
        assert_eq!(
            result
                .structured_content
                .as_ref()
                .and_then(|value| value.pointer("/shared-session/cursor_id"))
                .and_then(serde_json::Value::as_str),
            Some("shared-cursor")
        );
    }

    #[test]
    fn runtime_namespace_restoration_refuses_duplicate_public_object_keys() {
        let context = standard_context();
        let mut args = serde_json::json!({"session": "shared-session"});
        let prefix = namespace_runtime_args(
            &mut args,
            context.as_ref(),
            &TrustedInvocationEvidence::default(),
        );
        let mut result = ToolResult::text("ambiguous").with_structured(serde_json::json!({
            "shared-session": "public",
            format!("{prefix}shared-session"): "private",
        }));

        restore_public_runtime_result(&mut result, &prefix);

        assert_eq!(result.is_error, Some(true));
        assert_eq!(
            result
                .structured_content
                .as_ref()
                .and_then(|value| { value.get("code").and_then(serde_json::Value::as_str) }),
            Some("runtime_output_key_collision")
        );
    }

    #[test]
    fn runtime_mints_a_private_ownership_key_for_direct_session_calls() {
        let context = standard_context();
        let mut args = serde_json::json!({"session": "direct-session"});
        let prefix = namespace_runtime_args(
            &mut args,
            context.as_ref(),
            &TrustedInvocationEvidence::default(),
        );
        assert_eq!(args["session"], format!("{prefix}direct-session"));
        assert_eq!(args["_session_id"], format!("{prefix}direct-session"));
    }

    #[test]
    fn anonymous_default_identity_keeps_its_legacy_process_scope() {
        let context = standard_context();
        let mut args = serde_json::json!({
            "session": "default",
            "_session_id": "default",
            "cursor_id": "default",
        });
        namespace_runtime_args(
            &mut args,
            context.as_ref(),
            &TrustedInvocationEvidence::default(),
        );
        assert_eq!(args["session"], "default");
        assert_eq!(args["_session_id"], "default");
        assert_eq!(args["cursor_id"], "default");
        assert!(args.get("_public_session_label").is_none());
    }
}

fn permission_denied_result(message: String) -> ToolResult {
    ToolResult::error(message.clone()).with_structured(serde_json::json!({
        "status": "refused",
        "refusal": {
            "code": "permission_denied",
            "message": message,
        }
    }))
}

fn protected_consent_refusal(error: crate::consent::ConsentError) -> ToolResult {
    let code = match &error {
        crate::consent::ConsentError::Unavailable => "authorization_required",
        crate::consent::ConsentError::Declined => "authorization_declined",
        crate::consent::ConsentError::Canceled => "authorization_canceled",
        crate::consent::ConsentError::DigestMismatch => "authorization_scope_mismatch",
        crate::consent::ConsentError::Expired => "authorization_expired",
        crate::consent::ConsentError::Provider(_) => "authorization_host_failed",
        crate::consent::ConsentError::BoundedResource(_) => "bounded_resource_outside_manifest",
    };
    let message = error.to_string();
    ToolResult::error(message.clone()).with_structured(serde_json::json!({
        "status": "refused",
        "refusal": {
            "code": code,
            "message": message,
        }
    }))
}

fn protected_scope_refusal(message: &str) -> ToolResult {
    protected_refusal("protected_resource_scope_invalid", message)
}

fn protected_refusal(code: &str, message: &str) -> ToolResult {
    ToolResult::error(message).with_structured(serde_json::json!({
        "status": "refused",
        "refusal": {
            "code": code,
            "message": message,
        }
    }))
}

fn is_existing_profile_prepare(tool_name: &str, args: &Value) -> bool {
    tool_name == "browser_prepare"
        && args
            .get("strategy")
            .and_then(|strategy| strategy.get("kind"))
            .and_then(Value::as_str)
            == Some("existing_profile")
}

fn recording_args_for(tool_name: &str, args: &Value) -> Value {
    let mut redacted = args.clone();
    if let Some(arguments) = redacted.as_object_mut() {
        match tool_name {
            "browser_prepare" => {
                arguments.remove("_transport_session_id");
            }
            "browser_dialog" => {
                if arguments.contains_key("prompt_text") {
                    arguments.insert(
                        "prompt_text".to_owned(),
                        Value::String("[redacted]".to_owned()),
                    );
                }
            }
            "clipboard_write" => {
                for field in ["text", "image_path", "file_path"] {
                    if arguments.contains_key(field) {
                        arguments.insert(field.to_owned(), Value::String("[redacted]".to_owned()));
                    }
                }
            }
            "browser_set_input_files" => {
                if let Some(count) = arguments
                    .get("files")
                    .and_then(Value::as_array)
                    .map(Vec::len)
                {
                    arguments.insert("files".to_owned(), serde_json::json!({ "count": count }));
                }
            }
            "browser_download" => {
                arguments.remove("_cua_browser_download_mcp_host_approved");
                if arguments.contains_key("destination_root") {
                    arguments.insert(
                        "destination_root".to_owned(),
                        Value::String("[redacted]".to_owned()),
                    );
                }
            }
            "browser_pointer" => {}
            _ => {}
        }
    }
    redacted
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Build a short, human-friendly label for the PiP overlay from the
/// tool name + raw args. Kept under ~60 chars so the macOS NSTextField
/// has room without truncation at default geometry.
fn synthesize_action_label(tool_name: &str, args: &Value) -> String {
    let arg = |k: &str| -> Option<String> {
        args.get(k).map(|v| match v {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        })
    };
    let summary = match tool_name {
        "click" | "double_click" | "right_click" => {
            if let Some(idx) = args.opt_u64("element_index") {
                format!("element_index={idx}")
            } else if let (Some(x), Some(y)) = (args.opt_f64("x"), args.opt_f64("y")) {
                format!("({x:.0}, {y:.0})")
            } else {
                "".into()
            }
        }
        "type_text" => {
            let text = arg("text").unwrap_or_default();
            let trimmed: String = text.chars().take(40).collect();
            if text.chars().count() > 40 {
                format!("\"{trimmed}…\"")
            } else {
                format!("\"{trimmed}\"")
            }
        }
        "press_key" | "hotkey" => arg("key").or_else(|| arg("keys")).unwrap_or_default(),
        "scroll" => format!(
            "dx={} dy={}",
            arg("dx").unwrap_or_else(|| "0".into()),
            arg("dy").unwrap_or_else(|| "0".into())
        ),
        "drag" => "drag".into(),
        "set_value" => arg("value").unwrap_or_default(),
        "launch_app" => arg("bundle_id").or_else(|| arg("name")).unwrap_or_default(),
        _ => String::new(),
    };
    if summary.is_empty() {
        tool_name.to_owned()
    } else {
        format!("{tool_name}: {summary}")
    }
}

#[cfg(test)]
mod capability_tests {
    //! Unit tests for the per-tool `capabilities` array and the
    //! top-level `capability_version` exposed in `tools/list`.
    //! These belong in cua-driver-core because they cover the shape
    //! of the registry response — no platform code involved.
    use super::*;

    #[test]
    fn browser_prepare_recording_redacts_transport_identity() {
        let recorded = recording_args_for(
            "browser_prepare",
            &serde_json::json!({
                "pid": 42,
                "window_id": 7,
                "session": "public-session",
                "strategy": {"kind": "existing_profile"},
                "_transport_session_id": "private-transport",
            }),
        );
        assert!(recorded.get("_transport_session_id").is_none());
        assert_eq!(recorded["pid"], 42);
        assert_eq!(recorded["window_id"], 7);
        assert_eq!(recorded["strategy"]["kind"], "existing_profile");
    }

    #[test]
    fn browser_sensitive_recording_args_are_path_and_text_free() {
        let dialog = recording_args_for(
            "browser_dialog",
            &serde_json::json!({"action": "accept", "prompt_text": "private reply"}),
        );
        assert_eq!(dialog["prompt_text"], "[redacted]");

        let upload = recording_args_for(
            "browser_set_input_files",
            &serde_json::json!({"files": ["/private/one", "/private/two"]}),
        );
        assert_eq!(upload["files"], serde_json::json!({"count": 2}));

        let download = recording_args_for(
            "browser_download",
            &serde_json::json!({
                "destination_root": "/private/destination",
                "_cua_browser_download_mcp_host_approved": true,
            }),
        );
        assert_eq!(download["destination_root"], "[redacted]");
        assert!(download
            .get("_cua_browser_download_mcp_host_approved")
            .is_none());

        let serialized = serde_json::json!([dialog, upload, download]).to_string();
        for forbidden in [
            "private reply",
            "/private/one",
            "/private/two",
            "/private/destination",
        ] {
            assert!(
                !serialized.contains(forbidden),
                "recording leaked {forbidden}"
            );
        }
    }

    #[test]
    fn clipboard_write_recording_args_are_content_free() {
        let recorded = recording_args_for(
            "clipboard_write",
            &serde_json::json!({
                "text": "private text",
                "image_path": "/private/image.png",
                "file_path": "/private/document.pdf",
                "session": "public-session",
            }),
        );
        assert_eq!(recorded["text"], "[redacted]");
        assert_eq!(recorded["image_path"], "[redacted]");
        assert_eq!(recorded["file_path"], "[redacted]");
        assert_eq!(recorded["session"], "public-session");
        let serialized = recorded.to_string();
        for forbidden in [
            "private text",
            "/private/image.png",
            "/private/document.pdf",
        ] {
            assert!(!serialized.contains(forbidden));
        }
    }

    #[test]
    fn existing_profile_prepare_is_a_private_consent_turn() {
        assert!(is_existing_profile_prepare(
            "browser_prepare",
            &serde_json::json!({"strategy": {"kind": "existing_profile"}}),
        ));
        assert!(!is_existing_profile_prepare(
            "browser_prepare",
            &serde_json::json!({"profile": {"mode": "isolated_new"}}),
        ));
        assert!(!is_existing_profile_prepare(
            "get_browser_state",
            &serde_json::json!({"strategy": {"kind": "existing_profile"}}),
        ));
    }

    /// Tools whose `default_capabilities_for` mapping must NOT be
    /// empty. Mirrors the documented vocabulary above. Lives here
    /// rather than in an integration test because adding a new tool
    /// without a capability claim should fail at unit-test time, not
    /// only when someone runs the platform-specific integration
    /// suite.
    const TOOLS_REQUIRING_CAPABILITIES: &[&str] = &[
        // pointer
        "click",
        "double_click",
        "right_click",
        "drag",
        "scroll",
        "move_cursor",
        "mouse_button_down",
        "mouse_button_up",
        "mouse_drag",
        "parallel_mouse_drag",
        // keyboard
        "type_text",
        "type_text_chars",
        "press_key",
        "hotkey",
        "set_value",
        // screen
        "zoom",
        "get_screen_size",
        "get_desktop_state",
        "get_cursor_position",
        // accessibility
        "get_accessibility_tree",
        "get_window_state",
        // app / window
        "launch_app",
        "list_apps",
        "kill_app",
        "list_windows",
        "bring_to_front",
        "set_window_frame",
        "debug_window_info",
        // permissions / config
        "check_permissions",
        "get_config",
        "set_config",
        // sessions
        "start_session",
        "get_session",
        "list_sessions",
        "escalate_session",
        "get_session_state",
        "end_session",
        // agent cursor
        "set_agent_cursor_enabled",
        "set_agent_cursor_motion",
        "set_agent_cursor_theme",
        "get_agent_cursor_state",
        // recording / replay
        "start_recording",
        "stop_recording",
        "get_recording_state",
        "replay_trajectory",
        "install_ffmpeg",
        // misc
        "page",
        "check_for_update",
        "probe",
        // browser-tool v1
        "get_browser_state",
        "browser_prepare",
        "browser_navigate",
        "browser_click",
        "browser_type",
        "browser_dialog",
        "browser_set_input_files",
        "browser_download",
        "browser_pointer",
        "history_status",
        "history_query",
    ];

    /// All capability tokens in the canonical vocabulary. Any token
    /// produced by `default_capabilities_for` MUST be in this set —
    /// catches typos and accidental ad-hoc extensions that would
    /// silently break consumers that match by token.
    const CANONICAL_VOCABULARY: &[&str] = &[
        // pointer
        "input.pointer.click",
        "input.pointer.click.left",
        "input.pointer.click.right",
        "input.pointer.click.double",
        "input.pointer.drag",
        "input.pointer.scroll",
        "input.pointer.move",
        "input.pointer.button",
        // keyboard
        "input.keyboard.type",
        "input.keyboard.type.terminal_safe",
        "input.keyboard.hotkey",
        "input.keyboard.press",
        "input.delivery_mode",
        // screen
        "screen.capture",
        "screen.capture.window",
        "screen.capture.region",
        "screen.dimensions",
        "screen.cursor.position",
        // accessibility
        "accessibility.tree",
        "accessibility.tree.structured",
        "accessibility.tree.bounded",
        "accessibility.window_state",
        // Surface 6 — claimed by tools that accept the opaque
        // `element_token` arg + get_window_state which emits them.
        "accessibility.element_tokens",
        // app / window
        "app.launch",
        "app.list",
        "app.kill",
        "window.list",
        "window.activate",
        "window.frame.set",
        "window.debug_info",
        // permissions
        "system.permissions.tcc",
        "system.permissions.tcc.accessibility",
        "system.permissions.tcc.screen_recording",
        // config
        "system.config.read",
        "system.config.write",
        // sessions
        "session.lifecycle.start",
        "session.lifecycle.read",
        "session.lifecycle.list",
        "session.lifecycle.end",
        "session.capture_scope",
        "session.capture_scope.read",
        "session.capture_scope.escalate",
        // agent cursor
        "agent_cursor.move",
        "agent_cursor.set_enabled",
        "agent_cursor.set_motion",
        "agent_cursor.set_theme",
        "agent_cursor.state",
        // recording
        "recording.start",
        "recording.stop",
        "recording.state",
        "recording.replay",
        "recording.install_dependency",
        // page
        "page.action",
        // browser-tool v1
        "browser.state",
        "browser.prepare",
        "browser.navigate",
        "browser.input.click",
        "browser.input.type",
        "browser.input.files",
        "browser.dialog",
        "browser.download",
        "browser.input.pointer",
        // driver self
        "driver.update_check",
        "driver.probe",
        // encrypted local history
        "history.status",
        "history.query",
    ];

    #[test]
    fn every_known_tool_has_at_least_one_capability() {
        for name in TOOLS_REQUIRING_CAPABILITIES {
            let caps = default_capabilities_for(name);
            assert!(
                !caps.is_empty(),
                "tool {name:?} must claim at least one capability — \
                 add it to default_capabilities_for() or remove it \
                 from TOOLS_REQUIRING_CAPABILITIES"
            );
        }
    }

    #[test]
    fn every_known_tool_has_reviewed_risk_metadata() {
        for name in TOOLS_REQUIRING_CAPABILITIES {
            let risk = crate::authorization::advertised_risk_for(name);
            assert_ne!(
                risk.class,
                crate::authorization::RiskClass::Unclassified,
                "tool {name:?} must have a reviewed risk classification"
            );
        }
    }

    #[test]
    fn every_claimed_capability_is_in_the_canonical_vocabulary() {
        let vocab: std::collections::HashSet<&str> = CANONICAL_VOCABULARY.iter().copied().collect();
        for name in TOOLS_REQUIRING_CAPABILITIES {
            for cap in default_capabilities_for(name) {
                assert!(
                    vocab.contains(cap.as_str()),
                    "tool {name:?} claims unknown capability {cap:?} — \
                     either add {cap:?} to CANONICAL_VOCABULARY or fix \
                     the typo in default_capabilities_for()"
                );
            }
        }
    }

    #[test]
    fn capability_version_is_string_one() {
        // Bumping this constant in a non-breaking PR is an error —
        // the version is the contract version, not the build version.
        // Pinned to "1" until we ship a BREAKING vocabulary change.
        assert_eq!(CAPABILITY_VERSION, "1");
    }

    #[test]
    fn delivery_mode_capability_is_derived_from_the_runtime_schema() {
        let with_delivery_mode = serde_json::json!({
            "type": "object",
            "properties": {
                "delivery_mode": crate::tool_schema::delivery_mode_schema()
            }
        });
        let without_delivery_mode = serde_json::json!({"type": "object", "properties": {}});

        assert!(
            advertised_capabilities_for("press_key", &with_delivery_mode)
                .iter()
                .any(|capability| capability == "input.delivery_mode")
        );
        assert!(
            !advertised_capabilities_for("press_key", &without_delivery_mode)
                .iter()
                .any(|capability| capability == "input.delivery_mode")
        );
    }

    #[test]
    fn delivery_mode_normalization_is_schema_gated_modern_first_and_fail_closed() {
        let with_delivery_mode = super::ToolDef {
            name: "click".into(),
            description: "test input".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "delivery_mode": crate::tool_schema::delivery_mode_schema()
                }
            }),
            read_only: false,
            destructive: false,
            idempotent: false,
            open_world: false,
        };
        let without_delivery_mode = super::ToolDef {
            name: "other".into(),
            description: "unrelated tool".into(),
            input_schema: serde_json::json!({"type": "object", "properties": {}}),
            read_only: false,
            destructive: false,
            idempotent: false,
            open_world: false,
        };

        for (mut args, expected) in [
            (serde_json::json!({"dispatch": "foreground"}), "foreground"),
            (serde_json::json!({"dispatch": "Foreground"}), "foreground"),
            (serde_json::json!({"dispatch": "background"}), "background"),
            (serde_json::json!({"dispatch": "auto"}), "background"),
            (serde_json::json!({"dispatch": "unknown"}), "background"),
            (serde_json::json!({"dispatch": null}), "background"),
            (
                serde_json::json!({"delivery_mode": "Foreground"}),
                "foreground",
            ),
            (
                serde_json::json!({"delivery_mode": "unknown"}),
                "background",
            ),
            (serde_json::json!({"delivery_mode": null}), "background"),
            (
                serde_json::json!({
                    "delivery_mode": "background",
                    "dispatch": "foreground"
                }),
                "background",
            ),
            (
                serde_json::json!({
                    "delivery_mode": null,
                    "dispatch": "foreground"
                }),
                "background",
            ),
        ] {
            super::normalize_delivery_mode_args(&with_delivery_mode, &mut args);
            assert_eq!(args["delivery_mode"], expected, "arguments: {args}");
            assert!(args.get("dispatch").is_none(), "arguments: {args}");
        }

        let mut absent = serde_json::json!({"x": 1});
        super::normalize_delivery_mode_args(&with_delivery_mode, &mut absent);
        assert_eq!(absent, serde_json::json!({"x": 1}));

        let mut unrelated = serde_json::json!({"dispatch": "foreground"});
        super::normalize_delivery_mode_args(&without_delivery_mode, &mut unrelated);
        assert_eq!(unrelated, serde_json::json!({"dispatch": "foreground"}));
    }

    #[test]
    fn unknown_tools_get_empty_capabilities() {
        // Tools without a mapping (typically internal/stub tools like
        // `unsupported_platform`) return `[]`. Consumers fall back to
        // name-matching for those, which is fine — they were never
        // load-bearing for capability routing.
        assert!(default_capabilities_for("unsupported_platform").is_empty());
        assert!(default_capabilities_for("totally_made_up_tool").is_empty());
    }

    fn dummy_def(name: &str) -> ToolDef {
        ToolDef {
            name: name.into(),
            description: format!("{name} (test)"),
            input_schema: serde_json::json!({"type":"object"}),
            read_only: false,
            destructive: false,
            idempotent: false,
            open_world: false,
        }
    }

    /// Surface 6: every tool that accepts the opaque `element_token`
    /// arg must claim the `accessibility.element_tokens` capability so
    /// Hermes/Codex/Claude Code consumers can branch on the capability
    /// token rather than coupling to tool names. Same set as the
    /// per-platform schema additions in this PR — keep the two lists
    /// in sync when a new element-targeting tool ships.
    #[test]
    fn every_token_accepting_tool_claims_element_tokens_capability() {
        const TOKEN_TOOLS: &[&str] = &[
            "click",
            "double_click",
            "right_click",
            "scroll",
            "type_text",
            "type_text_chars",
            "press_key",
            "set_value",
            // get_window_state emits the tokens — same capability
            // claim, from the other side of the contract.
            "get_window_state",
        ];
        for name in TOKEN_TOOLS {
            let caps = default_capabilities_for(name);
            assert!(
                caps.iter().any(|c| c == "accessibility.element_tokens"),
                "tool {name:?} accepts element_token but is missing the \
                 accessibility.element_tokens capability claim — add it \
                 in default_capabilities_for()"
            );
        }
    }

    #[test]
    fn to_list_entry_includes_capabilities_array_for_a_known_tool() {
        let def = dummy_def("click");
        let entry = def.to_list_entry();
        let caps = entry
            .get("capabilities")
            .and_then(|v| v.as_array())
            .expect("capabilities must be an array");
        assert!(
            !caps.is_empty(),
            "click must claim at least one capability via default_capabilities_for"
        );
        // Specifically: click claims the `input.pointer.click.left`
        // family — that's the contract Hermes' cua_backend.py is
        // expected to dispatch on once this surface is wired up.
        let cap_strs: Vec<&str> = caps.iter().filter_map(|v| v.as_str()).collect();
        assert!(
            cap_strs.contains(&"input.pointer.click"),
            "click missing input.pointer.click: {cap_strs:?}"
        );
        assert!(
            cap_strs.contains(&"input.pointer.click.left"),
            "click missing input.pointer.click.left: {cap_strs:?}"
        );
    }

    #[test]
    fn to_list_entry_advertises_delivery_mode_only_when_accepted() {
        let mut accepting = dummy_def("press_key");
        accepting.input_schema = serde_json::json!({
            "type": "object",
            "properties": {
                "delivery_mode": crate::tool_schema::delivery_mode_schema()
            }
        });
        let accepting_entry = accepting.to_list_entry();
        assert!(accepting_entry["capabilities"]
            .as_array()
            .expect("capabilities array")
            .iter()
            .any(|capability| capability == "input.delivery_mode"));

        let rejecting_entry = dummy_def("press_key").to_list_entry();
        assert!(!rejecting_entry["capabilities"]
            .as_array()
            .expect("capabilities array")
            .iter()
            .any(|capability| capability == "input.delivery_mode"));
    }

    #[test]
    fn to_list_entry_includes_versioned_risk_metadata() {
        let entry = dummy_def("browser_prepare").to_list_entry();
        assert_eq!(entry["risk"]["class"], "r2");
        assert_eq!(entry["risk"]["enforcement"], "active");
        assert_eq!(entry["risk"]["operation_sensitive"], true);
        assert_eq!(entry["risk"]["version"], "1");
    }

    #[test]
    fn to_list_entry_includes_empty_capabilities_array_for_unknown_tool() {
        // Even when no capabilities are claimed, the field is still
        // present — consumers can rely on the key existing.
        let def = dummy_def("totally_made_up_tool");
        let entry = def.to_list_entry();
        let caps = entry
            .get("capabilities")
            .and_then(|v| v.as_array())
            .expect("capabilities must be present even if empty");
        assert!(caps.is_empty());
    }

    #[test]
    fn to_list_entry_preserves_existing_fields() {
        // Regression guard for the additive-only contract: every
        // pre-existing key in the response must still be there.
        let def = ToolDef {
            name: "click".into(),
            description: "Click an element.".into(),
            input_schema: serde_json::json!({"type":"object","properties":{}}),
            read_only: false,
            destructive: true,
            idempotent: false,
            open_world: true,
        };
        let entry = def.to_list_entry();
        // Keys old consumers (Swift Hermes, the .NET driver, etc.)
        // already read — must still be present.
        assert_eq!(entry["name"], "click");
        assert_eq!(entry["description"], "Click an element.");
        assert!(entry["inputSchema"].is_object());
        assert_eq!(entry["annotations"]["readOnlyHint"], false);
        assert_eq!(entry["annotations"]["destructiveHint"], true);
        assert_eq!(entry["annotations"]["idempotentHint"], false);
        assert_eq!(entry["annotations"]["openWorldHint"], true);
        // New key — the whole point of this PR.
        assert!(entry["capabilities"].is_array());
    }

    fn action_tool_entry(name: &str) -> serde_json::Value {
        ToolDef {
            name: name.into(),
            description: "Action.".into(),
            input_schema: serde_json::json!({"type":"object","properties":{}}),
            read_only: false,
            destructive: false,
            idempotent: false,
            open_world: true,
        }
        .to_list_entry()
    }

    #[test]
    fn action_tools_advertise_the_same_closed_output_schema() {
        let expected =
            <cua_driver_contract::ActionResult as cua_driver_contract::ToolOutput>::output_schema();
        for name in ["click", "browser_click", "browser_pointer", "browser_type"] {
            let entry = action_tool_entry(name);
            // The success variant is unchanged and still closed; it now sits
            // beside the refusal envelope instead of standing alone.
            let success = &entry["outputSchema"]["anyOf"][0];
            assert_eq!(entry["outputSchema"]["type"], "object", "{name}");
            assert_eq!(success, &expected, "{name}");
            assert_eq!(success["additionalProperties"], false, "{name}");
        }
    }

    /// Regression: refusals must validate against the advertised schema.
    ///
    /// Both payloads below are verbatim captures from a live `cua-driver mcp`
    /// stdio session. Advertising only the success shape made strict MCP
    /// clients reject them with `-32602`, which discarded the refusal message
    /// the driver had put in `content` and left agents with no signal to
    /// re-snapshot.
    #[test]
    fn advertised_output_schema_accepts_live_refusal_payloads() {
        let schema = &action_tool_entry("click")["outputSchema"];
        let compiled = jsonschema::validator_for(schema).expect("advertised schema compiles");

        let stale_element_token = serde_json::json!({
            "refusal": {
                "code": "stale_element_token",
                "message": "element_token is stale; call get_window_state again to refresh"
            },
            "status": "refused"
        });
        let window_target_not_found = serde_json::json!({
            "candidates": [],
            "code": "window_target_not_found",
            "effect": "refused",
            "pid": 999_999
        });
        let success = serde_json::json!({
            "delivery": {"mode": "background"},
            "effect": "unverifiable",
            "route": "accessibility"
        });

        for payload in [&stale_element_token, &window_target_not_found, &success] {
            assert!(
                compiled.is_valid(payload),
                "advertised schema rejected {payload}"
            );
        }

        // The success variant stays strict: an unknown key is still a bug, and
        // must not be laundered through the permissive refusal variant.
        assert!(
            !compiled.is_valid(&serde_json::json!({
                "effect": "confirmed",
                "route": "accessibility",
                "bogus_key": true
            })),
            "a malformed success payload must not validate"
        );
    }

    #[test]
    fn typed_non_action_tools_advertise_outputs_without_inventing_runtime_schemas() {
        let verify = ToolDef {
            name: "verify_state".into(),
            description: "Verify.".into(),
            input_schema: serde_json::json!({"type":"object","properties":{}}),
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        }
        .to_list_entry();
        assert!(verify["outputSchema"].is_object());

        let runtime_only = ToolDef {
            name: "runtime_only_probe".into(),
            description: "Probe.".into(),
            input_schema: serde_json::json!({"type":"object","properties":{}}),
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        }
        .to_list_entry();
        assert!(runtime_only.get("outputSchema").is_none());
    }

    #[test]
    fn list_windows_advertises_the_shared_z_index_contract() {
        let entry = ToolDef {
            name: "list_windows".into(),
            description: "List windows.".into(),
            input_schema: serde_json::json!({"type":"object","properties":{}}),
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        }
        .to_list_entry();
        // anyOf[0] is the success variant; anyOf[1] is the refusal envelope.
        let z_index = &entry["outputSchema"]["anyOf"][0]["properties"]["windows"]["items"]
            ["properties"]["z_index"];
        assert_eq!(z_index["type"], serde_json::json!(["integer", "null"]));
        assert!(z_index["description"]
            .as_str()
            .expect("description")
            .contains("Higher values are closer to the front"));
    }

    #[test]
    fn type_text_claims_terminal_safe_capability() {
        // The terminal-emulator fallback shipped per platform must be
        // discoverable as a capability so consumers can pick `type_text`
        // confidently over `type_text_chars` (whose Linux implementation
        // is the bare per-char XSendEvent path, with no terminal
        // short-circuit). Freezing the token name here makes a
        // future-PR rename a hard test failure.
        let caps = default_capabilities_for("type_text");
        let cap_strs: Vec<&str> = caps.iter().map(String::as_str).collect();
        assert!(
            cap_strs.contains(&"input.keyboard.type"),
            "type_text must keep the base capability: {cap_strs:?}"
        );
        assert!(
            cap_strs.contains(&"input.keyboard.type.terminal_safe"),
            "type_text must claim terminal_safe (PR additive surface): {cap_strs:?}"
        );
    }

    #[test]
    fn type_text_chars_does_not_claim_terminal_safe() {
        // The contract is intentionally narrower: type_text_chars on
        // Linux uses a per-character XSendEvent path that does not
        // route past the AT-SPI/value channel on terminals. Tightening
        // this gate prevents a future drive-by edit from over-claiming.
        let caps = default_capabilities_for("type_text_chars");
        let cap_strs: Vec<&str> = caps.iter().map(String::as_str).collect();
        assert!(
            !cap_strs.contains(&"input.keyboard.type.terminal_safe"),
            "type_text_chars must NOT claim terminal_safe: {cap_strs:?}"
        );
    }

    #[test]
    fn tools_list_top_level_envelope_has_capability_and_schema_versions() {
        // An empty registry still emits both version fields so
        // consumers don't have to special-case the bootstrap window
        // between server start and first tool register.
        let reg = ToolRegistry::new();
        let v = reg.tools_list();
        assert_eq!(v["capability_version"], "1");
        assert_eq!(v["schema_version"], "1");
        assert!(v["tools"].is_array(), "tools array must still be present");
        assert_eq!(v["tools"].as_array().unwrap().len(), 0);
        assert!(
            v["enforcement_adapters"].is_array(),
            "permission enforcement inventory must be available before tools register"
        );
        assert!(v["enforcement_adapters"]
            .as_array()
            .unwrap()
            .iter()
            .any(|adapter| {
                adapter["id"] == "browser_prepare.existing_profile" && adapter["state"] == "active"
            }));
    }
}
