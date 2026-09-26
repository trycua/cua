//! Transport selection for window-addressed pixel clicks (`click`,
//! `double_click`, `right_click`) on macOS.
//!
//! The cross-platform foreground contract is that a foreground pixel click
//! moves the system pointer to the target and delivers real input there:
//! Windows uses `SetCursorPos` + `SendInput`, X11 uses an XTest warp + button,
//! and macOS desktop scope and foreground drags warp the pointer and post at the
//! HID tap. Window-scoped foreground pixel clicks follow the same model, so apps
//! that derive a click location from the hardware pointer (Tk, some Java and
//! game toolkits) receive it at the right point. Background delivery stays
//! PID-routed and never moves the pointer; a detectable pointer-reading toolkit
//! refuses with the shared `background_unavailable` contract instead of
//! reporting an unverifiable success.

use cua_driver_core::protocol::ToolResult;

use crate::input::pointer_toolkit::PointerReadingToolkit;

/// Transport for a pixel click once any background AX hit-test backend has
/// declined (or was not eligible).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PixelClickRoute {
    /// Explicit foreground rung with an exact window: activate that window,
    /// move the hardware pointer to the mapped global point, and post through
    /// the global HID event tap. The pointer stays at the target.
    ForegroundHid,
    /// Background delivery routed to the target pid. The hardware pointer is
    /// never moved, so the result stays "not driver-verified".
    BackgroundPid,
    /// Background delivery refused: the target's toolkit reads the hardware
    /// pointer, so a PID-routed event would land wherever the pointer is.
    RefuseBackground(PointerReadingToolkit),
}

/// Pure route decision. `effective_foreground` is the requested foreground
/// rung; it only selects the HID route with an exact window, because the HID
/// tap has no pid addressing.
pub(crate) fn pixel_click_route(
    effective_foreground: bool,
    has_window: bool,
    toolkit: Option<PointerReadingToolkit>,
) -> PixelClickRoute {
    if effective_foreground && has_window {
        return PixelClickRoute::ForegroundHid;
    }
    match toolkit {
        Some(toolkit) if has_window => PixelClickRoute::RefuseBackground(toolkit),
        _ => PixelClickRoute::BackgroundPid,
    }
}

/// Resolve the route for a live target, probing the toolkit only for
/// window-addressed background clicks. Returns the structured refusal when
/// background delivery cannot reach the target.
pub(crate) async fn resolve(
    pid: i32,
    effective_foreground: bool,
    window_id: Option<u32>,
    event_kind: &str,
) -> Result<PixelClickRoute, ToolResult> {
    let has_window = window_id.is_some();
    let toolkit = if !effective_foreground && has_window {
        tokio::task::spawn_blocking(move || crate::input::pointer_toolkit::detect(pid))
            .await
            .ok()
            .flatten()
    } else {
        None
    };
    match pixel_click_route(effective_foreground, has_window, toolkit) {
        PixelClickRoute::RefuseBackground(toolkit) => {
            Err(pointer_reading_background_refusal(pid, toolkit, event_kind))
        }
        route => Ok(route),
    }
}

pub(crate) fn pointer_reading_background_refusal(
    pid: i32,
    toolkit: PointerReadingToolkit,
    event_kind: &str,
) -> ToolResult {
    let name = toolkit.name();
    cua_driver_core::delivery::background_unavailable_result(
        format!(
            "Background pixel click is not available for pid {pid}: its {name} toolkit \
             derives click locations from the hardware pointer, and background delivery \
             never moves the pointer, so the click would land wherever the pointer \
             currently is. Retry this action with delivery_mode:\"foreground\"; Cua \
             Driver will activate the window, move the pointer to the target, and click."
        ),
        "background_unavailable",
        "the target toolkit reads the hardware pointer position, which background \
         PID-routed events cannot move; retry with delivery_mode:\"foreground\".",
        serde_json::json!({
            "event_kind": event_kind,
            "reason": "pointer_reading_toolkit",
            "toolkit": name,
        }),
    )
}

/// Human-readable delivery note appended to a successful pixel click result.
pub(crate) fn delivery_note(route: PixelClickRoute) -> &'static str {
    match route {
        PixelClickRoute::ForegroundHid => {
            "foreground: exact window activated, hardware pointer moved to the target, \
             HID event tap; not driver-verified — confirm via screenshot"
        }
        _ => {
            "background CGEvent routed to the pid; the hardware pointer was not moved, \
             so apps that read the pointer position may not receive it at the target; \
             not driver-verified — confirm via screenshot and retry with \
             delivery_mode:\"foreground\" if nothing changed"
        }
    }
}

/// Legacy `path` label consumed by the action-truth normalizer:
/// `cgevent_fg` maps to the HID transport with foreground delivery and
/// `cgevent` to the PID transport with background delivery.
pub(crate) fn path_label(route: PixelClickRoute) -> &'static str {
    match route {
        PixelClickRoute::ForegroundHid => "cgevent_fg",
        _ => "cgevent",
    }
}

/// Structured error for a foreground HID click whose exact-window activation
/// or dispatch failed. No input reaches another window in that case.
pub(crate) fn foreground_unavailable(action: &str, window_id: u32, cause: &str) -> ToolResult {
    ToolResult::error(format!(
        "{action} failed: foreground HID delivery to window {window_id} was not possible: \
         {cause}"
    ))
    .with_structured(serde_json::json!({ "code": "foreground_unavailable" }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// D-MAC-6: a foreground window pixel click must use the HID tap with a
    /// real pointer move (Tk reads the hardware pointer), background clicks
    /// keep PID routing, and a known pointer-reading toolkit refuses
    /// background delivery instead of reporting an unverifiable success.
    #[test]
    fn pixel_click_route_moves_pointer_only_for_foreground_and_refuses_tk_background() {
        let tk = Some(PointerReadingToolkit::Tk);
        assert_eq!(
            pixel_click_route(true, true, None),
            PixelClickRoute::ForegroundHid
        );
        assert_eq!(
            pixel_click_route(true, true, tk),
            PixelClickRoute::ForegroundHid,
            "foreground delivery moves the pointer, so Tk is supported"
        );
        assert_eq!(
            pixel_click_route(false, true, None),
            PixelClickRoute::BackgroundPid
        );
        assert_eq!(
            pixel_click_route(false, true, tk),
            PixelClickRoute::RefuseBackground(PointerReadingToolkit::Tk)
        );
        assert_eq!(
            pixel_click_route(false, false, tk),
            PixelClickRoute::BackgroundPid,
            "legacy window-less pid clicks keep their established route"
        );
        assert_eq!(
            pixel_click_route(true, false, None),
            PixelClickRoute::BackgroundPid,
            "foreground without an exact window cannot use the unaddressed HID tap"
        );
    }

    #[test]
    fn route_labels_say_what_actually_happened() {
        assert_eq!(path_label(PixelClickRoute::ForegroundHid), "cgevent_fg");
        assert_eq!(path_label(PixelClickRoute::BackgroundPid), "cgevent");
        assert!(delivery_note(PixelClickRoute::ForegroundHid).contains("hardware pointer moved"));
        assert!(delivery_note(PixelClickRoute::BackgroundPid).contains("pointer was not moved"));
    }

    #[test]
    fn pointer_reading_refusal_uses_the_shared_background_unavailable_contract() {
        let result =
            pointer_reading_background_refusal(42, PointerReadingToolkit::Tk, "mouse_click");
        assert_eq!(result.is_error, Some(true));
        let structured = result.structured_content.expect("structured refusal");
        assert_eq!(structured["code"], "background_unavailable");
        assert_eq!(structured["reason"], "pointer_reading_toolkit");
        assert_eq!(structured["toolkit"], "tk");
        assert_eq!(structured["event_kind"], "mouse_click");
        assert_eq!(structured["escalation"]["recommended"], "foreground");
    }

    #[test]
    fn foreground_failure_is_structured() {
        let result = foreground_unavailable("click", 7, "window never focused");
        assert_eq!(result.is_error, Some(true));
        assert_eq!(
            result.structured_content.expect("structured")["code"],
            "foreground_unavailable"
        );
    }
}
