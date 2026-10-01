import Cua
import Foundation

/// The one thing the SDK needs from a Spaces backend: call a named tool with
/// named arguments and get JSON back, or throw.
///
/// Everything above this line — Spaces, runs, rosters, transfers, streams — is
/// written against this protocol and nothing else, so a test can drive the
/// whole SDK against an in-process fake and a live suite can drive the same
/// code against a real Space.
public protocol SpacesTransport: Sendable {
    /// - Returns: the tool's payload, already parsed when it was JSON and
    ///   `.string` when the tool answered in prose.
    /// - Throws: `SpacesError.toolFailed` for any `isError` result, so that a
    ///   failure can never be returned as a value (`FRICTION.md` §3).
    func callTool(_ name: String, _ arguments: [String: JSONValue]) async throws -> JSONValue

    /// Tool names the backend offers. Empty when the transport cannot say.
    func availableTools() async throws -> [String]
}

extension SpacesTransport {
    public func availableTools() async throws -> [String] { [] }
}

extension SpacesTransport {
    /// The payload as an object, or `malformedResponse`.
    func object(_ name: String, _ arguments: [String: JSONValue]) async throws -> [String: JSONValue] {
        let out = try await callTool(name, arguments)
        guard let o = out.objectValue else {
            throw SpacesError.malformedResponse(tool: name, detail: "not an object: \(out)")
        }
        return o
    }

    /// The payload as an array.
    ///
    /// Several tools answer "none" with an English sentence rather than an
    /// empty array (`FRICTION.md` §4: *"`list_spaces` … returns the sentence
    /// 'No Spaces. Use create_space …' when there are none"*). An SDK must never
    /// make an app parse prose, so a non-array payload becomes the empty
    /// collection here — once, for every caller.
    func array(_ name: String, _ arguments: [String: JSONValue],
               unwrapping key: String? = nil) async throws -> [JSONValue] {
        let out = try await callTool(name, arguments)
        if let a = out.arrayValue { return a }
        if let key, let a = out[key]?.arrayValue { return a }
        return []
    }
}

/// The live transport: the cua SDK's own Spaces runtime.
///
/// Every call is `CuaSDK.Spaces.callToolJson`, which runs the contract tool in
/// the Rust `cua-spaces` crate — in this process for `Cua.embedded(...)`, or in
/// a running `cua daemon` for `Cua.connect(...)` (through
/// `SpaceService.CallSpaceTool`). There is no child process, no stdio framing
/// and no Python: the framing desync `FRICTION.md` §1 records cannot happen
/// because there is no stream to frame.
///
/// Stateless and `Sendable`; the SDK objects it holds are thread-safe, so any
/// actor (including `@MainActor`) may call it (`FRICTION.md` §30).
public struct CuaSpacesTransport: SpacesTransport {
    /// The generated Spaces object this transport drives.
    public let spaces: CuaSDK.Spaces

    public init(spaces: CuaSDK.Spaces) {
        self.spaces = spaces
    }

    public init(cua: Cua) {
        self.init(spaces: cua.spaces())
    }

    public func callTool(_ name: String, _ arguments: [String: JSONValue]) async throws -> JSONValue {
        SpacesCallCounter.record(name)
        let json = String(decoding: try JSONSerialization.data(
            withJSONObject: JSONValue.object(arguments).foundationObject), as: UTF8.self)
        let result: SpaceToolResult
        do {
            result = try await spaces.callToolJson(tool: name, argumentsJson: json)
        } catch let error as CuaError {
            throw SpacesError(cua: error, tool: name)
        }
        // A failing tool is a *result* carrying `isError` (§3); it becomes a
        // throw here, once, for every caller.
        if result.isError {
            throw SpacesError.toolFailed(tool: name, message: result.text)
        }
        if result.text.isEmpty { return .object([:]) }
        return JSONValue.parse(result.text) ?? .string(result.text)
    }

    public func availableTools() async throws -> [String] {
        let text = try await spaces.listToolsJson()
        let parsed = JSONValue.parse(text) ?? .object([:])
        return (parsed["tools"]?.arrayValue ?? []).compactMap { $0["name"]?.stringValue }
    }
}

extension SpacesError {
    /// A typed SDK error, as the overlay's error. The SDK's case survives in
    /// the message; capability and consent refusals keep their own cases.
    init(cua error: CuaError, tool: String) {
        switch error {
        case let .TeleportRefused(m):
            self = .teleportRefused(m)
        case let .Transport(m):
            self = .transportUnavailable(m)
        default:
            self = .toolFailed(tool: tool, message: "\(error)")
        }
    }
}

/// How many times each tool has been called over the live transport,
/// process-wide.
///
/// The honest measure of what a polling UI costs. Wall-clock stall is noisy —
/// it moves with link latency, how many runs the Space happens to hold and
/// what else the machine is doing — but the number of round trips a poll tick
/// costs is deterministic, and it is the thing that is usually actually wrong:
/// OpenKoalaBots's roster used to cost `1 + N` calls per tick for an N-Bot
/// roster, which is what made its window freeze.
///
/// This lived in the app's own MCP client until the SDK absorbed the
/// transport. It belongs here: a counter that only saw the app's calls could
/// not see the SDK's, and after the extraction nearly every call is the SDK's.
/// `samples/openkoalabots/FRICTION.md` §51.
public enum SpacesCallCounter {
    private static let lock = NSLock()
    nonisolated(unsafe) private static var counts: [String: Int] = [:]

    public static func record(_ tool: String) {
        lock.lock(); defer { lock.unlock() }
        counts[tool, default: 0] += 1
    }

    /// A snapshot, tool name to call count.
    public static var counted: [String: Int] {
        lock.lock(); defer { lock.unlock() }
        return counts
    }

    public static var total: Int {
        lock.lock(); defer { lock.unlock() }
        return counts.values.reduce(0, +)
    }

    /// Forget everything counted so far — for a measurement that wants a
    /// defined starting point.
    public static func reset() {
        lock.lock(); defer { lock.unlock() }
        counts.removeAll()
    }
}

public enum CuaSpacesSDK {
    public static let version = "0.2.0"
}
