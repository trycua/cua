import Foundation

/// Structure recovered from an agent's raw terminal output.
///
/// `FRICTION.md` §19 and §20: the harness publishes **one undifferentiated text
/// stream** — `agent_status` hands back `output_tail`, ANSI and all — and the
/// protocol gives no structure at all. Every app that wants a transcript rather
/// than a terminal therefore has to strip escape sequences, segment the stream,
/// and decide which chunk is prose and which is something richer. That is not
/// one app's taste; it is the same reverse-engineering, written again per app.
/// So it is here.
///
/// What stays out: **rendering**. The parser says "this paragraph is a choice
/// block with these options"; it does not know what a card looks like, and no
/// view type crosses into the SDK. The consumer maps `AgentOutputSegment` onto
/// its own message model, which is the part that is genuinely its own.
///
/// The rules are deliberately conservative. Anything not confidently one of the
/// richer shapes is `.prose` — a wrong guess that turns a line of output into a
/// card with buttons is worse than a plain bubble.
public enum AgentOutputSegment: Sendable, Hashable {
    /// Text, as written.
    case prose(String)
    /// A bare URL on its own line.
    case link(url: URL, host: String, kind: DocumentKind)
    /// A bare absolute path to a known document type — how an agent's produced
    /// artefact reaches a transcript.
    case file(path: String, name: String, directory: String, kind: DocumentKind)
    /// A block the agent emitted to ask the user to pick.
    case choices(AgentChoiceBlock)
    /// A single line narrating work on the computer rather than speaking to the
    /// user: "Opening Safari", "Working in Blender".
    case activity(String)
    /// A `Title:` line followed by an indented or quoted body.
    case titled(title: String, bodyLines: [String])

    /// The kinds of document an extension implies. Deliberately coarse: an app
    /// picks an icon from this, and a finer taxonomy would be this file
    /// guessing at a design system.
    public enum DocumentKind: String, Sendable, Hashable {
        case pdf, sheet, doc, slides, image, other
    }
}

/// A choice block an agent emitted.
///
/// ```
/// [[choices: <heading>]]
/// A) first option
/// B) second option
/// [[/choices]]
/// ```
///
/// **This marker format is the SDK's, and it exists to avoid a worse mistake.**
/// The protocol gives an agent no way to say "ask the user to pick", and the
/// choice-card content a client wants to show is *model output* arriving
/// over its own protocol, not app copy. So an app wanting choice cards has two
/// options: ship a handful of example questions as literals, so that every Bot
/// forever asks the same four questions about nothing — or give the agent a way
/// to say "ask this". This is the second. The card *component* is the app's;
/// what it says comes from the agent.
///
/// It lives in the SDK rather than in an app because a marker two parties agree
/// on is a protocol, and a protocol each app invents privately is the friction
/// §19 describes, one layer up.
///
/// The letters in the emitted block are ignored; positions decide them, so an
/// agent that writes `A) … C)` still yields two options.
public struct AgentChoiceBlock: Sendable, Hashable {
    public var heading: String
    public var options: [String]

    public init(heading: String, options: [String]) {
        self.heading = heading
        self.options = options
    }
}

/// Parses one agent's `output_tail` into segments. Pure — no Space, no I/O, no
/// view — so a consumer's transcript rules are testable without a machine.
public enum AgentOutputParser {

    public static let choiceOpenPrefix = "[[choices:"
    public static let choiceClose = "[[/choices]]"

    /// The acknowledgement vocabulary. A turn ending in one of these is an ack
    /// rather than prose, which is what lets a roster preview show "Done"
    /// verbatim instead of the paragraph before it.
    public static let acknowledgements: Set<String> = [
        "done", "sent", "ok", "okay", "ready", "finished", "complete", "completed",
        "saved", "sent.", "done.",
    ]

    /// Extensions that read as a document rather than a path.
    public static let documentKinds: [String: AgentOutputSegment.DocumentKind] = [
        "pdf": .pdf, "csv": .sheet, "tsv": .sheet, "xlsx": .sheet, "numbers": .sheet,
        "doc": .doc, "docx": .doc, "md": .doc, "txt": .doc, "rtf": .doc, "pages": .doc,
        "ppt": .slides, "pptx": .slides, "key": .slides,
        "png": .image, "jpg": .image, "jpeg": .image, "gif": .image, "webp": .image,
    ]

    /// Single-line output that reads as narration of work on the computer.
    public static let activityVerbs = [
        "working in ", "working on ", "opening ", "running ", "browsing ",
        "navigating to ", "typing into ", "clicking ", "searching ",
    ]

    // MARK: Entry point

    /// Parse one turn's worth of output.
    public static func segments(from output: String) -> [AgentOutputSegment] {
        let parts = paragraphs(in: strippingANSI(output)).compactMap(segment(for:))
        // Put prose back together.
        //
        // `paragraphs` splits on blank lines, which is right for finding the
        // *richer shapes* in a tail and wrong for everything between them: a
        // markdown reply is blank-line separated by construction, so every
        // heading, list and code block arrived as its own segment and one
        // answer came out as six bubbles. Consecutive prose is one utterance;
        // a titled block, a link/file row or an activity line still breaks the
        // run, because those genuinely are separate things.
        var out: [AgentOutputSegment] = []
        for part in parts {
            if case .prose(let next) = part, case .prose(let prev)? = out.last {
                out[out.count - 1] = .prose(prev + "\n\n" + next)
            } else {
                out.append(part)
            }
        }
        return out
    }

    public static func isAcknowledgement(_ text: String) -> Bool {
        let t = text.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        return !t.contains("\n") && acknowledgements.contains(t)
    }

    // MARK: Rules

    public static func choiceBlock(in paragraph: String) -> AgentChoiceBlock? {
        let lines = paragraph.split(separator: "\n").map {
            $0.trimmingCharacters(in: .whitespaces)
        }
        guard let first = lines.first,
              first.hasPrefix(choiceOpenPrefix),
              let end = first.range(of: "]]")
        else { return nil }
        let heading = String(first[first.index(first.startIndex,
                                               offsetBy: choiceOpenPrefix.count)..<end.lowerBound])
            .trimmingCharacters(in: .whitespaces)
        var options: [String] = []
        for line in lines.dropFirst() {
            if line == choiceClose { break }
            guard !line.isEmpty else { continue }
            // `A) text`, `A. text`, `- text`, or bare text. The label is
            // stripped because the renderer supplies its own.
            var text = line
            if text.count > 2 {
                let chars = Array(text)
                if chars[0].isLetter, chars[1] == ")" || chars[1] == "." {
                    text = String(chars.dropFirst(2))
                } else if chars[0] == "-" {
                    text = String(chars.dropFirst(1))
                }
            }
            let trimmed = text.trimmingCharacters(in: .whitespaces)
            if !trimmed.isEmpty { options.append(trimmed) }
        }
        guard !heading.isEmpty, options.count >= 2 else { return nil }
        return AgentChoiceBlock(heading: heading, options: options)
    }

    private static func segment(for paragraph: String) -> AgentOutputSegment? {
        if let block = choiceBlock(in: paragraph) { return .choices(block) }

        // A paragraph carrying a fence is code, and code is returned
        // **verbatim**.
        //
        // Everything below this line trims each line and drops the empty ones,
        // which is right for the shape heuristics and catastrophic for code: it
        // removes exactly the leading whitespace that makes the code mean what
        // it says, and closes up the blank lines between its functions. None of
        // the heuristics below can fire on a fenced block anyway.
        if containsFence(paragraph) {
            return .prose(paragraph.trimmingCharacters(in: .newlines))
        }

        let lines = paragraph.split(separator: "\n", omittingEmptySubsequences: false)
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty }
        guard let first = lines.first else { return nil }

        if lines.count == 1, let link = link(for: first) { return link }
        if lines.count == 1, let file = file(for: first) { return file }

        // `Title:` followed by an indented or quoted body is a titled block —
        // the drafted-email shape.
        //
        // **Unless the body is markdown.** `Then:` followed by bullets is a
        // list with a lead-in, not a drafted email, and treating it as a titled
        // block swallows the list into `bodyLines`, where a consumer renders it
        // as flat text with its `-` markers showing. The email shape this rule
        // is for has prose lines under it, never `- ` or `1. ` or `#`.
        if lines.count >= 2, first.hasSuffix(":"), lines.count <= 14,
           !lines.dropFirst().contains(where: isMarkdownBlockLine) {
            let title = String(first.dropLast()).trimmingCharacters(in: .whitespaces)
            if !title.isEmpty, title.count <= 60 {
                return .titled(title: title, bodyLines: Array(lines.dropFirst()))
            }
        }

        if lines.count == 1 {
            let lower = first.lowercased()
            if activityVerbs.contains(where: { lower.hasPrefix($0) }) {
                return .activity(first)
            }
        }

        // Everything else is prose, kept as written — **including its leading
        // whitespace**.
        //
        // `lines` above is trimmed and blank-filtered because the shape
        // heuristics need it that way. Returning prose built from it threw away
        // every indent, which flattened nested lists onto one level: a
        // sub-bullet came out as a sibling of the bullet it belonged to. The
        // heuristics keep their normalised view; the text the user reads is the
        // text the agent wrote.
        return .prose(paragraph.trimmingCharacters(in: .newlines))
    }

    /// Is this line a markdown *block* marker — a bullet, a numbered item, a
    /// heading or a quote?
    ///
    /// Used to keep the shape heuristics off markdown. The space after the
    /// marker is required, so `*emphasis*`, `--flag` and `1.5 seconds` are not
    /// list items and do not disqualify a genuine titled block.
    public static func isMarkdownBlockLine(_ line: String) -> Bool {
        let t = line.trimmingCharacters(in: .whitespaces)
        guard let f = t.first else { return false }
        if f == "-" || f == "*" || f == "+" { return t.dropFirst().first == " " }
        if f == ">" { return true }
        if f == "#" {
            let hashes = t.prefix { $0 == "#" }
            return (1...6).contains(hashes.count) && t.dropFirst(hashes.count).first == " "
        }
        let digits = t.prefix { $0.isNumber }
        guard !digits.isEmpty, digits.count <= 9 else { return false }
        let after = t.dropFirst(digits.count)
        guard let sep = after.first, sep == "." || sep == ")" else { return false }
        return after.dropFirst().first == " "
    }

    /// Does this paragraph open a fenced code block?
    public static func containsFence(_ paragraph: String) -> Bool {
        for raw in paragraph.split(separator: "\n", omittingEmptySubsequences: false) {
            let t = raw.trimmingCharacters(in: .whitespaces)
            guard let f = t.first, f == "`" || f == "~" else { continue }
            let run = t.prefix { $0 == f }
            guard run.count >= 3 else { continue }
            // A backtick fence's info string may not contain a backtick, which
            // is what keeps `` `a` `` an inline-code paragraph.
            if f == "`" && t.dropFirst(run.count).contains("`") { continue }
            return true
        }
        return false
    }

    private static func link(for line: String) -> AgentOutputSegment? {
        guard line.hasPrefix("http://") || line.hasPrefix("https://"),
              !line.contains(" "), let url = URL(string: line), let host = url.host
        else { return nil }
        return .link(url: url, host: host,
                     kind: documentKinds[url.pathExtension.lowercased()] ?? .other)
    }

    private static func file(for line: String) -> AgentOutputSegment? {
        guard !line.contains(" "), line.hasPrefix("/") || line.hasPrefix("~/") else { return nil }
        let ns = line as NSString
        guard let kind = documentKinds[ns.pathExtension.lowercased()] else { return nil }
        return .file(path: line, name: ns.lastPathComponent,
                     directory: ns.deletingLastPathComponent, kind: kind)
    }

    // MARK: Text handling

    /// Split a tail into blank-line separated paragraphs — **except inside a
    /// fenced code block**, where a blank line is content.
    ///
    /// Terminal output separates thoughts on blank lines far more reliably than
    /// it does anything else. But without the fence rule, an agent that emits a
    /// code block with a blank line in it (which is most of them) has that
    /// block torn in half, and the two halves reach the consumer as two
    /// segments with the fence markers stranded. The fence is tracked here
    /// rather than in a renderer because the split happens first and cannot be
    /// undone downstream.
    public static func paragraphs(in text: String) -> [String] {
        var out: [String] = []
        var current: [String] = []
        /// The open fence's character and length, when inside one.
        var fence: (Character, Int)?

        for raw in text.split(separator: "\n", omittingEmptySubsequences: false) {
            let line = String(raw)
            let trimmed = line.trimmingCharacters(in: .whitespaces)

            if let (char, length) = fence {
                current.append(line)
                if let f = trimmed.first, f == char {
                    let run = trimmed.prefix { $0 == char }
                    if run.count >= length,
                       trimmed.dropFirst(run.count).trimmingCharacters(in: .whitespaces).isEmpty {
                        fence = nil
                    }
                }
                continue
            }

            if let f = trimmed.first, f == "`" || f == "~" {
                let run = trimmed.prefix { $0 == f }
                if run.count >= 3 {
                    let info = trimmed.dropFirst(run.count)
                    if !(f == "`" && info.contains("`")) {
                        fence = (f, run.count)
                        current.append(line)
                        continue
                    }
                }
            }

            if trimmed.isEmpty {
                if !current.isEmpty { out.append(current.joined(separator: "\n")); current = [] }
            } else {
                current.append(line)
            }
        }
        if !current.isEmpty { out.append(current.joined(separator: "\n")) }
        return out
    }

    /// Strip CSI escape sequences and carriage returns. A live terminal tail is
    /// full of colour codes and progress redraws; left in, they render as
    /// mojibake inside a chat bubble.
    ///
    /// Works over unicode *scalars*, not `Character`s: Swift treats CR-LF as a
    /// single grapheme cluster, so a `Character`-level filter silently leaves
    /// every `\r\n` intact while looking like it worked.
    public static func strippingANSI(_ text: String) -> String {
        var out = String.UnicodeScalarView()
        var it = text.unicodeScalars.makeIterator()
        var pending: Unicode.Scalar? = nil
        while let c = pending ?? it.next() {
            pending = nil
            if c == "\u{1B}" {
                // ESC [ … <final byte in @..~>, or ESC ] … BEL/ST.
                guard let next = it.next() else { break }
                if next == "[" {
                    while let p = it.next() {
                        if ("\u{40}"..."\u{7E}").contains(p) { break }
                    }
                } else if next == "]" {
                    while let p = it.next() {
                        if p == "\u{07}" { break }
                        if p == "\u{1B}" { _ = it.next(); break }
                    }
                } else {
                    pending = next
                }
                continue
            }
            if c == "\r" { continue }
            out.append(c)
        }
        return String(out)
    }
}
