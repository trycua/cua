use async_trait::async_trait;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use serde_json::{json, Value};

// ── kill_app ──────────────────────────────────────────────────────────────────

pub struct KillAppTool;
static KILL_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for KillAppTool {
    fn def(&self) -> &ToolDef {
        KILL_DEF.get_or_init(|| ToolDef {
            name: "kill_app".into(),
            description: "Force-terminate a process by pid (kill -9 equivalent on Linux). \
                Use as escalation when the cooperative close path failed to make the process \
                exit. Unsaved state is lost — prefer the cooperative path first."
                .into(),
            input_schema: json!({"type":"object","required":["pid"],"properties":{
                "pid":{"type":"integer","description":"PID of the process to terminate."}
            },"additionalProperties":false}),
            read_only: false,
            destructive: true,
            idempotent: true,
            open_world: false,
        })
    }

    async fn protected_resource_scope(
        &self,
        adapter_id: &str,
        args: &Value,
    ) -> Result<Option<Value>, String> {
        if adapter_id != "process_control" {
            return Ok(None);
        }
        use cua_driver_core::browser::platform::BrowserPlatform;
        let pid = args
            .get("pid")
            .and_then(Value::as_i64)
            .filter(|pid| *pid > 0)
            .ok_or_else(|| "kill_app requires a positive integer pid".to_owned())?;
        let fingerprint = crate::browser_platform::LinuxBrowserPlatform::default()
            .process_fingerprint(pid)
            .await
            .map_err(|error| error.message)?;
        Ok(Some(json!({
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
                Err(message) => return kill_app_stale_process_refusal(message),
            };
            if current != *expected {
                return kill_app_stale_process_refusal(
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
        // Read the instance before signaling so the confirmation below can
        // tell its exit from a later process that reused the pid.
        let before = match crate::proc_fs::read_process_stat(pid_i as u32) {
            Ok(Some(before)) => before,
            Ok(None) => {
                return kill_app_failure(
                    pid_i,
                    "process_not_found",
                    format!("kill_app: pid {pid_i} does not exist; no signal was sent."),
                )
            }
            Err(error) => {
                return kill_app_failure(
                    pid_i,
                    "process_identity_unavailable",
                    format!(
                        "kill_app: cannot read /proc/{pid_i}/stat to verify termination: \
                         {error}; no signal was sent."
                    ),
                )
            }
        };
        // SAFETY: libc::kill is a thin syscall wrapper, no thread-safety concerns.
        let rc = unsafe { libc::kill(pid_i, libc::SIGKILL) };
        if rc != 0 {
            let err = std::io::Error::last_os_error();
            return kill_app_failure(
                pid_i,
                "signal_failed",
                format!(
                    "kill_app: kill(pid={pid_i}, SIGKILL) failed: {err}. \
                     The process may not exist, or the daemon lacks permission to signal it."
                ),
            );
        }
        // An accepted signal is not an exit: a sandbox (gVisor) can accept
        // SIGKILL for a process that keeps running. Report success only once
        // the same instance is gone.
        let deadline = tokio::time::Instant::now() + KILL_CONFIRM_TIMEOUT;
        loop {
            let now = crate::proc_fs::read_process_stat(pid_i as u32);
            if let Some(observation) = now
                .as_ref()
                .ok()
                .and_then(|now| crate::proc_fs::process_exit_observation(before, *now))
            {
                return ToolResult::text(format!(
                    "✅ Terminated pid {pid_i} with SIGKILL (confirmed: {observation})."
                ))
                .with_structured(json!({
                    "status": "terminated",
                    "effect": "confirmed",
                    "pid": pid_i,
                    "terminated": true,
                    "observation": observation,
                }));
            }
            if tokio::time::Instant::now() >= deadline {
                let detail = match now {
                    Err(error) => format!("/proc/{pid_i}/stat became unreadable: {error}"),
                    Ok(_) => format!(
                        "the same process was still alive {} ms later",
                        KILL_CONFIRM_TIMEOUT.as_millis()
                    ),
                };
                let mut result = kill_app_failure(
                    pid_i,
                    "termination_unconfirmed",
                    format!("kill_app: SIGKILL was accepted for pid {pid_i}, but {detail}."),
                );
                if let Some(structured) = result.structured_content.as_mut() {
                    structured["effect"] = json!("suspected_noop");
                }
                return result;
            }
            tokio::time::sleep(KILL_CONFIRM_INTERVAL).await;
        }
    }
}

/// How long `kill_app` waits for the signaled process to exit, and how often
/// it re-reads `/proc/<pid>/stat` meanwhile.
const KILL_CONFIRM_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
const KILL_CONFIRM_INTERVAL: std::time::Duration = std::time::Duration::from_millis(10);

fn kill_app_failure(pid: i32, code: &str, message: String) -> ToolResult {
    ToolResult::error(message.clone()).with_structured(json!({
        "status": "failed",
        "pid": pid,
        "terminated": false,
        "code": code,
        "message": message,
    }))
}

fn kill_app_stale_process_refusal(message: String) -> ToolResult {
    ToolResult::error(message.clone()).with_structured(json!({
        "status": "refused",
        "refusal": {
            "code": "protected_resource_scope_stale",
            "message": message,
        }
    }))
}
