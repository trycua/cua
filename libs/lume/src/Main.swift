import ArgumentParser
import Foundation

@main
struct Lume: AsyncParsableCommand {
    static var configuration: CommandConfiguration {
        _ = _parseTopLevelOptions  // Trigger parsing of top-level options and banner display

        return CommandConfiguration(
            commandName: "lume",
            abstract: "A lightweight CLI and local API server to build, run and manage macOS VMs.",
            version: Version.current,
            subcommands: CommandRegistry.allCommands,
            helpNames: .long
        )
    }

    // Run top-level parse once using a static stored property
    private static let _parseTopLevelOptions: Void = {
        let args = CommandLine.arguments

        // Check if help is requested and print banner
        if args.count == 1 || args.contains("--help") ||
           (args.count == 2 && args[1] == "help") {
            printBanner()
        }

        // Parse --config option
        for (index, arg) in args.enumerated() {
            if arg == "--config" && index + 1 < args.count {
                let configPath = args[index + 1]
                SettingsManager.shared.setCustomConfigPath(configPath)
                break
            }
        }
    }()

    @Option(name: .long,
        help: "Path to config file (default: $XDG_CONFIG_HOME/lume/config.yaml, falling back to ~/.config/lume/config.yaml if XDG_CONFIG_HOME is not set)"
    )
    var config: String?
}

// MARK: - Version Management
extension Lume {
    enum Version {
        static let current: String = "0.2.85"
    }
}

// MARK: - ASCII Art Banner
extension Lume {
    static let banner = """
    \u{001B}[34m  ⠀⣀⣀⡀⠀⠀⠀⠀⢀⣀⣀⣀⡀⠘⠋⢉⠙⣷⠀⠀ ⠀
     ⠀⠀⢀⣴⣿⡿⠋⣉⠁⣠⣾⣿⣿⣿⣿⡿⠿⣦⡈⠀⣿⡇⠃⠀
     ⠀⠀⠀⣽⣿⣧⠀⠃⢰⣿⣿⡏⠙⣿⠿⢧⣀⣼⣷⠀⡿⠃⠀⠀
     ⠀⠀⠀⠉⣿⣿⣦⠀⢿⣿⣿⣷⣾⡏⠀⠀⢹⣿⣿⠀⠀⠀⠀⠀⠀
     ⠀⠀⠀⠀⠀⠉⠛⠁⠈⠿⣿⣿⣿⣷⣄⣠⡼⠟⠁\u{001B}[0m\u{001B}[1m  lume v\(Version.current)\u{001B}[0m
    \u{001B}[34m           macOS VM CLI and server\u{001B}[0m
    """

    static func printBanner() {
        print(banner)
        print()
    }
}

// MARK: - Command Execution
extension Lume {
    func run() async throws {
        // Record installation event on first run (sent regardless of telemetry opt-out)
        TelemetryClient.shared.recordInstallation()

        // If no subcommand is provided, show help
        throw CleanExit.helpRequest(self)
    }
}