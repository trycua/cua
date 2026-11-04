// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation
import Testing

// The suites were written against XCTest. XCTest ships only with Xcode, and
// the cua SDK is built and tested with the Command Line Tools alone, so every
// suite now runs on swift-testing (`@Suite` / `@Test`). These are the
// XCTest assertion names, with XCTest's semantics, recording swift-testing
// issues. Keeping the names means the assertion lines themselves did not
// change in the port; only the declarations (`@Test`, `init` for
// `setUpWithError`) did.

@usableFromInline
func _record(_ message: String, _ detail: String, fileID: String, file: StaticString, line: UInt) {
    let text = message.isEmpty ? detail : "\(detail) - \(message)"
    Issue.record(Comment(rawValue: text),
                 sourceLocation: SourceLocation(fileID: fileID, filePath: "\(file)",
                                                line: Int(line), column: 1))
}

func XCTFail(_ message: String = "", fileID: String = #fileID,
             file: StaticString = #filePath, line: UInt = #line) {
    _record(message, "failed", fileID: fileID, file: file, line: line)
}

func XCTAssertTrue(_ expression: @autoclosure () throws -> Bool,
                   _ message: @autoclosure () -> String = "",
                   fileID: String = #fileID, file: StaticString = #filePath, line: UInt = #line) {
    do {
        if try !expression() {
            _record(message(), "XCTAssertTrue failed", fileID: fileID, file: file, line: line)
        }
    } catch {
        _record(message(), "XCTAssertTrue threw \(error)", fileID: fileID, file: file, line: line)
    }
}

func XCTAssert(_ expression: @autoclosure () throws -> Bool,
               _ message: @autoclosure () -> String = "",
               fileID: String = #fileID, file: StaticString = #filePath, line: UInt = #line) {
    XCTAssertTrue(try expression(), message(), fileID: fileID, file: file, line: line)
}

func XCTAssertFalse(_ expression: @autoclosure () throws -> Bool,
                    _ message: @autoclosure () -> String = "",
                    fileID: String = #fileID, file: StaticString = #filePath, line: UInt = #line) {
    do {
        if try expression() {
            _record(message(), "XCTAssertFalse failed", fileID: fileID, file: file, line: line)
        }
    } catch {
        _record(message(), "XCTAssertFalse threw \(error)", fileID: fileID, file: file, line: line)
    }
}

func XCTAssertNil<T>(_ expression: @autoclosure () throws -> T?,
                     _ message: @autoclosure () -> String = "",
                     fileID: String = #fileID, file: StaticString = #filePath, line: UInt = #line) {
    do {
        if let v = try expression() {
            _record(message(), "XCTAssertNil failed: \"\(v)\"", fileID: fileID, file: file, line: line)
        }
    } catch {
        _record(message(), "XCTAssertNil threw \(error)", fileID: fileID, file: file, line: line)
    }
}

func XCTAssertNotNil<T>(_ expression: @autoclosure () throws -> T?,
                        _ message: @autoclosure () -> String = "",
                        fileID: String = #fileID, file: StaticString = #filePath, line: UInt = #line) {
    do {
        if try expression() == nil {
            _record(message(), "XCTAssertNotNil failed", fileID: fileID, file: file, line: line)
        }
    } catch {
        _record(message(), "XCTAssertNotNil threw \(error)", fileID: fileID, file: file, line: line)
    }
}

func XCTAssertEqual<T: Equatable>(_ a: @autoclosure () throws -> T,
                                  _ b: @autoclosure () throws -> T,
                                  _ message: @autoclosure () -> String = "",
                                  fileID: String = #fileID, file: StaticString = #filePath,
                                  line: UInt = #line) {
    do {
        let (x, y) = (try a(), try b())
        if x != y {
            _record(message(), "XCTAssertEqual failed: (\"\(x)\") is not equal to (\"\(y)\")",
                    fileID: fileID, file: file, line: line)
        }
    } catch {
        _record(message(), "XCTAssertEqual threw \(error)", fileID: fileID, file: file, line: line)
    }
}

func XCTAssertEqual<T: FloatingPoint>(_ a: @autoclosure () throws -> T,
                                      _ b: @autoclosure () throws -> T,
                                      accuracy: T,
                                      _ message: @autoclosure () -> String = "",
                                      fileID: String = #fileID, file: StaticString = #filePath,
                                      line: UInt = #line) {
    do {
        let (x, y) = (try a(), try b())
        if !(abs(x - y) <= accuracy) {
            _record(message(), "XCTAssertEqual failed: (\"\(x)\") is not equal to (\"\(y)\") +/- (\"\(accuracy)\")",
                    fileID: fileID, file: file, line: line)
        }
    } catch {
        _record(message(), "XCTAssertEqual threw \(error)", fileID: fileID, file: file, line: line)
    }
}

func XCTAssertNotEqual<T: Equatable>(_ a: @autoclosure () throws -> T,
                                     _ b: @autoclosure () throws -> T,
                                     _ message: @autoclosure () -> String = "",
                                     fileID: String = #fileID, file: StaticString = #filePath,
                                     line: UInt = #line) {
    do {
        let (x, y) = (try a(), try b())
        if x == y {
            _record(message(), "XCTAssertNotEqual failed: (\"\(x)\") is equal to (\"\(y)\")",
                    fileID: fileID, file: file, line: line)
        }
    } catch {
        _record(message(), "XCTAssertNotEqual threw \(error)", fileID: fileID, file: file, line: line)
    }
}

private func _compare<T: Comparable>(_ name: String, _ a: () throws -> T, _ b: () throws -> T,
                                     _ holds: (T, T) -> Bool, _ op: String, _ message: String,
                                     fileID: String, file: StaticString, line: UInt) {
    do {
        let (x, y) = (try a(), try b())
        if !holds(x, y) {
            _record(message, "\(name) failed: (\"\(x)\") is not \(op) (\"\(y)\")",
                    fileID: fileID, file: file, line: line)
        }
    } catch {
        _record(message, "\(name) threw \(error)", fileID: fileID, file: file, line: line)
    }
}

func XCTAssertGreaterThan<T: Comparable>(_ a: @autoclosure () throws -> T, _ b: @autoclosure () throws -> T,
                                         _ message: @autoclosure () -> String = "",
                                         fileID: String = #fileID, file: StaticString = #filePath,
                                         line: UInt = #line) {
    _compare("XCTAssertGreaterThan", a, b, >, "greater than", message(), fileID: fileID, file: file, line: line)
}

func XCTAssertGreaterThanOrEqual<T: Comparable>(_ a: @autoclosure () throws -> T, _ b: @autoclosure () throws -> T,
                                                _ message: @autoclosure () -> String = "",
                                                fileID: String = #fileID, file: StaticString = #filePath,
                                                line: UInt = #line) {
    _compare("XCTAssertGreaterThanOrEqual", a, b, >=, "greater than or equal to", message(),
             fileID: fileID, file: file, line: line)
}

func XCTAssertLessThan<T: Comparable>(_ a: @autoclosure () throws -> T, _ b: @autoclosure () throws -> T,
                                      _ message: @autoclosure () -> String = "",
                                      fileID: String = #fileID, file: StaticString = #filePath,
                                      line: UInt = #line) {
    _compare("XCTAssertLessThan", a, b, <, "less than", message(), fileID: fileID, file: file, line: line)
}

func XCTAssertLessThanOrEqual<T: Comparable>(_ a: @autoclosure () throws -> T, _ b: @autoclosure () throws -> T,
                                             _ message: @autoclosure () -> String = "",
                                             fileID: String = #fileID, file: StaticString = #filePath,
                                             line: UInt = #line) {
    _compare("XCTAssertLessThanOrEqual", a, b, <=, "less than or equal to", message(),
             fileID: fileID, file: file, line: line)
}

func XCTAssertThrowsError<T>(_ expression: @autoclosure () throws -> T,
                             _ message: @autoclosure () -> String = "",
                             fileID: String = #fileID, file: StaticString = #filePath, line: UInt = #line,
                             _ errorHandler: (_ error: Error) -> Void = { _ in }) {
    do {
        _ = try expression()
        _record(message(), "XCTAssertThrowsError failed: did not throw an error",
                fileID: fileID, file: file, line: line)
    } catch {
        errorHandler(error)
    }
}

struct XCTUnwrapFailure: Error, CustomStringConvertible {
    let description: String
}

func XCTUnwrap<T>(_ expression: @autoclosure () throws -> T?,
                  _ message: @autoclosure () -> String = "",
                  fileID: String = #fileID, file: StaticString = #filePath, line: UInt = #line) throws -> T {
    if let v = try expression() { return v }
    _record(message(), "XCTUnwrap failed: expected non-nil value of type \"\(T.self)\"",
            fileID: fileID, file: file, line: line)
    throw XCTUnwrapFailure(description: "XCTUnwrap failed: \(message())")
}

/// XCTest's `throw XCTSkip(…)`: the test stops and is reported as skipped
/// (swift-testing: cancelled), not failed.
func XCTSkipNow(_ message: String) throws -> Never {
    try Test.cancel(Comment(rawValue: message))
}

func XCTSkipUnless(_ condition: @autoclosure () throws -> Bool, _ message: @autoclosure () -> String = "") throws {
    if try !condition() { try XCTSkipNow(message()) }
}

func XCTSkipIf(_ condition: @autoclosure () throws -> Bool, _ message: @autoclosure () -> String = "") throws {
    if try condition() { try XCTSkipNow(message()) }
}
