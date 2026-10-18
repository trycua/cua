// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
@testable import CuaSpacesMacKit
import Foundation
import Testing

@Suite("Join with a code")
struct JoinWithCodeTests {
    @Test func normalizesCodes() {
        #expect(JoinWithCode.normalizeCode("abcd-efgh-jkmn") == "ABCD-EFGH-JKMN")
        #expect(JoinWithCode.normalizeCode(" 0123 4567 89ab ") == "0123-4567-89AB")
        #expect(JoinWithCode.normalizeCode("ABCDEFGHJKMN") == "ABCD-EFGH-JKMN")
        #expect(JoinWithCode.normalizeCode("ABCD-EFGH-JKM") == nil)
        #expect(JoinWithCode.normalizeCode("ABCD-EFGH-JKMNP") == nil)
        #expect(JoinWithCode.normalizeCode("ABCD-EFGH-JKMI") == nil, "I is not in the alphabet")
        #expect(JoinWithCode.normalizeCode("ABCD-EFGH-JKMU") == nil, "U is not in the alphabet")
        #expect(JoinWithCode.normalizeCode("") == nil)
    }

    @Test func buildsArguments() {
        #expect(JoinWithCode.arguments(code: "abcd-efgh-jkmn", relay: nil)
                == ["host", "setup", "--pair", "ABCD-EFGH-JKMN"])
        #expect(JoinWithCode.arguments(code: "ABCDEFGHJKMN", relay: "https://relay.example.com")
                == ["host", "setup", "--pair", "ABCD-EFGH-JKMN", "--relay", "https://relay.example.com"])
        #expect(JoinWithCode.arguments(code: "ABCDEFGHJKMN", relay: "  ")
                == ["host", "setup", "--pair", "ABCD-EFGH-JKMN"])
        #expect(JoinWithCode.arguments(code: "https://cua.ai/add/abcd-efgh-jkmn?relay=https%3A%2F%2Fr.example.com", relay: nil)
                == ["host", "setup", "--pair", "ABCD-EFGH-JKMN", "--relay", "https://r.example.com"])
        #expect(JoinWithCode.arguments(code: "https://cua.ai/add/ABCDEFGHJKMN?relay=https%3A%2F%2Fr.example.com", relay: "https://mine.example.com")
                == ["host", "setup", "--pair", "ABCD-EFGH-JKMN", "--relay", "https://mine.example.com"])
        #expect(JoinWithCode.arguments(code: "", relay: nil) == nil)
        #expect(JoinWithCode.arguments(code: "   ", relay: "https://r") == nil)
    }

    @Test func picksTheLastStderrLine() {
        #expect(JoinWithCode.failureMessage(stderr: "warming up\ncode expired\n\n  \n", status: 1) == "code expired")
        #expect(JoinWithCode.failureMessage(stderr: "", status: 3) == "cua host setup exited with status 3")
    }

    @Test func parsesLinks() {
        func route(_ s: String) -> String? { URL(string: s).flatMap(JoinWithCode.route(for:)) }
        #expect(route("cua://add/abcd-efgh-jkmn") == "/machines?join=ABCD-EFGH-JKMN")
        #expect(route("cua://add/ABCDEFGHJKMN?relay=https%3A%2F%2Fr.example.com%2Fx")
                == "/machines?join=ABCD-EFGH-JKMN&relay=https%3A%2F%2Fr.example.com%2Fx")
        #expect(route("cua://add?code=abcdefghjkmn") == "/machines?join=ABCD-EFGH-JKMN")
        #expect(route("cua://add/ABCDEFGHJKMN/extra") == nil)
        #expect(route("cua://add/ABCD-EFGH-JKM") == nil)
        #expect(route("cua://add/ABCD-EFGH-JKMI") == nil)
        #expect(route("cua://add") == nil)
        #expect(route("cua://open/ABCDEFGHJKMN") == nil)
        #expect(route("https://cua.ai/add/ABCDEFGHJKMN") == nil)
    }

    @Test func linkRouteIsThePath() {
        #expect(MainUIRoute.path("/machines?join=ABCD-EFGH-JKMN").webPath == "/machines?join=ABCD-EFGH-JKMN")
    }

    @Test @MainActor func emptyCodeIsBadArgs() async throws {
        _ = NSApplication.shared
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("cua-join-\(UUID().uuidString)")
        let model = AppModel(backend: FixtureSpacesBackend(), keyvault: KeyvaultModel(client: nil),
                             onboarding: OnboardingModel(statePath: dir.appendingPathComponent("onboarding.json").path),
                             settingsPath: dir.appendingPathComponent("settings.json").path,
                             telemetry: FixtureTelemetry())
        let bridge = WebUIBridge(model: model, allowedOrigins: [])
        for args: [String: Any] in [[:], ["code": ""], ["code": "  "], ["code": 5]] {
            do {
                _ = try await bridge.handle("host.joinWithCode", args)
                Issue.record("expected bad_args for \(args)")
            } catch let f as WebUIBridge.Failure {
                #expect(f.code == "bad_args")
            }
        }
    }
}
