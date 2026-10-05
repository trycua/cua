// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The onboarding presentation page's animated previews: one miniature per
//! card ("Notch and menu bar", "Menu bar only"), drawn natively by each
//! shell (SwiftUI shapes in the Swift app, HTML and CSS in the Tauri app)
//! from the scene here, and animated by the frames here, so both shells
//! play the same beats.
//!
//! - The notch card: the top edge of a Mac screen with the notch and its
//!   "N Spaces" tab. A pointer glides up to the notch, rests through the
//!   hover dwell (the notch grows a little, [`MOTION`]'s cue), the notch
//!   springs open into the real panel (the search, the buttons and Space
//!   tiles, [`notch::view`]), holds, the pointer leaves, it closes.
//! - The menu bar card: the right end of the menu bar. A pointer glides to
//!   the Cua Spaces icon, clicks, the real menu ([`window::menu_bar`])
//!   drops down, the pointer moves onto "Open Cua Spaces" (highlighted),
//!   clicks it, the menu closes.
//!
//! One loop is [`LOOP_MS`]. With Reduce Motion the shells draw [`still`]:
//! the expanded state, no loop. Coordinates are the stage's points, origin
//! top left, y down. The stage is as tall as the open panel or the open menu
//! plus a margin; the cards are stacked, so the shells draw the desktop and
//! the menu bar across the card's full width and place the stage in it:
//! centred for the notch, at the trailing edge (the menu bar's end) for the
//! menu.

use crate::model::{Space, SpaceOs, SpaceProvider, SpaceStatus, ThumbnailScene};
use crate::notch::geometry::LogicalRect;
use crate::notch::{
    self, CLOSED_RADII, CONTENT_PADDING, MOTION, NotchRadii, NotchState, NotchView, OPEN_RADII,
    TILE_GAP, TILE_ROW_HEIGHT, TILE_WIDTH,
};
use crate::window::{self, MenuItem, MenuItemId};
use serde::{Deserialize, Serialize};

/// One loop of either preview (ms).
pub const LOOP_MS: u32 = 4000;
/// The time [`still`] shows: both previews expanded and settled.
pub const STILL_MS: u32 = 2400;
/// The stage (the part of each card's picture that moves), in points.
pub const STAGE_WIDTH: f64 = 232.0;
/// Room under the tallest state (the open panel, the open menu).
pub const STAGE_MARGIN: f64 = 8.0;
/// Real notch points to stage points.
pub const NOTCH_SCALE: f64 = 0.45;
/// Real menu points to stage points (larger, so the items read).
pub const MENU_SCALE: f64 = 0.55;
/// The Spaces the notch preview shows (three tiles).
pub const SPACES: u32 = 3;

// The notch the miniature draws: a 14-inch MacBook Pro's (the layout's
// width, with its 4 pt of slack, and the safe-area height).
const NOTCH_REAL: (f64, f64) = (192.0, 32.0);
// The tab's real width around "3 / SPACES" (the shells size it to its
// content, at most TAB_WIDTH).
const TAB_REAL: f64 = 56.0;
// A macOS menu, in real points.
const MENU_WIDTH_REAL: f64 = 190.0;
const MENU_ROW_REAL: f64 = 22.0;
const MENU_SEPARATOR_REAL: f64 = 11.0;
const MENU_PADDING_REAL: f64 = 5.0;
const MENU_RADIUS_REAL: f64 = 10.0;
const MENU_INSET_REAL: f64 = 12.0;
// The standard arrow pointer, tip at the origin, in real points.
const ARROW: [(f64, f64); 7] = [
    (0.0, 0.0),
    (0.0, 15.0),
    (3.6, 11.6),
    (6.2, 17.2),
    (8.6, 16.2),
    (6.1, 10.6),
    (10.8, 10.6),
];
// The pointer's size on the stage (it reads at any miniature scale).
const POINTER_SCALE: f64 = 0.8;

/// A point on the stage.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PreviewPoint {
    /// Left.
    pub x: f64,
    /// Down.
    pub y: f64,
}

/// The notch miniature: the closed and open shapes (they morph between
/// the two), the tab, and the open panel's real content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotchPreview {
    /// The closed notch with its ears, top centred.
    pub closed: LogicalRect,
    /// The open panel, top centred.
    pub open: LogicalRect,
    /// Closed radii, scaled.
    pub closed_radii: NotchRadii,
    /// Open radii, scaled.
    pub open_radii: NotchRadii,
    /// The "N Spaces" tab right of the notch.
    pub tab: LogicalRect,
    /// The camera housing (the closed notch without its ears).
    pub notch: LogicalRect,
    /// The open panel's content at real size (the header row flanking the
    /// notch, then the tiles): the shells lay it out at `content_width` by
    /// `content_height` points and scale it by `scale` into `open`.
    pub view: NotchView,
    /// The content's real width.
    pub content_width: f64,
    /// The content's real height.
    pub content_height: f64,
    /// The real notch's height (the header row's height, real points).
    pub notch_height: f64,
    /// The content's side inset, real points (open top radius + padding).
    pub side: f64,
    /// Real points to stage points.
    pub scale: f64,
}

/// One row of the miniature menu.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MenuPreviewRow {
    /// The core's item.
    pub item: MenuItem,
    /// Its row on the stage.
    pub frame: LogicalRect,
}

/// The menu bar miniature: the status item and its menu.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MenuPreview {
    /// The Cua Spaces icon (the tray template).
    pub icon: LogicalRect,
    /// The status item's highlight while pressed or open.
    pub highlight: LogicalRect,
    /// The clock at the menu bar's right end.
    pub clock: String,
    /// Its right edge.
    pub clock_right: f64,
    /// The menu.
    pub menu: LogicalRect,
    /// Its corner radius.
    pub radius: f64,
    /// The rows, top to bottom ([`window::menu_bar`] for [`SPACES`]).
    pub rows: Vec<MenuPreviewRow>,
    /// Text and shortcut inset from the menu's edges.
    pub inset: f64,
    /// The rows' font size.
    pub font_size: f64,
}

/// One card's miniature.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresentationPreview {
    /// The menu bar card (else the notch card).
    pub menu_bar: bool,
    /// Stage width.
    pub width: f64,
    /// Stage height.
    pub height: f64,
    /// The menu bar strip along the top.
    pub menu_bar_height: f64,
    /// One loop (ms).
    pub loop_ms: u32,
    /// The pointer's outline, tip at the origin (fill black, stroke white).
    pub pointer: Vec<PreviewPoint>,
    /// The notch card's miniature.
    pub notch: Option<NotchPreview>,
    /// The menu bar card's miniature.
    pub menu: Option<MenuPreview>,
}

/// One moment of a preview.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewFrame {
    /// The pointer's tip.
    pub pointer: PreviewPoint,
    /// The pointer presses (drawn a little smaller).
    pub pressed: bool,
    /// Notch: the hover cue, 0 to 1 (the closed notch and its tab grow by
    /// 1 + (hover_scale - 1) * hover horizontally, about the top centre).
    pub hover: f64,
    /// Notch: the shape, closed 0 to open 1 (a spring: it can pass 1);
    /// menu: the menu's opacity.
    pub open: f64,
    /// Notch: the content's fade, 0 to 1 (it also grows from
    /// `content_scale`, anchored at the top); menu: the same as `open`.
    pub content: f64,
    /// Notch: the tab, 1 shown to 0 tucked under the notch.
    pub tab: f64,
    /// Menu: the row under the pointer (an enabled item), highlighted.
    pub highlighted: Option<u32>,
    /// Menu: the status item is highlighted (pressed, or its menu open).
    pub active: bool,
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> LogicalRect {
    LogicalRect::new(x, y, w, h)
}

fn scaled(r: NotchRadii, s: f64) -> NotchRadii {
    NotchRadii {
        top: r.top * s,
        bottom: r.bottom * s,
    }
}

fn sample_space(name: &str, os: SpaceOs, os_name: &str, scene: ThumbnailScene) -> Space {
    Space {
        id: format!("local:{name}"),
        name: name.into(),
        os,
        status: SpaceStatus::Running,
        detail: String::new(),
        last_used_at: 0,
        started_at: None,
        scene,
        fleet_id: None,
        size: None,
        region: None,
        provider: Some(SpaceProvider::Local),
        sdk: None,
        os_name: Some(os_name.into()),
        os_pretty_name: None,
        image: None,
        image_digest: None,
        progress: None,
        kind: None,
        arch: None,
        host: None,
        host_name: None,
        power: None,
        cloud: None,
        cloud_place: None,
        cloud_delete: None,
    }
}

/// The Spaces the notch preview shows.
pub fn sample_spaces() -> Vec<Space> {
    vec![
        sample_space(
            "macos-qa",
            SpaceOs::Macos,
            "macOS",
            ThumbnailScene::MacDesktop,
        ),
        sample_space(
            "windows-11",
            SpaceOs::Windows,
            "Windows",
            ThumbnailScene::WindowsDesktop,
        ),
        sample_space(
            "ubuntu-ci",
            SpaceOs::Linux,
            "Ubuntu",
            ThumbnailScene::LinuxTerminal,
        ),
    ]
}

fn menu_bar_height() -> f64 {
    NOTCH_REAL.1 * NOTCH_SCALE
}

fn notch_preview() -> NotchPreview {
    let s = NOTCH_SCALE;
    let (nw, nh) = NOTCH_REAL;
    let closed_radii = scaled(CLOSED_RADII, s);
    let open_radii = scaled(OPEN_RADII, s);
    let side = OPEN_RADII.top + CONTENT_PADDING;
    let tiles = SPACES as f64;
    let content_width = 2.0 * side + tiles * TILE_WIDTH + (tiles - 1.0) * TILE_GAP;
    let content_height = nh + CONTENT_PADDING + TILE_ROW_HEIGHT + CONTENT_PADDING;
    let mid = STAGE_WIDTH / 2.0;
    let closed_w = nw * s + 2.0 * closed_radii.top;
    let open_w = content_width * s;
    let open_state = NotchState {
        open: true,
        ..NotchState::default()
    };
    NotchPreview {
        closed: rect(mid - closed_w / 2.0, 0.0, closed_w, nh * s),
        open: rect(mid - open_w / 2.0, 0.0, open_w, content_height * s),
        closed_radii,
        open_radii,
        tab: rect(mid + nw * s / 2.0, 0.0, TAB_REAL * s, nh * s),
        notch: rect(mid - nw * s / 2.0, 0.0, nw * s, nh * s),
        view: notch::view(&open_state, &sample_spaces()),
        content_width,
        content_height,
        notch_height: nh,
        side,
        scale: s,
    }
}

fn menu_preview() -> MenuPreview {
    let m = MENU_SCALE;
    let bar = menu_bar_height();
    let clock_right = STAGE_WIDTH - 8.0;
    let icon_size = 9.0;
    // The clock ("9:41" at ~6.5 pt) takes about 16 pt, then 8 pt of space.
    let icon = rect(
        clock_right - 16.0 - 8.0 - icon_size,
        (bar - icon_size) / 2.0,
        icon_size,
        icon_size,
    );
    let highlight = rect(icon.x - 4.0, 1.5, icon_size + 8.0, bar - 3.0);
    let width = MENU_WIDTH_REAL * m;
    // A menu opens under its item's left edge, kept on screen.
    let x = highlight.x.min(STAGE_WIDTH - 4.0 - width);
    let y = bar + 2.0;
    let mut rows = Vec::new();
    let mut top = y + MENU_PADDING_REAL * m;
    for item in window::menu_bar(SPACES) {
        let h = if item.id == MenuItemId::Separator {
            MENU_SEPARATOR_REAL
        } else {
            MENU_ROW_REAL
        } * m;
        rows.push(MenuPreviewRow {
            item,
            frame: rect(x, top, width, h),
        });
        top += h;
    }
    let height = top + MENU_PADDING_REAL * m - y;
    MenuPreview {
        icon,
        highlight,
        clock: "9:41".into(),
        clock_right,
        menu: rect(x, y, width, height),
        radius: MENU_RADIUS_REAL * m,
        rows,
        inset: MENU_INSET_REAL * m,
        font_size: 13.0 * m,
    }
}

/// A card's stage height: its tallest state (the open notch panel, the
/// open menu) plus [`STAGE_MARGIN`], in whole points.
pub fn stage_height(menu_bar: bool) -> f64 {
    let bottom = if menu_bar {
        let m = menu_preview().menu;
        m.y + m.height
    } else {
        let n = notch_preview().open;
        n.y + n.height
    };
    (bottom + STAGE_MARGIN).ceil()
}

/// A card's miniature.
pub fn preview(menu_bar: bool) -> PresentationPreview {
    PresentationPreview {
        menu_bar,
        width: STAGE_WIDTH,
        height: stage_height(menu_bar),
        menu_bar_height: menu_bar_height(),
        loop_ms: LOOP_MS,
        pointer: ARROW
            .iter()
            .map(|&(x, y)| PreviewPoint {
                x: x * POINTER_SCALE,
                y: y * POINTER_SCALE,
            })
            .collect(),
        notch: (!menu_bar).then(notch_preview),
        menu: menu_bar.then(menu_preview),
    }
}

// ---- Timing ------------------------------------------------------------------

/// Progress of `t` through `[from, to]`, clamped to 0..1.
fn span(t: f64, from: f64, to: f64) -> f64 {
    ((t - from) / (to - from)).clamp(0.0, 1.0)
}

/// Cubic ease in and out.
fn ease(p: f64) -> f64 {
    if p < 0.5 {
        4.0 * p * p * p
    } else {
        1.0 - (-2.0 * p + 2.0).powi(3) / 2.0
    }
}

/// A spring from 0 toward 1, `elapsed` seconds in (SwiftUI's
/// `.spring(response:dampingFraction:)`: the response is the undamped
/// period).
fn spring(elapsed: f64, response: f64, damping: f64) -> f64 {
    if elapsed <= 0.0 {
        return 0.0;
    }
    let w = 2.0 * std::f64::consts::PI / response;
    if damping >= 1.0 {
        1.0 - (1.0 + w * elapsed) * (-w * elapsed).exp()
    } else {
        let wd = w * (1.0 - damping * damping).sqrt();
        1.0 - (-damping * w * elapsed).exp()
            * ((wd * elapsed).cos() + damping * w / wd * (wd * elapsed).sin())
    }
}

fn glide(t: f64, from: f64, to: f64, a: PreviewPoint, b: PreviewPoint) -> PreviewPoint {
    let p = ease(span(t, from, to));
    PreviewPoint {
        x: a.x + (b.x - a.x) * p,
        y: a.y + (b.y - a.y) * p,
    }
}

// The notch beats (ms).
const N_GLIDE_IN: (f64, f64) = (400.0, 1300.0);
const N_OPEN: f64 = N_GLIDE_IN.1 + MOTION.hover_dwell_ms as f64;
const N_GLIDE_OUT: (f64, f64) = (2700.0, 3500.0);
// The pointer leaves the panel partway out; the close delay runs.
const N_CONTENT_OUT: f64 = 3100.0;
const N_CLOSE: f64 = N_CONTENT_OUT + MOTION.content_out * 1000.0;

fn notch_frame(t: f64) -> PreviewFrame {
    let n = notch_preview();
    let start = PreviewPoint {
        x: STAGE_WIDTH * 0.74,
        y: stage_height(false) * 0.8,
    };
    let at = PreviewPoint {
        x: STAGE_WIDTH / 2.0 + 4.0,
        y: n.notch.height * 0.55,
    };
    let pointer = if t < N_GLIDE_OUT.0 {
        glide(t, N_GLIDE_IN.0, N_GLIDE_IN.1, start, at)
    } else {
        glide(t, N_GLIDE_OUT.0, N_GLIDE_OUT.1, at, start)
    };
    let secs = |from: f64| (t - from) / 1000.0;
    let opened = spring(secs(N_OPEN), MOTION.open_response, MOTION.open_damping);
    let open = if t < N_CLOSE {
        opened
    } else {
        let at_close = spring(
            (N_CLOSE - N_OPEN) / 1000.0,
            MOTION.open_response,
            MOTION.open_damping,
        );
        at_close * (1.0 - spring(secs(N_CLOSE), MOTION.close_response, MOTION.close_damping))
    };
    let cue = if t < N_GLIDE_IN.1 {
        0.0
    } else {
        spring(
            secs(N_GLIDE_IN.1),
            MOTION.hover_response,
            MOTION.hover_damping,
        )
    };
    let hover = if t < N_OPEN {
        cue
    } else {
        cue * (1.0 - opened).max(0.0)
    };
    let content_in = N_OPEN + MOTION.content_delay_ms as f64;
    let content = if t < N_CONTENT_OUT {
        ease(span(t, content_in, content_in + MOTION.content_in * 1000.0))
    } else {
        1.0 - span(t, N_CONTENT_OUT, N_CLOSE)
    };
    PreviewFrame {
        pointer,
        pressed: false,
        hover,
        open,
        content,
        tab: 1.0 - open.clamp(0.0, 1.0),
        highlighted: None,
        active: false,
    }
}

// The menu beats (ms).
const M_GLIDE_IN: (f64, f64) = (400.0, 1300.0);
const M_PRESS: (f64, f64) = (1300.0, 1450.0);
const M_FADE_IN: (f64, f64) = (1450.0, 1530.0);
const M_TO_ROW: (f64, f64) = (1750.0, 2250.0);
const M_CLICK: (f64, f64) = (2700.0, 2850.0);
const M_FADE_OUT: (f64, f64) = (2850.0, 3050.0);
const M_GLIDE_OUT: (f64, f64) = (3100.0, 3800.0);
/// The row the pointer picks: "Open Cua Spaces".
const M_ROW: MenuItemId = MenuItemId::Open;

fn menu_frame(t: f64) -> PreviewFrame {
    let m = menu_preview();
    let start = PreviewPoint {
        x: STAGE_WIDTH * 0.36,
        y: stage_height(true) * 0.82,
    };
    let icon = PreviewPoint {
        x: m.icon.x + m.icon.width / 2.0,
        y: m.icon.y + m.icon.height / 2.0 + 1.0,
    };
    let row = m
        .rows
        .iter()
        .find(|r| r.item.id == M_ROW)
        .map(|r| r.frame)
        .unwrap_or(m.menu);
    let on_row = PreviewPoint {
        x: row.x + row.width * 0.42,
        y: row.y + row.height * 0.6,
    };
    let pointer = if t < M_TO_ROW.0 {
        glide(t, M_GLIDE_IN.0, M_GLIDE_IN.1, start, icon)
    } else if t < M_GLIDE_OUT.0 {
        glide(t, M_TO_ROW.0, M_TO_ROW.1, icon, on_row)
    } else {
        glide(t, M_GLIDE_OUT.0, M_GLIDE_OUT.1, on_row, start)
    };
    let open = if t < M_FADE_OUT.0 {
        span(t, M_FADE_IN.0, M_FADE_IN.1)
    } else {
        1.0 - span(t, M_FADE_OUT.0, M_FADE_OUT.1)
    };
    let pressed = (M_PRESS.0..M_PRESS.1).contains(&t) || (M_CLICK.0..M_CLICK.1).contains(&t);
    let highlighted = (open > 0.0)
        .then(|| {
            m.rows.iter().position(|r| {
                r.item.enabled
                    && r.item.id != MenuItemId::Separator
                    && pointer.x >= r.frame.x
                    && pointer.x < r.frame.x + r.frame.width
                    && pointer.y >= r.frame.y
                    && pointer.y < r.frame.y + r.frame.height
            })
        })
        .flatten()
        .map(|i| i as u32);
    PreviewFrame {
        pointer,
        pressed,
        hover: 0.0,
        open,
        content: open,
        tab: 0.0,
        highlighted,
        active: t >= M_PRESS.0 && t < M_FADE_OUT.1,
    }
}

/// A card's miniature at `t_ms` into its loop (any `t_ms`: it wraps).
pub fn frame(menu_bar: bool, t_ms: u32) -> PreviewFrame {
    let t = (t_ms % LOOP_MS) as f64;
    if menu_bar {
        menu_frame(t)
    } else {
        notch_frame(t)
    }
}

/// The Reduce Motion picture: expanded (the notch open, the menu down),
/// the pointer resting where it points, no loop.
pub fn still(menu_bar: bool) -> PreviewFrame {
    let f = frame(menu_bar, STILL_MS);
    // Settled exactly (the springs are within a hair of it by then).
    if menu_bar {
        f
    } else {
        PreviewFrame {
            open: 1.0,
            content: 1.0,
            tab: 0.0,
            hover: 0.0,
            ..f
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn both_miniatures_fit_the_stage() {
        for mb in [false, true] {
            let p = preview(mb);
            assert_eq!((p.width, p.height), (STAGE_WIDTH, stage_height(mb)));
            let rects: Vec<LogicalRect> = match (&p.notch, &p.menu) {
                (Some(n), None) => vec![n.closed, n.open, n.tab, n.notch],
                (None, Some(m)) => {
                    let mut r = vec![m.icon, m.highlight, m.menu];
                    r.extend(m.rows.iter().map(|r| r.frame));
                    r
                }
                _ => panic!("one miniature per card"),
            };
            // The tallest state fills the stage, a small margin below it.
            let bottom = rects.iter().map(|r| r.y + r.height).fold(0.0, f64::max);
            assert!(
                p.height - bottom <= 16.0,
                "{} below the content",
                p.height - bottom
            );
            // The pointer stays on the stage all loop.
            for t in (0..LOOP_MS).step_by(20) {
                let f = frame(mb, t).pointer;
                assert!(
                    f.x >= 0.0 && f.x + 9.0 <= p.width && f.y >= 0.0 && f.y + 14.0 <= p.height,
                    "{t}: {f:?}"
                );
            }
            for r in rects {
                assert!(r.x >= 0.0 && r.y >= 0.0, "{r:?}");
                assert!(
                    r.x + r.width <= p.width && r.y + r.height <= p.height,
                    "{r:?}"
                );
            }
        }
    }

    #[test]
    fn the_notch_shows_the_real_panel() {
        let n = preview(false).notch.unwrap();
        assert_eq!(n.view.tiles.len(), SPACES as usize);
        assert_eq!(n.view.tab.count, "3");
        let header = n.view.header.expect("the search and the buttons");
        assert_eq!(header.placeholder, "Search");
        assert_eq!(header.buttons.len(), 2);
        assert!(close(n.open.width, n.content_width * n.scale));
        assert!(close(n.open.x + n.open.width / 2.0, STAGE_WIDTH / 2.0));
    }

    #[test]
    fn the_menu_is_the_real_menu() {
        let m = preview(true).menu.unwrap();
        let items: Vec<MenuItem> = m.rows.iter().map(|r| r.item.clone()).collect();
        assert_eq!(items, window::menu_bar(SPACES));
        assert_eq!(m.rows[0].item.label, "3 Spaces");
        // Rows stack without gaps inside the menu.
        for pair in m.rows.windows(2) {
            assert!(close(
                pair[0].frame.y + pair[0].frame.height,
                pair[1].frame.y
            ));
        }
    }

    #[test]
    fn the_notch_loop_glides_dwells_opens_holds_and_closes() {
        let f = |t| frame(false, t);
        let s = f(0);
        assert_eq!((s.open, s.content, s.tab, s.hover), (0.0, 0.0, 1.0, 0.0));
        // Gliding up: no cue yet.
        let g = f(850);
        assert!(g.pointer.y < s.pointer.y && g.hover == 0.0 && g.open == 0.0);
        // The dwell: the notch grows a little, still closed.
        let d = f(1550);
        assert!(d.hover > 0.5 && d.open == 0.0 && d.tab == 1.0);
        // Opening: the spring overshoots, the content fades in after it.
        let peak = (1600..2400).map(f).map(|x| x.open).fold(0.0, f64::max);
        assert!(peak > 1.0 && peak < 1.1, "{peak}");
        assert!(f(1650).content == 0.0 && f(1650).open > 0.0);
        // Held open, settled.
        let h = f(STILL_MS);
        assert!(close(h.open, 1.0) && h.content == 1.0 && close(h.tab, 0.0) && close(h.hover, 0.0));
        // Closing, then closed well before the loop ends.
        assert!(f(3150).content < 1.0);
        assert!(f(3900).open.abs() < 0.01 && f(3900).tab > 0.99);
        // The loop is seamless.
        assert_eq!(f(3999).pointer, f(0).pointer);
        assert_eq!(f(LOOP_MS + 850), g);
    }

    #[test]
    fn the_menu_loop_clicks_opens_highlights_and_closes() {
        let m = preview(true).menu.unwrap();
        let open_row = m.rows.iter().position(|r| r.item.id == MenuItemId::Open);
        let f = |t| frame(true, t);
        assert_eq!((f(0).open, f(0).active, f(0).pressed), (0.0, false, false));
        let press = f(1350);
        assert!(press.pressed && press.active && press.open == 0.0);
        let p = press.pointer;
        assert!(
            p.x >= m.icon.x && p.x <= m.icon.x + m.icon.width,
            "on the icon"
        );
        assert_eq!(f(1600).open, 1.0);
        assert_eq!(f(1600).highlighted, None, "still on the icon");
        let held = f(STILL_MS);
        assert_eq!(held.highlighted.map(|i| i as usize), open_row);
        assert!(!held.pressed && held.active);
        assert!(f(2750).pressed);
        assert_eq!(f(3100).open, 0.0);
        assert!(!f(3100).active);
        assert_eq!(f(3999).pointer, f(0).pointer);
        // Never highlights the status line or a separator.
        for t in (0..LOOP_MS).step_by(10) {
            if let Some(i) = f(t).highlighted {
                assert!(m.rows[i as usize].item.enabled);
            }
        }
    }

    #[test]
    fn reduce_motion_shows_the_expanded_state() {
        let n = still(false);
        assert_eq!((n.open, n.content, n.tab, n.hover), (1.0, 1.0, 0.0, 0.0));
        assert!(!n.pressed);
        let m = still(true);
        assert_eq!(m.open, 1.0);
        assert!(m.highlighted.is_some() && !m.pressed);
    }
}
