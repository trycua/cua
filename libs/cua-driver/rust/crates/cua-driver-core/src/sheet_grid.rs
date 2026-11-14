// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Spreadsheet grids in the accessibility tree.
//!
//! Spreadsheet apps expose their visible cells as children of a table, one
//! element per cell, named by the cell's address (`A1`, `E12`) with the shown
//! text as its value, and often a name box (a combo box whose value is the
//! active cell's address). Read as a plain tree that is one row per cell,
//! hundreds of rows for one screen of a sheet, most of them empty cells. And
//! keyboard input into a sheet (the usual way to fill it) gives no element to
//! read a typed value back from, so a caller re-reads the window or a
//! screenshot after every entry.
//!
//! This module recognizes such a grid from the walked nodes and:
//!
//! - renders its filled cells compactly, one line per sheet row, keeping each
//!   cell's element index (empty cells are left out of the text; their
//!   indices still act on them):
//!
//!   ```text
//!   - grid "A1:H30" (filled cells by row; empty cells not listed, their [index] still acts)
//!     - row "2": [10] A2="GNC-512" | [12] C2="Orchard & Coast Paper" | [16] G2="pending"
//!   ```
//!
//! - compares the grid before and after an action: which cells' values
//!   changed, whether the active cell moved and whether the table itself (the
//!   sheet) changed, so an action's result can say what it did to the sheet.
//!
//! Detection is structural (a table whose descendants carry A1-style
//! addresses), not tied to any application.

use std::collections::{BTreeMap, HashMap};
use std::fmt;

/// Roles a table's cell elements take.
const CELL_ROLES: [&str; 4] = ["AXTextField", "AXCell", "AXStaticText", "AXTextArea"];
/// Roles of a table that may hold address-named cells.
const TABLE_ROLES: [&str; 2] = ["AXTable", "AXGrid"];
/// A table needs at least this many address-named cells to count as a grid.
const MIN_CELLS: usize = 4;
/// Changed cells listed in an action's result; the rest are counted.
pub const MAX_CHANGES: usize = 24;
/// Longest cell text shown in an action's result.
const MAX_VALUE_CHARS: usize = 80;

/// A cell position, ordered row-major (row 1 before row 2, A before B).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CellRef {
    /// 1-based row number.
    pub row: u32,
    /// 1-based column number (A = 1, Z = 26, AA = 27).
    pub col: u32,
}

impl fmt::Display for CellRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut letters = Vec::new();
        let mut col = self.col;
        while col > 0 {
            let rem = (col - 1) % 26;
            letters.push(char::from(b'A' + rem as u8));
            col = (col - 1) / 26;
        }
        letters.reverse();
        let letters: String = letters.into_iter().collect();
        write!(f, "{letters}{}", self.row)
    }
}

/// Parse an A1-style address (`E12`, `$E$12`, `AA3`). Up to three column
/// letters and seven row digits; row 0 and lowercase are not addresses.
pub fn parse_address(text: &str) -> Option<CellRef> {
    let text = text.trim();
    let bytes = text.as_bytes();
    let mut i = 0;
    if bytes.get(i) == Some(&b'$') {
        i += 1;
    }
    let letters_start = i;
    while i < bytes.len() && bytes[i].is_ascii_uppercase() {
        i += 1;
    }
    let letters = &bytes[letters_start..i];
    if letters.is_empty() || letters.len() > 3 {
        return None;
    }
    if bytes.get(i) == Some(&b'$') {
        i += 1;
    }
    let digits = &bytes[i..];
    if digits.is_empty()
        || digits.len() > 7
        || !digits.iter().all(u8::is_ascii_digit)
        || digits[0] == b'0'
    {
        return None;
    }
    let col = letters
        .iter()
        .fold(0u32, |acc, b| acc * 26 + u32::from(b - b'A' + 1));
    let row = std::str::from_utf8(digits).ok()?.parse().ok()?;
    Some(CellRef { row, col })
}

/// What the grid detector needs to know about one walked node, in walk
/// (depth-first) order.
#[derive(Clone, Copy, Debug)]
pub struct NodeFacts<'a> {
    pub depth: usize,
    pub role: &'a str,
    pub title: Option<&'a str>,
    pub description: Option<&'a str>,
    pub value: Option<&'a str>,
    /// The node's element index, when it is addressable.
    pub index: Option<usize>,
    pub selected: Option<bool>,
}

/// One visible cell.
#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    pub value: String,
    pub index: Option<usize>,
}

/// A spreadsheet grid as one read saw it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Grid {
    /// The table's name (often the sheet's name).
    pub table: String,
    /// Visible cells by position.
    pub cells: BTreeMap<CellRef, Cell>,
    /// The active cell, from a name box or the one selected cell.
    pub active: Option<CellRef>,
    /// False when the walk stopped early, so cells may be missing.
    pub complete: bool,
}

impl Grid {
    /// The visible range, top-left to bottom-right (`A1:H30`).
    pub fn range(&self) -> Option<String> {
        let first = self.cells.keys().next()?;
        let (mut min_col, mut max_col) = (first.col, first.col);
        let mut max_row = first.row;
        for at in self.cells.keys() {
            min_col = min_col.min(at.col);
            max_col = max_col.max(at.col);
            max_row = max_row.max(at.row);
        }
        Some(format!(
            "{}:{}",
            CellRef {
                row: first.row,
                col: min_col
            },
            CellRef {
                row: max_row,
                col: max_col
            }
        ))
    }
}

fn cell_address(facts: &NodeFacts<'_>) -> Option<CellRef> {
    if !CELL_ROLES.contains(&facts.role) {
        return None;
    }
    facts
        .title
        .and_then(parse_address)
        .or_else(|| facts.description.and_then(parse_address))
}

/// The grid in `nodes`: the table with the most address-named cells (at
/// least [`MIN_CELLS`]). `complete` is whether the walk saw every node.
pub fn find_grid(nodes: &[NodeFacts<'_>], complete: bool) -> Option<Grid> {
    let mut best: Option<Grid> = None;
    let mut i = 0;
    while i < nodes.len() {
        let table = &nodes[i];
        if !TABLE_ROLES.contains(&table.role) {
            i += 1;
            continue;
        }
        let mut cells = BTreeMap::new();
        let mut selected = Vec::new();
        let mut j = i + 1;
        while j < nodes.len() && nodes[j].depth > table.depth {
            if let Some(at) = cell_address(&nodes[j]) {
                cells.insert(
                    at,
                    Cell {
                        value: nodes[j].value.unwrap_or_default().to_owned(),
                        index: nodes[j].index,
                    },
                );
                if nodes[j].selected == Some(true) {
                    selected.push(at);
                }
            }
            j += 1;
        }
        if cells.len() >= MIN_CELLS && best.as_ref().is_none_or(|b| b.cells.len() < cells.len()) {
            let name = table
                .title
                .filter(|t| !t.trim().is_empty())
                .or(table.description)
                .unwrap_or_default();
            best = Some(Grid {
                table: name.to_owned(),
                cells,
                active: (selected.len() == 1).then(|| selected[0]),
                complete,
            });
        }
        i = j.max(i + 1);
    }
    let mut grid = best?;
    // A name box names the active cell even when the cell is scrolled out of
    // view or no cell reports itself selected.
    if let Some(at) = nodes
        .iter()
        .filter(|n| n.role == "AXComboBox")
        .find_map(|n| n.value.and_then(parse_address))
    {
        grid.active = Some(at);
    }
    Some(grid)
}

/// What an action did to a grid.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GridChange {
    /// `(cell, before, after)` for cells visible in both reads, in row order.
    pub changed: Vec<(CellRef, String, String)>,
    /// The table's name before, when it changed (another sheet is shown).
    pub table_was: Option<String>,
    /// The active cell before, when it moved.
    pub active_was: Option<Option<CellRef>>,
    /// The visible range before, when it changed (the view scrolled).
    pub range_was: Option<Option<String>>,
}

impl GridChange {
    /// Whether the action visibly changed the grid.
    pub fn is_change(&self) -> bool {
        !self.changed.is_empty()
            || self.table_was.is_some()
            || self.active_was.is_some()
            || self.range_was.is_some()
    }
}

/// Compare two reads of a window's grid.
pub fn diff(before: &Grid, after: &Grid) -> GridChange {
    let mut change = GridChange::default();
    if before.table != after.table {
        // Another sheet: its cells are not the same cells.
        change.table_was = Some(before.table.clone());
    } else {
        change.changed = after
            .cells
            .iter()
            .filter_map(|(at, cell)| {
                let old = before.cells.get(at)?;
                (old.value != cell.value).then(|| (*at, old.value.clone(), cell.value.clone()))
            })
            .collect();
    }
    if before.active != after.active {
        change.active_was = Some(before.active);
    }
    if before.range() != after.range() {
        change.range_was = Some(before.range());
    }
    change
}

fn shown(value: &str) -> String {
    let flat = value.replace('\n', "\\n").replace('"', "\\\"");
    if flat.chars().count() > MAX_VALUE_CHARS {
        let cut: String = flat.chars().take(MAX_VALUE_CHARS).collect();
        format!("\"{cut}…\"")
    } else {
        format!("\"{flat}\"")
    }
}

/// The action-result text for `change`, read off `after`.
pub fn render_change(change: &GridChange, after: &Grid) -> String {
    let mut out = format!("Sheet table \"{}\" read back after the action", after.table);
    if let Some(was) = &change.table_was {
        out.push_str(&format!(" (was \"{was}\": another sheet is shown)"));
    }
    out.push_str(": ");
    if change.table_was.is_none() {
        if change.changed.is_empty() {
            out.push_str(
                "no visible cell changed value (a typed entry is stored once committed with \\n, \\t or a move to another cell).",
            );
        } else {
            let listed: Vec<String> = change
                .changed
                .iter()
                .take(MAX_CHANGES)
                .map(|(at, old, new)| format!("{at} {} -> {}", shown(old), shown(new)))
                .collect();
            out.push_str(&format!(
                "{} cell(s) changed: {}",
                change.changed.len(),
                listed.join(", ")
            ));
            if change.changed.len() > MAX_CHANGES {
                out.push_str(&format!(" (+{} more)", change.changed.len() - MAX_CHANGES));
            }
            out.push('.');
        }
    }
    if let Some(at) = after.active {
        out.push_str(&format!(" Active cell {at}"));
        if let Some(Some(was)) = change.active_was {
            out.push_str(&format!(" (was {was})"));
        }
        match after.cells.get(&at) {
            Some(cell) if cell.value.is_empty() => out.push_str(", empty."),
            Some(cell) => out.push_str(&format!(" = {}.", shown(&cell.value))),
            None => out.push('.'),
        }
    }
    if let Some(range) = after.range().filter(|_| change.range_was.is_some()) {
        out.push_str(&format!(" Visible cells now {range}."));
    }
    out
}

/// Rewrite the grid's cell rows in `markdown` compactly: one `row` line per
/// sheet row with its filled cells as `[index] A1="value"` and runs of empty
/// cells as `[first..last] B1..D1 empty`; consecutive all-empty rows share one
/// `rows` line. Every cell keeps its index, so each stays addressable. Lines
/// that are not cells of `grid`, and the order of everything else, are kept.
pub fn compact_markdown(markdown: &str, grid: &Grid) -> String {
    let by_index: HashMap<usize, (CellRef, &Cell)> = grid
        .cells
        .iter()
        .filter_map(|(at, cell)| cell.index.map(|i| (i, (*at, cell))))
        .collect();
    if by_index.is_empty() {
        return markdown.to_owned();
    }
    let mut out = Rows::default();
    let mut header_done = false;
    let mut skipping_continuation = false;

    for line in markdown.lines() {
        let trimmed = line.trim_start();
        let Some(rest) = trimmed.strip_prefix("- ") else {
            // A continuation line of a multi-line cell value is part of the
            // cell already shown (escaped) on its row line.
            if !skipping_continuation {
                out.line(line);
            }
            continue;
        };
        skipping_continuation = false;
        let index = rest
            .strip_prefix('[')
            .and_then(|r| r.split_once(']'))
            .and_then(|(n, _)| n.parse::<usize>().ok());
        let Some((index, (at, cell))) = index.and_then(|i| by_index.get(&i).map(|c| (i, *c)))
        else {
            out.line(line);
            continue;
        };
        skipping_continuation = true;
        let indent = &line[..line.len() - trimmed.len()];
        if !header_done {
            header_done = true;
            out.line(&format!(
                "{indent}- grid \"{}\" (cells by row: [index] A1=\"value\"; [first..last] runs of \
                 empty cells)",
                grid.range().unwrap_or_default()
            ));
        }
        out.cell(&format!("{indent}  "), index, at, &cell.value);
    }
    let mut out = out.finish();
    if !markdown.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }
    out
}

/// One grid cell as walked: its index, position and value.
type WalkedCell = (usize, CellRef, String);

/// Builds [`compact_markdown`]'s output: plain lines pass through, cells are
/// gathered into the current sheet row, and all-empty rows are merged.
#[derive(Default)]
struct Rows {
    out: String,
    /// The indent and cells of the sheet row being gathered.
    row: Option<(String, Vec<WalkedCell>)>,
    /// A run of all-empty rows not yet written: indent, first and last cell.
    empty_rows: Option<(String, WalkedCell, WalkedCell)>,
}

impl Rows {
    fn line(&mut self, line: &str) {
        self.flush_row();
        self.flush_empty_rows();
        self.out.push_str(line);
        self.out.push('\n');
    }

    fn cell(&mut self, indent: &str, index: usize, at: CellRef, value: &str) {
        if self
            .row
            .as_ref()
            .is_some_and(|(_, cells)| cells.first().is_some_and(|c| c.1.row != at.row))
        {
            self.flush_row();
        }
        self.row
            .get_or_insert_with(|| (indent.to_owned(), Vec::new()))
            .1
            .push((index, at, value.to_owned()));
    }

    fn flush_row(&mut self) {
        let Some((indent, cells)) = self.row.take() else {
            return;
        };
        let (Some(first), Some(last)) = (cells.first().cloned(), cells.last().cloned()) else {
            return;
        };
        if cells.iter().all(|c| c.2.is_empty()) && is_run(&cells) {
            // Extend a run of empty rows when this row continues it (same
            // columns, next row, next index).
            if let Some((run_indent, run_first, run_last)) = self.empty_rows.as_mut() {
                if *run_indent == indent
                    && run_first.1.col == first.1.col
                    && run_last.1.col == last.1.col
                    && run_last.1.row + 1 == first.1.row
                    && run_last.0 + 1 == first.0
                {
                    *run_last = last;
                    return;
                }
            }
            self.flush_empty_rows();
            self.empty_rows = Some((indent, first, last));
            return;
        }
        self.flush_empty_rows();
        let mut parts: Vec<String> = Vec::new();
        let mut i = 0;
        while i < cells.len() {
            let (index, at, value) = &cells[i];
            if !value.is_empty() {
                parts.push(format!("[{index}] {at}={}", shown_full(value)));
                i += 1;
                continue;
            }
            let mut j = i;
            while j + 1 < cells.len()
                && cells[j + 1].2.is_empty()
                && cells[j + 1].0 == cells[j].0 + 1
                && cells[j + 1].1.col == cells[j].1.col + 1
            {
                j += 1;
            }
            parts.push(if j == i {
                format!("[{index}] {at} empty")
            } else {
                format!("[{index}..{}] {at}..{} empty", cells[j].0, cells[j].1)
            });
            i = j + 1;
        }
        self.out.push_str(&format!(
            "{indent}- row \"{}\": {}\n",
            first.1.row,
            parts.join(" | ")
        ));
    }

    fn flush_empty_rows(&mut self) {
        let Some((indent, first, last)) = self.empty_rows.take() else {
            return;
        };
        let rows = if first.1.row == last.1.row {
            format!("row \"{}\"", first.1.row)
        } else {
            format!("rows \"{}..{}\"", first.1.row, last.1.row)
        };
        let indices = if first.0 == last.0 {
            format!("[{}]", first.0)
        } else {
            format!("[{}..{}]", first.0, last.0)
        };
        self.out.push_str(&format!(
            "{indent}- {rows}: {indices} {}..{} empty\n",
            first.1, last.1
        ));
    }

    fn finish(mut self) -> String {
        self.flush_row();
        self.flush_empty_rows();
        self.out
    }
}

/// Whether a row's cells have consecutive indices and columns.
fn is_run(cells: &[WalkedCell]) -> bool {
    cells
        .windows(2)
        .all(|w| w[1].0 == w[0].0 + 1 && w[1].1.col == w[0].1.col + 1)
}

/// A cell value quoted on one line, never cut (the tree is the full read).
fn shown_full(value: &str) -> String {
    format!("\"{}\"", value.replace('\n', "\\n").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node<'a>(
        depth: usize,
        role: &'a str,
        title: Option<&'a str>,
        value: Option<&'a str>,
        index: Option<usize>,
    ) -> NodeFacts<'a> {
        NodeFacts {
            depth,
            role,
            title,
            description: None,
            value,
            index,
            selected: None,
        }
    }

    fn sheet<'a>(
        values: &'a [(&'a str, &'a str)],
        name_box: Option<&'a str>,
    ) -> Vec<NodeFacts<'a>> {
        let mut nodes = vec![
            node(0, "AXWindow", Some("book.ods"), None, Some(0)),
            node(1, "AXTable", Some("Sheet Data"), None, None),
        ];
        for (i, (addr, value)) in values.iter().enumerate() {
            nodes.push(node(
                2,
                "AXTextField",
                Some(addr),
                (!value.is_empty()).then_some(*value),
                Some(3 + i),
            ));
        }
        nodes.push(node(1, "AXToolbar", Some("Formula Tool Bar"), None, None));
        if let Some(at) = name_box {
            nodes.push(node(2, "AXComboBox", Some(at), Some(at), Some(90)));
        }
        nodes
    }

    const CELLS: [(&str, &str); 6] = [
        ("A1", "Name"),
        ("B1", "Amount"),
        ("A2", "Ada"),
        ("B2", ""),
        ("A3", ""),
        ("B3", ""),
    ];

    #[test]
    fn addresses_parse_and_print() {
        assert_eq!(parse_address("A1"), Some(CellRef { row: 1, col: 1 }));
        assert_eq!(parse_address("$AA$12"), Some(CellRef { row: 12, col: 27 }));
        assert_eq!(parse_address("E12").unwrap().to_string(), "E12");
        assert_eq!(CellRef { row: 3, col: 27 }.to_string(), "AA3");
        assert_eq!(CellRef { row: 3, col: 52 }.to_string(), "AZ3");
        for bad in ["", "A", "12", "A0", "a1", "ABCD1", "A1B", "Total", "A 1"] {
            assert_eq!(parse_address(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_table_of_addressed_cells_is_a_grid() {
        let nodes = sheet(&CELLS, Some("B2"));
        let grid = find_grid(&nodes, true).expect("grid");
        assert_eq!(grid.table, "Sheet Data");
        assert_eq!(grid.cells.len(), 6);
        assert_eq!(grid.active, parse_address("B2"));
        assert_eq!(grid.range().as_deref(), Some("A1:B3"));
        assert_eq!(grid.cells[&parse_address("A2").unwrap()].value, "Ada");
    }

    #[test]
    fn ordinary_tables_are_not_grids() {
        let nodes = vec![
            node(0, "AXWindow", Some("Files"), None, Some(0)),
            node(1, "AXTable", None, None, None),
            node(2, "AXRow", None, None, Some(1)),
            node(3, "AXTextField", Some("Name"), Some("a.txt"), Some(2)),
            node(3, "AXTextField", Some("Size"), Some("3 KB"), Some(3)),
            node(2, "AXRow", None, None, Some(4)),
            node(3, "AXTextField", Some("Name"), Some("b.txt"), Some(5)),
        ];
        assert!(find_grid(&nodes, true).is_none());
        // Too few addressed cells.
        let nodes = sheet(&CELLS[..3], None);
        assert!(find_grid(&nodes, true).is_none());
    }

    #[test]
    fn the_one_selected_cell_is_active_without_a_name_box() {
        let mut nodes = sheet(&CELLS, None);
        nodes[4].selected = Some(true);
        let grid = find_grid(&nodes, true).unwrap();
        assert_eq!(grid.active, parse_address("A2"));
    }

    #[test]
    fn typed_values_show_as_changed_cells() {
        let before = find_grid(&sheet(&CELLS, Some("B2")), true).unwrap();
        let typed = [
            ("A1", "Name"),
            ("B1", "Amount"),
            ("A2", "Ada"),
            ("B2", "12.5"),
            ("A3", "Bo"),
            ("B3", ""),
        ];
        let after = find_grid(&sheet(&typed, Some("B3")), true).unwrap();
        let change = diff(&before, &after);
        assert!(change.is_change());
        assert_eq!(change.changed.len(), 2);
        let text = render_change(&change, &after);
        assert_eq!(
            text,
            "Sheet table \"Sheet Data\" read back after the action: 2 cell(s) changed: \
             B2 \"\" -> \"12.5\", A3 \"\" -> \"Bo\". Active cell B3 (was B2), empty."
        );
    }

    #[test]
    fn a_move_without_edits_names_the_active_cell() {
        let before = find_grid(&sheet(&CELLS, Some("A1")), true).unwrap();
        let after = find_grid(&sheet(&CELLS, Some("A2")), true).unwrap();
        let change = diff(&before, &after);
        assert!(change.is_change());
        assert_eq!(
            render_change(&change, &after),
            "Sheet table \"Sheet Data\" read back after the action: no visible cell changed \
             value (a typed entry is stored once committed with \\n, \\t or a move to another \
             cell). Active cell A2 (was A1) = \"Ada\"."
        );
        let same = diff(&after, &after);
        assert!(!same.is_change());
    }

    #[test]
    fn another_sheet_is_not_diffed_cell_by_cell() {
        let before = find_grid(&sheet(&CELLS, Some("A1")), true).unwrap();
        let mut after = before.clone();
        after.table = "Sheet Notes".into();
        after
            .cells
            .get_mut(&parse_address("A1").unwrap())
            .unwrap()
            .value = "x".into();
        let change = diff(&before, &after);
        assert!(change.changed.is_empty());
        assert!(
            render_change(&change, &after).contains("(was \"Sheet Data\": another sheet is shown)")
        );
    }

    #[test]
    fn a_scrolled_view_reports_its_range() {
        let before = find_grid(&sheet(&CELLS, None), true).unwrap();
        let mut after = before.clone();
        after.cells.insert(
            parse_address("B4").unwrap(),
            Cell {
                value: String::new(),
                index: Some(40),
            },
        );
        let change = diff(&before, &after);
        assert!(render_change(&change, &after).ends_with("Visible cells now A1:B4."));
    }

    #[test]
    fn long_and_multiline_values_are_shown_on_one_line() {
        assert_eq!(shown("a\n\"b\""), "\"a\\n\\\"b\\\"\"");
        let long = "x".repeat(100);
        assert!(shown(&long).ends_with("…\""));
    }

    #[test]
    fn compact_markdown_lists_filled_cells_by_row() {
        let nodes = sheet(&CELLS, Some("B2"));
        let grid = find_grid(&nodes, true).unwrap();
        let md = "- [0] AXWindow \"book.ods\"\n  - AXTable \"Sheet Data\"\n    \
                  - [3] AXTextField \"A1\" = \"Name\"\n    - [4] AXTextField \"B1\" = \"Amount\"\n    \
                  - [5] AXTextField \"A2\" = \"Ada\"\n    - [6] AXTextField \"B2\"\n    \
                  - [7] AXTextField \"A3\"\n    - [8] AXTextField \"B3\"\n  \
                  - AXToolbar \"Formula Tool Bar\"\n    - [90] AXComboBox \"B2\" = \"B2\"\n";
        let compact = compact_markdown(md, &grid);
        assert_eq!(
            compact,
            "- [0] AXWindow \"book.ods\"\n  - AXTable \"Sheet Data\"\n    \
             - grid \"A1:B3\" (cells by row: [index] A1=\"value\"; [first..last] runs of empty cells)\n      \
             - row \"1\": [3] A1=\"Name\" | [4] B1=\"Amount\"\n      \
             - row \"2\": [5] A2=\"Ada\" | [6] B2 empty\n      \
             - row \"3\": [7..8] A3..B3 empty\n  \
             - AXToolbar \"Formula Tool Bar\"\n    - [90] AXComboBox \"B2\" = \"B2\"\n"
        );
    }

    #[test]
    fn compact_markdown_merges_empty_rows_and_runs() {
        let mut nodes = vec![
            node(0, "AXWindow", None, None, Some(0)),
            node(1, "AXTable", Some("Sheet A"), None, None),
        ];
        let mut md = String::from("- [0] AXWindow\n  - AXTable \"Sheet A\"\n");
        let addrs = [
            "A1", "B1", "C1", "D1", "A2", "B2", "C2", "D2", "A3", "B3", "C3", "D3", "A4", "B4",
            "C4", "D4",
        ];
        for (i, addr) in addrs.iter().enumerate() {
            let value = match *addr {
                "A1" => Some("x"),
                "D1" => Some("y"),
                "B4" => Some("z"),
                _ => None,
            };
            nodes.push(node(2, "AXTextField", Some(addr), value, Some(10 + i)));
            match value {
                Some(v) => md.push_str(&format!(
                    "    - [{}] AXTextField \"{addr}\" = \"{v}\"\n",
                    10 + i
                )),
                None => md.push_str(&format!("    - [{}] AXTextField \"{addr}\"\n", 10 + i)),
            }
        }
        let grid = find_grid(&nodes, true).unwrap();
        let compact = compact_markdown(&md, &grid);
        assert!(compact
            .contains("- row \"1\": [10] A1=\"x\" | [11..12] B1..C1 empty | [13] D1=\"y\"\n"));
        assert!(compact.contains("- rows \"2..3\": [14..21] A2..D3 empty\n"));
        assert!(compact
            .contains("- row \"4\": [22] A4 empty | [23] B4=\"z\" | [24..25] C4..D4 empty\n"));
    }

    #[test]
    fn compact_markdown_folds_multiline_values() {
        let nodes = vec![
            node(0, "AXWindow", None, None, Some(0)),
            node(1, "AXTable", Some("Sheet A"), None, None),
            node(2, "AXTextField", Some("A1"), Some("two\nlines"), Some(1)),
            node(2, "AXTextField", Some("B1"), None, Some(2)),
            node(2, "AXTextField", Some("A2"), None, Some(3)),
            node(2, "AXTextField", Some("B2"), Some("z"), Some(4)),
        ];
        let grid = find_grid(&nodes, true).unwrap();
        let md = "- [0] AXWindow\n  - AXTable \"Sheet A\"\n    - [1] AXTextField \"A1\" = \"two\nlines\"\n    \
                  - [2] AXTextField \"B1\"\n    - [3] AXTextField \"A2\"\n    - [4] AXTextField \"B2\" = \"z\"\n";
        let compact = compact_markdown(md, &grid);
        assert!(compact.contains("- row \"1\": [1] A1=\"two\\nlines\" | [2] B1 empty\n"));
        assert!(compact.contains("- row \"2\": [3] A2 empty | [4] B2=\"z\"\n"));
        assert!(!compact.contains("lines\"\n    -"));
    }

    #[test]
    fn markdown_without_grid_cells_is_unchanged() {
        let grid = find_grid(&sheet(&CELLS, None), true).unwrap();
        let md = "- [0] AXWindow \"book.ods\"\n  - [90] AXComboBox \"B2\" = \"B2\"";
        assert_eq!(compact_markdown(md, &grid), md);
    }
}
