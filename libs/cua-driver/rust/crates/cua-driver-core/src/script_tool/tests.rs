// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

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
type Pacings = Arc<Mutex<Vec<(String, crate::step_pacing::StepPacing)>>>;

/// Fake app: pid 42, window 7, a "Name" field and a "Save" button.
struct Fake {
    def: ToolDef,
    calls: Calls,
    pacings: Pacings,
}

#[async_trait]
impl Tool for Fake {
    fn def(&self) -> &ToolDef {
        &self.def
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let name = self.def.name.clone();
        self.pacings
            .lock()
            .unwrap()
            .push((name.clone(), crate::step_pacing::current()));
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
            _ if args["refuse"] == true => ToolResult::error(format!("{name} refused: stale token"))
                .with_structured(json!({"code": "stale_element_token"})),
            _ if args["fail"] == true => ToolResult::error(format!("{name} refused: asked to fail")),
            // The Linux driver's real-pointer click: dispatched, not confirmed.
            _ if args["unconfirmed"] == true => ToolResult::text(
                "Clicked element [2] (pid 42) with a real pointer click (delivery_mode=foreground, \
                 focus_after=target).",
            )
            .with_structured(json!({"verified": false, "effect": "unverifiable"})),
            _ if args["confirmed"] == true => {
                let structured = json!({
                    "path": "atspi", "verified": true, "effect": "confirmed",
                    "evidence": [{"kind": "window_change", "detail": "1 window(s) appeared"}],
                });
                let record = crate::action_record::ActionExecutionRecord::from_legacy(
                    &name, &args, &structured,
                )
                .expect("a click normalizes");
                ToolResult::text("Clicked element [2] (pid 42) via AT-SPI action 'click'; window opened.")
                    .with_structured(structured)
                    .with_action_record(record)
            }
            _ => ToolResult::text(format!("{name} done")),
        }
    }
}

struct Harness {
    registry: Arc<ToolRegistry>,
    calls: Calls,
    pacings: Pacings,
}

impl Harness {
    fn new() -> Self {
        Self::with(&[])
    }

    /// The fake registry plus fake tools named `extra`.
    fn with(extra: &[&str]) -> Self {
        let calls: Calls = Arc::default();
        let pacings: Pacings = Arc::default();
        let mut registry = ToolRegistry::new();
        for name in crate::batch_tools::BATCHABLE_TOOLS
            .iter()
            .copied()
            .chain([
                "launch_app",
                "list_apps",
                "list_windows",
                "get_window_state",
                "verify_state",
                "zoom",
                "check_permissions",
                "kill_app",
            ])
            .chain(extra.iter().copied())
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
                pacings: pacings.clone(),
            }));
        }
        registry.register_session_tools();
        registry.register_script_tool();
        let registry = Arc::new(registry);
        registry.init_self_weak();
        Self {
            registry,
            calls,
            pacings,
        }
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
        text(&result).contains("3. line 4 click UNCONFIRMED (") && text(&result).contains("on [2] AXButton \"Save\": click done; nothing read back. Look first: get_window_state"),
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
async fn a_refusal_in_a_script_carries_the_drivers_recovery_hint() {
    let harness = Harness::new();
    let result = harness
        .run("const app = await cua.getApp('Notes');\nawait app.click('s00000009:2', {refuse: true});\nreturn 1;")
        .await;
    assert_eq!(result.is_error, Some(true));
    let message = text(&result);
    assert!(
        message.contains(
            "click refused: stale token Hint: That element_token comes from an older read."
        ),
        "{message}"
    );
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
async fn nesting_tools_and_unknown_tools_are_unreachable() {
    let harness = Harness::new();
    for (script, needle) in [
        (
            "await cua.call('run_steps', {steps: []})",
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
fn the_tool_is_on_unless_the_operator_turns_it_off() {
    use super::disabled_from as off;
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.json");
    assert_eq!(off(None, None, None), None, "on by default");
    assert_eq!(off(None, None, Some(&config)), None, "no config file");
    std::fs::write(&config, "not json").unwrap();
    assert_eq!(off(None, None, Some(&config)), None, "unreadable config");
    std::fs::write(&config, r#"{"experimental_pip": true}"#).unwrap();
    assert_eq!(off(None, None, Some(&config)), None, "other keys");

    // The opt-outs.
    assert!(off(Some("1"), None, None)
        .unwrap()
        .contains("CUA_DRIVER_DISABLE_RUN_SCRIPT"));
    assert!(off(Some(" TRUE "), None, None).is_some());
    assert_eq!(off(Some("0"), None, None), None);
    assert_eq!(off(Some("maybe"), None, None), None);
    std::fs::write(&config, r#"{"disable_run_script": true}"#).unwrap();
    assert!(off(None, None, Some(&config))
        .unwrap()
        .contains("disable_run_script"));
    std::fs::write(&config, r#"{"disable_run_script": false}"#).unwrap();
    assert_eq!(off(None, None, Some(&config)), None);
    std::fs::write(&config, r#"{"experimental_script": false}"#).unwrap();
    assert!(off(None, None, Some(&config))
        .unwrap()
        .contains("experimental_script"));
}

#[test]
fn the_legacy_opt_in_flag_can_only_turn_the_tool_off() {
    use super::disabled_from as off;
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.json");
    // `=1` and `true` were the old opt-in: now they change nothing.
    assert_eq!(off(None, Some("1"), None), None);
    std::fs::write(&config, r#"{"experimental_script": true}"#).unwrap();
    assert_eq!(off(None, None, Some(&config)), None);
    // ...and they do not override an opt-out.
    assert!(off(Some("1"), Some("1"), Some(&config)).is_some());
    std::fs::write(
        &config,
        r#"{"experimental_script": true, "disable_run_script": true}"#,
    )
    .unwrap();
    assert!(off(None, Some("1"), Some(&config)).is_some());
    std::fs::write(&config, r#"{"experimental_script": false}"#).unwrap();
    assert!(off(None, Some("1"), Some(&config)).is_some());
    // `=0` turns it off, even over a config opt-in.
    std::fs::write(&config, r#"{"experimental_script": true}"#).unwrap();
    assert!(off(None, Some("0"), Some(&config))
        .unwrap()
        .contains("CUA_DRIVER_EXPERIMENTAL_SCRIPT=0"));
    assert!(off(None, Some("false"), None).is_some());
}

#[test]
fn a_default_registry_offers_the_tool() {
    let mut registry = ToolRegistry::new();
    registry.register_session_tools();
    // On unless whoever runs the tests turned it off.
    if super::enabled() {
        assert!(registry.get_def(super::RUN_SCRIPT_TOOL).is_some());
        let listed = registry.tools_list();
        assert!(listed["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"] == super::RUN_SCRIPT_TOOL));
    } else {
        assert!(registry.get_def(super::RUN_SCRIPT_TOOL).is_none());
    }
    assert!(registry
        .get_def(crate::batch_tools::RUN_STEPS_TOOL)
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

#[tokio::test]
async fn script_actions_are_paced_as_batched_steps_and_reads_are_not() {
    let harness = Harness::new();
    let result = harness
        .run(
            "const app = await cua.getApp({pid: 42, windowId: 7});\n\
             await app.getState();\n\
             await app.click({role: 'AXButton', name: 'Save'});\n\
             await app.pressKey('return');\n\
             return 'ok';",
        )
        .await;
    assert_ne!(result.is_error, Some(true), "{result:?}");
    let pacings = harness.pacings.lock().unwrap().clone();
    let of = |tool: &str| -> Vec<crate::step_pacing::StepPacing> {
        pacings
            .iter()
            .filter(|(name, _)| name == tool)
            .map(|(_, pacing)| *pacing)
            .collect()
    };
    use crate::step_pacing::StepPacing::{Batched, Single};
    assert_eq!(of("click"), vec![Batched]);
    assert_eq!(of("press_key"), vec![Batched]);
    assert!(
        of("get_window_state")
            .iter()
            .all(|pacing| *pacing == Single),
        "reads keep their own timing: {:?}",
        of("get_window_state")
    );
}

#[test]
fn script_only_mode_is_opt_in_and_yields_to_an_opt_out() {
    use super::script_only_from as only;
    assert!(!only(None, true), "off by default");
    assert!(!only(Some("0"), true));
    assert!(!only(Some("maybe"), true));
    assert!(only(Some("1"), true));
    assert!(only(Some(" On "), true));
    assert!(!only(Some("1"), false), "an opt-out of run_script wins");
}

#[test]
fn script_only_lists_run_script_alone_and_never_empties_the_list() {
    let harness = Harness::new();
    let names = |listed: Value| -> Vec<String> {
        listed["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap().to_owned())
            .collect()
    };
    // The harness registers run_script twice (session tools, then explicitly); compare names as a set.
    let only: std::collections::BTreeSet<String> = names(harness.registry.tools_list_for(true))
        .into_iter()
        .collect();
    assert_eq!(only, ["run_script".to_owned()].into());
    let all = names(harness.registry.tools_list_for(false));
    assert!(all.len() > 1 && all.iter().any(|name| name == "click"));
    // A registry without run_script keeps its tools in script-only mode.
    let mut bare = ToolRegistry::new();
    bare.register(Box::new(Fake {
        def: ToolDef {
            name: "click".into(),
            description: "fake".into(),
            input_schema: json!({"type": "object"}),
            read_only: false,
            destructive: false,
            idempotent: false,
            open_world: false,
        },
        calls: Calls::default(),
        pacings: Pacings::default(),
    }));
    assert_eq!(names(bare.tools_list_for(true)), ["click"]);
}

#[tokio::test]
async fn every_registered_tool_but_the_nesting_ones_is_callable_by_name() {
    let harness = Harness::new();
    // check_permissions was on no default-mode list; any registered tool is
    // reachable now, through cua.tools and cua.call.
    let result = harness
        .run(
            "await cua.tools.check_permissions({});\n\
             await cua.call('check_permissions', {});\n\
             return Object.keys(cua.tools);",
        )
        .await;
    assert_ne!(result.is_error, Some(true), "{}", text(&result));
    assert_eq!(harness.calls("check_permissions").len(), 2);
    let names: std::collections::BTreeSet<String> = structured(&result)["value"]
        .as_array()
        .unwrap()
        .iter()
        .map(|name| name.as_str().unwrap().to_owned())
        .collect();
    let mut want: std::collections::BTreeSet<String> =
        harness.registry.tool_names().map(str::to_owned).collect();
    want.remove("run_script");
    want.remove("run_steps");
    assert_eq!(names, want);
    for tool in ["kill_app", "move_cursor", "get_guide", "end_session"] {
        assert!(names.contains(tool), "{tool}");
    }

    // The generated functions take the tool's own arguments, and the action
    // tools resolve a {role, name} target the same way the helpers do.
    let direct = harness
        .run("await cua.tools.click({pid: 42, role: 'button', name: 'Save'});\nreturn 1;")
        .await;
    assert_ne!(direct.is_error, Some(true), "{}", text(&direct));
    let click = &harness.calls("click")[0];
    assert_eq!(click["element_token"], "s00000009:2");
    assert_eq!(click["window_id"], 7);
    assert!(click.get("role").is_none());

    // What a script may do is the driver's call, as for a direct call: kill_app
    // reaches the tool's own authorization, which refuses a process this
    // runtime did not launch.
    let killed = harness.run("await cua.tools.kill_app({pid: 42});").await;
    assert_eq!(killed.is_error, Some(true));
    assert_ne!(structured(&killed)["error"]["code"], "not_allowed");
    assert!(
        text(&killed).contains("launched by this Cua runtime"),
        "{}",
        text(&killed)
    );
    assert!(harness.calls("kill_app").is_empty());

    for tool in ["run_script", "run_steps"] {
        let result = harness
            .run(&format!(
                "if (cua.tools.{tool} !== undefined) return 'exposed';\nawait cua.call('{tool}', {{}});"
            ))
            .await;
        assert_eq!(result.is_error, Some(true), "{tool}: {}", text(&result));
        assert_eq!(
            structured(&result)["error"]["code"],
            "not_allowed",
            "{tool}"
        );
    }
}

#[tokio::test]
async fn describe_returns_a_tools_schema_and_lists_the_callable_tools() {
    let harness = Harness::new();
    let result = harness
        .run(
            "const all = await cua.describe();\n\
             const one = await cua.describe('get_guide');\n\
             let missing;\n\
             try { await cua.describe('run_steps'); } catch (e) { missing = e.code; }\n\
             return {all, one, missing};",
        )
        .await;
    assert_ne!(result.is_error, Some(true), "{}", text(&result));
    let value = &structured(&result)["value"];
    let def = harness.registry.get_def("get_guide").unwrap();
    assert_eq!(value["one"]["signature"], "get_guide(topic?)");
    assert_eq!(
        value["one"]["description"],
        def.to_list_entry()["description"]
    );
    assert_eq!(
        value["one"]["input_schema"],
        def.to_list_entry()["inputSchema"]
    );
    assert_eq!(value["missing"], "not_found");
    let all: Vec<&str> = value["all"]
        .as_array()
        .unwrap()
        .iter()
        .map(|line| line.as_str().unwrap())
        .collect();
    assert!(all.contains(&"get_guide(topic?)"), "{all:?}");
    assert!(!all.iter().any(|line| line.starts_with("run_s")), "{all:?}");
    // describe is free: it does not count against max_calls.
    assert_eq!(structured(&result)["driver_calls"], 0);
}

#[test]
fn a_signature_lists_required_arguments_then_optional_ones_and_never_session() {
    let def = ToolDef {
        name: "set_window_frame".into(),
        description: "fake".into(),
        input_schema: json!({
            "type": "object",
            "required": ["pid", "width"],
            "properties": {
                "session": {"type": "string"},
                "width": {"type": "number"},
                "window_id": {"type": "integer"},
                "pid": {"type": "integer"}
            },
            "additionalProperties": false
        }),
        read_only: false,
        destructive: false,
        idempotent: false,
        open_world: false,
    };
    assert_eq!(
        super::signature(&def),
        "set_window_frame(pid, width, window_id?)"
    );
    let bare = ToolDef {
        name: "get_screen_size".into(),
        input_schema: json!({"type": "object"}),
        ..def
    };
    assert_eq!(super::signature(&bare), "get_screen_size()");
}

#[test]
fn the_script_only_description_is_generated_from_the_registry() {
    // A tool on no list anywhere shows up.
    let harness = Harness::with(&["brand_new_tool"]);
    let description = super::script_only_description(&harness.registry);
    let listed = &harness.registry.tools_list_for(true)["tools"][0];
    assert_eq!(listed["name"], "run_script");
    assert_eq!(listed["description"], description);
    let signatures: Vec<&str> = description
        .split_once("Tools (`?` = optional):\n")
        .unwrap()
        .1
        .split("\n\n")
        .next()
        .unwrap()
        .lines()
        .collect();
    let mut want: Vec<String> = Vec::new();
    for name in harness.registry.tool_names() {
        if !matches!(name, "run_script" | "run_steps")
            && !want.iter().any(|w| w.starts_with(&format!("{name}(")))
        {
            want.push(super::signature(harness.registry.get_def(name).unwrap()));
        }
    }
    assert_eq!(signatures, want);
    assert!(signatures.contains(&"brand_new_tool()"));
    assert!(signatures.contains(&"move_cursor()"), "{signatures:?}");
    assert!(description.contains("cua.tools.<name>(args)"));
    assert!(description.contains("cua.describe(name)"));
    assert!(description.contains(crate::protocol::SCRIPT_PACING));

    // The default mode lists every tool beside run_script, so its
    // description names the rule, not the tools.
    let default = harness
        .registry
        .get_def("run_script")
        .unwrap()
        .description
        .clone();
    assert!(!default.contains("brand_new_tool"));
    assert!(default.contains("except run_script and run_steps"));
    assert!(
        !default.contains("launch_app, list_apps"),
        "no hand-written list"
    );
}

#[tokio::test]
async fn a_role_name_call_keeps_the_tools_verification_warning_in_the_call_log() {
    let harness = Harness::new();
    let result = harness
        .run(
            "await cua.tools.click({pid: 42, role: 'button', name: 'Save', unconfirmed: true});\n\
             await cua.tools.click({pid: 42, role: 'button', name: 'Save', confirmed: true});\n\
             return 1;",
        )
        .await;
    assert_ne!(result.is_error, Some(true), "{}", text(&result));
    let message = text(&result);
    assert!(
        message.contains("1. line 1 click UNCONFIRMED (")
            && message.contains(
                "on [2] AXButton \"Save\": Clicked element [2] (pid 42) with a real pointer click \
                 (delivery_mode=foreground, focus_after=target); nothing read back. Look first: \
                 get_window_state {\"pid\":42,\"since\":\"latest\",\"window_id\":7}"
            ),
        "{message}"
    );
    assert!(
        message.contains("1 of 2 calls unconfirmed: look before relying on them"),
        "{message}"
    );
    // A confirmed call stays plain `ok` and keeps its own text too.
    assert!(message.contains("2. line 2 click ok ("), "{message}");
    assert!(!message.contains("CONFIRMED: Clicked"), "{message}");
    assert!(
        message.contains("via AT-SPI action 'click'; window opened."),
        "{message}"
    );
    let calls = structured(&result)["calls"].clone();
    let clicks: Vec<&Value> = calls
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["op"] == "click")
        .collect();
    assert_eq!(clicks[0]["verified"], false);
    assert!(clicks[1].get("verified").is_none(), "{calls}");
}

#[tokio::test]
async fn a_script_whose_calls_all_confirm_has_no_unverified_line() {
    let harness = Harness::new();
    let result = harness
        .run("await cua.tools.click({pid: 42, role: 'button', name: 'Save', confirmed: true});\nreturn 1;")
        .await;
    assert!(!text(&result).contains("unverified"), "{}", text(&result));
}
