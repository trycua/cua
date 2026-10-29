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
    let entry = build_element_entry(&n, None, None, false).unwrap();
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
    let entry = build_element_entry(&n, None, None, false).unwrap();
    assert_eq!(entry["actions"], json!(["Press", "Open"]));
}

#[test]
fn display_only_rows_appear_only_when_asked_for() {
    let mut caption = node(Vec::new());
    caption.element_index = None;
    caption.role = "label".to_owned();
    caption.name = Some("Book return time".to_owned());

    assert!(build_element_entry(&caption, None, None, false).is_none());

    let entry = build_element_entry(&caption, None, Some((10, 20, 80, 16)), true).unwrap();
    assert_eq!(entry["display_only"], json!(true));
    assert_eq!(entry["label"], json!("Book return time"));
    assert_eq!(entry["frame"], json!({"x": 10, "y": 20, "w": 80, "h": 16}));
    assert!(entry.get("element_index").is_none());
    assert!(entry.get("element_token").is_none());

    let entry = build_element_entry(&node(vec!["Press".to_owned()]), Some(7), None, true).unwrap();
    assert_eq!(entry["element_index"], json!(1));
    assert!(entry.get("display_only").is_none());
    assert!(entry.get("element_token").is_some());
}
