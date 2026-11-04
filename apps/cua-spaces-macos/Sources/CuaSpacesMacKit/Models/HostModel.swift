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
    /// other machines.
    func configure(change: HostSettingsChange) async throws -> HostStatus
    /// Moves the host service from the old LaunchAgent into this app, as
    /// its own login item ("Cua Spaces"), keeping this machine's id,
    /// registration and Spaces; the old one comes back if the helper does
    /// not take over (outcome `rolled-back`).
    func migrateService(force: Bool, appVersion: String) async throws -> HostServiceMigration
}

extension HostRunning {
    public func migrateService(force: Bool, appVersion: String) async throws -> HostServiceMigration {
        HostServiceMigration(outcome: "not-needed", detail: "", permissionsNeeded: false)
    }
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
    /// The next `resumeSignedIn` for another account fails with this (once),
    /// after pausing the host as the real one does.
    public var failResume: String?
    /// What `migrateService` answers (nil: moved, needing permissions).
    public var migration: HostServiceMigration?

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
        // As cua-host decides when the request leaves it out: a machine set
        // up before keeps its setting; a new relay setup provides Spaces.
        let keep = current.configured ? current.provideSpaces : relay
        current = HostStatus(
            configured: true, mode: request.mode, relayUrl: relay ? (request.relayUrl ?? "https://relay.cua.ai") : nil,
            directUrl: relay ? nil : "http://\(request.direct ?? "")", envTokenPath: nil, machineId: "0123abcd4567",
            name: request.name ?? "This computer", sharing: true, serviceInstalled: true, serviceRunning: true,
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
            provideSpaces: request.provideSpaces ?? (spare || keep),
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
        if let n = change.maxSpaces { current.maxSpaces = n }
        if let n = change.maxMemoryGb { current.maxMemoryGb = n }
        if let n = change.maxDiskGb { current.maxDiskGb = n }
        if let n = change.maxRunMinutes { current.maxRunMinutes = n }
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
        let isOwner = account == owner || account.caseInsensitiveCompare(ownerEmail) == .orderedSame
        // Another account: the host registers again for it (as the real
        // one does), paused or not.
        if !isOwner, let why = failResume {
            failResume = nil
            current.pausedSignedOut = true
            current.sharing = false
            current.online = false
            current.serviceRunning = false
            throw CuaError.InvalidArgument(message: why)
        }
        if !isOwner {
            owner = account
            ownerEmail = ""
            current.owner = account
            current.ownerEmail = nil
        }
        guard current.pausedSignedOut || !isOwner else { return current }
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

    public func migrateService(force: Bool, appVersion: String) async throws -> HostServiceMigration {
        calls.append("migrate:\(force)")
        let m = migration ?? HostServiceMigration(
            outcome: "migrated", detail: "the host service now runs inside the Cua Spaces app", permissionsNeeded: true)
        if m.outcome == "migrated" || m.outcome == "cleaned-up" {
            current.serviceKind = "app-helper"
            current.legacyService = false
        }
        return m
    }
}

/// The one-time move of the host service into this app, as its own login
/// item (Cua Spaces 0.9.4 and earlier, and `cua host setup`
/// without the app, ran it from a LaunchAgent of their own). Offered on the
/// main window, so a person is there for the one thing macOS asks again:
/// Screen Recording and Accessibility, now for "Cua Spaces". A login
/// launch nobody sees never moves it.
public enum HelperMove: Equatable {
    /// "Cua Spaces moved its background helper": Continue or Not now.
    case offer
    case moving
    /// Moved; `permissions`: macOS needs Screen Recording and Accessibility
    /// for the helper.
    case moved(permissions: Bool)
    /// It did not take over; the old one runs as before (the reason).
    case rolledBack(String)
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
    /// What the system calls this computer (Sharing's Computer Name), which
    /// the setup form's Name starts with: `cua host setup` uses the same
    /// name when none is given. Tests pin it.
    public var computerName: () -> String? = { Foundation.Host.current().localizedName }
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
    /// Called when a setup, the Provide Spaces switch or joining with a code
    /// turned providing Spaces on here (never for a Mac that already
    /// provided them): the app asks for Local Network access then, on this
    /// Mac, so macOS Spaces your other devices create here can connect.
    public var onStartedProvidingSpaces: (() -> Void)?
    /// This Mac provides Spaces now (as of the last status).
    public var providesSpaces: Bool { status.map { $0.configured && $0.provideSpaces } ?? false }
    /// The host's last status, as the SDK reported it.
    public private(set) var status: HostStatus?
    /// The move into the app's helper, while it shows.
    public private(set) var helperMove: HelperMove? {
        didSet { if oldValue != helperMove { onHelperMove?(helperMove) } }
    }
    /// Shows or hides the move's window (`HelperMoveWindow`).
    public var onHelperMove: ((HelperMove?) -> Void)?
    /// "Not now" this launch.
    private var helperMoveDeclined = false
    /// Restarts of a helper that should run but did not, this launch.
    private var helperRepairs = 0
    /// This app's version (a move that failed is not offered again by
    /// itself with the same version).
    public var appVersion = Bundle.main.infoDictionary?["CuaVersion"] as? String
        ?? Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "0"

    public init(host: HostRunning?) {
        self.host = host
    }

    public var panel: AppHostPanelView { appHostPanel(state: state) }
    public var formView: AppHostFormView? { form.map { appHostFormView(state: $0, identity: identity) } }
    /// What the roster's This machine entry reads.
    public var summaryInput: AppHostSummaryInput? { state.map { appHostSummaryInput(state: $0) } }

    /// Calls `onStartedProvidingSpaces` when providing Spaces went from off
    /// (`was`) to on with the last status.
    func followProviding(from was: Bool) {
        if !was && providesSpaces { onStartedProvidingSpaces?() }
    }

    func apply(_ status: HostStatus) {
        self.status = status
        var next = appHostState(status: status)
        next.account = account
        state = next
        machineId = status.machineId
        onChange?()
    }


    public func refresh() async {
        guard let host else { return }
        do {
            apply(try await host.status())
        } catch {
            self.error = LiveSpacesBackend.words(error)
        }
        await followHelper()
    }

    // MARK: - The app's login item (the host service, as "Cua Spaces")

    /// After each status: offer the move when the old LaunchAgent runs this
    /// machine and this app carries the helper; quietly remove what an
    /// interrupted move left of the old one.
    func followHelper() async {
        guard let host, let status, status.configured, status.helperAvailable else { return }
        // The helper should run but does not (launchd lost it after the app
        // was replaced): starting it again registers it from this copy.
        if status.serviceKind == "app-helper", !status.serviceRunning, status.sharing,
           !status.pausedSignedOut, helperRepairs < 3 {
            helperRepairs += 1
            if let s = try? await host.startSharing() { apply(s) }
            return
        }
        if status.serviceKind == "app-helper", status.legacyService {
            if let m = try? await host.migrateService(force: false, appVersion: appVersion),
               m.outcome == "cleaned-up" {
                await refreshStatusOnly()
            }
            return
        }
        if status.serviceKind == "launchd", helperMove == nil, !helperMoveDeclined {
            helperMove = .offer
        }
    }

    private func refreshStatusOnly() async {
        guard let host, let s = try? await host.status() else { return }
        apply(s)
    }

    /// Continue: move it now (the old one comes back if the helper does not
    /// take over).
    public func moveToHelper() async {
        guard let host, helperMove == .offer else { return }
        helperMove = .moving
        do {
            // The person asked for it: try again even after a failed move.
            let m = try await host.migrateService(force: true, appVersion: appVersion)
            switch m.outcome {
            case "migrated", "switched", "cleaned-up":
                helperMove = .moved(permissions: m.permissionsNeeded)
            case "not-needed":
                helperMove = nil
            default:
                helperMove = .rolledBack(m.detail)
            }
        } catch {
            helperMove = .rolledBack(LiveSpacesBackend.words(error))
        }
        await refreshStatusOnly()
    }

    /// Not now: asked again at the next launch.
    public func declineHelperMove() {
        helperMoveDeclined = true
        helperMove = nil
    }

    /// Done (or OK) on the last step.
    public func closeHelperMove() {
        helperMoveDeclined = true
        helperMove = nil
    }

    // MARK: - Form

    public func openForm() {
        form = appHostFormInitial()
        if let name = computerName()?.trimmingCharacters(in: .whitespacesAndNewlines), !name.isEmpty {
            send(.setName(name: name))
        }
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
        do {
            try await setUp(request, on: host)
            form = nil
        } catch {
            let raw = LiveSpacesBackend.words(error)
            setupFailure = Self.isUnauthenticated(error)
                ? HostSetupFailure.presenting(raw, as: .signedOut)
                : HostSetupFailure.presenting(raw)
            send(.failed(error: raw))
        }
    }

    /// The setup a sign-in started (`addAfterSignIn`), while it runs: the
    /// web UI's own "signing in adds this computer" waits for it instead of
    /// setting up a second time.
    public private(set) var signInSetUp: Task<Void, Never>?

    /// Signing in adds this computer to the account: right after a sign-in,
    /// a computer that isn't set up yet is set up to provide Spaces with its
    /// screen kept private (the `spare` profile) and its own name. Sharing
    /// its desktop is a choice on its page, never a side effect of signing
    /// in. A computer already set up keeps its settings, so turning Provide
    /// Spaces off stays off. Never throws: a failure shows on This
    /// computer, where Set up for access retries.
    public func addAfterSignIn() async {
        if let running = signInSetUp { return await running.value }
        let task = Task { @MainActor [weak self] in
            guard let self, let host = self.host else { return }
            if self.status == nil { await self.refresh() }
            guard let status = self.status, !status.configured else { return }
            var f = appHostFormReduce(state: appHostFormInitial(), action: .setProfile(profile: "spare"))
            if let name = self.computerName()?.trimmingCharacters(in: .whitespacesAndNewlines), !name.isEmpty {
                f = appHostFormReduce(state: f, action: .setName(name: name))
            }
            guard let request = appHostFormView(state: f, identity: self.identity).request else { return }
            do {
                try await self.setUp(request, on: host)
            } catch {
                self.setupFailure = Self.isUnauthenticated(error)
                    ? HostSetupFailure.presenting(LiveSpacesBackend.words(error), as: .signedOut)
                    : HostSetupFailure.presenting(LiveSpacesBackend.words(error))
            }
        }
        signInSetUp = task
        await task.value
        signInSetUp = nil
    }

    /// Host setup with a request the core's form built and validated (the
    /// form's submit, and the web UI's): relay setup with the account token.
    func setUp(_ request: AppHostSetupRequest, on host: HostRunning) async throws {
        defer { progress = nil }
        let was = providesSpaces
        let status = request.mode == "relay"
            ? try await setUpRelay(host, request)
            : try await host.setupRequest(request: request, accountToken: nil)
        setupFailure = nil
        apply(status)
        followProviding(from: was)
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
            // What the failure left behind: a switch of account pauses the
            // old registration first, so Retry goes through the sign-in
            // check again, never straight to starting the old one.
            await refreshStatusOnly()
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

    /// Changes the limits on the Spaces other machines create here (0 is no
    /// limit; nil keeps a value). A failure stays in `actionFailure`.
    public func setLimits(maxSpaces: UInt32?, maxMemoryGb: UInt32?, maxDiskGb: UInt32?, maxRunMinutes: UInt32?) async {
        guard let host, !busy else { return }
        busy = true
        error = nil
        defer { busy = false }
        do {
            apply(try await host.configure(change: HostSettingsChange(
                shareDesktop: nil, provideSpaces: nil, maxSpaces: maxSpaces, maxMacosVms: nil,
                maxMemoryGb: maxMemoryGb, maxDiskGb: maxDiskGb, maxRunMinutes: maxRunMinutes)))
            actionFailure = nil
        } catch {
            actionFailure = HostSetupFailure.presenting(LiveSpacesBackend.words(error))
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
            let was = providesSpaces
            apply(try await host.configure(change: HostSettingsChange(
                shareDesktop: change.shareDesktop, provideSpaces: change.provideSpaces)))
            followProviding(from: was)
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
