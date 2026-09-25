use super::{resolve_cursor_key, NO_CURSOR};
use serde_json::json;

/// Precedence is core `tool_args::session_key`'s contract; Windows only
/// owns the cursor-less fallback for a direct call without lifecycle
/// metadata.
#[test]
fn anonymous_direct_call_is_cursor_less() {
    assert_eq!(resolve_cursor_key(&json!({ "pid": 1 })), NO_CURSOR);
    assert_eq!(
        resolve_cursor_key(&json!({ "session": "", "cursor_id": "" })),
        NO_CURSOR
    );
    assert_eq!(
        resolve_cursor_key(&json!({ "_session_id": "mcp-1-2" })),
        "mcp-1-2"
    );
}
