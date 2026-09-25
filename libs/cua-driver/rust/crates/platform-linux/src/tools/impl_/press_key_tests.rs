
use super::press_key_chord;

#[test]
fn unmodified_press_stays_on_the_single_key_route() {
    assert_eq!(press_key_chord(&[], "return"), None);
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
