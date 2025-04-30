import ArgumentParser
import Foundation
import Virtualization

struct RunVMRequest: Codable {
    let noDisplay: Bool?
    let sharedDirectories: [SharedDirectoryRequest]?
    let recoveryMode: Bool?
    let storage: String?

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
    let storage: String?

    enum CodingKeys: String, CodingKey {
        case image, name, registry, organization, storage
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        image = try container.decode(String.self, forKey: .image)
        name = try container.decodeIfPresent(String.self, forKey: .name)
        registry = try container.decodeIfPresent(String.self, forKey: .registry) ?? "ghcr.io"
        organization = try container.decodeIfPresent(String.self, forKey: .organization) ?? "trycua"
        storage = try container.decodeIfPresent(String.self, forKey: .storage)
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
    let storage: String?

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
    let storage: String?

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
    let sourceLocation: String?
    let destLocation: String?
}

struct PushRequest: Codable {
    let name: String // Name of the local VM
    let imageName: String // Base name for the image in the registry
    let tags: [String] // List of tags to push
    var registry: String // Registry URL
    var organization: String // Organization/user in the registry
    let storage: String? // Optional VM storage location or direct path
    var chunkSizeMb: Int // Chunk size
    // dryRun and reassemble are less common for API, default to false?
    // verbose is usually handled by server logging

    enum CodingKeys: String, CodingKey {
        case name, imageName, tags, registry, organization, storage, chunkSizeMb
    }

    // Provide default values for optional fields during decoding
    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        name = try container.decode(String.self, forKey: .name)
        imageName = try container.decode(String.self, forKey: .imageName)
        tags = try container.decode([String].self, forKey: .tags)
        registry = try container.decodeIfPresent(String.self, forKey: .registry) ?? "ghcr.io"
        organization = try container.decodeIfPresent(String.self, forKey: .organization) ?? "trycua"
        storage = try container.decodeIfPresent(String.self, forKey: .storage)
        chunkSizeMb = try container.decodeIfPresent(Int.self, forKey: .chunkSizeMb) ?? 512
    }
}
