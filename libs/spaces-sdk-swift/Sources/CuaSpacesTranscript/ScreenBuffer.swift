import Foundation

/// A single rendered character cell.
///
/// The parser reads text almost exclusively, but attributes are kept because
/// Claude Code uses colour to say things it says nowhere else: a diff's added
/// and removed lines differ only by background colour, and dim is how the
/// composer marks its placeholder. Throwing attributes away at the emulator
/// would make those distinctions unrecoverable downstream.
public struct ScreenCell: Sendable, Hashable {
    /// The grapheme drawn in this cell. `" "` for an untouched cell.
    public var character: Character
    public var attributes: CellAttributes
    /// True for the second half of a double-width character. Such a cell
    /// carries no character of its own and is skipped when reading text.
    public var isWideContinuation: Bool

    public init(character: Character = " ",
                attributes: CellAttributes = .default,
                isWideContinuation: Bool = false) {
        self.character = character
        self.attributes = attributes
        self.isWideContinuation = isWideContinuation
    }

    public static let blank = ScreenCell()
}

/// SGR state, reduced to what can be read back off a frame.
public struct CellAttributes: Sendable, Hashable {
    public var bold: Bool = false
    public var dim: Bool = false
    public var italic: Bool = false
    public var underline: Bool = false
    public var inverse: Bool = false
    public var strikethrough: Bool = false
    /// Foreground colour, or `nil` for the terminal default.
    public var foreground: TerminalColor?
    /// Background colour, or `nil` for the terminal default.
    public var background: TerminalColor?

    public init() {}
    public static let `default` = CellAttributes()

    public var isPlain: Bool { self == .default }
}

/// A colour as the terminal expressed it. Deliberately *not* resolved to RGB
/// when the stream gave an index: a palette index is what was sent, and which
/// pixel that becomes is the display's business, not the parser's.
public enum TerminalColor: Sendable, Hashable {
    /// ANSI 0-7 and their bright variants 8-15, plus the 256-colour cube.
    case indexed(UInt8)
    case rgb(UInt8, UInt8, UInt8)
}

/// A rectangular grid of cells: what a human would have seen at one instant.
///
/// This is the only thing the parser is allowed to look at. Everything the
/// SDK publishes about a Claude Code session is a statement about *this*, so
/// the honesty rule (`FRICTION.md` §93) has a precise meaning: if it is not
/// on the grid, it is not observed.
public struct ScreenBuffer: Sendable, Hashable {
    public let columns: Int
    public let rows: Int
    public private(set) var cells: [ScreenCell]
    /// Cursor position, clamped into the grid. Row and column are zero-based.
    public var cursorRow: Int
    public var cursorColumn: Int
    public var cursorVisible: Bool
    /// True when the application has switched to the alternate screen buffer.
    /// Claude Code's full-screen UI runs on the alternate buffer; a frame
    /// taken while this is false is the shell's scrollback, not the agent UI.
    public var isAlternateScreen: Bool

    public init(columns: Int, rows: Int) {
        self.columns = max(1, columns)
        self.rows = max(1, rows)
        self.cells = Array(repeating: .blank, count: self.columns * self.rows)
        self.cursorRow = 0
        self.cursorColumn = 0
        self.cursorVisible = true
        self.isAlternateScreen = false
    }

    public subscript(row: Int, column: Int) -> ScreenCell {
        get {
            guard row >= 0, row < rows, column >= 0, column < columns else { return .blank }
            return cells[row * columns + column]
        }
        set {
            guard row >= 0, row < rows, column >= 0, column < columns else { return }
            cells[row * columns + column] = newValue
        }
    }

    /// One row as text, with trailing blanks removed.
    public func line(_ row: Int) -> String {
        guard row >= 0, row < rows else { return "" }
        var out = ""
        for column in 0..<columns {
            let cell = self[row, column]
            if cell.isWideContinuation { continue }
            out.append(cell.character)
        }
        while out.hasSuffix(" ") { out.removeLast() }
        return out
    }

    /// Every row as text. Trailing blank rows are kept, because "the bottom
    /// three rows are empty" is a fact a layout parser uses.
    public var lines: [String] { (0..<rows).map { line($0) } }

    /// The whole frame as one newline-joined string.
    public var text: String { lines.joined(separator: "\n") }

    /// The attributes in effect across a run of a row, for callers that need
    /// to tell an added diff line from a removed one.
    public func attributes(row: Int) -> [CellAttributes] {
        (0..<columns).map { self[row, $0].attributes }
    }

    // MARK: - Mutation used by the emulator

    mutating func clearRow(_ row: Int, from: Int, through: Int, blank: ScreenCell) {
        guard row >= 0, row < rows else { return }
        for column in max(0, from)...min(columns - 1, max(0, through)) where from <= through {
            cells[row * columns + column] = blank
        }
    }

    mutating func scrollUp(top: Int, bottom: Int, count: Int, blank: ScreenCell) {
        guard count > 0, top < bottom else { return }
        let n = min(count, bottom - top + 1)
        for row in top...(bottom - n) {
            let source = (row + n) * columns
            let dest = row * columns
            for column in 0..<columns { cells[dest + column] = cells[source + column] }
        }
        for row in (bottom - n + 1)...bottom {
            for column in 0..<columns { cells[row * columns + column] = blank }
        }
    }

    mutating func scrollDown(top: Int, bottom: Int, count: Int, blank: ScreenCell) {
        guard count > 0, top < bottom else { return }
        let n = min(count, bottom - top + 1)
        var row = bottom
        while row >= top + n {
            let source = (row - n) * columns
            let dest = row * columns
            for column in 0..<columns { cells[dest + column] = cells[source + column] }
            row -= 1
        }
        for row in top...(top + n - 1) {
            for column in 0..<columns { cells[row * columns + column] = blank }
        }
    }

    mutating func fillAll(_ blank: ScreenCell) {
        cells = Array(repeating: blank, count: columns * rows)
    }
}
