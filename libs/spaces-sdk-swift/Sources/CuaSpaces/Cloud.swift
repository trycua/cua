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
    /// Hetzner: the project's Read & Write API token; Modal:
    /// `<token id>:<token secret>`. Secret: sealed to your devices on
    /// connect, never returned.
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
    /// `cli` (the cloud CLI's sign-in), `oidc` (no stored key) or `sealed`;
    /// empty when not connected.
    public let auth: String
    /// `connected`, `waiting-for-approval`, `not-connected`; from `connect`
    /// also `needs-trust`, `checking` or `failed`.
    public let status: String
    /// The monthly budget in US dollars, when set.
    public let budgetUsdMonth: Double?
    /// The largest machine (`4x16`: vCPUs x GB), empty: no limit.
    public let maxSize: String
    /// Estimated US dollars this calendar month.
    public let spentMonth: Double
    /// Machines Cua runs there now.
    public let running: Int
    /// `connection` or `limits` while a person's approval is pending.
    public let approval: String
    /// The agent whose request waits for approval.
    public let approvalAgent: String
    /// The monthly budget an agent asked for, waiting for approval.
    public let pendingBudgetUsdMonth: Double?
    /// The largest machine an agent asked for (`4x16`), waiting.
    public let pendingMaxSize: String
    /// Whether this caller (a person) can approve or deny what waits.
    public let canApprove: Bool
    /// `oidc` when this link could switch to linking without a stored key.
    public let suggestAuth: String
    /// What is wrong with the link and what to do, one sentence, or empty.
    public let issue: String
    /// `sealed`: the devices that can open the account's API token
    /// ("Can open it").
    public let holders: [String]

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
        auth = d["auth"]?.stringValue ?? ""
        status = d["status"]?.stringValue ?? ""
        budgetUsdMonth = d["budget_usd_month"]?.doubleValue
        maxSize = d["max_size"]?.stringValue ?? ""
        spentMonth = d["spent_month"]?.doubleValue ?? 0
        running = d["running"]?.intValue ?? 0
        approval = d["approval"]?.stringValue ?? ""
        approvalAgent = d["approval_agent"]?.stringValue ?? ""
        pendingBudgetUsdMonth = d["pending_budget_usd_month"]?.doubleValue
        pendingMaxSize = d["pending_max_size"]?.stringValue ?? ""
        canApprove = d["can_approve"]?.boolValue ?? false
        suggestAuth = d["suggest_auth"]?.stringValue ?? ""
        issue = d["issue"]?.stringValue ?? ""
        holders = (d["holders"]?.arrayValue ?? []).compactMap { $0.stringValue }
    }
}

/// How `connect` links a cloud, beyond where.
public struct CloudConnectOptions: Sendable, Hashable {
    /// `cli`, `oidc` (AWS, Google Cloud, Azure: no stored key;
    /// Azure always) or `sealed` (Hetzner, Modal: the
    /// API token sealed to your devices; the default for them on a device
    /// with the account's pool keys when a budget is given).
    public var auth: String?
    /// The monthly budget, US dollars.
    public var budgetUsdMonth: Double?
    /// The largest machine, `4x16`.
    public var maxSize: String?
    /// AWS `oidc`: the 12-digit account id.
    public var accountId: String?
    /// AWS `oidc`: the role ARN when it is not `cua-spaces`.
    public var roleArn: String?
    /// `oidc`: prove the link (nothing is created) and save it.
    public var check: Bool
    /// The agent making the change; its changes wait for a person.
    public var agent: String?
    /// Google Cloud `oidc`: the project number (implies `oidc`).
    public var projectNumber: String?
    /// Google Cloud `oidc`: a service account to act as.
    public var serviceAccount: String?
    /// Azure: the subscription id.
    public var subscriptionId: String?
    /// Azure: the Microsoft Entra tenant id.
    public var tenantId: String?
    /// Azure: the `cua-spaces` managed identity's client id.
    public var clientId: String?
    /// Azure: the resource group (default `cua-spaces`).
    public var resourceGroup: String?

    public init(auth: String? = nil, budgetUsdMonth: Double? = nil, maxSize: String? = nil,
                accountId: String? = nil, roleArn: String? = nil, check: Bool = false,
                agent: String? = nil, projectNumber: String? = nil,
                serviceAccount: String? = nil, subscriptionId: String? = nil,
                tenantId: String? = nil, clientId: String? = nil, resourceGroup: String? = nil) {
        self.auth = auth
        self.budgetUsdMonth = budgetUsdMonth
        self.maxSize = maxSize
        self.accountId = accountId
        self.roleArn = roleArn
        self.check = check
        self.agent = agent
        self.projectNumber = projectNumber
        self.serviceAccount = serviceAccount
        self.subscriptionId = subscriptionId
        self.tenantId = tenantId
        self.clientId = clientId
        self.resourceGroup = resourceGroup
    }

    var arguments: [String: JSONValue] {
        var a: [String: JSONValue] = [:]
        if let auth { a["auth"] = .string(auth) }
        if let budgetUsdMonth { a["budget_usd_month"] = .number(budgetUsdMonth) }
        if let maxSize { a["max_size"] = .string(maxSize) }
        if let accountId { a["account_id"] = .string(accountId) }
        if let roleArn { a["role_arn"] = .string(roleArn) }
        if check { a["check"] = .bool(true) }
        if let agent { a["agent"] = .string(agent) }
        if let projectNumber { a["project_number"] = .string(projectNumber) }
        if let serviceAccount { a["service_account"] = .string(serviceAccount) }
        if let subscriptionId { a["subscription_id"] = .string(subscriptionId) }
        if let tenantId { a["tenant_id"] = .string(tenantId) }
        if let clientId { a["client_id"] = .string(clientId) }
        if let resourceGroup { a["resource_group"] = .string(resourceGroup) }
        return a
    }
}

/// The next step of a `connect`: a command for the agent, or a page for a
/// person (and what to paste there), and why.
public struct CloudNext: Sendable, Hashable {
    public let run: String?
    public let open: String?
    public let paste: String?
    public let why: String

    init(_ d: [String: JSONValue]) {
        run = d["run"]?.stringValue
        open = d["open"]?.stringValue
        paste = d["paste"]?.stringValue
        why = d["why"]?.stringValue ?? ""
    }
}

/// What `connect(_:options:)` did: the cloud (its `status` is the outcome),
/// the checks it ran, the next step and the command that verifies it.
public struct CloudConnectResult: Sendable, Hashable {
    public let provider: CloudProviderInfo
    public let checks: [CloudCheck]
    /// `nil`: nothing left to do.
    public let next: CloudNext?
    public let verify: String

    init(_ d: [String: JSONValue]) {
        provider = CloudProviderInfo(d)
        checks = CloudTestReport(d).checks
        next = d["next"]?.objectValue.map(CloudNext.init)
        verify = d["verify"]?.stringValue ?? ""
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
    /// Who started it: an agent's name, or `user`.
    public let startedBy: String
    /// Estimated US dollars it cost so far.
    public let costUsd: Double

    init(_ d: [String: JSONValue]) {
        provider = d["provider"]?.stringValue ?? ""
        id = d["id"]?.stringValue ?? ""
        type = d["type"]?.stringValue ?? ""
        sandbox = d["sandbox"]?.stringValue ?? ""
        machine = d["machine"]?.stringValue ?? ""
        state = d["state"]?.stringValue ?? ""
        expires = d["expires"]?.stringValue ?? ""
        expired = d["expired"]?.boolValue ?? false
        startedBy = d["started_by"]?.stringValue ?? ""
        costUsd = d["cost_usd"]?.doubleValue ?? 0
    }
}

/// `cloud_status`.
public struct CloudStatus: Sendable, Hashable {
    public let defaultOn: String
    public let providers: [CloudProviderInfo]
    public let resources: [CloudResource]

    init(_ d: [String: JSONValue]) {
        defaultOn = d["default_on"]?.stringValue ?? ""
        providers = (d["providers"]?.arrayValue ?? []).compactMap { $0.objectValue }
            .map(CloudProviderInfo.init)
        resources = (d["resources"]?.arrayValue ?? []).compactMap { $0.objectValue }
            .map(CloudResource.init)
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

    init(_ d: [String: JSONValue]) {
        provider = d["provider"]?.stringValue ?? ""
        disconnected = d["disconnected"]?.boolValue ?? false
        left = (d["left"]?.arrayValue ?? []).compactMap { $0.objectValue }.map(CloudResource.init)
        deleted = (d["deleted"]?.arrayValue ?? []).compactMap { $0.objectValue }.map(CloudResource.init)
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
    /// `device` (this cua home's log) or `account` (Cua's issuer, for clouds
    /// linked to the account). Empty from an older daemon: this device.
    public let source: String

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
        source = d["source"]?.stringValue ?? ""
    }
}

/// One row of `cloud_cost`.
public struct CloudCostRow: Sendable, Hashable {
    /// The Space, agent, day (`YYYY-MM-DD`) or cloud.
    public let key: String
    /// Estimated US dollars.
    public let usd: Double
    /// Machine hours.
    public let hours: Double
    /// How many machines ran.
    public let machines: Int

    init(_ d: [String: JSONValue]) {
        key = d["key"]?.stringValue ?? ""
        usd = d["usd"]?.doubleValue ?? 0
        hours = d["hours"]?.doubleValue ?? 0
        machines = Int(d["machines"]?.doubleValue ?? 0)
    }
}

/// `cloud_cost`: an estimate from list price times runtime.
public struct CloudCost: Sendable, Hashable {
    /// `space`, `agent`, `day` or `cloud`.
    public let by: String
    /// The first day counted (`YYYY-MM-DD`).
    public let since: String
    /// Up to when (RFC 3339).
    public let until: String
    public let rows: [CloudCostRow]
    public let totalUsd: Double

    init(_ d: [String: JSONValue]) {
        by = d["by"]?.stringValue ?? ""
        since = d["since"]?.stringValue ?? ""
        until = d["until"]?.stringValue ?? ""
        rows = (d["rows"]?.arrayValue ?? []).compactMap { $0.objectValue }.map(CloudCostRow.init)
        totalUsd = d["total_usd"]?.doubleValue ?? 0
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

    /// `cloud_connect` with `options`: links without a stored key
    /// (`auth: "oidc"`), sets the cloud's own budget and largest machine.
    /// Never prompts; call it again until `provider.status` is `connected`,
    /// following `next`.
    @discardableResult
    public func connect(_ target: CloudTarget, options: CloudConnectOptions,
                        makeDefault: Bool = false, ttlHours: Int? = nil) async throws -> CloudConnectResult {
        var a = target.arguments
        a["make_default"] = .bool(makeDefault)
        if let ttlHours { a["ttl_hours"] = .number(Double(ttlHours)) }
        a.merge(options.arguments) { _, new in new }
        return CloudConnectResult(try await connection.object("cloud_connect", a))
    }

    /// `cloud_test`: creates nothing.
    public func test(_ target: CloudTarget) async throws -> CloudTestReport {
        CloudTestReport(try await connection.object("cloud_test", target.arguments))
    }

    /// `cloud_disconnect`: refuses while Cua resources still run in that
    /// cloud, unless `delete` is true, which deletes them first.
    @discardableResult
    public func disconnect(_ provider: String, delete: Bool = false) async throws -> CloudDisconnectReceipt {
        var a: [String: JSONValue] = ["provider": .string(provider)]
        if delete { a["delete"] = .bool(true) }
        return CloudDisconnectReceipt(try await connection.object("cloud_disconnect", a))
    }

    /// `cloud_approve`: a person approves what an agent asked for on
    /// `provider` (its link or its raised limits), or with `deny` declines
    /// it. For a person only: no MCP server serves it, so it needs the cua
    /// SDK transport (`SpacesConnection(cua:)`).
    @discardableResult
    public func approve(_ provider: String, deny: Bool = false) async throws -> CloudProviderInfo {
        var a: [String: JSONValue] = ["provider": .string(provider)]
        if deny { a["deny"] = .bool(true) }
        return CloudProviderInfo(try await connection.object("cloud_approve", a))
    }

    /// `cloud_audit`: the newest entries of what Cua did in your clouds.
    public func audit(limit: Int? = nil) async throws -> [CloudAuditEntry] {
        var a: [String: JSONValue] = [:]
        if let limit { a["limit"] = .number(Double(limit)) }
        return try await connection.array("cloud_audit", a).compactMap { $0.objectValue }
            .map(CloudAuditEntry.init)
    }

    /// `cloud_cost`: what the machines Cua created cost, by `space`,
    /// `agent`, `day` or `cloud`, since `since` (`YYYY-MM-DD`; default the
    /// first of this month).
    public func cost(by: String = "space", since: String? = nil,
                     provider: String? = nil) async throws -> CloudCost {
        var a: [String: JSONValue] = ["by": .string(by)]
        if let since { a["since"] = .string(since) }
        if let provider { a["provider"] = .string(provider) }
        return CloudCost(try await connection.object("cloud_cost", a))
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
