// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation
import Observation

/// The elastic pool as the Machines page shows it, read-only: the cloud hosts
/// the pool added (`cloud_hosts`), what the clouds cost now and this month
/// (`cloud_budget`), and the last few scaling events (the ledger,
/// `<cua home>/elastic/ledger.jsonl`). The pool decides; this only reads.
/// A daemon without these tools, or no cloud hosts, leaves it empty and the
/// page shows nothing extra.

/// One host the pool added (`cloud_hosts`; the fields of `CloudHost`).
public struct ElasticHostRow: Equatable, Sendable, Identifiable {
    public var id: String { name }
    /// Its pool name (`hetzner-1`).
    public let name: String
    public let provider: String
    public let typeName: String
    /// `creating`, `booting`, `joined`, `cordoned`, `removing`, `offline` or `gone`.
    public let state: String
    /// Its relay machine once joined (the Machines list shows it once).
    public let machine: String
    public let usdPerHour: Double
    public let pinned: Bool

    /// "Hetzner · cax41".
    public var kindLine: String { "\(Self.providerName(provider)) \u{00B7} \(typeName)" }

    /// The state in a word.
    public var stateWord: String {
        switch state {
        case "creating", "booting": return "Starting"
        case "joined": return "Running"
        case "cordoned": return "Winding down"
        case "removing": return "Removing"
        case "offline": return "Offline"
        case "gone": return "Removed"
        default: return state.isEmpty ? "Unknown" : state.prefix(1).uppercased() + state.dropFirst()
        }
    }

    /// Costs money now (starting or running, not gone or offline).
    public var running: Bool { ["creating", "booting", "joined", "cordoned"].contains(state) }

    public static func providerName(_ p: String) -> String {
        switch p {
        case "aws": return "AWS"
        case "gcp": return "Google Cloud"
        case "hetzner": return "Hetzner"
        case "modal": return "Modal"
        default: return p.isEmpty ? "Cloud" : p.prefix(1).uppercased() + p.dropFirst()
        }
    }

    init?(_ d: [String: Any]) {
        guard let name = d["name"] as? String, !name.isEmpty else { return nil }
        self.name = name
        provider = d["provider"] as? String ?? ""
        typeName = d["type_name"] as? String ?? ""
        state = d["state"] as? String ?? ""
        machine = d["machine"] as? String ?? ""
        usdPerHour = (d["usd_per_hour"] as? NSNumber)?.doubleValue ?? 0
        pinned = d["pinned"] as? Bool ?? false
    }

    public init(name: String, provider: String, typeName: String, state: String, machine: String = "",
                usdPerHour: Double, pinned: Bool = false) {
        self.name = name; self.provider = provider; self.typeName = typeName; self.state = state
        self.machine = machine; self.usdPerHour = usdPerHour; self.pinned = pinned
    }
}

/// `cloud_budget`'s numbers: the spend.
public struct ElasticSpend: Equatable, Sendable {
    /// USD per hour of the hosts running now.
    public let runningUsdPerHour: Double
    /// USD spent on elastic hosts this month.
    public let monthUsd: Double
    public let hosts: Int

    init(_ d: [String: Any]) {
        runningUsdPerHour = (d["running_usd_per_hour"] as? NSNumber)?.doubleValue ?? 0
        monthUsd = (d["month_usd"] as? NSNumber)?.doubleValue ?? 0
        hosts = (d["hosts"] as? NSNumber)?.intValue ?? 0
    }

    public init(runningUsdPerHour: Double, monthUsd: Double, hosts: Int) {
        self.runningUsdPerHour = runningUsdPerHour; self.monthUsd = monthUsd; self.hosts = hosts
    }

    /// "$0.07/h now · $12.50 this month".
    public var line: String {
        "\(Self.money(runningUsdPerHour, perHour: true))/h now \u{00B7} \(Self.money(monthUsd)) this month"
    }

    /// Dollars; the hourly rate keeps cents-of-cents (hosts cost $0.0684/h).
    public static func money(_ usd: Double, perHour: Bool = false) -> String {
        if perHour && usd > 0 && usd < 1 { return String(format: "$%.3f", usd) }
        return String(format: "$%.2f", usd)
    }
}

/// One scaling event (a ledger line).
public struct ElasticEvent: Equatable, Sendable, Identifiable {
    public var id: String { "\(at)-\(event)-\(host)" }
    /// Unix seconds.
    public let at: Int
    /// `add`, `remove`, `cordon`, `idle`, `busy`.
    public let event: String
    public let host: String
    public let provider: String
    public let typeName: String
    public let usdPerHour: Double
    public let reason: String

    /// "Added hetzner-1 (cax41)".
    public var text: String {
        let what: String
        switch event {
        case "add": what = "Added \(host)" + (typeName.isEmpty ? "" : " (\(typeName))")
        case "remove": what = "Removed \(host)"
        case "cordon": what = "Winding down \(host)"
        case "idle": what = "\(host) is idle"
        case "busy": what = "\(host) is busy"
        default: what = "\(event) \(host)"
        }
        return reason.isEmpty ? what : "\(what), \(reason)"
    }

    /// The events a person reads: the ones that change the pool.
    static let shown: Set<String> = ["add", "remove", "cordon", "idle", "busy"]

    init?(line: String) {
        guard let data = line.data(using: .utf8),
              let d = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let event = d["event"] as? String, Self.shown.contains(event) else { return nil }
        self.event = event
        at = (d["at"] as? NSNumber)?.intValue ?? 0
        host = d["host"] as? String ?? ""
        provider = d["provider"] as? String ?? ""
        typeName = d["type_name"] as? String ?? ""
        usdPerHour = (d["usd_per_hour"] as? NSNumber)?.doubleValue ?? 0
        reason = d["reason"] as? String ?? ""
    }

    public init(at: Int, event: String, host: String, provider: String = "", typeName: String = "",
                usdPerHour: Double = 0, reason: String = "") {
        self.at = at; self.event = event; self.host = host; self.provider = provider
        self.typeName = typeName; self.usdPerHour = usdPerHour; self.reason = reason
    }

    /// The newest `limit` events of a ledger's text, newest first.
    static func recent(ledger: String, limit: Int) -> [ElasticEvent] {
        Array(ledger.split(separator: "\n").compactMap { ElasticEvent(line: String($0)) }
            .sorted { $0.at > $1.at }.prefix(limit))
    }
}

@MainActor @Observable
public final class ElasticModel {
    private let tools: CloudToolRunning?
    /// The ledger's text (nil: none). Injected for tests.
    private let readLedger: @Sendable () -> String?

    /// Hosts the pool added, not counting the gone ones.
    public private(set) var hosts: [ElasticHostRow] = []
    public private(set) var spend: ElasticSpend?
    /// The last few scaling events, newest first.
    public private(set) var events: [ElasticEvent] = []
    private var readAt: Date?

    /// How many events the page lists.
    public static let eventLimit = 5
    /// How often a refresh actually asks.
    static let minInterval: TimeInterval = 5
    static let readSeconds: Double = 8

    public init(tools: CloudToolRunning?, ledger: (@Sendable () -> String?)? = nil) {
        self.tools = tools
        self.readLedger = ledger ?? { Self.liveLedger() }
    }

    /// Something to show: a host, or an event of an earlier one.
    public var visible: Bool { !hosts.isEmpty }

    /// The relay machines that are cloud hosts: the Machines list shows each once.
    public var machineIds: Set<String> { Set(hosts.map(\.machine).filter { !$0.isEmpty }) }

    /// Reads the hosts, the spend and the ledger again (no more often than
    /// every few seconds unless `force`). A daemon without the tools, or one
    /// that does not answer in time, leaves what was read.
    public func refresh(force: Bool = false) async {
        guard let tools else { return }
        if !force, let readAt, Date().timeIntervalSince(readAt) < Self.minInterval { return }
        readAt = Date()
        async let h = withTimeout(seconds: Self.readSeconds) { Box(value: try await tools.cloudTool("cloud_hosts", [:])) }
        async let b = withTimeout(seconds: Self.readSeconds) { Box(value: try await tools.cloudTool("cloud_budget", [:])) }
        let (hostsResult, budgetResult) = await (h, b)
        if case .success(let answer) = hostsResult {
            apply(hosts: answer.value)
        } else if case .failure(let error) = hostsResult, !(error is TimeoutError) {
            hosts = []
        }
        if case .success(let answer) = budgetResult { spend = (answer.value as? [String: Any]).map(ElasticSpend.init) }
        let reader = readLedger
        let text = await Task.detached { reader() }.value
        events = ElasticEvent.recent(ledger: text ?? "", limit: Self.eventLimit)
        // Throttle from when the read finished: a slow read must not make the next one due at once.
        readAt = Date()
    }

    /// `cloud_hosts` as it came.
    func apply(hosts value: Any) {
        hosts = (value as? [[String: Any]] ?? []).compactMap(ElasticHostRow.init).filter { $0.state != "gone" }
    }

    private struct Box: @unchecked Sendable { let value: Any }

    /// The live cua home's ledger (`CUA_HOME`, else `~/.cua`).
    nonisolated static func liveLedger() -> String? {
        let env = ProcessInfo.processInfo.environment
        let home = (env["CUA_HOME"]).flatMap { $0.isEmpty ? nil : URL(fileURLWithPath: $0, isDirectory: true) }
            ?? URL(fileURLWithPath: env["HOME"] ?? NSHomeDirectory(), isDirectory: true)
                .appendingPathComponent(".cua", isDirectory: true)
        return try? String(contentsOf: home.appendingPathComponent("elastic/ledger.jsonl"), encoding: .utf8)
    }
}
