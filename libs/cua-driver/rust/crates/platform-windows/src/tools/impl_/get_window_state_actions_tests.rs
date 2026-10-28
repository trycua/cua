use super::*;
use crate::uia::UiaNode;

fn node(actions: Vec<String>) -> UiaNode {
    UiaNode {
        element_index: Some(1),
        control_type: "Button".to_owned(),
        name: Some("OK".to_owned()),
        value: None,
        automation_id: None,
        help_text: None,
        actions,
        enabled: Some(true),
        selected: None,
        checked: None,
        expanded: None,
        element_ptr: 0,
        center_x: 0,
        center_y: 0,
        rect: None,
        msaa_role: None,
        depth: 0,
        parent_element_index: None,
        in_web_content: false,
    }
}

#[test]
fn element_entry_includes_actions_when_present() {
    let n = node(vec!["invoke".to_owned(), "toggle".to_owned()]);
    let entry = build_element_entry(&n, None).unwrap();
    assert_eq!(entry["actions"], json!(["invoke", "toggle"]));
}

#[test]
fn element_entry_omits_actions_when_empty() {
    let n = node(Vec::new());
    let entry = build_element_entry(&n, None).unwrap();
    assert!(entry.get("actions").is_none());
}

#[test]
fn element_entry_reports_toggle_state_including_mixed() {
    for (state, expected) in [
        (crate::uia::CheckedState::On, json!("on")),
        (crate::uia::CheckedState::Off, json!("off")),
        (crate::uia::CheckedState::Mixed, json!("mixed")),
    ] {
        let mut n = node(vec!["toggle".to_owned()]);
        n.checked = Some(state);
        let entry = build_element_entry(&n, None).unwrap();
        assert_eq!(entry["toggle_state"], expected);
    }
}

#[test]
fn element_entry_reports_expanded_and_collapsed() {
    for expanded in [true, false] {
        let mut n = node(vec!["expand".to_owned()]);
        n.expanded = Some(expanded);
        let entry = build_element_entry(&n, None).unwrap();
        assert_eq!(entry["expanded"], json!(expanded));
    }
}

#[test]
fn element_entry_omits_toggle_state_and_expanded_when_unknown() {
    let n = node(vec!["invoke".to_owned()]);
    let entry = build_element_entry(&n, None).unwrap();
    assert!(entry.get("toggle_state").is_none());
    assert!(entry.get("expanded").is_none());
}

#[test]
fn tree_row_changes_when_only_toggle_or_expanded_state_changes() {
    let mut off = node(vec!["toggle".to_owned()]);
    off.checked = Some(crate::uia::CheckedState::Off);
    let mut mixed = off.clone();
    mixed.checked = Some(crate::uia::CheckedState::Mixed);
    assert_eq!(
        crate::uia::format_node_line(&off),
        "- [1] Button \"OK\" [toggle=off actions=[toggle]]"
    );
    assert_ne!(
        crate::uia::format_node_line(&off),
        crate::uia::format_node_line(&mixed)
    );

    let mut collapsed = node(vec!["expand".to_owned()]);
    collapsed.expanded = Some(false);
    let mut expanded = collapsed.clone();
    expanded.expanded = Some(true);
    assert_ne!(
        crate::uia::format_node_line(&collapsed),
        crate::uia::format_node_line(&expanded)
    );
}
