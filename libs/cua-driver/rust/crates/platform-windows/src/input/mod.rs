//! Background input synthesis for Windows.
//!
//! Strategy (matches CuaDriver.Win reference):
//! - Mouse: PostMessageW(WM_LBUTTONDOWN/UP) with packed LPARAM(x,y).
//!   PostMessage is async and does NOT activate the target window.
//! - Keyboard text: PostMessageW(WM_CHAR) per character.
//! - Key press: PostMessageW(WM_KEYDOWN) + PostMessageW(WM_KEYUP).
//!
//! For clicks, coordinates are window-client-relative (ScreenToClient applied
//! if screen coords are given). We pack with MAKELPARAM(x,y).

pub mod mouse;
pub mod keyboard;

pub use mouse::{is_chromium_target_window, post_click, post_click_screen, send_click_synthesized};
pub use keyboard::{
    is_xaml_host_hwnd, post_char, post_key, post_type_text, post_type_text_with_delay,
    send_key_synthesized,
};

use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND};
use windows::Win32::Security::{
    GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation,
    TokenIntegrityLevel, TOKEN_MANDATORY_LABEL, TOKEN_QUERY,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;

/// Windows mandatory integrity-level RIDs. Higher = more privileged. See
/// https://learn.microsoft.com/en-us/windows/win32/secauthz/mandatory-integrity-control
#[allow(dead_code)]
mod il {
    pub const UNTRUSTED: u32   = 0x0000;
    pub const LOW: u32         = 0x1000;
    pub const MEDIUM: u32      = 0x2000;
    pub const MEDIUM_PLUS: u32 = 0x2100;
    pub const HIGH: u32        = 0x3000;
    pub const SYSTEM: u32      = 0x4000;
}

fn il_name(rid: u32) -> &'static str {
    match rid {
        il::UNTRUSTED   => "Untrusted",
        il::LOW         => "Low",
        il::MEDIUM      => "Medium",
        il::MEDIUM_PLUS => "Medium+",
        il::HIGH        => "High",
        il::SYSTEM      => "System",
        _ => "unknown",
    }
}

/// Read the mandatory integrity level (the last sub-authority of the
/// integrity SID) of a process handle. Returns `None` on any API failure.
unsafe fn process_integrity_rid(process: HANDLE) -> Option<u32> {
    let mut token = HANDLE::default();
    if OpenProcessToken(process, TOKEN_QUERY, &mut token).is_err() {
        return None;
    }
    let mut needed: u32 = 0;
    // First call: probe required buffer size. Returns ERROR_INSUFFICIENT_BUFFER
    // and writes `needed`; we don't check the error because we always need a
    // second call to actually populate `buf`.
    let _ = GetTokenInformation(token, TokenIntegrityLevel, None, 0, &mut needed);
    if needed == 0 {
        let _ = CloseHandle(token);
        return None;
    }
    let mut buf = vec![0u8; needed as usize];
    let ok = GetTokenInformation(
        token,
        TokenIntegrityLevel,
        Some(buf.as_mut_ptr() as _),
        needed,
        &mut needed,
    )
    .is_ok();
    let _ = CloseHandle(token);
    if !ok {
        return None;
    }
    let tml = &*(buf.as_ptr() as *const TOKEN_MANDATORY_LABEL);
    let sid = tml.Label.Sid;
    let count_ptr = GetSidSubAuthorityCount(sid);
    if count_ptr.is_null() {
        return None;
    }
    let count = *count_ptr;
    if count == 0 {
        return None;
    }
    let rid_ptr = GetSidSubAuthority(sid, (count - 1) as u32);
    if rid_ptr.is_null() {
        return None;
    }
    Some(*rid_ptr)
}

/// If posting messages from the current process to `hwnd` would be silently
/// blocked by UIPI (User Interface Privilege Isolation), return a diagnostic
/// string the caller should surface as an actionable error. Otherwise `None`.
///
/// UIPI blocks `PostMessage` / `SendMessage` of input-class messages
/// (`WM_KEYDOWN`, `WM_KEYUP`, `WM_CHAR`, `WM_LBUTTONDOWN`, etc.) from a
/// lower-integrity process to a higher-integrity window. Crucially, the
/// `PostMessage` call still returns `TRUE` — the message is queued but the
/// elevated target's message pump filters it out before delivery. The lower-
/// integrity sender has no way to detect this from the return value, so
/// without an explicit check upstream, `type_text` / `hotkey` / `click`
/// silently no-op against elevated apps.
///
/// Returning a diagnostic string before the `PostMessage` call is the
/// minimum honest behavior: the caller learns it can't drive this target
/// and why. Long-term fix is to route through `SendInput` (system input
/// queue, UIPI-permitted) from the UIAccess'd worker — out of scope here.
pub fn post_message_blocked_by_uipi(hwnd: u64) -> Option<String> {
    let h = HWND(hwnd as *mut _);
    let mut pid: u32 = 0;
    if unsafe { GetWindowThreadProcessId(h, Some(&mut pid)) } == 0 || pid == 0 {
        return None;
    }
    let own = unsafe { process_integrity_rid(GetCurrentProcess()) }?;
    let target_handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let target = unsafe { process_integrity_rid(target_handle) };
    let _ = unsafe { CloseHandle(target_handle) };
    let target = target?;
    if target > own {
        Some(format!(
            "UIPI: target hwnd 0x{hwnd:x} (pid {pid}) is at {} integrity; \
             cua-driver is at {} integrity. PostMessage to a higher-integrity \
             window is silently dropped by the target's message pump — the \
             call would return success but no input would land. Common cause: \
             a Win32 app whose application manifest requests \
             `requireAdministrator` (most Program-Files installs of Notepad++, \
             VS Code system-scope, etc. land at High integrity). Run the \
             daemon elevated to drive these, or use a non-elevated copy of \
             the target. See https://learn.microsoft.com/en-us/windows/win32/winauto/uipi",
            il_name(target),
            il_name(own),
        ))
    } else {
        None
    }
}
