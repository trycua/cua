// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Tool profiles: which tools `tools/list` advertises to an MCP client.
//!
//! A client that loads every advertised schema pays for all of them on every
//! session, and a model choosing among 60 tools picks the wrong one more
//! often than among 25. A profile narrows only what is **advertised**:
//!
//! * `full` (default today) advertises every tool, exactly as before.
//! * `core` advertises the desktop loop that bench trajectories use.
//! * `standard` is `core` plus the typed browser surface.
//! * A comma list (`core,recording`) adds named groups to `core`.
//!
//! Hidden tools stay registered and callable by name, and still pass through
//! the same authorization, so an older client, skill or script that calls
//! `browser_click` directly keeps working. Nothing is proxied or re-routed.
//! The operator picks the profile with `CUA_DRIVER_TOOL_PROFILE`.

use std::collections::BTreeMap;

use serde_json::{json, Value};

/// Environment variable that selects the profile for an MCP server process.
pub const TOOL_PROFILE_ENV: &str = "CUA_DRIVER_TOOL_PROFILE";

/// Tool groups, in the order they are reported. `core` is always advertised
/// unless the profile is `full`.
pub const GROUPS: &[(&str, &[&str])] = &[
    (
        "core",
        &[
            "list_apps",
            "list_windows",
            "launch_app",
            "bring_to_front",
            "get_window_state",
            "get_desktop_state",
            "zoom",
            "parse_visual_regions",
            "verify_state",
            "click",
            "double_click",
            "set_value",
            "type_text",
            "press_key",
            "hotkey",
            "scroll",
            "drag",
            "invoke_menu",
            "set_window_frame",
            "clipboard_read",
            "clipboard_write",
            "run_steps",
            "run_script",
            "get_guide",
        ],
    ),
    (
        "browser",
        &[
            "get_browser_state",
            "browser_prepare",
            "browser_navigate",
            "browser_click",
            "browser_type",
            "browser_pointer",
            "browser_select",
            "browser_dialog",
            "browser_download",
            "browser_set_input_files",
            "page",
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
            "install_extension",
        ],
    ),
    (
        "input_extra",
        &[
            "right_click",
            "move_cursor",
            "kill_app",
            "get_cursor_position",
            "get_screen_size",
            "get_accessibility_tree",
        ],
    ),
];

/// What `tools/list` advertises.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolProfile {
    /// Every tool.
    Full,
    /// Only the tools of these groups (`core` is always one of them).
    Groups(Vec<String>),
}

impl ToolProfile {
    /// Parse `full`, `core`, `standard`, or a comma list of group names.
    /// Unknown names are an error so a typo does not silently hide tools.
    pub fn parse(value: &str) -> Result<Self, String> {
        let value = value.trim().to_ascii_lowercase();
        if value.is_empty() || value == "full" || value == "all" {
            return Ok(Self::Full);
        }
        let mut groups = vec!["core".to_owned()];
        for part in value.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let group = match part {
                "full" | "all" => return Ok(Self::Full),
                "standard" => "browser",
                other if GROUPS.iter().any(|(name, _)| *name == other) => other,
                other => {
                    return Err(format!(
                        "unknown tool group {other:?}; use full, core, standard or any of: {}",
                        GROUPS
                            .iter()
                            .map(|(name, _)| *name)
                            .collect::<Vec<_>>()
                            .join(", ")
                    ))
                }
            };
            if !groups.iter().any(|g| g == group) {
                groups.push(group.to_owned());
            }
        }
        Ok(Self::Groups(groups))
    }

    /// The profile of this process. A bad value falls back to `full` with a
    /// warning: a typo must never leave an agent with no tools.
    pub fn from_env() -> Self {
        match std::env::var(TOOL_PROFILE_ENV) {
            Ok(value) => Self::parse(&value).unwrap_or_else(|error| {
                tracing::warn!("{TOOL_PROFILE_ENV}: {error}; advertising every tool");
                Self::Full
            }),
            Err(_) => Self::Full,
        }
    }

    pub fn is_full(&self) -> bool {
        matches!(self, Self::Full)
    }

    /// Group a tool belongs to, or `None` for a tool no group names. Tools
    /// without a group are always advertised, so a new tool is never hidden
    /// by accident.
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
        if self.is_full() {
            return;
        }
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
        let Self::Groups(groups) = self else { return };
        list["tool_profile"] = json!({
            "groups": groups,
            "hidden": hidden,
            "enable": format!("set {TOOL_PROFILE_ENV}=core,<group> or full"),
        });
    }

    /// One line for the server instructions when tools are hidden.
    pub fn instructions_note(&self) -> Option<String> {
        let Self::Groups(groups) = self else {
            return None;
        };
        let hidden: Vec<&str> = GROUPS
            .iter()
            .map(|(name, _)| *name)
            .filter(|name| !groups.iter().any(|g| g == name))
            .collect();
        if hidden.is_empty() {
            return None;
        }
        Some(format!(
            "Tool profile: {} are advertised. Not advertised: {}. The user enables them with {TOOL_PROFILE_ENV}.",
            groups.join("+"),
            hidden.join(", ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn parse_profiles() {
        assert_eq!(ToolProfile::parse("").unwrap(), ToolProfile::Full);
        assert_eq!(ToolProfile::parse("full").unwrap(), ToolProfile::Full);
        assert_eq!(
            ToolProfile::parse("core").unwrap(),
            ToolProfile::Groups(vec!["core".into()])
        );
        assert_eq!(
            ToolProfile::parse("standard").unwrap(),
            ToolProfile::Groups(vec!["core".into(), "browser".into()])
        );
        assert_eq!(
            ToolProfile::parse("core, recording").unwrap(),
            ToolProfile::Groups(vec!["core".into(), "recording".into()])
        );
        assert!(ToolProfile::parse("core,bogus").is_err());
    }

    #[test]
    fn apply_hides_only_other_groups_and_reports_them() {
        let mut list = json!({"tools": [
            {"name": "click"}, {"name": "browser_click"}, {"name": "set_config"}, {"name": "brand_new_tool"}
        ]});
        ToolProfile::parse("core").unwrap().apply(&mut list);
        let names: Vec<&str> = list["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["click", "brand_new_tool"]);
        assert_eq!(
            list["tool_profile"]["hidden"]["browser"][0],
            "browser_click"
        );
        assert_eq!(list["tool_profile"]["hidden"]["admin"][0], "set_config");
    }

    #[test]
    fn full_profile_is_a_no_op() {
        let mut list = json!({"tools": [{"name": "browser_click"}]});
        let before = list.clone();
        ToolProfile::Full.apply(&mut list);
        assert_eq!(list, before);
    }
}
