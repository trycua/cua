import Foundation

/// The four id namespaces the raw protocol spells as bare `String`, given four
/// distinct types so that passing one where another belongs does not compile.
///
/// `FRICTION.md` §7: *"Four id namespaces, all bare strings … Nothing is typed,
/// so passing a window id where a run id belongs compiles."*
public protocol SpacesIdentifier:
    Hashable, Codable, Sendable, CustomStringConvertible, ExpressibleByStringLiteral
{
    var rawValue: String { get }
    init(_ rawValue: String)
}

extension SpacesIdentifier {
    public var description: String { rawValue }
    public var isEmpty: Bool { rawValue.isEmpty }
    public init(stringLiteral value: String) { self.init(value) }

    public init(from decoder: Decoder) throws {
        self.init(try decoder.singleValueContainer().decode(String.self))
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.singleValueContainer()
        try c.encode(rawValue)
    }
}

/// A Space, e.g. `local:space-e3c1b54907`, `cloud:space-9f2a` or
/// `direct:10.0.0.5:3211`.
public struct SpaceID: SpacesIdentifier {
    public let rawValue: String
    public init(_ rawValue: String) { self.rawValue = rawValue }

    /// The provider half of a qualified Space id, when there is one.
    public var providerPrefix: String? {
        // `cloud:<name>`, `local:<name>`, `direct:<host:port>`,
        // `relay:<id>`, and the `space://<provider>/…` URL form.
        if rawValue.hasPrefix("space://") {
            let rest = rawValue.dropFirst("space://".count)
            return rest.split(separator: "/", maxSplits: 1).first.map(String.init)
        }
        guard let i = rawValue.firstIndex(of: ":") else { return nil }
        return String(rawValue[..<i])
    }
}

/// One agent run, e.g. `run-84a8dc1f`.
public struct RunID: SpacesIdentifier {
    public let rawValue: String
    public init(_ rawValue: String) { self.rawValue = rawValue }

    /// The run's private directory inside the Space. Published because
    /// `FRICTION.md` §8 records that cleanup otherwise forces every caller to
    /// hard-code this path; `AgentRun.delete()` is the supported way to use it.
    public var directory: String { "~/.cua/agents/\(rawValue)" }
}

/// An rcdp capture target, e.g. `target-172aad9a-…`.
///
/// The list tool names this field `window` and the stream tool takes it as
/// `window_id` (`FRICTION.md` §7). The SDK reads every spelling and publishes
/// exactly one.
public struct WindowID: SpacesIdentifier {
    public let rawValue: String
    public init(_ rawValue: String) { self.rawValue = rawValue }

    /// rcdp targets are `target-` prefixed. An id that is not is the silent
    /// failure §7 describes — a JSON parse that succeeded against the wrong key.
    public var looksLikeRCDPTarget: Bool { rawValue.hasPrefix("target-") }
}
