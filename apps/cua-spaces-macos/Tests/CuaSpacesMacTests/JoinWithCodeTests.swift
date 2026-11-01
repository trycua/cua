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
                == ["host", "setup", "--pair", "ABCD-EFGH-JKMN", "--no-desktop", "--provide-spaces"])
        #expect(JoinWithCode.arguments(code: "ABCDEFGHJKMN", relay: "https://relay.example.com")
                == ["host", "setup", "--pair", "ABCD-EFGH-JKMN", "--relay", "https://relay.example.com", "--no-desktop", "--provide-spaces"])
        #expect(JoinWithCode.arguments(code: "ABCDEFGHJKMN", relay: "  ")
                == ["host", "setup", "--pair", "ABCD-EFGH-JKMN", "--no-desktop", "--provide-spaces"])
        #expect(JoinWithCode.arguments(code: "https://cua.ai/add/abcd-efgh-jkmn?relay=https%3A%2F%2Fr.example.com", relay: nil)
                == ["host", "setup", "--pair", "ABCD-EFGH-JKMN", "--relay", "https://r.example.com", "--no-desktop", "--provide-spaces"])
        #expect(JoinWithCode.arguments(code: "https://cua.ai/add/ABCDEFGHJKMN?relay=https%3A%2F%2Fr.example.com", relay: "https://mine.example.com")
                == ["host", "setup", "--pair", "ABCD-EFGH-JKMN", "--relay", "https://mine.example.com", "--no-desktop", "--provide-spaces"])
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

    /// Each app answers to its own scheme only: `cua://` is Stable's and
    /// `cua-local://` Local's, and the two run on one Mac (CUA-1776).
    @Test func localPairingLinks() {
        func route(_ s: String, scheme: String) -> String? {
            URL(string: s).flatMap { JoinWithCode.route(for: $0, scheme: scheme) }
        }
        // Local takes every form of its own link.
        #expect(route("cua-local://add/abcd-efgh-jkmn", scheme: "cua-local") == "/machines?join=ABCD-EFGH-JKMN")
        #expect(route("cua-local://add/ABCDEFGHJKMN?relay=https%3A%2F%2Fr.example.com%2Fx", scheme: "cua-local")
                == "/machines?join=ABCD-EFGH-JKMN&relay=https%3A%2F%2Fr.example.com%2Fx")
        #expect(route("cua-local://add?code=abcdefghjkmn", scheme: "cua-local") == "/machines?join=ABCD-EFGH-JKMN")
        #expect(route("CUA-LOCAL://add/ABCDEFGHJKMN", scheme: "cua-local") == "/machines?join=ABCD-EFGH-JKMN")
        // ... and judges it as strictly as Stable judges `cua://`.
        #expect(route("cua-local://add/ABCDEFGHJKMN/extra", scheme: "cua-local") == nil)
        #expect(route("cua-local://add/ABCD-EFGH-JKMI", scheme: "cua-local") == nil)
        #expect(route("cua-local://open/ABCDEFGHJKMN", scheme: "cua-local") == nil)
        // A Local build rejects `cua://`: that link belongs to the released app.
        #expect(route("cua://add/ABCDEFGHJKMN", scheme: "cua-local") == nil)
        // A Stable build still rejects `cua-local://`.
        #expect(route("cua-local://add/ABCDEFGHJKMN", scheme: "cua") == nil)
        #expect(route("cua://add/ABCDEFGHJKMN", scheme: "cua") == "/machines?join=ABCD-EFGH-JKMN")
        // This test process is Stable (no `CuaUrlScheme` in its bundle).
        #expect(JoinWithCode.route(for: URL(string: "cua-local://add/ABCDEFGHJKMN")!) == nil)
        #expect(JoinWithCode.route(for: URL(string: "cua://add/ABCDEFGHJKMN")!) == "/machines?join=ABCD-EFGH-JKMN")
    }

    @Test func theSchemeComesFromTheBundle() {
        #expect(AppURLScheme.resolve(info: nil) == "cua")
        #expect(AppURLScheme.resolve(info: [:]) == "cua")
        #expect(AppURLScheme.resolve(info: ["CuaUrlScheme": "  "]) == "cua")
        #expect(AppURLScheme.resolve(info: ["CuaUrlScheme": 5]) == "cua")
        #expect(AppURLScheme.resolve(info: ["CuaUrlScheme": "cua-local"]) == "cua-local")
        #expect(AppURLScheme.resolve(info: ["CuaUrlScheme": " Cua-Local "]) == "cua-local")
        // The test bundle is no Local build.
        #expect(AppURLScheme.current == "cua")
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
