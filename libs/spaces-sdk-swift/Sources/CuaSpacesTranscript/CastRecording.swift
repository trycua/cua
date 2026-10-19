import Foundation

/// An asciinema v2 recording: a header line, then one JSON array per event.
///
/// The format is a documented standard rather than something invented here, so
/// a golden stays inspectable with `head` and `jq`, diffs legibly in review,
/// and can be played by `asciinema play` by anyone who wants to check the
/// fixture with their own eyes instead of trusting our parser.
public struct CastRecording: Sendable {
    public struct Header: Sendable {
        public let version: Int
        public let width: Int
        public let height: Int
        public let timestamp: Int?
        /// Which CLI produced this recording, as invoked.
        public let cli: String?
        /// The CLI's own `--version` output, recorded verbatim at capture time.
        ///
        /// Claude Code's rendering changes between releases. A parser result
        /// that does not travel with the version of the thing it parsed is not
        /// evidence about anything, so this is carried through into every
        /// `ParsedFrame`.
        public let cliVersion: String?
        public let scriptName: String?
        public let description: String?
        /// The scrub rules that were applied when the cast was written.
        public let scrubbed: [String]
        /// Anything the recorder could not make happen (an `expect` that timed
        /// out, a pty that closed early). Recorded, never hidden.
        public let notes: [String]
    }

    public enum EventKind: String, Sendable {
        case output = "o"
        case input = "i"
        case marker = "m"
        case resize = "r"
    }

    public struct Event: Sendable {
        public let time: TimeInterval
        public let kind: EventKind
        public let data: String
    }

    public let header: Header
    public let events: [Event]

    /// Wall-clock length of the recording.
    public var duration: TimeInterval { events.last?.time ?? 0 }

    public init(header: Header, events: [Event]) {
        self.header = header
        self.events = events
    }

    public init(contentsOf url: URL) throws {
        try self.init(text: String(contentsOf: url, encoding: .utf8))
    }

    public init(text: String) throws {
        var lines = text.split(separator: "\n", omittingEmptySubsequences: true)
        guard !lines.isEmpty else { throw TranscriptError.malformedCast("empty file") }
        let headerLine = lines.removeFirst()

        guard let headerData = headerLine.data(using: .utf8),
              let raw = try JSONSerialization.jsonObject(with: headerData) as? [String: Any],
              let version = raw["version"] as? Int,
              let width = raw["width"] as? Int,
              let height = raw["height"] as? Int
        else { throw TranscriptError.malformedCast("header is not an asciinema v2 header") }
        guard version == 2 else {
            throw TranscriptError.malformedCast("unsupported cast version \(version)")
        }

        header = Header(version: version,
                        width: width,
                        height: height,
                        timestamp: raw["timestamp"] as? Int,
                        cli: raw["cua_cli"] as? String,
                        cliVersion: raw["cua_cli_version"] as? String,
                        scriptName: raw["cua_script"] as? String,
                        description: raw["cua_description"] as? String,
                        scrubbed: raw["cua_scrubbed"] as? [String] ?? [],
                        notes: raw["cua_notes"] as? [String] ?? [])

        var parsed: [Event] = []
        parsed.reserveCapacity(lines.count)
        for line in lines {
            guard let data = line.data(using: .utf8),
                  let array = try? JSONSerialization.jsonObject(with: data) as? [Any],
                  array.count >= 3,
                  let time = (array[0] as? NSNumber)?.doubleValue,
                  let kindRaw = array[1] as? String,
                  let payload = array[2] as? String
            else { continue }
            parsed.append(Event(time: time,
                                kind: EventKind(rawValue: kindRaw) ?? .output,
                                data: payload))
        }
        events = parsed
    }
}

public enum TranscriptError: Error, CustomStringConvertible, Sendable {
    case malformedCast(String)

    public var description: String {
        switch self {
        case .malformedCast(let why): return "malformed cast: \(why)"
        }
    }
}
