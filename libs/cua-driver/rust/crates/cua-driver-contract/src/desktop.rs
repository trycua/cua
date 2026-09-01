// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Cross-platform desktop-loop contracts.
//!
//! These schemas intentionally expose only the intersection accepted by the
//! macOS, Linux, and Windows backends. They generate safe client methods but
//! do not replace the richer platform-owned runtime schemas.

use crate::{
    ActionResult, ClickInput, ClipboardReadInput, ClipboardReadOutput, ClipboardWriteInput,
    ClipboardWriteOutput, CursorAction, CursorPositionOutput, CursorSemantics, DesktopStateOutput,
    DragInput, GetCursorPositionInput, GetDesktopStateInput, GetScreenSizeInput, HotkeyInput,
    InvokeMenuInput, MoveCursorInput, Platform, PressKeyInput, SchemaMode, ScreenSizeOutput,
    ScrollInput, SetWindowFrameInput, ToolAnnotations, ToolContract, ToolInput, ToolOutput,
    TypeTextInput,
};

const ALL_PLATFORMS: [Platform; 3] = [Platform::Macos, Platform::Windows, Platform::Linux];

pub fn contracts() -> Vec<ToolContract> {
    vec![
        get_desktop_state(),
        get_screen_size(),
        get_cursor_position(),
        move_cursor(),
        set_window_frame(),
        invoke_menu(),
        click(),
        drag(),
        scroll(),
        clipboard_read(),
        clipboard_write(),
        type_text(),
        press_key(),
        hotkey(),
    ]
}

const Z_INDEX_DESCRIPTION: &str = "Higher values are closer to the front. Null means the provider cannot observe stacking order; callers must not infer an order from array position or treat null as zero.";

// The required fields are the cross-platform record shape emitted by macOS,
// Windows, X11, and Wayland. Platform-only fields remain additive, while
// z_index retains its portable nullable ordering semantics. This runtime schema
// stays outside the typed SDK manifest until that broader shape converges.
pub(crate) fn list_windows_success_output_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "required": ["windows"],
        "properties": {
            "windows": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": [
                        "window_id", "pid", "app_name", "title", "bounds",
                        "z_index", "is_on_screen"
                    ],
                    "properties": {
                        "window_id": { "type": "integer" },
                        "pid": { "type": ["integer", "null"] },
                        "app_name": { "type": "string" },
                        "title": { "type": "string" },
                        "bounds": {
                            "type": "object",
                            "required": ["x", "y", "width", "height"],
                            "properties": {
                                "x": { "type": "number" },
                                "y": { "type": "number" },
                                "width": { "type": "number" },
                                "height": { "type": "number" }
                            },
                            "additionalProperties": false
                        },
                        "z_index": {
                            "type": ["integer", "null"],
                            "description": Z_INDEX_DESCRIPTION
                        },
                        "is_on_screen": { "type": "boolean" }
                    },
                    "additionalProperties": true
                }
            },
            "current_space_id": { "type": ["integer", "null"] }
        },
        "additionalProperties": true
    })
}

pub(crate) fn validate_list_windows_output(value: serde_json::Value) -> Result<(), String> {
    let windows = value
        .get("windows")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "windows must be an array".to_owned())?;
    if let Some(current_space_id) = value.get("current_space_id") {
        if !(current_space_id.is_null() || current_space_id.is_u64() || current_space_id.is_i64()) {
            return Err("current_space_id must be an integer or null".to_owned());
        }
    }
    for (index, window) in windows.iter().enumerate() {
        let record = window
            .as_object()
            .ok_or_else(|| format!("windows[{index}] must be an object"))?;
        for field in [
            "window_id",
            "pid",
            "app_name",
            "title",
            "bounds",
            "z_index",
            "is_on_screen",
        ] {
            if !record.contains_key(field) {
                return Err(format!("windows[{index}].{field} is required"));
            }
        }
        let window_id = &record["window_id"];
        if !(window_id.is_u64() || window_id.is_i64()) {
            return Err(format!("windows[{index}].window_id must be an integer"));
        }
        let pid = &record["pid"];
        if !(pid.is_null() || pid.is_u64() || pid.is_i64()) {
            return Err(format!("windows[{index}].pid must be an integer or null"));
        }
        for field in ["app_name", "title"] {
            if !record[field].is_string() {
                return Err(format!("windows[{index}].{field} must be a string"));
            }
        }
        let bounds = record["bounds"]
            .as_object()
            .ok_or_else(|| format!("windows[{index}].bounds must be an object"))?;
        for field in ["x", "y", "width", "height"] {
            if !bounds.get(field).is_some_and(serde_json::Value::is_number) {
                return Err(format!("windows[{index}].bounds.{field} must be a number"));
            }
        }
        let z_index = &record["z_index"];
        if !(z_index.is_null() || z_index.is_u64() || z_index.is_i64()) {
            return Err(format!(
                "windows[{index}].z_index must be an integer or null"
            ));
        }
        if !record["is_on_screen"].is_boolean() {
            return Err(format!("windows[{index}].is_on_screen must be a boolean"));
        }
    }
    Ok(())
}

fn clipboard_read() -> ToolContract {
    let mut contract = contract::<ClipboardReadInput, ClipboardReadOutput>(
        "clipboard_read",
        "List available system clipboard types and optionally return privacy-sensitive plain text. Clipboard content is never retained in telemetry.",
        &["clipboard.read", "clipboard.types"],
        ToolAnnotations {
            read_only: true,
            destructive: false,
            idempotent: false,
            open_world: false,
        },
        CursorAction::Observe,
    );
    contract.schema_mode = SchemaMode::CanonicalRuntime;
    contract
}

fn clipboard_write() -> ToolContract {
    let mut contract = contract::<ClipboardWriteInput, ClipboardWriteOutput>(
        "clipboard_write",
        "Replace the system clipboard with exactly one value: plain text, an image from an absolute local path, or a file URL from an absolute local path. Returns the available types for read-back before paste.",
        &["clipboard.write", "clipboard.write.text", "clipboard.write.image", "clipboard.write.file_url", "clipboard.types"],
        ToolAnnotations {
            read_only: false,
            destructive: true,
            idempotent: true,
            open_world: false,
        },
        CursorAction::Text,
    );
    contract.schema_mode = SchemaMode::CanonicalRuntime;
    contract
}

fn contract<I: ToolInput, O: ToolOutput>(
    name: &str,
    description: &str,
    capabilities: &[&str],
    annotations: ToolAnnotations,
    cursor_action: CursorAction,
) -> ToolContract {
    assert_eq!(name, I::TOOL_NAME, "typed input is bound to the wrong tool");
    ToolContract {
        name: name.into(),
        description: description.into(),
        platforms: ALL_PLATFORMS.to_vec(),
        aliases: Vec::new(),
        capabilities: capabilities.iter().map(|value| (*value).into()).collect(),
        annotations,
        schema_mode: SchemaMode::PortableSubset,
        cursor_semantics: Some(CursorSemantics::new(cursor_action)),
        input_schema: I::input_schema(),
        success_output_schema: Some(O::output_schema()),
        output_validator: crate::validate_typed_output::<O>,
    }
}

fn get_desktop_state() -> ToolContract {
    contract::<GetDesktopStateInput, DesktopStateOutput>(
        "get_desktop_state",
        "Capture the complete primary display at native resolution for a desktop-scope GUI loop.",
        &["screen.capture", "screen.dimensions"],
        ToolAnnotations {
            read_only: true,
            destructive: false,
            idempotent: false,
            open_world: false,
        },
        CursorAction::Observe,
    )
}

fn get_screen_size() -> ToolContract {
    contract::<GetScreenSizeInput, ScreenSizeOutput>(
        "get_screen_size",
        "Return the primary display dimensions and scale factor in the platform's desktop coordinate space.",
        &["screen.dimensions"],
        ToolAnnotations {
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        },
        CursorAction::Observe,
    )
}

fn get_cursor_position() -> ToolContract {
    contract::<GetCursorPositionInput, CursorPositionOutput>(
        "get_cursor_position",
        "Return the OS cursor position when the platform can observe it.",
        &["screen.cursor.position"],
        ToolAnnotations {
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        },
        CursorAction::Observe,
    )
}

fn move_cursor() -> ToolContract {
    contract::<MoveCursorInput, ActionResult>(
        "move_cursor",
        "Move the real OS pointer in get_desktop_state coordinates.",
        &["agent_cursor.move", "input.pointer.move"],
        ToolAnnotations {
            read_only: false,
            destructive: false,
            idempotent: true,
            open_world: false,
        },
        CursorAction::Navigate,
    )
}

fn set_window_frame() -> ToolContract {
    contract::<SetWindowFrameInput, ActionResult>(
        "set_window_frame",
        "Set one exact top-level window's frame in the desktop-coordinate space reported by list_windows and verify the resulting geometry through an independent readback.",
        &["window.frame.set"],
        ToolAnnotations {
            read_only: false,
            destructive: false,
            idempotent: true,
            open_world: false,
        },
        CursorAction::App,
    )
}

fn invoke_menu() -> ToolContract {
    contract::<InvokeMenuInput, ActionResult>(
        "invoke_menu",
        "Resolve an exact application-menu path one live native level at a time and invoke its final item through accessibility APIs. Missing, ambiguous, disabled, or structurally mismatched segments fail closed; this tool never falls back to pixels.",
        &["menu.path.invoke", "accessibility.menu.native"],
        ToolAnnotations {
            read_only: false,
            destructive: true,
            idempotent: false,
            open_world: true,
        },
        CursorAction::App,
    )
}

fn click() -> ToolContract {
    contract::<ClickInput, ActionResult>(
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
        CursorAction::Click,
    )
}

fn drag() -> ToolContract {
    contract::<DragInput, ActionResult>(
        "drag",
        "Drag between two absolute points in get_desktop_state coordinates.",
        &["input.pointer.drag"],
        ToolAnnotations {
            read_only: false,
            destructive: true,
            idempotent: false,
            open_world: true,
        },
        CursorAction::Drag,
    )
}

fn scroll() -> ToolContract {
    contract::<ScrollInput, ActionResult>(
        "scroll",
        "Scroll at an absolute point in get_desktop_state coordinates.",
        &["input.pointer.scroll", "accessibility.element_tokens"],
        ToolAnnotations {
            read_only: false,
            destructive: false,
            idempotent: false,
            open_world: true,
        },
        CursorAction::Scroll,
    )
}

fn type_text() -> ToolContract {
    contract::<TypeTextInput, ActionResult>(
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
        CursorAction::Text,
    )
}

fn press_key() -> ToolContract {
    contract::<PressKeyInput, ActionResult>(
        "press_key",
        "Press one key, with optional modifiers, in the foreground desktop application.",
        &["input.keyboard.press", "accessibility.element_tokens"],
        ToolAnnotations {
            read_only: false,
            destructive: true,
            idempotent: false,
            open_world: true,
        },
        CursorAction::Key,
    )
}

fn hotkey() -> ToolContract {
    contract::<HotkeyInput, ActionResult>(
        "hotkey",
        "Press a key chord in the foreground desktop application.",
        &["input.keyboard.hotkey"],
        ToolAnnotations {
            read_only: false,
            destructive: true,
            idempotent: false,
            open_world: true,
        },
        CursorAction::Key,
    )
}
