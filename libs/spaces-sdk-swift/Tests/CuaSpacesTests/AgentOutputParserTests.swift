import Foundation
import Testing
@testable import CuaSpaces

/// The three structural defects the parser had while it lived in an app, each
/// pinned here — in the SDK — because this is where the parser lives now.
///
/// All three destroyed the document *before* any renderer could see it, which
/// is what made them survivable: a consumer looking at flattened output cannot
/// tell whether the agent wrote it that way. `FRICTION.md` §98–§99 and §104.
@Suite(.serialized) final class AgentOutputParserStructureTests {

    // MARK: §98 — a blank line is a paragraph break until it is inside a fence

    /// A fenced code block survives a blank line inside it.
    ///
    /// `paragraphs` runs before anything markdown-aware, so a fence it does not
    /// understand is torn in half and the markers are stranded in two different
    /// segments. Most agents emit blank lines inside code.
    @Test func testAFencedCodeBlockIsNotSplitByABlankLineInsideIt() {
        let source = "here:\n\n```swift\nlet a = 1\n\nlet b = 2\n```\n\ndone"
        let parts = AgentOutputParser.paragraphs(in: source)
        XCTAssertTrue(parts.contains { $0.contains("let a = 1") && $0.contains("let b = 2") },
                      "the fence was torn in half: \(parts)")
    }

    @Test func testATildeFenceAlsoHoldsItsBlankLines() {
        let parts = AgentOutputParser.paragraphs(in: "~~~\na\n\nb\n~~~")
        XCTAssertEqual(parts, ["~~~\na\n\nb\n~~~"])
    }

    /// A shorter fence inside a longer one is content, not a close.
    @Test func testAShorterFenceDoesNotCloseALongerOne() {
        let parts = AgentOutputParser.paragraphs(in: "````\n```\na\n\nb\n```\n````")
        XCTAssertEqual(parts.count, 1, "the outer fence closed early: \(parts)")
    }

    /// Inline code is not a fence — otherwise a paragraph mentioning `` `a` ``
    /// would swallow the rest of the tail.
    @Test func testInlineCodeIsNotAFence() {
        XCTAssertFalse(AgentOutputParser.containsFence("call `a` then stop"))
        XCTAssertEqual(AgentOutputParser.paragraphs(in: "call `a`\n\nthen stop").count, 2)
    }

    /// Consecutive prose is one utterance; a richer shape still breaks the run.
    ///
    /// A markdown reply is blank-line separated by construction, so before this
    /// one answer arrived as six segments — a heading, its paragraph, a list and
    /// a code block each becoming its own bubble.
    @Test func testConsecutiveProseIsRejoinedButARicherShapeBreaksTheRun() {
        let segments = AgentOutputParser.segments(from: """
        ## Heading

        Some prose about it.

        - one
        - two

        Drafted email:
          Hi there
          Thanks

        after the card
        """)
        XCTAssertEqual(segments.count, 3, "the reply fragmented: \(segments)")
        guard case let .prose(first) = segments[0] else { return XCTFail("\(segments)") }
        XCTAssertEqual(first, "## Heading\n\nSome prose about it.\n\n- one\n- two")
        guard case .titled = segments[1] else { return XCTFail("\(segments)") }
        guard case .prose("after the card") = segments[2] else { return XCTFail("\(segments)") }
    }

    /// End to end: a heading, a prose line and a fenced block with a blank line
    /// in it are **one** segment with the fence intact.
    @Test func testAMarkdownAnswerWithCodeIsOneSegment() {
        let segments = AgentOutputParser.segments(from: """
        ## How

        Like this:

        ```swift
        let a = 1

        let b = 2
        ```
        """)
        XCTAssertEqual(segments.count, 1, "one answer became \(segments.count) bubbles")
        guard case let .prose(text) = segments[0] else { return XCTFail("\(segments)") }
        XCTAssertTrue(text.contains("let a = 1\n\nlet b = 2"),
                      "the fence lost its blank line: \(text.debugDescription)")
        XCTAssertEqual(text.components(separatedBy: "```").count - 1, 2,
                       "the fence markers were stranded: \(text.debugDescription)")
    }

    // MARK: §99 — the normalisation that ate the indentation

    /// Indentation inside a code block survives the parser.
    ///
    /// It did not: every line was trimmed before the paragraph was rebuilt, so
    /// nested lines came out flush left and the code in the transcript was code
    /// that would not compile. Invisible in a one-line fixture.
    @Test func testIndentationInsideACodeBlockSurvives() {
        let source = """
        ```swift
        func f() {
            if x {
                return 1
            }
        }
        ```
        """
        guard case let .prose(text)? = AgentOutputParser.segments(from: source).first else {
            return XCTFail("expected prose")
        }
        XCTAssertEqual(text, source, "the indentation was trimmed away: \(text.debugDescription)")
    }

    /// A nested list keeps its depth. The nesting *is* the meaning of the list.
    @Test func testANestedListKeepsItsIndentation() {
        let source = "- outer\n  - inner\n    - deeper"
        guard case let .prose(text)? = AgentOutputParser.segments(from: source).first else {
            return XCTFail("expected prose")
        }
        XCTAssertEqual(text, source, "the list was flattened onto one level")
    }

    // MARK: §104 — a `Then:` lead-in is a list, not a drafted email

    /// A `Foo:` lead-in over a markdown list is a **list**, not a titled block.
    ///
    /// The titled rule matches `Title:` followed by body lines, which is the
    /// drafted-email shape. A markdown list with a lead-in has exactly that
    /// shape, and being captured swallowed the list into `bodyLines`, where a
    /// consumer renders it flat with its `-` markers still showing.
    @Test func testALeadInOverAMarkdownListIsNotATitledBlock() {
        for body in ["- outer\n- inner", "1. one\n2. two", "# head\nmore", "> quoted\nmore"] {
            let segments = AgentOutputParser.segments(from: "Then:\n" + body)
            guard case .prose? = segments.first else {
                return XCTFail("markdown became a titled block: \(segments)")
            }
        }
    }

    /// …and the half that must not regress: a genuine email card still forms.
    @Test func testTheDraftedEmailCardStillForms() {
        let segments = AgentOutputParser.segments(from: "New email:\nHi Dan,\n2 PM works.")
        guard case let .titled(title, lines)? = segments.first else {
            return XCTFail("the email card stopped forming: \(segments)")
        }
        XCTAssertEqual(title, "New email")
        XCTAssertEqual(lines, ["Hi Dan,", "2 PM works."])
    }

    /// The space after the marker is required, so a titled block whose body
    /// merely *starts* with one of those characters is still a titled block.
    @Test func testMarkerLikeTextDoesNotDisqualifyATitledBlock() {
        XCTAssertFalse(AgentOutputParser.isMarkdownBlockLine("*emphasis* here"))
        XCTAssertFalse(AgentOutputParser.isMarkdownBlockLine("--flag value"))
        XCTAssertFalse(AgentOutputParser.isMarkdownBlockLine("1.5 seconds"))
        XCTAssertFalse(AgentOutputParser.isMarkdownBlockLine("#nospace"))
        XCTAssertTrue(AgentOutputParser.isMarkdownBlockLine("- one"))
        XCTAssertTrue(AgentOutputParser.isMarkdownBlockLine("  1) one"))
        XCTAssertTrue(AgentOutputParser.isMarkdownBlockLine("### head"))
        XCTAssertTrue(AgentOutputParser.isMarkdownBlockLine("> quote"))

        let segments = AgentOutputParser.segments(from: "Flags:\n--verbose prints more\n1.5 seconds")
        guard case let .titled(title, _)? = segments.first else {
            return XCTFail("a genuine titled block was refused: \(segments)")
        }
        XCTAssertEqual(title, "Flags")
    }
}
