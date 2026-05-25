//! Background keyboard injection via PostMessage.
//!
//! WM_CHAR — posts a character code; simpler and more reliable for text entry.
//! WM_KEYDOWN/WM_KEYUP — used for non-printable keys (Enter, Tab, arrows, F-keys, etc.)
//!
//! All messages are posted async (PostMessageW), so they do NOT steal focus.
//!
//! Modern XAML / WinUI3 / UWP targets reject PostMessage-based keyboard
//! injection because their CoreInput dispatcher only consumes events from
//! the system input queue, not posted messages. This module exposes
//! [`is_xaml_host_hwnd`] so callers (e.g. the `type_text` tool) can route
//! around the PostMessage path for those targets — see CUA-543 and
//! `tools::impl_::TypeTextTool` for the routing logic. The actual UIA
//! `ValuePattern.SetValue` injection lives in the tools layer alongside
//! the existing `set_value` tool; this module deliberately stays
//! Win32-only so the unit tests don't depend on UIA initialisation.

use anyhow::{bail, Result};
use std::thread::sleep;
use std::time::Duration;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    MapVirtualKeyW, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT,
    KEYBD_EVENT_FLAGS, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP,
    KEYEVENTF_SCANCODE, MAPVK_VK_TO_VSC, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, SetForegroundWindow,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetWindowThreadProcessId, IsChild,
    PostMessageW, WM_CHAR, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
};
use windows::Win32::UI::Input::KeyboardAndMouse::GetFocus;
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};

// ── XAML / UWP host detection ────────────────────────────────────────────────
//
// Two routing signals, OR'd:
//   1. Top-level window class name matches a known XAML host class.
//   2. Owning process .exe basename matches a known XAML-hosted .exe.
//
// The EXE-basename signal is the more reliable of the two: cross-session
// `GetClassNameW` can return nothing, and modern apps like Win 11 Notepad
// keep the legacy `"Notepad"` window class even though they render XAML
// underneath. Diagnostic data captured by `tools::DebugWindowInfoTool`
// (see CUA-543) confirms `notepad.exe` is the reliable signal for modern
// Notepad; class name is not.

const XAML_HOST_CLASSES: &[&str] = &[
    "ApplicationFrameWindow",
    "WinUIDesktopWin32WindowClass",
    "Windows.UI.Core.CoreWindow",
    "Microsoft.UI.Content.DesktopChildSiteBridge",
];

const XAML_HOST_EXES: &[&str] = &[
    "notepad.exe",              // Win 11 modern Notepad (UWP-packaged)
    "calculatorapp.exe",        // UWP Calculator
    "calc.exe",                 // some Win 11 builds expose the stub directly
    "applicationframehost.exe", // generic UWP frame host
    "photos.exe",               // UWP Photos
    "systemsettings.exe",       // modern Settings
];

fn class_name(hwnd: HWND) -> Option<String> {
    let mut buf = [0u16; 256];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    if n <= 0 { None } else { Some(String::from_utf16_lossy(&buf[..n as usize])) }
}

fn owning_exe_basename(hwnd: HWND) -> Option<String> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW,
        PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    let mut pid: u32 = 0;
    let tid = unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if tid == 0 || pid == 0 {
        return None;
    }
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut buf = [0u16; 1024];
    let mut len: u32 = buf.len() as u32;
    let result = unsafe {
        QueryFullProcessImageNameW(handle, PROCESS_NAME_FORMAT(0),
                                   windows::core::PWSTR(buf.as_mut_ptr()), &mut len)
    };
    let _ = unsafe { CloseHandle(handle) };
    if result.is_err() || len == 0 {
        return None;
    }
    let path = String::from_utf16_lossy(&buf[..len as usize]);
    let name = path
        .rsplit(|c: char| c == '\\' || c == '/')
        .next()
        .unwrap_or(&path)
        .to_ascii_lowercase();
    Some(name)
}

/// `true` iff the given HWND should bypass the PostMessage keyboard
/// path and route through UIA patterns (or another non-PostMessage
/// mechanism). See module docs + CUA-543 for the routing rationale.
pub fn is_xaml_host_hwnd(hwnd: u64) -> bool {
    let h = HWND(hwnd as *mut _);
    if let Some(cls) = class_name(h) {
        if XAML_HOST_CLASSES.iter().any(|known| cls == *known) {
            return true;
        }
    }
    if let Some(exe) = owning_exe_basename(h) {
        if XAML_HOST_EXES.iter().any(|known| exe == *known) {
            return true;
        }
    }
    false
}

const KEY_DELAY_MS: u64 = 4;

/// If the target's UI thread has a focused child window that's a descendant
/// of `parent`, return that child. Otherwise `None`. Used to retarget
/// `PostMessage(WM_CHAR/WM_KEYDOWN)` from the top-level frame to the actual
/// editor control (Scintilla in Notepad++, RichEdit in WordPad, etc.) —
/// top-level WindowProcs don't forward keyboard messages to embedded editors
/// automatically, so without this drill-down `type_text` silently no-ops
/// against any app that puts its text surface in a child HWND.
///
/// Uses `AttachThreadInput` to read the target thread's focus state, which
/// is the standard cross-thread way to read another thread's `GetFocus()`.
/// We detach immediately after — attaching for the duration of the post
/// would change input-state visibility for the duration.
fn focused_descendant(parent: HWND) -> Option<HWND> {
    if parent.0.is_null() { return None; }
    let mut target_pid: u32 = 0;
    let target_thread = unsafe { GetWindowThreadProcessId(parent, Some(&mut target_pid)) };
    if target_thread == 0 { return None; }
    let our_thread = unsafe { GetCurrentThreadId() };

    let focused = if our_thread == target_thread {
        unsafe { GetFocus() }
    } else {
        let _ = unsafe { AttachThreadInput(our_thread, target_thread, true) };
        let f = unsafe { GetFocus() };
        let _ = unsafe { AttachThreadInput(our_thread, target_thread, false) };
        f
    };
    if focused.0.is_null() { return None; }
    if focused == parent { return None; }
    // Only retarget if focus is genuinely a descendant of `parent` — protects
    // against accidentally posting to an unrelated window if the target is
    // not the foreground app at the moment.
    if unsafe { IsChild(parent, focused) }.as_bool() {
        Some(focused)
    } else {
        None
    }
}

/// Post a Unicode character as WM_CHAR.
pub fn post_char(hwnd: u64, ch: char) -> Result<()> {
    if let Some(msg) = crate::input::post_message_blocked_by_uipi(hwnd) {
        anyhow::bail!(msg);
    }
    // Retarget to the focused child if any — top-level WindowProcs typically
    // don't forward WM_CHAR to embedded editor children (Scintilla, RichEdit).
    let h_parent = HWND(hwnd as *mut _);
    let h = focused_descendant(h_parent).unwrap_or(h_parent);
    let code = ch as u32 as usize;
    unsafe {
        PostMessageW(h, WM_CHAR, WPARAM(code), LPARAM(1))?;
    }
    Ok(())
}

/// Post all characters in a string as WM_CHAR messages with inter-key delay.
pub fn post_type_text(hwnd: u64, text: &str) -> Result<()> {
    if let Some(msg) = crate::input::post_message_blocked_by_uipi(hwnd) {
        anyhow::bail!(msg);
    }
    let h_parent = HWND(hwnd as *mut _);
    // Resolve focused-child once at entry — re-querying per-character would
    // race with text-insertion side effects on the focus.
    let h = focused_descendant(h_parent).unwrap_or(h_parent);
    for ch in text.chars() {
        let code = ch as u32 as usize;
        unsafe { PostMessageW(h, WM_CHAR, WPARAM(code), LPARAM(1))?; }
        sleep(Duration::from_millis(KEY_DELAY_MS));
    }
    Ok(())
}

/// Post all characters in `text` as WM_CHAR messages with a configurable
/// inter-character delay (on top of the baseline KEY_DELAY_MS gap).
pub fn post_type_text_with_delay(hwnd: u64, text: &str, inter_char_ms: u64) -> Result<()> {
    if let Some(msg) = crate::input::post_message_blocked_by_uipi(hwnd) {
        anyhow::bail!(msg);
    }
    let h_parent = HWND(hwnd as *mut _);
    let h = focused_descendant(h_parent).unwrap_or(h_parent);
    for ch in text.chars() {
        let code = ch as u32 as usize;
        unsafe { PostMessageW(h, WM_CHAR, WPARAM(code), LPARAM(1))?; }
        sleep(Duration::from_millis(KEY_DELAY_MS + inter_char_ms));
    }
    Ok(())
}

/// Press a named key (and optional modifiers) via WM_KEYDOWN/WM_KEYUP.
pub fn post_key(hwnd: u64, key: &str, modifiers: &[&str]) -> Result<()> {
    if let Some(msg) = crate::input::post_message_blocked_by_uipi(hwnd) {
        anyhow::bail!(msg);
    }
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

/// Press `key` (with optional `modifiers`) via `SendInput` against the system
/// input queue, briefly focusing `hwnd` so the keystrokes land there.
///
/// Why this exists alongside `post_key`: `PostMessage(WM_KEYDOWN, VK_CONTROL)`
/// puts a message in the target's queue but does NOT update the system-wide
/// modifier state that apps poll via `GetKeyState` / `GetAsyncKeyState`. For
/// any Win32 app whose accelerator dispatcher uses `TranslateAccelerator` (which
/// is most native Win32 apps — LibreOffice, FAR, classic Notepad, etc.), the
/// shortcut never fires; the `s` arrives as plain text input.
///
/// `SendInput` puts the synthesized events on the **system input queue** —
/// the same queue `GetKeyState` reads from — so `Ctrl+S` is properly detected
/// as an accelerator. The trade-off is a brief foreground swap (focus theft),
/// which we mitigate by saving the previous foreground HWND and restoring it
/// after the keystrokes are flushed.
///
/// UIAccess constraint: `SetForegroundWindow` is restricted from non-UIAccess
/// processes when not driven by user input. The `cua-driver-uia` worker runs
/// at UIAccess integrity precisely so this restriction is lifted; outside the
/// worker, the foreground swap may silently fail and SendInput land on the
/// wrong window. Callers should funnel hotkey calls through the uia worker.
pub fn send_key_synthesized(hwnd: u64, key: &str, modifiers: &[&str]) -> Result<()> {
    let target = HWND(hwnd as *mut _);
    if target.0.is_null() {
        bail!("invalid target hwnd");
    }
    if let Some(msg) = crate::input::post_message_blocked_by_uipi(hwnd) {
        // Same UIPI defense as the PostMessage path. SendInput from UIAccess
        // _is_ allowed cross-integrity, but if our daemon is somehow at a
        // lower integrity than target, SendInput would land in the wrong
        // window (we couldn't set foreground). Better to surface the
        // diagnostic early than silently no-op.
        bail!(msg);
    }
    let key_vk = key_name_to_vk(key)?;
    let mod_vks: Vec<VIRTUAL_KEY> = modifiers
        .iter()
        .filter_map(|m| modifier_vk(m))
        .collect();

    // Build the INPUT sequence: modifiers down, key down, key up, modifiers up
    // (reverse order). Each event sends the scancode + EXTENDEDKEY flag where
    // appropriate so apps that read scancodes (not virtual keys) work too.
    let mut events: Vec<INPUT> = Vec::with_capacity(mod_vks.len() * 2 + 2);
    for mvk in &mod_vks {
        events.push(key_input(*mvk, false));
    }
    events.push(key_input(key_vk, false));
    events.push(key_input(key_vk, true));
    for mvk in mod_vks.iter().rev() {
        events.push(key_input(*mvk, true));
    }

    unsafe {
        // Save & set foreground so SendInput lands on `target`.
        let prev_fg = GetForegroundWindow();
        let _ = SetForegroundWindow(target);
        // Brief settle so the foreground swap is processed before we send.
        sleep(Duration::from_millis(8));

        let sent = SendInput(&events, std::mem::size_of::<INPUT>() as i32);
        if sent as usize != events.len() {
            // SendInput returns the number of events successfully inserted.
            // Anything less is a partial insertion (blocked by another input
            // injector, foreground UIPI denial, etc.).
            let restored = SetForegroundWindow(prev_fg);
            let _ = restored;
            bail!(
                "SendInput inserted only {sent} of {} events. Likely cause: \
                 the daemon is not at UIAccess integrity, so SetForegroundWindow \
                 was rejected and the events landed on the wrong window. Run \
                 hotkey through the cua-driver-uia worker.",
                events.len()
            );
        }

        // Brief settle to let the target process the keystrokes before we
        // restore the previous foreground (otherwise the target might not
        // get a chance to handle the accelerator before losing focus).
        sleep(Duration::from_millis(40));
        if !prev_fg.0.is_null() && prev_fg != target {
            let _ = SetForegroundWindow(prev_fg);
        }
    }
    Ok(())
}

/// Build a single keyboard INPUT struct for `vk`, either down (`up = false`)
/// or up (`up = true`). Uses scancode + EXTENDEDKEY where applicable so the
/// target sees a hardware-like keystroke.
fn key_input(vk: VIRTUAL_KEY, up: bool) -> INPUT {
    let scan = unsafe { MapVirtualKeyW(vk.0 as u32, MAPVK_VK_TO_VSC) } as u16;
    let mut flags: KEYBD_EVENT_FLAGS = KEYBD_EVENT_FLAGS(0);
    // Scancode is more reliable than VK for some apps. EXTENDEDKEY flag
    // makes arrow / nav / right-side modifier keys work correctly.
    if scan != 0 { flags |= KEYEVENTF_SCANCODE; }
    if is_extended(vk) { flags |= KEYEVENTF_EXTENDEDKEY; }
    if up { flags |= KEYEVENTF_KEYUP; }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: if scan != 0 { VIRTUAL_KEY(0) } else { vk },
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
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
