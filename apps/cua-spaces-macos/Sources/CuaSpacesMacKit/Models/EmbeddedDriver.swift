// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Darwin
import Foundation

/// Cua Driver inside Cua Spaces (CUA-2706): the `cua-driver` this app
/// carries in `Contents/Helpers`, run as `serve --embedded` by the app's
/// own Cua Driver login item (`DriverService`, `DriverSupervisor`). macOS
/// holds Cua Spaces responsible for it, so Accessibility and Screen
/// Recording are Cua Spaces' and no second app shows in System Settings.
///
/// Coding agents reach it through `cua-driver mcp --embedded --socket
/// <socket> --host-bundle-id <id>` ([`mcpArguments`]), which Set up writes
/// into their configs. The app reads its state over the same socket.
public struct EmbeddedDriver: Sendable {
    /// `Contents/Helpers/cua-driver`.
    public let binary: String
    /// `<CuaHome>/run/cua-driver.sock`: the same for every launch, because
    /// agents' configs are static files.
    public let socket: String
    /// The app's bundle id: whose permissions the driver uses.
    public let hostBundleId: String
    /// The name agents' configs list it under: `cua-driver`, or
    /// `cua-driver-local` for Cua Spaces Local, so the Local app never
    /// writes over the released app's entry.
    public let serverName: String

    /// While this file exists the person has paused agents and the login item
    /// runs no driver (`DriverSupervisor`).
    public var pausedMarker: String { ((socket as NSString).deletingLastPathComponent as NSString).appendingPathComponent("cua-driver.off") }

    /// Written to have the login item start a driver that isn't running now,
    /// instead of after its restart backoff (`DriverSupervisor`).
    public var startRequest: String { ((socket as NSString).deletingLastPathComponent as NSString).appendingPathComponent("cua-driver.start") }

    /// What its login item reports (`DriverSupervisorReport`): beside the
    /// socket.
    public var statusFile: String { ((socket as NSString).deletingLastPathComponent as NSString).appendingPathComponent("cua-driver.status.json") }

    /// `<CuaHome>`: the folder above `run/`.
    public var cuaHome: String { ((socket as NSString).deletingLastPathComponent as NSString).deletingLastPathComponent }

    /// Where the driver's own output goes: `<CuaHome>/logs/cua-driver.log`.
    public var logFile: String { ((cuaHome as NSString).appendingPathComponent("logs") as NSString).appendingPathComponent("cua-driver.log") }

    /// What Set up, Repair and turning "Let agents use apps on this Mac" on
    /// do next to get a driver that isn't answering running. Cua Driver
    /// runs from its own login item (`DriverService`), so this never
    /// depends on whether this Mac is set up, shared, paused or signed in:
    /// only the person's switch turns it off (CUA-3638).
    public enum StartStep: Equatable, Sendable {
        /// Running, or paused by the person: nothing to do.
        case none
        /// Register the login item if needed, start it and ask it for a
        /// driver now ([`startRequest`]).
        case startLoginItem
        /// That didn't bring a driver up: restart the login item.
        case restartLoginItem
    }

    public static func startStep(paused: Bool, answering: Bool, askedLoginItem: Bool) -> StartStep {
        if paused || answering { return .none }
        return askedLoginItem ? .restartLoginItem : .startLoginItem
    }

    /// This app's embedded driver, or nil when the bundle carries none (a
    /// bare build) or names a channel without a home.
    public static func current(bundle: Bundle = .main) -> EmbeddedDriver? {
        let binary = bundle.bundleURL.appendingPathComponent("Contents/Helpers/cua-driver").path
        guard FileManager.default.isExecutableFile(atPath: binary) else { return nil }
        return make(binary: binary, info: bundle.infoDictionary, bundleId: bundle.bundleIdentifier)
    }

    /// [`current`] with its inputs explicit (tests).
    static func make(binary: String, info: [String: Any]?, bundleId: String?, home: String = NSHomeDirectory()) -> EmbeddedDriver? {
        // <CuaHome>/helper/service.json -> <CuaHome>
        guard let spec = HelperService.specPath(info: info, home: home) else { return nil }
        let cuaHome = ((spec as NSString).deletingLastPathComponent as NSString).deletingLastPathComponent
        let id = bundleId ?? "com.trycua.spaces.macos"
        return EmbeddedDriver(
            binary: binary,
            socket: (cuaHome as NSString).appendingPathComponent("run/cua-driver.sock"),
            hostBundleId: id,
            serverName: id.hasSuffix(".local") ? "cua-driver-local" : "cua-driver")
    }

    /// The MCP server's arguments: the stdio proxy to the embedded driver.
    public var mcpArguments: [String] {
        ["mcp", "--embedded", "--socket", socket, "--host-bundle-id", hostBundleId]
    }

    /// Whether an agent's MCP entry runs this driver: our binary with
    /// `--embedded` and our socket.
    public func isOurs(command: String, args: [String]) -> Bool {
        let resolved = (command as NSString).resolvingSymlinksInPath
        return (command == binary || resolved == (binary as NSString).resolvingSymlinksInPath)
            && args.contains("--embedded") && args.contains(socket)
    }

    // MARK: - The driver's socket

    /// What the driver says about itself (`metadata`).
    public struct Metadata: Sendable, Equatable {
        public var version: String
        public var pid: Int32
        public var embedded: Bool
        public var hostBundleId: String?
    }

    public struct CallError: Error, Sendable, CustomStringConvertible {
        public let message: String
        public var description: String { message }
    }

    /// `metadata`, or nil when nothing answers on the socket.
    public func metadata(timeout: TimeInterval = 2) -> Metadata? {
        guard let reply = try? Self.request(socket: socket, ["method": "metadata"], timeout: timeout),
              reply["ok"] as? Bool == true, let m = reply["result"] as? [String: Any] else { return nil }
        return Metadata(version: m["driver_version"] as? String ?? "",
                        pid: (m["pid"] as? NSNumber)?.int32Value ?? 0,
                        embedded: m["embedded"] as? Bool ?? false,
                        hostBundleId: m["host_bundle_id"] as? String)
    }

    /// One tool call: the tool's structured result (else its text), or the
    /// tool's error in its own words.
    public func call(_ tool: String, _ args: [String: Any] = [:], timeout: TimeInterval = 10) throws -> Any {
        let reply: [String: Any]
        do {
            reply = try Self.request(socket: socket, ["method": "call", "name": tool, "args": args], timeout: timeout)
        } catch {
            throw CallError(message: "Cua Driver isn't running")
        }
        guard reply["ok"] as? Bool == true else {
            throw CallError(message: reply["error"] as? String ?? "\(tool) failed")
        }
        let result = reply["result"] as? [String: Any] ?? [:]
        let text = (result["content"] as? [[String: Any]] ?? []).compactMap { $0["text"] as? String }.joined(separator: "\n")
        if result["isError"] as? Bool == true { throw CallError(message: text.isEmpty ? "\(tool) failed" : text) }
        return result["structuredContent"] ?? text
    }

    /// One request on a fresh connection: a JSON line out, a JSON line back.
    static func request(socket path: String, _ body: [String: Any], timeout: TimeInterval) throws -> [String: Any] {
        let fd = Darwin.socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { throw CallError(message: "socket: \(errno)") }
        defer { close(fd) }
        var tv = timeval(tv_sec: Int(timeout), tv_usec: Int32((timeout - floor(timeout)) * 1_000_000))
        setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, socklen_t(MemoryLayout<timeval>.size))
        setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &tv, socklen_t(MemoryLayout<timeval>.size))
        var one: Int32 = 1
        setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &one, socklen_t(MemoryLayout<Int32>.size))
        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)
        let bytes = Array(path.utf8)
        guard bytes.count < MemoryLayout.size(ofValue: addr.sun_path) else { throw CallError(message: "socket path too long") }
        withUnsafeMutableBytes(of: &addr.sun_path) { raw in
            raw.copyBytes(from: bytes)
            raw[bytes.count] = 0
        }
        let connected = withUnsafePointer(to: &addr) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) }
        }
        guard connected == 0 else { throw CallError(message: "connect: \(errno)") }
        var line = try JSONSerialization.data(withJSONObject: body)
        line.append(0x0A)
        let sent = line.withUnsafeBytes { send(fd, $0.baseAddress, $0.count, 0) }
        guard sent == line.count else { throw CallError(message: "send: \(errno)") }
        var reply = Data()
        var buffer = [UInt8](repeating: 0, count: 64 * 1024)
        while !reply.contains(0x0A) {
            let n = recv(fd, &buffer, buffer.count, 0)
            guard n > 0 else { break }
            reply.append(contentsOf: buffer[0..<n])
        }
        let first = reply.split(separator: 0x0A, maxSplits: 1, omittingEmptySubsequences: true).first ?? Data()
        guard let obj = try JSONSerialization.jsonObject(with: Data(first)) as? [String: Any] else {
            throw CallError(message: "no answer")
        }
        return obj
    }

    // MARK: - Agents' configs

    /// An agent's entry for `name` in its MCP config: Claude Code's JSON
    /// (`mcpServers.<name>`) or Codex's TOML (`[mcp_servers.<name>]`).
    public struct Entry: Equatable, Sendable {
        public var command: String
        public var args: [String]
    }

    public static func entry(named name: String, inConfig path: String) -> Entry? {
        let file = (path as NSString).expandingTildeInPath
        guard let text = try? String(contentsOfFile: file, encoding: .utf8) else { return nil }
        if file.hasSuffix(".toml") { return tomlEntry(named: name, text) }
        guard let obj = try? JSONSerialization.jsonObject(with: Data(text.utf8)) as? [String: Any],
              let server = (obj["mcpServers"] as? [String: Any])?[name] as? [String: Any],
              let command = server["command"] as? String else { return nil }
        return Entry(command: command, args: server["args"] as? [String] ?? [])
    }

    /// `[mcp_servers.<name>]` (or `."<name>"`) with its `command` and
    /// `args`: the two keys Set up writes, each on one line.
    static func tomlEntry(named name: String, _ text: String) -> Entry? {
        let headers = ["[mcp_servers.\(name)]", "[mcp_servers.\"\(name)\"]"]
        var inside = false
        var command: String?
        var args: [String] = []
        for raw in text.split(separator: "\n", omittingEmptySubsequences: false) {
            let line = raw.trimmingCharacters(in: .whitespaces)
            if line.hasPrefix("[") {
                if inside { break }
                inside = headers.contains(line)
                continue
            }
            guard inside, let eq = line.firstIndex(of: "=") else { continue }
            let key = line[..<eq].trimmingCharacters(in: .whitespaces)
            let value = String(line[line.index(after: eq)...]).trimmingCharacters(in: .whitespaces)
            if key == "command" { command = tomlStrings(value).first }
            if key == "args" { args = tomlStrings(value) }
        }
        return command.map { Entry(command: $0, args: args) }
    }

    /// The double-quoted strings in a TOML value (`"a"` or `["a", "b"]`).
    static func tomlStrings(_ value: String) -> [String] {
        var out: [String] = []
        var current: String?
        var escaped = false
        for ch in value {
            if var s = current {
                if escaped { s.append(ch); escaped = false; current = s; continue }
                if ch == "\\" { escaped = true; continue }
                if ch == "\"" { out.append(s); current = nil; continue }
                s.append(ch)
                current = s
            } else if ch == "\"" {
                current = ""
            }
        }
        return out
    }
}
