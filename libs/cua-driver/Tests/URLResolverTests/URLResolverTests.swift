import XCTest
@testable import CuaDriverCore

/// Regression tests for `resolveLaunchURL(_:)`.
///
/// Covers the CJK / non-ASCII crash reported in:
/// https://github.com/trycua/cua/issues/1519
final class URLResolverTests: XCTestCase {

    // MARK: - CJK / non-ASCII paths (the bug)

    func testCJKAbsolutePath() throws {
        let url = try XCTUnwrap(resolveLaunchURL("/Users/me/器材控/templates/file.key"))
        XCTAssertEqual(url.scheme, "file")
        // Path must be percent-encoded — no raw CJK bytes in the URL string.
        XCTAssertFalse(url.absoluteString.contains("器材控"),
            "CJK characters must be percent-encoded in the URL string")
        // But the decoded path must round-trip back to the original.
        XCTAssertEqual(url.path, "/Users/me/器材控/templates/file.key")
    }

    func testCyrillicPath() throws {
        let url = try XCTUnwrap(resolveLaunchURL("/Users/me/Документы/report.pdf"))
        XCTAssertEqual(url.scheme, "file")
        XCTAssertEqual(url.path, "/Users/me/Документы/report.pdf")
    }

    func testAccentedLatinPath() throws {
        let url = try XCTUnwrap(resolveLaunchURL("/Users/héloïse/Documents/résumé.pdf"))
        XCTAssertEqual(url.scheme, "file")
        XCTAssertEqual(url.path, "/Users/héloïse/Documents/résumé.pdf")
    }

    func testEmojiPath() throws {
        let url = try XCTUnwrap(resolveLaunchURL("/Users/me/🎵Music/track.mp3"))
        XCTAssertEqual(url.scheme, "file")
        XCTAssertEqual(url.path, "/Users/me/🎵Music/track.mp3")
    }

    // MARK: - ASCII paths (regression guard)

    func testASCIIAbsolutePath() throws {
        let url = try XCTUnwrap(resolveLaunchURL("/Users/me/Documents/file.pdf"))
        XCTAssertEqual(url.scheme, "file")
        XCTAssertEqual(url.path, "/Users/me/Documents/file.pdf")
    }

    func testTildePath() throws {
        let url = try XCTUnwrap(resolveLaunchURL("~/Downloads/file.zip"))
        XCTAssertEqual(url.scheme, "file")
        XCTAssertFalse(url.path.hasPrefix("~"), "Tilde must be expanded")
        XCTAssertTrue(url.path.hasSuffix("/Downloads/file.zip"))
    }

    // MARK: - Explicit file:// URLs

    func testFileURLPassthrough() throws {
        let url = try XCTUnwrap(resolveLaunchURL("file:///Users/me/file.pdf"))
        XCTAssertEqual(url.scheme, "file")
        XCTAssertEqual(url.path, "/Users/me/file.pdf")
    }

    func testFileURLWithCJK() throws {
        // file:// URL with percent-encoded CJK — must round-trip correctly.
        let encoded = "file:///Users/me/%E5%99%A8%E6%9D%90%E6%8E%A7/file.key"
        let url = try XCTUnwrap(resolveLaunchURL(encoded))
        XCTAssertEqual(url.scheme, "file")
        XCTAssertEqual(url.path, "/Users/me/器材控/file.key")
    }

    // MARK: - HTTP / HTTPS URLs

    func testHTTPURL() throws {
        let url = try XCTUnwrap(resolveLaunchURL("https://example.com/path"))
        XCTAssertEqual(url.scheme, "https")
        XCTAssertEqual(url.host, "example.com")
    }

    func testHTTPURLWithNonASCII() throws {
        // Non-ASCII in query string — must not crash or return nil.
        let url = try XCTUnwrap(resolveLaunchURL("https://example.com/search?q=器材控"))
        XCTAssertEqual(url.scheme, "https")
    }

    // MARK: - Edge cases

    func testEmptyStringReturnsNil() {
        XCTAssertNil(resolveLaunchURL(""))
    }

    func testAboutBlank() throws {
        // about:blank is used by browsers — must preserve scheme semantics,
        // not fall through to the file-path branch.
        let url = try XCTUnwrap(resolveLaunchURL("about:blank"))
        XCTAssertEqual(url.scheme, "about")
        XCTAssertEqual(url.absoluteString, "about:blank")
    }

    func testMailtoScheme() throws {
        // Other single-colon schemes must also pass through as-is.
        let url = try XCTUnwrap(resolveLaunchURL("mailto:user@example.com"))
        XCTAssertEqual(url.scheme, "mailto")
    }
}
