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
        static let current: String = "0.2.24"
    }
}

// MARK: - Command Execution
extension Lume {
    public static func main() async {
        do {
            try await executeCommand()
        } catch {
            exit(withError: error)
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