import Foundation

/// `vz.json` in the VM's directory: what the runtime asks `cua-vz` to run.
/// Paths are absolute or relative to the VM's directory (cua-vz runs there).
public struct VZConfig: Codable, Equatable {
    /// The guest kernel (an uncompressed arm64 `Image`).
    public var kernel: String
    /// The initramfs holding the guest init.
    public var initrd: String
    /// The kernel command line.
    public var cmdline: String
    public var cpus: Int
    public var memoryMB: UInt64
    /// Block devices in order (`/dev/vda`, `/dev/vdb`, ...).
    public var disks: [Disk]
    /// The network device's MAC address (a restore must use the saved one).
    public var mac: String
    /// File holding the VM's `VZGenericMachineIdentifier` (created on the
    /// first boot; a restore must use the saved one).
    public var machineID: String
    /// Directory for the sockets (`api.sock`, `v.sock`, and the network's
    /// `gv.sock`/`net.sock`): short, since a Unix socket path is at most
    /// 104 bytes.
    public var sockets: String
    /// The guest's serial console is appended here.
    public var console: String
    /// Egress through a user-mode network (gvproxy); `nil` leaves the
    /// guest's network device unconnected (no network).
    public var network: Network?
    /// Host loopback ports forwarded to guest ports over vsock.
    public var forwards: [Forward]
    /// A saved state to restore instead of booting.
    public var restore: String?

    public struct Disk: Codable, Equatable {
        public var path: String
        public var readOnly: Bool

        public init(path: String, readOnly: Bool) {
            self.path = path
            self.readOnly = readOnly
        }

        enum CodingKeys: String, CodingKey {
            case path
            case readOnly = "read_only"
        }
    }

    public struct Network: Codable, Equatable {
        /// The gvproxy binary.
        public var gvproxy: String
        /// Its configuration file (`-config`); it must listen for vfkit
        /// clients on `<sockets>/gv.sock`.
        public var config: String

        public init(gvproxy: String, config: String) {
            self.gvproxy = gvproxy
            self.config = config
        }
    }

    public struct Forward: Codable, Equatable {
        public var host: UInt16
        public var guest: UInt16

        public init(host: UInt16, guest: UInt16) {
            self.host = host
            self.guest = guest
        }
    }

    enum CodingKeys: String, CodingKey {
        case kernel, initrd, cmdline, cpus
        case memoryMB = "memory_mb"
        case disks, mac
        case machineID = "machine_id"
        case sockets, console, network, forwards, restore
    }

    public static func load(from url: URL) throws -> VZConfig {
        try JSONDecoder().decode(VZConfig.self, from: Data(contentsOf: url))
    }

    /// Problems the framework would report less clearly, if any.
    public func problems() -> [String] {
        var out: [String] = []
        if cpus < 1 { out.append("cpus must be at least 1") }
        if memoryMB < 128 { out.append("memory_mb must be at least 128") }
        if disks.isEmpty { out.append("no disks") }
        for name in ["api.sock", "v.sock", "gv.sock", "net.sock"] {
            let p = (sockets as NSString).appendingPathComponent(name)
            if p.utf8.count >= 104 {
                out.append("socket path \(p) is too long (Unix sockets take at most 103 bytes)")
                break
            }
        }
        return out
    }
}

/// The port of a vsock-proxy handshake line (`CONNECT <port>`), as
/// Firecracker's vsock Unix socket takes it.
public func parseConnect(_ line: String) -> UInt32? {
    let parts = line.trimmingCharacters(in: .whitespacesAndNewlines).split(separator: " ")
    guard parts.count == 2, parts[0] == "CONNECT" else { return nil }
    return UInt32(parts[1])
}

/// A control request on `api.sock` (one JSON line per connection).
public struct ControlRequest: Codable, Equatable {
    public var op: String
    public var path: String?

    public init(op: String, path: String? = nil) {
        self.op = op
        self.path = path
    }
}

/// The answer to a [`ControlRequest`].
public struct ControlReply: Codable, Equatable {
    public var ok: Bool
    public var error: String?
    public var state: String?
    public var ms: Int?

    public init(ok: Bool, error: String? = nil, state: String? = nil, ms: Int? = nil) {
        self.ok = ok
        self.error = error
        self.state = state
        self.ms = ms
    }

    public func line() -> Data {
        var d = (try? JSONEncoder().encode(self)) ?? Data("{\"ok\":false}".utf8)
        d.append(0x0A)
        return d
    }
}
