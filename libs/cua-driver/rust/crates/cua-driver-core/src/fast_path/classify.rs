// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Per-target choice of guard: rich tree, sparse tree, or a remote surface.

use super::Rect;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetClass {
    /// The tree describes the target: AX guard.
    Rich,
    /// Little or no tree at the point: window guards plus the pixel check.
    Sparse,
    /// A whole-window image with no children (remote desktop, VNC): sparse,
    /// with the wider remote tolerance.
    Remote,
}

impl TargetClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rich => "rich",
            Self::Sparse => "sparse",
            Self::Remote => "remote",
        }
    }
    pub fn needs_pixel_check(self) -> bool {
        self != Self::Rich
    }
}

/// The element a hit-test returned at the point.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HitInfo {
    pub role: Option<String>,
    pub has_label: bool,
    pub has_value: bool,
    pub has_identifier: bool,
    pub frame: Option<Rect>,
    /// The hit-test returned the window element itself.
    pub is_window: bool,
    pub child_count: Option<usize>,
}

/// Cheap stats from the last walk of the window.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TreeStats {
    pub elements: usize,
    /// Elements whose frame is 1 pt tall or less.
    pub thin_frames: usize,
}

const GENERIC_ROLES: &[&str] = &[
    "AXGroup",
    "AXUnknown",
    "AXImage",
    "AXScrollArea",
    "AXLayoutArea",
    "AXSplitGroup",
    // UIA / AT-SPI equivalents
    "Pane",
    "Custom",
    "Image",
    "Group",
    "panel",
    "filler",
    "canvas",
    "drawing area",
    "image",
    "unknown",
];

pub fn is_generic_role(role: &str) -> bool {
    GENERIC_ROLES.iter().any(|g| g.eq_ignore_ascii_case(role))
}

pub fn classify(hit: Option<&HitInfo>, stats: Option<&TreeStats>, window: Rect) -> TargetClass {
    let Some(hit) = hit else {
        return TargetClass::Sparse;
    };
    let covers = |fraction: f64| {
        hit.frame
            .map(|f| window.area() > 0.0 && f.area() / window.area() > fraction)
            .unwrap_or(false)
    };
    let role = hit.role.as_deref().unwrap_or("");
    let image_like = matches!(
        role,
        "AXImage" | "Image" | "image" | "canvas" | "AXGroup" | "Pane" | "Custom"
    );
    if image_like && hit.child_count == Some(0) && covers(0.6) && !hit.has_label {
        return TargetClass::Remote;
    }
    if let Some(stats) = stats {
        if stats.elements < 10 || (stats.elements > 0 && stats.thin_frames * 2 > stats.elements) {
            return TargetClass::Sparse;
        }
    }
    if hit.is_window || role.is_empty() {
        return TargetClass::Sparse;
    }
    let anonymous = !hit.has_label && !hit.has_value && !hit.has_identifier;
    if is_generic_role(role) && anonymous {
        return TargetClass::Sparse;
    }
    if covers(0.25) && is_generic_role(role) {
        return TargetClass::Sparse;
    }
    TargetClass::Rich
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIN: Rect = Rect {
        x: 0.0,
        y: 0.0,
        w: 1000.0,
        h: 800.0,
    };
    const RICH: TreeStats = TreeStats {
        elements: 120,
        thin_frames: 3,
    };

    fn hit(role: &str) -> HitInfo {
        HitInfo {
            role: Some(role.into()),
            has_label: true,
            frame: Some(Rect::new(10.0, 10.0, 80.0, 24.0)),
            ..Default::default()
        }
    }

    #[test]
    fn labelled_button_is_rich() {
        assert_eq!(
            classify(Some(&hit("AXButton")), Some(&RICH), WIN),
            TargetClass::Rich
        );
    }

    #[test]
    fn no_hit_or_window_hit_is_sparse() {
        assert_eq!(classify(None, Some(&RICH), WIN), TargetClass::Sparse);
        let mut h = hit("AXWindow");
        h.is_window = true;
        assert_eq!(classify(Some(&h), Some(&RICH), WIN), TargetClass::Sparse);
    }

    #[test]
    fn anonymous_group_and_big_container_are_sparse() {
        let mut h = hit("AXGroup");
        h.has_label = false;
        assert_eq!(classify(Some(&h), Some(&RICH), WIN), TargetClass::Sparse);
        let mut h = hit("AXScrollArea");
        h.frame = Some(Rect::new(0.0, 0.0, 900.0, 700.0));
        assert_eq!(classify(Some(&h), Some(&RICH), WIN), TargetClass::Sparse);
    }

    #[test]
    fn tiny_or_degraded_trees_are_sparse_everywhere() {
        let small = TreeStats {
            elements: 6,
            thin_frames: 0,
        };
        assert_eq!(
            classify(Some(&hit("AXButton")), Some(&small), WIN),
            TargetClass::Sparse
        );
        let thin = TreeStats {
            elements: 50,
            thin_frames: 40,
        };
        assert_eq!(
            classify(Some(&hit("AXButton")), Some(&thin), WIN),
            TargetClass::Sparse
        );
    }

    #[test]
    fn whole_window_image_without_children_is_remote() {
        let h = HitInfo {
            role: Some("AXImage".into()),
            frame: Some(Rect::new(0.0, 0.0, 1000.0, 780.0)),
            child_count: Some(0),
            ..Default::default()
        };
        assert_eq!(
            classify(
                Some(&h),
                Some(&TreeStats {
                    elements: 3,
                    thin_frames: 0
                }),
                WIN
            ),
            TargetClass::Remote
        );
        assert!(TargetClass::Remote.needs_pixel_check());
    }
}
