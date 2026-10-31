use async_trait::async_trait;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use serde_json::Value;

// ── set_window_frame (Linux) ─────────────────────────────────────────────────

pub struct SetWindowFrameTool;

static SET_WINDOW_FRAME_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for SetWindowFrameTool {
    fn def(&self) -> &ToolDef {
        SET_WINDOW_FRAME_DEF.get_or_init(|| {
            let contract = cua_driver_contract::tool_contract("set_window_frame")
                .expect("set_window_frame contract");
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
        use cua_driver_contract::SetWindowFrameInput;
        use cua_driver_core::action_record::{
            effect_from_value_readback, ActionEvidence, ActionExecutionRecord, ActionTransport,
            ActualDelivery, EvidenceKind, RequestedDelivery,
        };

        let input: SetWindowFrameInput =
            match cua_driver_core::tool_args::parse_typed_input("set_window_frame", args) {
                Ok(input) => input,
                Err(result) => return result,
            };
        if crate::wayland::is_wayland() {
            return ToolResult::error(
                "set_window_frame: this Wayland compositor exposes no protocol that permits a client to set another top-level window's geometry",
            );
        }
        let values = [input.x, input.y, input.width, input.height];
        if values.iter().any(|value| !value.is_finite())
            || input.width <= 0.0
            || input.height <= 0.0
        {
            return ToolResult::error(
                "set_window_frame: x/y must be finite and width/height must be finite positive numbers",
            );
        }
        let to_i32 = |name: &str, value: f64| -> Result<i32, String> {
            let rounded = value.round();
            if rounded < i32::MIN as f64 || rounded > i32::MAX as f64 {
                Err(format!("{name} is out of range for an X11 coordinate"))
            } else {
                Ok(rounded as i32)
            }
        };
        let to_u32 = |name: &str, value: f64| -> Result<u32, String> {
            let rounded = value.round();
            if rounded < 1.0 || rounded > u32::MAX as f64 {
                Err(format!("{name} is out of range for an X11 size"))
            } else {
                Ok(rounded as u32)
            }
        };
        let x = match to_i32("x", input.x) {
            Ok(value) => value,
            Err(error) => return ToolResult::error(format!("set_window_frame: {error}")),
        };
        let y = match to_i32("y", input.y) {
            Ok(value) => value,
            Err(error) => return ToolResult::error(format!("set_window_frame: {error}")),
        };
        let width = match to_u32("width", input.width) {
            Ok(value) => value,
            Err(error) => return ToolResult::error(format!("set_window_frame: {error}")),
        };
        let height = match to_u32("height", input.height) {
            Ok(value) => value,
            Err(error) => return ToolResult::error(format!("set_window_frame: {error}")),
        };
        let window_id = input.window_id;
        let pid = input.pid;
        let outcome = tokio::task::spawn_blocking(move || {
            let before = crate::x11::list_windows(Some(pid))
                .into_iter()
                .find(|window| window.xid == window_id)
                .map(|window| (window.x, window.y, window.width, window.height));
            let (observed, confirmed, mutation_error) =
                crate::x11::set_window_frame(window_id, pid, x, y, width, height)
                    .map_err(|error| error.to_string())?;
            let observed_frame =
                observed.map(|window| (window.x, window.y, window.width, window.height));
            let changed = observed_frame.is_some_and(|observed| before != Some(observed));
            Ok::<_, String>((observed_frame, confirmed, changed, mutation_error))
        })
        .await;
        let (observed, confirmed, changed, mutation_error) = match outcome {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(error)) => return ToolResult::error(format!("set_window_frame: {error}")),
            Err(error) => {
                return ToolResult::error(format!(
                    "set_window_frame: blocking task failed: {error}"
                ));
            }
        };
        let effect = effect_from_value_readback(confirmed, changed, observed.is_some());
        let requested = (x, y, width, height);
        let mut record = ActionExecutionRecord::builder(
            effect,
            ActionTransport::LinuxX11ConfigureWindow,
            RequestedDelivery::NotApplicable,
        )
        .actual_delivery(ActualDelivery::NotApplicable)
        .detail(format!(
            "requested={requested:?} observed={observed:?} mutation_error={mutation_error:?}"
        ));
        if observed.is_some() {
            record = record.evidence(ActionEvidence {
                kind: EvidenceKind::ValueReadback,
                detail: if confirmed {
                    "X11 geometry readback matched the requested frame".into()
                } else {
                    "X11 geometry readback did not match the requested frame".into()
                },
            });
        }
        ToolResult::text(if confirmed {
            "Set and verified the requested window frame."
        } else if observed.is_none() {
            "The native window mutation was attempted, but its resulting frame could not be read back."
        } else {
            "The window frame did not settle at the requested geometry."
        })
        .with_action_record(record.build().expect("set_window_frame record is valid"))
    }
}
