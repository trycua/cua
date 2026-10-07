//! `run_script` (experimental): run a model-written JavaScript program that
//! drives the computer through Cua Driver's own actions and reads.
//!
//! Off by default. It is registered only when the person running the driver
//! opts in, with `CUA_DRIVER_EXPERIMENTAL_SCRIPT=1` or
//! `"experimental_script": true` in `~/.cua-driver/config.json`.
//! `set_config` cannot write that key, so a model cannot turn it on.
//!
//! The script runs in an embedded QuickJS engine ([`engine`]) on its own
//! thread, with no filesystem, network, process or environment access. Its
//! only reach outside the engine is the `cua` API, and every call that API
//! makes is dispatched here, inside this tool call's task, through the same
//! [`ToolRegistry`] entry point a direct call uses. Session, policy,
//! capability manifest and approvals therefore apply per call, and a script
//! can do nothing the same sequence of direct calls could not.
//!
//! Limits: wall time, number of driver calls, heap size and stack size, and
//! the script stops when the request is cancelled (the tool future is
//! dropped).

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, OnceLock,
};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{json, Map, Value};

use crate::{
    batch_tools::{
        locate::{self, Check, ElementSpec, Locator, WindowSpec},
        BATCHABLE_TOOLS,
    },
    protocol::{Content, ToolResult},
    recording_tools::ReplayRegistrySlot,
    tool::{Tool, ToolDef, ToolRegistry},
};

mod engine;

use engine::{FailureKind, HostCall, HostError, Limits};

pub const RUN_SCRIPT_TOOL: &str = "run_script";
/// Environment switch that registers the tool.
pub const ENABLE_ENV: &str = "CUA_DRIVER_EXPERIMENTAL_SCRIPT";
/// `~/.cua-driver/config.json` key that registers the tool.
pub const ENABLE_CONFIG_KEY: &str = "experimental_script";

pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;
pub const MAX_TIMEOUT_MS: u64 = 120_000;
pub const DEFAULT_MAX_CALLS: u32 = 100;
pub const MAX_MAX_CALLS: u32 = 500;
pub const MAX_SCRIPT_BYTES: usize = 64 * 1024;
const MEMORY_BYTES: usize = 32 * 1024 * 1024;
const STACK_BYTES: usize = 1024 * 1024;
const THREAD_STACK_BYTES: usize = 16 * 1024 * 1024;
const RETURN_TEXT_LIMIT: usize = 8_000;
const CALL_TEXT_LIMIT: usize = 12_000;
const IMAGES_KEPT: usize = 2;
/// Extra time the engine thread gets to notice a deadline or cancel.
const SHUTDOWN_GRACE: Duration = Duration::from_millis(1_500);

/// Driver tools a script may call, besides the batchable actions.
const READ_TOOLS: &[&str] = &[
    "launch_app",
    "list_apps",
    "list_windows",
    "get_window_state",
    "verify_state",
    "zoom",
];

/// Whether the person running the driver opted in.
pub fn enabled() -> bool {
    let config = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|home| {
            std::path::PathBuf::from(home)
                .join(".cua-driver")
                .join("config.json")
        });
    enabled_from(std::env::var(ENABLE_ENV).ok().as_deref(), config.as_deref())
}

/// The environment variable wins; otherwise the config file decides.
fn enabled_from(env: Option<&str>, config: Option<&std::path::Path>) -> bool {
    if let Some(value) = env {
        return matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        );
    }
    config
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|config| config.get(ENABLE_CONFIG_KEY).and_then(Value::as_bool))
        .unwrap_or(false)
}

pub struct RunScriptTool {
    registry: ReplayRegistrySlot,
}

impl RunScriptTool {
    pub fn new(registry: ReplayRegistrySlot) -> Self {
        Self { registry }
    }
}

static DEF: OnceLock<ToolDef> = OnceLock::new();

fn platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "mac"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "linux"
    }
}

#[async_trait]
impl Tool for RunScriptTool {
    fn def(&self) -> &ToolDef {
        DEF.get_or_init(|| ToolDef {
            name: RUN_SCRIPT_TOOL.into(),
            description: "EXPERIMENTAL. Run a JavaScript program that drives apps through Cua \
                Driver, in ONE call: loops, conditions, reads and checks between actions, and \
                only the result comes back. Use it for a multi-step flow you can write down; \
                use run_actions for a fixed list of steps.\n\n\
                The script is the body of an async function: use `await`, and `return` a JSON \
                value. It runs in a sandbox with no filesystem, network, process, timers or \
                environment; the only API is `cua`:\n\
                - `await cua.getApp(\"TextEdit\" | \"com.apple.TextEdit\" | {pid, windowId})` → \
                app (follows the app's frontmost window unless a windowId is given)\n\
                - `await cua.launch(nameOrBundleId)` → app; `cua.listApps()`, \
                `cua.listWindows({pid?})`, `cua.sleep(ms)`, `cua.computer.target`\n\
                - app.getState(opts?) → get_window_state result (`tree_markdown` or \
                `tree_diff`, `snapshot_id`; pass get_window_state options such as \
                {since:\"latest\"})\n\
                - app.query({role?, name?, text?}) → matching elements across the app's \
                windows {element_token, role, label, value, enabled, selected, frame}, then \
                matching static-text rows {role, label, value, text, display_only} (read text \
                such as a status line from their `value`)\n\
                - app.waitFor({role?, name?, text?, gone?, value?, timeout_ms?}) and \
                app.verify(predicate | predicates) (verify_state)\n\
                - app.click(t), doubleClick(t), rightClick(t), typeText(text, t?), \
                pressKey(\"return\" | \"cmd+s\", t?), hotkey([\"cmd\",\"s\"]), scroll(t, \
                direction, amount?), setValue(t, value), drag([x,y], [x,y]), zoom([x1,y1,x2,y2])\n\
                where a target `t` is {role, name, nth?} (found fresh, waits up to 3 s), an \
                element_token string, a row number from this app's last getState(), or [x, y] \
                window pixels.\n\
                - `cua.call(tool, args)` for any other allowed tool: click, double_click, \
                right_click, set_value, type_text, press_key, hotkey, scroll, drag, \
                launch_app, list_apps, list_windows, get_window_state, verify_state, zoom.\n\
                A failed driver call throws a CuaError (catch it to recover). Every call goes \
                through the same session, permission and approval checks as a direct call.\n\n\
                Limits: `timeout_ms` wall time (default 30000, max 120000), `max_calls` driver \
                calls (default 100, max 500), 32 MiB heap. Returns the return value, console \
                output, and a per-call log; an error names the script line, the call and the \
                reason.\n\n\
                Example:\n\
                const app = await cua.getApp(\"Calculator\");\n\
                for (const key of [\"7\", \"multiply\", \"6\", \"equals\"]) await app.click({role: \"button\", name: key});\n\
                const [display] = await app.query({role: \"statictext\"});\n\
                return display.value;"
                .into(),
            input_schema: json!({
                "type": "object",
                "required": ["script"],
                "properties": {
                    "script": {
                        "type": "string",
                        "minLength": 1,
                        "maxLength": MAX_SCRIPT_BYTES,
                        "description": "JavaScript: the body of an async function. `return` the result."
                    },
                    "timeout_ms": {
                        "type": "integer",
                        "minimum": 100,
                        "maximum": MAX_TIMEOUT_MS,
                        "description": "Wall-time limit. Default 30000."
                    },
                    "max_calls": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": MAX_MAX_CALLS,
                        "description": "Most driver calls the script may make. Default 100."
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
                    "run_script is not available: registry not initialised yet.",
                )
            }
        };
        let request = match Request::parse(&args) {
            Ok(request) => request,
            Err(message) => {
                return ToolResult::error(format!("run_script rejected: {message}"))
                    .with_structured(
                        json!({"ok": false, "code": "invalid_script", "detail": message}),
                    )
            }
        };
        run(&registry, request).await
    }
}

struct Request {
    script: String,
    timeout: Duration,
    max_calls: u32,
    session: Option<String>,
}

impl Request {
    fn parse(args: &Value) -> Result<Self, String> {
        let object = args.as_object().ok_or("arguments must be a JSON object")?;
        if let Some(key) = object.keys().find(|key| {
            !key.starts_with('_')
                && !matches!(
                    key.as_str(),
                    "script" | "timeout_ms" | "max_calls" | "session"
                )
        }) {
            return Err(format!("unknown field `{key}`"));
        }
        let script = object
            .get("script")
            .and_then(Value::as_str)
            .filter(|script| !script.trim().is_empty())
            .ok_or("`script` must be a non-empty string")?;
        if script.len() > MAX_SCRIPT_BYTES {
            return Err(format!("`script` is longer than {MAX_SCRIPT_BYTES} bytes"));
        }
        let timeout_ms = match object.get("timeout_ms") {
            None | Some(Value::Null) => DEFAULT_TIMEOUT_MS,
            Some(value) => value
                .as_u64()
                .filter(|ms| (100..=MAX_TIMEOUT_MS).contains(ms))
                .ok_or_else(|| {
                    format!("`timeout_ms` must be an integer 100 to {MAX_TIMEOUT_MS}")
                })?,
        };
        let max_calls = match object.get("max_calls") {
            None | Some(Value::Null) => DEFAULT_MAX_CALLS,
            Some(value) => value
                .as_u64()
                .filter(|calls| (1..=u64::from(MAX_MAX_CALLS)).contains(calls))
                .ok_or_else(|| format!("`max_calls` must be an integer 1 to {MAX_MAX_CALLS}"))?
                as u32,
        };
        Ok(Self {
            script: script.to_owned(),
            timeout: Duration::from_millis(timeout_ms),
            max_calls,
            session: object
                .get("_public_session_label")
                .and_then(Value::as_str)
                .map(str::to_owned),
        })
    }
}

/// Sets the cancel flag when the tool call ends for any reason, including
/// the request being dropped.
struct CancelOnDrop(Arc<AtomicBool>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// One entry of the per-call log.
struct CallRecord {
    number: u32,
    op: String,
    line: u32,
    ok: bool,
    elapsed: Duration,
    detail: String,
}

async fn run(registry: &ToolRegistry, request: Request) -> ToolResult {
    let started = Instant::now();
    let deadline = started + request.timeout;
    let cancel = Arc::new(AtomicBool::new(false));
    let _cancel_guard = CancelOnDrop(cancel.clone());
    let (call_tx, mut call_rx) = tokio::sync::mpsc::channel::<HostCall>(1);
    let (done_tx, mut done_rx) = tokio::sync::oneshot::channel();

    let limits = Limits {
        deadline,
        max_calls: request.max_calls,
        memory_bytes: MEMORY_BYTES,
        stack_bytes: STACK_BYTES,
    };
    let script = request.script.clone();
    let engine_cancel = cancel.clone();
    let spawned = std::thread::Builder::new()
        .name("cua-run-script".into())
        .stack_size(THREAD_STACK_BYTES)
        .spawn(move || {
            let outcome = engine::run(&script, platform(), limits, engine_cancel, call_tx);
            let _ = done_tx.send(outcome);
        });
    if let Err(error) = spawned {
        return ToolResult::error(format!("run_script could not start its engine: {error}"));
    }

    let mut api = Api::new(registry, request.session.clone(), deadline);
    let mut log: Vec<CallRecord> = Vec::new();
    let hard_stop = deadline + SHUTDOWN_GRACE;
    let outcome = loop {
        tokio::select! {
            Some(call) = call_rx.recv() => {
                let call_started = Instant::now();
                let result = match tokio::time::timeout_at(deadline.into(), api.dispatch(&call.op, call.args)).await {
                    Ok(result) => result,
                    Err(_) => Err(HostError::new("the script's wall-time limit was reached", "timeout")),
                };
                if call.number > 0 {
                    log.push(CallRecord {
                        number: call.number,
                        op: call.op.clone(),
                        line: call.line,
                        ok: result.is_ok(),
                        elapsed: call_started.elapsed(),
                        detail: match &result {
                            Ok(value) => summarize(value),
                            Err(error) => error.message.clone(),
                        },
                    });
                }
                let _ = call.reply.send(result);
            }
            outcome = &mut done_rx => break outcome.ok(),
            _ = tokio::time::sleep_until(hard_stop.into()) => {
                cancel.store(true, Ordering::SeqCst);
                break tokio::time::timeout(SHUTDOWN_GRACE, &mut done_rx).await.ok().and_then(Result::ok);
            }
        }
    };
    render(
        outcome,
        log,
        api.images,
        started.elapsed(),
        request.max_calls,
    )
}

/// Executes the script's operations through the registry.
struct Api<'a> {
    registry: &'a ToolRegistry,
    locator: Locator<'a>,
    session: Option<String>,
    deadline: Instant,
    images: Vec<Content>,
}

impl<'a> Api<'a> {
    fn new(registry: &'a ToolRegistry, session: Option<String>, deadline: Instant) -> Self {
        Self {
            registry,
            locator: Locator::new(registry, session.clone(), deadline),
            session,
            deadline,
            images: Vec::new(),
        }
    }

    async fn dispatch(&mut self, op: &str, args: Value) -> Result<Value, HostError> {
        let Value::Object(mut args) = args else {
            return Err(HostError::new(
                "arguments must be an object",
                "invalid_args",
            ));
        };
        // Runtime-private fields are never reachable from a script.
        if let Some(key) = args.keys().find(|key| key.starts_with('_')) {
            return Err(HostError::new(
                format!("`{key}` is reserved and cannot be set"),
                "invalid_args",
            ));
        }
        match args.get("session").and_then(Value::as_str) {
            Some(own)
                if Some(own) != self.session.as_deref()
                    && !(self.session.is_none() && own == "default") =>
            {
                return Err(HostError::new(
                    "a script runs in its run_script call's session; drop `session`",
                    "invalid_args",
                ))
            }
            _ => {}
        }
        args.remove("session");
        match op {
            "sleep" => {
                let ms = args.get("ms").and_then(Value::as_u64).unwrap_or(0);
                let until = Instant::now() + Duration::from_millis(ms);
                if until > self.deadline {
                    tokio::time::sleep_until(self.deadline.into()).await;
                    return Err(HostError::new(
                        "the script's wall-time limit was reached during sleep",
                        "timeout",
                    ));
                }
                tokio::time::sleep_until(until.into()).await;
                Ok(Value::Null)
            }
            "get_app" => self.get_app(args).await,
            "find" => self.find(args).await,
            "wait_for" => self.wait_for(args).await,
            tool if BATCHABLE_TOOLS.contains(&tool) => self.act(tool, args).await,
            tool if READ_TOOLS.contains(&tool) => self.invoke(tool, args).await,
            other => Err(HostError::new(
                format!(
                    "`{other}` is not available to scripts; allowed: {}, {}",
                    BATCHABLE_TOOLS.join(", "),
                    READ_TOOLS.join(", ")
                ),
                "not_allowed",
            )),
        }
    }

    async fn invoke(
        &mut self,
        tool: &str,
        mut args: Map<String, Value>,
    ) -> Result<Value, HostError> {
        if let Some(session) = &self.session {
            args.insert("session".into(), json!(session));
        }
        let result = self.registry.invoke(tool, Value::Object(args)).await;
        let mut text = String::new();
        for content in &result.content {
            match content {
                Content::Text { text: part, .. } => {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(part);
                }
                Content::Image { .. } => {
                    self.images.push(content.clone());
                    if self.images.len() > IMAGES_KEPT {
                        self.images.remove(0);
                    }
                }
            }
        }
        if result.is_error == Some(true) {
            let code = result
                .structured_content
                .as_ref()
                .and_then(|value| value.get("code"))
                .and_then(Value::as_str)
                .unwrap_or("tool_error");
            return Err(HostError::new(
                clip(text.lines().next().unwrap_or("failed"), 500),
                code,
            ));
        }
        let mut value = match result.structured_content {
            Some(Value::Object(object)) => Value::Object(object),
            Some(other) => json!({ "result": other }),
            None => json!({}),
        };
        if !text.is_empty() && value.get("text").is_none() {
            value["text"] = json!(clip(&text, CALL_TEXT_LIMIT));
        }
        Ok(value)
    }

    async fn get_app(&mut self, args: Map<String, Value>) -> Result<Value, HostError> {
        let spec = WindowSpec {
            pid: args.get("pid").and_then(Value::as_i64),
            window_id: args.get("window_id").and_then(Value::as_u64),
            app: args.get("app").and_then(Value::as_str).map(str::to_owned),
            title: args
                .get("window")
                .and_then(Value::as_str)
                .map(str::to_owned),
        };
        if spec.is_empty() {
            return Err(HostError::new(
                "getApp needs an app name, bundle id, pid or windowId",
                "invalid_args",
            ));
        }
        // Always listed, so an exact pid/window_id is confirmed and named.
        // A launch may need a moment before its window shows.
        let wait = Duration::from_millis(
            args.get("timeout_ms")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                .min(locate::MAX_CHECK_TIMEOUT_MS),
        );
        let until = Instant::now() + wait;
        loop {
            match self.locator.listed_window(&spec).await {
                Ok(window) => return Ok(window.to_json()),
                Err(miss) if Instant::now() < until && miss.code != "lookup_failed" => {
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
                Err(miss) => {
                    let hint = if miss.code == "app_not_found" {
                        "; launch it with cua.launch()"
                    } else {
                        ""
                    };
                    return Err(HostError::new(format!("{}{hint}", miss.message), miss.code));
                }
            }
        }
    }

    /// Elements matching {role, name, text} across the app's windows
    /// (topmost first): addressable ones with their element_token, then
    /// matching display-only text rows, whose `value` holds the text.
    async fn find(&mut self, mut args: Map<String, Value>) -> Result<Value, HostError> {
        let text = args
            .remove("text")
            .and_then(|value| value.as_str().map(str::to_owned));
        let (window, element) = locate::take_selector(&mut args)
            .map_err(|message| HostError::new(message, "invalid_args"))?;
        let spec = ElementSpec {
            nth: None,
            text,
            ..element
        };
        let windows = self
            .locator
            .search_windows(&window)
            .await
            .map_err(|miss| HostError::new(miss.message, miss.code))?;
        let mut found: Vec<Value> = Vec::new();
        for resolved in &windows {
            let read = self
                .locator
                .read(resolved)
                .await
                .map_err(|miss| HostError::new(miss.message, miss.code))?;
            let window_json = json!({"window_id": resolved.window_id, "title": resolved.title});
            if spec.text.is_none() {
                for element in locate::matches(&read, &spec) {
                    let mut out = Map::new();
                    for key in [
                        "element_token",
                        "role",
                        "label",
                        "value",
                        "enabled",
                        "selected",
                        "frame",
                    ] {
                        if let Some(value) = element.get(key) {
                            out.insert(key.to_owned(), value.clone());
                        }
                    }
                    out.insert("window".into(), window_json.clone());
                    found.push(Value::Object(out));
                }
            }
            for mut row in locate::display_rows(&read, &spec) {
                row["window"] = window_json.clone();
                found.push(row);
            }
            if found.len() >= 50 {
                break;
            }
        }
        found.truncate(50);
        Ok(Value::Array(found))
    }

    async fn wait_for(&mut self, args: Map<String, Value>) -> Result<Value, HostError> {
        let check = Check::parse(&Value::Object(args), locate::DEFAULT_WAIT_TIMEOUT_MS)
            .map_err(|message| HostError::new(message, "invalid_args"))?;
        match self.locator.check(&check, &WindowSpec::default()).await {
            Ok(checked) => Ok(json!({
                "ok": true,
                "observed": checked.observed,
                "waited_ms": checked.waited.as_millis() as u64,
            })),
            Err(miss) => Err(HostError::new(
                format!("{} did not hold: {}", check.describe(), miss.message),
                miss.code,
            )),
        }
    }

    async fn act(&mut self, tool: &str, mut args: Map<String, Value>) -> Result<Value, HostError> {
        let find_timeout = match args.remove("timeout_ms") {
            None | Some(Value::Null) => locate::DEFAULT_FIND_TIMEOUT_MS,
            Some(value) => value
                .as_u64()
                .filter(|ms| *ms <= locate::MAX_CHECK_TIMEOUT_MS)
                .ok_or_else(|| {
                    HostError::new("`timeout_ms` must be an integer 0 to 10000", "invalid_args")
                })?,
        };
        let (window, element) = locate::take_selector(&mut args)
            .map_err(|message| HostError::new(message, "invalid_args"))?;
        let mut target = None;
        if !element.is_empty() {
            if tool == "drag" {
                return Err(HostError::new(
                    "drag takes coordinates, not a role/name target",
                    "invalid_args",
                ));
            }
            if let Some(conflict) = ["element_token", "x", "y"]
                .iter()
                .find(|key| args.contains_key(**key))
            {
                return Err(HostError::new(
                    format!("target by role/name OR by `{conflict}`, not both"),
                    "invalid_args",
                ));
            }
            let found = self
                .locator
                .find(&window, &element, Duration::from_millis(find_timeout))
                .await
                .map_err(|miss| HostError::new(miss.message, miss.code))?;
            args.insert("pid".into(), json!(found.window.pid));
            args.insert("window_id".into(), json!(found.window.window_id));
            args.insert(
                "element_token".into(),
                json!(found.token().unwrap_or_default()),
            );
            target = Some(json!({"window": found.window.to_json(), "element": found.describe()}));
        } else if window.window_id.is_none()
            && (window.pid.is_some() || window.app.is_some() || window.title.is_some())
            && !args.contains_key("element_token")
        {
            // Pixel and focus targets act on the app's frontmost window.
            let resolved = self
                .locator
                .window(&window)
                .await
                .map_err(|miss| HostError::new(miss.message, miss.code))?;
            args.insert("pid".into(), json!(resolved.pid));
            args.insert("window_id".into(), json!(resolved.window_id));
        } else if let Some(pid) = window.pid {
            args.insert("pid".into(), json!(pid));
        }
        let mut value = self.invoke(tool, args).await?;
        if let (Some(target), Value::Object(object)) = (target, &mut value) {
            object.insert("target".into(), target);
        }
        Ok(value)
    }
}

/// Short, model-facing summary of a successful call.
fn summarize(value: &Value) -> String {
    if let Some(target) = value
        .get("target")
        .and_then(|target| target.get("element"))
        .and_then(Value::as_str)
    {
        return format!("on {target}");
    }
    if let Some(text) = value.get("text").and_then(Value::as_str) {
        return clip(text.lines().next().unwrap_or(""), 120);
    }
    match value {
        Value::Array(items) => format!("{} item(s)", items.len()),
        Value::Object(object) if object.contains_key("window_id") && object.contains_key("pid") => {
            format!("window {} of pid {}", object["window_id"], object["pid"])
        }
        Value::Null => String::new(),
        other => clip(&other.to_string(), 120),
    }
}

fn render(
    outcome: Option<engine::Outcome>,
    log: Vec<CallRecord>,
    images: Vec<Content>,
    elapsed: Duration,
    max_calls: u32,
) -> ToolResult {
    let elapsed_ms = elapsed.as_millis() as u64;
    let outcome = outcome.unwrap_or_else(|| engine::Outcome {
        result: Err(engine::Failure {
            kind: FailureKind::Timeout,
            message: "the script did not stop after its wall-time limit; its engine was abandoned"
                .into(),
            line: None,
            op: None,
            code: None,
            call: None,
        }),
        console: Vec::new(),
        console_truncated: false,
        calls: log.len() as u32,
    });
    let calls_json: Vec<Value> = log
        .iter()
        .map(|record| {
            json!({
                "call": record.number,
                "op": record.op,
                "line": (record.line > 0).then_some(record.line),
                "ok": record.ok,
                "ms": record.elapsed.as_millis() as u64,
                "detail": record.detail,
            })
        })
        .collect();
    let mut lines = Vec::new();
    let mut structured = json!({
        "elapsed_ms": elapsed_ms,
        "driver_calls": outcome.calls,
        "max_calls": max_calls,
        "console": outcome.console,
        "console_truncated": outcome.console_truncated,
        "calls": calls_json,
    });
    let is_error = match &outcome.result {
        Ok(value) => {
            structured["ok"] = json!(true);
            structured["value"] = value.clone();
            lines.push(format!(
                "run_script: ok in {elapsed_ms} ms, {} driver call(s)",
                outcome.calls
            ));
            lines.push(format!(
                "return value: {}",
                clip(&value.to_string(), RETURN_TEXT_LIMIT)
            ));
            false
        }
        Err(failure) => {
            structured["ok"] = json!(false);
            // A failed driver call the script did not catch: say which call.
            let failed_call = failure
                .call
                .and_then(|number| log.iter().find(|record| record.number == number));
            let line = failure.line.or(failed_call
                .map(|record| record.line)
                .filter(|line| *line > 0));
            let mut location = Vec::new();
            if let Some(line) = line {
                location.push(format!("line {line}"));
            }
            if let Some(record) = failed_call {
                location.push(format!("call {} ({})", record.number, record.op));
            } else if let Some(op) = &failure.op {
                location.push(format!("call to {op}"));
            }
            structured["error"] = json!({
                "kind": failure.kind.as_str(),
                "message": failure.message,
                "line": line,
                "call": failure.call,
                "op": failure.op,
                "code": failure.code,
            });
            lines.push(format!(
                "run_script: ERROR ({}){}: {}",
                failure.kind.as_str(),
                if location.is_empty() {
                    String::new()
                } else {
                    format!(" at {}", location.join(", "))
                },
                failure.message
            ));
            lines.push(format!(
                "after {elapsed_ms} ms and {} driver call(s); calls before the error took effect",
                outcome.calls
            ));
            true
        }
    };
    if !outcome.console.is_empty() {
        lines.push(format!(
            "console ({} line(s){}):",
            outcome.console.len(),
            if outcome.console_truncated {
                ", truncated"
            } else {
                ""
            }
        ));
        lines.extend(outcome.console.iter().map(|line| format!("  {line}")));
    }
    if !log.is_empty() {
        lines.push("calls:".to_owned());
        for record in &log {
            lines.push(format!(
                "  {}. {}{} {} ({} ms){}",
                record.number,
                if record.line > 0 {
                    format!("line {} ", record.line)
                } else {
                    String::new()
                },
                record.op,
                if record.ok { "ok" } else { "ERROR" },
                record.elapsed.as_millis(),
                if record.detail.is_empty() {
                    String::new()
                } else {
                    format!(": {}", clip(&record.detail, 160))
                }
            ));
        }
    }
    let mut content = vec![Content::text(lines.join("\n"))];
    content.extend(images);
    ToolResult {
        content,
        is_error: is_error.then_some(true),
        structured_content: Some(structured),
        ..Default::default()
    }
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        text.chars().take(max).collect::<String>() + "…"
    }
}

#[cfg(test)]
mod tests;
