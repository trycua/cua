// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import CuaSpacesFFI
import Foundation
import Observation

/// This machine as an unattended-access host: the cua SDK's `Host` (live),
/// or `FixtureHost` (fixtures, tests). The form is validated in the app
/// core before any service is touched, as in the Tauri app.
public protocol HostRunning: AnyObject, Sendable {
    func status() async throws -> HostStatus
    func setupRequest(request: AppHostSetupRequest, accountToken: String?) async throws -> HostStatus
    func stopSharing() async throws -> HostStatus
    func startSharing() async throws -> HostStatus
    /// Relay sharing off while nobody is signed in: the host leaves the
    /// relay (and stays off it across restarts); its setup stays.
    func pauseSignedOut() async throws -> HostStatus
    /// Relay sharing back once `account` (id, else email) is signed in;
    /// refused for an account that does not own the machine.
    func resumeSignedIn(account: String) async throws -> HostStatus
    func remove() async throws
    /// Changes what this machine shares: its desktop, and Spaces for your
    /// other devices.
    func configure(change: HostSettingsChange) async throws -> HostStatus
}

extension CuaSDK.Host: HostRunning {}

/// An in-memory host: setup succeeds (or fails with `failSetup`), sharing
/// toggles. It never installs a service, touches launchd or the network.
public final class FixtureHost: HostRunning, @unchecked Sendable {
    public private(set) var current: HostStatus
    public private(set) var calls: [String] = []
    public var failSetup: String?
    /// The next `configure` fails with this (once).
    public var failConfigure: String?

    public init(status: HostStatus? = nil) {
        current = status ?? FixtureHost.unconfigured
    }

    /// The account a fixture relay host is registered to.
    public var owner = "user-1"
    public var ownerEmail = "ada@example.com"

    public static let unconfigured = HostStatus(
        configured: false, mode: nil, relayUrl: nil, directUrl: nil, envTokenPath: nil, machineId: nil,
        name: nil, sharing: false, serviceInstalled: false, serviceRunning: false, serviceKind: "process",
        online: nil, clients: [], allow: [], permissions: [], error: nil)

    public func status() async throws -> HostStatus { current }

    public func setupRequest(request: AppHostSetupRequest, accountToken: String?) async throws -> HostStatus {
        let request = try appHostValidateSetup(request: request)
        calls.append("setup:\(request.mode):\(request.direct ?? request.relayUrl ?? "https://relay.cua.ai")")
        if let failSetup { throw CuaError.Runtime(message: failSetup) }
        let relay = request.mode == "relay"
        let spare = request.profile == "spare"
        current = HostStatus(
            configured: true, mode: request.mode, relayUrl: relay ? (request.relayUrl ?? "https://relay.cua.ai") : nil,
            directUrl: relay ? nil : "http://\(request.direct ?? "")", envTokenPath: nil, machineId: "0123abcd4567",
            name: request.name ?? "This machine", sharing: true, serviceInstalled: true, serviceRunning: true,
            serviceKind: "launchd", online: true, clients: [], allow: request.allow ?? [],
            permissions: [
                HostPermission(id: "screen-recording", title: "Screen Recording",
                               settingsUrl: "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture",
                               instructions: "Turn on Cua Spaces"),
                HostPermission(id: "accessibility", title: "Accessibility",
                               settingsUrl: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
                               instructions: "Turn on Cua Spaces"),
            ],
            error: nil,
            shareDesktop: request.shareDesktop ?? !spare,
            provideSpaces: request.provideSpaces ?? spare,
            maxSpaces: 4, maxMacosVms: 2,
            owner: relay ? owner : nil, ownerEmail: relay ? ownerEmail : nil)
        return current
    }

    public func configure(change: HostSettingsChange) async throws -> HostStatus {
        calls.append("configure:\(change.shareDesktop.map { "\($0)" } ?? "-"):\(change.provideSpaces.map { "\($0)" } ?? "-")")
        if let failConfigure {
            self.failConfigure = nil
            throw CuaError.Runtime(message: failConfigure)
        }
        if let d = change.shareDesktop { current.shareDesktop = d }
        if let p = change.provideSpaces { current.provideSpaces = p }
        // Both off: nothing to share, so sharing stops (as the host does).
        if !current.shareDesktop && !current.provideSpaces {
            current.sharing = false
            current.clients = []
        }
        return current
    }

    public func stopSharing() async throws -> HostStatus {
        calls.append("stop")
        current.sharing = false
        current.clients = []
        return current
    }

    public func startSharing() async throws -> HostStatus {
        calls.append("start")
        current.sharing = true
        current.pausedSignedOut = false
        return current
    }

    public func pauseSignedOut() async throws -> HostStatus {
        calls.append("pause")
        guard current.configured, current.mode == "relay" else { return current }
        current.pausedSignedOut = true
        current.sharing = false
        current.online = false
        current.clients = []
        current.serviceInstalled = false
        current.serviceRunning = false
        return current
    }

    public func resumeSignedIn(account: String) async throws -> HostStatus {
        calls.append("resume:\(account)")
        guard current.pausedSignedOut else { return current }
        guard account == owner || account.caseInsensitiveCompare(ownerEmail) == .orderedSame else {
            throw CuaError.InvalidArgument(message: "this machine is shared with another Cua account")
        }
        current.pausedSignedOut = false
        current.sharing = true
        current.online = true
        current.serviceInstalled = true
        current.serviceRunning = true
        return current
    }

    public func remove() async throws {
        calls.append("remove")
        current = FixtureHost.unconfigured
    }
}

/// "This machine": the host's state, its page and the host setup form.
/// Every word and decision is the core's (`appHostPanel`, `appHostForm*`).
@MainActor
@Observable
public final class HostModel {
    let host: HostRunning?
    /// The host's state (`nil` until it answers).
    public private(set) var state: AppHostState?
    /// This install's relay machine id, once set up in relay mode: its own
    /// entry in the relay listing is "This machine", not one of "My
    /// machines".
    public private(set) var machineId: String?
    /// The host setup form, while it shows.
    public private(set) var form: AppHostFormState?
    public private(set) var busy = false
    public private(set) var error: String?
    /// The last page button that failed (a setting, Stop sharing, ...), in
    /// plain words with Retry; cleared when any button succeeds.
    public private(set) var actionFailure: HostSetupFailure?
    /// What Retry runs again.
    private var failedAction: AppHostActionId?
    /// The last "Set up for access" failure, in plain words with the raw
    /// error as details. It stays while a retry runs (so Retry can show
    /// progress) and clears when setup succeeds or the form closes.
    public private(set) var setupFailure: HostSetupFailure?
    /// "Set up for access" (or Retry) is running.
    public var settingUp: Bool { formView?.busy == true }
    /// The signed-in account (relay mode joins as it).
    public var identity: String?
    /// The account's token for relay setup (`true`: refresh it even if it
    /// looks valid). nil when no one is signed in; throws when it could not
    /// be read or refreshed (offline, the vault), which is not "signed out".
    public var accountToken: ((_ forceRefresh: Bool) async throws -> String?)?
    /// The signed-in account's id, email and display name, read without the
    /// network (nil: nobody, as far as the local session says).
    public var currentAccount: (() -> AppHostAccount?)?
    /// The account the page was last checked against (nil: signed out or
    /// not checked yet); the page's "Shared with" and its paused notice.
    public private(set) var account: AppHostAccount?
    /// When relay sharing last followed the sign-in.
    private var accountCheckedAt: Date?
    private var reconciling = false
    /// Signs in to Cua (opens the browser and waits): true once signed in.
    /// "Set up for access" runs it inline when relay setup has no account,
    /// then carries on by itself.
    public var signIn: (() async -> Bool)?
    /// What a running "Set up for access" is waiting for, under the form
    /// (finishing the sign-in in the browser); nil otherwise.
    public private(set) var progress: String?
    /// Waits before trying the account token again after a network error
    /// (tests set them to zero).
    var tokenRetryDelays: [Duration] = [.seconds(1), .seconds(3)]

    static let signInProgress =
        "Sign in to Cua in your browser to continue. Setup finishes on its own after that."
    /// Called whenever the state changes (the roster's entry follows it).
    public var onChange: (() -> Void)?
    /// Asks macOS for Local Network access once this Mac is configured
    /// (a host or a controller; see `LocalNetworkPermissionRequesting`);
    /// nil in fixtures and tests.
    public var localNetwork: LocalNetworkPermissionRequesting?
    private var askedLocalNetwork = false

    public init(host: HostRunning?) {
        self.host = host
    }

    public var panel: AppHostPanelView { appHostPanel(state: state) }
    public var formView: AppHostFormView? { form.map { appHostFormView(state: $0, identity: identity) } }
    /// What the roster's This machine entry reads.
    public var summaryInput: AppHostSummaryInput? { state.map { appHostSummaryInput(state: $0) } }

    func apply(_ status: HostStatus) {
        var next = appHostState(status: status)
        next.account = account
        state = next
        machineId = status.machineId
        // Configured setup asks now, while someone is at this Mac: a host
        // that provides Spaces and a controller that only accesses other
        // machines. Launch on a Mac that is not configured yet asks from
        // `AppEnvironment` (see `LocalNetworkPermissionRequesting`).
        if let state, state.configured, !askedLocalNetwork, let localNetwork {
            askedLocalNetwork = true
            localNetwork.request()
        }
        onChange?()
    }

    public func refresh() async {
        guard let host else { return }
        do {
            apply(try await host.status())
        } catch {
            self.error = LiveSpacesBackend.words(error)
        }
    }

    // MARK: - Form

    public func openForm() {
        form = appHostFormInitial()
        error = nil
        setupFailure = nil
    }

    public func closeForm() {
        form = nil
        setupFailure = nil
    }

    public func send(_ action: AppHostFormAction) {
        guard let f = form else { return }
        form = appHostFormReduce(state: f, action: action)
    }

    /// "Set up for access" (and Retry, which runs it again with the same
    /// choices): host setup with the core's validated request.
    public func submit() async {
        guard let host, let view = formView, view.canSubmit, let request = view.request else { return }
        send(.submit)
        defer { progress = nil }
        do {
            let status = request.mode == "relay"
                ? try await setUpRelay(host, request)
                : try await host.setupRequest(request: request, accountToken: nil)
            form = nil
            setupFailure = nil
            apply(status)
        } catch {
            let raw = LiveSpacesBackend.words(error)
            setupFailure = Self.isUnauthenticated(error)
                ? HostSetupFailure.presenting(raw, as: .signedOut)
                : HostSetupFailure.presenting(raw)
            send(.failed(error: raw))
        }
    }

    /// Relay setup always runs with a valid account token: signed out (or a
    /// session that can no longer refresh) signs in first, inline; a token
    /// the relay refuses is refreshed once, then signed in again.
    private func setUpRelay(_ host: HostRunning, _ request: AppHostSetupRequest) async throws -> HostStatus {
        // No account wired (fixtures, captures): the host decides.
        guard accountToken != nil else { return try await host.setupRequest(request: request, accountToken: nil) }
        let token = try await validToken(forceRefresh: false)
        do {
            return try await host.setupRequest(request: request, accountToken: token)
        } catch let error where Self.isUnauthenticated(error) {
            // The relay refused it (expired early, revoked, clock skew):
            // refresh it, then sign in again if that fails too.
            let fresh = try await validToken(forceRefresh: true)
            return try await host.setupRequest(request: request, accountToken: fresh)
        }
    }

    /// The account's token, signing in first when there is none.
    private func validToken(forceRefresh: Bool) async throws -> String {
        if let token = try await readToken(forceRefresh: forceRefresh) { return token }
        guard let signIn else { throw HostSetupAuthError.signedOut }
        progress = Self.signInProgress
        let signedIn = await signIn()
        progress = nil
        guard signedIn else { throw HostSetupAuthError.signInNotFinished }
        if let token = try await readToken(forceRefresh: false) { return token }
        throw HostSetupAuthError.signedOut
    }

    /// One token read: nil when signed out (or refused for good), a network
    /// error tried again with backoff before it is reported as one.
    private func readToken(forceRefresh: Bool) async throws -> String? {
        guard let accountToken else { return nil }
        var waits = tokenRetryDelays[...]
        while true {
            do {
                let token = try await accountToken(forceRefresh)
                return token?.isEmpty == false ? token : nil
            } catch let error where Self.isUnauthenticated(error) {
                return nil
            } catch let error where Self.isTransient(error) && !waits.isEmpty {
                try? await Task.sleep(for: waits.removeFirst())
            }
        }
    }

    static func isUnauthenticated(_ error: Error) -> Bool {
        if case CuaError.Unauthenticated = error { return true }
        if let e = error as? HostSetupAuthError { return e == .signedOut }
        return false
    }

    static func isTransient(_ error: Error) -> Bool {
        switch error {
        case CuaError.Http, CuaError.Timeout, CuaError.Transport: return true
        default: return false
        }
    }

    // MARK: - Relay sharing follows the sign-in

    /// Relay sharing needs a signed-in owner (the core's `account_step`):
    /// nobody signed in (signed out, or a session that can no longer
    /// refresh), or another account, pauses it; the owner signing in again
    /// resumes it. Not knowing (offline, the credential vault) changes
    /// nothing. Run at launch, after a sign-in or sign-out, and every
    /// `interval` from the app's refresh.
    public func reconcileAccount(ifOlderThan interval: TimeInterval = 0) async {
        guard let host, accountToken != nil, !reconciling else { return }
        if interval > 0, let at = accountCheckedAt, Date().timeIntervalSince(at) < interval { return }
        reconciling = true
        defer { reconciling = false }
        let signedIn: AppHostAccount?
        do {
            signedIn = try await readToken(forceRefresh: false) == nil ? nil
                : (currentAccount?() ?? AppHostAccount(id: nil, email: nil, display: identity))
        } catch {
            return
        }
        accountCheckedAt = Date()
        account = signedIn
        if state == nil {
            await refresh()
        } else if var s = state {
            s.account = signedIn
            state = s
        }
        guard let state else { return }
        do {
            switch appHostAccountStep(state: state, account: signedIn) {
            case .keep: break
            case .pause:
                apply(try await host.pauseSignedOut())
                actionFailure = nil
            case .resume:
                if let signedIn { apply(try await host.resumeSignedIn(account: appHostAccountKey(account: signedIn))) }
                actionFailure = nil
            }
        } catch {
            actionFailure = HostSetupFailure.presenting(LiveSpacesBackend.words(error))
            failedAction = .resumeSharing
        }
    }

    /// Sign In on the paused page, and Resume sharing while signed out:
    /// the inline sign-in, then sharing follows it. False when it did not
    /// finish.
    private func signInThenReconcile() async -> Bool {
        // Offline (the token could not be read) is not "signed out": no
        // sign-in then; the reconcile leaves sharing as it is.
        let signedOut: Bool
        do { signedOut = try await readToken(forceRefresh: false) == nil } catch { signedOut = false }
        if signedOut {
            guard let signIn else { return false }
            progress = Self.signInProgress
            let done = await signIn()
            progress = nil
            guard done else { return false }
        }
        await reconcileAccount()
        return true
    }

    // MARK: - Page buttons

    public func run(_ id: AppHostActionId) async {
        if id == .setUp {
            openForm()
            return
        }
        // Sharing over the relay needs a signed-in owner: Sign In, and
        // Resume while paused or signed out, sign in first (inline).
        if id == .signIn || (id == .resumeSharing && needsSignInToShare) {
            guard !busy else { return }
            busy = true
            defer { busy = false }
            actionFailure = nil
            _ = await signInThenReconcile()
            return
        }
        guard let host, !busy else { return }
        busy = true
        error = nil
        defer { busy = false }
        do {
            try await perform(id, on: host)
            actionFailure = nil
            failedAction = nil
        } catch {
            let raw = LiveSpacesBackend.words(error)
            actionFailure = HostSetupFailure.presenting(raw)
            failedAction = id
            // The page shows what the failure left behind (a stopped
            // service is offline), not the state before the button.
            await refresh()
        }
    }

    /// Resume sharing must sign in first: relay sharing paused while
    /// signed out, or relay mode with nobody signed in.
    private var needsSignInToShare: Bool {
        guard accountToken != nil, let state, state.mode == "relay" else { return false }
        return state.pausedSignedOut || account == nil && accountCheckedAt != nil
    }

    /// Runs the failed button again.
    public func retryFailedAction() async {
        guard let id = failedAction else { return }
        await run(id)
    }

    private func perform(_ id: AppHostActionId, on host: HostRunning) async throws {
        // The two settings: the core says what each switch changes.
        if let change = appHostSettingChange(id: id) {
            apply(try await host.configure(change: HostSettingsChange(
                shareDesktop: change.shareDesktop, provideSpaces: change.provideSpaces)))
            return
        }
        switch id {
        case .stopSharing: apply(try await host.stopSharing())
        case .resumeSharing: apply(try await host.startSharing())
        case .remove:
            try await host.remove()
            apply(try await host.status())
        case .setUp, .signIn, .shareDesktop, .hideDesktop, .provideSpaces, .stopProvidingSpaces: break
        }
    }
}

/// Why relay host setup could not get an account token. The words are what
/// `HostSetupFailure` shows (as "Sign in to Cua").
public enum HostSetupAuthError: Error, Equatable, LocalizedError, CustomStringConvertible {
    /// No one is signed in to Cua on this Mac.
    case signedOut
    /// The inline sign-in was cancelled or did not finish.
    case signInNotFinished

    public var description: String {
        switch self {
        case .signedOut: return "Not signed in to Cua: sign in to set up this Mac for access."
        case .signInNotFinished: return "Not signed in to Cua: the sign-in did not finish."
        }
    }

    public var errorDescription: String? { description }
}
