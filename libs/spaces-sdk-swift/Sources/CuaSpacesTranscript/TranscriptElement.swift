import Foundation

/// How a fact in a parsed frame came to be known.
///
/// This is the load-bearing type in the whole module. A judge ruled on it and
/// the ruling is binding (`FRICTION.md` §93): **the SDK must never mint a
/// structured element the agent did not actually offer.** A terminal frame is
/// pixels-as-characters; it is not an event stream. Some things really are
/// readable off it, and some things are a parser's opinion, and a consumer has
/// to be able to tell which without reading our source.
///
/// So every element carries one of these, and the JSON-UI encoding puts it in
/// the payload where a non-Swift renderer sees it too.
public enum Provenance: String, Sendable, Codable, Hashable, CaseIterable {
    /// The text is on the screen and the element is a direct reading of it.
    /// A tool-call row that literally says `Update(calc.py)` is `observed`:
    /// the name and the argument were drawn by the agent.
    case observed

    /// The element is a conclusion the parser drew from layout, colour or
    /// adjacency, and the agent never said it in those words. A tool call
    /// marked "running" because its row has no result line yet is `inferred`:
    /// the absence of a result is on screen, "still running" is our reading
    /// of that absence.
    case inferred

    /// The parser recognised the region as *something*, could not identify
    /// what, and is handing back the text unchanged rather than guessing.
    /// This is the degradation path required when Claude Code's layout
    /// changes under us, and it is a success, not a failure.
    case unrecognised
}

/// Where on the screen an element was read from. Always present, because a
/// claim about a frame that cannot be pointed at is not checkable.
public struct FrameRegion: Sendable, Codable, Hashable {
    public let firstRow: Int
    public let lastRow: Int
    public init(firstRow: Int, lastRow: Int) {
        self.firstRow = firstRow
        self.lastRow = lastRow
    }
    public init(row: Int) { self.init(firstRow: row, lastRow: row) }
}

/// The permission/mode indicator in the footer and its states.
///
/// The cases are the wordings Claude Code 2.1.x actually draws, not a tidier
/// vocabulary invented here. When a build draws something else the mode is
/// `.unrecognised` and `ModeIndicator.rawText` carries the footer verbatim, so
/// a renderer shows the truth rather than the nearest case we happened to have.
public enum PermissionMode: String, Sendable, Codable, Hashable {
    /// "⏸ manual mode on" — every tool call is confirmed.
    case manual
    /// "⏵⏵ auto mode on".
    case auto
    /// "⏵⏵ accept edits on".
    case acceptEdits = "accept_edits"
    /// "⏸ plan mode on".
    case plan
    /// "bypass permissions"/"dangerously skip permissions".
    case bypassPermissions = "bypass_permissions"
    /// The footer was found but its wording is not one we know. The raw text
    /// travels with it.
    case unrecognised
}

/// A tool call's status as far as the frame can support.
public enum ToolCallStatus: String, Sendable, Codable, Hashable {
    /// A result line is present and reads as success.
    case succeeded
    /// A result line is present and reads as an error.
    case failed
    /// No result line yet. Always `inferred` — "not finished" is our reading
    /// of an absence.
    case running
    /// A result line is present but says nothing about success.
    case completed
    case unknown
}

/// A single structured thing on the screen.
///
/// Deliberately a flat enum rather than a class hierarchy: a frame is a list
/// of regions read top to bottom, and anything more elaborate would imply
/// structure the terminal does not have.
public enum TranscriptElement: Sendable, Hashable {
    /// Prose the agent wrote. `markdown` keeps the structure the CLI drew
    /// (list bullets, fenced code, headings) as Markdown source; `text` is
    /// the same content flattened.
    case agentMessage(AgentMessage)
    /// Something the user typed, echoed into the transcript.
    case userMessage(UserMessage)
    /// A tool invocation row and whatever result rows follow it.
    case toolCall(ToolCall)
    /// A delegated subagent and its state.
    case subagent(Subagent)
    /// The working indicator: spinner, verb, elapsed time, token count.
    case agentRunning(AgentRunning)
    /// The composer at the bottom of the screen.
    case composer(Composer)
    /// The permission/mode footer.
    case modeIndicator(ModeIndicator)
    /// A question the agent is waiting on, with its options.
    case question(Question)
    /// A unified diff drawn for a file edit.
    case diff(Diff)
    /// An error banner that is not attached to a tool call.
    case error(ErrorBanner)
    /// A turn boundary the frame actually shows (a completion line).
    case turnBoundary(TurnBoundary)
    /// Screen content the parser could not classify. Never dropped.
    case rawText(RawText)

    public var provenance: Provenance {
        switch self {
        case .agentMessage(let value): return value.provenance
        case .userMessage(let value): return value.provenance
        case .toolCall(let value): return value.provenance
        case .subagent(let value): return value.provenance
        case .agentRunning(let value): return value.provenance
        case .composer(let value): return value.provenance
        case .modeIndicator(let value): return value.provenance
        case .question(let value): return value.provenance
        case .diff(let value): return value.provenance
        case .error(let value): return value.provenance
        case .turnBoundary(let value): return value.provenance
        case .rawText(let value): return value.provenance
        }
    }

    public var region: FrameRegion {
        switch self {
        case .agentMessage(let value): return value.region
        case .userMessage(let value): return value.region
        case .toolCall(let value): return value.region
        case .subagent(let value): return value.region
        case .agentRunning(let value): return value.region
        case .composer(let value): return value.region
        case .modeIndicator(let value): return value.region
        case .question(let value): return value.region
        case .diff(let value): return value.region
        case .error(let value): return value.region
        case .turnBoundary(let value): return value.region
        case .rawText(let value): return value.region
        }
    }
}

// MARK: - Element payloads

public struct AgentMessage: Sendable, Hashable {
    public let markdown: String
    public let text: String
    public let region: FrameRegion
    public let provenance: Provenance
    public init(markdown: String, text: String, region: FrameRegion,
                provenance: Provenance = .observed) {
        self.markdown = markdown; self.text = text
        self.region = region; self.provenance = provenance
    }
}

public struct UserMessage: Sendable, Hashable {
    public let text: String
    public let region: FrameRegion
    public let provenance: Provenance
    public init(text: String, region: FrameRegion, provenance: Provenance = .observed) {
        self.text = text; self.region = region; self.provenance = provenance
    }
}

public struct ToolCall: Sendable, Hashable {
    /// The tool's name exactly as drawn, e.g. `Update`, `Read`, `Bash`.
    public let name: String
    /// The single argument Claude Code shows in parentheses, verbatim. The
    /// CLI does not draw the full argument object, so this is never presented
    /// as "the arguments" — it is the argument summary the agent chose to show.
    public let argumentSummary: String?
    public let status: ToolCallStatus
    /// Whether `status` was read or concluded. Split out from `provenance`
    /// because the *name* can be observed while the *status* is inferred, and
    /// collapsing the two would overstate one or understate the other.
    public let statusProvenance: Provenance
    /// The result lines drawn under the call, verbatim.
    public let resultLines: [String]
    /// True when the CLI drew a "+N lines"/"ctrl+o to expand" affordance,
    /// meaning the result on screen is not the whole result.
    public let isCollapsed: Bool
    public let region: FrameRegion
    public let provenance: Provenance

    public init(name: String, argumentSummary: String?, status: ToolCallStatus,
                statusProvenance: Provenance, resultLines: [String],
                isCollapsed: Bool, region: FrameRegion,
                provenance: Provenance = .observed) {
        self.name = name; self.argumentSummary = argumentSummary
        self.status = status; self.statusProvenance = statusProvenance
        self.resultLines = resultLines; self.isCollapsed = isCollapsed
        self.region = region; self.provenance = provenance
    }
}

public struct Subagent: Sendable, Hashable {
    /// The subagent type as drawn, e.g. `Explore`, `general-purpose`.
    public let kind: String
    /// The task description the row shows, when it shows one.
    public let task: String?
    public let status: ToolCallStatus
    public let statusProvenance: Provenance
    /// Progress lines drawn beneath, verbatim (tool counts, token counts).
    public let detailLines: [String]
    public let region: FrameRegion
    public let provenance: Provenance

    public init(kind: String, task: String?, status: ToolCallStatus,
                statusProvenance: Provenance, detailLines: [String],
                region: FrameRegion, provenance: Provenance = .observed) {
        self.kind = kind; self.task = task; self.status = status
        self.statusProvenance = statusProvenance; self.detailLines = detailLines
        self.region = region; self.provenance = provenance
    }
}

public struct AgentRunning: Sendable, Hashable {
    /// The spinner glyph currently drawn, if any.
    public let spinner: String?
    /// The word the CLI is using this tick ("Baking", "Thinking", …). Claude
    /// Code rotates these; they are quoted, never interpreted.
    public let verb: String?
    /// Elapsed seconds, when the row shows them.
    public let elapsedSeconds: Int?
    /// Token count, when the row shows one.
    public let tokens: Int?
    /// The hint the row appends ("esc to interrupt"), verbatim.
    public let hint: String?
    public let rawLine: String
    public let region: FrameRegion
    public let provenance: Provenance

    public init(spinner: String?, verb: String?, elapsedSeconds: Int?, tokens: Int?,
                hint: String?, rawLine: String, region: FrameRegion,
                provenance: Provenance = .observed) {
        self.spinner = spinner; self.verb = verb
        self.elapsedSeconds = elapsedSeconds; self.tokens = tokens
        self.hint = hint; self.rawLine = rawLine
        self.region = region; self.provenance = provenance
    }
}

public struct Composer: Sendable, Hashable {
    /// What is typed in the box. Empty when the box shows only a placeholder.
    public let content: String
    /// The placeholder, when one is showing.
    public let placeholder: String?
    /// Whether the terminal cursor is inside the composer's box. This is the
    /// only focus signal a frame carries, and it is exactly that — the cursor
    /// is there — not a claim about which widget has keyboard focus.
    public let cursorIsInside: Bool
    public let region: FrameRegion
    public let provenance: Provenance

    public init(content: String, placeholder: String?, cursorIsInside: Bool,
                region: FrameRegion, provenance: Provenance = .observed) {
        self.content = content; self.placeholder = placeholder
        self.cursorIsInside = cursorIsInside
        self.region = region; self.provenance = provenance
    }
}

public struct ModeIndicator: Sendable, Hashable {
    public let mode: PermissionMode
    /// The footer text as drawn, always. When `mode` is `.unrecognised` this
    /// is the only thing a consumer should show.
    public let rawText: String
    /// The cycle hint, e.g. `shift+tab to cycle`.
    public let hint: String?
    public let region: FrameRegion
    public let provenance: Provenance

    public init(mode: PermissionMode, rawText: String, hint: String?,
                region: FrameRegion, provenance: Provenance = .observed) {
        self.mode = mode; self.rawText = rawText; self.hint = hint
        self.region = region; self.provenance = provenance
    }
}

public struct Question: Sendable, Hashable {
    public let prompt: String
    public let options: [Option]
    /// The option currently highlighted, by index, when the frame marks one.
    public let selectedIndex: Int?
    public let region: FrameRegion
    public let provenance: Provenance

    public struct Option: Sendable, Hashable, Codable {
        /// The number or key the CLI shows for this option, when it shows one.
        public let key: String?
        public let label: String
        public init(key: String?, label: String) { self.key = key; self.label = label }
    }

    public init(prompt: String, options: [Option], selectedIndex: Int?,
                region: FrameRegion, provenance: Provenance = .observed) {
        self.prompt = prompt; self.options = options
        self.selectedIndex = selectedIndex
        self.region = region; self.provenance = provenance
    }
}

public struct Diff: Sendable, Hashable {
    public let path: String?
    public let hunkLines: [Line]
    public let addedCount: Int
    public let removedCount: Int
    public let region: FrameRegion
    public let provenance: Provenance

    public struct Line: Sendable, Hashable, Codable {
        public enum Kind: String, Sendable, Codable { case added, removed, context }
        public let kind: Kind
        public let number: Int?
        public let text: String
        public init(kind: Kind, number: Int?, text: String) {
            self.kind = kind; self.number = number; self.text = text
        }
    }

    public init(path: String?, hunkLines: [Line], addedCount: Int, removedCount: Int,
                region: FrameRegion, provenance: Provenance = .observed) {
        self.path = path; self.hunkLines = hunkLines
        self.addedCount = addedCount; self.removedCount = removedCount
        self.region = region; self.provenance = provenance
    }
}

public struct ErrorBanner: Sendable, Hashable {
    public let text: String
    public let region: FrameRegion
    public let provenance: Provenance
    public init(text: String, region: FrameRegion, provenance: Provenance = .observed) {
        self.text = text; self.region = region; self.provenance = provenance
    }
}

public struct TurnBoundary: Sendable, Hashable {
    public enum Kind: String, Sendable, Codable {
        /// The CLI drew a completion summary line.
        case completed
        /// The CLI drew an interruption notice.
        case interrupted
    }
    public let kind: Kind
    public let text: String
    public let region: FrameRegion
    public let provenance: Provenance
    public init(kind: Kind, text: String, region: FrameRegion,
                provenance: Provenance = .observed) {
        self.kind = kind; self.text = text; self.region = region
        self.provenance = provenance
    }
}

public struct RawText: Sendable, Hashable {
    public let lines: [String]
    public let region: FrameRegion
    public let provenance: Provenance = .unrecognised
    public init(lines: [String], region: FrameRegion) {
        self.lines = lines; self.region = region
    }
}

/// Everything the parser concluded about one frame.
public struct ParsedFrame: Sendable {
    public let elements: [TranscriptElement]
    /// The CLI the frame came from and the version that drew it, carried
    /// through from the recording. A structured reading of a terminal is only
    /// meaningful against the layout it was written for.
    public let cli: String?
    public let cliVersion: String?
    /// The layout profile that matched. `nil` means nothing matched and the
    /// whole frame degraded to `rawText`.
    public let layoutProfile: String?
    public let frameTime: TimeInterval
    /// Escape sequences the emulator did not implement while producing this
    /// frame. Surfaced so a consumer can distrust a frame rather than being
    /// told everything was fine.
    public let unsupportedSequences: Int
    /// True when the frame was taken on the alternate screen buffer, which is
    /// where the Claude Code UI lives.
    public let isAlternateScreen: Bool

    public init(elements: [TranscriptElement], cli: String?, cliVersion: String?,
                layoutProfile: String?, frameTime: TimeInterval,
                unsupportedSequences: Int, isAlternateScreen: Bool) {
        self.elements = elements; self.cli = cli; self.cliVersion = cliVersion
        self.layoutProfile = layoutProfile; self.frameTime = frameTime
        self.unsupportedSequences = unsupportedSequences
        self.isAlternateScreen = isAlternateScreen
    }

    /// Elements the parser read directly off the screen.
    public var observed: [TranscriptElement] { elements.filter { $0.provenance == .observed } }
    /// Elements that are the parser's reading rather than the agent's words.
    public var inferred: [TranscriptElement] { elements.filter { $0.provenance == .inferred } }
}
