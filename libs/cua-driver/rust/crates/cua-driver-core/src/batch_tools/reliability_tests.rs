//! One test per run_actions failure class seen in bench run v036: the call
//! shapes models sent, and what the batch does with them now.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{json, Value};

use super::{text, Harness};
use crate::{
    protocol::ToolResult,
    tool::{Tool, ToolDef, ToolRegistry},
};

// ── schema leniency ──────────────────────────────────────────────────────

#[tokio::test]
async fn observe_sent_as_a_string_is_read_as_the_boolean() {
    let harness = Harness::new();
    // 21 of the 102 failed v036 calls: `"observe": "true"`.
    let result = harness
        .run(json!({"steps": [{"tool": "click", "args": {"pid": 7, "window_id": 3}}], "observe": "true"}))
        .await;
    assert_ne!(result.is_error, Some(true), "{}", text(&result));
    assert_eq!(harness.hits("get_window_state"), 1);

    // `"observe": "false"` with a trailing read: the read is the observation.
    let result = harness
        .run(json!({"steps": [
            {"tool": "click", "args": {"pid": 7, "window_id": 3}},
            {"tool": "get_window_state", "args": {"pid": 7, "window_id": 3, "max_elements": 100}}
        ], "observe": "false"}))
        .await;
    assert_ne!(result.is_error, Some(true), "{}", text(&result));
    assert_eq!(harness.last("get_window_state")["max_elements"], 100);
}

#[tokio::test]
async fn string_scalars_in_step_args_are_coerced_before_validation() {
    let harness = Harness::new();
    let result = harness
        .run(json!({"steps": [
            {"tool": "click", "args": {"pid": "7", "window_id": "3", "x": "10.5", "y": "4"}}
        ], "observe": {"include_screenshot": "false"}}))
        .await;
    assert_ne!(result.is_error, Some(true), "{}", text(&result));
    let click = harness.last("click");
    assert_eq!(click["pid"], 7);
    assert_eq!(click["window_id"], 3);
    assert_eq!(click["x"], 10.5);
    let observed = harness.last("get_window_state");
    assert_eq!(observed["include_screenshot"], false);
}

#[tokio::test]
async fn batch_level_pid_and_window_id_are_the_default_window() {
    let harness = Harness::new();
    // The v036 shape: pid/window_id once (as strings) on run_actions itself.
    let result = harness
        .run(json!({
            "pid": "37641",
            "window_id": "2126",
            "observe": "false",
            "steps": [
                {"tool": "set_value", "args": {"element_token": "s0000000a:46", "value": "12:00"}},
                {"tool": "set_value", "args": {"element_token": "s0000000a:48", "value": "Juniper"}},
                {"tool": "click", "args": {"pid": 5, "window_id": 6}}
            ]
        }))
        .await;
    assert_ne!(result.is_error, Some(true), "{}", text(&result));
    let calls = harness.calls["set_value"].lock().unwrap().clone();
    assert_eq!(calls.len(), 2);
    for call in &calls {
        assert_eq!(
            (call["pid"].clone(), call["window_id"].clone()),
            (json!(37641), json!(2126))
        );
    }
    // A step's own window wins over the batch default.
    let click = harness.last("click");
    assert_eq!(
        (click["pid"].clone(), click["window_id"].clone()),
        (json!(5), json!(6))
    );

    // Observe needs no window of its own when the batch names one.
    let result = harness
        .run(json!({"pid": 1, "window_id": 2, "steps": [{"press_key": {"key": "down"}}], "observe": true}))
        .await;
    assert_ne!(result.is_error, Some(true), "{}", text(&result));
    let observed = harness.last("get_window_state");
    assert_eq!(
        (observed["pid"].clone(), observed["window_id"].clone()),
        (json!(1), json!(2))
    );
}

#[tokio::test]
async fn an_empty_extra_step_field_is_ignored() {
    let harness = Harness::new();
    // v037a: `"args2": {}` next to a complete step.
    let result = harness
        .run(json!({"steps": [
            {"tool": "click", "args": {"pid": 7, "window_id": 3, "x": 1, "y": 2}, "args2": {}},
            {"tool": "click", "args": {"pid": 7, "window_id": 3}, "note": null}
        ]}))
        .await;
    assert_ne!(result.is_error, Some(true), "{}", text(&result));
    assert_eq!(harness.hits("click"), 2);
    // One that carries something is still refused.
    let refused = harness
        .run(json!({"steps": [{"tool": "click", "args": {"pid": 7}, "args2": {"x": 1}}]}))
        .await;
    assert_eq!(refused.is_error, Some(true));
    assert!(
        text(&refused).contains("unknown field `args2`"),
        "{}",
        text(&refused)
    );
}

#[tokio::test]
async fn a_step_refused_for_background_delivery_says_how_or_falls_back() {
    let harness = Harness::new();
    // v037b: LibreOffice ignores background pixel clicks.
    let refused = "Background pixel click is not available for pid 2261: its libreoffice-vcl toolkit ignores background (PID-routed) mouse events. Retry this action with delivery_mode:\"foreground\"; Cua Driver will activate the window.";
    let result = harness
        .run(json!({"pid": 2261, "window_id": 181, "steps": [
            {"tool": "click", "args": {"x": 217, "y": 779, "fail_message": refused}}
        ]}))
        .await;
    assert_eq!(result.is_error, Some(true));
    assert!(
        text(&result).contains("\"foreground_fallback\":true"),
        "{}",
        text(&result)
    );
    assert!(text(&result).contains("from step 1"), "{}", text(&result));

    // With foreground_fallback the step is retried once in the foreground.
    // (The probe fails on fail_message either way, so check the retry.)
    let before = harness.hits("click");
    let _ = harness
        .run(
            json!({"pid": 2261, "window_id": 181, "foreground_fallback": true, "steps": [
                {"tool": "click", "args": {"x": 217, "y": 779, "fail_message": refused}}
            ]}),
        )
        .await;
    assert_eq!(harness.hits("click"), before + 2);
    assert_eq!(harness.last("click")["delivery_mode"], "foreground");
}

#[test]
fn a_failed_batch_keeps_the_step_report_readable() {
    // v037c: a failed batch returned its step report plus a full read
    // (~4 100 characters); the client cut it to "1. click ok: ... AXCh",
    // hiding why step 2 failed.
    let report =
        "run_actions: step 2 of 2 failed\n1. click ok\n2. click ERROR (action): why".to_owned();
    let mut content = vec![
        crate::protocol::Content::text(report.clone()),
        crate::protocol::Content::text("x".repeat(6_000)),
    ];
    super::super::fit_error_text(&mut content, super::super::ERROR_TEXT_BUDGET);
    let text: String = content
        .iter()
        .filter_map(|part| match part {
            crate::protocol::Content::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(text.starts_with(&report));
    assert!(text.chars().count() <= super::super::ERROR_TEXT_BUDGET + 200);
    assert!(text.contains("call get_window_state for the full window"));
}

#[tokio::test]
async fn a_failed_batch_with_a_long_read_stays_within_the_budget() {
    let harness = Harness::new();
    let result = harness
        .run(json!({"steps": [
            {"tool": "click", "args": {"pid": 7, "window_id": 3}},
            {"tool": "click", "args": {"pid": 7, "window_id": 3, "fail": true}}
        ], "observe": true}))
        .await;
    assert_eq!(result.is_error, Some(true));
    assert!(
        text(&result).contains("2. click ERROR"),
        "{}",
        text(&result)
    );
    assert!(text(&result).chars().count() <= super::super::ERROR_TEXT_BUDGET + 200);
}

#[tokio::test]
async fn batch_level_delivery_mode_is_the_steps_default() {
    let harness = Harness::new();
    // v037c: `delivery_mode` on run_actions itself was refused.
    let result = harness
        .run(
            json!({"pid": 7, "window_id": 3, "delivery_mode": "foreground", "steps": [
                {"tool": "click", "args": {"x": 1, "y": 2}},
                {"tool": "click", "args": {"x": 1, "y": 2, "delivery_mode": "background"}}
            ]}),
        )
        .await;
    assert_ne!(result.is_error, Some(true), "{}", text(&result));
    let calls = harness.calls["click"].lock().unwrap().clone();
    assert_eq!(calls[0]["delivery_mode"], "foreground");
    assert_eq!(
        calls[1]["delivery_mode"], "background",
        "a step's own mode wins"
    );
}

// ── read steps ───────────────────────────────────────────────────────────

#[tokio::test]
async fn a_trailing_zoom_runs_after_the_batch_and_a_screenshot_step_is_observe() {
    let harness = Harness::new();
    let result = harness
        .run(json!({"steps": [
            {"tool": "click", "args": {"pid": 7, "window_id": 3}},
            {"tool": "zoom", "args": {"pid": 7, "window_id": 3, "x": 340, "y": 80, "width": 640, "height": 340}}
        ]}))
        .await;
    assert_ne!(result.is_error, Some(true), "{}", text(&result));
    assert_eq!(harness.hits("zoom"), 1);
    // zoom crops the latest screenshot, so the end read took one first.
    assert_eq!(harness.last("get_window_state")["include_screenshot"], true);
    let zoom = harness.last("zoom");
    assert_eq!(
        zoom["x2"], 980.0,
        "zoom's own x/y/width/height aliases apply"
    );
    assert!(text(&result).contains("zoom: ok"), "{}", text(&result));

    let result = harness
        .run(json!({"steps": [
            {"tool": "click", "args": {"pid": 7, "window_id": 3, "x": 1, "y": 1}},
            {"tool": "screenshot_placeholder", "args": {}}
        ]}))
        .await;
    assert_ne!(result.is_error, Some(true), "{}", text(&result));
    assert_eq!(harness.last("get_window_state")["include_screenshot"], true);
}

// ── tool and key shapes ──────────────────────────────────────────────────

#[tokio::test]
async fn tool_name_slips_become_the_action_they_mean() {
    let harness = Harness::new();
    let result = harness
        .run(json!({"pid": 9, "window_id": 4, "steps": [
            {"tool": "triple_click", "args": {"x": 186, "y": 254}},
            {"tool": "down", "args": {}},
            {"tool": "hotkey", "args": {"keys": ["Right"]}},
            {"tool": "press_key", "args": {"key": "asterisk"}},
            {"tool": "press_key", "args": {"key": "shift+Right"}},
            {"tool": "hotkey", "args": {"keys": ["ctrl", "Page_Down"]}},
            {"tool": "move_cursor", "args": {"x": 640, "y": 190}}
        ]}))
        .await;
    assert_ne!(result.is_error, Some(true), "{}", text(&result));
    assert_eq!(harness.last("click")["count"], 3);
    let presses = harness.calls["press_key"].lock().unwrap().clone();
    assert_eq!(presses.len(), 3);
    assert_eq!(presses[0]["key"], "down");
    assert_eq!(presses[1]["key"], "Right");
    assert_eq!(presses[2]["key"], "Right");
    assert_eq!(presses[2]["modifiers"], json!(["shift"]));
    assert_eq!(harness.last("type_text")["text"], "*");
    assert_eq!(harness.last("hotkey")["keys"], json!(["ctrl", "pagedown"]));
    assert_eq!(harness.hits("move_cursor"), 1);

    // v037-full: "press" with an element target is a click on it; with a
    // key it stays a key press.
    let result = harness
        .run(json!({"pid": 9, "window_id": 4, "steps": [
            {"tool": "press", "args": {"element_token": "s00000001:33"}},
            {"tool": "press", "args": {"key": "tab"}}
        ]}))
        .await;
    assert_ne!(result.is_error, Some(true), "{}", text(&result));
    assert_eq!(harness.last("click")["element_token"], "s00000001:33");
    assert_eq!(harness.last("press_key")["key"], "tab");
}

// ── recovery after a failed step ─────────────────────────────────────────

#[tokio::test]
async fn a_dialog_holding_focus_is_read_after_the_failure() {
    let harness = Harness::new();
    let message = "press_key delivery failed: exact target window did not become focused for \
                   foreground HID delivery: window_id 1112 \"Delete Contents\" of the same app \
                   holds focus (an open dialog or panel); act on that window_id, or close it first";
    let result = harness
        .run(json!({"steps": [
            {"tool": "press_key", "args": {"pid": 13051, "window_id": 1069, "key": "tab", "fail_message": message}},
            {"tool": "press_key", "args": {"pid": 13051, "window_id": 1069, "key": "tab"}}
        ]}))
        .await;
    assert_eq!(result.is_error, Some(true));
    let observed = harness.last("get_window_state");
    assert_eq!(observed["pid"], 13051);
    assert_eq!(
        observed["window_id"], 1112,
        "the dialog, not the blocked window"
    );
    assert!(
        text(&result).contains("recovery read: window 1112"),
        "{}",
        text(&result)
    );
}

#[tokio::test]
async fn a_window_never_captured_gets_a_screenshot_after_a_pixel_failure() {
    let harness = Harness::new();
    let message = "No screenshot of window 1320 in this session: it has not been read yet.";
    let result = harness
        .run(json!({"steps": [
            {"tool": "click", "args": {"pid": 19316, "window_id": 1320, "x": 1128, "y": 612, "fail_message": message}}
        ], "observe": {"query": "AXTextField"}}))
        .await;
    assert_eq!(result.is_error, Some(true));
    let observed = harness.last("get_window_state");
    assert_eq!(observed["window_id"], 1320);
    assert_eq!(observed["include_screenshot"], true);
    assert_eq!(
        observed["query"], "AXTextField",
        "the caller's observe is kept"
    );
}

#[test]
fn focus_holder_and_row_parsing() {
    assert_eq!(
        super::super::focus_holder(
            "click failed: ...: window_id 5256 \"Welcome to LibreOffice!\" of the same app holds focus (an open dialog)"
        ),
        Some(5256)
    );
    assert_eq!(
        super::super::focus_holder("exact target window did not become focused"),
        None
    );
    assert_eq!(
        super::super::row_role_and_label(
            "  - [14] AXButton \"Cinder decision LHP-CINDER\" [actions=[press]]"
        ),
        Some((
            "AXButton".to_owned(),
            "Cinder decision LHP-CINDER".to_owned()
        ))
    );
    assert_eq!(
        super::super::row_role_and_label(
            "- [17] AXButton (Lighthouse lunch hold (protected)) [actions=[press]]"
        ),
        Some((
            "AXButton".to_owned(),
            "Lighthouse lunch hold (protected)".to_owned()
        ))
    );
    assert_eq!(
        super::super::row_role_and_label("- [12] AXStaticText = \"Boreal review\""),
        None,
        "a row without a label cannot be found again safely"
    );
}

#[tokio::test]
async fn a_menu_bar_item_token_runs_as_invoke_menu() {
    let harness = Harness::new();
    // v037-full: a click on the LibreOffice menu item "To Next Sheet" by
    // token is refused as outside the window (menu-bar menus are the app's).
    crate::window_state_view::remember_for_test(
        "s0000a0e1",
        80,
        90,
        "- [0] AXWindow \"Doc\"\n- [27] AXMenuBar\n  - [700] AXMenuBarItem \"Sheet\"\n    - [701] AXMenu\n      - [760] AXMenuItem \"Navigate\"\n        - [761] AXMenu\n          - [771] AXMenuItem \"To Next Sheet\"\n",
    );
    let refused = "Background input refused (element_outside_target_window): the addressed element could not be proven to belong to window 90";
    let result = harness
        .run(json!({"steps": [{"tool": "click", "args": {"pid": 80, "window_id": 90, "element_token": "s0000a0e1:771", "fail_message": refused}}]}))
        .await;
    // The probe invoke_menu carries no action record, so only the routing
    // is checked here; the real tool reports its own outcome.
    let menu = harness.last("invoke_menu");
    assert_eq!(menu["path"], json!(["Sheet", "Navigate", "To Next Sheet"]));
    assert_eq!(
        (menu["pid"].clone(), menu["window_id"].clone()),
        (json!(80), json!(90))
    );
    assert!(
        text(&result).contains("ran as invoke_menu Sheet > Navigate > To Next Sheet"),
        "{}",
        text(&result)
    );
}

// ── stale element targets ────────────────────────────────────────────────

/// A window whose list re-renders on every click: element handles from an
/// older snapshot are dead, as in Chromium/Electron web content.
struct Rerender {
    def: ToolDef,
    state: Arc<Mutex<RerenderState>>,
}

#[derive(Default)]
struct RerenderState {
    snapshot: u32,
    /// Labels of the current snapshot, by row.
    rows: Vec<(&'static str, &'static str)>,
    clicked: Vec<String>,
}

#[async_trait]
impl Tool for Rerender {
    fn def(&self) -> &ToolDef {
        &self.def
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let mut state = self.state.lock().unwrap();
        match self.def.name.as_str() {
            "get_window_state" => {
                state.snapshot += 1;
                let snapshot = format!("s{:08x}", state.snapshot);
                let elements: Vec<Value> = state
                    .rows
                    .iter()
                    .enumerate()
                    .map(|(index, (role, label))| {
                        json!({
                            "element_index": index + 1,
                            "element_token": format!("{snapshot}:{}", index + 1),
                            "role": role,
                            "depth": 1,
                            "label": label,
                        })
                    })
                    .collect();
                ToolResult::text("read").with_structured(json!({
                    "pid": 42, "window_id": 7, "snapshot_id": snapshot,
                    "elements": elements, "tree_markdown": "",
                }))
            }
            "click" => {
                let token = args["element_token"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                let current = format!("s{:08x}:", state.snapshot);
                if !token.starts_with(&current) {
                    return ToolResult::error(
                        "Background input refused (element_outside_target_window): the addressed \
                         element could not be proven to belong to window 7; take a fresh \
                         get_window_state snapshot and re-address it",
                    );
                }
                let row: usize = token[current.len()..].parse().unwrap();
                let label = state.rows[row - 1].1.to_owned();
                state.clicked.push(label.clone());
                ToolResult::text(format!("clicked {label}"))
            }
            "list_windows" => ToolResult::text("windows").with_structured(json!({"windows": [{
                "pid": 42, "window_id": 7, "app_name": "Daymark", "title": "Daymark",
                "z_index": 1, "is_on_screen": true,
                "bounds": {"x": 0.0, "y": 0.0, "width": 800.0, "height": 600.0}
            }]})),
            _ => ToolResult::text("ok"),
        }
    }
}

fn rerender_registry(
    rows: Vec<(&'static str, &'static str)>,
) -> (Arc<ToolRegistry>, Arc<Mutex<RerenderState>>) {
    let state = Arc::new(Mutex::new(RerenderState {
        snapshot: 40,
        rows,
        ..Default::default()
    }));
    let mut registry = ToolRegistry::new();
    for name in ["click", "get_window_state", "list_windows", "list_apps"] {
        registry.register(Box::new(Rerender {
            def: ToolDef {
                name: name.into(),
                description: "fake".into(),
                input_schema: json!({"type": "object", "properties": {
                    "pid": {"type": "integer"}, "window_id": {"type": "integer"},
                    "element_token": {"type": "string"}
                }}),
                read_only: name != "click",
                destructive: false,
                idempotent: false,
                open_world: false,
            },
            state: state.clone(),
        }));
    }
    registry.register_session_tools();
    let registry = Arc::new(registry);
    registry.init_self_weak();
    (registry, state)
}

#[tokio::test]
async fn a_stale_element_token_is_found_again_by_its_role_and_label() {
    let (registry, state) = rerender_registry(vec![
        ("AXButton", "Update event"),
        ("AXButton", "Aster kickoff"),
        ("AXButton", "Cinder decision LHP-CINDER"),
    ]);
    // The read the model took its token from (snapshot s0000002c, row 14),
    // remembered as get_window_state remembers every read.
    crate::window_state_view::remember_for_test(
        "s0000002c",
        42,
        7,
        "- [0] AXWindow \"Daymark\"\n  - [14] AXButton \"Cinder decision LHP-CINDER\" [actions=[press]]\n",
    );
    let result = registry
        .invoke(
            super::super::RUN_ACTIONS_TOOL,
            json!({"steps": [{"tool": "click", "args": {"pid": 42, "window_id": 7, "element_token": "s0000002c:14"}}]}),
        )
        .await;
    assert_ne!(result.is_error, Some(true), "{}", text(&result));
    assert_eq!(
        state.lock().unwrap().clicked,
        vec!["Cinder decision LHP-CINDER"]
    );
    assert!(text(&result).contains("went stale"), "{}", text(&result));
}

#[tokio::test]
async fn a_stale_token_is_not_retried_when_its_label_is_ambiguous_or_unknown() {
    let (registry, state) = rerender_registry(vec![("AXButton", "OK"), ("AXButton", "OK")]);
    crate::window_state_view::remember_for_test(
        "s0000002d",
        42,
        7,
        "- [3] AXButton \"OK\"\n- [4] AXStaticText = \"no label\"\n",
    );
    for token in ["s0000002d:3", "s0000002d:4", "s0000002d:99", "s0000777f:3"] {
        let result = registry
            .invoke(
                super::super::RUN_ACTIONS_TOOL,
                json!({"steps": [{"tool": "click", "args": {"pid": 42, "window_id": 7, "element_token": token}}]}),
            )
            .await;
        assert_eq!(result.is_error, Some(true), "{token}");
        assert!(
            text(&result).contains("element_outside_target_window"),
            "{token}: {}",
            text(&result)
        );
    }
    assert!(state.lock().unwrap().clicked.is_empty());
}
