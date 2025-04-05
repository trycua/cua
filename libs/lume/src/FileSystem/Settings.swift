import Foundation

/// Manages the application settings using a config file
struct LumeSettings: Codable, Sendable {
    var vmLocations: [VMLocation]
    var defaultLocationName: String
    var cacheDirectory: String

    var defaultLocation: VMLocation? {
        vmLocations.first { $0.name == defaultLocationName }
    }

    // For backward compatibility
    var homeDirectory: String {
        defaultLocation?.path ?? "~/.lume"
    }

    static let defaultSettings = LumeSettings(
        vmLocations: [
            VMLocation(name: "default", path: "~/.lume")
        ],
        defaultLocationName: "default",
        cacheDirectory: "~/.lume/cache"
    )

    /// Gets all locations sorted by name
    var sortedLocations: [VMLocation] {
        vmLocations.sorted { $0.name < $1.name }
    }
}

final class SettingsManager: @unchecked Sendable {
    // MARK: - Constants

    private enum Constants {
        static let xdgConfigDir = "~/.config/lume"
        static let configFileName = "config.json"
    }

    // MARK: - Properties

    static let shared = SettingsManager()
    private let fileManager: FileManager

    // Path to XDG config file
    private var configFilePath: String {
        let configDir = (Constants.xdgConfigDir as NSString).expandingTildeInPath
        return "\(configDir)/\(Constants.configFileName)"
    }

    // MARK: - Initialization

    init(fileManager: FileManager = .default) {
        self.fileManager = fileManager
        ensureConfigDirectoryExists()
    }

    // MARK: - Settings Access

    func getSettings() -> LumeSettings {
        if let settings = readSettingsFromFile() {
            return settings
        }

        // No settings file found, use defaults
        let defaultSettings = LumeSettings.defaultSettings

        // Try to save default settings
        try? saveSettings(defaultSettings)

        return defaultSettings
    }

    func saveSettings(_ settings: LumeSettings) throws {
        let configDir = (Constants.xdgConfigDir as NSString).expandingTildeInPath
        try fileManager.createDirectory(atPath: configDir, withIntermediateDirectories: true)

        let data = try JSONEncoder().encode(settings)
        try data.write(to: URL(fileURLWithPath: configFilePath))
    }

    // MARK: - VM Location Management

    func addLocation(_ location: VMLocation) throws {
        var settings = getSettings()

        // Validate location name (alphanumeric, dash, underscore)
        let nameRegex = try NSRegularExpression(pattern: "^[a-zA-Z0-9_-]+$")
        let nameRange = NSRange(location.name.startIndex..., in: location.name)
        if nameRegex.firstMatch(in: location.name, range: nameRange) == nil {
            throw VMLocationError.invalidLocationName(name: location.name)
        }

        // Check for duplicate name
        if settings.vmLocations.contains(where: { $0.name == location.name }) {
            throw VMLocationError.duplicateLocationName(name: location.name)
        }

        // Validate location path
        try location.validate()

        // Add location
        settings.vmLocations.append(location)
        try saveSettings(settings)
    }

    func removeLocation(name: String) throws {
        var settings = getSettings()

        // Check location exists
        guard settings.vmLocations.contains(where: { $0.name == name }) else {
            throw VMLocationError.locationNotFound(name: name)
        }

        // Prevent removing default location
        if name == settings.defaultLocationName {
            throw VMLocationError.defaultLocationCannotBeRemoved(name: name)
        }

        // Remove location
        settings.vmLocations.removeAll(where: { $0.name == name })
        try saveSettings(settings)
    }

    func setDefaultLocation(name: String) throws {
        var settings = getSettings()

        // Check location exists
        guard settings.vmLocations.contains(where: { $0.name == name }) else {
            throw VMLocationError.locationNotFound(name: name)
        }

        // Set default
        settings.defaultLocationName = name
        try saveSettings(settings)
    }

    func getLocation(name: String) throws -> VMLocation {
        let settings = getSettings()

        if let location = settings.vmLocations.first(where: { $0.name == name }) {
            return location
        }

        throw VMLocationError.locationNotFound(name: name)
    }

    // MARK: - Legacy Home Directory Compatibility

    func setHomeDirectory(path: String) throws {
        var settings = getSettings()

        let defaultLocation = VMLocation(name: "default", path: path)
        try defaultLocation.validate()

        // Replace default location
        if let index = settings.vmLocations.firstIndex(where: { $0.name == "default" }) {
            settings.vmLocations[index] = defaultLocation
        } else {
            settings.vmLocations.append(defaultLocation)
            settings.defaultLocationName = "default"
        }

        try saveSettings(settings)
    }

    // MARK: - Cache Directory Management

    func setCacheDirectory(path: String) throws {
        var settings = getSettings()

        // Validate path
        let expandedPath = (path as NSString).expandingTildeInPath
        var isDir: ObjCBool = false

        // If directory exists, check if it's writable
        if fileManager.fileExists(atPath: expandedPath, isDirectory: &isDir) {
            if !isDir.boolValue {
                throw SettingsError.notADirectory(path: expandedPath)
            }

            if !fileManager.isWritableFile(atPath: expandedPath) {
                throw SettingsError.directoryNotWritable(path: expandedPath)
            }
        } else {
            // Try to create the directory
            do {
                try fileManager.createDirectory(
                    atPath: expandedPath,
                    withIntermediateDirectories: true
                )
            } catch {
                throw SettingsError.directoryCreationFailed(path: expandedPath, error: error)
            }
        }

        // Update settings
        settings.cacheDirectory = path
        try saveSettings(settings)
    }

    func getCacheDirectory() -> String {
        return getSettings().cacheDirectory
    }

    // MARK: - Private Helpers

    private func ensureConfigDirectoryExists() {
        let configDir = (Constants.xdgConfigDir as NSString).expandingTildeInPath
        try? fileManager.createDirectory(atPath: configDir, withIntermediateDirectories: true)
    }

    private func readSettingsFromFile() -> LumeSettings? {
        guard fileExists(at: configFilePath) else { return nil }

        do {
            let data = try Data(contentsOf: URL(fileURLWithPath: configFilePath))
            return try JSONDecoder().decode(LumeSettings.self, from: data)
        } catch {
            Logger.error(
                "Failed to read settings from file", metadata: ["error": error.localizedDescription]
            )
            return nil
        }
    }

    private func fileExists(at path: String) -> Bool {
        fileManager.fileExists(atPath: path)
    }
}

// MARK: - Errors

enum SettingsError: Error, LocalizedError {
    case notADirectory(path: String)
    case directoryNotWritable(path: String)
    case directoryCreationFailed(path: String, error: Error)

    var errorDescription: String? {
        switch self {
        case .notADirectory(let path):
            return "Path is not a directory: \(path)"
        case .directoryNotWritable(let path):
            return "Directory is not writable: \(path)"
        case .directoryCreationFailed(let path, let error):
            return "Failed to create directory at \(path): \(error.localizedDescription)"
        }
    }
}
