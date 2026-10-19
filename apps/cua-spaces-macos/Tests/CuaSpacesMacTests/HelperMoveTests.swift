// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import CuaSpacesFFI
@testable import CuaSpacesMacKit
import Foundation
import Testing

/// The one-time move of the host service into the app (Cua Spaces Helper):
/// offered only when the old LaunchAgent runs it and this app carries the
/// helper, moved only when the person says Continue, and what went wrong
/// shown in words with the old service kept.
@MainActor
@Suite("Moving the host service into the app")
struct HelperMoveTests {
    /// A host set up as Cua Spaces 0.9.4 left it.
    func oldInstall(helperAvailable: Bool = true, kind: String = "launchd") -> FixtureHost {
        var s = FixtureHost.unconfigured
        s.configured = true
        s.mode = "relay"
        s.machineId = "0123abcd4567"
        s.sharing = true
        s.serviceInstalled = true
        s.serviceRunning = true
        s.serviceKind = kind
        s.helperAvailable = helperAvailable
        s.legacyService = true
        return FixtureHost(status: s)
    }

    @Test func anOldAgentWithTheHelperIsOfferedNotMoved() async {
        let host = oldInstall()
        let m = HostModel(host: host)
        var shown: [HelperMove?] = []
        m.onHelperMove = { shown.append($0) }
        await m.refresh()
        #expect(m.helperMove == .offer)
        #expect(shown == [.offer])
        #expect(!host.calls.contains { $0.hasPrefix("migrate") }, "never moved without Continue")
    }

    @Test func withoutTheHelperNothingIsOffered() async {
        let m = HostModel(host: oldInstall(helperAvailable: false))
        await m.refresh()
        #expect(m.helperMove == nil)
    }

    @Test func continueMovesItAndAsksForThePermissions() async {
        let host = oldInstall()
        let m = HostModel(host: host)
        await m.refresh()
        await m.moveToHelper()
        #expect(host.calls.contains("migrate:true"))
        #expect(m.helperMove == .moved(permissions: true))
        #expect(m.status?.serviceKind == "app-helper")
        m.closeHelperMove()
        await m.refresh()
        #expect(m.helperMove == nil, "moved: never offered again")
    }

    @Test func aMoveThatDidNotTakeOverSaysSoAndKeepsTheOldOne() async {
        let host = oldInstall()
        host.migration = HostServiceMigration(
            outcome: "rolled-back", detail: "Cua Spaces Helper did not take over", permissionsNeeded: false)
        let m = HostModel(host: host)
        await m.refresh()
        await m.moveToHelper()
        #expect(m.helperMove == .rolledBack("Cua Spaces Helper did not take over"))
        #expect(m.status?.serviceKind == "launchd")
    }

    @Test func notNowWaitsForTheNextLaunch() async {
        let m = HostModel(host: oldInstall())
        await m.refresh()
        m.declineHelperMove()
        await m.refresh()
        #expect(m.helperMove == nil)
    }

    @Test func leftoversOfTheOldAgentAreRemovedQuietly() async {
        let host = oldInstall(kind: "app-helper")
        host.migration = HostServiceMigration(outcome: "cleaned-up", detail: "", permissionsNeeded: false)
        let m = HostModel(host: host)
        await m.refresh()
        #expect(host.calls.contains("migrate:false"))
        #expect(m.helperMove == nil)
    }
}
