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

    private let handle: BaseVirtualizationService.VirtualMachineHandle
    private let rule: Rule
    private var listenerFD: Int32 = -1
    private let queue = DispatchQueue(label: "lume.vsock.forwarder")

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
        if listenerFD >= 0 { close(listenerFD); listenerFD = -1 }
    }

    private func acceptLoop(_ listenFD: Int32) {
        while true {
            let client = accept(listenFD, nil, nil)
            if client < 0 {
                if errno == EINTR { continue }
                return
            }
            connectGuest(clientFD: client)
        }
    }

    private func connectGuest(clientFD: Int32) {
        let handle = self.handle
        let guestPort = rule.guestPort
        handle.queue.async {
            guard let device = handle.machine.socketDevices.first as? VZVirtioSocketDevice else {
                Logger.error("vsock forwarder: VM has no VZVirtioSocketDevice")
                close(clientFD)
                return
            }
            device.connect(toPort: guestPort) { result in
                switch result {
                case .success(let connection):
                    Self.splice(clientFD, connection)
                case .failure(let error):
                    Logger.error(
                        "vsock connect failed",
                        metadata: [
                            "guest_port": "\(guestPort)",
                            "error": error.localizedDescription,
                        ])
                    close(clientFD)
                }
            }
        }
    }

    private static func splice(_ clientFD: Int32, _ connection: VZVirtioSocketConnection) {
        let guestFD = connection.fileDescriptor
        let done = DispatchGroup()

        let pump: (Int32, Int32) -> Void = { from, to in
            done.enter()
            DispatchQueue.global(qos: .userInitiated).async {
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
                shutdown(to, SHUT_WR)
            }
        }
        pump(clientFD, guestFD)
        pump(guestFD, clientFD)

        done.notify(queue: DispatchQueue.global()) {
            close(clientFD)
            connection.close()
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
