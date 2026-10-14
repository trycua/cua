// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The AI agents page's "background computer-use" card: an animated
//! miniature drawn natively by each shell (SwiftUI shapes in the Swift app,
//! HTML and CSS in the Tauri app) from the scene here and animated by the
//! frames here, so both shells play the same beats (the same way as the
//! presentation cards, [`crate::onboarding_preview`]).
//!
//! Two windows on a desktop, both busy at once:
//!
//! - behind, a window where the cua-driver agent cursor (the driver's
//!   default theme: the Cua blue arrow with a white edge and a soft glow)
//!   glides to three checkboxes and clicks each (the theme's click rays),
//!   ticking them;
//! - in front, a document where the user's own pointer drags a selection
//!   across a line, holds, clicks it away and drifts back.
//!
//! The agent cursor is drawn above the back window and below the front one
//! (it works in the background window), the user's pointer above everything.
//! One loop is [`DRIVER_LOOP_MS`] and seamless. With Reduce Motion the
//! shells draw [`still`]. Coordinates are the stage's points, origin top
//! left, y down; the shells centre the stage in the card and draw the
//! desktop across the card's full width.

use crate::notch::geometry::LogicalRect;
use crate::onboarding_preview::PreviewPoint;
use serde::{Deserialize, Serialize};

/// One loop (ms).
pub const DRIVER_LOOP_MS: u32 = 4800;
/// The time [`still`] shows: the agent ticking the last box while the
/// user's selection is held.
pub const DRIVER_STILL_MS: u32 = 2600;
/// The stage, in points.
pub const DRIVER_STAGE_WIDTH: f64 = 232.0;
/// The stage's height.
pub const DRIVER_STAGE_HEIGHT: f64 = 112.0;
/// The driver's default cursor fill (cua-driver's `DEFAULT_CURSOR_FILL`).
pub const AGENT_FILL: &str = "#5EC0E8";

/// Checkbox rows in the back window.
const ROWS: usize = 3;
/// A window's title bar.
const TITLE_BAR: f64 = 11.0;
/// Window corner radius.
const RADIUS: f64 = 5.0;
/// The driver theme's canvas units to stage points.
const AGENT_SCALE: f64 = 0.2;
/// The click rays are drawn larger than the arrow's scale so they read.
const RAY_SCALE: f64 = 0.34;
/// The theme arrow's tip on its 128-unit canvas (the midpoint of its
/// rounded tip curve).
const AGENT_TIP: (f64, f64) = (46.0, 31.75);
/// A path vertex with its in and out tangents.
type PathVertex = ((f64, f64), (f64, f64), (f64, f64));
/// cua-driver's default theme arrow (`build_default_theme.py`'s
/// `CURSOR_PATH`): each vertex with its in and out tangents.
const AGENT_PATH: [PathVertex; 8] = [
    ((55.0, 30.0), (0.0, 0.0), (-7.0, -2.0)),
    ((43.0, 41.0), (-1.0, -8.0), (0.0, 0.0)),
    ((64.0, 98.0), (0.0, 0.0), (3.0, 8.0)),
    ((77.0, 99.0), (-4.0, 7.0), (0.0, 0.0)),
    ((86.0, 79.0), (0.0, 0.0), (2.0, -4.0)),
    ((95.0, 70.0), (-4.0, 2.0), (0.0, 0.0)),
    ((108.0, 63.0), (0.0, 0.0), (7.0, -4.0)),
    ((107.0, 50.0), (7.0, 3.0), (0.0, 0.0)),
];
/// The theme's click cue: three short rays off the tip (canvas units).
const AGENT_RAYS: [((f64, f64), (f64, f64)); 3] = [
    ((35.0, 20.0), (34.0, 11.0)),
    ((27.0, 25.0), (19.0, 19.0)),
    ((25.0, 34.0), (15.0, 34.0)),
];
/// The standard arrow pointer (the presentation cards' outline).
const ARROW: [(f64, f64); 7] = [
    (0.0, 0.0),
    (0.0, 15.0),
    (3.6, 11.6),
    (6.2, 17.2),
    (8.6, 16.2),
    (6.1, 10.6),
    (10.8, 10.6),
];
const POINTER_SCALE: f64 = 0.8;

/// A line segment on the stage.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PreviewSegment {
    /// Start.
    pub from: PreviewPoint,
    /// End.
    pub to: PreviewPoint,
}

/// One window of the miniature.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewWindow {
    /// Its frame on the stage.
    pub frame: LogicalRect,
    /// The title bar's height (the three window buttons sit in it).
    pub title_bar: f64,
    /// Corner radius.
    pub radius: f64,
}

/// The card's miniature.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriverPreview {
    /// Stage width.
    pub width: f64,
    /// Stage height.
    pub height: f64,
    /// One loop (ms).
    pub loop_ms: u32,
    /// The window the agent works in (behind).
    pub back: PreviewWindow,
    /// The window the user works in (in front).
    pub front: PreviewWindow,
    /// The back window's checkboxes, top to bottom.
    pub checkboxes: Vec<LogicalRect>,
    /// The label bar right of each checkbox.
    pub labels: Vec<LogicalRect>,
    /// The front window's text lines (placeholder bars).
    pub lines: Vec<LogicalRect>,
    /// The line the user selects (an index into `lines`).
    pub selected_line: u32,
    /// The user's pointer outline, tip at the origin (fill black, stroke
    /// white).
    pub pointer: Vec<PreviewPoint>,
    /// The agent cursor's outline, tip at the origin (fill `agent_fill`,
    /// stroke white, a soft glow of `agent_fill`).
    pub agent_pointer: Vec<PreviewPoint>,
    /// The agent cursor's fill.
    pub agent_fill: String,
    /// The click rays, relative to the agent cursor's tip.
    pub agent_rays: Vec<PreviewSegment>,
}

/// One moment of the miniature.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriverFrame {
    /// The user's pointer tip.
    pub pointer: PreviewPoint,
    /// The user presses (drawn a little smaller).
    pub pressed: bool,
    /// How much of the selected line is highlighted, 0 to 1 from its left.
    pub selection: f64,
    /// The agent cursor's tip.
    pub agent: PreviewPoint,
    /// The agent presses (drawn a little smaller).
    pub agent_pressed: bool,
    /// The click rays, 0 hidden, else 0 to 1 through their burst (they
    /// grow out and fade).
    pub ripple: f64,
    /// Each checkbox's tick, 0 to 1.
    pub checked: Vec<f64>,
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> LogicalRect {
    LogicalRect::new(x, y, w, h)
}

fn pt(x: f64, y: f64) -> PreviewPoint {
    PreviewPoint { x, y }
}

fn back_window() -> PreviewWindow {
    PreviewWindow {
        frame: rect(14.0, 10.0, 132.0, 80.0),
        title_bar: TITLE_BAR,
        radius: RADIUS,
    }
}

fn front_window() -> PreviewWindow {
    PreviewWindow {
        frame: rect(90.0, 34.0, 128.0, 70.0),
        title_bar: TITLE_BAR,
        radius: RADIUS,
    }
}

fn checkboxes() -> Vec<LogicalRect> {
    let b = back_window().frame;
    (0..ROWS)
        .map(|i| {
            rect(
                b.x + 10.0,
                b.y + TITLE_BAR + 9.0 + i as f64 * 17.0,
                8.0,
                8.0,
            )
        })
        .collect()
}

fn labels() -> Vec<LogicalRect> {
    let widths = [44.0, 34.0, 40.0];
    checkboxes()
        .iter()
        .zip(widths)
        .map(|(c, w)| rect(c.x + c.width + 7.0, c.y + 2.5, w, 3.0))
        .collect()
}

const SELECTED_LINE: usize = 1;

fn lines() -> Vec<LogicalRect> {
    let f = front_window().frame;
    [100.0, 88.0, 104.0, 62.0]
        .iter()
        .enumerate()
        .map(|(i, &w)| rect(f.x + 10.0, f.y + TITLE_BAR + 9.0 + i as f64 * 11.0, w, 3.0))
        .collect()
}

/// A cubic Bézier from `a` (out tangent `ao`) to `b` (in tangent `bi`),
/// sampled at `n` steps after `a`.
fn cubic(
    a: (f64, f64),
    ao: (f64, f64),
    b: (f64, f64),
    bi: (f64, f64),
    n: usize,
) -> Vec<(f64, f64)> {
    let c1 = (a.0 + ao.0, a.1 + ao.1);
    let c2 = (b.0 + bi.0, b.1 + bi.1);
    (1..=n)
        .map(|i| {
            let t = i as f64 / n as f64;
            let u = 1.0 - t;
            let w = [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t];
            (
                w[0] * a.0 + w[1] * c1.0 + w[2] * c2.0 + w[3] * b.0,
                w[0] * a.1 + w[1] * c1.1 + w[2] * c2.1 + w[3] * b.1,
            )
        })
        .collect()
}

/// The theme arrow as a polygon (curves sampled), tip at the origin.
fn agent_outline() -> Vec<PreviewPoint> {
    let mut out = Vec::new();
    let n = AGENT_PATH.len();
    for i in 0..n {
        let (a, _, ao) = AGENT_PATH[i];
        let (b, bi, _) = AGENT_PATH[(i + 1) % n];
        let curved = ao != (0.0, 0.0) || bi != (0.0, 0.0);
        let pts = if curved {
            cubic(a, ao, b, bi, 6)
        } else {
            vec![b]
        };
        out.extend(pts);
    }
    out.into_iter()
        .map(|(x, y)| {
            pt(
                (x - AGENT_TIP.0) * AGENT_SCALE,
                (y - AGENT_TIP.1) * AGENT_SCALE,
            )
        })
        .collect()
}

fn agent_rays() -> Vec<PreviewSegment> {
    let s = |(x, y): (f64, f64)| pt((x - AGENT_TIP.0) * RAY_SCALE, (y - AGENT_TIP.1) * RAY_SCALE);
    AGENT_RAYS
        .iter()
        .map(|&(a, b)| PreviewSegment {
            from: s(a),
            to: s(b),
        })
        .collect()
}

/// The card's miniature.
pub fn preview() -> DriverPreview {
    DriverPreview {
        width: DRIVER_STAGE_WIDTH,
        height: DRIVER_STAGE_HEIGHT,
        loop_ms: DRIVER_LOOP_MS,
        back: back_window(),
        front: front_window(),
        checkboxes: checkboxes(),
        labels: labels(),
        lines: lines(),
        selected_line: SELECTED_LINE as u32,
        pointer: ARROW
            .iter()
            .map(|&(x, y)| pt(x * POINTER_SCALE, y * POINTER_SCALE))
            .collect(),
        agent_pointer: agent_outline(),
        agent_fill: AGENT_FILL.into(),
        agent_rays: agent_rays(),
    }
}

// ---- Timing ------------------------------------------------------------------

fn span(t: f64, from: f64, to: f64) -> f64 {
    ((t - from) / (to - from)).clamp(0.0, 1.0)
}

fn ease(p: f64) -> f64 {
    if p < 0.5 {
        4.0 * p * p * p
    } else {
        1.0 - (-2.0 * p + 2.0).powi(3) / 2.0
    }
}

fn lerp(a: PreviewPoint, b: PreviewPoint, p: f64) -> PreviewPoint {
    pt(a.x + (b.x - a.x) * p, a.y + (b.y - a.y) * p)
}

/// Where the pointer is along `legs` (each `(from_ms, to_ms, target)`),
/// starting at `start`: resting between legs, eased along each.
fn path(t: f64, start: PreviewPoint, legs: &[(f64, f64, PreviewPoint)]) -> PreviewPoint {
    let mut at = start;
    for &(from, to, target) in legs {
        if t < from {
            return at;
        }
        if t < to {
            return lerp(at, target, ease(span(t, from, to)));
        }
        at = target;
    }
    at
}

// The agent's beats (ms): a glide to each box, a press, the tick.
const A_GLIDES: [(f64, f64); ROWS] = [(300.0, 900.0), (1300.0, 1700.0), (2100.0, 2500.0)];
const A_PRESS_MS: f64 = 150.0;
const A_TICK: (f64, f64) = (100.0, 250.0);
const A_RIPPLE_MS: f64 = 400.0;
const A_HOME: (f64, f64) = (3100.0, 3800.0);
const A_CLEAR: (f64, f64) = (4200.0, 4500.0);
// The user's beats (ms).
const U_TO_LINE: (f64, f64) = (200.0, 900.0);
const U_DRAG: (f64, f64) = (1000.0, 2000.0);
const U_CLICK: (f64, f64) = (3100.0, 3250.0);
const U_HOME: (f64, f64) = (3500.0, 4300.0);

fn agent_home() -> PreviewPoint {
    pt(66.0, 80.0)
}

fn box_centre(i: usize) -> PreviewPoint {
    let c = checkboxes()[i];
    pt(c.x + c.width / 2.0, c.y + c.height / 2.0)
}

fn user_home() -> PreviewPoint {
    pt(196.0, 88.0)
}

fn selection_span() -> (PreviewPoint, PreviewPoint) {
    let l = lines()[SELECTED_LINE];
    let y = l.y + l.height / 2.0;
    (pt(l.x, y), pt(l.x + l.width * 0.86, y))
}

fn agent_at(t: f64) -> PreviewPoint {
    let mut legs: Vec<(f64, f64, PreviewPoint)> = A_GLIDES
        .iter()
        .enumerate()
        .map(|(i, &(a, b))| (a, b, box_centre(i)))
        .collect();
    legs.push((A_HOME.0, A_HOME.1, agent_home()));
    path(t, agent_home(), &legs)
}

/// The miniature at `t_ms` into its loop (any `t_ms`: it wraps).
pub fn frame(t_ms: u32) -> DriverFrame {
    let t = (t_ms % DRIVER_LOOP_MS) as f64;
    let presses: Vec<f64> = A_GLIDES.iter().map(|&(_, arrive)| arrive).collect();
    let agent_pressed = presses.iter().any(|&p| t >= p && t < p + A_PRESS_MS);
    let ripple = presses
        .iter()
        .find(|&&p| t >= p && t < p + A_RIPPLE_MS)
        .map(|&p| (t - p) / A_RIPPLE_MS)
        .unwrap_or(0.0);
    let clear = 1.0 - span(t, A_CLEAR.0, A_CLEAR.1);
    let checked = presses
        .iter()
        .map(|&p| span(t, p + A_TICK.0, p + A_TICK.1) * clear)
        .collect();

    let (sel_from, sel_to) = selection_span();
    let drag = ease(span(t, U_DRAG.0, U_DRAG.1));
    let pointer = path(
        t,
        user_home(),
        &[
            (U_TO_LINE.0, U_TO_LINE.1, sel_from),
            (U_DRAG.0, U_DRAG.1, sel_to),
            (U_HOME.0, U_HOME.1, user_home()),
        ],
    );
    let pressed = (U_DRAG.0..U_DRAG.1).contains(&t) || (U_CLICK.0..U_CLICK.1).contains(&t);
    let selection = if t < U_CLICK.0 { drag * 0.86 } else { 0.0 };
    DriverFrame {
        pointer,
        pressed,
        selection: (selection * 1000.0).round() / 1000.0,
        agent: agent_at(t),
        agent_pressed,
        ripple,
        checked,
    }
}

/// The Reduce Motion picture: the agent at the last box with its click
/// rays, the first two ticked, the user's selection held. No loop.
pub fn still() -> DriverFrame {
    DriverFrame {
        pressed: false,
        agent_pressed: false,
        ripple: 0.5,
        ..frame(DRIVER_STILL_MS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inside(p: PreviewPoint, r: LogicalRect) -> bool {
        p.x >= r.x && p.x <= r.x + r.width && p.y >= r.y && p.y <= r.y + r.height
    }

    fn bounds(points: &[PreviewPoint]) -> (f64, f64, f64, f64) {
        points.iter().fold(
            (f64::MAX, f64::MAX, f64::MIN, f64::MIN),
            |(a, b, c, d), p| (a.min(p.x), b.min(p.y), c.max(p.x), d.max(p.y)),
        )
    }

    #[test]
    fn the_scene_fits_the_stage_and_the_windows_overlap() {
        let p = preview();
        for r in [p.back.frame, p.front.frame]
            .into_iter()
            .chain(p.checkboxes.iter().copied())
            .chain(p.labels.iter().copied())
            .chain(p.lines.iter().copied())
        {
            assert!(r.x >= 0.0 && r.y >= 0.0, "{r:?}");
            assert!(
                r.x + r.width <= p.width && r.y + r.height <= p.height,
                "{r:?}"
            );
        }
        // The front window covers part of the back one.
        assert!(p.front.frame.x < p.back.frame.x + p.back.frame.width);
        assert!(p.front.frame.y < p.back.frame.y + p.back.frame.height);
        // Checkboxes and labels are in the back window's visible part.
        for r in p.checkboxes.iter().chain(&p.labels) {
            assert!(
                r.x + r.width < p.front.frame.x || r.y + r.height < p.front.frame.y,
                "{r:?}"
            );
        }
        assert_eq!(p.checkboxes.len(), ROWS);
        assert_eq!(p.agent_fill, "#5EC0E8");
    }

    #[test]
    fn the_agent_cursor_is_the_driver_theme_arrow() {
        let p = preview();
        // Sampled curves: the four rounded corners give many vertices.
        assert!(p.agent_pointer.len() > 20);
        let (x0, y0, x1, y1) = bounds(&p.agent_pointer);
        // The tip is the top-left extreme, near the origin.
        assert!(x0 > -1.0 && y0 > -1.0, "{x0},{y0}");
        // It reads at the size of the user's pointer.
        assert!((10.0..16.0).contains(&(y1 - y0)), "{}", y1 - y0);
        assert!((10.0..16.0).contains(&(x1 - x0)), "{}", x1 - x0);
        // The rays point away from the arrow (up and left of the tip).
        for r in &p.agent_rays {
            assert!(r.from.x <= 0.5 && r.to.x < 0.0, "{r:?}");
        }
    }

    #[test]
    fn both_cursors_work_at_once_and_stay_in_their_windows() {
        let p = preview();
        let mut both = 0;
        for t in (0..DRIVER_LOOP_MS).step_by(20) {
            let f = frame(t);
            // The agent stays in the back window's uncovered part.
            assert!(inside(f.agent, p.back.frame), "{t}: {:?}", f.agent);
            assert!(
                f.agent.x + 13.0 < p.front.frame.x || f.agent.y + 14.0 < p.front.frame.y,
                "{t}: the agent under the front window {:?}",
                f.agent
            );
            // The user's pointer stays in the front window.
            assert!(inside(f.pointer, p.front.frame), "{t}: {:?}", f.pointer);
            assert!(f.pointer.x + 9.0 <= p.width && f.pointer.y + 14.0 <= p.height);
            let agent_moving = frame(t + 20).agent != f.agent;
            let user_moving = frame(t + 20).pointer != f.pointer;
            if agent_moving && user_moving {
                both += 1;
            }
        }
        assert!(
            both > 20,
            "both cursors move at the same time ({both} frames)"
        );
    }

    #[test]
    fn the_agent_ticks_each_box_and_the_user_selects_a_line() {
        let f = |t| frame(t);
        assert_eq!(f(0).checked, [0.0, 0.0, 0.0]);
        assert_eq!(f(0).selection, 0.0);
        // At the first box: pressed, rays, then ticked.
        let press = f(950);
        assert!(press.agent_pressed && press.ripple > 0.0);
        assert_eq!(press.agent, box_centre(0));
        assert_eq!(f(1200).checked, [1.0, 0.0, 0.0]);
        assert_eq!(f(3000).checked, [1.0, 1.0, 1.0]);
        // The user drags the selection while the agent works.
        let drag = f(1500);
        assert!(drag.pressed && drag.selection > 0.0 && drag.selection < 0.86);
        assert!((f(2500).selection - 0.86).abs() < 1e-9);
        assert!(f(3150).pressed);
        assert_eq!(f(3300).selection, 0.0);
        // Seamless: everything is back where it started.
        let end = f(DRIVER_LOOP_MS - 1);
        assert_eq!((end.agent, end.pointer), (f(0).agent, f(0).pointer));
        assert_eq!(end.checked, f(0).checked);
        assert_eq!(f(DRIVER_LOOP_MS + 950), press);
    }

    #[test]
    fn reduce_motion_shows_the_agent_mid_task() {
        let s = still();
        assert!(!s.pressed && !s.agent_pressed);
        assert_eq!(s.ripple, 0.5);
        assert_eq!(s.checked[..2], [1.0, 1.0]);
        assert!(s.selection > 0.8);
        assert_eq!(s.agent, box_centre(2));
    }
}
