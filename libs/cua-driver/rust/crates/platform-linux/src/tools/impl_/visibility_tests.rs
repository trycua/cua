
use super::*;

#[test]
fn framed_elements_lead_and_keep_their_relative_order() {
    let elements = vec![
        json!({"element_index": 0, "role": "menu"}),
        json!({"element_index": 1, "role": "push button", "frame": {"x": 1, "y": 1, "w": 2, "h": 2}}),
        json!({"element_index": 2, "role": "menu"}),
        json!({"element_index": 3, "role": "paragraph", "frame": {"x": 1, "y": 9, "w": 2, "h": 2}}),
    ];
    let ordered: Vec<u64> = framed_elements_first(elements)
        .iter()
        .map(|e| e["element_index"].as_u64().unwrap())
        .collect();
    assert_eq!(ordered, vec![1, 3, 0, 2]);
}

#[test]
fn a_frameless_popup_returns_only_the_menu_entries_drawn_inside_it() {
    let elements = vec![
        json!({"element_index": 1, "role": "push button", "frame": {"x": 5, "y": 5, "w": 20, "h": 20}}),
        json!({"element_index": 2, "role": "menu item", "label": "Paragraph...", "frame": {"x": 2, "y": 30, "w": 180, "h": 22}}),
        json!({"element_index": 3, "role": "menu item", "label": "elsewhere", "frame": {"x": 400, "y": 30, "w": 180, "h": 22}}),
        json!({"element_index": 4, "role": "menu", "label": "Format"}),
    ];
    let kept: Vec<u64> = popup_menu_elements(elements.clone(), (0, 0), 200, 400)
        .iter()
        .map(|e| e["element_index"].as_u64().unwrap())
        .collect();
    assert_eq!(kept, vec![2]);
    assert_eq!(
        popup_menu_elements(elements[..1].to_vec(), (0, 0), 200, 400).len(),
        1
    );
}

#[test]
fn a_combo_popup_returns_the_rows_of_the_list_that_fills_it() {
    // Qt: the combo list lives under the combo box inside the dialog; the
    // main window's menubar entries are elsewhere on the screen.
    let elements = vec![
        json!({"element_index": 1, "role": "menu", "label": "Audio", "frame": {"x": -600, "y": -300, "w": 50, "h": 20}}),
        json!({"element_index": 7, "role": "list", "frame": {"x": 1, "y": 1, "w": 198, "h": 118}}),
        json!({"element_index": 8, "role": "list item", "label": "Desktop", "parent_index": 7, "frame": {"x": 2, "y": 2, "w": 190, "h": 22}}),
        json!({"element_index": 9, "role": "list item", "label": "user", "parent_index": 7, "frame": {"x": 2, "y": 26, "w": 190, "h": 22}}),
        json!({"element_index": 12, "role": "push button", "label": "Open", "frame": {"x": 20, "y": 20, "w": 60, "h": 20}}),
    ];
    let kept: Vec<u64> = popup_menu_elements(elements, (0, 0), 200, 120)
        .iter()
        .map(|e| e["element_index"].as_u64().unwrap())
        .collect();
    assert_eq!(kept, vec![8, 9]);
}

#[test]
fn a_list_popup_without_an_indexed_container_keeps_its_item_rows_only() {
    let elements = vec![
        json!({"element_index": 1, "role": "menu", "label": "Audio", "frame": {"x": 1, "y": 1, "w": 50, "h": 20}}),
        json!({"element_index": 8, "role": "list item", "label": "Desktop", "frame": {"x": 2, "y": 2, "w": 190, "h": 22}}),
        json!({"element_index": 9, "role": "tree item", "label": "user", "frame": {"x": 2, "y": 26, "w": 190, "h": 22}}),
        json!({"element_index": 12, "role": "push button", "label": "Open", "frame": {"x": 20, "y": 20, "w": 60, "h": 20}}),
    ];
    let kept: Vec<u64> = popup_menu_elements(elements, (0, 0), 200, 120)
        .iter()
        .map(|e| e["element_index"].as_u64().unwrap())
        .collect();
    assert_eq!(kept, vec![8, 9]);
}

#[test]
fn menu_roles_take_the_real_press() {
    for role in ["menu", "Menu Item", "check menu item", "radio menu item"] {
        assert!(element_is_menu_role(role), "{role}");
    }
    for role in ["menu bar", "push button", "text"] {
        assert!(!element_is_menu_role(role), "{role}");
    }
}

#[test]
fn overlays_name_the_follow_up_call_per_dialog_and_popup() {
    let overlays = WindowOverlays {
        dialogs: vec![json!({
            "window_id": 71, "title": "Position and Size", "transient_for": 3, "modal": true,
            "bounds": {"x": 100, "y": 100, "width": 400, "height": 300}
        })],
        popups: vec![json!({
            "window_id": 72, "title": "", "pid": 5,
            "bounds": {"x": 10, "y": 30, "width": 200, "height": 400}
        })],
        covers_window: true,
        window_rect: Some((0, 0, 800, 600)),
    };
    let note = overlays.follow_up(5).unwrap();
    assert!(
        note.contains("dialog \"Position and Size\" (window_id 71, transient of window 3, modal)"),
        "{note}"
    );
    assert!(
        note.contains("get_window_state(pid=5, window_id=71)"),
        "{note}"
    );
    assert!(
        note.contains("popup (window_id 72, bounds x=10 y=30 200x400)"),
        "{note}"
    );
    assert!(rects_intersect((0, 0, 800, 600), (100, 100, 400, 300)));
    assert!(!rects_intersect((0, 0, 800, 600), (800, 0, 10, 10)));
}

#[test]
fn a_cross_process_dialog_names_its_real_owning_pid_in_the_follow_up() {
    // GIMP's "Export Image as JPEG" dialog spawned as a separate process
    // (owning_pid 999) but is transient-for the target pid (5)'s window.
    let overlays = WindowOverlays {
        dialogs: vec![json!({
            "window_id": 90, "title": "Export Image as JPEG", "transient_for": 10,
            "modal": false,
            "bounds": {"x": 0, "y": 0, "width": 300, "height": 200},
            "owning_pid": 999,
        })],
        popups: vec![],
        covers_window: false,
        window_rect: Some((0, 0, 800, 600)),
    };
    let note = overlays.follow_up(5).unwrap();
    assert!(
        note.contains("owned by a different process pid 999, not pid 5"),
        "{note}"
    );
    assert!(
        note.contains("get_window_state(pid=5, window_id=90)"),
        "{note}"
    );
}
