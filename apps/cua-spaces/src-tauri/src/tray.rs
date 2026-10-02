// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The menu bar item: a monochrome template image of the Cua Spaces mark,
//! the Cua logo in three stacked rounded squares (it follows light/dark and
//! the highlight), like other menu bar apps.
//!
//! A left click opens the main window (shows and focuses it). A right click
//! (or control-click) shows a small native menu: the app core's
//! (`window::menu`: a status line with the Spaces the user can open and
//! Cua Volume's sync state, Cua Volume's conflicts when there are any, Open
//! Cua Spaces, New Space…, Settings… and Quit), the same menu the SwiftUI
//! app's menu bar extra draws. The notch panel computes the items from the
//! same roster its tab counts (`tray_set_menu`), so both say the same
//! number. Nothing here opens the notch panel.

use std::sync::Mutex;

use cua_spaces_app_core::window::{self as core_menu, MenuItemId};
use tauri::image::Image;
use tauri::menu::{MenuBuilder, MenuItem, MenuItemBuilder, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};

use crate::viewer_windows;

/// The tray's id (one tray per app).
pub const TRAY_ID: &str = "cua-spaces";

/// 36 x 36 px (18 pt @2x) black-on-transparent Cua Spaces mark, used as a
/// template (source: icons/tray-template.svg, scripts/icons/gen-icon-svgs.py).
const TRAY_ICON_2X: &[u8] = include_bytes!("../icons/tray-template@2x.png");

/// Menu item ids (one per core [`MenuItemId`]).
pub mod ids {
    pub const STATUS: &str = "tray-status";
    pub const OPEN: &str = "tray-open";
    pub const NEW_SPACE: &str = "tray-new-space";
    pub const SETTINGS: &str = "tray-settings";
    pub const QUIT: &str = "tray-quit";
    pub const VOLUME_CONFLICTS: &str = "tray-volume-conflicts";
}

/// What a menu item does, by id; `None` for the (disabled) status line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    OpenMain,
    NewSpace,
    Settings,
    /// The main window on the Volume page (Cua Volume's conflicts).
    Volume,
    Quit,
}

pub fn action_for(id: &str) -> Option<TrayAction> {
    match id {
        ids::OPEN => Some(TrayAction::OpenMain),
        ids::NEW_SPACE => Some(TrayAction::NewSpace),
        ids::SETTINGS => Some(TrayAction::Settings),
        ids::VOLUME_CONFLICTS => Some(TrayAction::Volume),
        ids::QUIT => Some(TrayAction::Quit),
        _ => None,
    }
}

/// Whether a tray event is the plain click that opens the main window.
pub fn opens_main(event: &TrayIconEvent) -> bool {
    matches!(
        event,
        TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        }
    )
}

/// The status line, e.g. "3 Spaces" / "No Spaces" (the app core's words).
pub fn status_text(spaces: usize) -> String {
    cua_spaces_app_core::spaces::status_line(spaces as u32)
}

/// The menu item id for a core item (`None` for a separator).
pub fn id_for(item: MenuItemId) -> Option<&'static str> {
    match item {
        MenuItemId::Status => Some(ids::STATUS),
        MenuItemId::Open => Some(ids::OPEN),
        MenuItemId::NewSpace => Some(ids::NEW_SPACE),
        MenuItemId::Settings => Some(ids::SETTINGS),
        MenuItemId::Quit => Some(ids::QUIT),
        MenuItemId::VolumeConflicts => Some(ids::VOLUME_CONFLICTS),
        MenuItemId::Separator => None,
    }
}

/// A core shortcut (`⌘,`) as a Tauri accelerator (`CmdOrCtrl+,`).
pub fn accelerator(shortcut: &str) -> Option<String> {
    let key = shortcut.strip_prefix('\u{2318}')?;
    (!key.is_empty()).then(|| format!("CmdOrCtrl+{key}"))
}

/// Keeps the status line so `list_spaces` can set it before the notch
/// panel first reports the core's menu.
pub struct TrayStatus(pub Mutex<Option<MenuItem<Wry>>>);

/// The menu, kept so captures can show it as a context menu, and the core
/// items it was built from.
pub struct TrayMenu(pub Mutex<(tauri::menu::Menu<Wry>, Vec<core_menu::MenuItem>)>);

impl TrayMenu {
    /// The menu as it is now.
    pub fn menu(&self) -> Option<tauri::menu::Menu<Wry>> {
        self.0.lock().ok().map(|m| m.0.clone())
    }
}

/// The template icon, decoded.
pub fn icon() -> tauri::Result<Image<'static>> {
    Image::from_bytes(TRAY_ICON_2X)
}

pub fn run_action(app: &AppHandle, action: TrayAction) {
    let result = match action {
        TrayAction::OpenMain => viewer_windows::show_main_window(app, None),
        TrayAction::NewSpace => viewer_windows::open_new_space(app.clone()),
        TrayAction::Settings => viewer_windows::open_main_settings(app.clone()),
        TrayAction::Volume => viewer_windows::open_main_volume(app.clone()),
        TrayAction::Quit => {
            app.exit(0);
            Ok(())
        }
    };
    if let Err(error) = result {
        eprintln!("[cua-spaces] menu bar action {action:?} failed: {error}");
    }
}

/// The native menu for the core's items, and its first status line.
fn build_menu(
    app: &AppHandle,
    items: &[core_menu::MenuItem],
) -> tauri::Result<(tauri::menu::Menu<Wry>, Option<MenuItem<Wry>>)> {
    let mut builder = MenuBuilder::new(app);
    let mut status = None;
    for (i, item) in items.iter().enumerate() {
        let Some(id) = id_for(item.id) else {
            builder = builder.item(&PredefinedMenuItem::separator(app)?);
            continue;
        };
        // Item ids are unique per menu: a second status line gets its own.
        let id = if item.id == MenuItemId::Status && status.is_some() {
            format!("{id}-{i}")
        } else {
            id.to_string()
        };
        let mut b = MenuItemBuilder::with_id(id, &item.label).enabled(item.enabled);
        if let Some(acc) = item.shortcut.as_deref().and_then(accelerator) {
            b = b.accelerator(acc);
        }
        let built = b.build(app)?;
        builder = builder.item(&built);
        if item.id == MenuItemId::Status && status.is_none() {
            status = Some(built);
        }
    }
    Ok((builder.build()?, status))
}

/// Install the menu bar item.
pub fn install(app: &AppHandle) -> tauri::Result<TrayIcon> {
    let items = core_menu::menu_bar(0);
    let (menu, status) = build_menu(app, &items)?;
    app.manage(TrayStatus(Mutex::new(status)));
    app.manage(TrayMenu(Mutex::new((menu.clone(), items))));
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon()?)
        .icon_as_template(true)
        .tooltip("Cua Spaces")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_tray_icon_event(|tray, event| {
            if opens_main(&event) {
                run_action(tray.app_handle(), TrayAction::OpenMain);
            }
        })
        .on_menu_event(|app, event| {
            if let Some(action) = action_for(event.id().as_ref()) {
                run_action(app, action);
            }
        })
        .build(app)
}

/// The status line before the notch panel first reports the core's menu
/// (registry rows only). Once it has, the core's count stands.
pub fn set_space_count(app: &AppHandle, spaces: usize) {
    let Some(status) = app.try_state::<TrayStatus>() else {
        return;
    };
    let guard = status.0.lock();
    if let Ok(item) = guard.as_ref() {
        if let Some(item) = item.as_ref() {
            let _ = item.set_text(status_text(spaces));
        }
    };
}

/// Shows the core's menu (`window::menu` of the notch's roster); rebuilt
/// only when the items changed.
pub fn set_menu(app: &AppHandle, items: Vec<core_menu::MenuItem>) -> Result<(), String> {
    let state = app
        .try_state::<TrayMenu>()
        .ok_or("the menu bar item is not installed")?;
    let mut current = state.0.lock().map_err(|e| e.to_string())?;
    if current.1 == items {
        return Ok(());
    }
    let (menu, _) = build_menu(app, &items).map_err(|e| e.to_string())?;
    let tray = app
        .tray_by_id(TRAY_ID)
        .ok_or("the menu bar item is missing")?;
    tray.set_menu(Some(menu.clone()))
        .map_err(|e| e.to_string())?;
    *current = (menu, items);
    // The core's count stands from now on: registry refreshes leave it.
    if let Some(status) = app.try_state::<TrayStatus>() {
        if let Ok(mut status) = status.0.lock() {
            *status = None;
        }
    }
    Ok(())
}

/// The notch panel's menu for the menu bar item: the core's items for its
/// roster and Cua Volume's sync (`window.menu`).
#[tauri::command]
pub fn tray_set_menu(app: AppHandle, items: Vec<core_menu::MenuItem>) -> Result<(), String> {
    set_menu(&app, items)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_icon_is_an_18pt_template_at_2x() {
        let image = icon().expect("the tray icon decodes");
        assert_eq!((image.width(), image.height()), (36, 36));
        // A template image is black plus alpha: every pixel's colour is black.
        for px in image.rgba().as_chunks::<4>().0 {
            if px[3] > 0 {
                assert!(
                    px[0] < 16 && px[1] < 16 && px[2] < 16,
                    "non-black pixel {px:?}"
                );
            }
        }
        assert!(
            image.rgba().as_chunks::<4>().0.iter().any(|px| px[3] > 200),
            "the mark is visible"
        );
    }

    #[test]
    fn menu_items_map_to_actions() {
        assert_eq!(action_for(ids::OPEN), Some(TrayAction::OpenMain));
        assert_eq!(action_for(ids::NEW_SPACE), Some(TrayAction::NewSpace));
        assert_eq!(action_for(ids::SETTINGS), Some(TrayAction::Settings));
        assert_eq!(action_for(ids::QUIT), Some(TrayAction::Quit));
        assert_eq!(action_for(ids::VOLUME_CONFLICTS), Some(TrayAction::Volume));
        assert_eq!(action_for(ids::STATUS), None);
    }

    #[test]
    fn every_core_item_has_an_id_and_the_conflicts_open_the_volume_page() {
        use cua_spaces_app_core::drive_page::{DriveConflictInput, DriveSyncInput};
        let sync = DriveSyncInput {
            feed: "live".into(),
            pending_uploads: 3,
            conflicts: vec![DriveConflictInput::default(), DriveConflictInput::default()],
            ..Default::default()
        };
        let items = core_menu::menu(&core_menu::MenuInput {
            spaces: vec![],
            keyvault: None,
            sync: Some(sync),
            ..Default::default()
        });
        assert_eq!(items[0].label, "No Spaces \u{b7} Syncing 3\u{2026}");
        assert_eq!(items[1].id, MenuItemId::VolumeConflicts);
        assert_eq!(items[1].label, "2 conflicts");
        assert!(items[1].enabled);
        assert_eq!(id_for(items[1].id), Some(ids::VOLUME_CONFLICTS));
        assert_eq!(
            action_for(id_for(items[1].id).unwrap()),
            Some(TrayAction::Volume)
        );
        for item in &items {
            assert_eq!(id_for(item.id).is_none(), item.id == MenuItemId::Separator);
        }
    }

    #[test]
    fn the_menu_is_the_core_menu() {
        let words: Vec<(Option<&str>, String, Option<String>)> = core_menu::menu_bar(2)
            .into_iter()
            .map(|m| {
                (
                    id_for(m.id),
                    m.label,
                    m.shortcut.as_deref().and_then(accelerator),
                )
            })
            .collect();
        assert_eq!(words[0], (Some(ids::STATUS), "2 Spaces".into(), None));
        assert_eq!(words[1].0, None);
        assert_eq!(
            words[3],
            (Some(ids::NEW_SPACE), "New Space\u{2026}".into(), None)
        );
        assert_eq!(
            words[4],
            (
                Some(ids::SETTINGS),
                "Settings\u{2026}".into(),
                Some("CmdOrCtrl+,".into())
            )
        );
        assert_eq!(
            words.last().unwrap(),
            &(
                Some(ids::QUIT),
                "Quit Cua Spaces".into(),
                Some("CmdOrCtrl+Q".into())
            )
        );
        for (id, _, _) in &words {
            if let Some(id) = id {
                assert!(*id == ids::STATUS || action_for(id).is_some());
            }
        }
    }

    #[test]
    fn status_line_counts_spaces() {
        assert_eq!(status_text(0), "No Spaces");
        assert_eq!(status_text(1), "1 Space");
        assert_eq!(status_text(4), "4 Spaces");
    }
}
