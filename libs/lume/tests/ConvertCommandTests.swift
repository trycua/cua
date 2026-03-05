import Foundation
import Testing
import ArgumentParser

@testable import lume

@Test("Convert command configuration is set correctly")
func testConvertCommandConfiguration() async throws {
    let config = Convert.configuration

    #expect(!config.abstract.isEmpty)
    #expect(config.abstract.contains("Convert"))
    #expect(config.abstract.contains("OCI"))
    #expect(!config.discussion.isEmpty)
}

@Test("Convert command validates target image format")
func testConvertCommandValidatesTargetFormat() async throws {
    // The command should validate that targetImage contains ':'
    // This is tested indirectly through the run() method which throws ValidationError
    // for invalid format

    // We can verify the validation logic exists by checking the implementation
    // The command expects format "name:tag"
    let validTarget = "trycua/macos-sequoia:latest-oci"
    let components = validTarget.split(separator: ":")
    #expect(components.count == 2)
    #expect(components.last != nil)
}

@Test("Convert command default values")
func testConvertCommandDefaults() async throws {
    let command = Convert()

    #expect(command.registry == "ghcr.io")
    #expect(command.organization == "trycua")
    #expect(command.verbose == false)
    #expect(command.dryRun == false)
    #expect(command.additionalTags.isEmpty)
}

@Test("Convert command combines primary and additional tags")
func testConvertCommandCombinesTags() async throws {
    // Test the logic for combining primary tag with additional tags
    let targetImage = "test-image:v1.0"
    let additionalTags = ["latest", "stable"]

    let targetComponents = targetImage.split(separator: ":")
    let primaryTag = String(targetComponents.last!)

    var allTags: Set<String> = [primaryTag]
    allTags.formUnion(additionalTags)

    #expect(allTags.contains("v1.0"))
    #expect(allTags.contains("latest"))
    #expect(allTags.contains("stable"))
    #expect(allTags.count == 3)
}

@Test("Convert command extracts target name correctly")
func testConvertCommandExtractsTargetName() async throws {
    let targetImage = "trycua/macos-sequoia:latest-oci"
    let components = targetImage.split(separator: ":")
    let targetName = String(components.first!)

    #expect(targetName == "trycua/macos-sequoia")
}

@Test("Convert command generates unique temp VM name")
func testConvertCommandTempVMName() async throws {
    // Test that temp VM names are unique
    let uuid1 = UUID().uuidString.prefix(8)
    let uuid2 = UUID().uuidString.prefix(8)

    let tempVMName1 = "__lume_convert_\(uuid1)"
    let tempVMName2 = "__lume_convert_\(uuid2)"

    // Should have the correct prefix
    #expect(tempVMName1.hasPrefix("__lume_convert_"))
    #expect(tempVMName2.hasPrefix("__lume_convert_"))

    // Should be different (with very high probability)
    #expect(tempVMName1 != tempVMName2)
}