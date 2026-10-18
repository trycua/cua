// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import CuaSpacesFFI
@testable import CuaSpacesMacKit
import Foundation
import Testing

/// Relay sharing follows the sign-in: signed out pauses it (the host leaves
/// the relay, its setup stays), the owner signing in again resumes it, and
/// another account is never handed the old account's registration.
@MainActor
@Suite("Relay sharing and the sign-in")
struct HostSignInSharingTests {
    static let ada = AppHostAccount(id: "user-1", email: "ada@example.com", display: "Ada Lovelace")
    static let bob = AppHostAccount(id: "user-2", email: "bob@example.com", display: "Bob")

    /// A relay host set up while `ada` is signed in.
    func sharing(_ host: FixtureHost = FixtureHost()) async -> (HostModel, FakeAccountTokens, Box) {
        let account = FakeAccountTokens()
        account.token = "tok-ada"
        let who = Box(Self.ada)
        let m = HostModel(host: host)
        account.wire(m)
        m.currentAccount = { who.value }
        m.openForm()
        await m.submit()
        await m.reconcileAccount()
        return (m, account, who)
    }

    final class Box: @unchecked Sendable {
        var value: AppHostAccount?
        init(_ value: AppHostAccount?) { self.value = value }
    }

    @Test func signedInAndSharingSaysWhoseAccount() async {
        let host = FixtureHost()
        let (m, _, _) = await sharing(host)
        #expect(!host.calls.contains("pause"))
        let panel = m.panel
        #expect(panel.notice == nil)
        #expect(panel.facts.first { $0.label == "Shared with" }?.value == "Your account (Ada Lovelace)")
        #expect(panel.actions.map(\.id) == [.stopSharing, .remove])
    }

    @Test func signedOutPausesSharingAndSaysSo() async {
        let host = FixtureHost()
        let (m, account, who) = await sharing(host)
        account.token = nil
        who.value = nil
        await m.reconcileAccount()
        #expect(host.calls.last == "pause")
        #expect(host.current.configured, "the setup stays")
        #expect(!host.current.serviceRunning && !host.current.sharing)
        let panel = m.panel
        #expect(panel.notice == "Sign in to share this Mac through Cua. Sharing is paused while you\u{2019}re signed out.")
        #expect(panel.noticeAction?.id == .signIn)
        #expect(panel.noticeAction?.label == "Sign In")
        #expect(!panel.facts.contains { $0.label == "Shared with" })
        #expect(panel.actions.map(\.id) == [.remove])
        #expect(m.summaryInput?.pausedSignedOut == true)
        // Checked again: already paused, nothing more.
        await m.reconcileAccount()
        #expect(host.calls.filter { $0 == "pause" }.count == 1)
    }

    @Test func signingBackInResumesSharing() async {
        let host = FixtureHost()
        let (m, account, who) = await sharing(host)
        account.token = nil
        who.value = nil
        await m.reconcileAccount()
        // Sign In on the page: the inline sign-in, then sharing resumes.
        account.signInGives = "tok-ada-2"
        who.value = Self.ada
        await m.run(.signIn)
        #expect(account.signIns == 1)
        #expect(host.calls.last == "resume:user-1")
        #expect(host.current.sharing && host.current.serviceRunning)
        #expect(m.panel.notice == nil)
        #expect(m.panel.actions.first?.id == .stopSharing)
    }

    @Test func resumeSharingWhileSignedOutSignsInFirst() async {
        let host = FixtureHost()
        let (m, account, who) = await sharing(host)
        account.token = nil
        who.value = nil
        await m.reconcileAccount()
        // A cancelled sign-in leaves it paused.
        account.signInGives = nil
        await m.run(.resumeSharing)
        #expect(account.signIns == 1)
        #expect(host.current.pausedSignedOut)
        #expect(!host.calls.contains { $0.hasPrefix("resume") || $0 == "start" })
    }

    @Test func anotherAccountIsNotHandedTheOldRegistration() async {
        let host = FixtureHost()
        let (m, account, who) = await sharing(host)
        account.token = nil
        who.value = nil
        await m.reconcileAccount()
        account.signInGives = "tok-bob"
        who.value = Self.bob
        await m.run(.signIn)
        #expect(!host.calls.contains { $0.hasPrefix("resume") })
        #expect(host.current.pausedSignedOut)
        let panel = m.panel
        #expect(panel.notice?.contains("another Cua account") == true)
        #expect(panel.noticeAction == nil)
        #expect(panel.actions.map(\.id) == [.remove])
    }

    @Test func anotherAccountSignedInWhileSharingPausesIt() async {
        let host = FixtureHost()
        let (m, account, who) = await sharing(host)
        account.token = "tok-bob"
        who.value = Self.bob
        await m.reconcileAccount()
        #expect(host.calls.last == "pause")
    }

    @Test func offlineIsNotSignedOut() async {
        let host = FixtureHost()
        let (m, account, _) = await sharing(host)
        account.failures = [CuaError.Transport(message: "offline"), CuaError.Transport(message: "offline"),
                            CuaError.Transport(message: "offline")]
        await m.reconcileAccount()
        #expect(!host.calls.contains("pause"))
        #expect(m.panel.notice == nil)
    }

    /// Both settings off: sharing stops, the setup stays, and Resume
    /// sharing waits until one is on again.
    @Test func bothSettingsOffStopsSharingAndDisablesResume() async {
        let host = FixtureHost()
        let (m, _, _) = await sharing(host)
        #expect(m.panel.toggles.allSatisfy { $0.enabled })
        await m.run(.hideDesktop)
        #expect(m.panel.toggles.map(\.on) == [false, false])
        #expect(m.panel.toggles.allSatisfy { $0.enabled })
        #expect(host.current.configured && !host.current.sharing)
        let resume = m.panel.actions[0]
        #expect(resume.id == .resumeSharing)
        #expect(!resume.enabled)
        #expect(resume.help != nil)
        await m.run(.provideSpaces)
        #expect(m.panel.actions[0].enabled)
        await m.run(.resumeSharing)
        #expect(host.current.sharing)
    }
}
