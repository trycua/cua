// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

@testable import CuaSpacesMacKit
import CuaSDK
import Foundation
import Testing

/// The daemon says when the Space list changed (a Space started or stopped
/// on another computer), and the app reads it again then, instead of at the
/// next poll: within the push budget, once per burst of pushes.
@Suite("The list read again when the daemon pushes a change")
@MainActor
struct ListPushTests {
    /// A pushed change is on screen within this (the read included).
    static let pushBudget: Duration = .seconds(2)

    func model(_ backend: SpacesBackend) -> AppModel {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("cua-push-\(UUID().uuidString)")
        return AppModel(backend: backend, keyvault: KeyvaultModel(client: nil),
                        onboarding: OnboardingModel(statePath: dir.appendingPathComponent("onboarding.json").path),
                        settingsPath: dir.appendingPathComponent("settings.json").path,
                        telemetry: FixtureTelemetry())
    }

    func until(_ within: Duration, _ done: @MainActor () -> Bool) async -> Bool {
        let clock = ContinuousClock()
        let end = clock.now + within
        while clock.now < end {
            if done() { return true }
            try? await Task.sleep(for: .milliseconds(20))
        }
        return done()
    }

    @Test func aPushedChangeShowsWithinTheBudget() async {
        let backend = FixtureSpacesBackend(rows: FixtureSpacesBackend.sample)
        let m = model(backend)
        await m.refresh()
        await m.startListWatch()
        let before = m.spaces.count
        // A Space starts on another computer: it is in the next read.
        var row = FixtureSpacesBackend.sample[0]
        row.id = "relay:0000000000000abc"
        row.name = "on another computer"
        backend.fixtureRows.append(row)
        let start = ContinuousClock.now
        backend.pushListChanged()
        #expect(await until(Self.pushBudget) { m.spaces.count == before + 1 })
        #expect(ContinuousClock.now - start <= Self.pushBudget)
    }

    @Test func aBurstOfPushesReadsTheListOnceOrTwice() async {
        let backend = FixtureSpacesBackend(rows: FixtureSpacesBackend.sample)
        let m = model(backend)
        await m.refresh()
        await m.startListWatch()
        let reads = backend.fixtureListReads
        for _ in 0..<50 { backend.pushListChanged() }
        _ = await until(.seconds(1)) { false }
        // No read storm: the burst is one read, plus at most one for pushes
        // that came during it.
        let extra = backend.fixtureListReads - reads
        #expect(extra >= 1 && extra <= 2, "\(extra) reads for one burst")
    }
}
