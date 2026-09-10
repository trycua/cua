use super::*;
use cua_driver_contract::{ActionTarget, ClickPosition, InputDeliveryMode};
use serde_json::json;

fn success(tool: &str, structured: Value) -> ToolResult {
    normalize_result(
        tool,
        json!({"content": [], "structuredContent": structured, "isError": false}),
    )
    .unwrap()
}

#[tokio::test]
async fn portable_inputs_match_the_in_process_native_registry() {
    let driver = CuaDriver::create(None).unwrap();
    let live: Value = serde_json::from_str(&driver.list_tools_json().await.unwrap()).unwrap();
    let tools = live["tools"].as_array().expect("native tools list");
    for contract in cua_driver_contract::manifest().tools {
        let native = tools
            .iter()
            .find(|tool| tool["name"] == contract.name)
            .unwrap_or_else(|| panic!("missing native tool {}", contract.name));
        let violations = cua_driver_contract::compatibility::schema_subset_violations(
            &contract.input_schema,
            &native["inputSchema"],
        );
        assert!(violations.is_empty(), "{}: {violations:?}", contract.name);
        assert_eq!(
            native["annotations"],
            json!({
                "readOnlyHint": contract.annotations.read_only,
                "destructiveHint": contract.annotations.destructive,
                "idempotentHint": contract.annotations.idempotent,
                "openWorldHint": contract.annotations.open_world,
            }),
            "{} annotations differ from the portable contract",
            contract.name
        );
    }
    driver.shutdown().await.unwrap();
}

#[test]
fn typed_discovery_decodes_native_platform_variants() {
    for platform in ["macos", "windows", "x11", "wayland"] {
        let mut app = json!({"pid": 42, "name": "Editor", "running": true, "active": false});
        let mut window = json!({
            "window_id": 73, "pid": 42, "app_name": "Editor", "title": "Document",
            "bounds": {"x": -20, "y": 10, "width": 600, "height": 400},
            "is_on_screen": true, "z_index": 2
        });
        match platform {
            "macos" => {
                app["bundle_id"] = json!("example.editor");
                window["space_ids"] = json!([1, 2]);
                window["on_current_space"] = json!(true);
            }
            "windows" => window["minimized"] = json!(false),
            "wayland" => {
                window["pid"] = Value::Null;
                window["z_index"] = Value::Null;
            }
            _ => {}
        }
        let apps: ListAppsOutput = success("list_apps", json!({"apps": [app]}))
            .typed_success("list_apps")
            .unwrap();
        let windows: ListWindowsOutput = success("list_windows", json!({"windows": [window]}))
            .typed_success("list_windows")
            .unwrap();
        assert_eq!(apps.apps[0].pid, 42, "{platform}");
        assert_eq!(windows.windows[0].window_id, 73, "{platform}");
        assert_eq!(windows.windows[0].bounds.x, -20.0);
        assert_eq!(windows.windows[0].z_index.is_none(), platform == "wayland");
    }
}

#[test]
fn typed_methods_preserve_both_native_refusal_shapes() {
    for refusal in [
        json!({"code":"surface_identity_unproven", "message":"Window is not proven", "effect":"refused"}),
        json!({"status":"refused", "refusal":{"code":"surface_identity_unproven", "message":"Window is not proven"}}),
    ] {
        for tool in ["list_apps", "list_windows", "get_window_state", "click"] {
            let result = normalize_result(
                tool,
                json!({
                    "isError":true, "content":[{"type":"text","text":"Diagnostic details"}],
                    "structuredContent":refusal
                }),
            )
            .unwrap();
            let error = match tool {
                "list_apps" => result.typed_success::<ListAppsOutput>(tool).unwrap_err(),
                "list_windows" => result.typed_success::<ListWindowsOutput>(tool).unwrap_err(),
                "get_window_state" => result.window_state_success().unwrap_err(),
                _ => result.typed_success::<ActionResult>(tool).unwrap_err(),
            };
            assert!(
                matches!(error, DriverError::Tool { error_code, message, .. }
                if error_code == "surface_identity_unproven" && message == "Window is not proven")
            );
        }
    }
}

#[test]
fn typed_discovery_rejects_missing_and_malformed_success() {
    for structured in [
        Value::Null,
        json!({}),
        json!({"apps":"wrong", "windows":42}),
    ] {
        assert!(matches!(
            success("list_apps", structured.clone()).typed_success::<ListAppsOutput>("list_apps"),
            Err(DriverError::Protocol { .. })
        ));
        assert!(matches!(
            success("list_windows", structured.clone())
                .typed_success::<ListWindowsOutput>("list_windows"),
            Err(DriverError::Protocol { .. })
        ));
        assert!(matches!(
            success("get_window_state", structured).window_state_success(),
            Err(DriverError::Protocol { .. })
        ));
    }
}

fn snapshot_envelope() -> Value {
    json!({
        "isError": false,
        "content": [
            {"type":"image", "mimeType":"image/png", "data":"cG5n"},
            {"type":"image", "mimeType":"image/png", "data":"bW9yZQ=="}
        ],
        "structuredContent": {
            "pid":42, "window_id":73, "snapshot_id":"s1",
            "screenshot_width":600, "screenshot_height":400, "screenshot_mime_type":"image/png",
            "elements":[{"element_index":0,"element_token":"s1:0","role":"button","depth":0,"label":"Save"}],
            "degraded":true, "degraded_reason":"Partial tree", "truncated":true
        }
    })
}

#[test]
fn typed_snapshot_preserves_all_images_and_observation_metadata() {
    let output = normalize_result("get_window_state", snapshot_envelope())
        .unwrap()
        .window_state_success()
        .unwrap();
    assert_eq!(output.images.len(), 2);
    assert_eq!(output.images[0].data_base64, "cG5n");
    assert_eq!(output.images[1].data_base64, "bW9yZQ==");
    assert_eq!(output.snapshot_id.as_deref(), Some("s1"));
    assert_eq!(
        output.elements.unwrap()[0].element_token.as_deref(),
        Some("s1:0")
    );
    assert_eq!(output.degraded, Some(true));
    assert_eq!(output.truncated, Some(true));
}

#[test]
fn typed_snapshot_rejects_dropped_images_and_inconsistent_metadata() {
    for (pointer, value) in [
        ("/content/0/data", Value::Null),
        ("/content/0/data", json!("")),
        ("/content/0/mimeType", json!("text/plain")),
        ("/structuredContent/screenshot_width", json!(0)),
        ("/structuredContent/screenshot_height", Value::Null),
        (
            "/structuredContent/screenshot_mime_type",
            json!("image/jpeg"),
        ),
    ] {
        let mut envelope = snapshot_envelope();
        *envelope.pointer_mut(pointer).unwrap() = value;
        assert!(
            matches!(
                normalize_result("get_window_state", envelope)
                    .unwrap()
                    .window_state_success(),
                Err(DriverError::Protocol { .. })
            ),
            "{pointer}"
        );
    }
}

#[test]
fn typed_snapshot_retains_capture_failure_and_file_only_states() {
    for metadata in [
        json!({"pid":42,"window_id":73,"screenshot_frame_valid":false,"degraded":true}),
        json!({"pid":42,"window_id":73,"screenshot_width":600,"screenshot_height":400,"screenshot_file_path":"/tmp/snapshot.png"}),
    ] {
        let output = success("get_window_state", metadata)
            .window_state_success()
            .unwrap();
        assert!(output.images.is_empty());
        assert!(
            output.screenshot_frame_valid == Some(false) || output.screenshot_file_path.is_some()
        );
    }
}

#[tokio::test]
#[cfg(unix)]
async fn typed_discovery_requests_use_the_existing_daemon_transport() {
    let (_directory, socket, server) = serve_once(
        json!({"ok":true,"result":{"structuredContent":{"apps":[]},"content":[],"isError":false}}),
    );
    let driver = CuaDriver::connect(Some(socket)).unwrap();
    assert!(driver
        .list_apps(ListAppsInput {})
        .await
        .unwrap()
        .apps
        .is_empty());
    let request = server.join().unwrap();
    assert_eq!(request["name"], "list_apps");
    assert_eq!(request["args"], json!({}));

    let (_directory, socket, server) = serve_once(
        json!({"ok":true,"result":{"structuredContent":{"windows":[]},"content":[],"isError":false}}),
    );
    let driver = CuaDriver::connect(Some(socket)).unwrap();
    assert!(driver
        .list_windows(ListWindowsInput {
            pid: Some(42),
            on_screen_only: Some(true)
        })
        .await
        .unwrap()
        .windows
        .is_empty());
    let request = server.join().unwrap();
    assert_eq!(request["name"], "list_windows");
    assert_eq!(request["args"], json!({"pid":42,"on_screen_only":true}));
}

#[tokio::test]
#[cfg(unix)]
async fn typed_click_returns_action_facts_and_flat_native_arguments() {
    let (_directory, socket, server) = serve_once(json!({"ok":true,"result":{
        "structuredContent":{"effect":"confirmed","route":"accessibility","delivery":{"mode":"background"},"evidence":[{"kind":"value_readback"}]},
        "content":[],"isError":false
    }}));
    let driver = CuaDriver::connect(Some(socket)).unwrap();
    let output: ActionResult = driver
        .click(ClickInput {
            target: ActionTarget::Window {
                pid: 42,
                window_id: 73,
            },
            position: ClickPosition::Coordinates { x: 10.5, y: 20.0 },
            delivery_mode: InputDeliveryMode::Background,
            session: Some("run-1".into()),
            button: None,
            count: None,
        })
        .await
        .unwrap();
    assert_eq!(output.effect, cua_driver_contract::ActionEffect::Confirmed);
    let request = server.join().unwrap();
    assert_eq!(request["name"], "click");
    assert_eq!(
        request["args"],
        json!({"target":{"kind":"window","pid":42,"window_id":73},"x":10.5,"y":20.0,"delivery_mode":"background","session":"run-1"})
    );
}

#[tokio::test]
#[cfg(unix)]
async fn typed_snapshot_request_preserves_target_and_session() {
    let (_directory, socket, server) = serve_once(json!({"ok":true,"result":snapshot_envelope()}));
    let driver = CuaDriver::connect(Some(socket)).unwrap();
    let input: GetWindowStateInput = serde_json::from_value(json!({
        "pid":42,"window_id":73,"session":"run-1","include_screenshot":true,"max_elements":10
    }))
    .unwrap();
    let output = driver.get_window_state(input).await.unwrap();
    assert_eq!(output.images.len(), 2);
    let request = server.join().unwrap();
    assert_eq!(request["name"], "get_window_state");
    assert_eq!(
        request["args"],
        json!({"pid":42,"window_id":73,"session":"run-1","include_screenshot":true,"max_elements":10})
    );
}

#[tokio::test]
async fn invalid_typed_click_is_rejected_before_transport() {
    let driver = CuaDriver::connect(Some("/nonexistent/typed-click-test.sock".into())).unwrap();
    for (target, position, delivery_mode) in [
        (
            ActionTarget::Desktop {
                display_id: "primary".into(),
            },
            ClickPosition::Coordinates { x: 1.0, y: 2.0 },
            InputDeliveryMode::Background,
        ),
        (
            ActionTarget::Window {
                pid: 42,
                window_id: 73,
            },
            ClickPosition::Coordinates {
                x: f64::NAN,
                y: 2.0,
            },
            InputDeliveryMode::Background,
        ),
        (
            ActionTarget::Desktop {
                display_id: "primary".into(),
            },
            ClickPosition::Element {
                element_token: "s1:0".into(),
            },
            InputDeliveryMode::Foreground,
        ),
    ] {
        let error = driver
            .click(ClickInput {
                target,
                position,
                delivery_mode,
                session: None,
                button: None,
                count: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(error, DriverError::InvalidArguments { tool, .. } if tool == "click"));
    }
}
