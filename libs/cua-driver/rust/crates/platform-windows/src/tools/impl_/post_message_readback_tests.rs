use super::{changed_contains_post_message_result, post_message_readback_observed};

#[test]
fn observes_only_a_changed_value_containing_the_requested_text() {
    assert!(post_message_readback_observed(
        Some("prefix"),
        Some("prefixhello"),
        "hello"
    ));
    assert!(!post_message_readback_observed(None, None, "hello"));
    assert!(!post_message_readback_observed(
        Some("hello"),
        Some("hello"),
        "hello"
    ));
    assert!(!post_message_readback_observed(
        Some(""),
        Some("h"),
        "hello"
    ));
    assert!(!post_message_readback_observed(
        Some("before"),
        Some("different"),
        "hello"
    ));
}

#[test]
fn changed_contains_remains_unverifiable_without_retry_escalation() {
    let result = changed_contains_post_message_result(5);
    assert_eq!(result["effect"], "unverifiable");
    assert_eq!(result["verify"], "changed_contains");
    assert_eq!(result["verified"], false);
    assert!(result.get("escalation").is_none());

    let record = cua_driver_core::action_record::ActionExecutionRecord::from_legacy(
        "type_text",
        &serde_json::json!({"delivery_mode": "background"}),
        &result,
    )
    .expect("changed-and-contains PostMessage result should normalize");
    let public = serde_json::to_value(record.public_result().expect("valid ActionResult"))
        .expect("serialize ActionResult");
    assert_eq!(public["effect"], "unverifiable");
    assert!(public.get("escalation").is_none());
    assert!(public.get("evidence").is_none());
}
