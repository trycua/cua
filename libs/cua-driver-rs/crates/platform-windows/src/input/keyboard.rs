//! Background keyboard injection via PostMessage.
//!
//! WM_CHAR — posts a character code; simpler and more reliable for text entry.
//! WM_KEYDOWN/WM_KEYUP — used for non-printable keys (Enter, Tab, arrows, F-keys, etc.)
//!
//! All messages are posted async (PostMessageW), so they do NOT steal focus.

use anyhow::{bail, Result};
use std::thread::sleep;
use std::time::Duration;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    MapVirtualKeyW, MAPVK_VK_TO_VSC, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    PostMessageW, WM_CHAR, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

const KEY_DELAY_MS: u64 = 4;

/// Post a Unicode character as WM_CHAR.
pub fn post_char(hwnd: u64, ch: char) -> Result<()> {
    let hwnd = HWND(hwnd as *mut _);
    let code = ch as u32 as usize;
    unsafe {
        PostMessageW(hwnd, WM_CHAR, WPARAM(code), LPARAM(1))?;
    }
    Ok(())
}

/// Post all characters in a string as WM_CHAR messages with inter-key delay.
pub fn post_type_text(hwnd: u64, text: &str) -> Result<()> {
    for ch in text.chars() {
        post_char(hwnd, ch)?;
        sleep(Duration::from_millis(KEY_DELAY_MS));
    }
    Ok(())
}

/// Post all characters in `text` as WM_CHAR messages with a configurable
/// inter-character delay (on top of the baseline KEY_DELAY_MS gap).
pub fn post_type_text_with_delay(hwnd: u64, text: &str, inter_char_ms: u64) -> Result<()> {
    for ch in text.chars() {
        post_char(hwnd, ch)?;
        sleep(Duration::from_millis(KEY_DELAY_MS + inter_char_ms));
    }
    Ok(())
}

/// Press a named key (and optional modifiers) via WM_KEYDOWN/WM_KEYUP.
pub fn post_key(hwnd: u64, key: &str, modifiers: &[&str]) -> Result<()> {
    let hwnd_win = HWND(hwnd as *mut _);
    let vk = key_name_to_vk(key)?;
    let has_alt = modifiers.iter().any(|m| *m == "alt" || *m == "menu");

    let scan = unsafe { MapVirtualKeyW(vk.0 as u32, MAPVK_VK_TO_VSC) };
    let repeat_lp = |scan: u32, extended: bool, key_up: bool| {
        let mut lp: u32 = 1; // repeat count
        lp |= scan << 16;
        if extended { lp |= 1 << 24; }
        if key_up   { lp |= (1 << 30) | (1 << 31); }
        LPARAM(lp as isize)
    };

    let (down_msg, up_msg) = if has_alt {
        (WM_SYSKEYDOWN, WM_SYSKEYUP)
    } else {
        (WM_KEYDOWN, WM_KEYUP)
    };

    let mod_vks: Vec<VIRTUAL_KEY> = modifiers.iter()
        .filter_map(|m| modifier_vk(m))
        .collect();

    unsafe {
        // Press modifiers.
        for mvk in &mod_vks {
            let ms = MapVirtualKeyW(mvk.0 as u32, MAPVK_VK_TO_VSC);
            PostMessageW(hwnd_win, down_msg, WPARAM(mvk.0 as usize), repeat_lp(ms, false, false))?;
        }
        // Press key.
        PostMessageW(hwnd_win, down_msg, WPARAM(vk.0 as usize), repeat_lp(scan, is_extended(vk), false))?;
        sleep(Duration::from_millis(KEY_DELAY_MS));
        // Release key.
        PostMessageW(hwnd_win, up_msg, WPARAM(vk.0 as usize), repeat_lp(scan, is_extended(vk), true))?;
        // Release modifiers (reverse order).
        for mvk in mod_vks.iter().rev() {
            let ms = MapVirtualKeyW(mvk.0 as u32, MAPVK_VK_TO_VSC);
            PostMessageW(hwnd_win, up_msg, WPARAM(mvk.0 as usize), repeat_lp(ms, false, true))?;
        }
    }
    Ok(())
}

fn modifier_vk(name: &str) -> Option<VIRTUAL_KEY> {
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    match name.to_lowercase().as_str() {
        "ctrl" | "control" => Some(VK_CONTROL),
        "shift" => Some(VK_SHIFT),
        "alt" | "menu" => Some(VK_MENU),
        "win" | "meta" | "windows" => Some(VK_LWIN),
        _ => None,
    }
}

fn is_extended(vk: VIRTUAL_KEY) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    matches!(vk,
        VK_DELETE | VK_INSERT | VK_HOME | VK_END | VK_PRIOR | VK_NEXT |
        VK_UP | VK_DOWN | VK_LEFT | VK_RIGHT |
        VK_RCONTROL | VK_RMENU | VK_RWIN | VK_NUMLOCK | VK_SNAPSHOT
    )
}

fn key_name_to_vk(key: &str) -> Result<VIRTUAL_KEY> {
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    let vk = match key.to_lowercase().as_str() {
        "enter" | "return" => VK_RETURN,
        "tab" => VK_TAB,
        "escape" | "esc" => VK_ESCAPE,
        "space" | " " => VK_SPACE,
        "backspace" => VK_BACK,
        "delete" | "del" => VK_DELETE,
        "insert" | "ins" => VK_INSERT,
        "home" => VK_HOME,
        "end" => VK_END,
        "pageup" | "pgup" => VK_PRIOR,
        "pagedown" | "pgdn" => VK_NEXT,
        "up" => VK_UP,
        "down" => VK_DOWN,
        "left" => VK_LEFT,
        "right" => VK_RIGHT,
        "f1" => VK_F1, "f2" => VK_F2, "f3" => VK_F3, "f4" => VK_F4,
        "f5" => VK_F5, "f6" => VK_F6, "f7" => VK_F7, "f8" => VK_F8,
        "f9" => VK_F9, "f10" => VK_F10, "f11" => VK_F11, "f12" => VK_F12,
        "ctrl" | "control" => VK_CONTROL,
        "shift" => VK_SHIFT,
        "alt" => VK_MENU,
        "win" | "windows" | "meta" | "command" | "cmd" => VK_LWIN,
        "capslock" => VK_CAPITAL,
        "numlock" => VK_NUMLOCK,
        _ => {
            // Single printable character.
            let ch = key.chars().next()
                .ok_or_else(|| anyhow::anyhow!("Empty key name"))?;
            // VkKeyScanW returns VK in low byte.
            let vk_scan = unsafe {
                windows::Win32::UI::Input::KeyboardAndMouse::VkKeyScanW(ch as u16)
            };
            if vk_scan == -1i16 as u16 as i16 {
                bail!("Unknown key: {key}");
            }
            VIRTUAL_KEY((vk_scan & 0xFF) as u16)
        }
    };
    Ok(vk)
}
