// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpacesFFI
@testable import CuaSpacesMacKit
import Foundation
import Testing

/// Truncated or garbage state files must not crash the app or lose what they
/// held: the app starts from the defaults and keeps the file as
/// `<name>.damaged`, the way the core keeps settings.json.
@MainActor
@Suite("Damaged state files")
struct DamagedStateTests {
    static let garbage = ["{", "{\"completed\": tru", "", "null", "[]", "\u{0}\u{1}garbage", "\"a string\""]

    func temp(_ name: String, _ text: String) throws -> String {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("cua-damaged-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let path = dir.appendingPathComponent(name).path
        try Data(text.utf8).write(to: URL(fileURLWithPath: path))
        return path
    }

    @Test func aDamagedOnboardingFileMakesTheFirstRunDueAndIsKept() throws {
        for text in Self.garbage {
            let path = try temp("onboarding.json", text)
            let m = OnboardingModel(statePath: path)
            #expect(m.completed == false, "\(text.debugDescription)")
            #expect(String(data: FileManager.default.contents(atPath: path + ".damaged") ?? Data(), encoding: .utf8) == text)
            // Finishing writes a good file over it; the copy stays.
            m.finish(mode: .host)
            #expect(OnboardingModel(statePath: path).completed)
            #expect(FileManager.default.fileExists(atPath: path + ".damaged"))
        }
    }

    @Test func aDamagedChecklistStartsEmptyAndIsKept() throws {
        for text in Self.garbage {
            let path = try temp("checklist.json", text)
            let m = ChecklistModel(path: path)
            #expect(m.state.done.isEmpty && !m.state.dismissed, "\(text.debugDescription)")
            #expect(String(data: FileManager.default.contents(atPath: path + ".damaged") ?? Data(), encoding: .utf8) == text)
        }
    }

    @Test func aGoodOrMissingFileKeepsNothing() throws {
        let good = try temp("onboarding.json", "{\"completed\":true,\"mode\":\"host\"}")
        #expect(OnboardingModel(statePath: good).completed)
        #expect(!FileManager.default.fileExists(atPath: good + ".damaged"))
        let missing = FileManager.default.temporaryDirectory.appendingPathComponent("nope-\(UUID().uuidString)/checklist.json").path
        _ = ChecklistModel(path: missing)
        #expect(!FileManager.default.fileExists(atPath: missing + ".damaged"))
    }

    @Test func theLatestDamagedFileReplacesAnOlderCopyAndAnIdenticalOneIsNotCopiedAgain() throws {
        let path = try temp("x.json", "first")
        #expect(DamagedFile.keep(path) == path + ".damaged")
        try Data("second, longer".utf8).write(to: URL(fileURLWithPath: path))
        DamagedFile.keep(path)
        #expect(String(data: FileManager.default.contents(atPath: path + ".damaged") ?? Data(), encoding: .utf8) == "second, longer")
        #expect(DamagedFile.keep(path + "-missing") == nil)
    }
}
