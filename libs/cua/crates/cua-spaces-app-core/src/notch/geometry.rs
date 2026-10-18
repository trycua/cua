// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Pure geometry and mode mapping for the notch portal window (moved from
//! the Tauri app's `src-tauri/src/geometry.rs`; it re-exports this module).
//!
//! Everything in this module is free of UI types so it can be unit tested
//! without a display. Sizes are in logical points; the webview's
//! `FALLBACK_SIZES` table mirrors [`mode_size`].

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WindowMode {
    Ambient,
    /// Ambient, but grown taller so the drag teleport's "Teleport to Cua" box
    /// has room to hang below the notch. Same top-centre placement as `Ambient`.
    AmbientTeleport,
    Switcher,
    CreateFleet,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DisplayStyle {
    Notched,
    NoNotch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DisplayStyleSource {
    Override,
    Heuristic,
    Default,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LogicalRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl LogicalRect {
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortalGeometry {
    pub mode: WindowMode,
    pub display_style: DisplayStyle,
    pub frame: LogicalRect,
    pub monitor: LogicalRect,
    pub scale_factor: f64,
}

/// Logical window size for a mode. The notched ambient strip is wide enough
/// to wrap a 200pt notch with two shoulders; the no-notch capsule is compact.
pub fn mode_size(mode: WindowMode, style: DisplayStyle) -> Size {
    match (mode, style) {
        (WindowMode::Ambient, DisplayStyle::Notched) => Size {
            width: 420.0,
            // A few px taller than the 38pt notch so the springy hover can scale
            // the cap + tab down without the window clipping them. The extra
            // height sits transparent below the top-anchored notch content.
            height: 46.0,
        },
        (WindowMode::Ambient, DisplayStyle::NoNotch) => Size {
            width: 180.0,
            height: 34.0,
        },
        // Keep the ambient width (so the box stays centred under the notch) but
        // grow the height to fit the compact "Teleport to Cua" box plus shadow.
        (WindowMode::AmbientTeleport, DisplayStyle::Notched) => Size {
            width: 420.0,
            height: 96.0,
        },
        // No notch cap to merge with: widen enough to hold the box and drop it
        // below the centred capsule.
        (WindowMode::AmbientTeleport, DisplayStyle::NoNotch) => Size {
            width: 240.0,
            height: 96.0,
        },
        (WindowMode::Switcher, DisplayStyle::Notched) => Size {
            width: 760.0,
            height: 320.0,
        },
        (WindowMode::Switcher, DisplayStyle::NoNotch) => Size {
            width: 760.0,
            height: 300.0,
        },
        (WindowMode::CreateFleet, DisplayStyle::Notched) => Size {
            width: 600.0,
            height: 700.0,
        },
        (WindowMode::CreateFleet, DisplayStyle::NoNotch) => Size {
            width: 600.0,
            height: 680.0,
        },
    }
}

/// Centre `size` horizontally at the top edge of `monitor`, clamped so the
/// frame never leaves the monitor. Coordinates are rounded to whole points so
/// the window lands on pixel boundaries at any scale factor.
pub fn top_center(monitor: LogicalRect, size: Size) -> LogicalRect {
    let width = size.width.min(monitor.width).max(0.0);
    let height = size.height.min(monitor.height).max(0.0);
    let x = monitor.x + (monitor.width - width) / 2.0;
    let x = x.max(monitor.x).min(monitor.x + monitor.width - width);
    LogicalRect {
        x: x.round(),
        y: monitor.y.round(),
        width: width.round(),
        height: height.round(),
    }
}

/// Convert a physical monitor rectangle to logical points.
pub fn logical_monitor(x: i32, y: i32, width: u32, height: u32, scale: f64) -> LogicalRect {
    let scale = if scale > 0.0 { scale } else { 1.0 };
    LogicalRect {
        x: f64::from(x) / scale,
        y: f64::from(y) / scale,
        width: f64::from(width) / scale,
        height: f64::from(height) / scale,
    }
}

/// Best-effort notch heuristic.
///
/// Tauri has no public safe-area API, so we look at the panel's logical
/// aspect ratio. Notched MacBooks (2021+) report roughly 1.54:1 because the
/// menu-bar strip beside the notch adds height to an otherwise 16:10 panel
/// (e.g. 1512×982, 1728×1117, 1470×956, 1710×1112). Non-notched MacBooks and
/// external displays sit at 1.6:1 or 1.78:1. This is documented as
/// best-effort and can be overridden.
pub fn looks_notched(monitor: LogicalRect) -> bool {
    if monitor.height <= 0.0 {
        return false;
    }
    let ratio = monitor.width / monitor.height;
    (1.52..=1.56).contains(&ratio)
}

/// Parse an explicit override such as the `CUA_SPACES_DISPLAY` env var.
pub fn parse_display_override(value: &str) -> Option<DisplayStyle> {
    match value.trim().to_ascii_lowercase().as_str() {
        "notched" | "notch" => Some(DisplayStyle::Notched),
        "no-notch" | "nonotch" | "plain" => Some(DisplayStyle::NoNotch),
        _ => None,
    }
}

/// Decide the display style from an optional override and the monitor.
pub fn resolve_display_style(
    override_style: Option<DisplayStyle>,
    monitor: Option<LogicalRect>,
) -> (DisplayStyle, DisplayStyleSource) {
    if let Some(style) = override_style {
        return (style, DisplayStyleSource::Override);
    }
    match monitor {
        Some(m) if looks_notched(m) => (DisplayStyle::Notched, DisplayStyleSource::Heuristic),
        Some(_) => (DisplayStyle::NoNotch, DisplayStyleSource::Heuristic),
        None => (DisplayStyle::NoNotch, DisplayStyleSource::Default),
    }
}

pub fn compute_geometry(
    mode: WindowMode,
    style: DisplayStyle,
    monitor: LogicalRect,
    scale_factor: f64,
) -> PortalGeometry {
    PortalGeometry {
        mode,
        display_style: style,
        frame: top_center(monitor, mode_size(mode, style)),
        monitor,
        scale_factor,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MBP14: LogicalRect = LogicalRect::new(0.0, 0.0, 1512.0, 982.0);

    #[test]
    fn mode_sizes_are_distinct_and_grow_with_mode() {
        for style in [DisplayStyle::Notched, DisplayStyle::NoNotch] {
            let a = mode_size(WindowMode::Ambient, style);
            let s = mode_size(WindowMode::Switcher, style);
            let c = mode_size(WindowMode::CreateFleet, style);
            assert!(a.height < s.height && s.height < c.height, "{style:?}");
            assert!(a.width < s.width, "{style:?}");
        }
        // The notched ambient strip must wrap a 200pt notch with room for shoulders.
        assert!(mode_size(WindowMode::Ambient, DisplayStyle::Notched).width >= 200.0 + 2.0 * 80.0);
        // The capsule fallback is deliberately compact.
        assert!(mode_size(WindowMode::Ambient, DisplayStyle::NoNotch).width < 200.0);
        // The composer must leave room for its actions on a 768pt external display.
        assert_eq!(
            mode_size(WindowMode::CreateFleet, DisplayStyle::NoNotch).height,
            680.0
        );
    }

    #[test]
    fn ambient_teleport_grows_taller_without_shifting_the_notch() {
        for style in [DisplayStyle::Notched, DisplayStyle::NoNotch] {
            let ambient = mode_size(WindowMode::Ambient, style);
            let teleport = mode_size(WindowMode::AmbientTeleport, style);
            // Taller so the "Teleport to Cua" box fits below the notch.
            assert!(teleport.height > ambient.height, "{style:?}");
            // Wide enough to hold the notch-width box centred under the notch.
            assert!(teleport.width >= 220.0, "{style:?}");
        }
        // On notched displays the width is unchanged, so the box stays centred on
        // the same axis as the idle ambient tab / notch cap.
        assert_eq!(
            mode_size(WindowMode::AmbientTeleport, DisplayStyle::Notched).width,
            mode_size(WindowMode::Ambient, DisplayStyle::Notched).width
        );
        assert_eq!(
            serde_json::to_string(&WindowMode::AmbientTeleport).unwrap(),
            "\"ambient-teleport\""
        );
    }

    #[test]
    fn top_center_centres_on_primary_monitor() {
        let frame = top_center(
            MBP14,
            Size {
                width: 760.0,
                height: 320.0,
            },
        );
        assert_eq!(frame, LogicalRect::new(376.0, 0.0, 760.0, 320.0));
    }

    #[test]
    fn top_center_respects_monitor_origin() {
        // Secondary display to the right of, and slightly below, the primary.
        let monitor = LogicalRect::new(1512.0, 120.0, 1920.0, 1080.0);
        let frame = top_center(
            monitor,
            Size {
                width: 600.0,
                height: 620.0,
            },
        );
        assert_eq!(frame.x, 1512.0 + 660.0);
        assert_eq!(frame.y, 120.0);
    }

    #[test]
    fn top_center_clamps_oversized_windows() {
        let tiny = LogicalRect::new(-500.0, 40.0, 400.0, 300.0);
        let frame = top_center(
            tiny,
            Size {
                width: 760.0,
                height: 620.0,
            },
        );
        assert_eq!(frame, LogicalRect::new(-500.0, 40.0, 400.0, 300.0));
    }

    #[test]
    fn top_center_rounds_to_whole_points() {
        let odd = LogicalRect::new(0.0, 0.0, 1513.0, 982.0);
        let frame = top_center(
            odd,
            Size {
                width: 420.0,
                height: 38.0,
            },
        );
        assert_eq!(frame.x, 547.0); // 546.5 rounds up
        assert_eq!(frame.x.fract(), 0.0);
    }

    #[test]
    fn logical_monitor_divides_by_scale() {
        let m = logical_monitor(0, 0, 3024, 1964, 2.0);
        assert_eq!(m, MBP14);
        // A bogus scale factor falls back to 1.
        assert_eq!(
            logical_monitor(10, 20, 100, 50, 0.0),
            LogicalRect::new(10.0, 20.0, 100.0, 50.0)
        );
    }

    #[test]
    fn notch_heuristic_matches_known_panels() {
        for (w, h) in [
            (1512.0, 982.0),
            (1728.0, 1117.0),
            (1470.0, 956.0),
            (1710.0, 1112.0),
            (1800.0, 1169.0),
        ] {
            assert!(looks_notched(LogicalRect::new(0.0, 0.0, w, h)), "{w}x{h}");
        }
        for (w, h) in [
            (1440.0, 900.0),
            (1680.0, 1050.0),
            (1920.0, 1080.0),
            (2560.0, 1440.0),
            (0.0, 0.0),
        ] {
            assert!(!looks_notched(LogicalRect::new(0.0, 0.0, w, h)), "{w}x{h}");
        }
    }

    #[test]
    fn display_style_resolution_prefers_override() {
        assert_eq!(
            resolve_display_style(Some(DisplayStyle::NoNotch), Some(MBP14)),
            (DisplayStyle::NoNotch, DisplayStyleSource::Override)
        );
        assert_eq!(
            resolve_display_style(None, Some(MBP14)),
            (DisplayStyle::Notched, DisplayStyleSource::Heuristic)
        );
        assert_eq!(
            resolve_display_style(None, None),
            (DisplayStyle::NoNotch, DisplayStyleSource::Default)
        );
    }

    #[test]
    fn display_override_parsing() {
        assert_eq!(
            parse_display_override(" Notched "),
            Some(DisplayStyle::Notched)
        );
        assert_eq!(
            parse_display_override("no-notch"),
            Some(DisplayStyle::NoNotch)
        );
        assert_eq!(parse_display_override("wat"), None);
    }

    #[test]
    fn serde_uses_kebab_case_for_modes() {
        assert_eq!(
            serde_json::to_string(&WindowMode::CreateFleet).unwrap(),
            "\"create-fleet\""
        );
        assert_eq!(
            serde_json::from_str::<WindowMode>("\"switcher\"").unwrap(),
            WindowMode::Switcher
        );
        assert_eq!(
            serde_json::to_string(&DisplayStyle::NoNotch).unwrap(),
            "\"no-notch\""
        );
        let geo = compute_geometry(WindowMode::Ambient, DisplayStyle::Notched, MBP14, 2.0);
        let json = serde_json::to_value(geo).unwrap();
        assert_eq!(json["displayStyle"], "notched");
        assert_eq!(json["frame"]["x"], 546.0);
        assert_eq!(json["scaleFactor"], 2.0);
    }
}
