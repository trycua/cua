import Darwin
import Foundation
import Virtualization

/// Serves the host's wall-clock time to the guest over virtio-vsock
/// (`lume run --time-vsock <port>`).
///
/// Some guests have no real-time clock they can read at boot. With this
/// service on, the guest connects to the host (CID 2) on the chosen vsock
/// port and receives one line,
///
///     time-v1 <seconds> <nanoseconds>\n
///
/// the host's UTC time (seconds since the Unix epoch and the nanoseconds
/// within that second, both decimal) read when the connection arrives.
/// The host then closes the connection. The guest sends nothing, so it has
/// no way to change the host's clock, and each connection costs the host
/// one small non-blocking send.
final class GuestTimeService: NSObject, VZVirtioSocketListenerDelegate {
    /// The first word of every reply; a later format would use a new word.
    static let formatVersion = "time-v1"

    /// Ports the service can listen on: vsock ports are 32-bit, and 0 and
    /// 0xFFFFFFFF (VMADDR_PORT_ANY) are not usable as a fixed port.
    static let validPorts: ClosedRange<UInt32> = 1...(UInt32.max - 1)

    static func isValidPort(_ port: UInt32) -> Bool {
        validPorts.contains(port)
    }

    /// The reply for the instant `time`.
    static func reply(for time: timespec) -> String {
        "\(formatVersion) \(time.tv_sec) \(time.tv_nsec)\n"
    }

    /// The host's current UTC time.
    static func currentTime() -> timespec {
        var now = timespec()
        clock_gettime(CLOCK_REALTIME, &now)
        return now
    }

    /// Writes one reply to the socket `fd` and returns whether all of it
    /// was written. The send never blocks and never raises SIGPIPE: a guest
    /// that closed its end, or stopped reading, just gets nothing. The
    /// socket is made non-blocking (MSG_DONTWAIT alone does not stop a Unix
    /// socket send from blocking), and each send passes MSG_NOSIGNAL
    /// (SO_NOSIGPIPE cannot be set once the peer has already closed).
    @discardableResult
    static func serve(fd: Int32, now: () -> timespec = currentTime) -> Bool {
        let flags = fcntl(fd, F_GETFL)
        guard flags >= 0, fcntl(fd, F_SETFL, flags | O_NONBLOCK) == 0 else { return false }
        let bytes = Array(reply(for: now()).utf8)
        var sent = 0
        while sent < bytes.count {
            let n = bytes.withUnsafeBytes { buf in
                send(fd, buf.baseAddress! + sent, bytes.count - sent, MSG_NOSIGNAL)
            }
            if n < 0 && errno == EINTR { continue }
            if n <= 0 { return false }
            sent += n
        }
        return true
    }

    private let port: UInt32

    init(port: UInt32) {
        self.port = port
    }

    func listener(
        _ listener: VZVirtioSocketListener,
        shouldAcceptNewConnection connection: VZVirtioSocketConnection,
        from socketDevice: VZVirtioSocketDevice
    ) -> Bool {
        let served = Self.serve(fd: connection.fileDescriptor)
        connection.close()
        if !served {
            Logger.debug(
                "Guest time request closed before the reply was written",
                metadata: ["port": "\(port)", "guestPort": "\(connection.sourcePort)"])
        }
        return true
    }
}
