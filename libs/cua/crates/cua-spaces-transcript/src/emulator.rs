//! A VT100/xterm-class terminal emulator.
//!
//! A direct port of `Sources/CuaSpacesTranscript/TerminalEmulator.swift`.
//! Claude Code repaints rather than appends, so concatenating the byte stream
//! produces a wall of glyphs in an order no human ever saw. This turns the
//! stream back into the grid.
//!
//! Implemented, parsed-and-ignored, and unimplemented sequences are exactly as
//! documented on the Swift type; `unsupported_sequences` counts every sequence
//! that fell through so a frame can be recognised as lower-confidence rather
//! than quietly trusted.

use crate::screen::{CellAttributes, ScreenBuffer, ScreenCell, TerminalColor};
use std::collections::BTreeSet;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    Ground,
    Escape,
    Csi,
    Osc,
    OscEscape,
    Dcs,
    DcsEscape,
    Charset,
}

#[derive(Clone, Copy)]
struct SavedCursor {
    row: usize,
    column: usize,
    attributes: CellAttributes,
}

pub struct TerminalEmulator {
    pub screen: ScreenBuffer,
    pub window_title: String,
    pub unsupported_sequences: u32,

    attributes: CellAttributes,
    saved_cursor: Option<SavedCursor>,
    scroll_top: usize,
    scroll_bottom: usize,
    autowrap: bool,
    pending_wrap: bool,
    tab_stops: BTreeSet<usize>,
    saved_primary: Option<ScreenBuffer>,

    state: State,
    parameter_bytes: Vec<u8>,
    intermediate_bytes: Vec<u8>,
    string_buffer: Vec<u8>,
    utf8: Utf8Accumulator,
}

impl TerminalEmulator {
    pub fn new(columns: usize, rows: usize) -> Self {
        let screen = ScreenBuffer::new(columns, rows);
        let columns = screen.columns;
        let rows = screen.rows;
        let mut tab_stops = BTreeSet::new();
        let limit = columns.max(9);
        let mut stop = 8;
        while stop < limit {
            tab_stops.insert(stop);
            stop += 8;
        }
        TerminalEmulator {
            screen,
            window_title: String::new(),
            unsupported_sequences: 0,
            attributes: CellAttributes::default(),
            saved_cursor: None,
            scroll_top: 0,
            scroll_bottom: rows.saturating_sub(1),
            autowrap: true,
            pending_wrap: false,
            tab_stops,
            saved_primary: None,
            state: State::Ground,
            parameter_bytes: Vec::new(),
            intermediate_bytes: Vec::new(),
            string_buffer: Vec::new(),
            utf8: Utf8Accumulator::default(),
        }
    }

    /// Erasing uses the current background but never the current foreground or
    /// text attributes. That is what xterm does; getting it wrong makes whole
    /// erased regions inherit a colour and turns a diff-colour heuristic into
    /// nonsense.
    fn blank_cell(&self) -> ScreenCell {
        let blank = CellAttributes {
            background: self.attributes.background,
            ..CellAttributes::default()
        };
        ScreenCell {
            character: " ".to_string(),
            attributes: blank,
            is_wide_continuation: false,
        }
    }

    pub fn feed_str(&mut self, text: &str) {
        self.feed(text.as_bytes());
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.consume(*byte);
        }
    }

    fn consume(&mut self, byte: u8) {
        match self.state {
            State::Ground => self.ground(byte),
            State::Escape => self.escape(byte),
            State::Csi => self.csi(byte),
            State::Osc => {
                if byte == 0x07 {
                    self.finish_osc();
                    self.state = State::Ground;
                } else if byte == 0x1b {
                    self.state = State::OscEscape;
                } else {
                    self.string_buffer.push(byte);
                }
            }
            State::OscEscape => {
                if byte == 0x5c {
                    self.finish_osc();
                    self.state = State::Ground;
                } else {
                    self.string_buffer.push(0x1b);
                    self.string_buffer.push(byte);
                    self.state = State::Osc;
                }
            }
            State::Dcs => {
                if byte == 0x1b {
                    self.state = State::DcsEscape;
                } else {
                    self.string_buffer.push(byte);
                }
            }
            State::DcsEscape => {
                if byte == 0x5c {
                    self.unsupported_sequences += 1;
                    self.state = State::Ground;
                } else {
                    self.state = State::Dcs;
                }
            }
            State::Charset => {
                self.state = State::Ground;
            }
        }
    }

    fn ground(&mut self, byte: u8) {
        match byte {
            0x1b => {
                self.utf8.reset();
                self.state = State::Escape;
                self.parameter_bytes.clear();
                self.intermediate_bytes.clear();
            }
            0x07 => {}
            0x08 => {
                self.pending_wrap = false;
                self.screen.cursor_column = self.screen.cursor_column.saturating_sub(1);
            }
            0x09 => self.tab(),
            0x0a..=0x0c => self.line_feed(),
            0x0d => {
                self.pending_wrap = false;
                self.screen.cursor_column = 0;
            }
            0x0e | 0x0f => {}
            _ => {
                if let Some(scalar) = self.utf8.push(byte) {
                    self.put(scalar);
                }
            }
        }
    }

    fn escape(&mut self, byte: u8) {
        match byte {
            0x5b => self.state = State::Csi,
            0x5d => {
                self.string_buffer.clear();
                self.state = State::Osc;
            }
            0x50 => {
                self.string_buffer.clear();
                self.state = State::Dcs;
            }
            0x28..=0x2b => self.state = State::Charset,
            0x37 => {
                self.saved_cursor = Some(SavedCursor {
                    row: self.screen.cursor_row,
                    column: self.screen.cursor_column,
                    attributes: self.attributes,
                });
                self.state = State::Ground;
            }
            0x38 => {
                self.restore_cursor();
                self.state = State::Ground;
            }
            0x44 => {
                self.line_feed();
                self.state = State::Ground;
            }
            0x45 => {
                self.screen.cursor_column = 0;
                self.line_feed();
                self.state = State::Ground;
            }
            0x4d => {
                self.reverse_index();
                self.state = State::Ground;
            }
            0x63 => {
                self.reset();
                self.state = State::Ground;
            }
            0x3d | 0x3e => self.state = State::Ground,
            _ => {
                self.unsupported_sequences += 1;
                self.state = State::Ground;
            }
        }
    }

    fn csi(&mut self, byte: u8) {
        match byte {
            0x30..=0x3f => self.parameter_bytes.push(byte),
            0x20..=0x2f => self.intermediate_bytes.push(byte),
            0x40..=0x7e => {
                self.dispatch_csi(byte);
                self.state = State::Ground;
            }
            _ => {
                self.unsupported_sequences += 1;
                self.state = State::Ground;
            }
        }
    }

    // MARK: CSI dispatch

    fn parameters(&self) -> Vec<i64> {
        let filtered: Vec<u8> = self
            .parameter_bytes
            .iter()
            .copied()
            .filter(|b| *b != 0x3f && *b != 0x3c && *b != 0x3e)
            .collect();
        let text = String::from_utf8_lossy(&filtered).to_string();
        if text.is_empty() {
            return Vec::new();
        }
        text.split(';')
            .map(|part| {
                let head = part.split(':').next().unwrap_or("");
                head.parse::<i64>().unwrap_or(0)
            })
            .collect()
    }

    fn is_private(&self) -> bool {
        self.parameter_bytes.first() == Some(&0x3f)
    }

    fn parameter(&self, index: usize, default: i64) -> i64 {
        let list = self.parameters();
        match list.get(index) {
            None => default,
            Some(0) => default,
            Some(value) => *value,
        }
    }

    fn dispatch_csi(&mut self, final_byte: u8) {
        self.pending_wrap = false;
        let rows = self.screen.rows;
        let columns = self.screen.columns;
        match final_byte {
            0x41 => {
                // A — CUU
                let n = self.parameter(0, 1).max(0) as usize;
                let target = self.screen.cursor_row.saturating_sub(n);
                self.screen.cursor_row = target.max(self.scroll_top);
            }
            0x42 => {
                // B — CUD
                let n = self.parameter(0, 1).max(0) as usize;
                self.screen.cursor_row = (self.screen.cursor_row + n).min(rows - 1);
            }
            0x43 => {
                // C — CUF
                let n = self.parameter(0, 1).max(0) as usize;
                self.screen.cursor_column = (self.screen.cursor_column + n).min(columns - 1);
            }
            0x44 => {
                // D — CUB
                let n = self.parameter(0, 1).max(0) as usize;
                self.screen.cursor_column = self.screen.cursor_column.saturating_sub(n);
            }
            0x45 => {
                // E — CNL
                let n = self.parameter(0, 1).max(0) as usize;
                self.screen.cursor_row = (self.screen.cursor_row + n).min(rows - 1);
                self.screen.cursor_column = 0;
            }
            0x46 => {
                // F — CPL
                let n = self.parameter(0, 1).max(0) as usize;
                self.screen.cursor_row = self.screen.cursor_row.saturating_sub(n);
                self.screen.cursor_column = 0;
            }
            0x47 | 0x60 => {
                // G, ` — CHA / HPA
                self.screen.cursor_column = self.clamp_column(self.parameter(0, 1) - 1);
            }
            0x48 | 0x66 => {
                // H, f — CUP / HVP
                self.screen.cursor_row = self.clamp_row(self.parameter(0, 1) - 1);
                self.screen.cursor_column = self.clamp_column(self.parameter(1, 1) - 1);
            }
            0x49 => {
                // I — CHT
                for _ in 0..self.parameter(0, 1).max(0) {
                    self.tab();
                }
            }
            0x4a => {
                // J — ED
                let mode = self.parameters().first().copied().unwrap_or(0);
                self.erase_in_display(mode);
            }
            0x4b => {
                // K — EL
                let mode = self.parameters().first().copied().unwrap_or(0);
                self.erase_in_line(mode);
            }
            0x4c => {
                // L — IL
                let blank = self.blank_cell();
                let count = self.parameter(0, 1).max(0) as usize;
                let (top, bottom) = (self.screen.cursor_row, self.scroll_bottom);
                self.screen.scroll_down(top, bottom, count, &blank);
            }
            0x4d => {
                // M — DL
                let blank = self.blank_cell();
                let count = self.parameter(0, 1).max(0) as usize;
                let (top, bottom) = (self.screen.cursor_row, self.scroll_bottom);
                self.screen.scroll_up(top, bottom, count, &blank);
            }
            0x50 => {
                // P — DCH
                self.delete_characters(self.parameter(0, 1).max(0) as usize);
            }
            0x53 => {
                // S — SU
                let blank = self.blank_cell();
                let count = self.parameter(0, 1).max(0) as usize;
                let (top, bottom) = (self.scroll_top, self.scroll_bottom);
                self.screen.scroll_up(top, bottom, count, &blank);
            }
            0x54 => {
                // T — SD
                let blank = self.blank_cell();
                let count = self.parameter(0, 1).max(0) as usize;
                let (top, bottom) = (self.scroll_top, self.scroll_bottom);
                self.screen.scroll_down(top, bottom, count, &blank);
            }
            0x58 => {
                // X — ECH
                let count = self.parameter(0, 1).max(0);
                let blank = self.blank_cell();
                let row = self.screen.cursor_row;
                let from = self.screen.cursor_column;
                let through = from as i64 + count - 1;
                self.screen.clear_row(row, from, through, &blank);
            }
            0x5a => {
                // Z — CBT
                self.back_tab(self.parameter(0, 1).max(0) as usize);
            }
            0x40 => {
                // @ — ICH
                self.insert_characters(self.parameter(0, 1).max(0) as usize);
            }
            0x64 => {
                // d — VPA
                self.screen.cursor_row = self.clamp_row(self.parameter(0, 1) - 1);
            }
            0x67 => {
                // g — TBC
                if self.parameters().first().copied().unwrap_or(0) == 3 {
                    self.tab_stops.clear();
                } else {
                    let column = self.screen.cursor_column;
                    self.tab_stops.remove(&column);
                }
            }
            0x68 => self.set_modes(true),
            0x6c => self.set_modes(false),
            0x6d => self.apply_sgr(),
            0x72 => {
                // r — DECSTBM
                let parameters = self.parameters();
                let top = self.clamp_row(self.parameter(0, 1) - 1);
                let bottom_raw = if parameters.len() > 1 && parameters[1] != 0 {
                    parameters[1] - 1
                } else {
                    rows as i64 - 1
                };
                let bottom = self.clamp_row(bottom_raw);
                if top < bottom {
                    self.scroll_top = top;
                    self.scroll_bottom = bottom;
                } else {
                    self.scroll_top = 0;
                    self.scroll_bottom = rows - 1;
                }
                self.screen.cursor_row = self.scroll_top;
                self.screen.cursor_column = 0;
            }
            0x73 => {
                self.saved_cursor = Some(SavedCursor {
                    row: self.screen.cursor_row,
                    column: self.screen.cursor_column,
                    attributes: self.attributes,
                });
            }
            0x75 => {
                // u — restore cursor, or kitty keyboard push/pop
                match self.parameter_bytes.first() {
                    Some(0x3c) | Some(0x3e) | Some(0x3f) => {}
                    _ => self.restore_cursor(),
                }
            }
            0x63 | 0x6e | 0x70 | 0x71 | 0x74 => {}
            _ => self.unsupported_sequences += 1,
        }
    }

    fn restore_cursor(&mut self) {
        if let Some(saved) = self.saved_cursor {
            self.screen.cursor_row = saved.row;
            self.screen.cursor_column = saved.column;
            self.attributes = saved.attributes;
        }
    }

    fn set_modes(&mut self, on: bool) {
        if !self.is_private() {
            return;
        }
        for mode in self.parameters() {
            match mode {
                7 => self.autowrap = on,
                25 => self.screen.cursor_visible = on,
                1047 | 1049 => self.switch_alternate_screen(on),
                1048 => {
                    if on {
                        self.saved_cursor = Some(SavedCursor {
                            row: self.screen.cursor_row,
                            column: self.screen.cursor_column,
                            attributes: self.attributes,
                        });
                    } else {
                        self.restore_cursor();
                    }
                }
                _ => {}
            }
        }
    }

    fn switch_alternate_screen(&mut self, on: bool) {
        if on {
            if self.saved_primary.is_some() {
                return;
            }
            self.saved_primary = Some(self.screen.clone());
            self.saved_cursor = Some(SavedCursor {
                row: self.screen.cursor_row,
                column: self.screen.cursor_column,
                attributes: self.attributes,
            });
            let blank = self.blank_cell();
            self.screen.fill_all(&blank);
            self.screen.cursor_row = 0;
            self.screen.cursor_column = 0;
            self.screen.is_alternate_screen = true;
        } else {
            let Some(mut primary) = self.saved_primary.take() else {
                return;
            };
            primary.cursor_visible = self.screen.cursor_visible;
            self.screen = primary;
            self.screen.is_alternate_screen = false;
            self.restore_cursor();
        }
    }

    // MARK: SGR

    fn apply_sgr(&mut self) {
        // Re-parse with colons preserved so `38:2::r:g:b` is handled as well as
        // `38;2;r;g;b`. Both forms appear in the wild.
        let text = String::from_utf8_lossy(&self.parameter_bytes).to_string();
        let mut codes: Vec<i64> = Vec::new();
        if text.is_empty() {
            codes.push(0);
        } else {
            for group in text.split(';') {
                for part in group.split(':') {
                    codes.push(part.parse::<i64>().unwrap_or(0));
                }
            }
            if codes.is_empty() {
                codes.push(0);
            }
        }

        let mut index = 0usize;
        while index < codes.len() {
            let code = codes[index];
            match code {
                0 => self.attributes = CellAttributes::default(),
                1 => self.attributes.bold = true,
                2 => self.attributes.dim = true,
                3 => self.attributes.italic = true,
                4 => self.attributes.underline = true,
                7 => self.attributes.inverse = true,
                9 => self.attributes.strikethrough = true,
                21 | 22 => {
                    self.attributes.bold = false;
                    self.attributes.dim = false;
                }
                23 => self.attributes.italic = false,
                24 => self.attributes.underline = false,
                27 => self.attributes.inverse = false,
                29 => self.attributes.strikethrough = false,
                30..=37 => {
                    self.attributes.foreground = Some(TerminalColor::Indexed((code - 30) as u8))
                }
                39 => self.attributes.foreground = None,
                40..=47 => {
                    self.attributes.background = Some(TerminalColor::Indexed((code - 40) as u8))
                }
                49 => self.attributes.background = None,
                90..=97 => {
                    self.attributes.foreground = Some(TerminalColor::Indexed((code - 90 + 8) as u8))
                }
                100..=107 => {
                    self.attributes.background =
                        Some(TerminalColor::Indexed((code - 100 + 8) as u8))
                }
                38 | 48 => {
                    let is_foreground = code == 38;
                    if index + 1 >= codes.len() {
                        // Swift's `break` leaves the switch and the trailing
                        // `index += 1` then ends the loop; leaving the loop
                        // here is the same outcome.
                        break;
                    }
                    let kind = codes[index + 1];
                    if kind == 5 && index + 2 < codes.len() {
                        let colour = TerminalColor::Indexed(clamp_u8(codes[index + 2]));
                        if is_foreground {
                            self.attributes.foreground = Some(colour);
                        } else {
                            self.attributes.background = Some(colour);
                        }
                        index += 2;
                    } else if kind == 2 && index + 4 < codes.len() {
                        let colour = TerminalColor::Rgb(
                            clamp_u8(codes[index + 2]),
                            clamp_u8(codes[index + 3]),
                            clamp_u8(codes[index + 4]),
                        );
                        if is_foreground {
                            self.attributes.foreground = Some(colour);
                        } else {
                            self.attributes.background = Some(colour);
                        }
                        index += 4;
                    } else {
                        index = codes.len();
                    }
                }
                _ => {}
            }
            index += 1;
        }
    }

    // MARK: OSC

    fn finish_osc(&mut self) {
        let payload = String::from_utf8_lossy(&self.string_buffer).to_string();
        self.string_buffer.clear();
        let Some(separator) = payload.find(';') else {
            return;
        };
        let code = &payload[..separator];
        let value = &payload[separator + 1..];
        if code == "0" || code == "1" || code == "2" {
            self.window_title = value.to_string();
        }
    }

    // MARK: primitives

    fn clamp_row(&self, row: i64) -> usize {
        row.max(0).min(self.screen.rows as i64 - 1) as usize
    }

    fn clamp_column(&self, column: i64) -> usize {
        column.max(0).min(self.screen.columns as i64 - 1) as usize
    }

    fn put(&mut self, character: String) {
        if self.pending_wrap && self.autowrap {
            self.screen.cursor_column = 0;
            self.line_feed();
            self.pending_wrap = false;
        }
        let width = terminal_width(&character);
        if width == 0 {
            // A combining mark belongs to the cell that was just written.
            let target = self.screen.cursor_column.saturating_sub(1);
            let row = self.screen.cursor_row;
            let mut cell = self.screen.get(row, target);
            cell.character.push_str(&character);
            self.screen.set(row, target, cell);
            return;
        }
        if self.screen.cursor_column + width > self.screen.columns {
            if self.autowrap {
                self.screen.cursor_column = 0;
                self.line_feed();
            } else {
                self.screen.cursor_column = self.screen.columns - width;
            }
        }
        let row = self.screen.cursor_row;
        let column = self.screen.cursor_column;
        self.screen.set(
            row,
            column,
            ScreenCell {
                character,
                attributes: self.attributes,
                is_wide_continuation: false,
            },
        );
        if width == 2 {
            self.screen.set(
                row,
                column + 1,
                ScreenCell {
                    character: " ".to_string(),
                    attributes: self.attributes,
                    is_wide_continuation: true,
                },
            );
        }
        self.screen.cursor_column += width;
        if self.screen.cursor_column >= self.screen.columns {
            // xterm's deferred wrap: the cursor sits on the last column and
            // only wraps when the next printable arrives. Wrapping eagerly
            // inserts a blank line every time a line exactly fills the width,
            // which for a box-drawn UI is every single frame.
            self.screen.cursor_column = self.screen.columns - 1;
            self.pending_wrap = true;
        }
    }

    fn line_feed(&mut self) {
        if self.screen.cursor_row == self.scroll_bottom {
            let blank = self.blank_cell();
            let (top, bottom) = (self.scroll_top, self.scroll_bottom);
            self.screen.scroll_up(top, bottom, 1, &blank);
        } else if self.screen.cursor_row < self.screen.rows - 1 {
            self.screen.cursor_row += 1;
        }
    }

    fn reverse_index(&mut self) {
        if self.screen.cursor_row == self.scroll_top {
            let blank = self.blank_cell();
            let (top, bottom) = (self.scroll_top, self.scroll_bottom);
            self.screen.scroll_down(top, bottom, 1, &blank);
        } else if self.screen.cursor_row > 0 {
            self.screen.cursor_row -= 1;
        }
    }

    fn tab(&mut self) {
        let next = self
            .tab_stops
            .iter()
            .copied()
            .find(|stop| *stop > self.screen.cursor_column)
            .unwrap_or(self.screen.columns - 1);
        self.screen.cursor_column = self.clamp_column(next as i64);
    }

    fn back_tab(&mut self, count: usize) {
        for _ in 0..count.max(1) {
            let previous = self
                .tab_stops
                .iter()
                .copied()
                .rfind(|stop| *stop < self.screen.cursor_column)
                .unwrap_or(0);
            self.screen.cursor_column = self.clamp_column(previous as i64);
        }
    }

    fn erase_in_display(&mut self, mode: i64) {
        let blank = self.blank_cell();
        let columns = self.screen.columns;
        let rows = self.screen.rows;
        let row = self.screen.cursor_row;
        let column = self.screen.cursor_column;
        match mode {
            0 => {
                self.screen
                    .clear_row(row, column, columns as i64 - 1, &blank);
                for r in (row + 1)..rows {
                    self.screen.clear_row(r, 0, columns as i64 - 1, &blank);
                }
            }
            1 => {
                self.screen.clear_row(row, 0, column as i64, &blank);
                for r in 0..row {
                    self.screen.clear_row(r, 0, columns as i64 - 1, &blank);
                }
            }
            _ => self.screen.fill_all(&blank),
        }
    }

    fn erase_in_line(&mut self, mode: i64) {
        let blank = self.blank_cell();
        let columns = self.screen.columns;
        let row = self.screen.cursor_row;
        let column = self.screen.cursor_column;
        match mode {
            0 => self
                .screen
                .clear_row(row, column, columns as i64 - 1, &blank),
            1 => self.screen.clear_row(row, 0, column as i64, &blank),
            _ => self.screen.clear_row(row, 0, columns as i64 - 1, &blank),
        }
    }

    fn insert_characters(&mut self, count: usize) {
        let row = self.screen.cursor_row;
        let start = self.screen.cursor_column;
        let columns = self.screen.columns;
        let n = count.min(columns.saturating_sub(start));
        if n == 0 {
            return;
        }
        let mut column = columns as i64 - 1;
        while column >= (start + n) as i64 {
            let moved = self.screen.get(row, column as usize - n);
            self.screen.set(row, column as usize, moved);
            column -= 1;
        }
        let blank = self.blank_cell();
        self.screen
            .clear_row(row, start, (start + n - 1) as i64, &blank);
    }

    fn delete_characters(&mut self, count: usize) {
        let row = self.screen.cursor_row;
        let start = self.screen.cursor_column;
        let columns = self.screen.columns;
        let n = count.min(columns.saturating_sub(start));
        if n == 0 {
            return;
        }
        for column in start..(columns - n) {
            let moved = self.screen.get(row, column + n);
            self.screen.set(row, column, moved);
        }
        let blank = self.blank_cell();
        self.screen
            .clear_row(row, columns - n, columns as i64 - 1, &blank);
    }

    fn reset(&mut self) {
        let columns = self.screen.columns;
        let rows = self.screen.rows;
        *self = TerminalEmulator::new(columns, rows);
    }
}

fn clamp_u8(value: i64) -> u8 {
    value.clamp(0, 255) as u8
}

// MARK: UTF-8 accumulation

/// Decodes UTF-8 one byte at a time. The emulator is fed chunks that split
/// wherever the recording split them, so a decoder that requires whole
/// sequences would lose every character that straddles a boundary.
#[derive(Default)]
struct Utf8Accumulator {
    buffer: Vec<u8>,
    expected: usize,
}

impl Utf8Accumulator {
    fn reset(&mut self) {
        self.buffer.clear();
        self.expected = 0;
    }

    fn push(&mut self, byte: u8) -> Option<String> {
        if self.expected == 0 {
            match byte {
                0x00..=0x7f => return Some((byte as char).to_string()),
                0xc2..=0xdf => self.expected = 1,
                0xe0..=0xef => self.expected = 2,
                0xf0..=0xf4 => self.expected = 3,
                _ => return None, // stray continuation or invalid lead: drop it
            }
            self.buffer = vec![byte];
            return None;
        }
        if byte & 0xc0 != 0x80 {
            // Not a continuation. Abandon the partial sequence and reinterpret
            // this byte as a fresh lead.
            self.reset();
            return self.push(byte);
        }
        self.buffer.push(byte);
        self.expected -= 1;
        if self.expected != 0 {
            return None;
        }
        let decoded = String::from_utf8_lossy(&self.buffer).to_string();
        self.buffer.clear();
        decoded.chars().next().map(|c| c.to_string())
    }
}

/// Terminal cell width: 0 for combining marks and variation selectors, 2 for
/// East Asian wide and the emoji ranges, 1 otherwise. A working approximation
/// of `wcwidth`, not a full Unicode database, and deliberately identical to
/// the Swift `Character.terminalWidth` it was ported from.
pub fn terminal_width(character: &str) -> usize {
    let mut scalars = character.chars();
    let Some(first) = scalars.next() else {
        return 1;
    };
    if character.chars().count() > 1 && character.chars().any(|c| c as u32 == 0xFE0F) {
        return 2;
    }
    let value = first as u32;
    match value {
        0x0300..=0x036F | 0x200B..=0x200F | 0xFE00..=0xFE0F | 0x20D0..=0x20FF => 0,
        0x1100..=0x115F
        | 0x2E80..=0x303E
        | 0x3041..=0x33FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xA000..=0xA4CF
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFE30..=0xFE6F
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x1F300..=0x1F64F
        | 0x1F900..=0x1F9FF
        | 0x1FA70..=0x1FAFF
        | 0x20000..=0x3FFFD => 2,
        _ => 1,
    }
}
