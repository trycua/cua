// The shared sandbox model (SandboxSpec + PoolOptions) and the pool writer
// (Cua Cloud, closed: it says so).
import Foundation
import Testing

@testable import Cua

@Suite struct PoolTests {
    @Test func recordsHaveDefaults() {
        let spec = SandboxSpec(image: "python:3.12-slim", command: ["python", "-m", "srv"])
        #expect(spec.env.isEmpty && spec.services.isEmpty && spec.sidecars.isEmpty)
        #expect(!spec.claimSecrets && !spec.efi)
        let opts = PoolOptions(warm: true, idleTtlSeconds: 3600, ttlPolicy: "Cascade")
        #expect(opts.maxPoolSize == nil)
        #expect(CloudOptions(pool: "p", apply: true).apply)
        #expect(fleetGenerateClaimToken().count == 64)
    }

    @Test func sandboxRefsParseAndAmbiguityListsCandidates() throws {
        for ref in ["local:box", "cloud:box", "direct:10.0.0.5:3211", "relay:0123abcd4567ef89"] {
            #expect(try parseSandboxRef(input: ref).id == ref)
        }
        let legacy = try parseSandboxRef(input: "space://fleet/ns/box")
        #expect(legacy.location == "cloud" && legacy.name == "box" && legacy.id == "cloud:box")
        #expect(try parseSandboxRef(input: "box").location == nil)
        #expect(try qualifySandboxRef(name: "box", local: true) == "local:box")
        #expect(try qualifySandboxRef(name: "box", local: false) == "cloud:box")
        #expect(throws: CuaError.self) { try qualifySandboxRef(name: "cloud:box", local: true) }
        let e = CuaError.AmbiguousSandbox(
            message: "\"box\" names 2 sandboxes; use one of: local:box, cloud:box")
        #expect(e.ambiguousCandidates == ["local:box", "cloud:box"])
        #expect(CuaError.NotFound(message: "x").ambiguousCandidates.isEmpty)
    }

    @Test func poolApplySaysCuaCloudHasClosed() async throws {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("cua-swift-pool-\(UUID().uuidString)")
        let cua = try Cua.embedded(
            stateDir: dir.path,
            fleet: FleetSettings(baseUrl: "http://127.0.0.1:9", token: "t"),
            fleetFromEnv: false)
        let fleet = try cua.fleet()
        let spec = SandboxSpec(image: "ghcr.io/trycua/cua-e2e-swift@sha256:0123")
        do {
            _ = try await Pool.apply(
                fleet, name: "cua-e2e-swift-apply", spec: spec,
                options: PoolOptions(runtime: "gvisor"))
            Issue.record("expected the closure")
        } catch CuaError.Fleet(let message) {
            #expect(message.contains("Cua Cloud has closed"))
        }
    }
}
