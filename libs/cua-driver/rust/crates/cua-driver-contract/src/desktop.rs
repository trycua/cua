// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Cross-platform desktop-loop contracts.
//!
//! These schemas intentionally expose only the intersection accepted by the
//! macOS, Linux, and Windows backends. They generate safe client methods but
//! do not replace the richer platform-owned runtime schemas.

use crate::{schema, Platform, SchemaMode, ToolAnnotations, ToolContract};
use serde_json::{json, Value};

const ALL_PLATFORMS: [Platform; 3] = [Platform::Macos, Platform::Windows, Platform::Linux];

pub fn contracts() -> Vec<ToolContract> {
    vec![
        get_desktop_state(),
        get_screen_size(),
        get_cursor_position(),
        move_cursor(),
        click(),
        drag(),
        scroll(),
        type_text(),
        press_key(),
        hotkey(),
    ]
}

fn contract(
    name: &str,
    description: &str,
    capabilities: &[&str],
    annotations: ToolAnnotations,
    input_schema: Value,
    success_output_schema: Value,
) -> ToolContract {
    ToolContract {
        name: name.into(),
        description: description.into(),
        platforms: ALL_PLATFORMS.to_vec(),
        aliases: Vec::new(),
        capabilities: capabilities.iter().map(|value| (*value).into()).collect(),
        annotations,
        schema_mode: SchemaMode::PortableSubset,
        input_schema,
        success_output_schema: Some(success_output_schema),
    }
}

fn get_desktop_state() -> ToolContract {
    contract(
        "get_desktop_state",
        "Capture the complete primary display at native resolution for a desktop-scope GUI loop.",
        &["screen.capture", "screen.dimensions"],
        ToolAnnotations {
            read_only: true,
            destructive: false,
            idempotent: false,
            open_world: false,
        },
        schema::closed_object(
            &[],
            [
                ("session", schema::string(Some("Optional session id."))),
                (
                    "screenshot_out_file",
                    schema::string(Some("Write the PNG here instead of returning base64.")),
                ),
            ],
        ),
        json!({
            "type": "object",
            "required": [
                "platform", "display", "screenshot_width", "screenshot_height",
                "screen_width", "screen_height", "scale_factor", "screenshot_mime_type"
            ],
            "properties": {
                "platform": { "type": "string", "enum": ["macos", "linux", "windows"] },
                "display": { "type": "string" },
                "screenshot_width": { "type": "integer" },
                "screenshot_height": { "type": "integer" },
                "screen_width": { "type": "integer" },
                "screen_height": { "type": "integer" },
                "scale_factor": { "type": "number" },
                "screenshot_mime_type": { "const": "image/png" },
                "screenshot_file_path": { "type": "string" }
            },
            "additionalProperties": true
        }),
    )
}

fn get_screen_size() -> ToolContract {
    contract(
        "get_screen_size",
        "Return the primary display dimensions and scale factor in the platform's desktop coordinate space.",
        &["screen.dimensions"],
        ToolAnnotations {
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        },
        schema::closed_object(&[], []),
        json!({
            "type": "object",
            "required": ["width", "height", "scale_factor"],
            "properties": {
                "width": { "type": "number" },
                "height": { "type": "number" },
                "scale_factor": { "type": "number" }
            },
            "additionalProperties": true
        }),
    )
}

fn get_cursor_position() -> ToolContract {
    contract(
        "get_cursor_position",
        "Return the OS cursor position when the platform can observe it.",
        &["screen.cursor.position"],
        ToolAnnotations {
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        },
        schema::closed_object(&[], []),
        json!({
            "type": "object",
            "properties": {
                "x": { "type": "number" },
                "y": { "type": "number" },
                "available": { "type": "boolean" },
                "source": { "type": "string" }
            },
            "additionalProperties": true
        }),
    )
}

fn move_cursor() -> ToolContract {
    contract(
        "move_cursor",
        "Move the real OS pointer in get_desktop_state coordinates.",
        &["agent_cursor.move", "input.pointer.move"],
        ToolAnnotations {
            read_only: false,
            destructive: false,
            idempotent: true,
            open_world: false,
        },
        schema::closed_object(
            &["x", "y", "scope"],
            [
                ("x", json!({ "type": "number" })),
                ("y", json!({ "type": "number" })),
                ("scope", json!({ "const": "desktop" })),
                ("session", schema::string(Some("Optional session id."))),
            ],
        ),
        json!({
            "type": "object",
            "properties": {
                "scope": { "const": "desktop" },
                "x": { "type": "number" },
                "y": { "type": "number" }
            },
            "additionalProperties": true
        }),
    )
}

fn click() -> ToolContract {
    contract(
        "click",
        "Click an absolute point in get_desktop_state coordinates without targeting a window.",
        &[
            "input.pointer.click",
            "input.pointer.click.left",
            "accessibility.element_tokens",
        ],
        ToolAnnotations {
            read_only: false,
            destructive: true,
            idempotent: false,
            open_world: true,
        },
        schema::closed_object(
            &["x", "y", "scope"],
            [
                ("x", json!({ "type": "number" })),
                ("y", json!({ "type": "number" })),
                ("scope", json!({ "const": "desktop" })),
                ("session", schema::string(Some("Optional session id."))),
                (
                    "button",
                    schema::string_enum(&["left", "right", "middle"], None),
                ),
                ("count", json!({ "type": "integer", "minimum": 1 })),
            ],
        ),
        json!({
            "type": "object",
            "properties": {
                "scope": { "const": "desktop" },
                "x": { "type": "number" },
                "y": { "type": "number" },
                "verified": { "type": "boolean" }
            },
            "additionalProperties": true
        }),
    )
}

fn action_output() -> Value {
    json!({
        "type": "object",
        "properties": {
            "scope": { "const": "desktop" },
            "effect": { "type": "string" },
            "verified": { "type": "boolean" }
        },
        "additionalProperties": true
    })
}

fn drag() -> ToolContract {
    contract(
        "drag",
        "Drag between two absolute points in get_desktop_state coordinates.",
        &["input.pointer.drag"],
        ToolAnnotations {
            read_only: false,
            destructive: true,
            idempotent: false,
            open_world: true,
        },
        schema::closed_object(
            &["from_x", "from_y", "to_x", "to_y", "scope"],
            [
                ("from_x", json!({ "type": "number" })),
                ("from_y", json!({ "type": "number" })),
                ("to_x", json!({ "type": "number" })),
                ("to_y", json!({ "type": "number" })),
                ("scope", json!({ "const": "desktop" })),
                ("session", schema::string(Some("Optional session id."))),
                (
                    "duration_ms",
                    json!({ "type": "integer", "minimum": 0, "maximum": 10000 }),
                ),
                (
                    "steps",
                    json!({ "type": "integer", "minimum": 1, "maximum": 200 }),
                ),
                (
                    "button",
                    schema::string_enum(&["left", "right", "middle"], None),
                ),
                (
                    "modifier",
                    json!({ "type": "array", "items": { "type": "string" } }),
                ),
            ],
        ),
        action_output(),
    )
}

fn scroll() -> ToolContract {
    contract(
        "scroll",
        "Scroll at an absolute point in get_desktop_state coordinates.",
        &["input.pointer.scroll", "accessibility.element_tokens"],
        ToolAnnotations {
            read_only: false,
            destructive: false,
            idempotent: false,
            open_world: true,
        },
        schema::closed_object(
            &["x", "y", "direction", "scope"],
            [
                ("x", json!({ "type": "number" })),
                ("y", json!({ "type": "number" })),
                (
                    "direction",
                    schema::string_enum(&["up", "down", "left", "right"], None),
                ),
                ("scope", json!({ "const": "desktop" })),
                ("session", schema::string(Some("Optional session id."))),
                ("by", schema::string_enum(&["line", "page"], None)),
                (
                    "amount",
                    json!({ "type": "integer", "minimum": 1, "maximum": 50 }),
                ),
            ],
        ),
        action_output(),
    )
}

fn type_text() -> ToolContract {
    contract(
        "type_text",
        "Type text into the current foreground desktop application.",
        &[
            "input.keyboard.type",
            "input.keyboard.type.terminal_safe",
            "accessibility.element_tokens",
        ],
        ToolAnnotations {
            read_only: false,
            destructive: true,
            idempotent: false,
            open_world: true,
        },
        schema::closed_object(
            &["text", "scope"],
            [
                ("text", schema::string(None)),
                ("scope", json!({ "const": "desktop" })),
                ("session", schema::string(Some("Optional session id."))),
            ],
        ),
        action_output(),
    )
}

fn press_key() -> ToolContract {
    contract(
        "press_key",
        "Press one key, with optional modifiers, in the foreground desktop application.",
        &["input.keyboard.press", "accessibility.element_tokens"],
        ToolAnnotations {
            read_only: false,
            destructive: true,
            idempotent: false,
            open_world: true,
        },
        schema::closed_object(
            &["key", "scope"],
            [
                ("key", schema::string(None)),
                ("scope", json!({ "const": "desktop" })),
                ("session", schema::string(Some("Optional session id."))),
                (
                    "modifiers",
                    json!({ "type": "array", "items": { "type": "string" } }),
                ),
            ],
        ),
        action_output(),
    )
}

fn hotkey() -> ToolContract {
    contract(
        "hotkey",
        "Press a key chord in the foreground desktop application.",
        &["input.keyboard.hotkey"],
        ToolAnnotations {
            read_only: false,
            destructive: true,
            idempotent: false,
            open_world: true,
        },
        schema::closed_object(
            &["keys", "scope"],
            [
                (
                    "keys",
                    json!({
                        "type": "array",
                        "items": { "type": "string" },
                        "minItems": 2
                    }),
                ),
                ("scope", json!({ "const": "desktop" })),
                ("session", schema::string(Some("Optional session id."))),
            ],
        ),
        action_output(),
    )
}
