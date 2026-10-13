// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSDK
import CuaSpacesFFI
@testable import CuaSpacesMacKit
import CuaSpacesStreaming
import Foundation
import Testing

/// What the web UI's bridge keeps for Teleport and Streams, and the consent
/// it hands the SDK.
@Suite("Teleport bridge")
@MainActor
struct TeleportBridgeTests {
    init() { _ = NSApplication.shared }

    func bridge(keyvault: FakeKeyvault? = nil) async -> (WebUIBridge, AppModel) {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("cua-teleport-\(UUID().uuidString)")
        let kv = KeyvaultModel(client: keyvault, clock: { Date(timeIntervalSince1970: Double(KeyvaultFixtures.now) / 1000) })
        let model = AppModel(backend: FixtureSpacesBackend(), keyvault: kv,
                             onboarding: OnboardingModel(statePath: dir.appendingPathComponent("onboarding.json").path),
                             settingsPath: dir.appendingPathComponent("settings.json").path,
                             telemetry: FixtureTelemetry())
        await model.refresh()
        let bridge = WebUIBridge(model: model, allowedOrigins: [])
        bridge.actions = WebUIActions(openSpace: { _ in }, openMain: {})
        return (bridge, model)
    }

    func stream() -> (rows: StreamRowsModel, pips: StreamPiPSet) {
        let provider = SyntheticStreamProvider(units: [], size: CGSize(width: 1280, height: 800), fps: 30)
        return (StreamFixture.model(), StreamPiPSet(provider: provider))
    }

    /// A deleted Space's stream rows and panels are dropped; the live
    /// Spaces keep theirs.
    @Test func deletedSpacesStreamsAreDropped() async throws {
        let (bridge, model) = await bridge()
        let live = try #require(model.spaces.first?.id)
        bridge.streams[live] = stream()
        bridge.streams["local:deleted"] = stream()
        bridge.pruneStreams()
        #expect(Set(bridge.streams.keys) == [live])
    }

    /// The page's consent reaches the SDK through `sdkConsent`: Save to
    /// Keyvault (which the SDK defaults to off) and the review's choices.
    @Test func saveToKeyvaultReachesTheSdkConsent() {
        let on = WebUIBridge.teleportConsent([
            "approved": true, "acknowledgeSensitive": true, "saveToKeyvault": true,
            "acknowledgeRelayPlaintext": true, "cookieDomains": ["github.com"], "exclude": ["Default/Bookmarks"],
            "fromVault": ["a", "b"], "includePasswords": true,
        ])
        #expect(on == sdkConsent(AppTeleportConsent(
            approved: true, acknowledgeSensitive: true, saveToKeyvault: true, acknowledgeRelayPlaintext: true,
            cookieDomains: ["github.com"], exclude: ["Default/Bookmarks"], fromVault: ["a", "b"], includePasswords: true)))
        #expect(on.saveToKeyvault)
        let off = WebUIBridge.teleportConsent(["approved": true, "acknowledgeSensitive": true])
        #expect(!off.saveToKeyvault && off.exclude.isEmpty && off.cookieDomains == nil)
    }

    // MARK: - Keyvault, as the SwiftUI list does it

    /// The vault with a live copy of two GitHub items in Aurora.
    func vaultWithCopy() -> FakeKeyvault {
        var o = KeyvaultFixtures.overview()
        o.deliveries = [KvDelivery(importId: "imp-1", target: "local:aurora", providerId: "chrome",
                                   items: ["c-gh-1", "c-gh-2"], callerFp: "fp", deliveredMs: UInt64(KeyvaultFixtures.now - 600_000),
                                   expiresMs: 0, wiped: false)]
        return FakeKeyvault(o)
    }

    /// Delete asks first, in the core's words (naming the Spaces whose
    /// copies are wiped too); declined, nothing is deleted; confirmed, the
    /// items and their copies go.
    @Test func deleteAsksFirstAndWipesTheLiveCopies() async throws {
        let fake = vaultWithCopy()
        let (bridge, model) = await bridge(keyvault: fake)
        await model.keyvault.refresh()
        var asked: [KvDeleteConfirm] = []
        bridge.askDelete = { asked.append($0); return false }
        do {
            _ = try await bridge.handle("keyvault.delete", ["ids": ["c-gh-1", "c-gh-2"]])
            Issue.record("a declined delete must not run")
        } catch let f as WebUIBridge.Failure {
            #expect(f.code == "cancelled")
        }
        #expect(!fake.commands.contains { if case .deleteItems = $0 { return true }; return false })
        #expect(model.keyvault.deleteConfirm == nil, "the page's alert replaces the list's sheet")
        #expect(asked.first?.title == "Delete 2 items?")
        #expect(asked.first?.message.contains("Copies delivered to a Space will be wiped as well.") == true)

        bridge.askDelete = { _ in true }
        let answer = try #require(try await bridge.handle("keyvault.delete", ["ids": ["c-gh-1", "c-gh-2"]]) as? [String: Any])
        #expect(fake.commands.contains { if case .deleteItems(let ids) = $0 { return ids == ["c-gh-1", "c-gh-2"] }; return false })
        #expect(fake.current.deliveries.allSatisfy { $0.wiped })
        let items = (answer["overview"] as? [String: Any])?["items"] as? [[String: Any]] ?? []
        #expect(!items.contains { ["c-gh-1", "c-gh-2"].contains($0["id"] as? String ?? "") })
    }

    /// Dismiss hides copies from the notch and is remembered; it never
    /// revokes or wipes. The answer carries them for the page.
    @Test func dismissHidesCopiesFromTheNotchOnly() async throws {
        let fake = vaultWithCopy()
        let (bridge, model) = await bridge(keyvault: fake)
        await model.keyvault.refresh()
        #expect(model.keyvault.notchLabel != nil)
        let answer = try #require(try await bridge.handle("keyvault.dismiss", ["imports": ["imp-1"]]) as? [String: Any])
        #expect(answer["dismissed"] as? [String] == ["imp-1"])
        #expect(model.keyvault.notchLabel == nil)
        #expect(model.settings.dismissedAccess == ["imp-1"])
        #expect(fake.commands.isEmpty, "nothing is revoked or wiped")
        #expect(model.keyvault.signedIn(model.spaces).contains("local:aurora"), "the Spaces list still says Signed in")
    }

    /// An Access row's command runs; nothing else does.
    @Test func accessCommandsRunAndNothingElseDoes() async throws {
        let fake = vaultWithCopy()
        let (bridge, _) = await bridge(keyvault: fake)
        _ = try await bridge.handle("keyvault.run", ["command": ["type": "release", "target": "local:aurora"]])
        _ = try await bridge.handle("keyvault.run", ["command": ["type": "remove-rule", "id": "r1"]])
        _ = try await bridge.handle("keyvault.run", ["command": ["type": "revoke-grant", "id": "g1"]])
        #expect(fake.commands == [.release(target: "local:aurora"), .removeRule(id: "r1"), .revokeGrant(id: "g1")])
        for bad: [String: Any] in [["type": "delete-items", "itemIds": ["c-gh-1"]], ["type": "set-disabled", "disabled": true], [:]] {
            do {
                _ = try await bridge.handle("keyvault.run", ["command": bad])
                Issue.record("\(bad) must not run")
            } catch let f as WebUIBridge.Failure {
                #expect(f.code == "bad_args")
            }
        }
        #expect(fake.commands.count == 3)
    }

    /// Show Items asks the daemon for the names (Touch ID).
    @Test func showItemsOpensTheBrowseWindow() async throws {
        let fake = FakeKeyvault(KeyvaultFixtures.overview(namesVisible: false))
        let (bridge, _) = await bridge(keyvault: fake)
        let answer = try #require(try await bridge.handle("keyvault.showItems", [:]) as? [String: Any])
        #expect(fake.commands == [.browse])
        #expect((answer["overview"] as? [String: Any])?["namesVisible"] as? Bool == true)
    }

    // MARK: - Teleport review, as the SwiftUI sheet does it

    /// The sites sent are remembered per app and Space, and the review
    /// starts from them next time; a send from the Keyvault remembers none.
    @Test func rememberedSitesStartTheNextReview() async throws {
        let (bridge, model) = await bridge()
        let space = try #require(model.spaces.first)
        #expect(try await bridge.handle("teleport.remembered", ["providerId": "chrome", "spaceId": space.id]) is NSNull)
        bridge.rememberChoice(providerId: "chrome", spaceId: space.id,
                              consent: ["cookieDomains": ["github.com", "linear.app"], "fromVault": NSNull()])
        let kept = try await bridge.handle("teleport.remembered", ["providerId": "chrome", "spaceId": space.id]) as? [String]
        #expect(kept == ["github.com", "linear.app"])
        // It is the same key the native review reads.
        #expect(appReviewRemembered(choices: model.settings.teleportChoices,
                                    key: appReviewRememberKey(app: "chrome", space: space.name)) == kept)
        // From the Keyvault, or with no site choice (an app that has none), nothing changes.
        bridge.rememberChoice(providerId: "chrome", spaceId: space.id, consent: ["cookieDomains": ["x.com"], "fromVault": ["a"]])
        bridge.rememberChoice(providerId: "slack", spaceId: space.id, consent: ["cookieDomains": NSNull()])
        #expect(try await bridge.handle("teleport.remembered", ["providerId": "slack", "spaceId": space.id]) is NSNull)
        #expect(try await bridge.handle("teleport.remembered", ["providerId": "chrome", "spaceId": space.id]) as? [String] == kept)
    }

    /// The notch shows the transfer while a teleport runs.
    @Test func theNotchShowsTheTransferWhileItRuns() async {
        let (bridge, model) = await bridge()
        #expect(model.notch.state.transfer == nil)
        bridge.setTransfer(true)
        #expect(model.notch.state.transfer != nil)
        bridge.setTransfer(false)
        #expect(model.notch.state.transfer == nil)
    }
}
