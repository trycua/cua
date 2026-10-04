import Foundation
import Testing
@testable import CuaSpacesTranscript

/// The emulator is the foundation everything else stands on. These are the
/// behaviours a naive implementation gets wrong, each one taken from something
/// Claude Code actually emits.
@Suite(.serialized) final class TerminalEmulatorTests {

    private func render(_ stream: String, columns: Int = 20, rows: Int = 5) -> ScreenBuffer {
        var emulator = TerminalEmulator(columns: columns, rows: rows)
        emulator.feed(stream)
        return emulator.screen
    }

    @Test func testAbsolutePositioningOverwritesRatherThanAppends() {
        // This is the whole reason the module exists: Claude Code draws a
        // spinner by homing, moving, and stamping one glyph. Concatenation
        // would give "abcXYZ"; the screen says something else entirely.
        let screen = render("abc\u{1b}[H\u{1b}[1;1HX")
        XCTAssertEqual(screen.line(0), "Xbc")
    }

    @Test func testColumnAddressingBetweenWords() {
        // Claude Code interleaves CHA between individual words.
        let screen = render("Enter\u{1b}[8Gto\u{1b}[11Gconfirm", columns: 40)
        XCTAssertEqual(screen.line(0), "Enter  to confirm")
    }

    @Test func testEraseInLineUsesCurrentBackgroundOnly() {
        var emulator = TerminalEmulator(columns: 10, rows: 2)
        emulator.feed("\u{1b}[1;31;44mXXXX\u{1b}[0m\u{1b}[1;1H\u{1b}[K")
        let cell = emulator.screen[0, 0]
        XCTAssertEqual(cell.character, " ")
        XCTAssertFalse(cell.attributes.bold, "erase must not inherit text attributes")
        XCTAssertNil(cell.attributes.foreground)
    }

    @Test func testDeferredWrapDoesNotInsertABlankLine() {
        // A line that exactly fills the width must not consume the next row.
        let screen = render("0123456789", columns: 10, rows: 3)
        XCTAssertEqual(screen.line(0), "0123456789")
        XCTAssertEqual(screen.line(1), "")
        XCTAssertEqual(screen.cursorRow, 0)
    }

    @Test func testAutowrapOnTheNextPrintable() {
        let screen = render("0123456789A", columns: 10, rows: 3)
        XCTAssertEqual(screen.line(0), "0123456789")
        XCTAssertEqual(screen.line(1), "A")
    }

    @Test func testScrollRegionScrollsOnlyItself() {
        var emulator = TerminalEmulator(columns: 6, rows: 5)
        emulator.feed("\u{1b}[1;1Htop\u{1b}[2;5r\u{1b}[5;1Ha\n\u{1b}[6;1Hb")
        XCTAssertEqual(emulator.screen.line(0), "top", "row 0 is outside the region")
    }

    @Test func testAlternateScreenRestoresThePrimaryBuffer() {
        var emulator = TerminalEmulator(columns: 12, rows: 3)
        emulator.feed("shell output")
        emulator.feed("\u{1b}[?1049h")
        XCTAssertTrue(emulator.screen.isAlternateScreen)
        emulator.feed("\u{1b}[1;1HTUI")
        XCTAssertEqual(emulator.screen.line(0), "TUI")
        emulator.feed("\u{1b}[?1049l")
        XCTAssertFalse(emulator.screen.isAlternateScreen)
        XCTAssertEqual(emulator.screen.line(0), "shell output")
    }

    @Test func testSGRTrueColourAndIndexedForms() {
        var emulator = TerminalEmulator(columns: 4, rows: 1)
        emulator.feed("\u{1b}[38;5;81mA\u{1b}[48;2;10;20;30mB")
        XCTAssertEqual(emulator.screen[0, 0].attributes.foreground, .indexed(81))
        XCTAssertEqual(emulator.screen[0, 1].attributes.background, .rgb(10, 20, 30))
    }

    @Test func testUTF8SplitAcrossFeedBoundaries() {
        // Box drawing is three bytes; a cast chunk splits wherever the kernel
        // split it.
        var emulator = TerminalEmulator(columns: 4, rows: 1)
        let bytes = Array("─".utf8)
        emulator.feed([bytes[0]])
        emulator.feed([bytes[1], bytes[2]])
        XCTAssertEqual(emulator.screen.line(0), "─")
    }

    @Test func testOSCTitleIsCaptured() {
        var emulator = TerminalEmulator(columns: 8, rows: 1)
        emulator.feed("\u{1b}]0;✳ working\u{07}")
        XCTAssertEqual(emulator.windowTitle, "✳ working")
    }

    @Test func testUnsupportedSequencesAreCountedNotSwallowed() {
        var emulator = TerminalEmulator(columns: 4, rows: 1)
        XCTAssertEqual(emulator.unsupportedSequences, 0)
        emulator.feed("\u{1b}P1;2|junk\u{1b}\\")
        XCTAssertEqual(emulator.unsupportedSequences, 1,
                       "a frame built over sequences we ignored must be able to say so")
    }

    @Test func testInsertAndDeleteCharacters() {
        var emulator = TerminalEmulator(columns: 8, rows: 1)
        emulator.feed("abcdef\u{1b}[1;1H\u{1b}[2@")
        XCTAssertEqual(emulator.screen.line(0), "  abcdef".prefix(8).description)
        emulator.feed("\u{1b}[1;1H\u{1b}[2P")
        XCTAssertEqual(emulator.screen.line(0), "abcdef")
    }
}
