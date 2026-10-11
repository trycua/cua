// The shared sandbox model (SandboxSpec + PoolOptions) and the one pool
// writer against the fixtures' fake Fleet (loopback only).
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

    @Test(.enabled(if: Fixtures.binary() != nil, "cua-test-fixtures is not built"))
    func applyMismatchReconcileExport() async throws {
        let fx = try #require(try Fixtures.start())
        defer { fx.stop() }
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("cua-swift-pool-\(UUID().uuidString)")
        let cua = try Cua.embedded(
            stateDir: dir.path,
            fleet: FleetSettings(
                baseUrl: fx.fields["fleet_base_url"],
                token: fx.fields["fleet_token"]),
            fleetFromEnv: false)
        let fleet = try cua.fleet()
        let name = "cua-e2e-swift-apply"
        let spec = SandboxSpec(
            image: "ghcr.io/trycua/cua-e2e-swift@sha256:0123",
            command: ["python", "-m", "srv"], services: ["mcp": 8765], cpu: 2, memoryMb: 2048)
        let pool = try await Pool.apply(
            fleet, name: name, spec: spec,
            options: PoolOptions(runtime: "gvisor", warm: true, idleTtlSeconds: 3600))
        #expect(pool.name == name)
        #expect(pool.replicas == 1)

        try await fleet.checkPoolSpec(pool: name, spec: spec)
        let other = SandboxSpec(command: ["node", "srv.js"], cpu: 4)
        do {
            try await fleet.checkPoolSpec(pool: name, spec: other)
            Issue.record("expected PoolSpecMismatch")
        } catch CuaError.PoolSpecMismatch(let message) {
            #expect(message.contains("cpu: pool has 2, requested 4"))
        }
        try await fleet.applyPoolTemplate(pool: name, spec: other)
        try await fleet.checkPoolSpec(pool: name, spec: other)
        let exported = try await fleet.exportPool(name: name)
        #expect(exported.runtime == "gvisor")
        #expect(exported.spec.command == ["node", "srv.js"])
        #expect(exported.terraform.contains("resource \"fleets_pool\" \"cua_e2e_swift_apply\""))
        try await fleet.deletePool(name: name)
    }
}
