// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Background `invoke_menu` for a classic Win32 menu bar (trycua/cua#6823).
//!
//! The UI Automation route opens each menu on the way to the item, and a
//! classic Win32 menu opens only in its window's modal menu loop, which makes
//! that window foreground and takes the user's keyboard and pointer. A window
//! with a menu bar (`GetMenu`) instead publishes the whole menu as an `HMENU`
//! tree that any process on the desktop can read. This module resolves the
//! path through that tree and posts the final item's `WM_COMMAND` to the
//! window, which is what the menu loop itself sends when the user picks the
//! item. No menu opens and the window is never activated.
//!
//! Before reading a submenu the window gets the `WM_INITMENUPOPUP` it would
//! get just before the submenu opened, so the app can enable, gray or add
//! items as it does for a real open.

/// Why a menu path could not be resolved through the window's `HMENU`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum MenuCommandError {
    /// The item cannot be reached without opening a menu: the window has no
    /// classic menu bar, or an item on the path is not published until its
    /// menu opens or carries no readable label.
    MustOpen(String),
    /// The path itself is wrong for this menu: ambiguous, disabled, or a
    /// submenu where a command was expected (or the reverse).
    Path(String),
}

/// One item of a Win32 menu, as `GetMenuItemInfoW` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MenuItem {
    /// The item's string, or `None` for an owner-drawn or bitmap item whose
    /// label lives in the app's own memory.
    pub text: Option<String>,
    pub separator: bool,
    pub disabled: bool,
    pub has_submenu: bool,
    pub id: u32,
}

/// What a path segment resolved to within one menu level.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum MenuStep {
    /// Descend into the submenu at this position.
    Submenu(usize),
    /// Post this command identifier.
    Command(u16),
}

/// The label UI Automation names a Win32 menu item by: the mnemonic `&`
/// markers removed (`&&` is a literal `&`) and the accelerator text after the
/// tab dropped, so `invoke_menu` paths match in both routes.
pub(crate) fn menu_label(raw: &str) -> String {
    let text = raw.split('\t').next().unwrap_or_default();
    let mut label = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '&' {
            if chars.peek() == Some(&'&') {
                label.push('&');
                chars.next();
            }
            continue;
        }
        label.push(c);
    }
    label.trim().to_owned()
}

/// Resolve `segment` (path index `depth`) among one menu level's `items`.
pub(crate) fn resolve_step(
    items: &[MenuItem],
    segment: &str,
    depth: usize,
    last: bool,
) -> Result<MenuStep, MenuCommandError> {
    let matches: Vec<usize> = items
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            !item.separator && item.text.as_deref().map(menu_label).as_deref() == Some(segment)
        })
        .map(|(position, _)| position)
        .collect();
    let position = match matches.as_slice() {
        [position] => *position,
        [] => {
            let detail = if items
                .iter()
                .any(|item| !item.separator && item.text.is_none())
            {
                "is not a readable item of the window's menu (the menu draws its own items)"
            } else {
                "is not in the window's menu until that menu opens"
            };
            return Err(MenuCommandError::MustOpen(format!(
                "menu path segment {depth} {detail}"
            )));
        }
        _ => {
            return Err(MenuCommandError::Path(format!(
                "menu path segment {depth} is ambiguous"
            )))
        }
    };
    let item = &items[position];
    if item.disabled {
        return Err(MenuCommandError::Path(format!(
            "menu path segment {depth} is disabled"
        )));
    }
    match (last, item.has_submenu) {
        (false, true) => Ok(MenuStep::Submenu(position)),
        (false, false) => Err(MenuCommandError::Path(format!(
            "menu path segment {depth} is a command, not a submenu"
        ))),
        (true, true) => Err(MenuCommandError::Path(format!(
            "menu path segment {depth} is a submenu, not a command"
        ))),
        // WM_COMMAND carries a 16-bit identifier; a wider one is not a
        // command the menu loop could send either.
        (true, false) => u16::try_from(item.id).map(MenuStep::Command).map_err(|_| {
            MenuCommandError::MustOpen(format!(
                "menu path segment {depth} has no 16-bit command identifier"
            ))
        }),
    }
}

#[cfg(target_os = "windows")]
mod native {
    use super::{resolve_step, MenuCommandError, MenuItem, MenuStep};
    use windows::core::PWSTR;
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetMenu, GetMenuItemCount, GetMenuItemInfoW, PostMessageW, SendMessageTimeoutW, HMENU,
        MENUITEMINFOW, MFS_DISABLED, MFT_BITMAP, MFT_OWNERDRAW, MFT_SEPARATOR, MIIM_FTYPE, MIIM_ID,
        MIIM_STATE, MIIM_STRING, MIIM_SUBMENU, SMTO_ABORTIFHUNG, WM_COMMAND, WM_INITMENUPOPUP,
    };

    fn read_items(menu: HMENU) -> Result<Vec<(MenuItem, HMENU)>, MenuCommandError> {
        let count = unsafe { GetMenuItemCount(menu) };
        if count < 0 {
            return Err(MenuCommandError::MustOpen(
                "the window's menu could not be read".into(),
            ));
        }
        (0..count as u32)
            .map(|position| {
                let mut info = MENUITEMINFOW {
                    cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
                    fMask: MIIM_FTYPE | MIIM_STATE | MIIM_ID | MIIM_SUBMENU | MIIM_STRING,
                    ..Default::default()
                };
                unsafe { GetMenuItemInfoW(menu, position, true, &mut info) }.map_err(|error| {
                    MenuCommandError::MustOpen(format!("a menu item could not be read: {error}"))
                })?;
                let unreadable = (info.fType & (MFT_OWNERDRAW | MFT_BITMAP)).0 != 0;
                let text = if unreadable {
                    None
                } else {
                    let mut buffer = vec![0u16; info.cch as usize + 1];
                    info.dwTypeData = PWSTR(buffer.as_mut_ptr());
                    info.cch = buffer.len() as u32;
                    unsafe { GetMenuItemInfoW(menu, position, true, &mut info) }.map_err(
                        |error| {
                            MenuCommandError::MustOpen(format!(
                                "a menu item could not be read: {error}"
                            ))
                        },
                    )?;
                    Some(String::from_utf16_lossy(&buffer[..info.cch as usize]))
                };
                Ok((
                    MenuItem {
                        text,
                        separator: (info.fType & MFT_SEPARATOR).0 != 0,
                        // MFS_DISABLED covers both MF_GRAYED and MF_DISABLED.
                        disabled: (info.fState & MFS_DISABLED).0 != 0,
                        has_submenu: !info.hSubMenu.is_invalid(),
                        id: info.wID,
                    },
                    info.hSubMenu,
                ))
            })
            .collect()
    }

    /// Resolve `path` through `hwnd`'s menu bar and post the final item's
    /// `WM_COMMAND`. Never opens a menu or activates the window.
    pub(crate) fn post_menu_command(hwnd: u64, path: &[String]) -> Result<(), MenuCommandError> {
        let window = HWND(hwnd as usize as *mut _);
        let mut menu = unsafe { GetMenu(window) };
        if menu.is_invalid() {
            return Err(MenuCommandError::MustOpen(
                "the window has no classic Win32 menu bar; its menus are reachable only by \
                 opening them"
                    .into(),
            ));
        }
        for (depth, segment) in path.iter().enumerate() {
            let items = read_items(menu)?;
            let plain: Vec<MenuItem> = items.iter().map(|(item, _)| item.clone()).collect();
            match resolve_step(&plain, segment, depth, depth + 1 == path.len())? {
                MenuStep::Submenu(position) => {
                    menu = items[position].1;
                    let mut result = 0usize;
                    // The app enables, grays or fills the submenu here, as it
                    // does just before a real open.
                    unsafe {
                        SendMessageTimeoutW(
                            window,
                            WM_INITMENUPOPUP,
                            WPARAM(menu.0 as usize),
                            LPARAM(position as isize),
                            SMTO_ABORTIFHUNG,
                            1000,
                            Some(&mut result),
                        )
                    };
                }
                MenuStep::Command(id) => {
                    return unsafe {
                        PostMessageW(window, WM_COMMAND, WPARAM(id as usize), LPARAM(0))
                    }
                    .map_err(|error| {
                        MenuCommandError::MustOpen(format!(
                            "the menu command could not be posted: {error}"
                        ))
                    });
                }
            }
        }
        unreachable!("the last path segment always resolves to a command or an error")
    }
}

#[cfg(target_os = "windows")]
pub(crate) use native::post_menu_command;

#[cfg(test)]
mod tests {
    use super::*;

    fn item(text: &str) -> MenuItem {
        MenuItem {
            text: Some(text.into()),
            separator: false,
            disabled: false,
            has_submenu: false,
            id: 0,
        }
    }

    fn submenu(text: &str) -> MenuItem {
        MenuItem {
            has_submenu: true,
            ..item(text)
        }
    }

    fn command(text: &str, id: u32) -> MenuItem {
        MenuItem { id, ..item(text) }
    }

    #[test]
    fn labels_match_the_ui_automation_names() {
        assert_eq!(menu_label("&Edit"), "Edit");
        assert_eq!(menu_label("Select &All\tCtrl+A"), "Select All");
        assert_eq!(menu_label("Save &As..."), "Save As...");
        assert_eq!(menu_label("Find && &Replace"), "Find & Replace");
        assert_eq!(menu_label(" &Undo \tCtrl+Z"), "Undo");
    }

    #[test]
    fn a_path_resolves_to_its_submenu_then_its_command() {
        let bar = [submenu("&File"), submenu("&Edit")];
        assert_eq!(
            resolve_step(&bar, "Edit", 0, false),
            Ok(MenuStep::Submenu(1))
        );
        let edit = [
            command("&Undo\tCtrl+Z", 16),
            MenuItem {
                separator: true,
                ..item("")
            },
            command("Select &All\tCtrl+A", 25),
        ];
        assert_eq!(
            resolve_step(&edit, "Select All", 1, true),
            Ok(MenuStep::Command(25))
        );
    }

    #[test]
    fn an_item_published_only_when_its_menu_opens_must_open() {
        assert!(matches!(
            resolve_step(&[], "Recent", 1, false),
            Err(MenuCommandError::MustOpen(_))
        ));
        let owner_drawn = [MenuItem {
            text: None,
            ..command("", 7)
        }];
        assert!(matches!(
            resolve_step(&owner_drawn, "Paste", 1, true),
            Err(MenuCommandError::MustOpen(detail)) if detail.contains("draws its own items")
        ));
        assert!(matches!(
            resolve_step(&[command("Big", 0x1_0000)], "Big", 0, true),
            Err(MenuCommandError::MustOpen(_))
        ));
    }

    #[test]
    fn a_wrong_path_is_a_path_error() {
        let path_error = |items: &[MenuItem], segment: &str, last: bool| match resolve_step(
            items, segment, 2, last,
        ) {
            Err(MenuCommandError::Path(detail)) => detail,
            other => panic!("expected a path error, got {other:?}"),
        };
        assert_eq!(
            path_error(&[command("Copy", 1), command("&Copy", 2)], "Copy", true),
            "menu path segment 2 is ambiguous"
        );
        let grayed = MenuItem {
            disabled: true,
            ..command("Cut", 3)
        };
        assert_eq!(
            path_error(&[grayed], "Cut", true),
            "menu path segment 2 is disabled"
        );
        assert_eq!(
            path_error(&[command("Cut", 3)], "Cut", false),
            "menu path segment 2 is a command, not a submenu"
        );
        assert_eq!(
            path_error(&[submenu("Zoom")], "Zoom", true),
            "menu path segment 2 is a submenu, not a command"
        );
    }
}
