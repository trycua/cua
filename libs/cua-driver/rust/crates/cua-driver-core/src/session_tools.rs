//! Session lifecycle and per-session capture-scope tools.
//!
//! A session is lifecycle identity for an agent run (see [`crate::session`]).
//! It owns the agent cursor, per-run configuration, recording, cleanup, and
//! telemetry. A trusted transport lease receives one implicit session when a
//! session-requiring call omits a public label; explicit labels remain an
//! optional coordination mechanism inside the same trusted namespace.
//!
//! The daemon may mirror an explicit `session` arg into reserved transport
//! fields, but lifecycle and policy tools accept only the public `session`
//! name. Transport metadata can never mint or alter capture policy.

use crate::capture_scope::{bind_session, escalate_session, get_session, BindError, EscalateError};
use crate::protocol::ToolResult;
use crate::tool::{Tool, ToolDef};
use crate::tool_args::parse_typed_input;
use async_trait::async_trait;
use cua_driver_contract::{
    CaptureScope, EndSessionInput, EscalateSessionInput, EscalationReason, GetSessionInput,
    GetSessionStateInput, ListSessionsInput, ListSessionsOutput, SessionClientKindOutput,
    SessionLifecycleState, SessionOutput, SessionTransportOutput, StartSessionInput,
};
use serde_json::{json, Value};
use std::sync::OnceLock;

/// Resolve the runtime-private lifecycle id injected at the trusted registry
/// boundary. An explicit public label wins; otherwise the transport lease's
/// implicit id is used.
fn session_id_of(args: &Value) -> Option<String> {
    let args = args.as_object()?;
    ["session", "_session_id"].into_iter().find_map(|key| {
        args.get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty() && *s != "default")
            .map(str::to_owned)
    })
}

fn public_label_of(args: &Value) -> Option<String> {
    args.get("_public_session_label")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn response_session_label(args: &Value, session_id: &str) -> String {
    public_label_of(args).unwrap_or_else(|| {
        if args
            .get("session")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty() && value != "default")
        {
            crate::session::public_session_label(session_id).to_owned()
        } else {
            // Never expose a transport UUID or runtime-private key merely
            // because the caller used its unnamed implicit session.
            "implicit".to_owned()
        }
    })
}

fn owner_transport_of(args: &Value, session_id: &str) -> String {
    args.get("_transport_session_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or(session_id)
        .to_owned()
}

fn lifecycle_idle_ttl_of(args: &Value) -> Option<std::time::Duration> {
    args.get("_session_idle_ttl_ms")
        .and_then(Value::as_u64)
        .filter(|milliseconds| *milliseconds > 0)
        .map(std::time::Duration::from_millis)
}

fn lifecycle_output(snapshot: crate::session::LifecycleSessionSnapshot) -> SessionOutput {
    let cursor_visible = crate::session::cursor_visible(&snapshot.runtime_id);
    let recording_active = crate::session::recording_active(&snapshot.runtime_id);
    SessionOutput {
        session: snapshot.public_label,
        implicit: snapshot.implicit,
        state: if snapshot.ending {
            SessionLifecycleState::Ending
        } else {
            SessionLifecycleState::Active
        },
        client_kind: match snapshot.client_kind {
            crate::session::SessionClientKind::Cli => SessionClientKindOutput::Cli,
            crate::session::SessionClientKind::Direct => SessionClientKindOutput::Direct,
            crate::session::SessionClientKind::Mcp => SessionClientKindOutput::Mcp,
            crate::session::SessionClientKind::PythonSdk => SessionClientKindOutput::PythonSdk,
            crate::session::SessionClientKind::TypescriptSdk => {
                SessionClientKindOutput::TypescriptSdk
            }
        },
        transport: match snapshot.transport {
            crate::session::SessionTransport::Cli => SessionTransportOutput::Cli,
            crate::session::SessionTransport::Daemon => SessionTransportOutput::Daemon,
            crate::session::SessionTransport::McpStdio => SessionTransportOutput::McpStdio,
            crate::session::SessionTransport::McpHttp => SessionTransportOutput::McpHttp,
        },
        cursor_visible,
        recording_active,
        idle_seconds: snapshot.idle.as_secs(),
        expires_in_seconds: snapshot.expires_in.as_secs(),
    }
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
            return ToolResult::error("start_session requires an authenticated transport lease or a non-empty `session` label.");
        };
        if let Some(raw) = args.get("capture_scope") {
            let Some(raw) = raw.as_str() else {
                return ToolResult::error(
                    "start_session.capture_scope must be one of: auto, window, desktop.",
                )
                .with_structured(json!({ "code": "invalid_capture_scope" }));
            };
            if CaptureScope::parse(raw).is_none() {
                return ToolResult::error(format!(
                    "invalid capture_scope '{raw}'; expected auto, window, or desktop"
                ))
                .with_structured(json!({
                    "code": "invalid_capture_scope",
                    "capture_scope": raw,
                }));
            }
        }
        let input = match parse_typed_input::<StartSessionInput>("start_session", args.clone()) {
            Ok(input) => input,
            Err(result) => return result,
        };
        let requested = input.capture_scope;
        let legacy_capture = requested.is_some();
        let public_label = public_label_of(&args);
        let owner = owner_transport_of(&args, &id);
        let (transport, client_kind) = crate::session::infer_transport_metadata(&owner);
        if let Some(requested) = requested {
            if let Some(existing) = get_session(&id) {
                if existing.policy != requested {
                    return ToolResult::error(
                        "session already uses a different deprecated capture policy",
                    )
                    .with_structured(json!({
                        "code": "session_policy_conflict",
                        "capture_scope": existing.policy.as_str(),
                        "requested_capture_scope": requested.as_str(),
                    }));
                }
            }
        }
        // Tombstone removal and owner binding are one atomic lifecycle
        // transition. A guessed public label cannot be claimed in the gap
        // between revival and activation.
        let revived = match crate::session::activate_or_revive_session_for_owner(
            &id,
            public_label.as_deref(),
            &owner,
            public_label.is_none(),
            transport,
            client_kind,
            lifecycle_idle_ttl_of(&args),
        ) {
            Ok(revived) => revived,
            Err(_) => {
                return ToolResult::error("session is not available to this transport")
                    .with_structured(json!({ "code": "session_unavailable" }));
            }
        };
        let scope = if legacy_capture {
            match bind_session(&id, requested) {
                Ok((bound, _)) => bound,
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
            }
        } else {
            get_session(&id).unwrap_or_else(|| {
                crate::capture_scope::SessionCaptureScope::new(CaptureScope::Auto)
            })
        };
        // Platform overlays keep their own late-command tombstones. Clear
        // those only after core revival and capture-policy binding both
        // succeeded, so a failed declaration cannot revive render state.
        if revived {
            crate::session::fire_session_revive_for_owner(&id, &owner);
        }
        let structured = serde_json::to_value(cua_driver_contract::StartSessionOutput {
            state: scope.output(&response_session_label(&args, &id)),
            active: true,
            revived,
        })
        .expect("start_session output serializes");
        ToolResult::text(format!(
            "✅ Session '{}' is active{}.",
            public_label.as_deref().unwrap_or("implicit"),
            if legacy_capture {
                " with deprecated capture_scope compatibility"
            } else {
                ""
            }
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
        if args
            .get("reason")
            .and_then(Value::as_str)
            .and_then(EscalationReason::parse)
            .is_none()
        {
            return ToolResult::error("escalate_session.reason is invalid.")
                .with_structured(json!({ "code": "invalid_escalation_reason" }));
        }
        let input =
            match parse_typed_input::<EscalateSessionInput>("escalate_session", args.clone()) {
                Ok(input) => input,
                Err(result) => return result,
            };
        if input
            .detail
            .as_ref()
            .is_some_and(|detail| detail.chars().count() > 200)
        {
            return ToolResult::error("escalate_session.detail must be at most 200 characters.")
                .with_structured(json!({ "code": "invalid_escalation_detail" }));
        }
        let owner = owner_transport_of(&args, &id);
        if crate::session::session_snapshot(&id, &owner, crate::session::DEFAULT_SESSION_IDLE_TTL)
            .is_none()
        {
            return ToolResult::error("session is not visible to this transport")
                .with_structured(json!({ "code": "session_not_started" }));
        }
        match escalate_session(&id, input.reason, input.detail.as_deref()) {
            Ok(state) => ToolResult::text("✅ Session escalated to desktop scope.")
                .with_structured(state.as_json(&response_session_label(&args, &id))),
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
        if let Err(result) =
            parse_typed_input::<GetSessionStateInput>("get_session_state", args.clone())
        {
            return result;
        }
        let owner = owner_transport_of(&args, &id);
        if crate::session::session_snapshot(&id, &owner, crate::session::DEFAULT_SESSION_IDLE_TTL)
            .is_none()
        {
            return ToolResult::error("session is not visible to this transport")
                .with_structured(json!({ "code": "session_not_started" }));
        }
        let state = get_session(&id)
            .unwrap_or_else(|| crate::capture_scope::SessionCaptureScope::new(CaptureScope::Auto));
        ToolResult::text(format!(
            "Session '{id}' uses capture_scope='{}' (effective_scope='{}').",
            state.policy,
            state.effective_scope().as_str()
        ))
        .with_structured(state.as_json(&response_session_label(&args, &id)))
    }
}

// ── get_session / list_sessions ─────────────────────────────────────────────

pub struct GetSessionTool;

static GET_DEF: OnceLock<ToolDef> = OnceLock::new();

#[async_trait]
impl Tool for GetSessionTool {
    fn def(&self) -> &ToolDef {
        GET_DEF.get_or_init(|| session_tool_def("get_session"))
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        if let Err(result) = parse_typed_input::<GetSessionInput>("get_session", args.clone()) {
            return result;
        }
        let Some(id) = session_id_of(&args) else {
            return ToolResult::error("no implicit session is attached to this transport")
                .with_structured(json!({ "code": "session_not_started" }));
        };
        let owner = owner_transport_of(&args, &id);
        let Some(snapshot) =
            crate::session::session_snapshot(&id, &owner, crate::session::DEFAULT_SESSION_IDLE_TTL)
        else {
            return ToolResult::error("session is not visible to this transport")
                .with_structured(json!({ "code": "session_not_started" }));
        };
        let output = lifecycle_output(snapshot);
        ToolResult::text("Session is active.")
            .with_structured(serde_json::to_value(output).expect("get_session output serializes"))
    }
}

pub struct ListSessionsTool;

static LIST_DEF: OnceLock<ToolDef> = OnceLock::new();

#[async_trait]
impl Tool for ListSessionsTool {
    fn def(&self) -> &ToolDef {
        LIST_DEF.get_or_init(|| session_tool_def("list_sessions"))
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let input = match parse_typed_input::<ListSessionsInput>("list_sessions", args.clone()) {
            Ok(input) => input,
            Err(result) => return result,
        };
        let Some(owner) = args
            .get("_transport_session_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        else {
            return ToolResult::error("list_sessions requires an authenticated transport lease")
                .with_structured(json!({ "code": "transport_lease_required" }));
        };
        let limit = input.limit.unwrap_or(50);
        if limit == 0 || limit > 100 {
            return ToolResult::error("list_sessions.limit must be from 1 to 100")
                .with_structured(json!({ "code": "invalid_limit" }));
        }
        let offset = match input.cursor.as_deref() {
            None => 0,
            Some(cursor) => match cursor
                .strip_prefix("o:")
                .and_then(|v| v.parse::<usize>().ok())
            {
                Some(offset) => offset,
                None => {
                    return ToolResult::error("list_sessions.cursor is invalid")
                        .with_structured(json!({ "code": "invalid_cursor" }))
                }
            },
        };
        let sessions =
            crate::session::list_session_snapshots(owner, crate::session::DEFAULT_SESSION_IDLE_TTL);
        let end = (offset + limit as usize).min(sessions.len());
        let page = sessions
            .get(offset..end)
            .unwrap_or_default()
            .iter()
            .cloned()
            .map(lifecycle_output)
            .collect::<Vec<_>>();
        let output = ListSessionsOutput {
            sessions: page,
            next_cursor: (end < sessions.len()).then(|| format!("o:{end}")),
        };
        ToolResult::text(format!("{} visible session(s).", output.sessions.len()))
            .with_structured(serde_json::to_value(output).expect("list_sessions output serializes"))
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
            return ToolResult::error(
                "end_session requires an attached implicit session or a non-empty `session` label.",
            );
        };
        if let Err(result) = parse_typed_input::<EndSessionInput>("end_session", args.clone()) {
            return result;
        }
        let owner = owner_transport_of(&args, &id);
        let owned = crate::session::end_session_for_owner(&id, &owner);
        if owned {
            let cleanup = if crate::session::is_session_ended(&id) {
                crate::session::session_cleanup_status(&id)
            } else {
                crate::session::SessionCleanupReport {
                    complete: false,
                    in_progress: true,
                    failures: Vec::new(),
                }
            };
            if !cleanup.complete {
                return ToolResult::error(if cleanup.in_progress {
                    "Session is ending after its in-flight action completes; retry end_session for final cleanup status."
                } else {
                    "Session ended, but one or more cleanup hooks failed; retry end_session."
                })
                .with_structured(json!({
                    "code": if cleanup.in_progress { "session_cleanup_pending" } else { "session_cleanup_partial" },
                    "session": public_label_of(&args)
                        .unwrap_or_else(|| crate::session::public_session_label(&id).to_owned()),
                    "cleanup_complete": false,
                    "cleanup_in_progress": cleanup.in_progress,
                    // Hook names are stable, non-sensitive categories. Keep
                    // implementation errors private because adapters may
                    // include local paths or platform details in them.
                    "failed_hooks": cleanup.failures.into_iter().map(|failure| json!({
                        "hook": failure.hook,
                        "error": "cleanup failed",
                    })).collect::<Vec<_>>(),
                }));
            }
        }
        let response_label = response_session_label(&args, &id);
        ToolResult::text(format!("✅ Session '{response_label}' ended.")).with_structured(
            serde_json::to_value(cua_driver_contract::EndSessionOutput {
                session: response_label,
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
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    fn result_code(result: &ToolResult) -> Option<&str> {
        result.structured_content.as_ref()?.get("code")?.as_str()
    }

    #[test]
    fn session_id_of_prefers_public_and_uses_trusted_implicit_fallback() {
        assert_eq!(
            session_id_of(&json!({ "session": "a" })).as_deref(),
            Some("a")
        );
        assert_eq!(
            session_id_of(&json!({ "_session_id": "b" })).as_deref(),
            Some("b")
        );
        assert_eq!(
            session_id_of(&json!({ "session": "", "_session_id": "c" })).as_deref(),
            Some("c")
        );
        assert_eq!(session_id_of(&json!({})), None);
        assert_eq!(session_id_of(&json!({ "session": "" })), None);
        assert_eq!(session_id_of(&json!({ "session": "default" })), None);
    }

    #[tokio::test]
    async fn implicit_start_does_not_expose_its_private_transport_identity() {
        let id = format!("private-implicit-session-{}", std::process::id());
        let owner = format!("private-transport-{}", std::process::id());
        let result = StartSessionTool
            .invoke(json!({
                "_session_id": id.clone(),
                "_transport_session_id": owner.clone(),
            }))
            .await;
        assert_ne!(result.is_error, Some(true));
        let structured = result.structured_content.as_ref().unwrap();
        assert_eq!(structured["session"], "implicit");
        let encoded = serde_json::to_string(structured).unwrap();
        assert!(!encoded.contains(&owner));
        EndSessionTool
            .invoke(json!({
                "_session_id": id,
                "_transport_session_id": owner,
            }))
            .await;
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
    async fn successful_explicit_revival_notifies_session_owned_subsystems_once() {
        let id = format!("session-tool-revive-hook-{}", std::process::id());
        let notifications = Arc::new(AtomicUsize::new(0));
        let observed_id = id.clone();
        let notifications_for_hook = notifications.clone();
        let _registration = crate::session::register_scoped_session_revive_hook(move |got| {
            if got == observed_id {
                notifications_for_hook.fetch_add(1, Ordering::SeqCst);
            }
        });
        let start = StartSessionTool;

        let fresh = start
            .invoke(json!({"session": id, "capture_scope": "window"}))
            .await;
        assert_ne!(fresh.is_error, Some(true));
        assert_eq!(notifications.load(Ordering::SeqCst), 0);

        EndSessionTool.invoke(json!({"session": id})).await;
        let revived = start
            .invoke(json!({"session": id, "capture_scope": "desktop"}))
            .await;
        assert_ne!(revived.is_error, Some(true));
        assert_eq!(
            revived.structured_content.as_ref().unwrap()["revived"],
            true
        );
        assert_eq!(notifications.load(Ordering::SeqCst), 1);

        let idempotent = start
            .invoke(json!({"session": id, "capture_scope": "desktop"}))
            .await;
        assert_ne!(idempotent.is_error, Some(true));
        assert_eq!(notifications.load(Ordering::SeqCst), 1);
        EndSessionTool.invoke(json!({"session": id})).await;
    }

    #[tokio::test]
    async fn cleanup_failure_response_redacts_adapter_error_details() {
        let id = format!("session-tool-cleanup-redaction-{}", std::process::id());
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_for_hook = calls.clone();
        let observed_id = id.clone();
        let _registration =
            crate::session::register_scoped_fallible_session_end_hook("recording", move |ended| {
                if ended != observed_id {
                    return Ok(());
                }
                let call = calls_for_hook.fetch_add(1, Ordering::SeqCst);
                if call < 2 {
                    Err("failed at /Users/private/recording.mov".into())
                } else {
                    Ok(())
                }
            });

        let started = StartSessionTool.invoke(json!({"session": id})).await;
        assert_ne!(started.is_error, Some(true));
        let ended = EndSessionTool.invoke(json!({"session": id})).await;
        assert_eq!(ended.is_error, Some(true));
        assert_eq!(result_code(&ended), Some("session_cleanup_partial"));
        let encoded = serde_json::to_string(&ended.structured_content).unwrap();
        assert!(encoded.contains("cleanup failed"));
        assert!(!encoded.contains("/Users/private"));

        let second = EndSessionTool.invoke(json!({"session": id})).await;
        assert_eq!(second.is_error, Some(true));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let third = EndSessionTool.invoke(json!({"session": id})).await;
        assert_ne!(third.is_error, Some(true));
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn lifecycle_tools_do_not_expose_or_mutate_foreign_transport_sessions() {
        let id = format!("session-tool-owner-scope-{}", std::process::id());
        let owner_a = format!("http-owner-a-{}", std::process::id());
        let owner_b = format!("http-owner-b-{}", std::process::id());
        let owned_args = json!({
            "session": id,
            "_public_session_label": "shared-label",
            "_transport_session_id": owner_a,
            "capture_scope": "auto"
        });
        let started = StartSessionTool.invoke(owned_args.clone()).await;
        assert_ne!(started.is_error, Some(true));

        let foreign = json!({
            "session": id,
            "_public_session_label": "shared-label",
            "_transport_session_id": owner_b
        });
        let get = GetSessionTool.invoke(foreign.clone()).await;
        assert_eq!(get.is_error, Some(true));
        assert_eq!(result_code(&get), Some("session_not_started"));

        let state = GetSessionStateTool.invoke(foreign.clone()).await;
        assert_eq!(state.is_error, Some(true));
        assert_eq!(result_code(&state), Some("session_not_started"));

        let list = ListSessionsTool
            .invoke(json!({"_transport_session_id": owner_b}))
            .await;
        assert_eq!(
            list.structured_content.as_ref().unwrap()["sessions"],
            json!([])
        );

        let ended = EndSessionTool.invoke(foreign.clone()).await;
        assert_ne!(ended.is_error, Some(true), "end remains non-enumerating");
        assert!(crate::session::session_snapshot(
            &id,
            &owner_a,
            crate::session::DEFAULT_SESSION_IDLE_TTL
        )
        .is_some());

        let stolen = StartSessionTool.invoke(foreign).await;
        assert_eq!(stolen.is_error, Some(true));
        assert_eq!(result_code(&stolen), Some("session_unavailable"));

        EndSessionTool.invoke(owned_args).await;
    }

    #[tokio::test]
    async fn auto_requires_explicit_bounded_escalation() {
        let id = format!("session-tool-auto-{}", std::process::id());
        let started = StartSessionTool
            .invoke(json!({"session": id, "capture_scope": "auto"}))
            .await;
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
