//! `set_value` on a native Win32 combo box (the `COMBOBOX` class, which
//! WinForms and MFC combo boxes subclass).
//!
//! A UIA `ValuePattern.SetValue` on such a combo only rewrites the window
//! text (`WM_SETTEXT`): the control shows the new value but the app is never
//! told, so `SelectedIndexChanged` / `TextChanged` (WinForms) or the dialog's
//! `CBN_SELCHANGE` handler never run. This module makes the change the way the
//! control itself reports a user's choice, without focusing or raising the
//! window:
//!
//! * a listed option is selected with `CB_SETCURSEL`, then the parent gets the
//!   `WM_COMMAND` notifications a real selection sends (`CBN_SELENDOK`,
//!   `CBN_SELCHANGE`);
//! * an editable combo's text is replaced through its edit child with
//!   `EM_SETSEL` + `EM_REPLACESEL`, which the edit treats as typing, so the
//!   combo forwards `CBN_EDITUPDATE` / `CBN_EDITCHANGE` itself.
//!
//! The string messages (`CB_FINDSTRINGEXACT`, `CB_GETLBTEXT`,
//! `EM_REPLACESEL`, `WM_GETTEXT`) are marshalled across processes by the
//! system for these control classes.

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, GetClassNameW, GetDlgCtrlID, GetParent, GetWindowLongW, SendMessageTimeoutW,
    GWL_STYLE, SMTO_ABORTIFHUNG,
};

const CB_ERR: isize = -1;
const CB_GETCOUNT: u32 = 0x0146;
const CB_GETCURSEL: u32 = 0x0147;
const CB_GETLBTEXT: u32 = 0x0148;
const CB_GETLBTEXTLEN: u32 = 0x0149;
const CB_SETCURSEL: u32 = 0x014E;
const CB_FINDSTRINGEXACT: u32 = 0x0158;
const CBN_SELCHANGE: u16 = 1;
const CBN_EDITCHANGE: u16 = 5;
const CBN_EDITUPDATE: u16 = 6;
const CBN_SELENDOK: u16 = 9;
const CBS_TYPE_MASK: i32 = 0x3;
const CBS_DROPDOWNLIST: i32 = 0x3;
const EM_SETSEL: u32 = 0x00B1;
const EM_REPLACESEL: u32 = 0x00C2;
const WM_COMMAND: u32 = 0x0111;
const WM_GETTEXT: u32 = 0x000D;
const WM_GETTEXTLENGTH: u32 = 0x000E;
const WM_SETTEXT: u32 = 0x000C;

/// Per-message timeout. A notification runs the app's change handler before
/// it returns, so this is the budget for that handler too.
const SEND_TIMEOUT_MS: u32 = 3_000;
/// Options listed in a "no such option" refusal.
const MAX_LISTED_OPTIONS: usize = 50;

/// A native combo box found behind a UIA element.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Combo {
    pub hwnd: isize,
    /// `CBS_DROPDOWN` / `CBS_SIMPLE` (has an edit field), as opposed to
    /// `CBS_DROPDOWNLIST`.
    pub editable: bool,
}

/// What [`set_value`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComboWrite {
    /// The value was applied. `selected` is the option index chosen, if the
    /// value names an option.
    Applied {
        selected: Option<usize>,
        /// The control already held this value; nothing was sent.
        unchanged: bool,
        /// Text and selection read back from the control afterwards.
        readback_text: Option<String>,
        readback_index: Option<usize>,
    },
    /// A drop-down list has no option with this text; nothing was changed.
    NoSuchOption { options: Vec<String> },
}

/// `true` for the Win32 combo box class and its subclasses (WinForms
/// `WindowsForms10.COMBOBOX.*`), not `ComboBoxEx32` (whose notifications
/// differ) or the drop-down's `ComboLBox` list.
pub fn is_combo_class(class: &str) -> bool {
    let class = class.to_ascii_lowercase();
    class.contains("combobox") && !class.contains("comboboxex") && !class.contains("combolbox")
}

/// `true` for the `CBS_*` type bits of an editable combo (simple or drop-down).
pub fn style_is_editable(style: i32) -> bool {
    style & CBS_TYPE_MASK != CBS_DROPDOWNLIST
}

/// `WM_COMMAND` wParam for a control notification: `MAKEWPARAM(id, code)`.
pub fn command_wparam(control_id: i32, code: u16) -> usize {
    (control_id as u32 as usize & 0xFFFF) | ((code as usize) << 16)
}

/// Whether the write took: a listed option must be the selected index, and
/// an editable combo must read back the text (case-insensitively when the
/// option it matched is spelled differently).
pub fn write_verified(write: &ComboWrite, value: &str) -> bool {
    match write {
        ComboWrite::Applied {
            selected,
            readback_text,
            readback_index,
            ..
        } => match selected {
            Some(index) => *readback_index == Some(*index),
            None => readback_text.as_deref() == Some(value),
        },
        ComboWrite::NoSuchOption { .. } => false,
    }
}

fn hwnd(raw: isize) -> HWND {
    HWND(raw as *mut _)
}

fn class_name(raw: isize) -> String {
    let mut buf = [0u16; 256];
    let len = unsafe { GetClassNameW(hwnd(raw), &mut buf) };
    String::from_utf16_lossy(&buf[..len.max(0) as usize])
}

fn send(raw: isize, msg: u32, wparam: usize, lparam: isize) -> Option<isize> {
    let mut result = 0usize;
    let ok = unsafe {
        SendMessageTimeoutW(
            hwnd(raw),
            msg,
            WPARAM(wparam),
            LPARAM(lparam),
            SMTO_ABORTIFHUNG,
            SEND_TIMEOUT_MS,
            Some(&mut result),
        )
    };
    (ok.0 != 0).then_some(result as isize)
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn window_text(raw: isize) -> Option<String> {
    let len = send(raw, WM_GETTEXTLENGTH, 0, 0)?.max(0) as usize;
    let mut buf = vec![0u16; len + 1];
    let copied = send(raw, WM_GETTEXT, buf.len(), buf.as_mut_ptr() as isize)?.max(0) as usize;
    Some(String::from_utf16_lossy(&buf[..copied.min(len)]))
}

fn option_text(combo: isize, index: usize) -> Option<String> {
    let len = send(combo, CB_GETLBTEXTLEN, index, 0)?;
    if len < 0 {
        return None;
    }
    let mut buf = vec![0u16; len as usize + 1];
    let copied = send(combo, CB_GETLBTEXT, index, buf.as_mut_ptr() as isize)?;
    if copied < 0 {
        return None;
    }
    Some(String::from_utf16_lossy(
        &buf[..(copied as usize).min(len as usize)],
    ))
}

fn selected_index(combo: isize) -> Option<usize> {
    send(combo, CB_GETCURSEL, 0, 0).and_then(|index| usize::try_from(index).ok())
}

fn options(combo: isize) -> Vec<String> {
    let count = send(combo, CB_GETCOUNT, 0, 0).unwrap_or(0).max(0) as usize;
    (0..count.min(MAX_LISTED_OPTIONS))
        .filter_map(|index| option_text(combo, index))
        .collect()
}

fn edit_child(combo: isize) -> Option<isize> {
    let class = wide("Edit");
    unsafe {
        FindWindowExW(
            hwnd(combo),
            HWND::default(),
            PCWSTR(class.as_ptr()),
            PCWSTR::null(),
        )
    }
    .ok()
    .filter(|edit| !edit.is_invalid())
    .map(|edit| edit.0 as isize)
}

/// Send a combo notification to the combo's parent, as the control does.
fn notify_parent(combo: isize, code: u16) -> bool {
    let Ok(parent) = (unsafe { GetParent(hwnd(combo)) }) else {
        return false;
    };
    let id = unsafe { GetDlgCtrlID(hwnd(combo)) };
    send(
        parent.0 as isize,
        WM_COMMAND,
        command_wparam(id, code),
        combo,
    )
    .is_some()
}

/// The native combo box behind a UIA element's window handle: the combo
/// itself, or the combo that owns this edit field.
pub fn combo_for_window(raw: isize) -> Option<Combo> {
    if raw == 0 {
        return None;
    }
    let combo = if is_combo_class(&class_name(raw)) {
        raw
    } else if class_name(raw).eq_ignore_ascii_case("Edit") {
        let parent = unsafe { GetParent(hwnd(raw)) }.ok()?.0 as isize;
        if parent == 0 || !is_combo_class(&class_name(parent)) {
            return None;
        }
        parent
    } else {
        return None;
    };
    let style = unsafe { GetWindowLongW(hwnd(combo), GWL_STYLE) };
    Some(Combo {
        hwnd: combo,
        editable: style_is_editable(style),
    })
}

/// Select `value` in the combo (or type it into an editable one) and tell
/// the app, as described in the module docs.
pub fn set_value(combo: Combo, value: &str) -> anyhow::Result<ComboWrite> {
    let text = wide(value);
    let found = send(
        combo.hwnd,
        CB_FINDSTRINGEXACT,
        usize::MAX,
        text.as_ptr() as isize,
    )
    .ok_or_else(|| anyhow::anyhow!("the combo box did not answer CB_FINDSTRINGEXACT"))?;
    let selected = (found != CB_ERR).then_some(found as usize);
    let before = selected_index(combo.hwnd);

    if !combo.editable {
        let Some(index) = selected else {
            return Ok(ComboWrite::NoSuchOption {
                options: options(combo.hwnd),
            });
        };
        let unchanged = before == Some(index);
        if !unchanged {
            send(combo.hwnd, CB_SETCURSEL, index, 0)
                .ok_or_else(|| anyhow::anyhow!("the combo box did not answer CB_SETCURSEL"))?;
            notify_parent(combo.hwnd, CBN_SELENDOK);
            notify_parent(combo.hwnd, CBN_SELCHANGE);
        }
        return Ok(ComboWrite::Applied {
            selected,
            unchanged,
            readback_text: selected_index(combo.hwnd).and_then(|i| option_text(combo.hwnd, i)),
            readback_index: selected_index(combo.hwnd),
        });
    }

    let edit = edit_child(combo.hwnd);
    let current = edit.or(Some(combo.hwnd)).and_then(window_text);
    let unchanged = current.as_deref() == Some(value) && (selected.is_none() || before == selected);
    if !unchanged {
        match edit {
            // Replacing the selection is how the edit sees typing: it sends
            // EN_UPDATE / EN_CHANGE and the combo forwards them to the app.
            Some(edit) => {
                send(edit, EM_SETSEL, 0, -1);
                send(edit, EM_REPLACESEL, 1, text.as_ptr() as isize)
                    .ok_or_else(|| anyhow::anyhow!("the combo's edit field did not answer"))?;
            }
            None => {
                send(combo.hwnd, WM_SETTEXT, 0, text.as_ptr() as isize)
                    .ok_or_else(|| anyhow::anyhow!("the combo box did not answer WM_SETTEXT"))?;
                notify_parent(combo.hwnd, CBN_EDITUPDATE);
                notify_parent(combo.hwnd, CBN_EDITCHANGE);
            }
        }
        if let Some(index) = selected {
            send(combo.hwnd, CB_SETCURSEL, index, 0);
            notify_parent(combo.hwnd, CBN_SELENDOK);
            notify_parent(combo.hwnd, CBN_SELCHANGE);
        }
    }
    Ok(ComboWrite::Applied {
        selected,
        unchanged,
        readback_text: edit.or(Some(combo.hwnd)).and_then(window_text),
        readback_index: selected_index(combo.hwnd),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combo_classes_include_subclasses_but_not_ex_or_list() {
        assert!(is_combo_class("ComboBox"));
        assert!(is_combo_class(
            "WindowsForms10.COMBOBOX.app.0.29c74c7_r10_ad1"
        ));
        assert!(!is_combo_class("ComboBoxEx32"));
        assert!(!is_combo_class("ComboLBox"));
        assert!(!is_combo_class("Edit"));
    }

    #[test]
    fn style_bits_tell_editable_from_drop_down_list() {
        // Styles observed on WinForms combos (WS_* bits plus CBS_*).
        assert!(!style_is_editable(0x5601_0243));
        assert!(style_is_editable(0x5601_0242));
        assert!(style_is_editable(0x1)); // CBS_SIMPLE
    }

    #[test]
    fn command_wparam_packs_id_and_code() {
        assert_eq!(command_wparam(66056, 1), (66056 & 0xFFFF) | (1 << 16));
        assert_eq!(command_wparam(1001, 9), 1001 | (9 << 16));
    }

    #[test]
    fn verification_needs_the_selected_index_or_the_text() {
        let applied = |selected, text: Option<&str>, index| ComboWrite::Applied {
            selected,
            unchanged: false,
            readback_text: text.map(str::to_owned),
            readback_index: index,
        };
        assert!(write_verified(
            &applied(Some(2), Some("Blue"), Some(2)),
            "blue"
        ));
        assert!(!write_verified(
            &applied(Some(2), Some("Red"), Some(0)),
            "Blue"
        ));
        assert!(write_verified(
            &applied(None, Some("Custom"), None),
            "Custom"
        ));
        assert!(!write_verified(
            &applied(None, Some("Small"), Some(0)),
            "Custom"
        ));
        assert!(!write_verified(
            &ComboWrite::NoSuchOption { options: vec![] },
            "x"
        ));
    }
}
