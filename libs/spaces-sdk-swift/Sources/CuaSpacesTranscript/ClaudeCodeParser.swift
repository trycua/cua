import Foundation

/// Stage one: a rendered frame becomes a typed model of what is on screen.
///
/// The parser is a *reader*, not an interpreter. It is allowed to say "row 11
/// contains a tool-call row for `Update` with the argument `calc.py`" because
/// that is what the characters say. It is not allowed to say "the agent called
/// the Edit tool with `{file_path: ...}`", because the frame does not contain
/// that and inventing it is precisely the failure this module exists to
/// prevent (`FRICTION.md` §93).
///
/// Everything that is a conclusion rather than a reading is marked `.inferred`
/// and travels to the consumer that way. Everything the parser cannot classify
/// comes back as `.rawText` — degrading to plain text is the required
/// behaviour when Claude Code's layout moves, not an error.
public struct ClaudeCodeParser: Sendable {

    /// The layout this parser was written against.
    public static let layoutProfile = "claude-code/2.x"

    public init() {}

    public func parse(frame: RenderedFrame) -> ParsedFrame {
        let lines = frame.lines
        var elements: [TranscriptElement] = []

        // A frame that is not on the alternate screen is not the agent UI.
        // Claude Code enters the alternate buffer for its TUI; shell
        // scrollback, an exited session, or a `claude -p` run is not something
        // this layout profile describes, and claiming otherwise would be the
        // whole defect. Hand the text back and say so.
        guard frame.screen.isAlternateScreen else {
            return ParsedFrame(elements: nonEmptyRawText(lines),
                               cli: frame.cli, cliVersion: frame.cliVersion,
                               layoutProfile: nil,
                               frameTime: frame.effectiveTime,
                               unsupportedSequences: frame.unsupportedSequences,
                               isAlternateScreen: false)
        }

        var index = 0
        var unclassified: [(Int, String)] = []

        func flushUnclassified() {
            guard !unclassified.isEmpty else { return }
            let body = unclassified.map(\.1)
            if body.contains(where: { !$0.trimmed.isEmpty }) {
                elements.append(.rawText(RawText(
                    lines: body,
                    region: FrameRegion(firstRow: unclassified[0].0,
                                        lastRow: unclassified[unclassified.count - 1].0))))
            }
            unclassified.removeAll()
        }

        while index < lines.count {
            let line = lines[index]

            if let composer = parseComposer(lines: lines, at: index) {
                flushUnclassified()
                elements.append(.composer(composerElement(composer, frame: frame)))
                index = composer.lastRow + 1
                continue
            }
            if let question = parseQuestion(lines: lines, at: index) {
                flushUnclassified()
                elements.append(.question(question.value))
                index = question.nextRow
                continue
            }
            if let mode = parseModeIndicator(line: line, row: index) {
                flushUnclassified()
                elements.append(.modeIndicator(mode))
                index += 1
                continue
            }
            // Order matters: the completion line reuses a spinner glyph
            // ("✻ Cogitated for 5s · done 5:58 PM"), so it has to be claimed
            // as a turn boundary before the working-indicator rule sees it.
            if let boundary = parseTurnBoundary(line: line, row: index) {
                flushUnclassified()
                elements.append(.turnBoundary(boundary))
                index += 1
                continue
            }
            if let running = parseAgentRunning(line: line, row: index) {
                flushUnclassified()
                elements.append(.agentRunning(running))
                index += 1
                continue
            }
            if let banner = parseErrorBanner(line: line, row: index) {
                flushUnclassified()
                elements.append(.error(banner))
                index += 1
                continue
            }
            if let bullet = parseBulletRow(lines: lines, frame: frame, at: index) {
                flushUnclassified()
                elements.append(contentsOf: bullet.elements)
                index = bullet.nextRow
                continue
            }
            if let summary = parseActivitySummary(lines: lines, at: index) {
                flushUnclassified()
                elements.append(.toolCall(summary.value))
                index = summary.nextRow
                continue
            }
            if let user = parseUserMessage(lines: lines, at: index) {
                flushUnclassified()
                elements.append(.userMessage(user.value))
                index = user.nextRow
                continue
            }

            unclassified.append((index, line))
            index += 1
        }
        flushUnclassified()

        return ParsedFrame(elements: elements,
                           cli: frame.cli, cliVersion: frame.cliVersion,
                           layoutProfile: elements.contains(where: { $0.provenance != .unrecognised })
                               ? Self.layoutProfile : nil,
                           frameTime: frame.effectiveTime,
                           unsupportedSequences: frame.unsupportedSequences,
                           isAlternateScreen: true)
    }

    // MARK: - Raw fallback

    private func nonEmptyRawText(_ lines: [String]) -> [TranscriptElement] {
        var out: [TranscriptElement] = []
        var run: [(Int, String)] = []
        func flush() {
            if run.contains(where: { !$0.1.trimmed.isEmpty }) {
                out.append(.rawText(RawText(lines: run.map(\.1),
                                            region: FrameRegion(firstRow: run[0].0,
                                                                lastRow: run[run.count - 1].0))))
            }
            run.removeAll()
        }
        for (row, line) in lines.enumerated() {
            if line.trimmed.isEmpty && run.isEmpty { continue }
            run.append((row, line))
        }
        flush()
        return out
    }

    // MARK: - The bullet rows

    /// Claude Code draws every transcript entry as a `⏺` bullet in column 1
    /// followed by either prose (an agent message) or `Name(arg)` (a tool
    /// call), with `⎿` continuation rows underneath for results.
    private struct BulletResult {
        let elements: [TranscriptElement]
        let nextRow: Int
    }

    private func parseBulletRow(lines: [String], frame: RenderedFrame,
                                at row: Int) -> BulletResult? {
        let line = lines[row]
        let trimmedLeading = line.drop(while: { $0 == " " })
        guard let first = trimmedLeading.first, Self.bulletGlyphs.contains(first) else {
            return nil
        }
        let head = String(trimmedLeading.dropFirst()).trimmed
        guard !head.isEmpty else { return nil }

        // Gather the continuation block: the `⎿` result rows and any indented
        // rows that belong with them.
        var lastRow = row
        var resultLines: [String] = []
        var scan = row + 1
        var sawContinuationMarker = false
        while scan < lines.count {
            let candidate = lines[scan]
            let stripped = candidate.trimmed
            if stripped.isEmpty { break }
            let firstNonSpace = candidate.drop(while: { $0 == " " }).first
            if let glyph = firstNonSpace, Self.bulletGlyphs.contains(glyph) { break }
            if let glyph = firstNonSpace, Self.continuationGlyphs.contains(glyph) {
                sawContinuationMarker = true
                resultLines.append(String(candidate.drop(while: { $0 == " " }).dropFirst()).trimmed)
                lastRow = scan
                scan += 1
                continue
            }
            // An indented row only belongs to this entry once a `⎿` has
            // introduced the block. Otherwise it is a wrapped prose line and
            // is handled as part of the message body below.
            if sawContinuationMarker && candidate.hasPrefix("    ") {
                resultLines.append(stripped)
                lastRow = scan
                scan += 1
                continue
            }
            break
        }

        let region = FrameRegion(firstRow: row, lastRow: lastRow)

        // `Agent "List functions in calc.py" finished · 4s` — the CLI's own
        // completion row for a backgrounded subagent. Both the task and the
        // outcome are literally drawn, so this is `.observed` end to end.
        if let finished = Self.agentFinishedRow(head) {
            return BulletResult(elements: [.subagent(Subagent(
                kind: "Agent", task: finished.task,
                status: finished.failed ? .failed : .succeeded,
                statusProvenance: .observed,
                detailLines: resultLines,
                region: region))], nextRow: lastRow + 1)
        }

        if let call = Self.toolCallPattern(head) {
            if Self.subagentTools.contains(call.name) {
                let joined = resultLines.joined(separator: " ").lowercased()
                let subagentStatus: ToolCallStatus
                let subagentStatusProvenance: Provenance
                if joined.contains("backgrounded") || joined.contains("running") {
                    // "Backgrounded agent (↓ to manage)" says it was launched,
                    // not that it finished. Reading that as a result would
                    // report a running subagent as done.
                    subagentStatus = .running
                    subagentStatusProvenance = .inferred
                } else if resultLines.isEmpty {
                    subagentStatus = .running
                    subagentStatusProvenance = .inferred
                } else {
                    subagentStatus = status(for: resultLines)
                    subagentStatusProvenance = .observed
                }
                return BulletResult(elements: [.subagent(Subagent(
                    kind: call.name,
                    task: call.argument,
                    status: subagentStatus,
                    statusProvenance: subagentStatusProvenance,
                    detailLines: resultLines,
                    region: region))], nextRow: lastRow + 1)
            }
            var produced: [TranscriptElement] = []
            let collapsed = resultLines.contains { Self.collapsedHints.contains(where: $0.contains) }
            produced.append(.toolCall(ToolCall(
                name: call.name,
                argumentSummary: call.argument,
                status: status(for: resultLines),
                // "running" is never on the screen. It is our reading of a
                // call row that has no result under it yet, and it is labelled
                // as such so a renderer cannot present it as the agent's word.
                statusProvenance: resultLines.isEmpty ? .inferred : .observed,
                resultLines: resultLines,
                isCollapsed: collapsed,
                region: region)))
            if let diff = parseDiff(frame: frame, path: call.argument,
                                   firstRow: row + 1, lastRow: lastRow) {
                produced.append(.diff(diff))
            }
            return BulletResult(elements: produced, nextRow: lastRow + 1)
        }

        // A bullet whose continuation block opens with `$ <command>` is a
        // shell call the CLI has titled with the agent's own description
        // rather than a tool name. Both parts are kept: the description as the
        // message it is, and the command as a call whose *name* is our
        // conclusion (the word "Bash" is nowhere on the screen) and so is
        // labelled `.inferred`.
        if let first = resultLines.first, first.hasPrefix("$ ") {
            let command = String(first.dropFirst(2))
            let rest = Array(resultLines.dropFirst())
            return BulletResult(elements: [
                .agentMessage(AgentMessage(markdown: head, text: head,
                                           region: FrameRegion(row: row))),
                .toolCall(ToolCall(name: "Bash", argumentSummary: command,
                                   status: rest.isEmpty ? .running : status(for: rest),
                                   statusProvenance: .inferred,
                                   resultLines: rest,
                                   isCollapsed: rest.contains { line in
                                       Self.collapsedHints.contains(where: line.contains)
                                   },
                                   region: FrameRegion(firstRow: row + 1, lastRow: lastRow),
                                   provenance: .inferred)),
            ], nextRow: lastRow + 1)
        }

        // Not `Name(arg)`: prose. Continue through wrapped lines, which the
        // CLI indents to the bullet's text column.
        // A message is not one run of lines. Claude Code separates paragraphs
        // and list blocks inside a single message with a blank row, so a
        // parser that stops at the first blank splits one message into a
        // paragraph and a pile of `rawText` — which is exactly the "markdown
        // structure preserved" requirement failing quietly. A blank row is
        // therefore kept only when the row after it is still part of the
        // message body.
        var body = [head]
        var prose = row + 1
        var lastContent = row
        var pendingBlanks = 0
        while prose < lines.count {
            let candidate = lines[prose]
            if candidate.trimmed.isEmpty {
                pendingBlanks += 1
                if pendingBlanks > 1 { break }
                prose += 1
                continue
            }
            let firstNonSpace = candidate.drop(while: { $0 == " " }).first
            if let glyph = firstNonSpace,
               Self.bulletGlyphs.contains(glyph) || Self.continuationGlyphs.contains(glyph)
                || Self.promptGlyphs.contains(glyph) { break }
            guard candidate.hasPrefix("  "), !Self.isHorizontalRule(candidate) else { break }
            if parseModeIndicator(line: candidate, row: prose) != nil { break }
            if parseTurnBoundary(line: candidate, row: prose) != nil { break }
            if parseAgentRunning(line: candidate, row: prose) != nil { break }
            for _ in 0..<pendingBlanks { body.append("") }
            pendingBlanks = 0
            body.append(candidate.trimmed)
            lastContent = prose
            prose += 1
        }
        let text = body.joined(separator: "\n")
        return BulletResult(elements: [.agentMessage(AgentMessage(
            markdown: Markdown.fromScreenLines(body),
            text: text,
            region: FrameRegion(firstRow: row, lastRow: lastContent)))],
                            nextRow: lastContent + 1)
    }

    private func status(for resultLines: [String]) -> ToolCallStatus {
        guard !resultLines.isEmpty else { return .running }
        let joined = resultLines.joined(separator: " ").lowercased()
        if Self.errorMarkers.contains(where: joined.contains) { return .failed }
        if Self.successMarkers.contains(where: joined.contains) { return .succeeded }
        return .completed
    }

    // MARK: - Diffs

    /// A Claude Code edit draws numbered source lines whose *background*
    /// colour is the only thing distinguishing an addition from a removal.
    /// That is why the emulator keeps attributes: read as text alone, a diff
    /// is indistinguishable from a file listing.
    private func parseDiff(frame: RenderedFrame, path: String?,
                           firstRow: Int, lastRow: Int) -> Diff? {
        guard firstRow <= lastRow else { return nil }
        var hunk: [Diff.Line] = []
        var added = 0, removed = 0
        var anySignDrawn = false
        for row in firstRow...min(lastRow, frame.screen.rows - 1) {
            let text = frame.screen.line(row)
            guard let numbered = Self.numberedSourceLine(text) else { continue }
            let kind: Diff.Line.Kind
            if let sign = numbered.sign {
                // 2.1.x draws a literal `+`/`-` between the line number and
                // the source. When it is there, "added" is something the agent
                // wrote, not something we concluded.
                anySignDrawn = true
                kind = sign == "+" ? .added : .removed
            } else {
                switch dominantBackground(frame: frame, row: row) {
                case .some(.indexed(let index)) where Self.addedBackgrounds.contains(index):
                    kind = .added
                case .some(.indexed(let index)) where Self.removedBackgrounds.contains(index):
                    kind = .removed
                default:
                    kind = .context
                }
            }
            if kind == .added { added += 1 }
            if kind == .removed { removed += 1 }
            hunk.append(Diff.Line(kind: kind, number: numbered.number, text: numbered.text))
        }
        guard !hunk.isEmpty, added + removed > 0 else { return nil }
        return Diff(path: path, hunkLines: hunk, addedCount: added, removedCount: removed,
                    region: FrameRegion(firstRow: firstRow, lastRow: lastRow),
                    // If the `+`/`-` markers were drawn, the classification is
                    // a reading. If it rests on background colour alone, it is
                    // our conclusion and is labelled as one — a consumer can
                    // then decide whether a colour-derived diff is good enough
                    // to render as a diff.
                    provenance: anySignDrawn ? .observed : .inferred)
    }

    private func dominantBackground(frame: RenderedFrame, row: Int) -> TerminalColor? {
        var counts: [TerminalColor: Int] = [:]
        for column in 0..<frame.screen.columns {
            if let background = frame.screen[row, column].attributes.background {
                counts[background, default: 0] += 1
            }
        }
        // Require a real run, not a stray cell.
        return counts.filter { $0.value >= 4 }.max { $0.value < $1.value }?.key
    }

    // MARK: - Composer

    private struct ComposerBox { let firstRow: Int; let lastRow: Int; let body: [String] }

    /// 2.1.x draws the composer as two full-width horizontal rules with the
    /// prompt rows between them. Earlier builds drew a rounded box
    /// (`╭─…╮ │ … │ ╰─…╯`), so both are accepted: recognising one layout and
    /// silently mis-reading the other is the failure mode this module is for.
    private func parseComposer(lines: [String], at row: Int) -> ComposerBox? {
        if let boxed = parseBoxedComposer(lines: lines, at: row) { return boxed }
        return parseRuledComposer(lines: lines, at: row)
    }

    private func parseBoxedComposer(lines: [String], at row: Int) -> ComposerBox? {
        let line = lines[row]
        guard line.hasPrefix("╭"), line.contains("─"), line.hasSuffix("╮") else { return nil }
        var scan = row + 1
        var body: [String] = []
        while scan < lines.count {
            let candidate = lines[scan]
            if candidate.hasPrefix("╰") { return ComposerBox(firstRow: row, lastRow: scan, body: body) }
            guard candidate.hasPrefix("│") else { return nil }
            var inner = String(candidate.dropFirst())
            if inner.hasSuffix("│") { inner = String(inner.dropLast()) }
            body.append(inner)
            scan += 1
            if scan - row > 24 { return nil }
        }
        return nil
    }

    private func parseRuledComposer(lines: [String], at row: Int) -> ComposerBox? {
        guard Self.isHorizontalRule(lines[row]) else { return nil }
        var scan = row + 1
        var body: [String] = []
        while scan < lines.count, scan - row <= 12 {
            let candidate = lines[scan]
            if Self.isHorizontalRule(candidate) {
                // A pair of rules with nothing prompt-shaped between them is
                // a divider, not the composer.
                guard body.contains(where: { $0.drop(while: { $0 == " " }).first.map(Self.promptGlyphs.contains) ?? false })
                else { return nil }
                return ComposerBox(firstRow: row, lastRow: scan, body: body)
            }
            body.append(candidate)
            scan += 1
        }
        return nil
    }

    private func composerElement(_ box: ComposerBox, frame: RenderedFrame) -> Composer {
        var content: [String] = []
        var promptColumn = 0
        for (offset, raw) in box.body.enumerated() {
            var text = raw.trimmed
            if let first = text.first, Self.promptGlyphs.contains(first) {
                if offset == 0 {
                    promptColumn = (raw.count - raw.drop(while: { $0 == " " }).count) + 2
                }
                text = String(text.dropFirst()).trimmed
            }
            content.append(text)
        }
        while let last = content.last, last.isEmpty { content.removeLast() }
        let joined = content.joined(separator: "\n")

        // A placeholder is dim; typed text is not. Reading it off the cells is
        // the difference between "the user typed nothing" and "the user typed
        // the placeholder", which a text-only parser cannot tell apart.
        var placeholder: String?
        if !joined.isEmpty, box.firstRow + 1 < frame.screen.rows {
            let attributes = frame.screen.attributes(row: box.firstRow + 1)
            let bodyAttributes = attributes.dropFirst(promptColumn).prefix(joined.count)
            if !bodyAttributes.isEmpty && bodyAttributes.allSatisfy({ $0.dim }) {
                placeholder = joined
            }
        }
        let cursorInside = frame.screen.cursorRow > box.firstRow
            && frame.screen.cursorRow < box.lastRow
        return Composer(content: placeholder == nil ? joined : "",
                        placeholder: placeholder,
                        cursorIsInside: cursorInside,
                        region: FrameRegion(firstRow: box.firstRow, lastRow: box.lastRow))
    }

    // MARK: - Mode indicator

    private func parseModeIndicator(line: String, row: Int) -> ModeIndicator? {
        let text = line.trimmed
        guard !text.isEmpty else { return nil }
        let lowered = text.lowercased()
        // The footer is recognisable by its cycle hint, which is the one part
        // that has not moved across releases.
        guard lowered.contains("shift+tab to cycle") || lowered.contains("shift-tab to cycle")
                || lowered.hasPrefix("⏵⏵") || lowered.hasPrefix("⏸") else { return nil }

        let hint = Self.captureGroup(in: text, pattern: "\\((shift\\+tab to cycle[^)]*)\\)")
        let mode: PermissionMode
        if lowered.contains("accept edits") { mode = .acceptEdits }
        else if lowered.contains("plan mode") { mode = .plan }
        else if lowered.contains("bypass") || lowered.contains("dangerously") { mode = .bypassPermissions }
        else if lowered.contains("auto mode") { mode = .auto }
        else if lowered.contains("manual mode") { mode = .manual }
        else { mode = .unrecognised }
        return ModeIndicator(mode: mode, rawText: text, hint: hint,
                             region: FrameRegion(row: row),
                             // The words are on screen; mapping them onto our
                             // enum is ours. When the mapping fails we say
                             // `.unrecognised` and hand back `rawText`.
                             provenance: mode == .unrecognised ? .unrecognised : .observed)
    }

    // MARK: - Working indicator

    private func parseAgentRunning(line: String, row: Int) -> AgentRunning? {
        let text = line.trimmed
        guard let first = text.first, Self.spinnerGlyphs.contains(first) else { return nil }
        let rest = String(text.dropFirst()).trimmed
        guard !rest.isEmpty else { return nil }

        // "Channeling… (3s · ↓ 59 tokens)" — the verb is the leading word with
        // its ellipsis removed. Claude Code rotates these words constantly;
        // they are quoted back, never mapped onto a state vocabulary we made
        // up, because "Channeling" does not mean anything the SDK can define.
        let verb = rest.split(separator: " ").first
            .map { String($0).trimmingCharacters(in: CharacterSet(charactersIn: "…·.")) }
            .flatMap { $0.isEmpty ? nil : $0 }
        let elapsed = Self.captureGroup(in: rest, pattern: "for (\\d+)s").flatMap { Int($0) }
            ?? Self.captureGroup(in: rest, pattern: "\\((\\d+)s").flatMap { Int($0) }
        let tokensText = Self.captureGroup(in: rest, pattern: "([\\d,]+) tokens")
        let tokens = tokensText.flatMap { Int($0.replacingOccurrences(of: ",", with: "")) }
        let hint = Self.captureGroup(in: rest, pattern: "(esc to interrupt[^)]*)")

        return AgentRunning(spinner: String(first), verb: verb,
                            elapsedSeconds: elapsed, tokens: tokens, hint: hint,
                            rawLine: text, region: FrameRegion(row: row))
    }

    // MARK: - Turn boundaries

    private func parseTurnBoundary(line: String, row: Int) -> TurnBoundary? {
        let text = line.trimmed
        guard !text.isEmpty else { return nil }
        let lowered = text.lowercased()
        if lowered.contains("interrupted by user") || lowered.contains("request interrupted") {
            return TurnBoundary(kind: .interrupted, text: text, region: FrameRegion(row: row))
        }
        // "✻ Baked for 4s · done 5:55 PM" — the CLI's own completion line.
        if let first = text.first, Self.spinnerGlyphs.contains(first),
           lowered.contains("· done") {
            return TurnBoundary(kind: .completed, text: text, region: FrameRegion(row: row))
        }
        return nil
    }

    // MARK: - Error banners

    /// A standalone error row: the CLI marks these with a cross or a bare
    /// `Error:` prefix, outside any tool row. Matching on the marker keeps
    /// this from claiming every sentence that contains the word "error" —
    /// including the agent's own prose *about* an error, which is a message,
    /// not a banner.
    private func parseErrorBanner(line: String, row: Int) -> ErrorBanner? {
        let text = line.trimmed
        guard !text.isEmpty else { return nil }
        if let first = text.first, Self.errorGlyphs.contains(first) {
            return ErrorBanner(text: String(text.dropFirst()).trimmed,
                               region: FrameRegion(row: row))
        }
        let lowered = text.lowercased()
        guard lowered.hasPrefix("error:") || lowered.hasPrefix("api error")
                || lowered.hasPrefix("fatal:") else { return nil }
        return ErrorBanner(text: text, region: FrameRegion(row: row))
    }

    // MARK: - Questions

    private struct QuestionResult { let value: Question; let nextRow: Int }

    private func parseQuestion(lines: [String], at row: Int) -> QuestionResult? {
        // The rule is structural, not lexical: a line that asks something,
        // followed by numbered options. Matching on the wording ("Do you want
        // to…") fails the moment the CLI rephrases, and the rephrasing is
        // exactly when a mis-parse would be least visible.
        let text = lines[row].trimmed
        guard text.hasSuffix("?"), text.count > 3 else { return nil }

        var options: [Question.Option] = []
        var selected: Int?
        var scan = row + 1
        var last = row
        while scan < lines.count, scan - row < 12 {
            let candidate = lines[scan].trimmed
            if candidate.isEmpty { scan += 1; continue }
            guard let option = Self.optionPattern(candidate) else {
                // Only blank rows may separate the prompt from its first
                // option. Skipping arbitrary prose would let any question mark
                // anywhere on screen adopt an unrelated numbered list.
                if options.isEmpty { return nil }
                break
            }
            if option.selected { selected = options.count }
            options.append(Question.Option(key: option.key, label: option.label))
            last = scan
            scan += 1
        }
        guard !options.isEmpty else { return nil }
        return QuestionResult(value: Question(prompt: text, options: options,
                                              selectedIndex: selected,
                                              region: FrameRegion(firstRow: row, lastRow: last)),
                              nextRow: last + 1)
    }

    // MARK: - Bullet-less tool activity

    private struct SummaryResult { let value: ToolCall; let nextRow: Int }

    /// While a turn is in flight, and after it collapses, Claude Code draws
    /// tool activity *without* a bullet and without the `Name(arg)` shape:
    /// `  Reading calc.py` with `  ⎿  calc.py` under it, or `  Read 1 file`
    /// once the call has folded away.
    ///
    /// These are emitted as tool calls because that is plainly what they are,
    /// but the whole element is `.inferred`: the CLI never wrote a tool name
    /// here. "Reading" is a participle in a status line, and treating it as
    /// `ToolCall.name == "Reading"` is our reading of the sentence, not the
    /// agent's declaration of a call. A consumer that wants only calls the
    /// agent actually named can filter on `provenance == .observed`.
    private func parseActivitySummary(lines: [String], at row: Int) -> SummaryResult? {
        let line = lines[row]
        guard line.hasPrefix("  "), !line.hasPrefix("   ") else { return nil }
        let text = line.trimmed
        guard let first = text.first, first.isUppercase else { return nil }
        let words = text.split(separator: " ")
        guard let head = words.first.map(String.init),
              Self.activityVerbs.contains(head) else { return nil }

        var resultLines: [String] = []
        var last = row
        var scan = row + 1
        while scan < lines.count {
            let candidate = lines[scan]
            let firstNonSpace = candidate.drop(while: { $0 == " " }).first
            guard let glyph = firstNonSpace, Self.continuationGlyphs.contains(glyph) else { break }
            resultLines.append(String(candidate.drop(while: { $0 == " " }).dropFirst()).trimmed)
            last = scan
            scan += 1
        }
        let argument = words.dropFirst().joined(separator: " ")
        return SummaryResult(value: ToolCall(
            name: head,
            argumentSummary: argument.isEmpty ? nil : argument,
            // An activity row with nothing under it is *not* evidence that the
            // call is still running: it is just as likely to be a finished
            // call the CLI has folded away. `.unknown` is the only status this
            // shape supports.
            status: resultLines.isEmpty ? .unknown : status(for: resultLines),
            statusProvenance: .inferred,
            resultLines: resultLines,
            isCollapsed: resultLines.contains { Self.collapsedHints.contains(where: $0.contains) },
            region: FrameRegion(firstRow: row, lastRow: last),
            provenance: .inferred), nextRow: last + 1)
    }

    // MARK: - User messages

    private struct UserResult { let value: UserMessage; let nextRow: Int }

    /// The transcript echoes what the user sent as `> text`, left-aligned and
    /// outside any box. The composer is a box and is matched first, so a `>`
    /// row reaching here is a sent message.
    private func parseUserMessage(lines: [String], at row: Int) -> UserResult? {
        let line = lines[row]
        let stripped = line.drop(while: { $0 == " " })
        guard let marker = stripped.first, Self.promptGlyphs.contains(marker),
              stripped.dropFirst().hasPrefix(" ") else { return nil }
        var body = [String(stripped.dropFirst(2)).trimmed]
        var scan = row + 1
        while scan < lines.count {
            let candidate = lines[scan]
            guard candidate.hasPrefix("  "), !candidate.trimmed.isEmpty else { break }
            let firstNonSpace = candidate.drop(while: { $0 == " " }).first
            if let glyph = firstNonSpace, Self.promptGlyphs.contains(glyph) { break }
            if let glyph = firstNonSpace,
               Self.bulletGlyphs.contains(glyph) || Self.continuationGlyphs.contains(glyph)
                || Self.spinnerGlyphs.contains(glyph) { break }
            body.append(candidate.trimmed)
            scan += 1
        }
        return UserResult(value: UserMessage(text: body.joined(separator: "\n"),
                                             region: FrameRegion(firstRow: row, lastRow: scan - 1)),
                          nextRow: scan)
    }
}

// MARK: - Vocabulary

extension ClaudeCodeParser {
    /// The glyphs Claude Code uses to open a transcript entry.
    static let bulletGlyphs: Set<Character> = ["⏺", "●"]
    /// The glyph that opens a result/continuation row.
    static let continuationGlyphs: Set<Character> = ["⎿", "└", "╰"]
    /// The spinner frames. Claude Code rotates through these on the working
    /// row and reuses the last one on the completion line.
    static let spinnerGlyphs: Set<Character> = ["✻", "✽", "✶", "✳", "✢", "·", "∗", "*"]
    /// Tools whose row means "a subagent is doing this".
    static let subagentTools: Set<String> = ["Task", "Agent", "Explore", "Plan",
                                             "general-purpose", "Subagent"]
    /// Glyphs that open an error banner.
    static let errorGlyphs: Set<Character> = ["✗", "✘", "×", "⚠", "🛑"]
    /// The leading words of a bullet-less activity row. Deliberately a closed
    /// list: an open rule ("any capitalised first word") turns every sentence
    /// into a tool call.
    static let activityVerbs: Set<String> = [
        "Read", "Reading", "Wrote", "Writing", "Ran", "Running", "Listed",
        "Listing", "Searched", "Searching", "Fetched", "Fetching", "Updated",
        "Updating", "Created", "Creating", "Edited", "Editing", "Found",
        "Deleted", "Deleting", "Globbed", "Grepped",
    ]
    static let errorMarkers = ["error", "failed", "no such file", "denied",
                               "not found", "cannot ", "exception"]
    static let successMarkers = ["added ", "updated ", "read ", "wrote ", "applied ",
                                 "found ", "listed ", "removed ", "created "]
    static let collapsedHints = ["ctrl+o to expand", "ctrl-o to expand", "… +", "... +",
                                 "more lines", "expand"]
    /// 256-colour background indices Claude Code uses for diff shading. Two
    /// families because the palette differs between its light and dark themes.
    static let addedBackgrounds: Set<UInt8> = [22, 28, 29, 35, 65, 151, 194, 2]
    static let removedBackgrounds: Set<UInt8> = [52, 88, 89, 95, 124, 131, 224, 1]

    static func toolCallPattern(_ text: String) -> (name: String, argument: String?)? {
        // `Name(argument)` where Name is a bare identifier. Anything else is
        // prose that happens to contain a bracket.
        guard let open = text.firstIndex(of: "("), text.hasSuffix(")") else {
            // A bare `Name` with no argument is also a tool row the CLI draws
            // (e.g. `TodoWrite`), but only when the whole line is one word in
            // PascalCase — otherwise every one-word sentence becomes a call.
            let trimmed = text.trimmed
            guard !trimmed.contains(" "), trimmed.count >= 3,
                  let first = trimmed.first, first.isUppercase,
                  trimmed.allSatisfy({ $0.isLetter }) else { return nil }
            return (trimmed, nil)
        }
        let name = String(text[text.startIndex..<open])
        guard !name.isEmpty, name.allSatisfy({ $0.isLetter || $0 == "_" }),
              let first = name.first, first.isUppercase else { return nil }
        let argumentStart = text.index(after: open)
        let argument = String(text[argumentStart..<text.index(before: text.endIndex)])
        return (name, argument.isEmpty ? nil : argument)
    }

    static func numberedSourceLine(_ text: String) -> (number: Int?, sign: Character?, text: String)? {
        // `     5  def subtract(a, b):`  (context)
        // `     9 +def multiply(a, b):`  (added)
        // A right-aligned line number, then an optional `+`/`-`, then source.
        let stripped = text.drop(while: { $0 == " " || $0 == "\u{a0}" })
        let digits = stripped.prefix(while: { $0.isNumber })
        guard !digits.isEmpty, let number = Int(digits) else { return nil }
        var rest = stripped.dropFirst(digits.count)
        guard rest.hasPrefix(" ") else { return nil }
        rest = rest.dropFirst()
        if let first = rest.first, first == "+" || first == "-" {
            return (number, first, String(rest.dropFirst()))
        }
        // An unchanged line still reserves the sign column, so drop the blank
        // that stands where a `+` would be. Leaving it in shifts every context
        // line one space right relative to the lines around it.
        if rest.hasPrefix(" ") { rest = rest.dropFirst() }
        return (number, nil, String(rest))
    }

    static func agentFinishedRow(_ text: String) -> (task: String, failed: Bool)? {
        guard text.hasPrefix("Agent \""), let closing = text.dropFirst(7).firstIndex(of: "\"")
        else { return nil }
        let task = String(text[text.index(text.startIndex, offsetBy: 7)..<closing])
        let tail = String(text[text.index(after: closing)...]).trimmed.lowercased()
        guard tail.hasPrefix("finished") || tail.hasPrefix("failed")
                || tail.hasPrefix("stopped") || tail.hasPrefix("errored") else { return nil }
        return (task, tail.hasPrefix("failed") || tail.hasPrefix("errored"))
    }

    static func isHorizontalRule(_ text: String) -> Bool {
        let trimmed = text.trimmed
        return trimmed.count >= 20 && trimmed.allSatisfy { $0 == "─" || $0 == "━" || $0 == "—" }
    }

    /// The glyphs that open a prompt row: the composer's own marker and the
    /// echo of a sent message.
    static let promptGlyphs: Set<Character> = ["❯", ">", "›"]

    static func optionPattern(_ text: String) -> (key: String?, label: String, selected: Bool)? {
        // `❯ 1. Yes`  /  `  2. No, and tell Claude what to do differently`
        var working = text
        var selected = false
        for marker in ["❯", ">", "▸", "→"] where working.hasPrefix(marker) {
            working = String(working.dropFirst(marker.count)).trimmed
            selected = true
            break
        }
        let digits = working.prefix(while: { $0.isNumber })
        guard !digits.isEmpty else { return nil }
        let rest = working.dropFirst(digits.count)
        guard rest.hasPrefix(".") || rest.hasPrefix(")") else { return nil }
        let label = String(rest.dropFirst()).trimmed
        guard !label.isEmpty else { return nil }
        return (String(digits), label, selected)
    }

    static func captureGroup(in text: String, pattern: String) -> String? {
        guard let regex = try? NSRegularExpression(pattern: pattern, options: [.caseInsensitive]),
              let match = regex.firstMatch(in: text, range: NSRange(text.startIndex..., in: text)),
              match.numberOfRanges > 1,
              let range = Range(match.range(at: 1), in: text)
        else { return nil }
        return String(text[range])
    }
}

// MARK: - Markdown

enum Markdown {
    /// Reconstruct Markdown source from lines the CLI has already rendered.
    ///
    /// This is genuinely lossy and the type system should not pretend
    /// otherwise: bold that was drawn with SGR is gone by the time the text is
    /// read, and a wrapped paragraph cannot be unwrapped with certainty. What
    /// survives is block structure — bullets, numbered items, indented code —
    /// which is the part a renderer needs and the part the CLI draws with
    /// characters rather than colour.
    static func fromScreenLines(_ lines: [String]) -> String {
        var out: [String] = []
        for line in lines {
            var text = line
            if text.hasPrefix("• ") || text.hasPrefix("◦ ") || text.hasPrefix("- ") {
                text = "- " + text.dropFirst(2)
            }
            out.append(text)
        }
        return out.joined(separator: "\n")
    }
}

extension StringProtocol {
    var trimmed: String { trimmingCharacters(in: .whitespaces) }
}
