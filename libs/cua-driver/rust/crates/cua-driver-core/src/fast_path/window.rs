// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Window guards: run for every pixel click, rich or sparse, with no tree.

use super::reason::{Reason, Verdict};
use super::Rect;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowFingerprint {
    pub window_id: u64,
    pub pid: i32,
    /// Screen frame in points.
    pub frame: Rect,
    /// Backing scale (pixels per point).
    pub scale: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowObservation {
    pub exists: bool,
    pub pid: i32,
    pub frame: Rect,
    pub scale: f64,
    /// On screen and capturable (not minimized, not on another Space).
    pub visible: bool,
    /// Owner pid of the window at the click point, if the platform can say.
    pub pid_at_point: Option<i32>,
    /// Another window covers the point in the front-to-back list.
    pub covered_at_point: Option<bool>,
}

/// How the pointer event reaches the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerDelivery {
    /// Posted to the window or pid (background): occlusion does not matter
    /// and window-relative coordinates survive a window move.
    Background,
    /// Global screen coordinates (foreground, or remote desktops that read
    /// the real pointer): the window must not move or be covered.
    Screen,
}

pub const FRAME_TOLERANCE: f64 = 2.0;

pub fn verify_window(
    fp: &WindowFingerprint,
    now: &WindowObservation,
    delivery: PointerDelivery,
) -> Verdict {
    if !now.exists {
        return Verdict::refuse(Reason::WindowChanged, "the window is gone");
    }
    if now.pid != fp.pid {
        return Verdict::refuse(
            Reason::WindowChanged,
            "the window belongs to another process now",
        );
    }
    if !now.visible {
        return Verdict::refuse(
            Reason::NotVisible,
            "the window is minimized or not on screen",
        );
    }
    if (now.scale - fp.scale).abs() > 0.01 {
        return Verdict::refuse(
            Reason::WindowChanged,
            "the window moved to a display with another scale",
        );
    }
    if (now.frame.w - fp.frame.w).abs() > FRAME_TOLERANCE
        || (now.frame.h - fp.frame.h).abs() > FRAME_TOLERANCE
    {
        return Verdict::refuse(
            Reason::WindowChanged,
            format!(
                "the window was {:.0}x{:.0} and is now {:.0}x{:.0}",
                fp.frame.w, fp.frame.h, now.frame.w, now.frame.h
            ),
        );
    }
    let moved = (now.frame.x - fp.frame.x).abs() > FRAME_TOLERANCE
        || (now.frame.y - fp.frame.y).abs() > FRAME_TOLERANCE;
    if delivery == PointerDelivery::Screen {
        if moved {
            return Verdict::refuse(Reason::WindowChanged, "the window moved");
        }
        if let Some(pid) = now.pid_at_point {
            if pid != fp.pid {
                return Verdict::refuse(Reason::Occluded, "another app's window is at the point");
            }
        }
        if now.covered_at_point == Some(true) {
            return Verdict::refuse(Reason::Occluded, "another window covers the point");
        }
    }
    Verdict::Ok
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp() -> WindowFingerprint {
        WindowFingerprint {
            window_id: 7,
            pid: 42,
            frame: Rect::new(0.0, 25.0, 800.0, 600.0),
            scale: 2.0,
        }
    }
    fn obs() -> WindowObservation {
        WindowObservation {
            exists: true,
            pid: 42,
            frame: Rect::new(0.0, 25.0, 800.0, 600.0),
            scale: 2.0,
            visible: true,
            pid_at_point: Some(42),
            covered_at_point: Some(false),
        }
    }

    #[test]
    fn same_window_passes() {
        assert!(verify_window(&fp(), &obs(), PointerDelivery::Screen).is_ok());
    }

    #[test]
    fn each_window_reason() {
        let mut o = obs();
        o.exists = false;
        assert_eq!(
            verify_window(&fp(), &o, PointerDelivery::Background).reason(),
            Some(Reason::WindowChanged)
        );
        let mut o = obs();
        o.pid = 1;
        assert_eq!(
            verify_window(&fp(), &o, PointerDelivery::Background).reason(),
            Some(Reason::WindowChanged)
        );
        let mut o = obs();
        o.visible = false;
        assert_eq!(
            verify_window(&fp(), &o, PointerDelivery::Background).reason(),
            Some(Reason::NotVisible)
        );
        let mut o = obs();
        o.scale = 1.0;
        assert_eq!(
            verify_window(&fp(), &o, PointerDelivery::Background).reason(),
            Some(Reason::WindowChanged)
        );
        let mut o = obs();
        o.frame.w = 1024.0;
        assert_eq!(
            verify_window(&fp(), &o, PointerDelivery::Background).reason(),
            Some(Reason::WindowChanged)
        );
    }

    #[test]
    fn occlusion_matters_only_for_screen_delivery() {
        let mut o = obs();
        o.pid_at_point = Some(99);
        assert!(verify_window(&fp(), &o, PointerDelivery::Background).is_ok());
        assert_eq!(
            verify_window(&fp(), &o, PointerDelivery::Screen).reason(),
            Some(Reason::Occluded)
        );
        let mut o = obs();
        o.covered_at_point = Some(true);
        assert_eq!(
            verify_window(&fp(), &o, PointerDelivery::Screen).reason(),
            Some(Reason::Occluded)
        );
    }

    #[test]
    fn a_window_move_breaks_only_screen_coordinates() {
        let mut o = obs();
        o.frame.x += 50.0;
        assert!(verify_window(&fp(), &o, PointerDelivery::Background).is_ok());
        assert_eq!(
            verify_window(&fp(), &o, PointerDelivery::Screen).reason(),
            Some(Reason::WindowChanged)
        );
        o.frame.x = fp().frame.x + 1.5; // within tolerance
        assert!(verify_window(&fp(), &o, PointerDelivery::Screen).is_ok());
    }
}
