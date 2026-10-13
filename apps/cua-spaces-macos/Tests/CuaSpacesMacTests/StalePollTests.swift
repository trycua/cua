// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

@testable import CuaSpacesMacKit
import CuaSDK
import CuaSpacesFFI
import CuaSpacesStreaming
import Foundation
import Testing

/// The Space list and creates survive a daemon connection that stops
/// answering (after a relaunch next to a running daemon, the
/// list went stale, a create sat at Creating for 4 min and never reached the
/// daemon). The fakes hang the way an SDK call does: forever, and deaf to
/// task cancellation. No real daemon, keychain or SDK is touched.
@MainActor
@Suite struct StalePollTests {
    /// A daemon connection: answers until `wedge()`, then every list and
    /// create hangs for good.
    final class Connection: SpacesBackend, @unchecked Sendable {
        let inner: FixtureSpacesBackend
        private let lock = NSLock()
        private var isWedged = false
        private(set) var listCalls = 0
        private(set) var createCalls = 0
        /// Holds every hung call (never resumed).
        private var hung: [CheckedContinuation<Void, Never>] = []

        init(rows: [AppSpaceRow]) { inner = FixtureSpacesBackend(rows: rows) }

        func wedge() { lock.withLock { isWedged = true } }
        var wedged: Bool { lock.withLock { isWedged } }
        /// Every list read answers with this error (the daemon's own).
        private var refusal: Error?
        func refuse(_ error: Error?) { lock.withLock { refusal = error } }

        /// Never returns, cancelled or not (a UniFFI future).
        private func hang() async {
            await withCheckedContinuation { c in lock.withLock { hung.append(c) } }
        }

        func rows() async throws -> [AppSpaceRow] {
            lock.withLock { listCalls += 1 }
            if wedged { await hang() }
            if let refusal = lock.withLock({ refusal }) { throw refusal }
            return try await inner.rows()
        }

        func create(_ args: AppCreateSpaceArgs, createId: String,
                    progress: @escaping @Sendable (SpaceCreateProgress) -> Void) async throws -> String {
            lock.withLock { createCalls += 1 }
            if wedged { await hang() }
            // The daemon says "preparing" as it takes the create.
            progress(SpaceCreateProgress(phase: "preparing", fraction: nil, detail: ""))
            return try await inner.create(args, createId: createId, progress: progress)
        }

        func cancelCreate(createId: String) async throws {
            if wedged { await hang() }
            try await inner.cancelCreate(createId: createId)
        }
        func add(url: String, token: String?, name: String?) async throws { try await inner.add(url: url, token: token, name: name) }
        func remove(id: String, removeOnly: Bool) async throws { try await inner.remove(id: id, removeOnly: removeOnly) }
        func setPower(id: String, on: Bool) async throws { try await inner.setPower(id: id, on: on) }
        func streamProvider(id: String) async throws -> SpaceStreamSourceProviding { try await inner.streamProvider(id: id) }
        func localBackends() async -> [String]? { ["docker"] }
        func localStorage() async -> LocalStorage? { nil }
        func cloudAvailable() async -> Bool { false }
        func teleportContext(id: String) async throws -> (CuaSpacesFFI.Teleport, CuaSDK.Space)? { nil }
        func teleportHandle() -> CuaSpacesFFI.Teleport? { nil }
        func sendFiles(id: String, paths: [String]) async throws -> [AppSentFileInfo] { [] }
        func agentRuns(id: String) async throws -> [AppSpaceAgentRun] { [] }
    }

    static func row(_ id: String) -> AppSpaceRow {
        AppSpaceRow(id: id, name: String(id.split(separator: ":").last ?? ""), provider: "local",
                    spacesdVersion: "0.5.3", features: ["desktop_stream"], addedAt: nil, os: .linux,
                    osName: nil, osPrettyName: nil, image: "ghcr.io/trycua/linux:24.04", imageDigest: nil,
                    kind: .container, arch: nil, reachable: true, error: nil, host: nil, hostName: nil,
                    power: nil, powerState: "running", cloud: nil, cloudPlace: nil, cloudDelete: nil)
    }

    func eventually(_ seconds: Double = 120, _ condition: @MainActor () -> Bool) async -> Bool {
        let deadline = Date().addingTimeInterval(seconds)
        while Date() < deadline {
            if condition() { return true }
            try? await Task.sleep(for: .milliseconds(10))
        }
        return condition()
    }

    /// The app as `AppEnvironment` wires it: the stand-in backend over the
    /// gate, and a reconnect that hands it the next connection.
    func makeModel(gate: LiveGate<LiveServices>, startup: StartupModel? = nil,
                   next: @escaping @MainActor () -> Connection?) -> AppModel {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("cua-stale-\(UUID().uuidString)")
        let pending = PendingSpacesBackend(gate: gate)
        let model = AppModel(backend: pending, keyvault: KeyvaultModel(client: nil),
                             onboarding: OnboardingModel(statePath: dir.appendingPathComponent("onboarding.json").path),
                             settingsPath: dir.appendingPathComponent("settings.json").path,
                             telemetry: FixtureTelemetry(), startup: startup ?? StartupModel())
        // Short, with room for a loaded test run.
        model.listTimeout = 1
        model.hostTimeout = 1
        model.listStaleAfter = 3
        model.createAcceptTimeout = 2
        model.reconnect = {
            guard let connection = next() else { return false }
            pending.replace(connection, live: nil)
            return true
        }
        return model
    }

    func args(_ name: String) -> AppCreateSpaceArgs {
        AppCreateSpaceArgs(image: "ghcr.io/trycua/linux:24.04", on: "local", kind: .container, runtime: .auto,
                           name: name, cpus: nil, memoryMb: nil, diskGb: nil, spacesd: true, gpu: nil)
    }

    // MARK: - The bound

    @Test func aTimeoutReturnsWhenTheCallNeverDoes() async throws {
        let started = Date()
        let result = await withTimeout(seconds: 0.2) { () async throws -> Int in
            // Deaf to cancellation, as a UniFFI future is.
            await withCheckedContinuation { (_: CheckedContinuation<Void, Never>) in }
            return 1
        }
        guard case .failure(TimeoutError.timedOut) = result else {
            Issue.record("expected a timeout, got \(result)")
            return
        }
        // Well before "never" (the run may be loaded).
        #expect(Date().timeIntervalSince(started) < 60)
        #expect(try await withTimeout(seconds: 5) { 7 }.get() == 7)
    }

    // MARK: - A stale list reconnects

    @Test func aListPollThatHangsReconnectsAndShowsWhatTheCLIMade() async throws {
        let first = Connection(rows: [Self.row("local:e2e-1005-linux")])
        let gate = LiveGate<LiveServices>()
        gate.resolve(LiveServices(backend: first))
        // The second connection sees the Space `cua` made meanwhile.
        let second = Connection(rows: [Self.row("local:e2e-1005-linux"), Self.row("local:e2e-1005-cli")])
        var made = 0
        let model = makeModel(gate: gate) { made += 1; return second }
        let poll = model.startListPoll(every: .milliseconds(50))
        defer { poll.cancel() }
        #expect(await eventually { model.spaces.contains { $0.id == "local:e2e-1005-linux" } })

        first.wedge()
        #expect(await eventually { model.spaces.contains { $0.id == "local:e2e-1005-cli" } })
        // At least once (a loaded run may also find the first read late).
        #expect(model.reconnects >= 1)
        #expect(made == model.reconnects)
        // A hung read is asked once, not once a poll.
        #expect(first.listCalls <= 3)
        // No banner for a read that only timed out.
        #expect(model.banner == nil)
    }

    /// Signed out on Windows and Linux, every read failed with the relay's
    /// refusal and the app reconnected every 50 s: a daemon that answers
    /// is not a stale connection. The poll's steps run by hand on a stepped
    /// clock, so a loaded run cannot make a read late (it failed on a
    /// 3-core CI runner when the real 3 s staleness passed between polls).
    @Test func aListTheDaemonRefusesIsNotAStaleConnection() async throws {
        let first = Connection(rows: [Self.row("local:a")])
        first.refuse(CuaError.Unauthenticated(message: "Relay authentication failed. Sign in again to refresh your machines."))
        let gate = LiveGate<LiveServices>()
        gate.resolve(LiveServices(backend: first))
        let model = makeModel(gate: gate) { first }
        // Nothing times out on real time; only the stepped clock moves.
        model.listTimeout = 600
        model.hostTimeout = 600
        model.listStaleAfter = 45
        var now = Date()
        model.listClock = { now }
        model.watchListFromNow()
        /// One poll, `seconds` after the last.
        func poll(after seconds: TimeInterval) async {
            now += seconds
            await model.refresh()
            await model.checkListHealth()
        }
        await poll(after: 0)
        #expect(model.rosterError != nil)
        // Ten minutes of refusals, a poll every 30 s: never stale.
        for _ in 0..<20 { await poll(after: 30) }
        #expect(model.reconnects == 0)
        #expect(first.listCalls == 21)

        // A connection that broke is made again, once it stayed so past
        // the staleness (and not before).
        first.refuse(CuaError.DaemonNotRunning(message: "The cua daemon is not running."))
        await poll(after: 30)
        #expect(model.reconnects == 0)
        await poll(after: 30)
        #expect(model.reconnects == 1)
        #expect(AppModel.daemonAnswered(CuaError.Transport(message: "reset")) == false)
    }

    @Test func aReconnectThatFailsIsTriedAgainLater() async {
        let first = Connection(rows: [Self.row("local:a")])
        first.wedge()
        let gate = LiveGate<LiveServices>()
        gate.resolve(LiveServices(backend: first))
        let second = Connection(rows: [Self.row("local:a"), Self.row("local:b")])
        var tries = 0
        let model = makeModel(gate: gate) {
            tries += 1
            return tries < 2 ? nil : second
        }
        let poll = model.startListPoll(every: .milliseconds(50))
        defer { poll.cancel() }
        #expect(await eventually { model.spaces.contains { $0.id == "local:b" } })
        #expect(tries >= 2)
    }

    // MARK: - The startup gate

    @Test func theListIsReadOnceTheGateResolvesAndNeverReconnectsBefore() async {
        let gate = LiveGate<LiveServices>()
        let startup = StartupModel(phase: .starting)
        let live = Connection(rows: [Self.row("local:e2e-1005-cli")])
        let model = makeModel(gate: gate, startup: startup) { live }
        let poll = model.startListPoll(every: .milliseconds(50))
        defer { poll.cancel() }
        // Waiting for the daemon (the keychain, "Starting Cua…"): reads wait on
        // the gate, and that is not a stale list.
        try? await Task.sleep(for: .seconds(4))
        #expect(model.reconnects == 0)
        #expect(model.spaces.isEmpty)

        startup.start = { [weak model] in
            gate.resolve(LiveServices(backend: live))
            model?.attachLive(nil)
        }
        startup.begin()
        #expect(await eventually { startup.isReady })
        // The read that waited on the gate goes on to the live backend.
        #expect(await eventually { model.spaces.contains { $0.id == "local:e2e-1005-cli" } })
    }

    // MARK: - Creates

    @Test func aCreateTheDaemonNeverTakesFailsFastAndTryAgainWorks() async throws {
        let first = Connection(rows: [])
        let gate = LiveGate<LiveServices>()
        gate.resolve(LiveServices(backend: first))
        let second = Connection(rows: [])
        let model = makeModel(gate: gate) { second }
        model.listStaleAfter = 600
        first.wedge()

        let started = Date()
        do {
            _ = try await model.runCreate(args("e2e-1005-linux"), os: .linux, pendingId: "pending:one")
            Issue.record("the create should have failed")
        } catch let e as AppModel.CreateNotAccepted {
            #expect(e.seconds == 2)
        }
        #expect(Date().timeIntervalSince(started) < 60)
        let row = model.creates.pending.first { $0.id == "pending:one" }
        #expect(row?.error?.contains("didn't start this create") == true)
        #expect(row?.error?.contains("Try again") == true)
        // The connection is made again for the next try.
        #expect(await eventually { model.reconnects == 1 })

        model.delete(try #require(model.spaces.first { $0.id == "pending:one" }))
        // A loaded run must not fail the healthy create or its list read.
        model.createAcceptTimeout = 60
        model.listTimeout = 60
        let id = try await model.runCreate(args("e2e-1005-linux"), os: .linux, pendingId: "pending:two")
        #expect(id == "local:e2e-1005-linux")
        #expect(second.createCalls == 1)
        #expect(model.spaces.contains { $0.id == id })
    }

    @Test func aTakenCreateMayRunLongerThanTheWatchdog() async throws {
        let connection = Connection(rows: [])
        connection.inner.holdCreates = true
        let gate = LiveGate<LiveServices>()
        gate.resolve(LiveServices(backend: connection))
        let model = makeModel(gate: gate) { nil }
        let create = Task { try await model.runCreate(args("slow"), os: .linux, pendingId: "pending:slow") }
        try await Task.sleep(for: .seconds(3))
        connection.inner.releaseCreate()
        let id = try await create.value
        #expect(id == "local:slow")
        #expect(model.reconnects == 0)
    }

    // MARK: - The daemon this app found running

    @Test func anAdoptedDaemonThatDiesIsStartedAgainAndTheAppReconnects() async throws {
        let supervisor = try #require(DaemonSupervisor(bundledCua: "/bin/sh"))
        final class Daemon: @unchecked Sendable {
            let lock = NSLock()
            var up = true
            var starts = 0
            /// A probe of a dead connection may hang instead of failing.
            var probeHangs = false
            var restarted = 0
        }
        let daemon = Daemon()
        let task = supervisor.supervise(
            interval: .milliseconds(50),
            isUp: {
                if daemon.lock.withLock({ daemon.probeHangs }) {
                    let r = await withTimeout(seconds: 0.2) { () async throws -> Bool in
                        await withCheckedContinuation { (_: CheckedContinuation<Void, Never>) in }
                        return true
                    }
                    return (try? r.get()) ?? false
                }
                return daemon.lock.withLock { daemon.up }
            },
            report: { _ in },
            restarted: { daemon.lock.withLock { daemon.restarted += 1 } },
            start: {
                daemon.lock.withLock {
                    daemon.starts += 1
                    daemon.up = true
                    daemon.probeHangs = false
                }
                return nil
            })
        defer { task.cancel() }
        try await Task.sleep(for: .milliseconds(300))
        #expect(daemon.lock.withLock { daemon.starts } == 0)

        // Killed: its old connection's probe never answers.
        daemon.lock.withLock {
            daemon.up = false
            daemon.probeHangs = true
        }
        #expect(await eventually { daemon.lock.withLock { daemon.starts } == 1 })
        #expect(await eventually { daemon.lock.withLock { daemon.restarted } == 1 })
    }
}
