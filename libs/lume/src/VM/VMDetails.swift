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
    let vncUrl: String?
    let ipAddress: String?
    let sshAvailable: Bool?
    let locationName: String
    let sharedDirectories: [SharedDirectory]?

    enum CodingKeys: String, CodingKey {
        case name, os, cpuCount, memorySize, diskSize, display, status
        case vncUrl, ipAddress, sshAvailable, locationName, sharedDirectories
    }

    init(
        name: String,
        os: String,
        cpuCount: Int,
        memorySize: UInt64,
        diskSize: DiskSize,
        display: String,
        status: String,
        vncUrl: String?,
        ipAddress: String?,
        sshAvailable: Bool? = nil,
        locationName: String,
        sharedDirectories: [SharedDirectory]? = nil
    ) {
        self.name = name
        self.os = os
        self.cpuCount = cpuCount
        self.memorySize = memorySize
        self.diskSize = diskSize
        self.display = display
        self.status = status
        self.vncUrl = vncUrl
        self.ipAddress = ipAddress
        self.sshAvailable = sshAvailable
        self.locationName = locationName
        self.sharedDirectories = sharedDirectories
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
        try container.encode(vncUrl, forKey: .vncUrl)
        try container.encode(ipAddress, forKey: .ipAddress)
        try container.encode(sshAvailable, forKey: .sshAvailable)
        try container.encode(locationName, forKey: .locationName)
        try container.encode(sharedDirectories, forKey: .sharedDirectories)
    }
}
