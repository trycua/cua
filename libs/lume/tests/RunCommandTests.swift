import Testing

@testable import lume

@MainActor
@Test("run parses repeated additional disk options")
func runCommandParsesAdditionalDisks() throws {
    let root = try Lume.parseAsRoot([
        "run",
        "test-vm",
        "--disk",
        "scratch.img",
        "--disk=cache.img",
    ])
    let command = try #require(root as? Run)

    #expect(command.name == "test-vm")
    #expect(command.additionalDisks == ["scratch.img", "cache.img"])
}
