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
use cua_driver_core::{
    browser::platform::BrowserPlatform,
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use serde_json::Value;

pub struct KillAppTool;

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "kill_app".into(),
        description: "Force-terminate a process by pid (kill -9 equivalent on macOS / Linux; \
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
    fn def(&self) -> &ToolDef {
        def()
    }

    async fn protected_resource_scope(
        &self,
        adapter_id: &str,
        args: &Value,
    ) -> Result<Option<Value>, String> {
        if adapter_id != "process_control" {
            return Ok(None);
        }
        let pid = args
            .get("pid")
            .and_then(Value::as_i64)
            .filter(|pid| *pid > 0)
            .ok_or_else(|| "kill_app requires a positive integer pid".to_owned())?;
        let fingerprint = crate::browser::MacOsBrowserPlatform::default()
            .process_fingerprint(pid)
            .await
            .map_err(|error| error.message)?;
        Ok(Some(serde_json::json!({
            "kind": "process_instance",
            "fingerprint": fingerprint,
        })))
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        if let Some(expected) = args.get("_protected_process_fingerprint") {
            let current = match self
                .protected_resource_scope("process_control", &args)
                .await
            {
                Ok(Some(scope)) => scope["fingerprint"].clone(),
                Ok(None) => Value::Null,
                Err(message) => {
                    return stale_process_refusal(message);
                }
            };
            if current != *expected {
                return stale_process_refusal(
                    "the process identity changed at the termination boundary".to_owned(),
                );
            }
        }
        let pid_i = match args.get("pid").and_then(|v| v.as_i64()) {
            Some(p) if p > 0 && p <= i32::MAX as i64 => p as i32,
            Some(_) => {
                return ToolResult::error("kill_app: `pid` must be a positive integer".to_string())
            }
            None => {
                return ToolResult::error(
                    "kill_app: missing required integer field `pid`".to_string(),
                )
            }
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

fn stale_process_refusal(message: String) -> ToolResult {
    ToolResult::error(message.clone()).with_structured(serde_json::json!({
        "status": "refused",
        "refusal": {
            "code": "protected_resource_scope_stale",
            "message": message,
        }
    }))
}
