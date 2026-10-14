use super::*;
use crate::atspi::AtspiNode;

fn node(actions: Vec<String>) -> AtspiNode {
    AtspiNode {
        element_index: Some(1),
        role: "button".to_owned(),
        name: Some("ok".to_owned()),
        value: None,
        checked: None,
        enabled: Some(true),
        selected: None,
        description: None,
        actions,
        element_key: 1,
        identity: None,
        depth: 0,
        parent_element_index: None,
        in_web_content: false,
        object_ref: None,
    }
}

#[test]
fn element_entry_omits_actions_when_empty() {
    let n = node(Vec::new());
    let entry = build_element_entry(&n, None, None).unwrap();
    assert!(entry.get("actions").is_none());
}

#[test]
fn element_entry_filters_blank_action_names() {
    let n = node(vec![
        "Press".to_owned(),
        "".to_owned(),
        "   ".to_owned(),
        "Open".to_owned(),
    ]);
    let entry = build_element_entry(&n, None, None).unwrap();
    assert_eq!(entry["actions"], json!(["Press", "Open"]));
}
