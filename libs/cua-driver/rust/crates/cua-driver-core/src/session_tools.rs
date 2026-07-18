//! Session lifecycle and per-session capture-scope tools.
//!
//! A session is a **caller-declared** identity for an agent run (see
//! [`crate::session`]). It owns the agent cursor and is the key for
//! per-session config + recording. These tools bookend a run's lifetime
//! explicitly, decoupled from the MCP connection: the same `session` id works
//! identically over MCP, the CLI (`--session`), or the raw socket, and a run
//! can span any number of apps/windows.
//!
//! The daemon may mirror an explicit `session` arg into reserved transport
//! fields, but lifecycle and policy tools accept only the public `session`
//! name. Transport metadata can never mint or alter capture policy.

use crate::capture_scope::{
    bind_session, escalate_session, get_session, BindError, CaptureScopePolicy, EscalateError,
    EscalationReason,
};
use crate::protocol::ToolResult;
use crate::tool::{Tool, ToolDef};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::OnceLock;

/// Read the public, caller-declared session id. Capture-scope policy must never
/// bind to the daemon's reserved `_session_id` transport mirror.
fn session_id_of(args: &Value) -> Option<String> {
    args.as_object()?
        .get("session")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty() && *s != "default")
        .map(|s| s.to_owned())
}

fn capture_scope_arg(args: &Value) -> Result<Option<CaptureScopePolicy>, ToolResult> {
    let Some(raw) = args.get("capture_scope") else {
        return Ok(None);
    };
    let Some(raw) = raw.as_str() else {
        return Err(ToolResult::error(
            "start_session.capture_scope must be one of: auto, window, desktop.",
        )
        .with_structured(json!({ "code": "invalid_capture_scope" })));
    };
    CaptureScopePolicy::parse(raw).map(Some).ok_or_else(|| {
        ToolResult::error(format!(
            "invalid capture_scope '{raw}'; expected auto, window, or desktop"
        ))
        .with_structured(json!({
            "code": "invalid_capture_scope",
            "capture_scope": raw,
        }))
    })
}

// ── start_session ─────────────────────────────────────────────────────────────

pub struct StartSessionTool;

static START_DEF: OnceLock<ToolDef> = OnceLock::new();

#[async_trait]
impl Tool for StartSessionTool {
    fn def(&self) -> &ToolDef {
        START_DEF.get_or_init(|| session_tool_def("start_session"))
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let Some(id) = session_id_of(&args) else {
            return ToolResult::error("start_session requires a non-empty `session` id.");
        };
        let requested = match capture_scope_arg(&args) {
            Ok(scope) => scope,
            Err(result) => return result,
        };
        let was_ended = crate::session::is_session_ended(&id);
        if !was_ended {
            match bind_session(&id, requested) {
                Ok(_) => {}
                Err(BindError::Conflict {
                    existing,
                    requested,
                }) => {
                    return ToolResult::error(format!(
                        "session '{id}' already uses capture_scope='{existing}'; capture scope is immutable until the session ends"
                    ))
                    .with_structured(json!({
                        "code": "session_policy_conflict",
                        "session": id,
                        "capture_scope": existing.as_str(),
                        "requested_capture_scope": requested.as_str(),
                    }));
                }
                Err(BindError::Ended) => {
                    return ToolResult::error(format!(
                        "session '{id}' ended concurrently; retry start_session to revive it"
                    ))
                    .with_structured(json!({ "code": "session_ended", "session": id }));
                }
            }
        }
        // Revive a recycled id: if this session was previously ended (explicit
        // `end_session` / idle-TTL / connection EOF), clear its tombstone so its
        // actions stop being rejected by the daemon's resurrection guard.
        // Re-declaring a session is the EXPLICIT, caller-driven way to reuse an
        // id — a stray late action still can't silently resurrect a dead one.
        let revived = crate::session::revive_session(&id);
        let (scope, _) = match bind_session(&id, requested) {
            Ok(bound) => bound,
            Err(BindError::Conflict {
                existing,
                requested,
            }) => {
                return ToolResult::error(format!(
                    "session '{id}' already uses capture_scope='{existing}'; capture scope is immutable until the session ends"
                ))
                .with_structured(json!({
                    "code": "session_policy_conflict",
                    "session": id,
                    "capture_scope": existing.as_str(),
                    "requested_capture_scope": requested.as_str(),
                }));
            }
            Err(BindError::Ended) => {
                return ToolResult::error(format!("session '{id}' could not be revived"))
                    .with_structured(json!({ "code": "session_ended", "session": id }));
            }
        };
        // Refresh (or begin) the session's idle-TTL clock. The cursor appears on
        // the first action carrying this `session`.
        crate::session::touch_session(&id);
        let structured = serde_json::to_value(cua_driver_contract::StartSessionOutput {
            state: scope.output(&id),
            active: true,
            revived,
        })
        .expect("start_session output serializes");
        ToolResult::text(format!(
            "✅ Session '{id}' is active with capture_scope='{}'.",
            scope.policy
        ))
        .with_structured(structured)
    }
}

// ── escalate_session ─────────────────────────────────────────────────────────

pub struct EscalateSessionTool;

static ESCALATE_DEF: OnceLock<ToolDef> = OnceLock::new();

#[async_trait]
impl Tool for EscalateSessionTool {
    fn def(&self) -> &ToolDef {
        ESCALATE_DEF.get_or_init(|| session_tool_def("escalate_session"))
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let Some(id) = session_id_of(&args) else {
            return ToolResult::error("escalate_session requires a public `session` id.")
                .with_structured(json!({ "code": "session_required" }));
        };
        let Some(reason) = args
            .get("reason")
            .and_then(Value::as_str)
            .and_then(EscalationReason::parse)
        else {
            return ToolResult::error("escalate_session.reason is invalid.")
                .with_structured(json!({ "code": "invalid_escalation_reason" }));
        };
        let detail = args.get("detail").and_then(Value::as_str);
        match escalate_session(&id, reason, detail) {
            Ok(state) => ToolResult::text(format!("✅ Session '{id}' escalated to desktop scope."))
                .with_structured(state.as_json(&id)),
            Err(error) => {
                let (code, message) = match error {
                    EscalateError::Ended => (
                        "session_ended",
                        format!("session '{id}' has ended; start it again before escalating"),
                    ),
                    EscalateError::NotStarted => (
                        "session_not_started",
                        format!("session '{id}' has no capture policy; call start_session first"),
                    ),
                    EscalateError::WindowStrict => (
                        "desktop_scope_disabled",
                        format!("session '{id}' is strict window scope and cannot escalate"),
                    ),
                    EscalateError::DesktopAlreadyActive => (
                        "desktop_already_active",
                        format!("session '{id}' already has effective desktop scope"),
                    ),
                };
                ToolResult::error(message).with_structured(json!({
                    "code": code,
                    "session": id,
                }))
            }
        }
    }
}

// ── get_session_state ────────────────────────────────────────────────────────

pub struct GetSessionStateTool;

static GET_STATE_DEF: OnceLock<ToolDef> = OnceLock::new();

#[async_trait]
impl Tool for GetSessionStateTool {
    fn def(&self) -> &ToolDef {
        GET_STATE_DEF.get_or_init(|| session_tool_def("get_session_state"))
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let Some(id) = session_id_of(&args) else {
            return ToolResult::error("get_session_state requires a public `session` id.")
                .with_structured(json!({ "code": "session_required" }));
        };
        let Some(state) = get_session(&id) else {
            return ToolResult::error(format!("session '{id}' is not active"))
                .with_structured(json!({ "code": "session_not_started", "session": id }));
        };
        ToolResult::text(format!(
            "Session '{id}' uses capture_scope='{}' (effective_scope='{}').",
            state.policy,
            state.effective_scope().as_str()
        ))
        .with_structured(state.as_json(&id))
    }
}

// ── end_session ───────────────────────────────────────────────────────────────

pub struct EndSessionTool;

static END_DEF: OnceLock<ToolDef> = OnceLock::new();

#[async_trait]
impl Tool for EndSessionTool {
    fn def(&self) -> &ToolDef {
        END_DEF.get_or_init(|| session_tool_def("end_session"))
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let Some(id) = session_id_of(&args) else {
            return ToolResult::error("end_session requires a non-empty `session` id.");
        };
        crate::session::end_session(&id);
        ToolResult::text(format!("✅ Session '{id}' ended.")).with_structured(
            serde_json::to_value(cua_driver_contract::EndSessionOutput {
                session: id,
                active: false,
            })
            .expect("end_session output serializes"),
        )
    }
}

fn session_tool_def(name: &str) -> ToolDef {
    let contract = cua_driver_contract::tool_contract(name)
        .unwrap_or_else(|| panic!("canonical contract missing {name}"));
    ToolDef::from_contract(&contract)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result_code(result: &ToolResult) -> Option<&str> {
        result.structured_content.as_ref()?.get("code")?.as_str()
    }

    #[test]
    fn session_id_of_reads_only_public_session() {
        assert_eq!(
            session_id_of(&json!({ "session": "a" })).as_deref(),
            Some("a")
        );
        assert_eq!(session_id_of(&json!({ "_session_id": "b" })), None);
        assert_eq!(
            session_id_of(&json!({ "session": "", "_session_id": "c" })),
            None
        );
        assert_eq!(session_id_of(&json!({})), None);
        assert_eq!(session_id_of(&json!({ "session": "" })), None);
        assert_eq!(session_id_of(&json!({ "session": "default" })), None);
    }

    #[tokio::test]
    async fn live_policy_is_immutable_and_ended_id_gets_fresh_policy() {
        let id = format!("session-tool-policy-{}", std::process::id());
        let start = StartSessionTool;
        let first = start
            .invoke(json!({"session": id, "capture_scope": "window"}))
            .await;
        assert_ne!(first.is_error, Some(true));
        assert_eq!(
            first.structured_content.as_ref().unwrap()["capture_scope"],
            "window"
        );

        let conflict = start
            .invoke(json!({"session": id, "capture_scope": "desktop"}))
            .await;
        assert_eq!(conflict.is_error, Some(true));
        assert_eq!(result_code(&conflict), Some("session_policy_conflict"));

        EndSessionTool.invoke(json!({"session": id})).await;
        assert!(get_session(&id).is_none());
        let revived = start
            .invoke(json!({"session": id, "capture_scope": "desktop"}))
            .await;
        assert_ne!(revived.is_error, Some(true));
        let structured = revived.structured_content.as_ref().unwrap();
        assert_eq!(structured["capture_scope"], "desktop");
        assert_eq!(structured["effective_scope"], "desktop");
        assert_eq!(structured["revived"], true);
    }

    #[tokio::test]
    async fn auto_requires_explicit_bounded_escalation() {
        let id = format!("session-tool-auto-{}", std::process::id());
        let started = StartSessionTool.invoke(json!({"session": id})).await;
        assert_eq!(
            started.structured_content.as_ref().unwrap()["capture_scope"],
            "auto"
        );
        let escalated = EscalateSessionTool
            .invoke(json!({
                "session": id,
                "reason": "background_delivery_failed",
                "detail": "window ladder exhausted"
            }))
            .await;
        assert_ne!(escalated.is_error, Some(true));
        let structured = escalated.structured_content.as_ref().unwrap();
        assert_eq!(structured["effective_scope"], "desktop");
        assert_eq!(structured["desktop_unlocked"], true);

        let twice = EscalateSessionTool
            .invoke(json!({"session": id, "reason": "other"}))
            .await;
        assert_eq!(twice.is_error, Some(true));
        assert_eq!(result_code(&twice), Some("desktop_already_active"));
    }
}
