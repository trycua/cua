use async_trait::async_trait;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use serde_json::{json, Value};

// ── kill_app ──────────────────────────────────────────────────────────────────
//
// Force-terminate a process by pid. Background:
//   * Win32 / GDI apps respond cleanly to `WM_CLOSE` (Alt+F4 via PostMessage),
//     so `click` on the X button or a hand-rolled hotkey gesture suffices.
//   * UWP / WinUI3 apps (Calculator, Photos, modern Settings, Win11 Notepad)
//     have backgrounding semantics — they accept `WM_CLOSE` but route it
//     into a "minimize to suspended" path instead of exiting. Cooperative
//     close-button automation produces orphan processes that survive every
//     polite eviction attempt. See CUA-541 for the live repro from
//     WINDOWS_CLAUDE_CODE_TEST_PLAN.md Test G (3 leftover CalculatorApp.exe).
//
// `kill_app` is the force-terminate escalation — `taskkill /F` equivalent —
// keyed on pid. Marked `destructive: true` so MCP clients with permission
// gating prompt before invoking.

pub struct KillAppTool;
static KILL_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for KillAppTool {
    fn def(&self) -> &ToolDef {
        KILL_DEF.get_or_init(|| ToolDef {
            name: "kill_app".into(),
            description: "Force-terminate a process by pid. Use when the standard close path \
                (Alt+F4 / WM_CLOSE via `click` on the X button) fails to make the process \
                exit — typical for UWP / WinUI3 apps (Calculator, Photos, modern Notepad) \
                that route WM_CLOSE into a suspended-but-resident state. Equivalent to \
                `taskkill /F /PID <pid>`. Unsaved state is lost. Prefer the click-the-X path \
                first; only escalate to `kill_app` when polite close didn't terminate."
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
        let fingerprint = crate::browser_platform::WindowsBrowserPlatform::default()
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
        let pid_v: u32 = match args.get("pid").and_then(|v| v.as_u64()) {
            Some(p) if p > 0 && p <= u32::MAX as u64 => p as u32,
            Some(_) => {
                return ToolResult::error("kill_app: `pid` must be a positive integer".to_string())
            }
            None => {
                return ToolResult::error(
                    "kill_app: missing required integer field `pid`".to_string(),
                )
            }
        };

        // Run the syscalls on a blocking thread — TerminateProcess + the
        // WaitForSingleObject confirmation are both blocking, and we don't
        // want to stall the tokio reactor.
        let outcome = tokio::task::spawn_blocking(move || -> Result<(), String> {
            use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
            use windows::Win32::System::Threading::{
                OpenProcess, TerminateProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
                PROCESS_TERMINATE,
            };

            // SAFETY: OpenProcess returns a Result<HANDLE> in windows-rs;
            // PROCESS_TERMINATE | PROCESS_SYNCHRONIZE lets us both kill the
            // target AND wait for the OS to fully reap it (so the caller's
            // follow-up list_apps reflects the change immediately).
            let h = unsafe { OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, false, pid_v) }
                .map_err(|e| {
                format!(
                    "OpenProcess(pid={pid_v}, PROCESS_TERMINATE|PROCESS_SYNCHRONIZE) failed: {e}. \
                     The process may not exist, or the daemon lacks the right to terminate it. \
                     Try listing processes first to confirm the pid is correct."
                )
            })?;

            let term_result = unsafe { TerminateProcess(h, 1) };

            // WaitForSingleObject so the caller can assume the process is
            // really gone on return. 2-second cap — TerminateProcess is
            // synchronous in practice but the OS still needs a moment to
            // reap kernel structures. WAIT_TIMEOUT is treated as a soft
            // success (the kill request landed; the process may take a
            // little longer to disappear from list_apps).
            let wait_status = unsafe { WaitForSingleObject(h, 2000) };
            let _ = unsafe { CloseHandle(h) };

            match (term_result, wait_status) {
                (Ok(()), s) if s == WAIT_OBJECT_0 => Ok(()),
                (Ok(()), _) => Ok(()), // killed, wait timed out — caller can re-check
                (Err(e), _) => Err(format!(
                    "TerminateProcess(pid={pid_v}) failed: {e}. The handle was opened \
                     successfully but the kill itself was rejected; this is unusual for \
                     PROCESS_TERMINATE rights and may indicate a protected-process target \
                     (PPL, e.g. csrss / system services)."
                )),
            }
        })
        .await;

        match outcome {
            Ok(Ok(())) => ToolResult::text(format!("✅ Terminated pid {pid_v}.")),
            Ok(Err(e)) => ToolResult::error(format!("kill_app: {e}")),
            Err(join_err) => {
                ToolResult::error(format!("kill_app: blocking-task join error: {join_err}"))
            }
        }
    }
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
