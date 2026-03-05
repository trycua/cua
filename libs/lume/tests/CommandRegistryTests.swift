import Foundation
import Testing
import ArgumentParser

@testable import lume

@Test("CommandRegistry contains all expected commands")
func testCommandRegistryContainsAllCommands() async throws {
    let commands = CommandRegistry.allCommands

    // Verify minimum expected number of commands
    #expect(commands.count >= 16)

    // Verify specific commands are present
    let commandNames = commands.map { String(describing: $0) }

    #expect(commandNames.contains("Create"))
    #expect(commandNames.contains("Pull"))
    #expect(commandNames.contains("Push"))
    #expect(commandNames.contains("Convert"))
    #expect(commandNames.contains("Images"))
    #expect(commandNames.contains("Clone"))
    #expect(commandNames.contains("Get"))
    #expect(commandNames.contains("Set"))
    #expect(commandNames.contains("List"))
    #expect(commandNames.contains("Run"))
    #expect(commandNames.contains("Stop"))
    #expect(commandNames.contains("SSH"))
    #expect(commandNames.contains("IPSW"))
    #expect(commandNames.contains("Serve"))
    #expect(commandNames.contains("Delete"))
    #expect(commandNames.contains("Prune"))
    #expect(commandNames.contains("Config"))
    #expect(commandNames.contains("Logs"))
    #expect(commandNames.contains("Setup"))
    #expect(commandNames.contains("DumpDocs"))
}

@Test("CommandRegistry commands are ParsableCommand types")
func testCommandRegistryCommandsAreParsable() async throws {
    let commands = CommandRegistry.allCommands

    // All commands should conform to ParsableCommand
    for command in commands {
        #expect(command is ParsableCommand.Type)
    }
}

@Test("CommandRegistry contains no duplicates")
func testCommandRegistryNoDuplicates() async throws {
    let commands = CommandRegistry.allCommands
    let commandNames = commands.map { String(describing: $0) }

    // Check for uniqueness
    let uniqueNames = Set(commandNames)
    #expect(uniqueNames.count == commandNames.count, "CommandRegistry contains duplicate commands")
}

@Test("CommandRegistry commands have configuration")
func testCommandRegistryCommandsHaveConfiguration() async throws {
    let commands = CommandRegistry.allCommands

    // Each command should have a configuration
    for command in commands {
        let config = command.configuration
        // Abstract should not be empty (it's the command description)
        #expect(!config.abstract.isEmpty, "Command \(command) has empty abstract")
    }
}