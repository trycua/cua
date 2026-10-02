// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSDK
import CuaSpacesFFI
@testable import CuaSpacesMacKit
import Foundation
import Testing

/// Live, opt-in: the app's Cua Volume models against a real `cua daemon`
/// through the app's own backend (`LiveSpacesBackend`, the tools the pages
/// call). Set `CUA_DRIVE_LIVE=1` with `CUA_HOME` (and `CUA_CREDENTIAL_STORE=file`)
/// naming a throwaway home whose daemon is running; the test mounts the
/// volume at that home's `~/Cua Volume` and unmounts it again. With
/// `CUA_DRIVE_LIVE_S3` (an S3-compatible endpoint with a versioned
/// `cua-volume` bucket) and `CUA_DRIVE_LIVE_S3_KEYS` (`<id> <secret>`), it
/// also tests, saves and switches back from the bucket. Nothing is revealed
/// in Finder (the reveal is captured).
private func liveEnv(_ k: String) -> String? {
    ProcessInfo.processInfo.environment[k].flatMap { $0.isEmpty ? nil : $0 }
}

@MainActor
@Suite("Live Cua Volume", .serialized, .enabled(if: liveEnv("CUA_DRIVE_LIVE") == "1"))
struct LiveDriveTests {
    init() { _ = NSApplication.shared }

    func until(_ seconds: Double, _ done: () async -> Bool) async -> Bool {
        let deadline = Date().addingTimeInterval(seconds)
        while Date() < deadline {
            if await done() { return true }
            try? await Task.sleep(for: .milliseconds(500))
        }
        return false
    }

    @Test func firstRunMountsSettingsShowsItAndTheDrivePageOpensIt() async throws {
        let backend = try LiveSpacesBackend.make()
        // First run: off by default, ticked, Continue mounts.
        let o = OnboardingModel(statePath: nil)
        o.driveTools = backend
        // The Volume page shows with the Cua Volume experiment on.
        o.send(.experimentsLoaded(experiments: AppExperiments(cuaVolume: true, yourCloud: false, sharing: false)))
        o.send(.start)
        o.send(.signinDone)
        o.send(.agentsDone(configured: []))
        o.send(.presentationDone)
        await o.checkDrive()
        let card = try #require(o.view.drive)
        #expect(card.enabled, "the daemon can mount here: \(card.note ?? "")")
        #expect(!card.checked, "off by default")
        o.send(.driveToggled(on: true))
        await o.continueDrive()
        #expect(o.state.driveError == nil, "\(o.state.driveError ?? "")")
        #expect(o.view.step == .mode, "mounted, on to the next page")

        // Settings, Storage: the volume and its path; Show in Finder.
        let s = StorageModel(tools: backend)
        var revealed: [String] = []
        s.reveal = { revealed.append($0) }
        #expect(await until(20) {
            await s.load()
            return s.section.rows.contains { $0.id == "mount-path" && $0.button != nil }
        }, "\(s.section.rows.map(\.id))")
        await s.press("mount-path")
        #expect(revealed.first?.hasSuffix("Cua Volume") == true, "\(revealed)")
        #expect(s.section.rows.contains { $0.id == "cache-limit" })

        // The Drive page: this device, Open in Finder.
        let p = PersistentModel(tools: backend)
        await p.sendDrive(nil)
        let v = p.driveView()
        #expect(v.openLabel == "Open in Finder")
        #expect(v.devices.first?.text.hasSuffix("(this device)") == true, "\(v.devices.map(\.text))")

        // Off again.
        await s.choose("mount", "off")
        #expect(s.state.error == nil, "\(s.state.error ?? "")")
        #expect(await until(20) {
            await s.load()
            return !s.section.rows.contains { $0.id == "mount-path" }
        })
    }

    @Test(.enabled(if: liveEnv("CUA_DRIVE_LIVE_S3") != nil))
    func anS3BucketIsTestedSavedAndLeftAgain() async throws {
        let endpoint = try #require(liveEnv("CUA_DRIVE_LIVE_S3"))
        let keys = try #require(liveEnv("CUA_DRIVE_LIVE_S3_KEYS")).split(separator: " ").map(String.init)
        let s = StorageModel(tools: try LiveSpacesBackend.make())
        await s.load()
        await s.choose("backend", "s3")
        await s.press("s3-manual")
        s.edit("s3-endpoint", endpoint)
        s.edit("s3-bucket", "cua-volume")
        await s.choose("s3-path-style", "on")
        s.edit("s3-access-key", keys[0])
        s.edit("s3-secret", keys[1])
        await s.press("s3-test")
        #expect(s.state.check?.ok == true, "\(s.state.check?.detail ?? s.state.error ?? "")")
        await s.send(.save)
        #expect(s.state.check?.applied == true, "\(s.state.check?.detail ?? s.state.error ?? "")")
        #expect(!s.state.dirty && s.state.form.secretAccessKey.isEmpty)
        #expect(s.section.rows.first { $0.id == "s3-secret" }?.placeholder == "Saved")
        await s.choose("backend", "fs")
        await s.send(.save)
        #expect(s.state.check?.applied == true, "\(s.state.check?.detail ?? s.state.error ?? "")")
        #expect(s.section.rows.first?.options.first { $0.active }?.id == "fs")
    }
}
