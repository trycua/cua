//! `check_for_update` MCP tool — programmatic mirror of
//! `cua-driver check-update`.
//!
//! Lives in the `cua-driver` binary crate (not in `cua-driver-core` or the
//! per-platform `tools/` modules) for one reason: the implementation calls
//! into `version_check`, which pulls `ureq` + rustls. Putting that crypto
//! stack into `cua-driver-core` would propagate it into every platform
//! crate's dep graph and break cross-target builds from macOS host (ring's
//! C bits can't find MSVC headers without an MSVC toolchain installed).
//! Keeping it here, behind a thin registry-injection seam, means
//! `cua-driver` keeps its existing dep footprint while every platform's
//! tool table still exposes the new MCP tool.
//!
//! Registered by [`register_into`] from `main.rs` immediately after the
//! per-platform `register_tools()` call, so the result is identical to
//! the per-platform tools registering it themselves.

use async_trait::async_trait;
use serde_json::Value;

use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef, ToolRegistry},
};

pub struct CheckForUpdateTool;

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "check_for_update".into(),
        description: "Check whether a newer cua-driver-rs release is available on GitHub. \
             Returns the current and latest versions, an `update_available` boolean, \
             the install one-liner, and the release notes URL. Read-only — never \
             installs. Mirror of `cua-driver check-update --json`."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }),
        read_only: true,
        destructive: false,
        idempotent: true,
        // The check hits GitHub over the network, so the response can
        // change between invocations even though the tool is read-only
        // from the caller's perspective.
        open_world: true,
    })
}

#[async_trait]
impl Tool for CheckForUpdateTool {
    fn def(&self) -> &ToolDef {
        def()
    }

    async fn invoke(&self, _args: Value) -> ToolResult {
        // The fetch hits the network with a 4s timeout — run on a
        // blocking pool so the MCP server's tokio runtime keeps
        // multiplexing other tool calls during the round-trip.
        let state = tokio::task::spawn_blocking(|| crate::version_check::check_update_state(false))
            .await
            .expect("version_check::check_update_state never panics");

        let summary = if let Some(err) = &state.error {
            format!("Update check failed: {err}")
        } else if state.update_available {
            let latest = state.latest_version.as_deref().unwrap_or("?");
            format!(
                "Update available: cua-driver {latest} (you have {}).",
                state.current_version
            )
        } else {
            format!("Up to date (cua-driver {}).", state.current_version)
        };

        let structured = serde_json::to_value(&state).unwrap_or_else(|_| serde_json::json!({}));

        ToolResult::text(summary).with_structured(structured)
    }
}

/// Register the `check_for_update` tool into a freshly-built platform
/// registry. Call site lives in `main.rs`; this keeps the per-platform
/// `register_tools()` signature untouched.
pub fn register_into(registry: &mut ToolRegistry) {
    registry.register(Box::new(CheckForUpdateTool));
}
