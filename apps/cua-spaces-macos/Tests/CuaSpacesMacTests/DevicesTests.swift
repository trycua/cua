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

    /// The Machines page reads who the relay sees connected now: a host
    /// that rejoined after the last read shows online once the answer is
    /// older than half a minute.
    @Test func relayPresenceIsReadAgainOnceStale() async {
        let fixture = FixtureDevices(snapshot: FixtureDevices.sample(now: Self.now))
        let m = model(fixture)
        var now = TimeInterval(Self.now)
        m.clock = { Date(timeIntervalSince1970: now) }
        await m.refreshIfStale()
        #expect(m.relayOnline == ["m1": true, "m2": false])
        #expect(m.deviceStates["dev_work"] == "pending")
        fixture.current.machines[1].online = true
        now += 10
        await m.refreshIfStale()
        #expect(m.relayOnline["m2"] == false, "a fresh answer is kept")
        now += 30
        await m.refreshIfStale()
        #expect(m.relayOnline["m2"] == true)
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

    /// The relay answers an approval with the device still pending: the
    /// sheet shows an error, never "approved".
    @Test func anApprovalTheRelayDidNotEnrollIsAnError() async {
        func relayDevice(_ state: String) -> RelayDevice {
            RelayDevice(id: "dev_work", name: "Work laptop", state: state, platform: "macos",
                        createdAt: Self.now, enrolledUntil: nil, lastSeen: nil, current: false)
        }
        #expect(throws: Never.self) { try LiveDevices.requireEnrolled(relayDevice("enrolled")) }
        #expect(throws: CuaError.self) { try LiveDevices.requireEnrolled(relayDevice("pending")) }
        let relay = RelayAnswering(state: "pending")
        let live = DevicesModel(devices: relay, presence: FixturePresence())
        live.pollInterval = .milliseconds(1)
        live.signedIn = true
        await live.refresh()
        #expect(live.approval?.deviceId == "dev_work")
        live.setCode("k7qxm2rp")
        await live.approve()
        #expect(live.approval != nil, "still open: nothing was approved")
        #expect(live.approvalView?.error?.contains("pending") == true)
        relay.state = "enrolled"
        await live.approve()
        #expect(live.approval == nil)
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

/// FixtureDevices whose approvals answer with the relay's device in `state`,
/// through LiveDevices' guard.
final class RelayAnswering: DevicesRunning, @unchecked Sendable {
    let fixture = FixtureDevices(snapshot: FixtureDevices.sample(now: DevicesTests.now))
    var state: String
    init(state: String) { self.state = state }

    func snapshot() async throws -> DevicesSnapshot { try await fixture.snapshot() }
    func enroll() async throws -> DeviceEnrollment { try await fixture.enroll() }
    func checkEnrolled() async -> Bool { await fixture.checkEnrolled() }
    func approve(code: String?, deviceId: String?) async throws {
        try LiveDevices.requireEnrolled(RelayDevice(
            id: "dev_work", name: "Work laptop", state: state, platform: "macos",
            createdAt: DevicesTests.now, enrolledUntil: nil, lastSeen: nil, current: false))
        try await fixture.approve(code: code, deviceId: deviceId)
    }
    func rename(id: String, name: String) async throws { try await fixture.rename(id: id, name: name) }
    func revoke(id: String) async throws { try await fixture.revoke(id: id) }
    func confirmMachine(id: String) async throws { try await fixture.confirmMachine(id: id) }
    func createPairingCode(team: String?) async throws -> PairingCode { try await fixture.createPairingCode(team: team) }
    func pairingCodes() async throws -> [PairingCode] { try await fixture.pairingCodes() }
    func revokePairingCode(id: String) async throws { try await fixture.revokePairingCode(id: id) }
    func relayUsage() async throws -> AppRelayUsage? { try await fixture.relayUsage() }
}

/// Settings' Relay usage row and the notice above the main window: the
/// core's words, read as the web UI reads them.
@MainActor
@Suite("Relay usage")
struct RelayUsageTests {
    func model(_ fixture: FixtureDevices) -> (DevicesModel, UserDefaults) {
        let suite = "cua-relay-usage-\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defaults.removePersistentDomain(forName: suite)
        let m = DevicesModel(devices: fixture, presence: FixturePresence(), defaults: defaults)
        m.signedIn = true
        return (m, defaults)
    }

    @Test func withinTheAllowanceOneRowAndNoNotice() async throws {
        let (m, _) = model(FixtureDevices())
        await m.readRelayUsage()
        let view = try #require(m.relayUsageView)
        #expect(view.label == "Remote use")
        #expect(view.line == "Live sessions 1 of 2 \u{b7} Remote hours 6.5 of 20 this month")
        #expect(view.plan == "Free plan")
        #expect(m.relayNotice == nil)
    }

    @Test func pastItTheNoticeShowsUntilDismissedForThatMonthAndReason() async throws {
        let fixture = FixtureDevices()
        fixture.usage = FixtureDevices.usage("reached")
        let (m, defaults) = model(fixture)
        await m.readRelayUsage()
        let notice = try #require(m.relayNotice)
        #expect(notice.text == "You're using 2 of 2 live sessions and 20.4 of 20 remote hours on the free plan. Your Spaces keep working. Pro gives you 5 live sessions and 200 remote hours.")
        #expect(notice.upgrade == "Upgrade")
        #expect(notice.upgradeUrl == "https://app.cua.ai/buy/pro?from=app_meter")
        #expect(notice.level == "limit")
        m.dismissRelayNotice()
        #expect(m.relayNotice == nil)
        #expect(defaults.string(forKey: DevicesModel.relayNoticeDismissedKey) == "2026-10:hours,sessions")
        // Kept across launches; another reason shows it again.
        let again = DevicesModel(devices: fixture, presence: FixturePresence(), defaults: defaults)
        again.signedIn = true
        await again.readRelayUsage()
        #expect(again.relayNotice == nil)
        fixture.usage?.liveSessions = 1
        fixture.usage?.reasons = ["hours"]
        await again.readRelayUsage()
        #expect(again.relayNotice?.dismissKey == "2026-10:hours")
    }

    @Test func aRefusedSessionShowsTheSentenceWithUpgradeUntilDismissed() async throws {
        let (m, _) = model(FixtureDevices())
        m.noteRefusal("timed out")
        #expect(m.relayNotice == nil)
        m.noteRefusal("relay: You're using 5 of 5 live sessions on Pro. Close one or upgrade to Teams.")
        let notice = try #require(m.relayNotice)
        #expect(notice.text == "You're using 5 of 5 live sessions on Pro. Close one or upgrade to Teams.")
        #expect(notice.upgrade == "Upgrade")
        #expect(notice.upgradeUrl?.hasSuffix("/buy/teams?from=plan_limit") == true)
        m.dismissRelayNotice()
        #expect(m.relayNotice == nil)
    }

    @Test func atEightyPercentOfHoursAFreePlanGetsAHeadsUpThatWaitsForLiveSessions() async throws {
        let fixture = FixtureDevices()
        fixture.usage = FixtureDevices.usage("warn")
        let (m, _) = model(fixture)
        await m.readRelayUsage()
        // The fixture has a session going: the prompt waits.
        #expect(m.relayNotice == nil)
        fixture.usage?.liveSessions = 0
        await m.readRelayUsage()
        let notice = try #require(m.relayNotice)
        #expect(notice.level == "warn")
        #expect(notice.text == "You've used 16.5 of 20 remote hours this month. Nothing stops at the limit. Pro gives you 200 hours and 5 live sessions.")
        #expect(notice.upgradeUrl == "https://app.cua.ai/buy/pro?from=app_meter")
        #expect(notice.dismissKey == "2026-10:warn")
    }

    @Test func theFreePlanCardOffersProAndATeam() async throws {
        let (m, _) = model(FixtureDevices())
        await m.readRelayUsage()
        let card = try #require(m.relayUsageView?.planCard)
        #expect(card.label == "Free plan")
        #expect(card.primary?.label == "Get Pro")
        #expect(card.primary?.url == "https://app.cua.ai/buy/pro?from=app_plan_card")
        #expect(card.secondary?.label == "Start a team")
        #expect(card.secondary?.url == "https://app.cua.ai/buy/teams?from=app_plan_card")
    }

    @Test func signedOutUnmeteredAndFailedReads() async {
        let fixture = FixtureDevices()
        let (m, _) = model(fixture)
        await m.readRelayUsage()
        #expect(m.relayUsage != nil)
        // A recent read is not repeated.
        await m.readRelayUsage(maxAge: 60)
        #expect(fixture.calls.filter { $0 == "relay-usage" }.count == 1)
        m.signedIn = false
        #expect(m.relayUsage == nil)
        await m.readRelayUsage()
        #expect(m.relayUsageView == nil)
        m.signedIn = true
        fixture.usage = FixtureDevices.usage("none")
        await m.readRelayUsage()
        #expect(m.relayUsage == nil)
        fixture.usage = FixtureDevices.usage(nil)
        fixture.usage?.metered = false
        await m.readRelayUsage()
        #expect(m.relayUsageView == nil)
    }

    @Test func theWebUIGetsTheSameAnswer() async throws {
        let fixture = FixtureDevices()
        fixture.usage = FixtureDevices.usage("team")
        let (m, _) = model(fixture)
        await m.readRelayUsage()
        #expect(m.relayUsageView?.line.hasSuffix("Team pool, 3 seats") == true)
        let encoded = BridgeValue.encode(try #require(fixture.usage)) as? [String: Any]
        #expect(encoded?["liveSessionsLimit"] as? NSNumber == 6)
        #expect((encoded?["team"] as? [String: Any])?["seats"] as? NSNumber == 3)
        #expect(encoded?["upgradeUrl"] as? String == "https://cua.ai/pricing")
        let unlimited = BridgeValue.encode(AppRelayUsage(
            metered: true, period: "", plan: "", team: nil, liveSessions: 0, liveSessionsLimit: nil, hoursUsed: 0,
            hoursLimit: nil, myHoursUsed: 0, softLimitReached: false, reasons: [], upgradeUrl: nil)) as? [String: Any]
        #expect(unlimited?["team"] is NSNull && unlimited?["hoursLimit"] is NSNull)
    }
}
