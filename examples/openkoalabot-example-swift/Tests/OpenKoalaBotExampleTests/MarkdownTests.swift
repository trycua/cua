// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation
import Testing
import SwiftUI
@testable import OpenKoalaBotExample

/// The markdown block parser.
///
/// Pure — no Space, no window server, no fixtures. The cases here are the ones
/// that decide whether an agent's reply reads as code or as debris: a fence
/// that contains backticks, a fence that never closes because the reply is
/// still streaming, nested lists, and links whose URL has parentheses in it.
@Suite final class MarkdownParserTests {

    // MARK: Fences

    @Test func testFencedCodeBlockKeepsItsLanguageAndBody() {
        let blocks = MarkdownParser.blocks(from: """
            Here you go:

            ```swift
            let x = 1
            print(x)
            ```
            """)
        XCTAssertEqual(blocks, [
            .paragraph("Here you go:"),
            .code(language: "swift", text: "let x = 1\nprint(x)"),
        ])
    }

    /// A three-backtick fence must be able to hold backticks. An agent
    /// explaining markdown, or printing a shell command with a substitution,
    /// does this constantly.
    @Test func testFenceMayContainBackticks() {
        let blocks = MarkdownParser.blocks(from: """
            ```sh
            echo `date`
            x=``
            ```
            """)
        XCTAssertEqual(blocks, [.code(language: "sh", text: "echo `date`\nx=``")])
    }

    /// A longer fence holds a shorter one verbatim — the only way to show a
    /// fenced block *inside* a fenced block.
    @Test func testLongerFenceHoldsAShorterFenceAsContent() {
        let blocks = MarkdownParser.blocks(from: """
            ````md
            ```swift
            let x = 1
            ```
            ````
            """)
        XCTAssertEqual(blocks, [
            .code(language: "md", text: "```swift\nlet x = 1\n```"),
        ])
    }

    /// A shorter closer cannot close a longer fence.
    @Test func testShorterFenceDoesNotCloseALongerOne() {
        let blocks = MarkdownParser.blocks(from: """
            ````
            ```
            still inside
            ````
            """)
        XCTAssertEqual(blocks, [.code(language: nil, text: "```\nstill inside")])
    }

    /// Streaming output is read mid-write, so the *common* case is a fence
    /// with no closer yet. It must still be a code block: rendering it as
    /// prose means the user watches their code arrive as literal backticks and
    /// then reflow when the fence lands.
    @Test func testUnterminatedFenceStillRendersAsCode() {
        let blocks = MarkdownParser.blocks(from: """
            Working on it:

            ```python
            def f():
                return 1
            """)
        XCTAssertEqual(blocks, [
            .paragraph("Working on it:"),
            .code(language: "python", text: "def f():\n    return 1"),
        ])
    }

    /// An unterminated fence must not swallow a closing fence that *does*
    /// arrive later in the same reply.
    @Test func testSecondFencePairIsNotSwallowedByTheFirst() {
        let blocks = MarkdownParser.blocks(from: """
            ```
            a
            ```

            ```
            b
            ```
            """)
        XCTAssertEqual(blocks, [
            .code(language: nil, text: "a"),
            .code(language: nil, text: "b"),
        ])
    }

    /// Inline code on a line of prose is not a fence opening.
    @Test func testInlineCodeIsNotAFence() {
        let blocks = MarkdownParser.blocks(from: "Run `ls -la` first.")
        XCTAssertEqual(blocks, [.paragraph("Run `ls -la` first.")])
    }

    @Test func testBlankLinesInsideAFenceAreKept() {
        let blocks = MarkdownParser.blocks(from: "```\na\n\nb\n```")
        XCTAssertEqual(blocks, [.code(language: nil, text: "a\n\nb")])
    }

    @Test func testTildeFenceIsAFence() {
        XCTAssertEqual(MarkdownParser.blocks(from: "~~~js\nlet a=1\n~~~"),
                       [.code(language: "js", text: "let a=1")])
    }

    // MARK: Lists

    @Test func testNestedBulletsCarryTheirDepth() {
        let blocks = MarkdownParser.blocks(from: """
            - top
              - nested
                - deeper
            - back
            """)
        XCTAssertEqual(blocks, [
            .listItem(depth: 0, marker: nil, text: "top"),
            .listItem(depth: 1, marker: nil, text: "nested"),
            .listItem(depth: 2, marker: nil, text: "deeper"),
            .listItem(depth: 0, marker: nil, text: "back"),
        ])
    }

    @Test func testOrderedListsKeepTheirOwnNumbers() {
        let blocks = MarkdownParser.blocks(from: """
            1. first
            2. second
            10) tenth
            """)
        XCTAssertEqual(blocks, [
            .listItem(depth: 0, marker: "1.", text: "first"),
            .listItem(depth: 0, marker: "2.", text: "second"),
            .listItem(depth: 0, marker: "10)", text: "tenth"),
        ])
    }

    @Test func testOrderedAndBulletedListsCanNest() {
        let blocks = MarkdownParser.blocks(from: """
            1. build
               - debug
               - release
            2. ship
            """)
        XCTAssertEqual(blocks, [
            .listItem(depth: 0, marker: "1.", text: "build"),
            .listItem(depth: 1, marker: nil, text: "debug"),
            .listItem(depth: 1, marker: nil, text: "release"),
            .listItem(depth: 0, marker: "2.", text: "ship"),
        ])
    }

    /// The space after the marker is required, so none of these are lists.
    /// Without that rule `*emphasis*` opens a bullet and `--flag` opens
    /// another, which is how a paragraph of prose turns into a ragged list.
    @Test func testMarkerLikeTextIsNotAList() {
        XCTAssertEqual(MarkdownParser.blocks(from: "*emphasis* matters"),
                       [.paragraph("*emphasis* matters")])
        XCTAssertEqual(MarkdownParser.blocks(from: "--flag=value"),
                       [.paragraph("--flag=value")])
        XCTAssertEqual(MarkdownParser.blocks(from: "1.5 seconds"),
                       [.paragraph("1.5 seconds")])
    }

    // MARK: Headings, quotes, rules

    @Test func testHeadingsCarryTheirLevel() {
        XCTAssertEqual(MarkdownParser.blocks(from: "# One"),
                       [.heading(level: 1, text: "One")])
        XCTAssertEqual(MarkdownParser.blocks(from: "### Three"),
                       [.heading(level: 3, text: "Three")])
    }

    /// ATX requires the space, so a hashtag is prose.
    @Test func testHashWithoutASpaceIsNotAHeading() {
        XCTAssertEqual(MarkdownParser.blocks(from: "#nofilter"),
                       [.paragraph("#nofilter")])
        XCTAssertEqual(MarkdownParser.blocks(from: "####### seven"),
                       [.paragraph("####### seven")])
    }

    @Test func testBlockQuoteLinesJoinIntoOneQuote() {
        XCTAssertEqual(MarkdownParser.blocks(from: "> one\n> two"),
                       [.quote("one\ntwo")])
    }

    @Test func testThematicBreak() {
        XCTAssertEqual(MarkdownParser.blocks(from: "a\n\n---\n\nb"),
                       [.paragraph("a"), .rule, .paragraph("b")])
    }

    @Test func testConsecutiveProseLinesAreOneParagraphAndBlankLinesSplitThem() {
        XCTAssertEqual(MarkdownParser.blocks(from: "one\ntwo\n\nthree"),
                       [.paragraph("one\ntwo"), .paragraph("three")])
    }

    // MARK: Inline

    /// A link whose URL contains balanced parentheses — the Wikipedia case,
    /// and the one a naive `](...)` scan truncates halfway.
    @Test func testLinkWithParenthesesInItsURLSurvives() {
        let s = MarkdownParser.inline("see [Swift](https://en.wikipedia.org/wiki/Swift_(bird))")
        let link = s.runs.compactMap(\.link).first
        XCTAssertEqual(link?.absoluteString, "https://en.wikipedia.org/wiki/Swift_(bird)")
        XCTAssertTrue(String(s.characters).contains("see Swift"),
                      "link text was lost: \(String(s.characters))")
    }

    @Test func testInlineBoldItalicAndCodeAreMarked() {
        let s = MarkdownParser.inline("**bold** and *it* and `code`")
        // The markers themselves must not survive into the rendered text.
        XCTAssertEqual(String(s.characters), "bold and it and code")
        XCTAssertTrue(s.runs.contains { $0.inlinePresentationIntent?.contains(.code) == true },
                      "inline code was not marked")
        XCTAssertTrue(s.runs.contains { $0.inlinePresentationIntent?.contains(.stronglyEmphasized) == true },
                      "bold was not marked")
    }

    /// Malformed inline markup must degrade to the text the agent wrote, not
    /// to an empty bubble.
    @Test func testMalformedInlineFallsBackToLiteralText() {
        let s = MarkdownParser.inline("unclosed [link](")
        XCTAssertFalse(String(s.characters).isEmpty)
    }

    // MARK: The plain-path gate

    /// Ordinary prose still takes the plain `Text` path, so the common reply
    /// is laid out by exactly the code that laid it out before.
    @Test func testOrdinaryProseIsPlain() {
        XCTAssertTrue(MarkdownParser.isPlain("Done — the build passed."))
        XCTAssertTrue(MarkdownParser.isPlain("No markdown here at all"))
    }

    @Test func testAnythingWithStructureIsNotPlain() {
        for source in ["```\ncode\n```", "- a\n- b", "# Heading", "> quoted",
                       "has **bold**", "has `code`", "see [x](https://e.com)"] {
            XCTAssertFalse(MarkdownParser.isPlain(source),
                           "\(source.debugDescription) should not take the plain path")
        }
    }

    // MARK: Rendering

    /// The renderer must accept every block kind without trapping. Building
    /// the view is enough to catch a `ForEach` id collision or a bad range
    /// subscript in the inline styler.
    @MainActor
    @Test func testRendererAcceptsEveryBlockKind() {
        let source = """
            # Heading

            Prose with **bold**, `code` and a [link](https://example.com/a_(b)).

            - one
              - nested
            1. first

            > quoted

            ---

            ```swift
            let x = 1
            ```
            """
        for theme in [DesktopTheme.dark, DesktopTheme.light] {
            let view = MarkdownText(source: source, theme: theme, maxWidth: 400)
            XCTAssertNotNil(ImageRenderer(content: view.frame(width: 400)).cgImage,
                            "the markdown renderer produced no image")
        }
    }

    /// The code-block styling numbers are the ones in the token table, not the
    /// untokenised olive/moss hexes. Those are
    /// scaffolding; this asserts none of them reached the palette.
    @Test func testCodePaletteUsesSyntaxTokensAndNotTheScaffoldingHexes() {
        let scaffolding: Set<UInt32> = [0x20231F, 0xA9C85D, 0xBFE86B, 0xC5F467, 0x1A1D19, 0x9BA392]
        for theme in [DesktopTheme.dark, DesktopTheme.light] {
            let p = MarkdownPalette.forTheme(theme)
            for hex in scaffolding {
                XCTAssertNotEqual(p.codeBackground, Color(hex: UInt32(hex)))
                XCTAssertNotEqual(p.codeForeground, Color(hex: UInt32(hex)))
            }
        }
        // The syntax background, both modes.
        XCTAssertEqual(MarkdownPalette.forTheme(.dark).codeBackground, Color(hex: 0x181818))
        XCTAssertEqual(MarkdownPalette.forTheme(.light).codeBackground, Color(hex: 0xFCFCFC))
        // Code block: 11pt/12pt padding, 8pt radius, 11pt type.
        XCTAssertEqual(MD.codePadV, 11)
        XCTAssertEqual(MD.codePadH, 12)
        XCTAssertEqual(MD.codeSize, 11)
        XCTAssertEqual(Corner.base, 8)
    }
}
