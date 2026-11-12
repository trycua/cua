// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

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
//! `tools::type_text::TypeTextTool` for the routing logic. The actual UIA
//! `ValuePattern.SetValue` injection lives in the tools layer alongside
//! the existing `set_value` tool; this module deliberately stays
//! Win32-only so the unit tests don't depend on UIA initialisation.

use anyhow::{bail, Result};
use std::thread::sleep;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, TRUE, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    MapVirtualKeyW, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, KEYEVENTF_UNICODE, MAPVK_VK_TO_VSC,
    VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, GetClassNameW, GetGUIThreadInfo, GetParent, GetWindowThreadProcessId,
    IsChild, PostMessageW, GUITHREADINFO, WM_CHAR, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN,
    WM_SYSKEYUP,
};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, SetForegroundWindow};

use crate::typing_turn::{restores_previous, ForegroundNotRestored, TypingTurn};
use crate::win32::{foreground_matches_target_or_owned_window, windows::ForegroundTarget};
use cua_driver_core::type_pacing;

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
    if n <= 0 {
        None
    } else {
        Some(String::from_utf16_lossy(&buf[..n as usize]))
    }
}

fn owning_exe_basename(hwnd: HWND) -> Option<String> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT,
        PROCESS_QUERY_LIMITED_INFORMATION,
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
        QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_FORMAT(0),
            windows::core::PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
    };
    let _ = unsafe { CloseHandle(handle) };
    if result.is_err() || len == 0 {
        return None;
    }
    let path = String::from_utf16_lossy(&buf[..len as usize]);
    let name = path
        .rsplit(['\\', '/'])
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

/// If any UI thread under the target has a focused child window that's a
/// descendant of `parent`, return that child. Otherwise `None`. Used to retarget
/// `PostMessage(WM_CHAR/WM_KEYDOWN)` from the top-level frame to the actual
/// editor control (Scintilla in Notepad++, RichEdit in WordPad, etc.) —
/// top-level WindowProcs don't forward keyboard messages to embedded editors
/// automatically, so without this drill-down `type_text` silently no-ops
/// against any app that puts its text surface in a child HWND.
///
/// Embedded renderers such as WebView2 may put their focused child on a
/// different UI thread from the native top-level frame. Enumerating descendant
/// thread ids is therefore required; checking only the frame thread queues the
/// message successfully but leaves the renderer untouched. More than one of
/// those threads can retain a focused HWND, so choose the deepest focused
/// descendant rather than whichever thread happens to enumerate first.
fn focused_descendant(parent: HWND) -> Option<HWND> {
    if parent.0.is_null() {
        return None;
    }
    let parent_thread = unsafe { GetWindowThreadProcessId(parent, None) };
    if parent_thread == 0 {
        return None;
    }

    unsafe extern "system" fn collect_thread(child: HWND, lparam: LPARAM) -> BOOL {
        let threads = &mut *(lparam.0 as *mut Vec<u32>);
        let thread = GetWindowThreadProcessId(child, None);
        if thread != 0 && !threads.contains(&thread) {
            threads.push(thread);
        }
        TRUE
    }

    let mut target_threads = vec![parent_thread];
    unsafe {
        let _ = EnumChildWindows(
            parent,
            Some(collect_thread),
            LPARAM(&mut target_threads as *mut Vec<u32> as isize),
        );
    }
    let mut best: Option<(usize, HWND)> = None;
    for target_thread in target_threads {
        let mut info = GUITHREADINFO {
            cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        if unsafe { GetGUIThreadInfo(target_thread, &mut info) }.is_err() {
            continue;
        }
        let focused = info.hwndFocus;
        if focused.0.is_null()
            || focused == parent
            || !unsafe { IsChild(parent, focused) }.as_bool()
        {
            continue;
        }

        let mut depth = 0usize;
        let mut current = focused;
        while current != parent && depth < 64 {
            let Ok(next) = (unsafe { GetParent(current) }) else {
                break;
            };
            if next.0.is_null() {
                break;
            }
            depth += 1;
            current = next;
        }
        if current == parent && best.as_ref().is_none_or(|(d, _)| depth > *d) {
            best = Some((depth, focused));
        }
    }
    best.map(|(_, focused)| focused)
}

/// Wait for an element-focused embedded renderer to expose its child HWND.
/// UIA SetFocus can complete before WebView2 updates GUITHREADINFO; polling the
/// observable focus target avoids posting the key to the native frame in that
/// short interval.
pub fn wait_for_focused_descendant(hwnd: u64, timeout: Duration) -> Option<u64> {
    let parent = HWND(hwnd as *mut _);
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(target) = focused_descendant(parent) {
            return Some(target.0 as usize as u64);
        }
        if Instant::now() >= deadline {
            return None;
        }
        sleep(Duration::from_millis(10));
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

/// Post `\n` (or `\r`, or `\r\n`) as a real Enter keystroke pair —
/// WM_KEYDOWN/WM_KEYUP(VK_RETURN) — to the focused child `h`.
///
/// Why a keystroke and not WM_CHAR(0x0A) or WM_CHAR(0x0D): standard Win32
/// edit controls accept `WM_CHAR(0x0D)` (carriage return) and insert a
/// newline; richer controls (LibreOffice's VCL edit, modern WPF/WinForms,
/// Scintilla) ignore `0x0A` outright and only sometimes accept `0x0D`.
/// A real `WM_KEYDOWN(VK_RETURN)` is the universally-honoured "Enter
/// pressed" signal — it produces a paragraph break in word processors,
/// activates the default button in dialogs, and submits forms in browsers.
/// The previous `WM_CHAR(0x0A)`-as-newline path silently joined every
/// line of multi-line `type_text` input into a single run (visible in the
/// LibreOffice Writer ode-to-a-background-cursor screenshot).
unsafe fn post_enter_keystroke(h: HWND) -> Result<()> {
    post_vk_keystroke(h, windows::Win32::UI::Input::KeyboardAndMouse::VK_RETURN)
}

/// Post `vk` as a WM_KEYDOWN/WM_KEYUP pair to `h`, with the hold gap
/// [`post_enter_keystroke`] explains. Used for the keys `type_text` keeps
/// real: Return and Tab.
unsafe fn post_vk_keystroke(h: HWND, vk: VIRTUAL_KEY) -> Result<()> {
    let scan = MapVirtualKeyW(vk.0 as u32, MAPVK_VK_TO_VSC);
    let lp_down = 1u32 | (scan << 16);
    let lp_up = lp_down | (1u32 << 30) | (1u32 << 31);
    PostMessageW(
        h,
        WM_KEYDOWN,
        WPARAM(vk.0 as usize),
        LPARAM(lp_down as isize),
    )?;
    // Hold time between KEYDOWN and KEYUP — matches `post_key`'s pattern and
    // gives the target's message loop time to TranslateMessage the KEYDOWN
    // (which synthesizes WM_CHAR(0x0D)) and DispatchMessage the paragraph
    // break before the KEYUP arrives. Without this gap, LibreOffice Writer
    // intermittently produced a double paragraph break AND dropped the
    // next character — visible in the "ABC\nDEF\nGHI" repro as "ABC / DEF /
    // (gap) / HI".
    sleep(Duration::from_millis(KEY_DELAY_MS));
    PostMessageW(h, WM_KEYUP, WPARAM(vk.0 as usize), LPARAM(lp_up as isize))?;
    Ok(())
}

/// Post all characters in a string as WM_CHAR messages with inter-key delay.
///
/// Line breaks (`\n`, `\r`, or `\r\n`) are emitted as Enter keystrokes
/// (see [`post_enter_keystroke`]) instead of literal `WM_CHAR(0x0A/0x0D)`,
/// which most rich-text Win32 controls drop.
pub fn post_type_text(hwnd: u64, text: &str) -> Result<()> {
    post_type_text_with_delay(hwnd, text, 0)
}

/// Post all characters in `text` as WM_CHAR messages with a configurable
/// inter-character delay (on top of the baseline KEY_DELAY_MS gap).
///
/// Line breaks (`\n`, `\r`, or `\r\n`) are emitted as Enter keystrokes
/// (see [`post_enter_keystroke`]) — see the LibreOffice screenshot in
/// the PR description for the bug this fixes.
pub fn post_type_text_with_delay(hwnd: u64, text: &str, inter_char_ms: u64) -> Result<()> {
    if let Some(msg) = crate::input::post_message_blocked_by_uipi(hwnd) {
        anyhow::bail!(msg);
    }
    let h_parent = HWND(hwnd as *mut _);
    // Resolve focused-child once at entry — re-querying per-character would
    // race with text-insertion side effects on the focus.
    let h = focused_descendant(h_parent).unwrap_or(h_parent);

    let units = text_units(text);
    let batched = type_pacing::is_batched(inter_char_ms);
    // Batched: up to BATCH_UNITS posts, then one pump barrier, so the target
    // never holds more than a batch and the call returns with a settled field.
    // Paced: one unit per range, each followed by its pause.
    let ranges = if batched {
        type_pacing::plan_chunks(&unit_kinds(&units), type_pacing::BATCH_UNITS)
    } else {
        (0..units.len()).map(|i| i..i + 1).collect()
    };
    let mut since_barrier = 0usize;
    for range in ranges {
        let batch_len = range.len();
        for unit in &units[range] {
            match unit {
                // The Enter keystroke makes the target's TranslateMessage append a
                // WM_CHAR(0x0D) to ITS queue when it dispatches the WM_KEYDOWN.
                // Anything we post before that lands ahead of the Return and the
                // text is reordered. Wait for the target to pump (twice, because a
                // sent message can overtake the posted keydown) before moving on.
                // Tab is the same: a real key press that the target translates.
                TextUnit::Return | TextUnit::Tab => {
                    unsafe {
                        match unit {
                            TextUnit::Tab => post_vk_keystroke(
                                h,
                                windows::Win32::UI::Input::KeyboardAndMouse::VK_TAB,
                            )?,
                            _ => post_enter_keystroke(h)?,
                        }
                    }
                    wait_for_target_pump(h);
                    // Extra settle after Enter: paragraph creation in rich
                    // editors (VCL Writer, Scintilla, RichEdit) is heavier than
                    // a single-character insert.
                    sleep(Duration::from_millis(if batched {
                        KEY_DELAY_MS
                    } else {
                        KEY_DELAY_MS + inter_char_ms + 20
                    }));
                    since_barrier = 0;
                }
                // One UTF-16 unit per WM_CHAR. A supplementary scalar (emoji) is a
                // high then a low surrogate; posting the scalar value in a single
                // WM_CHAR is truncated by the target and shows up as garbage.
                TextUnit::Packet(code) => {
                    unsafe {
                        PostMessageW(h, WM_CHAR, WPARAM(*code as usize), LPARAM(1))?;
                    }
                    since_barrier += 1;
                    if !batched {
                        sleep(Duration::from_millis(KEY_DELAY_MS + inter_char_ms));
                        // Bound the target's backlog so the read-back after the
                        // call sees a settled field, and so a hung target aborts
                        // the call.
                        if since_barrier >= POST_CHUNK_UNITS {
                            wait_for_target_pump(h);
                            since_barrier = 0;
                        }
                    }
                }
            }
        }
        if batched && since_barrier > 0 {
            wait_for_target_pump(h);
            // A pump round trip does not prove the target has dispatched the
            // posted characters: sent messages are serviced ahead of posted
            // ones. Give it time to drain the batch, so the backlog stays
            // bounded to one batch; the caller's read-back repairs the rest.
            sleep(Duration::from_millis(
                BATCH_DRAIN_MS_PER_UNIT * batch_len as u64,
            ));
            since_barrier = 0;
        }
    }
    wait_for_target_pump(h);
    Ok(())
}

/// Drain time per posted unit at the end of a batched `WM_CHAR` batch.
const BATCH_DRAIN_MS_PER_UNIT: u64 = 2;

/// Posted characters between two pump barriers.
const POST_CHUNK_UNITS: usize = 32;

/// Round-trip `WM_NULL` to `h`'s thread twice. Sent messages are serviced
/// whenever the target's message loop next retrieves a message, so this
/// returns once the target has gone around its loop after everything posted
/// so far. Bounded and best effort: a hung target just times out.
fn wait_for_target_pump(h: HWND) {
    use windows::Win32::UI::WindowsAndMessaging::{
        SendMessageTimeoutW, SMTO_ABORTIFHUNG, SMTO_BLOCK, WM_NULL,
    };
    for _ in 0..2 {
        let mut out = 0usize;
        let _ = unsafe {
            SendMessageTimeoutW(
                h,
                WM_NULL,
                WPARAM(0),
                LPARAM(0),
                SMTO_ABORTIFHUNG | SMTO_BLOCK,
                250,
                Some(&mut out),
            )
        };
    }
}

/// Press a named key (and optional modifiers) via WM_KEYDOWN/WM_KEYUP.
pub fn post_key(hwnd: u64, key: &str, modifiers: &[&str]) -> Result<()> {
    if let Some(msg) = crate::input::post_message_blocked_by_uipi(hwnd) {
        anyhow::bail!(msg);
    }
    let hwnd_win = HWND(hwnd as *mut _);
    // WebView2/Tauri keeps the editable renderer in a focused child HWND.
    // Posting only to the top-level frame reports success but never reaches
    // the renderer; mirror the WM_CHAR path and retarget to that child when
    // the target thread exposes one.
    let target = focused_descendant(hwnd_win).unwrap_or(hwnd_win);
    let (vk, scan) = match resolve_press(target, key, modifiers)? {
        Press::Key { vk, scan } => (vk, scan),
        Press::Text(ch) => return post_type_text(hwnd, &ch.to_string()),
    };
    let has_alt = modifiers.iter().any(|m| *m == "alt" || *m == "menu");

    let repeat_lp = |scan: u32, extended: bool, key_up: bool| {
        let mut lp: u32 = 1; // repeat count
        lp |= scan << 16;
        if extended {
            lp |= 1 << 24;
        }
        if key_up {
            lp |= (1 << 30) | (1 << 31);
        }
        LPARAM(lp as isize)
    };

    let (down_msg, up_msg) = if has_alt {
        (WM_SYSKEYDOWN, WM_SYSKEYUP)
    } else {
        (WM_KEYDOWN, WM_KEYUP)
    };

    let mod_vks: Vec<VIRTUAL_KEY> = modifiers.iter().filter_map(|m| modifier_vk(m)).collect();

    unsafe {
        // Press modifiers.
        for mvk in &mod_vks {
            let ms = MapVirtualKeyW(mvk.0 as u32, MAPVK_VK_TO_VSC);
            PostMessageW(
                target,
                down_msg,
                WPARAM(mvk.0 as usize),
                repeat_lp(ms, false, false),
            )?;
        }
        // Press key.
        PostMessageW(
            target,
            down_msg,
            WPARAM(vk.0 as usize),
            repeat_lp(scan, is_extended(vk), false),
        )?;
        sleep(Duration::from_millis(KEY_DELAY_MS));
        // Release key.
        PostMessageW(
            target,
            up_msg,
            WPARAM(vk.0 as usize),
            repeat_lp(scan, is_extended(vk), true),
        )?;
        // Release modifiers (reverse order).
        for mvk in mod_vks.iter().rev() {
            let ms = MapVirtualKeyW(mvk.0 as u32, MAPVK_VK_TO_VSC);
            PostMessageW(
                target,
                up_msg,
                WPARAM(mvk.0 as usize),
                repeat_lp(ms, false, true),
            )?;
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
/// Windows may refuse the foreground swap (`SetForegroundWindow` is
/// restricted when not driven by user input), and UIPI drops input sent to a
/// higher-integrity target. Both are detected and reported as errors; no
/// input is sent to an unconfirmed window.
pub fn send_key_synthesized(hwnd: u64, key: &str, modifiers: &[&str]) -> Result<()> {
    send_key_synthesized_after_focus(hwnd, key, modifiers, || Ok(()))
}

/// Foreground key delivery with a target-specific focus step performed only
/// after Windows has confirmed the exact top-level HWND as foreground.
///
/// Element-addressed callers use this to avoid the activation/focus race:
/// UIA `SetFocus` before `SetForegroundWindow` can be overwritten by the
/// ensuing activation. The callback may establish and verify child focus; no
/// input is inserted when either foreground or child-focus confirmation fails.
pub fn send_key_synthesized_after_focus(
    hwnd: u64,
    key: &str,
    modifiers: &[&str],
    focus: impl FnOnce() -> Result<()>,
) -> Result<()> {
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
    let (key_vk, key_scan) = match resolve_press(target, key, modifiers)? {
        Press::Key { vk, scan } => (vk, scan),
        Press::Text(ch) => return send_text_synthesized_after_focus(hwnd, &ch.to_string(), focus),
    };
    let mod_vks: Vec<VIRTUAL_KEY> = modifiers.iter().filter_map(|m| modifier_vk(m)).collect();

    // Build the INPUT sequence: modifiers down, key down, key up, modifiers up
    // (reverse order). Each event sends the scancode + EXTENDEDKEY flag where
    // appropriate so apps that read scancodes (not virtual keys) work too.
    let mut events: Vec<INPUT> = Vec::with_capacity(mod_vks.len() * 2 + 2);
    for mvk in &mod_vks {
        events.push(key_input(*mvk, false));
    }
    events.push(scanned_key_input(key_vk, key_scan, false));
    events.push(scanned_key_input(key_vk, key_scan, true));
    for mvk in mod_vks.iter().rev() {
        events.push(key_input(*mvk, true));
    }

    with_confirmed_foreground(target, "key delivery", focus, |_, _, turn| unsafe {
        let sent = SendInput(&events, std::mem::size_of::<INPUT>() as i32);
        if sent > 0 {
            *turn = TypingTurn::Undrained;
        }
        if sent as usize != events.len() {
            bail!(
                "SendInput inserted only {sent} of {} events. Windows blocked the \
                 rest: the target runs at a higher integrity level than the \
                 Driver (UIPI), or the input desktop is locked or showing a \
                 secure prompt. To drive an elevated app, run the Driver \
                 elevated (the default autostart daemon is).",
                events.len()
            );
        }
        Ok(())
    })
}

/// Foreground-delivery text entry: the `delivery_mode:"foreground"` rung for
/// `type_text`. Symmetric with [`send_key_synthesized`] — briefly fronts the
/// target, types `text` as SendInput Unicode (`KEYEVENTF_UNICODE`, so every
/// codepoint lands regardless of keyboard layout), then restores the prior
/// foreground. This is only reached on the explicit `delivery_mode:"foreground"`
/// rung (background never fronts); it does NOT silently fall back to
/// PostMessage: if the foreground swap is rejected
/// or the input is blocked, it bails with the same diagnostic
/// `send_key_synthesized` returns, so the caller gets an honest error instead
/// of a false success. Required for VCL/LibreOffice document grids and other
/// targets where PostMessage WM_CHAR is silently dropped.
/// Attached delivery sends batches of UTF-16 units and keys, each read-confirmed.
pub fn send_text_synthesized(hwnd: u64, text: &str) -> Result<()> {
    send_text_synthesized_after_focus(hwnd, text, || Ok(()))
}

/// Foreground Unicode delivery with child focus established after exact
/// top-level activation and before `SendInput`.
pub fn send_text_synthesized_after_focus(
    hwnd: u64,
    text: &str,
    focus: impl FnOnce() -> Result<()>,
) -> Result<()> {
    send_text_synthesized_paced_after_focus(hwnd, text, type_pacing::DEFAULT_DELAY_MS, focus)
}

/// [`send_text_synthesized_after_focus`] with the `delay_ms` contract: 0 sends
/// batches of up to [`type_pacing::BATCH_UNITS`] units per SendInput (each
/// batch read-confirmed through the drain sentinel), a positive value sends
/// one unit at a time and pauses `delay_ms` after each.
pub fn send_text_synthesized_paced_after_focus(
    hwnd: u64,
    text: &str,
    delay_ms: u64,
    focus: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let target = HWND(hwnd as *mut _);
    if target.0.is_null() {
        bail!("invalid target hwnd");
    }
    if let Some(msg) = crate::input::post_message_blocked_by_uipi(hwnd) {
        bail!(msg);
    }
    // Build the key-event sequence. Printable codepoints go through as Unicode
    // packets (KEYEVENTF_UNICODE, wVk=0, wScan=code unit), but line breaks are
    // mapped to a real VK_RETURN keystroke — mirroring the background path
    // (`post_enter_keystroke`) — because terminals and rich editors honour an
    // Enter key event, not a raw `\r`/`\n` Unicode packet. `\r\n` collapses to
    // a single Return (the `\r` emits the Enter; the following `\n` is silent).
    let units = text_units(text);
    if units.is_empty() {
        return Ok(());
    }
    let n = units.len();

    with_confirmed_foreground(target, "text delivery", focus, |att, ft, turn| unsafe {
        if let Some(att) = att {
            let ranges = if type_pacing::is_batched(delay_ms) {
                type_pacing::plan_chunks(&unit_kinds(&units), type_pacing::BATCH_UNITS)
            } else {
                (0..n).map(|i| i..i + 1).collect()
            };
            for range in ranges {
                let k = range.start;
                let batch = &units[range.clone()];
                let actual = GetForegroundWindow();
                if !foreground_matches_target_or_owned_window(ft, actual.0 as usize as u64) {
                    *turn = TypingTurn::Lost;
                    bail!(
                        "foreground_lost: foreground moved to HWND {h:?} after {k} of {n} \
                         text units were read; the remaining {remaining} were not sent. \
                         Nothing was retried; inspect the target before sending the rest.",
                        h = actual.0,
                        remaining = n - k,
                    );
                }
                let events = batch_events(batch);
                let read = att.send_and_await_read(&events, Duration::from_secs(2));
                *turn = match read {
                    UnitRead::Read => TypingTurn::Drained,
                    UnitRead::Short(0) => *turn,
                    UnitRead::Short(_) | UnitRead::Stalled => TypingTurn::Undrained,
                };
                let first = k + 1;
                let last = range.end;
                match read {
                    UnitRead::Read => {}
                    UnitRead::Short(sent) => bail!(
                        "SendInput inserted only {sent} of {total} key events for text units \
                         {first}..{last} of {n} ({k} earlier units were read). Windows blocked \
                         the rest: the target runs at a higher integrity level than the Driver \
                         (UIPI), or the input desktop is locked or showing a secure prompt. To \
                         drive an elevated app, run the Driver elevated (the default autostart \
                         daemon is).",
                        total = events.len()
                    ),
                    UnitRead::Stalled => bail!(
                        "text_delivery_stalled: the target did not read text units {first}..{last} \
                         of {n} within 2000 ms; {k} earlier units were read. Those units were \
                         inserted and may still appear in the target. Nothing was retried; \
                         inspect the target before sending the rest.",
                    ),
                }
                if delay_ms > 0 && last < n {
                    sleep(Duration::from_millis(delay_ms));
                }
            }
            return Ok(());
        }
        let mut events = Vec::with_capacity(n * 2);
        for unit in &units {
            events.extend_from_slice(&unit_events(unit)[..2]);
        }
        let sent = SendInput(&events, std::mem::size_of::<INPUT>() as i32);
        if sent > 0 {
            *turn = TypingTurn::Undrained;
        }
        if sent as usize != events.len() {
            bail!(
                "SendInput inserted only {sent} of {} key events. Windows blocked \
                 the rest: the target runs at a higher integrity level than the \
                 Driver (UIPI), or the input desktop is locked or showing a \
                 secure prompt. To drive an elevated app, run the Driver \
                 elevated (the default autostart daemon is).",
                events.len()
            );
        }
        Ok(())
    })
}

fn wait_for_exact_foreground(target: HWND, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if unsafe { GetForegroundWindow() } == target {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        sleep(Duration::from_millis(10));
    }
}

/// Run one system-queue input transaction while `target`, or a live visible
/// same-process window whose owner chain reaches `target`, is foreground.
/// Foreground and optional child focus are checked against
/// external Win32/UIA state instead of inferred from API return values or a
/// fixed settle delay. The body records in `turn` what it inserted; the prior
/// foreground is restored only as [`restores_previous`] allows, and a failed
/// turn that keeps it returns [`ForegroundNotRestored`].
fn with_confirmed_foreground<T>(
    target: HWND,
    operation: &str,
    focus: impl FnOnce() -> Result<()>,
    body: impl FnOnce(Option<&InputQueueAttachment>, ForegroundTarget, &mut TypingTurn) -> Result<T>,
) -> Result<T> {
    let previous = unsafe { GetForegroundWindow() };
    let _ = unsafe { crate::input::force_foreground_assisted(target) };
    if !wait_for_exact_foreground(target, Duration::from_millis(500)) {
        let actual = unsafe { GetForegroundWindow() };
        if !previous.0.is_null() && previous != target {
            let _ = unsafe { SetForegroundWindow(previous) };
        }
        bail!(
            "foreground_unavailable: Windows did not confirm exact target HWND {:?} for {operation} \
             within 500 ms (actual foreground HWND {:?}). Windows refused the foreground \
             change, usually because another window holds the foreground lock; no input \
             was sent. Retry, or use background delivery where the target accepts it.",
            target.0,
            actual.0
        );
    }

    let Some(foreground_target) = crate::win32::capture_foreground_target(target.0 as usize as u64)
    else {
        if !previous.0.is_null() && previous != target {
            let _ = unsafe { SetForegroundWindow(previous) };
        }
        bail!(
            "foreground_unavailable: exact target HWND {:?} disappeared before {operation}; \
             no input was sent",
            target.0
        );
    };

    let mut attachment: Option<InputQueueAttachment> = None;
    let mut turn = TypingTurn::NotSent;
    let mut result = (|| {
        focus()?;
        let deadline = Instant::now() + Duration::from_millis(250);
        let actual = loop {
            let actual = unsafe { GetForegroundWindow() };
            if foreground_matches_target_or_owned_window(
                foreground_target,
                actual.0 as usize as u64,
            ) {
                break actual;
            }
            if Instant::now() >= deadline {
                break actual;
            }
            sleep(Duration::from_millis(10));
        };
        if !foreground_matches_target_or_owned_window(foreground_target, actual.0 as usize as u64) {
            bail!(
                "foreground_unavailable: exact target HWND {:?} or a verified same-process \
                 owned window was not foreground while preparing {operation} \
                 (actual foreground HWND {:?}); no input was sent",
                target.0,
                actual.0
            );
        }
        // Attach before inserting anything: attaching resets the shared key
        // state, which must not race with modifiers the body is about to send.
        attachment = InputQueueAttachment::attach(actual);
        body(attachment.as_ref(), foreground_target, &mut turn)
    })();

    // Keep the target foreground until its thread has read every inserted
    // event; restoring earlier hands the unread tail to another window (#4477).
    // Paced text already saw each unit read, so it skips the final drain.
    if result.is_ok() && turn != TypingTurn::Drained {
        turn = match attachment.as_ref() {
            Some(attachment) if attachment.wait_for_drain(Duration::from_secs(2)) => {
                TypingTurn::Drained
            }
            Some(_) => {
                result = Err(anyhow::anyhow!(
                    "input_delivery_stalled: Windows did not confirm that the target read the \
                     {operation} input within 2000 ms. It was inserted and may still appear in \
                     the target. Nothing was retried; inspect the target before repeating it."
                ));
                TypingTurn::Undrained
            }
            None => {
                sleep(Duration::from_millis(40));
                TypingTurn::Settled
            }
        };
    }
    drop(attachment);
    if !restores_previous(turn) {
        // Marked even when the target was already in front: a caller that
        // fronted it (browser setup) must not restore its own prior window.
        return result.map_err(|error| ForegroundNotRestored { error, turn }.into());
    }
    if !previous.0.is_null() && previous != target {
        let _ = unsafe { SetForegroundWindow(previous) };
    }
    result
}

/// Unassigned virtual key used as a harmless input-drain sentinel (the same
/// "mask key" convention AutoHotkey uses): applications do not bind it and it
/// produces no character.
const DRAIN_SENTINEL_VK: VIRTUAL_KEY = VIRTUAL_KEY(0xE8);

enum UnitRead {
    Read,
    Short(u32),
    Stalled,
}

/// The caller's thread attached to the foreground thread's input queue for
/// the duration of one foreground input transaction; detaches on drop.
struct InputQueueAttachment {
    own_thread: u32,
    target_thread: u32,
}

impl InputQueueAttachment {
    fn attach(foreground: HWND) -> Option<Self> {
        use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
        let target_thread = unsafe { GetWindowThreadProcessId(foreground, None) };
        let own_thread = unsafe { GetCurrentThreadId() };
        if target_thread == 0 || target_thread == own_thread {
            return None;
        }
        unsafe { AttachThreadInput(own_thread, target_thread, true) }
            .as_bool()
            .then_some(Self {
                own_thread,
                target_thread,
            })
    }

    /// Block until the foreground thread has retrieved all input inserted so
    /// far, bounded by `timeout`. A sentinel key press is appended behind the
    /// caller's events; while attached, `GetKeyState` reflects the shared
    /// queue's synchronous key state, whose toggle bit flips only when the
    /// target thread reads the sentinel key-down.
    fn wait_for_drain(&self, timeout: Duration) -> bool {
        use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState;
        let toggled = || unsafe { GetKeyState(DRAIN_SENTINEL_VK.0 as i32) } & 1;
        let before = toggled();
        let sentinel = [sentinel_key_input(false), sentinel_key_input(true)];
        let sent = unsafe { SendInput(&sentinel, std::mem::size_of::<INPUT>() as i32) };
        if sent as usize != sentinel.len() {
            sleep(Duration::from_millis(40));
            return false;
        }
        let deadline = Instant::now() + timeout;
        loop {
            if toggled() != before {
                return true;
            }
            if Instant::now() >= deadline {
                tracing::warn!(
                    "foreground input drain: target thread did not consume the sentinel within {} ms",
                    timeout.as_millis()
                );
                return false;
            }
            sleep(Duration::from_millis(5));
        }
    }

    /// Insert `events`, then yield until the sentinel key-up is read or `timeout`.
    fn send_and_await_read(&self, events: &[INPUT], timeout: Duration) -> UnitRead {
        use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState;
        let before = unsafe { GetKeyState(DRAIN_SENTINEL_VK.0 as i32) } & 1;
        let sent = unsafe { SendInput(events, std::mem::size_of::<INPUT>() as i32) };
        if (sent as usize) < events.len() {
            return UnitRead::Short(sent);
        }
        let deadline = Instant::now() + timeout;
        loop {
            let state = unsafe { GetKeyState(DRAIN_SENTINEL_VK.0 as i32) };
            if (state & 1) != before && state >= 0 {
                return UnitRead::Read;
            }
            if Instant::now() >= deadline {
                return UnitRead::Stalled;
            }
            std::thread::yield_now();
        }
    }
}

impl Drop for InputQueueAttachment {
    fn drop(&mut self) {
        use windows::Win32::System::Threading::AttachThreadInput;
        let _ = unsafe { AttachThreadInput(self.own_thread, self.target_thread, false) };
    }
}

fn sentinel_key_input(up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: DRAIN_SENTINEL_VK,
                wScan: 0,
                dwFlags: if up {
                    KEYEVENTF_KEYUP
                } else {
                    KEYBD_EVENT_FLAGS(0)
                },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

enum TextUnit {
    Packet(u16),
    Return,
    Tab,
}

/// The planner's view of `units`: a high surrogate stays with its low one,
/// and Return and Tab are batch boundaries.
fn unit_kinds(units: &[TextUnit]) -> Vec<type_pacing::UnitKind> {
    use type_pacing::UnitKind;
    units
        .iter()
        .map(|unit| match unit {
            TextUnit::Return | TextUnit::Tab => UnitKind::Key,
            TextUnit::Packet(0xD800..=0xDBFF) => UnitKind::Lead,
            TextUnit::Packet(_) => UnitKind::Text,
        })
        .collect()
}

/// One readable VK_PACKET, or one Return. A supplementary scalar is two Packets (high, low).
fn text_units(text: &str) -> Vec<TextUnit> {
    let mut units = Vec::with_capacity(text.len());
    let mut prev_was_cr = false;
    for ch in text.chars() {
        match ch {
            '\n' if prev_was_cr => {
                prev_was_cr = false;
            }
            '\n' | '\r' => {
                units.push(TextUnit::Return);
                prev_was_cr = ch == '\r';
            }
            '\t' => {
                prev_was_cr = false;
                units.push(TextUnit::Tab);
            }
            _ => {
                prev_was_cr = false;
                let mut buf = [0u16; 2];
                for unit in ch.encode_utf16(&mut buf) {
                    units.push(TextUnit::Packet(*unit));
                }
            }
        }
    }
    units
}

/// The unit's key events followed by the drain sentinel, inserted by one SendInput.
fn unit_events(unit: &TextUnit) -> [INPUT; 4] {
    let (d, u) = unit_down_up(unit);
    [d, u, sentinel_key_input(false), sentinel_key_input(true)]
}

fn unit_down_up(unit: &TextUnit) -> (INPUT, INPUT) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{VK_RETURN, VK_TAB};
    match unit {
        TextUnit::Packet(c) => (unicode_key_input(*c, false), unicode_key_input(*c, true)),
        TextUnit::Return => (key_input(VK_RETURN, false), key_input(VK_RETURN, true)),
        TextUnit::Tab => (key_input(VK_TAB, false), key_input(VK_TAB, true)),
    }
}

/// One batch's key events followed by one drain sentinel, for a single SendInput.
fn batch_events(units: &[TextUnit]) -> Vec<INPUT> {
    let mut events = Vec::with_capacity(units.len() * 2 + 2);
    for unit in units {
        let (d, u) = unit_down_up(unit);
        events.push(d);
        events.push(u);
    }
    events.push(sentinel_key_input(false));
    events.push(sentinel_key_input(true));
    events
}

/// Build a single Unicode keyboard INPUT struct for one UTF-16 code unit,
/// either down (`up = false`) or up (`up = true`). Used by
/// [`send_text_synthesized`].
fn unicode_key_input(unit: u16, up: bool) -> INPUT {
    let mut flags = KEYEVENTF_UNICODE;
    if up {
        flags |= KEYEVENTF_KEYUP;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0),
                wScan: unit,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// Build a single keyboard INPUT struct for `vk`, either down (`up = false`)
/// or up (`up = true`). Uses scancode + EXTENDEDKEY where applicable so the
/// target sees a hardware-like keystroke.
fn key_input(vk: VIRTUAL_KEY, up: bool) -> INPUT {
    let scan = unsafe { MapVirtualKeyW(vk.0 as u32, MAPVK_VK_TO_VSC) };
    scanned_key_input(vk, scan, up)
}

fn scanned_key_input(vk: VIRTUAL_KEY, scan: u32, up: bool) -> INPUT {
    let scan = scan as u16;
    let mut flags: KEYBD_EVENT_FLAGS = KEYBD_EVENT_FLAGS(0);
    // Scancode is more reliable than VK for some apps. EXTENDEDKEY flag
    // makes arrow / nav / right-side modifier keys work correctly.
    if scan != 0 {
        flags |= KEYEVENTF_SCANCODE;
    }
    if is_extended(vk) {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if up {
        flags |= KEYEVENTF_KEYUP;
    }
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
        "alt" | "menu" | "option" => Some(VK_MENU),
        "win" | "meta" | "windows" | "cmd" | "command" => Some(VK_LWIN),
        _ => None,
    }
}

/// Build the SendInput `INPUT` events that HOLD the named modifier keys around a
/// pointer action: `(downs, ups)` where `downs` presses each modifier (in order)
/// and `ups` releases them (reverse order). A pointer path emits `downs`, does
/// the click, then emits `ups`. Unknown modifier names are skipped. Empty input
/// → two empty vecs (no-op). Lets the mouse SendInput path hold cmd/shift/alt/
/// ctrl exactly like `send_key_synthesized` does for keystrokes.
pub fn modifier_hold_inputs(modifiers: &[&str]) -> (Vec<INPUT>, Vec<INPUT>) {
    let mod_vks: Vec<VIRTUAL_KEY> = modifiers.iter().filter_map(|m| modifier_vk(m)).collect();
    let downs: Vec<INPUT> = mod_vks.iter().map(|v| key_input(*v, false)).collect();
    let ups: Vec<INPUT> = mod_vks.iter().rev().map(|v| key_input(*v, true)).collect();
    (downs, ups)
}

fn is_extended(vk: VIRTUAL_KEY) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    matches!(
        vk,
        VK_DELETE
            | VK_INSERT
            | VK_HOME
            | VK_END
            | VK_PRIOR
            | VK_NEXT
            | VK_UP
            | VK_DOWN
            | VK_LEFT
            | VK_RIGHT
            | VK_RCONTROL
            | VK_RMENU
            | VK_LWIN
            | VK_RWIN
            | VK_NUMLOCK
            | VK_SNAPSHOT
    )
}

/// A key press resolved for one target window.
enum Press {
    /// A virtual key and its scan code on the layout that translates it.
    Key { vk: VIRTUAL_KEY, scan: u32 },
    /// A character the target's layout cannot type with one unshifted key;
    /// it goes through `type_text`'s Unicode path instead.
    Text(char),
}

/// A single printable character with no modifiers must type that character,
/// so it is resolved on the target thread's keyboard layout (#6181): on Thai
/// Kedmanee VK_3 types '-', and on AZERTY '3' needs Shift. Named keys and
/// chords such as ctrl+s stay on their virtual keys.
fn resolve_press(target: HWND, key: &str, modifiers: &[&str]) -> Result<Press> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetKeyboardLayout, MapVirtualKeyExW, VkKeyScanExW,
    };
    let mut chars = key.chars();
    if let (Some(ch), None, true) = (chars.next(), chars.next(), modifiers.is_empty()) {
        if !ch.is_control() {
            // Letter names are case-insensitive, as on the other platforms:
            // "A" types what "a" types and does not add Shift.
            let ch = ch.to_ascii_lowercase();
            let hkl = unsafe { GetKeyboardLayout(GetWindowThreadProcessId(target, None)) };
            let vk = u16::try_from(ch as u32).ok().and_then(|unit| {
                crate::keycodes::unshifted_layout_key(unsafe { VkKeyScanExW(unit, hkl) })
            });
            return Ok(match vk {
                Some(vk) => Press::Key {
                    vk: VIRTUAL_KEY(vk),
                    scan: unsafe { MapVirtualKeyExW(vk as u32, MAPVK_VK_TO_VSC, hkl) },
                },
                None => Press::Text(ch),
            });
        }
    }
    let vk = key_name_to_vk(key)?;
    let scan = unsafe { MapVirtualKeyW(vk.0 as u32, MAPVK_VK_TO_VSC) };
    Ok(Press::Key { vk, scan })
}

fn key_name_to_vk(key: &str) -> Result<VIRTUAL_KEY> {
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    // Windows reserves the ASCII values themselves as the virtual-key codes
    // for A-Z and 0-9. Resolve the documented alphanumeric vocabulary
    // directly so it does not depend on the daemon thread having a current
    // keyboard layout, which VkKeyScanW requires.
    if let Some(vk) = crate::keycodes::ascii_alphanumeric_virtual_key_code(key) {
        return Ok(VIRTUAL_KEY(vk));
    }

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
        "f1" => VK_F1,
        "f2" => VK_F2,
        "f3" => VK_F3,
        "f4" => VK_F4,
        "f5" => VK_F5,
        "f6" => VK_F6,
        "f7" => VK_F7,
        "f8" => VK_F8,
        "f9" => VK_F9,
        "f10" => VK_F10,
        "f11" => VK_F11,
        "f12" => VK_F12,
        "ctrl" | "control" => VK_CONTROL,
        "shift" => VK_SHIFT,
        "alt" => VK_MENU,
        "win" | "windows" | "meta" | "command" | "cmd" => VK_LWIN,
        "capslock" => VK_CAPITAL,
        "numlock" => VK_NUMLOCK,
        _ => {
            // Single printable character.
            let ch = key
                .chars()
                .next()
                .ok_or_else(|| anyhow::anyhow!("Empty key name"))?;
            // VkKeyScanW returns VK in low byte.
            let vk_scan =
                unsafe { windows::Win32::UI::Input::KeyboardAndMouse::VkKeyScanW(ch as u16) };
            if vk_scan == -1i16 as u16 as i16 {
                bail!("Unknown key: {key}");
            }
            VIRTUAL_KEY((vk_scan & 0xFF) as u16)
        }
    };
    Ok(vk)
}

#[cfg(test)]
mod extended_key_tests {
    use super::*;

    #[test]
    fn windows_key_presses_and_releases_carry_the_extended_key_flag() {
        // The left Windows key's scan code is 0xE05B; without the extended
        // flag, Windows treats it as an unrelated key and shortcuts such as
        // Win+S do nothing.
        let (downs, ups) = modifier_hold_inputs(&["win"]);
        assert_eq!((downs.len(), ups.len()), (1, 1));
        for input in downs.iter().chain(&ups) {
            let flags = unsafe { input.Anonymous.ki.dwFlags };
            assert_ne!(flags.0 & KEYEVENTF_EXTENDEDKEY.0, 0, "{flags:?}");
        }
    }
}

#[cfg(test)]
mod text_unit_tests {
    use super::*;

    #[test]
    fn cjk_issue_fixture_is_three_packets() {
        assert!(matches!(
            text_units("二等座").as_slice(),
            [
                TextUnit::Packet(0x4E8C),
                TextUnit::Packet(0x7B49),
                TextUnit::Packet(0x5EA7)
            ]
        ));
    }

    #[test]
    fn emoji_is_two_surrogate_packets() {
        assert!(matches!(
            text_units("🙂").as_slice(),
            [TextUnit::Packet(0xD83D), TextUnit::Packet(0xDE42)]
        ));
    }

    #[test]
    fn crlf_and_lone_breaks_are_one_return_each() {
        assert!(matches!(
            text_units("a\r\nb\rc\nd").as_slice(),
            [
                TextUnit::Packet(0x61),
                TextUnit::Return,
                TextUnit::Packet(0x62),
                TextUnit::Return,
                TextUnit::Packet(0x63),
                TextUnit::Return,
                TextUnit::Packet(0x64),
            ]
        ));
    }

    #[test]
    fn newline_then_cr_is_two_returns() {
        assert!(matches!(
            text_units("\n\r").as_slice(),
            [TextUnit::Return, TextUnit::Return]
        ));
    }

    #[test]
    fn tab_is_a_key_of_its_own() {
        assert!(matches!(
            text_units("a\tb").as_slice(),
            [
                TextUnit::Packet(0x61),
                TextUnit::Tab,
                TextUnit::Packet(0x62)
            ]
        ));
        for input in &unit_events(&TextUnit::Tab)[..2] {
            let flags = unsafe { input.Anonymous.ki.dwFlags };
            assert_eq!(flags.0 & KEYEVENTF_UNICODE.0, 0);
        }
    }

    #[test]
    fn batches_never_split_a_surrogate_pair_or_cross_a_key() {
        let text = format!(
            "{}\u{1F600}\n{}\t{}",
            "a".repeat(15),
            "b".repeat(40),
            "\u{1F642}".repeat(9)
        );
        let units = text_units(&text);
        let kinds = unit_kinds(&units);
        let mut next = 0;
        for range in type_pacing::plan_chunks(&kinds, type_pacing::BATCH_UNITS) {
            assert_eq!(range.start, next);
            next = range.end;
            let batch = &units[range];
            assert!(batch.len() <= type_pacing::BATCH_UNITS);
            // A batch holds either one key or only packets, and ends on a
            // pair boundary.
            let keys = batch
                .iter()
                .filter(|u| !matches!(u, TextUnit::Packet(_)))
                .count();
            assert!(keys == 0 || batch.len() == 1);
            if let Some(TextUnit::Packet(last)) = batch.last() {
                assert!(!(0xD800..=0xDBFF).contains(last));
            }
            // One SendInput per batch: two events per unit plus the sentinel pair.
            assert_eq!(batch_events(batch).len(), batch.len() * 2 + 2);
        }
        assert_eq!(next, units.len());
    }

    #[test]
    fn empty_text_has_no_units() {
        assert!(text_units("").is_empty());
    }

    #[test]
    fn packet_events_are_unicode_then_sentinel() {
        let events = unit_events(&TextUnit::Packet(0x4E8C));
        let keys: Vec<_> = events
            .iter()
            .map(|input| unsafe { input.Anonymous.ki })
            .collect();
        assert_eq!(keys[0].wVk.0, 0);
        assert_eq!(keys[1].wVk.0, 0);
        assert_ne!(keys[0].dwFlags.0 & KEYEVENTF_UNICODE.0, 0);
        assert_ne!(keys[1].dwFlags.0 & KEYEVENTF_UNICODE.0, 0);
        assert_eq!(keys[0].dwFlags.0 & KEYEVENTF_KEYUP.0, 0);
        assert_ne!(keys[1].dwFlags.0 & KEYEVENTF_KEYUP.0, 0);
        assert_ne!(keys[3].dwFlags.0 & KEYEVENTF_KEYUP.0, 0);
        assert_eq!(keys[2].wVk.0, 0xE8);
        assert_eq!(keys[3].wVk.0, 0xE8);
    }

    #[test]
    fn return_events_are_not_unicode() {
        let events = unit_events(&TextUnit::Return);
        for input in &events[..2] {
            let flags = unsafe { input.Anonymous.ki.dwFlags };
            assert_eq!(flags.0 & KEYEVENTF_UNICODE.0, 0);
        }
    }
}

#[cfg(test)]
mod post_text_unit_tests {
    use super::*;

    fn packets(text: &str) -> Vec<Option<u16>> {
        text_units(text)
            .into_iter()
            .map(|u| match u {
                TextUnit::Packet(c) => Some(c),
                TextUnit::Return | TextUnit::Tab => None,
            })
            .collect()
    }

    #[test]
    fn supplementary_scalar_is_two_surrogate_units() {
        assert_eq!(packets("\u{1F600}"), vec![Some(0xD83D), Some(0xDE00)]);
    }

    #[test]
    fn crlf_is_one_return_and_symbols_survive() {
        assert_eq!(
            packets("a\r\nb\n~é"),
            vec![Some(97), None, Some(98), None, Some(126), Some(0xE9)]
        );
    }
}

/// Micro-benchmark and correctness check for background `WM_CHAR` text
/// delivery into a real EDIT control. Run on a Windows desktop session:
/// `cargo test -p platform-windows --lib bench_post_type_text -- --ignored --nocapture`
#[cfg(test)]
mod type_text_bench {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::sync::Arc;
    use std::time::Instant;
    use windows::core::w;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, DispatchMessageW, GetWindowTextLengthW, GetWindowTextW,
        PeekMessageW, SetWindowTextW, TranslateMessage, MSG, PM_REMOVE, WINDOW_EX_STYLE,
        WINDOW_STYLE, WS_POPUP,
    };

    /// An EDIT control with its own message pump on a worker thread.
    struct Edit {
        hwnd: HWND,
        stop: Arc<AtomicBool>,
        pump: Option<std::thread::JoinHandle<()>>,
    }

    impl Edit {
        fn new() -> Self {
            let stop = Arc::new(AtomicBool::new(false));
            let pump_stop = stop.clone();
            let (tx, rx) = mpsc::channel();
            let pump = std::thread::spawn(move || unsafe {
                let instance = GetModuleHandleW(None).expect("module handle");
                // ES_MULTILINE (0x4) so a long text is not clipped to one line.
                let hwnd = CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    w!("EDIT"),
                    w!(""),
                    WINDOW_STYLE(WS_POPUP.0 | 0x0004),
                    0,
                    0,
                    400,
                    200,
                    None,
                    None,
                    instance,
                    None,
                )
                .expect("create EDIT control");
                tx.send(hwnd.0 as isize).expect("send handle");
                let mut msg = MSG::default();
                while !pump_stop.load(Ordering::Acquire) {
                    if PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                        // Like a real application loop: a posted WM_KEYDOWN
                        // becomes the WM_CHAR the control inserts.
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    } else {
                        std::thread::sleep(Duration::from_micros(200));
                    }
                }
                let _ = DestroyWindow(hwnd);
            });
            let hwnd = rx
                .recv_timeout(Duration::from_secs(10))
                .expect("EDIT control created");
            Self {
                hwnd: HWND(hwnd as *mut _),
                stop,
                pump: Some(pump),
            }
        }

        fn text(&self) -> String {
            unsafe {
                let len = GetWindowTextLengthW(self.hwnd);
                let mut buf = vec![0u16; len as usize + 1];
                let got = GetWindowTextW(self.hwnd, &mut buf);
                String::from_utf16_lossy(&buf[..got as usize])
            }
        }

        fn clear(&self) {
            unsafe {
                let _ = SetWindowTextW(self.hwnd, w!(""));
            }
        }
    }

    impl Drop for Edit {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            if let Some(pump) = self.pump.take() {
                let _ = pump.join();
            }
        }
    }

    #[test]
    #[ignore = "benchmark; needs an interactive Windows desktop session"]
    fn bench_post_type_text() {
        let edit = Edit::new();
        let text = "The quick brown fox jumps over the lazy dog, then types 1234567890 in a field.";
        let n = text.chars().count();
        // 30 is the old default; 0 is the new one.
        for (label, delay_ms) in [
            ("paced (before, delay_ms=30)", 30u64),
            ("batched (after, delay_ms=0)", 0),
        ] {
            let mut runs = Vec::new();
            for _ in 0..9 {
                edit.clear();
                let started = Instant::now();
                post_type_text_with_delay(edit.hwnd.0 as u64, text, delay_ms).expect("post");
                runs.push(started.elapsed().as_secs_f64() * 1000.0);
                assert_eq!(
                    edit.text(),
                    text,
                    "{label}: every character landed in order"
                );
            }
            runs.sort_by(|a, b| a.total_cmp(b));
            eprintln!(
                "BENCH wm_char {label} chars={n} p50_ms={:.1} min_ms={:.1} max_ms={:.1}",
                runs[4], runs[0], runs[8]
            );
        }
    }

    #[test]
    #[ignore = "needs an interactive Windows desktop session"]
    fn batched_post_keeps_order_with_a_tab_between_batches() {
        let edit = Edit::new();
        let text = "alpha beta gamma delta epsilon\tzeta eta theta iota kappa lambda";
        post_type_text_with_delay(edit.hwnd.0 as u64, text, 0).expect("post");
        assert_eq!(edit.text(), text);
    }
}
