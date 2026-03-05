import Foundation
import Testing

@testable import lume

@Test("Logger filters messages below minimum level")
func testLoggerLevelFiltering() async throws {
    // Test that debug messages are filtered when min level is info
    // This test validates the log level filtering mechanism
    // Since Logger.minLevel is static and set at initialization from env var,
    // we can only test the comparison logic indirectly

    // Test level comparison
    #expect(Logger.Level.debug < Logger.Level.info)
    #expect(Logger.Level.info < Logger.Level.error)
    #expect(Logger.Level.debug < Logger.Level.error)
}

@Test("Logger level ordering is correct")
func testLoggerLevelOrdering() async throws {
    let levels: [Logger.Level] = [.debug, .info, .error]
    let sorted = levels.sorted()

    #expect(sorted[0] == .debug)
    #expect(sorted[1] == .info)
    #expect(sorted[2] == .error)
}

@Test("Logger level from string parsing")
func testLoggerLevelFromString() async throws {
    // Test that level can be created from raw value
    #expect(Logger.Level(rawValue: "debug") == .debug)
    #expect(Logger.Level(rawValue: "info") == .info)
    #expect(Logger.Level(rawValue: "error") == .error)
    #expect(Logger.Level(rawValue: "invalid") == nil)
}

@Test("Logger metadata formatting")
func testLoggerMetadataFormatting() async throws {
    // This test ensures metadata is properly formatted
    // by calling the log functions and checking they don't crash
    // (actual output goes to stdout which we can't easily capture)

    let metadata: Logger.Metadata = [
        "key1": "value1",
        "key2": "value2",
        "count": "42"
    ]

    // These should not throw or crash
    Logger.info("Test info message", metadata: metadata)
    Logger.error("Test error message", metadata: metadata)
    Logger.debug("Test debug message", metadata: metadata)
}

@Test("Logger handles empty metadata")
func testLoggerEmptyMetadata() async throws {
    // Test logging with empty metadata
    Logger.info("Test message without metadata")
    Logger.error("Error without metadata")
    Logger.debug("Debug without metadata")
}

@Test("Logger handles special characters in metadata")
func testLoggerSpecialCharacters() async throws {
    let metadata: Logger.Metadata = [
        "path": "/tmp/test file with spaces.txt",
        "message": "Error: something = bad",
        "url": "https://example.com?param=value&other=123"
    ]

    // Should handle special characters without crashing
    Logger.info("Message with special chars", metadata: metadata)
}

@Test("Logger level comparison edge cases")
func testLoggerLevelComparisonEdgeCases() async throws {
    // Test that levels are equal to themselves
    #expect(Logger.Level.debug >= .debug)
    #expect(Logger.Level.info >= .info)
    #expect(Logger.Level.error >= .error)

    // Test inequality
    #expect(Logger.Level.info > .debug)
    #expect(Logger.Level.error > .info)
}