// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// XCTest's assertion vocabulary on swift-testing.
//
// These suites were written against XCTest, which only ships with Xcode; the
// Command Line Tools (and Linux CI images) carry swift-testing. The shim keeps
// every assertion as written, reports through `Issue.record` at the caller's
// line, and lets each suite be a `@Suite` whose `@Test` methods swift-testing
// discovers.
import Foundation
import Testing

private func loc(_ file: StaticString, _ line: UInt) -> SourceLocation {
    SourceLocation(fileID: "\(file)", filePath: "\(file)", line: Int(line), column: 1)
}

private func fail(_ what: String, _ message: String, _ file: StaticString, _ line: UInt) {
    Issue.record(Comment(rawValue: message.isEmpty ? what : "\(what): \(message)"),
                 sourceLocation: loc(file, line))
}

struct XCTSkipError: Error { let message: String }

func XCTFail(_ message: String = "", file: StaticString = #filePath, line: UInt = #line) {
    fail("failed", message, file, line)
}

func XCTAssertTrue(_ e: @autoclosure () throws -> Bool, _ message: @autoclosure () -> String = "",
                   file: StaticString = #filePath, line: UInt = #line) {
    do { if try !e() { fail("expected true", message(), file, line) } }
    catch { fail("threw \(error)", message(), file, line) }
}

func XCTAssertFalse(_ e: @autoclosure () throws -> Bool, _ message: @autoclosure () -> String = "",
                    file: StaticString = #filePath, line: UInt = #line) {
    do { if try e() { fail("expected false", message(), file, line) } }
    catch { fail("threw \(error)", message(), file, line) }
}

func XCTAssertEqual<T: Equatable>(_ a: @autoclosure () throws -> T, _ b: @autoclosure () throws -> T,
                                  _ message: @autoclosure () -> String = "",
                                  file: StaticString = #filePath, line: UInt = #line) {
    do {
        let (x, y) = (try a(), try b())
        if x != y { fail("\(x) != \(y)", message(), file, line) }
    } catch { fail("threw \(error)", message(), file, line) }
}

func XCTAssertNotEqual<T: Equatable>(_ a: @autoclosure () throws -> T, _ b: @autoclosure () throws -> T,
                                     _ message: @autoclosure () -> String = "",
                                     file: StaticString = #filePath, line: UInt = #line) {
    do {
        let (x, y) = (try a(), try b())
        if x == y { fail("\(x) == \(y)", message(), file, line) }
    } catch { fail("threw \(error)", message(), file, line) }
}

func XCTAssertGreaterThan<T: Comparable>(_ a: @autoclosure () throws -> T, _ b: @autoclosure () throws -> T,
                                         _ message: @autoclosure () -> String = "",
                                         file: StaticString = #filePath, line: UInt = #line) {
    do {
        let (x, y) = (try a(), try b())
        if !(x > y) { fail("\(x) <= \(y)", message(), file, line) }
    } catch { fail("threw \(error)", message(), file, line) }
}

func XCTAssertLessThan<T: Comparable>(_ a: @autoclosure () throws -> T, _ b: @autoclosure () throws -> T,
                                      _ message: @autoclosure () -> String = "",
                                      file: StaticString = #filePath, line: UInt = #line) {
    do {
        let (x, y) = (try a(), try b())
        if !(x < y) { fail("\(x) >= \(y)", message(), file, line) }
    } catch { fail("threw \(error)", message(), file, line) }
}

func XCTAssertNil(_ e: @autoclosure () throws -> Any?, _ message: @autoclosure () -> String = "",
                  file: StaticString = #filePath, line: UInt = #line) {
    do { if let v = try e() { fail("expected nil, got \(v)", message(), file, line) } }
    catch { fail("threw \(error)", message(), file, line) }
}

func XCTAssertNotNil(_ e: @autoclosure () throws -> Any?, _ message: @autoclosure () -> String = "",
                     file: StaticString = #filePath, line: UInt = #line) {
    do { if try e() == nil { fail("expected non-nil", message(), file, line) } }
    catch { fail("threw \(error)", message(), file, line) }
}

func XCTUnwrap<T>(_ e: @autoclosure () throws -> T?, _ message: @autoclosure () -> String = "",
                  file: StaticString = #filePath, line: UInt = #line) throws -> T {
    guard let v = try e() else {
        fail("unexpected nil", message(), file, line)
        throw XCTSkipError(message: "XCTUnwrap found nil")
    }
    return v
}

func XCTAssertThrowsError<T>(_ e: @autoclosure () throws -> T, _ message: @autoclosure () -> String = "",
                             file: StaticString = #filePath, line: UInt = #line,
                             _ handler: (Error) -> Void = { _ in }) {
    do {
        _ = try e()
        fail("expected an error", message(), file, line)
    } catch {
        handler(error)
    }
}
