//! The QuickJS side of `run_script`. Runs on its own OS thread.
//!
//! The context gets the ECMAScript built-ins and nothing else: QuickJS has no
//! filesystem, network, process or environment API unless its `std`/`os`
//! modules are added, and no module loader is installed, so `import()` fails.
//! The one way out is `__host`, which the prelude captures and hides, and
//! which only forwards an operation name and JSON arguments to the async
//! side; that side decides what the operation may do.

use std::sync::{
    atomic::{AtomicBool, AtomicU32, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

use rquickjs::{context::EvalOptions, Context, Function, Runtime, Value as JsValue};
use serde_json::{json, Value};

/// Ops that do not count as driver calls.
const FREE_OPS: &[&str] = &["platform", "sleep"];
/// Longest console output kept.
const CONSOLE_MAX_LINES: usize = 200;
const CONSOLE_MAX_BYTES: usize = 32 * 1024;
const CONSOLE_LINE_MAX: usize = 2_000;

pub(crate) struct Limits {
    pub deadline: Instant,
    pub max_calls: u32,
    pub memory_bytes: usize,
    pub stack_bytes: usize,
}

/// One driver operation the script asked for.
pub(crate) struct HostCall {
    pub op: String,
    pub args: Value,
    pub line: u32,
    pub number: u32,
    pub reply: std::sync::mpsc::SyncSender<Result<Value, HostError>>,
}

#[derive(Debug, Clone)]
pub(crate) struct HostError {
    pub message: String,
    pub code: Option<String>,
}

impl HostError {
    pub fn new(message: impl Into<String>, code: &str) -> Self {
        Self {
            message: message.into(),
            code: Some(code.to_owned()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FailureKind {
    /// The script threw (or a driver call it made failed and it did not
    /// catch that).
    Exception,
    Syntax,
    Timeout,
    Cancelled,
    Memory,
    /// The script's promise never settled.
    Stalled,
    Internal,
}

impl FailureKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            FailureKind::Exception => "exception",
            FailureKind::Syntax => "syntax",
            FailureKind::Timeout => "timeout",
            FailureKind::Cancelled => "cancelled",
            FailureKind::Memory => "memory",
            FailureKind::Stalled => "stalled",
            FailureKind::Internal => "internal",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Failure {
    pub kind: FailureKind,
    pub message: String,
    pub line: Option<u32>,
    pub op: Option<String>,
    pub code: Option<String>,
    pub call: Option<u32>,
}

impl Failure {
    fn new(kind: FailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            line: None,
            op: None,
            code: None,
            call: None,
        }
    }
}

pub(crate) struct Outcome {
    pub result: Result<Value, Failure>,
    pub console: Vec<String>,
    pub console_truncated: bool,
    pub calls: u32,
}

#[derive(Default)]
struct Console {
    lines: Vec<String>,
    bytes: usize,
    truncated: bool,
}

impl Console {
    fn push(&mut self, level: &str, text: &str) {
        if self.lines.len() >= CONSOLE_MAX_LINES || self.bytes >= CONSOLE_MAX_BYTES {
            self.truncated = true;
            return;
        }
        let mut line = if level == "log" || level == "info" {
            text.to_owned()
        } else {
            format!("[{level}] {text}")
        };
        if line.chars().count() > CONSOLE_LINE_MAX {
            line = line.chars().take(CONSOLE_LINE_MAX).collect::<String>() + "…";
        }
        self.bytes += line.len();
        self.lines.push(line);
    }
}

/// Run `script` to completion. Blocks the calling thread.
pub(crate) fn run(
    script: &str,
    platform: &str,
    limits: Limits,
    cancel: Arc<AtomicBool>,
    calls: tokio::sync::mpsc::Sender<HostCall>,
) -> Outcome {
    let console = Arc::new(Mutex::new(Console::default()));
    let used = Arc::new(AtomicU32::new(0));
    let result = run_inner(script, platform, &limits, &cancel, calls, &console, &used);
    let console = std::mem::take(&mut *console.lock().unwrap_or_else(|e| e.into_inner()));
    Outcome {
        result,
        console: console.lines,
        console_truncated: console.truncated,
        calls: used.load(Ordering::SeqCst),
    }
}

fn run_inner(
    script: &str,
    platform: &str,
    limits: &Limits,
    cancel: &Arc<AtomicBool>,
    calls: tokio::sync::mpsc::Sender<HostCall>,
    console: &Arc<Mutex<Console>>,
    used: &Arc<AtomicU32>,
) -> Result<Value, Failure> {
    let internal = |error: rquickjs::Error| Failure::new(FailureKind::Internal, error.to_string());
    let runtime = Runtime::new().map_err(internal)?;
    runtime.set_memory_limit(limits.memory_bytes);
    runtime.set_max_stack_size(limits.stack_bytes);
    let deadline = limits.deadline;
    {
        let cancel = cancel.clone();
        runtime.set_interrupt_handler(Some(Box::new(move || {
            cancel.load(Ordering::SeqCst) || Instant::now() >= deadline
        })));
    }
    let context = Context::full(&runtime).map_err(internal)?;
    let settled: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

    let started = context.with(|ctx| -> Result<(), Failure> {
        let globals = ctx.globals();
        let host = {
            let cancel = cancel.clone();
            let used = used.clone();
            let max_calls = limits.max_calls;
            let platform = platform.to_owned();
            Function::new(
                ctx.clone(),
                move |op: String, args: String, line: Option<u32>| -> String {
                    host_call(
                        &op,
                        &args,
                        line.unwrap_or(0),
                        &platform,
                        deadline,
                        max_calls,
                        &cancel,
                        &used,
                        &calls,
                    )
                },
            )
            .map_err(internal)?
        };
        globals.set("__host", host).map_err(internal)?;
        let done = {
            let settled = settled.clone();
            Function::new(ctx.clone(), move |encoded: String| {
                let mut slot = settled.lock().unwrap_or_else(|e| e.into_inner());
                if slot.is_none() {
                    *slot = Some(encoded);
                }
            })
            .map_err(internal)?
        };
        globals.set("__done", done).map_err(internal)?;
        let log = {
            let console = console.clone();
            Function::new(ctx.clone(), move |level: String, text: String| {
                console
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(&level, &text);
            })
            .map_err(internal)?
        };
        globals.set("__log", log).map_err(internal)?;

        let mut prelude_options = EvalOptions::default();
        prelude_options.global = true;
        prelude_options.strict = true;
        prelude_options.filename = Some("prelude.js".into());
        let _: JsValue = ctx
            .eval_with_options(include_str!("prelude.js"), prelude_options)
            .map_err(|error| {
                Failure::new(
                    FailureKind::Internal,
                    format!("prelude failed: {error}: {}", describe_exception(&ctx)),
                )
            })?;
        let settle: Function = globals.get("__settle").map_err(internal)?;
        globals.remove("__settle").map_err(internal)?;

        // The script is the body of an async function, opened on its first
        // line so its line numbers are the caller's.
        let wrapped = format!("(async () => {{{script}\n}})()");
        let mut options = EvalOptions::default();
        options.global = true;
        options.strict = false;
        options.filename = Some("script.js".into());
        let promise: JsValue = match ctx.eval_with_options(wrapped, options) {
            Ok(value) => value,
            Err(_) => return Err(exception_failure(&ctx, cancel, deadline, true)),
        };
        settle
            .call::<_, ()>((promise,))
            .map_err(|_| exception_failure(&ctx, cancel, deadline, false))?;
        Ok(())
    });
    started?;

    // Every await resolves synchronously (driver calls block this thread),
    // so draining the job queue runs the script to its end.
    loop {
        match runtime.execute_pending_job() {
            Ok(true) => continue,
            Ok(false) => break,
            Err(_) => {
                if let Some(failure) = stop_reason(cancel, deadline) {
                    return Err(failure);
                }
                // An exception escaping a job is reported through __settle;
                // keep draining.
            }
        }
    }

    let settled = settled.lock().unwrap_or_else(|e| e.into_inner()).take();
    let Some(encoded) = settled else {
        return Err(stop_reason(cancel, deadline).unwrap_or_else(|| {
            Failure::new(
                FailureKind::Stalled,
                "the script awaited something that never settles (a promise nothing resolves); \
                 run_script has no timers or events, only driver calls",
            )
        }));
    };
    let outcome: Value = serde_json::from_str(&encoded)
        .map_err(|error| Failure::new(FailureKind::Internal, format!("bad result: {error}")))?;
    if outcome["ok"] == true {
        return Ok(outcome["value"].clone());
    }
    if let Some(failure) = stop_reason(cancel, deadline) {
        return Err(failure);
    }
    let name = outcome["name"].as_str();
    let message = outcome["message"].as_str().unwrap_or("error");
    // QuickJS reports an exhausted heap as a null exception or "out of memory".
    let out_of_memory = (name.is_none() && message == "null") || message.contains("out of memory");
    let mut failure = if out_of_memory {
        Failure::new(
            FailureKind::Memory,
            format!(
                "the script ran out of memory (limit {} MiB)",
                limits.memory_bytes >> 20
            ),
        )
    } else {
        Failure::new(
            FailureKind::Exception,
            match name {
                Some(name) if name != "CuaError" => format!("{name}: {message}"),
                _ => message.to_owned(),
            },
        )
    };
    failure.line = outcome["line"].as_u64().map(|line| line as u32);
    failure.op = outcome["op"].as_str().map(str::to_owned);
    failure.code = outcome["code"].as_str().map(str::to_owned);
    failure.call = outcome["call"].as_u64().map(|call| call as u32);
    Err(failure)
}

#[allow(clippy::too_many_arguments)]
fn host_call(
    op: &str,
    args: &str,
    line: u32,
    platform: &str,
    deadline: Instant,
    max_calls: u32,
    cancel: &AtomicBool,
    used: &AtomicU32,
    calls: &tokio::sync::mpsc::Sender<HostCall>,
) -> String {
    let reply = |result: Result<Value, HostError>, number: Option<u32>| -> String {
        match result {
            Ok(value) => json!({"ok": true, "result": value}).to_string(),
            Err(error) => json!({
                "ok": false,
                "error": error.message,
                "code": error.code,
                "call": number,
            })
            .to_string(),
        }
    };
    if op == "platform" {
        return reply(Ok(json!(platform)), None);
    }
    if cancel.load(Ordering::SeqCst) {
        return reply(
            Err(HostError::new("the script was cancelled", "cancelled")),
            None,
        );
    }
    let free = FREE_OPS.contains(&op);
    let number = if free {
        0
    } else {
        let number = used.fetch_add(1, Ordering::SeqCst) + 1;
        if number > max_calls {
            used.fetch_sub(1, Ordering::SeqCst);
            return reply(
                Err(HostError::new(
                    format!("driver call limit reached (max_calls={max_calls})"),
                    "call_limit",
                )),
                None,
            );
        }
        number
    };
    let args: Value = match serde_json::from_str(args) {
        Ok(value @ Value::Object(_)) => value,
        Ok(_) => {
            return reply(
                Err(HostError::new(
                    "arguments must be an object",
                    "invalid_args",
                )),
                Some(number),
            )
        }
        Err(error) => {
            return reply(
                Err(HostError::new(
                    format!("bad arguments: {error}"),
                    "invalid_args",
                )),
                Some(number),
            )
        }
    };
    let (reply_tx, reply_rx) = std::sync::mpsc::sync_channel(1);
    let request = HostCall {
        op: op.to_owned(),
        args,
        line,
        number,
        reply: reply_tx,
    };
    if calls.blocking_send(request).is_err() {
        cancel.store(true, Ordering::SeqCst);
        return reply(
            Err(HostError::new("the script was cancelled", "cancelled")),
            Some(number),
        );
    }
    let remaining = deadline.saturating_duration_since(Instant::now()) + Duration::from_millis(250);
    match reply_rx.recv_timeout(remaining) {
        Ok(result) => reply(result, (!free).then_some(number)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => reply(
            Err(HostError::new(
                "the script's wall-time limit was reached",
                "timeout",
            )),
            Some(number),
        ),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            cancel.store(true, Ordering::SeqCst);
            reply(
                Err(HostError::new("the script was cancelled", "cancelled")),
                Some(number),
            )
        }
    }
}

fn stop_reason(cancel: &AtomicBool, deadline: Instant) -> Option<Failure> {
    if cancel.load(Ordering::SeqCst) {
        Some(Failure::new(
            FailureKind::Cancelled,
            "the script was cancelled",
        ))
    } else if Instant::now() >= deadline {
        Some(Failure::new(
            FailureKind::Timeout,
            "the script's wall-time limit was reached",
        ))
    } else {
        None
    }
}

fn exception_failure(
    ctx: &rquickjs::Ctx<'_>,
    cancel: &AtomicBool,
    deadline: Instant,
    at_parse: bool,
) -> Failure {
    let caught = ctx.catch();
    if let Some(failure) = stop_reason(cancel, deadline) {
        return failure;
    }
    let (name, message, stack) = match caught.as_exception() {
        Some(exception) => {
            let name: Option<String> = exception.get("name").ok();
            (
                name,
                exception.message().unwrap_or_default(),
                exception.stack().unwrap_or_default(),
            )
        }
        None => (None, format!("{caught:?}"), String::new()),
    };
    let kind = if name.as_deref() == Some("SyntaxError") && at_parse {
        FailureKind::Syntax
    } else if message.contains("out of memory") {
        FailureKind::Memory
    } else {
        FailureKind::Exception
    };
    let mut failure = Failure::new(
        kind,
        match name {
            Some(name) => format!("{name}: {message}"),
            None => message,
        },
    );
    failure.line = script_line(&stack);
    failure
}

fn describe_exception(ctx: &rquickjs::Ctx<'_>) -> String {
    let caught = ctx.catch();
    caught
        .as_exception()
        .and_then(|exception| exception.message())
        .unwrap_or_else(|| format!("{caught:?}"))
}

fn script_line(stack: &str) -> Option<u32> {
    let at = stack.find("script.js:")?;
    stack[at + "script.js:".len()..]
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}
