use super::{
    choose_keyboard_cursor_target, cursor_control_scope, named_session_cursor_key,
    reveal_pointer_action_for, CursorControlScope, ToolState,
};
use serde_json::json;

#[test]
fn lifecycle_owned_sessions_opt_into_keyboard_cursor_positioning() {
    assert_eq!(
        named_session_cursor_key(&json!({"session": "editing-run"})).as_deref(),
        Some("editing-run")
    );
    assert_eq!(
        named_session_cursor_key(&json!({"cursor_id": "legacy"})),
        None
    );
    assert_eq!(
        named_session_cursor_key(&json!({"_session_id": "implicit"})).as_deref(),
        Some("implicit")
    );
    assert_eq!(named_session_cursor_key(&json!({})), None);
}

#[test]
fn keyboard_cursor_uses_explicit_then_remembered_then_safe_seed() {
    let explicit = Some((10.0, 20.0));
    let remembered = Some((30.0, 40.0));
    let window_center = Some((50.0, 60.0));
    let current_pointer = Some((70.0, 80.0));

    assert_eq!(
        choose_keyboard_cursor_target(explicit, remembered, window_center, current_pointer),
        explicit
    );
    assert_eq!(
        choose_keyboard_cursor_target(None, remembered, window_center, current_pointer),
        remembered
    );
    assert_eq!(
        choose_keyboard_cursor_target(None, None, window_center, current_pointer),
        window_center
    );
    assert_eq!(
        choose_keyboard_cursor_target(None, None, None, current_pointer),
        current_pointer
    );
}

#[test]
fn invalid_coordinates_do_not_poison_session_position_reuse() {
    assert_eq!(
        choose_keyboard_cursor_target(Some((f64::NAN, 1.0)), Some((12.0, 34.0)), None, None,),
        Some((12.0, 34.0))
    );
}

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
