// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Whole-display targets on macOS: the active CoreGraphics displays, in
//! global logical points (the coordinate space cua-driver's click and
//! pointer tools use), with the current mode's backing pixel size.
//! Capture goes through ScreenCaptureKit (`macos_capture::CaptureSource::
//! Display`), keyed by the same CGDirectDisplayID.

use core_graphics::display::CGDisplay;
use cua_spacesd_provider_api::ProviderDisplay;

/// Every active display, the main (menu bar) display first.
pub(crate) fn displays() -> Vec<ProviderDisplay> {
    let main = CGDisplay::main().id;
    let mut displays: Vec<ProviderDisplay> = CGDisplay::active_displays()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|id| display(id, id == main))
        .collect();
    displays.sort_by_key(|display| !display.primary);
    displays
}

fn display(id: u32, primary: bool) -> Option<ProviderDisplay> {
    let cg = CGDisplay::new(id);
    let bounds = cg.bounds();
    let (width, height) = (bounds.size.width, bounds.size.height);
    if width < 1.0 || height < 1.0 {
        return None;
    }
    let mode = cg.display_mode();
    let (native_width_px, native_height_px, refresh_rate_hz) = match &mode {
        Some(mode) if mode.pixel_width() > 0 && mode.pixel_height() > 0 => (
            mode.pixel_width() as u32,
            mode.pixel_height() as u32,
            mode.refresh_rate().round().max(0.0) as u32,
        ),
        _ => (width.round() as u32, height.round() as u32, 0),
    };
    Some(ProviderDisplay {
        id: id.to_string(),
        name: format!("display-{id}"),
        primary,
        bounds: (bounds.origin.x, bounds.origin.y, width, height),
        native_width_px,
        native_height_px,
        scale_factor: backing_scale(native_width_px, width),
        refresh_rate_hz,
    })
}

/// Backing pixels per point, rounded to the nearest half (the same rule as
/// cua-driver's `get_screen_size`).
fn backing_scale(pixel_width: u32, point_width: f64) -> f64 {
    if pixel_width == 0 || point_width < 1.0 {
        return 1.0;
    }
    ((f64::from(pixel_width) / point_width) * 2.0)
        .round()
        .max(2.0)
        / 2.0
}

/// The CGDirectDisplayID behind a display id string.
pub(crate) fn parse_id(display_id: &str) -> Option<u32> {
    display_id.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backing_scale_rounds_to_halves() {
        assert_eq!(backing_scale(1024, 1024.0), 1.0);
        assert_eq!(backing_scale(2880, 1440.0), 2.0);
        assert_eq!(backing_scale(1511, 1007.0), 1.5);
        assert_eq!(backing_scale(0, 1024.0), 1.0);
    }

    #[test]
    fn display_ids_are_numeric() {
        assert_eq!(parse_id("69734272"), Some(69_734_272));
        assert_eq!(parse_id("primary"), None);
    }
}
