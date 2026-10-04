//! Surface-local geometry for the three fixture regions.
//!
//! The fixture publishes this layout in its startup journal record so the
//! runner clicks exact published centers instead of guessing coordinates. Both
//! sides then agree on which region an action targeted, which is what makes
//! "this action was supposed to change nothing" a checkable claim rather than
//! an interpretation.

use serde::{Deserialize, Serialize};

use crate::sample::Region;

/// A surface-local rectangle in logical pixels.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Rect {
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= f64::from(self.x)
            && y >= f64::from(self.y)
            && x < f64::from(self.x + self.width)
            && y < f64::from(self.y + self.height)
    }

    /// Part of the published region map rather than of the paint path: the
    /// runner reads the layout out of the startup record and clicks region
    /// centres, so this is exercised by the tests and by JSON consumers.
    #[allow(dead_code)]
    pub fn center(&self) -> (i32, i32) {
        (self.x + self.width / 2, self.y + self.height / 2)
    }
}

/// The published region map for one surface size.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Layout {
    /// Mutates state and submits exactly one content update.
    pub active: Rect,
    /// Receives input and deliberately changes nothing.
    pub inert: Rect,
    /// Submits two content updates back to back.
    pub supersede: Rect,
    pub width: i32,
    pub height: i32,
}

impl Layout {
    pub fn for_size(width: i32, height: i32) -> Self {
        let scale = |value: i32, numerator: i32, denominator: i32| value * numerator / denominator;
        let active = Rect {
            x: scale(width, 1, 20),
            y: scale(height, 1, 10),
            width: scale(width, 2, 5),
            height: scale(height, 2, 5),
        };
        let inert = Rect {
            x: scale(width, 11, 20),
            y: scale(height, 1, 10),
            width: scale(width, 2, 5),
            height: scale(height, 2, 5),
        };
        let supersede = Rect {
            x: scale(width, 1, 20),
            y: scale(height, 3, 5),
            width: scale(width, 9, 10),
            height: scale(height, 1, 4),
        };
        Self {
            active,
            inert,
            supersede,
            width,
            height,
        }
    }

    /// Which region a surface-local point lands in, if any.
    pub fn region_at(&self, x: f64, y: f64) -> Option<Region> {
        if self.active.contains(x, y) {
            Some(Region::Active)
        } else if self.inert.contains(x, y) {
            Some(Region::Inert)
        } else if self.supersede.contains(x, y) {
            Some(Region::Supersede)
        } else {
            None
        }
    }

    /// Region lookup for callers holding a `Region`; see `center` on why this
    /// is not reached from the fixture's own paint path.
    #[allow(dead_code)]
    pub fn rect(&self, region: Region) -> Rect {
        match region {
            Region::Active => self.active,
            Region::Inert => self.inert,
            Region::Supersede => self.supersede,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both the fixture's own default and the size the canonical Sway lane
    /// forces on `CuaTestHarness` windows.
    const SIZES: [(i32, i32); 3] = [(800, 600), (940, 780), (640, 480)];

    fn overlaps(left: Rect, right: Rect) -> bool {
        left.x < right.x + right.width
            && right.x < left.x + left.width
            && left.y < right.y + right.height
            && right.y < left.y + left.height
    }

    #[test]
    fn regions_never_overlap_at_any_configured_size() {
        for (width, height) in SIZES {
            let layout = Layout::for_size(width, height);
            assert!(!overlaps(layout.active, layout.inert), "{width}x{height}");
            assert!(
                !overlaps(layout.active, layout.supersede),
                "{width}x{height}"
            );
            assert!(
                !overlaps(layout.inert, layout.supersede),
                "{width}x{height}"
            );
        }
    }

    #[test]
    fn every_published_center_resolves_to_its_own_region() {
        for (width, height) in SIZES {
            let layout = Layout::for_size(width, height);
            for region in [Region::Active, Region::Inert, Region::Supersede] {
                let (x, y) = layout.rect(region).center();
                assert_eq!(
                    layout.region_at(f64::from(x), f64::from(y)),
                    Some(region),
                    "{region:?} center at {width}x{height}"
                );
            }
        }
    }

    #[test]
    fn regions_stay_inside_the_surface() {
        for (width, height) in SIZES {
            let layout = Layout::for_size(width, height);
            for region in [Region::Active, Region::Inert, Region::Supersede] {
                let rect = layout.rect(region);
                assert!(rect.x >= 0 && rect.y >= 0, "{region:?}");
                assert!(rect.x + rect.width <= width, "{region:?} at {width}");
                assert!(rect.y + rect.height <= height, "{region:?} at {height}");
                assert!(rect.width > 0 && rect.height > 0, "{region:?}");
            }
        }
    }

    #[test]
    fn points_outside_every_region_belong_to_no_region() {
        let layout = Layout::for_size(800, 600);
        assert_eq!(layout.region_at(0.0, 0.0), None);
        assert_eq!(layout.region_at(799.0, 599.0), None);
        // The gap between the active and inert rectangles is deliberate: an
        // off-by-a-few-pixels click must not silently become a mutation.
        let gap_x = f64::from(layout.active.x + layout.active.width) + 1.0;
        let gap_y = f64::from(layout.active.y) + 1.0;
        assert_eq!(layout.region_at(gap_x, gap_y), None);
    }

    #[test]
    fn rectangles_are_half_open_so_edges_belong_to_one_region_only() {
        let layout = Layout::for_size(800, 600);
        let rect = layout.active;
        assert!(rect.contains(f64::from(rect.x), f64::from(rect.y)));
        assert!(!rect.contains(f64::from(rect.x + rect.width), f64::from(rect.y)));
        assert!(!rect.contains(f64::from(rect.x), f64::from(rect.y + rect.height)));
    }
}
