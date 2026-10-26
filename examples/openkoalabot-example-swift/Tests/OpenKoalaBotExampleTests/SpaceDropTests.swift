// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import Cua
import CuaSpaces
import Foundation
import Testing
@testable import OpenKoalaBotExample

/// The one drop zone for the Bot's Space: files dropped on it (or picked
/// with "Send file…") upload for the Bot, and nothing else is claimed. App
/// teleport is not part of this sample: it ships with Cua Spaces
/// (source-available), and the embedded runtime the sample runs refuses it.
/// Fixtures live in a temp directory; nothing on the machine is read.
@Suite @MainActor struct SpaceDropTests {
    private func scratch(_ tag: String) throws -> URL {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("okb-\(tag)-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        return root
    }

    /// Only local files are taken: a drop with no Bot, a web link, a folder
    /// or an app bundle is left alone.
    @Test func onlyFilesForABotAreClaimed() throws {
        setenv("CUA_ENV_TEST_SANDBOX", "1", 1)
        let root = try scratch("drop")
        defer { try? FileManager.default.removeItem(at: root) }
        let note = root.appendingPathComponent("note.txt")
        try "hi".write(to: note, atomically: true, encoding: .utf8)
        let app = root.appendingPathComponent("Visual Studio Code.app")
        try FileManager.default.createDirectory(at: app.appendingPathComponent("Contents"),
                                                withIntermediateDirectories: true)

        let c = SpaceDropCoordinator(store: BotStore(client: ScriptedSpacesClient()))
        #expect(!c.drop([note], botID: nil), "files need a Bot to send to")
        #expect(!c.drop([URL(string: "https://example.com")!], botID: "koala"))
        #expect(!c.drop([app], botID: "koala"), "an app bundle is a folder, not a file")
        #expect(SpaceDropZone.files(in: [app, note]) == [note])
        #expect(c.zoneStatus == nil)
        #expect(SpaceDropZone.caption == "Drop a file")
        #expect(SpaceDropZone.sendFileTitle == "Send file\u{2026}")
    }

    /// Files dropped on the one zone upload for the Bot, and the zone's
    /// status line says whether they landed.
    @Test func filesDroppedOnTheZoneUploadForTheBot() async throws {
        setenv("CUA_ENV_TEST_SANDBOX", "1", 1)
        let root = try scratch("zone")
        defer { try? FileManager.default.removeItem(at: root) }
        let note = root.appendingPathComponent("note.txt")
        try "hi".write(to: note, atomically: true, encoding: .utf8)

        let c = SpaceDropCoordinator(store: BotStore(client: ScriptedSpacesClient()))
        #expect(c.drop([note], botID: "koala"))
        #expect(c.zoneStatus == .working("Sending note.txt\u{2026}"))
        for _ in 0..<50 where c.zoneStatus == .working("Sending note.txt\u{2026}") {
            try await Task.sleep(nanoseconds: 20_000_000)
        }
        // The scripted client is not attached to a Space, so nothing landed
        // and the zone must not say it did.
        #expect(c.zoneStatus == .failed("Could not send note.txt"))
    }

    /// The takeover shows the same drop zone as the Computer pane (built by
    /// `SpaceDropCoordinator.zone`, on the coordinator both share), and there
    /// is no other file-drop overlay: the shell's only drop target is it.
    @Test func theTakeoverUsesTheComputerPanesDropZone() async throws {
        setenv("CUA_ENV_TEST_SANDBOX", "1", 1)
        let root = try scratch("takeover")
        defer { try? FileManager.default.removeItem(at: root) }
        let note = root.appendingPathComponent("note.txt")
        try "hi".write(to: note, atomically: true, encoding: .utf8)

        let c = SpaceDropCoordinator(store: BotStore(client: ScriptedSpacesClient()))
        let shell = DesktopShell(takeover: true, dropZone: c.zone(space: "space-1", botID: "koala"))
        let shown = try #require(shell.dropZone)
        #expect(shown.spaceID == "space-1")
        #expect(shown.status == nil, "no drop yet")

        // A drop's progress shows on the zone the takeover rebuilds.
        #expect(c.drop([note], botID: "koala"))
        #expect(c.zone(space: "space-1", botID: "koala").status == .working("Sending note.txt\u{2026}"))
    }

    /// Session teleport ships with Cua Spaces: the embedded runtime this
    /// sample runs (a temp registry, never `~/.cua`) refuses its tools and
    /// says where teleport ships, before touching any Space or host app.
    @Test func theEmbeddedRuntimeRefusesTeleport() async throws {
        setenv("CUA_ENV_TEST_SANDBOX", "1", 1)
        let root = try scratch("teleport")
        defer { try? FileManager.default.removeItem(at: root) }
        let client = try SDKSpacesClient(backend: .embedded(spacesHome: root.path))
        for tool in ["teleport_manifest", "teleport_app"] {
            do {
                _ = try await client.raw(tool, ["space": "direct:127.0.0.1:1", "app": "firefox"])
                Issue.record("\(tool) must be refused without Cua Spaces")
            } catch {
                let text = "\(error)"
                #expect(text.contains("teleport is not available on this host"), "\(tool): \(text)")
                #expect(text.contains("ships with Cua Spaces"), "\(tool): \(text)")
            }
        }
    }

    /// The same refusal on the SDK's typed path (`Space.teleportManifest`,
    /// `Space.teleport`): `CuaError.HostCapabilityMissing`. Needs a Space
    /// added to an embedded runtime (`OPENKOALABOTS_TEST_SPACE_URL`); skips
    /// otherwise. The approver declines, so nothing could move either way.
    @Test func anEmbeddedSpaceRefusesSessionTeleportWithHostCapabilityMissing() async throws {
        let target = try LiveSpace.require("the embedded teleport refusal")
        // A Space reached through a daemon may be served by Cua Spaces.
        guard !(ProcessInfo.processInfo.environment["OPENKOALABOTS_TEST_SPACE_URL"] ?? "").isEmpty else { return }
        let space = try #require(try await target.client.sdkConnection.nativeSpace(SpaceID(target.space)))
        do {
            _ = try await space.teleportManifest(app: "firefox", scope: nil)
            Issue.record("teleport ships with Cua Spaces")
        } catch CuaError.HostCapabilityMissing(let message) {
            #expect(message.contains("Cua Spaces"))
        }
        do {
            _ = try await space.teleport(app: "firefox", scope: nil, approver: Decline())
            Issue.record("teleport ships with Cua Spaces")
        } catch CuaError.HostCapabilityMissing {}
    }
}

/// Declines every manifest.
private final class Decline: TeleportApprover, @unchecked Sendable {
    func approve(manifest: CuaSDK.TeleportManifest) -> TeleportDecision? { nil }
}
