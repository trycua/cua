import Darwin
import Foundation
import Testing

@testable import lume

/// The guest ports a fake connector was asked for.
private final class PortLog: @unchecked Sendable {
    private let lock = NSLock()
    private var ports: [UInt32] = []

    func record(_ port: UInt32) { lock.withLock { ports.append(port) } }
    var recorded: [UInt32] { lock.withLock { ports } }
}

/// A stand-in for the guest daemon: each "vsock" connect is one end of a Unix
/// socket pair whose other end echoes what it reads until end of file.
private func echoConnect(log: PortLog) -> VsockForwarder.Connect {
    return { port in
        log.record(port)
        var fds: [Int32] = [0, 0]
        guard socketpair(AF_UNIX, SOCK_STREAM, 0, &fds) == 0 else { return nil }
        let (host, guest) = (fds[0], fds[1])
        let t = Thread {
            var buf = [UInt8](repeating: 0, count: 1024)
            while true {
                let n = read(guest, &buf, buf.count)
                if n <= 0 { break }
                _ = VsockForwarder.writeAll(guest, Data(buf[0..<n]))
            }
            shutdown(guest, SHUT_WR)
            Darwin.close(guest)
        }
        t.start()
        return VsockForwarder.Stream(fd: host, close: { Darwin.close(host) })
    }
}

/// A loopback port nothing listens on right now.
private func freeLoopbackPort() throws -> UInt16 {
    let (fd, port) = try VsockForwarder.listenLoopback()
    Darwin.close(fd)
    return port
}

private func connectLoopback(_ port: UInt16) -> Int32 {
    let fd = socket(AF_INET, SOCK_STREAM, 0)
    var timeout = timeval(tv_sec: 5, tv_usec: 0)
    setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &timeout, socklen_t(MemoryLayout<timeval>.size))
    var a = sockaddr_in()
    a.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
    a.sin_family = sa_family_t(AF_INET)
    a.sin_port = port.bigEndian
    a.sin_addr.s_addr = inet_addr("127.0.0.1")
    let rc = withUnsafePointer(to: &a) {
        $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
            Darwin.connect(fd, $0, socklen_t(MemoryLayout<sockaddr_in>.size))
        }
    }
    if rc != 0 {
        Darwin.close(fd)
        return -1
    }
    return fd
}

private func readToEnd(_ fd: Int32) -> String {
    var out = Data()
    var buf = [UInt8](repeating: 0, count: 1024)
    while true {
        let n = read(fd, &buf, buf.count)
        if n <= 0 { break }
        out.append(contentsOf: buf[0..<n])
    }
    return String(decoding: out, as: UTF8.self)
}

@Suite("VsockPortForwarder")
struct VsockPortForwarderTests {
    @Test("A rule listens on loopback unless it names an address")
    func ruleAddress() throws {
        let local = try VsockPortForwarder.Rule(argument: "8000:5005")
        #expect(local.address == "127.0.0.1")
        #expect(local.hostPort == 8000)
        #expect(local.guestPort == 5005)

        let any = try VsockPortForwarder.Rule(argument: "0.0.0.0:8000:5005")
        #expect(any.address == "0.0.0.0")
        #expect(any.hostPort == 8000)
        #expect(any.guestPort == 5005)
    }

    @Test(
        "Malformed rules are refused",
        arguments: [
            "", "8000", "0:5005", "8000:0", "70000:5005", "8000:x", "localhost:8000:5005",
            "256.0.0.1:8000:5005", "::8000:5005", "1.2.3.4:8000:5005:1",
        ])
    func malformedRule(_ argument: String) {
        #expect(throws: VsockPortForwarderError.self) {
            try VsockPortForwarder.Rule(argument: argument)
        }
    }

    @Test("A connection to the host port reaches the guest port and back")
    func forwardsBytes() throws {
        let port = try freeLoopbackPort()
        let log = PortLog()
        let forwarder = VsockPortForwarder(
            connect: echoConnect(log: log),
            rule: try VsockPortForwarder.Rule(argument: "\(port):5005"))
        try forwarder.start()
        defer { forwarder.stop() }

        let c = connectLoopback(port)
        try #require(c >= 0)
        defer { Darwin.close(c) }
        #expect(VsockForwarder.writeAll(c, Data("hello".utf8)))
        shutdown(c, SHUT_WR)
        #expect(readToEnd(c) == "hello")
        #expect(log.recorded == [5005])
    }

    @Test("A connection the guest refuses is closed")
    func refusedByGuest() throws {
        let port = try freeLoopbackPort()
        let forwarder = VsockPortForwarder(
            connect: { _ in nil },
            rule: try VsockPortForwarder.Rule(argument: "\(port):5005"))
        try forwarder.start()
        defer { forwarder.stop() }

        let c = connectLoopback(port)
        try #require(c >= 0)
        defer { Darwin.close(c) }
        #expect(readToEnd(c).isEmpty)
    }

    @Test("--vsock-forward rules add the socket device on a cold boot")
    func socketDevice() {
        #expect(
            VM.usesSocketDevice(
                forwards: [], vsockPortForwards: true, hasSavedState: false,
                savedStateHasVsock: false))
        #expect(
            !VM.usesSocketDevice(
                forwards: [], vsockPortForwards: true, hasSavedState: true,
                savedStateHasVsock: false))
        #expect(
            VM.usesSocketDevice(
                forwards: [], vsockPortForwards: true, hasSavedState: true,
                savedStateHasVsock: true))
    }
}
