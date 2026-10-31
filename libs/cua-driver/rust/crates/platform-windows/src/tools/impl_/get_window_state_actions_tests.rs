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
    let entry = build_element_entry(&n, None, false).unwrap();
    assert_eq!(entry["actions"], json!(["invoke", "toggle"]));
}

#[test]
fn element_entry_omits_actions_when_empty() {
    let n = node(Vec::new());
    let entry = build_element_entry(&n, None, false).unwrap();
    assert!(entry.get("actions").is_none());
}

#[test]
fn display_only_rows_appear_only_when_asked_for_with_their_automation_id() {
    // A caption and a staff-ID label (no element_index) beside a date field.
    let mut caption = node(Vec::new());
    caption.element_index = None;
    caption.control_type = "Text".to_owned();
    caption.name = Some("Book return time".to_owned());
    caption.automation_id = Some("lblReturnCaption".to_owned());
    let mut shown = caption.clone();
    shown.name = Some("RP-20417".to_owned());
    shown.automation_id = Some("lblAgent".to_owned());
    let mut field = node(vec!["set_value".to_owned()]);
    field.element_index = Some(3);
    field.control_type = "Edit".to_owned();
    field.name = Some("Book return time".to_owned());
    field.value = Some("10/29/2026 10:15 AM".to_owned());
    field.automation_id = Some("dtReturn".to_owned());

    // Without `include_display_only`: the label rows are omitted, as before.
    assert!(build_element_entry(&caption, Some(7), false).is_none());
    assert!(build_element_entry(&shown, Some(7), false).is_none());
    let field_entry = build_element_entry(&field, Some(7), false).unwrap();
    assert_eq!(field_entry["element_index"], json!(3));
    assert_eq!(field_entry["automation_id"], json!("dtReturn"));
    assert!(field_entry.get("display_only").is_none());

    // With it: the label rows are present, identifiable, and not addressable.
    let shown_entry = build_element_entry(&shown, Some(7), true).unwrap();
    assert_eq!(shown_entry["display_only"], json!(true));
    assert_eq!(shown_entry["role"], json!("Text"));
    assert_eq!(shown_entry["label"], json!("RP-20417"));
    assert_eq!(shown_entry["automation_id"], json!("lblAgent"));
    assert!(shown_entry.get("element_index").is_none());
    assert!(shown_entry.get("element_token").is_none());
    assert_eq!(
        build_element_entry(&field, Some(7), true).unwrap(),
        field_entry
    );
}
