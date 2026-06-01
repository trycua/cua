//! Platform-independent `name_session` tool.
//!
//! Registered on all platforms via `ToolRegistry::register_session_tools()`.
//! Sets a friendly, IMMUTABLE, per-session name (write-once; first call wins)
//! used as the label shown beside that session's agent cursor so concurrent
//! agents are distinguishable. Lives in core (not a platform crate) so every
//! platform shares it — matching the recording-tools precedent.

use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::OnceLock;

use crate::{
    protocol::ToolResult,
    session,
    tool::{Tool, ToolDef},
    tool_args::ArgsExt,
};

pub struct NameSessionTool;

static NAME_SESSION_DEF: OnceLock<ToolDef> = OnceLock::new();

/// Resolve the SESSION key for this invocation. This MUST stay identical to
/// `platform-macos::cursor_tools::resolve_cursor_key` (which can't be imported
/// into core) so the name key and the cursor key never diverge: explicit
/// non-empty `cursor_id` > daemon-injected non-empty `_session_id` > `"default"`.
fn resolve_session_key(args: &Value) -> String {
    if let Some(explicit) = args.opt_str("cursor_id") {
        if !explicit.is_empty() {
            return explicit;
        }
    }
    if let Some(sess) = args.opt_str("_session_id") {
        if !sess.is_empty() {
            return sess;
        }
    }
    "default".to_owned()
}

#[async_trait]
impl Tool for NameSessionTool {
    fn def(&self) -> &ToolDef {
        NAME_SESSION_DEF.get_or_init(|| ToolDef {
            name: "name_session".into(),
            description: "Set a short, friendly, IMMUTABLE name for THIS session, shown beside \
                its agent cursor so concurrent agents are distinguishable. Call it once, early \
                in a session. The first call wins; later calls return the existing name unchanged \
                (write-once for the session lifetime). The name is sanitized (single line, \
                control chars stripped, capped at 24 chars). If you never name a session, a short \
                auto tag derived from the session id (e.g. `mcp-51088`) is shown instead, so the \
                cursor is always identifiable."
                .into(),
            input_schema: json!({
                "type": "object",
                "required": ["name"],
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Short, immutable label (≤24 chars) for THIS session, \
                            shown beside its cursor so concurrent agents are distinguishable. \
                            First call wins; later calls return the existing name unchanged."
                    }
                },
                "additionalProperties": false
            }),
            read_only: false,
            destructive: false,
            idempotent: true,
            open_world: false,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let name = match args.require_str("name") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let key = resolve_session_key(&args);
        let effective = session::set_session_name(&key, &name);
        ToolResult::text(format!("Session named '{effective}'."))
            .with_structured(json!({ "session_name": effective }))
    }
}
