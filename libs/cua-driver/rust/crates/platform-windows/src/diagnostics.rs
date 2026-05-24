//! Windows-specific environment probes used by `cua-driver doctor`.
//!
//! These are read-only checks against the process environment + Win32 APIs.
//! They never block, never touch the network, and never mutate state — they
//! exist purely so `doctor` can report a structured snapshot of "is this
//! process actually able to drive GUIs on this host".
//!
//! ## Why the session-id probe matters
//!
//! Window-driving tools (`list_windows`, `click`, `type_text`, `screenshot`,
//! `get_window_state`) all bottom out in Win32 APIs that are scoped to the
//! calling process's WindowStation + Desktop. A process that lives in
//! Session 0 (services) — which is where Windows lands all SSH-launched
//! processes by default — has no attached interactive desktop, so
//! `EnumWindows` / `GetForegroundWindow` / `PrintWindow` silently return
//! empty results. That looks like the tools are broken; they aren't. The
//! session probe surfaces this misconfiguration directly so users know to
//! re-run from an interactive logon (RDP, console, or a scheduled task in
//! the user's session) instead of debugging a non-bug.

#[cfg(target_os = "windows")]
use windows::core::PCWSTR;
#[cfg(target_os = "windows")]
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED,
};
#[cfg(target_os = "windows")]
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
#[cfg(target_os = "windows")]
use windows::Win32::System::StationsAndDesktops::{
    CloseWindowStation, OpenWindowStationW,
};
#[cfg(target_os = "windows")]
use windows::Win32::System::Threading::GetCurrentProcessId;
#[cfg(target_os = "windows")]
use windows::Win32::UI::Accessibility::{CUIAutomation, IUIAutomation};
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

/// Session ID of the calling process via `ProcessIdToSessionId`.
///
/// Returns `None` only when the API call itself fails — a healthy process
/// always has a session id, even if it's `0` (services session).
#[cfg(target_os = "windows")]
pub fn current_session_id() -> Option<u32> {
    unsafe {
        let pid = GetCurrentProcessId();
        let mut sid: u32 = 0;
        if ProcessIdToSessionId(pid, &mut sid).is_ok() {
            Some(sid)
        } else {
            None
        }
    }
}

/// Whether the calling process has an attached interactive desktop.
///
/// Two-step check:
///   1. `OpenWindowStationW(L"WinSta0", ...)` — succeeds when the
///      interactive window station exists and we can open a handle.
///   2. `GetForegroundWindow()` — returns a non-null HWND when a desktop
///      with a foreground window is actually attached (i.e. an
///      interactive logon session is active, not just a locked screen
///      with no foreground app).
///
/// `Ok(true)` only when both succeed. `Ok(false)` when `OpenWindowStationW`
/// succeeded but `GetForegroundWindow` returned null (locked desktop or
/// transient state). `Err` when `OpenWindowStationW` failed outright.
#[cfg(target_os = "windows")]
pub fn interactive_desktop_check() -> Result<bool, String> {
    unsafe {
        let name: Vec<u16> = "WinSta0\0".encode_utf16().collect();
        // 0 access mask = read-only probe; we never actually need to read
        // or modify the station, just confirm a handle is openable.
        let h = OpenWindowStationW(PCWSTR(name.as_ptr()), false, 0)
            .map_err(|e| format!("OpenWindowStationW(WinSta0): {e}"))?;
        let fg = GetForegroundWindow();
        let _ = CloseWindowStation(h);
        Ok(fg.0 != std::ptr::null_mut())
    }
}

/// Probe whether `CoCreateInstance(CUIAutomation)` succeeds — the same
/// COM call used by `get_window_state` / `list_windows` element-walks.
///
/// Initialises COM (apartment-threaded) for the duration of the probe
/// and uninitialises before returning so the rest of the doctor run is
/// unaffected.
///
/// **COM lifecycle invariant** — the IUIAutomation interface produced by
/// CoCreateInstance must be released BEFORE CoUninitialize tears down
/// the apartment, or `IUnknown::Release` will dereference freed COM
/// infrastructure and segfault (0xC0000005 ACCESS_VIOLATION). We flatten
/// the probe result into a plain `Result<(), String>` first so the
/// `IUIAutomation` is dropped at the end of the statement, then call
/// CoUninitialize on the next line.
#[cfg(target_os = "windows")]
pub fn ui_automation_available() -> Result<(), String> {
    unsafe {
        // Failure here means COM is already initialised in this thread
        // (likely apartment-threaded too); we only call CoUninitialize
        // when our CoInitializeEx actually paired with a fresh init.
        let init_result = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let need_uninit = init_result.is_ok();

        // Bind to `_` immediately so the IUIAutomation interface is
        // dropped (and `Release()` runs while COM is still up) before
        // we proceed to CoUninitialize. Holding the binding past
        // CoUninitialize and letting Drop run at function return
        // causes a use-after-free in the COM apartment.
        let result = CoCreateInstance::<_, IUIAutomation>(
            &CUIAutomation,
            None,
            CLSCTX_INPROC_SERVER,
        )
        .map(|_| ())
        .map_err(|e| format!("{e}"));

        if need_uninit {
            CoUninitialize();
        }

        result
    }
}

// ── Non-Windows stubs so the cua-driver crate can call into us
// unconditionally during compilation on other targets. These never run —
// they exist purely to keep the type-checker happy on macOS / Linux.

#[cfg(not(target_os = "windows"))]
pub fn current_session_id() -> Option<u32> {
    None
}

#[cfg(not(target_os = "windows"))]
pub fn interactive_desktop_check() -> Result<bool, String> {
    Err("not a Windows host".to_owned())
}

#[cfg(not(target_os = "windows"))]
pub fn ui_automation_available() -> Result<(), String> {
    Err("not a Windows host".to_owned())
}

/// True when `err` is the specific failure shape produced by
/// `CoCreateInstance(CUIAutomation)` running in a non-interactive
/// (Session 0 / service) context where the UI Automation COM server
/// can't be activated. Used by the test below to keep CI matrices that
/// run under SYSTEM from failing on what is an expected outcome.
///
/// We pattern-match on the formatted error string rather than the raw
/// HRESULT because `ui_automation_available` already flattens the COM
/// error to a `String`. The substrings cover the two HRESULTs that
/// surface here in practice — `CO_E_NOTINITIALIZED` (`0x800401F0`) and
/// `REGDB_E_CLASSNOTREG` (`0x80040154`) — plus a generic "not
/// registered" wording the windows crate sometimes emits.
#[cfg(target_os = "windows")]
pub fn is_non_interactive_error(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    lower.contains("0x800401f0")
        || lower.contains("0x80040154")
        || lower.contains("co_e_notinitialized")
        || lower.contains("regdb_e_classnotreg")
        || lower.contains("class not registered")
        || lower.contains("not been initialized")
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;

    #[test]
    fn current_session_id_returns_some_on_windows() {
        // ProcessIdToSessionId on the current process must always succeed.
        // The value depends on the runner: an interactive dev box gives
        // `>0`, CI running as SYSTEM gives `0`.
        assert!(current_session_id().is_some());
    }

    #[test]
    fn ui_automation_available_succeeds_in_test_runner() {
        // On an interactive logon session, CoCreateInstance(CUIAutomation)
        // must succeed. On non-interactive sessions (Session 0, SYSTEM-
        // run CI agents, scheduled tasks without a desktop) it fails in
        // a predictable way — we accept that as a valid outcome rather
        // than failing the test, so the same suite runs green on both
        // dev boxes and headless CI runners.
        match ui_automation_available() {
            Ok(()) => {}
            Err(e) => {
                let non_interactive = current_session_id() == Some(0)
                    || is_non_interactive_error(&e);
                assert!(
                    non_interactive,
                    "ui_automation_available failed in what looks like an interactive session: {e}",
                );
            }
        }
    }
}
