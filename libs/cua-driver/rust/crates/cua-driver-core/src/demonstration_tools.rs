//! MCP tools for human demonstrations.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::demonstration::{DemonstrationConfig, DemonstrationManager};
use crate::protocol::ToolResult;
use crate::tool::{Tool, ToolDef};

pub struct StartDemonstrationTool {
    manager: Arc<DemonstrationManager>,
}

impl StartDemonstrationTool {
    pub fn new(manager: Arc<DemonstrationManager>) -> Self {
        Self { manager }
    }
}

static START_DEF: OnceLock<ToolDef> = OnceLock::new();

#[async_trait]
impl Tool for StartDemonstrationTool {
    fn def(&self) -> &ToolDef {
        START_DEF.get_or_init(|| ToolDef {
            name: "start_demonstration".into(),
            description: "Start observing human input on one Windows window. A red border is shown before capture starts. Input is ignored when the target is not foreground or border rendering stops. Typed content is never retained. Call stop_demonstration when the human finishes.".into(),
            input_schema: json!({
                "type": "object",
                "required": ["pid", "window_id"],
                "properties": {
                    "pid": { "type": "integer", "description": "Process id returned by list_windows." },
                    "window_id": { "type": "integer", "description": "Window id returned by list_windows." },
                    "output_dir": { "type": "string", "description": "New or empty output directory. Defaults to a unique directory under the system temporary directory." }
                },
                "additionalProperties": false
            }),
            read_only: false,
            destructive: false,
            idempotent: false,
            open_world: false,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use crate::tool_args::ArgsExt;

        let Some(pid) = args.opt_i64("pid") else {
            return ToolResult::error("`pid` is required.");
        };
        let Some(window_id) = args.opt_u64("window_id") else {
            return ToolResult::error("`window_id` is required.");
        };
        let output_dir = args
            .opt_str("output_dir")
            .filter(|path| !path.is_empty())
            .map(expand_tilde)
            .unwrap_or_else(|| {
                std::env::temp_dir()
                    .join("cua-demonstrations")
                    .join(format!("demo-{}", crate::recording::now_ms()))
            });
        let config = DemonstrationConfig {
            pid,
            window_id,
            output_dir,
            owner: args.opt_str("_session_id"),
        };
        let manager = self.manager.clone();
        match tokio::task::spawn_blocking(move || manager.start(config)).await {
            Ok(Ok(output_dir)) => ToolResult::text(format!(
                "Demonstration started on window {window_id}. Recording border active. Output: {}",
                output_dir.display()
            ))
            .with_structured(json!({
                "active": true,
                "pid": pid,
                "window_id": window_id,
                "output_dir": output_dir,
            })),
            Ok(Err(error)) => ToolResult::error(format!("Failed to start demonstration: {error}")),
            Err(error) => ToolResult::error(format!("Demonstration startup task failed: {error}")),
        }
    }
}

pub struct StopDemonstrationTool {
    manager: Arc<DemonstrationManager>,
}

impl StopDemonstrationTool {
    pub fn new(manager: Arc<DemonstrationManager>) -> Self {
        Self { manager }
    }
}

static STOP_DEF: OnceLock<ToolDef> = OnceLock::new();

#[async_trait]
impl Tool for StopDemonstrationTool {
    fn def(&self) -> &ToolDef {
        STOP_DEF.get_or_init(|| ToolDef {
            name: "stop_demonstration".into(),
            description: "Stop human input capture, remove the recording border, and write the trajectory and summary artifacts. Calling this without an active demonstration is a no-op.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            read_only: false,
            destructive: false,
            idempotent: true,
            open_world: false,
        })
    }

    async fn invoke(&self, _args: Value) -> ToolResult {
        let manager = self.manager.clone();
        match tokio::task::spawn_blocking(move || manager.stop()).await {
            Ok(Ok(Some(result))) => ToolResult::text(format!(
                "Demonstration stopped. {} actions captured. Trajectory: {}",
                result.artifacts.summary.action_count,
                result.artifacts.trajectory_md.display()
            ))
            .with_structured(json!({
                "active": false,
                "output_dir": result.output_dir,
                "trajectory_md": result.artifacts.trajectory_md,
                "summary_json": result.artifacts.summary_json,
                "action_count": result.artifacts.summary.action_count,
            })),
            Ok(Ok(None)) => ToolResult::text("No demonstration is active."),
            Ok(Err(error)) => ToolResult::error(format!(
                "Demonstration stopped, but artifact processing failed: {error}"
            )),
            Err(error) => ToolResult::error(format!("Demonstration stop task failed: {error}")),
        }
    }
}

fn expand_tilde(path: String) -> std::path::PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return std::path::PathBuf::from(home).join(rest);
        }
    }
    path.into()
}
