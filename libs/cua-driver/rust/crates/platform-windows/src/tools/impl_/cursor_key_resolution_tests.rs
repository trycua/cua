use super::{resolve_cursor_key, NO_CURSOR};
use serde_json::json;

#[test]
fn direct_platform_call_without_lifecycle_resolves_to_no_cursor() {
    // No session/cursor_id → NO_CURSOR (""): the action still runs but no
    // cursor is shown. Canonical core dispatch injects `_session_id` before
    // real platform calls.
    assert_eq!(resolve_cursor_key(&json!({})), NO_CURSOR);
    assert_eq!(resolve_cursor_key(&json!({ "pid": 1 })), NO_CURSOR);
    assert_eq!(
        resolve_cursor_key(&json!({ "_session_id": "mcp-1-2" })),
        "mcp-1-2"
    );
}

#[test]
fn explicit_session_owns_a_cursor() {
    assert_eq!(
        resolve_cursor_key(&json!({ "session": "research-run" })),
        "research-run"
    );
}

#[test]
fn cursor_id_is_a_legacy_alias_and_session_wins() {
    assert_eq!(
        resolve_cursor_key(&json!({ "cursor_id": "user-handle" })),
        "user-handle"
    );
    assert_eq!(
        resolve_cursor_key(&json!({ "session": "s1", "cursor_id": "c1" })),
        "s1"
    );
    assert_eq!(
        resolve_cursor_key(&json!({ "_session_id": "implicit", "cursor_id": "c1" })),
        "implicit"
    );
}

#[test]
fn empty_strings_fall_through_to_no_cursor() {
    // An empty `session` falls through to `cursor_id`; both empty → NO_CURSOR.
    assert_eq!(
        resolve_cursor_key(&json!({ "session": "", "cursor_id": "c1" })),
        "c1"
    );
    assert_eq!(
        resolve_cursor_key(&json!({ "session": "", "cursor_id": "" })),
        NO_CURSOR
    );
}
