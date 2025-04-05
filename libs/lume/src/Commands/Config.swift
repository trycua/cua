import ArgumentParser
import Foundation

struct Config: ParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "config",
        abstract: "Get or set lume configuration",
        subcommands: [Get.self, Location.self, Cache.self],
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
                "Default VM location: \(settings.defaultLocationName) (\(settings.defaultLocation?.path ?? "not set"))"
            )

            // Display cache directory
            print("Cache directory: \(settings.cacheDirectory)")

            // Display all locations
            if !settings.vmLocations.isEmpty {
                print("\nConfigured VM locations:")
                for location in settings.sortedLocations {
                    let isDefault = location.name == settings.defaultLocationName
                    let defaultMark = isDefault ? " (default)" : ""
                    print("  - \(location.name): \(location.path)\(defaultMark)")
                }
            }
        }
    }

    // MARK: - Cache Management Subcommands

    struct Cache: ParsableCommand {
        static let configuration = CommandConfiguration(
            commandName: "cache",
            abstract: "Manage cache settings",
            subcommands: [GetCache.self, SetCache.self]
        )

        struct GetCache: ParsableCommand {
            static let configuration = CommandConfiguration(
                commandName: "get",
                abstract: "Get current cache directory"
            )

            func run() throws {
                let controller = LumeController()
                let cacheDir = controller.getCacheDirectory()
                print("Cache directory: \(cacheDir)")
            }
        }

        struct SetCache: ParsableCommand {
            static let configuration = CommandConfiguration(
                commandName: "set",
                abstract: "Set cache directory"
            )

            @Argument(help: "Path to cache directory")
            var path: String

            func run() throws {
                let controller = LumeController()
                try controller.setCacheDirectory(path: path)
                print("Cache directory set to: \(path)")
            }
        }
    }

    // MARK: - Location Management Subcommands

    struct Location: ParsableCommand {
        static let configuration = CommandConfiguration(
            commandName: "location",
            abstract: "Manage VM locations",
            subcommands: [Add.self, Remove.self, List.self, Default.self]
        )

        struct Add: ParsableCommand {
            static let configuration = CommandConfiguration(
                commandName: "add",
                abstract: "Add a new VM location"
            )

            @Argument(help: "Location name (alphanumeric with dashes/underscores)")
            var name: String

            @Argument(help: "Path to VM location directory")
            var path: String

            func run() throws {
                let controller = LumeController()
                try controller.addLocation(name: name, path: path)
                print("Added VM location: \(name) at \(path)")
            }
        }

        struct Remove: ParsableCommand {
            static let configuration = CommandConfiguration(
                commandName: "remove",
                abstract: "Remove a VM location"
            )

            @Argument(help: "Location name to remove")
            var name: String

            func run() throws {
                let controller = LumeController()
                try controller.removeLocation(name: name)
                print("Removed VM location: \(name)")
            }
        }

        struct List: ParsableCommand {
            static let configuration = CommandConfiguration(
                commandName: "list",
                abstract: "List all VM locations"
            )

            func run() throws {
                let controller = LumeController()
                let settings = controller.getSettings()

                if settings.vmLocations.isEmpty {
                    print("No VM locations configured")
                    return
                }

                print("VM Locations:")
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
                abstract: "Set the default VM location"
            )

            @Argument(help: "Location name to set as default")
            var name: String

            func run() throws {
                let controller = LumeController()
                try controller.setDefaultLocation(name: name)
                print("Set default VM location to: \(name)")
            }
        }
    }
}
