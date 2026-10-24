// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import Foundation
@testable import CuaSpacesMacKit
import SwiftUI
import Testing

/// The elastic pool on the Machines page: the model decodes the tools'
/// JSON exactly as the Rust structs serialize it (`CloudHost`,
/// `ElasticPolicy`, a ledger line), and the page shows hosts, one summary
/// line and the last events.
@MainActor
@Suite("Elastic pool", .serialized)
struct ElasticTests {
    /// `cloud_hosts`, as `serde_json` writes `Vec<CloudHost>`.
    static let hostsJson = """
    [{"name":"hetzner-1","provider":"hetzner","type_name":"cax41","region":"fsn1","id":"42",
      "machine":"m-1","state":"joined","started":1700000000,"expires":1700086400,
      "usd_per_hour":0.0684,"billing_step_secs":3600,"billing_min_secs":3600,"pinned":false},
     {"name":"aws-1","provider":"aws","type_name":"m7g.large","region":"us-west-2","id":"i-9",
      "machine":"","state":"booting","started":1700000100,"expires":0,
      "usd_per_hour":0.0816,"billing_step_secs":1,"billing_min_secs":60,"pinned":true},
     {"name":"hetzner-0","provider":"hetzner","type_name":"cax11","region":"fsn1","id":"1",
      "machine":"","state":"gone","started":1,"expires":0,
      "usd_per_hour":0.01,"billing_step_secs":3600,"billing_min_secs":3600,"pinned":false}]
    """
    /// `cloud_budget`: `policy` (`ElasticPolicy`), `month_usd`, ...
    static let budgetJson = """
    {"policy":{"autoscale":true,"max_usd_per_hour":3.0,"monthly_usd":50.0,"max_new_hosts":10,
               "warm":true,"interactive_slo_secs":120,"routed":true},
     "month_usd":12.5,"running_usd_per_hour":0.15,"hosts":2}
    """
    /// `ledger.jsonl`: `LedgerLine`s, one per line (older lines first).
    static let ledger = """
    {"at":1000,"event":"add","host":"hetzner-1","provider":"hetzner","type_name":"cax41","region":"fsn1","usd_per_hour":0.0684,"billing_step_secs":3600,"billing_min_secs":3600,"reason":"24 Spaces"}
    {"at":2000,"event":"plan","host":"","reason":"ignored"}
    {"at":3000,"event":"idle","host":"hetzner-1","provider":"hetzner","type_name":"cax41","usd_per_hour":0.0684,"reason":""}
    not json
    {"at":4000,"event":"remove","host":"hetzner-1","provider":"hetzner","type_name":"cax41","usd_per_hour":0.0684,"reason":"idle at the hour"}
    """

    func json(_ s: String) throws -> Any {
        try JSONSerialization.jsonObject(with: Data(s.utf8), options: [.fragmentsAllowed])
    }

    /// Answers the elastic tools from fixed JSON.
    final class Tools: CloudToolRunning, @unchecked Sendable {
        var hosts: Any
        var budget: Any
        var fail = false
        /// The two reads run concurrently, so the record is locked.
        private let lock = NSLock()
        private var recorded: [String] = []
        var calls: [String] { lock.withLock { recorded } }
        init(hosts: Any, budget: Any) { self.hosts = hosts; self.budget = budget }
        func cloudTool(_ tool: String, _ args: [String: Any]) async throws -> Any {
            lock.withLock { recorded.append(tool) }
            if fail { throw FixtureError(message: "no such tool") }
            switch tool {
            case "cloud_hosts": return hosts
            case "cloud_budget": return budget
            default: throw FixtureError(message: "unknown tool \(tool)")
            }
        }
    }

    func model(ledger: String? = ElasticTests.ledger) throws -> (ElasticModel, Tools) {
        let tools = Tools(hosts: try json(Self.hostsJson), budget: try json(Self.budgetJson))
        return (ElasticModel(tools: tools, ledger: { ledger }), tools)
    }

    @Test func hostsDecodeAndGoneOnesAreLeftOut() async throws {
        let (m, _) = try model()
        await m.refresh()
        #expect(m.hosts.map(\.name) == ["hetzner-1", "aws-1"])
        let h = m.hosts[0]
        #expect(h.kindLine == "Hetzner \u{00B7} cax41" && h.stateWord == "Running" && h.running)
        #expect(h.usdPerHour == 0.0684 && h.machine == "m-1")
        #expect(m.hosts[1].stateWord == "Starting" && m.hosts[1].pinned)
        #expect(m.machineIds == ["m-1"])
        #expect(m.visible)
    }

    @Test func spendLineSaysNowAndThisMonth() async throws {
        let (m, _) = try model()
        await m.refresh()
        #expect(m.spend == ElasticSpend(runningUsdPerHour: 0.15, monthUsd: 12.5, hosts: 2))
        #expect(m.spend?.line == "$0.150/h now \u{00B7} $12.50 this month")
        #expect(ElasticSpend.money(2.5, perHour: true) == "$2.50")
    }

    @Test func eventsAreTheNewestFirstAndOnlyPoolChanges() async throws {
        let (m, _) = try model()
        await m.refresh()
        #expect(m.events.map(\.event) == ["remove", "idle", "add"])
        #expect(m.events[0].text == "Removed hetzner-1, idle at the hour")
        #expect(m.events[2].text == "Added hetzner-1 (cax41), 24 Spaces")
    }

    @Test func onlyTheLastFiveEventsAreKept() {
        let text = (1...9).map { #"{"at":\#($0),"event":"add","host":"h\#($0)"}"# }.joined(separator: "\n")
        let events = ElasticEvent.recent(ledger: text, limit: ElasticModel.eventLimit)
        #expect(events.map(\.host) == ["h9", "h8", "h7", "h6", "h5"])
    }

    @Test func noToolsOrNoHostsShowsNothing() async throws {
        let none = ElasticModel(tools: nil)
        await none.refresh()
        #expect(!none.visible && none.events.isEmpty)

        let (m, tools) = try model(ledger: nil)
        tools.hosts = [Any]()
        await m.refresh()
        #expect(!m.visible && m.events.isEmpty)
    }

    @Test func aDaemonWithoutTheToolsClearsTheHosts() async throws {
        let (m, tools) = try model()
        await m.refresh()
        #expect(m.visible)
        tools.fail = true
        await m.refresh(force: true)
        #expect(!m.visible)
    }

    @Test func refreshesAreThrottled() async throws {
        let (m, tools) = try model()
        await m.refresh()
        await m.refresh()
        #expect(tools.calls.sorted() == ["cloud_budget", "cloud_hosts"], "one read of each, in either order")
        await m.refresh(force: true)
        #expect(tools.calls.count == 4)
    }

    @Test func theAppRefreshReadsThePoolFromFixtures() async throws {
        let backend = FixtureSpacesBackend()
        backend.fixtureElastic = true
        let app = ViewModelTests().makeModel(backend)
        await app.refresh()
        await app.elastic.refresh(force: true)
        #expect(app.elastic.hosts.count == 2)
        #expect(app.elastic.spend?.hosts == 2)
        #expect(app.elastic.events.count == 4)
        // Without the pool, the page has nothing extra.
        let plain = ViewModelTests().makeModel(FixtureSpacesBackend())
        await plain.elastic.refresh(force: true)
        #expect(!plain.elastic.visible)
    }

    /// The Machines page with the pool, rendered offscreen for review (only
    /// with `ELASTIC_PREVIEW_DIR` set; nothing is launched or shown).
    @Test func rendersTheMachinesPageForReview() async throws {
        guard let dir = ProcessInfo.processInfo.environment["ELASTIC_PREVIEW_DIR"], !dir.isEmpty else { return }
        _ = NSApplication.shared
        let backend = FixtureSpacesBackend()
        backend.fixtureElastic = true
        let app = ViewModelTests().makeModel(backend)
        app.onboarding.finish()
        await app.refresh()
        await app.elastic.refresh(force: true)
        let size = CGSize(width: 900, height: 560)
        let rep = SnapshotTests().render(MachinesPageView(model: app), size: size)
        let out = URL(fileURLWithPath: dir)
        try FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
        try rep.representation(using: .png, properties: [:])!.write(to: out.appendingPathComponent("machines-elastic.png"))
    }
}
