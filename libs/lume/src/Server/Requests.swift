import ArgumentParser
import Foundation
import Virtualization

struct RunVMRequest: Codable {
    let noDisplay: Bool?
    let sharedDirectories: [SharedDirectoryRequest]?
    let recoveryMode: Bool?

    struct SharedDirectoryRequest: Codable {
        let hostPath: String
        let readOnly: Bool?
    }

    func parse() throws -> [SharedDirectory] {
        guard let sharedDirectories = sharedDirectories else { return [] }

        return try sharedDirectories.map { dir -> SharedDirectory in
            // Validate that the host path exists and is a directory
            var isDirectory: ObjCBool = false
            guard FileManager.default.fileExists(atPath: dir.hostPath, isDirectory: &isDirectory),
                isDirectory.boolValue
            else {
                throw ValidationError(
                    "Host path does not exist or is not a directory: \(dir.hostPath)")
            }

            return SharedDirectory(
                hostPath: dir.hostPath,
                tag: VZVirtioFileSystemDeviceConfiguration.macOSGuestAutomountTag,
                readOnly: dir.readOnly ?? false
            )
        }
    }
}

struct PullRequest: Codable {
    let image: String
    let name: String?
    var registry: String
    var organization: String

    enum CodingKeys: String, CodingKey {
        case image, name, registry, organization
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        image = try container.decode(String.self, forKey: .image)
        name = try container.decodeIfPresent(String.self, forKey: .name)
        registry = try container.decodeIfPresent(String.self, forKey: .registry) ?? "ghcr.io"
        organization = try container.decodeIfPresent(String.self, forKey: .organization) ?? "trycua"
    }
}

struct CreateVMRequest: Codable {
    let name: String
    let os: String
    let cpu: Int
    let memory: String
    let diskSize: String
    let display: String
    let ipsw: String?

    func parse() throws -> (memory: UInt64, diskSize: UInt64) {
        return (
            memory: try parseSize(memory),
            diskSize: try parseSize(diskSize)
        )
    }
}

struct SetVMRequest: Codable {
    let cpu: Int?
    let memory: String?
    let diskSize: String?
    let display: String?

    func parse() throws -> (memory: UInt64?, diskSize: UInt64?, display: VMDisplayResolution?) {
        return (
            memory: try memory.map { try parseSize($0) },
            diskSize: try diskSize.map { try parseSize($0) },
            display: try display.map {
                guard let resolution = VMDisplayResolution(string: $0) else {
                    throw ValidationError(
                        "Invalid display resolution format: \($0). Expected format: WIDTHxHEIGHT")
                }
                return resolution
            }
        )
    }
}

struct CloneRequest: Codable {
    let name: String
    let newName: String
}
