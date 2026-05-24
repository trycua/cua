import Foundation
import MCP

/// Line-delimited JSON protocol between `cua-driver` CLI invocations and a
/// long-running `cua-driver serve` daemon. One request per line on stdin,
/// one response per line on stdout. The point of the daemon is to keep
/// `AppStateRegistry`-backed state (the per-pid element_index map) alive
/// across CLI invocations — a fresh `cua-driver call` otherwise starts
/// with an empty AppStateEngine and fails any element-indexed action.

/// Exit codes mirrored from `CallCommand.swift` so the CLI can reproduce
/// sysexits.h semantics after forwarding through the daemon.
public enum DaemonExit {
    public static let toolError: Int32 = 1
    public static let usage: Int32 = 64
    public static let dataError: Int32 = 65
    public static let software: Int32 = 70
}

/// Default filesystem path for the daemon's Unix domain socket. Lives under
/// `~/Library/Caches` so it's per-user and gets swept by normal macOS cache
/// hygiene. The pid file sits next to it.
public enum DaemonPaths {
    public static func defaultSocketPath() -> String {
        return cacheDirectory() + "/cua-driver.sock"
    }

    public static func defaultPidFilePath() -> String {
        return cacheDirectory() + "/cua-driver.pid"
    }

    /// Advisory-lock file held by a live daemon for the lifetime of its
    /// process. `flock(LOCK_EX|LOCK_NB)` on this fd is what makes
    /// `cua-driver serve` fail fast when another daemon is already running
    /// (or simultaneously starting). Separate from the pid file because
    /// the pid file gets atomically rewritten (tmpfile + rename), which
    /// drops any flock on the original inode.
    public static func defaultLockFilePath() -> String {
        return cacheDirectory() + "/cua-driver.lock"
    }

    public static func cacheDirectory() -> String {
        let home = ProcessInfo.processInfo.environment["HOME"] ?? NSHomeDirectory()
        return home + "/Library/Caches/cua-driver"
    }
}

/// Wire types. Requests are a tagged union on `method`; responses are a
/// single shape with `ok`, optional `result`, optional `error`, optional
/// `exitCode`. Kept intentionally tiny — line-oriented JSON on a UDS is
/// the whole protocol.

public struct DaemonRequest: Codable, Sendable {
    public let method: String
    public let name: String?
    public let args: [String: Value]?

    public init(method: String, name: String? = nil, args: [String: Value]? = nil) {
        self.method = method
        self.name = name
        self.args = args
    }
}

public struct DaemonResponse: Codable, Sendable {
    public let ok: Bool
    public let result: DaemonResult?
    public let error: String?
    public let exitCode: Int32?

    public init(
        ok: Bool,
        result: DaemonResult? = nil,
        error: String? = nil,
        exitCode: Int32? = nil
    ) {
        self.ok = ok
        self.result = result
        self.error = error
        self.exitCode = exitCode
    }
}

/// Discriminated result payload. Each CLI method returns a different shape;
/// rather than use `Value` here we keep strong types so daemon + CLI share
/// the serialization contract without either side hand-parsing JSON.
public enum DaemonResult: Codable, Sendable {
    case call(CallTool.Result)
    case list([Tool])
    case describe(Tool)

    private enum CodingKeys: String, CodingKey { case kind, payload }
    private enum Kind: String, Codable { case call, list, describe }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .call(let result):
            try container.encode(Kind.call, forKey: .kind)
            try container.encode(result, forKey: .payload)
        case .list(let tools):
            try container.encode(Kind.list, forKey: .kind)
            try container.encode(tools, forKey: .payload)
        case .describe(let tool):
            try container.encode(Kind.describe, forKey: .kind)
            try container.encode(tool, forKey: .payload)
        }
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let kind = try container.decode(Kind.self, forKey: .kind)
        switch kind {
        case .call:
            let value = try container.decode(CallTool.Result.self, forKey: .payload)
            self = .call(value)
        case .list:
            let value = try container.decode([Tool].self, forKey: .payload)
            self = .list(value)
        case .describe:
            let value = try container.decode(Tool.self, forKey: .payload)
            self = .describe(value)
        }
    }
}
