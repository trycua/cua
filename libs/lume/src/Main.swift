import ArgumentParser
import Foundation

@main
struct Lume: AsyncParsableCommand {
    static var configuration: CommandConfiguration {
        CommandConfiguration(
            commandName: "lume",
            abstract: "A lightweight CLI and local API server to build, run and manage macOS VMs.",
            version: Version.current,
            subcommands: CommandRegistry.allCommands,
            helpNames: .long
        )
    }
}

// MARK: - Version Management
extension Lume {
    enum Version {
        static let current: String = "0.3.16" // x-release-please-version
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

    static func shouldShowBanner() -> Bool {
        let args = CommandLine.arguments.dropFirst()
        // Show banner when: no args, --help, -h, help, or just the root command
        if args.isEmpty {
            return true
        }
        if args.contains("--help") || args.contains("-h") {
            return true
        }
        // Check if first arg is "help" (e.g., "lume help")
        if args.first == "help" && args.count == 1 {
            return true
        }
        return false
    }
}

// MARK: - Command Execution
extension Lume {
    public static func main() async {
        // Telemetry management must be able to be the first invocation without
        // creating an ID or making a request. Every other entry point performs
        // consent-aware registration and per-version release recording before
        // routine command telemetry can fire.
        if !isTelemetryManagementInvocation() {
            let rawChannel = ProcessInfo.processInfo.environment["LUME_INSTALL_CHANNEL"]
            let channel = TelemetryClient.normalizedInstallChannel(rawChannel)
            if channel == "first_run" && TelemetryClient.shared.shouldShowFirstRunNotice() {
                writeTelemetryNoticeToStandardError()
            }
            await TelemetryClient.shared.recordInstallation(channel: channel)
        }

        // Print banner when showing help
        if shouldShowBanner() {
            printBanner()
        }

        do {
            try await executeCommand()
            await TelemetryClient.shared.flush()
        } catch {
            await TelemetryClient.shared.flush()
            exit(withError: error)
        }
    }

    private static func isTelemetryManagementInvocation() -> Bool {
        let args = Array(CommandLine.arguments.dropFirst())
        return args.count >= 2 && args[0] == "config" && args[1] == "telemetry"
    }

    private static func writeTelemetryNoticeToStandardError() {
        let notice = """
            Telemetry: enabled by default; Lume collects pseudonymous install and bounded usage metadata only.
              No prompts, VM/image names, file paths, or VM contents are collected.
              Disable persistently at any time: lume config telemetry disable

            """
        if let data = notice.data(using: .utf8) {
            FileHandle.standardError.write(data)
        }
    }

    private static func executeCommand() async throws {
        var command = try parseAsRoot()

        if var asyncCommand = command as? AsyncParsableCommand {
            try await asyncCommand.run()
        } else {
            try command.run()
        }
    }
}
