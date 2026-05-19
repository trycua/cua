//! Launch packaged (Microsoft Store / UWP / MSIX) apps on Windows via
//! `IApplicationActivationManager::ActivateApplication`.
//!
//! ## Why this exists
//!
//! On Win11 many built-in apps (Notepad, Calculator, Paint, …) ship as
//! packaged apps. The legacy `notepad.exe` / `calc.exe` / `mspaint.exe`
//! in `C:\Windows\System32\` are now ~7 KB stubs that activate the
//! packaged equivalent and exit almost immediately. Calling
//! `ShellExecuteExW("notepad")` and reading `GetProcessId(hProcess)`
//! therefore returns the **stub's** pid — gone within milliseconds —
//! rather than the pid of the actual UWP process the user can see.
//! `list_windows(pid)` for that stub pid is always empty.
//!
//! `IApplicationActivationManager::ActivateApplication` is the
//! Microsoft-canonical API for launching packaged apps from outside a
//! packaged context. It returns the **real** UWP process pid via its
//! `pid` out-parameter — the pid whose `MainWindowHandle` is the
//! window the user interacts with.
//!
//! ## Two entry points
//!
//! - [`launch_uwp`] — given an AUMID (App User Model ID, the
//!   `{PackageFamilyName}!{ApplicationId}` form a packaged app exposes),
//!   activate it and return the real pid.
//! - [`resolve_aumid_by_name`] — given a display name (e.g. `"Notepad"`),
//!   walk `shell:AppsFolder` (the same virtual folder the Start Menu
//!   reads) and return the matching AUMID. Used as the fallback when a
//!   caller passes a plain name and we want to route through the
//!   packaged-app path instead of `ShellExecuteExW`.
//!
//! Both functions are no-ops / compile errors on non-Windows targets;
//! the module is `#[cfg(target_os = "windows")]`-gated at the crate
//! root.

use std::sync::{OnceLock, RwLock};

use windows::core::{Interface, GUID, HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, IBindCtx, CLSCTX_LOCAL_SERVER,
    COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Shell::PropertiesSystem::PROPERTYKEY;
use windows::Win32::UI::Shell::{
    ApplicationActivationManager, IApplicationActivationManager, IEnumShellItems, IShellItem,
    IShellItem2, SHCreateItemFromParsingName, AO_NONE, BHID_EnumItems, SIGDN_NORMALDISPLAY,
};
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

/// `PKEY_AppUserModel_ID` (System.AppUserModel.ID) — the property key whose
/// string value on each `shell:AppsFolder` entry is the AUMID we need to
/// hand to `ActivateApplication`.
///
/// Defined inline (rather than pulled from `Win32_Storage_EnhancedStorage`)
/// to keep the windows-rs feature surface narrow — adding an entire
/// `Storage` feature subtree just for one constant is not worth the
/// build-time + binary-size cost.
///
/// Reference: `propkey.h` —
///   `DEFINE_PROPERTYKEY(PKEY_AppUserModel_ID,
///     0x9F4C2855, 0x9F79, 0x4B39, 0xA8,0xD0, 0xE1,0xD4,0x2D,0xE1,0xD5,0xF3, 5);`
const PKEY_APP_USER_MODEL_ID: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0x9F4C2855_9F79_4B39_A8D0_E1D42DE1D5F3),
    pid: 5,
};

/// Activate a packaged app by AUMID. Returns the **real** process id of
/// the spawned UWP / MSIX process (not a stub pid).
///
/// `args` is forwarded verbatim as the activation `arguments` string;
/// pass `""` if the app takes no launch arguments. `AO_NONE` keeps
/// behavior identical to a user-driven Start Menu click — splash screen
/// shown, error UI shown on failure, default activation kind.
///
/// **Background-launch invariant** — the AppX runtime's default activation
/// brings the newly-activated app to the foreground (same as a Start Menu
/// click). cua-driver's contract is the opposite: launched apps must not
/// steal focus from whatever the human is doing. We snapshot the pre-call
/// foreground window with `GetForegroundWindow` and restore it after
/// activation returns. This matches macOS's `NSWorkspace.openApplication`
/// + `activates=false` + `oapp` AppleEvent invariant.
///
/// Errors propagate the `HRESULT` from `ActivateApplication`. The most
/// common failures:
/// - `E_INVALIDARG` — AUMID not installed for the current user
/// - `HRESULT 0x80073D54` (`ERROR_INSTALL_RESOLVE_DEPENDENCY_FAILED`) —
///   packaged dependency unresolved (rare)
/// - Any COM-init failure on a thread that has previously been
///   initialized with a conflicting apartment model
pub fn launch_uwp(aumid: &str, args: &str) -> windows::core::Result<u32> {
    // Session-0 short-circuit: `IApplicationActivationManager::ActivateApplication`
    // relies on the per-user AppX runtime that doesn't exist in services /
    // SSH-launched contexts. Calling it in Session 0 hangs indefinitely
    // (no CPU, no progress, no error). Fail fast with a clear message so
    // tools / tests don't time out silently.
    if matches!(crate::diagnostics::current_session_id(), Some(0)) {
        return Err(windows::core::Error::new(
            windows::core::HRESULT(0x80004005u32 as i32), // E_FAIL
            format!("UWP activation of {aumid:?} requires an interactive session — current process is in Session 0 (services). Re-run from an interactive logon (RDP, console, or scheduled task in the user's session)."),
        ));
    }

    // Apartment model: AAM is a local-server COM object that historically
    // requires STA on the calling thread. `CoInitializeEx` is idempotent
    // per-thread; its HRESULT is intentionally discarded — if the thread
    // was previously initialized with a different model, the subsequent
    // CoCreateInstance call will surface a clear error.
    let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };

    let manager: IApplicationActivationManager = unsafe {
        CoCreateInstance(&ApplicationActivationManager, None, CLSCTX_LOCAL_SERVER)?
    };

    let aumid_h = HSTRING::from(aumid);
    let args_h = HSTRING::from(args);

    // Snapshot prior foreground BEFORE activation — that's our restore
    // target. Captured even if null; the restore step is a no-op then.
    let prior_foreground = unsafe { GetForegroundWindow() };

    let pid = unsafe {
        manager.ActivateApplication(
            PCWSTR(aumid_h.as_ptr()),
            PCWSTR(args_h.as_ptr()),
            AO_NONE,
        )?
    };

    // Restore the prior foreground window. AppX activation has already
    // run by this point and may have stolen focus; flip it back so the
    // human's typing context is preserved. The restore is best-effort —
    // failures (null prior, target window destroyed, target hung) are
    // logged at debug level and otherwise swallowed because the launch
    // itself succeeded and we don't want to fail the tool call on a
    // focus-restore corner case.
    restore_foreground_best_effort(prior_foreground);

    Ok(pid)
}

/// Best-effort restore of the foreground window. Run in a tight retry
/// loop because UWP activations sometimes push their own window to
/// foreground a few milliseconds AFTER `ActivateApplication` returns —
/// a single immediate SetForegroundWindow call would race the AppX
/// runtime and lose. Three attempts spaced 50ms apart covers the window.
///
/// **Foreground-lock workaround:** Windows restricts SetForegroundWindow
/// to processes that meet specific conditions (foreground process,
/// owner of last input, etc.). The daemon usually meets none of these
/// when launch_app is called from MCP. We inject a single VK_NONAME
/// synthetic keypress via `keybd_event` to claim "owner of last input"
/// status long enough for SetForegroundWindow to succeed. VK_NONAME
/// (0xFC) is the reserved no-name virtual-key code — it does not map
/// to any UI action and is safe to inject without side effects.
fn restore_foreground_best_effort(prior: HWND) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        keybd_event, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
    };
    use windows::Win32::UI::WindowsAndMessaging::{IsWindow, SetForegroundWindow};

    if prior.0.is_null() {
        tracing::debug!(target: "launch_uwp", "no prior foreground to restore");
        return;
    }

    // Claim "owner of last input" so the next SetForegroundWindow call
    // isn't silently dropped by the foreground lock. VK_NONAME (0xFC)
    // doesn't trigger any application's key handler.
    const VK_NONAME: u8 = 0xFC;
    unsafe {
        keybd_event(VK_NONAME, 0, KEYBD_EVENT_FLAGS(0), 0);
        keybd_event(VK_NONAME, 0, KEYEVENTF_KEYUP, 0);
    }

    for attempt in 0..3 {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        unsafe {
            if !IsWindow(prior).as_bool() {
                tracing::debug!(target: "launch_uwp", "prior foreground HWND no longer valid");
                return;
            }
            let ok = SetForegroundWindow(prior).as_bool();
            tracing::debug!(
                target: "launch_uwp",
                "SetForegroundWindow attempt {} -> {}",
                attempt + 1,
                ok,
            );
            if ok {
                return;
            }
        }
    }
}

/// Heuristic for "this string looks like an AUMID".
///
/// An AUMID is `{PackageFamilyName}!{ApplicationId}`, e.g.
/// `Microsoft.WindowsNotepad_8wekyb3d8bbwe!App`. The unambiguous
/// marker is the literal `!` separator — no Win32 path or executable
/// name legitimately contains `!`, so its presence is a safe signal
/// the caller intends packaged-app activation.
pub fn is_aumid(s: &str) -> bool {
    // Must contain exactly one `!`, with non-empty halves on each side.
    let mut parts = s.split('!');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(pfn), Some(app_id), None) => !pfn.is_empty() && !app_id.is_empty(),
        _ => false,
    }
}

/// Single AppsFolder entry: display name + AUMID.
///
/// `lowercase_display_name` is a one-time precomputation of
/// `display_name.to_lowercase()`, populated at enumeration time. The
/// prefix-match pass in [`resolve_aumid_by_name`] runs over every cached
/// entry on every lookup; without this field the loop would allocate a
/// fresh lowercased `String` per entry per call (~150–300 allocations
/// per resolve on a stock Win11 install). Caching it once turns the hot
/// path into a borrow.
#[derive(Clone)]
struct AppsFolderEntry {
    display_name: String,
    lowercase_display_name: String,
    aumid: String,
}

/// Cached snapshot of all `shell:AppsFolder` entries.
///
/// Enumeration is slow (~200 ms cold on a stock Win11 install — the
/// folder has ~150–300 entries and each `BindToHandler` round-trips
/// through the shell namespace). We cache the result for the lifetime
/// of the driver process. The pid returned by `ActivateApplication`
/// is the real packaged-app pid regardless of cache staleness, so the
/// only correctness cost of cache staleness is: an app installed after
/// the driver started will be invisible to name-based lookup until the
/// driver restarts. That is acceptable — explicit AUMID via
/// `bundle_id` always works regardless of cache state.
static APPS_FOLDER_CACHE: OnceLock<RwLock<Option<Vec<AppsFolderEntry>>>> = OnceLock::new();

fn cache() -> &'static RwLock<Option<Vec<AppsFolderEntry>>> {
    APPS_FOLDER_CACHE.get_or_init(|| RwLock::new(None))
}

/// Resolve a packaged-app display name to its AUMID.
///
/// Lookup order, all case-insensitive against the display name shown
/// in the Start Menu:
/// 1. Exact match (e.g. `"Notepad"` → `Microsoft.WindowsNotepad_8wekyb3d8bbwe!App`).
/// 2. Case-insensitive prefix match — picks the shortest display name
///    that starts with the query, so `"calc"` resolves to `"Calculator"`
///    rather than `"Calculator Plus"`.
///
/// `"notepad.exe"` and `"notepad"` both resolve identically — the
/// `.exe` suffix is stripped before matching so callers carrying a
/// Win32 idiom still hit the packaged display name.
///
/// Returns `None` if no packaged app matches. The caller should fall
/// back to `ShellExecuteExW` for plain Win32 apps in that case.
pub fn resolve_aumid_by_name(display_name: &str) -> Option<String> {
    // Session 0 short-circuit: `shell:AppsFolder` enumeration goes through
    // the interactive shell broker, which doesn't exist in services /
    // SSH-launched contexts and causes the underlying COM call to hang
    // indefinitely (~no CPU, no progress). Returning None here makes the
    // caller fall through to ShellExecuteExW's PATH-based lookup, which
    // works fine in Session 0 for any app reachable via PATH (notepad,
    // calc, regedit, etc.). Packaged-only apps (Windows 11 Notepad) won't
    // resolve here in Session 0 — explicit `aumid` is the workaround.
    if matches!(crate::diagnostics::current_session_id(), Some(0)) {
        tracing::debug!(
            target: "launch_uwp",
            "skipping shell:AppsFolder lookup in Session 0 (no interactive shell broker); falling back to ShellExecuteEx PATH lookup"
        );
        return None;
    }
    let entries = load_or_get_cache()?;
    let query = display_name.trim().to_lowercase();
    if query.is_empty() {
        return None;
    }

    // Strip an optional `.exe` suffix so callers passing `"notepad.exe"`
    // (a Win32 idiom) still resolve to the packaged display name `"Notepad"`.
    let query_stripped = query.strip_suffix(".exe").unwrap_or(&query);

    // Pass 1: exact case-insensitive match.
    if let Some(hit) = entries
        .iter()
        .find(|e| e.display_name.eq_ignore_ascii_case(query_stripped))
    {
        return Some(hit.aumid.clone());
    }

    // Pass 2: shortest case-insensitive prefix match. Uses the
    // precomputed `lowercase_display_name` to avoid a fresh allocation
    // per entry per call (see `AppsFolderEntry` docs for rationale).
    let mut best: Option<&AppsFolderEntry> = None;
    for entry in entries.iter() {
        if entry.lowercase_display_name.starts_with(query_stripped) {
            match best {
                None => best = Some(entry),
                Some(current) if entry.display_name.len() < current.display_name.len() => {
                    best = Some(entry)
                }
                _ => {}
            }
        }
    }
    best.map(|e| e.aumid.clone())
}

fn load_or_get_cache() -> Option<Vec<AppsFolderEntry>> {
    {
        let guard = cache().read().ok()?;
        if let Some(cached) = guard.as_ref() {
            return Some(cached.clone());
        }
    }

    // Double-checked init — if two threads race here both enumerate; the
    // second discards its result. AppsFolder enumeration is read-only, so
    // the duplicated work is wasted CPU only, not a correctness hazard.
    let fresh = enumerate_apps_folder().unwrap_or_default();
    if let Ok(mut guard) = cache().write() {
        if guard.is_none() {
            *guard = Some(fresh.clone());
        }
    }
    Some(fresh)
}

/// Walk `shell:AppsFolder` and collect `(display_name, AUMID)` for every
/// entry that exposes `PKEY_AppUserModel_ID`. Entries without an AUMID
/// (legacy Win32 shortcuts that happen to be indexed in AppsFolder) are
/// skipped — those are reachable via `ShellExecuteExW` anyway.
fn enumerate_apps_folder() -> windows::core::Result<Vec<AppsFolderEntry>> {
    let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };

    // `shell:AppsFolder` is the well-known parsing path for the virtual
    // folder that backs the Start Menu's "all apps" list. It enumerates
    // every activatable app on the system, packaged or not.
    let folder_path = HSTRING::from("shell:AppsFolder");
    let apps_folder: IShellItem = unsafe {
        SHCreateItemFromParsingName(PCWSTR(folder_path.as_ptr()), None::<&IBindCtx>)?
    };

    // Bind to the `BHID_EnumItems` handler to iterate children.
    let enumerator: IEnumShellItems = unsafe {
        apps_folder.BindToHandler(None::<&IBindCtx>, &BHID_EnumItems)?
    };

    let mut entries = Vec::with_capacity(256);
    loop {
        // Request one item at a time. windows-rs surfaces `S_FALSE`
        // (the natural end-of-enumeration HRESULT) as `Err`, so we
        // intentionally ignore the Result and gate on `fetched == 0`
        // — the documented sentinel for "iterator exhausted".
        let mut slot: [Option<IShellItem>; 1] = [None];
        let mut fetched: u32 = 0;
        let _ = unsafe {
            enumerator.Next(&mut slot, Some(&mut fetched as *mut u32))
        };
        if fetched == 0 {
            break;
        }
        let Some(item) = slot[0].take() else { break; };

        // Cast to IShellItem2 to access PKEY_AppUserModel_ID; if the
        // entry doesn't expose AAM (rare; pure-Win32 shortcut), skip.
        let item2: IShellItem2 = match item.cast() {
            Ok(i) => i,
            Err(_) => continue,
        };

        let aumid = match unsafe { item2.GetString(&PKEY_APP_USER_MODEL_ID) } {
            Ok(pwstr) => pwstr_to_string_and_free(pwstr),
            Err(_) => continue, // No AUMID → not a packaged/registered app.
        };
        if aumid.is_empty() {
            continue;
        }

        let display_name = match unsafe { item2.GetDisplayName(SIGDN_NORMALDISPLAY) } {
            Ok(pwstr) => pwstr_to_string_and_free(pwstr),
            Err(_) => String::new(),
        };
        if display_name.is_empty() {
            continue;
        }

        let lowercase_display_name = display_name.to_lowercase();
        entries.push(AppsFolderEntry { display_name, lowercase_display_name, aumid });
    }

    Ok(entries)
}

/// Drain a COM-allocated wide string (`PWSTR` returned by `GetString` /
/// `GetDisplayName`) into a Rust `String`, then free the underlying
/// COM-task-allocated buffer with `CoTaskMemFree` as the shell requires.
fn pwstr_to_string_and_free(p: PWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    let s = unsafe { p.to_string().unwrap_or_default() };
    unsafe { CoTaskMemFree(Some(p.0 as *const core::ffi::c_void)) };
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_aumid_recognises_canonical_form() {
        assert!(is_aumid("Microsoft.WindowsNotepad_8wekyb3d8bbwe!App"));
        assert!(is_aumid("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"));
    }

    #[test]
    fn is_aumid_rejects_plain_names_and_paths() {
        assert!(!is_aumid("notepad"));
        assert!(!is_aumid("notepad.exe"));
        assert!(!is_aumid(r"C:\Windows\System32\notepad.exe"));
        assert!(!is_aumid(""));
        assert!(!is_aumid("!App")); // empty PFN half
        assert!(!is_aumid("Pkg!")); // empty AppId half
        assert!(!is_aumid("a!b!c")); // two bangs
    }
}
