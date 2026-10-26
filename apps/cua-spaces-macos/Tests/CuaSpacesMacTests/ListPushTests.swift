// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

@testable import CuaSpacesMacKit
import CuaSDK
import Foundation
import Testing

/// The daemon says when the Space list changed (a Space started or stopped
/// on another computer), and the app reads it again then, instead of at the
/// next poll, once per burst of pushes. (The time budget from a change to
/// the push is the core's, asserted in `cua_spaces::list_watch`; these run
/// beside suites that hold the main actor for seconds, so they check what
/// happens, with room on the clock.)
@Suite("The list read again when the daemon pushes a change")
@MainActor
struct ListPushTests {
    /// Well past the app's settle time, for a busy main actor.
    static let deadline: Duration = .seconds(15)

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

    @Test func aPushedChangeShowsWithoutAPoll() async {
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
        // No poll runs here: only the push can bring it in.
        backend.pushListChanged()
        #expect(await until(Self.deadline) { m.spaces.count == before + 1 })
    }

    @Test func aBurstOfPushesReadsTheListOnceOrTwice() async {
        let backend = FixtureSpacesBackend(rows: FixtureSpacesBackend.sample)
        let m = model(backend)
        await m.refresh()
        await m.startListWatch()
        let reads = backend.fixtureListReads
        for _ in 0..<50 { backend.pushListChanged() }
        // Until the reads it started are done (no new one for a while).
        #expect(await until(Self.deadline) { backend.fixtureListReads > reads })
        var settled = backend.fixtureListReads
        while true {
            try? await Task.sleep(for: .milliseconds(500))
            if backend.fixtureListReads == settled { break }
            settled = backend.fixtureListReads
        }
        // No read storm: the burst is one read, plus at most one for pushes
        // that came during it.
        let extra = backend.fixtureListReads - reads
        #expect(extra >= 1 && extra <= 2, "\(extra) reads for one burst")
    }
}
