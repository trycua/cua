//! Windows-specific environment probes used by `cua-driver doctor`.
//!
//! These are read-only checks against the process environment + Win32 APIs.
//! They never block, never touch the network, and never mutate state — they
//! exist purely so `doctor` can report a structured snapshot of "is this
//! process actually able to drive GUIs on this host".

#[cfg(target_os = "windows")]
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED,
};
#[cfg(target_os = "windows")]
use windows::Win32::UI::Accessibility::{CUIAutomation, IUIAutomation};

/// Probe whether `CoCreateInstance(CUIAutomation)` succeeds — the same
/// COM call used by `get_window_state` / `list_windows` element-walks.
///
/// Initialises COM (apartment-threaded) for the duration of the probe
/// and uninitialises before returning so the rest of the doctor run is
/// unaffected.
#[cfg(target_os = "windows")]
pub fn ui_automation_available() -> Result<(), String> {
    unsafe {
        // Failure here means COM is already initialised in this thread
        // (likely apartment-threaded too); we only call CoUninitialize
        // when our CoInitializeEx actually paired with a fresh init.
        let init_result = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let need_uninit = init_result.is_ok();

        let probe = CoCreateInstance::<_, IUIAutomation>(
            &CUIAutomation,
            None,
            CLSCTX_INPROC_SERVER,
        );

        if need_uninit {
            CoUninitialize();
        }

        probe.map(|_| ()).map_err(|e| format!("{e}"))
    }
}

// ── Non-Windows stubs so the cua-driver crate can call into us
// unconditionally during compilation on other targets. These never run —
// they exist purely to keep the type-checker happy on macOS / Linux.

#[cfg(not(target_os = "windows"))]
pub fn ui_automation_available() -> Result<(), String> {
    Err("not a Windows host".to_owned())
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;

    #[test]
    fn ui_automation_available_succeeds_in_test_runner() {
        // The test runner runs in an interactive logon session on every
        // supported Windows CI matrix, so CUIAutomation should always
        // CoCreate successfully.
        assert!(ui_automation_available().is_ok());
    }
}
