import Foundation

/// Manages the application settings using a config file
struct LumeSettings: Codable, Sendable {
    var vmLocations: [VMLocation]
    var defaultLocationName: String
    var cacheDirectory: String
    var cachingEnabled: Bool

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
        cachingEnabled: true
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
            cachingEnabled: true
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

        // VM locations
        yamlContent += "\n# VM Locations\nvmLocations:\n"
        for location in settings.vmLocations {
            yamlContent += "  - name: \"\(location.name)\"\n"
            yamlContent += "    path: \"\(location.path)\"\n"
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
        var cachingEnabled = true  // default to true for backward compatibility
        var vmLocations: [VMLocation] = []

        var inLocationsSection = false
        var currentLocation: (name: String?, path: String?) = (nil, nil)

        let lines = yamlString.split(separator: "\n")

        for (_, line) in lines.enumerated() {
            let trimmedLine = line.trimmingCharacters(in: .whitespaces)

            // Skip comments and empty lines
            if trimmedLine.hasPrefix("#") || trimmedLine.isEmpty {
                continue
            }

            // Check for section marker
            if trimmedLine == "vmLocations:" {
                inLocationsSection = true
                continue
            }

            // In the locations section, handle line indentation more carefully
            if inLocationsSection {
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
            } else {
                // Process top-level keys outside the locations section
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

        return LumeSettings(
            vmLocations: vmLocations,
            defaultLocationName: defaultLocationName,
            cacheDirectory: cacheDirectory,
            cachingEnabled: cachingEnabled
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

// MARK: - macOS Cache Normalization & Migration

@MainActor fileprivate var _cacheDeprecationNoteThisRun: String? = nil
@MainActor fileprivate var _cacheDeprecationEmittedThisRun: Bool = false

extension SettingsManager {
    #if os(macOS)
    /// Performs macOS-only cache path normalization and safe migration from legacy
    /// ~/Library/Caches paths to the canonical ~/.lume/cache. Returns a one-time
    /// deprecation note to be printed by the CLI if work was performed or legacy
    /// directories were detected. Subsequent runs are suppressed by a marker file.
    @MainActor
    func normalizeAndMigrateCacheDirectoryIfNeeded() -> String? {
        let cfgDirURL = URL(fileURLWithPath: self.configDir)
        let markerURL = cfgDirURL.appendingPathComponent(".cache_migration_done")
        let fm = self.fileManager

        // If we've already migrated/warned in a previous run, skip
        if fm.fileExists(atPath: markerURL.path) {
            return nil
        }

        let settings = getSettings()
        let configuredCache = settings.cacheDirectory
        let expandedCache = (configuredCache as NSString).expandingTildeInPath
        let defaultCache = ("~/.lume/cache" as NSString).expandingTildeInPath

        let homePath = NSHomeDirectory()
        let legacyLower = URL(fileURLWithPath: (homePath as NSString).appendingPathComponent("Library/Caches/lume"))
        let legacyUpper = URL(fileURLWithPath: (homePath as NSString).appendingPathComponent("Library/Caches/Lume"))

        // Gather existing legacy sources and de-duplicate by file resource identifier
        var sources: [URL] = []
        let candidates = [legacyLower, legacyUpper].filter { fm.fileExists(atPath: $0.path) }
        var seenIds: [AnyHashable] = []
        for url in candidates {
            if let id = try? url.resourceValues(forKeys: [.fileResourceIdentifierKey]).fileResourceIdentifier as? AnyHashable {
                if !seenIds.contains(where: { $0 == id }) {
                    sources.append(url)
                    seenIds.append(id)
                }
            } else {
                // If we can't obtain an identifier, include it conservatively
                sources.append(url)
            }
        }

        // If no legacy caches are present, nothing to do.
        if sources.isEmpty {
            return nil
        }

        // Ensure config dir exists to store marker
        try? fm.createDirectory(at: cfgDirURL, withIntermediateDirectories: true)

        // If using default cache, migrate contents safely. Otherwise warn-only.
        if expandedCache == defaultCache {
            let destURL = URL(fileURLWithPath: expandedCache)
            // Ensure destination exists
            try? fm.createDirectory(at: destURL, withIntermediateDirectories: true)
            // Perform a safe merge copy for each legacy source
            for src in sources {
                mergeCopy(from: src, to: destURL, fileManager: fm)
            }

            // Write marker to avoid repeating
            fm.createFile(atPath: markerURL.path, contents: Data(), attributes: nil)

            let note = "DEPRECATION: Lume no longer uses ~/Library/Caches on macOS. Your cache has been migrated to ~/.lume/cache."
            _cacheDeprecationNoteThisRun = note
            return note
        } else {
            // Custom cache configured - do not migrate automatically
            fm.createFile(atPath: markerURL.path, contents: Data(), attributes: nil)
            let note = "DEPRECATION: Legacy cache directories were detected under ~/Library/Caches, but a custom cache directory is configured (\(configuredCache)). Lume now uses ~/.lume/cache by default. No automatic migration was performed. Consider running 'lume config cache set \(configuredCache)' or moving files manually."
            _cacheDeprecationNoteThisRun = note
            return note
        }
    }

    /// Returns any pending deprecation note captured during normalization for this run.
    @MainActor
    func pendingDeprecationNote() -> String? { _cacheDeprecationNoteThisRun }
    /// Marks that the deprecation note has been emitted in this process.
    @MainActor
    func markDeprecationEmitted() { _cacheDeprecationEmittedThisRun = true }
    /// Whether the deprecation note has been emitted in this process.
    @MainActor
    func deprecationAlreadyEmitted() -> Bool { _cacheDeprecationEmittedThisRun }

    /// Internal helper to safely merge-copy one directory tree into another.
    /// - On conflict, destination wins and source file is skipped.
    private func mergeCopy(from source: URL, to destination: URL, fileManager fm: FileManager) {
        var isDir: ObjCBool = false
        guard fm.fileExists(atPath: source.path, isDirectory: &isDir), isDir.boolValue else {
            return
        }
        if !fm.fileExists(atPath: destination.path) {
            try? fm.createDirectory(at: destination, withIntermediateDirectories: true)
        }

        let keys: [URLResourceKey] = [.isDirectoryKey]
        let enumerator = fm.enumerator(
            at: source,
            includingPropertiesForKeys: keys,
            options: [.skipsHiddenFiles,
                      .skipsPackageDescendants],
            errorHandler: { (url, error) -> Bool in
                Logger.info("Skipping unreadable item", metadata: ["path": url.path, "error": error.localizedDescription])
                return true
            }
        )

        while let item = enumerator?.nextObject() as? URL {
            let relativePath = item.path.replacingOccurrences(of: source.path + "/", with: "")
            let destURL = destination.appendingPathComponent(relativePath)
            do {
                let values = try item.resourceValues(forKeys: [.isDirectoryKey])
                if values.isDirectory == true {
                    if !fm.fileExists(atPath: destURL.path) {
                        try fm.createDirectory(at: destURL, withIntermediateDirectories: true)
                    }
                } else {
                    if fm.fileExists(atPath: destURL.path) {
                        Logger.info("Skipping existing cache file", metadata: ["path": destURL.path])
                        continue
                    }
                    try fm.createDirectory(at: destURL.deletingLastPathComponent(), withIntermediateDirectories: true)
                    try fm.copyItem(at: item, to: destURL)
                }
            } catch {
                Logger.info("Skipping item due to error", metadata: ["path": item.path, "error": error.localizedDescription])
                continue
            }
        }
    }

    // MARK: - Testing helper
    /// Internal test-only entrypoint to drive migration with injected paths.
    /// Returns the deprecation note if any work/warning was performed.
    internal func _normalizeAndMigrateForTesting(cacheDir: URL, configDirURL: URL, legacySources: [URL], useDefaultCache: Bool) -> String? {
        let fm = self.fileManager
        let markerURL = configDirURL.appendingPathComponent(".cache_migration_done")
        if fm.fileExists(atPath: markerURL.path) { return nil }
        if legacySources.isEmpty { return nil }
        try? fm.createDirectory(at: configDirURL, withIntermediateDirectories: true)
        if useDefaultCache {
            try? fm.createDirectory(at: cacheDir, withIntermediateDirectories: true)
            for src in legacySources { mergeCopy(from: src, to: cacheDir, fileManager: fm) }
            fm.createFile(atPath: markerURL.path, contents: Data(), attributes: nil)
            let note = "DEPRECATION: Lume no longer uses legacy cache directories. Your cache has been migrated to ~/.lume/cache."
            return note
        } else {
            fm.createFile(atPath: markerURL.path, contents: Data(), attributes: nil)
            let note = "DEPRECATION: Legacy cache directories were detected under ~/Library/Caches, but a custom cache directory is configured. No automatic migration was performed."
            return note
        }
    }
    #endif
}
