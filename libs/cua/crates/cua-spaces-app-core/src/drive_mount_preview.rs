// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The first run's Cua Volume page: an animated miniature drawn natively by
//! each shell (SwiftUI shapes in the Swift app, HTML and CSS in the Tauri
//! app) from the scene here and animated by the frames here, the same way
//! as the AI agents page's card ([`crate::driver_preview`]).
//!
//! Two windows side by side:
//!
//! - left, a Space with three files;
//! - right, a Finder window whose sidebar gains the "Cua Volume" volume
//!   (it fades in, selected), then each file lifts off the Space, arcs over
//!   and lands in the volume, one after another.
//!
//! Everything fades out before the loop ends, so one loop of
//! [`DRIVE_LOOP_MS`] is seamless. With Reduce Motion the shells draw
//! [`still`]. Coordinates are the stage's points, origin top left, y down;
//! the shells centre the stage in the card and draw the desktop across the
//! card's full width.

use crate::driver_preview::PreviewWindow;
use crate::notch::geometry::LogicalRect;
use crate::onboarding_preview::PreviewPoint;
use serde::{Deserialize, Serialize};

/// One loop (ms).
pub const DRIVE_LOOP_MS: u32 = 4800;
/// The time [`still`] shows: the volume mounted, two files in it, the third
/// on its way.
pub const DRIVE_STILL_MS: u32 = 2400;
/// The stage, in points.
pub const DRIVE_STAGE_WIDTH: f64 = 232.0;
/// The stage's height.
pub const DRIVE_STAGE_HEIGHT: f64 = 112.0;
/// The volume's name in the Finder sidebar.
pub const VOLUME_NAME: &str = "Cua Volume";

const FILES: usize = 3;
const TITLE_BAR: f64 = 11.0;
const RADIUS: f64 = 5.0;
/// A file icon.
const ICON: (f64, f64) = (7.0, 9.0);
/// Label bar widths, one per file.
const LABELS: [f64; FILES] = [40.0, 30.0, 36.0];
/// Rows are this far apart.
const ROW: f64 = 16.0;
/// How high a file arcs above the straight line between its rows.
const LIFT: f64 = 12.0;

/// The miniature.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveMountPreview {
    /// Stage width.
    pub width: f64,
    /// Stage height.
    pub height: f64,
    /// One loop (ms).
    pub loop_ms: u32,
    /// The Space (left).
    pub space: PreviewWindow,
    /// The Finder window (right).
    pub finder: PreviewWindow,
    /// The Finder window's sidebar (a shade darker than its content).
    pub sidebar: LogicalRect,
    /// The sidebar's other places (placeholder bars).
    pub places: Vec<LogicalRect>,
    /// The volume's row in the sidebar (the selection highlight).
    pub volume: LogicalRect,
    /// The volume's drive glyph.
    pub volume_icon: LogicalRect,
    /// The volume's name, drawn from `volume_label_x`, centred on the row.
    pub volume_label: String,
    /// Where the name starts.
    pub volume_label_x: f64,
    /// The name's font size.
    pub font_size: f64,
    /// The Space's files (icons), top to bottom.
    pub source_icons: Vec<LogicalRect>,
    /// Their name bars.
    pub source_labels: Vec<LogicalRect>,
    /// Where each file lands in the volume (icons).
    pub dest_icons: Vec<LogicalRect>,
    /// Their name bars.
    pub dest_labels: Vec<LogicalRect>,
}

/// One moment of the miniature.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveMountFrame {
    /// The volume in the sidebar, 0 absent to 1 shown (and selected).
    pub volume: f64,
    /// The file in flight: its icon's top left (drawn above both windows).
    pub flight: Option<PreviewPoint>,
    /// Each file in the volume, 0 absent to 1 shown.
    pub arrived: Vec<f64>,
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> LogicalRect {
    LogicalRect::new(x, y, w, h)
}

fn pt(x: f64, y: f64) -> PreviewPoint {
    PreviewPoint { x, y }
}

fn space_window() -> PreviewWindow {
    PreviewWindow {
        frame: rect(10.0, 18.0, 88.0, 72.0),
        title_bar: TITLE_BAR,
        radius: RADIUS,
    }
}

fn finder_window() -> PreviewWindow {
    PreviewWindow {
        frame: rect(106.0, 8.0, 116.0, 96.0),
        title_bar: TITLE_BAR,
        radius: RADIUS,
    }
}

const SIDEBAR_WIDTH: f64 = 56.0;

fn sidebar() -> LogicalRect {
    let f = finder_window().frame;
    rect(f.x, f.y, SIDEBAR_WIDTH, f.height)
}

fn icons(x: f64, top: f64) -> Vec<LogicalRect> {
    (0..FILES)
        .map(|i| rect(x, top + i as f64 * ROW, ICON.0, ICON.1))
        .collect()
}

fn labels_for(icons: &[LogicalRect]) -> Vec<LogicalRect> {
    icons
        .iter()
        .zip(LABELS)
        .map(|(r, w)| rect(r.x + r.width + 4.0, r.y + 3.0, w, 3.0))
        .collect()
}

fn source_icons() -> Vec<LogicalRect> {
    let s = space_window().frame;
    icons(s.x + 8.0, s.y + TITLE_BAR + 8.0)
}

fn dest_icons() -> Vec<LogicalRect> {
    let f = finder_window().frame;
    icons(f.x + SIDEBAR_WIDTH + 6.0, f.y + TITLE_BAR + 8.0)
}

/// The miniature.
pub fn preview() -> DriveMountPreview {
    let sb = sidebar();
    let volume = rect(
        sb.x + 3.0,
        sb.y + TITLE_BAR + 27.0,
        SIDEBAR_WIDTH - 6.0,
        11.0,
    );
    let icon = rect(volume.x + 3.0, volume.y + 3.0, 7.0, 5.0);
    let source = source_icons();
    let dest = dest_icons();
    DriveMountPreview {
        width: DRIVE_STAGE_WIDTH,
        height: DRIVE_STAGE_HEIGHT,
        loop_ms: DRIVE_LOOP_MS,
        space: space_window(),
        finder: finder_window(),
        sidebar: sb,
        places: vec![
            rect(sb.x + 6.0, sb.y + TITLE_BAR + 7.0, 22.0, 3.0),
            rect(sb.x + 6.0, sb.y + TITLE_BAR + 16.0, 28.0, 3.0),
        ],
        volume,
        volume_icon: icon,
        volume_label: VOLUME_NAME.into(),
        volume_label_x: icon.x + icon.width + 3.0,
        font_size: 6.0,
        source_labels: labels_for(&source),
        source_icons: source,
        dest_labels: labels_for(&dest),
        dest_icons: dest,
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

/// The volume fades in.
const VOLUME_IN: (f64, f64) = (200.0, 600.0);
/// Each file's flight, back to back.
const FLIGHTS: [(f64, f64); FILES] = [(900.0, 1500.0), (1500.0, 2100.0), (2100.0, 2700.0)];
/// A landed file's row fades in over this long.
const LAND_MS: f64 = 150.0;
/// Everything fades out.
const CLEAR: (f64, f64) = (4200.0, 4600.0);

/// The miniature at `t_ms` into its loop (any `t_ms`: it wraps).
pub fn frame(t_ms: u32) -> DriveMountFrame {
    let t = (t_ms % DRIVE_LOOP_MS) as f64;
    let clear = 1.0 - span(t, CLEAR.0, CLEAR.1);
    let round = |v: f64| (v * 1000.0).round() / 1000.0;
    let source = source_icons();
    let dest = dest_icons();
    let flight = FLIGHTS
        .iter()
        .enumerate()
        .find(|(_, (a, b))| t >= *a && t < *b)
        .map(|(i, &(a, b))| {
            let p = ease(span(t, a, b));
            let (s, d) = (source[i], dest[i]);
            let lift = LIFT * (std::f64::consts::PI * p).sin();
            pt(
                round(s.x + (d.x - s.x) * p),
                round(s.y + (d.y - s.y) * p - lift),
            )
        });
    DriveMountFrame {
        volume: round(ease(span(t, VOLUME_IN.0, VOLUME_IN.1)) * clear),
        flight,
        arrived: FLIGHTS
            .iter()
            .map(|&(_, land)| round(span(t, land, land + LAND_MS) * clear))
            .collect(),
    }
}

/// The Reduce Motion picture: the volume mounted, two files in it, the
/// third on its way. No loop.
pub fn still() -> DriveMountFrame {
    frame(DRIVE_STILL_MS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn within(r: LogicalRect, outer: LogicalRect) -> bool {
        r.x >= outer.x
            && r.y >= outer.y
            && r.x + r.width <= outer.x + outer.width
            && r.y + r.height <= outer.y + outer.height
    }

    #[test]
    fn the_scene_fits_and_each_part_is_in_its_window() {
        let p = preview();
        let stage = rect(0.0, 0.0, p.width, p.height);
        assert!(within(p.space.frame, stage) && within(p.finder.frame, stage));
        // Side by side, not overlapping.
        assert!(p.space.frame.x + p.space.frame.width < p.finder.frame.x);
        for r in p.source_icons.iter().chain(&p.source_labels) {
            assert!(within(*r, p.space.frame), "{r:?}");
        }
        let content = rect(
            p.sidebar.x + p.sidebar.width,
            p.finder.frame.y,
            p.finder.frame.width - p.sidebar.width,
            p.finder.frame.height,
        );
        for r in p.dest_icons.iter().chain(&p.dest_labels) {
            assert!(within(*r, content), "{r:?}");
        }
        for r in p.places.iter().chain([&p.volume, &p.volume_icon]) {
            assert!(within(*r, p.sidebar), "{r:?}");
        }
        assert_eq!(p.volume_label, "Cua Volume");
        assert_eq!(p.source_icons.len(), p.dest_icons.len());
    }

    #[test]
    fn the_volume_mounts_then_each_file_flies_over_in_turn() {
        let f = frame;
        let start = f(0);
        assert_eq!((start.volume, start.flight.is_none()), (0.0, true));
        assert_eq!(start.arrived, [0.0, 0.0, 0.0]);
        assert_eq!(f(700).volume, 1.0, "mounted before the first file");
        let p = preview();
        // Take-off at the source, landing at the destination, above both
        // mid-way.
        assert_eq!(
            f(900).flight.unwrap(),
            pt(p.source_icons[0].x, p.source_icons[0].y)
        );
        let mid = f(1200).flight.unwrap();
        assert!(
            mid.y < p.source_icons[0].y.min(p.dest_icons[0].y),
            "{mid:?}"
        );
        assert_eq!(f(1700).arrived, [1.0, 0.0, 0.0]);
        assert!(f(1700).flight.is_some(), "the next one is on its way");
        assert_eq!(f(3000).arrived, [1.0, 1.0, 1.0]);
        assert!(f(3000).flight.is_none());
        // One file in the air at a time.
        for t in (0..DRIVE_LOOP_MS).step_by(10) {
            let n = FLIGHTS
                .iter()
                .filter(|(a, b)| (t as f64) >= *a && (t as f64) < *b)
                .count();
            assert!(n <= 1, "{t}");
        }
        // Seamless.
        let end = f(DRIVE_LOOP_MS - 1);
        assert_eq!(end, f(0));
        assert_eq!(f(DRIVE_LOOP_MS + 1200), f(1200));
    }

    #[test]
    fn reduce_motion_shows_the_volume_with_files_arriving() {
        let s = still();
        assert_eq!(s.volume, 1.0);
        assert_eq!(s.arrived, [1.0, 1.0, 0.0]);
        assert!(s.flight.is_some());
    }
}
