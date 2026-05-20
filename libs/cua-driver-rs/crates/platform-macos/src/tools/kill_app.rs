//! macOS `kill_app` tool — force-terminate a process by pid via SIGKILL.
//!
//! Cross-platform mirror of the Windows `kill_app` tool (which calls
//! `TerminateProcess`). On macOS the canonical force-terminate is
//! `kill(pid, SIGKILL)` — same semantics as `kill -9 <pid>`.
//!
//! Used as the escalation path when the cooperative quit-app flow
//! (`hotkey cmd+q` / sending an `NSWorkspaceTerminate` Apple Event)
//! doesn't actually exit the target — e.g. an app that has caught the
//! quit handler and is showing a "save before quitting?" dialog the
//! agent can't interact with.

use async_trait::async_trait;
use mcp_server::{protocol::ToolResult, tool::{Tool, ToolDef}};
use serde_json::Value;

pub struct KillAppTool;

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "kill_app".into(),
        description:
            "Force-terminate a process by pid (kill -9 equivalent on macOS / Linux; \
             taskkill /F equivalent on Windows). Use as escalation when the cooperative \
             close path (hotkey cmd+q on macOS, click-the-X on Windows) failed to make \
             the process exit. Unsaved state is lost — prefer the cooperative path first."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["pid"],
            "properties": {
                "pid": { "type": "integer", "description": "PID of the process to terminate." }
            },
            "additionalProperties": false,
        }),
        read_only: false,
        destructive: true,
        idempotent: true,
        open_world: false,
    })
}

#[async_trait]
impl Tool for KillAppTool {
    fn def(&self) -> &ToolDef { def() }

    async fn invoke(&self, args: Value) -> ToolResult {
        let pid_i = match args.get("pid").and_then(|v| v.as_i64()) {
            Some(p) if p > 0 && p <= i32::MAX as i64 => p as i32,
            Some(_) => return ToolResult::error("kill_app: `pid` must be a positive integer".to_string()),
            None => return ToolResult::error("kill_app: missing required integer field `pid`".to_string()),
        };

        // libc::kill is fast and synchronous; no need to spawn_blocking.
        // Returns 0 on success, -1 on error (errno).
        // SAFETY: SIGKILL (9) is the standard force-kill signal; libc::kill
        // is a thin syscall wrapper with no thread-safety concerns.
        let rc = unsafe { libc::kill(pid_i, libc::SIGKILL) };
        if rc == 0 {
            ToolResult::text(format!("✅ Sent SIGKILL to pid {pid_i}."))
        } else {
            // Capture errno before any allocation that might overwrite it.
            let err = std::io::Error::last_os_error();
            ToolResult::error(format!(
                "kill_app: kill(pid={pid_i}, SIGKILL) failed: {err}. \
                 The process may not exist, or the daemon lacks permission to signal it \
                 (cross-user or root-owned processes will return EPERM)."
            ))
        }
    }
}
