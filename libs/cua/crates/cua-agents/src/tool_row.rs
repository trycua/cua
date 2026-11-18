//! One short muted line per tool call: a verb and its object (`Read
//! cart.py`, `Ran python3 -m unittest`, `Edited tests/test_cart.py`),
//! the same on every client.
//!
//! Agents name the same work many ways (`Bash`, `exec_command`, `terminal`;
//! `mcp__cua-driver__click`; a Space's `space_bash`), and send long
//! absolute paths and whole command lines. [`tool_row`] maps all of that to
//! a verb with a category (so a group can say `Ran 4 commands`) and an
//! object cut to a basename or a short command, on one line of at most
//! [`ROW_MAX`] characters. The full command, arguments and output belong to
//! the row's expanded view, never to the collapsed line.
//!
//! The phone app's tool rows (`cua-spaces-app-core::phone::tool_rows`) use
//! this file; its cases are in `tests/fixtures/tool-rows.json`.

use serde_json::Value;

/// Longest collapsed tool row, in characters.
pub const ROW_MAX: usize = 64;

/// What kind of work a tool call is, for grouping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cat {
    Ran,
    Read,
    Edited,
    Searched,
    Fetched,
    Computer,
    Planned,
    Delegated,
    Other,
}

impl Cat {
    /// The stable word clients and goldens use.
    pub fn word(self) -> &'static str {
        match self {
            Cat::Ran => "ran",
            Cat::Read => "read",
            Cat::Edited => "edited",
            Cat::Searched => "searched",
            Cat::Fetched => "fetched",
            Cat::Computer => "computer",
            Cat::Planned => "planned",
            Cat::Delegated => "delegated",
            Cat::Other => "other",
        }
    }

    /// The group's phrase for `n` calls of this category (`ran 4 commands`).
    fn phrase(self, n: usize) -> String {
        let (verb, one, many) = match self {
            Cat::Ran => ("ran", "command", "commands"),
            Cat::Read => ("read", "file", "files"),
            Cat::Edited => ("edited", "file", "files"),
            Cat::Searched => ("searched", "time", "times"),
            Cat::Fetched => ("fetched", "page", "pages"),
            Cat::Computer => ("used", "machine action", "machine actions"),
            Cat::Planned => ("updated", "plan", "plans"),
            Cat::Delegated => ("delegated", "task", "tasks"),
            Cat::Other => ("used", "tool", "tools"),
        };
        match (self, n) {
            (Cat::Searched, 1) => "searched once".into(),
            (Cat::Searched, _) => format!("searched {n} times"),
            (_, 1) => format!("{verb} 1 {one}"),
            _ => format!("{verb} {n} {many}"),
        }
    }
}

/// A tool call as one row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolRow {
    pub cat: Cat,
    /// The collapsed line, one line, at most [`ROW_MAX`] characters.
    pub line: String,
}

/// The group header for tool calls of these categories, in order of first
/// use: `Ran 4 commands, read 2 files`, at most three clauses and then
/// `and 2 more`. No calls: `None`.
pub fn group_label(cats: &[Cat]) -> Option<String> {
    let mut order: Vec<(Cat, usize)> = Vec::new();
    for c in cats {
        match order.iter_mut().find(|(k, _)| k == c) {
            Some((_, n)) => *n += 1,
            None => order.push((*c, 1)),
        }
    }
    if order.is_empty() {
        return None;
    }
    let mut parts: Vec<String> = order.iter().take(3).map(|(c, n)| c.phrase(*n)).collect();
    if order.len() > 3 {
        let rest: usize = order.iter().skip(3).map(|(_, n)| n).sum();
        parts.push(format!("{rest} more"));
    }
    let mut s = parts.join(", ");
    if let Some(f) = s.get(0..1) {
        let up = f.to_uppercase();
        s.replace_range(0..1, &up);
    }
    Some(s)
}

/// `text` flattened to one line and cut to `max` characters.
fn cut(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let mut c: String = flat.chars().take(max.saturating_sub(1)).collect();
    c.truncate(c.trim_end().len());
    c.push('\u{2026}');
    c
}

fn str_of<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|k| v.get(*k).and_then(Value::as_str))
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// A path as a row shows it: a relative path keeps up to two parts, an
/// absolute one only its last part.
pub fn short_path(p: &str) -> String {
    let p = p.trim().trim_matches(|c| c == '`' || c == '"' || c == '\'');
    let parts: Vec<&str> = p.split(['/', '\\']).filter(|s| !s.is_empty()).collect();
    let absolute = p.starts_with('/') || p.starts_with('~') || p.as_bytes().get(1) == Some(&b':');
    match (parts.len(), absolute) {
        (0, _) => p.to_string(),
        (_, true) | (1, _) => parts[parts.len() - 1].to_string(),
        (n, false) => parts[n.saturating_sub(2)..].join("/"),
    }
}

/// A command as a row shows it: its first line, `cd x &&` dropped, absolute
/// paths cut to their last part.
fn short_command(cmd: &str) -> String {
    let first = cmd
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let mut s = first.trim_matches('`').trim();
    while let Some(rest) = s.strip_prefix("cd ").and_then(|r| r.split_once("&&")) {
        s = rest.1.trim();
    }
    // `bash -lc "..."` and `sh -c '...'` run what is inside.
    for pre in ["bash -lc ", "bash -c ", "sh -c ", "zsh -lc ", "zsh -c "] {
        if let Some(r) = s.strip_prefix(pre) {
            s = r.trim().trim_matches(|c| c == '"' || c == '\'');
            break;
        }
    }
    let words: Vec<String> = s
        .split_whitespace()
        .map(|w| {
            if w.starts_with('/') && w.matches('/').count() >= 2 {
                short_path(w)
            } else {
                w.to_string()
            }
        })
        .collect();
    words.join(" ")
}

fn host_of(url: &str) -> String {
    let u = url.trim();
    let rest = u.split_once("://").map(|x| x.1).unwrap_or(u);
    rest.split(['/', '?', '#'])
        .next()
        .unwrap_or(rest)
        .to_string()
}

fn humanize(key: &str) -> String {
    key.replace(['_', '-'], " ").trim().to_lowercase()
}

/// `mcp__server__tool` to `(server, tool)`; any other name has no server.
fn split_mcp(name: &str) -> (Option<&str>, &str) {
    match name.strip_prefix("mcp__").and_then(|r| r.split_once("__")) {
        Some((s, t)) => (Some(s), t),
        None => (None, name),
    }
}

/// The category of a finished row line (the writer's `summary` is all a
/// client has), or `None` for a line that is not a tool call (`Thinking`,
/// `Turn 1 ended`, an install step).
pub fn cat_of_line(line: &str) -> Option<Cat> {
    const OTHER: &[&str] = &[
        "Asked ",
        "Notified ",
        "Posted ",
        "Read the thread",
        "Claimed ",
        "Checked ",
        "Created a Space",
        "Started a Space",
        "Stopped a Space",
        "Deleted a Space",
        "Listed Spaces",
        "Used ",
    ];
    const COMPUTER: &[&str] = &[
        "Clicked",
        "Double-clicked",
        "Right-clicked",
        "Typed",
        "Pressed",
        "Took a screenshot",
        "Scrolled",
        "Dragged",
        "Moved the cursor",
        "Opened",
        "Brought a window",
        "Closed",
        "Looked at the screen",
    ];
    let starts = |ps: &[&str]| ps.iter().any(|p| line.starts_with(p));
    if starts(OTHER) {
        return Some(Cat::Other);
    }
    if starts(COMPUTER) {
        return Some(Cat::Computer);
    }
    if line.starts_with("Updated the to-do list") {
        return Some(Cat::Planned);
    }
    let first = line.split_whitespace().next().unwrap_or("");
    match first {
        "Ran" => Some(Cat::Ran),
        "Read" => Some(Cat::Read),
        "Edited" | "Wrote" | "Deleted" | "Moved" => Some(Cat::Edited),
        "Searched" | "Listed" => Some(Cat::Searched),
        "Fetched" => Some(Cat::Fetched),
        "Delegated" => Some(Cat::Delegated),
        _ => None,
    }
}

fn row(cat: Cat, verb: &str, object: &str) -> ToolRow {
    let o = object.trim();
    let line = if o.is_empty() {
        verb.to_string()
    } else {
        format!("{verb} {o}")
    };
    ToolRow {
        cat,
        line: cut(&line, ROW_MAX),
    }
}

/// The first file a patch touches (`*** Update File: x`, `+++ b/x`).
fn patch_file(patch: &str) -> Option<String> {
    patch.lines().find_map(|l| {
        let l = l.trim();
        for p in [
            "*** Update File:",
            "*** Add File:",
            "*** Delete File:",
            "*** Move to:",
        ] {
            if let Some(r) = l.strip_prefix(p) {
                return Some(short_path(r));
            }
        }
        l.strip_prefix("+++ ")
            .map(|r| short_path(r.trim_start_matches("b/")))
    })
}

/// The tool call as one row. `name` is the tool's own name when the agent
/// gave one (`Bash`, `mcp__cua-driver__click`), `kind` ACP's kind, `title`
/// the agent's title, `input` the arguments (`Null` when unknown) and
/// `location` the first path the call touches, when it names one.
pub fn tool_row(name: &str, kind: &str, title: &str, input: &Value, location: &str) -> ToolRow {
    let (server, tool) = split_mcp(name.trim());
    let mut key = tool.trim().to_lowercase();
    // The generic MCP relay names the real tool in its arguments.
    if matches!(key.as_str(), "call_tool" | "computer_call")
        && let Some(t) = str_of(input, &["tool", "name"])
    {
        key = t.to_lowercase();
    }
    if key.is_empty() {
        let t = title.trim();
        if !t.is_empty() && !t.contains(' ') {
            key = split_mcp(t).1.to_lowercase();
        }
    }
    let kind = kind.trim().to_lowercase();
    let path = || {
        str_of(
            input,
            &[
                "file_path",
                "path",
                "filePath",
                "filename",
                "file",
                "notebook_path",
            ],
        )
        .map(short_path)
        .or_else(|| {
            Some(location.trim())
                .filter(|l| !l.is_empty())
                .map(short_path)
        })
    };
    let after_verb = |t: &str| -> String {
        let t = t.trim();
        let rest = t
            .split_once(' ')
            .filter(|(v, _)| {
                matches!(
                    v.to_lowercase().as_str(),
                    "read"
                        | "edit"
                        | "write"
                        | "create"
                        | "delete"
                        | "run"
                        | "list"
                        | "search"
                        | "fetch"
                        | "view"
                        | "open"
                )
            })
            .map(|x| x.1)
            .unwrap_or(t);
        short_path(rest)
    };
    let command =
        || -> Option<String> { str_of(input, &["command", "cmd", "script"]).map(short_command) };

    match key.as_str() {
        "bash" | "exec_command" | "execute" | "shell" | "run_command" | "bash_command"
        | "space_bash" | "computer_shell" | "run_shell_command" | "local_shell" => {
            let obj = command().unwrap_or_else(|| {
                let t = title.trim();
                if matches!(
                    t.to_lowercase().as_str(),
                    "" | "terminal" | "bash" | "shell" | "run command"
                ) {
                    String::new()
                } else {
                    short_command(t)
                }
            });
            if obj.is_empty() {
                row(Cat::Ran, "Ran a command", "")
            } else {
                row(Cat::Ran, "Ran", &obj)
            }
        }
        "read" | "read_file" | "view" | "cat" | "computer_file_read" | "volume_read"
        | "read_many_files" | "space_read" => {
            let obj = path().unwrap_or_else(|| after_verb(title));
            row(Cat::Read, "Read", &obj)
        }
        "edit" | "multiedit" | "str_replace" | "str_replace_editor" | "edit_file" | "replace"
        | "notebookedit" | "patch" | "apply_patch" => {
            let obj = path()
                .or_else(|| str_of(input, &["patch", "input", "diff"]).and_then(patch_file))
                .unwrap_or_else(|| after_verb(title));
            row(Cat::Edited, "Edited", &obj)
        }
        "write"
        | "write_file"
        | "create_file"
        | "computer_file_write"
        | "volume_write"
        | "space_write" => {
            let obj = path().unwrap_or_else(|| after_verb(title));
            row(Cat::Edited, "Wrote", &obj)
        }
        "delete" | "delete_file" | "remove" | "rm" => {
            let obj = path().unwrap_or_else(|| after_verb(title));
            row(Cat::Edited, "Deleted", &obj)
        }
        "ls" | "list" | "list_files" | "list_directory" | "computer_file_list" | "volume_ls" => {
            let obj = path().unwrap_or_default();
            row(Cat::Searched, "Listed", &obj)
        }
        "grep" | "glob" | "search" | "find" | "rg" | "search_files" | "codebase_search" => {
            let obj = str_of(input, &["pattern", "query", "regex", "glob"])
                .map(|p| format!("\u{201c}{}\u{201d}", cut(p, 40)))
                .unwrap_or_else(|| cut(title.trim(), 40));
            row(Cat::Searched, "Searched", &obj)
        }
        "websearch" | "web_search" | "google_web_search" => {
            let obj = str_of(input, &["query", "q"])
                .map(|q| cut(q, 44))
                .unwrap_or_default();
            row(
                Cat::Searched,
                "Searched the web",
                &if obj.is_empty() {
                    obj
                } else {
                    format!("for {obj}")
                },
            )
        }
        "webfetch" | "web_fetch" | "fetch" | "browser_navigate" | "web" => {
            let obj = str_of(input, &["url", "uri"])
                .map(host_of)
                .unwrap_or_default();
            row(Cat::Fetched, "Fetched", &obj)
        }
        "todowrite" | "todo_write" | "update_plan" | "plan" | "todo" => {
            row(Cat::Planned, "Updated the to-do list", "")
        }
        "task" | "agent" | "spawn_agent" | "delegate" => {
            let obj = str_of(input, &["description", "prompt", "task"])
                .map(|d| cut(d, 44))
                .unwrap_or_default();
            row(Cat::Delegated, "Delegated", &obj)
        }
        // Host tools.
        "repo_push" => {
            let repo = str_of(input, &["repo"]).unwrap_or("");
            row(
                Cat::Other,
                "Asked to push to",
                if repo.is_empty() { "GitHub" } else { repo },
            )
        }
        "notify_user" => row(Cat::Other, "Notified you", ""),
        "request_site_login" => row(Cat::Other, "Asked for a sign-in", ""),
        "thread_post" | "thread_reply" => row(Cat::Other, "Posted to the thread", ""),
        "thread_read" | "thread_inbox" => row(Cat::Other, "Read the thread", ""),
        "thread_claim" => row(Cat::Other, "Claimed a task", ""),
        "volume_request_access" => row(Cat::Other, "Asked for volume access", ""),
        "volume_sync_status" => row(Cat::Other, "Checked volume sync", ""),
        // Child Spaces.
        "create_space" => row(Cat::Other, "Created a Space", ""),
        "start_space" => row(Cat::Other, "Started a Space", ""),
        "stop_space" => row(Cat::Other, "Stopped a Space", ""),
        "delete_space" | "remove_space" => row(Cat::Other, "Deleted a Space", ""),
        "list_spaces" => row(Cat::Other, "Listed Spaces", ""),
        // Computer use (cua-driver and the Space computer tools).
        _ if is_computer(server, &key) => computer_row(&key, input),
        _ => {
            // By kind when the name says nothing.
            match kind.as_str() {
                "execute" => {
                    let obj = command().unwrap_or_else(|| short_command(title));
                    if obj.is_empty() || matches!(obj.to_lowercase().as_str(), "terminal" | "bash")
                    {
                        row(Cat::Ran, "Ran a command", "")
                    } else {
                        row(Cat::Ran, "Ran", &obj)
                    }
                }
                "read" => row(
                    Cat::Read,
                    "Read",
                    &path().unwrap_or_else(|| after_verb(title)),
                ),
                "edit" => row(
                    Cat::Edited,
                    "Edited",
                    &path().unwrap_or_else(|| after_verb(title)),
                ),
                "delete" => row(
                    Cat::Edited,
                    "Deleted",
                    &path().unwrap_or_else(|| after_verb(title)),
                ),
                "move" => row(
                    Cat::Edited,
                    "Moved",
                    &path().unwrap_or_else(|| after_verb(title)),
                ),
                "search" => row(Cat::Searched, "Searched", &cut(title.trim(), 40)),
                "fetch" => row(
                    Cat::Fetched,
                    "Fetched",
                    &str_of(input, &["url"]).map(host_of).unwrap_or_default(),
                ),
                _ => {
                    let h = if key.is_empty() {
                        cut(title.trim(), 40)
                    } else {
                        humanize(&key)
                    };
                    if h.is_empty() {
                        row(Cat::Other, "Used a tool", "")
                    } else {
                        row(Cat::Other, "Used", &h)
                    }
                }
            }
        }
    }
}

fn is_computer(server: Option<&str>, key: &str) -> bool {
    matches!(server, Some("cua-driver") | Some("cua_driver"))
        || key.starts_with("computer_")
        || matches!(
            key,
            "click"
                | "double_click"
                | "right_click"
                | "type_text"
                | "press_key"
                | "hotkey"
                | "screenshot"
                | "scroll"
                | "drag"
                | "move_cursor"
                | "launch_app"
                | "get_window_state"
                | "get_accessibility_tree"
                | "list_windows"
                | "get_screen_size"
                | "get_desktop_state"
        )
}

fn computer_row(key: &str, input: &Value) -> ToolRow {
    let k = key.strip_prefix("computer_").unwrap_or(key);
    let c = Cat::Computer;
    match k {
        "click" | "browser_click" => row(c, "Clicked", ""),
        "double_click" => row(c, "Double-clicked", ""),
        "right_click" => row(c, "Right-clicked", ""),
        "type" | "type_text" | "browser_type" | "set_value" => row(c, "Typed", ""),
        "press_key" | "key" | "hotkey" | "key_down" | "key_up" => {
            let keys = input
                .get("keys")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join("+")
                })
                .or_else(|| str_of(input, &["key"]).map(String::from))
                .unwrap_or_default();
            row(c, "Pressed", &cut(&keys, 24))
        }
        "screenshot" | "zoom" => row(c, "Took a screenshot", ""),
        "scroll" => row(c, "Scrolled", ""),
        "drag" => row(c, "Dragged", ""),
        "move_cursor" | "mouse_down" | "mouse_up" => row(c, "Moved the cursor", ""),
        "launch_app" | "launch" | "window_open" => row(
            c,
            "Opened",
            &str_of(input, &["name", "app", "bundle_id", "path"])
                .map(short_path)
                .unwrap_or_default(),
        ),
        "bring_to_front" | "window_focus" => row(c, "Brought a window forward", ""),
        "kill_app" | "window_close" => {
            row(c, "Closed", str_of(input, &["name", "app"]).unwrap_or(""))
        }
        "get_window_state"
        | "get_accessibility_tree"
        | "list_windows"
        | "get_screen_size"
        | "get_desktop_state"
        | "get_current_window"
        | "window_list"
        | "get_browser_state"
        | "list_apps" => row(c, "Looked at the screen", ""),
        "browser_navigate" => row(
            c,
            "Opened",
            &str_of(input, &["url"]).map(host_of).unwrap_or_default(),
        ),
        _ => row(c, "Used", &humanize(k)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The cases both this file and the TypeScript mirror run.
    #[test]
    fn the_shared_cases() {
        let cases: Value =
            serde_json::from_str(include_str!("../tests/fixtures/tool-rows.json")).unwrap();
        for c in cases["rows"].as_array().unwrap() {
            let a = &c["in"];
            let got = tool_row(
                a["name"].as_str().unwrap_or(""),
                a["kind"].as_str().unwrap_or(""),
                a["title"].as_str().unwrap_or(""),
                &a["input"],
                a["location"].as_str().unwrap_or(""),
            );
            assert_eq!(got.line, c["line"].as_str().unwrap(), "{a}");
            assert_eq!(got.cat.word(), c["cat"].as_str().unwrap(), "{a}");
            assert_eq!(cat_of_line(&got.line), Some(got.cat), "{}", got.line);
        }
        for g in cases["groups"].as_array().unwrap() {
            let cats: Vec<Cat> = g["cats"]
                .as_array()
                .unwrap()
                .iter()
                .map(|w| {
                    [
                        Cat::Ran,
                        Cat::Read,
                        Cat::Edited,
                        Cat::Searched,
                        Cat::Fetched,
                        Cat::Computer,
                        Cat::Planned,
                        Cat::Delegated,
                        Cat::Other,
                    ]
                    .into_iter()
                    .find(|c| c.word() == w.as_str().unwrap())
                    .unwrap()
                })
                .collect();
            assert_eq!(group_label(&cats).as_deref(), g["label"].as_str(), "{g}");
        }
    }

    /// The product says "machines": no row or group header says computer or device.
    #[test]
    fn no_row_or_group_says_computer_or_device() {
        let all = [
            Cat::Ran,
            Cat::Read,
            Cat::Edited,
            Cat::Searched,
            Cat::Fetched,
            Cat::Computer,
            Cat::Planned,
            Cat::Delegated,
            Cat::Other,
        ];
        let mut lines: Vec<String> = Vec::new();
        for c in all {
            for n in [1, 2, 7] {
                lines.push(group_label(&vec![c; n]).unwrap());
            }
        }
        let cases: Value =
            serde_json::from_str(include_str!("../tests/fixtures/tool-rows.json")).unwrap();
        for c in cases["rows"].as_array().unwrap() {
            lines.push(c["line"].as_str().unwrap().to_string());
        }
        for k in [
            "click",
            "screenshot",
            "get_window_state",
            "launch_app",
            "computer_click",
            "computer_type",
        ] {
            lines.push(tool_row(&format!("mcp__cua-driver__{k}"), "", "", &Value::Null, "").line);
        }
        assert!(
            lines.contains(&"Used 3 machine actions".to_string())
                || group_label(&[Cat::Computer; 3]).as_deref() == Some("Used 3 machine actions")
        );
        for l in lines {
            let l = l.to_lowercase();
            assert!(!l.contains("computer") && !l.contains("device"), "{l}");
        }
    }

    #[test]
    fn lines_that_are_not_tool_calls_have_no_category() {
        for l in [
            "Thinking: plan",
            "Turn 1 ended (end_turn)",
            "Install node: cached",
            "Error: x",
            "Allowed: Ran ls",
            "Updated the plan",
        ] {
            assert_eq!(cat_of_line(l), None, "{l}");
        }
    }

    #[test]
    fn no_row_is_longer_than_one_short_line() {
        let long = format!("python3 {}", "-m very.long.module.name ".repeat(40));
        let r = tool_row(
            "Bash",
            "execute",
            "",
            &json!({"command": format!("{long}\nsecond")}),
            "",
        );
        assert!(r.line.chars().count() <= ROW_MAX, "{}", r.line);
        assert!(!r.line.contains('\n') && r.line.ends_with('\u{2026}'));
        let r = tool_row(
            "Read",
            "read",
            "",
            &json!({"file_path": "/Users/x/very/deep/dir/cart.py"}),
            "",
        );
        assert_eq!(r.line, "Read cart.py");
        for l in [
            tool_row("x", "", "", &Value::Null, "").line,
            tool_row("", "", "", &Value::Null, "").line,
        ] {
            assert!(!l.contains('\u{b7}') && !l.contains('\n'));
        }
    }

    #[test]
    fn paths_keep_two_relative_parts_and_one_absolute_part() {
        assert_eq!(short_path("tests/unit/test_cart.py"), "unit/test_cart.py");
        assert_eq!(short_path("tests/test_cart.py"), "tests/test_cart.py");
        assert_eq!(short_path("/a/b/c.py"), "c.py");
        assert_eq!(short_path("cart.py"), "cart.py");
    }
}
