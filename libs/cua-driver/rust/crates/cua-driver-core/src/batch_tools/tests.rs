use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::{
    authorization::PermissionMode,
    protocol::ToolResult,
    session_authorization::{
        EffectiveAuthorizationContext, SessionAuthorizationRegistry, SessionModeCeiling,
    },
    tool::{Tool, ToolDef, ToolRegistry},
};

/// A stand-in platform tool that records every call it receives and fails
/// when asked to (`{"fail": true}`).
struct Probe {
    def: ToolDef,
    calls: Arc<Mutex<Vec<Value>>>,
}

#[async_trait]
impl Tool for Probe {
    fn def(&self) -> &ToolDef {
        &self.def
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        self.calls.lock().unwrap().push(args.clone());
        if args["fail"] == true {
            ToolResult::error(format!("{} refused: probe asked to fail", self.def.name))
        } else {
            ToolResult::text(format!("{} done", self.def.name))
        }
    }
}

struct Harness {
    registry: Arc<ToolRegistry>,
    calls: std::collections::HashMap<&'static str, Arc<Mutex<Vec<Value>>>>,
}

impl Harness {
    fn new() -> Self {
        let mut registry = ToolRegistry::new();
        let mut calls = std::collections::HashMap::new();
        for name in super::BATCHABLE_TOOLS
            .iter()
            .copied()
            .chain(["get_window_state"])
        {
            let log = Arc::new(Mutex::new(Vec::new()));
            calls.insert(name, log.clone());
            registry.register(Box::new(Probe {
                def: ToolDef {
                    name: name.into(),
                    description: "probe".into(),
                    input_schema: json!({
                        "type": "object",
                        "properties": {
                            "pid": {"type": "integer"},
                            "window_id": {"type": "integer"},
                            "text": {"type": "string"},
                            "fail": {"type": "boolean"},
                            "include_screenshot": {"type": "boolean"},
                            "since": {"type": "string"},
                            "direction": {"type": "string", "enum": ["up", "down", "left", "right"]},
                            "amount": {"type": "integer", "minimum": 1, "maximum": 50},
                            "x": {"type": "number"},
                            "y": {"type": "number"},
                            "full_output": {"type": "boolean"},
                            "max_elements": {"type": "integer", "minimum": 1}
                        },
                        "additionalProperties": false
                    }),
                    read_only: name == "get_window_state",
                    destructive: false,
                    idempotent: false,
                    open_world: false,
                },
                calls: log,
            }));
        }
        registry.register_session_tools();
        let registry = Arc::new(registry);
        registry.init_self_weak();
        Self { registry, calls }
    }

    fn hits(&self, tool: &str) -> usize {
        self.calls[tool].lock().unwrap().len()
    }

    fn last(&self, tool: &str) -> Value {
        self.calls[tool]
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("tool was called")
    }

    async fn run(&self, args: Value) -> ToolResult {
        self.registry
            .invoke_with_context(super::RUN_ACTIONS_TOOL, args, context(None))
            .await
    }
}

fn context(manifest: Option<&str>) -> Arc<EffectiveAuthorizationContext> {
    let mode = if manifest.is_some() {
        PermissionMode::Bounded
    } else {
        PermissionMode::Unrestricted
    };
    let ceiling = SessionModeCeiling::for_trusted_sessions(
        [mode],
        mode == PermissionMode::Unrestricted,
        Duration::from_secs(60),
        Duration::from_secs(30),
    )
    .unwrap();
    let manifest = manifest.map(|source| {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(source.as_bytes()).unwrap();
        Arc::new(crate::session_manifest::load_manifest(file.path()).expect("manifest loads"))
    });
    SessionAuthorizationRegistry::with_ceiling(ceiling)
        .compatibility_context(mode, manifest)
        .unwrap()
}

fn text(result: &ToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|content| match content {
            crate::protocol::Content::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn runs_steps_in_order_and_reports_each() {
    let harness = Harness::new();
    let result = harness
        .run(json!({"steps": [
            {"tool": "set_value", "args": {"pid": 42, "window_id": 7, "text": "a"}},
            {"tool": "type_text", "args": {"pid": 42, "text": "b"}},
            {"tool": "press_key", "args": {"pid": 42}},
        ]}))
        .await;
    assert_ne!(result.is_error, Some(true), "{}", text(&result));
    let structured = result.structured_content.unwrap();
    assert_eq!(structured["ok"], true);
    assert_eq!(structured["executed"], 3);
    let tools: Vec<_> = structured["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|step| step["tool"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(tools, ["set_value", "type_text", "press_key"]);
    assert_eq!(structured.get("observation"), None);
    assert_eq!(harness.hits("get_window_state"), 0, "no observe, no read");
}

#[tokio::test]
async fn stops_at_the_first_failure_and_still_observes() {
    let harness = Harness::new();
    let result = harness
        .run(json!({
            "steps": [
                {"tool": "click", "args": {"pid": 42, "window_id": 7}},
                {"tool": "set_value", "args": {"pid": 42, "window_id": 7, "fail": true}},
                {"tool": "press_key", "args": {"pid": 42}},
            ],
            "observe": {}
        }))
        .await;
    assert_eq!(result.is_error, Some(true));
    let structured = result.structured_content.clone().unwrap();
    assert_eq!(structured["failed_step"], 1);
    assert_eq!(structured["executed"], 2);
    assert_eq!(structured["steps"][1]["ok"], false);
    assert!(structured["steps"][1]["message"]
        .as_str()
        .unwrap()
        .contains("probe asked to fail"));
    assert_eq!(
        harness.hits("press_key"),
        0,
        "steps after the failure must not run"
    );
    assert!(text(&result).contains("step 2 of 3 failed"));
    assert_eq!(harness.hits("get_window_state"), 1);
    assert_eq!(structured["observation"]["ok"], true);
}

#[tokio::test]
async fn validates_every_step_before_running_any() {
    let harness = Harness::new();
    let cases = [
        (
            json!({"tool": "zoom", "args": {}}),
            "reads state and a batch only acts",
        ),
        (
            json!({"tool": "run_actions", "args": {}}),
            "cannot run in a batch",
        ),
        (
            json!({"tool": "click", "args": {"pid": "forty-two"}}),
            "invalid arguments",
        ),
        (
            json!({"tool": "click", "args": {"bogus": 1}}),
            "invalid arguments",
        ),
        (
            json!({"tool": "click", "args": {"_session_id": "x"}}),
            "reserved",
        ),
        (
            json!({"tool": "click", "args": {"session": "other"}}),
            "differs from the batch",
        ),
        (json!({"tool": "click", "extra": 1}), "unknown field"),
        (json!({"tool": "click", "args": 5}), "must be an object"),
    ];
    for (bad, expected) in cases {
        let result = harness
            .run(json!({"steps": [
                {"tool": "click", "args": {"pid": 42}},
                {"tool": "press_key", "args": {"pid": 42}},
                bad.clone(),
            ]}))
            .await;
        assert_eq!(result.is_error, Some(true), "{bad}");
        let message = text(&result);
        assert!(message.contains("step 3"), "{bad}: {message}");
        assert!(message.contains(expected), "{bad}: {message}");
        assert_eq!(result.structured_content.unwrap()["code"], "invalid_batch");
    }
    assert_eq!(
        harness.hits("click"),
        0,
        "a rejected batch must execute nothing"
    );
    assert_eq!(harness.hits("press_key"), 0);
}

#[tokio::test]
async fn rejects_bad_batch_shapes() {
    let harness = Harness::new();
    for args in [
        json!({"steps": []}),
        json!({"steps": "click"}),
        json!({"steps": [{"tool": "click"}], "delay_ms": 5000}),
        json!({"steps": vec![json!({"tool": "click"}); super::MAX_STEPS + 1]}),
        json!({"steps": [{"tool": "click"}], "observe": {"pid": 1}}),
        json!({"steps": [{"tool": "click"}], "observe": {}}),
        json!({"steps": [{"tool": "click", "args": {"pid": 1, "window_id": 2}}], "observe": {"max_elements": 0}}),
    ] {
        let result = harness.run(args.clone()).await;
        assert_eq!(result.is_error, Some(true), "{args}");
    }
    assert_eq!(harness.hits("click"), 0);
}

#[tokio::test]
async fn observation_inherits_the_window_and_stays_bounded() {
    let harness = Harness::new();
    let result = harness
        .run(json!({
            "steps": [
                {"tool": "click", "args": {"pid": 42, "window_id": 7}},
                {"tool": "type_text", "args": {"pid": 42, "text": "x"}},
            ],
            "observe": {}
        }))
        .await;
    assert_ne!(result.is_error, Some(true), "{}", text(&result));
    let observed = harness.last("get_window_state");
    assert_eq!(observed["pid"], 42);
    assert_eq!(observed["window_id"], 7);
    assert_eq!(observed["include_screenshot"], false);
    assert_eq!(
        observed["max_elements"], 250,
        "the plain-read budget, so `latest` finds it"
    );
    assert_eq!(
        observed["since"], "latest",
        "the observation is a diff by default"
    );

    harness
        .run(json!({
            "steps": [{"tool": "click", "args": {"pid": 42, "window_id": 7}}],
            "observe": {"pid": 1, "window_id": 2, "include_screenshot": true, "max_elements": 10}
        }))
        .await;
    let observed = harness.last("get_window_state");
    assert_eq!(observed["pid"], 1);
    assert_eq!(observed["include_screenshot"], true);
    assert_eq!(observed["max_elements"], 10);
    assert_eq!(
        harness.hits("get_window_state"),
        2,
        "exactly one read per batch"
    );
}

#[tokio::test]
async fn pauses_between_steps_but_not_after_the_last() {
    let harness = Harness::new();
    let started = std::time::Instant::now();
    harness
        .run(json!({"delay_ms": 120, "steps": [
            {"tool": "click", "args": {"pid": 42}},
            {"tool": "click", "args": {"pid": 42}},
        ]}))
        .await;
    let elapsed = started.elapsed();
    assert!(elapsed >= Duration::from_millis(120), "{elapsed:?}");
    assert!(elapsed < Duration::from_millis(240), "{elapsed:?}");
}

#[tokio::test]
async fn every_step_runs_in_the_batch_session() {
    let harness = Harness::new();
    let result = harness
        .run(json!({"session": "batch-s1", "steps": [
            {"tool": "click", "args": {"pid": 42}},
            {"tool": "press_key", "args": {"pid": 42, "session": "batch-s1"}},
        ]}))
        .await;
    assert_ne!(result.is_error, Some(true), "{}", text(&result));
    for tool in ["click", "press_key"] {
        let session = harness.last(tool)["session"].as_str().unwrap().to_owned();
        assert!(session.ends_with("batch-s1"), "{tool}: {session}");
    }
}

#[tokio::test]
async fn each_step_is_held_to_the_capability_manifest() {
    let harness = Harness::new();
    let manifest = "version: 2\nmode: bounded\nexpires_after: 1h\nidle_timeout: 30m\nallow:\n  tools: [run_actions, click]\n";
    let result = harness
        .registry
        .invoke_with_context(
            super::RUN_ACTIONS_TOOL,
            json!({"steps": [
                {"tool": "click", "args": {"pid": 42}},
                {"tool": "set_value", "args": {"pid": 42, "text": "secret"}},
                {"tool": "click", "args": {"pid": 42}},
            ]}),
            context(Some(manifest)),
        )
        .await;
    assert_eq!(result.is_error, Some(true));
    let message = text(&result);
    let structured = result.structured_content.unwrap();
    // The manifest scopes pid-targeted input to declared applications, and
    // none is declared: the very first step is refused by the same gate a
    // direct call meets, so nothing reaches any tool.
    assert_eq!(structured["failed_step"], 0, "{message}");
    assert!(message.contains("capability manifest"), "{message}");
    assert_eq!(harness.hits("click"), 0);
    assert_eq!(harness.hits("set_value"), 0, "later steps never run");
}

#[tokio::test]
async fn the_batch_itself_needs_the_manifest_to_allow_it() {
    let harness = Harness::new();
    let manifest = "version: 2\nmode: bounded\nexpires_after: 1h\nidle_timeout: 30m\nallow:\n  tools: [click]\n";
    let result = harness
        .registry
        .invoke_with_context(
            super::RUN_ACTIONS_TOOL,
            json!({"steps": [{"tool": "click", "args": {"pid": 42}}]}),
            context(Some(manifest)),
        )
        .await;
    assert_eq!(result.is_error, Some(true));
    assert_eq!(harness.hits("click"), 0);
}

#[tokio::test]
async fn the_batch_is_advertised_with_its_schema() {
    let harness = Harness::new();
    let def = harness
        .registry
        .get_def(super::RUN_ACTIONS_TOOL)
        .expect("registered");
    assert_eq!(def.input_schema["required"], json!(["steps"]));
    let listed: Vec<_> = def.input_schema["properties"]["steps"]["items"]["properties"]
        ["tool"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect();
    assert_eq!(listed, super::BATCHABLE_TOOLS);
}

#[test]
fn run_actions_advertises_provider_compatible_schema_without_unions() {
    let harness = Harness::new();
    let schema = &harness.registry.get_def(super::RUN_ACTIONS_TOOL).unwrap().input_schema;
    let item = &schema["properties"]["steps"]["items"];
    assert_eq!(item["type"], "object");
    assert!(item.get("anyOf").is_none());
    assert!(item.get("oneOf").is_none());
    assert_eq!(item["additionalProperties"], true);
    assert_eq!(item["properties"]["expect"]["type"], "array");
    assert_eq!(item["properties"]["expect"]["items"]["type"], "object");
    assert_eq!(item["properties"]["expect"]["maxItems"], super::MAX_EXPECTS);
    assert_eq!(schema["properties"]["observe"]["type"], "object");
    // The adjacent legacy runtime test still exercises observe:true.
}

#[tokio::test]
async fn observe_true_reads_a_diff_and_since_null_reads_in_full() {
    let harness = Harness::new();
    let step = json!([{"tool": "click", "args": {"pid": 42, "window_id": 7}}]);

    let result = harness.run(json!({"steps": step, "observe": true})).await;
    assert_ne!(result.is_error, Some(true), "{}", text(&result));
    let observed = harness.last("get_window_state");
    assert_eq!(observed["window_id"], 7);
    assert_eq!(observed["since"], "latest");

    harness
        .run(json!({"steps": step, "observe": {"since": null}}))
        .await;
    assert!(harness.last("get_window_state").get("since").is_none());

    harness
        .run(json!({"steps": step, "observe": {"full_output": true}}))
        .await;
    assert!(harness.last("get_window_state").get("since").is_none());

    harness
        .run(json!({"steps": step, "observe": {"since": "s0000002a"}}))
        .await;
    assert_eq!(harness.last("get_window_state")["since"], "s0000002a");

    harness.run(json!({"steps": step, "observe": false})).await;
    assert_eq!(
        harness.hits("get_window_state"),
        4,
        "observe:false reads nothing"
    );
}

#[tokio::test]
async fn common_tool_name_slips_map_to_the_batchable_tool() {
    let harness = Harness::new();
    let result = harness
        .run(json!({"steps": [
            {"tool": "press", "args": {"pid": 1}},
            {"tool": "type", "args": {"pid": 1, "text": "x"}},
        ]}))
        .await;
    assert_ne!(result.is_error, Some(true), "{}", text(&result));
    assert_eq!(harness.hits("press_key"), 1);
    assert_eq!(harness.hits("type_text"), 1);

    let refused = harness
        .run(json!({"steps": [{"tool": "get_window_state", "args": {"pid": 1}}]}))
        .await;
    assert_eq!(refused.is_error, Some(true));
}

#[tokio::test]
async fn scroll_steps_accept_dx_dy_like_a_direct_call() {
    let harness = Harness::new();
    // The live-check shape: {"dy": 500} in a batch step.
    let result = harness
        .run(json!({"steps": [
            {"tool": "scroll", "args": {"pid": 1, "window_id": 2, "x": 600, "y": 500, "dy": 500}},
            {"tool": "scroll", "args": {"pid": 1, "window_id": 2, "x": 600, "y": 500, "dx": "-4"}},
        ]}))
        .await;
    assert_ne!(result.is_error, Some(true), "{}", text(&result));
    let calls = harness.calls["scroll"].lock().unwrap().clone();
    assert_eq!(calls.len(), 2);
    assert_eq!(
        (calls[0]["direction"].clone(), calls[0]["amount"].clone()),
        (json!("down"), json!(5))
    );
    assert_eq!(
        (calls[1]["direction"].clone(), calls[1]["amount"].clone()),
        (json!("left"), json!(4))
    );
    assert!(calls
        .iter()
        .all(|c| c.get("dy").is_none() && c.get("dx").is_none()));

    let refused = harness
        .run(json!({"steps": [{"tool": "scroll", "args": {"pid": 1, "dx": 1, "dy": 1}}]}))
        .await;
    assert_eq!(refused.is_error, Some(true));
    assert!(
        text(&refused).contains("one axis per call"),
        "{}",
        text(&refused)
    );
    assert_eq!(harness.hits("scroll"), 2, "a refused batch runs nothing");
}

#[tokio::test]
async fn a_trailing_read_becomes_the_observation_and_a_middle_one_points_to_observe() {
    let harness = Harness::new();
    // The v035 / live-check shape: act, act, then get_window_state as a step.
    let result = harness
        .run(json!({"steps": [
            {"tool": "click", "args": {"pid": 7, "window_id": 3}},
            {"tool": "click", "args": {"pid": 7, "window_id": 3, "text": "x"}},
            {"tool": "get_window_state", "args": {"pid": 7, "window_id": 3, "max_elements": 70}},
        ]}))
        .await;
    assert_ne!(result.is_error, Some(true), "{}", text(&result));
    let structured = result.structured_content.unwrap();
    assert_eq!(structured["total"], 2, "the read is not counted as a step");
    assert_eq!(structured["observation"]["ok"], true);
    let observed = harness.last("get_window_state");
    assert_eq!(observed["max_elements"], 70);
    assert_eq!(observed["since"], "latest");
    assert_eq!(harness.hits("click"), 2);

    let refused = harness
        .run(json!({"steps": [
            {"tool": "click", "args": {"pid": 7, "window_id": 3}},
            {"tool": "get_window_state", "args": {"pid": 7, "window_id": 3}},
            {"tool": "click", "args": {"pid": 7, "window_id": 3}},
        ]}))
        .await;
    assert_eq!(refused.is_error, Some(true));
    let message = text(&refused);
    assert!(message.contains("step 2 of the batch"), "{message}");
    assert!(
        message.contains("Put get_window_state arguments in `observe`"),
        "{message}"
    );

    // With an explicit observe, a trailing read is not silently dropped.
    let both = harness
        .run(json!({"steps": [
            {"tool": "click", "args": {"pid": 7, "window_id": 3}},
            {"tool": "get_window_state", "args": {"pid": 7, "window_id": 3}},
        ], "observe": true}))
        .await;
    assert_eq!(both.is_error, Some(true));
    assert_eq!(harness.hits("get_window_state"), 1);
}
