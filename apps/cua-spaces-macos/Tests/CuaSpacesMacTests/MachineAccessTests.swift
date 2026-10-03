// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSDK
import CuaSpacesFFI
@testable import CuaSpacesMacKit
import SwiftUI
import Testing

/// Simulated account machines and this Mac's enrollment for the machines
/// tests (and the review previews).
@MainActor
enum MachinesFixture {
    static let now = UInt64(Date().timeIntervalSince1970)
    static let shared = ["desktop_stream", "window_stream", "host_spaces", "files"]

    static func row(_ id: String, _ name: String, os: AppSpaceOs, features: [String], reachable: Bool,
                    error: String? = nil, host: String? = nil, hostName: String? = nil,
                    power: String? = nil, powerState: String? = nil) -> AppSpaceRow {
        AppSpaceRow(id: id, name: name, provider: "relay", spacesdVersion: "0.5.0", features: features,
                    addedAt: "2026-09-28T08:00:00Z", os: os,
                    osName: os == .macos ? "macOS" : "Ubuntu",
                    osPrettyName: os == .macos ? "macOS 26.0" : "Ubuntu 24.04.3 LTS",
                    image: nil, imageDigest: nil, kind: nil, arch: "arm64", reachable: reachable,
                    error: error, host: host, hostName: hostName, power: power, powerState: powerState,
                    cloud: nil, cloudPlace: nil, cloudDelete: nil)
    }

    /// This Mac's relay machine id (its host setup).
    static let thisMacId = "0123abcd4567"

    static let notEnrolledError =
        "this device is not enrolled for your cua.ai account: run `cua devices enroll` (or approve it from the Cua Spaces app on an enrolled device)"

    /// The account's machines. `reachable` false: this Mac cannot open them
    /// (not enrolled), so the probe failed. `includeSelf`: this Mac's own
    /// relay entry, which the relay lists once it shares its desktop.
    static func rows(reachable: Bool = true, includeSelf: Bool = false) -> [AppSpaceRow] {
        let err = reachable ? nil : notEnrolledError
        func f(_ x: [String]) -> [String] { reachable ? x : [] }
        var out: [AppSpaceRow] = [FixtureSpacesBackend.sample[0]]
        out.append(row("relay:m-studio", "Studio", os: .macos, features: f(shared), reachable: reachable, error: err))
        out.append(row("relay:m-mini", "Mac mini", os: .macos, features: f(["host_spaces", "files"]),
                       reachable: reachable, error: err))
        out.append(row("relay:s-mini-ubuntu", "Ubuntu dev", os: .linux, features: f(["desktop_stream", "window_stream"]),
                       reachable: reachable, error: err, host: "m-mini", hostName: "Mac mini",
                       power: "stop", powerState: nil))
        out.append(row("relay:m-ci", "Build box", os: .linux, features: f(["files"]), reachable: reachable, error: err))
        out.append(row("relay:m-dana-2", "Dana's MacBook Pro", os: .macos, features: f(shared),
                       reachable: reachable, error: err))
        if includeSelf {
            out.append(row("relay:\(thisMacId)", "Dana's MacBook Pro", os: .macos, features: f(shared),
                           reachable: reachable, error: err))
        }
        return out
    }

    enum Enrollment { case enrolled, never, pending, expired }

    /// This Mac set up as a host on the relay (sharing its desktop).
    static var relayHost: HostStatus {
        var s = FixtureHost.unconfigured
        s.configured = true
        s.mode = "relay"
        s.machineId = thisMacId
        s.name = "Dana's MacBook Pro"
        s.sharing = true
        s.serviceInstalled = true
        s.serviceRunning = true
        s.online = true
        return s
    }

    static func devices(_ e: Enrollment) -> DevicesSnapshot {
        var s = FixtureDevices.sample(now: now)
        switch e {
        case .enrolled: break
        case .never:
            s.devices.removeAll { $0.id == "dev_mac" }
            s.localDeviceId = nil
        case .pending:
            if let i = s.devices.firstIndex(where: { $0.id == "dev_mac" }) {
                s.devices[i].state = "pending"
                s.devices[i].enrolledUntil = nil
            }
        case .expired:
            if let i = s.devices.firstIndex(where: { $0.id == "dev_mac" }) {
                s.devices[i].state = "expired"
                s.devices[i].enrolledUntil = now - 86_400
            }
        }
        return s
    }

    static func model(rows: [AppSpaceRow], enrollment: Enrollment, hostSetUp: Bool = false) async -> AppModel {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("cua-machines-\(UUID().uuidString)")
        let fixture = FixtureDevices(snapshot: devices(enrollment))
        let m = AppModel(backend: FixtureSpacesBackend(rows: rows),
                         keyvault: KeyvaultModel(client: nil, clock: { fixtureNow }),
                         onboarding: OnboardingModel(statePath: dir.appendingPathComponent("o.json").path),
                         settingsPath: dir.appendingPathComponent("settings.json").path,
                         host: hostSetUp ? FixtureHost(status: relayHost) : FixtureHost(),
                         account: FixtureAccount(), telemetry: FixtureTelemetry(),
                         devices: fixture, presence: FixturePresence())
        m.onboarding.finish()
        await m.choose(row: "auto-connect", option: "off")
        m.devices.signedIn = true
        await m.devices.refresh()
        if enrollment == .pending {
            // This launch registered and shows its code.
            m.devices.pollLimit = 1
            m.devices.pollInterval = .milliseconds(1)
            m.devices.startEnroll(in: .main)
            await m.devices.chooseEnroll(.approve)
            m.devices.closeEnroll()
        }
        await m.refresh()
        return m
    }
}


/// Your machines from this Mac: listed while it is signed in but not
/// enrolled (Connect greyed out under why, with the one action), and a
/// machine that does not share its desktop shows why instead of Stream,
/// Agents and Teleport.
@MainActor
@Suite("Machines", .serialized)
struct MachineAccessTests {
    init() { _ = NSApplication.shared }

    func space(_ m: AppModel, _ id: String) throws -> AppSpace {
        try #require(m.spaces.first { $0.id == id })
    }

    // MARK: - View models

    @Test func enrolledMachinesConnectAsBefore() async throws {
        let m = await MachinesFixture.model(rows: MachinesFixture.rows(), enrollment: .enrolled)
        #expect(m.devices.accessNotice == nil)
        let d = m.detail(try space(m, "relay:m-studio"))
        #expect(d.access == nil && d.desktopNote == nil)
        #expect(d.canStream && d.sections == ["Stream", "Agents", "Teleport"])
        let cover = m.cover(d, requested: false, stream: .noSession)
        #expect(cover.kind == .connect && !cover.buttonDisabled && cover.action == nil)
    }

    @Test func notEnrolledPendingAndExpiredListMachinesWithConnectGreyedOut() async throws {
        for (e, kind, status) in [
            (MachinesFixture.Enrollment.never, AppEnrollmentKind.needsEnrollment, "Not enrolled"),
            (.pending, .waiting, "Waiting for approval"),
            (.expired, .due, "Re-verification due"),
        ] {
            let m = await MachinesFixture.model(rows: MachinesFixture.rows(reachable: false), enrollment: e)
            let notice = try #require(m.devices.accessNotice)
            #expect(notice.kind == kind)
            // Every machine is still listed under My machines.
            let mine = try #require(m.sidebar.sections.first { $0.title == "My machines" })
            #expect(mine.rows.map(\.id).contains("relay:m-studio"))
            #expect(mine.rows.map(\.id).contains("relay:m-mini"))
            let d = m.detail(try space(m, "relay:m-studio"))
            #expect(d.access == notice)
            #expect(!d.canStream && d.sections.isEmpty)
            #expect(d.facts.first { $0.label == "Status" }?.value == status)
            #expect(!d.actions.contains { [.open, .teleport, .pip].contains($0.id) && $0.enabled })
            let cover = m.cover(d, requested: true, stream: .noSession)
            #expect(cover.kind == .connect && cover.buttonDisabled && !cover.openStream)
            #expect(cover.text == notice.text && cover.action == notice.actionLabel)
            // A local Space is unaffected.
            #expect(m.detail(try space(m, "local:aurora")).access == nil)
            if e == .pending {
                // This launch's code, and how to approve from another device.
                #expect(notice.text.contains("K7QX-M2RP"))
            }
        }
    }

    @Test func theActionOpensTheEnrollSheet() async throws {
        let m = await MachinesFixture.model(rows: MachinesFixture.rows(reachable: false), enrollment: .never)
        #expect(m.devices.enroll == nil)
        m.devices.startEnroll(in: .main)
        #expect(m.devices.enrollView?.options.isEmpty == false)
    }

    @Test func aMachineThatDoesNotShareItsDesktop() async throws {
        let m = await MachinesFixture.model(rows: MachinesFixture.rows(), enrollment: .enrolled)
        let mini = m.detail(try space(m, "relay:m-mini"))
        #expect(mini.desktopNote == "Mac mini isn\u{2019}t sharing its desktop. You can still create Spaces on it.")
        #expect(!mini.canStream && !mini.showSections && mini.sections.isEmpty)
        #expect(!mini.actions.contains { [.open, .teleport, .pip].contains($0.id) })
        #expect(mini.newSpace?.on == "host:m-mini")
        #expect(m.hostedRows("relay:m-mini").map(\.id) == ["relay:s-mini-ubuntu"])
        // Neither the desktop nor Spaces: the note alone.
        let ci = m.detail(try space(m, "relay:m-ci"))
        #expect(ci.desktopNote == "Build box isn\u{2019}t sharing its desktop or Spaces.")
        #expect(ci.newSpace == nil && ci.sections.isEmpty)
        // Shared: as before.
        #expect(m.detail(try space(m, "relay:m-studio")).desktopNote == nil)
    }

    @Test func newSpaceOnAMachinePicksItInRunOn() async throws {
        let m = await MachinesFixture.model(rows: MachinesFixture.rows(), enrollment: .enrolled)
        await m.openNewSpace(on: "host:m-mini")
        #expect(m.showingNewSpace)
    }

    /// The probe's live features: the desktop's are unsupported for a
    /// machine that does not share it.
    @Test func probeReadsSupportedFeatures() {
        let json = #"{"version":"0.5.0","features":[{"name":"desktop_stream","supported":false,"limitation":"this machine does not share its desktop (it only provides Spaces)"},{"name":"host_spaces","supported":true},{"name":"files","supported":true}]}"#
        #expect(LiveSpacesBackend.supportedFeatures(capabilitiesJson: json) == ["host_spaces", "files"])
        #expect(LiveSpacesBackend.supportedFeatures(capabilitiesJson: nil) == nil)
        #expect(LiveSpacesBackend.supportedFeatures(capabilitiesJson: "nope") == nil)
    }

    /// Sharing its desktop lists this Mac on the relay too: it stays
    /// "This machine" and is not repeated under My machines, while another
    /// Mac of the same name stays.
    @Test func thisMacIsNotOneOfMyMachines() async throws {
        let m = await MachinesFixture.model(rows: MachinesFixture.rows(includeSelf: true), enrollment: .enrolled,
                                            hostSetUp: true)
        #expect(m.sidebar.thisMachine != nil)
        let mine = try #require(m.sidebar.sections.first { $0.title == "My machines" })
        let ids = mine.rows.map(\.id)
        #expect(!ids.contains("relay:\(MachinesFixture.thisMacId)"))
        #expect(ids.contains("relay:m-dana-2"))
        try SnapshotTests().assertSnapshot(Sidebar(model: m).frame(width: 260), "sidebar-without-this-mac",
                                           size: CGSize(width: 260, height: 600))
    }

    // MARK: - Snapshots

    func card(_ cover: AppDesktopCover) -> some View {
        Form {
            Section { PreviewCard(session: nil) { DesktopCoverView(cover: cover, image: nil) } }
        }
        .formStyle(.grouped)
    }

    /// The machine's card: Connect (enrolled), then greyed out under why
    /// and the one action (not enrolled, waiting for approval, expired).
    @Test func connectSnapshots() async throws {
        let size = CGSize(width: 520, height: 360)
        let snap = SnapshotTests()
        let enrolled = await MachinesFixture.model(rows: MachinesFixture.rows(), enrollment: .enrolled)
        let d = enrolled.detail(try space(enrolled, "relay:m-studio"))
        try snap.assertSnapshot(card(enrolled.cover(d, requested: false, stream: .noSession)),
                                "machine-connect-enrolled", size: size)
        for (e, name) in [(MachinesFixture.Enrollment.never, "machine-connect-not-enrolled"),
                          (.pending, "machine-connect-pending"), (.expired, "machine-connect-expired")] {
            let m = await MachinesFixture.model(rows: MachinesFixture.rows(reachable: false), enrollment: e)
            let d = m.detail(try space(m, "relay:m-studio"))
            try snap.assertSnapshot(card(m.cover(d, requested: false, stream: .noSession)), name, size: size)
        }
    }

    /// Desktop shared (its live card), not shared with Spaces on (the note,
    /// New Space on it, its Spaces), and both off (the note alone).
    @Test func desktopSnapshots() async throws {
        let size = CGSize(width: 560, height: 440)
        let snap = SnapshotTests()
        let m = await MachinesFixture.model(rows: MachinesFixture.rows(), enrollment: .enrolled)
        let studio = try space(m, "relay:m-studio")
        let shared = m.detail(studio)
        try snap.assertSnapshot(Form {
            Section {
                PreviewCard(session: nil) {
                    DesktopCoverView(cover: m.cover(shared, requested: false, stream: .noSession), image: nil)
                }
            }
            Section("Stream") { Text(appSpaceDetailCopy().streamLoading).foregroundStyle(.secondary) }
        }.formStyle(.grouped), "machine-desktop-shared", size: size)
        for (id, name) in [("relay:m-mini", "machine-desktop-not-shared"), ("relay:m-ci", "machine-desktop-and-spaces-off")] {
            let d = m.detail(try space(m, id))
            let note = try #require(d.desktopNote)
            try snap.assertSnapshot(Form {
                DesktopNotSharedSections(model: m, machineId: id, note: note, newSpace: d.newSpace)
                Section { FactRows(facts: d.facts, write: { _ in }, copied: false) }
            }.formStyle(.grouped), name, size: size)
        }
    }

    /// Signed in, not enrolled: My machines still lists every machine.
    @Test func sidebarSnapshot() async throws {
        let m = await MachinesFixture.model(rows: MachinesFixture.rows(reachable: false), enrollment: .never)
        try SnapshotTests().assertSnapshot(Sidebar(model: m).frame(width: 260), "sidebar-my-machines-not-enrolled",
                                           size: CGSize(width: 260, height: 560))
    }
}
