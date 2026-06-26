//! `start_session` / `end_session` tools.
//!
//! A session is a **caller-declared** identity for an agent run (see
//! [`crate::session`]). It owns the agent cursor and is the key for
//! per-session config + recording. These tools bookend a run's lifetime
//! explicitly, decoupled from the MCP connection: the same `session` id works
//! identically over MCP, the CLI (`--session`), or the raw socket, and a run
//! can span any number of apps/windows.
//!
//! The daemon mirrors an explicit `session` arg into the reserved `_session_id`
//! key (see `serve.rs::apply_session_identity`), so these tools accept either —
//! `session` is the public name.

use crate::protocol::ToolResult;
use crate::tool::{Tool, ToolDef};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::OnceLock;

/// Read the explicit session id from a tool call (`session`, or its daemon
/// mirror `_session_id`). Empty / missing → `None`.
fn session_id_of(args: &Value) -> Option<String> {
    let obj = args.as_object()?;
    ["session", "_session_id"]
        .into_iter()
        .find_map(|k| obj.get(k).and_then(|v| v.as_str()).filter(|s| !s.is_empty()))
        .map(|s| s.to_owned())
}

// ── start_session ─────────────────────────────────────────────────────────────

pub struct StartSessionTool;

static START_DEF: OnceLock<ToolDef> = OnceLock::new();

#[async_trait]
impl Tool for StartSessionTool {
    fn def(&self) -> &ToolDef {
        START_DEF.get_or_init(|| ToolDef {
            name: "start_session".into(),
            description:
                "Declare a session — a named, color-coded identity for THIS agent run. \
                 Pass a stable `session` id; the agent cursor, per-session config, and \
                 recording all key on it, and it follows the run across any apps/windows. \
                 The cursor's color is derived from the id, so distinct runs are visually \
                 distinct. A cursor is shown only for a declared session — call this (or \
                 pass `session` on your first action) to opt in. Idempotent: re-calling \
                 with the same id refreshes its idle-TTL, and RESUMES the session if a \
                 prior idle-TTL reclaim or `end_session` had ended it — so calling this \
                 is also how you recover from a \"session ended; tool call ignored\" \
                 error. End it with `end_session` (or let the idle-TTL reclaim it). \
                 Concurrent runs/subagents each pass their own `session` to get their \
                 own cursor."
                    .into(),
            input_schema: json!({
                "type": "object",
                "required": ["session"],
                "properties": {
                    "session": {
                        "type": "string",
                        "description": "Stable session id for this run (e.g. \"research-run-1\")."
                    }
                },
                "additionalProperties": true
            }),
            read_only: false,
            destructive: false,
            idempotent: true,
            open_world: false,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let Some(id) = session_id_of(&args) else {
            return ToolResult::error(
                "start_session requires a non-empty `session` id.",
            );
        };
        // Begin / refresh / RESUME the session. `revive_session` is a superset
        // of `touch_session`: for a live or brand-new id it just (re)arms the
        // idle-TTL clock; for an id a prior idle-TTL sweep or `end_session`
        // marked ended it also clears the tombstone so the run can continue.
        // Without this, an idle-reaped session was permanently dead — even
        // `start_session` got rejected by the daemon's resurrection guard.
        crate::session::revive_session(&id);
        ToolResult::text(format!("✅ Session '{id}' is active."))
            .with_structured(json!({ "session": id, "active": true }))
    }
}

// ── end_session ───────────────────────────────────────────────────────────────

pub struct EndSessionTool;

static END_DEF: OnceLock<ToolDef> = OnceLock::new();

#[async_trait]
impl Tool for EndSessionTool {
    fn def(&self) -> &ToolDef {
        END_DEF.get_or_init(|| ToolDef {
            name: "end_session".into(),
            description:
                "End a session declared with `start_session`: removes its agent cursor, \
                 stops any recording it owns, and clears its per-session config. Call this \
                 when a run finishes so its cursor doesn't linger (otherwise the idle-TTL \
                 reclaims it after a period of inactivity). Idempotent."
                    .into(),
            input_schema: json!({
                "type": "object",
                "required": ["session"],
                "properties": {
                    "session": {
                        "type": "string",
                        "description": "The session id to end."
                    }
                },
                "additionalProperties": true
            }),
            read_only: false,
            destructive: true,
            idempotent: true,
            open_world: false,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let Some(id) = session_id_of(&args) else {
            return ToolResult::error("end_session requires a non-empty `session` id.");
        };
        crate::session::end_session(&id);
        ToolResult::text(format!("✅ Session '{id}' ended."))
            .with_structured(json!({ "session": id, "active": false }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_id_of_reads_session_then_mirror() {
        assert_eq!(session_id_of(&json!({ "session": "a" })).as_deref(), Some("a"));
        assert_eq!(session_id_of(&json!({ "_session_id": "b" })).as_deref(), Some("b"));
        assert_eq!(session_id_of(&json!({ "session": "", "_session_id": "c" })).as_deref(), Some("c"));
        assert_eq!(session_id_of(&json!({})), None);
        assert_eq!(session_id_of(&json!({ "session": "" })), None);
    }
}
