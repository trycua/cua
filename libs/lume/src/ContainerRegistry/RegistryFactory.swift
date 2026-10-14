import Foundation

/// Factory for creating image registries based on configuration
struct RegistryFactory {
    private init() {}

    // Default values for CLI options (must match Pull.swift and Push.swift defaults)
    private static let defaultGHCRRegistry = "ghcr.io"
    private static let defaultGHCROrganization = "trycua"

    /// Create an image registry based on the current settings
    /// - Parameters:
    ///   - registry: CLI override for the ghcr registry URL (from --registry flag)
    ///   - organization: CLI override for the ghcr organization (from --organization flag)
    /// - Returns: An ImageRegistry instance appropriate for the configured registry type
    ///
    /// Logic:
    /// - If CLI provides non-default values for registry/organization, always use ghcr with those values
    /// - Otherwise, use the registry type from settings (ghcr or gcs)
    static func createRegistry(
        registry: String? = nil,
        organization: String? = nil,
        username: String? = nil,
        password: String? = nil,
        preparationWorkRoot: String? = nil,
        preparationScopeID: String? = nil,
        preparationOperationID: String? = nil
    ) throws -> any ImageRegistry {
        let preparation = try OwnedPreparation.make(
            workRoot: preparationWorkRoot,
            scopeID: preparationScopeID,
            operationID: preparationOperationID)
        let settings = SettingsManager.shared.getSettings()
        let config = settings.registry ?? .defaultConfig

        // Check if user explicitly provided CLI overrides that differ from defaults
        let hasCliOverride = (registry != nil && registry != defaultGHCRRegistry) ||
                             (organization != nil && organization != defaultGHCROrganization)

        // If CLI override provided, force ghcr with those values
        if hasCliOverride {
            let registryURL = registry ?? defaultGHCRRegistry
            let org = organization ?? defaultGHCROrganization
            return try ImageContainerRegistry(registry: registryURL, organization: org, username: username, password: password, preparation: preparation)
        }

        // Otherwise use configured registry type
        switch config.type {
        case .ghcr:
            return try ImageContainerRegistry(
                registry: config.ghcr.registry,
                organization: config.ghcr.organization,
                username: username,
                password: password,
                preparation: preparation
            )

        case .gcs:
            if preparation != nil {
                throw PreparationPlacementError.unsupportedGCSWorkRoot
            }
            guard let gcsConfig = config.gcs else {
                throw RegistryConfigError.gcsConfigMissing
            }
            return try GCSImageRegistry(config: gcsConfig)
        }
    }

    /// Create a registry with explicit configuration (useful for testing or direct API calls)
    static func createRegistry(config: RegistryConfig, preparationWorkRoot: String? = nil, preparationScopeID: String? = nil, preparationOperationID: String? = nil) throws -> any ImageRegistry {
        let preparation = try OwnedPreparation.make(
            workRoot: preparationWorkRoot,
            scopeID: preparationScopeID,
            operationID: preparationOperationID)
        switch config.type {
        case .ghcr:
            return try ImageContainerRegistry(
                registry: config.ghcr.registry,
                organization: config.ghcr.organization,
                preparation: preparation
            )

        case .gcs:
            if preparation != nil {
                throw PreparationPlacementError.unsupportedGCSWorkRoot
            }
            guard let gcsConfig = config.gcs else {
                throw RegistryConfigError.gcsConfigMissing
            }
            return try GCSImageRegistry(config: gcsConfig)
        }
    }
}
