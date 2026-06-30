//! Shared `capture_mode` vocabulary for `get_window_state`.
//!
//! Two modes, mapping 1:1 to the two action-addressing paths so the agent
//! commits to a modality instead of getting handed both:
//!
//! - [`CaptureMode::Ax`] — the accessibility path. Walk the AX/UIA/AT-SPI tree
//!   and return it; act via `element_index`. **No screenshot is delivered** in
//!   the response (the frame is captured to disk only when `screenshot_out_file`
//!   is set). Keeps the hot lookup path cheap — no image tokens.
//! - [`CaptureMode::Vision`] — the visual path. Return a fresh screenshot only
//!   (skip the AX walk); act via `x,y` pixel coordinates. The agent's deliberate
//!   switch to pixels when the surface has no usable AX tree (canvas / WebGL /
//!   Electron web content) or the tree is too large (#22865).
//!
//! The old three-mode vocabulary (`som` = "AX + screenshot") is gone: because a
//! screenshot is no longer delivered on the AX path, `som` collapsed into `ax`.
//! `som` and `tree` still decode to [`CaptureMode::Ax`] and `screenshot` to
//! [`CaptureMode::Vision`] as deprecated aliases so existing MCP/CLI callers
//! don't hard-break — but they're no longer advertised in the schema enum.

/// Which modality `get_window_state` captures. See the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CaptureMode {
    /// Accessibility path: tree in the response, screenshot withheld.
    #[default]
    Ax,
    /// Visual path: fresh screenshot in the response, AX walk skipped.
    Vision,
}

impl CaptureMode {
    /// Parse the inbound `capture_mode` argument. Absent or unrecognised →
    /// [`CaptureMode::Ax`] (the default). Deprecated aliases are accepted:
    /// `som` / `tree` → `Ax`, `screenshot` → `Vision`.
    pub fn parse(raw: Option<&str>) -> Self {
        match raw.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
            Some("vision") | Some("screenshot") => CaptureMode::Vision,
            // "ax", the "som"/"tree" deprecated aliases, anything else → Ax.
            _ => CaptureMode::Ax,
        }
    }

    /// True when the AX tree should be walked and returned (i.e. `Ax` mode).
    pub fn walks_ax(self) -> bool {
        matches!(self, CaptureMode::Ax)
    }

    /// True when a screenshot should be **delivered to the caller** in the
    /// response (i.e. `Vision` mode). In `Ax` mode a frame is only captured to
    /// satisfy `screenshot_out_file`, never embedded — see the get_window_state
    /// tools.
    pub fn delivers_screenshot(self) -> bool {
        matches!(self, CaptureMode::Vision)
    }

    /// Canonical wire string for the resolved mode.
    pub fn as_str(self) -> &'static str {
        match self {
            CaptureMode::Ax => "ax",
            CaptureMode::Vision => "vision",
        }
    }
}

/// The `capture_mode` JSON-schema fragment, shared by every platform's
/// `get_window_state` tool definition so the advertised enum + description stay
/// identical across macOS / Linux / Windows. Only the two live modes are
/// advertised; the `som` / `screenshot` aliases still decode (see
/// [`CaptureMode::parse`]) but are intentionally not listed.
pub fn capture_mode_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "string",
        "enum": ["ax", "vision"],
        "description": "Which modality to capture — maps 1:1 to how you'll act. \
            \"ax\" (default): walk the accessibility tree and return it; act via \
            element_index. No screenshot is delivered (the AX path stays cheap; \
            set screenshot_out_file to also write the frame to disk). \"vision\": \
            return a fresh screenshot only (no AX walk); act via x,y pixel \
            coordinates. Pick \"vision\" when the surface has no usable AX tree \
            (canvas / WebGL / Electron web content) or the tree is too large — \
            that is your deliberate switch to the pixel path. (\"som\" and \
            \"screenshot\" are accepted as deprecated aliases for \"ax\" and \
            \"vision\".)"
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_ax() {
        assert_eq!(CaptureMode::default(), CaptureMode::Ax);
        assert_eq!(CaptureMode::parse(None), CaptureMode::Ax);
    }

    #[test]
    fn parses_live_modes() {
        assert_eq!(CaptureMode::parse(Some("ax")), CaptureMode::Ax);
        assert_eq!(CaptureMode::parse(Some("vision")), CaptureMode::Vision);
    }

    #[test]
    fn deprecated_aliases_decode() {
        // som / tree collapse into ax (no screenshot on the AX path anymore).
        assert_eq!(CaptureMode::parse(Some("som")), CaptureMode::Ax);
        assert_eq!(CaptureMode::parse(Some("tree")), CaptureMode::Ax);
        // screenshot is the old name for vision.
        assert_eq!(CaptureMode::parse(Some("screenshot")), CaptureMode::Vision);
    }

    #[test]
    fn parse_is_case_and_whitespace_insensitive() {
        assert_eq!(CaptureMode::parse(Some("  VISION ")), CaptureMode::Vision);
        assert_eq!(CaptureMode::parse(Some("AX")), CaptureMode::Ax);
    }

    #[test]
    fn unknown_falls_back_to_ax() {
        assert_eq!(CaptureMode::parse(Some("banana")), CaptureMode::Ax);
        assert_eq!(CaptureMode::parse(Some("")), CaptureMode::Ax);
    }

    #[test]
    fn mode_predicates() {
        assert!(CaptureMode::Ax.walks_ax());
        assert!(!CaptureMode::Ax.delivers_screenshot());
        assert!(!CaptureMode::Vision.walks_ax());
        assert!(CaptureMode::Vision.delivers_screenshot());
    }

    #[test]
    fn schema_advertises_only_live_modes() {
        let schema = capture_mode_schema();
        let modes = schema["enum"].as_array().unwrap();
        assert_eq!(modes.len(), 2);
        assert!(modes.iter().any(|m| m == "ax"));
        assert!(modes.iter().any(|m| m == "vision"));
        assert!(!modes.iter().any(|m| m == "som"));
    }
}
