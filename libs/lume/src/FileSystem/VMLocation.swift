import Foundation

/// Represents a location where VMs can be stored
struct VMLocation: Codable, Equatable, Sendable {
    let name: String
    let path: String

    var expandedPath: String {
        (path as NSString).expandingTildeInPath
    }

    /// Validates the location path exists and is writable
    func validate() throws {
        let fullPath = expandedPath
        var isDir: ObjCBool = false

        if FileManager.default.fileExists(atPath: fullPath, isDirectory: &isDir) {
            if !isDir.boolValue {
                throw VMLocationError.notADirectory(path: fullPath)
            }

            if !FileManager.default.isWritableFile(atPath: fullPath) {
                throw VMLocationError.directoryNotWritable(path: fullPath)
            }
        } else {
            // Try to create the directory
            do {
                try FileManager.default.createDirectory(
                    atPath: fullPath,
                    withIntermediateDirectories: true
                )
            } catch {
                throw VMLocationError.directoryCreationFailed(path: fullPath, error: error)
            }
        }
    }
}

// MARK: - Errors

enum VMLocationError: Error, LocalizedError {
    case notADirectory(path: String)
    case directoryNotWritable(path: String)
    case directoryCreationFailed(path: String, error: Error)
    case locationNotFound(name: String)
    case duplicateLocationName(name: String)
    case invalidLocationName(name: String)
    case defaultLocationCannotBeRemoved(name: String)

    var errorDescription: String? {
        switch self {
        case .notADirectory(let path):
            return "Path is not a directory: \(path)"
        case .directoryNotWritable(let path):
            return "Directory is not writable: \(path)"
        case .directoryCreationFailed(let path, let error):
            return "Failed to create directory at \(path): \(error.localizedDescription)"
        case .locationNotFound(let name):
            return "VM location not found: \(name)"
        case .duplicateLocationName(let name):
            return "VM location with name '\(name)' already exists"
        case .invalidLocationName(let name):
            return
                "Invalid location name: \(name). Names should be alphanumeric with underscores or dashes."
        case .defaultLocationCannotBeRemoved(let name):
            return "Cannot remove the default location '\(name)'. Set a new default location first."
        }
    }
}
