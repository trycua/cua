use async_trait::async_trait;
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
        self.position_approximately_eq(other, tolerance)
            && self.size_approximately_eq(other, tolerance)
    }

    fn position_approximately_eq(self, other: Self, tolerance: f64) -> bool {
        (self.x - other.x).abs() <= tolerance && (self.y - other.y).abs() <= tolerance
    }

    fn size_approximately_eq(self, other: Self, tolerance: f64) -> bool {
        (self.width - other.width).abs() <= tolerance
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FrameMutation {
    Position,
    Size,
}

// Position must precede size. On macOS Tahoe, writing AXPosition immediately
// after AXSize can silently restore the window's previous size even though both
// accessibility writes return success.
const FRAME_MUTATION_ORDER: [FrameMutation; 2] = [FrameMutation::Position, FrameMutation::Size];

const POSITION_ONLY: [FrameMutation; 1] = [FrameMutation::Position];
const SIZE_ONLY: [FrameMutation; 1] = [FrameMutation::Size];

fn corrective_mutations(requested: Frame, observed: Frame) -> &'static [FrameMutation] {
    const TOLERANCE: f64 = 2.0;
    match (
        requested.position_approximately_eq(observed, TOLERANCE),
        requested.size_approximately_eq(observed, TOLERANCE),
    ) {
        (true, true) => &[],
        (false, true) => &POSITION_ONLY,
        (true, false) => &SIZE_ONLY,
        (false, false) => &FRAME_MUTATION_ORDER,
    }
}

// A window whose size cannot be set, such as Calculator's, can still be moved. Writing the
// size only when it changes keeps AXSize out of the request, so such a window only needs a
// settable AXPosition.
fn initial_mutations(requested: Frame, current: Frame) -> &'static [FrameMutation] {
    const TOLERANCE: f64 = 2.0;
    if requested.size_approximately_eq(current, TOLERANCE) {
        &POSITION_ONLY
    } else {
        &FRAME_MUTATION_ORDER
    }
}

fn window_server_frame(window_id: u32) -> Option<Frame> {
    crate::windows::window_bounds_by_id(window_id).map(|bounds| Frame {
        x: bounds.x,
        y: bounds.y,
        width: bounds.width,
        height: bounds.height,
    })
}

fn mutate_and_verify(
    input: &SetWindowFrameInput,
    window: super::ax_window::Window,
) -> Result<FrameOutcome, String> {
    use crate::ax::bindings::{
        element_screen_rect, is_attribute_settable, kAXErrorSuccess, set_point_attr, set_size_attr,
    };
    let requested = Frame::from_input(input);
    if !requested.is_valid() {
        return Err("x/y must be finite and width/height must be finite positive numbers".into());
    }
    let window_id = window.id;
    // Retain #4348's containment until the embedded SDK/AppKit regression passes.
    if input.pid == std::process::id() {
        return Err(format!(
            "refusing to move window_id {window_id} in the driver's own process (pid {}): AppKit window writes must run on the main thread",
            input.pid
        ));
    }
    let before = window_server_frame(window_id);
    let mutations = before.map_or(&FRAME_MUTATION_ORDER[..], |before| {
        initial_mutations(requested, before)
    });
    let apply = move |target, mutations: &[FrameMutation]| unsafe {
        mutations
            .iter()
            .filter_map(|mutation| {
                let (attribute, error) = match mutation {
                    FrameMutation::Position => (
                        "AXPosition",
                        set_point_attr(target, "AXPosition", requested.x, requested.y),
                    ),
                    FrameMutation::Size => (
                        "AXSize",
                        set_size_attr(target, "AXSize", requested.width, requested.height),
                    ),
                };
                (error != kAXErrorSuccess)
                    .then(|| format!("{attribute} was rejected with AXError {error}"))
            })
            .collect::<Vec<_>>()
    };
    let mut mutation_errors = window.with(move |target| unsafe {
        if !is_attribute_settable(target, "AXPosition") {
            return Err(format!(
                "window_id {window_id} does not expose a settable AXPosition"
            ));
        }
        if mutations.contains(&FrameMutation::Size) && !is_attribute_settable(target, "AXSize") {
            return Err(format!(
                "window_id {window_id} does not expose a settable AXSize"
            ));
        }
        before.ok_or_else(|| {
            format!("could not read the current WindowServer frame of window_id {window_id}")
        })?;
        Ok(apply(target, mutations))
    })?;
    let before = before.unwrap(); // Checked before the first write.
    let mut observed = None;
    for attempt in 0..20 {
        std::thread::sleep(std::time::Duration::from_millis(50));
        if attempt < 6 {
            mutation_errors.extend(window.with(move |target| unsafe {
                Ok(element_screen_rect(target).map_or_else(Vec::new, |rect| {
                    apply(
                        target,
                        corrective_mutations(requested, Frame::from_ax(rect)),
                    )
                }))
            })?);
        }
        if let Some(frame) = window_server_frame(window_id) {
            observed = Some(frame);
            if frame.approximately_eq(requested, 2.0) {
                break;
            }
        }
    }
    Ok(FrameOutcome {
        requested,
        observed,
        confirmed: observed.is_some_and(|frame| frame.approximately_eq(requested, 2.0)),
        changed: observed.is_some_and(|frame| !frame.approximately_eq(before, 2.0)),
        mutation_errors,
    })
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
                "WindowServer matched the requested frame within 2 points".into()
            } else {
                "WindowServer returned a frame that did not match the request".into()
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
        let input: SetWindowFrameInput =
            match cua_driver_core::tool_args::parse_typed_input("set_window_frame", args) {
                Ok(input) => input,
                Err(result) => return result,
            };
        let pid = match i32::try_from(input.pid) {
            Ok(pid) => pid,
            Err(_) => {
                return ToolResult::error(format!(
                    "set_window_frame: pid {} is out of range on macOS",
                    input.pid
                ))
            }
        };
        let window_id = match u32::try_from(input.window_id) {
            Ok(id) => id,
            Err(_) => {
                return ToolResult::error(format!(
                    "set_window_frame: window_id {} is out of range on macOS",
                    input.window_id
                ))
            }
        };
        let window = super::ax_window::Window::new(pid, window_id);
        let outcome =
            match tokio::task::spawn_blocking(move || mutate_and_verify(&input, window)).await {
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
    fn macos_applies_position_before_size_to_avoid_size_rollback() {
        assert_eq!(
            FRAME_MUTATION_ORDER,
            [FrameMutation::Position, FrameMutation::Size]
        );
    }

    #[test]
    fn initial_pass_writes_the_size_only_when_it_changes() {
        let current = Frame {
            x: 414.0,
            y: 560.0,
            width: 230.0,
            height: 408.0,
        };
        assert_eq!(
            initial_mutations(
                Frame {
                    x: 454.0,
                    y: 600.0,
                    ..current
                },
                current
            ),
            &POSITION_ONLY
        );
        assert_eq!(
            initial_mutations(
                Frame {
                    width: 500.0,
                    ..current
                },
                current
            ),
            &FRAME_MUTATION_ORDER
        );
    }

    #[test]
    fn corrective_pass_updates_only_the_component_that_did_not_settle() {
        let requested = Frame {
            x: 10.0,
            y: 20.0,
            width: 800.0,
            height: 600.0,
        };
        assert_eq!(
            corrective_mutations(
                requested,
                Frame {
                    x: 30.0,
                    y: 40.0,
                    ..requested
                }
            ),
            &POSITION_ONLY
        );
        assert_eq!(
            corrective_mutations(
                requested,
                Frame {
                    width: 900.0,
                    height: 700.0,
                    ..requested
                }
            ),
            &SIZE_ONLY
        );
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
