//! Background-safe "scroll an off-screen element into view" for coordinate actions.
//!
//! The element cache records each actionable element's screen center at walk
//! time. On a short display (e.g. 1024×768) a tall window reflows so some
//! controls sit below the visible area; their cached center then falls outside
//! the window rect. A raw coordinate tap there lands on whatever is actually
//! at that pixel — the taskbar, another window — instead of the control. The
//! click/double-click/right-click element paths already turn that into a clean
//! error via `input::point_in_window_bounds`; this module lets them first ask
//! the control to make itself visible so the action can proceed.
//!
//! `IUIAutomationScrollItemPattern::ScrollIntoView` asks the *element* to
//! scroll itself into view through its own scroll container (the cleanest
//! option — the control knows how to reveal itself). It is delivered over the
//! UI Automation accessibility channel, not the input queue, so it neither
//! raises nor activates the window. We still wrap it in the UWP foreground-steal
//! bypass for parity with the Invoke path (XAML hosts can self-foreground on
//! UIA-driven layout changes); the bypass is a no-op for classic Win32 hosts.

use windows::core::Interface;
use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, IUIAutomationScrollItemPattern, UIA_ScrollItemPatternId,
};

/// Ask the UIA element behind `element_ptr` to scroll itself into view, then
/// return its fresh on-screen center (from the element's *live* bounding rect,
/// since the cached center is stale once the control has moved).
///
/// Returns `None` when the element doesn't support `ScrollItemPattern`, the
/// scroll call fails, or the rect can't be re-read — callers fall back to the
/// existing off-screen failure in that case.
///
/// # Safety
/// `element_ptr` must be a live `IUIAutomationElement` vtable pointer. Callers
/// hold it alive through a `RetainedElement` guard (its COM `AddRef`) for the
/// duration of this call. We borrow the pointer without consuming the cache's
/// refcount (the constructed handle is `forget`-ten before return).
pub unsafe fn scroll_into_view_and_recenter(
    host_hwnd: u64,
    element_ptr: usize,
) -> Option<(i32, i32)> {
    if element_ptr == 0 {
        return None;
    }
    let elem: IUIAutomationElement = IUIAutomationElement::from_raw(element_ptr as *mut _);

    let result = (|| {
        let pattern = elem.GetCurrentPattern(UIA_ScrollItemPatternId).ok()?;
        let scroll_item = pattern.cast::<IUIAutomationScrollItemPattern>().ok()?;
        // Drive the scroll inside the UWP foreground-steal bypass (no-op for
        // non-XAML hosts) so a XAML host can't self-raise on the layout change.
        crate::uia::fg_bypass::run_with_uwp_bypass(host_hwnd as isize, || {
            scroll_item.ScrollIntoView()
        })
        .ok()?;
        // Re-read the live bounding rect — the cached center is now stale.
        let rect = elem.CurrentBoundingRectangle().ok()?;
        let cx = (rect.left + rect.right) / 2;
        let cy = (rect.top + rect.bottom) / 2;
        Some((cx, cy))
    })();

    // Don't Release the cache's ref — the RetainedElement guard owns it.
    std::mem::forget(elem);
    result
}
