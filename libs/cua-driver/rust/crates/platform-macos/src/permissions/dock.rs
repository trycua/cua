// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Where the permission guide docks under System Settings, as plain
//! geometry so it can be tested without a screen.
//!
//! The rule is Cua Spaces' `PanelPlacement`
//! (`apps/cua-spaces-macos/Vendor/PermissionFlow`): the guide sits under the
//! System Settings window's content area, lined up with the pane to the
//! right of the sidebar, and is kept inside the part of the screen the Dock
//! and the menu bar leave free. With no room under the window it overlaps
//! the window's bottom instead of leaving the screen; the bottom wins, so
//! the drag tile stays reachable.
//!
//! All rectangles here are in AppKit screen coordinates: origin at the
//! bottom left of the primary display, y up.

/// A rectangle in AppKit screen coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn max_x(&self) -> f64 {
        self.x + self.width
    }

    pub fn max_y(&self) -> f64 {
        self.y + self.height
    }

    fn overlap_area(&self, other: &Rect) -> f64 {
        let w = self.max_x().min(other.max_x()) - self.x.max(other.x);
        let h = self.max_y().min(other.max_y()) - self.y.max(other.y);
        if w > 0.0 && h > 0.0 {
            w * h
        } else {
            0.0
        }
    }
}

/// One display: its whole frame and the part the Dock and the menu bar
/// leave free (`NSScreen.visibleFrame`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Screen {
    pub frame: Rect,
    pub visible: Rect,
}

/// System Settings' leading sidebar: the guide lines up with the pane to
/// its right, where the switches are.
pub const SIDEBAR_WIDTH: f64 = 230.0;
/// The gap kept from the edges of the usable area.
pub const SCREEN_INSET: f64 = 12.0;

/// A window-server rectangle (`kCGWindowBounds`: origin at the top left of
/// the primary display, y down) in AppKit coordinates. `primary_height` is
/// the height of the primary display's frame.
pub fn from_window_server(bounds: Rect, primary_height: f64) -> Rect {
    Rect::new(
        bounds.x,
        primary_height - bounds.y - bounds.height,
        bounds.width,
        bounds.height,
    )
}

/// The screen System Settings is mostly on.
pub fn screen_for(settings: Rect, screens: &[Screen]) -> Option<Screen> {
    screens
        .iter()
        .copied()
        .map(|screen| (screen.frame.overlap_area(&settings), screen))
        .filter(|(area, _)| *area > 0.0)
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, screen)| screen)
}

/// The guide's frame, `width` by `height`, docked under `settings` and kept
/// `SCREEN_INSET` inside `usable`. The guide is moved, never resized.
pub fn docked_frame(settings: Rect, usable: Rect, width: f64, height: f64) -> Rect {
    let min_x = usable.x + SCREEN_INSET;
    let max_x = (usable.max_x() - width - SCREEN_INSET).max(min_x);
    let min_y = usable.y + SCREEN_INSET;
    let max_y = (usable.max_y() - height - SCREEN_INSET).max(min_y);
    let x = (settings.x + SIDEBAR_WIDTH).clamp(min_x, max_x);
    // The bottom wins over the top: the drag tile is at the guide's bottom.
    let y = (settings.y - height).clamp(min_y, max_y);
    Rect::new(x, y, width, height)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOCK_W: f64 = 480.0;
    const DOCK_H: f64 = 160.0;

    /// A 1920 x 1080 display with the Dock at the bottom (70 points) and
    /// the menu bar at the top (30 points).
    fn full_hd() -> Screen {
        Screen {
            frame: Rect::new(0.0, 0.0, 1920.0, 1080.0),
            visible: Rect::new(0.0, 70.0, 1920.0, 980.0),
        }
    }

    /// System Settings where macOS 26 opens it on `full_hd`, measured on a
    /// VM: window-server bounds x 598, y 197, 723 x 626.
    fn settings_on_full_hd() -> Rect {
        from_window_server(Rect::new(598.0, 197.0, 723.0, 626.0), 1080.0)
    }

    #[test]
    fn window_server_bounds_turn_into_appkit_coordinates() {
        let r = settings_on_full_hd();
        assert_eq!(r, Rect::new(598.0, 257.0, 723.0, 626.0));
        // A display above the primary one has negative window-server y and
        // AppKit y above the primary height.
        let above = from_window_server(Rect::new(0.0, -900.0, 400.0, 300.0), 1080.0);
        assert_eq!(above, Rect::new(0.0, 1680.0, 400.0, 300.0));
    }

    #[test]
    fn the_guide_docks_under_the_settings_pane_when_there_is_room() {
        let settings = settings_on_full_hd();
        let frame = docked_frame(settings, full_hd().visible, DOCK_W, DOCK_H);
        assert_eq!(frame.x, settings.x + SIDEBAR_WIDTH);
        // Directly under the window, inside the area above the Dock.
        assert_eq!(frame.max_y(), settings.y);
        assert!(frame.y >= full_hd().visible.y + SCREEN_INSET);
    }

    #[test]
    fn with_no_room_below_the_guide_overlaps_the_settings_bottom_above_the_dock() {
        // 1280 x 800: System Settings opens centred and leaves less room
        // under it than the guide needs.
        let screen = Screen {
            frame: Rect::new(0.0, 0.0, 1280.0, 800.0),
            visible: Rect::new(0.0, 70.0, 1280.0, 705.0),
        };
        let settings = from_window_server(Rect::new(278.0, 110.0, 723.0, 600.0), 800.0);
        let frame = docked_frame(settings, screen.visible, DOCK_W, DOCK_H);
        assert_eq!(frame.y, screen.visible.y + SCREEN_INSET);
        assert!(frame.y < settings.y + settings.height);
        assert!(frame.max_x() <= screen.visible.max_x() - SCREEN_INSET);
        // Still lined up with the pane, clear of the sidebar.
        assert_eq!(frame.x, settings.x + SIDEBAR_WIDTH);
    }

    #[test]
    fn the_guide_stays_on_screen_when_settings_is_at_an_edge() {
        let visible = full_hd().visible;
        let right = Rect::new(1700.0, 400.0, 723.0, 600.0);
        let frame = docked_frame(right, visible, DOCK_W, DOCK_H);
        assert_eq!(frame.max_x(), visible.max_x() - SCREEN_INSET);
        let left = Rect::new(-500.0, 400.0, 723.0, 600.0);
        let frame = docked_frame(left, visible, DOCK_W, DOCK_H);
        assert_eq!(frame.x, visible.x + SCREEN_INSET);
    }

    #[test]
    fn the_guide_never_leaves_the_top_of_the_usable_area() {
        // A window dragged mostly off the top of the screen: the guide is
        // not pushed above the visible frame.
        let visible = full_hd().visible;
        let settings = Rect::new(600.0, 1060.0, 723.0, 600.0);
        let frame = docked_frame(settings, visible, DOCK_W, DOCK_H);
        assert_eq!(frame.max_y(), visible.max_y() - SCREEN_INSET);
    }

    #[test]
    fn the_screen_holding_most_of_settings_wins() {
        let left = Screen {
            frame: Rect::new(-1440.0, 0.0, 1440.0, 900.0),
            visible: Rect::new(-1440.0, 0.0, 1440.0, 875.0),
        };
        let main = full_hd();
        let mostly_main = Rect::new(-200.0, 300.0, 723.0, 600.0);
        assert_eq!(screen_for(mostly_main, &[left, main]), Some(main));
        let mostly_left = Rect::new(-600.0, 300.0, 723.0, 600.0);
        assert_eq!(screen_for(mostly_left, &[left, main]), Some(left));
        let nowhere = Rect::new(5000.0, 5000.0, 723.0, 600.0);
        assert_eq!(screen_for(nowhere, &[left, main]), None);
    }
}
