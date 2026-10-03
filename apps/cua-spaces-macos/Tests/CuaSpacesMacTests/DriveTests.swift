// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSDK
import CuaSpacesFFI
@testable import CuaSpacesMacKit
import Foundation
import SwiftUI
import Testing

/// An in-memory daemon with the Cua Volume's listing, mount, storage, cache
/// and sync tools (CONTRACT.md's shapes). The models call tools
/// concurrently (`async let`), so every call runs under one lock.
final class FakeDriveTools: AgentsToolRunning, @unchecked Sendable {
    private let lock = NSLock()
    var calls: [(String, [String: Any])] = []
    /// `off`, `needs_approval`, `mounted`.
    var mount = "off"
    /// `volume_mount` mounts at once (NFS), rather than asking for approval.
    var mountGoesStraightThrough = false
    var enabled = false
    /// No drive tools at all (a daemon without them).
    var missing = false
    var backend = "fs"
    var hasKeys = false
    var capacity: UInt64 = 10 * 1024 * 1024 * 1024
    var cached: UInt64 = 1_288_490_188
    var conflicts: [[String: Any]] = [[
        "path": "public/plan.md",
        "conflict_path": "public/plan (conflict from maya-linux 2026-09-29 14.02.11).md",
        "winner_device": "d1", "loser_device": "d2", "winner_version": "v2", "loser_version": "v1",
        "ts_ms": 1_790_000_000_000 - 120_000,
    ]]

    func status() -> [String: Any] {
        [
            "enabled": enabled, "state": mount, "method": "fskit",
            "path": mount == "mounted" ? "/Volumes/Cua Volume" : NSNull(), "volume_name": "Cua Volume",
            "detail": mount == "needs_approval" ? "Turn on Cua Volume in File System Extensions." : NSNull(),
            "settings_url": mount == "needs_approval"
                ? "x-apple.systempreferences:com.apple.LoginItems-Settings.extension" : NSNull(),
        ]
    }

    func agentsTool(_ tool: String, _ args: [String: Any]) async throws -> Any {
        try lock.withLock { try answer(tool, args) }
    }

    private func answer(_ tool: String, _ args: [String: Any]) throws -> Any {
        calls.append((tool, args))
        if missing, tool.hasPrefix("volume_"), !["volume_ls", "volume_requests", "volume_grants"].contains(tool) {
            throw AgentsToolError(message: "unknown tool \(tool)")
        }
        let now: UInt64 = 1_790_000_000_000
        switch tool {
        case "volume_mount_status": return status()
        case "volume_mount":
            enabled = true
            if mount == "off" { mount = mountGoesStraightThrough ? "mounted" : "needs_approval" }
            return status()
        case "volume_unmount":
            enabled = false
            mount = "off"
            return status()
        case "volume_storage":
            return ["backend": backend, "fs_path": "/Users/maya/.cua/volume/data", "has_keys": hasKeys,
                    "cloud_available": false,
                    "s3": backend == "s3" ? ["endpoint": "http://127.0.0.1:9000", "region": "us-east-1",
                                             "bucket": "cua-volume", "root": "", "path_style": true] as Any : NSNull()]
        case "volume_storage_set":
            let dry = args["dry_run"] as? Bool ?? false
            if !dry {
                backend = args["backend"] as? String ?? "fs"
                hasKeys = hasKeys || args["access_key_id"] is String
            }
            return ["ok": true, "reachable": true, "authorized": true, "versioning": true, "applied": !dry]
        case "volume_cache_stats":
            return ["dir": "/Users/maya/.cua/volume/cache", "size_bytes": cached, "capacity_bytes": capacity]
        case "volume_cache_set":
            capacity = (args["capacity_bytes"] as? UInt64) ?? capacity
            return ["size_bytes": cached, "capacity_bytes": capacity]
        case "volume_cache_clear":
            cached = 0
            return ["size_bytes": cached, "capacity_bytes": capacity]
        case "volume_sync_status":
            return [
                "device_id": "d1", "device_name": "maya-mbp", "feed": "live", "poll_interval_ms": 500,
                "last_poll_ms": now - 3_000, "last_remote_change_ms": now - 120_000, "pending_uploads": 2,
                "pending_bytes": 4096, "conflicts": conflicts, "last_error": NSNull(),
                "devices": [
                    ["id": "d1", "name": "maya-mbp", "this_device": true, "last_seen_ms": now - 3_000,
                     "last_change_ms": now - 60_000, "changes": 4],
                    ["id": "d2", "name": "maya-linux", "this_device": false, "last_seen_ms": now - 300_000,
                     "last_change_ms": now - 120_000, "changes": 2],
                ],
            ]
        case "volume_sync_resolve":
            conflicts.removeAll { $0["path"] as? String == args["path"] as? String }
            return [:]
        case "volume_ls":
            return ["entries": [["path": "agents/", "name": "agents", "folder": true],
                                ["path": "public/", "name": "public", "folder": true],
                                ["path": "spaces/", "name": "spaces", "folder": true]]]
        case "volume_requests": return ["requests": []]
        case "volume_grants": return ["grants": []]
        default: throw AgentsToolError(message: "unknown tool \(tool)")
        }
    }

    func names() -> [String] { lock.withLock { calls.map(\.0) } }
}

@MainActor
@Suite("Cua Volume", .serialized)
struct DriveTests {
    init() { _ = NSApplication.shared }

    func atDrive(_ tools: AgentsToolRunning?) -> OnboardingModel {
        let o = OnboardingModel(statePath: nil)
        o.driveTools = tools
        // The Volume page shows with the Cua Volume experiment on.
        o.send(.experimentsLoaded(experiments: AppExperiments(cuaVolume: true, yourCloud: false, sharing: false)))
        o.send(.start)
        o.send(.signinDone)
        o.send(.agentsDone(configured: []))
        o.send(.presentationDone)
        return o
    }

    @Test func theFirstRunCheckboxIsOffAndMountsOnContinue() async throws {
        let tools = FakeDriveTools()
        let o = atDrive(tools)
        await o.checkDrive()
        var card = try #require(o.view.drive)
        #expect(card.label == "Add Cua Volume to Finder")
        #expect(!card.checked && card.enabled, "off by default")
        // Unticked: Continue mounts nothing.
        await o.continueDrive()
        #expect(o.view.step == .mode)
        #expect(!tools.names().contains("volume_mount"))

        let p = atDrive(tools)
        await p.checkDrive()
        p.send(.driveToggled(on: true))
        await p.continueDrive()
        #expect(tools.names().contains("volume_mount"))
        // macOS asks for the extension first: the page stays and says so.
        #expect(p.view.step == .drive)
        card = try #require(p.view.drive)
        #expect(card.settingsLabel == "Open System Settings")
        var opened: [URL] = []
        p.openURL = { opened.append($0) }
        p.openURL(URL(string: card.settingsUrl!)!)
        #expect(opened.count == 1)
        // Approved: the daemon finishes on its own; Continue moves on.
        tools.mount = "mounted"
        await p.checkDrive()
        await p.continueDrive()
        #expect(p.view.step == .mode)
        p.send(.modeChosen(mode: .client))
        #expect(p.view.summary.first { $0.label == "Cua Volume" }?.value == "In Finder")
    }

    @Test func theFirstRunSavesYourBucketThenMounts() async throws {
        let tools = FakeDriveTools()
        let o = atDrive(tools)
        await o.checkDrive()
        var card = try #require(o.view.drive)
        #expect(card.storageOptions.map(\.label) == ["This Mac", "Your S3 bucket", "Set up later"])
        #expect(card.storageOptions.first(where: \.active)?.id == "local", "this Mac by default")
        o.send(.storageChosen(choice: .s3))
        o.send(.driveStorage(action: .showManual(on: true)))
        for (id, value) in [("s3-endpoint", "http://127.0.0.1:9000"), ("s3-bucket", "cua-volume"),
                            ("s3-access-key", "maya-drive"), ("s3-secret", "fixture-secret")] {
            o.send(.driveStorage(action: try #require(appStorageEdit(id: id, value: value))))
        }
        await o.driveStorage(.test)
        let test = try #require(tools.calls.last { $0.0 == "volume_storage_set" })
        #expect(test.1["dry_run"] as? Bool == true)
        card = try #require(o.view.drive)
        #expect(card.storageRows.first { $0.id == "s3-test" }?.value == "Connected, versioning on")
        o.send(.driveToggled(on: true))
        await o.continueDrive()
        let save = try #require(tools.calls.last { $0.0 == "volume_storage_set" })
        #expect(save.1["dry_run"] as? Bool == false && save.1["secret_access_key"] as? String == "fixture-secret")
        #expect(tools.names().last == "volume_mount", "saved, then mounted")
        #expect(o.view.step == .drive, "the fake asks for approval")
        #expect(o.state.storage.form.secretAccessKey.isEmpty)
    }

    @Test func theAgentPromptNoticesTheBucketAndConnects() async throws {
        let tools = FakeDriveTools()
        let o = atDrive(tools)
        await o.checkDrive()
        #expect(o.view.drive?.storedIn?.hasPrefix("Stored in ") == true)
        o.send(.storageChosen(choice: .s3))
        #expect(o.watchingStorage, "the prompt shows")
        #expect(o.view.drive?.canContinue == false)
        await o.loadDriveStorage()
        #expect(!tools.names().contains("volume_storage_set"), "nothing yet")
        // The agent ran `cua volume config set` and `set-keys`.
        tools.backend = "s3"
        tools.hasKeys = true
        await o.loadDriveStorage()
        let adopt = try #require(tools.calls.last { $0.0 == "volume_storage_set" })
        #expect(adopt.1["dry_run"] as? Bool == false && adopt.1["access_key_id"] as? String == nil)
        let card = try #require(o.view.drive)
        #expect(card.storageRows.contains { $0.value == "Connected, versioning on" })
        #expect(card.canContinue)
        // Settings notices the same way.
        tools.backend = "fs"
        tools.hasKeys = false
        let st = StorageModel(tools: tools)
        await st.load()
        await st.choose("backend", "s3")
        #expect(st.watching)
        tools.backend = "s3"
        tools.hasKeys = true
        await st.load()
        #expect(st.section.rows.contains { $0.value == "Connected, versioning on" })
    }

    @Test func setUpLaterSkipsStorage() async throws {
        let tools = FakeDriveTools()
        let o = atDrive(tools)
        await o.checkDrive()
        o.send(.storageChosen(choice: .later))
        #expect(o.view.drive?.storageNote != nil)
        await o.continueDrive()
        #expect(o.view.step == .mode)
        #expect(!tools.names().contains("volume_storage_set"))
    }

    @Test func withoutTheDrivesToolsThePageIsHonest() async throws {
        let tools = FakeDriveTools()
        tools.missing = true
        let o = atDrive(tools)
        await o.checkDrive()
        let card = try #require(o.view.drive)
        #expect(!card.enabled && !card.checked)
        #expect(card.note == "Not available on this Mac yet")
        await o.continueDrive()
        #expect(o.view.step == .mode)
        #expect(!tools.names().contains("volume_mount"))
    }

    @Test func storageSendsKeysOnlyToTheToolAndShowsTheVolume() async throws {
        let tools = FakeDriveTools()
        let s = StorageModel(tools: tools)
        var revealed: [String] = []
        s.reveal = { revealed.append($0) }
        await s.load()
        #expect(s.section.rows.first?.options.map(\.label) == ["This Mac", "S3-compatible"])
        #expect(s.section.rows.allSatisfy { !$0.options.contains { $0.id == "cloud" } })
        await s.choose("backend", "s3")
        #expect(s.section.rows.first { $0.id == "s3-prompt" }?.value?.contains("cua volume config set-keys") == true)
        await s.press("s3-manual")
        #expect(!s.section.rows.contains { $0.id == "s3-prompt" })
        s.edit("s3-endpoint", "http://127.0.0.1:9000")
        s.edit("s3-bucket", "cua-volume")
        s.edit("s3-access-key", "maya-drive")
        s.edit("s3-secret", "fixture-secret")
        await s.press("s3-test")
        let test = try #require(tools.calls.last { $0.0 == "volume_storage_set" })
        #expect(test.1["dry_run"] as? Bool == true)
        #expect(s.section.rows.first { $0.id == "s3-test" }?.value == "Connected, versioning on")
        await s.send(.save)
        let save = try #require(tools.calls.last { $0.0 == "volume_storage_set" })
        #expect(save.1["dry_run"] as? Bool == false)
        #expect(save.1["access_key_id"] as? String == "maya-drive")
        #expect(save.1["secret_access_key"] as? String == "fixture-secret")
        // Saved: the form forgets the keys; the daemon says they are saved.
        #expect(s.state.form.secretAccessKey.isEmpty && !s.state.dirty)
        #expect(s.section.rows.first { $0.id == "s3-secret" }?.placeholder == "Saved")

        await s.choose("mount", "on")
        #expect(tools.names().contains("volume_mount"))
        #expect(s.section.rows.contains { $0.id == "mount-approval" })
        tools.mount = "mounted"
        await s.load()
        await s.press("mount-path")
        #expect(revealed == ["/Volumes/Cua Volume"])
        await s.choose("cache-limit", String(5 * 1024 * 1024 * 1024 as UInt64))
        #expect(tools.capacity == 5 * 1024 * 1024 * 1024)
        await s.press("cache")
        #expect(tools.cached == 0)
        #expect(s.section.rows.first { $0.id == "cache" }?.value == "0 KB of 5 GB")
    }

    @Test func noDaemonMeansOneDisabledRow() async {
        let tools = FakeDriveTools()
        tools.missing = true
        let s = StorageModel(tools: tools)
        await s.load()
        #expect(s.section.rows.map(\.id) == ["storage"])
        #expect(s.section.rows[0].value == "Not available on this Mac yet" && !s.section.rows[0].enabled)
    }

    @Test func theDrivePageShowsDevicesConflictsAndOpensInFinder() async throws {
        let tools = FakeDriveTools()
        tools.mount = "mounted"
        tools.enabled = true
        let m = PersistentModel(tools: tools)
        m.now = { Date(timeIntervalSince1970: 1_790_000_000) }
        var revealed: [String] = []
        m.reveal = { revealed.append($0) }
        await m.sendDrive(nil)
        let v = m.driveView()
        #expect(v.openLabel == "Open in Finder")
        #expect(v.devices.map(\.text) == ["maya-mbp (this device)", "maya-linux"])
        #expect(v.devices[0].trailing == "2 pending, synced just now")
        #expect(v.conflicts.map(\.text) == ["public/plan.md"])
        #expect(v.mountLine == "In Finder at /Volumes/Cua Volume")
        await m.sendDrive(.openVolume(mounted: v.mountPath))
        #expect(revealed == ["/Volumes/Cua Volume"])
        #expect(!tools.names().contains("volume_ls"), "not a file browser")
        await m.sendDrive(.reveal(path: v.conflicts[0].reveal!))
        #expect(revealed.last?.hasSuffix("plan (conflict from maya-linux 2026-09-29 14.02.11).md") == true)
        await m.sendDrive(.resolve(path: "public/plan.md"))
        #expect(tools.calls.contains { $0.0 == "volume_sync_resolve" && $0.1["path"] as? String == "public/plan.md" })
        #expect(m.driveView().conflicts.isEmpty)
    }

    @Test func openInFinderMountsFirst() async throws {
        let tools = FakeDriveTools()
        tools.mountGoesStraightThrough = true
        let m = PersistentModel(tools: tools)
        var revealed: [String] = []
        m.reveal = { revealed.append($0) }
        await m.sendDrive(nil)
        var v = m.driveView()
        #expect(v.openLabel == "Open in Finder" && v.mountPath == nil && v.mountLine == "Not mounted")
        await m.sendDrive(.openVolume(mounted: v.mountPath))
        #expect(tools.names().contains("volume_mount"))
        #expect(revealed == ["/Volumes/Cua Volume"])
        v = m.driveView()
        #expect(v.mountPath == "/Volumes/Cua Volume")
    }

    @Test func snapshots() async throws {
        let snap = SnapshotTests()
        let tools = FakeDriveTools()
        // The first run's card at beats of its loop, and the Reduce Motion
        // still, ticked.
        let o = atDrive(tools)
        await o.checkDrive()
        let card = try #require(o.view.drive)
        let size = CGSize(width: 380, height: 180)
        for (ms, name) in [(UInt32(400), "mounting"), (1200, "flying"), (3000, "arrived")] {
            try snap.assertSnapshot(DriveCardView(card: card, toggle: { _ in }, fixedMs: ms).padding(12),
                                    "onboarding-drive-\(name)", size: size)
        }
        o.send(.driveToggled(on: true))
        try snap.assertSnapshot(DriveCardView(card: o.view.drive!, toggle: { _ in }, still: true).padding(12),
                                "onboarding-drive-still-on", size: size)
        // Your S3 bucket: the agent prompt first.
        o.send(.storageChosen(choice: .s3))
        try snap.assertSnapshot(DriveCardView(card: o.view.drive!, toggle: { _ in }, still: true).padding(12),
                                "onboarding-drive-prompt", size: CGSize(width: 380, height: 420))
        o.send(.driveStorage(action: .showManual(on: true)))
        // Your S3 bucket, entered manually and tested.
        o.send(.driveStorage(action: .showManual(on: true)))
        for (id, value) in [("s3-endpoint", "http://127.0.0.1:9000"), ("s3-bucket", "cua-volume"),
                            ("s3-access-key", "maya-drive"), ("s3-secret", "fixture-secret")] {
            o.send(.driveStorage(action: try #require(appStorageEdit(id: id, value: value))))
        }
        await o.driveStorage(.test)
        try snap.assertSnapshot(DriveCardView(card: o.view.drive!, toggle: { _ in }, still: true).padding(12),
                                "onboarding-drive-bucket", size: CGSize(width: 380, height: 420))
        // Settings with the Storage section (S3, the volume mounted).
        tools.mount = "mounted"
        tools.enabled = true
        tools.backend = "s3"
        tools.hasKeys = true
        let m = try await snap.model()
        m.storage = StorageModel(tools: tools)
        await m.loadSettings()
        // Storage shows only with the Cua Volume experiment on.
        m.settings.experiments = AppExperiments(cuaVolume: true, yourCloud: false, sharing: false)
        try snap.assertSnapshot(SettingsView(model: m), "settings-storage", size: CGSize(width: 520, height: 900))
        // The Drive page, mounted and syncing with a conflict.
        let p = PersistentModel(tools: tools)
        p.now = { Date(timeIntervalSince1970: 1_790_000_000) }
        await p.sendDrive(nil)
        try snap.assertSnapshot(DrivePageView(model: p), "drive-page-sync", size: CGSize(width: 720, height: 620))
    }
}
