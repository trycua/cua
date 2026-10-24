use async_trait::async_trait;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use serde_json::Value;

// ── set_window_frame ──────────────────────────────────────────────────────

fn outer_frame_for_visible_request(
    requested: (i32, i32, i32, i32),
    outer_before: (i32, i32, i32, i32),
    visible_before: (i32, i32, i32, i32),
) -> Result<(i32, i32, i32, i32), String> {
    let (requested_x, requested_y, requested_width, requested_height) = requested;
    let (outer_x, outer_y, outer_width, outer_height) = outer_before;
    let (visible_x, visible_y, visible_width, visible_height) = visible_before;
    let left = i64::from(visible_x) - i64::from(outer_x);
    let top = i64::from(visible_y) - i64::from(outer_y);
    let right = i64::from(outer_x) + i64::from(outer_width)
        - (i64::from(visible_x) + i64::from(visible_width));
    let bottom = i64::from(outer_y) + i64::from(outer_height)
        - (i64::from(visible_y) + i64::from(visible_height));
    let adjusted = (
        i64::from(requested_x) - left,
        i64::from(requested_y) - top,
        i64::from(requested_width) + left + right,
        i64::from(requested_height) + top + bottom,
    );
    if adjusted.2 <= 0 || adjusted.3 <= 0 {
        return Err("visible-frame adjustment produced a non-positive outer size".to_owned());
    }
    let to_i32 = |value: i64| {
        i32::try_from(value)
            .map_err(|_| "visible-frame adjustment exceeded Windows coordinate range".to_owned())
    };
    Ok((
        to_i32(adjusted.0)?,
        to_i32(adjusted.1)?,
        to_i32(adjusted.2)?,
        to_i32(adjusted.3)?,
    ))
}

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
        let outcome = tokio::task::spawn_blocking(move || {
            use windows::Win32::{
                Foundation::{HWND, RECT},
                Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS},
                UI::WindowsAndMessaging::{
                    GetWindowRect, GetWindowThreadProcessId, IsIconic, IsWindow, IsZoomed,
                    SetWindowPos, SWP_NOACTIVATE, SWP_NOZORDER,
                },
            };

            let values = [input.x, input.y, input.width, input.height];
            if values.iter().any(|value| !value.is_finite())
                || input.width <= 0.0
                || input.height <= 0.0
            {
                return Err(
                    "x/y must be finite and width/height must be finite positive numbers"
                        .to_owned(),
                );
            }
            let to_i32 = |name: &str, value: f64| -> Result<i32, String> {
                let rounded = value.round();
                if rounded < i32::MIN as f64 || rounded > i32::MAX as f64 {
                    Err(format!("{name} is out of range for a Windows window coordinate"))
                } else {
                    Ok(rounded as i32)
                }
            };
            let x = to_i32("x", input.x)?;
            let y = to_i32("y", input.y)?;
            let width = to_i32("width", input.width)?;
            let height = to_i32("height", input.height)?;
            if width <= 0 || height <= 0 {
                return Err("width/height must round to positive Windows window sizes".to_owned());
            }
            let hwnd = HWND(input.window_id as usize as *mut std::ffi::c_void);
            if !unsafe { IsWindow(hwnd).as_bool() } {
                return Err(format!("window_id {} is closed, stale, or invalid", input.window_id));
            }
            let mut owner_pid = 0_u32;
            unsafe { GetWindowThreadProcessId(hwnd, Some(&mut owner_pid)) };
            if owner_pid != input.pid {
                return Err(format!(
                    "window_id {} belongs to pid {owner_pid}, not pid {}",
                    input.window_id, input.pid
                ));
            }
            if unsafe { IsIconic(hwnd).as_bool() } {
                return Err(format!(
                    "window_id {} is minimized; restore it with bring_to_front before setting its frame",
                    input.window_id
                ));
            }
            if unsafe { IsZoomed(hwnd).as_bool() } {
                return Err(format!(
                    "window_id {} is maximized; restore it before setting its frame",
                    input.window_id
                ));
            }
            let read_frames = |hwnd: HWND| -> Result<
                ((i32, i32, i32, i32), (i32, i32, i32, i32)),
                String,
            > {
                let mut outer = RECT::default();
                unsafe { GetWindowRect(hwnd, &mut outer) }
                    .map_err(|error| format!("could not read the current window frame: {error}"))?;
                let outer = (
                    outer.left,
                    outer.top,
                    outer.right - outer.left,
                    outer.bottom - outer.top,
                );
                let mut visible = RECT::default();
                let visible = unsafe {
                    DwmGetWindowAttribute(
                        hwnd,
                        DWMWA_EXTENDED_FRAME_BOUNDS,
                        &mut visible as *mut RECT as *mut _,
                        std::mem::size_of::<RECT>() as u32,
                    )
                }
                .ok()
                .map(|()| {
                    (
                        visible.left,
                        visible.top,
                        visible.right - visible.left,
                        visible.bottom - visible.top,
                    )
                })
                .filter(|(_, _, width, height)| *width > 0 && *height > 0)
                .unwrap_or(outer);
                Ok((outer, visible))
            };
            let (outer_before, before) = read_frames(hwnd)?;
            let requested = (x, y, width, height);
            let outer_requested =
                outer_frame_for_visible_request(requested, outer_before, before)?;
            let mutation_error = unsafe {
                SetWindowPos(
                    hwnd,
                    HWND::default(),
                    outer_requested.0,
                    outer_requested.1,
                    outer_requested.2,
                    outer_requested.3,
                    SWP_NOACTIVATE | SWP_NOZORDER,
                )
            }
            .err()
            .map(|error| format!("SetWindowPos failed: {error}"));

            let mut observed = None;
            for _ in 0..6 {
                if let Ok((_, visible)) = read_frames(hwnd) {
                    observed = Some(visible);
                    if observed == Some(requested) {
                        break;
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(40));
            }
            let changed = observed.is_some_and(|observed| before != observed);
            Ok((
                requested,
                outer_requested,
                observed,
                changed,
                mutation_error,
            ))
        })
        .await;

        let (requested, outer_requested, observed, changed, mutation_error) = match outcome {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(error)) => return ToolResult::error(format!("set_window_frame: {error}")),
            Err(error) => {
                return ToolResult::error(format!(
                    "set_window_frame: blocking task failed: {error}"
                ));
            }
        };
        let confirmed = observed == Some(requested);
        let effect = effect_from_value_readback(confirmed, changed, observed.is_some());
        let mut record = ActionExecutionRecord::builder(
            effect,
            ActionTransport::WindowsSetWindowPos,
            RequestedDelivery::NotApplicable,
        )
        .actual_delivery(ActualDelivery::NotApplicable)
        .detail(format!(
            "requested_visible={requested:?} requested_outer={outer_requested:?} observed_visible={observed:?} mutation_error={mutation_error:?}"
        ));
        if observed.is_some() {
            record = record.evidence(ActionEvidence {
                kind: EvidenceKind::ValueReadback,
                detail: if confirmed {
                    "DWM visible-frame bounds matched the requested frame".into()
                } else {
                    "DWM visible-frame bounds did not match the requested frame".into()
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

#[cfg(test)]
mod set_window_frame_geometry_tests;
