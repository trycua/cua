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
        is_password: None,
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
fn element_entry_includes_is_password_when_known() {
    let mut n = node(vec!["set_value".to_owned()]);
    n.is_password = Some(true);
    let entry = build_element_entry(&n, None).unwrap();
    assert_eq!(entry["is_password"], json!(true));

    n.is_password = Some(false);
    let entry = build_element_entry(&n, None).unwrap();
    assert_eq!(entry["is_password"], json!(false));
}

#[test]
fn element_entry_omits_is_password_when_unknown() {
    let n = node(vec!["invoke".to_owned()]);
    let entry = build_element_entry(&n, None).unwrap();
    assert!(entry.get("is_password").is_none());
}

#[test]
fn is_password_is_kept_for_text_entry_and_true_values_only() {
    use crate::uia::text_entry_is_password;
    let text = vec!["set_value".to_owned()];
    let typed = vec!["text".to_owned()];
    let button = vec!["invoke".to_owned()];
    assert_eq!(text_entry_is_password(Some(true), &button), Some(true));
    assert_eq!(text_entry_is_password(Some(true), &text), Some(true));
    assert_eq!(text_entry_is_password(Some(false), &text), Some(false));
    assert_eq!(text_entry_is_password(Some(false), &typed), Some(false));
    assert_eq!(text_entry_is_password(Some(false), &button), None);
    assert_eq!(text_entry_is_password(None, &text), None);
}

#[test]
fn markdown_line_marks_password_fields_only() {
    use crate::uia::format_node_line;
    let mut n = node(vec!["set_value".to_owned()]);
    n.control_type = "Edit".to_owned();
    n.name = Some("Password".to_owned());
    n.is_password = Some(true);
    assert_eq!(
        format_node_line(&n),
        "- [1] Edit \"Password\" [password actions=[set_value]]"
    );
    n.is_password = Some(false);
    assert_eq!(
        format_node_line(&n),
        "- [1] Edit \"Password\" [actions=[set_value]]"
    );
}

#[test]
fn password_field_value_is_never_echoed() {
    use crate::uia::redact_password_value;
    let secret = Some("hunter2".to_owned());
    assert_eq!(redact_password_value(secret.clone(), Some(true)), None);
    assert_eq!(redact_password_value(secret.clone(), Some(false)), secret);
    assert_eq!(redact_password_value(secret.clone(), None), secret);
}
