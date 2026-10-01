// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

@testable import CuaSpacesMacKit
import Foundation
import Testing

/// The app finds its resources wherever it is installed: SwiftPM's
/// `Bundle.module` only looks at the bundle root and the build machine's
/// absolute path, so the built app crashed on every other Mac.
@Suite("Resource lookup")
struct ResourceLookupTests {
    @Test func theResourcesLoad() {
        #expect(ModuleResources.bundle != nil)
        #expect(StackMark.image != nil)
        #expect(ModuleResources.url(forResource: "tray-template@2x", withExtension: "png") != nil)
    }

    /// An app bundle's `Contents/Resources` is searched first: that is where
    /// `scripts/build-app.sh` installs the resource bundle.
    @Test func anAppLooksInContentsResourcesFirst() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("cua-resource-lookup-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let app = root.appendingPathComponent("Relocated.app", isDirectory: true)
        let macos = app.appendingPathComponent("Contents/MacOS", isDirectory: true)
        try FileManager.default.createDirectory(at: macos, withIntermediateDirectories: true)
        try FileManager.default.createDirectory(
            at: app.appendingPathComponent("Contents/Resources", isDirectory: true), withIntermediateDirectories: true)
        try Data("<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><dict><key>CFBundleExecutable</key><string>X</string><key>CFBundleIdentifier</key><string>test.relocated</string><key>CFBundlePackageType</key><string>APPL</string></dict></plist>".utf8)
            .write(to: app.appendingPathComponent("Contents/Info.plist"))
        let main = try #require(Bundle(url: app))
        let first = try #require(ModuleResources.candidates(main: main).first)
        #expect(first.deletingLastPathComponent().standardizedFileURL
            == app.appendingPathComponent("Contents/Resources", isDirectory: true).standardizedFileURL)
        #expect(first.lastPathComponent == ModuleResources.bundleName)
    }

    /// No source uses the generated accessor (comments may name it).
    @Test func noSourceUsesBundleModule() throws {
        let sources = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("Sources", isDirectory: true)
        let files = try #require(FileManager.default.enumerator(at: sources, includingPropertiesForKeys: nil))
            .compactMap { $0 as? URL }.filter { $0.pathExtension == "swift" }
        #expect(!files.isEmpty)
        var offenders: [String] = []
        for file in files {
            let text = try String(contentsOf: file, encoding: .utf8)
            for (n, line) in text.split(separator: "\n", omittingEmptySubsequences: false).enumerated() {
                let code = line.components(separatedBy: "//").first ?? ""
                if code.contains("Bundle.module") || code.contains("bundle: .module") {
                    offenders.append("\(file.lastPathComponent):\(n + 1)")
                }
            }
        }
        #expect(offenders.isEmpty, "use ModuleResources, not Bundle.module: \(offenders)")
    }
}
