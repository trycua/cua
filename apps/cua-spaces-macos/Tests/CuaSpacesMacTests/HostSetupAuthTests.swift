// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import CuaSpacesFFI
@testable import CuaSpacesMacKit
import Foundation
import Testing

/// A host whose setups answer from a script (then succeed), recording the
/// token each one got. No service, no network.
final class ScriptedHost: HostRunning, @unchecked Sendable {
    let inner = FixtureHost()
    var script: [Error] = []
    private(set) var tokens: [String?] = []

    func setupRequest(request: AppHostSetupRequest, accountToken: String?) async throws -> HostStatus {
        tokens.append(accountToken)
        if !script.isEmpty { throw script.removeFirst() }
        return try await inner.setupRequest(request: request, accountToken: accountToken)
    }

    func status() async throws -> HostStatus { try await inner.status() }
    func stopSharing() async throws -> HostStatus { try await inner.stopSharing() }
    func startSharing() async throws -> HostStatus { try await inner.startSharing() }
    func pauseSignedOut() async throws -> HostStatus { try await inner.pauseSignedOut() }
    func resumeSignedIn(account: String) async throws -> HostStatus { try await inner.resumeSignedIn(account: account) }
    func remove() async throws { try await inner.remove() }
    func configure(change: HostSettingsChange) async throws -> HostStatus { try await inner.configure(change: change) }
}

/// The account as host setup sees it: a token store that can be signed
/// out, offline, or refuse to refresh, and a sign-in that can finish or not.
@MainActor
final class FakeAccountTokens {
    /// The session's token; nil is signed out.
    var token: String?
    /// What a forced refresh yields (nil: the refresh was refused).
    var refreshed: String?
    /// The next reads fail with these first.
    var failures: [Error] = []
    /// What a sign-in leaves signed in as (nil: cancelled).
    var signInGives: String?
    private(set) var reads: [Bool] = []
    private(set) var signIns = 0
    /// The progress the form showed while the sign-in ran.
    private(set) var progressDuringSignIn: String?

    func wire(_ model: HostModel) {
        model.tokenRetryDelays = [.zero, .zero]
        model.accountToken = { [unowned self] force in
            self.reads.append(force)
            if !self.failures.isEmpty { throw self.failures.removeFirst() }
            if force {
                self.token = self.refreshed
            }
            return self.token
        }
        model.signIn = { [unowned self, unowned model] in
            self.signIns += 1
            self.progressDuringSignIn = model.progress
            self.token = self.signInGives
            return self.signInGives != nil
        }
    }
}

@MainActor
@Suite("Host setup account")
struct HostSetupAuthTests {
    func model(_ host: ScriptedHost, _ account: FakeAccountTokens) -> HostModel {
        let m = HostModel(host: host)
        account.wire(m)
        m.openForm()
        return m
    }

    @Test func signedInRunsSetupWithTheToken() async {
        let host = ScriptedHost(), account = FakeAccountTokens()
        account.token = "tok-1"
        let m = model(host, account)
        await m.submit()
        #expect(host.tokens == ["tok-1"])
        #expect(account.signIns == 0)
        #expect(m.form == nil && m.setupFailure == nil)
        #expect(m.state?.configured == true)
    }

    /// The launch bug: onboarding allows skipping sign-in, and 0.3.0/0.3.1
    /// then ran relay setup with no token, failing `unauthenticated`. Now
    /// "Set up for access" signs in inline and carries on.
    @Test func signedOutSignsInInlineThenFinishesSetup() async {
        let host = ScriptedHost(), account = FakeAccountTokens()
        account.signInGives = "tok-new"
        let m = model(host, account)
        await m.submit()
        #expect(account.signIns == 1)
        #expect(account.progressDuringSignIn == HostModel.signInProgress)
        #expect(host.tokens == ["tok-new"], "setup never runs without a token")
        #expect(m.form == nil && m.setupFailure == nil)
        #expect(m.progress == nil)
        #expect(m.state?.configured == true)
    }

    @Test func aCancelledSignInIsASignInFailureWithSignInAsItsButton() async {
        let host = ScriptedHost(), account = FakeAccountTokens()
        let m = model(host, account)
        await m.submit()
        #expect(host.tokens.isEmpty, "the relay is never asked without an account")
        let f = try? #require(m.setupFailure)
        #expect(f?.kind == .signedOut)
        #expect(f?.actionLabel == "Sign In")
        #expect(m.form != nil && m.progress == nil)

        // Sign In (the failure's button) runs the same flow again.
        account.signInGives = "tok-2"
        await m.submit()
        #expect(host.tokens == ["tok-2"])
        #expect(m.setupFailure == nil && m.form == nil)
    }

    @Test func aNetworkErrorReadingTheTokenIsRetriedNotTakenAsSignedOut() async {
        let host = ScriptedHost(), account = FakeAccountTokens()
        account.token = "tok-1"
        account.failures = [CuaError.Http(message: "auth.cua.ai: connection reset"),
                            CuaError.Timeout(message: "refresh timed out")]
        let m = model(host, account)
        await m.submit()
        #expect(account.reads.count == 3)
        #expect(account.signIns == 0)
        #expect(host.tokens == ["tok-1"])
        #expect(m.setupFailure == nil)
    }

    @Test func offlineStaysANetworkFailureWithRetry() async {
        let host = ScriptedHost(), account = FakeAccountTokens()
        account.token = "tok-1"
        account.failures = Array(repeating: CuaError.Http(
            message: "error sending request for url (https://auth.cua.ai/token): connection refused"), count: 5)
        let m = model(host, account)
        await m.submit()
        #expect(host.tokens.isEmpty)
        #expect(account.signIns == 0, "offline is not signed out")
        #expect(m.setupFailure?.kind == .network)
        #expect(m.setupFailure?.actionLabel == "Retry")
    }

    /// The old closure turned any token error (vault, network) into nil,
    /// which reached the relay as "not signed in". Now the error is shown.
    @Test func aVaultErrorIsShownNotSentAsNoToken() async {
        let host = ScriptedHost(), account = FakeAccountTokens()
        account.token = "tok-1"
        account.failures = [CuaError.Internal(message: "credential store: keychain is locked")]
        let m = model(host, account)
        await m.submit()
        #expect(host.tokens.isEmpty)
        #expect(m.setupFailure?.details.contains("keychain is locked") == true)
        #expect(m.setupFailure?.kind != .signedOut)
    }

    @Test func aTokenTheRelayRefusesIsRefreshedOnceThenSetupFinishes() async {
        let host = ScriptedHost(), account = FakeAccountTokens()
        account.token = "tok-stale"
        account.refreshed = "tok-fresh"
        host.script = [CuaError.Unauthenticated(message: "relay: invalid account token: ExpiredSignature")]
        let m = model(host, account)
        await m.submit()
        #expect(account.reads == [false, true])
        #expect(host.tokens == ["tok-stale", "tok-fresh"])
        #expect(account.signIns == 0)
        #expect(m.setupFailure == nil && m.form == nil)
    }

    @Test func aRefreshTheAccountRefusesSignsInAgainThenFinishes() async {
        let host = ScriptedHost(), account = FakeAccountTokens()
        account.token = "tok-stale"
        account.refreshed = nil
        account.signInGives = "tok-new"
        host.script = [CuaError.Unauthenticated(message: "relay: invalid account token")]
        let m = model(host, account)
        await m.submit()
        #expect(account.signIns == 1)
        #expect(host.tokens == ["tok-stale", "tok-new"])
        #expect(m.setupFailure == nil)
    }

    @Test func aRelayThatKeepsRefusingIsASignInFailure() async {
        let host = ScriptedHost(), account = FakeAccountTokens()
        account.token = "tok-1"
        account.refreshed = "tok-2"
        host.script = [CuaError.Unauthenticated(message: "relay: invalid account token"),
                       CuaError.Unauthenticated(message: "relay: invalid account token")]
        let m = model(host, account)
        await m.submit()
        #expect(host.tokens == ["tok-1", "tok-2"], "one refresh, not a loop")
        #expect(m.setupFailure?.kind == .signedOut)
    }

    @Test func otherSetupFailuresAreNotRetriedWithANewToken() async {
        let host = ScriptedHost(), account = FakeAccountTokens()
        account.token = "tok-1"
        host.script = [CuaError.Http(message: "download: https://x/cua-spacesd.tar.gz: HTTP 404 Not Found")]
        let m = model(host, account)
        await m.submit()
        #expect(host.tokens == ["tok-1"])
        #expect(account.reads == [false])
        #expect(m.setupFailure?.kind == .download)
        #expect(m.setupFailure?.actionLabel == "Retry")
    }

    @Test func directSetupNeedsNoAccount() async {
        let host = ScriptedHost(), account = FakeAccountTokens()
        let m = model(host, account)
        m.send(.setDirect(on: true))
        m.send(.setListen(listen: "10.0.0.2:3211"))
        await m.submit()
        #expect(account.reads.isEmpty && account.signIns == 0)
        #expect(host.tokens == [nil])
    }

    @Test func withoutAnAccountWiredTheHostDecides() async {
        let host = ScriptedHost()
        let m = HostModel(host: host)
        m.openForm()
        await m.submit()
        #expect(host.tokens == [nil])
    }
}
