// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

@testable import CuaSpacesMacKit
import CuaSDK
import CuaSpacesFFI
import Foundation
import Testing

@Suite("Host setup failure")
struct HostSetupFailureTests {
    static let missingRelease =
        "http: download: https://github.com/trycua/cua/releases/download/cua-spacesd-v0.2.0/"
        + "cua-spacesd-aarch64-apple-darwin.tar.gz: HTTP 404"

    @Test func aMissingReleaseIsADownloadFailureWithoutTheURL() {
        let f = HostSetupFailure.presenting(Self.missingRelease)
        #expect(f.kind == .download)
        #expect(f.title == "Couldn\u{2019}t download the Cua host service")
        #expect(f.message.contains("update Cua Spaces"))
        #expect(!f.message.contains("http"))
        #expect(!f.title.contains("http"))
        #expect(f.details == Self.missingRelease, "details stay raw")
    }

    @Test(arguments: [
        "cua-spacesd: no published release for this platform",
        "download: cua-spacesd v0.2.0 is not available",
        "Download failed: HTTP 404 Not Found",
    ])
    func releaseNotAvailableVariants(_ raw: String) {
        #expect(HostSetupFailure.presenting(raw).kind == .download)
    }

    @Test(arguments: [
        "error sending request for url (https://relay.cua.ai/v1/hosts): operation timed out",
        "The Internet connection appears to be offline.",
        "tcp connect error: Connection refused (os error 61)",
        "dns error: failed to lookup address information",
        "download: https://github.com/x: operation timed out",
    ])
    func networkFailures(_ raw: String) {
        let f = HostSetupFailure.presenting(raw)
        #expect(f.kind == .network)
        #expect(!f.message.contains("http"))
    }

    @Test(arguments: [
        "launchctl bootstrap gui/501: Bootstrap failed: 125: Domain does not support specified action (no Aqua session)",
        "no GUI session: sign in at the console",
    ])
    func guiSessionFailures(_ raw: String) {
        let f = HostSetupFailure.presenting(raw)
        #expect(f.kind == .guiSession)
        #expect(f.message.contains("Sign in at the Mac"))
    }

    @Test(arguments: [
        "relay: HTTP 401 Unauthorized",
        "not signed in: run `cua login`",
        "unauthenticated",
    ])
    func signedOutFailures(_ raw: String) {
        let f = HostSetupFailure.presenting(raw)
        #expect(f.kind == .signedOut)
        #expect(f.message.contains("Sign in"))
    }

    @Test func anythingElseIsGeneric() {
        let f = HostSetupFailure.presenting("launchctl bootstrap failed\nmore")
        #expect(f.kind == .other)
        #expect(f.title == "Couldn\u{2019}t set up this Mac for access")
        #expect(f.message == "Something went wrong. Try again, or open Details to see what happened.")
        #expect(f.details == "launchctl bootstrap failed\nmore")
    }

    @Test func aPortNumberIsNotAStatusCode() {
        #expect(HostSetupFailure.presenting("listen 0.0.0.0:14041 failed").kind == .other)
    }

    // MARK: - The model

    @MainActor func onboarding(_ host: HostRunning) -> OnboardingModel {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("cua-mac-tests-\(UUID().uuidString)")
        return OnboardingModel(statePath: dir.appendingPathComponent("onboarding.json").path, host: host)
    }

    @MainActor @Test func aFailedSetupKeepsTheRawErrorAsDetails() async {
        let host = FixtureHost()
        host.failSetup = Self.missingRelease + "\ncaused by: 404"
        let model = onboarding(host)
        model.host.openForm()
        await model.setUpHost()
        let failure = model.host.setupFailure
        #expect(failure?.kind == .download)
        #expect(failure?.details == Self.missingRelease + "\ncaused by: 404", "the whole error, every line")
        #expect(model.host.formView?.canSubmit == true, "Retry can run")
        #expect(!model.host.settingUp)
        #expect(model.showingHostForm)
    }

    @MainActor @Test func retryRunsTheSameChoiceAndClearsOnSuccess() async {
        let host = FixtureHost()
        host.failSetup = Self.missingRelease
        let model = onboarding(host)
        model.host.openForm()
        model.host.send(.setProfile(profile: "spare"))
        await model.setUpHost()
        #expect(model.host.setupFailure != nil)
        #expect(model.host.formView?.request?.profile == "spare", "the choice survives the failure")

        host.failSetup = nil
        await model.setUpHost()
        #expect(host.calls.count == 2)
        #expect(host.calls[0] == host.calls[1], "the retry sends the same request")
        #expect(model.host.setupFailure == nil)
        #expect(!model.showingHostForm)
        #expect(model.host.state?.configured == true)
    }

    @MainActor @Test func whileRetryingTheFailureStaysAndSetupIsBusy() async {
        let gate = GatedHost()
        let model = onboarding(gate)
        model.host.openForm()
        gate.fail = "no GUI session"
        await model.setUpHost()
        #expect(model.host.setupFailure?.kind == .guiSession)

        gate.fail = nil
        gate.hold = true
        let retry = Task { await model.setUpHost() }
        await gate.waitUntilCalled(times: 2)
        #expect(model.host.settingUp, "Retry shows progress and is disabled")
        #expect(model.host.formView?.canSubmit == false, "no second setup while one runs")
        #expect(model.host.setupFailure?.kind == .guiSession, "the failure stays while retrying")
        gate.release()
        await retry.value
        #expect(!model.host.settingUp)
        #expect(model.host.setupFailure == nil)
    }

    @MainActor @Test func closingTheFormClearsTheFailure() async {
        let host = FixtureHost()
        host.failSetup = "boom"
        let model = onboarding(host)
        model.host.openForm()
        await model.setUpHost()
        #expect(model.host.setupFailure != nil)
        model.host.closeForm()
        #expect(model.host.setupFailure == nil)
        model.host.openForm()
        #expect(model.host.setupFailure == nil)
    }
}

/// A host whose setup can fail or wait until released (no service, no
/// network: it wraps `FixtureHost`).
final class GatedHost: HostRunning, @unchecked Sendable {
    let inner = FixtureHost()
    var fail: String?
    var hold = false
    private var calls = 0
    private var held: CheckedContinuation<Void, Never>?
    private var callWaiters: [(Int, CheckedContinuation<Void, Never>)] = []
    private let lock = NSLock()

    func status() async throws -> HostStatus { try await inner.status() }

    func setupRequest(request: AppHostSetupRequest, accountToken: String?) async throws -> HostStatus {
        let (shouldHold, ready) = lock.withLock { () -> (Bool, [CheckedContinuation<Void, Never>]) in
            calls += 1
            let ready = callWaiters.filter { $0.0 <= calls }.map(\.1)
            callWaiters.removeAll { $0.0 <= calls }
            return (hold, ready)
        }
        ready.forEach { $0.resume() }
        if shouldHold { await withCheckedContinuation { c in lock.withLock { held = c } } }
        inner.failSetup = fail
        return try await inner.setupRequest(request: request, accountToken: accountToken)
    }

    func waitUntilCalled(times: Int) async {
        await withCheckedContinuation { c in
            let done = lock.withLock { () -> Bool in
                if calls >= times { return true }
                callWaiters.append((times, c))
                return false
            }
            if done { c.resume() }
        }
    }

    func release() {
        // The setup may not have parked yet: poll briefly for it.
        Task {
            while true {
                if let c = lock.withLock({ () -> CheckedContinuation<Void, Never>? in
                    let c = held
                    held = nil
                    return c
                }) {
                    c.resume()
                    return
                }
                await Task.yield()
            }
        }
    }

    func stopSharing() async throws -> HostStatus { try await inner.stopSharing() }
    func startSharing() async throws -> HostStatus { try await inner.startSharing() }
    func remove() async throws { try await inner.remove() }
    func configure(change: HostSettingsChange) async throws -> HostStatus { try await inner.configure(change: change) }
}
