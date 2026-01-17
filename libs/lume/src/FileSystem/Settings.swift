import Foundation

/// Manages the application settings using a config file
struct LumeSettings: Codable, Sendable {
    var vmLocations: [VMLocation]
    var defaultLocationName: String
    var cacheDirectory: String
    var cachingEnabled: Bool
    var registry: RegistryConfig?
    var telemetryEnabled: Bool

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
        cacheDirectory: "~/.lume/cache",
        cachingEnabled: false,
        registry: .defaultConfig,
        telemetryEnabled: true
    )

    /// Gets all locations sorted by name
    var sortedLocations: [VMLocation] {
        vmLocations.sorted { $0.name < $1.name }
    }
}

final class SettingsManager: @unchecked Sendable {
    // MARK: - Constants

    private enum Constants {
        // Default path for config
        static let fallbackConfigDir = "~/.config/lume"
        static let configFileName = "config.yaml"
    }

    // MARK: - Properties

    static let shared = SettingsManager()
    private let fileManager: FileManager

    // Get the config directory following XDG spec
    private var configDir: String {
        // Check XDG_CONFIG_HOME environment variable first
        if let xdgConfigHome = ProcessInfo.processInfo.environment["XDG_CONFIG_HOME"] {
            return "\(xdgConfigHome)/lume"
        }
        // Fall back to default
        return (Constants.fallbackConfigDir as NSString).expandingTildeInPath
    }

    // Path to config file
    private var configFilePath: String {
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
        let defaultSettings = LumeSettings(
            vmLocations: [
                VMLocation(name: "default", path: "~/.lume")
            ],
            defaultLocationName: "default",
            cacheDirectory: "~/.lume/cache",
            cachingEnabled: false,
            registry: .defaultConfig,
            telemetryEnabled: true
        )

        // Try to save default settings
        try? saveSettings(defaultSettings)

        return defaultSettings
    }

    func saveSettings(_ settings: LumeSettings) throws {
        try fileManager.createDirectory(atPath: configDir, withIntermediateDirectories: true)

        // Create a human-readable YAML-like configuration file
        var yamlContent = "# Lume Configuration\n\n"

        // Default location
        yamlContent += "defaultLocationName: \"\(settings.defaultLocationName)\"\n"

        // Cache directory
        yamlContent += "cacheDirectory: \"\(settings.cacheDirectory)\"\n"

        // Caching enabled flag
        yamlContent += "cachingEnabled: \(settings.cachingEnabled)\n"

        // Telemetry enabled flag
        yamlContent += "telemetryEnabled: \(settings.telemetryEnabled)\n"

        // VM locations
        yamlContent += "\n# VM Locations\nvmLocations:\n"
        for location in settings.vmLocations {
            yamlContent += "  - name: \"\(location.name)\"\n"
            yamlContent += "    path: \"\(location.path)\"\n"
        }

        // Registry configuration
        if let registry = settings.registry {
            yamlContent += "\n# Registry Configuration\nregistry:\n"
            yamlContent += "  type: \"\(registry.type.rawValue)\"\n"
            yamlContent += "  ghcr:\n"
            yamlContent += "    registry: \"\(registry.ghcr.registry)\"\n"
            yamlContent += "    organization: \"\(registry.ghcr.organization)\"\n"
            if let gcs = registry.gcs {
                yamlContent += "  gcs:\n"
                yamlContent += "    apiUrl: \"\(gcs.apiUrl)\"\n"
                yamlContent += "    apiKey: \"\(gcs.apiKey)\"\n"
            }
        }

        // Write YAML content to file
        try yamlContent.write(
            to: URL(fileURLWithPath: configFilePath), atomically: true, encoding: .utf8)
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

    func setCachingEnabled(_ enabled: Bool) throws {
        var settings = getSettings()
        settings.cachingEnabled = enabled
        try saveSettings(settings)
    }

    func isCachingEnabled() -> Bool {
        return getSettings().cachingEnabled
    }

    // MARK: - Telemetry Management

    func setTelemetryEnabled(_ enabled: Bool) throws {
        var settings = getSettings()
        settings.telemetryEnabled = enabled
        try saveSettings(settings)
    }

    func isTelemetryEnabled() -> Bool {
        return getSettings().telemetryEnabled
    }

    // MARK: - Private Helpers

    private func ensureConfigDirectoryExists() {
        try? fileManager.createDirectory(atPath: configDir, withIntermediateDirectories: true)
    }

    private func readSettingsFromFile() -> LumeSettings? {
        // Read from YAML file
        if fileExists(at: configFilePath) {
            do {
                let yamlString = try String(
                    contentsOf: URL(fileURLWithPath: configFilePath), encoding: .utf8)
                return parseYamlSettings(yamlString)
            } catch {
                Logger.error(
                    "Failed to read settings from YAML file",
                    metadata: ["error": error.localizedDescription]
                )
            }
        }
        return nil
    }

    private func parseYamlSettings(_ yamlString: String) -> LumeSettings? {
        // This is a very basic YAML parser for our specific config format
        // A real implementation would use a proper YAML library

        var defaultLocationName = "default"
        var cacheDirectory = "~/.lume/cache"
        var cachingEnabled = false  // default to false to save disk space
        var telemetryEnabled = true  // default to true for anonymous usage tracking
        var vmLocations: [VMLocation] = []

        // Registry config parsing state
        var registryType: RegistryType = .ghcr
        var ghcrRegistry = "ghcr.io"
        var ghcrOrganization = "trycua"
        var gcsApiUrl: String?
        var gcsApiKey: String?

        enum ParserSection {
            case top
            case vmLocations
            case registry
            case registryGhcr
            case registryGcs
        }

        var currentSection: ParserSection = .top
        var currentLocation: (name: String?, path: String?) = (nil, nil)

        let lines = yamlString.split(separator: "\n")

        for (_, line) in lines.enumerated() {
            let trimmedLine = line.trimmingCharacters(in: .whitespaces)

            // Skip comments and empty lines
            if trimmedLine.hasPrefix("#") || trimmedLine.isEmpty {
                continue
            }

            // Check for section markers
            if trimmedLine == "vmLocations:" {
                // Save any pending location before switching sections
                if let name = currentLocation.name, let path = currentLocation.path {
                    vmLocations.append(VMLocation(name: name, path: path))
                }
                currentLocation = (nil, nil)
                currentSection = .vmLocations
                continue
            } else if trimmedLine == "registry:" {
                // Save any pending location before switching sections
                if let name = currentLocation.name, let path = currentLocation.path {
                    vmLocations.append(VMLocation(name: name, path: path))
                }
                currentLocation = (nil, nil)
                currentSection = .registry
                continue
            } else if trimmedLine == "ghcr:" && currentSection == .registry {
                currentSection = .registryGhcr
                continue
            } else if trimmedLine == "gcs:" && currentSection == .registry {
                currentSection = .registryGcs
                continue
            }

            // Handle indentation-based section detection
            let lineIndent = line.prefix(while: { $0 == " " }).count

            // Process based on current section
            switch currentSection {
            case .vmLocations:
                if trimmedLine.hasPrefix("-") || trimmedLine.contains("- name:") {
                    // Process the previous location before starting a new one
                    if let name = currentLocation.name, let path = currentLocation.path {
                        vmLocations.append(VMLocation(name: name, path: path))
                    }
                    currentLocation = (nil, nil)
                }

                // Process the key-value pairs within a location
                if let colonIndex = trimmedLine.firstIndex(of: ":") {
                    let key = trimmedLine[..<colonIndex].trimmingCharacters(in: .whitespaces)
                    let rawValue = trimmedLine[trimmedLine.index(after: colonIndex)...]
                        .trimmingCharacters(in: .whitespaces)
                    let value = extractValueFromYaml(rawValue)

                    if key.hasSuffix("name") {
                        currentLocation.name = value
                    } else if key.hasSuffix("path") {
                        currentLocation.path = value
                    }
                }

            case .registry:
                if let colonIndex = trimmedLine.firstIndex(of: ":") {
                    let key = trimmedLine[..<colonIndex].trimmingCharacters(in: .whitespaces)
                    let rawValue = trimmedLine[trimmedLine.index(after: colonIndex)...]
                        .trimmingCharacters(in: .whitespaces)
                    let value = extractValueFromYaml(rawValue)

                    if key == "type" {
                        registryType = RegistryType(rawValue: value) ?? .ghcr
                    }
                }

            case .registryGhcr:
                if lineIndent < 4 && !trimmedLine.isEmpty {
                    // Back to registry section or new section
                    if trimmedLine == "gcs:" {
                        currentSection = .registryGcs
                        continue
                    } else if !trimmedLine.hasPrefix("registry") && !trimmedLine.hasPrefix("organization") {
                        currentSection = .top
                    }
                }

                if let colonIndex = trimmedLine.firstIndex(of: ":") {
                    let key = trimmedLine[..<colonIndex].trimmingCharacters(in: .whitespaces)
                    let rawValue = trimmedLine[trimmedLine.index(after: colonIndex)...]
                        .trimmingCharacters(in: .whitespaces)
                    let value = extractValueFromYaml(rawValue)

                    if key == "registry" {
                        ghcrRegistry = value
                    } else if key == "organization" {
                        ghcrOrganization = value
                    }
                }

            case .registryGcs:
                if lineIndent < 4 && !trimmedLine.isEmpty && !trimmedLine.hasPrefix("apiUrl") && !trimmedLine.hasPrefix("apiKey") {
                    currentSection = .top
                }

                if let colonIndex = trimmedLine.firstIndex(of: ":") {
                    let key = trimmedLine[..<colonIndex].trimmingCharacters(in: .whitespaces)
                    let rawValue = trimmedLine[trimmedLine.index(after: colonIndex)...]
                        .trimmingCharacters(in: .whitespaces)
                    let value = extractValueFromYaml(rawValue)

                    if key == "apiUrl" {
                        gcsApiUrl = value
                    } else if key == "apiKey" {
                        gcsApiKey = value
                    }
                }

            case .top:
                // Process top-level keys
                if let colonIndex = trimmedLine.firstIndex(of: ":") {
                    let key = trimmedLine[..<colonIndex].trimmingCharacters(in: .whitespaces)
                    let rawValue = trimmedLine[trimmedLine.index(after: colonIndex)...]
                        .trimmingCharacters(in: .whitespaces)
                    let value = extractValueFromYaml(rawValue)

                    if key == "defaultLocationName" {
                        defaultLocationName = value
                    } else if key == "cacheDirectory" {
                        cacheDirectory = value
                    } else if key == "cachingEnabled" {
                        cachingEnabled = value.lowercased() == "true"
                    } else if key == "telemetryEnabled" {
                        telemetryEnabled = value.lowercased() == "true"
                    }
                }
            }
        }

        // Don't forget to add the last location
        if let name = currentLocation.name, let path = currentLocation.path {
            vmLocations.append(VMLocation(name: name, path: path))
        }

        // Ensure at least one location exists
        if vmLocations.isEmpty {
            vmLocations.append(VMLocation(name: "default", path: "~/.lume"))
        }

        // Build registry config
        let ghcrConfig = GHCRConfig(registry: ghcrRegistry, organization: ghcrOrganization)
        var gcsConfig: GCSConfig?
        if let apiUrl = gcsApiUrl, let apiKey = gcsApiKey {
            gcsConfig = GCSConfig(apiUrl: apiUrl, apiKey: apiKey)
        }
        let registryConfig = RegistryConfig(type: registryType, ghcr: ghcrConfig, gcs: gcsConfig)

        return LumeSettings(
            vmLocations: vmLocations,
            defaultLocationName: defaultLocationName,
            cacheDirectory: cacheDirectory,
            cachingEnabled: cachingEnabled,
            registry: registryConfig,
            telemetryEnabled: telemetryEnabled
        )
    }

    // Helper method to extract a value from YAML, handling quotes
    private func extractValueFromYaml(_ rawValue: String) -> String {
        if rawValue.hasPrefix("\"") && rawValue.hasSuffix("\"") && rawValue.count >= 2 {
            // Remove the surrounding quotes
            let startIndex = rawValue.index(after: rawValue.startIndex)
            let endIndex = rawValue.index(before: rawValue.endIndex)
            return String(rawValue[startIndex..<endIndex])
        }
        return rawValue
    }

    // Helper method to output debug information about the current settings
    func debugSettings() -> String {
        let settings = getSettings()

        var output = "Current Settings:\n"
        output += "- Default VM storage: \(settings.defaultLocationName)\n"
        output += "- Cache directory: \(settings.cacheDirectory)\n"
        output += "- VM Locations (\(settings.vmLocations.count)):\n"

        for (i, location) in settings.vmLocations.enumerated() {
            let isDefault = location.name == settings.defaultLocationName
            let defaultMark = isDefault ? " (default)" : ""
            output += "  \(i+1). \(location.name): \(location.path)\(defaultMark)\n"
        }

        // Also add raw file content
        if fileExists(at: configFilePath) {
            if let content = try? String(contentsOf: URL(fileURLWithPath: configFilePath)) {
                output += "\nRaw YAML file content:\n"
                output += content
            }
        }

        return output
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
