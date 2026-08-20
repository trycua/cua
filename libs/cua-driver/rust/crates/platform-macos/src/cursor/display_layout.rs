//! Pure macOS display geometry for agent-cursor presentation.
//!
//! Cua cursor coordinates use CoreGraphics' global top-left coordinate space.
//! AppKit window frames use a bottom-left origin. Keeping both frames here
//! prevents either convention from leaking into cursor state or rendering.

use objc2_foundation::{NSPoint, NSRect, NSSize};

pub(crate) type DisplayId = u32;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct GlobalPoint {
    pub x: f64,
    pub y: f64,
}

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
    pub(crate) fn contains(self, point: GlobalPoint) -> bool {
        point.x >= self.x
            && point.x < self.x + self.width
            && point.y >= self.y
            && point.y < self.y + self.height
    }

    pub(crate) fn local_point(self, point: GlobalPoint) -> GlobalPoint {
        GlobalPoint {
            x: point.x - self.x,
            y: point.y - self.y,
        }
    }

    pub(crate) fn pixel_size(self) -> (u32, u32) {
        let scale = self.backing_scale.max(1.0);
        (
            (self.width * scale).round().max(1.0) as u32,
            (self.height * scale).round().max(1.0) as u32,
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
    pub(crate) fn route(&self, point: GlobalPoint) -> Option<(DisplayId, GlobalPoint)> {
        self.displays
            .iter()
            .find(|display| display.contains(point))
            .map(|display| (display.id, display.local_point(point)))
    }

    #[cfg(test)]
    pub(crate) fn display(&self, id: DisplayId) -> Option<DisplayGeometry> {
        self.displays
            .iter()
            .copied()
            .find(|display| display.id == id)
    }

    pub(crate) fn display_for_or_primary(&self, point: GlobalPoint) -> Option<DisplayGeometry> {
        self.displays
            .iter()
            .copied()
            .find(|display| display.contains(point))
            .or_else(|| {
                self.displays
                    .iter()
                    .copied()
                    .find(|display| display.is_primary)
            })
            .or_else(|| self.displays.first().copied())
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
    fn routes_negative_axes_into_display_local_points() {
        let layout = layout();
        assert_eq!(
            layout.route(GlobalPoint {
                x: -867.0,
                y: 400.0
            }),
            Some((
                2,
                GlobalPoint {
                    x: 1053.0,
                    y: 580.0
                }
            ))
        );
        assert_eq!(
            layout.route(GlobalPoint {
                x: 300.0,
                y: -867.0
            }),
            Some((3, GlobalPoint { x: 300.0, y: 333.0 }))
        );
    }

    #[test]
    fn seams_are_half_open_and_owned_once() {
        let layout = layout();
        assert_eq!(
            layout
                .route(GlobalPoint {
                    x: -0.001,
                    y: 100.0
                })
                .unwrap()
                .0,
            2
        );
        assert_eq!(layout.route(GlobalPoint { x: 0.0, y: 100.0 }).unwrap().0, 1);
    }

    #[test]
    fn each_display_keeps_its_own_pixel_scale() {
        let layout = layout();
        assert_eq!(layout.display(1).unwrap().pixel_size(), (2880, 1800));
        assert_eq!(layout.display(2).unwrap().pixel_size(), (1920, 1080));
        assert_eq!(layout.display(3).unwrap().pixel_size(), (1800, 1800));
    }

    #[test]
    fn appkit_frame_flips_global_y_around_the_primary_display() {
        let display = layout().display(3).unwrap();
        let frame = display.appkit_frame(900.0);
        assert_eq!(frame.origin.x, 0.0);
        assert_eq!(frame.origin.y, 900.0);
        assert_eq!(frame.size.width, 1200.0);
        assert_eq!(frame.size.height, 1200.0);
    }
}
