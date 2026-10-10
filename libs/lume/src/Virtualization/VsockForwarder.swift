import Foundation
import Virtualization

final class VsockForwarder: @unchecked Sendable {
    struct Rule {
        let hostPort: UInt16
        let guestPort: UInt32

        init(argument: String) throws {
            let parts = argument.split(separator: ":", maxSplits: 1).map(String.init)
            guard parts.count == 2,
                let h = UInt16(parts[0]), let g = UInt32(parts[1]), h > 0, g > 0
            else {
                throw VsockForwarderError.invalidRule(argument)
            }
            hostPort = h
            guestPort = g
        }
    }

    static let maxConnections = 64

    private let handle: BaseVirtualizationService.VirtualMachineHandle
    private let rule: Rule
    private var listenerFD: Int32 = -1
    private let queue = DispatchQueue(label: "lume.vsock.forwarder")
    private let lock = NSLock()
    private var active = 0
    private var stopped = false

    init(handle: BaseVirtualizationService.VirtualMachineHandle, rule: Rule) {
        self.handle = handle
        self.rule = rule
    }

    func start() throws {
        let fd = socket(AF_INET, SOCK_STREAM, 0)
        guard fd >= 0 else { throw VsockForwarderError.listenFailed(errno) }
        var yes: Int32 = 1
        setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &yes, socklen_t(MemoryLayout<Int32>.size))

        var addr = sockaddr_in()
        addr.sin_family = sa_family_t(AF_INET)
        addr.sin_port = rule.hostPort.bigEndian
        addr.sin_addr.s_addr = INADDR_ANY.bigEndian
        let bound = withUnsafePointer(to: &addr) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                bind(fd, $0, socklen_t(MemoryLayout<sockaddr_in>.size))
            }
        }
        guard bound == 0, listen(fd, 16) == 0 else {
            let e = errno
            close(fd)
            throw VsockForwarderError.listenFailed(e)
        }
        listenerFD = fd
        Logger.info(
            "vsock forwarder listening",
            metadata: ["host_port": "\(rule.hostPort)", "guest_port": "\(rule.guestPort)"])

        queue.async { [weak self] in self?.acceptLoop(fd) }
    }

    func stop() {
        lock.lock()
        stopped = true
        lock.unlock()
        if listenerFD >= 0 { close(listenerFD); listenerFD = -1 }
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
        while !isStopped {
            var peer = sockaddr_in()
            var len = socklen_t(MemoryLayout<sockaddr_in>.size)
            let client = withUnsafeMutablePointer(to: &peer) {
                $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { accept(listenFD, $0, &len) }
            }
            if client < 0 {
                let e = errno
                switch e {
                case EINTR, ECONNABORTED:
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
                close(client)
                continue
            }
            Self.configure(client)
            Logger.debug("vsock forwarder accepted a connection", metadata: ["client": address])
            connectGuest(clientFD: client)
        }
    }

    private func connectGuest(clientFD: Int32) {
        let handle = self.handle
        let guestPort = rule.guestPort
        let release: () -> Void = { [weak self] in self?.releaseSlot() }
        handle.queue.async {
            guard let device = handle.machine.socketDevices.first as? VZVirtioSocketDevice else {
                Logger.error("vsock forwarder: VM has no VZVirtioSocketDevice")
                close(clientFD)
                release()
                return
            }
            device.connect(toPort: guestPort) { result in
                switch result {
                case .success(let connection):
                    Self.splice(clientFD, connection, onDone: release)
                case .failure(let error):
                    Logger.error(
                        "vsock connect failed",
                        metadata: [
                            "guest_port": "\(guestPort)",
                            "error": error.localizedDescription,
                        ])
                    close(clientFD)
                    release()
                }
            }
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
        return "\(String(cString: buf)):\(UInt16(bigEndian: peer.sin_port))"
    }

    private static func splice(
        _ clientFD: Int32, _ connection: VZVirtioSocketConnection, onDone: @escaping () -> Void
    ) {
        let guestFD = connection.fileDescriptor
        setOption(guestFD, SOL_SOCKET, SO_NOSIGPIPE, 1)
        let done = DispatchGroup()

        let pump: (Int32, Int32, Int32, Int32) -> Void = { from, to, endFD, how in
            done.enter()
            Thread {
                defer { done.leave() }
                var buf = [UInt8](repeating: 0, count: 64 * 1024)
                while true {
                    let n = buf.withUnsafeMutableBytes { read(from, $0.baseAddress, $0.count) }
                    if n <= 0 { break }
                    var off = 0
                    while off < n {
                        let w = buf.withUnsafeBytes {
                            write(to, $0.baseAddress!.advanced(by: off), n - off)
                        }
                        if w <= 0 { break }
                        off += w
                    }
                    if off < n { break }
                }
                shutdown(endFD, how)
            }.start()
        }
        pump(clientFD, guestFD, guestFD, SHUT_WR)
        pump(guestFD, clientFD, clientFD, SHUT_RDWR)

        done.notify(queue: DispatchQueue.global()) {
            close(clientFD)
            connection.close()
            onDone()
        }
    }
}

enum VsockForwarderError: CustomNSError, LocalizedError {
    case invalidRule(String)
    case listenFailed(Int32)

    var errorDescription: String? {
        switch self {
        case .invalidRule(let s):
            return "Invalid vsock forward '\(s)' — expected <hostPort>:<guestPort>, e.g. 8000:5005"
        case .listenFailed(let e):
            return "vsock forwarder could not listen: \(String(cString: strerror(e)))"
        }
    }

    static var errorDomain: String { "VsockForwarderError" }
    var errorCode: Int {
        switch self {
        case .invalidRule: return 1
        case .listenFailed: return 2
        }
    }
}
