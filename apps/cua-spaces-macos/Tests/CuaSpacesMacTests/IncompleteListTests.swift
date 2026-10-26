// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

@testable import CuaSpacesMacKit
import CuaSDK
import CuaSpacesFFI
import Foundation
import Testing

/// Right after an update restarts the daemon, this Mac's Spaces list at once
/// and your other computers answer one by one. Until every one did (or was
/// given up on) the list is not "no Spaces": the model says how far it got,
/// the core decides there is no "Create your first Space" yet, and the web
/// page gets it with `spaces.list`.
@Suite("A list your computers are still answering")
@MainActor
struct IncompleteListTests {
    func model(_ backend: SpacesBackend) -> AppModel {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("cua-incomplete-\(UUID().uuidString)")
        return AppModel(backend: backend, keyvault: KeyvaultModel(client: nil),
                        onboarding: OnboardingModel(statePath: dir.appendingPathComponent("onboarding.json").path),
                        settingsPath: dir.appendingPathComponent("settings.json").path,
                        telemetry: FixtureTelemetry())
    }

    @Test func beforeTheFirstReadThereIsNoEmptyPage() {
        let m = model(FixtureSpacesBackend(rows: []))
        #expect(!m.loaded)
        #expect(m.listView.empty == false)
        #expect(m.listView.reachingLine == "Loading Spaces…")
    }

    @Test func computersStillAnsweringAreNotAnEmptyAccount() async throws {
        let backend = FixtureSpacesBackend(rows: [])
        backend.fixtureIncomplete = true
        backend.fixtureReaching = 6
        let m = model(backend)
        let bridge = WebUIBridge(model: m, allowedOrigins: [])
        await m.refresh()
        #expect(m.loaded)
        #expect(m.listIncomplete)
        #expect(m.listView.empty == false)
        #expect(m.listView.reachingLine == "Reaching 6 computers…")
        let answer = try #require(try await bridge.handle("spaces.list", [:]) as? [String: Any])
        #expect(answer["incomplete"] as? Bool == true)
        #expect(answer["reaching"] as? Int == 6)
        #expect(answer["loaded"] as? Bool == true)

        // Every computer answered and none has a Space: now it is empty.
        backend.fixtureIncomplete = false
        backend.fixtureReaching = 0
        await m.refresh(fresh: true)
        #expect(!m.listIncomplete)
        #expect(m.listView.empty)
        #expect(m.listView.reachingLine == nil)
        let done = try #require(try await bridge.handle("spaces.list", [:]) as? [String: Any])
        #expect(done["incomplete"] as? Bool == false)
    }

    @Test func theSpacesKnownSoFarShowWhileTheRestAnswer() async {
        let backend = FixtureSpacesBackend(rows: FixtureSpacesBackend.sample)
        backend.fixtureIncomplete = true
        backend.fixtureReaching = 1
        let m = model(backend)
        await m.refresh()
        #expect(!m.spaces.isEmpty)
        #expect(m.listView.empty == false)
        #expect(m.listView.reachingLine == "Reaching 1 computer…")
    }
}
