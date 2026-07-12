//! The transport abstraction every test targets.

use crate::response::ToolResponse;
use serde_json::Value;

/// A way to invoke cua-driver tools. Implemented by [`crate::McpDriver`]
/// (long-lived server) and [`crate::CliDriver`] (stateless per-call process).
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
