//! Tool trait and registry.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::{
    pip_hook,
    protocol::{Content, ToolResult},
    recording::{now_ms, screenshot_for, RecordingSession},
    recording_tools::{
        init_replay_registry, GetRecordingStateTool, ReplayTrajectoryTool, StartRecordingTool,
        StopRecordingTool,
    },
    tool_args::ArgsExt,
};

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
        let caps = advertised_capabilities_for(&self.name, &self.input_schema);
        let risk = crate::authorization::risk_metadata_json(&self.name);
        serde_json::json!({
            "name": self.name,
            "description": self.description,
            "inputSchema": self.input_schema,
            "annotations": {
                "readOnlyHint": self.read_only,
                "destructiveHint": self.destructive,
                "idempotentHint": self.idempotent,
                "openWorldHint": self.open_world,
            },
            "capabilities": caps,
            "risk": risk,
        })
    }
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
///   `window.activate`, `window.debug_info`
/// - `system.permissions.tcc`,
///   `system.permissions.tcc.accessibility`,
///   `system.permissions.tcc.screen_recording`
/// - `system.config.read`, `system.config.write`
/// - `session.lifecycle.start`, `session.lifecycle.end`,
///   `session.capture_scope`, `session.capture_scope.read`,
///   `session.capture_scope.escalate`
/// - `agent_cursor.move`, `agent_cursor.set_enabled`,
///   `agent_cursor.set_motion`, `agent_cursor.set_style`,
///   `agent_cursor.state`
/// - `recording.start`, `recording.stop`, `recording.state`,
///   `recording.replay`, `recording.install_dependency`
/// - `page.action`
/// - `browser.state`, `browser.prepare`, `browser.navigate`,
///   `browser.input.click`, `browser.input.type`, `browser.input.files`,
///   `browser.dialog`
/// - `driver.update_check`, `driver.probe`
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
        "set_agent_cursor_style" => &["agent_cursor.set_style"],
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
    let accepts_delivery_mode = input_schema
        .pointer("/properties/delivery_mode")
        .is_some_and(Value::is_object);
    if accepts_delivery_mode
        && !capabilities
            .iter()
            .any(|capability| capability == "input.delivery_mode")
    {
        capabilities.push("input.delivery_mode".into());
    }
    capabilities
}

/// A callable tool handler. Object-safe — uses `Box<dyn Tool>`.
#[async_trait]
pub trait Tool: Send + Sync {
    fn def(&self) -> &ToolDef;
    async fn invoke(&self, args: Value) -> ToolResult;
}

/// Thread-safe collection of all registered tools.
pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
    /// Ordered list of tool names for `tools/list`.
    order: Vec<String>,
    /// Shared recording session — auto-records each non-read-only tool call.
    pub recording: Arc<RecordingSession>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
            order: Vec::new(),
            recording: Arc::new(RecordingSession::new()),
        }
    }

    pub fn register(&mut self, tool: Box<dyn Tool>) {
        let name = tool.def().name.clone();
        self.order.push(name.clone());
        self.tools.insert(name, tool);
    }

    /// Register the four platform-independent recording/replay tools.
    /// Call this after all platform tools have been registered.
    pub fn register_recording_tools(&mut self) {
        let session = self.recording.clone();
        self.register(Box::new(StartRecordingTool::new(session.clone())));
        self.register(Box::new(StopRecordingTool::new(session.clone())));
        self.register(Box::new(GetRecordingStateTool::new(session)));
        self.register(Box::new(ReplayTrajectoryTool));
        self.register(Box::new(crate::recording_tools::InstallFfmpegTool));
    }

    /// Register the platform-independent session-lifecycle tools
    /// (`start_session` / `end_session`). Call alongside
    /// `register_recording_tools` from each platform's `register_all`.
    pub fn register_session_tools(&mut self) {
        use crate::session_tools::{
            EndSessionTool, EscalateSessionTool, GetSessionStateTool, StartSessionTool,
        };
        self.register(Box::new(StartSessionTool));
        self.register(Box::new(EscalateSessionTool));
        self.register(Box::new(GetSessionStateTool));
        self.register(Box::new(EndSessionTool));
    }

    /// Wire up the replay tool's weak self-reference.
    /// Call this once, immediately after `Arc::new(registry)`.
    pub fn init_self_weak(self: &Arc<Self>) {
        init_replay_registry(Arc::downgrade(self));
    }

    pub fn tools_list(&self) -> Value {
        let list: Vec<Value> = self
            .order
            .iter()
            .filter_map(|n| self.tools.get(n))
            .map(|t| t.def().to_list_entry())
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

        // This registry is the canonical native dispatch boundary shared by
        // the same-process SDK and every transport adapter. Authorization must
        // live here: transport-only checks leave CuaDriver::create() able to
        // invoke platform tools without policy, permission-mode, hard-
        // invariant, or reviewed-risk enforcement.
        //
        // Transports may still reject earlier for defense in depth, but those
        // checks must remain side-effect free. Active consent/grant adapters
        // run only downstream of this boundary so a call cannot prompt twice.
        if let Err(error) = crate::authorization::authorize_tool_call(resolved_name, &args) {
            let message = error.to_string();
            return ToolResult::error(message.clone()).with_structured(serde_json::json!({
                "status": "refused",
                "refusal": {
                    "code": "permission_denied",
                    "message": message,
                }
            }));
        }

        // Reject modality violations before reserving a recording turn. A
        // rejected action has no before/after evidence and must not leave a
        // pending recorder entry behind.
        if let Err(violation) = crate::capture_scope::enforce_tool(resolved_name, &args) {
            let structured =
                violation.as_json(args.get("session").and_then(Value::as_str).unwrap_or(""));
            return ToolResult::error(violation.message).with_structured(structured);
        }

        // Capture start time for recording timestamps only after validation.
        let start_ms = now_ms();

        // Reserve and capture the turn before dispatch so recorded evidence
        // shows the application immediately before the action changed it.
        let should_record = !tool.def().read_only
            && !matches!(
                resolved_name,
                "start_recording" | "stop_recording" | "get_recording_state" | "replay_trajectory"
            );
        let private_consent_turn = is_existing_profile_prepare(resolved_name, &args);
        let recording_args = recording_args_for(resolved_name, &args);
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
        let validate_portable_output = match resolved_name {
            "get_desktop_state" | "get_screen_size" | "get_cursor_position" => true,
            "move_cursor" | "click" | "drag" | "scroll" | "type_text" | "press_key" | "hotkey" => {
                args.get("scope").and_then(Value::as_str) == Some("desktop")
            }
            _ => true,
        };
        if result.is_error != Some(true) && validate_portable_output {
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

        // Record non-read-only, non-recording tool calls. The recording-
        // control tools themselves are excluded so the recorded turn
        // stream stays the actual user-action sequence (not the meta
        // start/stop frames).
        if let Some(pending_turn) = pending_turn {
            let result_text = result
                .content
                .iter()
                .find_map(|c| {
                    if let Content::Text { text, .. } = c {
                        Some(text.as_str())
                    } else {
                        None
                    }
                })
                .unwrap_or("");
            self.recording.finish_turn(pending_turn, result_text);
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
                let label = synthesize_action_label(name, &args);
                pip_hook::push_pip_frame(pip_hook::PipHookFrame {
                    png_bytes,
                    action_label: label,
                    timestamp_ms: now_ms(),
                });
            }
        }

        result
    }
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
                if arguments.contains_key("approval_token") {
                    arguments.insert(
                        "approval_token".to_owned(),
                        Value::String("[redacted]".to_owned()),
                    );
                }
                arguments.remove("_cua_browser_prepare_mcp_host_approved");
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
    fn browser_prepare_recording_redacts_authority_and_transport_secrets() {
        let recorded = recording_args_for(
            "browser_prepare",
            &serde_json::json!({
                "pid": 42,
                "window_id": 7,
                "session": "public-session",
                "strategy": {"kind": "existing_profile"},
                "approval_token": "one-use-secret",
                "_cua_browser_prepare_mcp_host_approved": true,
                "_transport_session_id": "private-transport",
            }),
        );
        assert_eq!(recorded["approval_token"], "[redacted]");
        assert!(recorded
            .get("_cua_browser_prepare_mcp_host_approved")
            .is_none());
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
        "debug_window_info",
        // permissions / config
        "check_permissions",
        "get_config",
        "set_config",
        // sessions
        "start_session",
        "escalate_session",
        "get_session_state",
        "end_session",
        // agent cursor
        "set_agent_cursor_enabled",
        "set_agent_cursor_motion",
        "set_agent_cursor_style",
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
        "session.lifecycle.end",
        "session.capture_scope",
        "session.capture_scope.read",
        "session.capture_scope.escalate",
        // agent cursor
        "agent_cursor.move",
        "agent_cursor.set_enabled",
        "agent_cursor.set_motion",
        "agent_cursor.set_style",
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
        assert_eq!(entry["risk"]["enforcement"], "metadata_only");
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
