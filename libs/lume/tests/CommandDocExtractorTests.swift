import Testing

@testable import lume

@Test("CLI reference covers every registered command")
func commandDocumentationCoversRegistry() {
    let coverage = CommandDocExtractor.documentationCoverage

    #expect(coverage.missing.isEmpty)
    #expect(coverage.extra.isEmpty)
    #expect(CommandDocExtractor.extractAll().commands.contains { $0.name == "sip" })
}
