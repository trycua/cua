import Darwin
import Foundation
import Testing

@testable import CuaVZCore

/// The shape the Rust runtime writes (cua-vmm microvm/vz.rs).
let sample = """
    {"kernel":"/k/vmlinux","initrd":"/k/init.cpio","cmdline":"console=hvc0",
     "cpus":2,"memory_mb":2048,
     "disks":[{"path":"base.sqfs","read_only":true},{"path":"scratch.ext4","read_only":false}],
     "mac":"06:00:ac:10:00:02","machine_id":"machine-id","sockets":"/tmp/s",
     "console":"console.log",
     "network":{"gateway":"/bin/cua-netgw","config":"netgw.json"},
     "forwards":[{"host":50000,"guest":3211}],"restore":"snap/vzstate"}
    """

@Test func theRuntimesConfigDecodes() throws {
    let c = try JSONDecoder().decode(VZConfig.self, from: Data(sample.utf8))
    #expect(c.memoryMB == 2048)
    #expect(
        c.disks == [.init(path: "base.sqfs", readOnly: true), .init(path: "scratch.ext4", readOnly: false)])
    #expect(c.network == .init(gateway: "/bin/cua-netgw", config: "netgw.json"))
    #expect(c.forwards == [.init(host: 50000, guest: 3211)])
    #expect(c.restore == "snap/vzstate")
    #expect(c.problems() == [])
    // No network and no restore are optional.
    var j = try #require(try JSONSerialization.jsonObject(with: Data(sample.utf8)) as? [String: Any])
    j["network"] = nil
    j["restore"] = nil
    let bare = try JSONDecoder().decode(VZConfig.self, from: JSONSerialization.data(withJSONObject: j))
    #expect(bare.network == nil)
    #expect(bare.restore == nil)
}

@Test func problemsAreReportedBeforeTheFrameworkSeesThem() throws {
    var c = try JSONDecoder().decode(VZConfig.self, from: Data(sample.utf8))
    c.sockets = "/" + String(repeating: "x", count: 100)
    #expect(c.problems().first?.contains("too long") == true, "\(c.problems())")
    c.sockets = "/tmp/s"
    c.cpus = 0
    c.disks = []
    #expect(c.problems().count == 2)
}

@Test func connectLines() {
    #expect(parseConnect("CONNECT 1024\n") == 1024)
    #expect(parseConnect("CONNECT 52") == 52)
    #expect(parseConnect("CONNECT") == nil)
    #expect(parseConnect("connect 1024") == nil)
    #expect(parseConnect("CONNECT x") == nil)
    #expect(parseConnect("CONNECT 1 2") == nil)
}

@Test func controlLinesAreOneJsonLine() throws {
    let line = ControlReply(ok: true, state: "paused", ms: 12).line()
    #expect(line.last == 0x0A)
    let back = try JSONDecoder().decode(ControlReply.self, from: line.dropLast())
    #expect(back == ControlReply(ok: true, state: "paused", ms: 12))
    let req = try JSONDecoder().decode(ControlRequest.self, from: Data(#"{"op":"save","path":"/x"}"#.utf8))
    #expect(req == ControlRequest(op: "save", path: "/x"))
}

@Test func readLineStopsAtTheNewline() {
    var fds: [Int32] = [0, 0]
    #expect(socketpair(AF_UNIX, SOCK_STREAM, 0, &fds) == 0)
    defer {
        close(fds[0])
        close(fds[1])
    }
    #expect(Sockets.writeAll(fds[1], Data("CONNECT 1024\nrest".utf8)))
    #expect(Sockets.readLine(fds[0]) == "CONNECT 1024")
    var buf = [UInt8](repeating: 0, count: 4)
    #expect(read(fds[0], &buf, 4) == 4)
    #expect(String(decoding: buf, as: UTF8.self) == "rest", "nothing past the line was consumed")
    shutdown(fds[1], SHUT_WR)
    #expect(Sockets.readLine(fds[0]) == nil, "EOF")
}

func readAll(_ fd: Int32) -> Data {
    var out = Data()
    var buf = [UInt8](repeating: 0, count: 8192)
    while true {
        let n = read(fd, &buf, buf.count)
        if n <= 0 { return out }
        out.append(contentsOf: buf[0..<n])
    }
}

@Test func spliceCarriesBothWaysAndHalfCloses() async {
    var a: [Int32] = [0, 0]
    var b: [Int32] = [0, 0]
    #expect(socketpair(AF_UNIX, SOCK_STREAM, 0, &a) == 0)
    #expect(socketpair(AF_UNIX, SOCK_STREAM, 0, &b) == 0)
    let (a0, a1, b0, b1) = (a[0], a[1], b[0], b[1])
    let ended = DispatchSemaphore(value: 0)
    Sockets.splice(a1, b1) {
        close(a1)
        close(b1)
        ended.signal()
    }
    let big = Data((0..<300_000).map { UInt8($0 % 251) })
    Thread {
        Sockets.writeAll(a0, big)
        shutdown(a0, SHUT_WR)
    }.start()
    #expect(readAll(b0) == big, "a to b, then EOF")
    #expect(Sockets.writeAll(b0, Data("pong".utf8)))
    shutdown(b0, SHUT_WR)
    #expect(String(decoding: readAll(a0), as: UTF8.self) == "pong")
    #expect(ended.wait(timeout: .now() + 5) == .success, "both directions ended")
    close(a0)
    close(b0)
}

@Test func loopbackListenersRefuseATakenPort() throws {
    let fd = try Sockets.listenLoopback(0)
    defer { close(fd) }
    var addr = sockaddr_in()
    var len = socklen_t(MemoryLayout<sockaddr_in>.size)
    _ = withUnsafeMutablePointer(to: &addr) {
        $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { getsockname(fd, $0, &len) }
    }
    let port = UInt16(bigEndian: addr.sin_port)
    #expect(throws: (any Error).self) { try Sockets.listenLoopback(port) }
}
