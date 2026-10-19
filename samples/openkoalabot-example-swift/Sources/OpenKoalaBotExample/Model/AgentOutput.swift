// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpaces
import Foundation

/// Maps the SDK's parsed agent output onto **this app's** transcript rows.
///
/// The parsing itself moved to `CuaSpaces.AgentOutputParser`. The reason is the
/// friction it comes from: §19 and §20 say the harness publishes one
/// undifferentiated text stream and the protocol gives no structure, so *every*
/// app on Spaces has to strip ANSI, segment the stream and classify each chunk.
/// That is the same reverse-engineering written once per app, which makes it
/// SDK work by definition.
///
/// What stayed here is the part that is genuinely this app's: which of
/// OpenKoalaBots's message shapes — prose bubble, card with buttons, link/file
/// chip, Agent Computer glyph, choice card — a given segment becomes. The SDK
/// says "this paragraph is a titled block with these body lines"; it does not
/// know what an OpenKoalaBots card looks like, and no view type crosses into it.
enum AgentOutputParser {

    // The vocabulary lives in the SDK now. These re-exports keep the call
    // sites, and the tests that pin them, spelled as they were.
    static var acknowledgements: Set<String> { CuaSpaces.AgentOutputParser.acknowledgements }
    static var choiceOpenPrefix: String { CuaSpaces.AgentOutputParser.choiceOpenPrefix }
    static var choiceClose: String { CuaSpaces.AgentOutputParser.choiceClose }
    static var computerVerbs: [String] { CuaSpaces.AgentOutputParser.activityVerbs }

    /// Parse one turn's worth of agent output into message bodies.
    ///
    /// - Parameter active: whether the run is still `running`, which decides
    ///   whether a computer-status glyph is drawn live or spent. This is a
    ///   rendering decision, so it stays on this side of the boundary.
    static func bodies(from output: String, active: Bool) -> [MessageBody] {
        CuaSpaces.AgentOutputParser.segments(from: output).map { body(for: $0, active: active) }
    }

    /// The single line the roster shows as a Bot's preview: its last utterance.
    /// Falls back to the harness's own `summary` when there is no output yet.
    static func preview(from output: String, fallback: String) -> String {
        for b in bodies(from: output, active: false).reversed() {
            switch b {
            case .prose(let t):
                return t.split(separator: "\n").last.map(String.init) ?? t
            case .card(let c):      return c.title
            case .linkFile(let lf): return lf.title
            case .computerStatus(let cs): return cs.text
            default: continue
            }
        }
        return fallback
    }

    static func isAcknowledgement(_ text: String) -> Bool {
        CuaSpaces.AgentOutputParser.isAcknowledgement(text)
    }

    static func paragraphs(in text: String) -> [String] {
        CuaSpaces.AgentOutputParser.paragraphs(in: text)
    }

    static func strippingANSI(_ text: String) -> String {
        CuaSpaces.AgentOutputParser.strippingANSI(text)
    }

    /// The `[[choices: …]]` block, as this app's card type.
    static func choiceCard(in paragraph: String) -> ChoiceCard? {
        CuaSpaces.AgentOutputParser.choiceBlock(in: paragraph)
            .map { ChoiceCard(heading: $0.heading, options: $0.options) }
    }

    // MARK: - Segment -> this app's rows

    private static func body(for segment: AgentOutputSegment, active: Bool) -> MessageBody {
        switch segment {
        case let .prose(text):
            return .prose(text)
        case let .choices(block):
            return .choices(ChoiceCard(heading: block.heading, options: block.options))
        case let .link(url, host, kind):
            let name = url.lastPathComponent.isEmpty || url.lastPathComponent == "/"
                ? host : url.lastPathComponent
            return .linkFile(LinkFile(title: name, subtitle: host, kind: LinkFile.Kind(kind)))
        case let .file(_, name, directory, kind):
            return .linkFile(LinkFile(title: name, subtitle: directory, kind: LinkFile.Kind(kind)))
        case let .titled(title, bodyLines):
            return .card(Card(title: title, bodyLines: bodyLines,
                              primary: "Open", secondary: "Dismiss"))
        case let .activity(text):
            return .computerStatus(ComputerStatus(text: text, active: active))
        }
    }
}

private extension LinkFile.Kind {
    /// The SDK reports a coarse document kind; picking the chip is this app's.
    init(_ kind: AgentOutputSegment.DocumentKind) {
        switch kind {
        case .pdf: self = .pdf
        case .sheet: self = .sheet
        case .doc: self = .doc
        case .slides: self = .slides
        case .image: self = .image
        case .other: self = .file
        }
    }
}
