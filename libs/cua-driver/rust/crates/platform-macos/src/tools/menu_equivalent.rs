//! Find the menu item a key combination stands for.
//!
//! Background key events reach a window's views but never its app's menu key
//! equivalents (see `input::keyboard::hotkey_no_auth`). A background
//! `cmd+s` to TextEdit was reported as "Pressed" and saved nothing. When a
//! combo is a menu item's key equivalent, `hotkey` acts on that menu item
//! through accessibility instead, or refuses when macOS keeps the item
//! disabled in the background.

use core_foundation::base::{CFRelease, CFTypeRef};

use crate::ax::bindings::{
    copy_bool_attr, copy_children, copy_element_attr, copy_number_attr, copy_string_attr,
    AXUIElementCreateApplication, AXUIElementRef, AXUIElementSetMessagingTimeout,
};

/// `kAXMenuItemModifier*` bits of `AXMenuItemCmdModifiers`.
const MOD_SHIFT: i64 = 1 << 0;
const MOD_OPTION: i64 = 1 << 1;
const MOD_CONTROL: i64 = 1 << 2;
const MOD_NO_COMMAND: i64 = 1 << 3;

/// Upper bound on menu items read per lookup, so a huge dynamic menu cannot
/// stall the call.
const MAX_ITEMS: usize = 2000;

/// A hotkey as a menu key equivalent compares it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Combo {
    shift: bool,
    option: bool,
    control: bool,
    /// The uppercase character of the base key.
    character: String,
}

impl Combo {
    /// A combo with Command and one printable base key; `None` for anything
    /// else (no Command, a named key such as `return`), which keeps today's
    /// key-event route.
    pub(super) fn parse(modifiers: &[String], key: &str) -> Option<Self> {
        let mut cmd = false;
        let mut combo = Combo {
            shift: false,
            option: false,
            control: false,
            character: String::new(),
        };
        for modifier in modifiers {
            match modifier.to_ascii_lowercase().as_str() {
                "cmd" | "command" | "super" | "meta" | "win" => cmd = true,
                "shift" => combo.shift = true,
                "option" | "alt" => combo.option = true,
                "ctrl" | "control" => combo.control = true,
                _ => return None,
            }
        }
        let mut chars = key.chars();
        let (Some(ch), None) = (chars.next(), chars.next()) else {
            return None;
        };
        if !cmd || ch.is_whitespace() || ch.is_control() {
            return None;
        }
        combo.character = ch.to_uppercase().collect();
        Some(combo)
    }

    /// Whether a menu item with this `AXMenuItemCmdChar` and
    /// `AXMenuItemCmdModifiers` fires on this combo.
    fn matches(&self, cmd_char: &str, modifiers: i64) -> bool {
        modifiers & MOD_NO_COMMAND == 0
            && cmd_char.to_uppercase() == self.character
            && (modifiers & MOD_SHIFT != 0) == self.shift
            && (modifiers & MOD_OPTION != 0) == self.option
            && (modifiers & MOD_CONTROL != 0) == self.control
    }
}

/// A menu item found for a combo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MenuMatch {
    /// Titles from the menu bar item down to the item.
    pub path: Vec<String>,
    pub enabled: bool,
}

/// Find the item in `pid`'s menu bar whose key equivalent is `combo`. An
/// enabled match wins over a disabled one with the same equivalent. The
/// Apple menu is skipped: its commands belong to the system.
pub(super) fn find(pid: i32, combo: &Combo) -> Option<MenuMatch> {
    unsafe {
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return None;
        }
        let _ = AXUIElementSetMessagingTimeout(app, 1.0);
        let bar = copy_element_attr(app, "AXMenuBar");
        CFRelease(app as CFTypeRef);
        let bar = bar?;
        let mut found: Option<MenuMatch> = None;
        let mut budget = MAX_ITEMS;
        for (index, top) in semantic_items(bar).into_iter().enumerate() {
            let title = copy_string_attr(top, "AXTitle").unwrap_or_default();
            if index > 0 || title != "Apple" {
                search(top, vec![title], combo, &mut found, &mut budget, 0);
            }
            CFRelease(top as CFTypeRef);
            if found.as_ref().is_some_and(|m| m.enabled) {
                break;
            }
        }
        CFRelease(bar as CFTypeRef);
        found
    }
}

/// The items under a menu bar item or submenu item (through the untitled
/// `AXMenu` container AppKit puts between them).
unsafe fn semantic_items(parent: AXUIElementRef) -> Vec<AXUIElementRef> {
    let mut out = Vec::new();
    for child in copy_children(parent) {
        if copy_string_attr(child, "AXRole").as_deref() == Some("AXMenu") {
            out.extend(copy_children(child));
            CFRelease(child as CFTypeRef);
        } else {
            out.push(child);
        }
    }
    out
}

unsafe fn search(
    parent: AXUIElementRef,
    path: Vec<String>,
    combo: &Combo,
    found: &mut Option<MenuMatch>,
    budget: &mut usize,
    depth: usize,
) {
    for item in semantic_items(parent) {
        if *budget == 0 || found.as_ref().is_some_and(|m| m.enabled) {
            CFRelease(item as CFTypeRef);
            continue;
        }
        *budget -= 1;
        let title = copy_string_attr(item, "AXTitle").unwrap_or_default();
        let cmd_char = copy_string_attr(item, "AXMenuItemCmdChar").unwrap_or_default();
        if !cmd_char.is_empty() {
            let modifiers = copy_number_attr(item, "AXMenuItemCmdModifiers").unwrap_or(0.0) as i64;
            if combo.matches(&cmd_char, modifiers) {
                let enabled = copy_bool_attr(item, "AXEnabled") != Some(false);
                if found.is_none() || enabled {
                    let mut item_path = path.clone();
                    item_path.push(title.clone());
                    *found = Some(MenuMatch {
                        path: item_path,
                        enabled,
                    });
                }
            }
        } else if depth < 2 && !title.is_empty() {
            let mut sub_path = path.clone();
            sub_path.push(title);
            search(item, sub_path, combo, found, budget, depth + 1);
        }
        CFRelease(item as CFTypeRef);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn combo(modifiers: &[&str], key: &str) -> Option<Combo> {
        let modifiers: Vec<String> = modifiers.iter().map(|m| m.to_string()).collect();
        Combo::parse(&modifiers, key)
    }

    #[test]
    fn only_command_combos_with_one_printable_key_are_menu_candidates() {
        assert!(combo(&["cmd"], "s").is_some());
        assert!(combo(&["command", "shift"], "S").is_some());
        assert!(combo(&["cmd"], ",").is_some());
        assert_eq!(combo(&["ctrl"], "a"), None);
        assert_eq!(combo(&["cmd"], "return"), None);
        assert_eq!(combo(&["cmd"], "up"), None);
        assert_eq!(combo(&["cmd", "hyper"], "s"), None);
    }

    #[test]
    fn modifiers_must_match_exactly() {
        let save = combo(&["cmd"], "s").unwrap();
        assert!(save.matches("S", 0));
        assert!(save.matches("s", 0));
        assert!(!save.matches("S", MOD_SHIFT), "Save As is cmd+shift+s");
        assert!(!save.matches("S", MOD_NO_COMMAND));
        assert!(!save.matches("A", 0));
        let save_as = combo(&["cmd", "shift"], "s").unwrap();
        assert!(save_as.matches("S", MOD_SHIFT));
        assert!(!save_as.matches("S", 0));
        let minimize_all = combo(&["cmd", "option"], "m").unwrap();
        assert!(minimize_all.matches("M", MOD_OPTION));
        assert!(!minimize_all.matches("M", MOD_OPTION | MOD_CONTROL));
    }
}
