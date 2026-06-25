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
        GetRecordingStateTool, ReplayTrajectoryTool, StartRecordingTool,
        StopRecordingTool,
        init_replay_registry,
    },
    tool_args::ArgsExt,
};

/// MCP `tools/list` capability-vocabulary version. Bumped on BREAKING
/// changes only (renaming a capability token, removing a capability
/// claim from a tool). Additive changes — new capability tokens, new
/// tools, new tools that newly claim an existing token — keep the
/// version. Downstream consumers (Hermes, Codex) read this to gate
/// strict-vs-tolerant capability matching. See
/// `default_capabilities_for` for the live vocabulary.
pub const CAPABILITY_VERSION: &str = "1";

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
    pub fn to_list_entry(&self) -> Value {
        // `capabilities` is always emitted (even when empty) so consumers
        // can rely on the key existing. Additive only — old consumers
        // that ignore the field keep working unchanged.
        //
        // Capabilities are resolved from the centralised
        // `default_capabilities_for` name → tokens map. Keeping the
        // mapping in one place — rather than scattered across every
        // per-platform tool literal — means adding a new capability
        // claim is a one-file change, and sibling PRs touching
        // individual tool files don't conflict with this surface.
        let caps = default_capabilities_for(&self.name);
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
/// - `session.lifecycle.start`, `session.lifecycle.end`
/// - `agent_cursor.move`, `agent_cursor.set_enabled`,
///   `agent_cursor.set_motion`, `agent_cursor.set_style`,
///   `agent_cursor.state`
/// - `recording.start`, `recording.stop`, `recording.state`,
///   `recording.replay`, `recording.install_dependency`
/// - `page.action`
/// - `driver.update_check`, `driver.probe`
///
/// Tools with no entry get `[]` — that's fine, it just means
/// downstream consumers fall back to matching by tool name for them.
pub fn default_capabilities_for(tool_name: &str) -> Vec<String> {
    let caps: &[&str] = match tool_name {
        // ── input.pointer ────────────────────────────────────────────
        //
        // Surface 6: tools that accept the opaque `element_token` arg
        // (in addition to the integer `element_index`) claim the
        // `accessibility.element_tokens` token so consumers can branch
        // on its presence — Hermes' wrapper currently does this by name
        // for each tool; the capability token removes that coupling.
        "click" => &[
            "input.pointer.click",
            "input.pointer.click.left",
            "accessibility.element_tokens",
        ],
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
        "drag" => &["input.pointer.drag"],
        "mouse_drag" => &["input.pointer.drag"],
        "parallel_mouse_drag" => &["input.pointer.drag"],
        "mouse_button_down" => &["input.pointer.button"],
        "mouse_button_up" => &["input.pointer.button"],
        "scroll" => &[
            "input.pointer.scroll",
            "accessibility.element_tokens",
        ],
        "move_cursor" => &[
            // Visual overlay move, not a real OS pointer move on
            // macOS/Windows — see SKILL.md. Surfaced as
            // `agent_cursor.move` because the canonical name on the
            // overlay side is "agent cursor"; `input.pointer.move` is
            // intentionally omitted to avoid claiming we shift the
            // real cursor.
            "agent_cursor.move",
        ],

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
        "type_text" => &[
            "input.keyboard.type",
            "input.keyboard.type.terminal_safe",
            "accessibility.element_tokens",
        ],
        // `type_text_chars` is a deprecated alias resolved at invoke
        // time on macOS/Windows. On Linux it's still registered (see
        // platform-linux/impl_.rs). The Linux implementation runs
        // XSendEvent per-character without the terminal short-circuit,
        // so we deliberately do NOT claim `terminal_safe` here — the
        // contract is intentionally narrower than `type_text`'s. It
        // still accepts `element_token`, hence the tokens claim.
        "type_text_chars" => &[
            "input.keyboard.type",
            "accessibility.element_tokens",
        ],
        "press_key" => &[
            "input.keyboard.press",
            "accessibility.element_tokens",
        ],
        "hotkey" => &["input.keyboard.hotkey"],
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
        "get_screen_size" => &["screen.dimensions"],
        "get_desktop_state" => &["screen.capture", "screen.dimensions"],
        "get_cursor_position" => &["screen.cursor.position"],

        // ── accessibility / window state ─────────────────────────────
        "get_accessibility_tree" => &[
            "accessibility.tree",
            "accessibility.tree.structured",
        ],
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

        // ── sessions ─────────────────────────────────────────────────
        "start_session" => &["session.lifecycle.start"],
        "end_session" => &["session.lifecycle.end"],

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

        // ── driver self-service ──────────────────────────────────────
        "check_for_update" => &["driver.update_check"],
        "probe" => &["driver.probe"],

        // ── unsupported_platform stub & anything else ────────────────
        _ => &[],
    };
    caps.iter().map(|s| (*s).to_owned()).collect()
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
        use crate::session_tools::{EndSessionTool, StartSessionTool};
        self.register(Box::new(StartSessionTool));
        self.register(Box::new(EndSessionTool));
    }

    /// Wire up the replay tool's weak self-reference.
    /// Call this once, immediately after `Arc::new(registry)`.
    pub fn init_self_weak(self: &Arc<Self>) {
        init_replay_registry(Arc::downgrade(self));
    }

    pub fn tools_list(&self) -> Value {
        let list: Vec<Value> =
            self.order.iter().filter_map(|n| self.tools.get(n)).map(|t| t.def().to_list_entry()).collect();
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
            "schema_version": "1",
        })
    }

    /// Iterate over (name, &ToolDef) in registration order.
    pub fn iter_defs(&self) -> impl Iterator<Item = (&str, &ToolDef)> {
        self.order.iter().filter_map(move |n| {
            self.tools.get(n).map(|t| (n.as_str(), t.def()))
        })
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
        // Capture start time for recording timestamps.
        let start_ms = now_ms();

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

        let result = match self.tools.get(resolved_name) {
            Some(tool) => tool.invoke(args.clone()).await,
            None => return ToolResult::error(format!("Unknown tool: {name}")),
        };
        // Use the original name for downstream code paths below so the
        // exit-code matching and recording paths keep treating the alias
        // as a distinct call site.
        let name = resolved_name;

        // Record non-read-only, non-recording tool calls. The recording-
        // control tools themselves are excluded so the recorded turn
        // stream stays the actual user-action sequence (not the meta
        // start/stop frames).
        let should_record = self.tools.get(name)
            .map(|t| !t.def().read_only)
            .unwrap_or(false)
            && !matches!(
                name,
                "start_recording" | "stop_recording" | "get_recording_state" | "replay_trajectory"
            );

        if should_record {
            let result_text = result.content.iter()
                .find_map(|c| {
                    if let Content::Text { text, .. } = c { Some(text.as_str()) }
                    else { None }
                })
                .unwrap_or("");
            self.recording.record(name, &args, result_text, start_ms);
        }

        // Experimental PiP push — only when --experimental-pip is on argv
        // (otherwise `pip_enabled()` is false and we skip the screenshot
        // entirely to avoid wasted capture work). We push for the same set
        // of action tools the recording pipeline cares about (non-read-only,
        // not the recording-control meta-tools) so the live view matches
        // what the recorder would have captured for the turn.
        if pip_hook::pip_enabled() && should_record {
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

    /// Tools whose `default_capabilities_for` mapping must NOT be
    /// empty. Mirrors the documented vocabulary above. Lives here
    /// rather than in an integration test because adding a new tool
    /// without a capability claim should fail at unit-test time, not
    /// only when someone runs the platform-specific integration
    /// suite.
    const TOOLS_REQUIRING_CAPABILITIES: &[&str] = &[
        // pointer
        "click", "double_click", "right_click", "drag", "scroll",
        "move_cursor", "mouse_button_down", "mouse_button_up",
        "mouse_drag", "parallel_mouse_drag",
        // keyboard
        "type_text", "type_text_chars", "press_key", "hotkey", "set_value",
        // screen
        "zoom", "get_screen_size", "get_desktop_state",
        "get_cursor_position",
        // accessibility
        "get_accessibility_tree", "get_window_state",
        // app / window
        "launch_app", "list_apps", "kill_app", "list_windows",
        "bring_to_front", "debug_window_info",
        // permissions / config
        "check_permissions", "get_config", "set_config",
        // sessions
        "start_session", "end_session",
        // agent cursor
        "set_agent_cursor_enabled", "set_agent_cursor_motion",
        "set_agent_cursor_style", "get_agent_cursor_state",
        // recording / replay
        "start_recording", "stop_recording", "get_recording_state",
        "replay_trajectory", "install_ffmpeg",
        // misc
        "page", "check_for_update", "probe",
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
    fn every_claimed_capability_is_in_the_canonical_vocabulary() {
        let vocab: std::collections::HashSet<&str> =
            CANONICAL_VOCABULARY.iter().copied().collect();
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
        let caps = entry.get("capabilities")
            .and_then(|v| v.as_array())
            .expect("capabilities must be an array");
        assert!(!caps.is_empty(),
            "click must claim at least one capability via default_capabilities_for");
        // Specifically: click claims the `input.pointer.click.left`
        // family — that's the contract Hermes' cua_backend.py is
        // expected to dispatch on once this surface is wired up.
        let cap_strs: Vec<&str> =
            caps.iter().filter_map(|v| v.as_str()).collect();
        assert!(cap_strs.contains(&"input.pointer.click"),
            "click missing input.pointer.click: {cap_strs:?}");
        assert!(cap_strs.contains(&"input.pointer.click.left"),
            "click missing input.pointer.click.left: {cap_strs:?}");
    }

    #[test]
    fn to_list_entry_includes_empty_capabilities_array_for_unknown_tool() {
        // Even when no capabilities are claimed, the field is still
        // present — consumers can rely on the key existing.
        let def = dummy_def("totally_made_up_tool");
        let entry = def.to_list_entry();
        let caps = entry.get("capabilities")
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
    }
}
