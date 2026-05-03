import ArgumentParser
import Foundation
import Virtualization

struct Config: ParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "config",
        abstract: "Get or set lume configuration",
        subcommands: [Get.self, Storage.self, Cache.self, Telemetry.self, Registry.self, Network.self],
        defaultSubcommand: Get.self
    )

    // MARK: - Basic Configuration Subcommands

    struct Get: ParsableCommand {
        static let configuration = CommandConfiguration(
            commandName: "get",
            abstract: "Get current configuration"
        )

        func run() throws {
            let controller = LumeController()
            let settings = controller.getSettings()

            // Display default location
            print(
                "Default VM storage: \(settings.defaultLocationName) (\(settings.defaultLocation?.path ?? "not set"))"
            )

            // Display cache directory
            print("Cache directory: \(settings.cacheDirectory)")

            // Display caching enabled status
            print("Caching enabled: \(settings.cachingEnabled)")

            // Display telemetry status
            print("Telemetry enabled: \(settings.telemetryEnabled)")

            // Display registry info
            if let registry = settings.registry {
                print("Registry type: \(registry.type.rawValue)")
                print("Registry: \(registry.ghcr.registry)/\(registry.ghcr.organization)")
            }

            // Display all locations
            if !settings.vmLocations.isEmpty {
                print("\nConfigured VM storage locations:")
                for location in settings.sortedLocations {
                    let isDefault = location.name == settings.defaultLocationName
                    let defaultMark = isDefault ? " (default)" : ""
                    print("  - \(location.name): \(location.path)\(defaultMark)")
                }
            }
        }
    }

    // MARK: - Debug Command

    struct Debug: ParsableCommand {
        static let configuration = CommandConfiguration(
            commandName: "debug",
            abstract: "Output detailed debug information about current configuration",
            shouldDisplay: false
        )

        func run() throws {
            let debugInfo = SettingsManager.shared.debugSettings()
            print(debugInfo)
        }
    }

    // MARK: - Cache Management Subcommands

    struct Cache: ParsableCommand {
        static let configuration = CommandConfiguration(
            commandName: "cache",
            abstract: "Manage image cache settings",
            subcommands: [CacheDir.self, CacheEnable.self, CacheDisable.self, CacheStatus.self],
            defaultSubcommand: CacheStatus.self
        )

        struct CacheStatus: ParsableCommand {
            static let configuration = CommandConfiguration(
                commandName: "status",
                abstract: "Show cache status and directory"
            )

            func run() throws {
                let controller = LumeController()
                let settings = controller.getSettings()
                print("Caching enabled: \(settings.cachingEnabled)")
                print("Cache directory: \(settings.cacheDirectory)")
            }
        }

        struct CacheDir: ParsableCommand {
            static let configuration = CommandConfiguration(
                commandName: "dir",
                abstract: "Get or set cache directory"
            )

            @Argument(help: "Path to cache directory (omit to show current)")
            var path: String?

            func run() throws {
                let controller = LumeController()
                if let path = path {
                    try controller.setCacheDirectory(path: path)
                    print("Cache directory set to: \(path)")
                } else {
                    let cacheDir = controller.getCacheDirectory()
                    print("Cache directory: \(cacheDir)")
                }
            }
        }

        struct CacheEnable: ParsableCommand {
            static let configuration = CommandConfiguration(
                commandName: "enable",
                abstract: "Enable image caching"
            )

            func run() throws {
                let controller = LumeController()
                try controller.setCachingEnabled(true)
                print("Caching enabled")
            }
        }

        struct CacheDisable: ParsableCommand {
            static let configuration = CommandConfiguration(
                commandName: "disable",
                abstract: "Disable image caching"
            )

            func run() throws {
                let controller = LumeController()
                try controller.setCachingEnabled(false)
                print("Caching disabled")
            }
        }
    }

    // MARK: - Storage Management Subcommands

    struct Storage: ParsableCommand {
        static let configuration = CommandConfiguration(
            commandName: "storage",
            abstract: "Manage VM storage locations",
            subcommands: [Add.self, Remove.self, List.self, Default.self]
        )

        struct Add: ParsableCommand {
            static let configuration = CommandConfiguration(
                commandName: "add",
                abstract: "Add a new VM storage location"
            )

            @Argument(help: "Storage name (alphanumeric with dashes/underscores)")
            var name: String

            @Argument(help: "Path to VM storage directory")
            var path: String

            func run() throws {
                let controller = LumeController()
                try controller.addLocation(name: name, path: path)
                print("Added VM storage location: \(name) at \(path)")
            }
        }

        struct Remove: ParsableCommand {
            static let configuration = CommandConfiguration(
                commandName: "remove",
                abstract: "Remove a VM storage location"
            )

            @Argument(help: "Storage name to remove")
            var name: String

            func run() throws {
                let controller = LumeController()
                try controller.removeLocation(name: name)
                print("Removed VM storage location: \(name)")
            }
        }

        struct List: ParsableCommand {
            static let configuration = CommandConfiguration(
                commandName: "list",
                abstract: "List all VM storage locations"
            )

            func run() throws {
                let controller = LumeController()
                let settings = controller.getSettings()

                if settings.vmLocations.isEmpty {
                    print("No VM storage locations configured")
                    return
                }

                print("VM Storage Locations:")
                for location in settings.sortedLocations {
                    let isDefault = location.name == settings.defaultLocationName
                    let defaultMark = isDefault ? " (default)" : ""
                    print("  - \(location.name): \(location.path)\(defaultMark)")
                }
            }
        }

        struct Default: ParsableCommand {
            static let configuration = CommandConfiguration(
                commandName: "default",
                abstract: "Set the default VM storage location"
            )

            @Argument(help: "Storage name to set as default")
            var name: String

            func run() throws {
                let controller = LumeController()
                try controller.setDefaultLocation(name: name)
                print("Set default VM storage location to: \(name)")
            }
        }
    }

    // MARK: - Telemetry Management Subcommands

    struct Telemetry: ParsableCommand {
        static let configuration = CommandConfiguration(
            commandName: "telemetry",
            abstract: "Manage anonymous telemetry settings",
            subcommands: [Status.self, Enable.self, Disable.self],
            defaultSubcommand: Status.self
        )

        struct Status: ParsableCommand {
            static let configuration = CommandConfiguration(
                commandName: "status",
                abstract: "Show current telemetry status"
            )

            func run() throws {
                let controller = LumeController()
                let settings = controller.getSettings()
                let envOverride = ProcessInfo.processInfo.environment["LUME_TELEMETRY_ENABLED"]

                print("Telemetry enabled: \(settings.telemetryEnabled)")

                if let envValue = envOverride {
                    let lowercased = envValue.lowercased()
                    let envEnabled = ["1", "true", "yes", "on"].contains(lowercased)
                    let envDisabled = ["0", "false", "no", "off"].contains(lowercased)
                    if envEnabled || envDisabled {
                        print("  (overridden by LUME_TELEMETRY_ENABLED=\(envValue))")
                    }
                }

                print("\nTelemetry collects anonymous usage data to help improve Lume.")
                print("No personal information or VM contents are ever collected.")
            }
        }

        struct Enable: ParsableCommand {
            static let configuration = CommandConfiguration(
                commandName: "enable",
                abstract: "Enable anonymous telemetry"
            )

            func run() throws {
                let controller = LumeController()
                try controller.setTelemetryEnabled(true)
                print("Telemetry enabled")
                print("Thank you for helping improve Lume!")
            }
        }

        struct Disable: ParsableCommand {
            static let configuration = CommandConfiguration(
                commandName: "disable",
                abstract: "Disable anonymous telemetry"
            )

            func run() throws {
                let controller = LumeController()
                try controller.setTelemetryEnabled(false)
                print("Telemetry disabled")
            }
        }
    }

    // MARK: - Registry Management Subcommands

    struct Registry: ParsableCommand {
        static let configuration = CommandConfiguration(
            commandName: "registry",
            abstract: "Manage container registry settings",
            subcommands: [RegistryStatus.self, RegistryType_.self, GHCR.self, GCS.self],
            defaultSubcommand: RegistryStatus.self
        )

        struct RegistryStatus: ParsableCommand {
            static let configuration = CommandConfiguration(
                commandName: "status",
                abstract: "Show current registry configuration"
            )

            func run() throws {
                let controller = LumeController()
                let settings = controller.getSettings()
                let registry = settings.registry ?? .defaultConfig

                print("Registry type: \(registry.type.rawValue)")
                print("")
                print("GHCR configuration:")
                print("  Registry: \(registry.ghcr.registry)")
                print("  Organization: \(registry.ghcr.organization)")

                if let gcs = registry.gcs {
                    print("")
                    print("GCS configuration:")
                    print("  API URL: \(gcs.apiUrl)")
                    print("  API Key: \(gcs.apiKey.prefix(8))...")
                } else if registry.type == .gcs {
                    print("")
                    print("GCS configuration: not set")
                    print("  Run 'lume config registry gcs --api-url <url> --api-key <key>' to configure")
                }
            }
        }

        struct RegistryType_: ParsableCommand {
            static let configuration = CommandConfiguration(
                commandName: "type",
                abstract: "Get or set registry type"
            )

            @Argument(help: "Registry type to use (ghcr, gcs). Omit to show current.")
            var type: String?

            func run() throws {
                let settings = SettingsManager.shared.getSettings()

                guard let typeStr = type else {
                    let registry = settings.registry ?? .defaultConfig
                    print("Registry type: \(registry.type.rawValue)")
                    return
                }

                guard let newType = RegistryType(rawValue: typeStr) else {
                    throw ValidationError("Invalid registry type '\(typeStr)'. Valid types: ghcr, gcs")
                }

                var updated = settings
                var registry = updated.registry ?? .defaultConfig
                registry.type = newType
                updated.registry = registry
                try SettingsManager.shared.saveSettings(updated)
                print("Registry type set to: \(newType.rawValue)")
            }
        }

        struct GHCR: ParsableCommand {
            static let configuration = CommandConfiguration(
                commandName: "ghcr",
                abstract: "Configure GitHub Container Registry settings"
            )

            @Option(name: .long, help: "Registry URL (default: ghcr.io)")
            var registry: String?

            @Option(name: .long, help: "Organization/namespace (default: trycua)")
            var organization: String?

            func run() throws {
                let settings = SettingsManager.shared.getSettings()
                let currentRegistry = settings.registry ?? .defaultConfig

                // If no flags, just print current
                if registry == nil && organization == nil {
                    print("GHCR configuration:")
                    print("  Registry: \(currentRegistry.ghcr.registry)")
                    print("  Organization: \(currentRegistry.ghcr.organization)")
                    return
                }

                var updated = settings
                var reg = updated.registry ?? .defaultConfig
                if let registry = registry {
                    reg.ghcr.registry = registry
                }
                if let organization = organization {
                    reg.ghcr.organization = organization
                }
                updated.registry = reg
                try SettingsManager.shared.saveSettings(updated)

                print("GHCR configuration updated:")
                print("  Registry: \(reg.ghcr.registry)")
                print("  Organization: \(reg.ghcr.organization)")
            }
        }

        struct GCS: ParsableCommand {
            static let configuration = CommandConfiguration(
                commandName: "gcs",
                abstract: "Configure GCS registry settings"
            )

            @Option(name: .long, help: "API URL for signed URL generation")
            var apiUrl: String?

            @Option(name: .long, help: "API key for authentication")
            var apiKey: String?

            func run() throws {
                let settings = SettingsManager.shared.getSettings()
                let currentRegistry = settings.registry ?? .defaultConfig

                // If no flags, just print current
                if apiUrl == nil && apiKey == nil {
                    if let gcs = currentRegistry.gcs {
                        print("GCS configuration:")
                        print("  API URL: \(gcs.apiUrl)")
                        print("  API Key: \(gcs.apiKey.prefix(8))...")
                    } else {
                        print("GCS configuration: not set")
                        print("  Use --api-url and --api-key to configure")
                    }
                    return
                }

                var updated = settings
                var reg = updated.registry ?? .defaultConfig
                let currentGcs = reg.gcs ?? GCSConfig(apiUrl: "", apiKey: "")

                let newApiUrl = apiUrl ?? currentGcs.apiUrl
                let newApiKey = apiKey ?? currentGcs.apiKey

                if newApiUrl.isEmpty {
                    throw ValidationError("API URL is required. Use --api-url <url>")
                }
                if newApiKey.isEmpty {
                    throw ValidationError("API key is required. Use --api-key <key>")
                }

                reg.gcs = GCSConfig(apiUrl: newApiUrl, apiKey: newApiKey)
                updated.registry = reg
                try SettingsManager.shared.saveSettings(updated)

                print("GCS configuration updated:")
                print("  API URL: \(newApiUrl)")
                print("  API Key: \(newApiKey.prefix(8))...")
            }
        }
    }

    // MARK: - Network Management Subcommands

    struct Network: ParsableCommand {
        static let configuration = CommandConfiguration(
            commandName: "network",
            abstract: "Manage network settings",
            subcommands: [Interfaces.self]
        )

        struct Interfaces: ParsableCommand {
            static let configuration = CommandConfiguration(
                commandName: "interfaces",
                abstract: "List available network interfaces for bridged networking"
            )

            func run() throws {
                let interfaces = VZBridgedNetworkInterface.networkInterfaces

                if interfaces.isEmpty {
                    print("No bridgeable network interfaces found.")
                    print("")
                    print("Note: Bridged networking requires the com.apple.vm.networking entitlement.")
                    return
                }

                print("Available network interfaces for bridged networking:")
                print("")
                for iface in interfaces {
                    let name = iface.localizedDisplayName ?? "Unknown"
                    print("  \(iface.identifier) — \(name)")
                }
                print("")
                print("Usage: lume run <vm-name> --network bridged:\(interfaces.first?.identifier ?? "en0")")
            }
        }
    }
}
