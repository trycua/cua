import Testing

@testable import lume

@Test("CLI reference covers every registered command")
func commandDocumentationCoversRegistry() {
    let coverage = CommandDocExtractor.documentationCoverage

    #expect(coverage.missing.isEmpty)
    #expect(coverage.extra.isEmpty)
    let commands = CommandDocExtractor.extractAll().commands
    #expect(commands.contains { $0.name == "sip" })

    let importDoc = commands.first { $0.name == "import" }
    let exportDoc = commands.first { $0.name == "export" }
    #expect(importDoc?.subcommands.contains { $0.name == "utm" } == true)
    #expect(exportDoc?.subcommands.contains { $0.name == "utm" } == true)
    #expect(!commands.contains { $0.name == "import-utm" || $0.name == "export-utm" })
}
