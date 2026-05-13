//! Tool trait and registry.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::{
    browser_eval::BrowserEvalTool,
    protocol::{Content, ToolResult},
    recording::{now_ms, RecordingSession},
    recording_tools::{
        GetRecordingStateTool, ReplayTrajectoryTool, SetRecordingTool,
        init_replay_registry,
    },
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

    /// Register the three platform-independent recording/replay tools.
    /// Call this after all platform tools have been registered.
    pub fn register_recording_tools(&mut self) {
        let session = self.recording.clone();
        self.register(Box::new(SetRecordingTool::new(session.clone())));
        self.register(Box::new(GetRecordingStateTool::new(session)));
        self.register(Box::new(ReplayTrajectoryTool));
        self.register(Box::new(BrowserEvalTool));
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

        // Record non-read-only, non-recording tool calls.
        let should_record = self.tools.get(name)
            .map(|t| !t.def().read_only)
            .unwrap_or(false)
            && !matches!(
                name,
                "set_recording" | "get_recording_state" | "replay_trajectory"
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

        result
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}
