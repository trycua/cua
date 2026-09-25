
use super::{background_element_click_result, ActionTransport};

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
