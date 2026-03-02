import Foundation

/// Represents a DHCP lease entry from the system's DHCP lease file
private struct DHCPLease {
    let macAddress: String
    let ipAddress: String
    let expirationDate: Date
    
    /// Creates a lease entry from raw DHCP lease file key-value pairs
    /// - Parameter dict: Dictionary containing the raw lease data
    /// - Returns: A DHCPLease instance if the data is valid, nil otherwise
    static func from(_ dict: [String: String]) -> DHCPLease? {
        guard let hwAddress = dict["hw_address"],
              let ipAddress = dict["ip_address"],
              let lease = dict["lease"] else {
            return nil
        }
        
        // Parse MAC address from hw_address field (format can be "1,xx:xx:xx:xx:xx:xx" or "ff,...")
        let hwParts = hwAddress.split(separator: ",")
        guard hwParts.count >= 2 else { return nil }
        
        // Get the MAC part after the prefix and normalize it
        let rawMacAddress = String(hwParts[1]).trimmingCharacters(in: .whitespaces)
        
        // Normalize the MAC address by ensuring each component is two digits
        let normalizedMacAddress = rawMacAddress.split(separator: ":")
            .map { component in
                let hex = String(component)
                return hex.count == 1 ? "0\(hex)" : hex
            }
            .joined(separator: ":")
        
        // Convert hex timestamp to Date
        let timestampHex = lease.trimmingCharacters(in: CharacterSet(charactersIn: "0x"))
        guard let timestamp = UInt64(timestampHex, radix: 16) else { return nil }
        let expirationDate = Date(timeIntervalSince1970: TimeInterval(timestamp))
        
        return DHCPLease(
            macAddress: normalizedMacAddress,
            ipAddress: ipAddress,
            expirationDate: expirationDate
        )
    }
    
    /// Checks if the lease is currently valid
    var isValid: Bool {
        expirationDate > Date()
    }
}

/// Parses DHCP lease files to retrieve IP addresses for VMs based on their MAC addresses
enum DHCPLeaseParser {
    private static let leasePath = "/var/db/dhcpd_leases"
    
    /// Retrieves the IP address for a given MAC address.
    /// First checks vmnet's DHCP leases (for NAT mode), then falls back to the
    /// system ARP table (for bridged mode where the VM gets its IP from the
    /// physical network's DHCP server).
    /// - Parameter macAddress: The MAC address to look up
    /// - Returns: The IP address if found, nil otherwise
    static func getIPAddress(forMAC macAddress: String) -> String? {
        // Normalize the input MAC address to ensure consistent format
        let normalizedMacAddress = macAddress.split(separator: ":").map { component in
            let hex = String(component)
            return hex.count == 1 ? "0\(hex)" : hex
        }.joined(separator: ":")

        // Check both sources: vmnet DHCP leases (NAT mode) and ARP table (bridged mode)
        let dhcpIP: String? = {
            guard let leaseContents = try? String(contentsOfFile: leasePath, encoding: .utf8),
                  let leases = try? parseDHCPLeases(leaseContents) else {
                return nil
            }
            return leases.first(where: { $0.macAddress == normalizedMacAddress })?.ipAddress
        }()
        let arpIP = getIPFromARP(forMAC: normalizedMacAddress)

        // If ARP has an entry, the VM is actively communicating on the physical
        // network (bridged mode). Prefer ARP since DHCP may contain a stale NAT
        // lease from a previous session.
        if let arpIP = arpIP {
            return arpIP
        }

        // No ARP entry means NAT mode (vmnet traffic doesn't appear in host ARP table)
        return dhcpIP
    }

    /// Looks up an IP address in the system ARP table by MAC address.
    /// Used for bridged networking where the VM gets its IP from the physical
    /// network's DHCP server instead of vmnet.
    private static func getIPFromARP(forMAC macAddress: String) -> String? {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/sbin/arp")
        process.arguments = ["-an"]

        let pipe = Pipe()
        process.standardOutput = pipe
        process.standardError = Pipe()

        do {
            try process.run()
            process.waitUntilExit()
        } catch {
            return nil
        }

        guard let output = String(data: pipe.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) else {
            return nil
        }

        // ARP output format: ? (192.168.14.18) at be:bd:fd:9d:8b:38 on en0 ifscope [ethernet]
        let lowercaseMAC = macAddress.lowercased()
        for line in output.components(separatedBy: "\n") {
            if line.lowercased().contains(lowercaseMAC) {
                // Extract IP from parentheses: ? (192.168.14.18) at ...
                if let openParen = line.firstIndex(of: "("),
                   let closeParen = line.firstIndex(of: ")") {
                    let ip = String(line[line.index(after: openParen)..<closeParen])
                    return ip
                }
            }
        }

        return nil
    }
    
    /// Parses the contents of a DHCP lease file into lease entries
    /// - Parameter contents: The raw contents of the lease file
    /// - Returns: Array of parsed lease entries
    private static func parseDHCPLeases(_ contents: String) throws -> [DHCPLease] {
        var leases: [DHCPLease] = []
        var currentLease: [String: String] = [:]
        var inLeaseBlock = false
        
        let lines = contents.components(separatedBy: .newlines)
        
        for line in lines {
            let trimmedLine = line.trimmingCharacters(in: .whitespaces)
            
            if trimmedLine == "{" {
                inLeaseBlock = true
                currentLease = [:]
            } else if trimmedLine == "}" {
                if let lease = DHCPLease.from(currentLease) {
                    leases.append(lease)
                }
                inLeaseBlock = false
            } else if inLeaseBlock {
                let parts = trimmedLine.split(separator: "=", maxSplits: 1)
                if parts.count == 2 {
                    let key = String(parts[0]).trimmingCharacters(in: .whitespaces)
                    let value = String(parts[1]).trimmingCharacters(in: .whitespaces)
                    currentLease[key] = value
                }
            }
        }
        
        return leases
    }
} 