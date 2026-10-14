import Foundation
import Testing
@testable import CuaSpacesTranscript

/// Golden data: real recordings of the real CLI, parsed at named instants, and
/// compared byte-for-byte against a committed JSON-UI document.
///
/// The point is that a parser change which alters output *must* change a
/// golden, and that change shows up as a readable diff in review. Nothing here
/// is generated at test time; the expectations are files on disk.
///
/// To re-bless after an intentional change:
///
///     CUA_TRANSCRIPT_RECORD=1 swift test --filter GoldenTests
///
/// then read every changed `.json` before committing it. Re-blessing without
/// reading the diff defeats the whole mechanism.
@Suite(.serialized) final class GoldenTests {

    /// A golden is a recording plus the instants worth pinning.
    ///
    /// The timestamps are chosen, not sampled: each one is a moment where the
    /// screen shows something the parser is supposed to understand.
    struct Golden {
        let cast: String
        /// `(label, time, what this instant exercises)`
        let instants: [(String, TimeInterval, String)]
    }

    static let goldens: [Golden] = [
        Golden(cast: "claude-basic-turn", instants: [
            ("thinking", 8.5, "working indicator with no elapsed time yet, empty composer"),
            ("tool-running", 11.0, "an in-flight tool activity row and a token count"),
            ("complete", 13.5, "file edit with a real diff, agent message, completion line"),
        ]),
        Golden(cast: "claude-subagent-and-mode", instants: [
            ("mode-manual", 14.0, "the mode footer after cycling with shift+tab"),
            ("subagent-done", 40.0, "a backgrounded subagent, its completion row, a numbered list"),
        ]),
        Golden(cast: "claude-tool-error", instants: [
            ("read-failed", 30.0, "the agent reporting a failed read in prose"),
        ]),
        Golden(cast: "claude-bash-error", instants: [
            ("permission-question", 25.0, "a shell call parked on a four-option permission question"),
        ]),
        Golden(cast: "claude-plan-question", instants: [
            ("plan-approval", 30.0, "plan-mode approval question with a selected option"),
        ]),
        Golden(cast: "claude-interrupted", instants: [
            ("mid-turn", 13.2, "the agent working, before the interrupt"),
            ("after-escape", 16.0, "after Esc: 2.1.278 removes the turn rather than marking it"),
        ]),
        Golden(cast: "claude-awaiting-answer", instants: [
            ("auto-mode-answer", 18.0, "auto mode footer, composer holding a restored draft"),
        ]),
    ]

    private var isRecording: Bool {
        ProcessInfo.processInfo.environment["CUA_TRANSCRIPT_RECORD"] == "1"
    }

    @Test func testGoldensMatch() throws {
        var rewritten: [String] = []
        for golden in Self.goldens {
            let castURL = try Fixtures.url(golden.cast + ".cast")
            let player = try CastPlayer(contentsOf: castURL)

            // Every cast must say which build drew it. A golden whose CLI
            // version is unknown cannot be re-derived or argued with.
            XCTAssertNotNil(player.recording.header.cliVersion,
                            "\(golden.cast) has no recorded CLI version")

            for (label, time, purpose) in golden.instants {
                let frame = player.frame(at: time)
                let parsed = ClaudeCodeParser().parse(frame: frame)
                let actual = try parsed.jsonUIDocument().prettyJSONString() + "\n"
                let name = "\(golden.cast)@\(label).json"

                guard let expectedURL = try? Fixtures.url(name),
                      let expected = try? String(contentsOf: expectedURL, encoding: .utf8)
                else {
                    try Fixtures.write(actual, named: name)
                    rewritten.append(name)
                    continue
                }
                if expected == actual { continue }
                if isRecording {
                    try Fixtures.write(actual, named: name)
                    rewritten.append(name)
                    continue
                }
                XCTFail("""
                    golden \(name) differs (\(purpose))
                    Re-bless with CUA_TRANSCRIPT_RECORD=1 swift test --filter GoldenTests
                    and read the diff.

                    --- expected
                    \(expected)
                    --- actual
                    \(actual)
                    """)
            }
        }
        if !rewritten.isEmpty {
            XCTFail("wrote \(rewritten.count) golden(s): \(rewritten.joined(separator: ", ")). "
                    + "Review the diff and re-run.")
        }
    }

    /// The honesty rule, enforced rather than documented.
    ///
    /// Anything the parser emits unqualified must be quotable from the frame
    /// it came from. This test takes every `observed` element in every golden
    /// instant and checks that its principal strings actually appear on that
    /// screen. An element that passes this cannot be a hallucinated card.
    @Test func testObservedElementsAreQuotableFromTheirFrame() throws {
        for golden in Self.goldens {
            let player = try CastPlayer(contentsOf: try Fixtures.url(golden.cast + ".cast"))
            for (label, time, _) in golden.instants {
                let frame = player.frame(at: time)
                let flat = frame.text.replacingOccurrences(of: "\u{a0}", with: " ")
                for element in ClaudeCodeParser().parse(frame: frame).elements
                where element.provenance == .observed {
                    for quote in element.quotableStrings {
                        XCTAssertTrue(flat.contains(quote),
                                      """
                                      \(golden.cast)@\(label): an `observed` \
                                      \(element.typeName) claims \(quote.debugDescription) \
                                      but that text is not on the frame.
                                      """)
                    }
                }
            }
        }
    }

    /// The parser must never report a tool call as running unless it has said
    /// that the status is its own conclusion.
    @Test func testRunningStatusIsAlwaysLabelledInferred() throws {
        for golden in Self.goldens {
            let player = try CastPlayer(contentsOf: try Fixtures.url(golden.cast + ".cast"))
            for (label, time, _) in golden.instants {
                let parsed = ClaudeCodeParser().parse(frame: player.frame(at: time))
                for element in parsed.elements {
                    if case .toolCall(let call) = element, call.status == .running {
                        XCTAssertEqual(call.statusProvenance, .inferred,
                                       "\(golden.cast)@\(label): running is never on screen")
                    }
                    if case .subagent(let subagent) = element, subagent.status == .running {
                        XCTAssertEqual(subagent.statusProvenance, .inferred,
                                       "\(golden.cast)@\(label): running is never on screen")
                    }
                }
            }
        }
    }

    /// Every golden must round-trip through the JSON encoder to the same bytes
    /// twice, or a golden diff would be noise rather than signal.
    @Test func testEncodingIsStable() throws {
        let player = try CastPlayer(contentsOf: try Fixtures.url("claude-basic-turn.cast"))
        let parsed = ClaudeCodeParser().parse(frame: player.frame(at: 13.5))
        let first = try parsed.jsonUIDocument().prettyJSONString()
        let second = try parsed.jsonUIDocument().prettyJSONString()
        XCTAssertEqual(first, second)
    }
}

extension TranscriptElement {
    /// The strings an `observed` element asserts are on the screen.
    ///
    /// Deliberately excludes anything reassembled across rows: a wrapped
    /// agent message is stitched from several rows and no single row contains
    /// it, so quoting the whole thing would fail for the right reason and the
    /// wrong assertion. Single-row text is checked in full.
    var quotableStrings: [String] {
        switch self {
        case .toolCall(let value):
            return [value.name] + (value.argumentSummary.map { [$0] } ?? [])
        case .subagent(let value):
            return [value.kind] + (value.task.map { [$0] } ?? [])
        case .agentRunning(let value):
            return [value.rawLine]
        case .modeIndicator(let value):
            return [value.rawText]
        case .turnBoundary(let value):
            return [value.text]
        case .question(let value):
            return [value.prompt] + value.options.map(\.label)
        case .composer(let value):
            return value.content.contains("\n") ? [] : [value.content].filter { !$0.isEmpty }
        case .userMessage(let value):
            return value.text.contains("\n") ? [] : [value.text]
        case .agentMessage(let value):
            return value.text.contains("\n") ? [] : [value.text]
        case .diff, .error, .rawText:
            return []
        }
    }
}

enum Fixtures {
    static func url(_ name: String) throws -> URL {
        guard let url = Bundle.module.url(forResource: "Goldens/" + name, withExtension: nil)
                ?? Bundle.module.url(forResource: name, withExtension: nil,
                                     subdirectory: "Goldens")
        else {
            throw NSError(domain: "Fixtures", code: 1,
                          userInfo: [NSLocalizedDescriptionKey: "missing fixture \(name)"])
        }
        return url
    }

    /// Writing goes to the *source* tree, not the copied bundle: a golden that
    /// only exists in `.build` is not a golden.
    static func write(_ contents: String, named name: String) throws {
        let source = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .appendingPathComponent("Goldens")
            .appendingPathComponent(name)
        try contents.write(to: source, atomically: true, encoding: .utf8)
    }
}
