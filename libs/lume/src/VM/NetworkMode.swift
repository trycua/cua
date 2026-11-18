import ArgumentParser
import Foundation

/// Represents the networking mode for a virtual machine.
///
/// Supports NAT (default), bridged and gateway networking modes.
/// Bridged networking allows the VM to get its own IP on the LAN via DHCP,
/// bypassing the host's routing table entirely. Gateway networking connects
/// the VM to a user-mode network gateway (Cua's `cua-netgw`) through a
/// datagram socket, so the gateway decides what the guest may reach.
///
/// Usage:
///   - `"nat"` → `.nat`
///   - `"bridged"` → `.bridged(interface: nil)` (auto-select first available)
///   - `"bridged:en0"` → `.bridged(interface: "en0")` (specific interface)
///   - `"gateway:/path/net.sock"` → `.gateway(socket: "/path/net.sock")`: the
///     gateway's datagram socket (vfkit protocol). Its control socket,
///     which reports the guest's address, is `control.sock` in the same
///     folder.
enum NetworkMode: Equatable, CustomStringConvertible {
    case nat
    case bridged(interface: String?)
    case gateway(socket: String)

    /// Parse a network mode string.
    ///
    /// Accepted formats:
    ///   - `"nat"` → NAT mode (default)
    ///   - `"bridged"` → Bridged mode with auto-detected interface
    ///   - `"bridged:<interface>"` → Bridged mode on a specific interface (e.g. `"bridged:en0"`)
    ///
    /// - Parameter string: The string to parse.
    /// - Returns: A `NetworkMode` value, or `nil` if the string is invalid.
    static func parse(_ string: String) -> NetworkMode? {
        let lowercased = string.lowercased()

        if lowercased == "nat" {
            return .nat
        }

        if lowercased == "bridged" {
            return .bridged(interface: nil)
        }

        if lowercased.hasPrefix("gateway:") {
            let socket = String(string.dropFirst("gateway:".count))
            // An absolute socket path is required: the VM runs in another
            // process's working directory.
            guard socket.hasPrefix("/") else { return nil }
            return .gateway(socket: socket)
        }

        if lowercased.hasPrefix("bridged:") {
            let interface = String(string.dropFirst("bridged:".count))
            guard !interface.isEmpty else {
                return .bridged(interface: nil)
            }
            return .bridged(interface: interface)
        }

        return nil
    }

    var description: String {
        switch self {
        case .nat:
            return "nat"
        case .bridged(let interface):
            if let interface = interface {
                return "bridged:\(interface)"
            }
            return "bridged"
        case .gateway(let socket):
            return "gateway:\(socket)"
        }
    }

    /// Refuses a mode that only makes sense for one run: a gateway's socket
    /// lives as long as that run's gateway, so it is never stored with a VM.
    func checkStorable() throws {
        if case .gateway = self {
            throw ValidationError(
                "gateway networking is chosen per run (lume run --network gateway:<socket path>), not stored with the VM"
            )
        }
    }

    /// The gateway's control socket (`control.sock` beside its network
    /// socket), for gateway mode.
    var gatewayControlSocket: String? {
        guard case .gateway(let socket) = self else { return nil }
        return (socket as NSString).deletingLastPathComponent + "/control.sock"
    }

    /// The string representation used for config persistence.
    var configString: String {
        return description
    }
}

// MARK: - Codable

extension NetworkMode: Codable {
    init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        let string = try container.decode(String.self)
        guard let mode = NetworkMode.parse(string) else {
            throw DecodingError.dataCorruptedError(
                in: container,
                debugDescription: "Invalid network mode: \(string). Expected 'nat', 'bridged', 'bridged:<interface>' or 'gateway:<socket path>'."
            )
        }
        self = mode
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        try container.encode(configString)
    }
}
