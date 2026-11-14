// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Short per-app notes carried on the first read of an app's window.
//!
//! Some apps behave in ways an agent otherwise rediscovers over many turns
//! (a toolkit that refuses background pixel clicks, a tree that is sparse, an
//! accessibility write the app ignores). The skill documents the routes that
//! work, but a model rarely opens it. This module keeps a small table keyed by
//! bundle id, app name or toolkit, and `get_window_state` attaches the matching
//! note once per app per session, so the working route arrives with the first
//! read. The table is per platform (`os`) so Linux and Windows can add entries.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

/// Operating system an entry applies to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    MacOs,
    Linux,
    Windows,
}

impl Os {
    /// The platform this binary was built for.
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Os::MacOs
        } else if cfg!(target_os = "windows") {
            Os::Windows
        } else {
            Os::Linux
        }
    }
}

/// One table entry.
pub struct AppNote {
    /// Stable id; the once-per-session marker is keyed on it, so every app
    /// that shares an entry (all LibreOffice modules) shares one note.
    pub id: &'static str,
    pub os: Os,
    /// Display name used in the `Notes for <name>:` lead.
    pub display: &'static str,
    /// Bundle id match: exact, or a prefix when the entry ends in `.`
    /// (case-insensitive).
    pub bundle_ids: &'static [&'static str],
    /// App name match: case-insensitive, the whole name or its first word.
    pub names: &'static [&'static str],
    /// 3-6 imperative sentences ending with the skill section to read.
    pub text: &'static str,
}

/// The identity of the app a window belongs to.
#[derive(Clone, Copy, Debug, Default)]
pub struct AppIdentity<'a> {
    pub bundle_id: Option<&'a str>,
    pub name: Option<&'a str>,
    /// Named MCP session, when the caller has one.
    pub session_id: Option<&'a str>,
}

pub static NOTES: &[AppNote] = &[
    AppNote {
        id: "libreoffice",
        os: Os::MacOs,
        display: "LibreOffice",
        bundle_ids: &["org.libreoffice."],
        names: &["libreoffice", "soffice"],
        text: "LibreOffice refuses background pixel clicks, so pass window_id and drive it with keys and menus: type_text and press_key post to that exact window. Run commands (Save, Paste Special, Format) with invoke_menu({pid, window_id, path}), not by clicking the menu bar. In Calc, go to a cell through the Name Box (the combo box named after the current cell): hotkey cmd+shift+t focuses it, hotkey cmd+a selects its text (without it the address is added to the old one), type_text the address alone, with no \\n, then a separate press_key return (a sheet-qualified address such as `$Sheet2.B7` also switches sheets). Return on text that is not an address names a range instead and puts focus back on the grid, so check the result's Active cell before typing values; then type values with \\n and \\t (an address typed while a cell is selected is written into that cell); accessibility value writes to cells are ignored (write_not_applied), and Calc lists only the visible cells. Sheet tabs are AXRadioButtons without actions: switch with cmd+pagedown / cmd+pageup or invoke_menu Sheet > Navigate > To Next Sheet (ctrl+pagedown does nothing on macOS). Clear the active cell with press_key forwarddelete (the Delete key); press_key delete is Backspace, which opens a Delete Contents dialog. See MACOS.md, \"LibreOffice, Numbers and other editors in the background\".",
    },
    AppNote {
        id: "gnucash",
        os: Os::MacOs,
        display: "GnuCash",
        bundle_ids: &["org.gnucash."],
        names: &["gnucash"],
        text: "GnuCash exposes a sparse accessibility tree, so work the registers by keyboard and read the result from screenshots. Pass window_id and use press_key / type_text: Tab moves between the fields of a transaction, Enter commits it, and arrow keys move between rows. Run menu commands with invoke_menu({pid, window_id, path}). Re-read with a screenshot after each commit to confirm the row landed. In a register the R column only toggles n (not cleared) and c (cleared) when clicked or given a key; y (reconciled) is set only through Actions > Reconcile: enter the statement's ending balance, tick each transaction in the Reconcile window, then Finish. See MACOS.md, \"LibreOffice, Numbers and other editors in the background\".",
    },
    AppNote {
        id: "electron",
        os: Os::MacOs,
        display: "this Electron app",
        bundle_ids: &[
            "com.microsoft.VSCode",
            "com.tinyspeck.slackmacgap",
            "com.hnc.Discord",
            "com.linear",
            "notion.id",
            "md.obsidian",
            "com.todesktop.",
        ],
        names: &["code", "slack", "discord", "linear", "notion", "obsidian"],
        text: "Electron apps render web content: act on rows by element_token (AX press) and set_value, and scroll by element rather than by pixel. The AX value can echo text that is not really in the editor, so type_text may report effect:\"unverifiable\": trust the screenshot, not the read-back. For a text field the tree cannot focus, call type_text({pid, window_id, x, y, text}) to focus and type in one call. Open a closed control (a search button) with an AX press first, then type. See MACOS.md, \"Known text-input limits (Catalyst + Electron)\".",
    },
    AppNote {
        id: "canvas-apps",
        os: Os::MacOs,
        display: "this viewport app",
        bundle_ids: &["org.blenderfoundation.blender", "com.unity3d."],
        names: &["blender", "unity"],
        text: "This app draws its main surface in one opaque viewport, so the tree has no useful rows and background pointer or key events are dropped. Look at the screenshot, and if the background route has no effect, retry the one needed action with delivery_mode:\"foreground\" (visible control; confirm authorization first), then verify from fresh state. See MACOS.md, \"Canvases, viewports, games\".",
    },
    AppNote {
        id: "preview",
        os: Os::MacOs,
        display: "Preview",
        bundle_ids: &["com.apple.Preview"],
        names: &["preview"],
        text: "Duplicate is disabled in a background Preview, so edit an export instead: File > Export through invoke_menu to the destination, then resize or edit the exported copy. Do not resize the original first. See MACOS.md, \"Preview: export before resizing\".",
    },
];

fn bundle_matches(entry: &str, bundle_id: &str) -> bool {
    let entry = entry.to_ascii_lowercase();
    let bundle = bundle_id.to_ascii_lowercase();
    if entry.ends_with('.') {
        bundle.starts_with(&entry)
    } else {
        bundle == entry
    }
}

fn name_matches(entry: &str, name: &str) -> bool {
    let name = name.trim().to_ascii_lowercase();
    name == entry
        || name
            .split(|c: char| !c.is_ascii_alphanumeric())
            .next()
            .is_some_and(|first| first == entry)
}

/// The table entry for an app on `os`. A bundle id match wins over a name match.
pub fn lookup(os: Os, bundle_id: Option<&str>, name: Option<&str>) -> Option<&'static AppNote> {
    let mut by_name = None;
    for note in NOTES.iter().filter(|n| n.os == os) {
        if let Some(bundle) = bundle_id.filter(|b| !b.is_empty()) {
            if note.bundle_ids.iter().any(|e| bundle_matches(e, bundle)) {
                return Some(note);
            }
        }
        if by_name.is_none() {
            if let Some(name) = name.filter(|n| !n.is_empty()) {
                if note.names.iter().any(|e| name_matches(e, name)) {
                    by_name = Some(note);
                }
            }
        }
    }
    by_name
}

fn shown() -> &'static Mutex<HashSet<(String, &'static str)>> {
    static SHOWN: OnceLock<Mutex<HashSet<(String, &'static str)>>> = OnceLock::new();
    SHOWN.get_or_init(|| Mutex::new(HashSet::new()))
}

/// The note text for this app if it has one that this session has not been
/// given yet, marking it as given. The text starts with `Notes for <app>:`.
pub fn take_first_read(os: Os, app: &AppIdentity<'_>) -> Option<String> {
    let note = lookup(os, app.bundle_id, app.name)?;
    let key = (app.session_id.unwrap_or_default().to_owned(), note.id);
    if !shown()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key)
    {
        return None;
    }
    let display = match note.id {
        "electron" | "canvas-apps" => app
            .name
            .filter(|n| !n.is_empty())
            .unwrap_or(note.display)
            .to_owned(),
        _ => note.display.to_owned(),
    };
    Some(format!("Notes for {display}: {}", note.text))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app<'a>(
        bundle: Option<&'a str>,
        name: Option<&'a str>,
        session: &'a str,
    ) -> AppIdentity<'a> {
        AppIdentity {
            bundle_id: bundle,
            name,
            session_id: Some(session),
        }
    }

    #[test]
    fn notes_appear_on_the_first_read_and_not_the_second() {
        let a = app(
            Some("org.libreoffice.script"),
            Some("LibreOffice"),
            "t-first",
        );
        let first = take_first_read(Os::MacOs, &a).expect("first read has notes");
        assert!(first.starts_with("Notes for LibreOffice: "));
        assert!(first.contains("Delete, not Backspace"));
        assert!(take_first_read(Os::MacOs, &a).is_none());
    }

    #[test]
    fn a_new_session_gets_the_note_again() {
        let one = app(None, Some("GnuCash"), "t-s1");
        let two = app(None, Some("GnuCash"), "t-s2");
        assert!(take_first_read(Os::MacOs, &one).is_some());
        assert!(take_first_read(Os::MacOs, &two).is_some());
        assert!(take_first_read(Os::MacOs, &two).is_none());
    }

    #[test]
    fn unknown_apps_get_no_notes() {
        let a = app(Some("com.apple.finder"), Some("Finder"), "t-unknown");
        assert!(take_first_read(Os::MacOs, &a).is_none());
        assert!(lookup(Os::MacOs, None, None).is_none());
        assert!(lookup(Os::MacOs, Some(""), Some("")).is_none());
    }

    #[test]
    fn matches_by_bundle_id_and_by_name() {
        assert_eq!(
            lookup(Os::MacOs, Some("org.libreoffice.script"), None).map(|n| n.id),
            Some("libreoffice")
        );
        assert_eq!(
            lookup(Os::MacOs, Some("ORG.GnuCash.GnuCash"), Some("x")).map(|n| n.id),
            Some("gnucash")
        );
        assert_eq!(
            lookup(Os::MacOs, None, Some("soffice")).map(|n| n.id),
            Some("libreoffice")
        );
        assert_eq!(
            lookup(Os::MacOs, None, Some("Gnucash")).map(|n| n.id),
            Some("gnucash")
        );
        assert_eq!(
            lookup(Os::MacOs, Some("com.tinyspeck.slackmacgap"), None).map(|n| n.id),
            Some("electron")
        );
        // A name must be the whole name or its first word, not any substring.
        assert!(lookup(Os::MacOs, None, Some("Decoder Ring")).is_none());
        assert!(lookup(Os::MacOs, None, Some("Xcode")).is_none());
    }

    #[test]
    fn the_table_is_platform_aware() {
        assert!(lookup(
            Os::Linux,
            Some("org.libreoffice.script"),
            Some("LibreOffice")
        )
        .is_none());
        assert!(lookup(Os::Windows, None, Some("gnucash")).is_none());
    }

    #[test]
    fn every_note_is_short_and_names_its_skill_section() {
        for note in NOTES {
            let sentences = note.text.matches(". ").count() + 1;
            assert!(sentences <= 6, "{} is too long", note.id);
            assert!(
                note.text.contains("MACOS.md"),
                "{} has no skill link",
                note.id
            );
        }
    }
}
