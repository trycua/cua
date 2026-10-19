import Foundation

// MARK: - Spaces in your own cloud account

/// Where in a cloud account Spaces go. Credentials are never here: each
/// cloud uses its own CLI sign-in (the AWS profile, the gcloud account, the
/// Modal profile), and Cua stores only these names.
public struct CloudTarget: Sendable, Hashable {
    /// `aws`, `gcp` or `modal`.
    public var provider: String
    public var profile: String?
    public var region: String?
    public var zone: String?
    public var project: String?
    public var environment: String?

    public init(_ provider: String, profile: String? = nil, region: String? = nil,
                zone: String? = nil, project: String? = nil, environment: String? = nil) {
        self.provider = provider
        self.profile = profile
        self.region = region
        self.zone = zone
        self.project = project
        self.environment = environment
    }

    var arguments: [String: JSONValue] {
        var a: [String: JSONValue] = ["provider": .string(provider)]
        if let profile { a["profile"] = .string(profile) }
        if let region { a["region"] = .string(region) }
        if let zone { a["zone"] = .string(zone) }
        if let project { a["project"] = .string(project) }
        if let environment { a["environment"] = .string(environment) }
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

/// Your clouds: connect AWS, Google Cloud or Modal, check one without
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

    /// `cloud_disconnect`: nothing in the cloud is deleted.
    public func disconnect(_ provider: String) async throws {
        _ = try await connection.object("cloud_disconnect", ["provider": .string(provider)])
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

extension SpacesConnection {
    /// Spaces in your own cloud account.
    public var cloud: Cloud { Cloud(connection: self) }
}
