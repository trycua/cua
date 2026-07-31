//! Background-safe macOS window placement through the Accessibility API.

use async_trait::async_trait;
use core_foundation::base::{CFRelease, CFTypeRef};
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use serde_json::Value;

use crate::{
    ax::bindings::{
        ax_get_window_id, copy_ax_windows, kAXErrorSuccess, set_point_attr,
        AXUIElementCreateApplication,
    },
    windows::{WindowBounds, WindowOwner},
};

pub struct MoveWindowTool;

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "move_window".into(),
        description:
            "Move one macOS window to a global screen position without activating, raising, \
             resizing, or changing the real cursor. The target is scoped by both pid and \
             window_id. Coordinates use the same top-left global point space as \
             list_windows and get_screen_size.displays."
                .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["pid", "window_id", "x", "y"],
            "properties": {
                "pid": {
                    "type": "integer",
                    "description": "PID that owns the target window."
                },
                "window_id": {
                    "type": "integer",
                    "description": "CGWindowID from list_windows."
                },
                "x": {
                    "type": "number",
                    "description": "Requested global left edge in logical points."
                },
                "y": {
                    "type": "number",
                    "description": "Requested global top edge in logical points."
                }
            },
            "additionalProperties": false
        }),
        read_only: false,
        destructive: false,
        idempotent: true,
        open_world: false,
    })
}

#[async_trait]
impl Tool for MoveWindowTool {
    fn def(&self) -> &ToolDef {
        def()
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let Some(pid_value) = args.get("pid").and_then(Value::as_i64) else {
            return ToolResult::error("move_window requires integer field `pid`");
        };
        let Ok(pid) = i32::try_from(pid_value) else {
            return ToolResult::error("move_window `pid` is outside the process-id range");
        };
        if pid <= 0 {
            return ToolResult::error("move_window `pid` must be positive");
        }
        let Some(window_value) = args.get("window_id").and_then(Value::as_u64) else {
            return ToolResult::error("move_window requires integer field `window_id`");
        };
        let Ok(window_id) = u32::try_from(window_value) else {
            return ToolResult::error("move_window `window_id` is outside the CGWindowID range");
        };
        let Some(x) = args
            .get("x")
            .and_then(Value::as_f64)
            .filter(|v| v.is_finite())
        else {
            return ToolResult::error("move_window requires finite numeric field `x`");
        };
        let Some(y) = args
            .get("y")
            .and_then(Value::as_f64)
            .filter(|v| v.is_finite())
        else {
            return ToolResult::error("move_window requires finite numeric field `y`");
        };

        let result = tokio::task::spawn_blocking(move || move_window(pid, window_id, x, y)).await;
        match result {
            Ok(Ok((before, after))) => ToolResult::text(format!(
                "Moved window {window_id} for pid {pid} to ({:.0}, {:.0}) without activation.",
                after.x, after.y
            ))
            .with_structured(serde_json::json!({
                "pid": pid,
                "window_id": window_id,
                "requested_position": {"x": x, "y": y},
                "before_bounds": bounds_json(&before),
                "after_bounds": bounds_json(&after),
                "verified": true,
                "activated": false
            })),
            Ok(Err(error)) => {
                ToolResult::error(error.to_string()).with_structured(serde_json::json!({
                    "pid": pid,
                    "window_id": window_id,
                    "requested_position": {"x": x, "y": y},
                    "verified": false
                }))
            }
            Err(error) => ToolResult::error(format!("move_window task failed: {error}")),
        }
    }
}

pub(crate) fn move_window(
    pid: i32,
    window_id: u32,
    x: f64,
    y: f64,
) -> anyhow::Result<(WindowBounds, WindowBounds)> {
    match crate::windows::resolve_window_owner(pid, window_id) {
        WindowOwner::SamePid => {}
        WindowOwner::ForeignPid {
            owner_pid,
            owner_app_name,
        } => anyhow::bail!(
            "move_window refused: window {window_id} belongs to pid {owner_pid} \
             ({owner_app_name}), not requested pid {pid}"
        ),
        WindowOwner::Unknown => {
            anyhow::bail!("move_window refused: window {window_id} no longer exists")
        }
    }

    let before = crate::windows::window_bounds_by_id(window_id)
        .ok_or_else(|| anyhow::anyhow!("move_window could not read the current window bounds"))?;

    unsafe {
        let application = AXUIElementCreateApplication(pid);
        if application.is_null() {
            anyhow::bail!(
                "move_window could not create an accessibility application for pid {pid}"
            );
        }
        let windows = copy_ax_windows(application);
        CFRelease(application as CFTypeRef);

        let mut matched = false;
        let mut set_error = None;
        for window in windows {
            if ax_get_window_id(window) == Some(window_id) {
                matched = true;
                let error = set_point_attr(window, "AXPosition", x, y);
                if error != kAXErrorSuccess {
                    set_error = Some(error);
                }
            }
            CFRelease(window as CFTypeRef);
        }
        if !matched {
            anyhow::bail!(
                "move_window could not match window {window_id} in pid {pid}'s accessibility windows"
            );
        }
        if let Some(error) = set_error {
            anyhow::bail!("move_window AXPosition write failed with error {error}");
        }
    }

    for _ in 0..25 {
        if let Some(after) = crate::windows::window_bounds_by_id(window_id) {
            if position_matches(&after, x, y) {
                return Ok((before, after));
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let after = crate::windows::window_bounds_by_id(window_id)
        .ok_or_else(|| anyhow::anyhow!("move_window lost the target window after the AX write"))?;
    anyhow::bail!(
        "move_window was not verified: requested ({x:.1}, {y:.1}), observed ({:.1}, {:.1})",
        after.x,
        after.y
    )
}

fn position_matches(bounds: &WindowBounds, x: f64, y: f64) -> bool {
    (bounds.x - x).abs() <= 2.0 && (bounds.y - y).abs() <= 2.0
}

fn bounds_json(bounds: &WindowBounds) -> Value {
    serde_json::json!({
        "x": bounds.x,
        "y": bounds.y,
        "width": bounds.width,
        "height": bounds.height
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_requires_exact_window_and_global_position() {
        let definition = def();
        assert_eq!(
            definition.input_schema["required"],
            serde_json::json!(["pid", "window_id", "x", "y"])
        );
        assert!(!definition.read_only);
        assert!(!definition.destructive);
        assert!(definition.idempotent);
    }

    #[test]
    fn verification_allows_window_server_rounding_only() {
        let bounds = WindowBounds {
            x: 101.5,
            y: -39.0,
            width: 800.0,
            height: 600.0,
        };
        assert!(position_matches(&bounds, 100.0, -40.0));
        assert!(!position_matches(&bounds, 97.0, -40.0));
    }
}
