use super::*;

#[test]
fn pid_only_pointer_call_is_refused_not_clicked_at_origin() {
    let refusal =
        require_point_args("click", &json!({"target_id": "s1:147", "pid": 1867})).expect("refused");
    assert_eq!(refusal.is_error, Some(true));
    let structured = refusal.structured_content.expect("structured");
    assert_eq!(structured["code"], "invalid_arguments");
    assert_eq!(structured["effect"], "refused");
    assert!(structured["accepted_forms"].as_array().unwrap().len() == 3);
}

#[test]
fn complete_point_passes() {
    assert!(require_point_args(
        "click",
        &json!({"pid": 1, "window_id": 2, "x": 3, "y": 4.5})
    )
    .is_none());
    assert!(require_point_args("click", &json!({"x": 1, "y": null})).is_some());
    assert!(require_point_args("click", &json!({"y": 1})).is_some());
}

#[test]
fn foreground_report_becomes_evidence() {
    let report = crate::input::ForegroundReport {
        already_active: false,
        retried_activation: false,
        confirm_ms: 12,
        focus_after: crate::input::FocusAfter::SamePid,
        window_change: Some("appeared: window 9 \"Brightness-Contrast\"".into()),
    };
    let v = foreground_structured("x11_xtest_fg", report, serde_json::Map::new());
    assert_eq!(v["effect"], "confirmed");
    assert_eq!(v["focus_after"], "same_pid");
    assert_eq!(v["evidence"][0]["kind"], "window_change");
    let quiet = crate::input::ForegroundReport {
        already_active: true,
        retried_activation: false,
        confirm_ms: 0,
        focus_after: crate::input::FocusAfter::Target,
        window_change: None,
    };
    let v = foreground_structured("x11_xtest_fg", quiet, serde_json::Map::new());
    assert_eq!(v["effect"], "unverifiable");
    assert_eq!(v["evidence"][0]["kind"], "native_api_result");
    let lost = crate::input::ForegroundReport {
        already_active: true,
        retried_activation: false,
        confirm_ms: 0,
        focus_after: crate::input::FocusAfter::Elsewhere,
        window_change: Some("closed: window 3".into()),
    };
    let v = foreground_structured("x11_xtest_fg", lost, serde_json::Map::new());
    assert_eq!(v["effect"], "suspected_noop");
}
