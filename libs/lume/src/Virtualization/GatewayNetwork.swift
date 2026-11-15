import Darwin
import Foundation
import Virtualization

/// Gateway networking (`NetworkMode.gateway`): the VM's network device is a
/// datagram socket connected to a user-mode network gateway (Cua's
/// `cua-netgw`), which carries the guest's Ethernet frames and decides what
/// the guest may reach. The gateway runs DHCP; its control socket reports
/// the address it leased to the guest.
enum GatewayNetwork {
    struct Failure: Error, CustomStringConvertible {
        let description: String
    }

    /// A file-handle attachment on a datagram socket bound beside `socket`
    /// (`lume-<pid>.sock`) and connected to it, after the vfkit greeting
    /// the gateway learns its peer from.
    static func attachment(socket path: String) throws -> VZFileHandleNetworkDeviceAttachment {
        let local = (path as NSString).deletingLastPathComponent + "/lume-\(getpid()).sock"
        unlink(local)
        let fd = Darwin.socket(AF_UNIX, SOCK_DGRAM, 0)
        guard fd >= 0 else { throw Failure(description: "gateway network: socket: \(errno)") }
        do {
            var la = try unixAddress(local)
            var ra = try unixAddress(path)
            let bound = withUnsafePointer(to: &la) {
                $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                    bind(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
                }
            }
            guard bound == 0 else { throw Failure(description: "gateway network: bind \(local): \(errno)") }
            let connected = withUnsafePointer(to: &ra) {
                $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                    connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
                }
            }
            guard connected == 0 else {
                throw Failure(description: "gateway network: connect \(path): \(errno) (is the gateway running?)")
            }
            // Room for bursts of 1500-byte frames (as vfkit sets them).
            var snd: Int32 = 1 << 20
            var rcv: Int32 = 4 << 20
            setsockopt(fd, SOL_SOCKET, SO_SNDBUF, &snd, 4)
            setsockopt(fd, SOL_SOCKET, SO_RCVBUF, &rcv, 4)
            let greeting = Array("VFKT".utf8)
            guard write(fd, greeting, greeting.count) == greeting.count else {
                throw Failure(description: "gateway network: greeting: \(errno)")
            }
        } catch {
            close(fd)
            unlink(local)
            throw error
        }
        return VZFileHandleNetworkDeviceAttachment(
            fileHandle: FileHandle(fileDescriptor: fd, closeOnDealloc: true))
    }

    /// The address the gateway's DHCP leased to `mac` and the guest uses,
    /// asked over its control socket (`lease <mac>` → `ok <address>`).
    static func leasedAddress(control path: String, mac: String) -> String? {
        let fd = Darwin.socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { return nil }
        defer { close(fd) }
        var tv = timeval(tv_sec: 2, tv_usec: 0)
        setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, socklen_t(MemoryLayout<timeval>.size))
        setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &tv, socklen_t(MemoryLayout<timeval>.size))
        guard var addr = try? unixAddress(path) else { return nil }
        let connected = withUnsafePointer(to: &addr) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        guard connected == 0 else { return nil }
        let request = Array("lease \(mac)\n".utf8)
        guard write(fd, request, request.count) == request.count else { return nil }
        var reply = [UInt8]()
        var buf = [UInt8](repeating: 0, count: 256)
        while reply.count < 256 {
            let n = read(fd, &buf, buf.count)
            if n <= 0 { break }
            reply.append(contentsOf: buf[0..<n])
            if reply.contains(UInt8(ascii: "\n")) { break }
        }
        return parseLeaseReply(String(decoding: reply, as: UTF8.self))
    }

    /// `ok <address>` → the address; anything else → nil.
    static func parseLeaseReply(_ reply: String) -> String? {
        let line = reply.trimmingCharacters(in: .whitespacesAndNewlines)
        guard line.hasPrefix("ok ") else { return nil }
        let ip = String(line.dropFirst(3)).trimmingCharacters(in: .whitespaces)
        return ip.isEmpty ? nil : ip
    }

    private static func unixAddress(_ path: String) throws -> sockaddr_un {
        var a = sockaddr_un()
        a.sun_family = sa_family_t(AF_UNIX)
        let bytes = Array(path.utf8)
        guard bytes.count < MemoryLayout.size(ofValue: a.sun_path) else {
            throw Failure(description: "gateway network: socket path too long: \(path)")
        }
        withUnsafeMutableBytes(of: &a.sun_path) { raw in
            for (i, b) in bytes.enumerated() { raw[i] = b }
        }
        return a
    }
}
