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
    DragInput, GetCursorPositionInput, GetDesktopStateInput, GetScreenSizeInput,
    GetWindowStateInput, HotkeyInput, InvokeMenuInput, ListAppsInput, ListAppsOutput,
    ListWindowsInput, ListWindowsOutput, MoveCursorInput, Platform, PressKeyInput, SchemaMode,
    ScreenSizeOutput, ScrollInput, SetWindowFrameInput, ToolAnnotations, ToolContract, ToolInput,
    ToolOutput, TypeTextInput, WindowStateOutput,
};

const ALL_PLATFORMS: [Platform; 3] = [Platform::Macos, Platform::Windows, Platform::Linux];

pub fn contracts() -> Vec<ToolContract> {
    vec![
        list_apps(),
        list_windows(),
        get_window_state(),
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

fn list_apps() -> ToolContract {
    contract::<ListAppsInput, ListAppsOutput>(
        "list_apps",
        "Discover installed and running native applications.",
        &["app.list"],
        ToolAnnotations {
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        },
        CursorAction::Observe,
    )
}

fn list_windows() -> ToolContract {
    contract::<ListWindowsInput, ListWindowsOutput>(
        "list_windows",
        "Discover exact native windows and observable bounds and stacking order.",
        &["window.list"],
        ToolAnnotations {
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        },
        CursorAction::Observe,
    )
}

fn get_window_state() -> ToolContract {
    contract::<GetWindowStateInput, WindowStateOutput>(
        "get_window_state",
        "Observe an exact native window with snapshot-bound elements and optional screenshots.",
        &[
            "accessibility.window_state",
            "accessibility.tree",
            "accessibility.tree.structured",
            "accessibility.tree.bounded",
            "accessibility.element_tokens",
            "screen.capture",
            "screen.capture.window",
        ],
        ToolAnnotations {
            read_only: true,
            destructive: false,
            idempotent: false,
            open_world: false,
        },
        CursorAction::Observe,
    )
}

fn clipboard_read() -> ToolContract {
    let mut contract = contract::<ClipboardReadInput, ClipboardReadOutput>(
        "clipboard_read",
        "List clipboard types and optionally return its plain text (sensitive).",
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
        "Replace the clipboard with exactly one of text, an image path or a file path. Returns the resulting types for read-back.",
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
        error_output_schema: None,
        output_validator: crate::validate_typed_output::<O>,
    }
}

fn get_desktop_state() -> ToolContract {
    contract::<GetDesktopStateInput, DesktopStateOutput>(
        "get_desktop_state",
        "Screenshot the primary display (desktop-scope GUI loop).",
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
        "Primary display size and scale factor.",
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
        "Set a top-level window's frame in list_windows coordinates and verify it by readback.",
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
        "Invoke an application menu item by exact path (e.g. File > Save As...) through accessibility; fails closed on a missing, ambiguous or disabled segment, never falls back to pixels.",
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
        "Click coordinates or a snapshot element, with an explicit target and delivery mode.",
        &[
            "input.pointer.click",
            "input.pointer.click.left",
            "accessibility.element_tokens",
            "input.delivery_mode",
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
