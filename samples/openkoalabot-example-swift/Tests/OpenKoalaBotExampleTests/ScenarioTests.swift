// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CryptoKit
import Foundation
import Testing
@testable import OpenKoalaBotExample

/// The shared scenario's language-neutral inputs, pinned from Swift. The
/// Tauri and TypeScript implementations pin the same values, so a drift in
/// any generator shows up as a failing unit test rather than a guest
/// sha mismatch on a live lane.
@Suite struct ScenarioSpecTests {

    static let specURL = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        .deletingLastPathComponent()
        .appendingPathComponent("openkoalabot-example-scenario/scenario.json")

    static func spec() throws -> [String: Any] {
        try JSONSerialization.jsonObject(with: Data(contentsOf: specURL)) as? [String: Any] ?? [:]
    }

    @Test func theGeneratedFileMatchesTheSpecsSha256() throws {
        let step = try #require((try Self.spec()["steps"] as? [[String: Any]])?.first { $0["op"] as? String == "file.send" })
        let gen = try #require(step["generate"] as? [String: Any])
        let seed = try #require(UInt64((gen["seed"] as? String ?? "").replacingOccurrences(of: "0x", with: ""), radix: 16))
        let bytes = ScenarioRunner.xorshiftBytes(count: gen["bytes"] as? Int ?? 0, seed: seed)
        #expect(bytes.count == 1 << 20)
        let sha = SHA256.hash(data: bytes).map { String(format: "%02x", $0) }.joined()
        #expect(sha == step["sha256"] as? String)
    }

    @Test func theSpecCoversEveryOperationTheRunnerImplements() throws {
        let ops = (try Self.spec()["steps"] as? [[String: Any]] ?? []).compactMap { $0["op"] as? String }
        #expect(ops == ["space.open", "stream.desktop", "agent.thread", "file.send",
                        "teleport.app", "presence.pair", "routine.schedule", "group.chat",
                        "space.delete"])
    }

    /// Without a model endpoint the agent-turn steps skip, never fail.
    @Test func theModelStepsSkipWithoutAnEndpoint() throws {
        let spec = try Self.spec()
        let model = try #require(spec["model"] as? [String: Any])
        #expect(model["urlEnv"] as? String == "OPENKOALABOTS_SCENARIO_MODEL_URL")
        if ProcessInfo.processInfo.environment["OPENKOALABOTS_SCENARIO_MODEL_URL"] == nil {
            #expect(throws: ScenarioRunner.Skip.self) { try ScenarioRunner.endpoint(spec) }
        }
    }

    /// One lifecycle vocabulary: a lane adds a Space or creates one in the
    /// cloud, and the last step deletes it.
    @Test func theSpaceStepAddsOrCreatesAndTheLastStepDeletes() throws {
        let steps = try Self.spec()["steps"] as? [[String: Any]] ?? []
        let modes = try #require(steps.first?["modes"] as? [String: String])
        #expect(modes == ["fixture": "add", "docker": "add", "cloud": "create"])
        #expect(steps.last?["id"] as? String == "delete")
    }

    @Test func theRunnerRefusesToRunWithoutItsArguments() {
        #expect(ScenarioRunner.main([]) == 2)
    }

    @Test func theUsageListsTheScenario() {
        #expect(CLI.usage.contains("scenario --spec"))
    }
}
