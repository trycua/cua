// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import CuaSpacesFFI
@testable import CuaSpacesMacKit
import Foundation
import Testing

/// Settings → Permissions over a throwaway policy and a fake approval
/// (no Touch ID, never the real Cua home).
@MainActor
@Suite("Permissions")
struct ApprovalsTests {
    func model(accept: Bool = true) -> ApprovalsModel {
        let home = FileManager.default.temporaryDirectory
            .appendingPathComponent("cua-approvals-test-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
        return .fixture(home: home.path, accept: accept)
    }

    @Test func showsTheCoresWordsAndLockedGatesWithoutToggles() {
        let m = model()
        #expect(m.view.intro == "Choose what an agent must ask you for. Changes need Touch ID.")
        #expect(m.view.rows.count == 9)
        #expect(m.view.notice == nil)
        #expect(m.view.rows.allSatisfy { !$0.title.isEmpty && !$0.detail.isEmpty })
        #expect(m.view.lockedTitle == "Always asks")
        #expect(!m.view.locked.isEmpty)
    }

    @Test func anAcceptedChangeSticks() async {
        let m = model()
        let row = m.view.rows.first { $0.id == "cloud" }!
        #expect(row.require)
        await m.set(row, to: false)
        #expect(m.view.rows.first { $0.id == "cloud" }?.require == false)
        #expect(m.pending.isEmpty)
        #expect(m.error == nil)
    }

    @Test func aDeclinedChangeRevertsAndSaysWhy() async {
        let m = model(accept: false)
        let row = m.view.rows.first { $0.id == "cloud" }!
        await m.set(row, to: false)
        #expect(m.view.rows.first { $0.id == "cloud" }?.require == true)
        #expect(m.requires(row))
        #expect(m.pending.isEmpty)
        #expect(m.error != nil)
    }

    @Test func aSettingsFileEditedOutsideCuaAsksForEverythingAndSaysSo() async throws {
        let home = FileManager.default.temporaryDirectory
            .appendingPathComponent("cua-approvals-test-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
        let m = ApprovalsModel.fixture(home: home.path)
        #expect(m.view.notice == nil)
        await m.set(m.view.rows.first { $0.id == "cloud" }!, to: false)
        // A process of the same user loosens another row by hand.
        let file = home.appendingPathComponent("approvals.json")
        let text = try String(contentsOf: file, encoding: .utf8)
        try text.replacingOccurrences(of: "\"host_files\": true", with: "\"host_files\": false")
            .write(to: file, atomically: true, encoding: .utf8)
        m.reload()
        #expect(m.view.notice != nil)
        #expect(m.view.rows.allSatisfy { $0.require }, "everything asks")
        // Reviewing one row (Touch ID) makes the settings the user's again.
        await m.set(m.view.rows.first { $0.id == "network" }!, to: false)
        #expect(m.view.notice == nil)
        #expect(m.view.rows.first { $0.id == "host_files" }?.require == true)
    }
}
