import Foundation

/// Represents the networking mode for a virtual machine.
///
/// Supports NAT (default) and bridged networking modes.
/// Bridged networking allows the VM to get its own IP on the LAN via DHCP,
/// bypassing the host's routing table entirely.
///
/// Usage:
///   - `"nat"` → `.nat`
///   - `"bridged"` → `.bridged(interface: nil)` (auto-select first available)
///   - `"bridged:en0"` → `.bridged(interface: "en0")` (specific interface)
enum NetworkMode: Equatable, CustomStringConvertible {
    case nat
    case bridged(interface: String?)

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
        }
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
                debugDescription: "Invalid network mode: \(string). Expected 'nat', 'bridged', or 'bridged:<interface>'."
            )
        }
        self = mode
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        try container.encode(configString)
    }
}
