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
             not Retina). `create` adds an extra display of width x height pixels. By default \
             the calling session owns it, and it is removed by `remove`, when that session ends \
             or idles out, or when the daemon exits. With `lifetime: \"daemon\"` no session owns \
             it: it stays until `remove` or the daemon exits, so windows parked there stay off \
             the user's screen between sessions. `list` returns the displays this daemon \
             created and each one's lifetime; `remove` needs a display_id. Requires no TCC \
             permissions."
            .into(),
        input_schema: json!({
            "type": "object",
            "required": ["action"],
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["create", "list", "remove"],
                    "description": "create a display, list this daemon's displays, or remove one."
                },
                "width": { "type": "integer", "description": "create: pixels, default 1920." },
                "height": { "type": "integer", "description": "create: pixels, default 1080." },
                "name": { "type": "string", "description": "create: display name, default \"Agent Desktop\"." },
                "lifetime": {
                    "type": "string",
                    "enum": ["session", "daemon"],
                    "description": "create: session (default) removes the display when the calling session ends or idles out; daemon keeps it until remove or the daemon exits."
                },
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lifetime {
    Session,
    Daemon,
}

impl Lifetime {
    fn parse(args: &Value) -> Result<Self, String> {
        match args.get("lifetime") {
            None | Some(Value::Null) => Ok(Self::Session),
            Some(Value::String(value)) if value == "session" => Ok(Self::Session),
            Some(Value::String(value)) if value == "daemon" => Ok(Self::Daemon),
            Some(_) => Err("agent_display: `lifetime` must be session or daemon".into()),
        }
    }

    fn of(display_id: u32) -> Self {
        if session_resources::is_session_owned(display_id) {
            Self::Session
        } else {
            Self::Daemon
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Daemon => "daemon",
        }
    }
}

fn info_json(info: &DisplayInfo, lifetime: Lifetime) -> Value {
    json!({
        "display_id": info.display_id,
        "x": info.x,
        "y": info.y,
        "width": info.width,
        "height": info.height,
        "lifetime": lifetime.as_str(),
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
                let lifetime = match Lifetime::parse(&args) {
                    Ok(lifetime) => lifetime,
                    Err(error) => return ToolResult::error(error),
                };
                let name = args
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|n| !n.is_empty())
                    .unwrap_or("Agent Desktop")
                    .to_owned();
                let create = move || virtual_display::create(width, height, &name);
                let created = match lifetime {
                    Lifetime::Session => {
                        let session = args.opt_str("_session_id").unwrap_or_default();
                        session_resources::create_owned(session, create).await
                    }
                    Lifetime::Daemon => tokio::task::spawn_blocking(create).await,
                };
                match created {
                    Ok(Ok(info)) => ToolResult::text(format!(
                        "✅ Created display {} at ({}, {}) {}x{}, {} lifetime.",
                        info.display_id,
                        info.x,
                        info.y,
                        info.width,
                        info.height,
                        lifetime.as_str()
                    ))
                    .with_structured(info_json(&info, lifetime)),
                    Ok(Err(error)) => ToolResult::error(format!("agent_display: {error}")),
                    Err(error) => ToolResult::error(format!("agent_display: task failed: {error}")),
                }
            }
            "list" => {
                let displays: Vec<Value> = virtual_display::list()
                    .iter()
                    .map(|info| info_json(info, Lifetime::of(info.display_id)))
                    .collect();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifetime_defaults_to_the_session() {
        assert_eq!(Lifetime::parse(&json!({})), Ok(Lifetime::Session));
        assert_eq!(
            Lifetime::parse(&json!({ "lifetime": null })),
            Ok(Lifetime::Session)
        );
        assert_eq!(
            Lifetime::parse(&json!({ "lifetime": "session" })),
            Ok(Lifetime::Session)
        );
        assert_eq!(
            Lifetime::parse(&json!({ "lifetime": "daemon" })),
            Ok(Lifetime::Daemon)
        );
    }

    #[test]
    fn an_unknown_lifetime_is_refused() {
        for value in [json!("forever"), json!(true), json!(1)] {
            assert_eq!(
                Lifetime::parse(&json!({ "lifetime": value })),
                Err("agent_display: `lifetime` must be session or daemon".into())
            );
        }
    }

    #[test]
    fn the_schema_offers_both_lifetimes() {
        let lifetime = &def().input_schema["properties"]["lifetime"];
        assert_eq!(lifetime["enum"], json!(["session", "daemon"]));
    }

    #[test]
    fn a_display_no_session_owns_is_listed_with_the_daemon_lifetime() {
        let info = DisplayInfo {
            display_id: 4_000_000_001,
            x: -1280.0,
            y: 0.0,
            width: 1280.0,
            height: 800.0,
        };
        let listed = info_json(&info, Lifetime::of(info.display_id));
        assert_eq!(listed["lifetime"], "daemon");
        assert_eq!(listed["display_id"], 4_000_000_001u32);
    }
}
