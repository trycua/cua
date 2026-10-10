use async_trait::async_trait;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use serde_json::Value;

pub struct ListAppsTool;

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "list_apps".into(),
        description: "List macOS apps — both currently running and installed-but-not-running — \
            with per-app state flags:\n\n\
            - running: is a process for this app live? (pid is 0 when false)\n\
            - active: is it the system-frontmost app? (implies running)\n\
            - launch_path: filesystem path to the `.app` bundle, when known. \
            Pass this to `launch_app` to start the app cold.\n\
            - kind: `\"desktop\"` for `.app` bundles on macOS.\n\
            - last_used: RFC3339 timestamp from the bundle's filesystem mtime, \
            when readable.\n\n\
            Optional fields (bundle_id, launch_path, kind, last_used) are \
            omitted when unknown, never null.\n\n\
            Standalone running entries include only apps with \
            NSApplicationActivationPolicyRegular — background helpers and \
            system UI agents are filtered out. Installed apps resolve their \
            running/pid state against all live processes by bundle identifier, \
            so an installed app whose process runs as an accessory \
            (LSUIElement / menu-bar apps, e.g. Cua Driver itself) still reports \
            its live pid. Installed apps come from scanning /Applications, \
            /Applications/Utilities, ~/Applications, /System/Applications, and \
            /System/Applications/Utilities.\n\n\
            Use this for \"is X installed?\" as well as \"is X running?\"; \
            the list includes every installed app. To get an open app's pid \
            and window_id, call list_windows({app: \"Name\"}) instead, which \
            also gives per-window state (on-screen, on-current-Space, \
            minimized, window titles). For just opening an \
            app — running or not — call launch_app({bundle_id: ...}) directly; \
            list_apps is not a prerequisite."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }),
        read_only: true,
        destructive: false,
        idempotent: true,
        open_world: false,
    })
}

#[async_trait]
impl Tool for ListAppsTool {
    fn def(&self) -> &ToolDef {
        def()
    }

    async fn invoke(&self, _args: Value) -> ToolResult {
        let apps = tokio::task::spawn_blocking(crate::apps::list_all_apps)
            .await
            .unwrap_or_default();
        let text = crate::apps::format_app_list(&apps);
        let structured = serde_json::json!({
            "apps": apps.iter().map(app_record_json).collect::<Vec<_>>()
        });
        ToolResult::text(text).with_structured(structured)
    }
}

/// One `list_apps` record. `pid`, `name`, `running` and `active` are always
/// present; `bundle_id`, `launch_path`, `kind` and `last_used` are omitted
/// when unknown, as the contract allows. Every agent reads this payload, so it
/// carries no null or always-empty field (0.34 sent `windows: []`).
fn app_record_json(a: &crate::apps::AppInfo) -> Value {
    let mut entry = serde_json::json!({
        "pid": a.pid,
        "name": a.name,
        "bundle_id": a.bundle_id,
        "active": a.active,
        "running": a.running,
        "launch_path": a.launch_path,
        "kind": a.kind,
        "last_used": a.last_used,
    });
    if let Some(entry) = entry.as_object_mut() {
        entry.retain(|_, value| !value.is_null());
    }
    entry
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::AppInfo;

    fn app(bundle_id: Option<&str>, launch_path: Option<&str>) -> AppInfo {
        AppInfo {
            name: "Example".into(),
            pid: 0,
            bundle_id: bundle_id.map(Into::into),
            running: false,
            active: false,
            launch_path: launch_path.map(Into::into),
            kind: Some("desktop".into()),
            last_used: None,
        }
    }

    #[test]
    fn record_omits_unknown_fields_and_the_empty_windows_list() {
        let record = app_record_json(&app(None, None));
        assert_eq!(
            record,
            serde_json::json!({
                "pid": 0,
                "name": "Example",
                "active": false,
                "running": false,
                "kind": "desktop",
            })
        );
        assert!(record.get("windows").is_none());
    }

    #[test]
    fn record_keeps_known_fields_and_matches_the_contract() {
        let record = app_record_json(&app(
            Some("org.example.App"),
            Some("/Applications/Example.app"),
        ));
        assert_eq!(record["bundle_id"], "org.example.App");
        assert_eq!(record["launch_path"], "/Applications/Example.app");
        assert!(record.get("last_used").is_none());
        let typed: cua_driver_contract::ListAppsOutput =
            serde_json::from_value(serde_json::json!({ "apps": [record] })).unwrap();
        assert_eq!(typed.apps[0].bundle_id.as_deref(), Some("org.example.App"));
        assert_eq!(typed.apps[0].last_used, None);
    }
}
