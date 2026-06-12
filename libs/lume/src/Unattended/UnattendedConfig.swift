import Foundation
import Yams

/// Health check configuration for verifying unattended setup success
struct HealthCheck: Codable, Sendable {
    /// Type of health check: "ssh", "http", etc.
    let type: String

    /// Username for SSH health check
    let user: String?

    /// Password for SSH health check
    let password: String?

    /// Timeout in seconds for the health check (default: 30)
    let timeout: Int?

    /// Number of retries before failing (default: 3)
    let retries: Int?

    /// Delay between retries in seconds (default: 5)
    let retryDelay: Int?

    enum CodingKeys: String, CodingKey {
        case type
        case user
        case password
        case timeout
        case retries
        case retryDelay = "retry_delay"
    }

    init(type: String, user: String? = nil, password: String? = nil, timeout: Int? = nil, retries: Int? = nil, retryDelay: Int? = nil) {
        self.type = type
        self.user = user
        self.password = password
        self.timeout = timeout
        self.retries = retries
        self.retryDelay = retryDelay
    }
}

/// Configuration for unattended macOS installation
struct UnattendedConfig: Codable, Sendable {
    /// Seconds to wait before starting automation after VM boots
    let bootWait: Int

    /// Boot commands to execute for Setup Assistant automation
    let bootCommands: [String]

    /// Optional health check to verify setup success
    let healthCheck: HealthCheck?

    /// Optional commands to run via SSH after health check passes
    /// These are more reliable than typing in Terminal via VNC
    let postSshCommands: [String]?

    enum CodingKeys: String, CodingKey {
        case bootWait = "boot_wait"
        case bootCommands = "boot_commands"
        case healthCheck = "health_check"
        case postSshCommands = "post_ssh_commands"
    }

    init(bootWait: Int = 60, bootCommands: [String], healthCheck: HealthCheck? = nil, postSshCommands: [String]? = nil) {
        self.bootWait = bootWait
        self.bootCommands = bootCommands
        self.healthCheck = healthCheck
        self.postSshCommands = postSshCommands
    }

    /// Load configuration from a YAML file path or preset name
    /// If the path matches a known preset name (e.g., "tahoe", "enable-ssh"), loads the built-in preset
    /// Otherwise, treats it as a file path
    static func load(from pathOrPreset: String) throws -> UnattendedConfig {
        // Check if it's a preset name first
        if let presetConfig = try? loadPreset(name: pathOrPreset) {
            return presetConfig
        }

        // Otherwise treat it as a file path
        let expandedPath = (pathOrPreset as NSString).expandingTildeInPath
        let url = URL(fileURLWithPath: expandedPath)
        let yamlString = try String(contentsOf: url, encoding: .utf8)
        return try parse(yaml: yamlString)
    }

    /// Load a built-in preset by name
    static func loadPreset(name: String) throws -> UnattendedConfig {
        guard let url = Bundle.lumeResources.url(
            forResource: name,
            withExtension: "yml",
            subdirectory: "unattended-presets"
        ) else {
            throw UnattendedConfigError.presetNotFound(name)
        }

        let yamlString = try String(contentsOf: url, encoding: .utf8)
        return try parse(yaml: yamlString)
    }

    /// Check if a name is a known preset
    static func isPreset(name: String) -> Bool {
        return Bundle.lumeResources.url(
            forResource: name,
            withExtension: "yml",
            subdirectory: "unattended-presets"
        ) != nil
    }

    /// List all available preset names
    static func availablePresets() -> [String] {
        guard let resourceURL = Bundle.lumeResources.url(forResource: "unattended-presets", withExtension: nil),
              let contents = try? FileManager.default.contentsOfDirectory(at: resourceURL, includingPropertiesForKeys: nil)
        else {
            return []
        }

        return contents
            .filter { $0.pathExtension == "yml" }
            .map { $0.deletingPathExtension().lastPathComponent }
            .sorted()
    }

    /// Parse configuration from a YAML string
    static func parse(yaml: String) throws -> UnattendedConfig {
        let decoder = YAMLDecoder()
        return try decoder.decode(UnattendedConfig.self, from: yaml)
    }
}

/// Errors related to unattended configuration
enum UnattendedConfigError: Error, LocalizedError {
    case fileNotFound(String)
    case invalidYaml(String)
    case missingRequiredField(String)
    case presetNotFound(String)

    var errorDescription: String? {
        switch self {
        case .fileNotFound(let path):
            return "Unattended config file not found: \(path)"
        case .invalidYaml(let message):
            return "Invalid YAML in unattended config: \(message)"
        case .missingRequiredField(let field):
            return "Missing required field in unattended config: \(field)"
        case .presetNotFound(let name):
            let available = UnattendedConfig.availablePresets()
            if available.isEmpty {
                return "Unattended preset '\(name)' not found"
            }
            return "Unattended preset '\(name)' not found. Available presets: \(available.joined(separator: ", "))"
        }
    }
}
