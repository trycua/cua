// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import CuaSpacesFFI
@testable import CuaSpacesMacKit
import Foundation
import Testing

/// Counts Local Network requests; never touches the network.
final class FakeLocalNetwork: LocalNetworkPermissionRequesting, @unchecked Sendable {
    private(set) var requests = 0
    func request() { requests += 1 }
}

@MainActor
@Suite("Local Network access")
struct LocalNetworkPermissionTests {
    func setUp(profile: String, host: FixtureHost = FixtureHost()) async -> (HostModel, FakeLocalNetwork) {
        let m = HostModel(host: host)
        let fake = FakeLocalNetwork()
        m.localNetwork = fake
        m.openForm()
        m.send(.setProfile(profile: profile))
        await m.submit()
        return (m, fake)
    }

    @Test func setupThatProvidesSpacesAsksOnce() async {
        let (m, fake) = await setUp(profile: "spare")
        #expect(m.form == nil)
        #expect(m.state?.provideSpaces == true)
        #expect(fake.requests == 1)
        // Later refreshes do not ask again.
        await m.refresh()
        await m.refresh()
        #expect(fake.requests == 1)
    }

    @Test func desktopOnlySetupDoesNotAsk() async {
        let (m, fake) = await setUp(profile: "desktop")
        #expect(m.state?.configured == true)
        #expect(m.state?.provideSpaces == false)
        #expect(fake.requests == 0)
    }

    @Test func failedSetupDoesNotAsk() async {
        let host = FixtureHost()
        host.failSetup = "boom"
        let (_, fake) = await setUp(profile: "spare", host: host)
        #expect(fake.requests == 0)
    }

    @Test func launchOnAMacThatProvidesSpacesAsks() async throws {
        // A Mac set up earlier: the first status read at launch asks.
        let earlier = FixtureHost()
        _ = try await earlier.setupRequest(
            request: AppHostSetupRequest(mode: "relay", relayUrl: nil, direct: nil, name: nil, allow: nil,
                                         profile: "spare", shareDesktop: nil, provideSpaces: nil),
            accountToken: nil)
        let m = HostModel(host: FixtureHost(status: earlier.current))
        let fake = FakeLocalNetwork()
        m.localNetwork = fake
        await m.refresh()
        #expect(fake.requests == 1)
    }

    @Test func subnetHostIsTheFirstAddress() {
        let ip: UInt32 = 0xC0A8_0117 // 192.168.1.23
        #expect(LiveLocalNetworkPermission.subnetHost(ip: ip, mask: 0xFFFF_FF00) == "192.168.1.1")
        #expect(LiveLocalNetworkPermission.subnetHost(ip: 0xC0A8_0101, mask: 0xFFFF_FF00) == "192.168.1.2")
        #expect(LiveLocalNetworkPermission.subnetHost(ip: 0xA9FE_0102, mask: 0xFFFF_0000) == nil)
        #expect(LiveLocalNetworkPermission.subnetHost(ip: ip, mask: 0xFFFF_FFFF) == nil)
    }
}
