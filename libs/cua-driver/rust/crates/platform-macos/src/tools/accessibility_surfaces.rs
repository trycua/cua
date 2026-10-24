use async_trait::async_trait;
use cua_driver_core::{
    accessibility_surface,
    protocol::ToolResult,
    tool::{Tool, ToolDef},
    tool_args::ArgsExt,
};
use serde_json::{json, Value};

pub struct GetAccessibilitySurfacesTool;

fn require_pid(args: &Value) -> Result<i32, ToolResult> {
    match args.require_i32("pid") {
        Ok(pid) if pid > 0 => Ok(pid),
        Ok(_) => Err(ToolResult::error("pid must be a positive integer")
            .with_structured(json!({"code": "invalid_pid", "effect": "refused"}))),
        Err(error) => Err(error),
    }
}

fn bounded_arg(args: &Value, name: &str, maximum: usize) -> usize {
    args.get(name)
        .and_then(Value::as_u64)
        .map(|value| value.clamp(1, maximum as u64) as usize)
        .unwrap_or(maximum)
}

#[async_trait]
impl Tool for GetAccessibilitySurfacesTool {
    fn def(&self) -> &ToolDef {
        accessibility_surface::tool_def()
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let pid = match require_pid(&args) {
            Ok(pid) => pid,
            Err(error) => return error,
        };
        if !crate::permissions::status::accessibility_granted() {
            return ToolResult::error(
                "accessibility-only surfaces require macOS Accessibility permission",
            )
            .with_structured(json!({
                "code": "capability_unavailable",
                "effect": "refused",
                "pid": pid,
                "capability": accessibility_surface::capability("permission_denied", "macos"),
            }));
        }

        let max_elements = bounded_arg(&args, "max_elements", accessibility_surface::MAX_ELEMENTS);
        let max_depth = bounded_arg(&args, "max_depth", accessibility_surface::MAX_DEPTH);
        let observation = match tokio::task::spawn_blocking(move || {
            crate::ax::surface::observe_surfaces(pid, max_elements, max_depth)
        })
        .await
        {
            Ok(Ok(observation)) => observation,
            Ok(Err(failure)) => {
                return ToolResult::error(format!(
                    "accessibility surface observation failed for pid={pid} during {}",
                    failure.operation
                ))
                .with_structured(json!({
                    "code": "ax_observation_failed",
                    "effect": "refused",
                    "pid": pid,
                    "failure": failure,
                    "capability": accessibility_surface::capability("query_failed", "macos"),
                    "actions_supported": false,
                    "screenshot_supported": false,
                }))
            }
            Err(error) => {
                return ToolResult::error(format!(
                    "accessibility surface observation failed for pid={pid}: {error}"
                ))
            }
        };

        ToolResult::text(format!(
            "Found {} accessibility-only window surface(s) with {} semantic node(s) for pid={pid}.",
            observation.surfaces.len(),
            observation.node_count,
        ))
        .with_structured(json!({
            "pid": pid,
            "capability": accessibility_surface::capability("supported", "macos"),
            "surface_count": observation.surfaces.len(),
            "node_count": observation.node_count,
            "truncated": observation.truncated,
            "surfaces": observation.surfaces,
            "actions_supported": false,
            "screenshot_supported": false,
        }))
    }
}
