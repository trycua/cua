import Foundation

/// The serialisable document a non-Swift renderer consumes.
///
/// Two stages, kept apart on purpose: `ClaudeCodeParser` turns a frame into
/// `ParsedFrame` (a Swift model of what is on screen), and *this* file turns
/// that into a document with no Swift in it. A JavaScript or Python client
/// gets the same structure, and a change to the Swift model that does not
/// change the document is not a wire change.
///
/// # Schema `cua.transcript.jsonui/1`
///
/// ```jsonc
/// {
///   "schema": "cua.transcript.jsonui/1",
///   "source": {
///     "cli": "claude",                 // string|null, as invoked
///     "cliVersion": "2.1.278 (Claude Code)", // string|null, verbatim
///     "layoutProfile": "claude-code/2.x",    // string|null; null = nothing matched
///     "frameTime": 12.734,             // seconds into the recording
///     "alternateScreen": true,         // the agent UI lives here
///     "unsupportedSequences": 0        // >0 means the frame is incomplete
///   },
///   "elements": [ <element>, ... ]
/// }
/// ```
///
/// Every `<element>` has the same three required keys and then a type-specific
/// body:
///
/// ```jsonc
/// {
///   "type": "toolCall",
///   "provenance": "observed" | "inferred" | "unrecognised",
///   "region": {"firstRow": 11, "lastRow": 12},
///   ...
/// }
/// ```
///
/// `provenance` is not decoration. `observed` means the text is on the screen
/// and this element is a direct reading of it. `inferred` means the parser
/// concluded it from layout, colour or adjacency and the agent never said it.
/// `unrecognised` means the parser is handing back text it could not classify.
/// A renderer that draws a confident card for an `inferred` element is
/// misrepresenting the agent, which is the defect this schema exists to make
/// impossible to commit by accident.
///
/// Element types and their bodies:
///
/// * `agentMessage` — `markdown` (string), `text` (string)
/// * `userMessage` — `text` (string)
/// * `toolCall` — `name` (string), `argumentSummary` (string|null),
///   `status` ("succeeded"|"failed"|"running"|"completed"|"unknown"),
///   `statusProvenance` (a `provenance` value, separate because the name can
///   be read while the status is concluded), `resultLines` (string[]),
///   `collapsed` (bool)
/// * `subagent` — `kind` (string), `task` (string|null), `status`,
///   `statusProvenance`, `detailLines` (string[])
/// * `agentRunning` — `spinner` (string|null), `verb` (string|null),
///   `elapsedSeconds` (int|null), `tokens` (int|null), `hint` (string|null),
///   `rawLine` (string)
/// * `composer` — `content` (string), `placeholder` (string|null),
///   `cursorIsInside` (bool)
/// * `modeIndicator` — `mode` ("normal"|"accept_edits"|"plan"|
///   "bypass_permissions"|"unrecognised"), `rawText` (string),
///   `hint` (string|null)
/// * `question` — `prompt` (string), `options` ([{key: string|null,
///   label: string}]), `selectedIndex` (int|null)
/// * `diff` — `path` (string|null), `addedCount` (int), `removedCount` (int),
///   `lines` ([{kind: "added"|"removed"|"context", number: int|null,
///   text: string}])
/// * `error` — `text` (string)
/// * `turnBoundary` — `kind` ("completed"|"interrupted"), `text` (string)
/// * `rawText` — `lines` (string[]); always `provenance: "unrecognised"`
///
/// ## Versioning
///
/// The `schema` string carries the version. Adding an optional key or a new
/// element type is a minor change and keeps `/1`; removing a key, renaming
/// one, or changing what a `provenance` value means requires `/2`. A consumer
/// that does not recognise an element `type` must render its `region` and
/// whatever strings it has rather than dropping it — `rawText` exists so
/// there is always something to render.
public enum JSONUI {
    public static let schemaIdentifier = "cua.transcript.jsonui/1"
}

/// A JSON document, held as ordered key/value pairs so the encoding is stable
/// and a golden diff is readable.
public struct JSONUIDocument: Sendable, Equatable {
    public let value: JSONUINode
    public init(value: JSONUINode) { self.value = value }

    public func jsonData(pretty: Bool = true) throws -> Data {
        var options: JSONSerialization.WritingOptions = [.sortedKeys]
        if pretty { options.insert(.prettyPrinted) }
        options.insert(.withoutEscapingSlashes)
        return try JSONSerialization.data(withJSONObject: value.foundationObject,
                                          options: options)
    }

    public func prettyJSONString() throws -> String {
        String(decoding: try jsonData(pretty: true), as: UTF8.self)
    }
}

/// A minimal JSON tree. The module has no dependencies, and `Codable` on an
/// enum with heterogeneous payloads costs more clarity than it buys here.
public indirect enum JSONUINode: Sendable, Equatable {
    case string(String)
    case int(Int)
    case double(Double)
    case bool(Bool)
    case null
    case array([JSONUINode])
    case object([(String, JSONUINode)])

    public static func == (lhs: JSONUINode, rhs: JSONUINode) -> Bool {
        switch (lhs, rhs) {
        case (.string(let a), .string(let b)): return a == b
        case (.int(let a), .int(let b)): return a == b
        case (.double(let a), .double(let b)): return a == b
        case (.bool(let a), .bool(let b)): return a == b
        case (.null, .null): return true
        case (.array(let a), .array(let b)): return a == b
        case (.object(let a), .object(let b)):
            guard a.count == b.count else { return false }
            let left = Dictionary(uniqueKeysWithValues: a.map { ($0.0, $0.1) })
            let right = Dictionary(uniqueKeysWithValues: b.map { ($0.0, $0.1) })
            return left == right
        default: return false
        }
    }

    var foundationObject: Any {
        switch self {
        case .string(let value): return value
        case .int(let value): return value
        case .double(let value): return value
        case .bool(let value): return value
        case .null: return NSNull()
        case .array(let items): return items.map(\.foundationObject)
        case .object(let pairs):
            var dictionary: [String: Any] = [:]
            for (key, node) in pairs { dictionary[key] = node.foundationObject }
            return dictionary
        }
    }

    static func optionalString(_ value: String?) -> JSONUINode {
        value.map { .string($0) } ?? .null
    }
    static func optionalInt(_ value: Int?) -> JSONUINode {
        value.map { .int($0) } ?? .null
    }
}

public extension ParsedFrame {
    /// Stage two: the Swift model becomes a document with no Swift in it.
    func jsonUIDocument() -> JSONUIDocument {
        let source: JSONUINode = .object([
            ("cli", .optionalString(cli)),
            ("cliVersion", .optionalString(cliVersion)),
            ("layoutProfile", .optionalString(layoutProfile)),
            ("frameTime", .double((frameTime * 1000).rounded() / 1000)),
            ("alternateScreen", .bool(isAlternateScreen)),
            ("unsupportedSequences", .int(unsupportedSequences)),
        ])
        return JSONUIDocument(value: .object([
            ("schema", .string(JSONUI.schemaIdentifier)),
            ("source", source),
            ("elements", .array(elements.map(\.jsonUINode))),
        ]))
    }
}

public extension TranscriptElement {
    var jsonUINode: JSONUINode {
        var pairs: [(String, JSONUINode)] = [
            ("type", .string(typeName)),
            ("provenance", .string(provenance.rawValue)),
            ("region", .object([("firstRow", .int(region.firstRow)),
                                ("lastRow", .int(region.lastRow))])),
        ]
        pairs.append(contentsOf: bodyPairs)
        return .object(pairs)
    }

    var typeName: String {
        switch self {
        case .agentMessage: return "agentMessage"
        case .userMessage: return "userMessage"
        case .toolCall: return "toolCall"
        case .subagent: return "subagent"
        case .agentRunning: return "agentRunning"
        case .composer: return "composer"
        case .modeIndicator: return "modeIndicator"
        case .question: return "question"
        case .diff: return "diff"
        case .error: return "error"
        case .turnBoundary: return "turnBoundary"
        case .rawText: return "rawText"
        }
    }

    private var bodyPairs: [(String, JSONUINode)] {
        switch self {
        case .agentMessage(let value):
            return [("markdown", .string(value.markdown)), ("text", .string(value.text))]
        case .userMessage(let value):
            return [("text", .string(value.text))]
        case .toolCall(let value):
            return [("name", .string(value.name)),
                    ("argumentSummary", .optionalString(value.argumentSummary)),
                    ("status", .string(value.status.rawValue)),
                    ("statusProvenance", .string(value.statusProvenance.rawValue)),
                    ("resultLines", .array(value.resultLines.map { .string($0) })),
                    ("collapsed", .bool(value.isCollapsed))]
        case .subagent(let value):
            return [("kind", .string(value.kind)),
                    ("task", .optionalString(value.task)),
                    ("status", .string(value.status.rawValue)),
                    ("statusProvenance", .string(value.statusProvenance.rawValue)),
                    ("detailLines", .array(value.detailLines.map { .string($0) }))]
        case .agentRunning(let value):
            return [("spinner", .optionalString(value.spinner)),
                    ("verb", .optionalString(value.verb)),
                    ("elapsedSeconds", .optionalInt(value.elapsedSeconds)),
                    ("tokens", .optionalInt(value.tokens)),
                    ("hint", .optionalString(value.hint)),
                    ("rawLine", .string(value.rawLine))]
        case .composer(let value):
            return [("content", .string(value.content)),
                    ("placeholder", .optionalString(value.placeholder)),
                    ("cursorIsInside", .bool(value.cursorIsInside))]
        case .modeIndicator(let value):
            return [("mode", .string(value.mode.rawValue)),
                    ("rawText", .string(value.rawText)),
                    ("hint", .optionalString(value.hint))]
        case .question(let value):
            return [("prompt", .string(value.prompt)),
                    ("options", .array(value.options.map { option in
                        .object([("key", .optionalString(option.key)),
                                 ("label", .string(option.label))])
                    })),
                    ("selectedIndex", .optionalInt(value.selectedIndex))]
        case .diff(let value):
            return [("path", .optionalString(value.path)),
                    ("addedCount", .int(value.addedCount)),
                    ("removedCount", .int(value.removedCount)),
                    ("lines", .array(value.hunkLines.map { line in
                        .object([("kind", .string(line.kind.rawValue)),
                                 ("number", .optionalInt(line.number)),
                                 ("text", .string(line.text))])
                    }))]
        case .error(let value):
            return [("text", .string(value.text))]
        case .turnBoundary(let value):
            return [("kind", .string(value.kind.rawValue)), ("text", .string(value.text))]
        case .rawText(let value):
            return [("lines", .array(value.lines.map { .string($0) }))]
        }
    }
}
