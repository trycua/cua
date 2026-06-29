//! Shared delivery-mode logic for cua-driver-rs Windows input tools.
//!
//! Mirrors the macOS `DeliveryMode` model (see platform-macos
//! `tools::DeliveryMode`): each input tool accepts an optional
//! `delivery_mode` field with exactly two modes — the agent-selected rung
//! of the best-effort-background ladder, passed per call (never a stored
//! setting):
//!
//! - `background` (DEFAULT) — PostMessage / UIA path only, never fronts. If
//!   `would_be_silently_dropped(target, event_kind)` returns true, the tool
//!   returns a structured `background_unavailable` error so the caller can
//!   `bring_to_front` then retry with `delivery_mode:"foreground"`. This is
//!   the default because cua-driver's value proposition is that input never
//!   steals foreground — surfacing an honest error beats silently fronting.
//!
//! - `foreground` — SendInput with a brief `SetForegroundWindow(target)`
//!   swap, restoring the prior foreground after the events flush. Brief
//!   flash unless the target was already foreground. The agent's explicit
//!   last resort, and the only rung that reaches Chromium-content / GTK-
//!   button / VCL-accelerator targets reliably.
//!
//! The legacy Windows-only `auto` mode (silent SendInput fallback) was
//! removed in the macOS-alignment pass: it could front without the caller
//! opting in, breaking the no-foreground contract macOS guarantees. Any
//! unrecognised value now resolves to `background` (see [`DeliveryMode::parse`]).
//!
//! The matrix of "which (window class, event kind) pairs are silently
//! dropped by PostMessage" lives here as `would_be_silently_dropped` — a
//! Windows-specific safeguard with no macOS analogue (CGEvent posting does
//! not have the same per-framework silent-drop problem). Tools call into
//! this helper instead of inlining the class checks.

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

/// Input delivery modality — the agent-selected rung of the best-effort-
/// background ladder, passed per call (never a stored/config setting).
/// Mirrors macOS `tools::DeliveryMode`.
///
/// - `Background` (default): post synthetic input to the target without
///   fronting; on a known silent-drop (class, event) pair the tool returns
///   a structured `background_unavailable` error instead of fronting.
/// - `Foreground`: briefly front the target window, act via SendInput, then
///   restore the prior foreground. The agent's explicit last resort.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum DeliveryMode {
    /// PostMessage / UIA only. Error if delivery would be silently dropped.
    #[default]
    Background,
    /// SendInput with brief foreground swap.
    Foreground,
}

impl DeliveryMode {
    /// Parse the per-call `delivery_mode` argument. Anything other than an
    /// explicit case-insensitive `"foreground"` resolves to `Background` —
    /// the correct default, so an omitted/garbage value (or the removed
    /// legacy `"auto"`) never silently fronts. Matches macOS
    /// `DeliveryMode::parse`.
    pub fn parse(arg: Option<&str>) -> Self {
        match arg {
            Some(s) if s.eq_ignore_ascii_case("foreground") => Self::Foreground,
            _ => Self::Background,
        }
    }

    /// Parse from a tool's JSON args, reading the `delivery_mode` field.
    pub fn from_args(args: &Value) -> Self {
        Self::parse(args.get("delivery_mode").and_then(|v| v.as_str()))
    }

    pub fn is_foreground(self) -> bool { matches!(self, Self::Foreground) }
}

/// JSON-schema fragment for the `delivery_mode` field. Include this in every
/// input tool's `input_schema.properties.delivery_mode`. Two modes, matching
/// the macOS surface.
pub fn delivery_mode_schema() -> Value {
    serde_json::json!({
        "type": "string",
        "enum": ["background", "foreground"],
        "default": "background",
        "description":
            "Input delivery mode. 'background' (default) never swaps foreground: \
             it routes through UIA Invoke / PostMessage and the window is never \
             raised. For targets whose input stack silently drops posted events \
             (Chromium/Electron content, GTK buttons, VCL/LibreOffice accelerators) \
             the tool returns a structured background_unavailable error rather than \
             fronting. 'foreground' is the explicit escalation: a brief \
             SetForegroundWindow swap + SendInput, restoring the prior foreground \
             afterward (call bring_to_front first to avoid the flash). Matches the \
             macOS delivery_mode surface."
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
    if is_wpf_target_window(hwnd) {
        // WPF ignores PostMessage mouse (its input manager drops mouse messages
        // unless the live system cursor is over the window — verified: posted
        // WM_MOUSE* raise no WPF events). It must be driven by coordinate-routed
        // system-queue input. We use a PERSISTENT synthetic touch digitizer
        // (see inject::TOUCH_DEV): WPF's stylus stack binds to the standing
        // device and consumes the contact as touch/stylus — promoting to mouse
        // internally, with NO OS cursor movement. WM_CHAR keystrokes still work,
        // so flag only the pointer-class events.
        return matches!(kind, MouseClick | MouseMove | MouseScroll);
    }
    // NB: WinUI3 (`WinUIDesktopWin32WindowClass`) is deliberately NOT flagged
    // here. It looks WPF-like, but its composition input-site does NOT consume
    // the synthetic pen/touch the way WPF's stylus stack does — routing WinUI3
    // background clicks through the coordinate injector neither lands double/
    // right-click NOR preserves the no-foreground contract (measured: it
    // regressed ax-bg from 0/8 to 8/8 stolen). Driving WinUI3 double/right-click
    // in the background needs a WinUI3-specific input path (composition input-
    // site target), tracked separately; single left-click already works via UIA
    // Invoke and the contract holds.
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

/// Detect WPF top-level windows. WPF hosts its visual tree in an HWND whose
/// class is `HwndWrapper[<module>;;<guid>]`. WPF TextBoxes consume only real
/// keyboard input routed through WPF's input manager, so a posted `WM_CHAR`
/// (the `post_type_text` path) is silently dropped — type_text must instead
/// deliver genuine SendInput keystrokes (see `inject_text_cloaked`).
pub fn is_wpf_target_window(hwnd: u64) -> bool {
    read_class_name(hwnd).starts_with("HwndWrapper")
}

/// Detect WinUI3 / Windows-App-SDK desktop top-level windows. The frame is a
/// Win32 HWND of class `WinUIDesktopWin32WindowClass`, but — unlike WPF — that
/// frame does NOT host the visual tree or consume pointer input. The XAML
/// content renders into a child *content island* (DirectComposition / a
/// `Microsoft.UI.Content.DesktopChildSiteBridge` HWND) whose input stack only
/// processes real pointer input off the system input queue — it ignores both
/// posted `WM_*BUTTON` messages AND the synthetic pen/touch injector (the
/// latter additionally click-activates the frame: measured 8/8 foreground
/// steals). The only background-safe actuator that lands on a WinUI3 element is
/// a UIA pattern call (see `winui3_uia_multi_invoke` in the tools layer), and
/// that has no right-click / drag analogue — so those gestures cannot both land
/// and hold the no-foreground contract and surface a structured
/// `background_unavailable` error instead.
pub fn is_winui3_target_window(hwnd: u64) -> bool {
    read_class_name(hwnd) == "WinUIDesktopWin32WindowClass"
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
        "Background delivery is not available for target window class \
         '{class}' on this event kind ({}). Either call bring_to_front \
         then retry with delivery_mode:\"foreground\", or accept the foreground \
         swap directly by setting delivery_mode:\"foreground\".",
        kind.name()
    );
    cua_driver_core::protocol::ToolResult::error(text)
        .with_structured(serde_json::json!({
            "code": "background_unavailable",
            "target_class": class,
            "event_kind": kind.name(),
            "suggestion":
                "Either call bring_to_front then retry with delivery_mode:\"foreground\", \
                 or accept the foreground swap by setting delivery_mode:\"foreground\" directly.",
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delivery_mode_parses_known_values() {
        let j = |s: &str| serde_json::json!({"delivery_mode": s});
        assert_eq!(DeliveryMode::from_args(&j("background")), DeliveryMode::Background);
        assert_eq!(DeliveryMode::from_args(&j("foreground")), DeliveryMode::Foreground);
        // Case-insensitive, matching macOS DeliveryMode::parse.
        assert_eq!(DeliveryMode::from_args(&j("Foreground")), DeliveryMode::Foreground);
    }

    #[test]
    fn delivery_mode_defaults_to_background() {
        // Missing field, garbage value, null, and the removed legacy "auto"
        // all resolve to Background. This is the cua-driver no-foreground-by-
        // default contract: an unrecognised value never silently fronts.
        assert_eq!(DeliveryMode::from_args(&serde_json::json!({})), DeliveryMode::Background);
        assert_eq!(
            DeliveryMode::from_args(&serde_json::json!({"delivery_mode": "garbage"})),
            DeliveryMode::Background
        );
        assert_eq!(
            DeliveryMode::from_args(&serde_json::json!({"delivery_mode": "auto"})),
            DeliveryMode::Background
        );
        assert_eq!(
            DeliveryMode::from_args(&serde_json::json!({"delivery_mode": null})),
            DeliveryMode::Background
        );
    }
}
