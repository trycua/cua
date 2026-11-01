use async_trait::async_trait;
use cua_driver_contract::InvokeMenuInput;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
    tool_args::parse_typed_input,
};
use serde_json::{json, Value};

// ── invoke_menu ───────────────────────────────────────────────────────────

pub struct InvokeMenuTool;
static INVOKE_MENU_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn normalized_menu_path(path: Vec<String>) -> Result<Vec<String>, String> {
    if path.is_empty() || path.len() > 16 {
        return Err("invoke_menu: path must contain between 1 and 16 segments".into());
    }
    path.into_iter()
        .enumerate()
        .map(|(index, segment)| {
            let segment = segment.trim();
            if segment.is_empty() {
                Err(format!("invoke_menu: path segment {index} is empty"))
            } else {
                Ok(segment.to_owned())
            }
        })
        .collect()
}

fn menu_refusal(message: String) -> ToolResult {
    ToolResult::error(message.clone()).with_structured(json!({
        "status": "refused",
        "refusal": { "code": "menu_path_unavailable", "message": message }
    }))
}

/// Activate a window for a visible native-menu operation. Windows may reject
/// the first foreground request after the test/user session has been idle even
/// when we attach to the current foreground thread. A reserved no-name key has
/// no application meaning but makes this process the owner of the most recent
/// input, allowing a bounded retry without sending a real shortcut or text to
/// whichever application currently has focus.
fn activate_window_for_menu(hwnd: windows::Win32::Foundation::HWND) -> Result<bool, String> {
    let (activated, assisted) = unsafe { crate::input::force_foreground_assisted(hwnd) };
    if activated {
        Ok(assisted)
    } else {
        Err("invoke_menu: target window could not be activated".to_owned())
    }
}

#[async_trait]
impl Tool for InvokeMenuTool {
    fn def(&self) -> &ToolDef {
        INVOKE_MENU_DEF.get_or_init(|| {
            let contract =
                cua_driver_contract::tool_contract("invoke_menu").expect("invoke_menu contract");
            ToolDef {
                name: contract.name,
                description: contract.description,
                input_schema: contract.input_schema,
                read_only: contract.annotations.read_only,
                destructive: contract.annotations.destructive,
                idempotent: contract.annotations.idempotent,
                open_world: contract.annotations.open_world,
            }
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::action_record::{
            ActionEffect, ActionEvidence, ActionExecutionRecord, ActionTransport, ActualDelivery,
            EvidenceKind, RequestedDelivery,
        };
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{
            GetForegroundWindow, GetWindowThreadProcessId, IsWindow,
        };

        let input: InvokeMenuInput = match parse_typed_input("invoke_menu", args) {
            Ok(input) => input,
            Err(result) => return result,
        };
        let path = match normalized_menu_path(input.path) {
            Ok(path) => path,
            Err(error) => return menu_refusal(error),
        };
        let hwnd_value = input.window_id;
        let hwnd = HWND(hwnd_value as usize as *mut _);
        if !unsafe { IsWindow(hwnd) }.as_bool() {
            return menu_refusal("invoke_menu: window_id is closed, stale, or invalid".into());
        }
        let mut owner_pid = 0_u32;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut owner_pid)) };
        if owner_pid != input.pid {
            return menu_refusal("invoke_menu: window_id does not belong to pid".into());
        }

        let outcome = tokio::task::spawn_blocking(move || {
            // `HWND` wraps a raw pointer and is intentionally not `Send`.
            // Move the integer handle into the blocking worker and rebuild the
            // process-local wrapper there instead of carrying it across threads.
            let hwnd = HWND(hwnd_value as usize as *mut _);
            let prior = unsafe { GetForegroundWindow() };
            let needs_activation = prior != hwnd;
            let used_activation_retry = if needs_activation {
                activate_window_for_menu(hwnd)?
            } else {
                false
            };
            if needs_activation {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            let result = crate::uia::invoke_menu_path(hwnd_value, &path);
            if needs_activation && !prior.0.is_null() {
                let _ = unsafe { crate::input::force_foreground_attached(prior) };
            }
            result.map(|()| used_activation_retry)
        })
        .await;

        match outcome {
            Ok(Ok(used_activation_retry)) => ToolResult::text(
                "Resolved the live native menu path and dispatched its final accessibility action; verify the command's semantic effect from fresh state.",
            )
            .with_action_record(
                ActionExecutionRecord::builder(
                    ActionEffect::Unverifiable,
                    ActionTransport::WindowsUiaInvoke,
                    RequestedDelivery::Foreground,
                )
                .actual_delivery(ActualDelivery::Foreground)
                .evidence(ActionEvidence {
                    kind: EvidenceKind::NativeApiResult,
                    detail: "Every menu hop resolved uniquely and UI Automation accepted the final action"
                        .into(),
                })
                .detail(if used_activation_retry {
                    "target activation required a bounded reserved-key foreground-lock retry"
                } else {
                    "target was already foreground or activated without a foreground-lock retry"
                })
                .build()
                .expect("invoke_menu record is valid"),
            ),
            Ok(Err(error)) => menu_refusal(error),
            Err(error) => menu_refusal(format!("invoke_menu: blocking task failed: {error}")),
        }
    }
}
