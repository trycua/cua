// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CoreGraphics
import CuaSDK
import CuaSpaces
import CuaSpacesFFI
import CuaSpacesStreaming
import Foundation
import Observation

// The launch never waits on the keychain or the daemon on the main thread.
// The window (or New UI's) opens first, on stand-ins for the live services
// (`Pending*` below) that wait for them; `StartupModel` then checks, off the
// main thread and without any prompt, that the saved sign-in can be read
// (`cua auth keychain`), asks the user to allow access when it cannot (the
// one macOS prompt, only after a click), starts this app's daemon and the
// SDK, and hands the live services to the stand-ins. Every step shows in the
// window: "Allow Keychain access", "Waiting for Keychain access…",
// "Starting Cua…".

// MARK: - The keychain check

/// What the keychain check found.
public enum KeychainCheckResult: Equatable, Sendable {
    /// The saved sign-in (and every other Cua item) can be read without a
    /// prompt, or there is none, or this build keeps it in a file.
    case ready
    /// Reading it needs a macOS prompt (`locked`: the login keychain is
    /// locked; else this build is not trusted by it yet).
    case needsAccess(locked: Bool)
    /// The user denied the prompt.
    case denied
    /// The check itself failed (an older `cua`, a timeout): the launch
    /// goes on.
    case failed(String)
}

/// Checks the Cua keychain items (`cua auth keychain`).
public protocol KeychainAccessChecking: AnyObject, Sendable {
    /// Whether they can be read; with `prompt`, macOS asks where needed
    /// (and the items get the Cua access list, so no later Cua build asks).
    func check(prompt: Bool) async -> KeychainCheckResult
    /// Removes the saved sign-in without reading it ("Sign in again").
    func forget() async -> KeychainCheckResult
    /// Stops a check still running (its prompt goes away with it).
    func cancel()
}

/// The bundled `cua`'s `auth keychain`: a process of its own, so disabling
/// keychain prompts (process-wide) never touches the app, and a prompt
/// still up goes away when the check is cancelled.
public final class BundledCuaKeychain: KeychainAccessChecking, @unchecked Sendable {
    let cua: String
    /// A check without a prompt never takes this long.
    let quietTimeout: TimeInterval
    /// How long a stopped check gets to exit before it is killed.
    let killAfter: TimeInterval
    private let lock = NSLock()
    private var running: Process?

    public init(cua: String, quietTimeout: TimeInterval = 15, killAfter: TimeInterval = 2) {
        self.cua = cua
        self.quietTimeout = quietTimeout
        self.killAfter = killAfter
    }

    public func check(prompt: Bool) async -> KeychainCheckResult {
        await run(prompt ? ["auth", "keychain", "--prompt"] : ["auth", "keychain"],
                  timeout: prompt ? nil : quietTimeout, interactive: prompt)
    }

    public func forget() async -> KeychainCheckResult {
        await run(["auth", "keychain", "--forget"], timeout: quietTimeout, interactive: false)
    }

    /// Stops the check still running: SIGTERM, then SIGKILL after
    /// `killAfter` (a read waiting on SecurityAgent may not end on SIGTERM).
    public func cancel() {
        guard let p = lock.withLock({ running }), p.isRunning else { return }
        Self.stop(p, killAfter: killAfter)
    }

    /// The process running now (tests).
    var runningPid: Int32? { lock.withLock { running.flatMap { $0.isRunning ? $0.processIdentifier : nil } } }

    static func stop(_ p: Process, killAfter: TimeInterval) {
        p.terminate()
        let pid = p.processIdentifier
        DispatchQueue.global().asyncAfter(deadline: .now() + killAfter) {
            if p.isRunning { kill(pid, SIGKILL) }
        }
    }

    /// Waits (bounded) until the previous check has exited, so a new
    /// prompt never queues behind one still open.
    private func stopPrevious() async {
        guard let p = lock.withLock({ running }), p.isRunning else { return }
        Self.stop(p, killAfter: killAfter)
        let deadline = Date().addingTimeInterval(killAfter + 3)
        while p.isRunning, Date() < deadline { try? await Task.sleep(for: .milliseconds(50)) }
    }

    private func run(_ args: [String], timeout: TimeInterval?, interactive: Bool) async -> KeychainCheckResult {
        await stopPrevious()
        let process = Process()
        process.executableURL = URL(fileURLWithPath: cua)
        process.arguments = args
        // The app shows the first-run usage notice itself (as for its daemon).
        var env = ProcessInfo.processInfo.environment
        env["CUA_DAEMON_STARTED_BY"] = "app"
        // Only the explicit, clicked prompt may show one.
        if interactive {
            env.removeValue(forKey: AppEnvironment.keychainNonInteractiveEnv)
        } else {
            env[AppEnvironment.keychainNonInteractiveEnv] = "1"
        }
        process.environment = env
        process.standardInput = FileHandle.nullDevice
        let out = Pipe()
        process.standardOutput = out
        process.standardError = FileHandle.nullDevice
        let killAfter = self.killAfter
        return await withCheckedContinuation { (done: CheckedContinuation<KeychainCheckResult, Never>) in
            process.terminationHandler = { [weak self] p in
                self?.lock.withLock { if self?.running === p { self?.running = nil } }
                let text = String(decoding: out.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
                // Stopped or killed: never an answer (least of all access).
                done.resume(returning: p.terminationReason == .uncaughtSignal
                    ? .failed("stopped") : Self.parse(text))
            }
            do {
                try process.run()
            } catch {
                process.terminationHandler = nil
                done.resume(returning: .failed("could not run \(cua): \(error.localizedDescription)"))
                return
            }
            lock.withLock { running = process }
            if let timeout {
                DispatchQueue.global().asyncAfter(deadline: .now() + timeout) { [weak process] in
                    if let process, process.isRunning { Self.stop(process, killAfter: killAfter) }
                }
            }
        }
    }

    /// `cua auth keychain`'s JSON (`cua_auth::KeychainCheck`), after
    /// anything else it printed.
    static func parse(_ text: String) -> KeychainCheckResult {
        let json = text.firstIndex(of: "{").flatMap { start in
            text.lastIndex(of: "}").map { String(text[start...$0]) }
        } ?? ""
        guard let obj = try? JSONSerialization.jsonObject(with: Data(json.utf8)) as? [String: Any],
              let state = obj["state"] as? String else {
            return .failed(text.isEmpty ? "no answer" : String(text.prefix(200)))
        }
        switch state {
        case "ready", "not_used": return .ready
        case "needs_access": return .needsAccess(locked: false)
        case "locked": return .needsAccess(locked: true)
        case "denied": return .denied
        default:
            let items = obj["items"] as? [[String: Any]] ?? []
            return .failed(items.compactMap { $0["error"] as? String }.first ?? state)
        }
    }
}

/// Whether macOS's keychain password prompt is on screen: a SecurityAgent
/// window wider than 100 pt (it keeps a tiny offscreen one when the prompt
/// is gone). Window owners and bounds need no screen recording permission.
public enum SecurityAgentWindow {
    public static func isShowing() -> Bool {
        guard let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] else {
            return true
        }
        return list.contains { w in
            guard (w[kCGWindowOwnerName as String] as? String) == "SecurityAgent",
                  let b = w[kCGWindowBounds as String] as? [String: Any],
                  let width = (b["Width"] as? NSNumber)?.doubleValue else { return false }
            return width > 100
        }
    }
}

// MARK: - The launch

/// The launch, as the window shows it until the live services are in.
@MainActor
@Observable
public final class StartupModel {
    public enum Phase: Equatable, Sendable {
        /// Checking the keychain, starting the daemon and the SDK.
        case starting
        /// The saved sign-in needs the user's permission (no prompt yet).
        case needsKeychain(locked: Bool)
        /// macOS is showing its prompt.
        case waitingForKeychain
        /// The user denied it.
        case keychainDenied
        /// The daemon and the SDK did not start in `startTimeout`.
        case startFailed
        case ready
    }

    public enum Action: String, Sendable, CaseIterable {
        case allowAccess, tryAgain, signInAgain
    }

    /// What the window shows: the host's words, in both UIs.
    public struct Copy: Equatable, Sendable {
        public var title: String
        public var body: String
        /// The buttons, the primary one first.
        public var actions: [Action]
    }

    public private(set) var phase: Phase
    /// Waiting longer than expected (`slowAfter` for the prompt,
    /// `startingSlowAfter` for the daemon).
    public private(set) var slow = false
    /// The prompt we asked for is no longer on screen (closed or hidden).
    public private(set) var promptHidden = false
    /// Why the window asks again ("macOS didn't give access.").
    public private(set) var note: String?
    public var isReady: Bool { phase == .ready }

    /// The keychain check (nil: none, as in fixtures and bare builds).
    public var keychain: KeychainAccessChecking?
    /// Starts the daemon and the SDK and hands them over. May run again
    /// after a failed start (it must cope with an earlier run finishing).
    public var start: (@MainActor () async -> Void)?
    /// Restarts this app's daemon (Try again after a failed start, and once
    /// access was given after the services were in).
    public var restart: (@MainActor () async -> Void)?
    /// After the live services are in.
    public var onReady: [@MainActor () -> Void] = []
    /// Whether the keychain prompt is on screen (nil: unknown).
    public var promptShowing: @MainActor () -> Bool? = { nil }
    public var slowAfter: Duration = .seconds(25)
    /// Once waiting this long, a prompt that is not on screen is reported.
    public var hiddenAfter: Duration = .seconds(25)
    public var startingSlowAfter: Duration = .seconds(20)
    /// "Starting Cua…" never lasts longer than this.
    public var startTimeout: Duration = .seconds(75)

    private var attempt = 0
    /// The live services were handed over (at least once).
    private var servicesIn = false
    private static let notGiven = "macOS didn't give access. Click Allow access to ask again."
    private static let stillWaiting = "Cua is waiting for Keychain access."

    public init(phase: Phase = .ready) { self.phase = phase }

    /// Begins the launch: the quiet keychain check, then the start.
    public func begin() {
        attempt += 1
        let current = attempt
        phase = .starting
        Task { await checkThenLaunch(current, note: nil) }
    }

    /// A button in the window.
    public func act(_ action: Action) {
        switch (action, phase) {
        case (.signInAgain, .needsKeychain), (.signInAgain, .waitingForKeychain),
             (.signInAgain, .keychainDenied):
            Task { await forget() }
        case (.tryAgain, .startFailed):
            Task { await retryStart() }
        case (.allowAccess, .needsKeychain), (.tryAgain, .needsKeychain),
             (.allowAccess, .waitingForKeychain), (.tryAgain, .waitingForKeychain),
             (.allowAccess, .keychainDenied), (.tryAgain, .keychainDenied):
            Task { await ask() }
        default:
            break
        }
    }

    public var copy: Copy {
        switch phase {
        case .ready:
            return Copy(title: "", body: "", actions: [])
        case .starting:
            return slow
                ? Copy(title: "Still starting Cua…", body: "This can take a minute after an update.", actions: [])
                : Copy(title: "Starting Cua…", body: "", actions: [])
        case .needsKeychain(let locked):
            let base = locked
                ? "Cua Spaces keeps your sign-in in the macOS Keychain, which is locked. "
                    + "Click Allow access, then enter your Mac login password."
                : "Cua Spaces keeps your sign-in in the macOS Keychain. This version needs your permission "
                    + "to read it. Click Allow access, then enter your Mac login password and choose Always Allow."
            return Copy(title: locked ? "Unlock your keychain" : "Allow Keychain access",
                        body: note.map { "\($0) \(base)" } ?? base,
                        actions: [.allowAccess, .signInAgain])
        case .waitingForKeychain:
            if promptHidden {
                return Copy(title: "The password prompt isn't showing",
                            body: "The macOS password prompt was closed or is hidden. Click Try again to show it again.",
                            actions: [.tryAgain, .signInAgain])
            }
            let body = "macOS is asking for your login password so Cua Spaces can read your sign-in. "
                + "Look for the prompt (it can be behind other windows). Enter your password and choose Always Allow."
            return slow
                ? Copy(title: "Still waiting for Keychain access", body: body, actions: [.tryAgain, .signInAgain])
                : Copy(title: "Waiting for Keychain access…", body: body, actions: [])
        case .keychainDenied:
            return Copy(title: "Keychain access was not allowed",
                        body: "Cua Spaces can't read your sign-in without it. Try again and choose Always Allow, "
                            + "or sign in again.",
                        actions: [.tryAgain, .signInAgain])
        case .startFailed:
            return Copy(title: "Cua's background service didn't start",
                        body: "Cua Spaces couldn't start its background service. Click Try again to start it again.",
                        actions: [.tryAgain])
        }
    }

    /// The quiet check; then the start, or the "Allow Keychain access" state.
    private func checkThenLaunch(_ current: Int, note: String?) async {
        if let keychain {
            let result = await keychain.check(prompt: false)
            guard current == attempt else { return }
            switch result {
            case .ready:
                break
            case .needsAccess(let locked):
                show(.needsKeychain(locked: locked), note: note)
                return
            case .denied:
                show(.needsKeychain(locked: false), note: note)
                return
            case .failed(let why):
                // A check that cannot run never blocks the launch (and with
                // keychain prompts off, nothing after it can hang on one).
                NSLog("Cua Spaces: the keychain check failed (%@); starting anyway", why)
            }
        }
        await launch(current)
    }

    /// Allow access / Try again: the one prompt, then a quiet check decides.
    private func ask() async {
        attempt += 1
        let current = attempt
        guard let keychain else { await launch(current); return }
        // A prompt still open (Try again) goes first.
        keychain.cancel()
        show(.waitingForKeychain, note: nil)
        markSlow(after: slowAfter, attempt: current)
        watchPrompt(current)
        let result = await keychain.check(prompt: true)
        guard current == attempt else { return }
        if result == .denied {
            show(.keychainDenied, note: nil)
            return
        }
        // Cancelled, killed, failed or "ready": only a fresh check without a
        // prompt says whether access was really given.
        await checkThenLaunch(current, note: Self.notGiven)
    }

    private func forget() async {
        attempt += 1
        let current = attempt
        keychain?.cancel()
        show(.starting, note: nil)
        let result = await keychain?.forget()
        guard current == attempt else { return }
        if case .needsAccess? = result {
            NSLog("Cua Spaces: after Sign in again a Cua keychain item still needs access")
        }
        await launch(current)
    }

    private func retryStart() async {
        attempt += 1
        let current = attempt
        show(.starting, note: nil)
        await restart?()
        guard current == attempt else { return }
        await checkThenLaunch(current, note: Self.stillWaiting)
    }

    private func launch(_ current: Int) async {
        show(.starting, note: nil)
        markSlow(after: startingSlowAfter, attempt: current)
        if servicesIn {
            // Access came after the services were in: the daemon reads the
            // session again.
            await restart?()
        } else {
            watchStart(current)
            await start?()
            servicesIn = true
        }
        guard current == attempt || phase == .startFailed || phase == .starting else { return }
        attempt += 1
        show(.ready, note: nil)
        for f in onReady { f() }
        onReady = []
    }

    /// "Starting Cua…" is bounded: then say what is stuck.
    private func watchStart(_ current: Int) {
        let timeout = startTimeout
        Task { @MainActor [weak self] in
            try? await Task.sleep(for: timeout)
            guard let self, self.attempt == current, self.phase == .starting else { return }
            self.attempt += 1
            let diagnosis = self.attempt
            if let keychain = self.keychain, case .needsAccess(let locked) = await keychain.check(prompt: false) {
                guard self.attempt == diagnosis, self.phase == .starting else { return }
                self.show(.needsKeychain(locked: locked), note: Self.stillWaiting)
            } else {
                guard self.attempt == diagnosis, self.phase == .starting else { return }
                self.show(.startFailed, note: nil)
            }
        }
    }

    /// While the prompt is up: once `hiddenAfter` passed, say so when it is
    /// no longer on screen.
    private func watchPrompt(_ current: Int) {
        let after = hiddenAfter
        Task { @MainActor [weak self] in
            try? await Task.sleep(for: after)
            while let self, self.attempt == current, self.phase == .waitingForKeychain {
                if let showing = self.promptShowing() { self.promptHidden = !showing }
                try? await Task.sleep(for: .seconds(2))
            }
        }
    }

    private func show(_ phase: Phase, note: String?) {
        self.phase = phase
        self.note = note
        slow = false
        promptHidden = false
    }

    private func markSlow(after: Duration, attempt current: Int) {
        Task { @MainActor [weak self] in
            try? await Task.sleep(for: after)
            guard let self, self.attempt == current, self.phase == .starting || self.phase == .waitingForKeychain
            else { return }
            self.slow = true
        }
    }
}

/// A scripted keychain for fixtures and tests: answers `quiet` to the
/// check without a prompt and `prompted` after one (`nil`: the prompt never
/// returns, as when it is left unanswered), and counts every call.
public final class FixtureKeychain: KeychainAccessChecking, @unchecked Sendable {
    public var quiet: KeychainCheckResult
    public var prompted: KeychainCheckResult?
    public var delay: Duration = .zero
    /// A prompt answering `.ready` gives access (the next quiet check reads it).
    public var grants = true
    public private(set) var calls: [String] = []
    private var hung: [CheckedContinuation<KeychainCheckResult, Never>] = []

    public init(quiet: KeychainCheckResult, prompted: KeychainCheckResult? = .ready) {
        self.quiet = quiet
        self.prompted = prompted
    }

    public func check(prompt: Bool) async -> KeychainCheckResult {
        calls.append(prompt ? "prompt" : "check")
        if delay != .zero { try? await Task.sleep(for: delay) }
        if !prompt { return quiet }
        if let prompted {
            // Access given: the next check without a prompt reads it.
            if prompted == .ready, grants { quiet = .ready }
            return prompted
        }
        return await withCheckedContinuation { hung.append($0) }
    }

    public func forget() async -> KeychainCheckResult {
        calls.append("forget")
        return .ready
    }

    public func cancel() {
        let waiting = hung
        hung = []
        if !waiting.isEmpty { calls.append("cancel") }
        for c in waiting { c.resume(returning: .failed("stopped")) }
    }
}

// MARK: - The live services, once made

/// What the launch made off the main thread: the SDK backend and what
/// hangs off it, or (`live` nil) the reason it could not start.
public struct LiveServices: @unchecked Sendable {
    public var backend: SpacesBackend
    public var live: LiveSpacesBackend?
    public var account: AccountRunning?
    public var agentSetup: AgentSetupRunning?
    public var billing: BillingRunning?
    public var devices: DevicesRunning?
    public var host: HostRunning
    public var startError: String?
    public var daemonError: String?

    public init(backend: SpacesBackend, live: LiveSpacesBackend? = nil, account: AccountRunning? = nil,
                agentSetup: AgentSetupRunning? = nil, billing: BillingRunning? = nil,
                devices: DevicesRunning? = nil, host: HostRunning = FixtureHost(),
                startError: String? = nil, daemonError: String? = nil) {
        self.backend = backend
        self.live = live
        self.account = account
        self.agentSetup = agentSetup
        self.billing = billing
        self.devices = devices
        self.host = host
        self.startError = startError
        self.daemonError = daemonError
    }
}

/// A value set once, which callers wait for.
public final class LiveGate<Value: Sendable>: @unchecked Sendable {
    private let lock = NSLock()
    private var value: Value?
    private var waiters: [CheckedContinuation<Value, Never>] = []

    public init() {}

    /// The value, when set.
    public var current: Value? { lock.withLock { value } }

    /// How many calls wait for it now.
    var waiting: Int { lock.withLock { waiters.count } }

    /// Sets it (once; later calls are ignored, answering false) and wakes
    /// every waiter.
    @discardableResult
    public func resolve(_ v: Value) -> Bool {
        let woken: [CheckedContinuation<Value, Never>]? = lock.withLock {
            guard value == nil else { return nil }
            value = v
            defer { waiters = [] }
            return waiters
        }
        for w in woken ?? [] { w.resume(returning: v) }
        return woken != nil
    }

    /// The value, once set.
    public func wait() async -> Value {
        await withCheckedContinuation { (c: CheckedContinuation<Value, Never>) in
            let now: Value? = lock.withLock {
                if let value { return value }
                waiters.append(c)
                return nil
            }
            if let now { c.resume(returning: now) }
        }
    }
}

/// Why a live service is missing (the SDK did not start).
struct NotStarted: LocalizedError {
    let what: String
    var errorDescription: String? { "\(what) needs Cua, which did not start. Reopen Cua Spaces to try again." }
}

// MARK: - Stand-ins that wait for the live services

/// The Spaces backend until the SDK is in: every call waits for it; the
/// synchronous reads answer nothing until then. A reconnect (the list poll
/// went stale, a create was never taken) swaps in a new SDK backend
/// (`replace`): every later call goes to it.
public final class PendingSpacesBackend: SpacesBackend, AgentsToolRunning, CloudToolRunning, @unchecked Sendable {
    let gate: LiveGate<LiveServices>
    private let lock = NSLock()
    private var replaced: SpacesBackend?
    private var replacedLive: LiveSpacesBackend?
    public init(gate: LiveGate<LiveServices>) { self.gate = gate }

    /// Calls from now on go to `backend` (a fresh client of the daemon).
    public func replace(_ backend: SpacesBackend, live: LiveSpacesBackend?) {
        lock.withLock {
            replaced = backend
            replacedLive = live
        }
    }

    /// The backend calls go to now (nil until the SDK is in).
    public var current: SpacesBackend? { lock.withLock { replaced } ?? gate.current?.backend }

    private func b() async -> SpacesBackend {
        if let now = lock.withLock({ replaced }) { return now }
        return await gate.wait().backend
    }

    private func liveNow() async -> LiveSpacesBackend? {
        if let now = lock.withLock({ replacedLive }) { return now }
        return await gate.wait().live
    }

    public func rows() async throws -> [AppSpaceRow] { try await b().rows() }
    public func create(_ args: AppCreateSpaceArgs, createId: String,
                       progress: @escaping @Sendable (SpaceCreateProgress) -> Void) async throws -> String {
        try await b().create(args, createId: createId, progress: progress)
    }
    public func cancelCreate(createId: String) async throws { try await b().cancelCreate(createId: createId) }
    public func gpuChoices() async -> [AppGpuChoice]? { await b().gpuChoices() }
    public func hosts() async -> [AppSpaceHost] { await b().hosts() }
    public func reportedHostname(id: String) -> String? { current?.reportedHostname(id: id) }
    public func add(url: String, token: String?, name: String?) async throws {
        try await b().add(url: url, token: token, name: name)
    }
    public func remove(id: String, removeOnly: Bool) async throws { try await b().remove(id: id, removeOnly: removeOnly) }
    public func setPower(id: String, on: Bool) async throws { try await b().setPower(id: id, on: on) }
    public func streamProvider(id: String) async throws -> SpaceStreamSourceProviding {
        try await b().streamProvider(id: id)
    }
    public func localBackends() async -> [String]? { await b().localBackends() }
    public func localRuntimes() async -> (ready: [String], details: [String: String])? { await b().localRuntimes() }
    public func lumeSource() async -> String? { await b().lumeSource() }
    public func setLumeSource(_ value: String) async throws { try await b().setLumeSource(value) }
    public func linuxSource() async -> String? { await b().linuxSource() }
    public func setLinuxSource(_ value: String) async throws { try await b().setLinuxSource(value) }
    public func localStorage() async -> LocalStorage? { await b().localStorage() }
    public func cloudAvailable() async -> Bool { await b().cloudAvailable() }
    public func runningMacosVms() async -> Int? { await b().runningMacosVms() }
    public func cloudPricing() async -> AppCloudPricing? { await b().cloudPricing() }
    public func teleportContext(id: String) async throws -> (CuaSpacesFFI.Teleport, CuaSDK.Space)? {
        try await b().teleportContext(id: id)
    }
    public func teleportHandle() -> CuaSpacesFFI.Teleport? { current?.teleportHandle() }
    public func sendFiles(id: String, paths: [String]) async throws -> [AppSentFileInfo] {
        try await b().sendFiles(id: id, paths: paths)
    }
    public func agentRuns(id: String) async throws -> [AppSpaceAgentRun] { try await b().agentRuns(id: id) }
    public func thumbnail(id: String, maxAgeMs: UInt64?) async -> SpaceThumbnailData? {
        await b().thumbnail(id: id, maxAgeMs: maxAgeMs)
    }
    public func appIcons(id: String, requests: [SpaceAppIconRequest]) async -> [Data?] {
        await b().appIcons(id: id, requests: requests)
    }
    public func primaryDisplay(id: String) async -> AppStreamDisplay? { await b().primaryDisplay(id: id) }
    public func usage(id: String) async -> AppSpaceUsage? { await b().usage(id: id) }
    public func shares(id: String) async throws -> [AppShareEntryInput] { try await b().shares(id: id) }
    public func share(id: String, who: String, role: String) async throws -> [AppShareEntryInput] {
        try await b().share(id: id, who: who, role: role)
    }
    public func unshare(id: String, who: String) async throws -> [AppShareEntryInput] {
        try await b().unshare(id: id, who: who)
    }

    public func agentsTool(_ tool: String, _ args: [String: Any]) async throws -> Any {
        guard let tools = await b() as? AgentsToolRunning else { throw NotStarted(what: "This") }
        return try await tools.agentsTool(tool, args)
    }

    public func cloudTool(_ tool: String, _ args: [String: Any]) async throws -> Any {
        guard let live = await liveNow() else { throw NotStarted(what: "Clouds") }
        return try await live.cloudTool(tool, args)
    }
}

/// The account until the SDK is in: no identity yet; sign-in waits.
public final class PendingAccount: AccountRunning, @unchecked Sendable {
    let gate: LiveGate<LiveServices>
    public init(gate: LiveGate<LiveServices>) { self.gate = gate }

    public func identity() -> String? { gate.current?.account?.identity() }
    public func profile() -> AccountProfile? { gate.current?.account?.profile() }
    public func beginSignIn() async throws -> SignInAttempt {
        guard let account = await gate.wait().account else { throw NotStarted(what: "Signing in") }
        return try await account.beginSignIn()
    }
    public func signOut() async throws { try await gate.wait().account?.signOut() }
}

/// The coding agents' setup until the SDK is in.
public final class PendingAgentSetup: AgentSetupRunning, @unchecked Sendable {
    let gate: LiveGate<LiveServices>
    public init(gate: LiveGate<LiveServices>) { self.gate = gate }

    private func s() async throws -> AgentSetupRunning {
        guard let s = await gate.wait().agentSetup else { throw NotStarted(what: "Agent setup") }
        return s
    }
    public func statuses() async -> [AppAgentSetupStatus] { await gate.wait().agentSetup?.statuses() ?? [] }
    public func skillsTotal() -> UInt32 { gate.current?.agentSetup?.skillsTotal() ?? 0 }
    public func setUp(agents: [String], skills: Bool, mcp: Bool) async throws -> [AppAgentSetupOutcomeInput] {
        try await s().setUp(agents: agents, skills: skills, mcp: mcp)
    }
    public func setUpCuaDriver(agents: [String]) async throws -> [AppAgentSetupOutcomeInput] {
        try await s().setUpCuaDriver(agents: agents)
    }
    public func remove(agents: [String]) async throws -> [AppAgentSetupOutcomeInput] {
        try await s().remove(agents: agents)
    }
}

/// Cua Cloud billing until the SDK is in.
public final class PendingBilling: BillingRunning, @unchecked Sendable {
    let gate: LiveGate<LiveServices>
    public init(gate: LiveGate<LiveServices>) { self.gate = gate }

    public func status() async throws -> AppBillingStatus {
        guard let b = await gate.wait().billing else { throw NotStarted(what: "Billing") }
        return try await b.status()
    }
}

/// This Mac's relay devices until the SDK is in.
public final class PendingDevices: DevicesRunning, @unchecked Sendable {
    let gate: LiveGate<LiveServices>
    public init(gate: LiveGate<LiveServices>) { self.gate = gate }

    private func d() async throws -> DevicesRunning {
        guard let d = await gate.wait().devices else { throw NotStarted(what: "Devices") }
        return d
    }
    public func snapshot() async throws -> DevicesSnapshot { try await d().snapshot() }
    public func enroll() async throws -> DeviceEnrollment { try await d().enroll() }
    public func checkEnrolled() async -> Bool {
        guard let d = try? await d() else { return false }
        return await d.checkEnrolled()
    }
    public func approve(code: String?, deviceId: String?) async throws {
        try await d().approve(code: code, deviceId: deviceId)
    }
    public func rename(id: String, name: String) async throws { try await d().rename(id: id, name: name) }
    public func revoke(id: String) async throws { try await d().revoke(id: id) }
    public func confirmMachine(id: String) async throws { try await d().confirmMachine(id: id) }
}

/// This machine's host service until the SDK is in.
public final class PendingHost: HostRunning, @unchecked Sendable {
    let gate: LiveGate<LiveServices>
    public init(gate: LiveGate<LiveServices>) { self.gate = gate }

    private func h() async -> HostRunning { await gate.wait().host }
    public func status() async throws -> HostStatus { try await h().status() }
    public func setupRequest(request: AppHostSetupRequest, accountToken: String?) async throws -> HostStatus {
        try await h().setupRequest(request: request, accountToken: accountToken)
    }
    public func stopSharing() async throws -> HostStatus { try await h().stopSharing() }
    public func startSharing() async throws -> HostStatus { try await h().startSharing() }
    public func pauseSignedOut() async throws -> HostStatus { try await h().pauseSignedOut() }
    public func resumeSignedIn(account: String) async throws -> HostStatus {
        try await h().resumeSignedIn(account: account)
    }
    public func remove() async throws { try await h().remove() }
    public func configure(change: HostSettingsChange) async throws -> HostStatus {
        try await h().configure(change: change)
    }
}
