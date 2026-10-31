use async_trait::async_trait;
use cua_driver_contract::InvokeMenuInput;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
    tool_args::parse_typed_input,
};
use serde_json::{json, Value};

use super::impl_::attach_focus_guard;

// ── invoke_menu (Linux) ──────────────────────────────────────────────────────

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

        // Menu paths are resolved and fired through AT-SPI, which needs no
        // window activation on X11: the default is background (focus-free,
        // under the focus guard). `delivery_mode:"foreground"` is the explicit
        // escalation that activates the window first, as every other tool.
        let delivery = crate::input::delivery::DeliveryMode::from_args(&args);
        let mut args = args;
        if let Some(object) = args.as_object_mut() {
            // Not part of the closed InvokeMenuInput contract; consumed above.
            object.remove("delivery_mode");
        }
        let input: InvokeMenuInput = match parse_typed_input("invoke_menu", args) {
            Ok(input) => input,
            Err(result) => return result,
        };
        let path = match normalized_menu_path(input.path) {
            Ok(path) => path,
            Err(error) => return menu_refusal(error),
        };
        let pid = input.pid;
        let window_id = input.window_id;
        if !crate::wayland::list_windows_dispatch(Some(pid))
            .iter()
            .any(|window| window.xid == window_id && window.pid == Some(pid))
        {
            return menu_refusal("invoke_menu: window_id does not belong to pid".into());
        }

        if !crate::wayland::is_wayland() && !delivery.is_foreground() {
            let outcome = tokio::task::spawn_blocking(move || {
                crate::input::focus_guard::guarded(Some(pid), || {
                    crate::atspi::native::invoke_menu_path_in(pid, window_id, &path)
                })
            })
            .await;
            return match outcome {
                Ok(Ok(((), guard))) => attach_focus_guard(
                    ToolResult::text(
                        "Resolved the live native menu path and dispatched its final accessibility action (delivery_mode=background, focus untouched); verify the command's semantic effect from fresh state.",
                    )
                    .with_structured(json!({
                        "path": "ax",
                        "verified": false,
                        "effect": "unverifiable",
                        "delivery_mode": "background",
                    }))
                    .with_action_record(
                        ActionExecutionRecord::builder(
                            ActionEffect::Unverifiable,
                            ActionTransport::LinuxAtSpiAction,
                            RequestedDelivery::Background,
                        )
                        .actual_delivery(ActualDelivery::Background)
                        .evidence(ActionEvidence {
                            kind: EvidenceKind::NativeApiResult,
                            detail: "Every menu hop resolved uniquely and AT-SPI accepted the final action"
                                .into(),
                        })
                        .build()
                        .expect("invoke_menu record is valid"),
                    ),
                    guard.as_ref(),
                ),
                Ok(Err(error)) => menu_refusal(format!("invoke_menu: {error}")),
                Err(error) => menu_refusal(format!("invoke_menu: blocking task failed: {error}")),
            };
        }

        let activation = if crate::wayland::is_wayland() {
            let result = tokio::task::spawn_blocking(move || {
                crate::wayland::activate_window_for_input_target(window_id, Some(pid))
            })
            .await;
            match result {
                Ok(Ok(())) => None,
                Ok(Err(error)) => {
                    return menu_refusal(format!(
                        "invoke_menu: target window could not be activated: {error}"
                    ))
                }
                Err(error) => {
                    return menu_refusal(format!("invoke_menu: activation task failed: {error}"))
                }
            }
        } else {
            match tokio::task::spawn_blocking(move || {
                crate::input::x11_activate_window_persistent(window_id)
            })
            .await
            {
                Ok(Ok(prior)) => Some(prior),
                Ok(Err(error)) => {
                    return menu_refusal(format!(
                        "invoke_menu: target window could not be activated: {error}"
                    ))
                }
                Err(error) => {
                    return menu_refusal(format!("invoke_menu: activation task failed: {error}"))
                }
            }
        };

        let outcome = tokio::task::spawn_blocking(move || {
            let result = crate::atspi::native::invoke_menu_path_in(pid, window_id, &path);
            if let Some(Some(prior_window)) = activation {
                let _ = crate::input::x11_activate_window_persistent(prior_window);
            }
            result
        })
        .await;

        match outcome {
            Ok(Ok(())) => ToolResult::text(
                "Resolved the live native menu path and dispatched its final accessibility action (delivery_mode=foreground); verify the command's semantic effect from fresh state.",
            )
            .with_structured(json!({
                "path": "ax",
                "verified": false,
                "effect": "unverifiable",
                "delivery_mode": "foreground",
            }))
            .with_action_record(
                ActionExecutionRecord::builder(
                    ActionEffect::Unverifiable,
                    ActionTransport::LinuxAtSpiAction,
                    RequestedDelivery::Foreground,
                )
                .actual_delivery(ActualDelivery::Foreground)
                .evidence(ActionEvidence {
                    kind: EvidenceKind::NativeApiResult,
                    detail: "Every menu hop resolved uniquely and AT-SPI accepted the final action"
                        .into(),
                })
                .build()
                .expect("invoke_menu record is valid"),
            ),
            Ok(Err(error)) => menu_refusal(format!("invoke_menu: {error}")),
            Err(error) => menu_refusal(format!("invoke_menu: blocking task failed: {error}")),
        }
    }
}
