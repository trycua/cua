use async_trait::async_trait;
use core_foundation::base::{CFRelease, CFTypeRef};
use cua_driver_contract::SetWindowFrameInput;
use cua_driver_core::{
    action_record::{
        effect_from_value_readback, ActionEvidence, ActionExecutionRecord, ActionTransport,
        ActualDelivery, EvidenceKind, RequestedDelivery,
    },
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use serde_json::Value;

pub struct SetWindowFrameTool;

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| {
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

#[derive(Clone, Copy, Debug)]
struct Frame {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl Frame {
    fn from_input(input: &SetWindowFrameInput) -> Self {
        Self {
            x: input.x,
            y: input.y,
            width: input.width,
            height: input.height,
        }
    }

    fn from_ax(rect: [f64; 4]) -> Self {
        Self {
            x: rect[0],
            y: rect[1],
            width: rect[2],
            height: rect[3],
        }
    }

    fn is_valid(self) -> bool {
        self.x.is_finite()
            && self.y.is_finite()
            && self.width.is_finite()
            && self.height.is_finite()
            && self.width > 0.0
            && self.height > 0.0
    }

    fn approximately_eq(self, other: Self, tolerance: f64) -> bool {
        (self.x - other.x).abs() <= tolerance
            && (self.y - other.y).abs() <= tolerance
            && (self.width - other.width).abs() <= tolerance
            && (self.height - other.height).abs() <= tolerance
    }
}

#[derive(Debug)]
struct FrameOutcome {
    requested: Frame,
    observed: Option<Frame>,
    confirmed: bool,
    changed: bool,
    mutation_errors: Vec<String>,
}

fn mutate_and_verify(input: &SetWindowFrameInput) -> Result<FrameOutcome, String> {
    use crate::{
        ax::bindings::{
            ax_get_window_id, copy_ax_windows, element_screen_rect, is_attribute_settable,
            kAXErrorSuccess, set_point_attr, set_size_attr, AXUIElementCreateApplication,
            AXUIElementSetMessagingTimeout,
        },
        windows::WindowOwner,
    };

    let pid = i32::try_from(input.pid)
        .map_err(|_| format!("pid {} is out of range on macOS", input.pid))?;
    let window_id = u32::try_from(input.window_id)
        .map_err(|_| format!("window_id {} is out of range on macOS", input.window_id))?;
    let requested = Frame::from_input(input);
    if !requested.is_valid() {
        return Err("x/y must be finite and width/height must be finite positive numbers".into());
    }

    match crate::windows::resolve_window_owner(pid, window_id) {
        WindowOwner::SamePid => {}
        WindowOwner::ForeignPid {
            owner_pid,
            owner_app_name,
        } => {
            return Err(format!(
                "window_id {window_id} belongs to pid {owner_pid} ({owner_app_name}), not pid {pid}"
            ));
        }
        WindowOwner::Unknown => {
            return Err(format!(
                "window_id {window_id} is closed, stale, or unknown to WindowServer"
            ));
        }
    }

    unsafe {
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return Err(format!(
                "could not create an accessibility element for pid {pid}"
            ));
        }
        AXUIElementSetMessagingTimeout(app, 2.0);
        let windows = copy_ax_windows(app);
        let target = windows
            .iter()
            .copied()
            .find(|window| ax_get_window_id(*window) == Some(window_id));

        let result = if let Some(target) = target {
            AXUIElementSetMessagingTimeout(target, 2.0);
            if !is_attribute_settable(target, "AXPosition") {
                Err(format!(
                    "window_id {window_id} does not expose a settable AXPosition"
                ))
            } else if !is_attribute_settable(target, "AXSize") {
                Err(format!(
                    "window_id {window_id} does not expose a settable AXSize"
                ))
            } else {
                let before = element_screen_rect(target)
                    .map(Frame::from_ax)
                    .ok_or_else(|| {
                        format!("could not read the current frame of window_id {window_id}")
                    });
                before.and_then(|before| {
                    let size_error =
                        set_size_attr(target, "AXSize", requested.width, requested.height);
                    let position_error =
                        set_point_attr(target, "AXPosition", requested.x, requested.y);
                    let mut mutation_errors = Vec::new();
                    if size_error != kAXErrorSuccess {
                        mutation_errors
                            .push(format!("AXSize was rejected with AXError {size_error}"));
                    }
                    if position_error != kAXErrorSuccess {
                        mutation_errors.push(format!(
                            "AXPosition was rejected with AXError {position_error}"
                        ));
                    }

                    let mut observed = None;
                    for _ in 0..6 {
                        if let Some(rect) = element_screen_rect(target) {
                            let frame = Frame::from_ax(rect);
                            let confirmed = frame.approximately_eq(requested, 2.0);
                            observed = Some(frame);
                            if confirmed {
                                break;
                            }
                        }
                        std::thread::sleep(std::time::Duration::from_millis(40));
                    }
                    let confirmed =
                        observed.is_some_and(|frame| frame.approximately_eq(requested, 2.0));
                    let changed =
                        observed.is_some_and(|frame| !frame.approximately_eq(before, 2.0));
                    Ok(FrameOutcome {
                        requested,
                        observed,
                        confirmed,
                        changed,
                        mutation_errors,
                    })
                })
            }
        } else {
            Err(format!(
                "window_id {window_id} belongs to pid {pid} in WindowServer but has no matching AXWindow"
            ))
        };

        for window in windows {
            CFRelease(window as CFTypeRef);
        }
        CFRelease(app as CFTypeRef);
        result
    }
}

fn action_record(outcome: &FrameOutcome) -> ActionExecutionRecord {
    let effect = effect_from_value_readback(
        outcome.confirmed,
        outcome.changed,
        outcome.observed.is_some(),
    );
    let mut builder = ActionExecutionRecord::builder(
        effect,
        ActionTransport::MacosAxWindowFrame,
        RequestedDelivery::NotApplicable,
    )
    .actual_delivery(ActualDelivery::NotApplicable)
    .detail(format!(
        "requested={:?} observed={:?} mutation_errors={:?}",
        outcome.requested, outcome.observed, outcome.mutation_errors,
    ));
    if outcome.observed.is_some() {
        builder = builder.evidence(ActionEvidence {
            kind: EvidenceKind::ValueReadback,
            detail: if outcome.confirmed {
                "AXPosition and AXSize matched the requested frame within 2 points".into()
            } else {
                "AXPosition and AXSize were readable but did not match the requested frame".into()
            },
        });
    }
    builder.build().expect("set_window_frame record is valid")
}

#[async_trait]
impl Tool for SetWindowFrameTool {
    fn def(&self) -> &ToolDef {
        def()
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let input: SetWindowFrameInput = match serde_json::from_value(args) {
            Ok(input) => input,
            Err(error) => return ToolResult::error(format!("set_window_frame: {error}")),
        };
        let outcome = match tokio::task::spawn_blocking(move || mutate_and_verify(&input)).await {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(error)) => return ToolResult::error(format!("set_window_frame: {error}")),
            Err(error) => {
                return ToolResult::error(format!(
                    "set_window_frame: blocking task failed: {error}"
                ));
            }
        };
        ToolResult::text(if outcome.confirmed {
            "Set and verified the requested window frame."
        } else if outcome.observed.is_none() {
            "The native window mutation was attempted, but its resulting frame could not be read back."
        } else {
            "The window frame did not settle at the requested geometry."
        })
        .with_action_record(action_record(&outcome))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_validation_rejects_non_positive_or_non_finite_size() {
        assert!(Frame {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
        }
        .is_valid());
        assert!(!Frame {
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 600.0,
        }
        .is_valid());
        assert!(!Frame {
            x: f64::NAN,
            y: 0.0,
            width: 800.0,
            height: 600.0,
        }
        .is_valid());
    }

    #[test]
    fn frame_readback_uses_small_platform_tolerance() {
        let requested = Frame {
            x: 10.0,
            y: 20.0,
            width: 800.0,
            height: 600.0,
        };
        assert!(requested.approximately_eq(
            Frame {
                x: 11.5,
                y: 19.0,
                width: 799.0,
                height: 601.0,
            },
            2.0
        ));
        assert!(!requested.approximately_eq(
            Frame {
                x: 13.0,
                ..requested
            },
            2.0
        ));
    }

    #[test]
    fn missing_frame_readback_is_unverifiable_without_fabricated_evidence() {
        let frame = Frame {
            x: 10.0,
            y: 20.0,
            width: 800.0,
            height: 600.0,
        };
        let record = action_record(&FrameOutcome {
            requested: frame,
            observed: None,
            confirmed: false,
            changed: false,
            mutation_errors: vec!["AXPosition timed out".into()],
        });
        let projection = record.stable_projection().expect("valid projection");
        assert_eq!(
            projection.effect,
            cua_driver_core::action_record::ActionEffect::Unverifiable
        );
        assert_eq!(projection.evidence, None);
    }
}
