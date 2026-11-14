// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Text layer for windows whose content is missing from the accessibility
//! tree.
//!
//! Toolkits that draw their own widgets (GTK on macOS, many Java, Qt-on-canvas
//! and game UIs) expose a window frame, its title-bar buttons and the menu
//! bar, and nothing for the content: no rows, no cells, no labels. A model
//! reading such a window gets an empty tree, a `query` that matches nothing,
//! and pixel actions it can only check by reading the next screenshot.
//!
//! This module turns text recognized in the window's own screenshot into
//! tree rows, grouped into visual lines:
//!
//! ```text
//! - OCR "window content is not in the accessibility tree; ..."
//!   - OCRRow "y=165" = "Old Canal Packaging @(232,165) | c @(846,165) | 73.10 @(1012,165)"
//! ```
//!
//! Coordinates are the centre of each fragment in the delivered screenshot's
//! pixels, the space pixel actions take. Each row's label is its `y`, so a
//! `since` diff read reports a changed cell as a `~` row instead of a removed
//! row plus an added one. The platform backend does the recognition; this
//! module only decides when a window is opaque and how the text renders.

/// One recognized piece of text, in screenshot pixels (top-left origin).
#[derive(Clone, Debug, PartialEq)]
pub struct OcrFragment {
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub confidence: f32,
}

impl OcrFragment {
    fn center(&self) -> (f64, f64) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }
}

/// Fragments below this recognizer confidence are dropped.
pub const MIN_CONFIDENCE: f32 = 0.3;
/// At most this many rows are rendered (top to bottom).
pub const MAX_ROWS: usize = 150;
/// Role of the header row; also how a reader recognizes an OCR block.
pub const HEADER_ROLE: &str = "OCR";
/// Role of each text row.
pub const ROW_ROLE: &str = "OCRRow";

/// Header row text. One line, so it never shows up in a diff read.
pub const HEADER_TEXT: &str = "window content is not in the accessibility tree; rows below are \
text read from this window's screenshot (text + @(x,y)). Act by those pixels; each action's result \
lists the rows that changed and any new window, so no screenshot is needed to check it \
(a row's ink= changes with its pixels, also where no text was read)";

/// Block shown while the recognizer is still loading (first use after boot).
pub const PENDING_BLOCK: &str =
    "- OCR \"window content is not in the accessibility tree; reading its \
text from the screenshot is still starting: read the window again in a few seconds\"\n";

/// Block shown when the window's text could not be read this time.
pub fn failed_block(reason: &str) -> String {
    let reason = reason.replace(['"', '\n'], " ");
    format!(
        "- {HEADER_ROLE} \"window content is not in the accessibility tree; reading its text \
from the screenshot failed: {reason}\"\n"
    )
}

/// A visual line: fragments whose vertical centres overlap, left to right.
#[derive(Clone, Debug, PartialEq)]
pub struct OcrRow {
    pub y: f64,
    pub fragments: Vec<OcrFragment>,
    /// Fingerprint of the row's ink pixels (see [`stamp_ink`]).
    pub ink: Option<u16>,
}

/// What the classifier needs to know about one walked accessibility node.
#[derive(Clone, Copy, Debug)]
pub struct NodeFacts<'a> {
    /// Depth in the rendered tree; 0 for top-level elements (windows, the menu bar).
    pub depth: usize,
    pub role: &'a str,
    pub subrole: Option<&'a str>,
    /// The node's first non-empty title, value or description, if any.
    pub text: Option<&'a str>,
}

/// Title-bar buttons every window has, whatever draws its content.
const FRAME_SUBROLES: &[&str] = &[
    "AXCloseButton",
    "AXMinimizeButton",
    "AXZoomButton",
    "AXFullScreenButton",
    "AXToolbarButton",
];

/// Top-level roles whose subtree is the window's content.
const WINDOW_ROLES: &[&str] = &["AXWindow", "AXSheet", "AXDrawer"];

/// Whether a window's content is missing from the accessibility tree.
///
/// `None` when the walk did not cover the window (no window row, or the walk
/// stopped early): the caller should reuse an earlier verdict, not guess.
/// `Some(true)` when the window's subtree holds nothing but its frame (the
/// title-bar buttons and the static text that repeats the window's title),
/// plus at most two unlabeled containers.
pub fn content_is_opaque<'a>(
    nodes: impl IntoIterator<Item = NodeFacts<'a>>,
    walk_complete: bool,
) -> Option<bool> {
    if !walk_complete {
        return None;
    }
    let mut saw_window = false;
    let mut in_window = false;
    let mut window_title = None;
    let mut content = 0usize;
    for node in nodes {
        let text = node.text.map(str::trim).filter(|text| !text.is_empty());
        if node.depth == 0 {
            in_window = WINDOW_ROLES.contains(&node.role);
            saw_window |= in_window;
            window_title = text;
            continue;
        }
        if !in_window {
            continue;
        }
        if node
            .subrole
            .is_some_and(|subrole| FRAME_SUBROLES.contains(&subrole))
        {
            continue;
        }
        // The title bar's own label (a static text child repeating the
        // window's title, as GTK windows expose it) is frame, not content.
        if node.depth == 1 && node.role == "AXStaticText" && text.is_some() && text == window_title
        {
            continue;
        }
        if text.is_some() {
            return Some(false);
        }
        content += 1;
        if content > 2 {
            return Some(false);
        }
    }
    saw_window.then_some(true)
}

/// Group fragments into visual lines, top to bottom, each left to right.
pub fn group_rows(fragments: impl IntoIterator<Item = OcrFragment>) -> Vec<OcrRow> {
    let mut frags: Vec<OcrFragment> = fragments
        .into_iter()
        .filter(|f| f.confidence >= MIN_CONFIDENCE && !f.text.trim().is_empty())
        .collect();
    frags.sort_by(|a, b| a.center().1.total_cmp(&b.center().1));
    let mut rows: Vec<(f64, f64, Vec<OcrFragment>)> = Vec::new(); // (centre y, height, fragments)
    for frag in frags {
        let (_, cy) = frag.center();
        match rows.last_mut() {
            Some((row_y, row_h, members))
                if (cy - *row_y).abs() <= 0.5 * row_h.min(frag.height).max(1.0) =>
            {
                let n = members.len() as f64;
                *row_y = (*row_y * n + cy) / (n + 1.0);
                *row_h = row_h.max(frag.height);
                members.push(frag);
            }
            _ => rows.push((cy, frag.height, vec![frag])),
        }
    }
    rows.into_iter()
        .map(|(y, _, mut fragments)| {
            fragments.sort_by(|a, b| a.x.total_cmp(&b.x));
            OcrRow {
                y,
                fragments,
                ink: None,
            }
        })
        .collect()
}

fn clean(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('"', "'")
}

/// One row as a tree line (without indentation).
pub fn render_row(row: &OcrRow) -> String {
    let mut line = format!(
        "- {ROW_ROLE} \"y={}\" = \"{}\"",
        row.y.round() as i64,
        row_cells(row)
    );
    if let Some(ink) = row.ink {
        line.push_str(&format!(" ink={ink:04x}"));
    }
    line
}

/// Scanlines searched for ink above and below a row's recognized boxes.
const BAND_SLACK: i64 = 2;

/// A pixel this far from the row's mean luminance counts as ink.
const INK_CONTRAST: i16 = 64;

/// Fingerprint each row's band of the screenshot: which pixels are ink
/// (far from the row's mean luminance, so light and dark themes both work).
///
/// Text recognition misses some glyphs (a lone `n` or `c` in a narrow cell)
/// and a missed glyph that changes would leave the row's text identical. The
/// fingerprint changes with any ink pixel in the band, so a `since` diff
/// still reports the row. It also snaps the row's `y` to the band's centre. `luma` is the screenshot's luminance plane,
/// `width` x `height`, in the same pixels as the fragments.
pub fn stamp_ink(rows: &mut [OcrRow], luma: &[u8], width: u32, height: u32) {
    let (width, height) = (width as usize, height as usize);
    if width == 0 || luma.len() < width * height {
        return;
    }
    for row in rows.iter_mut() {
        let top = row
            .fragments
            .iter()
            .map(|f| f.y)
            .fold(f64::INFINITY, f64::min);
        let bottom = row
            .fragments
            .iter()
            .map(|f| f.y + f.height)
            .fold(f64::NEG_INFINITY, f64::max);
        if !top.is_finite() || !bottom.is_finite() {
            continue;
        }
        // The band runs from the first to the last inked scanline near the
        // row's boxes. Recognized boxes shift by a pixel between reads of the
        // same pixels; the ink itself does not, so a band taken from it keeps
        // the fingerprint (and the row's y label) still until the row changes.
        let lo = (top.floor() as i64 - BAND_SLACK).clamp(0, height as i64) as usize;
        let hi = (bottom.ceil() as i64 + BAND_SLACK).clamp(0, height as i64) as usize;
        if lo >= hi {
            continue;
        }
        // Ink is what stands out from the row's background, light or dark.
        let window = &luma[lo * width..hi * width];
        let background =
            (window.iter().map(|&l| u64::from(l)).sum::<u64>() / window.len() as u64) as i16;
        let is_ink = |l: u8| (i16::from(l) - background).abs() > INK_CONTRAST;
        let has_ink = |y: usize| luma[y * width..(y + 1) * width].iter().any(|&l| is_ink(l));
        let (Some(y0), Some(last)) = (
            (lo..hi).find(|&y| has_ink(y)),
            (lo..hi).rev().find(|&y| has_ink(y)),
        ) else {
            row.ink = Some(0);
            continue;
        };
        let y1 = last + 1;
        row.y = (y0 + last) as f64 / 2.0;
        // FNV-1a over the band's ink mask, one byte per 8 pixels.
        let mut hash: u32 = 0x811c_9dc5;
        for line in luma[y0 * width..y1 * width].chunks(width) {
            for chunk in line.chunks(8) {
                let byte = chunk
                    .iter()
                    .enumerate()
                    .fold(0u8, |acc, (bit, &l)| acc | (u8::from(is_ink(l)) << bit));
                hash ^= u32::from(byte);
                hash = hash.wrapping_mul(0x0100_0193);
            }
        }
        row.ink = Some((hash ^ (hash >> 16)) as u16);
    }
}

/// The OCR block appended to a window's tree markdown: a header row and one
/// indented row per visual line. With a `query`, only rows containing it
/// (case-insensitive) are kept, and nothing renders when none match.
pub fn render_block(rows: &[OcrRow], query: Option<&str>) -> String {
    let needle = query
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .map(str::to_lowercase);
    let lines: Vec<String> = rows
        .iter()
        .take(MAX_ROWS)
        .map(render_row)
        .filter(|line| {
            needle
                .as_deref()
                .is_none_or(|needle| line.to_lowercase().contains(needle))
        })
        .collect();
    if lines.is_empty() {
        return String::new();
    }
    let mut out = format!("- {HEADER_ROLE} \"{HEADER_TEXT}\"\n");
    for line in &lines {
        out.push_str("  ");
        out.push_str(line);
        out.push('\n');
    }
    if rows.len() > MAX_ROWS {
        out.push_str(&format!(
            "  - {ROW_ROLE} \"more\" = \"{} more rows below; zoom or narrow with query\"\n",
            rows.len() - MAX_ROWS
        ));
    }
    out
}

/// Insert `block` into `markdown` after the tree rows and before any trailing
/// walk note (a line that is not a `- ` row, such as the partial-tree note).
pub fn insert_block(markdown: &str, block: &str) -> String {
    if block.is_empty() {
        return markdown.to_owned();
    }
    let lines: Vec<&str> = markdown.lines().collect();
    let tail_start = lines
        .iter()
        .rposition(|line| line.trim_start().starts_with("- "))
        .map_or(0, |last_row| last_row + 1);
    let mut out = String::new();
    for line in &lines[..tail_start] {
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(block);
    for line in &lines[tail_start..] {
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// At most this many changed rows are listed in an action's result.
pub const MAX_CHANGES: usize = 8;

/// Rows whose `y` labels are this close are the same row across two reads.
const SAME_ROW_SLACK: i64 = 3;

/// A row that differs between two reads of the same window.
#[derive(Clone, Debug, PartialEq)]
pub struct RowChange {
    /// The row's `y` label (the later read's, or the earlier one's when the
    /// row is gone).
    pub y: i64,
    /// The row's text before; `None` when the row is new.
    pub before: Option<String>,
    /// The row as rendered cells (`text @(x,y) | ...`) after; `None` when
    /// the row is gone.
    pub after: Option<String>,
    /// Same recognized text, different pixels (a glyph the recognizer
    /// missed, a toggled mark, a highlight).
    pub ink_only: bool,
}

/// A row's recognized text, cells joined with ` | `.
pub fn row_text(row: &OcrRow) -> String {
    row.fragments
        .iter()
        .map(|f| clean(&f.text))
        .collect::<Vec<_>>()
        .join(" | ")
}

/// A row's cells with their pixel centres, as [`render_row`] shows them.
pub fn row_cells(row: &OcrRow) -> String {
    row.fragments
        .iter()
        .map(|f| {
            let (cx, cy) = f.center();
            format!(
                "{} @({},{})",
                clean(&f.text),
                cx.round() as i64,
                cy.round() as i64
            )
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

/// The rows that differ between two reads of one window taken in the same
/// pixel frame, top to bottom. Rows pair by their `y` label (within a few
/// pixels), the way a `since` read pairs them; a pair differs when its text
/// or its ink fingerprint does.
pub fn diff_rows(before: &[OcrRow], after: &[OcrRow]) -> Vec<RowChange> {
    let y = |row: &OcrRow| row.y.round() as i64;
    let mut used = vec![false; before.len()];
    let mut changes = Vec::new();
    for row in after {
        let ry = y(row);
        let pair = before
            .iter()
            .enumerate()
            .filter(|(i, old)| !used[*i] && (y(old) - ry).abs() <= SAME_ROW_SLACK)
            .min_by_key(|(_, old)| (y(old) - ry).abs())
            .map(|(i, _)| i);
        match pair {
            Some(i) => {
                used[i] = true;
                let old = &before[i];
                let (old_text, new_text) = (row_text(old), row_text(row));
                let ink_changed = old.ink.is_some() && row.ink.is_some() && old.ink != row.ink;
                if old_text != new_text || ink_changed {
                    changes.push(RowChange {
                        y: ry,
                        before: Some(old_text.clone()),
                        after: Some(row_cells(row)),
                        ink_only: old_text == new_text,
                    });
                }
            }
            None => changes.push(RowChange {
                y: ry,
                before: None,
                after: Some(row_cells(row)),
                ink_only: false,
            }),
        }
    }
    for (old, _) in before.iter().zip(&used).filter(|(_, used)| !**used) {
        changes.push(RowChange {
            y: y(old),
            before: Some(row_text(old)),
            after: None,
            ink_only: false,
        });
    }
    changes.sort_by_key(|change| change.y);
    changes
}

/// One changed row as a short line: `y=165: "a | n" -> "a @(10,165) | c @(80,165)"`.
pub fn render_change(change: &RowChange) -> String {
    let quote = |text: &Option<String>| match text {
        Some(text) => format!("\"{text}\""),
        None => "(none)".to_owned(),
    };
    let mut line = format!(
        "y={}: {} -> {}",
        change.y,
        quote(&change.before),
        quote(&change.after)
    );
    if change.ink_only {
        line.push_str(" (same text, pixels changed)");
    }
    line
}

/// The changed rows of an action's result, at most [`MAX_CHANGES`], one per
/// line, with a note when more changed.
pub fn render_changes(changes: &[RowChange]) -> String {
    let mut out: Vec<String> = changes
        .iter()
        .take(MAX_CHANGES)
        .map(render_change)
        .collect();
    if changes.len() > MAX_CHANGES {
        out.push(format!(
            "{} more rows changed; get_window_state since=latest lists them all",
            changes.len() - MAX_CHANGES
        ));
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frag(text: &str, x: f64, y: f64) -> OcrFragment {
        OcrFragment {
            text: text.to_owned(),
            x,
            y,
            width: 40.0,
            height: 14.0,
            confidence: 0.9,
        }
    }

    fn row(y: f64, cells: &[&str], ink: u16) -> OcrRow {
        OcrRow {
            y,
            fragments: cells
                .iter()
                .enumerate()
                .map(|(i, text)| frag(text, 20.0 + 100.0 * i as f64, y - 7.0))
                .collect(),
            ink: Some(ink),
        }
    }

    #[test]
    fn identical_reads_have_no_changes() {
        let rows = [
            row(100.0, &["Old Canal", "n", "73.10"], 1),
            row(130.0, &["Total"], 2),
        ];
        assert!(diff_rows(&rows, &rows).is_empty());
    }

    #[test]
    fn a_changed_cell_is_one_change_with_the_new_pixels() {
        let before = [
            row(100.0, &["Old Canal", "n"], 1),
            row(130.0, &["Total"], 2),
        ];
        let after = [
            row(101.0, &["Old Canal", "c"], 3),
            row(130.0, &["Total"], 2),
        ];
        let changes = diff_rows(&before, &after);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].y, 101);
        assert_eq!(changes[0].before.as_deref(), Some("Old Canal | n"));
        assert_eq!(
            changes[0].after.as_deref(),
            Some("Old Canal @(40,101) | c @(140,101)")
        );
        assert!(!changes[0].ink_only);
        assert_eq!(
            render_changes(&changes),
            "y=101: \"Old Canal | n\" -> \"Old Canal @(40,101) | c @(140,101)\""
        );
    }

    #[test]
    fn a_pixel_only_change_is_flagged() {
        let before = [row(100.0, &["Old Canal"], 1)];
        let after = [row(100.0, &["Old Canal"], 9)];
        let changes = diff_rows(&before, &after);
        assert_eq!(changes.len(), 1);
        assert!(changes[0].ink_only);
        assert!(render_change(&changes[0]).ends_with("(same text, pixels changed)"));
    }

    #[test]
    fn added_and_removed_rows_are_listed_in_order() {
        let before = [row(100.0, &["A"], 1), row(200.0, &["B"], 2)];
        let after = [row(100.0, &["A"], 1), row(150.0, &["New"], 3)];
        let changes = diff_rows(&before, &after);
        assert_eq!(changes.len(), 2);
        assert_eq!((changes[0].y, changes[0].before.as_deref()), (150, None));
        assert_eq!((changes[1].y, changes[1].after.as_deref()), (200, None));
        assert_eq!(render_change(&changes[1]), "y=200: \"B\" -> (none)");
    }

    #[test]
    fn long_change_lists_are_capped() {
        let before: Vec<OcrRow> = (0..20).map(|i| row(30.0 * i as f64, &["x"], 1)).collect();
        let after: Vec<OcrRow> = (0..20).map(|i| row(30.0 * i as f64, &["y"], 1)).collect();
        let text = render_changes(&diff_rows(&before, &after));
        assert_eq!(text.lines().count(), MAX_CHANGES + 1);
        assert!(
            text.ends_with("12 more rows changed; get_window_state since=latest lists them all")
        );
    }

    fn facts(
        depth: usize,
        role: &'static str,
        subrole: Option<&'static str>,
    ) -> NodeFacts<'static> {
        NodeFacts {
            depth,
            role,
            subrole,
            text: None,
        }
    }

    #[test]
    fn a_window_with_only_frame_buttons_is_opaque() {
        let nodes = [
            facts(0, "AXWindow", None),
            facts(1, "AXButton", Some("AXCloseButton")),
            facts(1, "AXButton", Some("AXZoomButton")),
            facts(1, "AXButton", Some("AXMinimizeButton")),
            facts(0, "AXMenuBar", None),
            NodeFacts {
                text: Some("File"),
                ..facts(1, "AXMenuBarItem", None)
            },
        ];
        assert_eq!(content_is_opaque(nodes, true), Some(true));
    }

    #[test]
    fn a_failed_read_is_one_header_row_with_its_reason() {
        let block = failed_block("Vision text recognition failed: \"no model\"\nretry");
        assert_eq!(block.lines().count(), 1);
        assert!(block.starts_with("- OCR \"window content is not in the accessibility tree;"));
        assert!(block.contains("failed: Vision text recognition failed:  no model  retry\""));
        assert!(block.ends_with("\"\n"));
    }

    #[test]
    fn the_title_bar_label_is_frame_not_content() {
        // GnuCash (GTK, quartz backend) as walked in a live read: the window,
        // its title-bar buttons, a static text repeating the window title,
        // then the menu bar.
        let title = "kw.gnucash - Accounts - GnuCash";
        let window = NodeFacts {
            text: Some(title),
            ..facts(0, "AXWindow", Some("AXStandardWindow"))
        };
        let nodes = [
            window,
            facts(1, "AXButton", Some("AXCloseButton")),
            facts(1, "AXButton", Some("AXFullScreenButton")),
            facts(1, "AXButton", Some("AXMinimizeButton")),
            NodeFacts {
                text: Some(title),
                ..facts(1, "AXStaticText", None)
            },
            facts(0, "AXMenuBar", None),
            NodeFacts {
                text: Some("File"),
                ..facts(1, "AXMenuBarItem", None)
            },
        ];
        assert_eq!(content_is_opaque(nodes, true), Some(true));
        // Other text, or the title repeated inside the content, is content.
        let other = [
            window,
            NodeFacts {
                text: Some("Assets"),
                ..facts(1, "AXStaticText", None)
            },
        ];
        assert_eq!(content_is_opaque(other, true), Some(false));
        let nested = [
            window,
            NodeFacts {
                text: Some(title),
                ..facts(2, "AXStaticText", None)
            },
        ];
        assert_eq!(content_is_opaque(nested, true), Some(false));
    }

    #[test]
    fn labeled_content_or_many_rows_is_not_opaque() {
        let labeled = [
            facts(0, "AXWindow", None),
            NodeFacts {
                text: Some("Total"),
                ..facts(1, "AXStaticText", None)
            },
        ];
        assert_eq!(content_is_opaque(labeled, true), Some(false));
        let many = [
            facts(0, "AXWindow", None),
            facts(1, "AXButton", None),
            facts(1, "AXButton", None),
            facts(1, "AXButton", None),
        ];
        assert_eq!(content_is_opaque(many, true), Some(false));
    }

    #[test]
    fn an_incomplete_walk_or_missing_window_has_no_verdict() {
        let nodes = [facts(0, "AXWindow", None)];
        assert_eq!(content_is_opaque(nodes, false), None);
        let menu_only = [facts(0, "AXMenuBar", None)];
        assert_eq!(content_is_opaque(menu_only, true), None);
    }

    #[test]
    fn fragments_group_into_lines_left_to_right() {
        let rows = group_rows([
            frag("73.10", 990.0, 158.0),
            frag("Old Canal Packaging", 170.0, 159.0),
            frag("c", 840.0, 157.0),
            frag("Orchard & Coast Paper", 170.0, 181.0),
            OcrFragment {
                confidence: 0.1,
                ..frag("noise", 500.0, 181.0)
            },
        ]);
        assert_eq!(rows.len(), 2);
        let texts: Vec<&str> = rows[0].fragments.iter().map(|f| f.text.as_str()).collect();
        assert_eq!(texts, ["Old Canal Packaging", "c", "73.10"]);
        assert_eq!(rows[1].fragments.len(), 1);
    }

    #[test]
    fn rows_render_with_centres_and_a_stable_label() {
        let rows = group_rows([frag("Say \"hi\"", 100.0, 200.0), frag("n", 840.0, 200.0)]);
        assert_eq!(
            render_row(&rows[0]),
            "- OCRRow \"y=207\" = \"Say 'hi' @(120,207) | n @(860,207)\""
        );
    }

    #[test]
    fn ink_changes_with_an_unread_glyph_in_the_row() {
        let (w, h) = (100u32, 40u32);
        let mut luma = vec![255u8; (w * h) as usize];
        let mut rows = group_rows([frag("Paid", 0.0, 10.0)]);
        stamp_ink(&mut rows, &luma, w, h);
        let before = rows[0].ink.expect("stamped");
        // A dark glyph appears at x=80 in the row band, where no text was read.
        for y in 14..20 {
            luma[(y * w + 80) as usize] = 0;
        }
        stamp_ink(&mut rows, &luma, w, h);
        assert_ne!(rows[0].ink, Some(before));
        assert!(render_row(&rows[0]).ends_with(&format!(" ink={:04x}", rows[0].ink.unwrap())));
    }

    #[test]
    fn ink_and_label_hold_still_when_only_the_boxes_shift() {
        let (w, h) = (100u32, 40u32);
        let mut luma = vec![255u8; (w * h) as usize];
        for y in 14..22 {
            for x in 10..30 {
                luma[(y * w + x) as usize] = 0;
            }
        }
        let mut a = group_rows([frag("Paid", 8.0, 12.0)]);
        let mut b = group_rows([frag("Paid", 9.0, 13.5)]);
        stamp_ink(&mut a, &luma, w, h);
        stamp_ink(&mut b, &luma, w, h);
        assert_eq!(a[0].ink, b[0].ink);
        assert_eq!(a[0].y, b[0].y);
        assert_eq!(
            render_row(&a[0]).split(" = ").next(),
            Some("- OCRRow \"y=18\"")
        );
    }

    #[test]
    fn query_keeps_matching_rows_and_drops_an_empty_block() {
        let rows = group_rows([
            frag("Checking Account", 60.0, 180.0),
            frag("Savings", 60.0, 220.0),
        ]);
        let block = render_block(&rows, Some("checking"));
        assert!(block.starts_with("- OCR \""), "{block}");
        assert!(block.contains("Checking Account"));
        assert!(!block.contains("Savings"));
        assert_eq!(render_block(&rows, Some("nothing here")), "");
    }

    #[test]
    fn the_block_goes_before_a_trailing_walk_note() {
        let md = "- [0] AXWindow \"w\"\n  - [1] AXButton\n\n⚠️ PARTIAL TREE: note";
        let out = insert_block(md, "- OCR \"h\"\n  - OCRRow \"y=1\" = \"a @(1,1)\"\n");
        let note = out.find("PARTIAL TREE").unwrap();
        let block = out.find("- OCR").unwrap();
        assert!(block < note, "{out}");
        assert!(out.starts_with("- [0] AXWindow"));
    }
}
