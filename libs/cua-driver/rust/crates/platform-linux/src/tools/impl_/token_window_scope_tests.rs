use super::{element_click_placement, refuse_unproven_window_element, window_element_refusal};
use crate::atspi::native::window_element_membership;
use crate::atspi::snapshot::CachedElement;
use cua_driver_core::element_token::ResolvedElement;
use cua_driver_core::protocol::ToolResult;

fn refusal_code(refusal: &ToolResult) -> &str {
    assert_eq!(refusal.is_error, Some(true));
    let structured = refusal.structured_content.as_ref().expect("structured");
    assert_eq!(structured["status"], "refused");
    structured["refusal"]["code"]
        .as_str()
        .expect("refusal code")
}

#[test]
fn an_unproven_window_element_refuses_before_dispatch() {
    // The membership walk no longer finds the element in window 2's scope.
    let unproven = window_element_membership(Some(0), None, || false);
    let refusal = window_element_refusal("click", 2, unproven).expect("refused");
    assert_eq!(refusal_code(&refusal), "window_scope_unproven");
    let gone = window_element_membership(None, Some(0), || true);
    let refusal = window_element_refusal("type_text", 2, gone).expect("refused");
    assert_eq!(refusal_code(&refusal), "window_scope_unproven");
    assert!(window_element_refusal("click", 2, Ok(())).is_none());
}

#[test]
fn a_membership_failure_in_the_click_glide_refuses_instead_of_acting() {
    let unproven = window_element_membership(Some(0), None, || false).map(|()| (2, 0.0, 0.0));
    let refusal = element_click_placement(unproven, Some(2)).expect_err("refused");
    assert_eq!(refusal_code(&refusal), "window_scope_unproven");
    // Any other lookup failure only skips the cursor glide.
    let other = element_click_placement(
        Err(anyhow::anyhow!("element exposes no Component interface")),
        Some(2),
    );
    assert!(matches!(other, Ok(None)));
    assert!(matches!(
        element_click_placement(Ok((2, 10.0, 20.0)), Some(2)),
        Ok(Some((2, x, y))) if x == 10.0 && y == 20.0
    ));
}

#[tokio::test]
async fn a_target_without_an_element_token_needs_no_membership_walk() {
    let none = ResolvedElement::<CachedElement>::None;
    assert!(refuse_unproven_window_element(1, &none, "click")
        .await
        .is_none());
}
