//! The rendered grid.
//!
//! A direct port of `Sources/CuaSpacesTranscript/ScreenBuffer.swift`. The
//! Swift implementation is the incumbent source of truth: where this file
//! looks odd, it is because it reproduces a decision made there, and the
//! conformance fixtures under `contract/fixtures/transcript` are what hold the
//! two honest.

/// A colour as the terminal expressed it. An index is kept as an index: which
/// pixel it becomes is the display's business, not the parser's.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TerminalColor {
    Indexed(u8),
    Rgb(u8, u8, u8),
}

/// SGR state, reduced to what can be read back off a frame.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct CellAttributes {
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
    pub strikethrough: bool,
    pub foreground: Option<TerminalColor>,
    pub background: Option<TerminalColor>,
}

impl CellAttributes {
    pub fn is_plain(&self) -> bool {
        *self == CellAttributes::default()
    }
}

/// A single rendered character cell.
///
/// `character` is a Swift `Character` in the original: a grapheme cluster, so
/// combining marks land in the cell that was already written rather than in a
/// cell of their own. A `String` is the honest Rust equivalent; a `char` would
/// silently drop every accent the CLI draws.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ScreenCell {
    pub character: String,
    pub attributes: CellAttributes,
    pub is_wide_continuation: bool,
}

impl Default for ScreenCell {
    fn default() -> Self {
        ScreenCell {
            character: " ".to_string(),
            attributes: CellAttributes::default(),
            is_wide_continuation: false,
        }
    }
}

impl ScreenCell {
    pub fn blank() -> Self {
        Self::default()
    }
}

/// A rectangular grid of cells: what a human would have seen at one instant.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ScreenBuffer {
    pub columns: usize,
    pub rows: usize,
    cells: Vec<ScreenCell>,
    pub cursor_row: usize,
    pub cursor_column: usize,
    pub cursor_visible: bool,
    pub is_alternate_screen: bool,
}

impl ScreenBuffer {
    pub fn new(columns: usize, rows: usize) -> Self {
        let columns = columns.max(1);
        let rows = rows.max(1);
        ScreenBuffer {
            columns,
            rows,
            cells: vec![ScreenCell::blank(); columns * rows],
            cursor_row: 0,
            cursor_column: 0,
            cursor_visible: true,
            is_alternate_screen: false,
        }
    }

    pub fn get(&self, row: usize, column: usize) -> ScreenCell {
        if row >= self.rows || column >= self.columns {
            return ScreenCell::blank();
        }
        self.cells[row * self.columns + column].clone()
    }

    pub fn set(&mut self, row: usize, column: usize, cell: ScreenCell) {
        if row >= self.rows || column >= self.columns {
            return;
        }
        self.cells[row * self.columns + column] = cell;
    }

    /// One row as text, with trailing blanks removed.
    pub fn line(&self, row: usize) -> String {
        if row >= self.rows {
            return String::new();
        }
        let mut out = String::new();
        for column in 0..self.columns {
            let cell = &self.cells[row * self.columns + column];
            if cell.is_wide_continuation {
                continue;
            }
            out.push_str(&cell.character);
        }
        while out.ends_with(' ') {
            out.pop();
        }
        out
    }

    /// Every row as text. Trailing blank rows are kept: "the bottom three rows
    /// are empty" is a fact a layout parser uses.
    pub fn lines(&self) -> Vec<String> {
        (0..self.rows).map(|row| self.line(row)).collect()
    }

    pub fn text(&self) -> String {
        self.lines().join("\n")
    }

    pub fn attributes_for_row(&self, row: usize) -> Vec<CellAttributes> {
        (0..self.columns)
            .map(|c| self.get(row, c).attributes)
            .collect()
    }

    // MARK: mutation used by the emulator

    pub(crate) fn clear_row(&mut self, row: usize, from: usize, through: i64, blank: &ScreenCell) {
        if row >= self.rows {
            return;
        }
        if through < from as i64 {
            return;
        }
        let through = (through as usize).min(self.columns - 1);
        for column in from..=through {
            self.cells[row * self.columns + column] = blank.clone();
        }
    }

    pub(crate) fn scroll_up(
        &mut self,
        top: usize,
        bottom: usize,
        count: usize,
        blank: &ScreenCell,
    ) {
        if count == 0 || top >= bottom {
            return;
        }
        let n = count.min(bottom - top + 1);
        for row in top..=(bottom - n) {
            let source = (row + n) * self.columns;
            let dest = row * self.columns;
            for column in 0..self.columns {
                self.cells[dest + column] = self.cells[source + column].clone();
            }
        }
        for row in (bottom - n + 1)..=bottom {
            for column in 0..self.columns {
                self.cells[row * self.columns + column] = blank.clone();
            }
        }
    }

    pub(crate) fn scroll_down(
        &mut self,
        top: usize,
        bottom: usize,
        count: usize,
        blank: &ScreenCell,
    ) {
        if count == 0 || top >= bottom {
            return;
        }
        let n = count.min(bottom - top + 1);
        let mut row = bottom as i64;
        while row >= (top + n) as i64 {
            let source = (row as usize - n) * self.columns;
            let dest = row as usize * self.columns;
            for column in 0..self.columns {
                self.cells[dest + column] = self.cells[source + column].clone();
            }
            row -= 1;
        }
        for row in top..=(top + n - 1) {
            for column in 0..self.columns {
                self.cells[row * self.columns + column] = blank.clone();
            }
        }
    }

    pub(crate) fn fill_all(&mut self, blank: &ScreenCell) {
        self.cells = vec![blank.clone(); self.columns * self.rows];
    }
}
