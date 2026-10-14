use super::{finish_pixel_uia_attempt, posted_pixel_click_result};
use crate::uia::windows_enum::PointInvokeOutcome;
use cua_driver_core::action_record::{
    ActionExecutionRecord, ActionTransport, ActualDelivery, RequestedDelivery,
};

#[test]
fn uia_unavailable_errors_advertise_foreground_escalation() {
    use cua_driver_core::action_record::EscalationKind;
    use cua_driver_core::protocol::Content;
    for (outcome, status) in [
        (PointInvokeOutcome::Busy, "busy"),
        (PointInvokeOutcome::Timeout, "timeout"),
        (PointInvokeOutcome::Unavailable, "unavailable"),
    ] {
        let result = finish_pixel_uia_attempt(outcome, 7, 3, 4)
            .expect("unavailable UIA must stop the route without fallback");
        assert!(result.is_error.unwrap_or(false), "{outcome:?}");
        let data = result
            .structured_content
            .as_ref()
            .expect("structured error");
        assert_eq!(data["code"], "background_unavailable");
        assert_eq!(data["uia_status"], status);
        assert_eq!(data["path"], "ax");
        assert_eq!(data["effect"], "unverifiable");
        assert_eq!(
            data["suggestion"].as_str(),
            Some("Retry this action with delivery_mode:\"foreground\"."),
            "{outcome:?}"
        );
        assert_eq!(
            data["escalation"]["recommended"].as_str(),
            Some("foreground"),
            "{outcome:?}"
        );
        let reason = data["escalation"]["reason"]
            .as_str()
            .expect("escalation reason");
        assert!(
            reason.contains(status),
            "reason must name the UIA status: {reason}"
        );
        assert!(
            reason.contains("delivery_mode:\"foreground\""),
            "reason must name the next rung: {reason}"
        );
        let text = match &result.content[0] {
            Content::Text { text, .. } => text,
            _ => panic!("expected text content for {outcome:?}"),
        };
        assert!(
            text.contains("No fallback input was sent"),
            "text must keep the no-replay guarantee: {text}"
        );
        assert!(
            text.contains("delivery_mode:\"foreground\""),
            "text must surface the escalation: {text}"
        );
        let record = ActionExecutionRecord::from_legacy(
            "click",
            &serde_json::json!({ "delivery_mode": "background" }),
            data,
        )
        .expect("UIA refusal should normalize into the public action contract");
        let public = serde_json::to_value(record.public_result().expect("valid ActionResult"))
            .expect("serialize ActionResult");
        assert_eq!(public["effect"], "unverifiable", "{outcome:?}");
    }
    // The completed-miss boundary is unchanged: only Miss falls through,
    // and a delivered Invoke carries no escalation hint.
    assert!(finish_pixel_uia_attempt(PointInvokeOutcome::Miss, 7, 3, 4).is_none());
    let ok = finish_pixel_uia_attempt(PointInvokeOutcome::Invoked, 7, 3, 4)
        .expect("invoked click reports success");
    assert!(!ok.is_error.unwrap_or(false));
    let ok_data = ok.structured_content.as_ref().expect("structured success");
    assert_eq!(ok_data["path"], "ax");
    assert!(ok_data.get("escalation").is_none());
    assert!(ok_data.get("suggestion").is_none());
    let ok_record = ActionExecutionRecord::from_legacy(
        "click",
        &serde_json::json!({ "delivery_mode": "background" }),
        ok_data,
    )
    .expect("UIA invoke should normalize into the public action contract");
    let ok_public = serde_json::to_value(ok_record.public_result().expect("valid ActionResult"))
        .expect("serialize ActionResult");
    assert_eq!(ok_public["effect"], "unverifiable");
    // The hint must flow into the existing escalation pipeline, not sit
    // as inert metadata: Timeout (the #3621 case) normalizes to a
    // foreground-delivery escalation on the internal record.
    let timeout_data = finish_pixel_uia_attempt(PointInvokeOutcome::Timeout, 7, 3, 4)
        .expect("timeout stops the route")
        .structured_content
        .expect("structured error");
    let timeout_record = ActionExecutionRecord::from_legacy(
        "click",
        &serde_json::json!({ "delivery_mode": "background" }),
        &timeout_data,
    )
    .expect("UIA timeout should normalize into the public action contract");
    assert_eq!(
        timeout_record.escalation.map(|escalation| escalation.kind),
        Some(EscalationKind::RetryWithForegroundDelivery)
    );
}

#[test]
fn post_message_pixel_click_reports_synthetic_transport() {
    let result = posted_pixel_click_result(42, "click");
    let structured = result
        .structured_content
        .as_ref()
        .expect("posted click should expose legacy transport metadata");
    assert_eq!(structured["path"], "post_message");

    let record = ActionExecutionRecord::from_legacy(
        "click",
        &serde_json::json!({ "delivery_mode": "background" }),
        structured,
    )
    .expect("posted click should normalize into the public action contract");
    assert_eq!(record.transport, ActionTransport::WindowsPostMessage);
    assert_eq!(record.requested_delivery, RequestedDelivery::Background);
    assert_eq!(record.actual_delivery, Some(ActualDelivery::Background));

    let public = serde_json::to_value(record.public_result().expect("valid ActionResult"))
        .expect("serialize ActionResult");
    assert_eq!(public["route"], "synthetic_events");
    assert_eq!(public["delivery"]["mode"], "background");
}
