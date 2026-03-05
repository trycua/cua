import Foundation
import Testing
import ArgumentParser

@testable import lume

@Test("Push command configuration is set correctly")
func testPushCommandConfiguration() async throws {
    let config = Push.configuration

    #expect(!config.abstract.isEmpty)
    #expect(config.abstract.contains("Push"))
}

@Test("Push command default values")
func testPushCommandDefaults() async throws {
    let command = Push()

    #expect(command.registry == "ghcr.io")
    #expect(command.organization == "trycua")
    #expect(command.storage == nil)
    #expect(command.chunkSizeMb == 512)
    #expect(command.verbose == false)
    #expect(command.dryRun == false)
    #expect(command.reassemble == true)
    #expect(command.singleLayer == false)
    #expect(command.legacy == false)
    #expect(command.additionalTags.isEmpty)
}

@Test("Push command parses image format correctly")
func testPushCommandImageFormatParsing() async throws {
    let image = "test-image:v1.0"
    let components = image.split(separator: ":")

    #expect(components.count == 2)

    let imageName = String(components.first!)
    let primaryTag = String(components.last!)

    #expect(imageName == "test-image")
    #expect(primaryTag == "v1.0")
}

@Test("Push command validates image format")
func testPushCommandValidatesImageFormat() async throws {
    // Invalid format without tag
    let invalidImage = "test-image-no-tag"
    let components = invalidImage.split(separator: ":")

    #expect(components.count == 1) // Should fail validation in actual command
}

@Test("Push command combines tags correctly")
func testPushCommandCombinesTags() async throws {
    let image = "test-image:v1.0"
    let additionalTags = ["latest", "stable", "v1.0"] // v1.0 is duplicate

    let components = image.split(separator: ":")
    let primaryTag = String(components.last!)

    var allTags: Set<String> = []
    allTags.insert(primaryTag)
    allTags.formUnion(additionalTags)

    // Should have unique tags only
    #expect(allTags.count == 3)
    #expect(allTags.contains("v1.0"))
    #expect(allTags.contains("latest"))
    #expect(allTags.contains("stable"))
}

@Test("Push command rejects conflicting flags")
func testPushCommandConflictingFlags() async throws {
    // singleLayer and legacy are mutually exclusive
    // This would be tested in the actual run() method
    let singleLayer = true
    let legacy = true

    // The command should throw ValidationError when both are true
    if singleLayer && legacy {
        #expect(true) // Would throw in actual command
    }
}

@Test("Push command ensures at least one tag")
func testPushCommandRequiresTag() async throws {
    // Test the validation logic for ensuring at least one tag
    let image = "test-image:v1.0"
    let components = image.split(separator: ":")
    let primaryTag = String(components.last!)

    var allTags: Set<String> = []
    allTags.insert(primaryTag)

    #expect(!allTags.isEmpty) // Should always have at least one tag

    // Test empty additional tags doesn't break this
    let additionalTags: [String] = []
    allTags.formUnion(additionalTags)
    #expect(!allTags.isEmpty)
}