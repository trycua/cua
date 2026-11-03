import ArgumentParser
import Foundation
import Testing

@testable import lume

@Test("Stop defaults to a graceful shutdown with the standard timeout")
func stopCommandDefaults() throws {
    let command = try Stop.parse(["test-vm"])

    #expect(command.name == "test-vm")
    #expect(command.force == false)
    #expect(command.timeout == 10)
    #expect(command.storage == nil)
}

@Test("Stop parses force, timeout, and storage")
func stopCommandParsesForceAndTimeout() throws {
    let command = try Stop.parse([
        "test-vm", "--force", "--timeout", "30", "--storage", "external",
    ])

    #expect(command.force)
    #expect(command.timeout == 30)
    #expect(command.storage == "external")
}

@Test("Stop rejects a negative timeout")
func stopCommandRejectsNegativeTimeout() throws {
    #expect(throws: Error.self) {
        _ = try Stop.parse(["test-vm", "--timeout=-1"])
    }
}
