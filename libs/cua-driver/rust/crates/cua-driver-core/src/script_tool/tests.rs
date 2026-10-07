//! `run_script` against fake driver tools: results, errors, limits and
//! sandbox-escape attempts.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::{
    protocol::{Content, ToolResult},
    tool::{Tool, ToolDef, ToolRegistry},
};

type Calls = Arc<Mutex<Vec<(String, Value)>>>;

/// Fake app: pid 42, window 7, a "Name" field and a "Save" button.
struct Fake {
    def: ToolDef,
    calls: Calls,
}

#[async_trait]
impl Tool for Fake {
    fn def(&self) -> &ToolDef {
        &self.def
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let name = self.def.name.clone();
        self.calls
            .lock()
            .unwrap()
            .push((name.clone(), args.clone()));
        match name.as_str() {
            "list_windows" => ToolResult::text("windows").with_structured(json!({"windows": [{
                "pid": 42, "window_id": 7, "app_name": "Notes", "title": "Draft",
                "z_index": 1, "is_on_screen": true,
                "bounds": {"x": 0.0, "y": 0.0, "width": 800.0, "height": 600.0}
            }]})),
            "list_apps" => ToolResult::text("apps").with_structured(json!({"apps": [
                {"pid": 42, "name": "Notes", "bundle_id": "com.example.notes", "running": true, "active": true}
            ]})),
            "get_window_state" => ToolResult::text("- [0] AXWindow \"Draft\"").with_structured(json!({
                "pid": 42, "window_id": 7, "snapshot_id": "s00000009",
                "tree_markdown": "- [0] AXWindow \"Draft\"\n  - [1] AXTextField \"Name\"\n  - [2] AXButton \"Save\"\n  - AXStaticText \"Saved\"\n",
                "elements": [
                    {"element_index": 1, "element_token": "s00000009:1", "role": "AXTextField", "label": "Name", "value": "Ada", "depth": 1},
                    {"element_index": 2, "element_token": "s00000009:2", "role": "AXButton", "label": "Save", "depth": 1}
                ]
            })),
            "launch_app" => ToolResult::text("launched").with_structured(json!({"pid": 42})),
            _ if args["fail"] == true => ToolResult::error(format!("{name} refused: asked to fail")),
            _ => ToolResult::text(format!("{name} done")),
        }
    }
}

struct Harness {
    registry: Arc<ToolRegistry>,
    calls: Calls,
}

impl Harness {
    fn new() -> Self {
        let calls: Calls = Arc::default();
        let mut registry = ToolRegistry::new();
        for name in crate::batch_tools::BATCHABLE_TOOLS
            .iter()
            .copied()
            .chain(super::READ_TOOLS.iter().copied())
            .chain(["kill_app"])
        {
            registry.register(Box::new(Fake {
                def: ToolDef {
                    name: name.into(),
                    description: "fake".into(),
                    input_schema: json!({"type": "object"}),
                    read_only: matches!(name, "get_window_state" | "list_windows" | "list_apps"),
                    destructive: false,
                    idempotent: false,
                    open_world: false,
                },
                calls: calls.clone(),
            }));
        }
        registry.register_session_tools();
        registry.register_script_tool();
        let registry = Arc::new(registry);
        registry.init_self_weak();
        Self { registry, calls }
    }

    async fn run(&self, script: &str) -> ToolResult {
        self.run_with(json!({ "script": script })).await
    }

    async fn run_with(&self, args: Value) -> ToolResult {
        self.registry.invoke(super::RUN_SCRIPT_TOOL, args).await
    }

    fn calls(&self, tool: &str) -> Vec<Value> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(name, _)| name == tool)
            .map(|(_, args)| args.clone())
            .collect()
    }
}

fn text(result: &ToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|content| match content {
            Content::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn structured(result: &ToolResult) -> Value {
    result.structured_content.clone().unwrap_or(Value::Null)
}

#[tokio::test]
async fn a_script_acts_by_name_and_returns_value_console_and_call_log() {
    let harness = Harness::new();
    let result = harness
        .run(
            r#"const app = await cua.getApp("Notes");
console.log("found", app.name, {window: app.windowId});
await app.setValue({role: "textfield", name: "Name"}, "Grace");
await app.click({role: "button", name: "Save"});
const [field] = await app.query({role: "textbox", name: "name"});
await app.waitFor({text: "Saved"});
return {value: field.value, platform: cua.computer.target};"#,
        )
        .await;
    assert_ne!(result.is_error, Some(true), "{}", text(&result));
    let out = structured(&result);
    assert_eq!(out["value"]["value"], "Ada");
    assert_eq!(out["console"][0], "found Notes {\"window\":7}");
    let click = &harness.calls("click")[0];
    assert_eq!(click["element_token"], "s00000009:2");
    assert_eq!(click["pid"], 42);
    assert_eq!(click["window_id"], 7);
    assert!(
        click.get("role").is_none(),
        "selector keys never reach the tool"
    );
    assert_eq!(harness.calls("set_value")[0]["value"], "Grace");
    let log = out["calls"].as_array().unwrap();
    assert!(
        log.iter()
            .any(|entry| entry["op"] == "click" && entry["line"] == 4),
        "{log:?}"
    );
    assert!(
        text(&result).contains("3. line 4 click ok"),
        "{}",
        text(&result)
    );
}

#[tokio::test]
async fn an_uncaught_driver_error_names_the_line_the_call_and_the_reason() {
    let harness = Harness::new();
    let result = harness
        .run("const app = await cua.getApp('Notes');\n\nawait app.click({role: 'button', name: 'Publish'}, {timeout_ms: 0});\nreturn 1;")
        .await;
    assert_eq!(result.is_error, Some(true));
    let message = text(&result);
    assert!(
        message.starts_with("run_script: ERROR (exception) at line 3, call 2 (click): click failed: no button \"Publish\""),
        "{message}"
    );
    assert!(
        message.contains("nearest: [2] AXButton \"Save\""),
        "{message}"
    );
    let error = &structured(&result)["error"];
    assert_eq!(error["line"], 3);
    assert_eq!(error["code"], "not_found");
    assert!(harness.calls("click").is_empty());
}

#[tokio::test]
async fn a_script_can_catch_a_failed_call_and_carry_on() {
    let harness = Harness::new();
    let result = harness
        .run("const app = await cua.getApp('Notes');\ntry { await app.click('s00000009:2', {fail: true}); } catch (e) { console.log(e.code, e.op); }\nawait app.pressKey('cmd+s');\nreturn 'ok';")
        .await;
    assert_ne!(result.is_error, Some(true), "{}", text(&result));
    assert_eq!(structured(&result)["console"][0], "tool_error click");
    assert_eq!(harness.calls("hotkey")[0]["keys"], json!(["cmd", "s"]));
}

#[tokio::test]
async fn the_sandbox_has_no_io_and_no_way_around_the_api() {
    let harness = Harness::new();
    let result = harness
        .run(
            r#"const probes = {};
for (const name of ["require", "process", "fetch", "std", "os", "XMLHttpRequest", "WebSocket", "setTimeout", "Deno", "Bun", "__host", "__done", "__log", "__settle"]) {
  probes[name] = typeof globalThis[name];
}
probes.viaFunction = typeof Function("return this")().__host;
probes.viaConstructor = (() => {}).constructor.constructor("return typeof process")();
try { await import("fs"); probes.import = "loaded"; } catch (e) { probes.import = String(e); }
try { cua.call = null; probes.frozen = cua.call === null ? "mutated" : "frozen"; } catch (e) { probes.frozen = "frozen"; }
return probes;"#,
        )
        .await;
    assert_ne!(result.is_error, Some(true), "{}", text(&result));
    let probes = &structured(&result)["value"];
    for name in [
        "require",
        "process",
        "fetch",
        "std",
        "os",
        "XMLHttpRequest",
        "WebSocket",
        "setTimeout",
        "Deno",
        "Bun",
        "__host",
        "__done",
        "__log",
        "__settle",
        "viaFunction",
    ] {
        assert_eq!(probes[name], "undefined", "{name}: {probes}");
    }
    assert_eq!(probes["viaConstructor"], "undefined");
    assert!(
        probes["import"]
            .as_str()
            .unwrap()
            .contains("could not load module"),
        "{probes}"
    );
    assert_eq!(probes["frozen"], "frozen");
}

#[tokio::test]
async fn only_driver_tools_on_the_allowlist_are_reachable() {
    let harness = Harness::new();
    for (script, needle) in [
        (
            "await cua.call('kill_app', {pid: 42})",
            "not available to scripts",
        ),
        (
            "await cua.call('run_actions', {steps: []})",
            "not available to scripts",
        ),
        (
            "await cua.call('run_script', {script: '1'})",
            "not available to scripts",
        ),
        (
            "await cua.call('set_config', {key: 'experimental_script', value: true})",
            "not available to scripts",
        ),
        (
            "await cua.call('click', {pid: 42, _observation_only: true})",
            "reserved",
        ),
        (
            "await cua.call('click', {pid: 42, session: 'someone-else'})",
            "session",
        ),
    ] {
        let result = harness.run(script).await;
        assert_eq!(result.is_error, Some(true), "{script}");
        assert!(
            text(&result).contains(needle),
            "{script}: {}",
            text(&result)
        );
    }
    assert!(harness.calls("kill_app").is_empty());
    assert!(harness.calls("click").is_empty());
}

#[tokio::test]
async fn an_endless_loop_stops_at_the_wall_time_limit() {
    let harness = Harness::new();
    let started = Instant::now();
    let result = harness
        .run_with(json!({"script": "while (true) {}", "timeout_ms": 300}))
        .await;
    assert_eq!(result.is_error, Some(true));
    assert_eq!(structured(&result)["error"]["kind"], "timeout");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );

    // A catch block cannot swallow the interrupt.
    let result = harness
        .run_with(json!({"script": "try { while (true) {} } catch (e) {} return 'escaped';", "timeout_ms": 300}))
        .await;
    assert_eq!(structured(&result)["error"]["kind"], "timeout");

    // Nor can a long sleep outlast the limit.
    let started = Instant::now();
    let result = harness
        .run_with(json!({"script": "await cua.sleep(60000); return 'late';", "timeout_ms": 300}))
        .await;
    assert_eq!(
        structured(&result)["error"]["kind"],
        "timeout",
        "{}",
        text(&result)
    );
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[tokio::test]
async fn memory_and_stack_are_bounded() {
    let harness = Harness::new();
    let result = harness
        .run("const chunks = []; while (true) chunks.push('x'.repeat(1 << 16));")
        .await;
    assert_eq!(result.is_error, Some(true));
    assert_eq!(
        structured(&result)["error"]["kind"],
        "memory",
        "{}",
        text(&result)
    );

    let result = harness
        .run("function down() { return down(); }\ndown();")
        .await;
    assert_eq!(result.is_error, Some(true));
    assert!(
        text(&result).contains("Maximum call stack size exceeded"),
        "{}",
        text(&result)
    );
    assert_eq!(structured(&result)["error"]["line"], 1);
}

#[tokio::test]
async fn the_driver_call_limit_holds() {
    let harness = Harness::new();
    let result = harness
        .run_with(json!({
            "script": "for (let i = 0; i < 10; i++) await cua.call('press_key', {pid: 42, key: 'a'});",
            "max_calls": 3
        }))
        .await;
    assert_eq!(result.is_error, Some(true));
    assert!(
        text(&result).contains("driver call limit reached (max_calls=3)"),
        "{}",
        text(&result)
    );
    assert_eq!(structured(&result)["error"]["code"], "call_limit");
    assert_eq!(harness.calls("press_key").len(), 3);
}

#[tokio::test]
async fn syntax_errors_and_stalled_promises_are_named() {
    let harness = Harness::new();
    let result = harness.run("const a = 1;\nconst b = ;").await;
    assert_eq!(
        structured(&result)["error"]["kind"],
        "syntax",
        "{}",
        text(&result)
    );
    assert_eq!(structured(&result)["error"]["line"], 2);

    let result = harness.run("await new Promise(() => {});").await;
    assert_eq!(
        structured(&result)["error"]["kind"],
        "stalled",
        "{}",
        text(&result)
    );

    let result = harness.run("throw new TypeError('nope');").await;
    assert!(
        text(&result).contains("ERROR (exception) at line 1: TypeError: nope"),
        "{}",
        text(&result)
    );
}

#[tokio::test]
async fn dropping_the_request_cancels_the_script() {
    let harness = Harness::new();
    let script = "while (true) { await cua.call('list_apps', {}); await cua.sleep(20); }";
    let call = harness.run_with(json!({"script": script, "max_calls": 500, "timeout_ms": 60000}));
    let _ = tokio::time::timeout(Duration::from_millis(300), call).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let after_cancel = harness.calls("list_apps").len();
    assert!(after_cancel > 0, "the script ran");
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(
        harness.calls("list_apps").len(),
        after_cancel,
        "no driver call after the request was dropped"
    );
}

#[tokio::test]
async fn bad_requests_are_rejected_before_running() {
    let harness = Harness::new();
    for args in [
        json!({}),
        json!({"script": ""}),
        json!({"script": "1", "timeout_ms": 999999}),
        json!({"script": "1", "max_calls": 0}),
        json!({"script": "1", "extra": true}),
    ] {
        let result = harness.run_with(args.clone()).await;
        assert_eq!(result.is_error, Some(true), "{args}");
    }
}

#[test]
fn the_tool_is_off_unless_the_operator_opts_in() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.json");
    assert!(!super::enabled_from(None, None), "off by default");
    assert!(!super::enabled_from(None, Some(&config)), "no config file");
    std::fs::write(&config, r#"{"experimental_pip": true}"#).unwrap();
    assert!(!super::enabled_from(None, Some(&config)));
    std::fs::write(&config, r#"{"experimental_script": true}"#).unwrap();
    assert!(super::enabled_from(None, Some(&config)), "config opt-in");
    assert!(
        !super::enabled_from(Some("0"), Some(&config)),
        "the environment wins"
    );
    assert!(super::enabled_from(Some("1"), None));
    assert!(super::enabled_from(Some(" TRUE "), None));
    assert!(!super::enabled_from(Some("maybe"), None));
}

#[test]
fn an_unflagged_registry_does_not_offer_the_tool() {
    let mut registry = ToolRegistry::new();
    registry.register_session_tools();
    // The flag is off in the test environment unless someone set it.
    if !super::enabled() {
        assert!(registry.get_def(super::RUN_SCRIPT_TOOL).is_none());
    }
    assert!(registry
        .get_def(crate::batch_tools::RUN_ACTIONS_TOOL)
        .is_some());
}

#[tokio::test]
async fn query_returns_static_text_rows_with_their_value() {
    let harness = Harness::new();
    let result = harness
        .run("const app = await cua.getApp('Notes');\nconst rows = await app.query({role: 'statictext', name: 'Saved'});\nconst both = await app.query({name: 'Save'});\nreturn {rows, kinds: both.map(r => r.display_only ? 'text' : r.role)};")
        .await;
    assert_ne!(result.is_error, Some(true), "{}", text(&result));
    let value = &structured(&result)["value"];
    assert_eq!(value["rows"][0]["role"], "AXStaticText");
    assert_eq!(value["rows"][0]["label"], "Saved");
    assert_eq!(value["rows"][0]["window"]["window_id"], 7);
    assert_eq!(value["kinds"], json!(["AXButton", "text"]));
}
