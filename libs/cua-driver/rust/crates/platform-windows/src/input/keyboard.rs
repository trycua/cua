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
        if current == parent && best.as_ref().map_or(true, |(d, _)| depth > *d) {
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
    let vk = windows::Win32::UI::Input::KeyboardAndMouse::VK_RETURN;
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

    let mut prev_was_cr = false;
    for ch in text.chars() {
        match ch {
            // `\r\n` (Windows-style line ending) → single Enter. The `\r`
            // does the Enter; the following `\n` is consumed silently.
            '\n' if prev_was_cr => {
                prev_was_cr = false;
            }
            '\n' | '\r' => {
                unsafe {
                    post_enter_keystroke(h)?;
                }
                prev_was_cr = ch == '\r';
                // Extra settle after Enter — paragraph creation in rich
                // editors (VCL Writer, Scintilla, RichEdit) is heavier than
                // a single-character insert, so the baseline KEY_DELAY_MS +
                // inter_char_ms is not enough on slower hosts. The extra
                // 20 ms covers the queue drain without noticeably slowing
                // multi-line typing.
                sleep(Duration::from_millis(KEY_DELAY_MS + inter_char_ms + 20));
            }
            _ => {
                prev_was_cr = false;
                let code = ch as u32 as usize;
                unsafe {
                    PostMessageW(h, WM_CHAR, WPARAM(code), LPARAM(1))?;
                }
                sleep(Duration::from_millis(KEY_DELAY_MS + inter_char_ms));
            }
        }
    }
    Ok(())
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
    let vk = key_name_to_vk(key)?;
    let has_alt = modifiers.iter().any(|m| *m == "alt" || *m == "menu");

    let scan = unsafe { MapVirtualKeyW(vk.0 as u32, MAPVK_VK_TO_VSC) };
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
    let key_vk = key_name_to_vk(key)?;
    let mod_vks: Vec<VIRTUAL_KEY> = modifiers.iter().filter_map(|m| modifier_vk(m)).collect();

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

    with_confirmed_foreground(
        target,
        "key delivery",
        ForegroundAdmissionPolicy::TargetOrOwned,
        focus,
        |_| unsafe {
            let sent = SendInput(&events, std::mem::size_of::<INPUT>() as i32);
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
        },
    )
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
/// targets where PostMessage WM_CHAR is silently dropped. Desktop scope keeps
/// this legacy single-SendInput behavior; explicit pid/window foreground text
/// uses [`send_text_synthesized_after_focus`] and its bounded batch admission.
pub fn send_text_synthesized(hwnd: u64, text: &str) -> Result<()> {
    send_text_synthesized_legacy(hwnd, text)
}

/// Preserve the legacy desktop-scope path: one activation and one SendInput
/// call for the complete text. The bounded per-batch experiment is restricted
/// to explicit pid/window foreground delivery.
pub fn send_text_synthesized_legacy(hwnd: u64, text: &str) -> Result<()> {
    let target = HWND(hwnd as *mut _);
    if target.0.is_null() {
        bail!("invalid target hwnd");
    }
    if let Some(msg) = crate::input::post_message_blocked_by_uipi(hwnd) {
        bail!(msg);
    }

    let return_vk = windows::Win32::UI::Input::KeyboardAndMouse::VK_RETURN;
    let mut events = Vec::with_capacity(text.len() * 2);
    let mut previous_was_cr = false;
    for ch in text.chars() {
        match ch {
            '\n' if previous_was_cr => previous_was_cr = false,
            '\r' | '\n' => {
                events.push(key_input(return_vk, false));
                events.push(key_input(return_vk, true));
                previous_was_cr = ch == '\r';
            }
            _ => {
                previous_was_cr = false;
                let mut utf16 = [0u16; 2];
                for unit in ch.encode_utf16(&mut utf16) {
                    events.push(unicode_key_input(*unit, false));
                    events.push(unicode_key_input(*unit, true));
                }
            }
        }
    }
    if events.is_empty() {
        return Ok(());
    }

    with_confirmed_foreground(
        target,
        "text delivery",
        ForegroundAdmissionPolicy::TargetOrOwned,
        || Ok(()),
        |_| unsafe {
            let sent = SendInput(&events, std::mem::size_of::<INPUT>() as i32);
            if sent as usize != events.len() {
                bail!(
                    "SendInput inserted only {sent} of {} key events. Windows blocked the \
                     rest: the target runs at a higher integrity level than the Driver (UIPI), \
                     or the input desktop is locked or showing a secure prompt.",
                    events.len()
                );
            }
            Ok(())
        },
    )
}

/// Foreground Unicode delivery with child focus established after exact
/// top-level activation and before `SendInput`. Contiguous text runs and each
/// logical Return pair are separate bounded input batches with a fresh exact
/// foreground/editor-focus and cancellation check immediately before sending.
pub fn send_text_synthesized_after_focus(
    hwnd: u64,
    text: &str,
    focus: impl FnOnce() -> Result<()>,
) -> Result<()> {
    send_text_synthesized_after_focus_with_admission(hwnd, text, focus, || Ok(()), || false)
        .map_err(anyhow::Error::new)
}

pub(crate) fn send_text_synthesized_after_focus_with_admission(
    hwnd: u64,
    text: &str,
    focus: impl FnOnce() -> Result<()>,
    verify_editor_focus: impl Fn() -> Result<()>,
    cancelled: impl Fn() -> bool,
) -> std::result::Result<(), ForegroundTextSendError> {
    let target = HWND(hwnd as *mut _);
    if target.0.is_null() {
        return Err(ForegroundTextSendError::before_input(
            ForegroundTextErrorCode::ForegroundUnavailable,
            "invalid target HWND",
        ));
    }
    if let Some(msg) = crate::input::post_message_blocked_by_uipi(hwnd) {
        return Err(ForegroundTextSendError::before_input(
            ForegroundTextErrorCode::ForegroundUnavailable,
            msg,
        ));
    }
    let plan = plan_foreground_text_batches(text)?;
    if plan.is_empty() {
        return Ok(());
    }
    // Lower all bounded batches before activation. The final admission checks
    // can therefore sit immediately beside each SendInput call.
    let input_batches: Vec<Vec<INPUT>> = plan.iter().map(foreground_text_batch_inputs).collect();
    if cancelled() {
        return Err(ForegroundTextSendError::before_input(
            ForegroundTextErrorCode::Cancelled,
            "the caller cancelled text delivery before target activation",
        ));
    }

    let focus = move || {
        focus().map_err(|error| {
            ForegroundTextSendError::before_input(
                ForegroundTextErrorCode::EditorFocusUnavailable,
                error.to_string(),
            )
        })
    };

    with_confirmed_foreground(
        target,
        "text delivery",
        ForegroundAdmissionPolicy::ExactTarget,
        focus,
        |check_foreground| {
            let expected_focus = strict_focused_hwnd(target).ok_or_else(|| {
                ForegroundTextSendError::before_input(
                    ForegroundTextErrorCode::EditorFocusUnavailable,
                    "Windows could not identify a focused HWND belonging to the exact target",
                )
            })?;
            let expected_focus_addr = expected_focus.0 as usize as u64;
            let target_addr = target.0 as usize as u64;
            let mut admission = |batch_index: usize| {
                verify_editor_focus().map_err(|error| BatchAdmissionFailure {
                    code: ForegroundTextErrorCode::EditorFocusUnavailable,
                    message: error.to_string(),
                })?;

                let actual_focus = strict_focused_hwnd(target).map(|hwnd| hwnd.0 as usize as u64);
                if !exact_editor_focus_matches(expected_focus_addr, actual_focus) {
                    return Err(BatchAdmissionFailure {
                        code: ForegroundTextErrorCode::EditorFocusUnavailable,
                        message: format!(
                            "the exact target/editor focus changed before batch {}",
                            batch_index + 1
                        ),
                    });
                }
                let actual_foreground = unsafe { GetForegroundWindow() }.0 as usize as u64;
                check_foreground(actual_foreground).map_err(|failure| BatchAdmissionFailure {
                    code: ForegroundTextErrorCode::ForegroundUnavailable,
                    message: format!(
                        "the original target HWND is not foreground before batch {} (actual HWND {:#x})",
                        batch_index + 1,
                        failure.actual_hwnd
                    ),
                })?;
                if actual_foreground != target_addr {
                    return Err(BatchAdmissionFailure {
                        code: ForegroundTextErrorCode::ForegroundUnavailable,
                        message: format!(
                            "the original target HWND is not foreground before batch {}",
                            batch_index + 1
                        ),
                    });
                }
                Ok(())
            };

            execute_foreground_text_batches(
                &input_batches,
                &mut admission,
                cancelled,
                |events| unsafe { SendInput(events, std::mem::size_of::<INPUT>() as i32) as usize },
            )
        },
    )
}

/// Maximum total number of keyboard INPUT events buffered by pid/window
/// foreground text delivery before activation.
pub const MAX_FOREGROUND_TEXT_EVENTS: usize = 32_768;
/// Maximum number of INPUT events in one contiguous Unicode run.
pub const MAX_FOREGROUND_TEXT_RUN_EVENTS: usize = 16_384;
/// Maximum number of separate SendInput batches in one pid/window request.
pub const MAX_FOREGROUND_TEXT_BATCHES: usize = 1_024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ForegroundTextErrorCode {
    InputTooLarge,
    Cancelled,
    ForegroundUnavailable,
    EditorFocusUnavailable,
    SendInputRejected,
    SendInputIncomplete,
    PartialInputUnknown,
}

impl ForegroundTextErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InputTooLarge => "input_too_large",
            Self::Cancelled => "cancelled",
            Self::ForegroundUnavailable => "foreground_unavailable",
            Self::EditorFocusUnavailable => "editor_focus_unavailable",
            Self::SendInputRejected => "sendinput_rejected",
            Self::SendInputIncomplete => "sendinput_incomplete",
            Self::PartialInputUnknown => "partial_input_unknown",
        }
    }
}

impl std::fmt::Display for ForegroundTextErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum ForegroundTextSendError {
    NotStarted {
        code: ForegroundTextErrorCode,
        message: String,
    },
    Partial {
        accepted_events: usize,
        batch_index: usize,
        batch_count: usize,
        cause_code: ForegroundTextErrorCode,
        message: String,
    },
}

impl ForegroundTextSendError {
    fn before_input(code: ForegroundTextErrorCode, message: impl Into<String>) -> Self {
        Self::NotStarted {
            code,
            message: message.into(),
        }
    }

    fn after_input(
        accepted_events: usize,
        batch_index: usize,
        batch_count: usize,
        cause_code: ForegroundTextErrorCode,
        message: impl Into<String>,
    ) -> Self {
        Self::Partial {
            accepted_events,
            batch_index,
            batch_count,
            cause_code,
            message: message.into(),
        }
    }

    pub fn public_code(&self) -> ForegroundTextErrorCode {
        match self {
            Self::NotStarted { code, .. } => *code,
            Self::Partial { .. } => ForegroundTextErrorCode::PartialInputUnknown,
        }
    }
}

impl std::fmt::Display for ForegroundTextSendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotStarted { code, message } => write!(f, "{code}: {message}"),
            Self::Partial {
                accepted_events,
                batch_index,
                batch_count,
                cause_code,
                message,
            } => write!(
                f,
                "partial_input_unknown: SendInput accepted {accepted_events} event(s) at batch {} of {}; application delivery is unknown ({cause_code}: {message}). Input was not replayed; do not automatically retry.",
                batch_index + 1,
                batch_count
            ),
        }
    }
}

impl std::error::Error for ForegroundTextSendError {}

impl From<anyhow::Error> for ForegroundTextSendError {
    fn from(error: anyhow::Error) -> Self {
        Self::before_input(
            ForegroundTextErrorCode::ForegroundUnavailable,
            error.to_string(),
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ForegroundTextBatch<'a> {
    UnicodeRun(&'a str),
    Return,
}

#[derive(Debug, Eq, PartialEq)]
struct BatchAdmissionFailure {
    code: ForegroundTextErrorCode,
    message: String,
}

/// Plan foreground Unicode input as maximal non-newline runs and logical
/// Return key pairs. CRLF is one logical Return; lone CR and LF are equivalent.
/// Keeping each Rust `char` intact in its run prevents planning from splitting
/// a UTF-16 surrogate pair across input batches.
fn plan_foreground_text_batches(
    text: &str,
) -> std::result::Result<Vec<ForegroundTextBatch<'_>>, ForegroundTextSendError> {
    let mut batches = Vec::new();
    let mut unicode_run_start = None;
    let mut unicode_run_events = 0usize;
    let mut previous_was_cr = false;
    let mut event_count = 0usize;

    for (byte_index, ch) in text.char_indices() {
        if ch == '\n' && previous_was_cr {
            previous_was_cr = false;
            continue;
        }

        if ch == '\r' || ch == '\n' {
            if let Some(start) = unicode_run_start.take() {
                push_foreground_text_batch(
                    &mut batches,
                    ForegroundTextBatch::UnicodeRun(&text[start..byte_index]),
                )?;
            }
            unicode_run_events = 0;
            event_count = add_foreground_text_events(event_count, 2)?;
            push_foreground_text_batch(&mut batches, ForegroundTextBatch::Return)?;
            previous_was_cr = ch == '\r';
        } else {
            unicode_run_start.get_or_insert(byte_index);
            let added_events = ch.len_utf16() * 2;
            unicode_run_events = unicode_run_events
                .checked_add(added_events)
                .filter(|count| *count <= MAX_FOREGROUND_TEXT_RUN_EVENTS)
                .ok_or_else(|| {
                    ForegroundTextSendError::before_input(
                        ForegroundTextErrorCode::InputTooLarge,
                        format!(
                            "a foreground Unicode run exceeds the {MAX_FOREGROUND_TEXT_RUN_EVENTS}-event limit"
                        ),
                    )
                })?;
            event_count = add_foreground_text_events(event_count, added_events)?;
            previous_was_cr = false;
        }
    }

    if let Some(start) = unicode_run_start {
        push_foreground_text_batch(
            &mut batches,
            ForegroundTextBatch::UnicodeRun(&text[start..]),
        )?;
    }
    Ok(batches)
}

fn add_foreground_text_events(
    current: usize,
    added: usize,
) -> std::result::Result<usize, ForegroundTextSendError> {
    let total = current.checked_add(added).ok_or_else(|| {
        ForegroundTextSendError::before_input(
            ForegroundTextErrorCode::InputTooLarge,
            "foreground text event count overflowed",
        )
    })?;
    if total > MAX_FOREGROUND_TEXT_EVENTS {
        return Err(ForegroundTextSendError::before_input(
            ForegroundTextErrorCode::InputTooLarge,
            format!("foreground text exceeds the {MAX_FOREGROUND_TEXT_EVENTS}-event limit"),
        ));
    }
    Ok(total)
}

fn push_foreground_text_batch<'a>(
    batches: &mut Vec<ForegroundTextBatch<'a>>,
    batch: ForegroundTextBatch<'a>,
) -> std::result::Result<(), ForegroundTextSendError> {
    if batches.len() >= MAX_FOREGROUND_TEXT_BATCHES {
        return Err(ForegroundTextSendError::before_input(
            ForegroundTextErrorCode::InputTooLarge,
            format!("foreground text exceeds the {MAX_FOREGROUND_TEXT_BATCHES}-batch limit"),
        ));
    }
    batches.push(batch);
    Ok(())
}

fn foreground_text_batch_inputs(batch: &ForegroundTextBatch<'_>) -> Vec<INPUT> {
    match batch {
        ForegroundTextBatch::UnicodeRun(text) => unicode_run_inputs(text),
        ForegroundTextBatch::Return => {
            let return_vk = windows::Win32::UI::Input::KeyboardAndMouse::VK_RETURN;
            vec![key_input(return_vk, false), key_input(return_vk, true)]
        }
    }
}

fn unicode_run_inputs(text: &str) -> Vec<INPUT> {
    // Planning has already capped all Unicode batches before this allocation.
    let mut events = Vec::with_capacity(text.len() * 2);
    for ch in text.chars() {
        let mut buf = [0u16; 2];
        for unit in ch.encode_utf16(&mut buf) {
            events.push(unicode_key_input(*unit, false));
            events.push(unicode_key_input(*unit, true));
        }
    }
    events
}

fn execute_foreground_text_batches<E>(
    batches: &[Vec<E>],
    mut admission: impl FnMut(usize) -> std::result::Result<(), BatchAdmissionFailure>,
    mut cancelled: impl FnMut() -> bool,
    mut send: impl FnMut(&[E]) -> usize,
) -> std::result::Result<(), ForegroundTextSendError> {
    let mut accepted_events = 0usize;
    for (batch_index, events) in batches.iter().enumerate() {
        let failure = if cancelled() {
            Some(BatchAdmissionFailure {
                code: ForegroundTextErrorCode::Cancelled,
                message: "the caller cancelled text delivery before this batch".to_owned(),
            })
        } else {
            admission(batch_index).err()
        };
        if let Some(failure) = failure {
            return Err(batch_failure_result(
                failure,
                accepted_events,
                batch_index,
                batches.len(),
            ));
        }
        if cancelled() {
            return Err(batch_failure_result(
                BatchAdmissionFailure {
                    code: ForegroundTextErrorCode::Cancelled,
                    message: "the caller cancelled text delivery during admission for this batch"
                        .to_owned(),
                },
                accepted_events,
                batch_index,
                batches.len(),
            ));
        }

        let sent = send(events);
        if sent > events.len() {
            return Err(batch_failure_result(
                BatchAdmissionFailure {
                    code: ForegroundTextErrorCode::SendInputIncomplete,
                    message: format!(
                        "the sender reported {sent} accepted events for a {}-event batch",
                        events.len()
                    ),
                },
                accepted_events,
                batch_index,
                batches.len(),
            ));
        }
        accepted_events += sent;
        if sent != events.len() {
            let failure = BatchAdmissionFailure {
                code: if accepted_events == 0 {
                    ForegroundTextErrorCode::SendInputRejected
                } else {
                    ForegroundTextErrorCode::SendInputIncomplete
                },
                message: format!(
                    "the sender accepted {sent} of {} events in this batch",
                    events.len()
                ),
            };
            return Err(batch_failure_result(
                failure,
                accepted_events,
                batch_index,
                batches.len(),
            ));
        }
    }
    Ok(())
}

fn batch_failure_result(
    failure: BatchAdmissionFailure,
    accepted_events: usize,
    batch_index: usize,
    batch_count: usize,
) -> ForegroundTextSendError {
    if accepted_events == 0 {
        ForegroundTextSendError::before_input(failure.code, failure.message)
    } else {
        ForegroundTextSendError::after_input(
            accepted_events,
            batch_index,
            batch_count,
            failure.code,
            failure.message,
        )
    }
}

fn strict_focused_hwnd(parent: HWND) -> Option<HWND> {
    if parent.0.is_null() {
        return None;
    }
    if let Some(descendant) = focused_descendant(parent) {
        return Some(descendant);
    }

    let target_thread = unsafe { GetWindowThreadProcessId(parent, None) };
    if target_thread == 0 {
        return None;
    }
    let mut info = GUITHREADINFO {
        cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
        ..Default::default()
    };
    unsafe { GetGUIThreadInfo(target_thread, &mut info) }.ok()?;
    let focused = info.hwndFocus;
    if focused == parent || (!focused.0.is_null() && unsafe { IsChild(parent, focused) }.as_bool())
    {
        Some(focused)
    } else {
        None
    }
}

fn exact_editor_focus_matches(expected_focus_hwnd: u64, actual_focus_hwnd: Option<u64>) -> bool {
    actual_focus_hwnd == Some(expected_focus_hwnd)
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ForegroundAdmissionPolicy {
    TargetOrOwned,
    ExactTarget,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ForegroundAdmissionFailure {
    actual_hwnd: u64,
}

/// Run one system-queue input transaction under the selected exact-target or
/// target-or-owned foreground policy. The prior foreground is restored on
/// every body/focus result after activation succeeds.
fn with_confirmed_foreground<T, E>(
    target: HWND,
    operation: &str,
    policy: ForegroundAdmissionPolicy,
    focus: impl FnOnce() -> std::result::Result<(), E>,
    body: impl FnOnce(
        &dyn Fn(u64) -> std::result::Result<(), ForegroundAdmissionFailure>,
    ) -> std::result::Result<T, E>,
) -> std::result::Result<T, E>
where
    E: From<anyhow::Error>,
{
    let original_target_snapshot = if policy == ForegroundAdmissionPolicy::ExactTarget {
        Some(
            crate::win32::capture_foreground_target(target.0 as usize as u64).ok_or_else(|| {
                anyhow::anyhow!(
                    "foreground_unavailable: could not capture the original target HWND before activation"
                )
            })?,
        )
    } else {
        None
    };
    let previous = unsafe { GetForegroundWindow() };
    let _ = unsafe { crate::input::force_foreground_assisted(target) };
    if !wait_for_exact_foreground(target, Duration::from_millis(500)) {
        let actual = unsafe { GetForegroundWindow() };
        if !previous.0.is_null() && previous != target {
            let _ = unsafe { SetForegroundWindow(previous) };
        }
        return Err(anyhow::anyhow!(
            "foreground_unavailable: Windows did not confirm exact target HWND {:?} for {operation} \
             within 500 ms (actual foreground HWND {:?}). Windows refused the foreground \
             change, usually because another window holds the foreground lock; no input \
             was sent. Retry, or use background delivery where the target accepts it.",
            target.0,
            actual.0
        )
        .into());
    }

    let Some(foreground_target) = original_target_snapshot
        .or_else(|| crate::win32::capture_foreground_target(target.0 as usize as u64))
    else {
        if !previous.0.is_null() && previous != target {
            let _ = unsafe { SetForegroundWindow(previous) };
        }
        return Err(anyhow::anyhow!(
            "foreground_unavailable: exact target HWND {:?} disappeared before {operation}; \
             no input was sent",
            target.0
        )
        .into());
    };

    let check_foreground = |actual_hwnd: u64| {
        let admitted = match policy {
            ForegroundAdmissionPolicy::ExactTarget => {
                crate::win32::foreground_matches_exact_target(foreground_target, actual_hwnd)
            }
            ForegroundAdmissionPolicy::TargetOrOwned => {
                crate::win32::foreground_matches_target_or_owned_window(
                    foreground_target,
                    actual_hwnd,
                )
            }
        };
        if !admitted {
            return Err(ForegroundAdmissionFailure { actual_hwnd });
        }
        Ok(())
    };

    let result = (|| -> std::result::Result<T, E> {
        let before_focus = unsafe { GetForegroundWindow() };
        if let Err(failure) = check_foreground(before_focus.0 as usize as u64) {
            return Err(anyhow::anyhow!(
                "foreground_unavailable: original target HWND {:?} was not admitted before \
                 preparing {operation} (actual foreground HWND {:?}); no input was sent",
                target.0,
                failure.actual_hwnd
            )
            .into());
        }
        focus()?;

        let actual = if policy == ForegroundAdmissionPolicy::TargetOrOwned {
            let deadline = Instant::now() + Duration::from_millis(250);
            loop {
                let actual = unsafe { GetForegroundWindow() };
                if check_foreground(actual.0 as usize as u64).is_ok() || Instant::now() >= deadline
                {
                    break actual;
                }
                sleep(Duration::from_millis(10));
            }
        } else {
            unsafe { GetForegroundWindow() }
        };
        if let Err(failure) = check_foreground(actual.0 as usize as u64) {
            return Err(anyhow::anyhow!(
                "foreground_unavailable: foreground admission for {operation} requires the \
                 original target HWND {:?}; actual foreground HWND {:?}. No input was sent.",
                target.0,
                failure.actual_hwnd
            )
            .into());
        }
        body(&check_foreground)
    })();

    // Give the target message loop a bounded opportunity to consume the
    // inserted sequence before restoring the user's prior foreground.
    if result.is_ok() {
        sleep(Duration::from_millis(40));
    }
    if !previous.0.is_null() && previous != target {
        let _ = unsafe { SetForegroundWindow(previous) };
    }
    result
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
    let scan = unsafe { MapVirtualKeyW(vk.0 as u32, MAPVK_VK_TO_VSC) } as u16;
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
mod foreground_text_batch_tests {
    use super::*;

    fn run(text: &str) -> ForegroundTextBatch<'_> {
        ForegroundTextBatch::UnicodeRun(text)
    }

    #[test]
    fn plans_text_runs_and_logical_returns() {
        let cases = [
            ("", vec![]),
            ("single line", vec![run("single line")]),
            (
                "A\nB",
                vec![run("A"), ForegroundTextBatch::Return, run("B")],
            ),
            (
                "A\rB",
                vec![run("A"), ForegroundTextBatch::Return, run("B")],
            ),
            (
                "A\r\nB",
                vec![run("A"), ForegroundTextBatch::Return, run("B")],
            ),
            (
                "A\n\nB",
                vec![
                    run("A"),
                    ForegroundTextBatch::Return,
                    ForegroundTextBatch::Return,
                    run("B"),
                ],
            ),
            (
                "\nA\n",
                vec![
                    ForegroundTextBatch::Return,
                    run("A"),
                    ForegroundTextBatch::Return,
                ],
            ),
            (
                "\u{4e2d}\u{6587}123\u{6570}",
                vec![run("\u{4e2d}\u{6587}123\u{6570}")],
            ),
            ("A\u{1f642}\u{1f680}B", vec![run("A\u{1f642}\u{1f680}B")]),
        ];

        for (text, expected) in cases {
            assert_eq!(
                plan_foreground_text_batches(text).unwrap(),
                expected,
                "{text:?}"
            );
        }
    }

    #[test]
    fn unicode_utf16_units_keep_down_up_pairs_inside_one_run() {
        let text = "A\u{1f642}B";
        let plan = plan_foreground_text_batches(text).unwrap();
        assert_eq!(plan.len(), 1);
        let batch = plan[0];
        let ForegroundTextBatch::UnicodeRun(run) = batch else {
            panic!("expected one Unicode run");
        };
        let events = unicode_run_inputs(&run);
        let expected_units = [0x0041, 0xd83d, 0xde42, 0x0042];
        assert_eq!(events.len(), expected_units.len() * 2);

        for (unit_index, expected_unit) in expected_units.into_iter().enumerate() {
            let down = unsafe { events[unit_index * 2].Anonymous.ki };
            let up = unsafe { events[unit_index * 2 + 1].Anonymous.ki };
            assert_eq!(down.wScan, expected_unit);
            assert_eq!(up.wScan, expected_unit);
            assert_ne!(down.dwFlags.0 & KEYEVENTF_UNICODE.0, 0);
            assert_ne!(up.dwFlags.0 & KEYEVENTF_UNICODE.0, 0);
            assert_eq!(down.dwFlags.0 & KEYEVENTF_KEYUP.0, 0);
            assert_ne!(up.dwFlags.0 & KEYEVENTF_KEYUP.0, 0);
        }
    }

    #[test]
    fn each_return_is_one_down_up_batch() {
        let returns = plan_foreground_text_batches("\n\r\r\n").unwrap();
        assert_eq!(returns, vec![ForegroundTextBatch::Return; 3]);

        for batch in &returns {
            let events = foreground_text_batch_inputs(batch);
            assert_eq!(events.len(), 2);
            let down = unsafe { events[0].Anonymous.ki };
            let up = unsafe { events[1].Anonymous.ki };
            let return_vk = windows::Win32::UI::Input::KeyboardAndMouse::VK_RETURN;
            let return_scan = unsafe { MapVirtualKeyW(return_vk.0 as u32, MAPVK_VK_TO_VSC) } as u16;
            if return_scan == 0 {
                assert_eq!(down.wVk, return_vk);
                assert_eq!(up.wVk, return_vk);
            } else {
                assert_eq!(down.wVk, VIRTUAL_KEY(0));
                assert_eq!(up.wVk, VIRTUAL_KEY(0));
                assert_eq!(down.wScan, return_scan);
                assert_eq!(up.wScan, return_scan);
                assert_ne!(down.dwFlags.0 & KEYEVENTF_SCANCODE.0, 0);
                assert_ne!(up.dwFlags.0 & KEYEVENTF_SCANCODE.0, 0);
            }
            assert_eq!(down.dwFlags.0 & KEYEVENTF_KEYUP.0, 0);
            assert_ne!(up.dwFlags.0 & KEYEVENTF_KEYUP.0, 0);
            assert_eq!(down.dwFlags.0 & KEYEVENTF_EXTENDEDKEY.0, 0);
            assert_eq!(up.dwFlags.0 & KEYEVENTF_EXTENDEDKEY.0, 0);
        }
    }
}

#[cfg(test)]
mod foreground_text_dispatch_tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    fn batches() -> Vec<Vec<u8>> {
        vec![vec![1, 2], vec![3, 4], vec![5, 6]]
    }

    fn admit_all(_: usize) -> std::result::Result<(), BatchAdmissionFailure> {
        Ok(())
    }

    #[test]
    fn each_prebuilt_batch_is_admitted_once_immediately_before_send() {
        let trace = RefCell::new(Vec::new());
        let result = execute_foreground_text_batches(
            &batches(),
            |_| {
                trace.borrow_mut().push("admit");
                Ok(())
            },
            || false,
            |events| {
                trace.borrow_mut().push("send");
                events.len()
            },
        );

        assert!(result.is_ok());
        assert_eq!(
            *trace.borrow(),
            ["admit", "send", "admit", "send", "admit", "send"]
        );
    }

    fn assert_partial(
        error: ForegroundTextSendError,
        expected_accepted: usize,
        expected_batch: usize,
        expected_count: usize,
        expected_cause: ForegroundTextErrorCode,
    ) {
        match error {
            ForegroundTextSendError::Partial {
                accepted_events,
                batch_index,
                batch_count,
                cause_code,
                ..
            } => {
                assert_eq!(accepted_events, expected_accepted);
                assert_eq!(batch_index, expected_batch);
                assert_eq!(batch_count, expected_count);
                assert_eq!(cause_code, expected_cause);
            }
            other => panic!("expected partial input error, got {other:?}"),
        }
    }

    #[test]
    fn first_batch_short_send_is_partial_and_stops_without_replay() {
        let mut sends = 0;
        let result = execute_foreground_text_batches(
            &batches(),
            admit_all,
            || false,
            |events| {
                sends += 1;
                if sends == 1 {
                    1
                } else {
                    events.len()
                }
            },
        );

        assert_partial(
            result.unwrap_err(),
            1,
            0,
            3,
            ForegroundTextErrorCode::SendInputIncomplete,
        );
        assert_eq!(sends, 1, "failed input must not be replayed or continued");
    }

    #[test]
    fn later_batch_short_send_keeps_cumulative_count_and_stops() {
        let mut sends = 0;
        let result = execute_foreground_text_batches(
            &batches(),
            admit_all,
            || false,
            |events| {
                sends += 1;
                if sends == 2 {
                    1
                } else {
                    events.len()
                }
            },
        );

        assert_partial(
            result.unwrap_err(),
            3,
            1,
            3,
            ForegroundTextErrorCode::SendInputIncomplete,
        );
        assert_eq!(
            sends, 2,
            "later batches must not be sent after a short count"
        );
    }

    #[test]
    fn zero_events_accepted_in_first_batch_is_not_started() {
        let mut sends = 0;
        let result = execute_foreground_text_batches(
            &batches(),
            admit_all,
            || false,
            |_| {
                sends += 1;
                0
            },
        )
        .unwrap_err();

        assert!(matches!(
            result,
            ForegroundTextSendError::NotStarted {
                code: ForegroundTextErrorCode::SendInputRejected,
                ..
            }
        ));
        assert_eq!(sends, 1);
    }

    #[test]
    fn editor_focus_change_stops_remaining_batches() {
        let mut sends = 0;
        let result = execute_foreground_text_batches(
            &batches(),
            |index| {
                if index == 1 {
                    Err(BatchAdmissionFailure {
                        code: ForegroundTextErrorCode::EditorFocusUnavailable,
                        message: "the exact editor lost keyboard focus".into(),
                    })
                } else {
                    Ok(())
                }
            },
            || false,
            |events| {
                sends += 1;
                events.len()
            },
        );

        assert_partial(
            result.unwrap_err(),
            2,
            1,
            3,
            ForegroundTextErrorCode::EditorFocusUnavailable,
        );
        assert_eq!(sends, 1, "no batch follows a failed editor admission check");
    }

    #[test]
    fn owned_popup_or_vanished_target_stops_remaining_batches() {
        for message in [
            "foreground is an owned popup, not the original target",
            "the original target HWND disappeared",
        ] {
            let mut sends = 0;
            let result = execute_foreground_text_batches(
                &batches(),
                |index| {
                    if index == 1 {
                        Err(BatchAdmissionFailure {
                            code: ForegroundTextErrorCode::ForegroundUnavailable,
                            message: message.into(),
                        })
                    } else {
                        Ok(())
                    }
                },
                || false,
                |events| {
                    sends += 1;
                    events.len()
                },
            );

            assert_partial(
                result.unwrap_err(),
                2,
                1,
                3,
                ForegroundTextErrorCode::ForegroundUnavailable,
            );
            assert_eq!(sends, 1, "no batch follows a failed target admission check");
        }
    }

    #[test]
    fn cancellation_after_a_batch_stops_without_replay() {
        let cancelled = Cell::new(false);
        let mut sends = 0;
        let result = execute_foreground_text_batches(
            &batches(),
            admit_all,
            || cancelled.get(),
            |events| {
                sends += 1;
                cancelled.set(true);
                events.len()
            },
        );

        assert_partial(
            result.unwrap_err(),
            2,
            1,
            3,
            ForegroundTextErrorCode::Cancelled,
        );
        assert_eq!(sends, 1, "cancellation is checked before every batch");
    }

    #[test]
    fn cancellation_during_admission_is_rechecked_before_send() {
        let cancelled = Cell::new(false);
        let mut sends = 0;
        let result = execute_foreground_text_batches(
            &batches(),
            |index| {
                if index == 1 {
                    cancelled.set(true);
                }
                Ok(())
            },
            || cancelled.get(),
            |events| {
                sends += 1;
                events.len()
            },
        );

        assert_partial(
            result.unwrap_err(),
            2,
            1,
            3,
            ForegroundTextErrorCode::Cancelled,
        );
        assert_eq!(
            sends, 1,
            "cancellation during the admission check blocks this batch"
        );
    }

    #[test]
    fn cancellation_during_first_admission_sends_nothing_and_stops_remaining_batches() {
        let cancelled = Cell::new(false);
        let mut admissions = 0;
        let mut sends = 0;
        let result = execute_foreground_text_batches(
            &batches(),
            |index| {
                admissions += 1;
                assert_eq!(index, 0);
                cancelled.set(true);
                Ok(())
            },
            || cancelled.get(),
            |events| {
                sends += 1;
                events.len()
            },
        );

        assert!(matches!(
            result,
            Err(ForegroundTextSendError::NotStarted {
                code: ForegroundTextErrorCode::Cancelled,
                ..
            })
        ));
        assert_eq!(admissions, 1, "remaining batches are not admitted");
        assert_eq!(sends, 0, "no events are sent after admission cancellation");
    }

    #[test]
    fn exact_editor_focus_rejects_sibling_and_missing_target() {
        assert!(exact_editor_focus_matches(11, Some(11)));
        assert!(!exact_editor_focus_matches(11, Some(12))); // changed child
        assert!(!exact_editor_focus_matches(11, None)); // target disappeared
    }

    #[test]
    fn oversized_run_total_and_batch_count_are_rejected_during_planning() {
        let oversized_run = "x".repeat(MAX_FOREGROUND_TEXT_RUN_EVENTS / 2 + 1);
        assert_eq!(
            plan_foreground_text_batches(&oversized_run)
                .unwrap_err()
                .public_code(),
            ForegroundTextErrorCode::InputTooLarge
        );

        let oversized_total = format!(
            "{}\n{}",
            "x".repeat(MAX_FOREGROUND_TEXT_RUN_EVENTS / 2),
            "y".repeat(MAX_FOREGROUND_TEXT_RUN_EVENTS / 2)
        );
        assert_eq!(
            plan_foreground_text_batches(&oversized_total)
                .unwrap_err()
                .public_code(),
            ForegroundTextErrorCode::InputTooLarge
        );

        let oversized_batch_count = "\n".repeat(MAX_FOREGROUND_TEXT_BATCHES + 1);
        assert_eq!(
            plan_foreground_text_batches(&oversized_batch_count)
                .unwrap_err()
                .public_code(),
            ForegroundTextErrorCode::InputTooLarge
        );
    }

    #[test]
    fn single_line_stays_one_unicode_batch() {
        let plan = plan_foreground_text_batches("abc123").unwrap();
        assert_eq!(plan, vec![ForegroundTextBatch::UnicodeRun("abc123")]);
        assert_eq!(foreground_text_batch_inputs(&plan[0]).len(), 12);
    }
}
