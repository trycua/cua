// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import CuaSpacesFFI
@testable import CuaSpacesMacKit
import Foundation
import Testing

/// Settings → Devices, the enroll and approval sheets and the banner, on
/// in-memory devices and presence (no relay, no device key, no Touch ID).
@MainActor
@Suite("Devices")
struct DevicesTests {
    static let now: UInt64 = 1_800_000_000

    func model(_ fixture: FixtureDevices = FixtureDevices(snapshot: FixtureDevices.sample(now: now)),
               presence: FixturePresence = FixturePresence()) -> DevicesModel {
        let m = DevicesModel(devices: fixture, presence: presence)
        m.clock = { Date(timeIntervalSince1970: TimeInterval(Self.now)) }
        m.locale = Locale(identifier: "en_US")
        m.timeZone = TimeZone(identifier: "UTC")!
        m.pollInterval = .milliseconds(1)
        m.signedIn = true
        return m
    }

    @Test func signedOutReadsNothing() async {
        let fixture = FixtureDevices()
        let m = model(fixture)
        m.signedIn = false
        await m.refresh()
        #expect(m.snapshot == nil)
        #expect(m.banner == nil)
        #expect(!m.view.labels.signedOut.isEmpty)
    }

    @Test func aDeviceAskingIsAnnouncedOnceAndOpensTheSheet() async {
        let m = model()
        var announced: [String] = []
        m.notify = { announced.append($0.deviceId) }
        await m.refresh()
        #expect(announced == ["dev_work", "dev_old"])
        #expect(m.approval?.deviceId == "dev_work")
        #expect(m.approvalSurface == .main)
        await m.refresh()
        #expect(announced.count == 2, "a device asks once per launch")
        #expect(m.view.thisDevice.kind == .enrolled)
        #expect(m.thisDeviceText(m.view.thisDevice) == "Enrolled until Feb 8, 2027")
        let studio = m.view.rows.first { $0.id == "dev_studio" }!
        #expect(m.rowDetail(studio) == "Linux \u{b7} Enrolled \u{b7} re-verify in 12 days \u{b7} Last seen 3 hours ago")
        #expect(m.view.recent.first?.text == "MacBook Pro opened build-box")
    }

    @Test func approvingAsksForPresenceFirst() async {
        let fixture = FixtureDevices(snapshot: FixtureDevices.sample(now: Self.now))
        let presence = FixturePresence(allow: false)
        let m = model(fixture, presence: presence)
        await m.refresh()
        m.setCode("k7qx")
        await m.approve()
        #expect(presence.asked.isEmpty, "Approve needs the whole code")
        m.setCode("k7qxm2rp")
        await m.approve()
        #expect(presence.asked == ["approve \u{201c}Work laptop\u{201d} for your Cua account"])
        #expect(fixture.calls.isEmpty, "no presence, no relay call")
        #expect(m.approvalView?.error == "Approval was cancelled")
        #expect(m.approvalView?.busy == false)
        presence.allow = true
        await m.approve()
        #expect(fixture.calls == ["approve:K7QX-M2RP"])
        #expect(m.approval == nil)
    }

    @Test func denyRevokesANewDeviceAndNotNowPutsOffReverification() async {
        let fixture = FixtureDevices(snapshot: FixtureDevices.sample(now: Self.now))
        let m = model(fixture)
        await m.refresh()
        await m.deny()
        #expect(fixture.calls == ["revoke:dev_work"])
        m.openApproval(deviceId: "dev_old", in: .settings)
        #expect(m.approvalView?.denyLabel == "Not Now")
        await m.deny()
        #expect(fixture.calls == ["revoke:dev_work"], "Not Now calls nothing")
        #expect(m.approval == nil)
    }

    @Test func anExpiredCodeOffersApprovingTheSoleWaitingDeviceByID() async {
        let fixture = FixtureDevices(snapshot: FixtureDevices.sample(now: Self.now))
        let m = model(fixture)
        await m.refresh()
        m.setCode("z9wy4tpn")
        fixture.failApprove = "not found: relay: no device is waiting with the code Z9WY-4TPN " +
            "(codes expire after 10 minutes); approve by id instead: `cua devices approve dev_work` " +
            "(ids in `cua devices ls`)"
        await m.approve()
        #expect(m.approvalView?.error == "The code expired.")
        #expect(m.approvalView?.approveLabel == "Approve by ID")
        #expect(m.approvalView?.canApprove == true)
        fixture.failApprove = nil
        await m.approve()
        #expect(fixture.calls == ["approve:Z9WY-4TPN", "approve:dev_work"], "the retry sends the device id, not the stale code")
        #expect(m.approval == nil)
    }

    @Test func theRowsDenyRevokesAPendingDeviceAndDismissesAnExpiredOne() async {
        let fixture = FixtureDevices(snapshot: FixtureDevices.sample(now: Self.now))
        let m = model(fixture)
        await m.refresh()
        await m.denyApproval(deviceId: "dev_old")
        #expect(fixture.calls.isEmpty, "a re-verification is only dismissed, like Not Now")
        #expect(m.dismissed.contains("dev_old"))
        await m.denyApproval(deviceId: "dev_work")
        #expect(fixture.calls == ["revoke:dev_work"])
    }

    @Test func enrollingByCodeWaitsForTheApproval() async {
        var snap = FixtureDevices.sample(now: Self.now)
        snap.devices[0].state = "pending"
        let fixture = FixtureDevices(snapshot: snap)
        let m = model(fixture)
        await m.refresh()
        #expect(m.banner?.tone == .warning)
        m.startEnroll(in: .main)
        #expect(m.enrollView?.options.count == 2)
        m.pollLimit = 3
        await m.chooseEnroll(.approve)
        #expect(m.enroll?.phase == .waiting, "no approval yet: the code keeps showing")
        #expect(m.enrollView?.code == "K7QX-M2RP")
        #expect(m.pendingCode == "K7QX-M2RP")
        #expect(fixture.calls == ["enroll", "check", "check", "check"])
        fixture.enrolledNow = true
        await m.waitForApproval()
        #expect(m.enroll?.phase == .enrolled)
        #expect(m.enrollView?.closeLabel == "Done")
        #expect(m.pendingCode == nil)
    }

    @Test func signingInAgainEnrollsTheFirstDevice() async {
        let fixture = FixtureDevices(snapshot: FixtureDevices.sample(now: Self.now))
        let m = model(fixture)
        m.startEnroll(in: .settings)
        m.signIn = { false }
        await m.chooseEnroll(.signIn)
        #expect(m.enroll?.phase == .failed)
        #expect(fixture.calls.isEmpty)
        m.backEnroll()
        m.signIn = { true }
        fixture.enrollEnrolled = true
        await m.chooseEnroll(.signIn)
        #expect(m.enroll?.phase == .enrolled)
        #expect(fixture.calls == ["enroll"])
    }

    @Test func renameAndRevokeGoToTheRelay() async {
        let fixture = FixtureDevices(snapshot: FixtureDevices.sample(now: Self.now))
        let m = model(fixture)
        await m.refresh()
        await m.rename(id: "dev_studio", to: "   ")
        #expect(fixture.calls.isEmpty, "an empty name is not sent")
        await m.rename(id: "dev_studio", to: "  Studio Mini ")
        await m.revoke(id: "dev_old")
        #expect(fixture.calls == ["rename:dev_studio:Studio Mini", "revoke:dev_old"])
        #expect(m.view.rows.contains { $0.title == "Studio Mini" })
    }
}
