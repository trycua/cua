//! Non-Windows stubs — return a helpful "not implemented" message.
//! Schemas match the macOS platform-macos implementations (cross-platform interface).

use async_trait::async_trait;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef, ToolRegistry},
};
use serde_json::Value;

fn not_impl(name: &str) -> ToolResult {
    ToolResult::error(format!(
        "{name}: Windows implementation is only available when compiled for Windows (target_os = \"windows\")."
    ))
}

macro_rules! stub_tool {
    ($mod_name:ident, $struct_name:ident, $tool_name:literal, $desc:expr, $schema:expr) => {
        mod $mod_name {
            use super::*;
            pub struct $struct_name;
            static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();
            impl $struct_name {
                pub fn def_static() -> &'static ToolDef {
                    DEF.get_or_init(|| ToolDef {
                        name: $tool_name.into(),
                        description: $desc.into(),
                        input_schema: $schema,
                        read_only: false,
                        destructive: false,
                        idempotent: false,
                        open_world: false,
                    })
                }
            }
            #[async_trait]
            impl Tool for $struct_name {
                fn def(&self) -> &ToolDef {
                    Self::def_static()
                }
                async fn invoke(&self, _args: Value) -> ToolResult {
                    not_impl($tool_name)
                }
            }
        }
        pub use $mod_name::$struct_name;
    };
}

macro_rules! contract_stub_tool {
    ($mod_name:ident, $struct_name:ident, $tool_name:literal) => {
        mod $mod_name {
            use super::*;
            pub struct $struct_name;
            static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();
            impl $struct_name {
                pub fn def_static() -> &'static ToolDef {
                    DEF.get_or_init(|| {
                        let contract = cua_driver_contract::tool_contract($tool_name)
                            .expect("canonical cursor contract exists");
                        ToolDef::from_contract(&contract)
                    })
                }
            }
            #[async_trait]
            impl Tool for $struct_name {
                fn def(&self) -> &ToolDef {
                    Self::def_static()
                }
                async fn invoke(&self, _args: Value) -> ToolResult {
                    not_impl($tool_name)
                }
            }
        }
        pub use $mod_name::$struct_name;
    };
}

stub_tool!(
    list_apps_m,
    ListAppsTool,
    "list_apps",
    "List running regular-UI applications.",
    serde_json::json!({"type":"object","properties":{},"additionalProperties":false})
);

stub_tool!(
    list_windows_m,
    ListWindowsTool,
    "list_windows",
    "List top-level windows, optionally filtered by pid.",
    serde_json::json!({"type":"object","properties":{"pid":{"type":"integer"}},"additionalProperties":false})
);

stub_tool!(get_window_state_m, GetWindowStateTool, "get_window_state",
    "Always returns BOTH the element tree AND a screenshot — ground on both and cross-check (the tree lies on some surfaces). Choose the modality at ACTION time: an element ax action (element_index/element_token → accessibility rung) or an element px action (x,y → pixel rung off this screenshot). capture_mode is deprecated and ignored.",
    serde_json::json!({"type":"object","required":["pid","window_id"],"properties":{"session": cua_driver_core::tool_schema::session_schema(),"pid":{"type":"integer"},"window_id":{"type":"integer"},"query":{"type":"string"},"capture_mode": cua_driver_core::capture_mode::capture_mode_schema(),"include_screenshot":{"type":"boolean","description":"Default true — returns a grounding screenshot alongside the tree. Set false to skip the grab and return tree only (the cheap path for re-indexing before an element ax action)."}},"additionalProperties":false}));

stub_tool!(
    launch_app_m,
    LaunchAppTool,
    "launch_app",
    "Launch an app in the background. Provide bundle_id (macOS) or name.",
    serde_json::json!({"type":"object","properties":{"bundle_id":{"type":"string"},"name":{"type":"string"},"urls":{"type":"array","items":{"type":"string"}}},"additionalProperties":false})
);

stub_tool!(kill_app_m, KillAppTool, "kill_app",
    "Force-terminate a process by pid (taskkill /F equivalent on Windows). Use as escalation when the cooperative close path didn't make the process exit — typical for UWP / WinUI3 apps that route WM_CLOSE into a suspended-but-resident state.",
    serde_json::json!({"type":"object","required":["pid"],"properties":{"pid":{"type":"integer","description":"PID of the process to terminate."}},"additionalProperties":false}));

stub_tool!(
    click_m,
    ClickTool,
    "click",
    "Click at (x, y) or on an AX/UIA element by element_index + window_id.",
    serde_json::json!({"type":"object","required":[],"properties":{"session": cua_driver_core::tool_schema::session_schema(),"pid":{"type":"integer"},"window_id":{"type":"integer"},"element_index": cua_driver_core::tool_schema::element_index_schema(),"element_token": cua_driver_core::tool_schema::element_token_schema(),"snapshot_id": cua_driver_core::tool_schema::snapshot_id_schema(),"x":{"type":"number"},"y":{"type":"number"},"button": cua_driver_core::tool_schema::button_schema(),"modifier": cua_driver_core::tool_schema::modifier_schema(),"from_zoom":{"type":"boolean"}},"additionalProperties":false})
);

stub_tool!(
    double_click_m,
    DoubleClickTool,
    "double_click",
    "Double-click at (x, y) or on an AX/UIA element by element_index + window_id.",
    serde_json::json!({"type":"object","required":["pid"],"properties":{"session": cua_driver_core::tool_schema::session_schema(),"pid":{"type":"integer"},"x":{"type":"number"},"y":{"type":"number"},"window_id":{"type":"integer"},"element_index": cua_driver_core::tool_schema::element_index_schema()},"additionalProperties":false})
);

stub_tool!(
    right_click_m,
    RightClickTool,
    "right_click",
    "Right-click (AXShowMenu on element, or synthesized event at x,y).",
    serde_json::json!({"type":"object","required":["pid"],"properties":{"session": cua_driver_core::tool_schema::session_schema(),"pid":{"type":"integer"},"element_index": cua_driver_core::tool_schema::element_index_schema(),"window_id":{"type":"integer"},"x":{"type":"number"},"y":{"type":"number"},"modifier": cua_driver_core::tool_schema::modifier_schema()},"additionalProperties":false})
);

stub_tool!(type_text_m, TypeTextTool, "type_text",
    "Insert text into the target pid via AXSelectedText or WM_CHAR. Falls back to CGEvent/keystrokes.",
    serde_json::json!({"type":"object","required":["pid","text"],"properties":{"session": cua_driver_core::tool_schema::session_schema(),"pid":{"type":"integer"},"text":{"type":"string"},"element_index": cua_driver_core::tool_schema::element_index_schema(),"window_id":{"type":"integer"},"x":{"type":"number"},"y":{"type":"number"}},"additionalProperties":false}));

stub_tool!(
    type_chars_m,
    TypeTextCharsTool,
    "type_text_chars",
    "Type text character-by-character with configurable per-character delay.",
    serde_json::json!({"type":"object","required":["pid","text"],"properties":{"pid":{"type":"integer"},"text":{"type":"string"},"delay_ms":{"type":"integer"},"window_id":{"type":"integer"},"element_index":{"type":"integer"}},"additionalProperties":false})
);

stub_tool!(
    press_key_m,
    PressKeyTool,
    "press_key",
    "Press and release a single key delivered directly to the target pid. No focus steal.",
    serde_json::json!({"type":"object","required":["pid","key"],"properties":{"session": cua_driver_core::tool_schema::session_schema(),"pid":{"type":"integer"},"key":{"type":"string"},"modifiers":{"type":"array","items":{"type":"string"}},"window_id":{"type":"integer"},"element_index": cua_driver_core::tool_schema::element_index_schema(),"x":{"type":"number"},"y":{"type":"number"}},"additionalProperties":false})
);

stub_tool!(
    hotkey_m,
    HotkeyTool,
    "hotkey",
    "Press a combination of keys simultaneously, e.g. [\"cmd\",\"c\"] for Copy.",
    serde_json::json!({"type":"object","required":["pid","keys"],"properties":{"session": cua_driver_core::tool_schema::session_schema(),"pid":{"type":"integer"},"keys":{"type":"array","items":{"type":"string"},"minItems":2},"x":{"type":"number"},"y":{"type":"number"}},"additionalProperties":false})
);

stub_tool!(
    set_value_m,
    SetValueTool,
    "set_value",
    "Set the value of an AX/UIA element (text field, dropdown, checkbox).",
    serde_json::json!({"type":"object","required":["pid","window_id","element_index","value"],"properties":{"session": cua_driver_core::tool_schema::session_schema(),"pid":{"type":"integer"},"window_id":{"type":"integer"},"element_index": cua_driver_core::tool_schema::element_index_schema(),"value":{"type":"string"}},"additionalProperties":false})
);

stub_tool!(scroll_m, ScrollTool, "scroll",
    "Scroll the target pid's focused region. direction required; by defaults to line, amount defaults to 3.",
    serde_json::json!({"type":"object","required":["direction"],"properties":{"session": cua_driver_core::tool_schema::session_schema(),"pid":{"type":"integer"},"direction":{"type":"string","enum":["up","down","left","right"]},"by":{"type":"string","enum":["line","page"]},"amount":{"type":"integer","minimum":1,"maximum":50},"window_id":{"type":"integer"},"element_index": cua_driver_core::tool_schema::element_index_schema()},"additionalProperties":false}));

stub_tool!(screenshot_m, ScreenshotTool, "screenshot",
    "Capture a screenshot. Without window_id captures the full display. Supports png and jpeg formats.",
    serde_json::json!({"type":"object","properties":{"window_id":{"type":"integer"},"format":{"type":"string","enum":["png","jpeg"]},"quality":{"type":"integer","minimum":1,"maximum":95}},"additionalProperties":false}));

stub_tool!(
    get_screen_size_m,
    GetScreenSizeTool,
    "get_screen_size",
    "Return the main screen's width and height in points.",
    serde_json::json!({"type":"object","properties":{},"additionalProperties":false})
);

stub_tool!(
    get_cursor_position_m,
    GetCursorPositionTool,
    "get_cursor_position",
    "Return the current real mouse cursor position.",
    serde_json::json!({"type":"object","properties":{},"additionalProperties":false})
);

contract_stub_tool!(
    cursor_enabled_v2_m,
    SetAgentCursorEnabledV2Tool,
    "set_agent_cursor_enabled"
);
contract_stub_tool!(
    cursor_motion_v2_m,
    SetAgentCursorMotionV2Tool,
    "set_agent_cursor_motion"
);
contract_stub_tool!(
    get_cursor_state_v2_m,
    GetAgentCursorStateV2Tool,
    "get_agent_cursor_state"
);
contract_stub_tool!(
    cursor_theme_m,
    SetAgentCursorThemeTool,
    "set_agent_cursor_theme"
);
contract_stub_tool!(invoke_menu_m, InvokeMenuTool, "invoke_menu");

stub_tool!(
    check_perms_m,
    CheckPermissionsTool,
    "check_permissions",
    "Check required permissions (Accessibility, Screen Recording).",
    serde_json::json!({"type":"object","properties":{"prompt":{"type":"boolean"}},"additionalProperties":false})
);

stub_tool!(
    get_config_m,
    GetConfigTool,
    "get_config",
    "Return current cua-driver-rs configuration.",
    serde_json::json!({"type":"object","properties":{},"additionalProperties":false})
);

stub_tool!(
    set_config_m,
    SetConfigTool,
    "set_config",
    "Update cua-driver-rs configuration.",
    serde_json::json!({"type":"object","properties":{"capture_mode":{"type":"string","enum":["ax","vision"]},"max_image_dimension":{"type":"integer"}},"additionalProperties":false})
);

stub_tool!(get_ax_tree_m, GetAccessibilityTreeTool, "get_accessibility_tree",
    "Lightweight desktop snapshot: running apps and visible windows. For per-window AX tree, use get_window_state.",
    serde_json::json!({"type":"object","properties":{},"additionalProperties":false}));

stub_tool!(
    zoom_m,
    ZoomTool,
    "zoom",
    "Crop a window region to JPEG with 20% padding, max 500px wide.",
    serde_json::json!({"type":"object","required":["window_id","x1","y1","x2","y2"],"properties":{"window_id":{"type":"integer"},"x1":{"type":"number"},"y1":{"type":"number"},"x2":{"type":"number"},"y2":{"type":"number"},"pid":{"type":"integer"}},"additionalProperties":false})
);

// move_cursor and set_agent_cursor_enabled route through the overlay channel.

mod move_cursor_m {
    use super::*;
    pub struct MoveCursorTool;
    static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();
    impl MoveCursorTool {
        pub fn def_static() -> &'static ToolDef {
            DEF.get_or_init(|| ToolDef {
                name: "move_cursor".into(),
                description: "Move the agent cursor overlay to (x, y). Does NOT move the real \
                    mouse cursor — the user's cursor stays where it is.".into(),
                input_schema: serde_json::json!({"type":"object","required":["x","y"],"properties":{"session": cua_driver_core::tool_schema::session_schema(),"x":{"type":"number"},"y":{"type":"number"},"cursor_id":{"type":"string"}},"additionalProperties":false}),
                read_only: false, destructive: false, idempotent: true, open_world: false,
            })
        }
    }
    #[async_trait]
    impl Tool for MoveCursorTool {
        fn def(&self) -> &ToolDef {
            Self::def_static()
        }
        async fn invoke(&self, args: Value) -> ToolResult {
            let x = args.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let y = args.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let cursor_id = args
                .get("cursor_id")
                .and_then(|v| v.as_str())
                .unwrap_or("default");
            crate::overlay::send_command(
                cursor_id.to_owned(),
                cursor_overlay::OverlayCommand::MoveTo {
                    x,
                    y,
                    end_heading_radians: 0.0,
                },
            );
            ToolResult::text(format!(
                "Agent cursor '{cursor_id}' moved to ({x:.1}, {y:.1})."
            ))
        }
    }
}
pub use move_cursor_m::MoveCursorTool;

pub fn build_registry() -> cua_driver_core::tool::ToolRegistry {
    let mut r = cua_driver_core::tool::ToolRegistry::new();
    r.register(Box::new(ListAppsTool));
    r.register(Box::new(ListWindowsTool));
    r.register(Box::new(GetWindowStateTool));
    r.register(Box::new(
        cua_driver_core::expectation::VerifyStateTool::new(std::sync::Arc::new(
            cua_driver_core::expectation::ToolObservationProvider::new(
                std::sync::Arc::new(ListWindowsTool),
                std::sync::Arc::new(GetWindowStateTool),
            ),
        )),
    ));
    r.register(Box::new(LaunchAppTool));
    r.register(Box::new(KillAppTool));
    r.register(Box::new(ClickTool));
    r.register(Box::new(DoubleClickTool));
    r.register(Box::new(RightClickTool));
    r.register(Box::new(TypeTextTool));
    r.register(Box::new(PressKeyTool));
    r.register(Box::new(HotkeyTool));
    r.register(Box::new(SetValueTool));
    r.register(Box::new(InvokeMenuTool));
    r.register(Box::new(ScrollTool));
    r.register(Box::new(ScreenshotTool));
    r.register(Box::new(GetScreenSizeTool));
    r.register(Box::new(GetCursorPositionTool));
    r.register(Box::new(MoveCursorTool));
    r.register(Box::new(SetAgentCursorEnabledV2Tool));
    r.register(Box::new(SetAgentCursorMotionV2Tool));
    r.register(Box::new(GetAgentCursorStateV2Tool));
    r.register(Box::new(SetAgentCursorThemeTool));
    r.register(Box::new(CheckPermissionsTool));
    // health_report is cross-platform — the Windows provider here lets
    // non-Windows hosts running the stubbed registry still advertise
    // the documented schema_version="1" contract.
    r.register(Box::new(
        cua_driver_core::health_report::HealthReportTool::new(std::sync::Arc::new(
            crate::health_report::WindowsHealthProvider,
        )),
    ));
    r.register(Box::new(GetConfigTool));
    r.register(Box::new(SetConfigTool));
    r.register(Box::new(GetAccessibilityTreeTool));
    r.register(Box::new(ZoomTool));
    r.register(Box::new(TypeTextCharsTool));
    r.register_recording_tools();
    r.register_session_tools();
    r
}
