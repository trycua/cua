// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

use async_trait::async_trait;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use serde_json::{json, Value};

use cua_driver_core::tool_args::ArgsExt;

use crate::session_resources;
use crate::virtual_display::{self, DisplayInfo};

pub struct AgentDisplayTool;

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "agent_display".into(),
        description: "Create, list or remove a virtual display the agent works on, so app \
             windows can be moved off the user's screen with set_window_frame (use the returned \
             x, y, width, height as the target area; screenshots of a virtual display are 1:1, \
             not Retina). `create` adds an extra display of width x height pixels owned by the \
             calling session, removed by `remove`, when that session ends or idles out, or \
             when the daemon exits; `list` returns the displays this daemon \
             created; `remove` needs a display_id. Requires no TCC permissions."
            .into(),
        input_schema: json!({
            "type": "object",
            "required": ["action"],
            "properties": {
                "action": { "type": "string", "enum": ["create", "list", "remove"] },
                "width": { "type": "integer", "description": "create: pixels, default 1920." },
                "height": { "type": "integer", "description": "create: pixels, default 1080." },
                "name": { "type": "string", "description": "create: display name, default \"Agent Desktop\"." },
                "display_id": { "type": "integer", "description": "remove: id returned by create or list." },
                "session": cua_driver_core::tool_schema::session_schema()
            },
            "additionalProperties": false,
        }),
        read_only: false,
        destructive: false,
        idempotent: false,
        open_world: false,
    })
}

fn info_json(info: &DisplayInfo) -> Value {
    json!({
        "display_id": info.display_id,
        "x": info.x,
        "y": info.y,
        "width": info.width,
        "height": info.height,
    })
}

fn edge(args: &Value, key: &str, default: u32) -> Result<u32, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(value) => value
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| format!("agent_display: `{key}` must be a positive integer")),
    }
}

#[async_trait]
impl Tool for AgentDisplayTool {
    fn def(&self) -> &ToolDef {
        def()
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let action = args.get("action").and_then(Value::as_str).unwrap_or("");
        match action {
            "create" => {
                let (width, height) =
                    match (edge(&args, "width", 1920), edge(&args, "height", 1080)) {
                        (Ok(w), Ok(h)) => (w, h),
                        (Err(e), _) | (_, Err(e)) => return ToolResult::error(e),
                    };
                let name = args
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|n| !n.is_empty())
                    .unwrap_or("Agent Desktop")
                    .to_owned();
                let session = args.opt_str("_session_id").unwrap_or_default();
                match tokio::task::spawn_blocking(move || {
                    virtual_display::create(width, height, &name)
                })
                .await
                {
                    Ok(Ok(info)) => {
                        session_resources::own_display(&session, info.display_id);
                        ToolResult::text(format!(
                            "✅ Created display {} at ({}, {}) {}x{}.",
                            info.display_id, info.x, info.y, info.width, info.height
                        ))
                        .with_structured(info_json(&info))
                    }
                    Ok(Err(error)) => ToolResult::error(format!("agent_display: {error}")),
                    Err(error) => ToolResult::error(format!("agent_display: task failed: {error}")),
                }
            }
            "list" => {
                let displays: Vec<Value> = virtual_display::list().iter().map(info_json).collect();
                ToolResult::text(format!("{} virtual display(s).", displays.len()))
                    .with_structured(json!({ "displays": displays }))
            }
            "remove" => match args
                .get("display_id")
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok())
            {
                Some(id) => match virtual_display::remove(id) {
                    Ok(()) => {
                        session_resources::forget_display(id);
                        ToolResult::text(format!("✅ Removed display {id}."))
                            .with_structured(json!({ "display_id": id }))
                    }
                    Err(error) => ToolResult::error(format!("agent_display: {error}")),
                },
                None => ToolResult::error("agent_display: `remove` needs an integer `display_id`"),
            },
            _ => ToolResult::error("agent_display: `action` must be create, list or remove"),
        }
    }
}
