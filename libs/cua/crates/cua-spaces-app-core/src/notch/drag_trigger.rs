// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! When a dragged window opens the notch's drop surface.
//!
//! The shells feed raw facts: the drag started (with the window's frame at
//! mouse down and now), a cursor sample with its time, a window frame
//! sample, a timer tick, the release. This machine decides, for both apps:
//!
//! - **move or resize**: only a moved window counts. A drag from an edge
//!   or a corner changes the window's size; a move keeps it (within
//!   [`SIZE_TOLERANCE`]). Until the frames tell, nothing shows. See
//!   [`classify`].
//! - **the "Teleport to Cua" box** ([`TriggerPhase::Prompt`]) shows while a
//!   moved window is dragged.
//! - **expand** ([`TriggerPhase::Expanded`], the Space tiles as drop
//!   targets) when the cursor crosses a line [`TRIGGER_LINE_INSET`] points
//!   above the notch's bottom edge (inside the notch's width), so the user
//!   has to push into the notch; or when the cursor rests (moves less than
//!   [`STILL_TOLERANCE`]) for [`DWELL_MS`] inside the "Teleport to Cua" box.
//! - **stay** expanded while the cursor is inside the expanded panel;
//!   **collapse** back to the box when it leaves.
//!
//! The output is [`DragOverlayEvent`]s for [`crate::teleport::drag::apply`]
//! (through [`crate::notch::reduce`] in the SwiftUI app) and the time the
//! shell should send a [`DragTriggerEvent::Tick`] (the dwell can elapse
//! while the cursor sends no events). Hit-testing the tiles stays with the
//! shell that lays them out.
//!
//! Geometry is per display, in global top-left points (the drag events'
//! space; origin at the primary display's top left). [`display`] and
//! [`portal_display`] build it from a screen's facts: the notch's real
//! bottom edge from the safe area and the auxiliary top areas, or the menu
//! bar's height on a display without a notch.

use serde::{Deserialize, Serialize};

use super::geometry::{self, DisplayStyle, LogicalRect, WindowMode};
use super::{ScreenFacts, layout};
use crate::teleport::drag::DragOverlayEvent;

/// The trigger line sits this far above the notch's bottom edge (points).
pub const TRIGGER_LINE_INSET: f64 = 5.0;
/// The cursor must rest this long in the box to expand (ms).
pub const DWELL_MS: u64 = 300;
/// The cursor counts as resting while it stays within this distance of
/// where it stopped (points).
pub const STILL_TOLERANCE: f64 = 3.0;
/// A window whose width and height each change by at most this much is
/// still being moved, not resized (points).
pub const SIZE_TOLERANCE: f64 = 4.0;
/// The frame must shift this far to count as a move (points; the SDK's
/// `cua_teleport::ux::window::MOVE_THRESHOLD`).
pub const MOVE_THRESHOLD: f64 = 8.0;

/// What the drag does to the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DragKind {
    /// Not known yet (no frames, or they have not changed enough).
    Pending,
    /// The window follows the cursor at its size.
    Move,
    /// An edge or a corner: the size changes.
    Resize,
}

/// Compares the frame at mouse down with a later one.
pub fn classify(start: &LogicalRect, now: &LogicalRect) -> DragKind {
    if (now.width - start.width).abs() > SIZE_TOLERANCE
        || (now.height - start.height).abs() > SIZE_TOLERANCE
    {
        DragKind::Resize
    } else if (now.x - start.x).abs() > MOVE_THRESHOLD || (now.y - start.y).abs() > MOVE_THRESHOLD {
        DragKind::Move
    } else {
        DragKind::Pending
    }
}

/// What the notch shows for the drag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TriggerPhase {
    /// Nothing (no drag, a resize, or not known yet).
    Hidden,
    /// The "Teleport to Cua" box.
    Prompt,
    /// The Space tiles as drop targets.
    Expanded,
}

/// One display's trigger geometry, global top-left points.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DragDisplay {
    /// The display.
    pub frame: LogicalRect,
    /// The notch (or, without one, the menu-bar-high stand-in).
    pub notch: LogicalRect,
    /// The "Teleport to Cua" box: the dwell area.
    pub prompt: LogicalRect,
    /// The expanded panel: the drag stays expanded inside it.
    pub expanded: LogicalRect,
}

/// Where the cursor started resting.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StillAnchor {
    /// X.
    pub x: f64,
    /// Y.
    pub y: f64,
    /// When (ms, the shell's monotonic clock).
    pub since_ms: u64,
}

/// The machine's state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DragTriggerState {
    /// A drag is in flight.
    pub active: bool,
    /// Move or resize.
    pub kind: DragKind,
    /// What shows.
    pub phase: TriggerPhase,
    /// The dragged window.
    pub window_id: Option<u32>,
    /// Its app's name.
    pub app_name: Option<String>,
    /// The window's frame at mouse down.
    pub start_frame: Option<LogicalRect>,
    /// The display (index into the shell's list) the panel expanded on.
    pub display: Option<u32>,
    /// Where the cursor rests, while it rests in the box.
    pub still: Option<StillAnchor>,
}

impl Default for DragTriggerState {
    fn default() -> Self {
        Self {
            active: false,
            kind: DragKind::Pending,
            phase: TriggerPhase::Hidden,
            window_id: None,
            app_name: None,
            start_frame: None,
            display: None,
            still: None,
        }
    }
}

/// An input. Times are ms on one monotonic clock the shell owns.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum DragTriggerEvent {
    /// A window drag started (the SDK's `start`).
    #[serde(rename_all = "camelCase")]
    Start {
        /// Window id.
        window_id: Option<u32>,
        /// App name.
        app_name: Option<String>,
        /// Cursor x.
        x: f64,
        /// Cursor y.
        y: f64,
        /// Time.
        t_ms: u64,
        /// The window's frame at mouse down.
        start_frame: Option<LogicalRect>,
        /// The window's frame now.
        frame: Option<LogicalRect>,
    },
    /// The cursor moved.
    #[serde(rename_all = "camelCase")]
    Cursor {
        /// X.
        x: f64,
        /// Y.
        y: f64,
        /// Time.
        t_ms: u64,
    },
    /// The dragged window's frame.
    #[serde(rename_all = "camelCase")]
    Frame {
        /// Frame.
        frame: LogicalRect,
    },
    /// The timer [`DragTriggerTransition::tick_at_ms`] asked for fired.
    #[serde(rename_all = "camelCase")]
    Tick {
        /// Time.
        t_ms: u64,
    },
    /// Released.
    #[serde(rename_all = "camelCase")]
    End {
        /// Cursor x.
        x: f64,
        /// Cursor y.
        y: f64,
        /// Time.
        t_ms: u64,
    },
    /// Abort (the notch was hidden, the monitor stopped).
    Cancel,
}

/// A transition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DragTriggerTransition {
    /// New state.
    pub state: DragTriggerState,
    /// Feed these to the drag overlay, in order. A `drop` carries no Space:
    /// the shell puts in the tile under the cursor, if any.
    pub overlay: Vec<DragOverlayEvent>,
    /// Send a `tick` at this time (replacing any earlier request); none
    /// cancels it.
    pub tick_at_ms: Option<u64>,
}

/// The idle state.
pub fn initial() -> DragTriggerState {
    DragTriggerState::default()
}

fn contains(r: &LogicalRect, x: f64, y: f64) -> bool {
    x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height
}

fn display_at(displays: &[DragDisplay], x: f64, y: f64) -> Option<usize> {
    displays.iter().position(|d| contains(&d.frame, x, y))
}

/// Above the line [`TRIGGER_LINE_INSET`] above the notch's bottom, inside
/// the notch's width.
pub fn in_trigger_zone(d: &DragDisplay, x: f64, y: f64) -> bool {
    let line = d.notch.y + d.notch.height - TRIGGER_LINE_INSET;
    x >= d.notch.x && x < d.notch.x + d.notch.width && y >= d.frame.y && y < line
}

/// Advances the machine over `displays` (the shell's notch displays).
pub fn apply(
    state: &DragTriggerState,
    event: &DragTriggerEvent,
    displays: &[DragDisplay],
) -> DragTriggerTransition {
    let mut s = state.clone();
    let mut overlay = Vec::new();
    match event {
        DragTriggerEvent::Start {
            window_id,
            app_name,
            x,
            y,
            t_ms,
            start_frame,
            frame,
        } => {
            if s.active && s.kind == DragKind::Move {
                // A start without an end: the previous drag is over.
                overlay.push(DragOverlayEvent::Cancel);
            }
            s = DragTriggerState {
                active: true,
                window_id: *window_id,
                app_name: app_name.clone(),
                start_frame: *start_frame,
                kind: match (start_frame, frame) {
                    (Some(a), Some(b)) => classify(a, b),
                    // Frames still to come.
                    (Some(_), None) => DragKind::Pending,
                    // No frames at all: trust the SDK's start (the frame
                    // followed the cursor).
                    (None, _) => DragKind::Move,
                },
                ..initial()
            };
            if s.kind == DragKind::Move {
                confirm(&mut s, &mut overlay);
                pointer(&mut s, &mut overlay, displays, *x, *y, *t_ms);
            }
        }
        DragTriggerEvent::Frame { frame } if s.active && s.kind == DragKind::Pending => {
            match s.start_frame {
                Some(start) => s.kind = classify(&start, frame),
                None => s.start_frame = Some(*frame),
            }
            if s.kind == DragKind::Move {
                confirm(&mut s, &mut overlay);
            }
        }
        DragTriggerEvent::Cursor { x, y, t_ms } if s.active && s.kind == DragKind::Move => {
            pointer(&mut s, &mut overlay, displays, *x, *y, *t_ms);
        }
        DragTriggerEvent::Tick { t_ms } if s.active && s.phase == TriggerPhase::Prompt => {
            if let Some(i) = s
                .still
                .filter(|a| *t_ms >= a.since_ms + DWELL_MS)
                .and_then(|a| display_at(displays, a.x, a.y))
            {
                expand(&mut s, &mut overlay, i);
            }
        }
        DragTriggerEvent::End { x, y, t_ms } => {
            if s.active && s.kind == DragKind::Move {
                pointer(&mut s, &mut overlay, displays, *x, *y, *t_ms);
                overlay.push(DragOverlayEvent::Drop { space_id: None });
            }
            s = initial();
        }
        DragTriggerEvent::Cancel => {
            if s.active && s.kind == DragKind::Move {
                overlay.push(DragOverlayEvent::Cancel);
            }
            s = initial();
        }
        _ => {}
    }
    let tick_at_ms = (s.active && s.phase == TriggerPhase::Prompt)
        .then(|| s.still.map(|a| a.since_ms + DWELL_MS))
        .flatten();
    DragTriggerTransition {
        state: s,
        overlay,
        tick_at_ms,
    }
}

fn confirm(s: &mut DragTriggerState, overlay: &mut Vec<DragOverlayEvent>) {
    s.phase = TriggerPhase::Prompt;
    overlay.push(DragOverlayEvent::Start {
        window_id: s.window_id,
        app_name: s.app_name.clone(),
    });
}

fn expand(s: &mut DragTriggerState, overlay: &mut Vec<DragOverlayEvent>, display: usize) {
    s.phase = TriggerPhase::Expanded;
    s.display = Some(display as u32);
    s.still = None;
    overlay.push(DragOverlayEvent::EnterNotch);
}

fn pointer(
    s: &mut DragTriggerState,
    overlay: &mut Vec<DragOverlayEvent>,
    displays: &[DragDisplay],
    x: f64,
    y: f64,
    t: u64,
) {
    let here = display_at(displays, x, y);
    if s.phase == TriggerPhase::Expanded {
        let inside = s
            .display
            .and_then(|i| displays.get(i as usize))
            .is_some_and(|d| contains(&d.expanded, x, y));
        if inside {
            return;
        }
        s.phase = TriggerPhase::Prompt;
        s.display = None;
        overlay.push(DragOverlayEvent::LeaveNotch);
    }
    let Some(i) = here else {
        s.still = None;
        return;
    };
    let d = &displays[i];
    if in_trigger_zone(d, x, y) {
        expand(s, overlay, i);
        return;
    }
    if !contains(&d.prompt, x, y) {
        s.still = None;
        return;
    }
    let anchor = match s.still {
        Some(a) if (x - a.x).hypot(y - a.y) <= STILL_TOLERANCE => a,
        _ => StillAnchor { x, y, since_ms: t },
    };
    s.still = Some(anchor);
    if t >= anchor.since_ms + DWELL_MS {
        expand(s, overlay, i);
    }
}

/// An AppKit rectangle (origin bottom left of the primary display, whose
/// top is `primary_top`) in global top-left points.
pub fn flip(r: &LogicalRect, primary_top: f64) -> LogicalRect {
    LogicalRect::new(r.x, primary_top - (r.y + r.height), r.width, r.height)
}

/// A screen's trigger geometry for the SwiftUI notch: the core's
/// [`layout`] (the open panel with its line above the tiles), flipped.
/// `primary_top` is the primary display's `frame.maxY`.
pub fn display(screen: &ScreenFacts, primary_top: f64) -> DragDisplay {
    let l = layout(screen, true);
    DragDisplay {
        frame: flip(&screen.frame, primary_top),
        notch: flip(&l.notch, primary_top),
        prompt: flip(&l.prompt_frame, primary_top),
        expanded: flip(&l.open_frame, primary_top),
    }
}

/// The same for the Tauri portal, whose expanded panel is its switcher
/// window, top-centred on the display.
pub fn portal_display(screen: &ScreenFacts, primary_top: f64) -> DragDisplay {
    let d = display(screen, primary_top);
    let style = if layout(screen, false).has_notch {
        DisplayStyle::Notched
    } else {
        DisplayStyle::NoNotch
    };
    DragDisplay {
        expanded: geometry::top_center(d.frame, geometry::mode_size(WindowMode::Switcher, style)),
        ..d
    }
}

/// [`display`] for every screen; the primary is the first.
pub fn displays(screens: &[ScreenFacts]) -> Vec<DragDisplay> {
    let top = primary_top(screens);
    screens.iter().map(|s| display(s, top)).collect()
}

/// [`portal_display`] for every screen; the primary is the first.
pub fn portal_displays(screens: &[ScreenFacts]) -> Vec<DragDisplay> {
    let top = primary_top(screens);
    screens.iter().map(|s| portal_display(s, top)).collect()
}

fn primary_top(screens: &[ScreenFacts]) -> f64 {
    screens.first().map_or(0.0, |s| s.frame.y + s.frame.height)
}

#[cfg(test)]
mod tests {
    use super::*;
    use DragTriggerEvent as E;

    /// A 14-inch MacBook Pro: a 32 pt notch, 185 pt wide at the top centre.
    fn mbp14() -> ScreenFacts {
        ScreenFacts {
            frame: LogicalRect::new(0.0, 0.0, 1512.0, 982.0),
            visible_frame: LogicalRect::new(0.0, 0.0, 1512.0, 945.0),
            safe_area_top: 32.0,
            aux_left_width: Some(665.5),
            aux_right_width: Some(665.5),
        }
    }

    /// A 1920 x 1080 display right of the MacBook, tops aligned (AppKit y
    /// = 982 - 1080), with a 25 pt menu bar.
    fn external() -> ScreenFacts {
        ScreenFacts {
            frame: LogicalRect::new(1512.0, -98.0, 1920.0, 1080.0),
            visible_frame: LogicalRect::new(1512.0, -98.0, 1920.0, 1055.0),
            safe_area_top: 0.0,
            aux_left_width: None,
            aux_right_width: None,
        }
    }

    fn win(x: f64, y: f64, w: f64, h: f64) -> Option<LogicalRect> {
        Some(LogicalRect::new(x, y, w, h))
    }

    fn start(start_frame: Option<LogicalRect>, frame: Option<LogicalRect>) -> E {
        E::Start {
            window_id: Some(7),
            app_name: Some("Google Chrome".into()),
            x: 700.0,
            y: 500.0,
            t_ms: 0,
            start_frame,
            frame,
        }
    }

    fn cursor(x: f64, y: f64, t_ms: u64) -> E {
        E::Cursor { x, y, t_ms }
    }

    /// Replays `events`, returning the state and every overlay event.
    fn run(
        displays: &[DragDisplay],
        events: &[E],
    ) -> (DragTriggerTransition, Vec<DragOverlayEvent>) {
        let mut s = initial();
        let mut all = Vec::new();
        let mut last = None;
        for e in events {
            let t = apply(&s, e, displays);
            s = t.state.clone();
            all.extend(t.overlay.clone());
            last = Some(t);
        }
        (last.unwrap(), all)
    }

    fn moved() -> E {
        start(
            win(400.0, 300.0, 800.0, 600.0),
            win(440.0, 260.0, 800.0, 600.0),
        )
    }

    #[test]
    fn a_title_bar_drag_is_a_move() {
        let a = LogicalRect::new(400.0, 300.0, 800.0, 600.0);
        assert_eq!(
            classify(&a, &LogicalRect::new(440.0, 260.0, 800.0, 600.0)),
            DragKind::Move
        );
        // Not far enough yet.
        assert_eq!(
            classify(&a, &LogicalRect::new(405.0, 300.0, 800.0, 600.0)),
            DragKind::Pending
        );
        let d = displays(&[mbp14()]);
        let (t, overlay) = run(&d, &[moved()]);
        assert_eq!(t.state.kind, DragKind::Move);
        assert_eq!(t.state.phase, TriggerPhase::Prompt);
        assert!(matches!(
            overlay.as_slice(),
            [DragOverlayEvent::Start {
                window_id: Some(7),
                ..
            }]
        ));
    }

    #[test]
    fn edge_and_corner_drags_are_resizes() {
        let a = LogicalRect::new(400.0, 300.0, 800.0, 600.0);
        for (name, b) in [
            ("left edge", LogicalRect::new(380.0, 300.0, 820.0, 600.0)),
            ("top edge", LogicalRect::new(400.0, 280.0, 800.0, 620.0)),
            ("right edge", LogicalRect::new(400.0, 300.0, 830.0, 600.0)),
            ("bottom edge", LogicalRect::new(400.0, 300.0, 800.0, 640.0)),
            (
                "top-left corner",
                LogicalRect::new(370.0, 290.0, 830.0, 610.0),
            ),
            (
                "bottom-right corner",
                LogicalRect::new(400.0, 300.0, 790.0, 590.0),
            ),
        ] {
            assert_eq!(classify(&a, &b), DragKind::Resize, "{name}");
        }
        // A Chrome left-edge resize the SDK reports as a start (its origin
        // shifted): nothing shows, even at the notch, and the release
        // drops nothing.
        let d = displays(&[mbp14()]);
        let (t, overlay) = run(
            &d,
            &[
                start(
                    win(400.0, 300.0, 800.0, 600.0),
                    win(380.0, 300.0, 820.0, 600.0),
                ),
                cursor(756.0, 2.0, 50),
                E::Tick { t_ms: 1000 },
                E::End {
                    x: 756.0,
                    y: 2.0,
                    t_ms: 1100,
                },
            ],
        );
        assert!(overlay.is_empty(), "{overlay:?}");
        assert_eq!(t.state, initial());
    }

    #[test]
    fn a_window_that_changes_size_slightly_while_it_moves_is_a_move() {
        let a = LogicalRect::new(400.0, 300.0, 800.0, 600.0);
        assert_eq!(
            classify(&a, &LogicalRect::new(460.0, 200.0, 803.0, 597.0)),
            DragKind::Move
        );
        // Undecided at the start; a later frame sample decides.
        let d = displays(&[mbp14()]);
        let (t, overlay) = run(
            &d,
            &[
                start(win(400.0, 300.0, 800.0, 600.0), None),
                cursor(756.0, 2.0, 10),
                E::Frame {
                    frame: LogicalRect::new(500.0, 100.0, 802.0, 601.0),
                },
            ],
        );
        assert_eq!(t.state.kind, DragKind::Move);
        assert_eq!(t.state.phase, TriggerPhase::Prompt);
        assert_eq!(overlay.len(), 1);
        // Once a move, a later size change (a display with another scale)
        // does not make it a resize.
        let t = apply(
            &t.state,
            &E::Frame {
                frame: LogicalRect::new(500.0, 100.0, 600.0, 400.0),
            },
            &d,
        );
        assert_eq!(t.state.kind, DragKind::Move);
    }

    #[test]
    fn the_line_sits_five_points_above_the_notch_bottom() {
        let d = displays(&[mbp14()]);
        // The notch: 32 pt tall at the top, so the line is at y = 27.
        assert_eq!(d[0].notch.y, 0.0);
        assert_eq!(d[0].notch.height, 32.0);
        let mid = d[0].notch.x + d[0].notch.width / 2.0;
        // Just below the line, moving (no dwell): still the box.
        let (t, _) = run(
            &d,
            &[moved(), cursor(mid, 27.0, 10), cursor(mid + 10.0, 27.5, 20)],
        );
        assert_eq!(t.state.phase, TriggerPhase::Prompt);
        // Just above it: expanded at once.
        let t = apply(&t.state, &cursor(mid, 26.9, 30), &d);
        assert_eq!(t.state.phase, TriggerPhase::Expanded);
        assert_eq!(t.overlay, vec![DragOverlayEvent::EnterNotch]);
        assert_eq!(t.tick_at_ms, None);
        // The top of the screen beside the notch (a normal window move to
        // the top) is not the zone.
        let (t, _) = run(&d, &[moved(), cursor(d[0].notch.x - 1.0, 0.0, 10)]);
        assert_eq!(t.state.phase, TriggerPhase::Prompt);
        let (t, _) = run(&d, &[moved(), cursor(300.0, 0.0, 10)]);
        assert_eq!(t.state.phase, TriggerPhase::Prompt);
    }

    #[test]
    fn resting_for_the_dwell_in_the_box_expands() {
        let d = displays(&[mbp14()]);
        let p = d[0].prompt;
        let (x, y) = (p.x + p.width / 2.0, p.y + p.height - 4.0);
        assert!(!in_trigger_zone(&d[0], x, y));
        // Jitter under the tolerance still counts as still.
        let (t, overlay) = run(
            &d,
            &[
                moved(),
                cursor(x, y, 1000),
                cursor(x + 1.5, y - 1.0, 1100),
                cursor(x - 1.0, y + 2.0, 1200),
            ],
        );
        assert_eq!(t.state.phase, TriggerPhase::Prompt);
        assert_eq!(t.tick_at_ms, Some(1000 + DWELL_MS));
        assert_eq!(overlay.len(), 1);
        // The tick before the dwell does nothing; at the dwell it expands.
        let early = apply(&t.state, &E::Tick { t_ms: 1299 }, &d);
        assert_eq!(early.state.phase, TriggerPhase::Prompt);
        let t = apply(&t.state, &E::Tick { t_ms: 1300 }, &d);
        assert_eq!(t.state.phase, TriggerPhase::Expanded);
        assert_eq!(t.overlay, vec![DragOverlayEvent::EnterNotch]);
        // A cursor sample past the dwell expands without a tick too.
        let (t, _) = run(&d, &[moved(), cursor(x, y, 1000), cursor(x, y + 1.0, 1300)]);
        assert_eq!(t.state.phase, TriggerPhase::Expanded);
    }

    #[test]
    fn movement_resets_the_dwell() {
        let d = displays(&[mbp14()]);
        let p = d[0].prompt;
        let (x, y) = (p.x + p.width / 2.0, p.y + p.height - 4.0);
        let (t, _) = run(
            &d,
            &[
                moved(),
                cursor(x, y, 1000),
                // More than the tolerance: the timer starts over.
                cursor(x + 4.0, y, 1200),
            ],
        );
        assert_eq!(t.tick_at_ms, Some(1200 + DWELL_MS));
        let t2 = apply(&t.state, &E::Tick { t_ms: 1300 }, &d);
        assert_eq!(t2.state.phase, TriggerPhase::Prompt);
        assert_eq!(t2.tick_at_ms, Some(1500));
        // Leaving the box drops the dwell.
        let t3 = apply(&t.state, &cursor(x, p.y + p.height + 40.0, 1250), &d);
        assert_eq!(t3.state.still, None);
        assert_eq!(t3.tick_at_ms, None);
        let t4 = apply(&t3.state, &E::Tick { t_ms: 2000 }, &d);
        assert_eq!(t4.state.phase, TriggerPhase::Prompt);
        // Resting outside the box never expands.
        let (t, _) = run(
            &d,
            &[moved(), cursor(200.0, 400.0, 0), cursor(200.0, 400.0, 900)],
        );
        assert_eq!(t.state.phase, TriggerPhase::Prompt);
        assert_eq!(t.tick_at_ms, None);
    }

    #[test]
    fn expanded_stays_inside_the_panel_and_collapses_when_the_cursor_leaves() {
        let d = displays(&[mbp14()]);
        let e = d[0].expanded;
        let mid = d[0].notch.x + d[0].notch.width / 2.0;
        let (t, _) = run(&d, &[moved(), cursor(mid, 4.0, 10)]);
        assert_eq!(t.state.phase, TriggerPhase::Expanded);
        // Down over the tiles, far below the line and outside the box.
        let t = apply(&t.state, &cursor(e.x + 20.0, e.y + e.height - 10.0, 20), &d);
        assert_eq!(t.state.phase, TriggerPhase::Expanded);
        assert!(t.overlay.is_empty());
        // Out of the panel: back to the box.
        let t = apply(&t.state, &cursor(e.x + 20.0, e.y + e.height + 1.0, 30), &d);
        assert_eq!(t.state.phase, TriggerPhase::Prompt);
        assert_eq!(t.overlay, vec![DragOverlayEvent::LeaveNotch]);
        // Released there: a drop the overlay commits to nothing.
        let end = apply(
            &t.state,
            &E::End {
                x: 10.0,
                y: 500.0,
                t_ms: 40,
            },
            &d,
        );
        assert_eq!(end.overlay, vec![DragOverlayEvent::Drop { space_id: None }]);
        assert_eq!(end.state, initial());
    }

    #[test]
    fn a_release_inside_the_panel_drops_there() {
        let d = displays(&[mbp14()]);
        let mid = d[0].notch.x + d[0].notch.width / 2.0;
        let (_, overlay) = run(
            &d,
            &[
                moved(),
                cursor(mid, 4.0, 10),
                E::End {
                    x: mid,
                    y: 90.0,
                    t_ms: 20,
                },
            ],
        );
        assert_eq!(
            overlay[1..],
            [
                DragOverlayEvent::EnterNotch,
                DragOverlayEvent::Drop { space_id: None }
            ]
        );
        // Cancel ends a move with a cancel, and a resize with nothing.
        let t = apply(&apply(&initial(), &moved(), &d).state, &E::Cancel, &d);
        assert_eq!(t.overlay, vec![DragOverlayEvent::Cancel]);
    }

    #[test]
    fn multiple_displays_with_and_without_a_notch() {
        let screens = [mbp14(), external()];
        let d = displays(&screens);
        // The external display: tops aligned in global top-left points, a
        // 25 pt menu-bar notch stand-in, so the line is at y = 20.
        assert_eq!(d[1].frame, LogicalRect::new(1512.0, 0.0, 1920.0, 1080.0));
        assert_eq!(d[1].notch.y, 0.0);
        assert_eq!(d[1].notch.height, 25.0);
        let mid = d[1].notch.x + d[1].notch.width / 2.0;
        assert!(mid > 1512.0 + 900.0 && mid < 1512.0 + 1020.0);
        let (t, _) = run(&d, &[moved(), cursor(mid, 20.0, 10)]);
        assert_eq!(t.state.phase, TriggerPhase::Prompt);
        let t = apply(&t.state, &cursor(mid, 19.0, 20), &d);
        assert_eq!(t.state.phase, TriggerPhase::Expanded);
        assert_eq!(t.state.display, Some(1));
        // Crossing onto the MacBook leaves the external panel.
        let t = apply(&t.state, &cursor(1400.0, 300.0, 30), &d);
        assert_eq!(t.state.phase, TriggerPhase::Prompt);
        // The MacBook's notch line is its own (27 pt).
        let nb = d[0].notch.x + 10.0;
        let t = apply(&t.state, &cursor(nb, 26.0, 40), &d);
        assert_eq!(t.state.display, Some(0));
        // A display above the MacBook (AppKit y above its top): flipped to
        // negative top-left y, with its own line.
        let above = ScreenFacts {
            frame: LogicalRect::new(0.0, 982.0, 1440.0, 900.0),
            visible_frame: LogicalRect::new(0.0, 982.0, 1440.0, 870.0),
            safe_area_top: 0.0,
            aux_left_width: None,
            aux_right_width: None,
        };
        let d = displays(&[mbp14(), above]);
        assert_eq!(d[1].frame.y, -900.0);
        assert_eq!(d[1].notch.height, 30.0);
        let mid = d[1].notch.x + d[1].notch.width / 2.0;
        let (t, _) = run(&d, &[moved(), cursor(mid, -875.0, 10)]);
        assert_eq!(t.state.phase, TriggerPhase::Prompt);
        let t = apply(&t.state, &cursor(mid, -875.5, 20), &d);
        assert_eq!(t.state.phase, TriggerPhase::Expanded);
        // Off every display: nothing rests.
        let (t, _) = run(&d, &[moved(), cursor(-50.0, -50.0, 10)]);
        assert_eq!(t.state.still, None);
    }

    #[test]
    fn the_portal_expands_into_its_switcher_frame() {
        let d = portal_displays(&[mbp14(), external()]);
        assert_eq!(d[0].expanded, LogicalRect::new(376.0, 0.0, 760.0, 320.0));
        assert_eq!(d[1].expanded.y, 0.0);
        assert_eq!(d[1].expanded.height, 300.0);
        assert_eq!(d[0].notch, displays(&[mbp14()])[0].notch);
    }

    #[test]
    fn a_start_without_frames_trusts_the_sdk_and_events_serialize() {
        let d = displays(&[mbp14()]);
        let t = apply(&initial(), &start(None, None), &d);
        assert_eq!(t.state.kind, DragKind::Move);
        let json = serde_json::to_value(&t).unwrap();
        assert_eq!(json["state"]["kind"], "move");
        assert_eq!(json["state"]["phase"], "prompt");
        assert_eq!(json["overlay"][0]["type"], "start");
        let e: E = serde_json::from_value(serde_json::json!({
            "type": "start", "windowId": 1, "appName": "A", "x": 1.0, "y": 2.0,
            "tMs": 3, "startFrame": null, "frame": null
        }))
        .unwrap();
        assert!(matches!(e, E::Start { t_ms: 3, .. }));
    }
}
