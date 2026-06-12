//! Bookmark-URL-field JavaScript-execution bypass for Chromium-family
//! browsers on Windows.
//!
//! Empirical mechanism (Edge 148.0.3967.70, no debug flag): the
//! Add-favorite / Edit-favorite dialog's URL `Edit` accepts an arbitrary
//! `javascript:` URL via UIA `ValuePattern::SetValue` without scrubbing,
//! and `InvokePattern::Invoke` on the resulting Favorites-bar `ListItem`
//! executes the JS in the active tab's DOM context. The bypass survives
//! Chromium's omnibox `javascript:` paste-scrub because Chromium can't
//! close the bookmark path without breaking bookmarklets, which are a
//! documented Web-platform feature.
//!
//! This module exposes [`try_bookmark_exec`] as a prefix on the existing
//! CDP-based JS-execution path in `tools::page::WindowsPageBackend`.
//! Bookmark exec needs zero config and no `--remote-debugging-port` flag;
//! when it fails for any reason, the caller falls through to the CDP
//! path, preserving the existing behaviour.
//!
//! Threading
//! ---------
//! A process-wide [`Mutex`] (`BOOKMARK_EVAL_MUTEX`) serialises every
//! invocation of [`try_bookmark_exec`]. Concurrent calls would race on the
//! single `cua-driver-eval` bookmark URL (one caller's write would be
//! invoked by another), so we deliberately give up parallelism within the
//! eval primitive itself.
//!
//! Stability caveat
//! ----------------
//! UIA workflows that drive bookmark dialogs and the favorites bar are
//! brittle: AutomationIds can drift between Chromium versions; the
//! Favorites bar can be hidden; right-click context menus open at the
//! cursor and so depend on z-order. Every failure here is logged and
//! returns `Err`, which the caller treats as "try CDP next". No path in
//! this module is allowed to panic.
//!
//! No-foreground contract
//! ----------------------
//! Earlier revisions of this module would synthesize `Ctrl+Shift+B` to
//! summon the favorites bar when it was hidden. That keystroke is routed
//! via [`crate::input::keyboard::send_key_synthesized`], which does a
//! transient `SetForegroundWindow(target) → SendInput →
//! SetForegroundWindow(prev_fg)` swap; the swap is visible to the user
//! and the restore can fail silently under UIPI / UIAccess constraints.
//! That violates the cua-driver no-foreground contract. The current code
//! therefore *requires* the user to have shown the favorites bar once
//! (the setting persists across sessions); if it's hidden we bail out and
//! let `execute_javascript` fall through to the CDP path.
//!
//! One residual foreground-activation source remains and is documented as
//! a known limitation: invoking the bookmark via UIA
//! `InvokePattern::Invoke` causes Chromium to briefly raise the browser
//! window because clicking a bookmark is a user-initiated navigation in
//! Chromium's input model. This activation is Chromium-side and not
//! something we can suppress from the UIA caller; see `WINDOWS.md` for
//! the user-facing note.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use windows::core::{Interface, BSTR};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationInvokePattern,
    IUIAutomationValuePattern, TreeScope_Children, TreeScope_Descendants, TreeScope_Subtree,
    UIA_ButtonControlTypeId, UIA_ControlTypePropertyId, UIA_EditControlTypeId,
    UIA_InvokePatternId, UIA_ListItemControlTypeId, UIA_MenuItemControlTypeId,
    UIA_NamePropertyId, UIA_TabItemControlTypeId, UIA_ToolBarControlTypeId,
    UIA_ValuePatternId, UIA_WindowControlTypeId,
};
use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;

// `RPC_E_CHANGED_MODE` mirrors the constant used in `uia::windows_enum`.
// Returned by `CoInitializeEx` when something else on the same thread already
// initialised COM in a different apartment.
const RPC_E_CHANGED_MODE: i32 = -2147417850; // 0x80010106

/// Process-wide serialisation around the bookmark-exec primitive. See
/// module docs ("Threading").
static BOOKMARK_EVAL_MUTEX: Mutex<()> = Mutex::new(());

/// Name we give our hidden bookmarklet. Visible to the user in the
/// Favorites bar; keep it greppable and ASCII-only.
const BOOKMARK_NAME: &str = "cua-driver-eval";

/// Per-call timeouts.
mod budget {
    use std::time::Duration;
    /// How long we wait for a freshly opened dialog to appear in the tree.
    pub const DIALOG_APPEAR: Duration = Duration::from_millis(2500);
    /// How long we wait for the page's title to start with our marker.
    pub const TITLE_RESULT: Duration = Duration::from_secs(5);
    /// UIA settle gap between Set + Invoke pairs.
    pub const UIA_SETTLE: Duration = Duration::from_millis(80);
}

/// Try the bookmark-URL bypass to run `javascript` in the page hosted by
/// the Edge / Chromium top-level window identified by `(pid, window_id)`.
///
/// On success returns the JS expression's result, serialised by the
/// wrapper installed via [`wrap_javascript`] — JSON-stringified for
/// JSON-stringifiable values; the value's `toString()` otherwise.
///
/// On any failure (UIA call returns an error, favorites bar not present
/// and not summonable, bookmark creation pre-conditions not met, title
/// poll exceeds [`budget::TITLE_RESULT`]) returns `Err` so the caller
/// can fall back to CDP.
pub async fn try_bookmark_exec(
    pid: i32,
    window_id: u32,
    javascript: &str,
) -> Result<String> {
    if javascript.is_empty() {
        bail!("javascript expression is empty");
    }
    let js = javascript.to_owned();
    tokio::task::spawn_blocking(move || -> Result<String> {
        let _g = BOOKMARK_EVAL_MUTEX
            .lock()
            .map_err(|_| anyhow!("bookmark-exec mutex poisoned"))?;
        unsafe { try_bookmark_exec_blocking(pid, window_id, &js) }
    })
    .await
    .map_err(|e| anyhow!("bookmark-exec join error: {e}"))?
}

/// Build the `javascript:` URL we install into the bookmark.
///
/// The wrapper:
/// - runs the user expression inside a try/catch so we surface exceptions
///   without leaving the page in a broken state,
/// - stringifies the result with `JSON.stringify` when possible, falling
///   back to `String(value)`,
/// - sets `document.title` to `CUA:<result>` (or `CUA_ERR:<message>`) so
///   we can detect completion via UIA's `NamePropertyId` on the active
///   `TabItem` — UIA tracks title changes cheaply and without focus or
///   CDP,
/// - schedules a 500 ms restore of the original title so the user
///   doesn't see "CUA:42" persist in the tab strip.
///
/// `original_title` is the title we sample immediately before invoking
/// the bookmark; the JS captures it via the IIFE argument so the restore
/// is robust against re-renders that change the title mid-flight.
pub fn wrap_javascript(user_js: &str) -> String {
    // Lex-normalise the user JS:
    //   - Replace \r / \n with spaces so the bookmark URL stays one line
    //     (Edge's URL field strips line breaks silently otherwise; the
    //     replacement keeps statements parseable since JS lexer treats
    //     newlines as whitespace outside ASI-relevant cases).
    //   - Embed as a JS string literal that `eval` consumes — match CDP
    //     `Runtime.evaluate` semantics: the user passes an expression
    //     (e.g. `document.title`), gets its value, no `return` keyword
    //     needed. (Previously the wrapper used `(function(){ ... })()`
    //     which silently returned undefined for any caller that didn't
    //     write `return X` themselves — wrong for CDP-style callers.)
    let safe_user_js = user_js.replace('\r', " ").replace('\n', " ");
    // Escape for inclusion inside a single-quoted JS string literal.
    let escaped = safe_user_js.replace('\\', "\\\\").replace('\'', "\\'");
    format!(
        "javascript:(function(_orig){{\
try {{\
var __r = eval('{escaped}');\
try {{ __r = JSON.stringify(__r); }} catch(e) {{ __r = String(__r); }}\
document.title = 'CUA:' + (__r === undefined ? '' : __r);\
setTimeout(function(){{ document.title = _orig; }}, 500);\
}} catch(e) {{\
document.title = 'CUA_ERR:' + (e && e.message ? e.message : String(e));\
setTimeout(function(){{ document.title = _orig; }}, 500);\
}}\
}})(document.title);"
    )
}

// ── blocking implementation ───────────────────────────────────────────────

unsafe fn try_bookmark_exec_blocking(
    pid: i32,
    window_id: u32,
    javascript: &str,
) -> Result<String> {
    let automation = init_uia()?;
    let hwnd_raw = window_id as u64;

    // 1) Resolve the Edge/Chromium top-level window matching pid+window_id.
    let edge_window = root_for_window(&automation, hwnd_raw, pid)
        .context("could not resolve Edge top-level window for (pid, window_id)")?;

    // 2) Ensure the favorites bar is visible. If not, send Ctrl+Shift+B
    //    via PostMessage (no foreground swap — Chromium's
    //    Browser::HandleKeyboardEvent dispatches accelerators from
    //    WM_KEYDOWN LPARAM modifier bits without consulting GetKeyState).
    //
    // Historical note: PR #1668 removed the auto-toggle here because it
    // used `send_key_synthesized` (SendInput + SetForegroundWindow swap)
    // which violated the no-foreground contract. With `post_key`
    // (PostMessage) routing — verified safe on every Chromium browser +
    // Electron app — the auto-toggle is restored. If the bar STILL isn't
    // visible after a brief settle, we bail with the same actionable
    // error (covers locked-down browser policies / classic Edge profiles).
    let fav_bar = if let Some(bar) = find_favorites_bar(&automation, &edge_window) {
        bar
    } else {
        // No SendInput, no SetForegroundWindow, no UIPI risk.
        let _ = crate::input::post_key(hwnd_raw, "b", &["ctrl", "shift"]);
        std::thread::sleep(std::time::Duration::from_millis(150));
        find_favorites_bar(&automation, &edge_window).ok_or_else(|| {
            anyhow!(
                "Favorites bar not visible after Ctrl+Shift+B PostMessage \
                 — cua-driver-rs's bookmark-URL JS exec needs the bar so \
                 we can invoke the `cua-driver-eval` bookmark. The toggle \
                 keystroke didn't take (browser policy override, locked-down \
                 profile, or non-Chromium target). Show the bar manually via \
                 Ctrl+Shift+B in the browser — the setting persists across \
                 sessions. `execute_javascript` will fall through to the \
                 CDP fallback if it's configured."
            )
        })?
    };

    // 3) Find or create the cua-driver-eval bookmark.
    let bookmark = find_bookmark(&automation, &fav_bar, BOOKMARK_NAME);
    if bookmark.is_none() {
        // Creation path requires driving the omnibox + Add-favorite dialog,
        // which is the most fragile part of the workflow.  If we don't yet
        // have a bookmark and a fresh creation attempt fails, surface a
        // clear error so the caller falls through to CDP.
        bail!(
            "`{BOOKMARK_NAME}` bookmark is not present in the favorites bar. \
             Automatic creation requires navigating the omnibox to \
             edge://favorites which is currently not implemented in this \
             release; create the bookmark manually (any URL is fine — the \
             driver overwrites it on first use) and retry, or relaunch the \
             browser with `--remote-debugging-port=N` to use the CDP path."
        );
    }
    let bookmark = bookmark.unwrap();

    // 4) Edit the bookmark URL via the right-click → Edit dialog.
    set_bookmark_url(&automation, &bookmark, &wrap_javascript(javascript))?;

    // 5) Capture the active tab's title before invocation. The wrapper
    //    restores this after 500 ms so the user doesn't see our marker.
    let active_tab = find_active_tab(&automation, &edge_window);
    let original_title = active_tab
        .as_ref()
        .and_then(|t| read_name(t))
        .unwrap_or_default();
    tracing::debug!(
        target: "page.bookmark_exec",
        "captured original tab title: {:?}", original_title
    );

    // 6) Invoke the bookmark with a no-foreground guard around it.
    //
    // Chromium activates the browser window when a bookmark is clicked
    // because `WebContents::OpenURL` treats it as user-initiated
    // navigation (the activation lives in Chromium's window-aura layer,
    // not in our UIA call). Two-layer mitigation:
    //
    //   (a) Capture `GetForegroundWindow()` immediately before the Invoke.
    //   (b) After the Invoke, briefly poll for "Chromium activated the
    //       browser" and `SetForegroundWindow(prev)` the user's window
    //       back. Same pattern `launch_app`'s `FocusRestoreGuard`
    //       successfully uses; the restore is gated on the new
    //       foreground being owned by Chromium so we never yank focus
    //       from a window the user legitimately tabbed to.
    //
    // Without UIAccess (the daemon's normal integrity), the restore
    // SetForegroundWindow may be blocked by the system foreground lock;
    // we log at trace level and continue — the brief activation is
    // unavoidable without forking Chromium, but the dwell time on the
    // browser is < ~150 ms instead of "until next user action".
    let prev_fg_for_invoke = unsafe {
        windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow()
    };
    invoke_element(&bookmark).context("InvokePattern.Invoke on bookmark failed")?;
    // Restore in a tight loop — at this point the bookmark URL is firing
    // and Chromium is about to activate. Poll for ~600 ms; once we see
    // Chromium grab foreground, swap back. We do this synchronously
    // (not via tokio::spawn) because we WANT to block the poll-marker
    // step below until the foreground is restored — otherwise a slow
    // restore leaves the browser frontmost while we read the title.
    restore_foreground_after_browser_activation(
        prev_fg_for_invoke,
        pid as u32,
    );

    // 7) Poll the active tab's title until it starts with CUA: / CUA_ERR:
    let result = poll_for_marker(&automation, &edge_window, &original_title)?;

    if let Some(err) = result.strip_prefix("CUA_ERR:") {
        bail!("bookmark-exec page exception: {}", err);
    }
    let payload = result
        .strip_prefix("CUA:")
        .ok_or_else(|| anyhow!("internal: marker poll returned non-CUA result"))?;
    Ok(payload.to_owned())
}

// ── UIA helpers ───────────────────────────────────────────────────────────

unsafe fn init_uia() -> Result<IUIAutomation> {
    let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    if hr.is_err() && hr.0 != RPC_E_CHANGED_MODE {
        tracing::debug!(
            target: "page.bookmark_exec",
            "CoInitializeEx returned {hr:?}"
        );
    }
    let automation: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
        .map_err(|e| anyhow!("CoCreateInstance(CUIAutomation) failed: {e}"))?;
    Ok(automation)
}

/// Resolve the IUIAutomation element for `(pid, window_id)`. We validate
/// the ProcessId matches because callers occasionally pass a stale HWND
/// for a different process.
unsafe fn root_for_window(
    automation: &IUIAutomation,
    hwnd_raw: u64,
    expected_pid: i32,
) -> Result<IUIAutomationElement> {
    let hwnd = HWND(hwnd_raw as *mut _);
    if hwnd.0.is_null() {
        bail!("invalid hwnd 0");
    }
    // Sanity-check the pid lines up before we go any further.
    let mut actual_pid: u32 = 0;
    let _ = GetWindowThreadProcessId(hwnd, Some(&mut actual_pid));
    if actual_pid as i32 != expected_pid {
        bail!(
            "hwnd 0x{hwnd_raw:x} belongs to pid {actual_pid}, not the expected {expected_pid}"
        );
    }
    automation
        .ElementFromHandle(hwnd)
        .map_err(|e| anyhow!("ElementFromHandle(0x{hwnd_raw:x}) failed: {e}"))
}

/// Find the favorites bar inside the Edge window.  Chromium exposes it as
/// a `ToolBar` named "Favorites bar".  Some Edge builds expose it as a
/// `Pane`; we tolerate both by walking by Name first, then by ControlType.
unsafe fn find_favorites_bar(
    automation: &IUIAutomation,
    root: &IUIAutomationElement,
) -> Option<IUIAutomationElement> {
    // Cheap path: a single Name lookup. Chromium has used "Favorites bar"
    // as the bar's Name for years across Edge/Chrome.
    let name_cond = automation
        .CreatePropertyCondition(
            UIA_NamePropertyId,
            &BSTR::from("Favorites bar").into(),
        )
        .ok()?;
    if let Ok(bar) = root.FindFirst(TreeScope_Subtree, &name_cond) {
        return Some(bar);
    }
    // Fallback: any visible ToolBar inside the chrome.
    let ctrl_cond = automation
        .CreatePropertyCondition(
            UIA_ControlTypePropertyId,
            &(UIA_ToolBarControlTypeId.0).into(),
        )
        .ok()?;
    let arr = root.FindAll(TreeScope_Subtree, &ctrl_cond).ok()?;
    let count = arr.Length().unwrap_or(0);
    for i in 0..count {
        let elem = match arr.GetElement(i) {
            Ok(e) => e,
            Err(_) => continue,
        };
        if let Some(name) = read_name(&elem) {
            if name.eq_ignore_ascii_case("favorites bar") || name.eq_ignore_ascii_case("bookmarks bar") {
                return Some(elem);
            }
        }
    }
    None
}

unsafe fn find_bookmark(
    automation: &IUIAutomation,
    fav_bar: &IUIAutomationElement,
    name: &str,
) -> Option<IUIAutomationElement> {
    // Bookmarks bar entries are exposed differently by browser:
    //   - Edge: ListItem under a Pane under the outer ToolBar
    //   - Chrome: Button directly under the ToolBar (with autogenerated
    //     AutomationId like `view_NNNN`)
    // FindAll(Descendants, ListItem|Button) covers both. Filter by Name
    // afterward so we don't pick up unrelated buttons (overflow chevron,
    // "Add favorite" entry, etc.).
    let listitem_cond = automation
        .CreatePropertyCondition(
            UIA_ControlTypePropertyId,
            &(UIA_ListItemControlTypeId.0).into(),
        )
        .ok()?;
    let button_cond = automation
        .CreatePropertyCondition(
            UIA_ControlTypePropertyId,
            &(UIA_ButtonControlTypeId.0).into(),
        )
        .ok()?;
    let cond = automation
        .CreateOrCondition(&listitem_cond, &button_cond)
        .ok()?;
    let items = fav_bar.FindAll(TreeScope_Descendants, &cond).ok()?;
    let count = items.Length().unwrap_or(0);
    for i in 0..count {
        let elem = match items.GetElement(i) {
            Ok(e) => e,
            Err(_) => continue,
        };
        if read_name(&elem).as_deref() == Some(name) {
            return Some(elem);
        }
    }
    None
}

unsafe fn read_name(elem: &IUIAutomationElement) -> Option<String> {
    let s = elem.CurrentName().ok()?.to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

unsafe fn read_automation_id(elem: &IUIAutomationElement) -> Option<String> {
    let s = elem.CurrentAutomationId().ok()?.to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Drive the right-click → Edit dialog so the `cua-driver-eval` bookmark
/// gets its URL replaced with `new_url`.
///
/// The dialog has AutomationId "control" on the URL `Edit` element (Edge
/// 148) — we pin by ControlType + position in the tree if the
/// AutomationId varies.
unsafe fn set_bookmark_url(
    automation: &IUIAutomation,
    bookmark: &IUIAutomationElement,
    new_url: &str,
) -> Result<()> {
    // Open the right-click context menu via UIA.  Chromium's ListItem
    // doesn't reliably expose ShowContextMenuPattern; the standard
    // workaround is a synthetic right-click at the element's centre.
    // We don't have BoundingRect handy at this layer — but the
    // bookmark's center is what we want, and the existing
    // `input::post_click_screen` helper handles screen-coord routing.
    let rect = bookmark
        .CurrentBoundingRectangle()
        .map_err(|e| anyhow!("CurrentBoundingRectangle on bookmark failed: {e}"))?;
    let cx = (rect.left + rect.right) / 2;
    let cy = (rect.top + rect.bottom) / 2;
    let bookmark_hwnd = bookmark
        .CurrentNativeWindowHandle()
        .ok()
        .map(|h| h.0 as u64)
        .unwrap_or(0);
    // post_click_screen requires the HWND the click targets; bookmark
    // items inherit the favorites bar's NativeWindowHandle which is the
    // Edge top-level HWND.  If that lookup returned 0, bail — we can't
    // route a click through a null HWND.
    if bookmark_hwnd == 0 {
        bail!("bookmark item has no associated NativeWindowHandle");
    }
    crate::input::post_click_screen(bookmark_hwnd, cx, cy, 1, "right")
        .context("right-click on bookmark for Edit menu failed")?;

    // The context menu opens asynchronously; poll for the "Edit"
    // MenuItem to show up under the desktop root (Chromium menus are
    // popup windows, not children of the bookmark in the UIA tree).
    // Edge labels this "Edit"; Chrome labels it "Edit..." (and may show
    // additional context depending on profile state). Match either.
    let edit_item =
        wait_for_menu_item(automation, &["Edit", "Edit..."], budget::DIALOG_APPEAR)
            .context("Edit menu item did not appear after right-click on bookmark")?;
    invoke_element(&edit_item).context("InvokePattern.Invoke on Edit menu item failed")?;

    // The Edit dialog is a top-level UIA Window. Edge 148 names it
    // "Edit favorite"; Chrome 148 names it "Edit bookmark". Both ship
    // alongside the larger "Edit bookmarks" bookmark-manager page so we
    // match the dialog by full Name (covered by `wait_for_window`'s
    // substring fallback too).
    let dialog = wait_for_window(
        automation,
        &["Edit favorite", "Edit bookmark"],
        budget::DIALOG_APPEAR,
    )
    .context("Edit-bookmark dialog did not appear")?;

    // Inside the dialog, find the URL Edit. Edge 148 names it "Favorite
    // URL"; Chrome 148 names it "Bookmark URL". `find_edit_in_dialog`
    // matches by Name with both exact + substring semantics; the
    // AutomationId fallback (`"control"`) catches Edge builds where the
    // Name drifts. Chrome's `id="input"` aliasing between URL and Name
    // fields is resolved by the substring match on "URL".
    let url_edit = find_edit_in_dialog(
        automation,
        &dialog,
        &["Favorite URL", "Bookmark URL", "URL"],
    )
    .context("URL edit element not found in Edit-bookmark dialog")?;

    // SetValue. Use a brief settle between writes so the validator
    // doesn't toss the change.
    set_value(&url_edit, new_url)?;
    std::thread::sleep(budget::UIA_SETTLE);

    // Save: find the Save / Done button.
    let save_btn = find_button_in_dialog(automation, &dialog, &["Save", "Done"])
        .context("Save button not found in Edit favorite dialog")?;
    invoke_element(&save_btn).context("InvokePattern.Invoke on Save button failed")?;
    std::thread::sleep(budget::UIA_SETTLE);
    Ok(())
}

unsafe fn wait_for_menu_item(
    automation: &IUIAutomation,
    accepted_names: &[&str],
    budget: Duration,
) -> Result<IUIAutomationElement> {
    let root = automation
        .GetRootElement()
        .map_err(|e| anyhow!("GetRootElement failed: {e}"))?;
    let cond = automation
        .CreatePropertyCondition(
            UIA_ControlTypePropertyId,
            &(UIA_MenuItemControlTypeId.0).into(),
        )
        .map_err(|e| anyhow!("CreatePropertyCondition(MenuItem) failed: {e}"))?;
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        if let Ok(items) = root.FindAll(TreeScope_Subtree, &cond) {
            let count = items.Length().unwrap_or(0);
            for i in 0..count {
                let elem = match items.GetElement(i) {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                let elem_name = read_name(&elem).unwrap_or_default();
                // Edge labels its context-menu items literally ("Edit");
                // Chrome adds ellipses ("Edit..."). Accept exact match,
                // ellipsis-stripped match, and prefix-up-to-ellipsis match.
                let trimmed = elem_name
                    .trim_end_matches(|c: char| c == '.' || c == '…')
                    .trim();
                if accepted_names.iter().any(|n| {
                    let n_trimmed = n.trim_end_matches(|c: char| c == '.' || c == '…').trim();
                    elem_name.eq_ignore_ascii_case(n) || trimmed.eq_ignore_ascii_case(n_trimmed)
                }) {
                    return Ok(elem);
                }
            }
        }
        std::thread::sleep(Duration::from_millis(60));
    }
    Err(anyhow!("MenuItem matching {accepted_names:?} not found within {budget:?}"))
}

unsafe fn wait_for_window(
    automation: &IUIAutomation,
    accepted_names: &[&str],
    budget: Duration,
) -> Result<IUIAutomationElement> {
    let root = automation
        .GetRootElement()
        .map_err(|e| anyhow!("GetRootElement failed: {e}"))?;
    let cond = automation
        .CreatePropertyCondition(
            UIA_ControlTypePropertyId,
            &(UIA_WindowControlTypeId.0).into(),
        )
        .map_err(|e| anyhow!("CreatePropertyCondition(Window) failed: {e}"))?;
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        if let Ok(windows) = root.FindAll(TreeScope_Children, &cond) {
            let count = windows.Length().unwrap_or(0);
            for i in 0..count {
                let elem = match windows.GetElement(i) {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                let elem_name = read_name(&elem).unwrap_or_default();
                let elem_lower = elem_name.to_ascii_lowercase();
                if accepted_names.iter().any(|n| {
                    elem_name.eq_ignore_ascii_case(n)
                        || elem_lower.contains(&n.to_ascii_lowercase())
                }) {
                    return Ok(elem);
                }
            }
        }
        std::thread::sleep(Duration::from_millis(80));
    }
    Err(anyhow!("Window matching {accepted_names:?} not found within {budget:?}"))
}

unsafe fn find_edit_in_dialog(
    automation: &IUIAutomation,
    dialog: &IUIAutomationElement,
    accepted_names: &[&str],
) -> Result<IUIAutomationElement> {
    let cond = automation
        .CreatePropertyCondition(
            UIA_ControlTypePropertyId,
            &(UIA_EditControlTypeId.0).into(),
        )
        .map_err(|e| anyhow!("CreatePropertyCondition(Edit) failed: {e}"))?;
    let edits = dialog
        .FindAll(TreeScope_Descendants, &cond)
        .map_err(|e| anyhow!("FindAll(Edit) in dialog failed: {e}"))?;
    let count = edits.Length().unwrap_or(0);
    for i in 0..count {
        let elem = match edits.GetElement(i) {
            Ok(e) => e,
            Err(_) => continue,
        };
        let name = read_name(&elem).unwrap_or_default();
        if accepted_names
            .iter()
            .any(|n| name.eq_ignore_ascii_case(n) || name.to_ascii_lowercase().contains(&n.to_ascii_lowercase()))
        {
            return Ok(elem);
        }
        // Edge 148: the URL edit's AutomationId is sometimes "control".
        if let Some(aid) = read_automation_id(&elem) {
            if aid == "control" && accepted_names.iter().any(|n| n.contains("URL")) {
                // This is the URL field iff it's the URL we're looking
                // for. We only fall back to AutomationId when the Name
                // didn't resolve and the caller asked for a URL field.
                return Ok(elem);
            }
        }
    }
    Err(anyhow!(
        "no Edit element matching {:?} found in dialog",
        accepted_names
    ))
}

unsafe fn find_button_in_dialog(
    automation: &IUIAutomation,
    dialog: &IUIAutomationElement,
    accepted_names: &[&str],
) -> Result<IUIAutomationElement> {
    let cond = automation
        .CreatePropertyCondition(
            UIA_ControlTypePropertyId,
            &(UIA_ButtonControlTypeId.0).into(),
        )
        .map_err(|e| anyhow!("CreatePropertyCondition(Button) failed: {e}"))?;
    let buttons = dialog
        .FindAll(TreeScope_Descendants, &cond)
        .map_err(|e| anyhow!("FindAll(Button) in dialog failed: {e}"))?;
    let count = buttons.Length().unwrap_or(0);
    for i in 0..count {
        let elem = match buttons.GetElement(i) {
            Ok(e) => e,
            Err(_) => continue,
        };
        let name = read_name(&elem).unwrap_or_default();
        if accepted_names.iter().any(|n| name.eq_ignore_ascii_case(n)) {
            return Ok(elem);
        }
    }
    Err(anyhow!(
        "no Button matching {:?} found in dialog",
        accepted_names
    ))
}

unsafe fn set_value(elem: &IUIAutomationElement, value: &str) -> Result<()> {
    let pattern = elem
        .GetCurrentPattern(UIA_ValuePatternId)
        .map_err(|e| anyhow!("element does not support ValuePattern: {e}"))?;
    let vp: IUIAutomationValuePattern = pattern
        .cast()
        .map_err(|e| anyhow!("cast to IUIAutomationValuePattern failed: {e}"))?;
    vp.SetValue(&BSTR::from(value))
        .map_err(|e| anyhow!("ValuePattern.SetValue failed: {e}"))?;
    Ok(())
}

unsafe fn invoke_element(elem: &IUIAutomationElement) -> Result<()> {
    let pattern = elem
        .GetCurrentPattern(UIA_InvokePatternId)
        .map_err(|e| anyhow!("element does not support InvokePattern: {e}"))?;
    let inv: IUIAutomationInvokePattern = pattern
        .cast()
        .map_err(|e| anyhow!("cast to IUIAutomationInvokePattern failed: {e}"))?;
    inv.Invoke()
        .map_err(|e| anyhow!("InvokePattern.Invoke failed: {e}"))?;
    Ok(())
}

/// Belt-and-suspenders foreground restore after the bookmark Invoke.
///
/// Chromium's bookmark click triggers `WebContents::OpenURL` →
/// `Browser::ActivateContents` → `SetForegroundWindow` on the browser
/// frame. This happens whether we like it or not, in Chromium-side code
/// we don't control. The mitigation: capture `prev_fg` immediately
/// before our `InvokePattern.Invoke` (the caller does this), then poll
/// for "the browser pid grabbed foreground" and call `SetForegroundWindow(prev_fg)`
/// the user's window back. Mirrors `launch_app`'s `FocusRestoreGuard`
/// (PR #1668, fixed by CodeRabbit feedback on PR #1668 to gate on PID
/// ownership rather than any foreground change).
///
/// Synchronous (not tokio::spawn) because the caller's next step is
/// `poll_for_marker`, which reads the active tab's title via UIA — that
/// must happen against the now-restored foreground state so we don't
/// see transient state.
///
/// Best-effort: when Windows' foreground-lock denies the restore (we're
/// not at UIAccess), the dwell time on the browser is still bounded to
/// the ~600 ms poll budget. Without this guard the browser stays
/// frontmost until the next user action.
unsafe fn restore_foreground_after_browser_activation(
    prev_fg: windows::Win32::Foundation::HWND,
    browser_pid: u32,
) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowThreadProcessId, IsWindow, SetForegroundWindow,
    };

    if prev_fg.0.is_null() {
        tracing::trace!(
            target: "page.bookmark_exec.focus_restore",
            "no prior foreground to restore — skipping"
        );
        return;
    }
    if browser_pid == 0 {
        tracing::trace!(
            target: "page.bookmark_exec.focus_restore",
            "no browser pid — skipping restore (no ownership signal)"
        );
        return;
    }

    // Poll for ~600 ms in 50 ms steps. Chromium's activation typically
    // lands within ~150 ms but the URL-load can stretch the window.
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(600);
    while std::time::Instant::now() < deadline {
        let fg_now = GetForegroundWindow();
        if fg_now == prev_fg {
            // Chromium didn't (yet) grab foreground — keep polling
            // briefly; if it never does we just exit cleanly.
            std::thread::sleep(std::time::Duration::from_millis(50));
            continue;
        }
        let mut fg_pid: u32 = 0;
        let _ = GetWindowThreadProcessId(fg_now, Some(&mut fg_pid));
        if fg_pid != browser_pid {
            // Foreground changed but to something we didn't expect
            // (user Alt-Tabbed mid-eval). Leave it alone.
            tracing::trace!(
                target: "page.bookmark_exec.focus_restore",
                "foreground changed to non-browser pid {fg_pid} (expected {browser_pid}) — leaving as-is"
            );
            return;
        }
        // Chromium grabbed foreground — restore the user's window.
        if !IsWindow(prev_fg).as_bool() {
            tracing::trace!(
                target: "page.bookmark_exec.focus_restore",
                "prev foreground HWND no longer valid — skipping restore"
            );
            return;
        }
        let ok = SetForegroundWindow(prev_fg).as_bool();
        if ok {
            tracing::debug!(
                target: "page.bookmark_exec.focus_restore",
                "restored foreground to HWND 0x{:x} after browser pid {browser_pid} activation",
                prev_fg.0 as usize
            );
        } else {
            tracing::trace!(
                target: "page.bookmark_exec.focus_restore",
                "SetForegroundWindow returned FALSE (foreground-lock denial; need UIAccess) — \
                 browser will keep focus until next user action"
            );
        }
        return;
    }
    tracing::trace!(
        target: "page.bookmark_exec.focus_restore",
        "browser pid {browser_pid} did not steal foreground within 600ms — no restore needed"
    );
    let _ = HWND::default(); // silence "unused import" possibility on cfg
}

/// Find the TabItem that's currently selected — Chromium exposes the
/// active tab as the only TabItem whose `IsSelected` is true.  We don't
/// strictly need IsSelected: a fallback is "the first TabItem in the tab
/// strip", which works for single-tab windows.
unsafe fn find_active_tab(
    automation: &IUIAutomation,
    root: &IUIAutomationElement,
) -> Option<IUIAutomationElement> {
    let cond = automation
        .CreatePropertyCondition(
            UIA_ControlTypePropertyId,
            &(UIA_TabItemControlTypeId.0).into(),
        )
        .ok()?;
    let tabs = root.FindAll(TreeScope_Subtree, &cond).ok()?;
    let count = tabs.Length().unwrap_or(0);
    if count == 0 {
        return None;
    }
    // Try SelectionItemPattern.IsSelected for a clean match.
    use windows::Win32::UI::Accessibility::{
        IUIAutomationSelectionItemPattern, UIA_SelectionItemPatternId,
    };
    for i in 0..count {
        let elem = match tabs.GetElement(i) {
            Ok(e) => e,
            Err(_) => continue,
        };
        if let Ok(pat) = elem.GetCurrentPattern(UIA_SelectionItemPatternId) {
            if let Ok(sip) = pat.cast::<IUIAutomationSelectionItemPattern>() {
                if sip.CurrentIsSelected().unwrap_or_default().as_bool() {
                    return Some(elem);
                }
            }
        }
    }
    // Fall back to the first one.
    tabs.GetElement(0).ok()
}

/// Poll the active tab's `Name` property until it starts with `CUA:` or
/// `CUA_ERR:`. Returns the full marker line (including prefix). Times
/// out per `budget::TITLE_RESULT`.
unsafe fn poll_for_marker(
    automation: &IUIAutomation,
    edge_window: &IUIAutomationElement,
    original_title: &str,
) -> Result<String> {
    let deadline = Instant::now() + budget::TITLE_RESULT;
    let _ = original_title; // currently unused; kept for diagnostic logs.
    while Instant::now() < deadline {
        if let Some(tab) = find_active_tab(automation, edge_window) {
            if let Some(name) = read_name(&tab) {
                // Chromium concatenates the page title with the tab's
                // status — e.g. "CUA:42 - Edge". Find the prefix
                // anywhere at the start of the title segment.
                if let Some(idx) = name.find("CUA:") {
                    if idx == 0 || name[..idx].chars().all(char::is_whitespace) {
                        return Ok(extract_marker(&name, "CUA:"));
                    }
                }
                if let Some(idx) = name.find("CUA_ERR:") {
                    if idx == 0 || name[..idx].chars().all(char::is_whitespace) {
                        return Ok(extract_marker(&name, "CUA_ERR:"));
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(anyhow!(
        "active tab title did not produce a CUA: / CUA_ERR: marker within {:?}",
        budget::TITLE_RESULT
    ))
}

/// Split `s` at the prefix and strip Chromium's trailing
/// ` - <browser name>` only — must not corrupt payloads that
/// legitimately contain ` - ` in their JSON content (e.g.
/// `CUA:"a - b"`).
///
/// We rsplit from the END (so only the last `" - "` is a candidate
/// separator) and only strip the suffix when it looks like one of the
/// known Chromium-family browser names. Anything else stays in the
/// payload verbatim.
fn extract_marker(s: &str, prefix: &str) -> String {
    let start = s.find(prefix).unwrap_or(0);
    let after = &s[start..];
    if let Some((left, right)) = after.rsplit_once(" - ") {
        let right_lc = right.to_ascii_lowercase();
        let looks_like_browser = right_lc.contains("edge")
            || right_lc.contains("chrome")
            || right_lc.contains("chromium")
            || right_lc.contains("brave")
            || right_lc.contains("arc")
            || right_lc.contains("vivaldi")
            || right_lc.contains("opera");
        if looks_like_browser {
            return left.to_owned();
        }
    }
    after.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_javascript_emits_expected_shape() {
        // CDP-style: caller passes an expression, NOT a function body
        // with `return`. The eval-based wrapper returns its value.
        let js = "document.title";
        let wrapped = wrap_javascript(js);
        assert!(wrapped.starts_with("javascript:"));
        assert!(wrapped.contains("try"));
        assert!(wrapped.contains("JSON.stringify"));
        assert!(wrapped.contains("setTimeout"));
        assert!(wrapped.contains("CUA:"));
        assert!(wrapped.contains("CUA_ERR:"));
        // The user expression is embedded as a single-quoted string
        // literal passed to eval — that's the CDP-compatible semantics.
        assert!(wrapped.contains("eval('document.title')"));
        // Original-title capture argument should be present.
        assert!(wrapped.contains("_orig"));
        assert!(wrapped.contains("document.title"));
    }

    #[test]
    fn wrap_javascript_escapes_single_quotes() {
        // Single quotes in the user JS must be escaped so they don't
        // close the eval string literal early.
        let js = "var x = 'hi'; x";
        let wrapped = wrap_javascript(js);
        assert!(wrapped.contains(r"eval('var x = \'hi\'; x')"));
    }

    #[test]
    fn wrap_javascript_escapes_backslashes() {
        // Backslashes inside the user JS must be doubled so the literal
        // survives eval-string-literal decoding.
        let js = r"'a\nb'";
        let wrapped = wrap_javascript(js);
        // The user JS already contains the literal sequence `'a\nb'`;
        // after escaping, single quotes become \' and the `\n` becomes
        // `\\n` (literal backslash + n).
        assert!(wrapped.contains(r"eval('\'a\\nb\'')"));
    }

    #[test]
    fn wrap_javascript_strips_newlines() {
        let js = "var x = 1;\nvar y = 2;\r\nx + y";
        let wrapped = wrap_javascript(js);
        // Bookmark URL must stay one line.
        assert!(!wrapped.contains('\n'));
        assert!(!wrapped.contains('\r'));
        // Replacement keeps the statements parseable as ASI-friendly JS.
        assert!(wrapped.contains("eval('var x = 1; var y = 2;  x + y')"));
    }

    #[test]
    fn extract_marker_strips_chromium_suffix() {
        assert_eq!(extract_marker("CUA:42", "CUA:"), "CUA:42");
        assert_eq!(
            extract_marker("CUA:42 - Microsoft Edge", "CUA:"),
            "CUA:42"
        );
        assert_eq!(
            extract_marker("CUA:42 - Google Chrome", "CUA:"),
            "CUA:42"
        );
        assert_eq!(
            extract_marker("Foo CUA:42 - Edge", "CUA:"),
            "CUA:42"
        );
        assert_eq!(
            extract_marker("CUA_ERR:not defined - Edge", "CUA_ERR:"),
            "CUA_ERR:not defined"
        );
    }

    #[test]
    fn extract_marker_preserves_dash_in_payload() {
        // Payload "a - b" must NOT be truncated at the embedded " - ".
        // (The previous `find(" - ")` implementation would return
        // `CUA:"a` here — wrong.)
        assert_eq!(
            extract_marker("CUA:\"a - b\" - Microsoft Edge", "CUA:"),
            "CUA:\"a - b\""
        );
        // No browser suffix present → keep the whole payload, even with
        // a stray " - " in the middle.
        assert_eq!(
            extract_marker("CUA:\"foo - bar\"", "CUA:"),
            "CUA:\"foo - bar\""
        );
        // The suffix must look like a known browser to be stripped —
        // an arbitrary trailing " - X" stays in the payload.
        assert_eq!(
            extract_marker("CUA:result - notabrowser", "CUA:"),
            "CUA:result - notabrowser"
        );
    }
}
