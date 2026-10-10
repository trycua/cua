import Foundation

/// A VT100/xterm-class terminal emulator, sized to what a coding CLI actually
/// emits.
///
/// Why this exists at all: Claude Code repaints. It does not append. A single
/// spinner tick is `ESC[?25l ESC[H CR ESC[28B ✻ ESC[34;1H ESC[32;3H ESC[?25h`
/// — home, move down 28, draw one glyph, park the cursor. Concatenating the
/// byte stream produces a wall of glyphs in an order no human ever saw.
/// `FRICTION.md` §19 says every app writes its own parser and they all guess
/// differently; the first and largest guess is "the output is the transcript",
/// and it is wrong.
///
/// Scope, stated so callers know the limits rather than discovering them:
///
/// * Implemented: CUP/HVP, CUU/CUD/CUF/CUB, CHA/VPA, CNL/CPL, ED, EL, ICH,
///   DCH, IL, DL, ECH, SU, SD, DECSTBM scroll regions, DECSC/DECRC, index and
///   reverse index, SGR (1-9, 21-29, 30-37, 39, 40-47, 49, 90-97, 100-107 and
///   both 256-colour and truecolour extended forms), DECAWM autowrap with the
///   deferred-wrap rule, DECTCEM cursor visibility, the 1047/1048/1049
///   alternate screen, tabs with default 8-column stops, and OSC 0/1/2 window
///   titles (captured, since Claude Code puts the live turn state in there).
/// * Parsed and deliberately ignored: DCS strings, mouse and bracketed-paste
///   mode sets, charset designation, XTVERSION and other query sequences.
///   They change nothing a reader of the frame can see.
/// * Not implemented: double-width/double-height line attributes (DECDWL),
///   sixel, and the soft character set. `unsupportedSequences` counts every
///   sequence that fell through, so a frame produced from a stream using them
///   can be recognised as lower-confidence rather than quietly trusted.
public struct TerminalEmulator: Sendable {
    public private(set) var screen: ScreenBuffer
    /// The most recent OSC 0/1/2 window title. Claude Code writes the live
    /// spinner glyph and the turn's subject here, which is the one piece of
    /// turn state that survives even when the grid is mid-repaint.
    public private(set) var windowTitle: String = ""
    /// Count of sequences the emulator recognised as escape sequences but did
    /// not act on. Non-zero does not mean the frame is wrong; it means the
    /// frame is not a complete account of the stream.
    public private(set) var unsupportedSequences: Int = 0

    private var attributes = CellAttributes()
    private var savedCursor: (row: Int, column: Int, attributes: CellAttributes)?
    private var scrollTop = 0
    private var scrollBottom: Int
    private var autowrap = true
    private var pendingWrap = false
    private var tabStops: Set<Int>

    // Alternate screen storage.
    private var savedPrimary: ScreenBuffer?

    // Incremental parser state.
    private enum State: Sendable {
        case ground
        case escape
        case csi
        case osc
        case oscEscape
        case dcs
        case dcsEscape
        case charset
    }
    private var state: State = .ground
    private var parameterBytes: [UInt8] = []
    private var intermediateBytes: [UInt8] = []
    private var stringBuffer: [UInt8] = []
    private var utf8Decoder = UTF8Accumulator()

    public init(columns: Int, rows: Int) {
        self.screen = ScreenBuffer(columns: columns, rows: rows)
        self.scrollBottom = max(0, rows - 1)
        self.tabStops = Set(stride(from: 8, to: max(9, columns), by: 8))
    }

    private var blankCell: ScreenCell {
        // Erasing uses the *current background* but never the current
        // foreground or text attributes: that is what xterm does, and getting
        // it wrong makes whole erased regions inherit a colour and turns a
        // diff-colour heuristic into nonsense.
        var blank = CellAttributes()
        blank.background = attributes.background
        return ScreenCell(character: " ", attributes: blank)
    }

    // MARK: - Feeding

    public mutating func feed(_ text: String) {
        feed(Array(text.utf8))
    }

    public mutating func feed(_ bytes: [UInt8]) {
        for byte in bytes { consume(byte) }
    }

    private mutating func consume(_ byte: UInt8) {
        switch state {
        case .ground:
            ground(byte)
        case .escape:
            escape(byte)
        case .csi:
            csi(byte)
        case .osc:
            if byte == 0x07 { finishOSC(); state = .ground }
            else if byte == 0x1b { state = .oscEscape }
            else { stringBuffer.append(byte) }
        case .oscEscape:
            if byte == 0x5c { finishOSC(); state = .ground }
            else { stringBuffer.append(0x1b); stringBuffer.append(byte); state = .osc }
        case .dcs:
            if byte == 0x1b { state = .dcsEscape } else { stringBuffer.append(byte) }
        case .dcsEscape:
            if byte == 0x5c { unsupportedSequences += 1; state = .ground }
            else { state = .dcs }
        case .charset:
            // ESC ( B and friends. Recorded as handled: the only designation a
            // coding CLI uses in practice is "back to ASCII".
            state = .ground
        }
    }

    private mutating func ground(_ byte: UInt8) {
        switch byte {
        case 0x1b:
            utf8Decoder.reset()
            state = .escape
            parameterBytes.removeAll(keepingCapacity: true)
            intermediateBytes.removeAll(keepingCapacity: true)
        case 0x07:
            break // bell
        case 0x08:
            pendingWrap = false
            screen.cursorColumn = max(0, screen.cursorColumn - 1)
        case 0x09:
            tab()
        case 0x0a, 0x0b, 0x0c:
            lineFeed()
        case 0x0d:
            pendingWrap = false
            screen.cursorColumn = 0
        case 0x0e, 0x0f:
            break // shift out/in; the CLI uses SI to return to ASCII
        default:
            if let scalar = utf8Decoder.push(byte) {
                put(Character(scalar))
            }
        }
    }

    private mutating func escape(_ byte: UInt8) {
        switch byte {
        case 0x5b: // [
            state = .csi
        case 0x5d: // ]
            stringBuffer.removeAll(keepingCapacity: true)
            state = .osc
        case 0x50: // P — DCS
            stringBuffer.removeAll(keepingCapacity: true)
            state = .dcs
        case 0x28, 0x29, 0x2a, 0x2b: // ( ) * +
            state = .charset
        case 0x37: // 7 — DECSC
            savedCursor = (screen.cursorRow, screen.cursorColumn, attributes)
            state = .ground
        case 0x38: // 8 — DECRC
            if let saved = savedCursor {
                screen.cursorRow = saved.row
                screen.cursorColumn = saved.column
                attributes = saved.attributes
            }
            state = .ground
        case 0x44: // D — IND
            lineFeed(); state = .ground
        case 0x45: // E — NEL
            screen.cursorColumn = 0; lineFeed(); state = .ground
        case 0x4d: // M — RI
            reverseIndex(); state = .ground
        case 0x63: // c — RIS
            reset(); state = .ground
        case 0x3d, 0x3e: // = > keypad modes
            state = .ground
        default:
            unsupportedSequences += 1
            state = .ground
        }
    }

    private mutating func csi(_ byte: UInt8) {
        switch byte {
        case 0x30...0x3f: // parameter bytes, including ? < > and ;
            parameterBytes.append(byte)
        case 0x20...0x2f: // intermediate
            intermediateBytes.append(byte)
        case 0x40...0x7e: // final
            dispatchCSI(final: byte)
            state = .ground
        default:
            unsupportedSequences += 1
            state = .ground
        }
    }

    // MARK: - CSI dispatch

    private var parameters: [Int] {
        let text = String(decoding: parameterBytes.filter { $0 != 0x3f && $0 != 0x3c && $0 != 0x3e },
                          as: UTF8.self)
        if text.isEmpty { return [] }
        return text.split(separator: ";", omittingEmptySubsequences: false).map {
            Int($0.split(separator: ":").first.map(String.init) ?? "") ?? 0
        }
    }

    private var isPrivate: Bool { parameterBytes.first == 0x3f }

    private func parameter(_ index: Int, default value: Int) -> Int {
        let list = parameters
        guard index < list.count else { return value }
        return list[index] == 0 ? value : list[index]
    }

    private mutating func dispatchCSI(final: UInt8) {
        pendingWrap = false
        switch final {
        case 0x41: // A — CUU
            screen.cursorRow = max(scrollTop, screen.cursorRow - parameter(0, default: 1))
        case 0x42: // B — CUD
            screen.cursorRow = min(screen.rows - 1, screen.cursorRow + parameter(0, default: 1))
        case 0x43: // C — CUF
            screen.cursorColumn = min(screen.columns - 1, screen.cursorColumn + parameter(0, default: 1))
        case 0x44: // D — CUB
            screen.cursorColumn = max(0, screen.cursorColumn - parameter(0, default: 1))
        case 0x45: // E — CNL
            screen.cursorRow = min(screen.rows - 1, screen.cursorRow + parameter(0, default: 1))
            screen.cursorColumn = 0
        case 0x46: // F — CPL
            screen.cursorRow = max(0, screen.cursorRow - parameter(0, default: 1))
            screen.cursorColumn = 0
        case 0x47, 0x60: // G, ` — CHA / HPA
            screen.cursorColumn = clampColumn(parameter(0, default: 1) - 1)
        case 0x48, 0x66: // H, f — CUP / HVP
            screen.cursorRow = clampRow(parameter(0, default: 1) - 1)
            screen.cursorColumn = clampColumn(parameter(1, default: 1) - 1)
        case 0x49: // I — CHT
            for _ in 0..<parameter(0, default: 1) { tab() }
        case 0x4a: // J — ED
            eraseInDisplay(parameters.first ?? 0)
        case 0x4b: // K — EL
            eraseInLine(parameters.first ?? 0)
        case 0x4c: // L — IL
            screen.scrollDown(top: screen.cursorRow, bottom: scrollBottom,
                              count: parameter(0, default: 1), blank: blankCell)
        case 0x4d: // M — DL
            screen.scrollUp(top: screen.cursorRow, bottom: scrollBottom,
                            count: parameter(0, default: 1), blank: blankCell)
        case 0x50: // P — DCH
            deleteCharacters(parameter(0, default: 1))
        case 0x53: // S — SU
            screen.scrollUp(top: scrollTop, bottom: scrollBottom,
                            count: parameter(0, default: 1), blank: blankCell)
        case 0x54: // T — SD
            screen.scrollDown(top: scrollTop, bottom: scrollBottom,
                              count: parameter(0, default: 1), blank: blankCell)
        case 0x58: // X — ECH
            let count = parameter(0, default: 1)
            screen.clearRow(screen.cursorRow, from: screen.cursorColumn,
                            through: screen.cursorColumn + count - 1, blank: blankCell)
        case 0x5a: // Z — CBT
            backTab(parameter(0, default: 1))
        case 0x40: // @ — ICH
            insertCharacters(parameter(0, default: 1))
        case 0x64: // d — VPA
            screen.cursorRow = clampRow(parameter(0, default: 1) - 1)
        case 0x67: // g — TBC
            if (parameters.first ?? 0) == 3 { tabStops.removeAll() }
            else { tabStops.remove(screen.cursorColumn) }
        case 0x68: // h — SM / DECSET
            setModes(true)
        case 0x6c: // l — RM / DECRST
            setModes(false)
        case 0x6d: // m — SGR
            applySGR()
        case 0x72: // r — DECSTBM
            let top = clampRow(parameter(0, default: 1) - 1)
            let bottom = clampRow(parameters.count > 1 && parameters[1] != 0
                                  ? parameters[1] - 1 : screen.rows - 1)
            if top < bottom { scrollTop = top; scrollBottom = bottom }
            else { scrollTop = 0; scrollBottom = screen.rows - 1 }
            screen.cursorRow = scrollTop
            screen.cursorColumn = 0
        case 0x73: // s — save cursor
            savedCursor = (screen.cursorRow, screen.cursorColumn, attributes)
        case 0x75: // u — restore cursor, or kitty keyboard push/pop
            if parameterBytes.first == 0x3c || parameterBytes.first == 0x3e
                || parameterBytes.first == 0x3f {
                break // kitty keyboard protocol; nothing visible
            }
            if let saved = savedCursor {
                screen.cursorRow = saved.row
                screen.cursorColumn = saved.column
                attributes = saved.attributes
            }
        case 0x63, 0x6e, 0x70, 0x71, 0x74: // c n p q t — queries and reports
            break
        default:
            unsupportedSequences += 1
        }
    }

    private mutating func setModes(_ on: Bool) {
        guard isPrivate else { return }
        for mode in parameters {
            switch mode {
            case 7: autowrap = on
            case 25: screen.cursorVisible = on
            case 1047, 1049:
                switchAlternateScreen(on, clearOnEnter: true)
            case 1048:
                if on { savedCursor = (screen.cursorRow, screen.cursorColumn, attributes) }
                else if let saved = savedCursor {
                    screen.cursorRow = saved.row
                    screen.cursorColumn = saved.column
                    attributes = saved.attributes
                }
            default:
                break // mouse reporting, bracketed paste, focus events: invisible
            }
        }
    }

    private mutating func switchAlternateScreen(_ on: Bool, clearOnEnter: Bool) {
        if on {
            guard savedPrimary == nil else { return }
            savedPrimary = screen
            savedCursor = (screen.cursorRow, screen.cursorColumn, attributes)
            if clearOnEnter { screen.fillAll(blankCell) }
            screen.cursorRow = 0
            screen.cursorColumn = 0
            screen.isAlternateScreen = true
        } else {
            guard var primary = savedPrimary else { return }
            primary.cursorVisible = screen.cursorVisible
            screen = primary
            screen.isAlternateScreen = false
            savedPrimary = nil
            if let saved = savedCursor {
                screen.cursorRow = saved.row
                screen.cursorColumn = saved.column
                attributes = saved.attributes
            }
        }
    }

    // MARK: - SGR

    private mutating func applySGR() {
        // Re-parse with colons preserved so `38:2::r:g:b` is handled as well
        // as `38;2;r;g;b`. Both forms appear in the wild.
        let text = String(decoding: parameterBytes, as: UTF8.self)
        var codes: [Int] = []
        if text.isEmpty {
            codes = [0]
        } else {
            for group in text.split(separator: ";", omittingEmptySubsequences: false) {
                for part in group.split(separator: ":", omittingEmptySubsequences: false) {
                    codes.append(Int(part) ?? 0)
                }
                if group.contains(":") { codes.append(-1) } // group terminator marker
            }
            codes.removeAll { $0 == -1 }
            if codes.isEmpty { codes = [0] }
        }

        var index = 0
        while index < codes.count {
            let code = codes[index]
            switch code {
            case 0: attributes = CellAttributes()
            case 1: attributes.bold = true
            case 2: attributes.dim = true
            case 3: attributes.italic = true
            case 4: attributes.underline = true
            case 7: attributes.inverse = true
            case 9: attributes.strikethrough = true
            case 21, 22: attributes.bold = false; attributes.dim = false
            case 23: attributes.italic = false
            case 24: attributes.underline = false
            case 27: attributes.inverse = false
            case 29: attributes.strikethrough = false
            case 30...37: attributes.foreground = .indexed(UInt8(code - 30))
            case 39: attributes.foreground = nil
            case 40...47: attributes.background = .indexed(UInt8(code - 40))
            case 49: attributes.background = nil
            case 90...97: attributes.foreground = .indexed(UInt8(code - 90 + 8))
            case 100...107: attributes.background = .indexed(UInt8(code - 100 + 8))
            case 38, 48:
                let isForeground = code == 38
                guard index + 1 < codes.count else { index = codes.count; break }
                let kind = codes[index + 1]
                if kind == 5, index + 2 < codes.count {
                    let colour = TerminalColor.indexed(UInt8(clamping: codes[index + 2]))
                    if isForeground { attributes.foreground = colour } else { attributes.background = colour }
                    index += 2
                } else if kind == 2, index + 4 < codes.count {
                    let colour = TerminalColor.rgb(UInt8(clamping: codes[index + 2]),
                                                   UInt8(clamping: codes[index + 3]),
                                                   UInt8(clamping: codes[index + 4]))
                    if isForeground { attributes.foreground = colour } else { attributes.background = colour }
                    index += 4
                } else {
                    index = codes.count
                }
            default:
                break
            }
            index += 1
        }
    }

    // MARK: - OSC

    private mutating func finishOSC() {
        let payload = String(decoding: stringBuffer, as: UTF8.self)
        stringBuffer.removeAll(keepingCapacity: true)
        guard let separator = payload.firstIndex(of: ";") else { return }
        let code = String(payload[payload.startIndex..<separator])
        let value = String(payload[payload.index(after: separator)...])
        if code == "0" || code == "1" || code == "2" {
            windowTitle = value
        }
    }

    // MARK: - Primitives

    private func clampRow(_ row: Int) -> Int { min(max(0, row), screen.rows - 1) }
    private func clampColumn(_ column: Int) -> Int { min(max(0, column), screen.columns - 1) }

    private mutating func put(_ character: Character) {
        if pendingWrap && autowrap {
            screen.cursorColumn = 0
            lineFeed()
            pendingWrap = false
        }
        let width = character.terminalWidth
        if width == 0 {
            // A combining mark belongs to the cell that was just written.
            let target = max(0, screen.cursorColumn - 1)
            var cell = screen[screen.cursorRow, target]
            cell.character = Character(String(cell.character) + String(character))
            screen[screen.cursorRow, target] = cell
            return
        }
        if screen.cursorColumn + width > screen.columns {
            if autowrap {
                screen.cursorColumn = 0
                lineFeed()
            } else {
                screen.cursorColumn = screen.columns - width
            }
        }
        screen[screen.cursorRow, screen.cursorColumn] =
            ScreenCell(character: character, attributes: attributes)
        if width == 2 {
            screen[screen.cursorRow, screen.cursorColumn + 1] =
                ScreenCell(character: " ", attributes: attributes, isWideContinuation: true)
        }
        screen.cursorColumn += width
        if screen.cursorColumn >= screen.columns {
            // xterm's deferred wrap: the cursor sits on the last column and
            // only wraps when the *next* printable arrives. Wrapping eagerly
            // inserts a blank line every time a line exactly fills the width,
            // which for a box-drawn UI is every single frame.
            screen.cursorColumn = screen.columns - 1
            pendingWrap = true
        }
    }

    private mutating func lineFeed() {
        if screen.cursorRow == scrollBottom {
            screen.scrollUp(top: scrollTop, bottom: scrollBottom, count: 1, blank: blankCell)
        } else if screen.cursorRow < screen.rows - 1 {
            screen.cursorRow += 1
        }
    }

    private mutating func reverseIndex() {
        if screen.cursorRow == scrollTop {
            screen.scrollDown(top: scrollTop, bottom: scrollBottom, count: 1, blank: blankCell)
        } else if screen.cursorRow > 0 {
            screen.cursorRow -= 1
        }
    }

    private mutating func tab() {
        let next = tabStops.filter { $0 > screen.cursorColumn }.min() ?? (screen.columns - 1)
        screen.cursorColumn = clampColumn(next)
    }

    private mutating func backTab(_ count: Int) {
        for _ in 0..<max(1, count) {
            let previous = tabStops.filter { $0 < screen.cursorColumn }.max() ?? 0
            screen.cursorColumn = clampColumn(previous)
        }
    }

    private mutating func eraseInDisplay(_ mode: Int) {
        switch mode {
        case 0:
            screen.clearRow(screen.cursorRow, from: screen.cursorColumn,
                            through: screen.columns - 1, blank: blankCell)
            if screen.cursorRow + 1 <= screen.rows - 1 {
                for row in (screen.cursorRow + 1)...(screen.rows - 1) {
                    screen.clearRow(row, from: 0, through: screen.columns - 1, blank: blankCell)
                }
            }
        case 1:
            screen.clearRow(screen.cursorRow, from: 0, through: screen.cursorColumn, blank: blankCell)
            if screen.cursorRow > 0 {
                for row in 0...(screen.cursorRow - 1) {
                    screen.clearRow(row, from: 0, through: screen.columns - 1, blank: blankCell)
                }
            }
        default:
            screen.fillAll(blankCell)
        }
    }

    private mutating func eraseInLine(_ mode: Int) {
        switch mode {
        case 0:
            screen.clearRow(screen.cursorRow, from: screen.cursorColumn,
                            through: screen.columns - 1, blank: blankCell)
        case 1:
            screen.clearRow(screen.cursorRow, from: 0, through: screen.cursorColumn, blank: blankCell)
        default:
            screen.clearRow(screen.cursorRow, from: 0, through: screen.columns - 1, blank: blankCell)
        }
    }

    private mutating func insertCharacters(_ count: Int) {
        let row = screen.cursorRow
        let start = screen.cursorColumn
        let n = min(count, screen.columns - start)
        guard n > 0 else { return }
        var column = screen.columns - 1
        while column >= start + n {
            screen[row, column] = screen[row, column - n]
            column -= 1
        }
        screen.clearRow(row, from: start, through: start + n - 1, blank: blankCell)
    }

    private mutating func deleteCharacters(_ count: Int) {
        let row = screen.cursorRow
        let start = screen.cursorColumn
        let n = min(count, screen.columns - start)
        guard n > 0 else { return }
        for column in start..<(screen.columns - n) {
            screen[row, column] = screen[row, column + n]
        }
        screen.clearRow(row, from: screen.columns - n, through: screen.columns - 1, blank: blankCell)
    }

    private mutating func reset() {
        let columns = screen.columns, rows = screen.rows
        self = TerminalEmulator(columns: columns, rows: rows)
    }
}

// MARK: - UTF-8 accumulation

/// Decodes UTF-8 one byte at a time. The emulator is fed chunks that split
/// wherever the recording split them, so a decoder that requires whole
/// sequences would lose every character that straddles a boundary.
struct UTF8Accumulator: Sendable {
    private var buffer: [UInt8] = []
    private var expected = 0

    mutating func reset() { buffer.removeAll(keepingCapacity: true); expected = 0 }

    mutating func push(_ byte: UInt8) -> Unicode.Scalar? {
        if expected == 0 {
            switch byte {
            case 0x00...0x7f:
                return Unicode.Scalar(byte)
            case 0xc2...0xdf: expected = 1
            case 0xe0...0xef: expected = 2
            case 0xf0...0xf4: expected = 3
            default:
                return nil // stray continuation or invalid lead: drop it
            }
            buffer = [byte]
            return nil
        }
        guard byte & 0xc0 == 0x80 else {
            // Not a continuation. Abandon the partial sequence and
            // reinterpret this byte as a fresh lead.
            reset()
            return push(byte)
        }
        buffer.append(byte)
        expected -= 1
        guard expected == 0 else { return nil }
        let decoded = String(decoding: buffer, as: UTF8.self)
        buffer.removeAll(keepingCapacity: true)
        return decoded.unicodeScalars.first
    }
}

extension Character {
    /// Terminal cell width: 0 for combining marks and variation selectors,
    /// 2 for East Asian wide and the emoji ranges, 1 otherwise.
    ///
    /// This is a working approximation of `wcwidth`, not a full Unicode
    /// database. It covers what a coding CLI draws — box drawing (width 1),
    /// the spinner glyphs (width 1), and the occasional emoji (width 2).
    var terminalWidth: Int {
        guard let scalar = unicodeScalars.first else { return 1 }
        if unicodeScalars.count > 1 && unicodeScalars.contains(where: { $0.value == 0xFE0F }) {
            return 2
        }
        switch scalar.value {
        case 0x0300...0x036F, 0x200B...0x200F, 0xFE00...0xFE0F, 0x20D0...0x20FF:
            return 0
        case 0x1100...0x115F, 0x2E80...0x303E, 0x3041...0x33FF,
             0x3400...0x4DBF, 0x4E00...0x9FFF, 0xA000...0xA4CF,
             0xAC00...0xD7A3, 0xF900...0xFAFF, 0xFE30...0xFE6F,
             0xFF00...0xFF60, 0xFFE0...0xFFE6,
             0x1F300...0x1F64F, 0x1F900...0x1F9FF, 0x1FA70...0x1FAFF,
             0x20000...0x3FFFD:
            return 2
        default:
            return 1
        }
    }
}
