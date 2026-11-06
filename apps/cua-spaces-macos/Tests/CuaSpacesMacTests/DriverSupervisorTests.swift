// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Darwin
import Foundation
import Testing
@testable import CuaSpacesMacKit

/// Cua Driver's login item (CUA-3638): it starts the driver whatever this
/// Mac shares, restarts it with a backoff, honours the person's pause and,
/// when the driver won't start, says why in the report the page reads.
@Suite(.serialized)
struct DriverSupervisorTests {
    /// A scratch Cua home under /tmp (socket paths must stay short).
    final class Home: @unchecked Sendable {
        let dir: URL
        init() throws {
            dir = URL(fileURLWithPath: "/tmp").appendingPathComponent("ds-\(UUID().uuidString.prefix(8))")
            try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        }
        deinit { try? FileManager.default.removeItem(at: dir) }

        func driver(script: String) throws -> EmbeddedDriver {
            let bin = dir.appendingPathComponent("cua-driver")
            try script.write(to: bin, atomically: true, encoding: .utf8)
            chmod(bin.path, 0o755)
            return EmbeddedDriver(binary: bin.path, socket: dir.appendingPathComponent("run/cua-driver.sock").path,
                                  hostBundleId: "com.trycua.spaces.macos", serverName: "cua-driver")
        }
    }

    /// A stand-in for `cua-driver serve --embedded`: answers `metadata` on
    /// its socket as itself, and exits when its stdin closes (the parent
    /// liveness pipe), as the real one does.
    static let answeringDriver = #"""
    #!/usr/bin/python3
    import json, os, socket, sys, threading
    a = sys.argv
    path = a[a.index("--socket") + 1]
    host = a[a.index("--host-bundle-id") + 1]
    assert "--embedded" in a and "--parent-liveness-stdio" in a
    def liveness():
        sys.stdin.buffer.read()
        os._exit(0)
    threading.Thread(target=liveness, daemon=True).start()
    try: os.unlink(path)
    except FileNotFoundError: pass
    s = socket.socket(socket.AF_UNIX); s.bind(path); s.listen(8)
    while True:
        c, _ = s.accept()
        c.recv(65536)
        c.sendall((json.dumps({"ok": True, "result": {"driver_version": "9.9.9", "pid": os.getpid(), "embedded": True, "host_bundle_id": host}}) + "\n").encode())
        c.close()
    """#

    static func options() -> DriverSupervisor.Options {
        var o = DriverSupervisor.Options()
        o.startupTimeout = 5
        o.poll = 0.05
        o.maxDelay = 0.2
        return o
    }

    /// Runs `supervisor` on a thread until `until` holds for its report
    /// (or 15 s), then stops it and returns the last report.
    static func run(_ supervisor: DriverSupervisor, until: @escaping (DriverSupervisorReport) -> Bool) -> DriverSupervisorReport? {
        let stop = Flag()
        let done = DispatchSemaphore(value: 0)
        Thread.detachNewThread {
            supervisor.run { stop.isSet }
            done.signal()
        }
        let end = Date().addingTimeInterval(15)
        var last: DriverSupervisorReport?
        while Date() < end {
            last = DriverSupervisorReport.read(supervisor.driver.statusFile)
            if let last, until(last) { break }
            Thread.sleep(forTimeInterval: 0.05)
        }
        stop.set()
        _ = done.wait(timeout: .now() + 15)
        return last
    }

    final class Flag: @unchecked Sendable {
        private let lock = NSLock()
        private var value = false
        var isSet: Bool { lock.lock(); defer { lock.unlock() }; return value }
        func set() { lock.lock(); value = true; lock.unlock() }
    }

    /// A driver that won't start is not a silent "not running": the report
    /// carries its exit and its last words, and it is tried again.
    @Test func aDriverThatWontStartSaysWhy() throws {
        let home = try Home()
        let driver = try home.driver(script: "#!/bin/sh\necho 'error: socket is held by another process' >&2\nexit 3\n")
        let report = Self.run(DriverSupervisor(driver: driver, options: Self.options())) { $0.restarts >= 2 }
        let r = try #require(report)
        let error = try #require(r.lastError)
        #expect(error.contains("exit code 3"), "\(error)")
        #expect(error.contains("socket is held by another process"), "\(error)")
        #expect(r.restarts >= 2)
        #expect(FileManager.default.fileExists(atPath: driver.logFile))
        // The run folder the socket lives in is private.
        let mode = try FileManager.default.attributesOfItem(atPath: (driver.socket as NSString).deletingLastPathComponent)[.posixPermissions] as? Int
        #expect(mode == 0o700)
    }

    /// A missing binary is reported plainly.
    @Test func aMissingDriverIsReported() throws {
        let home = try Home()
        var driver = try home.driver(script: "#!/bin/sh\n")
        driver = EmbeddedDriver(binary: home.dir.appendingPathComponent("nope").path, socket: driver.socket,
                                hostBundleId: driver.hostBundleId, serverName: driver.serverName)
        let r = try #require(Self.run(DriverSupervisor(driver: driver, options: Self.options())) { $0.lastError != nil })
        #expect(r.lastError?.contains("there is no Cua Driver") == true)
    }

    /// Started, it answers as this child, embedded, for this app; killed,
    /// it comes back as a new process; stopped, it is gone.
    @Test func startsAndRestartsTheDriver() throws {
        guard FileManager.default.isExecutableFile(atPath: "/usr/bin/python3") else { return }
        let home = try Home()
        let driver = try home.driver(script: Self.answeringDriver)
        let supervisor = DriverSupervisor(driver: driver, options: Self.options())
        var firstPid: Int32?
        let r = try #require(Self.run(supervisor) { report in
            guard report.state == "ready", let pid = report.pid else { return false }
            if firstPid == nil {
                firstPid = pid
                kill(pid, SIGKILL)
                return false
            }
            return pid != firstPid
        })
        #expect(r.state == "ready")
        #expect(r.restarts >= 1)
        #expect(r.lastError == nil)
        // After the stop, nothing answers and the report says stopped.
        #expect(driver.metadata(timeout: 0.5) == nil)
        #expect(DriverSupervisorReport.read(driver.statusFile)?.state == "stopped")
    }

    /// A driver killed outright leaves its socket file behind, and the
    /// real one refuses to start over it ("embedded daemon endpoint already
    /// exists"): the supervisor clears a socket nobody answers on, as the
    /// SDK's host does, and reports one that does answer or a path that
    /// isn't a socket (seen on the VM check for 0.21.1).
    @Test func aStaleSocketIsClearedBeforeAStart() throws {
        let home = try Home()
        let path = home.dir.appendingPathComponent("s.sock").path
        #expect(DriverSupervisor.prepareEndpoint(path) == nil, "nothing there")
        // A socket left by a dead process: bound, never listened on.
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)
        withUnsafeMutableBytes(of: &addr.sun_path) { raw in
            let b = Array(path.utf8)
            raw.copyBytes(from: b)
            raw[b.count] = 0
        }
        let bound = withUnsafePointer(to: &addr) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { bind(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) }
        }
        #expect(bound == 0)
        close(fd)
        #expect(FileManager.default.fileExists(atPath: path))
        #expect(DriverSupervisor.prepareEndpoint(path) == nil)
        #expect(!FileManager.default.fileExists(atPath: path), "the stale socket is gone")
        // A file in the way is reported, never removed.
        FileManager.default.createFile(atPath: path, contents: Data())
        #expect(DriverSupervisor.prepareEndpoint(path)?.contains("not a socket") == true)
        #expect(FileManager.default.fileExists(atPath: path))
    }

    /// Killed with SIGKILL (its socket stays), the real driver's refusal
    /// to start over a stale socket never sticks: it comes back.
    @Test func comesBackAfterSIGKILLLeavesItsSocket() throws {
        guard FileManager.default.isExecutableFile(atPath: "/usr/bin/python3") else { return }
        let home = try Home()
        // Like the real one: refuses an existing socket path.
        let script = Self.answeringDriver.replacingOccurrences(
            of: "try: os.unlink(path)\nexcept FileNotFoundError: pass",
            with: "if os.path.exists(path):\n    sys.stderr.write('embedded daemon endpoint already exists\\n'); sys.exit(1)")
        #expect(script != Self.answeringDriver)
        let driver = try home.driver(script: script)
        var firstPid: Int32?
        let r = try #require(Self.run(DriverSupervisor(driver: driver, options: Self.options())) { report in
            guard report.state == "ready", let pid = report.pid else { return false }
            if firstPid == nil {
                firstPid = pid
                kill(pid, SIGKILL)
                return false
            }
            return pid != firstPid
        })
        #expect(r.state == "ready", "\(r)")
        #expect(r.pid != firstPid)
    }

    /// Paused by the person: no driver runs, and it starts once the pause
    /// marker goes.
    @Test func thePauseKeepsItOff() throws {
        let home = try Home()
        let ran = home.dir.appendingPathComponent("ran").path
        let driver = try home.driver(script: "#!/bin/sh\ntouch '\(ran)'\nexit 1\n")
        try FileManager.default.createDirectory(atPath: (driver.pausedMarker as NSString).deletingLastPathComponent, withIntermediateDirectories: true)
        FileManager.default.createFile(atPath: driver.pausedMarker, contents: Data("1\n".utf8))
        let r = try #require(Self.run(DriverSupervisor(driver: driver, options: Self.options())) { $0.state == "paused" })
        #expect(r.state == "paused")
        #expect(!FileManager.default.fileExists(atPath: ran))
    }

    @Test func restartsBackOffToAMinute() {
        let secs = (1...9).map { Int(DriverSupervisor.restartDelay(attempt: $0)) }
        #expect(secs == [1, 2, 4, 8, 16, 32, 60, 60, 60])
    }

    /// The driver gets the SDK's arguments and only a safe environment.
    @Test func argumentsAndEnvironment() {
        let d = EmbeddedDriver(binary: "/A/cua-driver", socket: "/h/.cua/run/cua-driver.sock",
                               hostBundleId: "com.trycua.spaces.macos", serverName: "cua-driver")
        #expect(DriverSupervisor.serveArguments(d) == [
            "serve", "--embedded", "--parent-liveness-stdio", "--no-permissions-gate",
            "--socket", "/h/.cua/run/cua-driver.sock", "--host-bundle-id", "com.trycua.spaces.macos",
            "--permission-mode", "standard",
        ])
        let env = DriverSupervisor.childEnvironment(["HOME": "/h", "PATH": "/bin", "LC_ALL": "C", "SECRET_TOKEN": "x",
                                                     "CUA_DRIVER_KEY_GAP_MS": "5", "DYLD_INSERT_LIBRARIES": "/x"], hostPid: 7)
        #expect(env == ["HOME": "/h", "PATH": "/bin", "LC_ALL": "C", "CUA_DRIVER_KEY_GAP_MS": "5",
                        "CUA_DRIVER_EMBEDDED_HOST_PID": "7"])
    }
}
