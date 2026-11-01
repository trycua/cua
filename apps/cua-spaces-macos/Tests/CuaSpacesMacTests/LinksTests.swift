// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

@testable import CuaSpacesMacKit
import Foundation
import Testing

/// Links to things you run (`links.*`): the daemon's shares through the page's bridge.
@MainActor
struct LinksTests {
    private func bridge(_ backend: FixtureSpacesBackend) -> WebUIBridge {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("cua-links-\(UUID().uuidString)")
        let m = AppModel(backend: backend, keyvault: KeyvaultModel(client: nil),
                         onboarding: OnboardingModel(statePath: dir.appendingPathComponent("onboarding.json").path),
                         settingsPath: dir.appendingPathComponent("settings.json").path,
                         telemetry: FixtureTelemetry(), devices: FixtureDevices())
        return WebUIBridge(model: m, allowedOrigins: [])
    }

    @Test func createListAndStop() async throws {
        let b = bridge(FixtureSpacesBackend())
        let made = try await b.handle("links.create", ["target": "space:dev:3000", "access": "team", "ttlSeconds": 600]) as? [String: Any]
        #expect(made?["access"] as? String == "team")
        #expect(made?["kind"] as? String == "space_port")
        let id = try #require(made?["id"] as? String)
        let listed = try await b.handle("links.list", [:]) as? [String: Any]
        #expect((listed?["links"] as? [Any])?.count == 1)
        let stopped = try await b.handle("links.stop", ["id": id]) as? [String: Any]
        #expect(stopped?["stopped"] as? [String] == [id])
        let after = try await b.handle("links.list", [:]) as? [String: Any]
        #expect((after?["links"] as? [Any])?.isEmpty == true)
    }

    @Test func aMachineNotOnTheRelaySaysSo() async throws {
        let backend = FixtureSpacesBackend()
        backend.linksNeedRelay = true
        let b = bridge(backend)
        await #expect(throws: (any Error).self) {
            _ = try await b.handle("links.create", ["target": "3000"])
        }
    }

    @Test func aTargetIsRequired() async throws {
        let b = bridge(FixtureSpacesBackend())
        await #expect(throws: (any Error).self) {
            _ = try await b.handle("links.create", [:])
        }
    }
}
