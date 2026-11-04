import Testing
@testable import CuaSpaces

/// The elastic pool over the fake daemon: fixtures shaped exactly as the Rust
/// structs serialize (`CloudHost`, `ElasticPolicy`, `Plan`, `Decision`).
@Suite("Elastic pool")
struct ElasticTests {
    func connection() -> SpacesConnection { SpacesConnection(transport: FakeSpacesBackend()) }

    @Test func hostsDecodeEveryField() async throws {
        let hosts = try await connection().cloud.hosts()
        let h = try #require(hosts.first)
        #expect(h.name == "hetzner-1" && h.provider == "hetzner" && h.typeName == "cax41")
        #expect(h.region == "fsn1" && h.state == "joined" && h.machine == "m-1")
        #expect(h.usdPerHour == 0.0684 && h.billingStepSecs == 3600 && !h.pinned)
        #expect(h.started == 1_700_000_000 && h.expires == 1_700_086_400)
    }

    @Test func budgetReadsPolicyAndSpend() async throws {
        let b = try await connection().cloud.budget(ElasticBudgetUpdate(maxUsdPerHour: 3))
        #expect(b.maxUsdPerHour == 3 && b.monthlyUsd == 300 && b.maxNewHosts == 8)
        #expect(b.monthUsd == 12.5 && b.runningUsdPerHour == 0.0684 && b.hosts == 1)
    }

    @Test func budgetSendsOnlyTheFieldsGiven() {
        #expect(ElasticBudgetUpdate().arguments.isEmpty)
        let a = ElasticBudgetUpdate(autoscale: false, warm: true).arguments
        #expect(a.count == 2 && a["autoscale"]?.boolValue == false)
    }

    @Test func planDecodesTargetsAndCosts() async throws {
        let p = try await connection().cloud.plan(ElasticPlanRequest(count: 32, together: true))
        #expect(p.assignments.count == 2)
        #expect(p.assignments[0].host == "studio" && p.assignments[0].newIndex == nil)
        #expect(p.assignments[1].host == "" && p.assignments[1].newIndex == 0 && p.assignments[1].count == 24)
        #expect(p.newHosts.first?.billedSecs == 3600)
        #expect(p.unplaced.first?.reason == "cap")
        #expect(p.solver == "exact" && p.readyInSecs == 75 && p.added.isEmpty)
        let applied = try await connection().cloud.plan(ElasticPlanRequest(apply: true))
        #expect(applied.added.count == 1)
    }

    @Test func addRemoveAndReconcile() async throws {
        let c = await connection().cloud
        let added = try await c.addHost(ElasticHostRequest(provider: "hetzner", instanceType: "cax21", count: 2))
        #expect(added.first?.typeName == "cax21")
        let r = try await c.removeHost(["hetzner-1"])
        #expect(r.removed == ["hetzner-1"] && r.failed == ["ghost-9: not found"])
        let d = try await c.reconcile()
        #expect(d[0].action == "remove" && d[0].savesUsd == 0.07 && d[0].reason == "idle")
        #expect(d[1].action == "keep" && d[1].until == 1_800_000_000)
    }

    @Test func requestArgumentsUseTheToolNames() {
        let a = ElasticPlanRequest(count: 3, memoryMb: 2048, noScale: true, apply: false).arguments
        #expect(a["memory_mb"]?.intValue == 2048 && a["no_scale"]?.boolValue == true)
        let h = ElasticHostRequest(provider: "aws", instanceType: "m7g.large", ttlHours: 4).arguments
        #expect(h["instance_type"]?.stringValue == "m7g.large" && h["ttl_hours"]?.intValue == 4)
        #expect(h["region"] == nil)
    }
}
