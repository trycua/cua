import Darwin
import Foundation

/// A host TCP port carried straight to a guest AF_VSOCK port (`--vsock-forward`).
///
/// Unlike [`VsockForwarder`], which reaches the guest's own loopback ports
/// through the guest agent, this connects to a vsock port that a guest daemon
/// listens on itself, so the guest needs no agent and no networking. Streams
/// are opened through the virtualization service's `vsockConnector()`.
final class VsockPortForwarder: @unchecked Sendable {
    /// `[<IPv4 address>:]<hostPort>:<guestPort>`. The host port listens on
    /// loopback unless an address is given.
    struct Rule: Equatable {
        static let defaultAddress = "127.0.0.1"

        let address: String
        let hostPort: UInt16
        let guestPort: UInt32

        init(argument: String) throws {
            let parts = argument.split(separator: ":", omittingEmptySubsequences: false)
                .map(String.init)
            guard parts.count == 2 || parts.count == 3 else {
                throw VsockPortForwarderError.invalidRule(argument)
            }
            let address = parts.count == 3 ? parts[0] : Self.defaultAddress
            guard Self.isIPv4(address),
                let h = UInt16(parts[parts.count - 2]), let g = UInt32(parts[parts.count - 1]),
                h > 0, g > 0
            else {
                throw VsockPortForwarderError.invalidRule(argument)
            }
            self.address = address
            hostPort = h
            guestPort = g
        }

        static func isIPv4(_ address: String) -> Bool {
            var parsed = in_addr()
            return inet_pton(AF_INET, address, &parsed) == 1
        }
    }

    static let maxConnections = 64

    private let connect: VsockForwarder.Connect
    private let rule: Rule
    private let lock = NSLock()
    private var active = 0
    private var stopped = false

    init(connect: @escaping VsockForwarder.Connect, rule: Rule) {
        self.connect = connect
        self.rule = rule
    }

    func start() throws {
        let fd = socket(AF_INET, SOCK_STREAM, 0)
        guard fd >= 0 else { throw VsockPortForwarderError.listenFailed(errno) }
        Self.setOption(fd, SOL_SOCKET, SO_REUSEADDR, 1)

        var addr = sockaddr_in()
        addr.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
        addr.sin_family = sa_family_t(AF_INET)
        addr.sin_port = rule.hostPort.bigEndian
        inet_pton(AF_INET, rule.address, &addr.sin_addr)
        let bound = withUnsafePointer(to: &addr) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                bind(fd, $0, socklen_t(MemoryLayout<sockaddr_in>.size))
            }
        }
        guard bound == 0, listen(fd, 16) == 0 else {
            let e = errno
            Darwin.close(fd)
            throw VsockPortForwarderError.listenFailed(e)
        }
        Logger.info(
            "vsock forwarder listening",
            metadata: [
                "address": rule.address, "host_port": "\(rule.hostPort)",
                "guest_port": "\(rule.guestPort)",
            ])

        let t = Thread { [weak self] in
            guard let self else {
                Darwin.close(fd)
                return
            }
            self.acceptLoop(fd)
        }
        t.name = "vsock forward \(rule.hostPort)"
        t.start()
    }

    /// Stops accepting: the listener closes within half a second, on its own
    /// thread, so no descriptor is closed under a blocked `accept`. Open
    /// connections end with the VM.
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

    private func reserveSlot() -> Bool {
        lock.lock()
        defer { lock.unlock() }
        guard active < Self.maxConnections else { return false }
        active += 1
        return true
    }

    private func releaseSlot() {
        lock.lock()
        active -= 1
        lock.unlock()
    }

    private func acceptLoop(_ listenFD: Int32) {
        defer { Darwin.close(listenFD) }
        while !isStopped {
            var p = pollfd(fd: listenFD, events: Int16(POLLIN), revents: 0)
            let ready = poll(&p, 1, 500)
            if ready < 0 && errno != EINTR { return }
            if ready <= 0 || isStopped { continue }

            var peer = sockaddr_in()
            var len = socklen_t(MemoryLayout<sockaddr_in>.size)
            let client = withUnsafeMutablePointer(to: &peer) {
                $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { accept(listenFD, $0, &len) }
            }
            if client < 0 {
                let e = errno
                switch e {
                case EINTR, ECONNABORTED, EAGAIN:
                    continue
                case EBADF, EINVAL, ENOTSOCK:
                    return
                default:
                    Logger.error(
                        "vsock forwarder accept failed",
                        metadata: ["error": String(cString: strerror(e))])
                    usleep(100_000)
                    continue
                }
            }

            let address = Self.describe(peer)
            guard reserveSlot() else {
                Logger.info(
                    "vsock forwarder refused a connection over its limit",
                    metadata: ["client": address, "limit": "\(Self.maxConnections)"])
                Darwin.close(client)
                continue
            }
            Self.configure(client)
            Logger.debug("vsock forwarder accepted a connection", metadata: ["client": address])
            let worker = Thread { [weak self] in
                guard let self else {
                    Darwin.close(client)
                    return
                }
                self.forward(client)
            }
            worker.name = "vsock forward \(rule.hostPort) connection"
            worker.start()
        }
    }

    /// Opens a vsock stream to the guest port and carries bytes both ways.
    private func forward(_ clientFD: Int32) {
        guard let stream = connect(rule.guestPort) else {
            Logger.error("vsock connect failed", metadata: ["guest_port": "\(rule.guestPort)"])
            Darwin.close(clientFD)
            releaseSlot()
            return
        }
        Self.setOption(stream.fd, SOL_SOCKET, SO_NOSIGPIPE, 1)
        VsockForwarder.splice(clientFD, stream.fd) { [weak self] in
            stream.close()
            Darwin.close(clientFD)
            self?.releaseSlot()
        }
    }

    private static func setOption(_ fd: Int32, _ level: Int32, _ name: Int32, _ value: Int32) {
        var v = value
        setsockopt(fd, level, name, &v, socklen_t(MemoryLayout<Int32>.size))
    }

    private static func configure(_ fd: Int32) {
        setOption(fd, SOL_SOCKET, SO_NOSIGPIPE, 1)
        setOption(fd, SOL_SOCKET, SO_KEEPALIVE, 1)
        setOption(fd, IPPROTO_TCP, TCP_KEEPALIVE, 60)
        setOption(fd, IPPROTO_TCP, TCP_KEEPINTVL, 15)
        setOption(fd, IPPROTO_TCP, TCP_KEEPCNT, 4)
    }

    private static func describe(_ peer: sockaddr_in) -> String {
        var addr = peer.sin_addr
        var buf = [CChar](repeating: 0, count: Int(INET_ADDRSTRLEN))
        guard inet_ntop(AF_INET, &addr, &buf, socklen_t(INET_ADDRSTRLEN)) != nil else { return "unknown" }
        let host = buf.withUnsafeBufferPointer { String(cString: $0.baseAddress!) }
        return "\(host):\(UInt16(bigEndian: peer.sin_port))"
    }
}

enum VsockPortForwarderError: CustomNSError, LocalizedError {
    case invalidRule(String)
    case listenFailed(Int32)

    var errorDescription: String? {
        switch self {
        case .invalidRule(let s):
            return
                "Invalid vsock forward '\(s)' — expected [<IPv4 address>:]<hostPort>:<guestPort>, e.g. 8000:5005 or 0.0.0.0:8000:5005"
        case .listenFailed(let e):
            return "vsock forwarder could not listen: \(String(cString: strerror(e)))"
        }
    }

    static var errorDomain: String { "VsockPortForwarderError" }
    var errorCode: Int {
        switch self {
        case .invalidRule: return 1
        case .listenFailed: return 2
        }
    }
}
