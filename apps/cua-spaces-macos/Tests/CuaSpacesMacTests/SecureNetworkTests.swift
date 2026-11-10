// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation
@testable import CuaSpacesMacKit
import Testing

/// Secure Spaces (always on) and the Public TCP shares experiment (Labs) in the app:
/// the bridge answers, the New Space's Network word, and the notch's exit
/// indicator.
@MainActor
@Suite("Secure network and TCP shares")
struct SecureNetworkTests {
    /// A model over its own settings file.
    func makeModel(_ backend: FixtureSpacesBackend = FixtureSpacesBackend()) throws -> AppModel {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("cua-mac-tests-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let settings = dir.appendingPathComponent("settings.json")
        try Data(#"{"experiments":{}}"#.utf8).write(to: settings)
        return AppModel(backend: backend,
                        keyvault: KeyvaultModel(client: nil, clock: { Date() }),
                        onboarding: OnboardingModel(statePath: dir.appendingPathComponent("onboarding.json").path),
                        settingsPath: settings.path)
    }

    @Test func presetWordsBecomeNetworkSettings() {
        let internet = LiveSpacesBackend.networkSettings(word: "internet", allow: [], deny: [])
        #expect(internet.preset == "internet" && internet.localhostPorts.isEmpty && !internet.allLocalhostPorts)
        let all = LiveSpacesBackend.networkSettings(word: "localhost:all", allow: [], deny: [])
        #expect(all.preset == "localhost" && all.allLocalhostPorts)
        let ports = LiveSpacesBackend.networkSettings(word: "localhost:5432, 3000", allow: [], deny: [])
        #expect(ports.localhostPorts == [5432, 3000] && !ports.allLocalhostPorts)
        let both = LiveSpacesBackend.networkSettings(word: "localhost+lan:5432, 3000", allow: [], deny: [])
        #expect(both.preset == "localhost+lan" && both.localhostPorts == [5432, 3000] && !both.allLocalhostPorts)
        let bothAll = LiveSpacesBackend.networkSettings(word: "localhost+lan:all", allow: [], deny: [])
        #expect(bothAll.preset == "localhost+lan" && bothAll.localhostPorts.isEmpty && bothAll.allLocalhostPorts)
        let custom = LiveSpacesBackend.networkSettings(word: "custom:none", allow: ["github.com"], deny: ["10.0.0.0/8"], via: "this-machine")
        #expect(custom.customBase == "none" && custom.allow == ["github.com"] && custom.deny == ["10.0.0.0/8"] && custom.via == "this-machine")
    }

    @Test func theNewSpacesNetworkChoiceIsKeptUntilItsCreateIsOver() {
        CreateNetworks.shared.set("pending:a", "none")
        // A failed create runs once more: the choice is still there.
        #expect(CreateNetworks.shared.get("pending:a") == "none")
        #expect(CreateNetworks.shared.get("pending:a") == "none")
        CreateNetworks.shared.clear("pending:a")
        #expect(CreateNetworks.shared.get("pending:a") == nil)
    }

    @Test func shareSettingsKeepTheirReadOnlyFieldsFromTheHost() async throws {
        let b = FixtureSpacesBackend()
        var s = try await b.shareSettings()
        s.dailyCapMib = 5120
        s.approveAgentShares = false
        s.usedToday = 99
        s.edgeHost = "ignored"
        let saved = try await b.setShareSettings(s)
        #expect(saved.dailyCapMib == 5120 && !saved.approveAgentShares)
        #expect(saved.usedToday == 0 && saved.edgeHost == nil)
    }

    @Test func aTcpShareCarriesItsRouteAndWhoMayConnect() async throws {
        let b = FixtureSpacesBackend()
        let link = try await b.createTcpLink(target: "space:mc:25565", service: "minecraft", ttlSeconds: nil, name: nil,
                                             options: TcpShareOptions(route: "host", allow: ["203.0.113.0/24"]))
        #expect(link.route == "host" && link.allow == ["203.0.113.0/24"] && link.waiting == nil)
        #expect(link.tcpConnectWith != nil)
        let json = BridgeValue.encode(link) as? [String: Any]
        #expect(json?["routeLine"] as? String == "Through Cua")
    }

    @Test func exitsLightTheNotch() async throws {
        let backend = FixtureSpacesBackend()
        backend.fixtureExits = ["local:b", "local:a"]
        let model = try makeModel(backend)
        await model.syncNotchExits()
        #expect(model.notch.exitSpaces == ["local:a", "local:b"])
        #expect(model.notch.state.hotspot)
        // No exits left: the indicator goes out.
        backend.fixtureExits = []
        await model.syncNotchExits()
        #expect(model.notch.exitSpaces.isEmpty)
        #expect(model.notch.state.hotspot == false)
    }
}
