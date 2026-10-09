// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

@testable import CuaSpacesMacKit
import CuaSDK
import CuaSpacesFFI
import Foundation
import Testing

/// New UI's `spaces.createOptions` on the Mac: the exact shape the web
/// bridge reads (`NewSpaceOptions`), at once while the launch is still
/// starting, and in full once the live services are in.
@Suite("New Space options on the Mac")
@MainActor
struct CreateOptionsTests {
    static let gb: UInt64 = 1 << 30

    func bridge(_ backend: SpacesBackend) -> (WebUIBridge, AppModel) {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("cua-options-\(UUID().uuidString)")
        let model = AppModel(backend: backend, keyvault: KeyvaultModel(client: nil),
                             onboarding: OnboardingModel(statePath: dir.appendingPathComponent("onboarding.json").path),
                             settingsPath: dir.appendingPathComponent("settings.json").path,
                             telemetry: FixtureTelemetry())
        return (WebUIBridge(model: model, allowedOrigins: []), model)
    }

    /// A Mac like the one QA ran on: Docker Desktop and Lume ready, room on
    /// Macintosh HD, Linux pulled, two macOS VMs of its own running.
    func liveMac() -> FixtureSpacesBackend {
        let backend = FixtureSpacesBackend(rows: [])
        backend.fixtureBackends = ["container", "lume", "qemu"]
        let hd = StorageVolume(availableBytes: 82 * Self.gb, totalBytes: 1_800 * Self.gb, name: "Macintosh HD")
        backend.fixtureStorage = LocalStorage(hostArch: "arm64", reserveBytes: 5 * Self.gb, lume: hd, qemu: hd,
                                              container: hd, pulled: ["ghcr.io/trycua/linux:24.04"])
        backend.fixtureMacosVms = 2
        return backend
    }

    /// What WebKit hands the page: JSON, read back as the page reads it.
    func json(_ value: Any) throws -> [String: Any] {
        let data = try JSONSerialization.data(withJSONObject: value)
        return try #require(try JSONSerialization.jsonObject(with: data) as? [String: Any])
    }

    func decodedEnv(_ options: [String: Any]) throws -> AppWizardEnv {
        let env = try #require(options["env"] as? [String: Any])
        return try appWizardEnvFromJson(json: String(decoding: try JSONSerialization.data(withJSONObject: env), as: UTF8.self))
    }

    @Test func theAnswerHasTheShapeThePageReads() async throws {
        let (bridge, model) = bridge(liveMac())
        let options = try json(try await bridge.handle("spaces.createOptions", [:]))
        #expect(Set(options.keys) == ["local", "gpus", "cloudPricing", "experiments", "maxCpus", "env", "macosVmsRunning"])
        #expect(options["pending"] == nil)
        #expect(options["macosVmsRunning"] as? Int == 2)
        // The env the core takes, unchanged.
        #expect(try decodedEnv(options) == (await model.newSpaceEnv()))
        let env = try #require(options["env"] as? [String: Any])
        let storage = try #require(env["storage"] as? [String: Any])
        #expect(Set(storage.keys) == ["reserveBytes", "lume", "qemu", "container", "pulled"])
        let container = try #require(storage["container"] as? [String: Any])
        #expect(Set(container.keys) == ["availableBytes", "totalBytes", "name"])
        #expect(container["name"] as? String == "Macintosh HD")
        #expect((container["availableBytes"] as? NSNumber)?.uint64Value == 82 * Self.gb)
        #expect(storage["pulled"] as? [String] == ["ghcr.io/trycua/linux:24.04"])
        #expect(env["localBackends"] as? [String] == ["container", "lume", "qemu"])
        #expect(env["hostArch"] as? String == "arm64")
        // `local` says the same, for a page that reads it.
        let local = try #require(options["local"] as? [String: Any])
        #expect(local["available"] as? Bool == true)
        #expect(local["backends"] as? [String] == ["container", "lume", "qemu"])
        #expect(local["hostArch"] as? String == "arm64")
        #expect(NSDictionary(dictionary: try #require(local["storage"] as? [String: Any])).isEqual(to: storage))
    }

    @Test func whileStartingItAnswersAtOnceAndSaysSo() async throws {
        let gate = LiveGate<LiveServices>()
        let (bridge, model) = bridge(PendingSpacesBackend(gate: gate))
        #expect(!model.servicesIn)
        let options = try json(try await bridge.handle("spaces.createOptions", [:]))
        // Every probe would wait for the services (and keep waiting after
        // its timeout): none ran.
        #expect(gate.waiting == 0)
        #expect(options["pending"] as? Bool == true)
        #expect(options["macosVmsRunning"] is NSNull)
        let env = try decodedEnv(options)
        #expect(env.storage == nil && env.localBackends == nil && env.localAvailable)

        // The services arrive: the page asks again and gets the Mac's.
        gate.resolve(LiveServices(backend: liveMac()))
        #expect(model.servicesIn)
        let live = try json(try await bridge.handle("spaces.createOptions", [:]))
        #expect(live["pending"] == nil)
        #expect(live["macosVmsRunning"] as? Int == 2)
        let liveEnv = try decodedEnv(live)
        #expect(liveEnv.storage?.container?.name == "Macintosh HD")
        #expect(liveEnv.storage?.pulled == ["ghcr.io/trycua/linux:24.04"])
    }

    /// A deadline the test fires by hand, noting where it was set.
    final class ManualDeadline: @unchecked Sendable {
        private let lock = NSLock()
        private var fires: [@Sendable () -> Void] = []
        private var setAt: [(seconds: Double, onMain: Bool)] = []
        var schedule: DeadlineScheduler {
            { [self] seconds, fire in
                lock.withLock {
                    setAt.append((seconds, Thread.isMainThread))
                    fires.append(fire)
                }
            }
        }
        /// Each deadline set: its seconds, and whether on the main thread.
        var set: [(seconds: Double, onMain: Bool)] { lock.withLock { setAt } }
        var count: Int { lock.withLock { setAt.count } }
        func fire() { lock.withLock { fires.removeFirst() }() }
    }

    /// A call that runs until the test lets it go (an SDK call does not
    /// stop when cancelled; a file read waits behind a privacy prompt).
    final class StuckCall: @unchecked Sendable {
        private let lock = NSLock()
        private var waiting: CheckedContinuation<Void, Never>?
        private var released = false
        private var started = false
        private var finished = false
        private var cancelled = false
        func run() async -> Int {
            lock.withLock { started = true }
            await withCheckedContinuation { (c: CheckedContinuation<Void, Never>) in
                let now = lock.withLock { () -> Bool in
                    if released { return true }
                    waiting = c
                    return false
                }
                if now { c.resume() }
            }
            lock.withLock { finished = true; cancelled = Task.isCancelled }
            return 1
        }
        func release() {
            let c = lock.withLock { () -> CheckedContinuation<Void, Never>? in
                released = true
                defer { waiting = nil }
                return waiting
            }
            c?.resume()
        }
        var isStarted: Bool { lock.withLock { started } }
        var isFinished: Bool { lock.withLock { finished } }
        var sawCancel: Bool { lock.withLock { cancelled } }
    }

    /// Waits for `condition` (at most a minute; the run may be loaded).
    func eventually(_ condition: () -> Bool) async -> Bool {
        var tries = 0
        while !condition(), tries < 6000 {
            tries += 1
            try? await Task.sleep(for: .milliseconds(10))
        }
        return condition()
    }

    /// A probe that never stops still gives up: its deadline decides,
    /// without waiting for the call. The deadline is fired by hand, so a
    /// loaded run (other tests holding the main actor) cannot make it look
    /// late; what is checked is what decides the answer.
    @Test(.timeLimit(.minutes(2))) func aTimeoutReturnsAtItsDeadline() async throws {
        let deadline = ManualDeadline()
        let stuck = StuckCall()
        final class Answer: @unchecked Sendable { var result: Result<Int, Error>? }
        let answer = Answer()
        let call = Task { @MainActor in
            let r = await withTimeout(seconds: 0.2, deadline: deadline.schedule) { await stuck.run() }
            answer.result = r
            return r
        }
        #expect(await eventually { deadline.count == 1 && stuck.isStarted })
        // Set at once on the caller's actor (the main actor here), not on a
        // shared thread a blocked SDK call may be holding.
        #expect(deadline.set.map(\.seconds) == [0.2])
        #expect(deadline.set.map(\.onMain) == [true])
        // Nothing decided yet: the call is still running.
        #expect(answer.result == nil)

        deadline.fire()
        let result = await call.value
        guard case .failure(let error) = result else { Issue.record("expected the timeout, got \(result)"); return }
        #expect(error is TimeoutError)
        // The answer did not wait for the call, which is told to stop.
        #expect(!stuck.isFinished)
        stuck.release()
        #expect(await eventually { stuck.isFinished })
        #expect(stuck.sawCancel)

        // A call that answers first wins; its deadline firing later changes nothing.
        let late = ManualDeadline()
        let quick = await withTimeout(seconds: 5, deadline: late.schedule) { 7 }
        #expect((try? quick.get()) == 7)
        late.fire()
        // The dispatch timer is the default.
        #expect((try? await withTimeout(seconds: 5) { 7 }.get()) == 7)
    }

    @Test func runningMacosVmsCountsLumesRunningMacs() {
        let vms = """
        [{"name":"a","os":"macOS","status":"running"},{"name":"b","os":"macOS","status":"stopped"},
         {"name":"c","os":"linux","status":"running"},{"name":"d","os":"macOS","status":"running"}]
        """
        #expect(LiveSpacesBackend.runningMacosVms(lumeVms: Data(vms.utf8)) == 2)
        #expect(LiveSpacesBackend.runningMacosVms(lumeVms: Data("nope".utf8)) == nil)
    }
}
