//! The scenario engine: a deterministic script per conversation.
//!
//! Every wire format is parsed into one [`Convo`]; [`plan`] decides the next
//! assistant message from it, statelessly except for one-shot failures. The
//! script comes from a `mock:` directive in the latest user message that has
//! one, a `;`-separated list of actions:
//!
//! | action          | effect                                                        |
//! |-----------------|---------------------------------------------------------------|
//! | `say TEXT`      | assistant text                                                |
//! | `think TEXT`    | a reasoning block (where the wire format has one)             |
//! | `shell CMD`     | call the harness's shell tool with `CMD`                      |
//! | `plan`          | call the harness's todo or plan tool with two steps           |
//! | `tool NAME`     | call the first tool whose name contains `NAME` (MCP tools)    |
//! | `tool NAME {..}`| the same, with these JSON arguments over the schema defaults  |
//! | `slow N`        | stream the final message as N chunks, one per second          |
//! | `fail429`       | answer the first request with a 429 (then behave)             |
//! | `fail500`       | answer the first request with a 500 (then behave)             |
//!
//! Each tool action is one assistant message; the harness runs the tool for
//! real and sends the result back, and the next action runs. After the last
//! action the final message quotes the last tool result, so a transcript
//! proves the tool actually executed. No directive: a one-line reply, unless
//! a [`Rule`] matches a user message (see [`plan_with`]); rules let a demo
//! type a natural prompt and still get a deterministic plan.

use serde_json::{Map, Value, json};
use std::collections::HashSet;
use std::sync::Mutex;
use std::time::Duration;

/// Who said it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

/// One piece of a message.
#[derive(Clone, Debug, PartialEq)]
pub enum Part {
    Text(String),
    ToolUse { id: String, name: String },
    ToolResult { text: String },
}

/// One message.
#[derive(Clone, Debug, PartialEq)]
pub struct Msg {
    pub role: Role,
    pub parts: Vec<Part>,
}

/// A tool the harness offered.
#[derive(Clone, Debug, PartialEq)]
pub struct Tool {
    pub name: String,
    /// JSON Schema of its input.
    pub schema: Value,
}

/// A conversation as the model sees it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Convo {
    pub messages: Vec<Msg>,
    pub tools: Vec<Tool>,
}

/// One block of the reply.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    Thinking(String),
    Text(String),
    Call { name: String, input: Value },
}

/// A scripted failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    RateLimited,
    Server,
}

/// The next reply.
#[derive(Clone, Debug, PartialEq)]
pub struct Reply {
    pub steps: Vec<Step>,
    /// Chunks the text is streamed in.
    pub chunks: usize,
    /// Pause between chunks.
    pub pace: Duration,
    pub fail: Option<Failure>,
}

impl Reply {
    fn new(steps: Vec<Step>) -> Self {
        Reply {
            steps,
            chunks: 3,
            pace: Duration::from_millis(20),
            fail: None,
        }
    }

    /// Whether the reply ends with a tool call.
    pub fn calls_tool(&self) -> bool {
        matches!(self.steps.last(), Some(Step::Call { .. }))
    }
}

/// Failures already served, so each fires once per conversation.
#[derive(Default)]
pub struct Failures(Mutex<HashSet<String>>);

#[derive(Clone, Debug, PartialEq)]
enum Action {
    Say(String),
    Think(String),
    Shell(String),
    Plan,
    Tool(String, Option<Value>),
    Slow(usize),
    Fail(Failure),
}

fn parse_actions(directive: &str) -> Vec<Action> {
    directive
        .split(';')
        .filter_map(|raw| {
            let raw = raw.trim();
            let (verb, rest) = raw.split_once(' ').unwrap_or((raw, ""));
            let rest = rest.trim().to_string();
            Some(match verb {
                "say" | "echo" => Action::Say(rest),
                "think" => Action::Think(rest),
                "shell" => Action::Shell(rest),
                "plan" => Action::Plan,
                "tool" => {
                    // `tool NAME {json}`: arguments start at the first `{`.
                    match rest.find('{') {
                        Some(i) => {
                            let args = serde_json::from_str::<Value>(&rest[i..]).ok();
                            Action::Tool(rest[..i].trim().to_string(), args)
                        }
                        None => Action::Tool(rest, None),
                    }
                }
                "slow" => Action::Slow(rest.parse().unwrap_or(30)),
                "fail429" => Action::Fail(Failure::RateLimited),
                "fail500" => Action::Fail(Failure::Server),
                "" => return None,
                _ => Action::Say(raw.to_string()),
            })
        })
        .collect()
}

fn text_of(m: &Msg) -> String {
    m.parts
        .iter()
        .filter_map(|p| match p {
            Part::Text(t) => Some(t.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A script chosen by prompt text rather than by an inline `mock:` directive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    /// Text the user message contains.
    pub contains: String,
    /// The directive to run, in `mock:` syntax (`say hello; shell ls`).
    pub script: String,
    /// Match regardless of case.
    pub ignore_case: bool,
}

/// Rules from JSON, in either spelling:
///
/// - `[{"match": "text", "script": "say ..."}]` (`--rules`): case-sensitive;
/// - `[{"when": "text", "do": "say ..."}]` (`--scripts`): case-insensitive.
pub fn parse_rules(json: &str) -> Result<Vec<Rule>, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    v.as_array()
        .ok_or("rules: expected a JSON array")?
        .iter()
        .map(|r| {
            let s = |k: &str| r.get(k).and_then(Value::as_str).map(str::to_string);
            match (s("match"), s("script"), s("when"), s("do")) {
                (Some(contains), Some(script), None, None) if !contains.is_empty() => Ok(Rule {
                    contains,
                    script,
                    ignore_case: false,
                }),
                (None, None, Some(contains), Some(script)) if !contains.is_empty() => Ok(Rule {
                    contains,
                    script,
                    ignore_case: true,
                }),
                _ => Err(format!(
                    "rules: every rule needs a non-empty match and a script \
                     ({{\"match\", \"script\"}} or {{\"when\", \"do\"}}): {r}"
                )),
            }
        })
        .collect()
}

impl Rule {
    fn matches(&self, text: &str) -> bool {
        if self.ignore_case {
            text.to_lowercase().contains(&self.contains.to_lowercase())
        } else {
            text.contains(&self.contains)
        }
    }
}

/// The inline directive in `text`, else the script of the first rule whose
/// text it contains.
fn directive_or_rule(text: &str, rules: &[Rule]) -> Option<String> {
    directive(text).or_else(|| {
        rules
            .iter()
            .find(|r| r.matches(text))
            .map(|r| r.script.clone())
    })
}

/// The directive after the last `mock:` in `text`, to the end of its line.
pub fn directive(text: &str) -> Option<String> {
    let at = text.rfind("mock:")?;
    let rest = &text[at + 5..];
    Some(rest.lines().next().unwrap_or("").trim().to_string())
}

fn last_tool_result(messages: &[Msg]) -> Option<String> {
    messages.iter().rev().find_map(|m| {
        m.parts.iter().rev().find_map(|p| match p {
            Part::ToolResult { text } => Some(text.clone()),
            _ => None,
        })
    })
}

fn snippet(s: &str, n: usize) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= n {
        flat
    } else {
        format!("{}...", flat.chars().take(n).collect::<String>())
    }
}

/// Plans the next assistant message.
pub fn plan(convo: &Convo, failures: &Failures) -> Reply {
    plan_with(convo, failures, &[])
}

/// [`plan`], with scripts chosen by prompt text as well.
pub fn plan_with(convo: &Convo, failures: &Failures, rules: &[Rule]) -> Reply {
    let Some((at, dir)) = convo
        .messages
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, m)| m.role == Role::User)
        .find_map(|(i, m)| directive_or_rule(&text_of(m), rules).map(|d| (i, d)))
    else {
        let prompt = convo
            .messages
            .iter()
            .rev()
            .find(|m| m.role == Role::User)
            .map(text_of)
            .unwrap_or_default();
        return Reply::new(vec![Step::Text(format!(
            "mock reply (scripted): {}",
            snippet(&prompt, 80)
        ))]);
    };
    let actions = parse_actions(&dir);
    // Tool calls this script already made since its directive.
    let done = convo.messages[at + 1..]
        .iter()
        .filter(|m| m.role == Role::Assistant)
        .flat_map(|m| m.parts.iter())
        .filter(|p| matches!(p, Part::ToolUse { .. }))
        .count();
    let mut reply = Reply::new(vec![]);
    if done == 0 {
        let key = format!("{}\u{1f}{dir}", convo.messages.len().min(at + 1));
        for a in &actions {
            if let Action::Fail(f) = a
                && failures.0.lock().unwrap().insert(format!("{key}{f:?}"))
            {
                reply.fail = Some(*f);
                return reply;
            }
        }
    }
    let mut pending: Vec<Step> = vec![];
    let mut calls = 0;
    for a in &actions {
        match a {
            Action::Say(t) => pending.push(Step::Text(t.clone())),
            Action::Think(t) => pending.push(Step::Thinking(t.clone())),
            Action::Slow(n) => {
                reply.chunks = (*n).max(1);
                reply.pace = Duration::from_secs(1);
            }
            Action::Fail(_) => {}
            Action::Shell(_) | Action::Plan | Action::Tool(..) => {
                let call = match a {
                    Action::Shell(cmd) => shell_call(&convo.tools, cmd),
                    Action::Plan => plan_call(&convo.tools),
                    Action::Tool(name, args) => named_call(&convo.tools, name, args.as_ref()),
                    _ => unreachable!(),
                };
                match call {
                    // Already made (and answered): what led up to it is said.
                    Ok(_) if calls < done => {
                        calls += 1;
                        pending.clear();
                    }
                    Ok(step) => {
                        pending.push(step);
                        reply.steps = pending;
                        return reply;
                    }
                    // No such tool: it never produced a call. Say so the first
                    // time and go on with the script.
                    Err(why) => {
                        if calls == done {
                            pending.push(Step::Text(why));
                        }
                    }
                }
            }
        }
    }
    if !pending.iter().any(|s| matches!(s, Step::Text(_))) {
        let text = match last_tool_result(&convo.messages[at..]) {
            Some(out) if done > 0 => format!("Done. Last tool output: {}", snippet(&out, 300)),
            _ => "Done.".into(),
        };
        pending.push(Step::Text(text));
    }
    reply.steps = pending;
    reply
}

const SHELL_NAMES: &[&str] = &[
    "bash",
    "shell",
    "exec_command",
    "shell_command",
    "local_shell",
    "developer__shell",
    "run_shell_command",
    "run_command",
    "terminal",
];
const PLAN_NAMES: &[&str] = &["todowrite", "todo_write", "update_plan", "write_todos"];

fn find<'a>(tools: &'a [Tool], exact: &[&str], contains: &[&str]) -> Option<&'a Tool> {
    tools
        .iter()
        .find(|t| exact.contains(&t.name.to_ascii_lowercase().as_str()))
        .or_else(|| {
            tools.iter().find(|t| {
                let n = t.name.to_ascii_lowercase();
                contains.iter().any(|c| n.contains(c))
            })
        })
}

fn tool_names(tools: &[Tool]) -> String {
    let mut names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    names.truncate(40);
    names.join(", ")
}

fn shell_call(tools: &[Tool], cmd: &str) -> Result<Step, String> {
    let t = find(tools, SHELL_NAMES, &["shell", "bash", "exec"]).ok_or_else(|| {
        format!(
            "(mock: no shell tool offered; tools: {})",
            tool_names(tools)
        )
    })?;
    let mut input = fill(&t.schema, "Run the scripted mock command", 0);
    let obj = input
        .as_object_mut()
        .ok_or("shell tool schema is not an object")?;
    let props = t.schema.get("properties").and_then(Value::as_object);
    let key = ["command", "cmd", "script", "commandLine", "CommandLine"]
        .into_iter()
        .find(|k| props.is_some_and(|p| p.contains_key(*k)))
        .unwrap_or("command");
    let is_array = props
        .and_then(|p| p.get(key))
        .and_then(|s| s.get("type"))
        .and_then(Value::as_str)
        == Some("array");
    obj.insert(
        key.into(),
        if is_array {
            json!(["bash", "-lc", cmd])
        } else {
            json!(cmd)
        },
    );
    Ok(Step::Call {
        name: t.name.clone(),
        input,
    })
}

fn plan_call(tools: &[Tool]) -> Result<Step, String> {
    let t = find(tools, PLAN_NAMES, &["todo", "plan"])
        .ok_or_else(|| format!("(mock: no plan tool offered; tools: {})", tool_names(tools)))?;
    Ok(Step::Call {
        name: t.name.clone(),
        input: fill(&t.schema, "Scripted mock step", 0),
    })
}

/// MCP server the cua-agents harness registers the sandbox's `/mcp` under.
const SANDBOX_MCP_SERVER: &str = "cua-driver";

/// `base` with every key of `args` (an object) set over it.
fn with_args(mut base: Value, args: Option<&Value>) -> Value {
    if let (Some(obj), Some(Value::Object(extra))) = (base.as_object_mut(), args) {
        for (k, v) in extra {
            obj.insert(k.clone(), v.clone());
        }
    } else if let Some(a) = args {
        return a.clone();
    }
    base
}

fn named_call(tools: &[Tool], needle: &str, args: Option<&Value>) -> Result<Step, String> {
    // An exact name (or `server__name` suffix) beats a substring match.
    let exact = tools
        .iter()
        .find(|t| t.name == needle || t.name.ends_with(&format!("__{needle}")));
    if let Some(t) = exact.or_else(|| tools.iter().find(|t| t.name.contains(needle))) {
        return Ok(Step::Call {
            name: t.name.clone(),
            input: with_args(fill(&t.schema, "mock", 0), args),
        });
    }
    // Harnesses that load MCP tools lazily (Google Antigravity) offer one
    // generic `call_mcp_tool` instead of a native tool per MCP tool.
    if let Some(t) = tools.iter().find(|t| t.name == "call_mcp_tool") {
        let (server, tool) = needle
            .split_once('/')
            .unwrap_or((SANDBOX_MCP_SERVER, needle));
        let mut input = fill(&t.schema, "Calling the MCP tool", 0);
        let obj = input
            .as_object_mut()
            .ok_or("call_mcp_tool schema is not an object")?;
        obj.insert("ServerName".into(), json!(server));
        obj.insert("ToolName".into(), json!(tool));
        obj.insert(
            "Arguments".into(),
            args.cloned().unwrap_or_else(|| json!({})),
        );
        return Ok(Step::Call {
            name: t.name.clone(),
            input,
        });
    }
    Err(format!(
        "(mock: no tool matching {needle:?}; tools: {})",
        tool_names(tools)
    ))
}

/// A value that satisfies `schema`: every required property, enums take the
/// first "in progress"-like value (else the first), arrays get two items.
pub fn fill(schema: &Value, hint: &str, depth: usize) -> Value {
    if depth > 6 {
        return Value::Null;
    }
    if let Some(v) = schema.get("const") {
        return v.clone();
    }
    if let Some(e) = schema.get("enum").and_then(Value::as_array) {
        return e
            .iter()
            .find(|v| v.as_str().is_some_and(|s| s.contains("progress")))
            .or_else(|| e.first())
            .cloned()
            .unwrap_or(Value::Null);
    }
    for k in ["anyOf", "oneOf", "allOf"] {
        if let Some(first) = schema
            .get(k)
            .and_then(Value::as_array)
            .and_then(|a| a.first())
        {
            return fill(first, hint, depth + 1);
        }
    }
    let ty = match schema.get("type") {
        Some(Value::String(s)) => s.as_str(),
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(Value::as_str)
            .find(|s| *s != "null")
            .unwrap_or("null"),
        _ if schema.get("properties").is_some() => "object",
        _ => "string",
    };
    match ty {
        "object" => {
            let mut out = Map::new();
            let props = schema.get("properties").and_then(Value::as_object);
            let required: Vec<&str> = schema
                .get("required")
                .and_then(Value::as_array)
                .map(|r| r.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            if let Some(props) = props {
                for (name, sub) in props {
                    if required.contains(&name.as_str()) {
                        let is_int = matches!(
                            sub.get("type").and_then(Value::as_str),
                            Some("integer" | "number")
                        );
                        let v = if is_int {
                            int_for(name)
                        } else {
                            fill(sub, &field_hint(name, hint), depth + 1)
                        };
                        out.insert(name.clone(), v);
                    }
                }
            }
            Value::Object(out)
        }
        "array" => {
            let item = schema
                .get("items")
                .cloned()
                .unwrap_or(json!({"type": "string"}));
            let first = fill(&item, hint, depth + 1);
            let mut second = fill(&item, "Report the result", depth + 1);
            if let Some(o) = second.as_object_mut() {
                for (k, v) in o.iter_mut() {
                    if k == "status" && v.is_string() {
                        *v = json!("pending");
                    }
                    if k == "id" && v.is_string() {
                        *v = json!("2");
                    }
                }
            }
            json!([first, second])
        }
        "boolean" => json!(false),
        "integer" | "number" => json!(1),
        "null" => Value::Null,
        _ => json!(hint),
    }
}

fn field_hint(name: &str, parent: &str) -> String {
    match name {
        "id" => "1".into(),
        "activeForm" => format!("Doing: {parent}"),
        // A working directory the harness resolves against its own cwd.
        n if n.eq_ignore_ascii_case("cwd") || n.eq_ignore_ascii_case("workdir") => ".".into(),
        _ => parent.into(),
    }
}

/// Integers named like a wait or timeout get a generous value (in ms), so a
/// scripted command runs to completion synchronously.
fn int_for(name: &str) -> Value {
    let n = name.to_ascii_lowercase();
    if n.contains("wait") || n.contains("timeout") {
        json!(10_000)
    } else {
        json!(1)
    }
}

/// Roughly how many tokens `text` is (for usage fields).
pub fn tokens(text: &str) -> u64 {
    (text.len() as u64).div_ceil(4).max(1)
}

/// Splits `text` into at most `n` chunks on char boundaries.
pub fn chunks(text: &str, n: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return vec![];
    }
    let size = chars.len().div_ceil(n.max(1));
    chars.chunks(size).map(|c| c.iter().collect()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(t: &str) -> Msg {
        Msg {
            role: Role::User,
            parts: vec![Part::Text(t.into())],
        }
    }

    fn bash() -> Tool {
        Tool {
            name: "Bash".into(),
            schema: json!({"type": "object", "properties": {
                "command": {"type": "string"}, "description": {"type": "string"},
                "timeout": {"type": "number"}}, "required": ["command"]}),
        }
    }

    fn mcp_click() -> Tool {
        Tool {
            name: "mcp__cua-driver__click".into(),
            schema: json!({"type": "object", "properties": {
                "x": {"type": "number"}, "y": {"type": "number"},
                "window_id": {"type": "string"}}, "required": ["x", "y"]}),
        }
    }

    #[test]
    fn tool_arguments_override_the_schema_defaults() {
        let c = Convo {
            messages: vec![user(r#"go mock: tool click {"x": 120, "y": 48}"#)],
            tools: vec![bash(), mcp_click()],
        };
        let r = plan(&c, &Failures::default());
        let Some(Step::Call { name, input }) = r.steps.last() else {
            panic!("expected a call, got {:?}", r.steps)
        };
        assert_eq!(name, "mcp__cua-driver__click");
        assert_eq!(input["x"], json!(120));
        assert_eq!(input["y"], json!(48));
    }

    #[test]
    fn a_script_is_chosen_by_the_prompt_words_when_no_directive_is_given() {
        let scripts = parse_rules(
            r#"[{"when": "calculator", "do": "say on it; shell echo calc"},
                {"when": "notes", "do": "say notes"}]"#,
        )
        .unwrap();
        let c = Convo {
            messages: vec![user("Open the Calculator and add 2 and 2")],
            tools: vec![bash()],
        };
        let r = plan_with(&c, &Failures::default(), &scripts);
        assert!(matches!(r.steps.last(), Some(Step::Call { name, .. }) if name == "Bash"));
        // A harness's own context after the prompt does not hide it.
        let c = Convo {
            messages: vec![
                user("Open the Calculator"),
                user("# Environment\nYou are in /tmp"),
            ],
            tools: vec![bash()],
        };
        let r = plan_with(&c, &Failures::default(), &scripts);
        assert!(matches!(r.steps.last(), Some(Step::Call { name, .. }) if name == "Bash"));
        // An explicit directive still wins.
        let c = Convo {
            messages: vec![user("calculator mock: say explicit")],
            tools: vec![bash()],
        };
        let r = plan_with(&c, &Failures::default(), &scripts);
        assert_eq!(r.steps, vec![Step::Text("explicit".into())]);
        // No match: the labelled reply.
        let c = Convo {
            messages: vec![user("hello")],
            tools: vec![],
        };
        let r = plan_with(&c, &Failures::default(), &scripts);
        assert_eq!(
            r.steps,
            vec![Step::Text("mock reply (scripted): hello".into())]
        );
    }

    #[test]
    fn a_rule_scripts_a_prompt_without_a_directive() {
        let rules = parse_rules(
            r#"[{"match": "Where are we on the launch?", "script": "say On it."},
                {"match": "never", "script": "say no"}]"#,
        )
        .unwrap();
        let c = Convo {
            messages: vec![user(
                "[group:Launch] You are in a group chat.\nWhere are we on the launch?",
            )],
            tools: vec![],
        };
        let r = plan_with(&c, &Failures::default(), &rules);
        assert_eq!(r.steps, vec![Step::Text("On it.".into())]);
        // An inline directive still wins, and no match is the labelled reply.
        let inline = Convo {
            messages: vec![user("Where are we on the launch? mock: say inline")],
            tools: vec![],
        };
        assert_eq!(
            plan_with(&inline, &Failures::default(), &rules).steps,
            vec![Step::Text("inline".into())]
        );
        let other = Convo {
            messages: vec![user("hello")],
            tools: vec![],
        };
        assert!(
            matches!(&plan_with(&other, &Failures::default(), &rules).steps[0], Step::Text(t) if t.starts_with("mock reply"))
        );
        assert!(parse_rules(r#"[{"match": ""}]"#).is_err());
        assert!(parse_rules(r#"[{"match": "a", "do": "say b"}]"#).is_err());
    }

    #[test]
    fn no_directive_is_a_labelled_reply() {
        let c = Convo {
            messages: vec![user("hello there")],
            tools: vec![],
        };
        let r = plan(&c, &Failures::default());
        assert_eq!(
            r.steps,
            vec![Step::Text("mock reply (scripted): hello there".into())]
        );
    }

    #[test]
    fn a_script_walks_tool_calls_then_quotes_the_last_result() {
        let mut c = Convo {
            messages: vec![user("do it mock: think planning; say ok; shell echo hi")],
            tools: vec![bash()],
        };
        let f = Failures::default();
        let r = plan(&c, &f);
        assert_eq!(r.steps.len(), 3);
        assert_eq!(
            r.steps[2],
            Step::Call {
                name: "Bash".into(),
                input: json!({"command": "echo hi"})
            }
        );
        c.messages.push(Msg {
            role: Role::Assistant,
            parts: vec![Part::ToolUse {
                id: "t1".into(),
                name: "Bash".into(),
            }],
        });
        c.messages.push(Msg {
            role: Role::User,
            parts: vec![Part::ToolResult {
                text: "hi\n".into(),
            }],
        });
        let r = plan(&c, &f);
        assert_eq!(
            r.steps,
            vec![Step::Text("Done. Last tool output: hi".into())]
        );
    }

    #[test]
    fn array_commands_and_codex_style_schemas() {
        let t = Tool {
            name: "shell".into(),
            schema: json!({"type": "object", "properties": {
                "command": {"type": "array", "items": {"type": "string"}},
                "workdir": {"type": "string"}}, "required": ["command"]}),
        };
        let Ok(Step::Call { input, .. }) = shell_call(&[t], "ls") else {
            panic!()
        };
        assert_eq!(input, json!({"command": ["bash", "-lc", "ls"]}));
    }

    #[test]
    fn a_missing_tool_is_said_once_and_not_counted() {
        let mut c = Convo {
            messages: vec![user("mock: plan; shell ls")],
            tools: vec![bash()],
        };
        let f = Failures::default();
        let r = plan(&c, &f);
        assert!(matches!(&r.steps[0], Step::Text(t) if t.contains("no plan tool")));
        assert!(r.calls_tool());
        c.messages.push(Msg {
            role: Role::Assistant,
            parts: vec![Part::ToolUse {
                id: "t1".into(),
                name: "Bash".into(),
            }],
        });
        c.messages.push(Msg {
            role: Role::User,
            parts: vec![Part::ToolResult { text: "a b".into() }],
        });
        assert_eq!(
            plan(&c, &f).steps,
            vec![Step::Text("Done. Last tool output: a b".into())]
        );
    }

    #[test]
    fn failures_fire_once() {
        let c = Convo {
            messages: vec![user("mock: fail429; say fine")],
            tools: vec![],
        };
        let f = Failures::default();
        assert_eq!(plan(&c, &f).fail, Some(Failure::RateLimited));
        let r = plan(&c, &f);
        assert_eq!(r.fail, None);
        assert_eq!(r.steps, vec![Step::Text("fine".into())]);
    }

    #[test]
    fn plan_tools_get_schema_valid_todos() {
        let t = Tool {
            name: "TodoWrite".into(),
            schema: json!({"type": "object", "required": ["todos"], "properties": {"todos": {
                "type": "array", "items": {"type": "object",
                "required": ["content", "status", "activeForm"],
                "properties": {"content": {"type": "string"},
                    "status": {"type": "string", "enum": ["pending", "in_progress", "completed"]},
                    "activeForm": {"type": "string"}}}}}}),
        };
        let Ok(Step::Call { input, .. }) = plan_call(&[t]) else {
            panic!()
        };
        assert_eq!(input["todos"][0]["status"], "in_progress");
        assert_eq!(input["todos"][1]["status"], "pending");
    }

    #[test]
    fn slow_paces_by_the_second() {
        let c = Convo {
            messages: vec![user("mock: slow 5; say a long answer")],
            tools: vec![],
        };
        let r = plan(&c, &Failures::default());
        assert_eq!(r.chunks, 5);
        assert_eq!(r.pace, Duration::from_secs(1));
        assert_eq!(chunks("abcdefghij", 5).len(), 5);
    }

    #[test]
    fn named_tools_route_through_call_mcp_tool_when_lazy() {
        let tools = vec![
            Tool {
                name: "run_command".into(),
                schema: json!({"type": "object"}),
            },
            Tool {
                name: "call_mcp_tool".into(),
                schema: json!({"type": "object", "properties": {
                    "ServerName": {"type": "string"}, "ToolName": {"type": "string"},
                    "Arguments": {}, "toolSummary": {"type": "string"}},
                    "required": ["ServerName", "ToolName", "Arguments", "toolSummary"]}),
            },
        ];
        let Step::Call { name, input } = named_call(&tools, "list_windows", None).unwrap() else {
            panic!("expected a call");
        };
        assert_eq!(name, "call_mcp_tool");
        assert_eq!(input["ServerName"], "cua-driver");
        assert_eq!(input["ToolName"], "list_windows");
        assert_eq!(input["Arguments"], json!({}));
        assert!(input["toolSummary"].is_string());
        let Step::Call { input, .. } = named_call(&tools, "other/tool_x", None).unwrap() else {
            panic!("expected a call");
        };
        assert_eq!(
            (input["ServerName"].clone(), input["ToolName"].clone()),
            (json!("other"), json!("tool_x"))
        );
        assert!(named_call(&tools[..1], "list_windows", None).is_err());
    }
}
