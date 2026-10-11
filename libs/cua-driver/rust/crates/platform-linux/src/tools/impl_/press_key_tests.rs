use super::is_enter_key;
use super::press_key_chord;

#[test]
fn unmodified_press_stays_on_the_single_key_route() {
    assert_eq!(press_key_chord(&[], "return"), None);
}

#[test]
fn enter_key_gate_accepts_the_documented_enter_spelling() {
    assert!(is_enter_key("enter"));
    assert!(is_enter_key("Enter"));
    assert!(is_enter_key("ENTER"));
}

#[test]
fn enter_key_gate_accepts_the_documented_return_spelling() {
    // `key_name_to_keysym` resolves "return" and "enter" to the same
    // physical key (XK_Return, 0xFF0D); the pty short-circuit must not
    // drop a documented spelling on terminals (they discard synthetic
    // XSendEvent keys, so a dropped short-circuit loses the keypress).
    assert!(is_enter_key("return"));
    assert!(is_enter_key("Return"));
    assert!(is_enter_key("RETURN"));
}

#[test]
fn enter_key_gate_rejects_non_enter_keys() {
    assert!(!is_enter_key("tab"));
    assert!(!is_enter_key("escape"));
    assert!(!is_enter_key("space"));
    assert!(!is_enter_key(""));
}

#[test]
fn modifiers_are_promoted_to_a_chord_in_order() {
    assert_eq!(
        press_key_chord(&["ctrl".to_owned()], "s"),
        Some(vec!["ctrl".to_owned(), "s".to_owned()])
    );
    assert_eq!(
        press_key_chord(&["ctrl".to_owned(), "shift".to_owned()], "t"),
        Some(vec!["ctrl".to_owned(), "shift".to_owned(), "t".to_owned()])
    );
}
