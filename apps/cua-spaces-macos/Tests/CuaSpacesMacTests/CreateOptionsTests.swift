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
        let start = Date()
        let options = try json(try await bridge.handle("spaces.createOptions", [:]))
        // Every probe would wait for the services: none ran.
        #expect(Date().timeIntervalSince(start) < 2)
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

    /// A probe that never stops (an SDK call cannot be cancelled; a macOS
    /// privacy prompt blocks a file read) still gives up on time.
    @Test func aTimeoutReturnsAtItsDeadline() async {
        let start = Date()
        let result = await withTimeout(seconds: 0.2) { () async -> Int in
            await withUnsafeContinuation { c in
                DispatchQueue.global().asyncAfter(deadline: .now() + 3) { c.resume(returning: 1) }
            }
        }
        #expect(Date().timeIntervalSince(start) < 2.5)
        guard case .failure(let error) = result else { Issue.record("expected the timeout"); return }
        #expect(error is TimeoutError)
        let quick = await withTimeout(seconds: 5) { 7 }
        #expect((try? quick.get()) == 7)
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
