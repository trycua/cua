//! The agent-facing tool surface of `cua mcp`.
//!
//! The Spaces contract (86 tools) and the CLI's sandbox/computer tools are the
//! SDK and daemon API; advertising all of them costs an agent tens of
//! thousands of tokens on every turn. The agent surface is a small set of
//! hand-written tools, most with an `action`, that [`route`] maps onto the
//! underlying tools. The underlying names stay callable (they keep working
//! for existing agents and scripts) but are not listed, and rarely used ones
//! are reached through `more`.
//!
//! Routing happens before the permission filter and the approval gate (see
//! [`super::gate`]), so a facade call is checked exactly like the tool it
//! becomes.

use crate::error::{Error, Result};
use serde_json::{Map, Value, json};

/// Server instructions of the agent surface.
pub const INSTRUCTIONS: &str = "Cua Spaces: disposable computers the user can watch. Start with list_spaces; create_space only when none fits (on=local is free, cloud is metered; delete what you create). Work in a Space with space_bash, space_files, computer and window (its screen), list_tools/call_tool (its apps and browser). Actions on the user's own machines, cloud account or secrets ask the user for approval (Touch ID); on approval_denied stop and tell them. more lists the rarely used tools.";

/// Tools reached through `more`, not listed by default.
pub const ADVANCED: &[&str] = &[
    "persistent_agent_create",
    "persistent_agent_list",
    "persistent_agent_remove",
    "persistent_agent_send",
    "persistent_agent_save",
    "agent_pause",
    "agent_resume",
    "routine_add",
    "routine_list",
    "routine_remove",
    "routine_set_enabled",
    "notifications_list",
    "notifications_ack",
    "computer_access_grant",
    "computer_access_revoke",
    "computer_access_list",
    "hotspot_start",
    "hotspot_stop",
    "hotspot_status",
    "relay_register_space",
    "relay_unregister_space",
    "cloud_status",
    "cloud_connect",
    "cloud_test",
    "cloud_disconnect",
    "cloud_sweep",
    "volume_audit",
    "volume_storage",
    "volume_storage_set",
    "volume_mount_status",
    "volume_mount",
    "volume_unmount",
    "volume_sync_events",
    "volume_sync_resolve",
    "volume_cache_stats",
    "volume_cache_set",
    "volume_cache_clear",
    "stream_endpoint",
    "skills_list",
    "skills_read",
    "skills_record",
    "skills_delete",
];

/// Tools that only the user may use (the app, `cua` CLI or SDK): an agent
/// must not decide who may see the user's data or approve its own request.
/// Calling one through MCP is `forbidden`.
pub const USER_ONLY: &[&str] = &[
    "volume_grant",
    "volume_revoke",
    "volume_grants",
    "volume_requests",
    "volume_approve",
    "volume_deny",
];

fn s(desc: &str) -> Value {
    if desc.is_empty() {
        json!({"type": "string"})
    } else {
        json!({"type": "string", "description": desc})
    }
}
fn n(desc: &str) -> Value {
    if desc.is_empty() {
        json!({"type": "integer"})
    } else {
        json!({"type": "integer", "description": desc})
    }
}
fn b(desc: &str) -> Value {
    if desc.is_empty() {
        json!({"type": "boolean"})
    } else {
        json!({"type": "boolean", "description": desc})
    }
}
fn en(values: &[&str], desc: &str) -> Value {
    if desc.is_empty() {
        json!({"type": "string", "enum": values})
    } else {
        json!({"type": "string", "enum": values, "description": desc})
    }
}
fn obj(props: Vec<(&str, Value)>, required: &[&str]) -> Value {
    let mut p = Map::new();
    for (k, v) in props {
        p.insert(k.to_string(), v);
    }
    json!({"type": "object", "properties": p, "required": required})
}
fn tool(name: &str, description: &str, schema: Value, read_only: bool) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": schema,
        "annotations": {"readOnlyHint": read_only},
    })
}

/// The `tools/list` entries of the agent surface.
pub fn tools() -> Vec<Value> {
    let space = || s("Space id or name from list_spaces.");
    vec![
        tool(
            "list_spaces",
            "List your Spaces: id, name, provider, OS, power state, services.",
            obj(vec![], &[]),
            true,
        ),
        tool(
            "create_space",
            "Create a Space (a new computer) and wait until it is ready. Returns its id.",
            obj(
                vec![
                    (
                        "image",
                        s("Image ref or alias; see images. Default: the Linux desktop image."),
                    ),
                    (
                        "on",
                        s(
                            "local (default, free), cloud (metered), aws, gcp, modal, or host:<machine>.",
                        ),
                    ),
                    ("name", s("")),
                    ("count", n("1 to 8.")),
                    (
                        "reuse",
                        b(
                            "Return a reachable existing Space of this kind instead of creating one.",
                        ),
                    ),
                    ("wait", b("Default true; false returns while it starts.")),
                    (
                        "services",
                        json!({"type": "object", "description": "Named guest ports, e.g. {\"mcp\": 8765}."}),
                    ),
                    (
                        "command",
                        json!({"type": "array", "items": {"type": "string"}, "description": "Entrypoint argv."}),
                    ),
                    ("env", json!({"type": "object"})),
                    ("kind", en(&["auto", "container", "vm"], "")),
                    ("runtime", s("")),
                    ("timeout", n("Seconds to wait.")),
                ],
                &[],
            ),
            false,
        ),
        tool(
            "space",
            "Manage a Space: start or stop (suspend) it, delete it, forget it without deleting, or add an existing machine by url and token.",
            obj(
                vec![
                    (
                        "action",
                        en(&["start", "stop", "delete", "forget", "add"], ""),
                    ),
                    ("space", s("Space id or name (not for add).")),
                    ("url", s("add: the machine's address.")),
                    ("token", s("add: its access token.")),
                    ("name", s("add: display name.")),
                ],
                &["action"],
            ),
            false,
        ),
        tool(
            "space_bash",
            "Run a shell command in a Space. Returns stdout, stderr and the exit code.",
            obj(
                vec![
                    ("space", space()),
                    ("command", s("")),
                    ("timeout", n("Seconds, default 120.")),
                ],
                &["space", "command"],
            ),
            false,
        ),
        tool(
            "space_files",
            "Move files: write text to a file in the Space, upload a host file or folder into it, download one out, or send a host file to the Space's Downloads.",
            obj(
                vec![
                    ("action", en(&["write", "upload", "download", "send"], "")),
                    ("space", space()),
                    (
                        "path",
                        s(
                            "write: path in the Space. upload and send: host path. download: path in the Space.",
                        ),
                    ),
                    ("content", s("write: the text.")),
                    (
                        "dest",
                        s("upload: path in the Space. download: host folder."),
                    ),
                    (
                        "target_directory",
                        s("send: folder in the Space, default Downloads."),
                    ),
                ],
                &["action", "space", "path"],
            ),
            false,
        ),
        tool(
            "computer",
            "Use a Space's screen and input. Coordinates are pixels of the last screenshot, so take one first. click, double_click, move, mouse_down and mouse_up take x, y (and button); scroll takes direction and amount; drag takes start_x, start_y, end_x, end_y; hotkey takes keys like cmd+c.",
            obj(
                vec![
                    (
                        "action",
                        en(
                            &[
                                "screenshot",
                                "click",
                                "double_click",
                                "move",
                                "mouse_down",
                                "mouse_up",
                                "type",
                                "key",
                                "key_down",
                                "key_up",
                                "hotkey",
                                "scroll",
                                "drag",
                                "clipboard_get",
                                "clipboard_set",
                                "screen_size",
                                "cursor_position",
                            ],
                            "",
                        ),
                    ),
                    ("space", space()),
                    ("x", n("")),
                    ("y", n("")),
                    ("button", en(&["left", "right", "middle"], "")),
                    ("text", s("type and clipboard_set.")),
                    ("key", s("")),
                    ("keys", s("")),
                    ("direction", en(&["up", "down", "left", "right"], "")),
                    ("amount", n("")),
                    ("start_x", n("")),
                    ("start_y", n("")),
                    ("end_x", n("")),
                    ("end_y", n("")),
                ],
                &["action"],
            ),
            false,
        ),
        tool(
            "window",
            "Windows and apps in a Space. list takes app; open takes path; launch takes app and args; resize takes window_id, width, height; move takes window_id, x, y; tree returns the accessibility tree; act presses an element from a tree (snapshot_id, element_id, element_action, value).",
            obj(
                vec![
                    (
                        "action",
                        en(
                            &[
                                "list", "open", "launch", "current", "focus", "minimize",
                                "maximize", "close", "info", "resize", "move", "tree", "act",
                            ],
                            "",
                        ),
                    ),
                    ("space", space()),
                    ("window_id", s("")),
                    ("app", s("")),
                    ("path", s("")),
                    (
                        "args",
                        json!({"type": "array", "items": {"type": "string"}}),
                    ),
                    ("x", n("")),
                    ("y", n("")),
                    ("width", n("")),
                    ("height", n("")),
                    ("max_depth", n("")),
                    ("snapshot_id", s("")),
                    ("element_id", s("")),
                    ("element_action", s("Default press.")),
                    ("value", s("")),
                ],
                &["action"],
            ),
            false,
        ),
        tool(
            "list_tools",
            "List the tools of a service inside a Space: the desktop driver (default) or an MCP server the image runs. name filters and returns full schemas.",
            obj(
                vec![
                    ("space", space()),
                    ("service", s("")),
                    ("name", s("Substring filter.")),
                ],
                &["space"],
            ),
            true,
        ),
        tool(
            "call_tool",
            "Call a tool listed by list_tools.",
            obj(
                vec![
                    ("space", space()),
                    ("tool", s("")),
                    ("arguments", json!({"type": "object"})),
                    ("service", s("")),
                ],
                &["space", "tool"],
            ),
            false,
        ),
        tool(
            "open_browser",
            "Open a throwaway browser in a Space (a new Linux Space when none is given) and return the session, target_id and tab_id for the browser tools of call_tool.",
            obj(
                vec![
                    ("space", s("Existing Space; omit to create one.")),
                    ("url", s("")),
                    ("on", s("New Space only: local or cloud.")),
                ],
                &[],
            ),
            false,
        ),
        tool(
            "show_space",
            "FOR THE HUMAN: show a Space on the user's screen. Call only when asked. mode: viewer (full desktop), pip (small floating view), hide (close the pip), window (one window by window_id, title or app_name).",
            obj(
                vec![
                    ("space", space()),
                    ("mode", en(&["viewer", "pip", "hide", "window"], "")),
                    ("window_id", s("")),
                    ("title", s("")),
                    ("app_name", s("")),
                ],
                &["space", "mode"],
            ),
            false,
        ),
        tool(
            "agent",
            "Run a coding agent inside a Space. start (agent, prompt) returns a run_id; then message (run_id, text), status, events (cursor), interrupt, stop, list, or capabilities (supported agents).",
            obj(
                vec![
                    (
                        "action",
                        en(
                            &[
                                "start",
                                "message",
                                "status",
                                "events",
                                "interrupt",
                                "stop",
                                "list",
                                "capabilities",
                            ],
                            "",
                        ),
                    ),
                    ("space", space()),
                    ("run_id", s("")),
                    ("agent", s("Harness name from capabilities.")),
                    ("prompt", s("")),
                    ("text", s("")),
                    ("model", s("")),
                    ("cwd", s("")),
                    ("repo", s("")),
                    ("branch", s("")),
                    ("home", s("Persistent agent name whose memory to use.")),
                    ("cursor", s("events: continue from this cursor.")),
                    (
                        "env_from_host",
                        json!({"type": "array", "items": {"type": "string"}, "description": "Provider key variables to forward from the host."}),
                    ),
                ],
                &["action"],
            ),
            false,
        ),
        tool(
            "volume",
            "Cua Volume, the user's versioned shared files. Actions: ls, read, write (content), delete, history, restore (version), request_access (prefix, mode r|rw, reason: the user approves in the app), sync_status. path is relative to the volume root.",
            obj(
                vec![
                    (
                        "action",
                        en(
                            &[
                                "ls",
                                "read",
                                "write",
                                "delete",
                                "history",
                                "restore",
                                "request_access",
                                "sync_status",
                            ],
                            "",
                        ),
                    ),
                    ("path", s("")),
                    ("content", s("")),
                    ("encoding", en(&["utf8", "base64"], "")),
                    ("version", s("")),
                    ("prefix", s("")),
                    ("mode", en(&["r", "rw"], "")),
                    ("reason", s("")),
                    ("as_agent", s("Act as this agent to see what it sees.")),
                    ("in_space", s("")),
                ],
                &["action"],
            ),
            false,
        ),
        tool(
            "share_space",
            "Share a Space with someone through the relay (always needs the user's approval), stop sharing, or list who it is shared with.",
            obj(
                vec![
                    ("action", en(&["share", "unshare", "list"], "")),
                    ("space", space()),
                    ("who", s("Email or account id.")),
                    ("role", en(&["viewer", "editor"], "")),
                ],
                &["action", "space"],
            ),
            false,
        ),
        tool(
            "teleport",
            "Copy the user's signed-in app or browser session into a Space. manifest previews what would move; app and browser need the user's approval in Cua: the first call returns a request_id, call again with it once approved. browser takes sites.",
            obj(
                vec![
                    ("action", en(&["manifest", "app", "browser"], "")),
                    ("space", space()),
                    ("app", s("App id such as firefox, chrome or claude-code.")),
                    ("scope", en(&["full", "tabs"], "")),
                    (
                        "include",
                        json!({"type": "array", "items": {"type": "string"}}),
                    ),
                    (
                        "sites",
                        json!({"type": "array", "items": {"type": "string"}, "description": "browser: exact sites, e.g. github.com."}),
                    ),
                    ("browser", en(&["firefox", "chrome"], "")),
                    ("acknowledge_sensitive", b("")),
                    ("duration_minutes", n("")),
                    ("reason", s("")),
                    (
                        "request_id",
                        s("From the first call, after the user approved."),
                    ),
                ],
                &["action", "space"],
            ),
            false,
        ),
        tool(
            "request_site_login",
            "Sign in to a website in a Space's browser with a password the user saved in Keyvault. The first call files an approval request and returns a request_id; call again with it once the user approved. The password is never returned.",
            obj(
                vec![
                    ("space", space()),
                    ("url", s("The site's sign-in page.")),
                    ("request_id", s("")),
                    ("username", s("")),
                    (
                        "session",
                        s("Browser tab from get_browser_state: session, target_id, tab_id."),
                    ),
                    ("target_id", s("")),
                    ("tab_id", s("")),
                ],
                &["space", "url"],
            ),
            false,
        ),
        tool(
            "notify_user",
            "Send the user a notification in the Cua app.",
            obj(vec![("title", s("")), ("body", s(""))], &["title"]),
            false,
        ),
        tool(
            "images",
            "List the sandbox images create_space accepts.",
            obj(
                vec![
                    ("os", en(&["linux", "windows", "macos"], "")),
                    ("browser", b("Only images with a browser.")),
                ],
                &[],
            ),
            true,
        ),
        tool(
            "approvals",
            "Read which actions need the user's approval. Only the user changes this, in Settings \u{2192} Permissions.",
            obj(vec![], &[]),
            true,
        ),
        tool(
            "more",
            "Rarely used tools: cloud accounts, persistent agents and routines, hotspot, relay, volume storage and sync, skills. No name lists them; name returns one tool's full schema; name with arguments calls it.",
            obj(
                vec![("name", s("")), ("arguments", json!({"type": "object"}))],
                &[],
            ),
            false,
        ),
    ]
}

/// True when `name` is one of the agent surface's own tools.
pub fn is_facade(name: &str) -> bool {
    matches!(
        name,
        "space"
            | "space_files"
            | "computer"
            | "window"
            | "open_browser"
            | "show_space"
            | "agent"
            | "volume"
            | "share_space"
            | "teleport"
            | "images"
            | "approvals"
            | "more"
    )
}

/// The tools a call of agent-surface tool `name` can become (the tool itself
/// when it is not a facade).
pub fn underlying(name: &str) -> Vec<String> {
    match name {
        "more" | "approvals" => return vec![],
        "open_browser" => return vec!["sandbox_open_browser".into(), "sandbox_create".into()],
        _ => {}
    }
    if !is_facade(name) {
        return vec![name.to_string()];
    }
    let list = tools();
    let props = list
        .iter()
        .find(|t| t["name"] == name)
        .map(|t| t["inputSchema"]["properties"].clone())
        .unwrap_or_default();
    let key = if props.get("action").is_some() {
        "action"
    } else {
        "mode"
    };
    let values: Vec<String> = props[key]["enum"]
        .as_array()
        .map(|v| {
            v.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let mut out = vec![];
    for v in values {
        if let Some(Ok((tool, _))) = route(name, &json!({ key: v })) {
            out.push(tool);
        }
    }
    if out.is_empty() {
        if let Some(Ok((tool, _))) = route(name, &json!({})) {
            out.push(tool);
        }
    }
    out
}

/// The name an MCP client is known by: the Volume principal of the
/// agent surface (`host-<client>`), a valid agent name.
pub fn client_identity(client: &str) -> String {
    let mut slug: String = client
        .to_ascii_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    slug.truncate(40);
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "host-agent".to_string()
    } else {
        format!("host-{slug}")
    }
}

/// The Volume tools that take `as_agent`.
pub fn is_volume_data_tool(tool: &str) -> bool {
    matches!(
        tool,
        "volume_ls"
            | "volume_read"
            | "volume_write"
            | "volume_delete"
            | "volume_history"
            | "volume_restore"
            | "volume_request_access"
            | "volume_sync_status"
    )
}

fn bad(tool: &str, msg: impl std::fmt::Display) -> Error {
    Error::invalid(format!("{tool}: {msg}"))
}

fn take_action(tool: &str, a: &mut Map<String, Value>) -> Result<String> {
    match a.remove("action") {
        Some(Value::String(s)) if !s.is_empty() => Ok(s),
        _ => Err(bad(tool, "needs an action (see the tool's inputSchema)")),
    }
}

fn rename(a: &mut Map<String, Value>, from: &str, to: &str) {
    if let Some(v) = a.remove(from) {
        a.insert(to.to_string(), v);
    }
}

fn args_map(a: &Value) -> Map<String, Value> {
    a.as_object().cloned().unwrap_or_default()
}

/// Maps a call of an agent-surface tool to `(underlying tool, arguments)`.
/// `None` when `name` is not a facade tool (it is called as it is).
/// `more` and `approvals` are answered by the server itself and are not
/// routed here.
pub fn route(name: &str, a: &Value) -> Option<Result<(String, Value)>> {
    if !is_facade(name) || matches!(name, "more" | "approvals") {
        return None;
    }
    // `share_space` is also the contract's own tool, called without an action.
    if name == "share_space" && a.get("action").is_none() {
        return None;
    }
    Some(route_inner(name, a))
}

fn route_inner(name: &str, a: &Value) -> Result<(String, Value)> {
    let mut m = args_map(a);
    let out = |t: &str, m: Map<String, Value>| Ok((t.to_string(), Value::Object(m)));
    match name {
        "images" => out("images_list", m),
        "space" => {
            let action = take_action(name, &mut m)?;
            match action.as_str() {
                "start" => out("start_space", m),
                "stop" => out("stop_space", m),
                "delete" => out("delete_space", m),
                "forget" => out("remove_space", m),
                "add" => out("add_space", m),
                other => Err(bad(name, format!("unknown action {other:?}"))),
            }
        }
        "space_files" => {
            let action = take_action(name, &mut m)?;
            match action.as_str() {
                "write" => out("space_write", m),
                "upload" => out("upload", m),
                "download" => out("download", m),
                "send" => out("send_file", m),
                other => Err(bad(name, format!("unknown action {other:?}"))),
            }
        }
        "computer" => {
            let action = take_action(name, &mut m)?;
            rename(&mut m, "space", "sandbox");
            let tool = match action.as_str() {
                "screenshot" => "computer_screenshot",
                "click" => "computer_click",
                "double_click" => "computer_double_click",
                "move" => "computer_move_cursor",
                "mouse_down" => "computer_mouse_down",
                "mouse_up" => "computer_mouse_up",
                "type" => "computer_type",
                "key" => "computer_key",
                "key_down" => "computer_key_down",
                "key_up" => "computer_key_up",
                "hotkey" => "computer_hotkey",
                "scroll" => "computer_scroll",
                "drag" => "computer_drag",
                "clipboard_get" => "computer_clipboard_get",
                "clipboard_set" => "computer_clipboard_set",
                "screen_size" => "computer_get_screen_size",
                "cursor_position" => "computer_get_cursor_position",
                other => return Err(bad(name, format!("unknown action {other:?}"))),
            };
            out(tool, m)
        }
        "window" => {
            let action = take_action(name, &mut m)?;
            rename(&mut m, "space", "sandbox");
            let tool = match action.as_str() {
                "list" => "computer_window_list",
                "open" => "computer_window_open",
                "launch" => "computer_launch",
                "current" => "computer_get_current_window",
                "focus" => "computer_window_focus",
                "minimize" => "computer_window_minimize",
                "maximize" => "computer_window_maximize",
                "close" => "computer_window_close",
                "info" => "computer_window_get_info",
                "resize" => "computer_window_resize",
                "move" => "computer_window_move",
                "tree" => "computer_get_accessibility_tree",
                "act" => {
                    rename(&mut m, "element_action", "action");
                    "computer_accessibility_act"
                }
                other => return Err(bad(name, format!("unknown action {other:?}"))),
            };
            out(tool, m)
        }
        "open_browser" => {
            if m.get("space")
                .and_then(Value::as_str)
                .is_some_and(|s| !s.is_empty())
            {
                rename(&mut m, "space", "name");
                out("sandbox_open_browser", m)
            } else {
                m.remove("space");
                m.insert("browser".into(), json!(true));
                out("sandbox_create", m)
            }
        }
        "show_space" => {
            let mode = match m.remove("mode") {
                Some(Value::String(s)) => s,
                _ => return Err(bad(name, "needs a mode (see the tool's inputSchema)")),
            };
            match mode.as_str() {
                "viewer" => out("open_space_viewer", m),
                "pip" => out("show_space_pip", m),
                "hide" => out("hide_space_pip", m),
                "window" => out("stream_space_window", m),
                other => Err(bad(name, format!("unknown mode {other:?}"))),
            }
        }
        "agent" => {
            let action = take_action(name, &mut m)?;
            let tool = match action.as_str() {
                "start" => "agent_start",
                "message" => "agent_message",
                "status" => "agent_status",
                "events" => "agent_events",
                "interrupt" => "agent_interrupt",
                "stop" => "agent_stop",
                "list" => "agent_list",
                "capabilities" => "agent_capabilities",
                other => return Err(bad(name, format!("unknown action {other:?}"))),
            };
            out(tool, m)
        }
        "volume" => {
            let action = take_action(name, &mut m)?;
            let tool = match action.as_str() {
                "ls" => "volume_ls",
                "read" => "volume_read",
                "write" => "volume_write",
                "delete" => "volume_delete",
                "history" => "volume_history",
                "restore" => "volume_restore",
                "request_access" => "volume_request_access",
                "sync_status" => "volume_sync_status",
                other => return Err(bad(name, format!("unknown action {other:?}"))),
            };
            out(tool, m)
        }
        "share_space" => {
            let action = take_action(name, &mut m)?;
            match action.as_str() {
                "share" => out("share_space", m),
                "unshare" => out("unshare_space", m),
                "list" => out("space_shares", m),
                other => Err(bad(name, format!("unknown action {other:?}"))),
            }
        }
        "teleport" => {
            let action = take_action(name, &mut m)?;
            match action.as_str() {
                "manifest" => out("teleport_manifest", m),
                "app" => out("teleport_app", m),
                "browser" => {
                    rename(&mut m, "space", "name");
                    out("teleport_browser_session", m)
                }
                other => Err(bad(name, format!("unknown action {other:?}"))),
            }
        }
        _ => Err(bad(name, "not an agent-surface tool")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_facade_is_routable_or_server_owned() {
        for t in tools() {
            let name = t["name"].as_str().unwrap();
            if is_facade(name) && !matches!(name, "more" | "approvals") {
                // A call without an action is a clear error, not a panic.
                let r = route(name, &json!({"action": "", "mode": ""})).unwrap();
                let needs_action = r.is_err();
                let takes_action = t["inputSchema"]["properties"].get("action").is_some()
                    || t["inputSchema"]["properties"].get("mode").is_some();
                assert_eq!(needs_action, takes_action, "{name}");
            }
        }
    }

    #[test]
    fn actions_route_to_the_underlying_tools() {
        let (t, a) = route("space", &json!({"action": "forget", "space": "x"}))
            .unwrap()
            .unwrap();
        assert_eq!((t.as_str(), &a), ("remove_space", &json!({"space": "x"})));
        let (t, a) = route(
            "computer",
            &json!({"action": "click", "space": "s", "x": 1, "y": 2}),
        )
        .unwrap()
        .unwrap();
        assert_eq!(t, "computer_click");
        assert_eq!(a["sandbox"], "s");
        let (t, a) = route(
            "window",
            &json!({"action": "act", "space": "s", "element_action": "press", "element_id": "e"}),
        )
        .unwrap()
        .unwrap();
        assert_eq!(t, "computer_accessibility_act");
        assert_eq!(a["action"], "press");
        let (t, _) = route("open_browser", &json!({"url": "https://x"}))
            .unwrap()
            .unwrap();
        assert_eq!(t, "sandbox_create");
        assert!(route("space_bash", &json!({})).is_none());
        assert!(route("space", &json!({"action": "nope"})).unwrap().is_err());
    }

    #[test]
    fn the_surface_is_small() {
        let list = tools();
        assert!(list.len() <= 24, "{} tools", list.len());
        let bytes = serde_json::to_string(&list).unwrap().len();
        assert!(bytes < 14_000, "{bytes} bytes");
    }
}
