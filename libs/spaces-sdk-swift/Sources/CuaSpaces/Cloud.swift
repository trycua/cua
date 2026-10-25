import Foundation

// MARK: - Spaces in your own cloud account

/// Where in a cloud account Spaces go. Credentials are never here: each
/// cloud uses its own CLI sign-in (the AWS profile, the gcloud account, the
/// Modal profile), and Cua stores only these names. The one exception is
/// Hetzner, which has no CLI sign-in: its project API token goes in `token`
/// for one `cloudTest` / `cloudConnect` call, and is never printed.
public struct CloudTarget: Sendable, Hashable, CustomStringConvertible, CustomDebugStringConvertible {
    /// `aws`, `gcp`, `hetzner` or `modal`.
    public var provider: String
    public var profile: String?
    public var region: String?
    public var zone: String?
    public var project: String?
    public var environment: String?
    /// Hetzner only: the project's Read & Write API token. Secret.
    public var token: String?

    public init(_ provider: String, profile: String? = nil, region: String? = nil,
                zone: String? = nil, project: String? = nil, environment: String? = nil,
                token: String? = nil) {
        self.provider = provider
        self.profile = profile
        self.region = region
        self.zone = zone
        self.project = project
        self.environment = environment
        self.token = token
    }

    public var description: String {
        "CloudTarget(\(provider), token: \(token == nil ? "none" : "<redacted>"))"
    }
    public var debugDescription: String { description }

    var arguments: [String: JSONValue] {
        var a: [String: JSONValue] = ["provider": .string(provider)]
        if let profile { a["profile"] = .string(profile) }
        if let region { a["region"] = .string(region) }
        if let zone { a["zone"] = .string(zone) }
        if let project { a["project"] = .string(project) }
        if let environment { a["environment"] = .string(environment) }
        if let token, !token.isEmpty { a["token"] = .string(token) }
        return a
    }
}

/// What a cloud can run for one image family, and the estimated cost.
public struct CloudOffer: Sendable, Hashable {
    public let image: String
    public let kind: String
    public let supported: Bool
    public let reason: String
    public let machineType: String
    public let usdPerHour: Double

    init(_ d: [String: JSONValue]) {
        image = d["image"]?.stringValue ?? ""
        kind = d["kind"]?.stringValue ?? ""
        supported = d["supported"]?.boolValue ?? false
        reason = d["reason"]?.stringValue ?? ""
        machineType = d["machine_type"]?.stringValue ?? ""
        usdPerHour = d["usd_per_hour"]?.doubleValue ?? 0
    }
}

/// One cloud as `cloud_status` lists it.
public struct CloudProviderInfo: Sendable, Hashable {
    public let name: String
    public let title: String
    /// `vm` or `sandbox`.
    public let tier: String
    public let connected: Bool
    public let isDefault: Bool
    public let credentialsFound: Bool
    public let credentialsSource: String
    public let account: String
    public let region: String
    public let project: String
    public let environment: String
    /// "AWS · us-west-2".
    public let label: String
    public let ttlHours: Int
    public let offers: [CloudOffer]

    init(_ d: [String: JSONValue]) {
        name = d["name"]?.stringValue ?? ""
        title = d["title"]?.stringValue ?? ""
        tier = d["tier"]?.stringValue ?? ""
        connected = d["connected"]?.boolValue ?? false
        isDefault = d["default"]?.boolValue ?? false
        let creds = d["credentials"]?.objectValue ?? [:]
        credentialsFound = creds["found"]?.boolValue ?? false
        credentialsSource = creds["source"]?.stringValue ?? ""
        account = d["account"]?.stringValue ?? ""
        region = d["region"]?.stringValue ?? ""
        project = d["project"]?.stringValue ?? ""
        environment = d["environment"]?.stringValue ?? ""
        label = d["label"]?.stringValue ?? ""
        ttlHours = d["ttl_hours"]?.intValue ?? 0
        offers = (d["kinds"]?.arrayValue ?? []).compactMap { $0.objectValue }.map(CloudOffer.init)
    }
}

/// One thing Cua created in a cloud.
public struct CloudResource: Sendable, Hashable {
    public let provider: String
    public let id: String
    public let type: String
    /// The sandbox (`aws:<name>`).
    public let sandbox: String
    /// Its relay machine: the Space is `relay:<machine>`.
    public let machine: String
    public let state: String
    public let expires: String
    public let expired: Bool

    init(_ d: [String: JSONValue]) {
        provider = d["provider"]?.stringValue ?? ""
        id = d["id"]?.stringValue ?? ""
        type = d["type"]?.stringValue ?? ""
        sandbox = d["sandbox"]?.stringValue ?? ""
        machine = d["machine"]?.stringValue ?? ""
        state = d["state"]?.stringValue ?? ""
        expires = d["expires"]?.stringValue ?? ""
        expired = d["expired"]?.boolValue ?? false
    }
}

/// `cloud_status`.
public struct CloudStatus: Sendable, Hashable {
    public let defaultOn: String
    public let providers: [CloudProviderInfo]
    public let resources: [CloudResource]
    /// Clouds disconnected while they could not be checked that may still
    /// run Spaces Cua started.
    public let cleanup: [CloudCleanup]

    init(_ d: [String: JSONValue]) {
        defaultOn = d["default_on"]?.stringValue ?? ""
        providers = (d["providers"]?.arrayValue ?? []).compactMap { $0.objectValue }
            .map(CloudProviderInfo.init)
        resources = (d["resources"]?.arrayValue ?? []).compactMap { $0.objectValue }
            .map(CloudResource.init)
        cleanup = (d["cleanup"]?.arrayValue ?? []).compactMap { $0.objectValue }
            .map(CloudCleanup.init)
    }
}

/// A cloud disconnected while Cua could not check it: what Cua's records
/// say may still run there, until a sweep after sign-in confirms it clean.
public struct CloudCleanup: Sendable, Hashable {
    public let provider: String
    public let title: String
    public let account: String
    public let profile: String
    public let region: String
    public let spaces: Int
    public let resources: Int
    public let types: String
    /// RFC 3339; empty: none or never.
    public let latestExpires: String
    public let neverExpires: Bool
    public let usdPerHour: Double
    public let reason: String
    /// "AWS may still have 2 Spaces Cua started. Sign in and clean up."
    public let message: String

    init(_ d: [String: JSONValue]) {
        provider = d["provider"]?.stringValue ?? ""
        title = d["title"]?.stringValue ?? ""
        account = d["account"]?.stringValue ?? ""
        profile = d["profile"]?.stringValue ?? ""
        region = d["region"]?.stringValue ?? ""
        spaces = d["spaces"]?.intValue ?? 0
        resources = d["resources"]?.intValue ?? 0
        types = d["types"]?.stringValue ?? ""
        latestExpires = d["latest_expires"]?.stringValue ?? ""
        neverExpires = d["never_expires"]?.boolValue ?? false
        usdPerHour = d["usd_per_hour"]?.doubleValue ?? 0
        reason = d["reason"]?.stringValue ?? ""
        message = d["message"]?.stringValue ?? ""
    }
}

/// One check `cloud_test` ran.
public struct CloudCheck: Sendable, Hashable {
    public let name: String
    public let ok: Bool
    public let detail: String
}

/// `cloud_test` (and the checks `cloud_connect` passed).
public struct CloudTestReport: Sendable, Hashable {
    public let provider: String
    public let ok: Bool
    public let account: String
    public let checks: [CloudCheck]

    init(_ d: [String: JSONValue]) {
        provider = d["provider"]?.stringValue ?? d["name"]?.stringValue ?? ""
        ok = d["ok"]?.boolValue ?? true
        account = d["account"]?.stringValue ?? ""
        checks = (d["checks"]?.arrayValue ?? []).compactMap { $0.objectValue }.map {
            CloudCheck(name: $0["name"]?.stringValue ?? "", ok: $0["ok"]?.boolValue ?? false,
                       detail: $0["detail"]?.stringValue ?? "")
        }
    }
}

/// `cloud_sweep`: one row.
public struct CloudSweepItem: Sendable, Hashable {
    public let resource: CloudResource
    /// `delete` (dry run), `deleted`, `keep` or `failed`.
    public let action: String
    public let reason: String
}

/// What `cloud_disconnect` did.
public struct CloudDisconnectReceipt: Sendable, Hashable {
    public let provider: String
    public let disconnected: Bool
    /// What Cua still runs in the account (disconnect without delete).
    public let left: [CloudResource]
    /// What was deleted from the account first (with `delete`).
    public let deleted: [CloudResource]
    /// After a forced disconnect: what may still run there.
    public let cleanup: CloudCleanup?

    init(_ d: [String: JSONValue]) {
        provider = d["provider"]?.stringValue ?? ""
        disconnected = d["disconnected"]?.boolValue ?? false
        left = (d["left"]?.arrayValue ?? []).compactMap { $0.objectValue }.map(CloudResource.init)
        deleted = (d["deleted"]?.arrayValue ?? []).compactMap { $0.objectValue }.map(CloudResource.init)
        cleanup = d["cleanup"]?.objectValue.map(CloudCleanup.init)
    }
}

/// One line of the cloud audit log (`cloud_audit`). Never a secret.
public struct CloudAuditEntry: Sendable, Hashable {
    /// RFC 3339, UTC.
    public let ts: String
    /// `connect`, `disconnect`, `resource_created`, `resource_deleted`, ...
    public let action: String
    public let provider: String
    public let kind: String
    public let id: String
    public let space: String
    /// `user`, `sweep`, `auto-sweep`, `disconnect` or `rollback`.
    public let actor: String
    public let outcome: String
    public let detail: String

    init(_ d: [String: JSONValue]) {
        ts = d["ts"]?.stringValue ?? ""
        action = d["action"]?.stringValue ?? ""
        provider = d["provider"]?.stringValue ?? ""
        kind = d["kind"]?.stringValue ?? ""
        id = d["id"]?.stringValue ?? ""
        space = d["space"]?.stringValue ?? ""
        actor = d["actor"]?.stringValue ?? ""
        outcome = d["outcome"]?.stringValue ?? ""
        detail = d["detail"]?.stringValue ?? ""
    }
}

/// Your clouds: connect AWS, Google Cloud, Hetzner or Modal, check one without
/// creating anything, see and sweep what Cua created there.
public struct Cloud: Sendable {
    let connection: SpacesConnection

    /// `cloud_status`.
    public func status(provider: String? = nil) async throws -> CloudStatus {
        var a: [String: JSONValue] = [:]
        if let provider { a["provider"] = .string(provider) }
        return CloudStatus(try await connection.object("cloud_status", a))
    }

    /// `cloud_connect`: the connected cloud, after the checks it passed.
    @discardableResult
    public func connect(_ target: CloudTarget, makeDefault: Bool = false,
                        ttlHours: Int? = nil) async throws -> (CloudProviderInfo, CloudTestReport) {
        var a = target.arguments
        a["make_default"] = .bool(makeDefault)
        if let ttlHours { a["ttl_hours"] = .number(Double(ttlHours)) }
        let d = try await connection.object("cloud_connect", a)
        return (CloudProviderInfo(d), CloudTestReport(d))
    }

    /// `cloud_test`: creates nothing.
    public func test(_ target: CloudTarget) async throws -> CloudTestReport {
        CloudTestReport(try await connection.object("cloud_test", target.arguments))
    }

    /// `cloud_disconnect`: refuses while Cua resources still run in that
    /// cloud, unless `delete` is true, which deletes them first. When the
    /// cloud cannot be checked (signed out) it is refused with an error
    /// starting "Cua couldn't check"; ask the person, then `force` disconnects
    /// anyway and keeps Cua's records for a sweep after sign-in.
    @discardableResult
    public func disconnect(_ provider: String, delete: Bool = false,
                           force: Bool = false) async throws -> CloudDisconnectReceipt {
        var a: [String: JSONValue] = ["provider": .string(provider)]
        if delete { a["delete"] = .bool(true) }
        if force { a["force"] = .bool(true) }
        return CloudDisconnectReceipt(try await connection.object("cloud_disconnect", a))
    }

    /// `cloud_audit`: the newest entries of what Cua did in your clouds.
    public func audit(limit: Int? = nil) async throws -> [CloudAuditEntry] {
        var a: [String: JSONValue] = [:]
        if let limit { a["limit"] = .number(Double(limit)) }
        return try await connection.array("cloud_audit", a).compactMap { $0.objectValue }
            .map(CloudAuditEntry.init)
    }

    /// `cloud_sweep`: dry run by default.
    public func sweep(provider: String? = nil, dryRun: Bool = true,
                      all: Bool = false) async throws -> [CloudSweepItem] {
        var a: [String: JSONValue] = ["dry_run": .bool(dryRun), "all": .bool(all)]
        if let provider { a["provider"] = .string(provider) }
        let d = try await connection.object("cloud_sweep", a)
        return (d["resources"]?.arrayValue ?? []).compactMap { $0.objectValue }.map {
            CloudSweepItem(resource: CloudResource($0), action: $0["action"]?.stringValue ?? "",
                           reason: $0["reason"]?.stringValue ?? "")
        }
    }
}

// MARK: - The elastic pool

/// A host the elastic pool added in one of your clouds (`cloud_hosts`).
public struct ElasticHost: Sendable, Hashable {
    /// Its pool name (`hetzner-1`).
    public let name: String
    public let provider: String
    public let typeName: String
    public let region: String
    /// The cloud's id.
    public let id: String
    /// Its relay machine once joined.
    public let machine: String
    /// `creating`, `booting`, `joined`, `cordoned`, `offline` or `gone`.
    public let state: String
    /// Unix seconds.
    public let started: Int
    /// Unix seconds it ends on its own (0: none).
    public let expires: Int
    public let usdPerHour: Double
    public let billingStepSecs: Int
    public let billingMinSecs: Int
    /// Added by hand: the scaler never removes it.
    public let pinned: Bool

    init(_ d: [String: JSONValue]) {
        name = d["name"]?.stringValue ?? ""
        provider = d["provider"]?.stringValue ?? ""
        typeName = d["type_name"]?.stringValue ?? ""
        region = d["region"]?.stringValue ?? ""
        id = d["id"]?.stringValue ?? ""
        machine = d["machine"]?.stringValue ?? ""
        state = d["state"]?.stringValue ?? ""
        started = d["started"]?.intValue ?? 0
        expires = d["expires"]?.intValue ?? 0
        usdPerHour = d["usd_per_hour"]?.doubleValue ?? 0
        billingStepSecs = d["billing_step_secs"]?.intValue ?? 0
        billingMinSecs = d["billing_min_secs"]?.intValue ?? 0
        pinned = d["pinned"]?.boolValue ?? false
    }
}

/// Hosts to add by hand (`cloud_add_host`).
public struct ElasticHostRequest: Sendable, Hashable {
    public var provider: String
    public var instanceType: String
    public var region: String?
    /// 1 to 10. Default 1.
    public var count: Int?
    /// 1 to 168. Default 24.
    public var ttlHours: Int?

    public init(provider: String, instanceType: String, region: String? = nil,
                count: Int? = nil, ttlHours: Int? = nil) {
        self.provider = provider
        self.instanceType = instanceType
        self.region = region
        self.count = count
        self.ttlHours = ttlHours
    }

    var arguments: [String: JSONValue] {
        var a: [String: JSONValue] = ["provider": .string(provider), "instance_type": .string(instanceType)]
        if let region { a["region"] = .string(region) }
        if let count { a["count"] = .number(Double(count)) }
        if let ttlHours { a["ttl_hours"] = .number(Double(ttlHours)) }
        return a
    }
}

/// `cloud_budget`: the caps, what the clouds cost now and this month.
public struct ElasticBudget: Sendable, Hashable {
    public let autoscale: Bool
    public let maxUsdPerHour: Double
    public let monthlyUsd: Double
    public let maxNewHosts: Int
    public let warm: Bool
    public let interactiveSloSecs: Int
    /// USD spent on elastic hosts this month.
    public let monthUsd: Double
    /// USD per hour of the hosts running now.
    public let runningUsdPerHour: Double
    /// Hosts running.
    public let hosts: Int

    init(_ d: [String: JSONValue]) {
        let p = d["policy"]?.objectValue ?? [:]
        autoscale = p["autoscale"]?.boolValue ?? true
        maxUsdPerHour = p["max_usd_per_hour"]?.doubleValue ?? 0
        monthlyUsd = p["monthly_usd"]?.doubleValue ?? 0
        maxNewHosts = p["max_new_hosts"]?.intValue ?? 0
        warm = p["warm"]?.boolValue ?? true
        interactiveSloSecs = p["interactive_slo_secs"]?.intValue ?? 0
        monthUsd = d["month_usd"]?.doubleValue ?? 0
        runningUsdPerHour = d["running_usd_per_hour"]?.doubleValue ?? 0
        hosts = d["hosts"]?.intValue ?? 0
    }
}

/// The fields `cloud_budget` sets; those left nil stay as they are.
public struct ElasticBudgetUpdate: Sendable, Hashable {
    public var maxUsdPerHour: Double?
    public var monthlyUsd: Double?
    public var autoscale: Bool?
    public var maxNewHosts: Int?
    public var warm: Bool?

    public init(maxUsdPerHour: Double? = nil, monthlyUsd: Double? = nil, autoscale: Bool? = nil,
                maxNewHosts: Int? = nil, warm: Bool? = nil) {
        self.maxUsdPerHour = maxUsdPerHour
        self.monthlyUsd = monthlyUsd
        self.autoscale = autoscale
        self.maxNewHosts = maxNewHosts
        self.warm = warm
    }

    var arguments: [String: JSONValue] {
        var a: [String: JSONValue] = [:]
        if let maxUsdPerHour { a["max_usd_per_hour"] = .number(maxUsdPerHour) }
        if let monthlyUsd { a["monthly_usd"] = .number(monthlyUsd) }
        if let autoscale { a["autoscale"] = .bool(autoscale) }
        if let maxNewHosts { a["max_new_hosts"] = .number(Double(maxNewHosts)) }
        if let warm { a["warm"] = .bool(warm) }
        return a
    }
}

/// What to plan for (`cloud_plan`). Every field is optional; the pins are
/// constraints of the same solve.
public struct ElasticPlanRequest: Sendable, Hashable {
    public var count: Int?
    public var image: String?
    public var kind: String?
    public var cpus: Int?
    public var memoryMb: Int?
    public var together: Bool?
    public var caller: String?
    public var durationSecs: Int?
    public var provider: String?
    public var region: String?
    public var instanceType: String?
    public var hosts: Int?
    public var noScale: Bool?
    public var devicesOnly: Bool?
    public var maxUsdPerHour: Double?
    /// Add the machines the plan needs. Default: plan only.
    public var apply: Bool?

    public init(count: Int? = nil, image: String? = nil, kind: String? = nil, cpus: Int? = nil,
                memoryMb: Int? = nil, together: Bool? = nil, caller: String? = nil,
                durationSecs: Int? = nil, provider: String? = nil, region: String? = nil,
                instanceType: String? = nil, hosts: Int? = nil, noScale: Bool? = nil,
                devicesOnly: Bool? = nil, maxUsdPerHour: Double? = nil, apply: Bool? = nil) {
        self.count = count; self.image = image; self.kind = kind; self.cpus = cpus
        self.memoryMb = memoryMb; self.together = together; self.caller = caller
        self.durationSecs = durationSecs; self.provider = provider; self.region = region
        self.instanceType = instanceType; self.hosts = hosts; self.noScale = noScale
        self.devicesOnly = devicesOnly; self.maxUsdPerHour = maxUsdPerHour; self.apply = apply
    }

    var arguments: [String: JSONValue] {
        var a: [String: JSONValue] = [:]
        func num(_ k: String, _ v: Int?) { if let v { a[k] = .number(Double(v)) } }
        func str(_ k: String, _ v: String?) { if let v { a[k] = .string(v) } }
        func flag(_ k: String, _ v: Bool?) { if let v { a[k] = .bool(v) } }
        num("count", count); str("image", image); str("kind", kind); num("cpus", cpus)
        num("memory_mb", memoryMb); flag("together", together); str("caller", caller)
        num("duration_secs", durationSecs); str("provider", provider); str("region", region)
        str("instance_type", instanceType); num("hosts", hosts); flag("no_scale", noScale)
        flag("devices_only", devicesOnly); flag("apply", apply)
        if let maxUsdPerHour { a["max_usd_per_hour"] = .number(maxUsdPerHour) }
        return a
    }
}

/// Some Spaces of one group on one machine.
public struct ElasticAssignment: Sendable, Hashable {
    public let group: String
    /// A machine in the pool by name, or empty when it is a new one.
    public let host: String
    /// The index into `newHosts` (when `host` is empty).
    public let newIndex: Int?
    public let count: Int
}

/// A machine the plan adds.
public struct ElasticNewHost: Sendable, Hashable {
    public let provider: String
    public let typeName: String
    public let region: String
    public let usdPerHour: Double
    public let bootSecs: Int
    public let billedSecs: Int
}

/// Spaces with no place, and why.
public struct ElasticUnplaced: Sendable, Hashable {
    public let group: String
    public let count: Int
    public let reason: String
}

/// `cloud_plan`: where the Spaces go, which machines it adds, the cost.
public struct ElasticPlan: Sendable, Hashable {
    public let assignments: [ElasticAssignment]
    public let newHosts: [ElasticNewHost]
    public let unplaced: [ElasticUnplaced]
    public let addedUsdPerHour: Double
    public let projectedUsd: Double
    public let lowerBoundUsd: Double
    public let solver: String
    public let readyInSecs: Int
    public let explain: [String]
    /// The hosts added (with `apply`).
    public let added: [ElasticHost]

    init(_ d: [String: JSONValue]) {
        let p = d["plan"]?.objectValue ?? [:]
        func rows(_ v: JSONValue?) -> [[String: JSONValue]] {
            (v?.arrayValue ?? []).compactMap { $0.objectValue }
        }
        assignments = rows(p["assignments"]).map { a in
            let t = a["target"]?.objectValue ?? [:]
            let new = t["on"]?.stringValue == "new"
            return ElasticAssignment(group: a["group"]?.stringValue ?? "",
                                     host: new ? "" : (t["name"]?.stringValue ?? ""),
                                     newIndex: new ? (t["index"]?.intValue ?? 0) : nil,
                                     count: a["count"]?.intValue ?? 0)
        }
        newHosts = rows(p["new_hosts"]).map {
            ElasticNewHost(provider: $0["provider"]?.stringValue ?? "",
                           typeName: $0["type_name"]?.stringValue ?? "",
                           region: $0["region"]?.stringValue ?? "",
                           usdPerHour: $0["usd_per_hour"]?.doubleValue ?? 0,
                           bootSecs: $0["boot_secs"]?.intValue ?? 0,
                           billedSecs: $0["billed_secs"]?.intValue ?? 0)
        }
        unplaced = rows(p["unplaced"]).map {
            ElasticUnplaced(group: $0["group"]?.stringValue ?? "", count: $0["count"]?.intValue ?? 0,
                            reason: $0["reason"]?.stringValue ?? "")
        }
        addedUsdPerHour = p["added_usd_per_hour"]?.doubleValue ?? 0
        projectedUsd = p["projected_usd"]?.doubleValue ?? 0
        lowerBoundUsd = p["lower_bound_usd"]?.doubleValue ?? 0
        solver = p["solver"]?.stringValue ?? ""
        readyInSecs = p["ready_in_secs"]?.intValue ?? 0
        explain = (p["explain"]?.arrayValue ?? []).compactMap { $0.stringValue }
        added = rows(d["added"]).map(ElasticHost.init)
    }
}

/// `cloud_remove_host`: what went and what did not.
public struct ElasticRemoved: Sendable, Hashable {
    public let removed: [String]
    /// `name: error` per host not removed.
    public let failed: [String]
}

/// One scale-in decision (`cloud_reconcile`).
public struct ElasticDecision: Sendable, Hashable {
    public let host: String
    /// `keep`, `cordon` or `remove`.
    public let action: String
    public let reason: String
    /// `keep`: unix seconds it is looked at again.
    public let until: Int
    /// `remove`: USD saved against keeping it a billing step longer.
    public let savesUsd: Double
}

extension Cloud {
    /// `cloud_hosts`: the hosts the elastic pool added, with their live cost.
    public func hosts() async throws -> [ElasticHost] {
        try await connection.array("cloud_hosts", [:]).compactMap { $0.objectValue }.map(ElasticHost.init)
    }

    /// `cloud_budget`: reads the caps and the spend, after setting the
    /// fields `update` gives.
    @discardableResult
    public func budget(_ update: ElasticBudgetUpdate = ElasticBudgetUpdate()) async throws -> ElasticBudget {
        ElasticBudget(try await connection.object("cloud_budget", update.arguments))
    }

    /// `cloud_plan`: what the pool would do (and with `apply`, does).
    public func plan(_ request: ElasticPlanRequest = ElasticPlanRequest()) async throws -> ElasticPlan {
        ElasticPlan(try await connection.object("cloud_plan", request.arguments))
    }

    /// `cloud_add_host`: hosts by hand in a linked cloud (pinned, ending at their TTL).
    public func addHost(_ request: ElasticHostRequest) async throws -> [ElasticHost] {
        try await connection.array("cloud_add_host", request.arguments).compactMap { $0.objectValue }
            .map(ElasticHost.init)
    }

    /// `cloud_remove_host`: removes hosts this home added.
    public func removeHost(_ names: [String]) async throws -> ElasticRemoved {
        let d = try await connection.object("cloud_remove_host", ["names": .array(names.map { .string($0) })])
        return ElasticRemoved(
            removed: (d["removed"]?.arrayValue ?? []).compactMap { $0.stringValue },
            failed: (d["failed"]?.arrayValue ?? []).compactMap { $0.objectValue }.map {
                "\($0["name"]?.stringValue ?? ""): \($0["error"]?.stringValue ?? "")"
            })
    }

    /// `cloud_reconcile`: one scale-in pass now.
    public func reconcile() async throws -> [ElasticDecision] {
        try await connection.array("cloud_reconcile", [:]).compactMap { $0.objectValue }.map {
            let a = $0["action"]?.objectValue ?? [:]
            return ElasticDecision(host: $0["host"]?.stringValue ?? "",
                                   action: a["action"]?.stringValue ?? "",
                                   reason: a["reason"]?.stringValue ?? "",
                                   until: a["until"]?.intValue ?? 0,
                                   savesUsd: a["saves_usd"]?.doubleValue ?? 0)
        }
    }
}

/// Where a prewarm keeps Spaces ready (`prewarm`).
public struct PrewarmPlacement: Sendable, Hashable {
    public let host: String
    /// `local` or the machine's relay id.
    public let machine: String
    public let count: Int
    public let ready: Int
}

/// Spaces of one shape kept ready by the elastic pool (`prewarm`): the next
/// creates of that shape claim them in a fraction of a second.
public struct Prewarm: Sendable, Hashable {
    public let id: String
    public let image: String
    public let kind: String
    public let cpus: Int
    public let memoryMb: Int
    public let count: Int
    public let ready: Int
    public let placements: [PrewarmPlacement]
    /// Unix seconds.
    public let expiresAt: Int
    public let newHosts: [String]

    init(_ d: [String: JSONValue]) {
        id = d["id"]?.stringValue ?? ""
        image = d["image"]?.stringValue ?? ""
        kind = d["kind"]?.stringValue ?? ""
        cpus = d["cpus"]?.intValue ?? 0
        memoryMb = d["memory_mb"]?.intValue ?? 0
        count = d["count"]?.intValue ?? 0
        ready = d["ready"]?.intValue ?? 0
        placements = (d["placements"]?.arrayValue ?? []).compactMap { $0.objectValue }.map {
            PrewarmPlacement(host: $0["host"]?.stringValue ?? "", machine: $0["machine"]?.stringValue ?? "",
                             count: $0["count"]?.intValue ?? 0, ready: $0["ready"]?.intValue ?? 0)
        }
        expiresAt = d["expires_at"]?.intValue ?? 0
        newHosts = (d["new_hosts"]?.arrayValue ?? []).compactMap { $0.stringValue }
    }
}

extension Cloud {
    /// `prewarm`: keeps `count` Spaces of `image` ready where the pool puts
    /// them (`on`: `auto` by default, `local`, a cloud or `host:<machine>`).
    public func prewarm(_ image: String, count: Int = 1, on: String? = nil, kind: String? = nil,
                        cpus: Int? = nil, memoryMb: Int? = nil, ttlSecs: Int? = nil) async throws -> Prewarm {
        var a: [String: JSONValue] = ["image": .string(image), "count": .number(Double(count))]
        if let on { a["on"] = .string(on) }
        if let kind { a["kind"] = .string(kind) }
        if let cpus { a["cpus"] = .number(Double(cpus)) }
        if let memoryMb { a["memory_mb"] = .number(Double(memoryMb)) }
        if let ttlSecs { a["ttl_secs"] = .number(Double(ttlSecs)) }
        return Prewarm(try await connection.object("prewarm", a))
    }

    /// `prewarm_list`: the prewarms and how many each keeps ready now.
    public func prewarms() async throws -> [Prewarm] {
        try await connection.array("prewarm_list", [:]).compactMap { $0.objectValue }.map(Prewarm.init)
    }

    /// `prewarm_remove`: ends a prewarm (its ready Spaces go).
    @discardableResult
    public func removePrewarm(_ id: String) async throws -> Bool {
        try await connection.object("prewarm_remove", ["id": .string(id)])["removed"]?.boolValue ?? false
    }
}

extension SpacesConnection {
    /// Spaces in your own cloud account.
    public var cloud: Cloud { Cloud(connection: self) }
}
