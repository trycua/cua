//! The transport abstraction every test targets.

use crate::response::ToolResponse;
use serde_json::Value;

/// A way to invoke cua-driver tools. Implemented by [`crate::McpDriver`]
/// (long-lived proxy) and [`crate::CliDriver`] (one shell process per call).
///
/// Write scenarios against `Driver` to run them over either transport — the one
/// behavior that only surfaces across both is config persistence (`set_config`
/// is session-scoped over MCP but persists to disk over the CLI).
pub trait Driver {
    /// Invoke `tool` with `args`, returning the normalized response.
    fn call(&mut self, tool: &str, args: Value) -> ToolResponse;
}

/// Explicit lifecycle capability for canonical per-cell behavioral clips.
pub trait BehaviorRecording {
    fn start_behavior_recording(&mut self);
}

/// Harness scenarios read both `elements` and `tree_markdown` from
/// `get_window_state`. The tool's model-facing default is now one compact
/// markdown tree, so request the full response unless the scenario chose a
/// shape itself (`tree_format`, `full_output`, or `since`).
pub fn with_full_window_state(tool: &str, mut args: Value) -> Value {
    if tool == "get_window_state" {
        if let Some(map) = args.as_object_mut() {
            if !map.contains_key("tree_format")
                && !map.contains_key("full_output")
                && !map.contains_key("since")
            {
                map.insert("full_output".into(), Value::Bool(true));
            }
        }
    }
    args
}
