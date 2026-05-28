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
        serde_json::json!({
            "name": self.name,
            "description": self.description,
            "inputSchema": self.input_schema,
            "annotations": {
                "readOnlyHint": self.read_only,
                "destructiveHint": self.destructive,
                "idempotentHint": self.idempotent,
                "openWorldHint": self.open_world,
            }
        })
    }
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
    }

    /// Wire up the replay tool's weak self-reference.
    /// Call this once, immediately after `Arc::new(registry)`.
    pub fn init_self_weak(self: &Arc<Self>) {
        init_replay_registry(Arc::downgrade(self));
    }

    pub fn tools_list(&self) -> Value {
        let list: Vec<Value> =
            self.order.iter().filter_map(|n| self.tools.get(n)).map(|t| t.def().to_list_entry()).collect();
        serde_json::json!({ "tools": list })
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
