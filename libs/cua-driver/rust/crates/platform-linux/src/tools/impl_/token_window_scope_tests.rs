use super::{
    element_click_placement, prove_token_element, stale_observed_element, token_element_for,
};
use crate::atspi::native::{window_element_membership, ObjectRef};
use crate::atspi::snapshot::{cached_element, forget_window, update_snapshot, CachedElement};
use crate::atspi::{AtspiIdentity, AtspiNode, Snapshots, TokenElement, WindowProof};
use cua_driver_core::element_token::{token_for, ResolvedElement};
use cua_driver_core::protocol::ToolResult;

// One process owns windows A and B; each has a text field at the same index.
const PID: u32 = 515_151;
const WINDOW_A: u64 = 1;
const WINDOW_B: u64 = 2;
const INDEX: usize = 3;

fn identity(path: &str) -> AtspiIdentity {
    AtspiIdentity {
        bus_name: ":1.42".into(),
        path: path.into(),
        frame_bus_name: ":1.42".into(),
        frame_path: "/frame".into(),
    }
}

fn object_ref(path: &str) -> Option<ObjectRef> {
    Some(ObjectRef {
        bus: ":1.42".into(),
        path: path.into(),
    })
}

fn observed(path: &str) -> CachedElement {
    CachedElement {
        key: 0,
        identity: identity(path),
        object_ref: object_ref(path),
        role: "text".into(),
        in_web_content: false,
        bounds: None,
    }
}

fn text_field(path: &str) -> AtspiNode {
    AtspiNode {
        element_index: Some(INDEX),
        role: "text".into(),
        name: None,
        value: None,
        checked: None,
        enabled: Some(true),
        selected: None,
        description: None,
        actions: Vec::new(),
        element_key: 0,
        identity: Some(identity(path)),
        depth: 0,
        parent_element_index: None,
        in_web_content: false,
        object_ref: object_ref(path),
    }
}

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
    let proven = |membership: anyhow::Result<()>| membership.map(|()| WindowProof::Proven);
    let unproven = proven(window_element_membership(Some(0), None, || false));
    let refusal = token_element_for("click", 2, 3, observed("/a"), unproven).expect_err("refused");
    assert_eq!(refusal_code(&refusal), "window_scope_unproven");
    let gone = proven(window_element_membership(None, Some(0), || true));
    let refusal = token_element_for("type_text", 2, 3, observed("/a"), gone).expect_err("refused");
    assert_eq!(refusal_code(&refusal), "window_scope_unproven");
    assert!(token_element_for("click", 2, 3, observed("/a"), Ok(WindowProof::Proven)).is_ok());
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
    assert!(matches!(
        prove_token_element(1, &none, "click").await,
        Ok(None)
    ));
}

#[test]
fn a_proven_token_dispatches_to_its_own_element_while_another_window_is_retained() {
    // Both windows' snapshots are retained: in the runtime store and in the
    // (pid, window) side index.
    let snapshots = Snapshots::new();
    let tokens =
        [(WINDOW_A, "/window_a/text"), (WINDOW_B, "/window_b/text")].map(|(window, path)| {
            let payload = update_snapshot(PID, window, &[text_field(path)], &[]);
            (snapshots.publish(PID as i32, window, payload), window, path)
        });
    let mut pid_only = Vec::new();
    for (snapshot, window, path) in tokens {
        let args = serde_json::json!({ "element_token": token_for(snapshot, INDEX) });
        let Ok(ResolvedElement::Element {
            window_id,
            element_index,
            element,
        }) = snapshots.resolve(PID as i32, &args)
        else {
            panic!("the token for window {window} resolves to its element");
        };
        assert_eq!(window_id, window);
        // The fresh walk scoped to the window finds the observed identity.
        let proof =
            window_element_membership(Some(0), Some(0), || true).map(|()| WindowProof::Proven);
        let token = token_element_for("type_text", window_id, element_index, element, proof)
            .expect("membership proven");
        // Text, focus, value and scroll dispatch address this object.
        assert_eq!(token.object().expect("addressable").path, path);
        assert!(!token.may_reresolve, "a proven token never re-resolves");
        pid_only.push(cached_element(PID, None, INDEX).and_then(|element| element.object_ref));
    }
    // The PID-only lookup the earlier dispatch used gives both tokens the same
    // window's element, so it acted on the wrong window for one of them.
    assert_eq!(pid_only[0], pid_only[1]);
    forget_window(PID, WINDOW_A);
    forget_window(PID, WINDOW_B);
}

#[test]
fn only_an_unproven_session_may_re_resolve_a_token_index() {
    let proven = TokenElement::new(INDEX, observed("/window_b/text"), WindowProof::Proven);
    assert!(!proven.may_reresolve);
    let unproven = TokenElement::new(INDEX, observed("/window_b/text"), WindowProof::NotRequired);
    assert!(unproven.may_reresolve);
}

#[test]
fn a_gone_observed_object_refuses_as_a_stale_token() {
    let mut gone = observed("/window_b/text");
    gone.object_ref = None;
    let token = TokenElement::new(INDEX, gone, WindowProof::Proven);
    let error = token.object().expect_err("no D-Bus address");
    let refusal = stale_observed_element("type_text", INDEX, &error).expect("stale");
    assert_eq!(
        refusal.structured_content.expect("structured")["code"],
        "stale_element_token"
    );
    // A write the toolkit rejected is not a stale token.
    let rejected = anyhow::anyhow!("EditableText write was rejected");
    assert!(stale_observed_element("type_text", INDEX, &rejected).is_none());
}
