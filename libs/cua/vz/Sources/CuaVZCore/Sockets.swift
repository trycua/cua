import Darwin
import Foundation

/// Small blocking-socket helpers. Every connection gets its own threads:
/// the guest init's control traffic is a few connections at a time, and a
/// published port's connections are few and long-lived.
public enum Sockets {
    public struct Failure: Error, CustomStringConvertible {
        public let description: String
        public init(_ what: String) {
            description = "\(what): \(String(cString: strerror(errno)))"
        }
    }

    static func unixAddress(_ path: String) throws -> sockaddr_un {
        var a = sockaddr_un()
        a.sun_family = sa_family_t(AF_UNIX)
        let bytes = Array(path.utf8)
        guard bytes.count < MemoryLayout.size(ofValue: a.sun_path) else {
            throw Failure("socket path \(path) is too long")
        }
        withUnsafeMutableBytes(of: &a.sun_path) { buf in
            for (i, c) in bytes.enumerated() { buf[i] = c }
        }
        return a
    }

    /// A listening Unix stream socket at `path` (replacing a stale one).
    public static func listenUnix(_ path: String) throws -> Int32 {
        unlink(path)
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { throw Failure("socket") }
        var a = try unixAddress(path)
        let rc = withUnsafePointer(to: &a) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                bind(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        guard rc == 0, listen(fd, 64) == 0 else {
            let e = Failure("bind \(path)")
            close(fd)
            throw e
        }
        chmod(path, 0o600)
        return fd
    }

    /// A Unix datagram socket bound to `local` and connected to `remote`.
    public static func datagram(local: String, remote: String) throws -> Int32 {
        unlink(local)
        let fd = socket(AF_UNIX, SOCK_DGRAM, 0)
        guard fd >= 0 else { throw Failure("socket") }
        var la = try unixAddress(local)
        var ra = try unixAddress(remote)
        let bound = withUnsafePointer(to: &la) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                bind(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        let connected = withUnsafePointer(to: &ra) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        guard bound == 0, connected == 0 else {
            let e = Failure("datagram \(local) -> \(remote)")
            close(fd)
            throw e
        }
        // Room for bursts of 1500-byte frames (as vfkit sets them).
        var snd: Int32 = 1 << 20
        var rcv: Int32 = 4 << 20
        setsockopt(fd, SOL_SOCKET, SO_SNDBUF, &snd, 4)
        setsockopt(fd, SOL_SOCKET, SO_RCVBUF, &rcv, 4)
        return fd
    }

    /// A listening TCP socket on `127.0.0.1:port`.
    public static func listenLoopback(_ port: UInt16) throws -> Int32 {
        let fd = socket(AF_INET, SOCK_STREAM, 0)
        guard fd >= 0 else { throw Failure("socket") }
        var one: Int32 = 1
        setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &one, 4)
        var a = sockaddr_in()
        a.sin_family = sa_family_t(AF_INET)
        a.sin_port = port.bigEndian
        a.sin_addr.s_addr = inet_addr("127.0.0.1")
        let rc = withUnsafePointer(to: &a) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                bind(fd, $0, socklen_t(MemoryLayout<sockaddr_in>.size))
            }
        }
        guard rc == 0, listen(fd, 128) == 0 else {
            let e = Failure("bind 127.0.0.1:\(port)")
            close(fd)
            throw e
        }
        return fd
    }

    /// Accepts connections on `fd` forever, handing each to `handle` on a
    /// thread of its own.
    public static func serve(_ fd: Int32, name: String, _ handle: @escaping (Int32) -> Void) {
        let t = Thread {
            while true {
                let c = accept(fd, nil, nil)
                if c < 0 {
                    if errno == EINTR || errno == ECONNABORTED { continue }
                    if errno == EBADF || errno == EINVAL { return }
                    usleep(10_000)
                    continue
                }
                var one: Int32 = 1
                setsockopt(c, SOL_SOCKET, SO_NOSIGPIPE, &one, 4)
                let worker = Thread { handle(c) }
                worker.name = "\(name) connection"
                worker.start()
            }
        }
        t.name = name
        t.start()
    }

    /// Reads one `\n`-terminated line (without it), byte by byte so nothing
    /// past it is consumed; `nil` on EOF or error, or past `limit` bytes.
    public static func readLine(_ fd: Int32, limit: Int = 64 * 1024) -> String? {
        var bytes: [UInt8] = []
        var b: UInt8 = 0
        while bytes.count < limit {
            let n = read(fd, &b, 1)
            if n < 0 && errno == EINTR { continue }
            if n <= 0 { return nil }
            if b == 0x0A { return String(decoding: bytes, as: UTF8.self) }
            bytes.append(b)
        }
        return nil
    }

    /// Writes all of `data`.
    @discardableResult
    public static func writeAll(_ fd: Int32, _ data: Data) -> Bool {
        data.withUnsafeBytes { (buf: UnsafeRawBufferPointer) -> Bool in
            guard let p = buf.baseAddress else { return true }
            return writeAll(fd, p, buf.count)
        }
    }

    /// Writes `count` bytes at `p`.
    public static func writeAll(_ fd: Int32, _ p: UnsafeRawPointer, _ count: Int) -> Bool {
        var p = p
        var left = count
        while left > 0 {
            let n = write(fd, p, left)
            if n < 0 && errno == EINTR { continue }
            if n <= 0 { return false }
            p += n
            left -= n
        }
        return true
    }

    /// Copies `a` to `b` and `b` to `a` until both directions end, passing
    /// each end-of-stream on as a half-close; then `done` (which closes
    /// what the caller owns). Returns at once.
    public static func splice(_ a: Int32, _ b: Int32, done: @escaping () -> Void) {
        let group = DispatchGroup()
        for (from, to) in [(a, b), (b, a)] {
            group.enter()
            let t = Thread {
                var buf = [UInt8](repeating: 0, count: 64 * 1024)
                while true {
                    let n = read(from, &buf, buf.count)
                    if n < 0 && errno == EINTR { continue }
                    if n <= 0 { break }
                    let ok = buf.withUnsafeBytes { writeAll(to, $0.baseAddress!, n) }
                    if !ok { break }
                }
                shutdown(to, SHUT_WR)
                // A peer that stopped reading: unblock the other direction.
                shutdown(from, SHUT_RD)
                group.leave()
            }
            t.name = "splice"
            t.start()
        }
        group.notify(queue: .global()) { done() }
    }
}
