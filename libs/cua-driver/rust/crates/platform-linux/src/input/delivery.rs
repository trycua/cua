//! Shared delivery-mode logic for cua-driver Linux input tools.
//!
//! Mirrors macOS `tools::DeliveryMode` and Windows `input::delivery`: each
//! input tool accepts an optional `delivery_mode` field with exactly two modes
//! — the agent-selected rung of the best-effort-background ladder, passed per
//! call (never a stored setting):
//!
//! - `background` (DEFAULT) — inject without activating/raising the target.
//!   - **X11**: XTEST / XSendEvent / XInput2 MPX master-pointer no-focus-steal
//!     path (see `input::mod`). Lands on a backgrounded window without stealing
//!     focus — the direct analogue of macOS CGEvent / Windows PostMessage.
//!   - **Wayland**: libei via xdg-desktop-portal (`wayland::libei`). Wayland's
//!     security model has NO arbitrary per-window background targeting — libei
//!     injects to the compositor's input focus, so "background" cannot aim at a
//!     specific non-focused window the way X11/macOS/Windows can (this is a
//!     platform constraint, reported honestly, like macOS pixel input being
//!     driver-unverifiable). When no libei backend is available
//!     (`PORTAL_LIBEI_ENABLED == false`) the tool returns a structured
//!     `background_unavailable` error so the caller can escalate to foreground.
//!
//! - `foreground` — activate the target first, inject, then restore the prior
//!   active window.
//!   - **X11**: EWMH `_NET_ACTIVE_WINDOW` client message (the `wmctrl -a`
//!     equivalent) raise + activate.
//!   - **Wayland**: compositor-specific activate where supported; else the
//!     libei-to-focus path with an honest note.
//!   The agent's explicit last resort; a brief focus swap unless the target was
//!   already active. Matches the macOS / Windows `foreground` rung.

use serde_json::Value;

/// Input delivery modality — the agent-selected rung of the best-effort-
/// background ladder, passed per call (never a stored/config setting). Mirrors
/// macOS `tools::DeliveryMode` and Windows `input::delivery::DeliveryMode`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum DeliveryMode {
    /// Inject without activating the target (X11 XTEST/MPX; Wayland libei-to-focus).
    #[default]
    Background,
    /// Activate the target, inject, restore prior active window.
    Foreground,
}

impl DeliveryMode {
    /// Parse the per-call `delivery_mode` argument. Anything other than an
    /// explicit case-insensitive `"foreground"` resolves to `Background` — the
    /// correct default, so an omitted/garbage value never silently fronts.
    /// Matches macOS / Windows `DeliveryMode::parse`.
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
/// the macOS / Windows surface.
pub fn delivery_mode_schema() -> Value {
    // Source the SHAPE (`type` + `enum`) from the shared canon so it can't drift
    // from macOS / Windows, while keeping the Linux-specific Wayland/X11 prose
    // (the gate compares shape only — descriptions may vary per platform). The
    // `default` advertisement is additive on top of the canon shape.
    let mut v = cua_driver_core::tool_schema::delivery_mode_schema_with(
        "Input delivery mode. 'background' (default) never activates or raises \
         the target window. On X11 it injects via XTEST / the XInput2 master \
         pointer (no focus steal). On Wayland it goes through libei + \
         xdg-desktop-portal, which injects to the compositor's input focus — \
         Wayland's security model has no per-window background targeting, so a \
         specific non-focused window cannot be aimed at; when no libei backend \
         is available the tool returns a structured background_unavailable \
         error. 'foreground' is the explicit escalation: activate the target \
         (X11 _NET_ACTIVE_WINDOW; Wayland compositor activate), inject, then \
         restore the prior active window — a brief focus swap unless the \
         target was already active. Call bring_to_front first to avoid the \
         flash. Matches the macOS / Windows delivery_mode surface.",
    );
    v["default"] = serde_json::json!("background");
    v
}

/// Reason a `background` delivery cannot be performed on Wayland.
#[derive(Copy, Clone, Debug)]
pub enum BackgroundUnavailable {
    /// No libei backend (built without `portal-libei`, or the portal session
    /// was denied / unavailable). Input has no actuator at all.
    NoLibeiBackend,
    /// X11/Chromium does not accept synthetic pointer or keyboard input
    /// addressed to an occluded, unfocused renderer without briefly moving
    /// focus, which background delivery forbids.
    ChromiumInput,
    /// The remaining backend can only inject into the globally focused widget.
    FocusedInputOnly,
}

impl BackgroundUnavailable {
    fn code(self) -> &'static str {
        match self {
            Self::NoLibeiBackend => "background_unavailable",
            Self::ChromiumInput => "background_unavailable",
            Self::FocusedInputOnly => "background_unavailable",
        }
    }
    fn detail(self) -> &'static str {
        match self {
            Self::NoLibeiBackend => {
                "no libei input backend on this Wayland compositor (built without \
                 portal-libei, or the xdg-desktop-portal RemoteDesktop session was \
                 unavailable/denied): synthetic input has no actuator"
            }
            Self::ChromiumInput => {
                "Chromium/Electron does not accept pointer or keyboard input \
                 addressed to an occluded, unfocused renderer through X11 \
                 background injection"
            }
            Self::FocusedInputOnly => {
                "the requested target has no focus-free input backend; the remaining XTest/X11 route can only deliver to the globally focused widget"
            }
        }
    }
}

/// Build the structured `background_unavailable` error returned when a
/// `delivery_mode:"background"` injection has no Wayland backend. Mirrors the
/// Windows silent-drop error so callers branch on `code` identically.
pub fn background_unavailable_error(
    reason: BackgroundUnavailable,
) -> cua_driver_core::protocol::ToolResult {
    let detail = reason.detail();
    cua_driver_core::protocol::ToolResult::error(format!(
        "Background delivery is not available: {detail}. Either call bring_to_front \
         then retry with delivery_mode:\"foreground\", or accept activation by \
         setting delivery_mode:\"foreground\" directly."
    ))
    .with_structured(serde_json::json!({
        "code": reason.code(),
        "detail": detail,
        "suggestion":
            "Either call bring_to_front then retry with delivery_mode:\"foreground\", \
             or set delivery_mode:\"foreground\" directly.",
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // Case-insensitive, matching macOS / Windows.
        assert_eq!(
            DeliveryMode::from_args(&j("Foreground")),
            DeliveryMode::Foreground
        );
    }

    #[test]
    fn delivery_mode_defaults_to_background() {
        // Missing field, garbage value, null, and the removed legacy "auto" all
        // resolve to Background — the no-foreground-by-default contract.
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
    fn delivery_mode_schema_advertises_two_modes() {
        let s = delivery_mode_schema();
        let en: Vec<&str> = s["enum"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert_eq!(en, vec!["background", "foreground"]);
        assert_eq!(s["default"], "background");
    }

    #[test]
    fn background_unavailable_error_carries_code() {
        let r = background_unavailable_error(BackgroundUnavailable::NoLibeiBackend);
        assert_eq!(r.is_error, Some(true));
        assert_eq!(
            r.structured_content.as_ref().unwrap()["code"],
            serde_json::json!("background_unavailable")
        );
    }
}
