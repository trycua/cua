// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

@testable import CuaSpacesMacKit
import Foundation
import Testing

@Suite("Sparkle cache")
struct SparkleCacheTests {
    private func make() throws -> (caches: URL, folder: URL) {
        let caches = FileManager.default.temporaryDirectory.appendingPathComponent("sparkle-\(UUID().uuidString)")
        let folder = SparkleCache.folders(bundleID: "com.example.app", caches: caches)[0]
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        return (caches, folder)
    }

    private func put(_ name: String, in folder: URL, bytes: Int, age: TimeInterval) throws {
        let url = folder.appendingPathComponent(name)
        try Data(count: bytes).write(to: url)
        try FileManager.default.setAttributes([.modificationDate: Date().addingTimeInterval(-age)], ofItemAtPath: url.path)
    }

    @Test func anUpdateEmptiesIt() throws {
        let (caches, folder) = try make()
        defer { try? FileManager.default.removeItem(at: caches) }
        try put("a.zip", in: folder, bytes: 1000, age: 60)
        let freed = SparkleCache.trim(bundleID: "com.example.app", caches: caches, afterUpdate: true)
        #expect(freed == 1000)
        #expect(try FileManager.default.contentsOfDirectory(atPath: folder.path).isEmpty)
    }

    @Test func oldEntriesGoAndTheNewestSurvivesTheCap() throws {
        let (caches, folder) = try make()
        defer { try? FileManager.default.removeItem(at: caches) }
        try put("old.zip", in: folder, bytes: 500, age: 30 * 86400)
        try put("mid.zip", in: folder, bytes: 600, age: 3600)
        try put("new.zip", in: folder, bytes: 700, age: 60)
        SparkleCache.trim(bundleID: "com.example.app", caches: caches, afterUpdate: false, maxBytes: 800)
        let left = Set(try FileManager.default.contentsOfDirectory(atPath: folder.path))
        #expect(left == ["new.zip"])
        // Alone and over the cap it still stays: it may be waiting to install.
        SparkleCache.trim(bundleID: "com.example.app", caches: caches, afterUpdate: false, maxBytes: 1)
        #expect(try FileManager.default.contentsOfDirectory(atPath: folder.path) == ["new.zip"])
    }

    @Test func aMissingFolderIsFine() {
        let caches = FileManager.default.temporaryDirectory.appendingPathComponent("none-\(UUID().uuidString)")
        #expect(SparkleCache.trim(bundleID: "x", caches: caches, afterUpdate: true) == 0)
    }
}
