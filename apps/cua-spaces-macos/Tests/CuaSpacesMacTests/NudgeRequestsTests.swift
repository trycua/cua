// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation
import Testing
@testable import CuaSpacesMacKit

struct NudgeRequestsTests {
    func home() throws -> URL {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("nudges-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir.appendingPathComponent("nudges"), withIntermediateDirectories: true)
        return dir
    }

    func write(_ home: URL, _ id: String, expiresMs: Double) throws {
        let body: [String: Any] = ["id": id, "agent": "Claude Code", "action": "empty the Trash", "atMs": 1, "expiresMs": expiresMs]
        try JSONSerialization.data(withJSONObject: body)
            .write(to: home.appendingPathComponent("nudges/\(id).json"))
    }

    @Test func listsWaitingRequestsAndWritesTheAnswer() throws {
        let home = try home()
        defer { try? FileManager.default.removeItem(at: home) }
        let requests = NudgeRequests(home: home)
        try write(home, "risky-1-100", expiresMs: 5_000)
        try write(home, "risky-2-200", expiresMs: 500) // past its deadline
        let pending = requests.pending(nowMs: 1_000)
        #expect(pending.map { $0["id"] as? String } == ["risky-1-100"])
        #expect(pending.first?["action"] as? String == "empty the Trash")

        try requests.answer(id: "risky-1-100", choice: "separate")
        let answer = try String(contentsOf: home.appendingPathComponent("nudges/risky-1-100.answer"), encoding: .utf8)
        #expect(answer == "separate")
        #expect(requests.pending(nowMs: 1_000).isEmpty)
    }

    @Test func leavesAnythingElseAlone() throws {
        let home = try home()
        defer { try? FileManager.default.removeItem(at: home) }
        let requests = NudgeRequests(home: home)
        // No request with this id: nothing is written.
        try requests.answer(id: "risky-3-300", choice: "here")
        #expect(!FileManager.default.fileExists(atPath: home.appendingPathComponent("nudges/risky-3-300.answer").path))
        // An id that names a path, or an unknown choice, is refused.
        #expect(!NudgeRequests.validId("../settings"))
        try write(home, "risky-4-400", expiresMs: 5_000)
        try requests.answer(id: "risky-4-400", choice: "delete")
        #expect(!FileManager.default.fileExists(atPath: home.appendingPathComponent("nudges/risky-4-400.answer").path))
    }
}
