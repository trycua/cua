//! Shared dispatch-mode logic for cua-driver-rs Windows input tools.
//!
//! Each input tool accepts an optional `dispatch` field with three modes:
//!
//! - `background` (DEFAULT) — PostMessage / UIA path only. If
//!   `would_be_silently_dropped(target, event_kind)` returns true, the tool
//!   returns a structured `background_unavailable` error so the caller can
//!   `bring_to_front` then retry with `dispatch:"foreground"`. This is the
//!   default because cua-driver's value proposition is that input never
//!   steals foreground — making that the silent guarantee, not a hidden
//!   trade-off, is worth a breaking change to existing callers that
//!   previously got auto-fallback to SendInput.
//!
//! - `foreground` — SendInput with a brief `SetForegroundWindow(target)`
//!   swap, restoring the prior foreground after the events flush. Brief
//!   flash unless the target was already foreground. Required to drive
//!   Chromium-content or GTK-button widget targets reliably.
//!
//! - `auto` — current heuristics: each tool's existing PostMessage /
//!   SendInput routing is preserved bit-for-bit. Opt-in for callers that
//!   want the historical silent-fallback behavior. Not the default.
//!
//! The matrix of "which (window class, event kind) pairs are silently
//! dropped by PostMessage" lives here as `would_be_silently_dropped`.
//! Tools call into this helper instead of inlining the class checks.

use serde_json::Value;

/// Family of synthetic input events. Used together with the target HWND
/// to decide whether PostMessage will be silently dropped.
#[derive(Copy, Clone, Debug)]
pub enum EventKind {
    /// `WM_LBUTTONDOWN/UP`, `WM_RBUTTONDOWN/UP`, `WM_MBUTTONDOWN/UP`.
    MouseClick,
    /// `WM_MOUSEMOVE` (also covers drag intermediate steps).
    MouseMove,
    /// `WM_VSCROLL` / `WM_HSCROLL`.
    MouseScroll,
    /// `WM_KEYDOWN/UP` with no modifiers — plain key tap.
    Keystroke,
    /// `WM_KEYDOWN/UP` with modifiers — accelerator candidate (Ctrl+S etc).
    KeyCombo,
    /// `WM_CHAR` text input.
    TextInput,
}

impl EventKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::MouseClick  => "mouse_click",
            Self::MouseMove   => "mouse_move",
            Self::MouseScroll => "mouse_scroll",
            Self::Keystroke   => "keystroke",
            Self::KeyCombo    => "key_combo",
            Self::TextInput   => "text_input",
        }
    }
}

/// Client-controlled dispatch policy.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DispatchMode {
    /// Current heuristics. Each tool keeps its existing routing.
    Auto,
    /// PostMessage / UIA only. Error if dispatch would be silently dropped.
    Background,
    /// SendInput with brief foreground swap.
    Foreground,
}

impl DispatchMode {
    /// Parse from a tool's `dispatch` JSON arg.
    ///
    /// Missing / unknown values resolve to `Background` — cua-driver's
    /// default is now strict no-foreground. Callers that want the
    /// historical silent-fallback heuristics must explicitly pass
    /// `dispatch:"auto"`.
    pub fn from_args(args: &Value) -> Self {
        match args.get("dispatch").and_then(|v| v.as_str()) {
            Some("auto")       => Self::Auto,
            Some("foreground") => Self::Foreground,
            _                  => Self::Background,
        }
    }
}

/// JSON-schema fragment for the `dispatch` field. Include this in every
/// input tool's `input_schema.properties.dispatch`.
pub fn dispatch_schema() -> Value {
    serde_json::json!({
        "type": "string",
        "enum": ["background", "foreground", "auto"],
        "default": "background",
        "description":
            "Dispatch mode. 'background' (default) refuses to swap \
             foreground; returns a structured background_unavailable error \
             if PostMessage would be silently dropped for this event kind \
             on this target. 'foreground' explicitly accepts a brief \
             SetForegroundWindow swap (SendInput path) — required to drive \
             Chromium-content or GTK-button widget targets reliably. \
             'auto' uses cua-driver's internal heuristics (silent fallback \
             to SendInput on known-problematic targets); opt-in for \
             callers that prefer the historical behavior."
    })
}

/// Returns true if PostMessage on `hwnd` for this `event_kind` is empirically
/// known to be silently dropped by the target's input stack.
///
/// Combines the existing `is_chromium_target_window` detector with new GTK
/// (`gdkWindowToplevel` / `gdkSurfaceToplevel`) detection.
pub fn would_be_silently_dropped(hwnd: u64, kind: EventKind) -> bool {
    use EventKind::*;
    if crate::input::is_chromium_target_window(hwnd) {
        // Chromium's input thread architecture requires SendInput-queue
        // origin for mouse + key-combo events (#1623). Plain keystrokes and
        // text input via WM_CHAR still work because they go through
        // Chromium's IME path, which DOES consume Win32 messages.
        return matches!(kind, MouseClick | MouseMove | MouseScroll | KeyCombo);
    }
    if is_gtk_target_window(hwnd) {
        // Conservative flag for GTK: button widgets ignore PostMessage
        // clicks, drawing-area widgets accept them. We cannot distinguish
        // at the HWND level (single HWND for the whole GTK window) so we
        // flag mouse clicks broadly. Canvas-style drag works in practice;
        // caller can still opt to retry with dispatch:"background" on the
        // drag path if the click error wasn't actually load-bearing.
        return matches!(kind, MouseClick);
    }
    if is_vcl_target_window(hwnd) {
        // VCL (LibreOffice / OpenOffice family) routes accelerator keys
        // through TranslateAccelerator, which reads `GetKeyState`.
        // PostMessage(WM_KEYDOWN) delivers the key but does NOT update
        // GetKeyState, so single-key dialog accelerators (Y/N for
        // confirmations, Esc / Enter / Tab) and modifier combos (Ctrl+S,
        // Alt+F4) silently fail. Plain WM_CHAR text input through the
        // document widgets still works (verified end-to-end against
        // Writer's main editing area). Flag the keystroke-class events
        // so dispatch:"background" surfaces a structured error instead
        // of pretending to succeed.
        return matches!(kind, Keystroke | KeyCombo);
    }
    false
}

/// Detect LibreOffice / OpenOffice (VCL framework) windows.
///
/// VCL on Windows registers window classes with a `SAL` prefix (StarOffice's
/// "Service Abstraction Layer"): `SALFRAME` (top-level), `SALSUBFRAME`
/// (dialogs / popups), `SALOBJECT` (embedded), `SALMENU` (menu host),
/// `SALTMPSUBFRAME` (transient). The `SAL` prefix is unique enough to
/// match by leading characters.
pub fn is_vcl_target_window(hwnd: u64) -> bool {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::GetClassNameW;
    if hwnd == 0 {
        return false;
    }
    let mut buf = [0u16; 64];
    let n = unsafe { GetClassNameW(HWND(hwnd as *mut _), &mut buf) };
    if n <= 0 {
        return false;
    }
    let class = String::from_utf16_lossy(&buf[..n as usize]);
    class.starts_with("SAL")
}

/// Detect GTK/GDK top-level windows.
///
/// GTK 3 on Windows uses class `gdkWindowToplevel`; GTK 4 uses
/// `gdkSurfaceToplevel`. Match by prefix in case future GTK versions
/// append a suffix the way Chromium does (`Chrome_WidgetWin_0` /
/// `Chrome_WidgetWin_1`).
pub fn is_gtk_target_window(hwnd: u64) -> bool {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::GetClassNameW;
    if hwnd == 0 {
        return false;
    }
    let mut buf = [0u16; 64];
    let n = unsafe { GetClassNameW(HWND(hwnd as *mut _), &mut buf) };
    if n <= 0 {
        return false;
    }
    let class = String::from_utf16_lossy(&buf[..n as usize]);
    class.starts_with("gdkWindow") || class.starts_with("gdkSurface")
}

/// Read a window's class name into an owned String. Best-effort; returns
/// `"<unknown>"` on any failure so it can drop straight into a diagnostic.
pub fn read_class_name(hwnd: u64) -> String {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::GetClassNameW;
    if hwnd == 0 {
        return "<unknown>".into();
    }
    let mut buf = [0u16; 256];
    let n = unsafe { GetClassNameW(HWND(hwnd as *mut _), &mut buf) };
    if n <= 0 {
        return "<unknown>".into();
    }
    String::from_utf16_lossy(&buf[..n as usize])
}

/// Build the structured `background_unavailable` error returned when
/// `dispatch:"background"` would silently drop.
pub fn background_unavailable_error(
    hwnd: u64,
    kind: EventKind,
) -> cua_driver_core::protocol::ToolResult {
    let class = read_class_name(hwnd);
    let text = format!(
        "Background dispatch is not available for target window class \
         '{class}' on this event kind ({}). Either call bring_to_front \
         then retry with dispatch:\"foreground\", or accept the foreground \
         swap directly by setting dispatch:\"foreground\".",
        kind.name()
    );
    cua_driver_core::protocol::ToolResult::error(text)
        .with_structured(serde_json::json!({
            "code": "background_unavailable",
            "target_class": class,
            "event_kind": kind.name(),
            "suggestion":
                "Either call bring_to_front then retry with dispatch:\"foreground\", \
                 or accept the foreground swap by setting dispatch:\"foreground\" directly.",
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_mode_parses_known_values() {
        let j = |s: &str| serde_json::json!({"dispatch": s});
        assert_eq!(DispatchMode::from_args(&j("auto")), DispatchMode::Auto);
        assert_eq!(DispatchMode::from_args(&j("background")), DispatchMode::Background);
        assert_eq!(DispatchMode::from_args(&j("foreground")), DispatchMode::Foreground);
    }

    #[test]
    fn dispatch_mode_defaults_to_background() {
        // Missing field, garbage value, null — all resolve to Background.
        // This is the cua-driver no-foreground-by-default contract;
        // breaking change to callers who used to rely on silent SendInput
        // fallback against Chromium / GTK-button targets.
        assert_eq!(DispatchMode::from_args(&serde_json::json!({})), DispatchMode::Background);
        assert_eq!(
            DispatchMode::from_args(&serde_json::json!({"dispatch": "garbage"})),
            DispatchMode::Background
        );
        assert_eq!(
            DispatchMode::from_args(&serde_json::json!({"dispatch": null})),
            DispatchMode::Background
        );
    }
}
