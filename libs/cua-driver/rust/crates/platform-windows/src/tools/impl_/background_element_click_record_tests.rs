use super::{background_element_click_result, ActionTransport};

#[test]
fn semantic_deadline_stops_the_route_and_preserves_unknown_effect() {
    use super::element_uia_unavailable_result;
    use crate::uia::windows_enum::UiaDeadlineError;
    use cua_driver_core::action_record::{ActionEffect, ActionExecutionRecord};
    for (error, status, effect) in [
        (UiaDeadlineError::Busy, "busy", ActionEffect::Refused),
        (
            UiaDeadlineError::Timeout,
            "timeout",
            ActionEffect::Unverifiable,
        ),
        (
            UiaDeadlineError::Unavailable,
            "unavailable",
            ActionEffect::Unverifiable,
        ),
    ] {
        let mut result = element_uia_unavailable_result(error);
        cua_driver_core::recovery_hint::annotate(
            "click",
            &cua_driver_core::recovery_hint::Seed::from_args(&serde_json::json!({
                "pid": 42, "window_id": 7, "delivery_mode": "background"
            })),
            &mut result,
        );
        assert_eq!(result.is_error, Some(true));
        let data = result.structured_content.as_ref().unwrap();
        assert_eq!(data["code"], "background_unavailable");
        assert_eq!(data["uia_status"], status);
        assert_eq!(data["path"], "ax");
        assert_eq!(data["verified"], false);
        let hint = data["hint"].as_str().expect("public recovery hint");
        assert!(
            hint.contains("Do not replay this click"),
            "{status}: {hint}"
        );
        assert!(!hint.contains("foreground"), "{status}: {hint}");
        assert_eq!(data["next_call"]["tool"], "get_window_state");
        let record = ActionExecutionRecord::from_legacy(
            "click",
            &serde_json::json!({"delivery_mode": "background"}),
            data,
        )
        .expect("existing ActionResult contract");
        assert_eq!(record.effect, effect);
        record.public_result().expect("valid public projection");
        assert!(record.fallbacks.is_empty());
        assert!(
            record.escalation.is_none(),
            "unknown effects must not recommend immediate replay"
        );
    }
}

#[test]
fn raw_fallback_reports_its_physical_transport() {
    for (transport, path) in [
        (ActionTransport::WindowsTargetedInjection, "pixel"),
        (ActionTransport::WindowsPostMessage, "post_message"),
    ] {
        let result = background_element_click_result(
            "physical fallback".into(),
            transport,
            vec![ActionTransport::WindowsUiaInvoke],
        );
        assert_eq!(result.structured_content.as_ref().unwrap()["path"], path);
        let record = result.action_record.unwrap();
        assert_eq!(record.transport, transport);
        let truth = record.debug_json();
        assert_ne!(truth["route"], "accessibility");
        assert_eq!(
            record.attempts[0].transport,
            ActionTransport::WindowsUiaInvoke
        );
        assert_eq!(record.fallbacks[0].to, transport);
    }
}

#[test]
fn failed_provider_calls_disqualify_single_action_marker_exemption() {
    let result = background_element_click_result(
        "semantic success after provider failure".into(),
        ActionTransport::WindowsUiaToggle,
        vec![ActionTransport::WindowsUiaInvoke],
    );
    let record = result.action_record.unwrap();
    assert_eq!(record.transport, ActionTransport::WindowsUiaToggle);
    assert_eq!(record.attempts.len(), 1);
    assert_eq!(record.fallbacks.len(), 1);
    assert_eq!(record.fallbacks[0].from, ActionTransport::WindowsUiaInvoke);
    assert_eq!(record.fallbacks[0].to, ActionTransport::WindowsUiaToggle);
    // Point-free recording requires an empty fallback journal.
    assert!(!record.debug_json()["fallbacks"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn single_provider_success_has_no_invented_attempts() {
    for transport in [
        ActionTransport::WindowsUiaInvoke,
        ActionTransport::WindowsUiaToggle,
        ActionTransport::WindowsUiaSelection,
        ActionTransport::WindowsUiaExpandCollapse,
    ] {
        let result = background_element_click_result("semantic success".into(), transport, vec![]);
        assert_eq!(result.structured_content.as_ref().unwrap()["path"], "ax");
        let record = result.action_record.unwrap();
        assert_eq!(record.transport, transport);
        assert!(record.attempts.is_empty());
        assert!(record.fallbacks.is_empty());
    }
}
