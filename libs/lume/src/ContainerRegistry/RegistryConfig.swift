import Foundation

/// Type of container registry to use for pulling/pushing images
public enum RegistryType: String, Codable, CaseIterable, Sendable {
    case ghcr = "ghcr"
    case gcs = "gcs"

    public static let defaultType: RegistryType = .ghcr
}

/// Configuration for GitHub Container Registry (ghcr.io)
public struct GHCRConfig: Codable, Equatable, Sendable {
    /// The registry URL (default: ghcr.io)
    public var registry: String
    /// The organization/user namespace (default: trycua)
    public var organization: String

    public static let defaultConfig = GHCRConfig(
        registry: "ghcr.io",
        organization: "trycua"
    )

    public init(registry: String = "ghcr.io", organization: String = "trycua") {
        self.registry = registry
        self.organization = organization
    }
}

/// Configuration for GCS-backed registry (via Richmond API)
public struct GCSConfig: Codable, Equatable, Sendable {
    /// The API URL for getting signed URLs (e.g., https://api.cua.ai)
    public var apiUrl: String
    /// API key for authentication
    public var apiKey: String

    public init(apiUrl: String, apiKey: String) {
        self.apiUrl = apiUrl
        self.apiKey = apiKey
    }
}

/// Overall registry configuration
public struct RegistryConfig: Codable, Equatable, Sendable {
    /// The type of registry to use
    public var type: RegistryType
    /// GHCR configuration (used when type == .ghcr)
    public var ghcr: GHCRConfig
    /// GCS configuration (used when type == .gcs)
    public var gcs: GCSConfig?

    public static let defaultConfig = RegistryConfig(
        type: .ghcr,
        ghcr: .defaultConfig,
        gcs: nil
    )

    public init(type: RegistryType = .ghcr, ghcr: GHCRConfig = .defaultConfig, gcs: GCSConfig? = nil) {
        self.type = type
        self.ghcr = ghcr
        self.gcs = gcs
    }
}

// MARK: - Errors

public enum RegistryConfigError: Error, LocalizedError {
    case gcsConfigMissing
    case invalidApiUrl(String)
    case missingApiKey

    public var errorDescription: String? {
        switch self {
        case .gcsConfigMissing:
            return "GCS registry type selected but GCS configuration is missing. Run 'lume config set registry.gcs.api_url <url>' and 'lume config set registry.gcs.api_key <key>'"
        case .invalidApiUrl(let url):
            return "Invalid API URL: \(url)"
        case .missingApiKey:
            return "API key is required for GCS registry. Run 'lume config set registry.gcs.api_key <key>'"
        }
    }
}
