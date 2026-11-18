import Darwin
import Foundation

/// What `vsock.json` in a running VM's directory records: the host
/// loopback ports a run with `forwards` publishes, and whether the guest's
/// agent has answered on them yet. `get` and `list` (in any process) report
/// it, so a client knows to dial `127.0.0.1:<host>` instead of the VM's
/// address.
struct VsockSession: Codable, Equatable {
    /// Guest port (as a string, a JSON object key) to host loopback port.
    var forwards: [String: UInt16]
    /// The guest agent answered a ping: the forwards reach the guest.
    var agent: Bool
    /// The process that serves the forwards.
    var pid: Int32
}

/// Host loopback ports carried to a guest over its virtio socket device.
///
/// A macOS VM sits on Apple's shared VM network, where a host app's connect
/// to it is subject to macOS Local Network privacy and to a VPN's routes. A
/// vsock stream between this process (which owns the VM) and the guest
/// touches neither. For every forward this binds `127.0.0.1:0`; a
/// connection there opens a vsock stream to the guest agent
/// (`agentPort`), asks it to connect to the guest's own
/// `127.0.0.1:<guest port>` (`{"op":"forward","port":N}`, the protocol of
/// Cua's microVM init), and carries bytes both ways. A guest without the
/// agent answers nothing, so the connection closes at once and the client
/// keeps using the VM's address.
final class VsockForwarder: @unchecked Sendable {
    /// The vsock port the guest agent listens on.
    static let agentPort: UInt32 = 1024

    /// One vsock stream to the guest: its descriptor, and how to close it.
    struct Stream {
        let fd: Int32
        let close: @Sendable () -> Void
    }

    /// Opens a vsock stream to a guest port, or `nil`.
    typealias Connect = @Sendable (UInt32) -> Stream?

    /// Guest port to host loopback port.
    let forwards: [UInt16: UInt16]
    /// Each listener and the guest port it carries.
    private let listeners: [(fd: Int32, guest: UInt16)]
    private let connect: Connect
    private let lock = NSLock()
    private var stopped = false
    private var started = false

    /// Binds a loopback port for each of `guestPorts` (none is served yet).
    init(guestPorts: [UInt16], connect: @escaping Connect) throws {
        var forwards: [UInt16: UInt16] = [:]
        var listeners: [(fd: Int32, guest: UInt16)] = []
        do {
            for guest in Swift.Set(guestPorts) {
                let (fd, port) = try Self.listenLoopback()
                listeners.append((fd, guest))
                forwards[guest] = port
            }
        } catch {
            for l in listeners { Darwin.close(l.fd) }
            throw error
        }
        self.forwards = forwards
        self.listeners = listeners
        self.connect = connect
    }

    deinit {
        // Never served: nothing else closes the listeners.
        if !started {
            for l in listeners { Darwin.close(l.fd) }
        }
    }

    /// The session record for `agent`.
    func session(agent: Bool) -> VsockSession {
        VsockSession(
            forwards: Dictionary(
                uniqueKeysWithValues: forwards.map { (String($0.key), $0.value) }),
            agent: agent,
            pid: getpid())
    }

    /// Serves the forwards, and calls `onAgent` once the guest agent first
    /// answers a ping (checked every `interval` until `stop`).
    func start(interval: TimeInterval = 0.25, onAgent: @escaping @Sendable () -> Void) {
        lock.lock()
        started = true
        lock.unlock()
        for (fd, guest) in listeners {
            Self.serve(fd, name: "vsock forward \(guest)", stopped: { [weak self] in
                self?.isStopped ?? true
            }) { [weak self] c in
                self?.serveForward(c, guest: guest)
            }
        }
        let t = Thread { [weak self] in
            while let self, !self.isStopped {
                if self.ping() {
                    onAgent()
                    return
                }
                Thread.sleep(forTimeInterval: interval)
            }
        }
        t.name = "vsock agent ping"
        t.start()
    }

    /// Stops serving: each listener closes within half a second (its own
    /// thread closes it, so no descriptor is closed under a blocked
    /// `accept`); open connections end with the VM.
    func stop() {
        lock.lock()
        stopped = true
        lock.unlock()
    }

    private var isStopped: Bool {
        lock.lock()
        defer { lock.unlock() }
        return stopped
    }

    /// Whether the guest agent answers `{"op":"ping"}`.
    func ping() -> Bool {
        guard let s = connect(Self.agentPort) else { return false }
        defer { s.close() }
        guard Self.writeAll(s.fd, Data("{\"op\":\"ping\"}\n".utf8)),
            let reply = Self.readLine(s.fd, limit: 4096)
        else { return false }
        return Self.isOK(reply)
    }

    /// One connection to a forwarded port: the agent connects to the
    /// guest's own port, then bytes flow both ways.
    func serveForward(_ c: Int32, guest: UInt16) {
        var one: Int32 = 1
        setsockopt(c, IPPROTO_TCP, TCP_NODELAY, &one, 4)
        guard let s = connect(Self.agentPort) else {
            Darwin.close(c)
            return
        }
        guard Self.writeAll(s.fd, Self.forwardRequest(guest)),
            let reply = Self.readLine(s.fd, limit: 4096), Self.isOK(reply)
        else {
            s.close()
            Darwin.close(c)
            return
        }
        Self.splice(c, s.fd) {
            s.close()
            Darwin.close(c)
        }
    }

    // MARK: - Protocol

    static func forwardRequest(_ port: UInt16) -> Data {
        Data("{\"op\":\"forward\",\"port\":\(port)}\n".utf8)
    }

    static func isOK(_ reply: String) -> Bool {
        guard let data = reply.data(using: .utf8),
            let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else { return false }
        return object["ok"] as? Bool == true
    }

    // MARK: - Sockets (blocking; a thread per connection, as in cua-vz)

    /// A listening TCP socket on `127.0.0.1:0` and the port it got.
    static func listenLoopback() throws -> (Int32, UInt16) {
        let fd = socket(AF_INET, SOCK_STREAM, 0)
        guard fd >= 0 else { throw VsockForwarderError.socket(errno) }
        var a = sockaddr_in()
        a.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
        a.sin_family = sa_family_t(AF_INET)
        a.sin_port = 0
        a.sin_addr.s_addr = inet_addr("127.0.0.1")
        let bound = withUnsafePointer(to: &a) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                bind(fd, $0, socklen_t(MemoryLayout<sockaddr_in>.size))
            }
        }
        guard bound == 0, listen(fd, 128) == 0 else {
            let e = errno
            Darwin.close(fd)
            throw VsockForwarderError.socket(e)
        }
        var got = sockaddr_in()
        var len = socklen_t(MemoryLayout<sockaddr_in>.size)
        let named = withUnsafeMutablePointer(to: &got) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                getsockname(fd, $0, &len)
            }
        }
        guard named == 0 else {
            let e = errno
            Darwin.close(fd)
            throw VsockForwarderError.socket(e)
        }
        return (fd, UInt16(bigEndian: got.sin_port))
    }

    /// Accepts connections on `fd` until `stopped`, each on a thread of its
    /// own, then closes `fd`.
    static func serve(
        _ fd: Int32, name: String, stopped: @escaping @Sendable () -> Bool,
        _ handle: @escaping @Sendable (Int32) -> Void
    ) {
        let t = Thread {
            defer { Darwin.close(fd) }
            while !stopped() {
                var p = pollfd(fd: fd, events: Int16(POLLIN), revents: 0)
                let ready = poll(&p, 1, 500)
                if ready < 0 && errno != EINTR { return }
                if ready <= 0 || stopped() { continue }
                let c = accept(fd, nil, nil)
                if c < 0 {
                    if errno == EINTR || errno == ECONNABORTED || errno == EAGAIN { continue }
                    return
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

    /// One `\n`-terminated line (without it), byte by byte so nothing past
    /// it is consumed; `nil` on EOF, error or past `limit`.
    static func readLine(_ fd: Int32, limit: Int) -> String? {
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

    @discardableResult
    static func writeAll(_ fd: Int32, _ data: Data) -> Bool {
        data.withUnsafeBytes { (buf: UnsafeRawBufferPointer) -> Bool in
            guard var p = buf.baseAddress else { return true }
            var left = buf.count
            while left > 0 {
                let n = write(fd, p, left)
                if n < 0 && errno == EINTR { continue }
                if n <= 0 { return false }
                p += n
                left -= n
            }
            return true
        }
    }

    /// Copies `a` to `b` and `b` to `a` until both directions end, passing
    /// each end-of-stream on as a half-close; then `done`.
    static func splice(_ a: Int32, _ b: Int32, done: @escaping @Sendable () -> Void) {
        let group = DispatchGroup()
        for (from, to) in [(a, b), (b, a)] {
            group.enter()
            let t = Thread {
                var buf = [UInt8](repeating: 0, count: 64 * 1024)
                while true {
                    let n = read(from, &buf, buf.count)
                    if n < 0 && errno == EINTR { continue }
                    if n <= 0 { break }
                    let ok = buf.withUnsafeBytes {
                        writeAll(to, Data(bytesNoCopy: UnsafeMutableRawPointer(mutating: $0.baseAddress!), count: n, deallocator: .none))
                    }
                    if !ok { break }
                }
                shutdown(to, SHUT_WR)
                shutdown(from, SHUT_RD)
                group.leave()
            }
            t.name = "vsock splice"
            t.start()
        }
        group.notify(queue: .global()) { done() }
    }
}

enum VsockForwarderError: Error, CustomStringConvertible {
    case socket(Int32)

    var description: String {
        switch self {
        case .socket(let e): return "loopback listener: \(String(cString: strerror(e)))"
        }
    }
}
