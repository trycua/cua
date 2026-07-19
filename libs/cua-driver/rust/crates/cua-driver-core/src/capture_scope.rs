//! Per-session capture/action scope policy.
//!
//! The policy is keyed exclusively by the public, caller-declared `session`
//! argument. Reserved transport mirrors such as `_session_id` must never mint
//! policy state: doing so would let an anonymous proxy connection bypass the
//! per-session contract.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fmt;
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureScopePolicy {
    #[default]
    Auto,
    Window,
    Desktop,
}

impl CaptureScopePolicy {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(Self::Auto),
            "window" => Some(Self::Window),
            "desktop" => Some(Self::Desktop),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Window => "window",
            Self::Desktop => "desktop",
        }
    }
}

impl fmt::Display for CaptureScopePolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectiveCaptureScope {
    Window,
    Desktop,
}

impl EffectiveCaptureScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Window => "window",
            Self::Desktop => "desktop",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EscalationReason {
    AxTreePixelMismatch,
    BackgroundDeliveryFailed,
    ForegroundIneffective,
    NoWindowTarget,
    Other,
}

impl EscalationReason {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "ax_tree_pixel_mismatch" => Some(Self::AxTreePixelMismatch),
            "background_delivery_failed" => Some(Self::BackgroundDeliveryFailed),
            "foreground_ineffective" => Some(Self::ForegroundIneffective),
            "no_window_target" => Some(Self::NoWindowTarget),
            "other" => Some(Self::Other),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::AxTreePixelMismatch => "ax_tree_pixel_mismatch",
            Self::BackgroundDeliveryFailed => "background_delivery_failed",
            Self::ForegroundIneffective => "foreground_ineffective",
            Self::NoWindowTarget => "no_window_target",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCaptureScope {
    pub policy: CaptureScopePolicy,
    pub desktop_unlocked: bool,
    pub escalation_reason: Option<EscalationReason>,
    pub escalation_detail: Option<String>,
}

impl SessionCaptureScope {
    pub fn new(policy: CaptureScopePolicy) -> Self {
        Self {
            policy,
            desktop_unlocked: policy == CaptureScopePolicy::Desktop,
            escalation_reason: None,
            escalation_detail: None,
        }
    }

    pub fn effective_scope(&self) -> EffectiveCaptureScope {
        match self.policy {
            CaptureScopePolicy::Desktop => EffectiveCaptureScope::Desktop,
            CaptureScopePolicy::Window => EffectiveCaptureScope::Window,
            CaptureScopePolicy::Auto if self.desktop_unlocked => EffectiveCaptureScope::Desktop,
            CaptureScopePolicy::Auto => EffectiveCaptureScope::Window,
        }
    }

    pub fn as_json(&self, session: &str) -> Value {
        json!({
            "session": session,
            "capture_scope": self.policy.as_str(),
            "effective_scope": self.effective_scope().as_str(),
            "desktop_unlocked": self.desktop_unlocked,
            "escalation_reason": self.escalation_reason.map(EscalationReason::as_str),
            "escalation_detail": self.escalation_detail,
        })
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EscalateError {
    Ended,
    NotStarted,
    WindowStrict,
    DesktopAlreadyActive,
}

pub fn escalate_session(
    session: &str,
    reason: EscalationReason,
    detail: Option<&str>,
) -> Result<SessionCaptureScope, EscalateError> {
    let mut registry = scopes().lock().unwrap();
    // This check deliberately happens while holding the scope lock. If an end
    // races with escalation, either this write wins first and end removes it,
    // or the tombstone wins first and this write is rejected. A late action
    // can therefore never resurrect an unlocked session.
    if crate::session::is_session_ended(session) {
        return Err(EscalateError::Ended);
    }
    let Some(state) = registry.get_mut(session) else {
        return Err(EscalateError::NotStarted);
    };
    if state.desktop_unlocked {
        return Err(EscalateError::DesktopAlreadyActive);
    }
    match state.policy {
        CaptureScopePolicy::Window => return Err(EscalateError::WindowStrict),
        CaptureScopePolicy::Desktop => return Err(EscalateError::DesktopAlreadyActive),
        CaptureScopePolicy::Auto => {}
    }
    state.desktop_unlocked = true;
    state.escalation_reason = Some(reason);
    state.escalation_detail = detail.map(|value| value.chars().take(200).collect());
    Ok(state.clone())
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
            | "get_accessibility_tree"
            | "get_app_state"
            | "screenshot"
            | "screenshot_compat"
            | "zoom"
            | "page"
    ) || tool_name == "get_browser_state"
        || (tool_name.starts_with("browser_")
            && !matches!(tool_name, "browser_prepare" | "browser_download"))
    {
        return ToolScope::Window;
    }
    if is_scoped_action(tool_name) {
        let explicit_desktop = args.get("scope").and_then(Value::as_str) == Some("desktop");
        let has_window_target = args.get("pid").is_some()
            || args.get("window_id").is_some()
            || args.get("element_index").is_some()
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

/// Enforce the modality for a public session. A first non-lifecycle action
/// implicitly starts that session in `auto`, matching the existing cursor and
/// recording lifecycle behavior.
pub fn enforce_tool(tool_name: &str, args: &Value) -> Result<(), ScopeViolation> {
    if matches!(
        tool_name,
        "start_session" | "end_session" | "escalate_session" | "get_session_state"
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
    let (state, _) = bind_session(session, None).map_err(|_| ScopeViolation {
        code: "session_ended",
        message: format!("session '{session}' has ended"),
        state: SessionCaptureScope::new(CaptureScopePolicy::Auto),
    })?;
    match (state.effective_scope(), tool_scope(tool_name, args)) {
        (_, ToolScope::Unscoped)
        | (EffectiveCaptureScope::Window, ToolScope::Window)
        | (EffectiveCaptureScope::Desktop, ToolScope::Desktop) => Ok(()),
        (EffectiveCaptureScope::Window, ToolScope::Desktop)
            if state.policy == CaptureScopePolicy::Auto =>
        {
            Err(ScopeViolation {
                code: "desktop_escalation_required",
                message: format!(
                    "desktop-scope tool '{tool_name}' is locked for auto session '{session}'; exhaust the window action ladder, verify each attempt, then call escalate_session"
                ),
                state,
            })
        }
        (EffectiveCaptureScope::Window, ToolScope::Desktop) => Err(ScopeViolation {
            code: "desktop_scope_disabled",
            message: format!(
                "desktop-scope tool '{tool_name}' is disabled for strict window session '{session}'"
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

    fn fresh(prefix: &str) -> String {
        format!("capture-scope-{prefix}-{}", std::process::id())
    }

    #[test]
    fn sessions_are_isolated_and_auto_escalates_one_way() {
        let auto = fresh("auto");
        let desktop = fresh("desktop");
        let window = fresh("window");
        bind_session(&auto, Some(CaptureScopePolicy::Auto)).unwrap();
        bind_session(&desktop, Some(CaptureScopePolicy::Desktop)).unwrap();
        bind_session(&window, Some(CaptureScopePolicy::Window)).unwrap();

        assert_eq!(
            enforce_tool("get_desktop_state", &json!({"session": auto}))
                .unwrap_err()
                .code,
            "desktop_escalation_required"
        );
        assert!(enforce_tool("get_desktop_state", &json!({"session": desktop})).is_ok());
        assert_eq!(
            enforce_tool("get_desktop_state", &json!({"session": window}))
                .unwrap_err()
                .code,
            "desktop_scope_disabled"
        );

        escalate_session(&auto, EscalationReason::ForegroundIneffective, None).unwrap();
        assert!(enforce_tool("get_desktop_state", &json!({"session": auto})).is_ok());
        assert_eq!(
            enforce_tool("get_window_state", &json!({"session": auto, "pid": 1}))
                .unwrap_err()
                .code,
            "window_scope_disabled"
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
    fn escalation_detail_is_bounded() {
        let session = fresh("detail");
        bind_session(&session, Some(CaptureScopePolicy::Auto)).unwrap();
        let state =
            escalate_session(&session, EscalationReason::Other, Some(&"x".repeat(500))).unwrap();
        assert_eq!(state.escalation_detail.unwrap().chars().count(), 200);
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
    fn end_racing_escalation_never_resurrects_scope_state() {
        use std::sync::{Arc, Barrier};

        let session = fresh("end-escalate-race");
        bind_session(&session, Some(CaptureScopePolicy::Auto)).unwrap();
        let barrier = Arc::new(Barrier::new(3));
        let escalation_session = session.clone();
        let escalation_barrier = barrier.clone();
        let escalation = std::thread::spawn(move || {
            escalation_barrier.wait();
            escalate_session(
                &escalation_session,
                EscalationReason::ForegroundIneffective,
                None,
            )
        });
        let end_session = session.clone();
        let end_barrier = barrier.clone();
        let end = std::thread::spawn(move || {
            end_barrier.wait();
            crate::session::fire_session_end(&end_session)
        });
        barrier.wait();
        let _ = escalation.join().unwrap();
        assert!(end.join().unwrap());

        assert!(crate::session::is_session_ended(&session));
        assert!(get_session(&session).is_none());
        assert_eq!(
            enforce_tool("get_window_state", &json!({"session": session}))
                .unwrap_err()
                .code,
            "session_ended"
        );

        assert!(crate::session::revive_session(&session));
        let (fresh_state, created) =
            bind_session(&session, Some(CaptureScopePolicy::Window)).unwrap();
        assert!(created);
        assert_eq!(fresh_state.policy, CaptureScopePolicy::Window);
        assert!(!fresh_state.desktop_unlocked);
    }
}
