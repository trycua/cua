use super::{cursor_control_scope, reveal_pointer_action_for, CursorControlScope, ToolState};
use serde_json::json;

#[test]
fn real_pointer_control_requires_explicit_desktop_scope() {
    assert_eq!(cursor_control_scope(&json!({})), CursorControlScope::Agent);
    assert_eq!(
        cursor_control_scope(&json!({"scope": "window"})),
        CursorControlScope::Agent
    );
    assert_eq!(
        cursor_control_scope(&json!({"scope": "desktop"})),
        CursorControlScope::Desktop
    );
}

#[tokio::test]
async fn pointer_position_survives_an_unavailable_overlay() {
    let state = ToolState::new();
    reveal_pointer_action_for(&state, "no-renderer", 123.0, 456.0, true).await;

    let cursor = state
        .cursor_registry
        .get("no-renderer")
        .expect("pointer action records its position independently of rendering");
    assert!(cursor.config.enabled, "input must revive its agent cursor");
    assert_eq!(cursor.x.zip(cursor.y), Some((123.0, 456.0)));
}
