// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

@testable import CuaSpacesMacKit
import CuaSDK
import CuaSpacesFFI
import Foundation
import Testing

/// Signing in adds this computer to the account (CUA-2201), and "Add a
/// computer" opens the New UI's sheet from every native entry point.
@Suite("Signing in adds this computer")
struct SignInAddsComputerTests {
    @MainActor @Test func aComputerThatIsNotSetUpProvidesSpacesWithItsScreenPrivate() async {
        let host = FixtureHost()
        let model = HostModel(host: host)
        model.computerName = { "Office Mac" }
        await model.addAfterSignIn()
        #expect(host.calls.filter { $0.hasPrefix("setup:") } == ["setup:relay:https://relay.cua.ai"])
        #expect(model.status?.configured == true)
        #expect(model.status?.name == "Office Mac")
        #expect(model.status?.provideSpaces == true)
        #expect(model.status?.shareDesktop == false, "sharing the screen is a choice on its page")
        #expect(model.form == nil, "no form opens: nothing to fill in")
        #expect(model.signInSetUp == nil)
    }

    @MainActor @Test func aComputerAlreadySetUpKeepsItsSettings() async throws {
        let host = FixtureHost()
        _ = try await host.setupRequest(
            request: AppHostSetupRequest(mode: "relay", relayUrl: nil, direct: nil, name: "Desk", allow: nil,
                                         profile: "desktop", shareDesktop: nil, provideSpaces: false),
            accountToken: nil)
        let model = HostModel(host: host)
        await model.refresh()
        let before = host.calls.count
        await model.addAfterSignIn()
        #expect(host.calls.count == before, "no second setup")
        #expect(model.status?.provideSpaces == false, "Provide Spaces off stays off")
    }

    @MainActor @Test func twoSignInsSetItUpOnce() async {
        let host = FixtureHost()
        let model = HostModel(host: host)
        async let a: Void = model.addAfterSignIn()
        async let b: Void = model.addAfterSignIn()
        _ = await (a, b)
        #expect(host.calls.filter { $0.hasPrefix("setup:") }.count == 1)
    }

    @MainActor @Test func aFailedSetupShowsOnThisComputer() async {
        let host = FixtureHost()
        host.failSetup = "relay: unreachable"
        let model = HostModel(host: host)
        await model.addAfterSignIn()
        #expect(model.status?.configured == false)
        #expect(model.setupFailure != nil)
    }

    @Test func addAComputerOpensTheNewUISheet() {
        #expect(AppModel.addComputerRoute == .path("/machines?add=1"))
    }
}
