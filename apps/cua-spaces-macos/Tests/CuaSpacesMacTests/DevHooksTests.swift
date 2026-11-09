// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

@testable import CuaSpacesMacKit
import Foundation
import Testing

/// The recording and test hooks are debug-only: a release build reads no
/// `CUA_SPACES_*` variable, and every read goes through `DevHooks`.
@Suite("Dev hooks")
struct DevHooksTests {
    static let sample = [
        "CUA_SPACES_BROWSER": "/Applications/Other.app",
        "CUA_SPACES_START_VIEW": "keyvault",
        "CUA_SPACES_ACTIVATE": "1",
        "HOME": "/tmp/home",
    ]

    @Test func releaseBuildsReadNoHooks() {
        #expect(DevHooks.filter(Self.sample, enabled: false).isEmpty)
    }

    @Test func debugBuildsReadOnlyCuaSpacesHooks() {
        let hooks = DevHooks.filter(Self.sample, enabled: true)
        #expect(Set(hooks.keys) == ["CUA_SPACES_BROWSER", "CUA_SPACES_START_VIEW", "CUA_SPACES_ACTIVATE"])
    }

    @Test func theGateFollowsTheBuildConfiguration() {
        #if DEBUG
        #expect(DevHooks.enabled)
        #else
        #expect(!DevHooks.enabled)
        #endif
    }

    /// No source reads a `CUA_SPACES_*` variable except through `DevHooks`,
    /// and the gate itself is compiled out of release builds.
    @Test func everyHookReadGoesThroughTheGate() throws {
        let sources = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("Sources", isDirectory: true)
        let files = try #require(FileManager.default.enumerator(at: sources, includingPropertiesForKeys: nil))
            .compactMap { $0 as? URL }.filter { $0.pathExtension == "swift" }
        #expect(!files.isEmpty)
        var offenders: [String] = []
        for file in files {
            let text = try String(contentsOf: file, encoding: .utf8)
            if file.lastPathComponent == "DevHooks.swift" {
                #expect(text.contains("#if DEBUG"))
                continue
            }
            for (n, line) in text.split(separator: "\n", omittingEmptySubsequences: false).enumerated() {
                let code = line.components(separatedBy: "//").first ?? ""
                let rawRead = code.contains("processInfo.environment") && code.contains("CUA_SPACES_")
                let getenvRead = code.contains("getenv(") && code.contains("CUA_SPACES_")
                if rawRead || getenvRead { offenders.append("\(file.lastPathComponent):\(n + 1)") }
            }
            // A whole-environment copy may only feed `CUA_SPACES_*` reads
            // when it is the gated one.
            if text.contains("= ProcessInfo.processInfo.environment") {
                let lines = text.split(separator: "\n").map(String.init)
                for (i, line) in lines.enumerated() where line.contains("= ProcessInfo.processInfo.environment") {
                    let name = line.components(separatedBy: "let ").last?
                        .components(separatedBy: " =").first?.trimmingCharacters(in: .whitespaces) ?? ""
                    let scope = lines[i..<min(lines.count, i + 40)].joined(separator: "\n")
                    if !name.isEmpty, scope.contains("\(name)[\"CUA_SPACES_") {
                        offenders.append("\(file.lastPathComponent):\(i + 1)")
                    }
                }
            }
        }
        #expect(offenders.isEmpty, "ungated CUA_SPACES_* reads: \(offenders)")
    }
}
