// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import SwiftUI

// MARK: - Block model

/// The block structure of an agent's reply.
///
/// `AttributedString(markdown:)` is an **inline** parser. It will give you
/// bold, italic, inline code and links inside one run of text, and it will
/// give you nothing at all for the things that matter most in an agent's
/// answer: a fenced code block is not laid out as a block, a list is not
/// indented, a heading is not a heading. Those are *block* constructs, and
/// they have to be parsed before any attributed string is built, which is
/// what this type is. Inline styling is still `AttributedString`'s job, on the
/// text of each block, and that split is deliberate: block structure here,
/// inline styling there, and no third parser.
enum MarkdownBlock: Equatable {
    case heading(level: Int, text: String)
    case paragraph(String)
    /// `language` is the fence's info string, `nil` when the fence carried none.
    case code(language: String?, text: String)
    /// One list item. `ordered` carries the rendered marker (`1.`); a bullet
    /// carries `nil` and is drawn with the theme's disc.
    case listItem(depth: Int, marker: String?, text: String)
    case quote(String)
    /// A `---` / `***` / `___` thematic break.
    case rule
}

// MARK: - Parser

enum MarkdownParser {

    /// Parse `source` into blocks.
    ///
    /// Deliberately not a CommonMark implementation: it is the subset an
    /// agent actually emits, parsed precisely, with the ambiguous cases pinned
    /// by tests rather than left to chance. The three that bite:
    ///
    /// * **A fence may contain backticks.** The closing fence must use the same
    ///   character and be *at least as long* as the opening one, and carry
    ///   nothing else on its line. So ```` ``` ```` opens a block that can hold
    ///   `` ` `` and ``` `` ``` freely, and a four-backtick fence can hold a
    ///   three-backtick one verbatim.
    /// * **A fence may be unterminated.** Streaming output is read mid-write
    ///   all the time, so the common case is a fence with no closer yet. It
    ///   runs to the end of the input and still renders as a code block,
    ///   never as literal backticks, and never swallowing the rest as prose.
    /// * **Indentation is a list's depth, not a code block.** Four-space
    ///   indented code is not emitted by these agents, and treating it as code
    ///   turns every nested bullet into a grey slab.
    static func blocks(from source: String) -> [MarkdownBlock] {
        var out: [MarkdownBlock] = []
        // `omittingEmptySubsequences: false` so blank lines survive: they are
        // the paragraph separator and are significant inside a fence.
        let lines = source.replacingOccurrences(of: "\r\n", with: "\n")
            .split(separator: "\n", omittingEmptySubsequences: false).map(String.init)

        var paragraph: [String] = []
        var quoted: [String] = []

        func flushParagraph() {
            let joined = paragraph.joined(separator: "\n")
                .trimmingCharacters(in: .whitespacesAndNewlines)
            if !joined.isEmpty { out.append(.paragraph(joined)) }
            paragraph = []
        }
        func flushQuote() {
            let joined = quoted.joined(separator: "\n")
                .trimmingCharacters(in: .whitespacesAndNewlines)
            if !joined.isEmpty { out.append(.quote(joined)) }
            quoted = []
        }
        func flushAll() { flushParagraph(); flushQuote() }

        var i = 0
        while i < lines.count {
            let line = lines[i]
            let trimmed = line.trimmingCharacters(in: .whitespaces)

            // --- Fenced code -------------------------------------------------
            if let fence = Fence(opening: trimmed) {
                flushAll()
                var body: [String] = []
                var j = i + 1
                var closed = false
                while j < lines.count {
                    let candidate = lines[j].trimmingCharacters(in: .whitespaces)
                    if fence.closes(candidate) { closed = true; break }
                    body.append(lines[j])
                    j += 1
                }
                out.append(.code(language: fence.language,
                                 text: body.joined(separator: "\n")))
                // An unterminated fence consumed the rest of the input; a
                // closed one consumes its closing line too.
                i = closed ? j + 1 : j
                continue
            }

            // --- Blank ------------------------------------------------------
            if trimmed.isEmpty { flushAll(); i += 1; continue }

            // --- Thematic break ---------------------------------------------
            if isRule(trimmed) { flushAll(); out.append(.rule); i += 1; continue }

            // --- Heading ----------------------------------------------------
            if let h = heading(trimmed) {
                flushAll(); out.append(h); i += 1; continue
            }

            // --- Block quote -------------------------------------------------
            if trimmed.hasPrefix(">") {
                flushParagraph()
                var rest = String(trimmed.dropFirst())
                if rest.hasPrefix(" ") { rest.removeFirst() }
                quoted.append(rest)
                i += 1; continue
            }

            // --- List item ----------------------------------------------------
            if let item = listItem(line) {
                flushAll(); out.append(item); i += 1; continue
            }

            // --- Paragraph -----------------------------------------------------
            flushQuote()
            paragraph.append(trimmed)
            i += 1
        }
        flushAll()
        return out
    }

    // MARK: Fences

    /// An open fence, remembering what it takes to close it.
    private struct Fence {
        let char: Character
        let length: Int
        let language: String?

        init?(opening trimmed: String) {
            guard let first = trimmed.first, first == "`" || first == "~" else { return nil }
            let run = trimmed.prefix { $0 == first }
            guard run.count >= 3 else { return nil }
            let info = trimmed.dropFirst(run.count).trimmingCharacters(in: .whitespaces)
            // A backtick fence's info string may not itself contain a backtick;
            // that is how ``` `a` ``` stays an inline-code paragraph rather
            // than opening a block.
            if first == "`" && info.contains("`") { return nil }
            self.char = first
            self.length = run.count
            self.language = info.isEmpty ? nil : info
        }

        /// A closing fence is the same character, at least as long, and alone
        /// on its line. Anything else is content.
        func closes(_ trimmed: String) -> Bool {
            guard let f = trimmed.first, f == char else { return false }
            let run = trimmed.prefix { $0 == char }
            guard run.count >= length else { return false }
            return trimmed.dropFirst(run.count).trimmingCharacters(in: .whitespaces).isEmpty
        }
    }

    // MARK: Line kinds

    private static func isRule(_ t: String) -> Bool {
        guard let f = t.first, f == "-" || f == "*" || f == "_" else { return false }
        let body = t.filter { !$0.isWhitespace }
        return body.count >= 3 && body.allSatisfy { $0 == f }
    }

    private static func heading(_ t: String) -> MarkdownBlock? {
        let hashes = t.prefix { $0 == "#" }
        guard (1...6).contains(hashes.count) else { return nil }
        let rest = t.dropFirst(hashes.count)
        // `#hashtag` is not a heading: ATX requires the space.
        guard rest.first == " " else { return nil }
        let text = rest.trimmingCharacters(in: .whitespaces)
        guard !text.isEmpty else { return nil }
        return .heading(level: hashes.count, text: text)
    }

    /// A bullet or ordered item, with its nesting depth taken from the leading
    /// whitespace. Two spaces per level, which is what these agents emit, and
    /// four-space indents therefore read as two levels rather than as code.
    private static func listItem(_ raw: String) -> MarkdownBlock? {
        let indent = raw.prefix { $0 == " " || $0 == "\t" }
            .reduce(0) { $0 + ($1 == "\t" ? 4 : 1) }
        let t = raw.trimmingCharacters(in: .whitespaces)
        let depth = min(indent / 2, 4)

        // Bullet: `-`, `*`, `+` followed by a space. The space is required, so
        // `*emphasis*` and `--flag` are not list items.
        if let f = t.first, f == "-" || f == "*" || f == "+" {
            let rest = t.dropFirst()
            if rest.first == " " {
                return .listItem(depth: depth, marker: nil,
                                 text: rest.trimmingCharacters(in: .whitespaces))
            }
            return nil
        }

        // Ordered: digits then `.` or `)` then a space.
        let digits = t.prefix { $0.isNumber }
        guard !digits.isEmpty, digits.count <= 9 else { return nil }
        let after = t.dropFirst(digits.count)
        guard let sep = after.first, sep == "." || sep == ")" else { return nil }
        let rest = after.dropFirst()
        guard rest.first == " " else { return nil }
        return .listItem(depth: depth, marker: "\(digits)\(sep)",
                         text: rest.trimmingCharacters(in: .whitespaces))
    }

    // MARK: Inline

    /// Inline styling for one block's text: bold, italic, inline code, links.
    ///
    /// `.inlineOnlyPreservingWhitespace` is the important argument. The default
    /// parses a whole document and collapses the very whitespace that a block
    /// parser has already made significant; this option keeps the text as-is
    /// and applies nothing but inline runs.
    static func inline(_ text: String) -> AttributedString {
        let options = AttributedString.MarkdownParsingOptions(
            allowsExtendedAttributes: true,
            interpretedSyntax: .inlineOnlyPreservingWhitespace,
            failurePolicy: .returnPartiallyParsedIfPossible)
        // A malformed span must degrade to the literal text the agent wrote,
        // never to an empty bubble.
        guard let parsed = try? AttributedString(markdown: text, options: options) else {
            return AttributedString(text)
        }
        return parsed
    }

    /// Does this text carry any construct the block renderer would treat
    /// differently from plain `Text`? Used to keep the plain path plain.
    static func isPlain(_ text: String) -> Bool {
        let blocks = blocks(from: text)
        guard blocks.count == 1, case .paragraph(let p) = blocks[0] else { return false }
        return p == text.trimmingCharacters(in: .whitespacesAndNewlines)
            && inline(text) == AttributedString(p)
    }
}

// MARK: - Styling

/// The markdown surface's colours.
///
/// These are the syntax foreground on the syntax background with the
/// secondary stroke, from the token table. Untokenised olive/moss hexes
/// (`#1a1d19`, `#20231f`, `#a9c85d` and the rest) appear nowhere else in the
/// system and are deliberately not used. A test asserts none of them reached
/// this palette.
struct MarkdownPalette {
    var codeBackground: Color
    var codeForeground: Color
    var codeBorder: Color

    static let darkSyntax = MarkdownPalette(
        codeBackground: Color(hex: 0x181818),
        codeForeground: Color(hex: 0xD6D6DD),
        codeBorder: Color(hex: 0x2E2E2E))

    static let lightSyntax = MarkdownPalette(
        codeBackground: Color(hex: 0xFCFCFC),
        codeForeground: Color(hex: 0x141414).opacity(0.92),
        codeBorder: Color(hex: 0xE4E4E4))

    static func forTheme(_ theme: DesktopTheme) -> MarkdownPalette {
        var p = theme.isDark ? darkSyntax : lightSyntax
        p.codeBorder = theme.divider
        return p
    }
}

/// Everything the renderer needs that is not the markdown itself.
///
/// The two transcripts are set on different scales (the desktop shell's body
/// is 12pt, the mobile canvas's is 44 on a 1024-wide design surface), so the
/// metrics are carried as a **scale factor** rather than duplicated. One set of
/// ratios, two surfaces, and no second copy of the numbers to drift.
struct MarkdownStyle {
    var body: CGFloat
    var text: Color
    var secondary: Color
    var accent: Color
    var divider: Color
    var palette: MarkdownPalette

    /// How much larger this surface is than the desktop shell, whose sizes the
    /// metrics are written in. Every padding, indent and rail width scales by it.
    var scale: CGFloat { body / MD.body }

    static func desktop(_ theme: DesktopTheme) -> MarkdownStyle {
        MarkdownStyle(body: MD.body, text: theme.text, secondary: theme.secondary,
                      accent: theme.accent, divider: theme.divider,
                      palette: .forTheme(theme))
    }

    /// The phone transcript. Light ground, so light syntax colours.
    static let mobile = MarkdownStyle(
        body: DS.bodySize, text: DS.onSurface, secondary: DS.secondary,
        accent: Color(hex: 0x2E7BE9), divider: Color(hex: 0xD8D8D8),
        palette: MarkdownPalette(codeBackground: Color(hex: 0xFFFFFF),
                                 codeForeground: Color(hex: 0x141414).opacity(0.92),
                                 codeBorder: Color(hex: 0xD8D8D8)))
}

/// Metrics for the markdown surface.
enum MD {
    /// Code block padding: 11pt vertical, 12pt horizontal.
    static let codePadV: CGFloat = 11
    static let codePadH: CGFloat = 12
    /// Vertical margin around a code block.
    static let codeMargin: CGFloat = 10
    /// 11pt with a 1.55 line *height*, so the extra leading is
    /// `(1.55 - 1) * 11`.
    static let codeSize: CGFloat = 11
    static let codeLineSpacing: CGFloat = 11 * 0.55
    /// The list's leading indent.
    static let listIndent: CGFloat = 12
    /// The gap between a marker and its text.
    static let markerGap: CGFloat = 6
    /// The blockquote's leading rail width.
    static let quoteRail: CGFloat = 2
    static let quoteInset: CGFloat = 10
    /// Vertical rhythm between blocks.
    static let blockGap: CGFloat = 6
    /// Body text size in the live shell's transcript.
    static let body: CGFloat = 12

    /// Heading sizes. The type scale tops out well below a web `h1`, because
    /// this is a 12pt transcript: a heading that doubles the body size reads
    /// as a bug in a chat bubble.
    static func headingSize(_ level: Int) -> CGFloat {
        switch level {
        case 1: return 16
        case 2: return 14
        case 3: return 13
        default: return 12
        }
    }
}

// MARK: - Renderer

/// An agent's reply, rendered as markdown.
///
/// Block structure is laid out here; inline styling is `AttributedString`'s.
struct MarkdownText: View {
    var source: String
    var theme: DesktopTheme
    /// The width available to a code block, so it can scroll rather than push
    /// the bubble wide.
    var maxWidth: CGFloat

    private var palette: MarkdownPalette { .forTheme(theme) }

    var body: some View {
        VStack(alignment: .leading, spacing: MD.blockGap) {
            ForEach(Array(MarkdownParser.blocks(from: source).enumerated()), id: \.offset) { _, block in
                view(for: block)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    @ViewBuilder
    private func view(for block: MarkdownBlock) -> some View {
        switch block {
        case .heading(let level, let text):
            styled(text, size: MD.headingSize(level),
                   weight: level <= 2 ? .bold : .semibold)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.top, 2)

        case .paragraph(let text):
            styled(text, size: MD.body, weight: .regular)
                .frame(maxWidth: .infinity, alignment: .leading)
                .fixedSize(horizontal: false, vertical: true)

        case .code(let language, let text):
            codeBlock(language: language, text: text)

        case .listItem(let depth, let marker, let text):
            HStack(alignment: .firstTextBaseline, spacing: MD.markerGap) {
                Text(marker ?? "•")
                    .font(marker == nil
                          ? DS.font(MD.body)
                          : .system(size: MD.body, design: .monospaced))
                    .foregroundStyle(theme.secondary)
                    // A hanging indent: the marker keeps its own column so
                    // wrapped text lines up under the text, not the bullet.
                    .frame(minWidth: 14, alignment: .leading)
                styled(text, size: MD.body, weight: .regular)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .padding(.leading, CGFloat(depth) * MD.listIndent)

        case .quote(let text):
            HStack(alignment: .top, spacing: MD.quoteInset) {
                RoundedRectangle(cornerRadius: MD.quoteRail / 2)
                    .fill(theme.secondary.opacity(0.4))
                    .frame(width: MD.quoteRail)
                styled(text, size: MD.body, weight: .regular)
                    .foregroundStyle(theme.secondary)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .fixedSize(horizontal: false, vertical: true)

        case .rule:
            Rectangle().fill(theme.divider).frame(height: 1)
                .padding(.vertical, 2)
        }
    }

    /// A fenced code block: monospaced, on its own ground, scrolling sideways
    /// rather than wrapping. Wrapping is the thing to avoid: a broken line of
    /// code is worse than a line you have to scroll to, because it reads as
    /// code that does not compile.
    private func codeBlock(language: String?, text: String) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            if let language, !language.isEmpty {
                Text(language)
                    .font(.system(size: 9, weight: .medium, design: .monospaced))
                    .foregroundStyle(theme.secondary)
                    .textCase(.lowercase)
            }
            ScrollView(.horizontal, showsIndicators: false) {
                Text(text)
                    .font(.system(size: MD.codeSize, design: .monospaced))
                    .foregroundStyle(palette.codeForeground)
                    .lineSpacing(MD.codeLineSpacing)
                    .textSelection(.enabled)
                    // No wrapping: the block scrolls instead.
                    .fixedSize(horizontal: true, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .padding(.horizontal, MD.codePadH)
        .padding(.vertical, MD.codePadV)
        .frame(maxWidth: maxWidth, alignment: .leading)
        .background(
            RoundedRectangle(cornerRadius: Corner.base, style: .continuous)
                .fill(palette.codeBackground))
        .overlay(
            RoundedRectangle(cornerRadius: Corner.base, style: .continuous)
                .strokeBorder(palette.codeBorder, lineWidth: 1))
        .padding(.vertical, MD.codeMargin - MD.blockGap)
    }

    /// One block's text with its inline runs styled.
    private func styled(_ text: String, size: CGFloat, weight: Font.Weight) -> Text {
        var attributed = MarkdownParser.inline(text)
        // `AttributedString` marks inline code and links but does not style
        // them: the base font would render `code` in the body face, and a
        // link would be invisible. Both are set per run here.
        for run in attributed.runs {
            let range = run.range
            if run.inlinePresentationIntent?.contains(.code) == true {
                attributed[range].font = .system(size: size * 0.92, design: .monospaced)
                attributed[range].foregroundColor = theme.accent
            }
            if run.link != nil {
                attributed[range].foregroundColor = theme.accent
                attributed[range].underlineStyle = .single
            }
        }
        return Text(attributed)
            .font(DS.font(size, weight))
            .foregroundStyle(theme.text)
    }
}
