import Darwin
import Foundation
import Testing

@testable import lume

/// A stand-in for the guest agent: each "vsock" connect is one end of a
/// Unix socket pair whose other end runs `agent` on a thread.
private func fakeConnect(
    agent: @escaping @Sendable (Int32) -> Void
) -> VsockForwarder.Connect {
    return { _ in
        var fds: [Int32] = [0, 0]
        guard socketpair(AF_UNIX, SOCK_STREAM, 0, &fds) == 0 else { return nil }
        let (host, guest) = (fds[0], fds[1])
        let t = Thread {
            agent(guest)
            Darwin.close(guest)
        }
        t.start()
        return VsockForwarder.Stream(fd: host, close: { Darwin.close(host) })
    }
}

/// An agent that answers a ping, and a forward with an echo of what follows.
private let echoAgent: @Sendable (Int32) -> Void = { fd in
    guard let line = VsockForwarder.readLine(fd, limit: 4096) else { return }
    if line.contains("\"ping\"") {
        VsockForwarder.writeAll(fd, Data("{\"ok\":true}\n".utf8))
        return
    }
    guard line == "{\"op\":\"forward\",\"port\":3211}" else {
        VsockForwarder.writeAll(fd, Data("{\"ok\":false,\"error\":\"bad\"}\n".utf8))
        return
    }
    VsockForwarder.writeAll(fd, Data("{\"ok\":true}\n".utf8))
    var buf = [UInt8](repeating: 0, count: 1024)
    while true {
        let n = read(fd, &buf, buf.count)
        if n <= 0 { break }
        VsockForwarder.writeAll(fd, Data(buf[0..<n]))
    }
    shutdown(fd, SHUT_WR)
}

private func dialLoopback(_ port: UInt16) -> Int32 {
    let fd = socket(AF_INET, SOCK_STREAM, 0)
    var a = sockaddr_in()
    a.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
    a.sin_family = sa_family_t(AF_INET)
    a.sin_port = port.bigEndian
    a.sin_addr.s_addr = inet_addr("127.0.0.1")
    let rc = withUnsafePointer(to: &a) {
        $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
            connect(fd, $0, socklen_t(MemoryLayout<sockaddr_in>.size))
        }
    }
    #expect(rc == 0)
    return fd
}

private func readAll(_ fd: Int32) -> String {
    var out: [UInt8] = []
    var buf = [UInt8](repeating: 0, count: 1024)
    while true {
        let n = read(fd, &buf, buf.count)
        if n <= 0 { break }
        out.append(contentsOf: buf[0..<n])
    }
    return String(decoding: out, as: UTF8.self)
}

@Suite("Host port forwards over vsock")
struct VsockForwarderTests {
    @Test("A loopback connection reaches the guest port through the agent")
    func forwardsThroughTheAgent() throws {
        let forwarder = try VsockForwarder(guestPorts: [3211], connect: fakeConnect(agent: echoAgent))
        defer { forwarder.stop() }
        let host = try #require(forwarder.forwards[3211])
        #expect(host != 0 && host != 3211)
        forwarder.start(onAgent: {})
        let c = dialLoopback(host)
        defer { Darwin.close(c) }
        #expect(VsockForwarder.writeAll(c, Data("hello".utf8)))
        shutdown(c, SHUT_WR)
        #expect(readAll(c) == "hello")
    }

    @Test("Without the agent a connection closes at once")
    func noAgentClosesTheConnection() throws {
        let forwarder = try VsockForwarder(guestPorts: [3211], connect: { _ in nil })
        defer { forwarder.stop() }
        forwarder.start(onAgent: { Issue.record("no agent answered") })
        let c = dialLoopback(try #require(forwarder.forwards[3211]))
        defer { Darwin.close(c) }
        #expect(readAll(c) == "")
        #expect(!forwarder.ping())
    }

    @Test("The agent's first answer to a ping is reported once")
    func agentReadiness() throws {
        let forwarder = try VsockForwarder(guestPorts: [3211, 22], connect: fakeConnect(agent: echoAgent))
        defer { forwarder.stop() }
        let seen = DispatchSemaphore(value: 0)
        forwarder.start(interval: 0.01) { seen.signal() }
        #expect(seen.wait(timeout: .now() + 5) == .success)
        let session = forwarder.session(agent: true)
        #expect(session.agent)
        #expect(Swift.Set(session.forwards.keys) == ["3211", "22"])
        #expect(session.pid == getpid())
    }

    @Test("Replies and requests use the microVM init's protocol")
    func protocolLines() {
        #expect(VsockForwarder.isOK("{\"ok\":true}"))
        #expect(!VsockForwarder.isOK("{\"ok\":false,\"error\":\"refused\"}"))
        #expect(!VsockForwarder.isOK("garbage"))
        #expect(
            String(decoding: VsockForwarder.forwardRequest(3211), as: UTF8.self)
                == "{\"op\":\"forward\",\"port\":3211}\n")
    }

    @Test("A resume keeps the devices its state was saved with")
    func socketDeviceFollowsTheSavedState() {
        #expect(VM.usesSocketDevice(forwards: [3211], hasSavedState: false, savedStateHasVsock: false))
        #expect(VM.usesSocketDevice(forwards: [3211], hasSavedState: true, savedStateHasVsock: true))
        #expect(!VM.usesSocketDevice(forwards: [3211], hasSavedState: true, savedStateHasVsock: false))
        #expect(!VM.usesSocketDevice(forwards: [], hasSavedState: false, savedStateHasVsock: false))
    }

    @Test("The session and the state marker live in the VM directory")
    func directoryRecords() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let dir = VMDirectory(Path(root.path))
        #expect(dir.loadVsockSession() == nil)
        let session = VsockSession(forwards: ["3211": 50123], agent: true, pid: 42)
        dir.saveVsockSession(session)
        #expect(dir.loadVsockSession() == session)
        dir.clearVsockSession()
        #expect(dir.loadVsockSession() == nil)

        dir.markSavedStateVsock(true)
        #expect(!dir.savedStateHasVsock(), "no state, no marker that counts")
        FileManager.default.createFile(atPath: dir.statePath.path, contents: Data())
        #expect(dir.savedStateHasVsock())
        dir.discardSavedState()
        #expect(!dir.savedStateHasVsock())
        #expect(!dir.stateVsockPath.exists())
    }
}
