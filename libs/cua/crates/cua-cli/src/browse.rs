//! One-step web browsing in a sandbox: `cua sb create --browser` and the MCP
//! `sandbox_create {"browser": true}` preset.
//!
//! The browser runs INSIDE the sandbox and is driven by the sandbox's own
//! cua-driver (embedded in cua-spacesd) through its typed browser tools
//! (`browser_prepare`, `get_browser_state`, `browser_navigate`,
//! `browser_click`, `browser_type`, ...). This module only chains the
//! setup calls every agent would otherwise make one by one: wait for the
//! desktop, launch a driver-owned throwaway Chromium profile, find its
//! window, bind it, and optionally open a first URL. All input stays with
//! cua-driver; nothing here touches the host's browsers or profiles.

use crate::{computer::Computer, sandbox};
use cua_sdk::{Cua, CuaError, SpacesdClient};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};

/// The driver session label the preset uses when the caller names none.
pub const DEFAULT_SESSION: &str = "browse";

/// Upper bound on one driver call during setup.
const CALL_TIMEOUT: Duration = Duration::from_secs(90);
/// How long the desktop (display, window manager, cua-driver) may take.
const DESKTOP_TIMEOUT: Duration = Duration::from_secs(180);
/// Polls for the launched browser's first window (bounded).
const WINDOW_POLLS: u32 = 60;
const WINDOW_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Calls one cua-driver tool on the sandbox's spacesd and returns its
/// structured result (or its text, parsed as JSON when it is JSON). A
/// tool-level failure is an error carrying the tool's own message.
pub async fn driver_call(env: &SpacesdClient, tool: &str, args: Value) -> Result<Value, CuaError> {
    use cua_spacesd_client::pb;
    let resp = env
        .inner()
        .driver()
        .call_tool(pb::CallToolRequest {
            name: tool.into(),
            arguments_json: args.to_string(),
            timeout: Some(pbjson_types::Duration {
                seconds: CALL_TIMEOUT.as_secs() as i64,
                nanos: 0,
            }),
        })
        .await
        .map_err(|s| CuaError::Env(format!("{tool}: {}", s.message())))?
        .into_inner();
    let text: Vec<String> = resp
        .content
        .iter()
        .filter_map(|c| match &c.content {
            Some(pb::tool_content::Content::Text(t)) => Some(t.clone()),
            _ => None,
        })
        .collect();
    let structured = (!resp.structured_json.trim().is_empty())
        .then(|| serde_json::from_str::<Value>(&resp.structured_json).ok())
        .flatten();
    if resp.is_error {
        let detail = structured
            .as_ref()
            .map(Value::to_string)
            .unwrap_or_else(|| text.join("\n"));
        return Err(CuaError::Env(format!("{tool} failed: {detail}")));
    }
    Ok(structured.unwrap_or_else(|| {
        let joined = text.join("\n");
        serde_json::from_str(&joined).unwrap_or_else(|_| json!({ "text": joined }))
    }))
}

/// The first window `pid` owns, from a `list_windows` result.
fn window_of(list: &Value, pid: i64) -> Option<i64> {
    list["windows"]
        .as_array()?
        .iter()
        .filter(|w| w["pid"].as_i64() == Some(pid))
        .find_map(|w| w["window_id"].as_i64())
}

/// The active tab of a bind result (else its first tab).
fn tab_of(bind: &Value) -> Option<String> {
    let tabs = bind["tabs"].as_array()?;
    tabs.iter()
        .find(|t| t["active"].as_bool() == Some(true))
        .or_else(|| tabs.first())
        .and_then(|t| t["tab_id"].as_str())
        .map(str::to_string)
}

/// What an agent needs to drive the bound browser with `call_tool`: one
/// call template plus the extra arguments of each browser tool.
fn next_steps(space: &str, session: &str, target: &str, tab: &str) -> Value {
    json!({
        "call_tool": {
            "space": space,
            "tool": "<one of tools>",
            "arguments": {"session": session, "target_id": target, "tab_id": tab, "...": "the tool's own arguments"},
        },
        "tools": {
            "browser_navigate": {"url": "https://example.com"},
            "get_browser_state": {"snapshot_format": "semantic_v2", "include_screenshot": false},
            "browser_click": {"ref": "p1:0", "delivery_mode": "foreground"},
            "browser_type": {"ref": "p1:0", "text": "hello", "replace": true},
            "browser_pointer": {"action": "scroll", "ref": "p1:0", "delta_y": 600, "delivery_mode": "foreground"},
        },
        "note": "Read (get_browser_state) before acting: refs (p<snapshot>:<n>) come from the latest read and change after every page change. include_screenshot=true returns the tab as an image. Submit a field with browser_type text \"\\n\" and mode \"keystrokes\". delivery_mode foreground is fine here: the browser runs alone inside the sandbox. Delete the sandbox when done (sandbox_delete).",
    })
}

/// Launches a driver-owned Chromium with a fresh throwaway profile in the
/// sandbox `sb`, binds it, and navigates to `url` when given. Returns the
/// ids every later browser tool call needs.
pub async fn start(
    cua: &Arc<Cua>,
    sb: &str,
    url: Option<&str>,
    session: &str,
) -> Result<Value, CuaError> {
    let env = sandbox::env_of(cua, sb).await?;
    Computer::new(env.clone())
        .ensure_desktop(DESKTOP_TIMEOUT)
        .await?;
    let prepared = driver_call(
        &env,
        "browser_prepare",
        json!({
            "session": session,
            "allow_launch": true,
            "profile": {"mode": "isolated_new"},
        }),
    )
    .await?;
    let pid = prepared["prepared_pid"].as_i64().ok_or_else(|| {
        CuaError::Env(format!(
            "browser_prepare launched no browser (does the image ship Chromium? see `cua images ls --browser`): {prepared}"
        ))
    })?;
    let mut window = None;
    for _ in 0..WINDOW_POLLS {
        let list = driver_call(&env, "list_windows", json!({"pid": pid})).await?;
        window = window_of(&list, pid);
        if window.is_some() {
            break;
        }
        tokio::time::sleep(WINDOW_POLL_INTERVAL).await;
    }
    let window = window.ok_or_else(|| {
        CuaError::Timeout(format!(
            "the browser (pid {pid}) opened no window within {}s",
            WINDOW_POLLS as u64 * WINDOW_POLL_INTERVAL.as_millis() as u64 / 1000
        ))
    })?;
    let bind = driver_call(
        &env,
        "get_browser_state",
        json!({"session": session, "pid": pid, "window_id": window}),
    )
    .await?;
    if bind["status"] != "ok" || bind["mutation_allowed"] != true {
        return Err(CuaError::Env(format!(
            "the browser window could not be bound exactly: {bind}"
        )));
    }
    let target = bind["target_id"]
        .as_str()
        .ok_or_else(|| CuaError::Env(format!("bind returned no target_id: {bind}")))?
        .to_string();
    let tab =
        tab_of(&bind).ok_or_else(|| CuaError::Env(format!("bind returned no tab: {bind}")))?;
    let mut page = json!({"url": "about:blank"});
    if let Some(u) = url.filter(|u| !u.is_empty()) {
        page = driver_call(
            &env,
            "browser_navigate",
            json!({"session": session, "target_id": target, "tab_id": tab, "url": u}),
        )
        .await?;
    }
    Ok(json!({
        "space": sb,
        "session": session,
        "browser": {
            "product": "chromium",
            "profile": "driver-owned throwaway profile (deleted with the sandbox)",
            "pid": pid,
            "window_id": window,
            "target_id": target,
            "tab_id": tab,
        },
        "url": page["url"],
        "next": next_steps(sb, session, &target, &tab),
    }))
}

/// Registers the sandbox `sb` as a Space (so `call_tool`, `list_tools` and
/// the other Spaces tools accept it) unless it already is one.
pub async fn register_space(cua: &Arc<Cua>, sb: &str) -> Result<(), CuaError> {
    let spaces = cua.spaces();
    let listed = spaces.call_tool_json("list_spaces".into(), None).await?;
    let known = listed.content_json.contains(&format!("\"{sb}\""));
    if known {
        return Ok(());
    }
    let r = spaces
        .call_tool_json("add_space".into(), Some(json!({"url": sb}).to_string()))
        .await?;
    if r.is_error {
        return Err(CuaError::Env(format!(
            "could not register {sb} as a Space: {}",
            r.content_json
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_launched_window_and_the_active_tab() {
        let list = json!({"windows": [
            {"pid": 7, "window_id": 1},
            {"pid": 365, "window_id": 39845892},
            {"pid": 365, "window_id": 2},
        ]});
        assert_eq!(window_of(&list, 365), Some(39845892));
        assert_eq!(window_of(&list, 9), None);
        let bind = json!({"tabs": [
            {"tab_id": "a", "active": false},
            {"tab_id": "b", "active": true},
        ]});
        assert_eq!(tab_of(&bind).as_deref(), Some("b"));
        assert_eq!(
            tab_of(&json!({"tabs": [{"tab_id": "x"}]})).as_deref(),
            Some("x")
        );
        assert_eq!(tab_of(&json!({})), None);
    }

    #[test]
    fn next_steps_name_the_ids_once() {
        let n = next_steps("local:b", "s", "bt-1", "tab-1");
        let call = &n["call_tool"];
        assert_eq!(call["space"], "local:b");
        assert_eq!(call["arguments"]["session"], "s");
        assert_eq!(call["arguments"]["target_id"], "bt-1");
        assert_eq!(call["arguments"]["tab_id"], "tab-1");
        for tool in [
            "browser_navigate",
            "get_browser_state",
            "browser_click",
            "browser_type",
        ] {
            assert!(n["tools"].get(tool).is_some(), "{tool}");
        }
        assert_eq!(n["tools"]["browser_click"]["delivery_mode"], "foreground");
    }
}
