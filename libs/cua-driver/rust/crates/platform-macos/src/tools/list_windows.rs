use async_trait::async_trait;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use serde_json::Value;

pub struct ListWindowsTool;

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "list_windows".into(),
        description: "List top-level windows currently known to WindowServer. Without `pid` the \
            result is layer-0 only, because tooltips, popovers, menus and the Dock would swamp a \
            whole-desktop listing; with `pid` every layer of that process is reported, so an app \
            whose UI is an accessory window (floating panel, HUD, SwiftUI onboarding) is reachable \
            instead of looking closed. \
            Includes off-screen windows (minimized, on another Space, hidden-launched). \
            Use this to find a window_id before calling get_window_state.\n\n\
            Per-record fields: window_id, pid, app_name, title, bounds \
            (x/y/width/height, top-left origin), z_index (integer or null; higher values are \
            closer to the front; null means stacking order is unavailable and callers must not \
            infer one), is_on_screen, space_ids, current_space_id (the active Space on that \
            window's display), and on_current_space. The top-level current_space_id is \
            WindowServer's main/global active Space and can differ from a record's \
            current_space_id when displays use independent Spaces. To select a frontmost candidate, take the \
            maximum integer z_index; if every value is null, use an explicit fallback instead of \
            relying on array order.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "pid": {
                    "type": "integer",
                    "description": "Optional pid filter. When set, only this pid's windows are \
returned, and every CGWindow layer is admitted -- a caller that already named the process is not \
at risk of being swamped. Space attribution (space_ids, on_current_space, current_space_id) is not \
resolved for that enumeration and comes back null."
                },
                "on_screen_only": {
                    "type": "boolean",
                    "description": "When true, drop windows not on the current Space. Default false."
                }
            },
            "additionalProperties": false
        }),
        read_only: true,
        destructive: false,
        idempotent: true,
        open_world: false,
    })
}

#[async_trait]
impl Tool for ListWindowsTool {
    fn def(&self) -> &ToolDef {
        def()
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        let pid_filter: Option<i32> = args.opt_i64("pid").map(|v| v as i32);
        let on_screen_only = args.bool_or("on_screen_only", false);

        // Con pid, todas las capas. Es la semantica que el issue #1451 pidio y
        // el PR #1452 dejo en el backend Swift; la reescritura a Rust la
        // perdio, y con ella la unica via de alcanzar una app cuya UI entera
        // vive en una capa accesoria. El motivo del filtro -- no inundar al
        // llamante con tooltips, popovers, menus y el Dock -- solo aplica al
        // listado del escritorio entero, que sigue igual.
        let enumeration = match (on_screen_only, pid_filter.is_some()) {
            (true, false) => crate::windows::visible_windows_with_space_snapshot(),
            (false, false) => crate::windows::all_windows_with_space_snapshot(),
            (true, true) => crate::windows::visible_windows_any_layer_with_space_snapshot(),
            (false, true) => crate::windows::all_windows_any_layer_with_space_snapshot(),
        };
        let current_space_id = enumeration.current_space_id;
        let mut windows = enumeration.windows;

        if let Some(pid) = pid_filter {
            windows.retain(|w| w.pid == pid);
        }

        let windows_json: Vec<Value> = windows.iter().map(window_record_json).collect();

        ToolResult::text(format!("Found {} window(s).", windows_json.len())).with_structured(
            serde_json::json!({
                "windows": windows_json,
                "current_space_id": current_space_id
            }),
        )
    }
}

pub(super) fn window_record_json(w: &crate::windows::WindowInfo) -> Value {
    serde_json::json!({
        "window_id": w.window_id,
        "pid": w.pid,
        "app_name": w.app_name,
        "title": w.title,
        "bounds": {
            "x": w.bounds.x,
            "y": w.bounds.y,
            "width": w.bounds.width,
            "height": w.bounds.height
        },
        "layer": w.layer,
        "z_index": w.z_index,
        "is_on_screen": w.is_on_screen,
        "current_space_id": w.current_space_id,
        "on_current_space": w.on_current_space,
        "space_ids": w.space_ids,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sin pid el listado sigue siendo de capa 0: nadie que pregunte por el
    /// escritorio entero debe recibir el Dock, un tooltip ni cada NSMenu
    /// abierto. Con pid, el llamante ya nombro el proceso y no hay tal riesgo.
    #[test]
    fn pid_filter_documents_that_it_admits_every_layer() {
        let described = def().input_schema["properties"]["pid"]["description"]
            .as_str()
            .unwrap_or_default();
        assert!(
            described.contains("every CGWindow layer"),
            "el contrato de capas tiene que estar en el esquema, no solo en el codigo: {described}"
        );
        assert!(
            def().description.contains("layer-0 only"),
            "y el listado sin pid debe seguir anunciandose como capa 0"
        );
    }

    #[test]
    fn window_record_includes_observed_z_index() {
        let window = crate::windows::WindowInfo {
            window_id: 42,
            pid: 123,
            app_name: "Example".into(),
            title: "Document".into(),
            bounds: crate::windows::WindowBounds {
                x: 1.0,
                y: 2.0,
                width: 300.0,
                height: 200.0,
            },
            layer: 0,
            z_index: 7,
            is_on_screen: true,
            current_space_id: Some(1),
            on_current_space: Some(true),
            space_ids: Some(vec![1]),
        };

        assert_eq!(window_record_json(&window)["z_index"], serde_json::json!(7));
        assert_eq!(
            window_record_json(&window)["current_space_id"],
            serde_json::json!(1)
        );
        assert_eq!(
            window_record_json(&window)["on_current_space"],
            serde_json::json!(true)
        );
    }
}
