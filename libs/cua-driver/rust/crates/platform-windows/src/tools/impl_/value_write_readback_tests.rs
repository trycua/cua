
use super::{classify_value_write_readback, value_write_structured_result};

#[test]
fn confirms_when_the_expected_value_replaces_the_prior_value() {
    assert_eq!(
        classify_value_write_readback(Some("beforeinserted"), "before", "beforeinserted"),
        "confirmed"
    );
}

#[test]
fn treats_stale_or_unreadable_value_as_pending() {
    assert_eq!(
        classify_value_write_readback(Some("old value"), "old value", "old valueinserted"),
        "pending"
    );
    assert_eq!(
        classify_value_write_readback(None, "old value", "old valueinserted"),
        "pending"
    );
}

#[test]
fn deferred_publication_remains_unverifiable_without_retry_escalation() {
    let result = value_write_structured_result(8, "pending", false);
    assert_eq!(result["effect"], "unverifiable");
    assert_eq!(result["verify"], "pending");
    assert_eq!(result["verified"], false);
    assert!(result.get("escalation").is_none());

    let record = cua_driver_core::action_record::ActionExecutionRecord::from_legacy(
        "type_text",
        &serde_json::json!({"delivery_mode": "background"}),
        &result,
    )
    .expect("deferred ValuePattern result should normalize");
    let public = serde_json::to_value(record.public_result().expect("valid ActionResult"))
        .expect("serialize ActionResult");
    assert_eq!(public["effect"], "unverifiable");
    assert!(public.get("escalation").is_none());
    assert!(public.get("evidence").is_none());
}

#[test]
fn does_not_confirm_when_stale_value_contains_the_typed_text() {
    assert_eq!(
        classify_value_write_readback(Some("10.00"), "10.00", "10.000"),
        "pending"
    );
}

#[test]
fn does_not_confirm_an_empty_write() {
    assert_eq!(
        classify_value_write_readback(Some("unchanged"), "unchanged", "unchanged"),
        "pending"
    );
}
