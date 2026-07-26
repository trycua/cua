// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Cross-platform desktop-loop contracts.
//!
//! These schemas intentionally expose only the intersection accepted by the
//! macOS, Linux, and Windows backends. They generate safe client methods but
//! do not replace the richer platform-owned runtime schemas.

use crate::{
    ClickInput, ClickOutput, CursorPositionOutput, DesktopActionOutput, DesktopStateOutput,
    DragInput, GetCursorPositionInput, GetDesktopStateInput, GetScreenSizeInput, HotkeyInput,
    MoveCursorInput, MoveCursorOutput, Platform, PressKeyInput, SchemaMode, ScreenSizeOutput,
    ScrollInput, ToolAnnotations, ToolContract, ToolInput, ToolOutput, TypeTextInput,
};

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

fn contract<I: ToolInput, O: ToolOutput>(
    name: &str,
    description: &str,
    capabilities: &[&str],
    annotations: ToolAnnotations,
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
    )
}

fn move_cursor() -> ToolContract {
    contract::<MoveCursorInput, MoveCursorOutput>(
        "move_cursor",
        "Move the real OS pointer in get_desktop_state coordinates.",
        &["agent_cursor.move", "input.pointer.move"],
        ToolAnnotations {
            read_only: false,
            destructive: false,
            idempotent: true,
            open_world: false,
        },
    )
}

fn click() -> ToolContract {
    contract::<ClickInput, ClickOutput>(
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
    )
}

fn drag() -> ToolContract {
    contract::<DragInput, DesktopActionOutput>(
        "drag",
        "Drag between two absolute points in get_desktop_state coordinates.",
        &["input.pointer.drag"],
        ToolAnnotations {
            read_only: false,
            destructive: true,
            idempotent: false,
            open_world: true,
        },
    )
}

fn scroll() -> ToolContract {
    contract::<ScrollInput, DesktopActionOutput>(
        "scroll",
        "Scroll at an absolute point in get_desktop_state coordinates.",
        &["input.pointer.scroll", "accessibility.element_tokens"],
        ToolAnnotations {
            read_only: false,
            destructive: false,
            idempotent: false,
            open_world: true,
        },
    )
}

fn type_text() -> ToolContract {
    contract::<TypeTextInput, DesktopActionOutput>(
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
    )
}

fn press_key() -> ToolContract {
    contract::<PressKeyInput, DesktopActionOutput>(
        "press_key",
        "Press one key, with optional modifiers, in the foreground desktop application.",
        &["input.keyboard.press", "accessibility.element_tokens"],
        ToolAnnotations {
            read_only: false,
            destructive: true,
            idempotent: false,
            open_world: true,
        },
    )
}

fn hotkey() -> ToolContract {
    contract::<HotkeyInput, DesktopActionOutput>(
        "hotkey",
        "Press a key chord in the foreground desktop application.",
        &["input.keyboard.hotkey"],
        ToolAnnotations {
            read_only: false,
            destructive: true,
            idempotent: false,
            open_world: true,
        },
    )
}
