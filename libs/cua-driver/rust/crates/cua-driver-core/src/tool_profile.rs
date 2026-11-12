// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Tool profiles: which tools `tools/list` advertises to an MCP client.
//!
//! A client that loads every advertised schema pays for all of them on every
//! model request, and a model choosing among 60 tools picks the wrong one more
//! often than among 30. A profile narrows only what is **advertised**:
//!
//! * `agent` (default) advertises the everyday desktop and browser loop.
//! * `full` advertises every tool.
//! * A comma list (`agent,recording`) adds named groups to `agent`.
//!
//! Hidden tools stay registered and callable by name, and still pass through
//! the same authorization, so an older client, skill or script that calls
//! `start_session` directly keeps working. Nothing is proxied or re-routed.
//! A tool no group names is always advertised, so a new tool is never hidden
//! by accident.
//!
//! The operator picks the profile with `CUA_DRIVER_TOOL_PROFILE`, or the
//! `tool_profile` key in `~/.cua-driver/config.json`; the variable wins.
//! Compact docs (see [`crate::tool_docs`]) are applied to the same listing.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::tool_docs::ToolDocs;

/// Environment variable that selects the profile for an MCP server process.
pub const TOOL_PROFILE_ENV: &str = "CUA_DRIVER_TOOL_PROFILE";
/// Config key with the same meaning; the environment variable wins.
pub const TOOL_PROFILE_CONFIG_KEY: &str = "tool_profile";

/// The group every profile except `full` advertises.
pub const AGENT_GROUP: &str = "agent";

/// Tool groups, in the order they are reported.
pub const GROUPS: &[(&str, &[&str])] = &[
    (
        AGENT_GROUP,
        &[
            "list_apps",
            "list_windows",
            "get_window_state",
            "verify_state",
            "launch_app",
            "kill_app",
            "bring_to_front",
            "set_window_frame",
            "invoke_menu",
            "click",
            "double_click",
            "right_click",
            "drag",
            "type_text",
            "press_key",
            "hotkey",
            "set_value",
            "scroll",
            "clipboard_read",
            "clipboard_write",
            "get_desktop_state",
            "move_cursor",
            "zoom",
            "page",
            "get_browser_state",
            "browser_prepare",
            "browser_navigate",
            "browser_click",
            "browser_type",
            "browser_pointer",
            "get_guide",
            "run_steps",
            "run_script",
            "parse_visual_regions",
        ],
    ),
    (
        "browser",
        &[
            "browser_select",
            "browser_dialog",
            "browser_download",
            "browser_set_input_files",
            "install_extension",
        ],
    ),
    (
        "recording",
        &[
            "start_recording",
            "stop_recording",
            "get_recording_state",
            "replay_trajectory",
            "install_ffmpeg",
        ],
    ),
    (
        "cursor",
        &[
            "set_agent_cursor_enabled",
            "set_agent_cursor_motion",
            "set_agent_cursor_theme",
            "get_agent_cursor_state",
        ],
    ),
    (
        "session",
        &[
            "start_session",
            "end_session",
            "get_session",
            "get_session_state",
            "list_sessions",
            "escalate_session",
            "history_status",
            "history_query",
        ],
    ),
    (
        "admin",
        &[
            "get_config",
            "set_config",
            "health_report",
            "check_permissions",
            "check_for_update",
        ],
    ),
    (
        "desktop",
        &[
            "get_accessibility_tree",
            "get_cursor_position",
            "get_screen_size",
        ],
    ),
];

/// What `tools/list` advertises.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolProfile {
    /// Every tool.
    Full,
    /// Only the tools of these groups (`agent` is always one of them).
    Groups(Vec<String>),
}

impl ToolProfile {
    /// The default profile: the `agent` group alone.
    pub fn agent() -> Self {
        Self::Groups(vec![AGENT_GROUP.to_owned()])
    }

    /// Parse `agent`, `full`, or a comma list of group names. An empty value
    /// is the default. Unknown names are an error so a typo does not silently
    /// hide tools.
    pub fn parse(value: &str) -> Result<Self, String> {
        let value = value.trim().to_ascii_lowercase();
        if value.is_empty() {
            return Ok(Self::agent());
        }
        let mut groups = vec![AGENT_GROUP.to_owned()];
        for part in value.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            if part == "full" || part == "all" {
                return Ok(Self::Full);
            }
            if !GROUPS.iter().any(|(name, _)| *name == part) {
                return Err(format!(
                    "unknown tool group {part:?}; use full, agent or any of: {}",
                    GROUPS
                        .iter()
                        .map(|(name, _)| *name)
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            if !groups.iter().any(|g| g == part) {
                groups.push(part.to_owned());
            }
        }
        Ok(Self::Groups(groups))
    }

    /// The profile of this process: environment, then config, then `agent`. A
    /// bad value falls back to `full` with a warning: a typo must never leave
    /// an agent with fewer tools than it asked for.
    pub fn from_env() -> Self {
        Self::resolve(
            std::env::var(TOOL_PROFILE_ENV).ok().as_deref(),
            config_string(TOOL_PROFILE_CONFIG_KEY).as_deref(),
        )
    }

    fn resolve(env: Option<&str>, config: Option<&str>) -> Self {
        let Some((source, value)) = env
            .map(|v| (TOOL_PROFILE_ENV, v))
            .or(config.map(|v| (TOOL_PROFILE_CONFIG_KEY, v)))
        else {
            return Self::agent();
        };
        Self::parse(value).unwrap_or_else(|error| {
            tracing::warn!("{source}: {error}; advertising every tool");
            Self::Full
        })
    }

    pub fn is_full(&self) -> bool {
        matches!(self, Self::Full)
    }

    /// Group a tool belongs to, or `None` for a tool no group names.
    pub fn group_of(tool: &str) -> Option<&'static str> {
        GROUPS
            .iter()
            .find(|(_, tools)| tools.contains(&tool))
            .map(|(name, _)| *name)
    }

    pub fn advertises(&self, tool: &str) -> bool {
        match (self, Self::group_of(tool)) {
            (Self::Full, _) | (_, None) => true,
            (Self::Groups(groups), Some(group)) => groups.iter().any(|g| g == group),
        }
    }

    /// Narrow a `tools/list` result in place and record what was hidden under
    /// `tool_profile`, an additive field existing clients ignore.
    pub fn apply(&self, list: &mut Value) {
        let Self::Groups(groups) = self else { return };
        let Some(tools) = list.get_mut("tools").and_then(Value::as_array_mut) else {
            return;
        };
        let mut hidden: BTreeMap<&'static str, Vec<String>> = BTreeMap::new();
        tools.retain(|tool| {
            let Some(name) = tool.get("name").and_then(Value::as_str) else {
                return true;
            };
            if self.advertises(name) {
                return true;
            }
            if let Some(group) = Self::group_of(name) {
                hidden.entry(group).or_default().push(name.to_owned());
            }
            false
        });
        list["tool_profile"] = json!({
            "groups": groups,
            "hidden": hidden,
            "enable": format!(
                "set {TOOL_PROFILE_ENV}=full for every tool, or agent,<group> for one group; \
                 hidden tools stay callable by name"
            ),
        });
    }

    /// The one line the server instructions add when this profile or the docs
    /// mode leaves something out.
    pub fn instructions_note(&self, docs: ToolDocs) -> Option<String> {
        let mut parts = Vec::new();
        if !self.is_full() {
            parts.push(format!("Full tool list: {TOOL_PROFILE_ENV}=full."));
        }
        if docs == ToolDocs::Compact {
            parts.push("Full docs per tool: `get_guide({tool})`.".to_owned());
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join(" "))
        }
    }
}

/// What a client sees for `tools/list`: the profile applied, then the docs
/// mode. Callable surface and authorization are untouched; validation of a
/// `tools/call` still runs against the full list.
pub fn client_tools_list(mut list: Value) -> Value {
    ToolProfile::from_env().apply(&mut list);
    ToolDocs::from_env().apply(&mut list);
    list
}

/// A string setting from `~/.cua-driver/config.json`, if present.
pub fn config_string(key: &str) -> Option<String> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    let path = std::path::PathBuf::from(home)
        .join(".cua-driver")
        .join("config.json");
    let value: Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    value.get(key)?.as_str().map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &Value) -> Vec<&str> {
        list["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect()
    }

    #[test]
    fn no_tool_is_in_two_groups() {
        let mut seen = std::collections::HashSet::new();
        for (_, tools) in GROUPS {
            for tool in *tools {
                assert!(seen.insert(*tool), "{tool} is in two groups");
            }
        }
    }

    #[test]
    fn agent_group_is_the_documented_34() {
        let agent = GROUPS.iter().find(|(n, _)| *n == AGENT_GROUP).unwrap().1;
        assert_eq!(agent.len(), 34);
    }

    #[test]
    fn parse_profiles() {
        assert_eq!(ToolProfile::parse("").unwrap(), ToolProfile::agent());
        assert_eq!(ToolProfile::parse("agent").unwrap(), ToolProfile::agent());
        assert_eq!(ToolProfile::parse("full").unwrap(), ToolProfile::Full);
        assert_eq!(ToolProfile::parse(" ALL ").unwrap(), ToolProfile::Full);
        assert_eq!(
            ToolProfile::parse("recording, browser").unwrap(),
            ToolProfile::Groups(vec!["agent".into(), "recording".into(), "browser".into()])
        );
        assert_eq!(
            ToolProfile::parse("agent,agent").unwrap(),
            ToolProfile::agent()
        );
        assert!(ToolProfile::parse("agent,bogus").is_err());
    }

    #[test]
    fn resolution_order_and_bad_values() {
        assert_eq!(ToolProfile::resolve(None, None), ToolProfile::agent());
        assert_eq!(ToolProfile::resolve(Some("full"), None), ToolProfile::Full);
        assert_eq!(ToolProfile::resolve(None, Some("full")), ToolProfile::Full);
        assert_eq!(
            ToolProfile::resolve(Some("agent"), Some("full")),
            ToolProfile::agent()
        );
        assert_eq!(ToolProfile::resolve(Some("bogus"), None), ToolProfile::Full);
    }

    #[test]
    fn apply_hides_only_other_groups_and_reports_them() {
        let mut list = json!({"tools": [
            {"name": "click"}, {"name": "browser_dialog"}, {"name": "set_config"}, {"name": "brand_new_tool"}
        ]});
        ToolProfile::agent().apply(&mut list);
        assert_eq!(names(&list), ["click", "brand_new_tool"]);
        assert_eq!(
            list["tool_profile"]["hidden"]["browser"][0],
            "browser_dialog"
        );
        assert_eq!(list["tool_profile"]["hidden"]["admin"][0], "set_config");
    }

    #[test]
    fn a_group_can_be_added_to_the_agent_profile() {
        let mut list = json!({"tools": [{"name": "click"}, {"name": "start_recording"}, {"name": "set_config"}]});
        ToolProfile::parse("recording").unwrap().apply(&mut list);
        assert_eq!(names(&list), ["click", "start_recording"]);
    }

    #[test]
    fn hidden_tools_are_still_known_to_the_full_list() {
        // Hiding is a view over `tools/list`; `tools/call` validates against
        // the provider's full list, which `apply` never touches.
        let full = json!({"tools": [{"name": "click"}, {"name": "start_session"}]});
        let mut view = full.clone();
        ToolProfile::agent().apply(&mut view);
        assert_eq!(names(&view), ["click"]);
        assert_eq!(names(&full), ["click", "start_session"]);
    }

    #[test]
    fn full_profile_is_a_no_op() {
        let mut list = json!({"tools": [{"name": "browser_dialog"}]});
        let before = list.clone();
        ToolProfile::Full.apply(&mut list);
        assert_eq!(list, before);
    }

    #[test]
    fn instructions_note_tells_how_to_get_more() {
        let note = ToolProfile::agent()
            .instructions_note(ToolDocs::Compact)
            .unwrap();
        assert!(note.contains("CUA_DRIVER_TOOL_PROFILE=full"));
        assert!(note.contains("get_guide({tool})"));
        assert!(ToolProfile::Full
            .instructions_note(ToolDocs::Full)
            .is_none());
    }
}
