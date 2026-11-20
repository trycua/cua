// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Adapted from injaneity's multi-display implementation in trycua/cua#3019.
//!
//! Pure macOS display geometry for agent-cursor presentation.
//!
//! Cua cursor coordinates use CoreGraphics' global top-left coordinate space.
//! AppKit window frames use a bottom-left origin. Keeping both frames here
//! prevents either convention from leaking into cursor state or rendering.

use cursor_overlay::{ScreenFrame, SurfaceGeometry};
use objc2_foundation::{NSPoint, NSRect, NSSize};

pub(crate) type DisplayId = u32;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct DisplayGeometry {
    pub id: DisplayId,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub backing_scale: f64,
    pub is_primary: bool,
}

impl DisplayGeometry {
    pub(crate) fn contains(self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }

    #[cfg(test)]
    pub(crate) fn pixel_size(self) -> (u32, u32) {
        let scale = self.backing_scale.max(1.0);
        (
            (self.width * scale).round().max(1.0) as u32,
            (self.height * scale).round().max(1.0) as u32,
        )
    }

    /// The overlay surface that covers this display.
    pub(crate) fn surface_geometry(self) -> SurfaceGeometry {
        SurfaceGeometry::new(
            ScreenFrame::new(self.x, self.y, self.width, self.height),
            self.backing_scale,
        )
    }

    /// Convert the CoreGraphics top-left frame into AppKit's bottom-left frame.
    pub(crate) fn appkit_frame(self, primary_height: f64) -> NSRect {
        NSRect::new(
            NSPoint::new(self.x, primary_height - self.y - self.height),
            NSSize::new(self.width, self.height),
        )
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct DisplayLayout {
    pub generation: u64,
    pub displays: Vec<DisplayGeometry>,
}

impl DisplayLayout {
    pub(crate) fn display_at(&self, x: f64, y: f64) -> Option<DisplayGeometry> {
        self.displays
            .iter()
            .copied()
            .find(|display| display.contains(x, y))
    }

    pub(crate) fn primary_height(&self) -> Option<f64> {
        self.displays
            .iter()
            .find(|display| display.is_primary)
            .or_else(|| self.displays.first())
            .map(|display| display.height)
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn active_layout(generation: u64) -> Result<DisplayLayout, i32> {
    use core_graphics::display::CGDisplay;

    let primary_id = CGDisplay::main().id;
    let mut displays = CGDisplay::active_displays()?
        .into_iter()
        .filter_map(|id| {
            let display = CGDisplay::new(id);
            // A mirrored destination shares its source's coordinate space and
            // already receives that source window through system mirroring.
            if display.mirrors_display() != 0 {
                return None;
            }
            let bounds = display.bounds();
            let width = bounds.size.width;
            let height = bounds.size.height;
            if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
                return None;
            }
            let scale = crate::tools::get_screen_size::get_backing_scale(id);
            Some(DisplayGeometry {
                id,
                x: bounds.origin.x,
                y: bounds.origin.y,
                width,
                height,
                backing_scale: if scale.is_finite() && scale > 0.0 {
                    scale
                } else {
                    1.0
                },
                is_primary: id == primary_id,
            })
        })
        .collect::<Vec<_>>();
    displays.sort_by_key(|display| (!display.is_primary, display.id));
    Ok(DisplayLayout {
        generation,
        displays,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> DisplayLayout {
        DisplayLayout {
            generation: 7,
            displays: vec![
                DisplayGeometry {
                    id: 1,
                    x: 0.0,
                    y: 0.0,
                    width: 1440.0,
                    height: 900.0,
                    backing_scale: 2.0,
                    is_primary: true,
                },
                DisplayGeometry {
                    id: 2,
                    x: -1920.0,
                    y: -180.0,
                    width: 1920.0,
                    height: 1080.0,
                    backing_scale: 1.0,
                    is_primary: false,
                },
                DisplayGeometry {
                    id: 3,
                    x: 0.0,
                    y: -1200.0,
                    width: 1200.0,
                    height: 1200.0,
                    backing_scale: 1.5,
                    is_primary: false,
                },
            ],
        }
    }

    #[test]
    fn routes_negative_axes_to_their_displays() {
        let layout = layout();
        assert_eq!(layout.display_at(-867.0, 400.0).unwrap().id, 2);
        assert_eq!(layout.display_at(300.0, -867.0).unwrap().id, 3);
    }

    #[test]
    fn seams_are_half_open_and_owned_once() {
        let layout = layout();
        assert_eq!(layout.display_at(-0.001, 100.0).unwrap().id, 2);
        assert_eq!(layout.display_at(0.0, 100.0).unwrap().id, 1);
    }

    #[test]
    fn each_display_keeps_its_own_pixel_scale() {
        let layout = layout();
        let pixel_sizes = layout
            .displays
            .iter()
            .map(|display| (display.id, display.pixel_size()))
            .collect::<Vec<_>>();
        assert_eq!(
            pixel_sizes,
            vec![(1, (2880, 1800)), (2, (1920, 1080)), (3, (1800, 1800))]
        );
    }

    /// The layout from trycua/cua#4907: a 1728x1117 @2x primary below a
    /// 5120x1440 @1x display at (-4333, -1440). Each surface must sit at its
    /// own display's AppKit frame, as `NSScreen.screens` reports it, not at the
    /// primary's origin with the other display's size.
    #[test]
    fn upper_left_mixed_scale_display_keeps_its_own_appkit_frame() {
        let layout = DisplayLayout {
            generation: 0,
            displays: vec![
                DisplayGeometry {
                    id: 1,
                    x: 0.0,
                    y: 0.0,
                    width: 1728.0,
                    height: 1117.0,
                    backing_scale: 2.0,
                    is_primary: true,
                },
                DisplayGeometry {
                    id: 2,
                    x: -4333.0,
                    y: -1440.0,
                    width: 5120.0,
                    height: 1440.0,
                    backing_scale: 1.0,
                    is_primary: false,
                },
            ],
        };
        let primary_height = layout.primary_height().unwrap();
        let frames = layout
            .displays
            .iter()
            .map(|d| {
                let f = d.appkit_frame(primary_height);
                (f.origin.x, f.origin.y, f.size.width, f.size.height)
            })
            .collect::<Vec<_>>();
        assert_eq!(
            frames,
            vec![
                (0.0, 0.0, 1728.0, 1117.0),
                (-4333.0, 1117.0, 5120.0, 1440.0)
            ]
        );
        assert_eq!(layout.display_at(800.0, 500.0).unwrap().id, 1);
        assert_eq!(layout.display_at(-2000.0, -700.0).unwrap().id, 2);
        assert_eq!(layout.display_at(700.0, -1.0).unwrap().id, 2);
    }

    #[test]
    fn appkit_frame_flips_global_y_around_the_primary_display() {
        let display = layout().displays[2];
        let frame = display.appkit_frame(900.0);
        assert_eq!(frame.origin.x, 0.0);
        assert_eq!(frame.origin.y, 900.0);
        assert_eq!(frame.size.width, 1200.0);
        assert_eq!(frame.size.height, 1200.0);
    }
}
