// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Per-session capture/action scope policy.
//!
//! The policy is keyed exclusively by the public, caller-declared `session`
//! argument. Reserved transport mirrors such as `_session_id` must never mint
//! policy state: doing so would let an anonymous proxy connection bypass the
//! per-session contract.

use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

pub use cua_driver_contract::{
    CaptureScope as CaptureScopePolicy, EffectiveScope as EffectiveCaptureScope,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCaptureScope {
    pub policy: CaptureScopePolicy,
    pub desktop_unlocked: bool,
}

impl SessionCaptureScope {
    pub fn new(policy: CaptureScopePolicy) -> Self {
        Self {
            policy,
            desktop_unlocked: policy == CaptureScopePolicy::Desktop,
        }
    }

    pub fn effective_scope(&self) -> EffectiveCaptureScope {
        match self.policy {
            CaptureScopePolicy::Desktop => EffectiveCaptureScope::Desktop,
            CaptureScopePolicy::Window => EffectiveCaptureScope::Window,
            CaptureScopePolicy::Auto => EffectiveCaptureScope::Window,
        }
    }

    pub fn as_json(&self, session: &str) -> Value {
        serde_json::to_value(self.output(session)).expect("session output serializes")
    }

    pub fn output(&self, session: &str) -> cua_driver_contract::SessionStateOutput {
        cua_driver_contract::SessionStateOutput {
            session: session.to_owned(),
            capture_scope: self.policy,
            effective_scope: self.effective_scope(),
            desktop_capture_authorized: self.desktop_unlocked,
            desktop_unlocked: self.desktop_unlocked,
        }
    }
}

static SESSION_SCOPES: OnceLock<Mutex<HashMap<String, SessionCaptureScope>>> = OnceLock::new();

fn scopes() -> &'static Mutex<HashMap<String, SessionCaptureScope>> {
    SESSION_SCOPES.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindError {
    Ended,
    Conflict {
        existing: CaptureScopePolicy,
        requested: CaptureScopePolicy,
    },
}

/// Bind a policy to a live session. Omitted policy means "keep the existing
/// policy, or use auto for a new session" so repeated `start_session` calls
/// remain idempotent.
pub fn bind_session(
    session: &str,
    requested: Option<CaptureScopePolicy>,
) -> Result<(SessionCaptureScope, bool), BindError> {
    let mut registry = scopes().lock().unwrap();
    if crate::session::is_session_ended(session) {
        return Err(BindError::Ended);
    }
    if let Some(existing) = registry.get(session) {
        if let Some(requested) = requested {
            if existing.policy != requested {
                return Err(BindError::Conflict {
                    existing: existing.policy,
                    requested,
                });
            }
        }
        return Ok((existing.clone(), false));
    }
    let state = SessionCaptureScope::new(requested.unwrap_or_default());
    registry.insert(session.to_owned(), state.clone());
    Ok((state, true))
}

pub fn get_session(session: &str) -> Option<SessionCaptureScope> {
    scopes().lock().unwrap().get(session).cloned()
}

pub fn clear_session(session: &str) {
    scopes().lock().unwrap().remove(session);
}

pub fn clear_sessions_with_prefix(prefix: &str) -> usize {
    let mut registry = scopes().lock().unwrap();
    let before = registry.len();
    registry.retain(|session, _| !session.starts_with(prefix));
    before - registry.len()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolScope {
    Window,
    Desktop,
    Unscoped,
}

fn is_scoped_action(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "click"
            | "double_click"
            | "right_click"
            | "scroll"
            | "drag"
            | "mouse_drag"
            | "parallel_mouse_drag"
            | "move_cursor"
            | "type_text"
            | "type_text_chars"
            | "press_key"
            | "hotkey"
            | "set_value"
    )
}

fn tool_scope(tool_name: &str, args: &Value) -> ToolScope {
    if matches!(
        tool_name,
        "get_desktop_state" | "get_screen_size" | "get_cursor_position"
    ) {
        return ToolScope::Desktop;
    }
    if matches!(
        tool_name,
        "get_window_state"
            | "verify_state"
            | "get_accessibility_tree"
            | "get_app_state"
            | "zoom"
            | "page"
    ) || tool_name == "get_browser_state"
        || (tool_name.starts_with("browser_")
            && !matches!(tool_name, "browser_prepare" | "browser_download"))
    {
        return ToolScope::Window;
    }
    if matches!(tool_name, "mouse_button_down" | "mouse_button_up") {
        // These stateful primitives only deliver to a window or to the window
        // retained by a prior button-down; neither implements desktop scope.
        return ToolScope::Window;
    }
    if is_scoped_action(tool_name) {
        let explicit_desktop = args.get("scope").and_then(Value::as_str) == Some("desktop");
        let has_window_target = args.get("pid").is_some()
            || args.get("window_id").is_some()
            || args.get("element_token").is_some();
        if explicit_desktop && !has_window_target {
            ToolScope::Desktop
        } else {
            ToolScope::Window
        }
    } else {
        ToolScope::Unscoped
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeViolation {
    pub code: &'static str,
    pub message: String,
    pub state: SessionCaptureScope,
}

impl ScopeViolation {
    pub fn as_json(&self, session: &str) -> Value {
        let mut value = self.state.as_json(session);
        if let Some(object) = value.as_object_mut() {
            object.insert("code".to_owned(), Value::String(self.code.to_owned()));
        }
        value
    }
}

/// Enforce modality only for a session that explicitly opted into the
/// deprecated capture-scope compatibility contract. Lifecycle-only explicit
/// and implicit sessions select modality per call and carry no capture state.
pub fn enforce_tool(tool_name: &str, args: &Value) -> Result<(), ScopeViolation> {
    if matches!(
        tool_name,
        "start_session" | "end_session" | "get_session" | "list_sessions"
    ) {
        return Ok(());
    }
    let Some(session) = args
        .get("session")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && *value != "default")
    else {
        return Ok(());
    };
    let Some(state) = get_session(session) else {
        return Ok(());
    };
    match (state.effective_scope(), tool_scope(tool_name, args)) {
        (_, ToolScope::Unscoped)
        | (EffectiveCaptureScope::Window, ToolScope::Window)
        | (EffectiveCaptureScope::Desktop, ToolScope::Desktop) => Ok(()),
        (EffectiveCaptureScope::Window, ToolScope::Desktop) => Err(ScopeViolation {
            code: "desktop_scope_disabled",
            message: format!(
                "desktop-scope tool '{tool_name}' is disabled for window-scoped session '{session}'; pass an explicit per-call target, or start a session with capture_scope=desktop"
            ),
            state,
        }),
        (EffectiveCaptureScope::Desktop, ToolScope::Window) => Err(ScopeViolation {
            code: "window_scope_disabled",
            message: format!(
                "window-scope tool '{tool_name}' is disabled while session '{session}' is in desktop scope"
            ),
            state,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fresh(prefix: &str) -> String {
        format!("capture-scope-{prefix}-{}", std::process::id())
    }

    #[test]
    fn sessions_are_isolated_and_auto_is_window_scoped() {
        let auto = fresh("auto");
        let desktop = fresh("desktop");
        let window = fresh("window");
        bind_session(&auto, Some(CaptureScopePolicy::Auto)).unwrap();
        bind_session(&desktop, Some(CaptureScopePolicy::Desktop)).unwrap();
        bind_session(&window, Some(CaptureScopePolicy::Window)).unwrap();

        for session in [&auto, &window] {
            assert_eq!(
                enforce_tool("get_desktop_state", &json!({"session": session}))
                    .unwrap_err()
                    .code,
                "desktop_scope_disabled"
            );
            assert!(
                enforce_tool("get_window_state", &json!({"session": session, "pid": 1})).is_ok()
            );
        }
        assert!(enforce_tool("get_desktop_state", &json!({"session": desktop})).is_ok());
        assert_eq!(
            enforce_tool("get_window_state", &json!({"session": desktop, "pid": 1}))
                .unwrap_err()
                .code,
            "window_scope_disabled"
        );
        assert_eq!(
            get_session(&auto).unwrap().effective_scope(),
            EffectiveCaptureScope::Window
        );
        assert_eq!(
            get_session(&desktop).unwrap().effective_scope(),
            EffectiveCaptureScope::Desktop
        );
    }

    #[test]
    fn conflicting_live_rebind_is_rejected() {
        let session = fresh("conflict");
        bind_session(&session, Some(CaptureScopePolicy::Window)).unwrap();
        assert_eq!(
            bind_session(&session, Some(CaptureScopePolicy::Desktop)),
            Err(BindError::Conflict {
                existing: CaptureScopePolicy::Window,
                requested: CaptureScopePolicy::Desktop,
            })
        );
    }

    #[test]
    fn transport_mirror_does_not_create_policy() {
        let session = fresh("mirror");
        assert!(enforce_tool("get_desktop_state", &json!({"_session_id": session})).is_ok());
        assert!(get_session(&session).is_none());
    }

    #[test]
    fn strict_modes_apply_symmetrically_to_perception_and_actions() {
        let window = fresh("strict-window-actions");
        let desktop = fresh("strict-desktop-actions");
        bind_session(&window, Some(CaptureScopePolicy::Window)).unwrap();
        bind_session(&desktop, Some(CaptureScopePolicy::Desktop)).unwrap();

        assert_eq!(
            enforce_tool(
                "click",
                &json!({"session": window, "scope": "desktop", "x": 4, "y": 5})
            )
            .unwrap_err()
            .code,
            "desktop_scope_disabled"
        );
        assert_eq!(
            enforce_tool("get_cursor_position", &json!({"session": window}))
                .unwrap_err()
                .code,
            "desktop_scope_disabled"
        );
        assert!(enforce_tool("get_screen_size", &json!({"session": desktop})).is_ok());
        assert_eq!(
            enforce_tool(
                "click",
                &json!({"session": desktop, "pid": 42, "x": 4, "y": 5})
            )
            .unwrap_err()
            .code,
            "window_scope_disabled"
        );
    }

    #[test]
    fn mouse_button_actions_respect_strict_scope() {
        let window = fresh("strict-window-mouse-buttons");
        let desktop = fresh("strict-desktop-mouse-buttons");
        bind_session(&window, Some(CaptureScopePolicy::Window)).unwrap();
        bind_session(&desktop, Some(CaptureScopePolicy::Desktop)).unwrap();

        for tool_name in ["mouse_button_down", "mouse_button_up"] {
            assert!(enforce_tool(
                tool_name,
                &json!({"session": window, "pid": 42, "window_id": 7, "x": 4, "y": 5})
            )
            .is_ok());
            assert_eq!(
                enforce_tool(
                    tool_name,
                    &json!({"session": desktop, "pid": 42, "window_id": 7, "x": 4, "y": 5})
                )
                .unwrap_err()
                .code,
                "window_scope_disabled"
            );
        }
        assert_eq!(
            enforce_tool(
                "mouse_button_up",
                &json!({"session": desktop, "scope": "desktop"})
            )
            .unwrap_err()
            .code,
            "window_scope_disabled"
        );
    }

    #[test]
    fn ending_a_session_clears_its_scope_state() {
        let session = fresh("end-clears-scope");
        bind_session(&session, Some(CaptureScopePolicy::Auto)).unwrap();
        assert!(crate::session::fire_session_end(&session));

        assert!(crate::session::is_session_ended(&session));
        assert!(get_session(&session).is_none());
        assert!(
            enforce_tool("get_window_state", &json!({"session": session})).is_ok(),
            "the compatibility helper must not recreate cleared capture state; the canonical lifecycle gate rejects the ended session"
        );

        assert!(crate::session::revive_session(&session));
        let (fresh_state, created) =
            bind_session(&session, Some(CaptureScopePolicy::Window)).unwrap();
        assert!(created);
        assert_eq!(fresh_state.policy, CaptureScopePolicy::Window);
        assert!(!fresh_state.desktop_unlocked);
    }
}
