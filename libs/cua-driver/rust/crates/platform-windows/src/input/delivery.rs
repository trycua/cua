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
            Self::MouseClick => "mouse_click",
            Self::MouseMove => "mouse_move",
            Self::MouseScroll => "mouse_scroll",
            Self::Keystroke => "keystroke",
            Self::KeyCombo => "key_combo",
            Self::TextInput => "text_input",
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

    pub fn is_foreground(self) -> bool {
        matches!(self, Self::Foreground)
    }
}

/// JSON-schema fragment for the `delivery_mode` field. Include this in every
/// input tool's `input_schema.properties.delivery_mode`. Two modes, matching
/// the macOS surface.
pub fn delivery_mode_schema() -> Value {
    // Borrow the canonical SHAPE (type/enum) from the shared cross-platform
    // builder so it can never drift from the consistency gate, while keeping the
    // Windows-specific prose (UIA Invoke / PostMessage / background_unavailable).
    cua_driver_core::tool_schema::delivery_mode_schema_with(
        "Input delivery mode. 'background' (default) never swaps foreground: \
         it routes through UIA Invoke / PostMessage and the window is never \
         raised. For targets whose input stack silently drops posted events \
         (Chromium/Electron content, GTK buttons, VCL/LibreOffice accelerators) \
         the tool returns a structured background_unavailable error rather than \
         fronting. 'foreground' is the explicit escalation: a brief \
         SetForegroundWindow swap + SendInput, restoring the prior foreground \
         afterward (call bring_to_front first to avoid the flash). \
         IMPORTANT: 'background' is not a hint to weigh — it is the mandatory \
         first attempt. Do NOT pass 'foreground' preemptively because a target \
         'looks like' GTK/Chromium/Electron; the DRIVER decides when background \
         is impossible and tells you so via a background_unavailable error (or a \
         verified no-op). Only THEN re-issue the same action with 'foreground'. \
         The lists above are the driver's detectors, not a checklist for you to \
         front on a guess — fronting up-front needlessly steals the user's focus \
         and is a bug, not a shortcut. Matches the macOS delivery_mode surface.",
    )
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
        // origin for pointer and keyboard events (#1623). Posted WM_CHAR and
        // plain key messages can return success while a background renderer
        // receives nothing, so they must be refused as honestly as chords.
        return matches!(
            kind,
            MouseClick | MouseMove | MouseScroll | Keystroke | KeyCombo | TextInput
        );
    }
    if crate::input::has_chromium_descendant(hwnd) {
        // Embedded WebView2 hosts retain useful UIA/top-level routes for
        // clicks and ValuePattern text. Their drag, wheel and modifier-chord
        // paths still depend on the renderer's system input queue.
        return matches!(kind, MouseMove | MouseScroll | KeyCombo);
    }
    if is_wpf_target_window(hwnd) {
        // WPF ignores posted pointer messages (its input manager drops
        // WM_MOUSE* unless the live system cursor is over the window). It must
        // be driven by coordinate-routed system-queue input for clicks/moves.
        //
        // Do not classify WM_VSCROLL/WM_HSCROLL here: the scroll tool posts the
        // scrollbar messages directly to the top-level HWND, and WPF hosts that
        // explicitly handle those messages (including our harness hook) can
        // consume them without a foreground swap.
        return matches!(kind, MouseClick | MouseMove);
    }
    if is_tk_target_window(hwnd) {
        // Tk's Windows event loop does not treat posted WM_CHAR/WM_KEYDOWN as
        // genuine keyboard input for the focused widget. The messages can be
        // accepted by PostMessage while the Entry receives nothing, so refuse
        // instead of reporting a false background success.
        return matches!(kind, Keystroke | KeyCombo | TextInput);
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
/// (the `post_type_text` path) is silently dropped — so in background, type_text
/// with no element_index returns `background_unavailable` (escalate to
/// `delivery_mode:"foreground"`); with an element_index it uses UIA ValuePattern.
pub fn is_wpf_target_window(hwnd: u64) -> bool {
    read_class_name(hwnd).starts_with("HwndWrapper")
}

/// Detect Tk/Tkinter top-level windows. Tk registers this stable class name
/// for its root and child toplevels on Windows.
pub fn is_tk_target_window(hwnd: u64) -> bool {
    is_tk_class_name(&read_class_name(hwnd))
}

fn is_tk_class_name(class: &str) -> bool {
    class == "TkTopLevel" || class.starts_with("TkTopLevel.")
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
    cua_driver_core::protocol::ToolResult::error(text).with_structured(serde_json::json!({
        "code": "background_unavailable",
        "target_class": class,
        "event_kind": kind.name(),
        "suggestion":
            "Either call bring_to_front then retry with delivery_mode:\"foreground\", \
             or accept the foreground swap by setting delivery_mode:\"foreground\" directly.",
        // Windows analog of the macOS escalation signal: this surface drops
        // background input, so the deliberate next rung is foreground delivery.
        "escalation": {
            "recommended": "foreground",
            "reason": "background input is dropped by this surface — re-call \
                       delivery_mode:\"foreground\" (or bring_to_front first).",
        },
    }))
}

/// Build a structured background refusal while preserving the concrete
/// actuator failure. This is used after a background coordinate-injection
/// fallback was attempted and failed, so callers can distinguish a true
/// occlusion miss from a generic class-level refusal.
pub fn background_unavailable_error_with_cause(
    hwnd: u64,
    kind: EventKind,
    cause: impl Into<String>,
) -> cua_driver_core::protocol::ToolResult {
    let class = read_class_name(hwnd);
    let cause = cause.into();
    let cause_lower = cause.to_ascii_lowercase();
    let code = if cause_lower.contains("occluded") {
        "background_occluded"
    } else if cause_lower.contains("uipi") || cause_lower.contains("higher-integrity") {
        "background_uipi_blocked"
    } else {
        "background_unavailable"
    };
    let text = format!(
        "Background delivery is not available for target window class \
         '{class}' on this event kind ({}): {cause}",
        kind.name()
    );
    cua_driver_core::protocol::ToolResult::error(text).with_structured(serde_json::json!({
        "code": code,
        "target_class": class,
        "event_kind": kind.name(),
        "cause": cause,
        "suggestion":
            "Either call bring_to_front then retry with delivery_mode:\"foreground\", \
             or accept the foreground swap by setting delivery_mode:\"foreground\" directly.",
        "escalation": {
            "recommended": "foreground",
            "reason": "background input could not be delivered by the coordinate actuator — \
                       re-call delivery_mode:\"foreground\" (or bring_to_front first).",
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_tk_toplevel_classes_without_matching_unrelated_windows() {
        assert!(is_tk_class_name("TkTopLevel"));
        assert!(is_tk_class_name("TkTopLevel.1"));
        assert!(!is_tk_class_name("TkChild"));
        assert!(!is_tk_class_name("Chrome_WidgetWin_1"));
    }
    #[test]
    fn delivery_mode_parses_known_values() {
        let j = |s: &str| serde_json::json!({"delivery_mode": s});
        assert_eq!(
            DeliveryMode::from_args(&j("background")),
            DeliveryMode::Background
        );
        assert_eq!(
            DeliveryMode::from_args(&j("foreground")),
            DeliveryMode::Foreground
        );
        // Case-insensitive, matching macOS DeliveryMode::parse.
        assert_eq!(
            DeliveryMode::from_args(&j("Foreground")),
            DeliveryMode::Foreground
        );
    }

    #[test]
    fn delivery_mode_defaults_to_background() {
        // Missing field, garbage value, null, and the removed legacy "auto"
        // all resolve to Background. This is the cua-driver no-foreground-by-
        // default contract: an unrecognised value never silently fronts.
        assert_eq!(
            DeliveryMode::from_args(&serde_json::json!({})),
            DeliveryMode::Background
        );
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

    #[test]
    fn background_unavailable_with_cause_preserves_actuator_failure() {
        let result = background_unavailable_error_with_cause(
            0,
            EventKind::MouseClick,
            "target point is occluded by another window",
        );
        let structured = result.structured_content.as_ref().expect("structured");
        assert_eq!(result.is_error, Some(true));
        assert_eq!(structured["code"].as_str(), Some("background_occluded"));
        assert_eq!(structured["event_kind"].as_str(), Some("mouse_click"));
        assert!(structured["cause"]
            .as_str()
            .unwrap_or_default()
            .contains("occluded"));

        let text = match &result.content[0] {
            cua_driver_core::protocol::Content::Text { text, .. } => text,
            _ => panic!("expected text content"),
        };
        assert!(text.contains("occluded"), "result text lost cause: {text}");

        let uipi = background_unavailable_error_with_cause(
            0,
            EventKind::MouseClick,
            "blocked by UIPI higher-integrity target",
        );
        assert_eq!(
            uipi.structured_content
                .as_ref()
                .and_then(|v| v["code"].as_str()),
            Some("background_uipi_blocked")
        );
    }
}
