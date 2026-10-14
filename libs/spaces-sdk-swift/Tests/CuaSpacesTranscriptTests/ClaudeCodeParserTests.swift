import Foundation
import Testing
@testable import CuaSpacesTranscript

/// Unit tests for the rules, driven by synthetic frames so each rule can be
/// probed in isolation and — more importantly — so the *refusals* can be
/// tested. The goldens prove the parser reads real output; these prove it
/// declines to read things that are not there.
@Suite(.serialized) final class ClaudeCodeParserTests {

    /// Build a frame the way the CLI would: on the alternate screen.
    private func frame(_ lines: [String], columns: Int = 100, rows: Int = 34,
                       cursorRow: Int = 0) -> RenderedFrame {
        var emulator = TerminalEmulator(columns: columns, rows: rows)
        emulator.feed("\u{1b}[?1049h\u{1b}[1;1H")
        emulator.feed(lines.joined(separator: "\r\n"))
        emulator.feed("\u{1b}[\(cursorRow + 1);1H")
        return RenderedFrame(screen: emulator.screen, windowTitle: emulator.windowTitle,
                             requestedTime: 1, effectiveTime: 1, eventsApplied: 1,
                             unsupportedSequences: emulator.unsupportedSequences,
                             unsupportedResize: false,
                             cli: "claude", cliVersion: "test")
    }

    private func parse(_ lines: [String], cursorRow: Int = 0) -> ParsedFrame {
        ClaudeCodeParser().parse(frame: frame(lines, cursorRow: cursorRow))
    }

    // MARK: - Degradation

    @Test func testAFrameOnThePrimaryScreenIsNotParsedAsTheAgentUI() {
        // `claude -p`, an exited session, or plain shell scrollback. The
        // layout profile says nothing matched rather than pretending.
        var emulator = TerminalEmulator(columns: 40, rows: 4)
        emulator.feed("⏺ Update(calc.py)\r\n  ⎿  Added 4 lines")
        let rendered = RenderedFrame(screen: emulator.screen, windowTitle: "",
                                     requestedTime: 0, effectiveTime: 0, eventsApplied: 1,
                                     unsupportedSequences: 0, unsupportedResize: false,
                                     cli: "claude", cliVersion: "test")
        let parsed = ClaudeCodeParser().parse(frame: rendered)
        XCTAssertNil(parsed.layoutProfile)
        XCTAssertTrue(parsed.elements.allSatisfy { $0.provenance == .unrecognised })
    }

    @Test func testAnUnknownLayoutDegradesToTextRatherThanMisParsing() {
        let parsed = parse([
            "┏━ Something Claude Code has never drawn ━┓",
            "┃ status: whatever                        ┃",
            "┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┛",
        ])
        XCTAssertTrue(parsed.elements.allSatisfy { $0.provenance == .unrecognised })
        // Nothing is dropped: a consumer can always render the text.
        guard case .rawText(let raw) = parsed.elements.first else {
            return XCTFail("expected rawText")
        }
        XCTAssertTrue(raw.lines.contains { $0.contains("never drawn") })
    }

    @Test func testAnUnknownModeFooterKeepsItsWordsAndRefusesToGuess() {
        let parsed = parse(["  ⏵⏵ hyperdrive mode on (shift+tab to cycle)"])
        guard case .modeIndicator(let mode) = parsed.elements.first else {
            return XCTFail("expected a mode indicator")
        }
        XCTAssertEqual(mode.mode, .unrecognised)
        XCTAssertEqual(mode.provenance, .unrecognised)
        XCTAssertTrue(mode.rawText.contains("hyperdrive"))
    }

    // MARK: - Refusals

    @Test func testProseIsNotTurnedIntoAToolCall() {
        // "Reading the docs helped." starts with an activity verb but is not
        // indented as an activity row; and "Consider(this)" is not a tool.
        let parsed = parse([
            "⏺ Consider(this) carefully, and check the value(s).",
        ])
        XCTAssertFalse(parsed.elements.contains { if case .toolCall = $0 { return true }; return false })
    }

    @Test func testAQuestionMarkWithoutOptionsIsNotAQuestion() {
        let parsed = parse(["⏺ Should I keep going?", "", "  Probably."])
        XCTAssertFalse(parsed.elements.contains { if case .question = $0 { return true }; return false })
    }

    @Test func testTwoRulesWithoutAPromptAreNotAComposer() {
        let parsed = parse([
            String(repeating: "─", count: 60),
            "  just some text",
            String(repeating: "─", count: 60),
        ])
        XCTAssertFalse(parsed.elements.contains { if case .composer = $0 { return true }; return false })
    }

    @Test func testAgentProseAboutAnErrorIsNotAnErrorBanner() {
        let parsed = parse(["⏺ The read failed: \"File does not exist.\" Nothing was changed."])
        XCTAssertFalse(parsed.elements.contains { if case .error = $0 { return true }; return false })
        guard case .agentMessage = parsed.elements.first else {
            return XCTFail("expected an agent message")
        }
    }

    // MARK: - Readings

    @Test func testToolCallWithResultIsObservedAndSucceeded() {
        let parsed = parse(["⏺ Update(calc.py)", "  ⎿  Added 4 lines"])
        guard case .toolCall(let call) = parsed.elements.first else {
            return XCTFail("expected a tool call")
        }
        XCTAssertEqual(call.name, "Update")
        XCTAssertEqual(call.argumentSummary, "calc.py")
        XCTAssertEqual(call.status, .succeeded)
        XCTAssertEqual(call.statusProvenance, .observed)
        XCTAssertEqual(call.provenance, .observed)
    }

    @Test func testToolCallWithoutAResultIsRunningAndSaysThatIsInferred() {
        let parsed = parse(["⏺ Read(calc.py)"])
        guard case .toolCall(let call) = parsed.elements.first else {
            return XCTFail("expected a tool call")
        }
        XCTAssertEqual(call.status, .running)
        XCTAssertEqual(call.statusProvenance, .inferred,
                       "nothing on screen says `running`")
    }

    @Test func testFailedToolResultIsRead() {
        let parsed = parse(["⏺ Read(missing.txt)", "  ⎿  Error: File does not exist."])
        guard case .toolCall(let call) = parsed.elements.first else {
            return XCTFail("expected a tool call")
        }
        XCTAssertEqual(call.status, .failed)
        XCTAssertEqual(call.statusProvenance, .observed)
    }

    @Test func testCollapsedResultIsFlagged() {
        let parsed = parse(["⏺ Bash(ls)", "  ⎿  120 lines (ctrl+o to expand)"])
        guard case .toolCall(let call) = parsed.elements.first else {
            return XCTFail("expected a tool call")
        }
        XCTAssertTrue(call.isCollapsed)
    }

    @Test func testBackgroundedSubagentIsRunningNotCompleted() {
        let parsed = parse(["⏺ Explore(List functions)",
                            "  ⎿  Backgrounded agent (↓ to manage · ctrl+o to expand)"])
        guard case .subagent(let subagent) = parsed.elements.first else {
            return XCTFail("expected a subagent")
        }
        XCTAssertEqual(subagent.kind, "Explore")
        XCTAssertEqual(subagent.task, "List functions")
        XCTAssertEqual(subagent.status, .running)
        XCTAssertEqual(subagent.statusProvenance, .inferred)
    }

    @Test func testSubagentCompletionRowIsObserved() {
        let parsed = parse(["⏺ Agent \"List functions in calc.py\" finished · 4s"])
        guard case .subagent(let subagent) = parsed.elements.first else {
            return XCTFail("expected a subagent")
        }
        XCTAssertEqual(subagent.status, .succeeded)
        XCTAssertEqual(subagent.statusProvenance, .observed)
        XCTAssertEqual(subagent.task, "List functions in calc.py")
    }

    @Test func testWorkingIndicatorReadsElapsedAndTokens() {
        let parsed = parse(["✽ Unraveling… (5s · ↓ 1,248 tokens · esc to interrupt)"])
        guard case .agentRunning(let running) = parsed.elements.first else {
            return XCTFail("expected the working indicator")
        }
        XCTAssertEqual(running.spinner, "✽")
        XCTAssertEqual(running.verb, "Unraveling")
        XCTAssertEqual(running.elapsedSeconds, 5)
        XCTAssertEqual(running.tokens, 1248)
        XCTAssertEqual(running.hint, "esc to interrupt")
    }

    @Test func testCompletionLineIsATurnBoundaryNotAWorkingIndicator() {
        let parsed = parse(["✻ Cogitated for 5s · done 5:58 PM"])
        guard case .turnBoundary(let boundary) = parsed.elements.first else {
            return XCTFail("expected a turn boundary")
        }
        XCTAssertEqual(boundary.kind, .completed)
    }

    @Test func testInterruptionNoticeIsReadWhenTheCLIDrawsOne() {
        // 2.1.278 removes the turn instead of marking it, so this rule is
        // exercised synthetically. It is kept because other builds draw it,
        // and because inventing an interruption when nothing says so is the
        // failure the honesty rule forbids.
        let parsed = parse(["⏺ [Request interrupted by user]"])
        XCTAssertTrue(parsed.elements.contains {
            if case .turnBoundary(let boundary) = $0 { return boundary.kind == .interrupted }
            return false
        })
    }

    @Test func testComposerPlaceholderIsDistinguishedFromTypedText() {
        var emulator = TerminalEmulator(columns: 60, rows: 6)
        emulator.feed("\u{1b}[?1049h\u{1b}[1;1H")
        emulator.feed(String(repeating: "─", count: 60) + "\r\n")
        emulator.feed("❯ \u{1b}[2mTry \"fix the build\"\u{1b}[0m\r\n")
        emulator.feed(String(repeating: "─", count: 60))
        emulator.feed("\u{1b}[2;5H")
        let rendered = RenderedFrame(screen: emulator.screen, windowTitle: "",
                                     requestedTime: 0, effectiveTime: 0, eventsApplied: 1,
                                     unsupportedSequences: 0, unsupportedResize: false,
                                     cli: "claude", cliVersion: "test")
        let parsed = ClaudeCodeParser().parse(frame: rendered)
        guard case .composer(let composer) = parsed.elements.first else {
            return XCTFail("expected a composer")
        }
        XCTAssertEqual(composer.content, "", "a placeholder is not user content")
        XCTAssertEqual(composer.placeholder, "Try \"fix the build\"")
        XCTAssertTrue(composer.cursorIsInside)
    }

    @Test func testDiffSignsAreReadWhenDrawnAndInferredWhenOnlyColoured() {
        let drawn = parse(["⏺ Update(calc.py)",
                           "  ⎿  Added 1 line",
                           "       5  def subtract(a, b):",
                           "       6 +def multiply(a, b):"])
        guard let diff = drawn.elements.compactMap({ element -> Diff? in
            if case .diff(let value) = element { return value }; return nil
        }).first else { return XCTFail("expected a diff") }
        XCTAssertEqual(diff.provenance, .observed)
        XCTAssertEqual(diff.addedCount, 1)
        XCTAssertEqual(diff.hunkLines.first?.text, "def subtract(a, b):")

        // Same rows, colour only: now the classification is ours.
        var emulator = TerminalEmulator(columns: 60, rows: 5)
        emulator.feed("\u{1b}[?1049h\u{1b}[1;1H")
        emulator.feed("⏺ Update(calc.py)\r\n  ⎿  Added 1 line\r\n")
        emulator.feed("\u{1b}[48;5;22m       6  def multiply(a, b):\u{1b}[0m")
        let rendered = RenderedFrame(screen: emulator.screen, windowTitle: "",
                                     requestedTime: 0, effectiveTime: 0, eventsApplied: 1,
                                     unsupportedSequences: 0, unsupportedResize: false,
                                     cli: "claude", cliVersion: "test")
        guard let coloured = ClaudeCodeParser().parse(frame: rendered).elements
            .compactMap({ element -> Diff? in
                if case .diff(let value) = element { return value }; return nil
            }).first else { return XCTFail("expected a colour-derived diff") }
        XCTAssertEqual(coloured.provenance, .inferred)
        XCTAssertEqual(coloured.addedCount, 1)
    }

    @Test func testActivitySummaryIsAlwaysInferred() {
        let parsed = parse(["  Read 1 file"])
        guard case .toolCall(let call) = parsed.elements.first else {
            return XCTFail("expected a tool call")
        }
        XCTAssertEqual(call.provenance, .inferred,
                       "the CLI never wrote a tool name here")
        XCTAssertEqual(call.status, .unknown,
                       "a folded row says nothing about whether it is finished")
    }

    @Test func testMarkdownBlockStructureSurvivesABlankRow() {
        let parsed = parse(["⏺ Two functions:",
                            "",
                            "  1. add(a, b)",
                            "  2. subtract(a, b)"])
        guard case .agentMessage(let message) = parsed.elements.first else {
            return XCTFail("expected one agent message")
        }
        XCTAssertTrue(message.markdown.contains("1. add(a, b)"))
        XCTAssertTrue(message.markdown.contains("\n\n"), "the paragraph break is kept")
    }

    // MARK: - JSON-UI

    @Test func testEveryElementCarriesItsProvenanceIntoTheDocument() throws {
        let parsed = parse(["⏺ Read(calc.py)", "  Read 1 file"])
        let json = try parsed.jsonUIDocument().prettyJSONString()
        XCTAssertTrue(json.contains("\"schema\" : \"cua.transcript.jsonui\\/1\"")
                      || json.contains("\"schema\" : \"cua.transcript.jsonui/1\""))
        XCTAssertTrue(json.contains("\"provenance\""))
        XCTAssertTrue(json.contains("\"statusProvenance\""))
    }

    @Test func testRenderingARawOutputTailWorksWithoutARecording() {
        // The `RunSnapshot.outputTail` path: a terminal tail and nothing else.
        let tail = "\u{1b}[?1049h\u{1b}[1;1H⏺ Update(calc.py)\r\n  ⎿  Added 4 lines"
        let rendered = RenderedFrame.render(stream: tail, columns: 80, rows: 10,
                                            cli: "claude", cliVersion: "2.1.278")
        let parsed = ClaudeCodeParser().parse(frame: rendered)
        XCTAssertEqual(parsed.cliVersion, "2.1.278")
        XCTAssertTrue(parsed.elements.contains {
            if case .toolCall(let call) = $0 { return call.name == "Update" }
            return false
        })
    }
}
