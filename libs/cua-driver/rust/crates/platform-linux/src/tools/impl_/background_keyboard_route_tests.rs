use super::{element_needs_real_click, split_key_combo, wm_chord_kind, WmChord};

#[test]
fn key_combos_split_into_modifiers_and_key() {
    assert_eq!(
        split_key_combo("alt+F4"),
        (vec!["alt".to_owned()], "F4".to_owned())
    );
    assert_eq!(
        split_key_combo("ctrl+shift+t"),
        (vec!["ctrl".to_owned(), "shift".to_owned()], "t".to_owned())
    );
    assert_eq!(split_key_combo("+"), (vec![], "+".to_owned()));
    assert_eq!(
        split_key_combo("ctrl++"),
        (vec!["ctrl".to_owned()], "+".to_owned())
    );
    assert_eq!(split_key_combo("Return"), (vec![], "Return".to_owned()));
    // Not a modifier prefix: left untouched for the keysym resolver.
    assert_eq!(split_key_combo("a+b"), (vec![], "a+b".to_owned()));
}

#[test]
fn wm_chords_are_recognised() {
    let m = |xs: &[&str]| xs.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    assert_eq!(
        wm_chord_kind("F4", &m(&["alt"])),
        Some(WmChord::CloseWindow)
    );
    assert_eq!(
        wm_chord_kind("f4", &m(&["Alt_L"])),
        Some(WmChord::CloseWindow)
    );
    assert_eq!(
        wm_chord_kind("Tab", &m(&["alt"])),
        Some(WmChord::Unavailable)
    );
    assert_eq!(
        wm_chord_kind("Tab", &m(&["alt", "shift"])),
        Some(WmChord::Unavailable)
    );
    assert_eq!(
        wm_chord_kind("a", &m(&["super"])),
        Some(WmChord::Unavailable)
    );
    assert_eq!(
        wm_chord_kind("t", &m(&["ctrl", "alt"])),
        Some(WmChord::Unavailable)
    );
    assert_eq!(
        wm_chord_kind("F4", &m(&["ctrl", "alt"])),
        Some(WmChord::Unavailable)
    );
    // Application chords stay with the application.
    assert_eq!(wm_chord_kind("c", &m(&["ctrl"])), None);
    assert_eq!(wm_chord_kind("F4", &m(&["ctrl"])), None);
    assert_eq!(wm_chord_kind("f", &m(&["alt"])), None);
    assert_eq!(wm_chord_kind("F4", &m(&[])), None);
    assert_eq!(wm_chord_kind("s", &m(&["ctrl", "shift"])), None);
}

#[test]
fn focus_taking_roles_need_a_real_click() {
    for role in ["spin button", "Text", "entry", "slider", "combo box"] {
        assert!(element_needs_real_click(role), "{role}");
    }
    for role in ["menu item", "push button", "menu", "check box", "page tab"] {
        assert!(!element_needs_real_click(role), "{role}");
    }
}
