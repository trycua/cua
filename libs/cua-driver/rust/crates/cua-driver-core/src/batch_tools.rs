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
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{json, Map, Value};

use crate::{
    protocol::{Content, ToolResult},
    recording_tools::ReplayRegistrySlot,
    tool::{Tool, ToolDef, ToolRegistry},
};

pub(crate) mod locate;

use locate::{Check, ElementSpec, Locator, Window, WindowSpec};

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
/// Default `max_elements` for the end-of-batch observation: the same budget as
/// a plain read, so its automatic `since` diff finds that read as a baseline.
const DEFAULT_OBSERVE_MAX_ELEMENTS: usize = crate::window_state_view::DEFAULT_MAX_ELEMENTS;
/// Step tool names models reach for that mean a batchable tool.
const TOOL_ALIASES: &[(&str, &str)] = &[
    ("press", "press_key"),
    ("key", "press_key"),
    ("keypress", "press_key"),
    ("type", "type_text"),
    ("set", "set_value"),
];
const MESSAGE_LIMIT: usize = 300;
/// Read-only tools models put in a batch; refused with a pointer to `observe`.
const OBSERVATION_TOOLS: &[&str] = &[
    OBSERVE_TOOL,
    "get_desktop_state",
    "get_accessibility_tree",
    "get_browser_state",
    "list_windows",
    "verify_state",
    "zoom",
];
/// Most `expect` checks one step may carry.
pub const MAX_EXPECTS: usize = 4;
/// Wall-time budget for one batch, waits included. Keeps a batch inside
/// client call timeouts.
pub const MAX_BATCH_MS: u64 = 60_000;
/// Fields a step object may carry besides one action.
const STEP_KEYS: &[&str] = &["tool", "args", "wait_for", "expect", "timeout_ms"];
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
            description: "The default way to act: run one or more action tools in ONE call, in \
                order, stop at the first failure, and (with `observe`) get what changed in the \
                same response. Use it whenever you know the next action(s) and would otherwise \
                re-read the window after them: fill three fields, click, and see the result in \
                one call instead of five. Even a single step plus `observe:true` replaces an \
                action call followed by a get_window_state call. \
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
                given, ONE bounded get_window_state read after the last executed step. That \
                read is a `since:\"latest\"` diff by default: only the rows that changed \
                since your last read of the window, plus a new snapshot_id. Without \
                `observe` nothing is read. The batch uses one session: steps may \
                omit `session` or repeat the batch's. `delay_ms` pauses between steps (max \
                2000). At most 32 steps.\n\n\
                TARGET BY NAME (no earlier read needed, so one batch can cross screens): \
                a step can be written `{\"click\": {...args}}` as well as `{tool, args}`, and \
                its args may name the target instead of carrying a token: `role` (button, \
                textfield, link, checkbox, menuitem, ...; AX/UIA/AT-SPI names also work), \
                `name` (the element's label; exact match wins over substring, case-insensitive), \
                `nth` (0-based, when several match), and `app` (app name or bundle id) and/or \
                `window` (title substring) instead of pid/window_id. The batch reads the window \
                fresh, waits up to 3 s (step `timeout_ms`, max 10000) for the element to \
                appear, and acts on it; with only `app` it uses that app's frontmost window, so \
                a dialog that opened on top is found. Later steps inherit the window, so name \
                `app` once. Several matches fail with the candidates listed; add `nth` or a \
                fuller name.\n\n\
                WAITS AND CHECKS: a step may add `wait_for` (checked before its action) and \
                `expect` (one check or up to 4, checked after it); a step may also be only a \
                check. A check is {role?, name?, text?, gone?, value?, value_contains?, \
                enabled?, selected?, app?, window?, timeout_ms?}: `text` matches any row of the \
                tree, static text included; `gone:true` waits for it to disappear. Checks poll \
                until they hold or time out (wait_for 5 s, expect 2 s, max 10 s), and a check \
                that does not hold fails the step with what was found instead. The report says \
                which step failed, in which phase (wait_for, find, action, expect) and why. \
                Whole batch: at most 60 s.\n\n\
                Example (fill a form, submit, confirm the dialog, check the result):\n\
                {\"steps\":[\
                {\"set_value\":{\"app\":\"Safari\",\"role\":\"textfield\",\"name\":\"Email\",\"value\":\"ada@example.com\"}},\
                {\"click\":{\"role\":\"button\",\"name\":\"Submit\"},\"expect\":{\"role\":\"button\",\"name\":\"Confirm\"}},\
                {\"click\":{\"role\":\"button\",\"name\":\"Confirm\"},\"expect\":[{\"name\":\"Confirm\",\"gone\":true},{\"text\":\"Thanks\"}]}\
                ],\"observe\":true}"
                .into(),
            input_schema: json!({
                "type": "object",
                "required": ["steps"],
                "properties": {
                    "steps": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": MAX_STEPS,
                        "description": "Ordered actions. Execution stops at the first failing step. Each step is {tool, args} or {<tool>: args}, plus optional wait_for / expect / timeout_ms; a step may also be only a wait_for or expect check.",
                        "items": {
                            "anyOf": [
                                {
                                    "type": "object",
                                    "required": ["tool"],
                                    "properties": {
                                        "tool": { "type": "string", "enum": BATCHABLE_TOOLS, "description": "Action tool to run." },
                                        "args": { "type": "object", "description": "Arguments for that tool, as in a direct call, optionally with role/name/nth/app/window to name the target instead of pid/window_id/element_token." },
                                        "wait_for": { "type": "object", "description": "Check that must hold before the action: {role?, name?, text?, gone?, app?, window?, timeout_ms?}. Default timeout 5000 ms." },
                                        "expect": { "type": ["object", "array"], "description": "Check (or up to 4) that must hold after the action: {role?, name?, text?, gone?, value?, value_contains?, enabled?, selected?, app?, window?, timeout_ms?}. Default timeout 2000 ms." },
                                        "timeout_ms": { "type": "integer", "minimum": 0, "maximum": 10000, "description": "How long a named target may take to appear. Default 3000." }
                                    },
                                    "additionalProperties": false
                                },
                                {
                                    "type": "object",
                                    "description": "Shorthand {<tool>: args} with optional wait_for / expect / timeout_ms, or a step that is only a wait_for or expect check.",
                                    "properties": {
                                        "wait_for": { "type": "object" },
                                        "expect": { "type": ["object", "array"] },
                                        "timeout_ms": { "type": "integer", "minimum": 0, "maximum": 10000 }
                                    },
                                    "additionalProperties": true
                                }
                            ]
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
                        "type": ["object", "boolean"],
                        "description": "Optional end-of-batch observation: `true`, or arguments for ONE get_window_state call. `pid` and `window_id` default to those of the last step that names both. Defaults to since=\"latest\" (only what changed since your last read of that window with the same query/max_elements/max_depth; a full read when there is none), include_screenshot=false and max_elements=250; pass include_screenshot=true to see the window, or since=null for a full read. Omit to read nothing."
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

/// One action of a step.
struct Action {
    tool: &'static str,
    /// The tool's own arguments, without the selector keys.
    args: Value,
    /// Window named by the step itself (pid/window_id/app/window).
    window: WindowSpec,
    /// Element named by description; empty when the step targets by token,
    /// pixels or focus.
    element: ElementSpec,
    find_timeout: Duration,
}

struct Step {
    action: Option<Action>,
    wait_for: Option<Check>,
    expect: Vec<Check>,
}

impl Step {
    fn label(&self) -> &'static str {
        match &self.action {
            Some(action) => action.tool,
            None if self.wait_for.is_some() => "wait_for",
            None => "expect",
        }
    }
}

struct Plan {
    steps: Vec<Step>,
    delay: Duration,
    observe: Option<Value>,
    session: Option<String>,
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

/// How one step ended.
struct StepOutcome {
    report: Value,
    line: String,
    ok: bool,
}

/// Where in a step a failure happened.
#[derive(Clone, Copy)]
enum Phase {
    WaitFor,
    Find,
    Action,
    Expect,
    Budget,
}

impl Phase {
    fn as_str(self) -> &'static str {
        match self {
            Phase::WaitFor => "wait_for",
            Phase::Find => "find",
            Phase::Action => "action",
            Phase::Expect => "expect",
            Phase::Budget => "time_budget",
        }
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

        // A trailing get_window_state step is what `observe` is for: take it as
        // the observation when the batch has none.
        let mut raw_steps = raw_steps.as_slice();
        let mut trailing_observe = None;
        if let [actions @ .., last] = raw_steps {
            let observe_given = !matches!(
                object.get("observe"),
                None | Some(Value::Null) | Some(Value::Bool(false))
            );
            if !actions.is_empty() && !observe_given {
                if let Some(args) = observation_step_args(last) {
                    trailing_observe = Some(args);
                    raw_steps = actions;
                }
            }
        }

        let mut steps = Vec::with_capacity(raw_steps.len());
        // Whether some earlier step names a window a later step can inherit.
        let mut has_window = false;
        for (index, raw) in raw_steps.iter().enumerate() {
            let step = parse_step(registry, index, raw, session.as_deref())?;
            let names_window = step
                .action
                .as_ref()
                .is_some_and(|action| !action.window.is_empty())
                || step
                    .wait_for
                    .iter()
                    .chain(step.expect.iter())
                    .any(|check| !check.window.is_empty());
            let needs_window = step
                .action
                .as_ref()
                .is_some_and(|action| !action.element.is_empty() && action.window.is_empty())
                || step
                    .wait_for
                    .iter()
                    .chain(step.expect.iter())
                    .any(|check| check.window.is_empty());
            if needs_window && !names_window && !has_window {
                return Err(PlanError::step(
                    index,
                    "names an element but no window: add `app` (or pid and window_id) to this step or an earlier one",
                ));
            }
            has_window |= names_window;
            steps.push(step);
        }

        let observe = match (trailing_observe.as_ref(), object.get("observe")) {
            (Some(value), _) => Some(parse_observe(
                registry,
                value,
                has_window,
                session.as_deref(),
            )?),
            (None, None | Some(Value::Null) | Some(Value::Bool(false))) => None,
            (None, Some(value)) => Some(parse_observe(
                registry,
                value,
                has_window,
                session.as_deref(),
            )?),
        };
        Ok(Self {
            steps,
            delay: Duration::from_millis(delay_ms),
            observe,
            session,
        })
    }

    async fn run(self, registry: &ToolRegistry) -> ToolResult {
        let total = self.steps.len();
        let started = Instant::now();
        let locator = Locator::new(
            registry,
            self.session.clone(),
            started + Duration::from_millis(MAX_BATCH_MS),
        );
        let mut reports = Vec::with_capacity(total);
        let mut lines = Vec::with_capacity(total + 2);
        let mut failed_step = None;
        // The window later steps inherit, and the last concrete window used.
        let mut context = WindowSpec::default();
        let mut last_window: Option<Window> = None;

        for (index, step) in self.steps.into_iter().enumerate() {
            if index > 0 && !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            let outcome = if started.elapsed() >= Duration::from_millis(MAX_BATCH_MS) {
                failure(
                    index,
                    step.label(),
                    Phase::Budget,
                    "time_budget",
                    format!(
                        "the batch used its {} s budget before this step",
                        MAX_BATCH_MS / 1000
                    ),
                    json!({}),
                )
            } else {
                run_step(
                    registry,
                    &locator,
                    index,
                    step,
                    &mut context,
                    &mut last_window,
                )
                .await
            };
            lines.push(outcome.line);
            reports.push(outcome.report);
            if !outcome.ok {
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
        if let Some(mut args) = self.observe {
            let filled = fill_observed_window(&mut args, last_window.as_ref());
            let result = if filled {
                registry.invoke(OBSERVE_TOOL, args).await
            } else {
                ToolResult::error("no window to observe: no step resolved one")
            };
            let ok = result.is_error != Some(true);
            let mut report = json!({ "ok": ok, "tool": OBSERVE_TOOL });
            if ok {
                let summary = result
                    .structured_content
                    .as_ref()
                    .map(observation_summary)
                    .unwrap_or_default();
                if let Some(state) = result.structured_content.clone() {
                    report["state"] = state;
                }
                if !summary.is_empty() {
                    report["summary"] = Value::String(summary.clone());
                }
                observation_content = result.content;
                lines.push(if summary.is_empty() {
                    "observation: get_window_state ok (below)".to_owned()
                } else {
                    format!("observation: {summary} (below)")
                });
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

async fn run_step(
    registry: &ToolRegistry,
    locator: &Locator<'_>,
    index: usize,
    step: Step,
    context: &mut WindowSpec,
    last_window: &mut Option<Window>,
) -> StepOutcome {
    let label = step.label();
    let step_has_no_action = step.action.is_none();
    let mut report = json!({ "index": index, "tool": label });
    let mut notes: Vec<String> = Vec::new();

    if let Some(check) = &step.wait_for {
        match locator.check(check, context).await {
            Ok(checked) => {
                if !check.window.is_empty() {
                    *context = check.window.clone();
                }
                if let Some(window) = &checked.window {
                    *last_window = Some(window.clone());
                }
                report["wait_for"] = json!({
                    "ok": true,
                    "waited_ms": checked.waited.as_millis() as u64,
                });
                notes.push(format!(
                    "waited {} ms for {}",
                    checked.waited.as_millis(),
                    check.describe()
                ));
            }
            Err(miss) => {
                if let Some(window) = &miss.window {
                    *last_window = Some(window.clone());
                }
                return failure(
                    index,
                    label,
                    Phase::WaitFor,
                    miss.code,
                    format!("{} did not hold: {}", check.describe(), miss.message),
                    report,
                );
            }
        }
    }

    if let Some(action) = step.action {
        let Action {
            tool,
            mut args,
            window,
            element,
            find_timeout,
        } = action;
        if !window.is_empty() {
            *context = window.clone();
        }
        let window_spec = if window.is_empty() {
            context.clone()
        } else {
            window
        };
        if !element.is_empty() {
            match locator.find(&window_spec, &element, find_timeout).await {
                Ok(found) => {
                    let object = args.as_object_mut().expect("object");
                    object.insert("pid".into(), json!(found.window.pid));
                    object.insert("window_id".into(), json!(found.window.window_id));
                    object.insert(
                        "element_token".into(),
                        json!(found.token().unwrap_or_default()),
                    );
                    report["target"] = json!({
                        "window": found.window.to_json(),
                        "element": found.describe(),
                    });
                    notes.push(format!(
                        "on {} in {}",
                        found.describe(),
                        found.window.describe()
                    ));
                    *last_window = Some(found.window);
                }
                Err(miss) => {
                    if let Some(window) = &miss.window {
                        *last_window = Some(window.clone());
                    }
                    return failure(index, label, Phase::Find, miss.code, miss.message, report);
                }
            }
        } else if needs_lookup(&window_spec, &args, tool, registry) {
            match locator.window(&window_spec).await {
                Ok(resolved) => {
                    let object = args.as_object_mut().expect("object");
                    object.insert("pid".into(), json!(resolved.pid));
                    if accepts(registry, tool, "window_id") {
                        object
                            .entry("window_id")
                            .or_insert_with(|| json!(resolved.window_id));
                    }
                    report["target"] = json!({ "window": resolved.to_json() });
                    *last_window = Some(resolved);
                }
                Err(miss) => {
                    return failure(index, label, Phase::Find, miss.code, miss.message, report)
                }
            }
        } else if let (Some(pid), Some(window_id)) = (
            args.get("pid").and_then(Value::as_i64),
            args.get("window_id").and_then(Value::as_u64),
        ) {
            *last_window = Some(Window {
                pid,
                window_id,
                app: None,
                title: None,
                on_screen: true,
            });
        }

        let result = registry.invoke(tool, args).await;
        let ok = result.is_error != Some(true);
        let message = bounded_message(&result);
        report["ok"] = json!(ok);
        report["message"] = json!(message);
        if !ok {
            if let Some(code) = result
                .structured_content
                .as_ref()
                .and_then(|value| value.get("code"))
                .and_then(Value::as_str)
            {
                report["code"] = Value::String(code.to_owned());
            }
            return StepOutcome {
                line: format!("{}. {label} ERROR (action): {message}", index + 1),
                report: with_phase(report, Phase::Action),
                ok: false,
            };
        }
        if !message.is_empty() {
            notes.insert(0, message);
        }
    }

    let fallback = last_window
        .as_ref()
        .map(|window| WindowSpec {
            pid: Some(window.pid),
            window_id: Some(window.window_id),
            ..Default::default()
        })
        .filter(|_| context.is_empty())
        .unwrap_or_else(|| context.clone());
    let mut checked = Vec::new();
    for (nth, check) in step.expect.iter().enumerate() {
        match locator.check(check, &fallback).await {
            Ok(outcome) => {
                if let Some(window) = &outcome.window {
                    if last_window.is_none() || step_has_no_action {
                        *last_window = Some(window.clone());
                    }
                }
                if !check.window.is_empty() && step_has_no_action {
                    *context = check.window.clone();
                }
                checked.push(json!({
                    "ok": true,
                    "check": check.describe(),
                    "observed": outcome.observed,
                    "waited_ms": outcome.waited.as_millis() as u64,
                }));
            }
            Err(miss) => {
                if let Some(window) = &miss.window {
                    *last_window = Some(window.clone());
                }
                report["expect"] = Value::Array(checked);
                let which = if step.expect.len() > 1 {
                    format!("expect {} of {}: ", nth + 1, step.expect.len())
                } else {
                    String::new()
                };
                return failure(
                    index,
                    label,
                    Phase::Expect,
                    miss.code,
                    format!("{which}{}", miss.message),
                    report,
                );
            }
        }
    }
    if !checked.is_empty() {
        notes.push(format!("{} check(s) held", checked.len()));
        report["expect"] = Value::Array(checked);
    }
    report["ok"] = json!(true);
    if report.get("message").is_none() {
        report["message"] = json!(notes.join("; "));
    }
    let detail = notes.join("; ");
    StepOutcome {
        line: format!(
            "{}. {label} ok{}",
            index + 1,
            if detail.is_empty() {
                String::new()
            } else {
                format!(": {detail}")
            }
        ),
        report,
        ok: true,
    }
}

fn failure(
    index: usize,
    label: &str,
    phase: Phase,
    code: &str,
    message: String,
    mut report: Value,
) -> StepOutcome {
    report["ok"] = json!(false);
    report["code"] = json!(code);
    report["message"] = json!(message);
    StepOutcome {
        line: format!(
            "{}. {label} ERROR ({}): {message}",
            index + 1,
            phase.as_str()
        ),
        report: with_phase(report, phase),
        ok: false,
    }
}

fn with_phase(mut report: Value, phase: Phase) -> Value {
    report["phase"] = json!(phase.as_str());
    report
}

/// A step with a window by name, or one that relies on an inherited window
/// and does not carry its own pid, needs a window lookup.
fn needs_lookup(spec: &WindowSpec, args: &Value, tool: &str, registry: &ToolRegistry) -> bool {
    if spec.app.is_some() || spec.title.is_some() {
        return true;
    }
    let has_pid = args.get("pid").is_some();
    !has_pid && !spec.is_empty() && accepts(registry, tool, "pid")
}

fn accepts(registry: &ToolRegistry, tool: &str, field: &str) -> bool {
    registry
        .get_def(tool)
        .and_then(|def| def.input_schema.get("properties"))
        .and_then(|properties| properties.get(field))
        .is_some()
}

/// Fill the observation's window from the last window the batch used.
fn fill_observed_window(args: &mut Value, last: Option<&Window>) -> bool {
    let object = args.as_object_mut().expect("object");
    if object.contains_key("pid") && object.contains_key("window_id") {
        return true;
    }
    let Some(window) = last else {
        return false;
    };
    object.entry("pid").or_insert_with(|| json!(window.pid));
    object
        .entry("window_id")
        .or_insert_with(|| json!(window.window_id));
    true
}

/// One line saying what the end-of-batch read found.
fn observation_summary(state: &Value) -> String {
    let status = state.get("since_status").and_then(Value::as_str);
    let since = state.get("since").and_then(Value::as_str).unwrap_or("");
    match status {
        Some("diff") => {
            let counts = &state["diff_counts"];
            format!(
                "changed since {since}: {} added, {} changed, {} removed",
                counts["added"].as_u64().unwrap_or(0),
                counts["changed"].as_u64().unwrap_or(0),
                counts["removed"].as_u64().unwrap_or(0)
            )
        }
        Some("no_change") => format!("no change since {since}"),
        Some(_) => "full read (no usable earlier read of this window to diff against)".to_owned(),
        None => String::new(),
    }
}

/// The arguments of a get_window_state step, in either step form.
fn observation_step_args(step: &Value) -> Option<Value> {
    let object = step.as_object()?;
    let args = if object.get("tool").and_then(Value::as_str) == Some(OBSERVE_TOOL) {
        object.get("args")
    } else if object.len() == 1 && object.contains_key(OBSERVE_TOOL) {
        object.get(OBSERVE_TOOL)
    } else {
        return None;
    };
    Some(
        args.cloned()
            .filter(Value::is_object)
            .unwrap_or_else(|| Value::Object(Map::new())),
    )
}

fn parse_step(
    registry: &ToolRegistry,
    index: usize,
    raw: &Value,
    session: Option<&str>,
) -> Result<Step, PlanError> {
    let object = raw.as_object().ok_or_else(|| {
        PlanError::step(index, "must be an object: {tool, args} or {<tool>: args}")
    })?;
    // The action: `{tool, args}`, or the shorthand `{<tool>: args}`.
    let shorthand: Vec<&String> = object
        .keys()
        .filter(|key| !STEP_KEYS.contains(&key.as_str()))
        .collect();
    let (name, raw_args) = match (object.get("tool"), shorthand.as_slice()) {
        (Some(_), [extra, ..]) => {
            return Err(PlanError::step(
                index,
                format!("unknown field `{extra}`; a step has `tool` and `args`, or one `{{<tool>: args}}` key, plus optional wait_for/expect/timeout_ms"),
            ))
        }
        (Some(tool), []) => (
            Some(
                tool.as_str()
                    .ok_or_else(|| PlanError::step(index, "`tool` must be a string"))?,
            ),
            object.get("args"),
        ),
        (None, [key]) => {
            if object.contains_key("args") {
                return Err(PlanError::step(
                    index,
                    format!("`{key}` already carries the arguments; drop `args`"),
                ));
            }
            (Some(key.as_str()), object.get(key.as_str()))
        }
        (None, []) => {
            if object.contains_key("args") {
                return Err(PlanError::step(index, "`args` without `tool`"));
            }
            (None, None)
        }
        (None, keys) => {
            return Err(PlanError::step(
                index,
                format!(
                    "one action per step, got {}",
                    keys.iter()
                        .map(|key| format!("`{key}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ))
        }
    };

    let find_timeout = match object.get("timeout_ms") {
        None | Some(Value::Null) => locate::DEFAULT_FIND_TIMEOUT_MS,
        Some(value) => value
            .as_u64()
            .filter(|ms| *ms <= locate::MAX_CHECK_TIMEOUT_MS)
            .ok_or_else(|| {
                PlanError::step(
                    index,
                    format!(
                        "`timeout_ms` must be an integer 0 to {}",
                        locate::MAX_CHECK_TIMEOUT_MS
                    ),
                )
            })?,
    };
    let wait_for = match object.get("wait_for") {
        None | Some(Value::Null) => None,
        Some(raw) => Some(
            Check::parse(raw, locate::DEFAULT_WAIT_TIMEOUT_MS)
                .map_err(|message| PlanError::step(index, format!("wait_for: {message}")))?,
        ),
    };
    let expect = match object.get("expect") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) if items.is_empty() || items.len() > MAX_EXPECTS => {
            return Err(PlanError::step(
                index,
                format!("`expect` must hold 1 to {MAX_EXPECTS} checks"),
            ))
        }
        Some(Value::Array(items)) => items
            .iter()
            .enumerate()
            .map(|(nth, raw)| {
                Check::parse(raw, locate::DEFAULT_EXPECT_TIMEOUT_MS).map_err(|message| {
                    PlanError::step(index, format!("expect {}: {message}", nth + 1))
                })
            })
            .collect::<Result<_, _>>()?,
        Some(raw) => vec![Check::parse(raw, locate::DEFAULT_EXPECT_TIMEOUT_MS)
            .map_err(|message| PlanError::step(index, format!("expect: {message}")))?],
    };

    let action = match name {
        None => {
            if wait_for.is_none() && expect.is_empty() {
                return Err(PlanError::step(
                    index,
                    "empty step: give an action ({tool, args} or {<tool>: args}) or a wait_for/expect check",
                ));
            }
            None
        }
        Some(name) => Some(parse_action(
            registry,
            index,
            name,
            raw_args,
            find_timeout,
            session,
        )?),
    };
    Ok(Step {
        action,
        wait_for,
        expect,
    })
}

fn parse_action(
    registry: &ToolRegistry,
    index: usize,
    name: &str,
    raw_args: Option<&Value>,
    find_timeout: u64,
    session: Option<&str>,
) -> Result<Action, PlanError> {
    let name = TOOL_ALIASES
        .iter()
        .find(|(alias, _)| *alias == name)
        .map_or(name, |(_, tool)| *tool);
    let tool = BATCHABLE_TOOLS
        .iter()
        .copied()
        .find(|candidate| *candidate == name)
        .ok_or_else(|| {
            let message = if OBSERVATION_TOOLS.contains(&name) {
                format!(
                    "`{name}` reads state and a batch only acts. Put get_window_state arguments \
                     in `observe` (one read after the last step; a get_window_state as the \
                     last step is taken as `observe`), or use a wait_for/expect check, or split \
                     the batch where you need to look. Batchable tools: {}",
                    BATCHABLE_TOOLS.join(", ")
                )
            } else {
                format!(
                    "`{name}` cannot run in a batch; allowed tools: {}",
                    BATCHABLE_TOOLS.join(", ")
                )
            };
            PlanError::step(index, message)
        })?;
    let mut args = match raw_args {
        None | Some(Value::Null) => Value::Object(Map::new()),
        Some(value @ Value::Object(_)) => value.clone(),
        Some(_) => return Err(PlanError::step(index, "`args` must be an object")),
    };
    prepare_args(&mut args, session).map_err(|message| PlanError::step(index, message))?;
    let (window, element) = locate::take_selector(args.as_object_mut().expect("object"))
        .map_err(|message| PlanError::step(index, format!("{tool}: {message}")))?;
    crate::tool::normalize_argument_aliases(tool, &mut args)
        .map_err(|message| PlanError::step(index, format!("{tool}: {message}")))?;
    if !element.is_empty() {
        if tool == "drag" {
            return Err(PlanError::step(
                index,
                "drag takes coordinates; it cannot target an element by name",
            ));
        }
        if let Some(conflict) = ["element_token", "element_index", "x", "y"]
            .iter()
            .find(|key| args.get(**key).is_some())
        {
            return Err(PlanError::step(
                index,
                format!("{tool}: target by role/name OR by `{conflict}`, not both"),
            ));
        }
    }
    // Check the arguments now, with stand-ins for what the lookup fills.
    let mut probe = args.clone();
    if let Some(object) = probe.as_object_mut() {
        let needs_window = !element.is_empty() || window.app.is_some() || window.title.is_some();
        if needs_window {
            object.entry("pid").or_insert(json!(1));
            if accepts(registry, tool, "window_id") {
                object.entry("window_id").or_insert(json!(1));
            }
        }
        if !element.is_empty() {
            object.insert("element_token".into(), json!("s00000001:1"));
        }
    }
    validate_against_schema(registry, tool, &probe)
        .map_err(|message| PlanError::step(index, format!("{tool}: {message}")))?;
    Ok(Action {
        tool,
        args,
        window,
        element,
        find_timeout: Duration::from_millis(find_timeout),
    })
}

fn parse_observe(
    registry: &ToolRegistry,
    value: &Value,
    has_window: bool,
    session: Option<&str>,
) -> Result<Value, PlanError> {
    let mut args = match value {
        Value::Bool(true) => Value::Object(Map::new()),
        Value::Object(object) => Value::Object(object.clone()),
        _ => {
            return Err(PlanError::batch(
                "`observe` must be true or an object of get_window_state arguments",
            ))
        }
    };
    prepare_args(&mut args, session)
        .map_err(|message| PlanError::batch(format!("observe: {message}")))?;
    let object = args.as_object_mut().expect("object");
    // The window comes from the last window the batch used, so the common
    // "act, then look at the same window" case needs no repetition.
    let explicit = object.contains_key("pid") && object.contains_key("window_id");
    if !explicit && !has_window {
        return Err(PlanError::batch(
            "observe: `pid` and `window_id` are required (no step names a window to inherit from)",
        ));
    }
    object
        .entry("include_screenshot")
        .or_insert(Value::Bool(false));
    object
        .entry("max_elements")
        .or_insert_with(|| json!(DEFAULT_OBSERVE_MAX_ELEMENTS));
    // Diff against the caller's last read of the window unless it asked for
    // a full read (`since:null` or `full_output:true`).
    let full_output = object.get("full_output").and_then(Value::as_bool) == Some(true);
    match object.get("since") {
        Some(Value::Null) => {
            object.remove("since");
        }
        None if !full_output => {
            object.insert(
                "since".to_owned(),
                json!(crate::window_state_view::SINCE_LATEST),
            );
        }
        _ => {}
    }
    let mut probe = args.clone();
    if let Some(object) = probe.as_object_mut() {
        object.entry("pid").or_insert(json!(1));
        object.entry("window_id").or_insert(json!(1));
    }
    validate_against_schema(registry, OBSERVE_TOOL, &probe)
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

#[cfg(test)]
mod flow_tests;
