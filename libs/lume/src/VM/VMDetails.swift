import Foundation
import Network

struct DiskSize: Codable {
    let allocated: UInt64
    let total: UInt64
}

extension DiskSize {
    var formattedAllocated: String {
        formatBytes(allocated)
    }

    var formattedTotal: String {
        formatBytes(total)
    }

    private func formatBytes(_ bytes: UInt64) -> String {
        let units = ["B", "KB", "MB", "GB", "TB"]
        var size = Double(bytes)
        var unitIndex = 0

        while size >= 1024 && unitIndex < units.count - 1 {
            size /= 1024
            unitIndex += 1
        }

        return String(format: "%.1f%@", size, units[unitIndex])
    }
}

struct VMDetails: Codable {
    let name: String
    let os: String
    let cpuCount: Int
    let memorySize: UInt64
    let diskSize: DiskSize
    let display: String
    let status: String
    /// Operation type when status is "provisioning" (e.g., "ipsw_install", "unattended_setup")
    let provisioningOperation: String?
    let vncUrl: String?
    let ipAddress: String?
    let sshAvailable: Bool?
    let locationName: String
    let sharedDirectories: [SharedDirectory]?
    let networkMode: String?

    enum CodingKeys: String, CodingKey {
        case name, os, cpuCount, memorySize, diskSize, display, status
        case provisioningOperation, vncUrl, ipAddress, sshAvailable, locationName, sharedDirectories
        case networkMode
    }

    init(
        name: String,
        os: String,
        cpuCount: Int,
        memorySize: UInt64,
        diskSize: DiskSize,
        display: String,
        status: String,
        provisioningOperation: String? = nil,
        vncUrl: String?,
        ipAddress: String?,
        sshAvailable: Bool? = nil,
        locationName: String,
        sharedDirectories: [SharedDirectory]? = nil,
        networkMode: String? = nil
    ) {
        self.name = name
        self.os = os
        self.cpuCount = cpuCount
        self.memorySize = memorySize
        self.diskSize = diskSize
        self.display = display
        self.status = status
        self.provisioningOperation = provisioningOperation
        self.vncUrl = vncUrl
        self.ipAddress = ipAddress
        self.sshAvailable = sshAvailable
        self.locationName = locationName
        self.sharedDirectories = sharedDirectories
        self.networkMode = networkMode
    }

    // Custom encoder to always include optional fields (even when nil)
    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(name, forKey: .name)
        try container.encode(os, forKey: .os)
        try container.encode(cpuCount, forKey: .cpuCount)
        try container.encode(memorySize, forKey: .memorySize)
        try container.encode(diskSize, forKey: .diskSize)
        try container.encode(display, forKey: .display)
        try container.encode(status, forKey: .status)
        try container.encode(provisioningOperation, forKey: .provisioningOperation)
        try container.encode(vncUrl, forKey: .vncUrl)
        try container.encode(ipAddress, forKey: .ipAddress)
        try container.encode(sshAvailable, forKey: .sshAvailable)
        try container.encode(locationName, forKey: .locationName)
        try container.encode(sharedDirectories, forKey: .sharedDirectories)
        try container.encode(networkMode, forKey: .networkMode)
    }
}
