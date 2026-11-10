// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Darwin
import Foundation
import Testing
@testable import CuaSpacesMacKit

struct EmbeddedDriverTests {
    @Test func pathsFollowTheChannel() throws {
        let stable = try #require(EmbeddedDriver.make(binary: "/A/Contents/Helpers/cua-driver", info: [:],
                                                      bundleId: "com.trycua.spaces.macos", home: "/Users/ada"))
        #expect(stable.socket == "/Users/ada/.cua/run/cua-driver.sock")
        #expect(stable.pausedMarker == "/Users/ada/.cua/run/cua-driver.off")
        #expect(stable.startRequest == "/Users/ada/.cua/run/cua-driver.start")
        #expect(stable.serverName == "cua-driver")
        #expect(stable.mcpArguments == ["mcp", "--embedded", "--socket", "/Users/ada/.cua/run/cua-driver.sock",
                                        "--host-bundle-id", "com.trycua.spaces.macos"])

        let local = try #require(EmbeddedDriver.make(binary: "/L/Contents/Helpers/cua-driver",
                                                     info: ["CuaHome": "~/.cua-local", "CuaChannel": "local"],
                                                     bundleId: "com.trycua.spaces.macos.local", home: "/Users/ada"))
        #expect(local.socket == "/Users/ada/.cua-local/run/cua-driver.sock")
        #expect(local.serverName == "cua-driver-local")
        #expect(local.hostBundleId == "com.trycua.spaces.macos.local")
        // A Local bundle without its home never falls back to the released app's.
        #expect(EmbeddedDriver.make(binary: "/x", info: ["CuaChannel": "local"], bundleId: "x.local", home: "/Users/ada") == nil)
    }

    /// Set up and Repair start a driver that isn't answering through its
    /// own login item, then restart that item. Never while the person has
    /// paused agents, and never "set up this Mac first": whether this Mac is
    /// set up, shared, paused for another account or signed out plays no
    /// part (CUA-3638).
    @Test func setUpStartsADriverThatIsntRunning() {
        typealias D = EmbeddedDriver
        #expect(D.startStep(paused: false, answering: true, askedLoginItem: false) == .none)
        #expect(D.startStep(paused: true, answering: false, askedLoginItem: false) == .none)
        #expect(D.startStep(paused: false, answering: false, askedLoginItem: false) == .startLoginItem)
        #expect(D.startStep(paused: false, answering: false, askedLoginItem: true) == .restartLoginItem)
    }

    /// Each channel has its own Cua Driver login item, named after its host
    /// service's.
    @Test func theDriverLoginItemFollowsTheChannel() throws {
        #expect(DriverService.label(info: [:]) == "com.trycua.spaces.driver")
        #expect(DriverService.plistName(info: nil) == "com.trycua.spaces.driver.plist")
        #expect(DriverService.label(info: ["CuaHelperLabel": "com.trycua.spaces.local.helper"]) == "com.trycua.spaces.local.driver")
        let d = try #require(EmbeddedDriver.make(binary: "/A/Contents/Helpers/cua-driver", info: [:], bundleId: nil, home: "/Users/ada"))
        #expect(d.statusFile == "/Users/ada/.cua/run/cua-driver.status.json")
        #expect(d.logFile == "/Users/ada/.cua/logs/cua-driver.log")
    }

    @Test func knowsItsOwnEntry() throws {
        let d = try #require(EmbeddedDriver.make(binary: "/A/Contents/Helpers/cua-driver", info: [:], bundleId: nil, home: "/h"))
        #expect(d.isOurs(command: d.binary, args: d.mcpArguments))
        #expect(!d.isOurs(command: d.binary, args: ["mcp"]))
        #expect(!d.isOurs(command: "/Applications/CuaDriver.app/Contents/MacOS/cua-driver", args: d.mcpArguments))
    }

    @Test func readsClaudeCodesAndCodexsEntries() throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("driver-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        let claude = dir.appendingPathComponent("claude.json")
        let json: [String: Any] = ["mcpServers": ["cua-driver": ["type": "stdio", "command": "/A/cua-driver",
                                                                 "args": ["mcp", "--embedded", "--socket", "/s"]]]]
        try JSONSerialization.data(withJSONObject: json).write(to: claude)
        #expect(EmbeddedDriver.entry(named: "cua-driver", inConfig: claude.path)
            == .init(command: "/A/cua-driver", args: ["mcp", "--embedded", "--socket", "/s"]))
        #expect(EmbeddedDriver.entry(named: "cua", inConfig: claude.path) == nil)

        let codex = dir.appendingPathComponent("config.toml")
        try """
        model = "gpt"

        [mcp_servers.cua]
        command = "/x/cua"
        args = ["mcp"]

        [mcp_servers."cua-driver"]
        command = "/A/Contents/Helpers/cua-driver"
        args = ["mcp", "--embedded", "--socket", "/Users/a b/.cua/run/cua-driver.sock"]

        [profiles.x]
        command = "nope"
        """.write(to: codex, atomically: true, encoding: .utf8)
        #expect(EmbeddedDriver.entry(named: "cua-driver", inConfig: codex.path)
            == .init(command: "/A/Contents/Helpers/cua-driver",
                     args: ["mcp", "--embedded", "--socket", "/Users/a b/.cua/run/cua-driver.sock"]))
        #expect(EmbeddedDriver.entry(named: "cua", inConfig: codex.path) == .init(command: "/x/cua", args: ["mcp"]))
        #expect(EmbeddedDriver.entry(named: "missing", inConfig: codex.path) == nil)
    }

    @Test func talksToTheDriversSocket() throws {
        let dir = URL(fileURLWithPath: "/tmp").appendingPathComponent("ed-\(UUID().uuidString.prefix(8))")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        let socket = dir.appendingPathComponent("d.sock").path
        let server = try FakeDriver(path: socket) { request in
            switch request["method"] as? String {
            case "metadata":
                return ["ok": true, "result": ["driver_version": "0.38.0", "pid": 42, "embedded": true,
                                               "host_bundle_id": "com.trycua.spaces.macos"]]
            case "call" where request["name"] as? String == "check_permissions":
                return ["ok": true, "result": ["content": [["type": "text", "text": "ok"]],
                                               "structuredContent": ["accessibility": true, "screen_recording": ["granted": false]]]]
            case "call":
                return ["ok": true, "result": ["isError": true, "content": [["type": "text", "text": "no such window"]]]]
            default:
                return ["ok": false, "error": "unknown"]
            }
        }
        defer { server.stop() }
        let d = EmbeddedDriver(binary: "/x", socket: socket, hostBundleId: "com.trycua.spaces.macos", serverName: "cua-driver")
        #expect(d.metadata() == .init(version: "0.38.0", pid: 42, embedded: true, hostBundleId: "com.trycua.spaces.macos"))
        let p = try #require(try d.call("check_permissions", ["prompt": false]) as? [String: Any])
        #expect(WebUIBridge.granted(p, "accessibility"))
        #expect(!WebUIBridge.granted(p, "screen_recording"))
        #expect(throws: EmbeddedDriver.CallError.self) { try d.call("click", [:]) }

        let gone = EmbeddedDriver(binary: "/x", socket: dir.appendingPathComponent("none.sock").path,
                                  hostBundleId: "x", serverName: "cua-driver")
        #expect(gone.metadata(timeout: 0.5) == nil)
        do {
            _ = try gone.call("get_config")
            Issue.record("expected an error")
        } catch let error as EmbeddedDriver.CallError {
            #expect(error.message == "Cua Driver isn't running")
        }
    }
}

/// A one-thread Unix socket server answering one JSON line per connection.
private final class FakeDriver: @unchecked Sendable {
    let fd: Int32
    let path: String
    private var thread: Thread?

    init(path: String, answer: @escaping @Sendable ([String: Any]) -> [String: Any]) throws {
        self.path = path
        let sock = socket(AF_UNIX, SOCK_STREAM, 0)
        fd = sock
        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)
        let bytes = Array(path.utf8)
        withUnsafeMutableBytes(of: &addr.sun_path) { raw in
            raw.copyBytes(from: bytes)
            raw[bytes.count] = 0
        }
        let bound = withUnsafePointer(to: &addr) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { bind(sock, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) }
        }
        guard bound == 0, listen(sock, 8) == 0 else { throw EmbeddedDriver.CallError(message: "bind \(errno)") }
        thread = Thread {
            while true {
                let c = accept(sock, nil, nil)
                if c < 0 { return }
                var data = Data()
                var buf = [UInt8](repeating: 0, count: 4096)
                while !data.contains(0x0A) {
                    let n = recv(c, &buf, buf.count, 0)
                    if n <= 0 { break }
                    data.append(contentsOf: buf[0..<n])
                }
                let request = (try? JSONSerialization.jsonObject(with: data.prefix { $0 != 0x0A })) as? [String: Any] ?? [:]
                var out = (try? JSONSerialization.data(withJSONObject: answer(request))) ?? Data()
                out.append(0x0A)
                _ = out.withUnsafeBytes { send(c, $0.baseAddress, $0.count, 0) }
                close(c)
            }
        }
        thread?.start()
    }

    func stop() {
        shutdown(fd, SHUT_RDWR)
        close(fd)
        unlink(path)
    }
}
