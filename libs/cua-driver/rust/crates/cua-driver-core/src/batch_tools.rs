//! `run_actions`: run an ordered list of existing action tools in one call.
//!
//! The tool owns no input logic. Every step is dispatched through the same
//! [`ToolRegistry`] entry point a direct call uses, inside the batch's own
//! authorization scope. Policy, permission mode, capability manifest,
//! session capture scope, protected-resource consent, desktop coordination
//! and recording therefore run once per step exactly as for a single call,
//! and a batch cannot reach any tool or argument that a single call could
//! not. The batch itself is only a thin ordered loop plus up-front validation.

use std::sync::OnceLock;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Map, Value};

use crate::{
    protocol::{Content, ToolResult},
    recording_tools::ReplayRegistrySlot,
    tool::{Tool, ToolDef, ToolRegistry},
};

/// Public name of the batch tool.
pub const RUN_ACTIONS_TOOL: &str = "run_actions";

/// Action tools a batch may run. Observation tools are deliberately absent:
/// the batch has exactly one optional observation, at the end.
pub const BATCHABLE_TOOLS: &[&str] = &[
    "click",
    "double_click",
    "right_click",
    "set_value",
    "type_text",
    "press_key",
    "hotkey",
    "scroll",
    "drag",
];

/// Most steps one call may carry. Keeps a batch inside client call timeouts.
pub const MAX_STEPS: usize = 32;
/// Longest pause between two steps.
pub const MAX_DELAY_MS: u64 = 2_000;
/// Default `max_elements` for the end-of-batch observation.
const DEFAULT_OBSERVE_MAX_ELEMENTS: u64 = 200;
const MESSAGE_LIMIT: usize = 300;
const OBSERVE_TOOL: &str = "get_window_state";

pub struct RunActionsTool {
    registry: ReplayRegistrySlot,
}

impl RunActionsTool {
    pub fn new(registry: ReplayRegistrySlot) -> Self {
        Self { registry }
    }
}

static DEF: OnceLock<ToolDef> = OnceLock::new();

#[async_trait]
impl Tool for RunActionsTool {
    fn def(&self) -> &ToolDef {
        DEF.get_or_init(|| ToolDef {
            name: RUN_ACTIONS_TOOL.into(),
            description: "Run several action tools in ONE call, in order, and stop at the first \
                failure. Use it when you already know the next few actions and do not need to \
                look at the app between them (fill three fields, click, then read the result). \
                Each step is `{tool, args}` where `tool` is one of click, double_click, \
                right_click, set_value, type_text, press_key, hotkey, scroll, drag and `args` \
                are exactly that tool's arguments. Every step goes through the same session, \
                permission and approval checks as a direct call; a batch grants nothing extra. \
                All steps are validated before the first one runs, so a malformed step 4 \
                changes nothing. Element tokens and element_index values come from a \
                get_window_state read before the batch; a step that changes the UI can \
                invalidate later element targets, so put element-targeted actions before the \
                actions that reshuffle the window, or use pixel targets after them.\n\n\
                Returns per-step status (`ok` or the error message) and, when `observe` is \
                given, ONE bounded get_window_state read after the last executed step. \
                Without `observe` nothing is read. The batch uses one session: steps may \
                omit `session` or repeat the batch's. `delay_ms` pauses between steps (max \
                2000). At most 32 steps."
                .into(),
            input_schema: json!({
                "type": "object",
                "required": ["steps"],
                "properties": {
                    "steps": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": MAX_STEPS,
                        "description": "Ordered actions. Execution stops at the first failing step.",
                        "items": {
                            "type": "object",
                            "required": ["tool"],
                            "properties": {
                                "tool": { "type": "string", "enum": BATCHABLE_TOOLS, "description": "Action tool to run." },
                                "args": { "type": "object", "description": "Arguments for that tool, as in a direct call." }
                            },
                            "additionalProperties": false
                        }
                    },
                    "delay_ms": {
                        "type": "integer",
                        "minimum": 0,
                        "maximum": MAX_DELAY_MS,
                        "description": "Pause between steps in milliseconds (not after the last). Default 0."
                    },
                    "observe": {
                        "type": "object",
                        "description": "Optional end-of-batch observation: arguments for ONE get_window_state call. `pid` and `window_id` default to those of the last step that names both. Defaults to include_screenshot=false and max_elements=200 to stay cheap; pass include_screenshot=true or a larger max_elements to widen it. Omit to read nothing."
                    }
                },
                "additionalProperties": false
            }),
            read_only: false,
            destructive: true,
            idempotent: false,
            open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let registry = match self.registry.lock().unwrap().upgrade() {
            Some(registry) => registry,
            None => {
                return ToolResult::error(
                    "run_actions is not available: registry not initialised yet.",
                )
            }
        };
        let plan = match Plan::parse(&registry, &args) {
            Ok(plan) => plan,
            Err(error) => return error.into_result(),
        };
        plan.run(&registry).await
    }
}

struct Step {
    tool: &'static str,
    args: Value,
}

struct Plan {
    steps: Vec<Step>,
    delay: Duration,
    observe: Option<Value>,
}

struct PlanError {
    step: Option<usize>,
    message: String,
}

impl PlanError {
    fn batch(message: impl Into<String>) -> Self {
        Self {
            step: None,
            message: message.into(),
        }
    }

    fn step(index: usize, message: impl Into<String>) -> Self {
        Self {
            step: Some(index),
            message: message.into(),
        }
    }

    fn into_result(self) -> ToolResult {
        let location = match self.step {
            Some(index) => format!("step {} of the batch: ", index + 1),
            None => String::new(),
        };
        ToolResult::error(format!(
            "run_actions rejected before running anything: {location}{}",
            self.message
        ))
        .with_structured(json!({
            "ok": false,
            "code": "invalid_batch",
            "executed": 0,
            "failed_step": self.step,
            "detail": self.message,
        }))
    }
}

impl Plan {
    fn parse(registry: &ToolRegistry, args: &Value) -> Result<Self, PlanError> {
        let object = args
            .as_object()
            .ok_or_else(|| PlanError::batch("arguments must be a JSON object"))?;
        let raw_steps = object
            .get("steps")
            .and_then(Value::as_array)
            .ok_or_else(|| PlanError::batch("`steps` must be a non-empty array"))?;
        if raw_steps.is_empty() || raw_steps.len() > MAX_STEPS {
            return Err(PlanError::batch(format!(
                "`steps` must hold 1 to {MAX_STEPS} items, got {}",
                raw_steps.len()
            )));
        }
        let delay_ms = match object.get("delay_ms") {
            None | Some(Value::Null) => 0,
            Some(value) => value
                .as_u64()
                .filter(|ms| *ms <= MAX_DELAY_MS)
                .ok_or_else(|| {
                    PlanError::batch(format!("`delay_ms` must be an integer 0 to {MAX_DELAY_MS}"))
                })?,
        };
        // The trusted label the dispatcher recorded for this call's public
        // session. Every step runs in that one session.
        let session = object
            .get("_public_session_label")
            .and_then(Value::as_str)
            .map(str::to_owned);

        let mut steps = Vec::with_capacity(raw_steps.len());
        for (index, raw) in raw_steps.iter().enumerate() {
            steps.push(parse_step(registry, index, raw, session.as_deref())?);
        }

        let observe = match object.get("observe") {
            None | Some(Value::Null) => None,
            Some(value) => Some(parse_observe(registry, value, &steps, session.as_deref())?),
        };
        Ok(Self {
            steps,
            delay: Duration::from_millis(delay_ms),
            observe,
        })
    }

    async fn run(self, registry: &ToolRegistry) -> ToolResult {
        let total = self.steps.len();
        let mut reports = Vec::with_capacity(total);
        let mut lines = Vec::with_capacity(total + 2);
        let mut failed_step = None;

        for (index, step) in self.steps.into_iter().enumerate() {
            if index > 0 && !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            let result = registry.invoke(step.tool, step.args).await;
            let ok = result.is_error != Some(true);
            let message = bounded_message(&result);
            lines.push(format!(
                "{}. {} {}{}",
                index + 1,
                step.tool,
                if ok { "ok" } else { "ERROR" },
                if message.is_empty() {
                    String::new()
                } else {
                    format!(": {message}")
                }
            ));
            let mut report = json!({
                "index": index,
                "tool": step.tool,
                "ok": ok,
                "message": message,
            });
            if !ok {
                if let Some(code) = result
                    .structured_content
                    .as_ref()
                    .and_then(|value| value.get("code"))
                    .and_then(Value::as_str)
                {
                    report["code"] = Value::String(code.to_owned());
                }
            }
            reports.push(report);
            if !ok {
                failed_step = Some(index);
                break;
            }
        }

        let executed = reports.len();
        let mut structured = json!({
            "ok": failed_step.is_none(),
            "total": total,
            "executed": executed,
            "failed_step": failed_step,
            "steps": reports,
        });
        let header = match failed_step {
            None => format!("run_actions: {total}/{total} steps ok"),
            Some(index) => format!(
                "run_actions: step {} of {total} failed; {} step(s) ran before it, {} not run",
                index + 1,
                index,
                total - executed
            ),
        };
        lines.insert(0, header);

        // One observation at the end, also after a failure so the caller can
        // see where the app was left.
        let mut observation_content = Vec::new();
        if let Some(args) = self.observe {
            let result = registry.invoke(OBSERVE_TOOL, args).await;
            let ok = result.is_error != Some(true);
            let mut report = json!({ "ok": ok, "tool": OBSERVE_TOOL });
            if ok {
                if let Some(state) = result.structured_content.clone() {
                    report["state"] = state;
                }
                observation_content = result.content;
                lines.push("observation: get_window_state ok (below)".to_owned());
            } else {
                let message = bounded_message(&result);
                lines.push(format!("observation: get_window_state ERROR: {message}"));
                report["message"] = Value::String(message);
            }
            structured["observation"] = report;
        }

        let mut content = vec![Content::text(lines.join("\n"))];
        content.append(&mut observation_content);
        ToolResult {
            content,
            is_error: failed_step.map(|_| true),
            structured_content: Some(structured),
            ..Default::default()
        }
    }
}

fn parse_step(
    registry: &ToolRegistry,
    index: usize,
    raw: &Value,
    session: Option<&str>,
) -> Result<Step, PlanError> {
    let object = raw
        .as_object()
        .ok_or_else(|| PlanError::step(index, "must be an object {tool, args}"))?;
    if let Some(extra) = object
        .keys()
        .find(|key| !matches!(key.as_str(), "tool" | "args"))
    {
        return Err(PlanError::step(
            index,
            format!("unknown field `{extra}`; a step has only `tool` and `args`"),
        ));
    }
    let name = object
        .get("tool")
        .and_then(Value::as_str)
        .ok_or_else(|| PlanError::step(index, "`tool` must be a string"))?;
    let tool = BATCHABLE_TOOLS
        .iter()
        .copied()
        .find(|candidate| *candidate == name)
        .ok_or_else(|| {
            PlanError::step(
                index,
                format!(
                    "`{name}` cannot run in a batch; allowed tools: {}",
                    BATCHABLE_TOOLS.join(", ")
                ),
            )
        })?;
    let mut args = match object.get("args") {
        None | Some(Value::Null) => Value::Object(Map::new()),
        Some(value @ Value::Object(_)) => value.clone(),
        Some(_) => return Err(PlanError::step(index, "`args` must be an object")),
    };
    prepare_args(&mut args, session).map_err(|message| PlanError::step(index, message))?;
    validate_against_schema(registry, tool, &args)
        .map_err(|message| PlanError::step(index, format!("{tool}: {message}")))?;
    Ok(Step { tool, args })
}

fn parse_observe(
    registry: &ToolRegistry,
    value: &Value,
    steps: &[Step],
    session: Option<&str>,
) -> Result<Value, PlanError> {
    let mut args = value
        .as_object()
        .cloned()
        .map(Value::Object)
        .ok_or_else(|| {
            PlanError::batch("`observe` must be an object of get_window_state arguments")
        })?;
    prepare_args(&mut args, session)
        .map_err(|message| PlanError::batch(format!("observe: {message}")))?;
    let object = args.as_object_mut().expect("object");
    // Inherit the window from the last step that names one, so the common
    // "act, then look at the same window" case needs no repetition.
    let inherited = steps.iter().rev().find_map(|step| {
        let pid = step.args.get("pid").and_then(Value::as_i64)?;
        let window_id = step.args.get("window_id").and_then(Value::as_i64)?;
        Some((pid, window_id))
    });
    if let Some((pid, window_id)) = inherited {
        object.entry("pid").or_insert_with(|| json!(pid));
        object
            .entry("window_id")
            .or_insert_with(|| json!(window_id));
    }
    if !object.contains_key("pid") || !object.contains_key("window_id") {
        return Err(PlanError::batch(
            "observe: `pid` and `window_id` are required (no step names both to inherit from)",
        ));
    }
    object
        .entry("include_screenshot")
        .or_insert(Value::Bool(false));
    object
        .entry("max_elements")
        .or_insert_with(|| json!(DEFAULT_OBSERVE_MAX_ELEMENTS));
    validate_against_schema(registry, OBSERVE_TOOL, &args)
        .map_err(|message| PlanError::batch(format!("observe: {message}")))?;
    Ok(args)
}

/// Reject runtime-private fields and pin the step to the batch's session.
fn prepare_args(args: &mut Value, session: Option<&str>) -> Result<(), String> {
    let object = args.as_object_mut().expect("object");
    if let Some(key) = object.keys().find(|key| key.starts_with('_')) {
        return Err(format!("`{key}` is reserved and cannot be set"));
    }
    let own = object
        .get("session")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    match (own, session) {
        (Some(own), Some(batch)) if own != batch => Err(format!(
            "session `{own}` differs from the batch's session `{batch}`; a batch runs in one session"
        )),
        (Some(own), None) if own != "default" => Err(format!(
            "session `{own}` differs from the batch's session; set `session` on run_actions itself"
        )),
        (_, Some(batch)) => {
            object.insert("session".to_owned(), Value::String(batch.to_owned()));
            Ok(())
        }
        _ => Ok(()),
    }
}

fn validate_against_schema(
    registry: &ToolRegistry,
    tool: &str,
    args: &Value,
) -> Result<(), String> {
    let def = registry
        .get_def(tool)
        .ok_or_else(|| format!("tool `{tool}` is not available on this platform"))?;
    let schema = crate::tool::advertised_runtime_input_schema(&def.name, &def.input_schema);
    let validator = jsonschema::validator_for(&schema)
        .map_err(|error| format!("input schema is unusable: {error}"))?;
    validator.validate(args).map_err(|error| {
        let path = error.instance_path().to_string();
        if path.is_empty() {
            format!("invalid arguments: {error}")
        } else {
            format!("invalid arguments at {path}: {error}")
        }
    })
}

fn bounded_message(result: &ToolResult) -> String {
    let text = result
        .content
        .iter()
        .find_map(|content| match content {
            Content::Text { text, .. } => Some(text.trim()),
            _ => None,
        })
        .unwrap_or("");
    let first_line = text.lines().next().unwrap_or("");
    let mut message: String = first_line.chars().take(MESSAGE_LIMIT).collect();
    if first_line.chars().count() > MESSAGE_LIMIT {
        message.push_str("...");
    }
    message
}

#[cfg(test)]
mod tests;
