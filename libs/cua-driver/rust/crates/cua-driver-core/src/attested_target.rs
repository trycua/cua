// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Driver-attested desktop target identity for permission policy.
//!
//! A policy that limits input or capture to one application cannot trust the
//! `pid` or `window_id` a caller declares: those are numbers the caller chose.
//! For the tools listed here the dispatch boundary resolves the call's target
//! from operating-system state (window ownership and the owning process
//! instance), hands that descriptor to policy separately from the caller's
//! arguments, and re-attests it immediately before the platform tool runs.
//! Any difference refuses the call.
//!
//! Application names and bundle ids are deliberately absent: `list_apps`
//! pairs running processes with installed apps by executable or command-line
//! basename on Linux and Windows, which is discovery, not identity. The
//! executable path is what each OS reports for the exact pid.

use serde::Serialize;
use serde_json::Value;

/// Input and capture tools whose call names one desktop target.
pub const ATTESTED_TARGET_TOOLS: &[&str] = &[
    "click",
    "double_click",
    "right_click",
    "drag",
    "scroll",
    "move_cursor",
    "mouse_button_down",
    "mouse_button_up",
    "mouse_drag",
    "type_text",
    "press_key",
    "hotkey",
    "set_value",
    "bring_to_front",
    "set_window_frame",
    "get_window_state",
    "get_accessibility_tree",
    "get_desktop_state",
    "zoom",
];

/// Descriptor fields a policy can constrain.
pub const ATTESTED_TARGET_FIELDS: &[&str] = &[
    "kind",
    "pid",
    "window_id",
    "executable",
    "process_start_time",
];

pub fn supports_attested_target(tool: &str) -> bool {
    ATTESTED_TARGET_TOOLS.contains(&tool)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AttestedTargetKind {
    /// One window, whose owner the driver verified.
    Window,
    /// A process named by pid alone; the platform picks its window later.
    Application,
    /// The whole desktop. There is no application identity.
    Desktop,
}

/// What the driver itself resolved for a call's target. Fields the platform
/// could not attest are absent, never guessed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AttestedTarget {
    pub kind: AttestedTargetKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub executable: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub process_start_time: Option<u64>,
}

impl AttestedTarget {
    pub fn desktop() -> Self {
        Self {
            kind: AttestedTargetKind::Desktop,
            pid: None,
            window_id: None,
            executable: None,
            process_start_time: None,
        }
    }

    pub fn to_policy_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

/// The pid and window a call names, before the driver verifies them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeclaredTarget {
    pub pid: Option<i64>,
    pub window_id: Option<u64>,
}

impl DeclaredTarget {
    /// Read the declared target from normalized call arguments, the way
    /// platform adapters read them. A desktop-frame call without a pid goes
    /// to the focused application whatever else it names, so it is a desktop
    /// target. A window must come with its pid, since adapters differ on a
    /// bare `window_id`. A call that names only an `element_token` takes its
    /// pid and window from `token_window`, the snapshot the token belongs to.
    pub fn from_args(args: &Value, token_window: Option<(i32, u64)>) -> Result<Self, String> {
        let pid = args.get("pid").and_then(Value::as_i64);
        let window_id = args.get("window_id").and_then(Value::as_u64);
        let desktop_frame = args.get("scope").and_then(Value::as_str) == Some("desktop")
            || args.get("coordinate_frame").and_then(Value::as_str) == Some("desktop");
        if pid.is_none() && desktop_frame {
            return Ok(Self {
                pid: None,
                window_id: None,
            });
        }
        if pid.is_some() {
            return Ok(Self { pid, window_id });
        }
        if window_id.is_some() {
            return Err("a window_id target must also name the window's pid".to_owned());
        }
        if args
            .get("element_token")
            .is_some_and(|value| !value.is_null())
        {
            let (pid, window_id) = token_window.ok_or_else(|| {
                "the element_token does not name a live snapshot window".to_owned()
            })?;
            return Ok(Self {
                pid: Some(i64::from(pid)),
                window_id: Some(window_id),
            });
        }
        Ok(Self {
            pid: None,
            window_id: None,
        })
    }
}

/// The owner `list_windows` reports for `window_id`.
pub fn listed_window_owner(windows: &Value, window_id: u64) -> Option<i64> {
    windows
        .get("windows")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|window| window.get("window_id").and_then(Value::as_u64) == Some(window_id))
        .and_then(|window| window.get("pid").and_then(Value::as_i64))
}

/// Accept `owner` as the target's pid only when it agrees with what the
/// caller declared.
pub fn check_window_owner(
    window_id: u64,
    owner: Option<i64>,
    declared_pid: Option<i64>,
) -> Result<i64, String> {
    match (owner, declared_pid) {
        (Some(owner), Some(pid)) if owner != pid => Err(format!(
            "window {window_id} belongs to pid {owner}, not pid {pid}"
        )),
        (Some(owner), _) => Ok(owner),
        (None, Some(pid)) => Err(format!(
            "window {window_id} is not an open window of pid {pid}"
        )),
        (None, None) => Err(format!("window {window_id} is not open")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn declared_target_prefers_explicit_fields_and_falls_back_to_the_token_window() {
        assert_eq!(
            DeclaredTarget::from_args(&json!({"pid": 7, "window_id": 9}), Some((1, 2))).unwrap(),
            DeclaredTarget {
                pid: Some(7),
                window_id: Some(9)
            }
        );
        assert_eq!(
            DeclaredTarget::from_args(&json!({"element_token": "s:1"}), Some((1, 2))).unwrap(),
            DeclaredTarget {
                pid: Some(1),
                window_id: Some(2)
            }
        );
        assert!(DeclaredTarget::from_args(&json!({"element_token": "s:1"}), None).is_err());
        assert_eq!(
            DeclaredTarget::from_args(&json!({"window_id": 9}), None).unwrap_err(),
            "a window_id target must also name the window's pid"
        );
        // Adapters send a pid-less desktop-frame call to the focused app.
        for args in [
            json!({"window_id": 9, "scope": "desktop"}),
            json!({"window_id": 9, "coordinate_frame": "desktop"}),
            json!({"element_token": "s:1", "scope": "desktop"}),
        ] {
            assert_eq!(
                DeclaredTarget::from_args(&args, Some((1, 2))).unwrap(),
                DeclaredTarget {
                    pid: None,
                    window_id: None
                },
                "{args}"
            );
        }
        assert_eq!(
            DeclaredTarget::from_args(&json!({"x": 1, "y": 2}), None).unwrap(),
            DeclaredTarget {
                pid: None,
                window_id: None
            }
        );
    }

    #[test]
    fn window_owner_refuses_a_missing_or_foreign_window() {
        let windows = json!({"windows": [{"window_id": 9, "pid": 7}]});
        assert_eq!(listed_window_owner(&windows, 9), Some(7));
        assert_eq!(listed_window_owner(&windows, 10), None);
        assert_eq!(check_window_owner(9, Some(7), Some(7)).unwrap(), 7);
        assert_eq!(check_window_owner(9, Some(7), None).unwrap(), 7);
        assert_eq!(
            check_window_owner(9, Some(7), Some(8)).unwrap_err(),
            "window 9 belongs to pid 7, not pid 8"
        );
        assert_eq!(
            check_window_owner(10, None, Some(7)).unwrap_err(),
            "window 10 is not an open window of pid 7"
        );
        assert_eq!(
            check_window_owner(10, None, None).unwrap_err(),
            "window 10 is not open"
        );
    }

    #[test]
    fn unattested_fields_are_absent_from_the_policy_value() {
        assert_eq!(
            AttestedTarget::desktop().to_policy_value(),
            json!({"kind": "desktop"})
        );
    }
}
