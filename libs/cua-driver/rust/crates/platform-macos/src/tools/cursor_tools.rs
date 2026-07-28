//! Session-owned agent cursor controls.

use async_trait::async_trait;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use serde_json::Value;
use std::sync::Arc;

use super::ToolState;

/// Resolve the cursor identity from the current session. The removed
/// `cursor_id` alias is intentionally not accepted by the new contract.
pub(crate) fn resolve_cursor_key(args: &Value) -> String {
    use cua_driver_core::tool_args::ArgsExt;
    for key in ["session", "_session_id"] {
        if let Some(value) = args.opt_str(key) {
            if !value.is_empty() {
                return value;
            }
        }
    }
    default_cursor_session()
}

fn default_cursor_session() -> String {
    static ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    ID.get_or_init(|| format!("auto-{:x}", std::process::id()))
        .clone()
}

pub struct SetAgentCursorEnabledTool {
    state: Arc<ToolState>,
}

impl SetAgentCursorEnabledTool {
    pub fn new(state: Arc<ToolState>) -> Self {
        Self { state }
    }
}

static ENABLED_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn enabled_def() -> &'static ToolDef {
    ENABLED_DEF.get_or_init(|| canonical_cursor_def("set_agent_cursor_enabled"))
}

#[async_trait]
impl Tool for SetAgentCursorEnabledTool {
    fn def(&self) -> &ToolDef {
        enabled_def()
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        if !self.state.cursor_overlay_available {
            return super::cursor_overlay_unavailable();
        }
        use cua_driver_core::tool_args::ArgsExt;
        let enabled = match args.require_bool("enabled") {
            Ok(value) => value,
            Err(error) => return error,
        };
        let session = resolve_cursor_key(&args);
        self.state.cursor_registry.set_enabled(&session, enabled);
        crate::cursor::overlay::send_command(
            session.clone(),
            cursor_overlay::OverlayCommand::SetEnabled(enabled),
        );
        ToolResult::text(format!(
            "Agent cursor for session '{session}' {}.",
            if enabled { "enabled" } else { "disabled" }
        ))
        .with_structured(serde_json::json!({
            "session": session,
            "enabled": enabled
        }))
    }
}

pub struct SetAgentCursorMotionTool {
    state: Arc<ToolState>,
}

impl SetAgentCursorMotionTool {
    pub fn new(state: Arc<ToolState>) -> Self {
        Self { state }
    }
}

static MOTION_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn motion_def() -> &'static ToolDef {
    MOTION_DEF.get_or_init(|| canonical_cursor_def("set_agent_cursor_motion"))
}

fn number(value: Option<&Value>) -> Option<f64> {
    value.and_then(|value| {
        value
            .as_f64()
            .or_else(|| value.as_i64().map(|integer| integer as f64))
    })
}

#[async_trait]
impl Tool for SetAgentCursorMotionTool {
    fn def(&self) -> &ToolDef {
        motion_def()
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        if !self.state.cursor_overlay_available {
            return super::cursor_overlay_unavailable();
        }
        let session = resolve_cursor_key(&args);
        let current = crate::cursor::overlay::current_motion(&session);
        let motion = current.with_overrides(
            number(args.get("start_handle")),
            number(args.get("end_handle")),
            number(args.get("arc_size")),
            number(args.get("arc_flow")),
            number(args.get("spring")),
            number(args.get("glide_duration_ms")),
            number(args.get("dwell_after_click_ms")),
            number(args.get("idle_hide_ms")),
            None,
            number(args.get("turn_radius")),
        );
        crate::cursor::overlay::send_command(
            session.clone(),
            cursor_overlay::OverlayCommand::SetMotion(motion.clone()),
        );
        ToolResult::text(format!(
            "Agent cursor motion updated for session '{session}'."
        ))
        .with_structured(serde_json::json!({
            "session": session,
            "motion": {
                "start_handle": motion.start_handle,
                "end_handle": motion.end_handle,
                "arc_size": motion.arc_size,
                "arc_flow": motion.arc_flow,
                "spring": motion.spring,
                "glide_duration_ms": motion.glide_duration_ms,
                "dwell_after_click_ms": motion.dwell_after_click_ms,
                "idle_hide_ms": motion.idle_hide_ms,
                "turn_radius": motion.turn_radius
            }
        }))
    }
}

pub struct SetAgentCursorThemeTool {
    state: Arc<ToolState>,
}

impl SetAgentCursorThemeTool {
    pub fn new(state: Arc<ToolState>) -> Self {
        Self { state }
    }
}

static THEME_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn theme_def() -> &'static ToolDef {
    THEME_DEF.get_or_init(|| canonical_cursor_def("set_agent_cursor_theme"))
}

fn parse_reduced_motion(value: Option<&str>) -> cursor_overlay::ReducedMotion {
    match value {
        Some("on") => cursor_overlay::ReducedMotion::On,
        Some("off") => cursor_overlay::ReducedMotion::Off,
        _ => cursor_overlay::ReducedMotion::Auto,
    }
}

#[async_trait]
impl Tool for SetAgentCursorThemeTool {
    fn def(&self) -> &ToolDef {
        theme_def()
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        if !self.state.cursor_overlay_available {
            return super::cursor_overlay_unavailable();
        }
        use cua_driver_core::tool_args::ArgsExt;
        let theme_id = match args.require_str("theme_id") {
            Ok(value) => value,
            Err(error) => return error,
        };
        let session = resolve_cursor_key(&args);
        let resolved_theme = match cursor_overlay::resolve_theme_selection(&theme_id) {
            Ok(theme) => theme,
            Err(error) => {
                return ToolResult::error(format!(
                    "Cursor theme '{theme_id}' cannot be selected: {error}"
                ));
            }
        };
        let (version, profile) = resolved_theme
            .as_deref()
            .map(|theme| (theme.version.as_str(), theme.profile.as_str()))
            .unwrap_or((
                cursor_overlay::DEFAULT_THEME_VERSION,
                cursor_overlay::THEME_PROFILE,
            ));
        let reduced_motion =
            parse_reduced_motion(args.get("reduced_motion").and_then(Value::as_str));
        let mut state = self.state.cursor_registry.get_or_create(&session);
        state.config.theme_id = theme_id.clone();
        state.config.reduced_motion = reduced_motion;
        self.state.cursor_registry.update_config(state.config);
        crate::cursor::overlay::send_command(
            session.clone(),
            cursor_overlay::OverlayCommand::SetTheme {
                theme_id: theme_id.clone(),
                reduced_motion,
            },
        );
        ToolResult::text(format!(
            "Agent cursor theme for session '{session}' set to '{theme_id}'."
        ))
        .with_structured(serde_json::json!({
            "session": session,
            "theme": {
                "id": theme_id,
                "version": version,
                "profile": profile,
                "reduced_motion": reduced_motion,
                "fallback": null
            }
        }))
    }
}

pub struct GetAgentCursorStateTool {
    state: Arc<ToolState>,
}

impl GetAgentCursorStateTool {
    pub fn new(state: Arc<ToolState>) -> Self {
        Self { state }
    }
}

static STATE_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn state_def() -> &'static ToolDef {
    STATE_DEF.get_or_init(|| canonical_cursor_def("get_agent_cursor_state"))
}

fn canonical_cursor_def(name: &str) -> ToolDef {
    let contract = cua_driver_contract::tool_contract(name)
        .unwrap_or_else(|| panic!("missing canonical cursor contract for {name}"));
    ToolDef::from_contract(&contract)
}

#[async_trait]
impl Tool for GetAgentCursorStateTool {
    fn def(&self) -> &ToolDef {
        state_def()
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        if !self.state.cursor_overlay_available {
            return super::cursor_overlay_unavailable();
        }
        let session = resolve_cursor_key(&args);
        let registry_state = self.state.cursor_registry.get(&session);
        let enabled = registry_state
            .as_ref()
            .map(|state| state.config.enabled)
            .unwrap_or(true);
        let position = registry_state
            .as_ref()
            .and_then(|state| state.position.as_ref())
            .map(|position| serde_json::json!({ "x": position.x, "y": position.y }));
        let motion = crate::cursor::overlay::current_motion(&session);
        let (theme_id, version, profile, fallback, visual) =
            crate::cursor::overlay::current_theme_state(&session).unwrap_or_else(|| {
                (
                    cursor_overlay::DEFAULT_THEME_ID.into(),
                    cursor_overlay::DEFAULT_THEME_VERSION.into(),
                    cursor_overlay::THEME_PROFILE.into(),
                    None,
                    cursor_overlay::CursorVisualState::default(),
                )
            });
        let modifiers: Vec<&str> = [
            visual.delivery.map(|value| value.as_str()),
            visual.target.map(|value| value.as_str()),
        ]
        .into_iter()
        .flatten()
        .collect();
        ToolResult::text(format!("Agent cursor state for session '{session}'.")).with_structured(
            serde_json::json!({
                "session": session,
                "enabled": enabled,
                "position": position,
                "theme": {
                    "id": theme_id,
                    "version": version,
                    "profile": profile,
                    "reduced_motion": visual.reduced_motion,
                    "fallback": fallback
                },
                "visual_state": {
                    "requested_action": visual.requested_action,
                    "resolved_action": visual.resolved_action,
                    "modifiers": modifiers,
                    "phase": visual.phase(),
                    "frame": visual.frame(),
                    "preempted_count": visual.preempted_count
                },
                "motion": {
                    "start_handle": motion.start_handle,
                    "end_handle": motion.end_handle,
                    "arc_size": motion.arc_size,
                    "arc_flow": motion.arc_flow,
                    "spring": motion.spring,
                    "glide_duration_ms": motion.glide_duration_ms,
                    "dwell_after_click_ms": motion.dwell_after_click_ms,
                    "idle_hide_ms": motion.idle_hide_ms,
                    "turn_radius": motion.turn_radius
                }
            }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_cursor_key;
    use serde_json::json;

    #[test]
    fn anonymous_calls_share_a_stable_auto_session() {
        let first = resolve_cursor_key(&json!({}));
        let second = resolve_cursor_key(&json!({ "x": 1 }));
        assert!(first.starts_with("auto-"));
        assert_eq!(first, second);
    }

    #[test]
    fn explicit_session_wins_over_injected_session() {
        assert_eq!(
            resolve_cursor_key(&json!({
                "session": "explicit",
                "_session_id": "injected"
            })),
            "explicit"
        );
    }

    #[test]
    fn removed_cursor_id_alias_is_not_an_identity_source() {
        assert_ne!(
            resolve_cursor_key(&json!({ "cursor_id": "legacy" })),
            "legacy"
        );
    }
}
