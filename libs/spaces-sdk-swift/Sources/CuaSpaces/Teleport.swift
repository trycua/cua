import Foundation

/// Session teleport: pushing a **logged-in host app session** into a Space, so
/// an agent working inside the Space is already authenticated.
///
/// Two facts shape everything here, and they are facts about real data rather
/// than preferences.
///
/// The first: a manifest is a **consent surface, not a file list**. The Claude
/// Code manifest comes back with three items totalling 935,980,082 bytes, and
/// the largest of them — `claude/projects/`, 14 projects of conversation
/// transcripts — is marked sensitive and deliberately *not* checked by default.
/// The server declined to make that decision on the user's behalf. A caller
/// that reads `items` as a list of paths to send ships ~900 MB of transcripts
/// onto a machine an autonomous agent is driving, one array element away from
/// the login-only teleport it meant to perform.
///
/// The second: consent is a **type**, not a defaulted parameter. An `Approval`
/// can only be minted from a manifest, so it cannot name a path the server did
/// not offer, and minting one that carries sensitive entries throws unless the
/// caller acknowledges them by name.
///
/// The provider split is behind one call. A cloud Space pushes to the cloud
/// teleport gateway; a local (Lume macOS) Space pushes to the Space's own
/// in-guest cua-spacesd teleport receiver, or copies the file directly for the one app
/// (codex) that has no rcdp provider. Both are `teleport_app`, and both work.

/// How much of an app's session to consider.
///
/// These are the two values `teleport_manifest` and `teleport_app` actually
/// take on the wire (`--scope full|tabs`). Nothing here is aspirational.
public enum TeleportScope: String, Sendable, Hashable, Codable {
    /// The whole profile: credentials, cookies, preferences, local state.
    case full
    /// Open tabs only, and no credentials.
    case tabs
}

/// An application whose logged-in session can be moved into a Space.
///
/// A raw-value type rather than a closed enum, because the set of providers
/// lives in rcdp and grows there, not here.
public struct TeleportableApp: Sendable, Hashable, Identifiable, ExpressibleByStringLiteral {
    /// The id the backend takes, e.g. `chrome`, `claude-code`.
    public let id: String
    public let displayName: String
    public let isInstalledOnHost: Bool

    public init(id: String, displayName: String? = nil, isInstalledOnHost: Bool = true) {
        self.id = id
        self.displayName = displayName ?? id
        self.isInstalledOnHost = isInstalledOnHost
    }

    public init(_ id: String) { self.init(id: id) }
    public init(stringLiteral value: String) { self.init(id: value) }

    public static let claudeCode = TeleportableApp("claude-code")
    public static let codex = TeleportableApp("codex")
    public static let chrome = TeleportableApp("chrome")
}

/// One transferable item in an app's manifest.
///
/// `isSensitive` and `isCheckedByDefault` are the two fields that make a
/// manifest a consent surface rather than a file list: the logged-in session is
/// sensitive and checked by default, while conversation transcripts are
/// sensitive and *unchecked*.
public struct TeleportItem: Sendable, Hashable, Identifiable {
    public var id: String { relativePath }
    /// A `rel_path` from the manifest — the exact string `include` takes.
    public let relativePath: String
    /// The server's display label, e.g. "Logged-in session".
    public let label: String
    public let estimatedBytes: Int
    /// Credentials, cookies, tokens, transcripts.
    public let isSensitive: Bool
    /// Whether the server's own default selection includes this. The server
    /// leaves the expensive items unchecked on purpose.
    public let isCheckedByDefault: Bool
    /// e.g. 14 `projects`, when the item is a directory the server counts.
    public let count: Int?
    public let countNoun: String?
    /// Display-ready, for a consent sheet.
    public let explanation: String

    public init(relativePath: String, label: String = "", estimatedBytes: Int = 0,
                isSensitive: Bool = false, isCheckedByDefault: Bool = false,
                count: Int? = nil, countNoun: String? = nil, explanation: String = "") {
        self.relativePath = relativePath
        self.label = label
        self.estimatedBytes = estimatedBytes
        self.isSensitive = isSensitive
        self.isCheckedByDefault = isCheckedByDefault
        self.count = count
        self.countNoun = countNoun
        self.explanation = explanation
    }

    /// The spelling the earlier API surface used, kept so a call site that
    /// already names `byteCount` and `isDefault` keeps compiling.
    public init(relativePath: String, byteCount: Int, isSensitive: Bool,
                isDefault: Bool, explanation: String = "") {
        self.init(relativePath: relativePath, label: "", estimatedBytes: byteCount,
                  isSensitive: isSensitive, isCheckedByDefault: isDefault,
                  explanation: explanation)
    }

    public var byteCount: Int { estimatedBytes }
    public var isDefault: Bool { isCheckedByDefault }

    /// Read a row without inventing fields. Both key spellings the backends use
    /// are accepted, and an unreadable row yields an empty `relativePath` the
    /// caller filters out rather than a guess.
    init(row: [String: JSONValue]) {
        self.init(relativePath: row["relative_path"]?.stringValue
                    ?? row["rel_path"]?.stringValue ?? row["path"]?.stringValue ?? "",
                  label: row["label"]?.stringValue ?? "",
                  estimatedBytes: row["estimated_bytes"]?.intValue ?? row["est_bytes"]?.intValue
                    ?? row["bytes"]?.intValue ?? row["size"]?.intValue ?? 0,
                  isSensitive: row["is_sensitive"]?.boolValue
                    ?? row["sensitive"]?.boolValue ?? false,
                  isCheckedByDefault: row["is_checked_by_default"]?.boolValue
                    ?? row["default_checked"]?.boolValue
                    ?? row["default"]?.boolValue ?? row["is_default"]?.boolValue ?? false,
                  count: row["count"]?.intValue,
                  countNoun: row["count_noun"]?.stringValue,
                  explanation: row["note"]?.stringValue ?? row["why"]?.stringValue ?? "")
    }
}

/// Exactly what would leave this machine, before anything does.
public struct TeleportManifest: Sendable, Hashable {
    /// The earlier spelling of an item, so existing call sites keep compiling.
    public typealias Entry = TeleportItem

    public let app: TeleportableApp
    public let displayName: String
    public let scope: TeleportScope
    /// The server's own scope string, verbatim — `full_profile` rather than
    /// `full`, on the backend that says so.
    public let serverScope: String
    public let items: [TeleportItem]
    /// The server's own total. It is not the sum of `items` on every backend,
    /// so it is carried rather than recomputed.
    public let totalEstimatedBytes: Int
    /// The server's own warnings, verbatim. They say things a caller cannot
    /// derive — that the logged-in session carries an OAuth token, for one.
    public let notes: [String]

    public init(app: TeleportableApp, scope: TeleportScope, entries: [TeleportItem],
                displayName: String? = nil, serverScope: String? = nil,
                totalEstimatedBytes: Int? = nil, notes: [String] = []) {
        self.app = app
        self.displayName = displayName ?? app.displayName
        self.scope = scope
        self.serverScope = serverScope ?? scope.rawValue
        self.items = entries
        self.totalEstimatedBytes =
            totalEstimatedBytes ?? entries.reduce(0) { $0 + $1.estimatedBytes }
        self.notes = notes
    }

    /// The earlier spelling of `items`.
    public var entries: [TeleportItem] { items }
    public var totalBytes: Int { totalEstimatedBytes }
    public var sensitiveEntries: [TeleportItem] { items.filter(\.isSensitive) }

    /// The items the server marks as checked by default — what a teleport sends
    /// when the caller does not choose. Not everything.
    public var defaultSelection: [TeleportItem] { items.filter(\.isCheckedByDefault) }

    /// The earlier spelling of `defaultSelection`.
    public var defaultEntries: [TeleportItem] { defaultSelection }

    /// Just the logged-in session: the smallest teleport that still leaves an
    /// in-Space agent authenticated. For Claude Code this is exactly
    /// `["claude/.credentials.json"]`.
    public var loginOnlySelection: [TeleportItem] {
        let checked = defaultSelection
        let sensitive = checked.filter(\.isSensitive)
        return sensitive.isEmpty ? checked : sensitive
    }

    public func item(_ relativePath: String) -> TeleportItem? {
        items.first { $0.relativePath == relativePath }
    }

    /// The **only** constructor of a sendable approval.
    ///
    /// This is the one place in the SDK where a step is mandatory rather than
    /// convenient, because this call moves live credentials onto a machine an
    /// autonomous agent is driving. A caller cannot ship cookies by leaving a
    /// parameter at its default: if any approved item is sensitive and
    /// `acknowledgingSensitiveItems` is `false`, this throws.
    public func approving(_ items: [TeleportItem],
                          into space: Space,
                          acknowledgingSensitiveItems: Bool = false) throws -> Approval {
        try approve(items, into: space,
                    acknowledgingSensitiveItems: acknowledgingSensitiveItems,
                    includes: items.map(\.relativePath))
    }

    /// Approve the **server's** default set without naming it.
    ///
    /// The resulting approval carries no `include` on the wire, which is what
    /// makes the server choose — the server's default, not everything. The
    /// consent gate still applies, because the default set contains the
    /// logged-in session and that is sensitive.
    public func approvingServerDefault(into space: Space,
                                       acknowledgingSensitiveItems: Bool = false) throws -> Approval {
        try approve(defaultSelection, into: space,
                    acknowledgingSensitiveItems: acknowledgingSensitiveItems,
                    includes: nil)
    }

    private func approve(_ selected: [TeleportItem], into space: Space,
                         acknowledgingSensitiveItems: Bool,
                         includes: [String]?) throws -> Approval {
        let unknown = selected.filter { candidate in
            !items.contains { $0.relativePath == candidate.relativePath }
        }
        guard unknown.isEmpty else {
            throw SpacesError.teleportRefused(
                "approved entries that are not in this manifest: "
                + unknown.map(\.relativePath).joined(separator: ", "))
        }
        let sensitive = selected.filter(\.isSensitive)
        guard sensitive.isEmpty || acknowledgingSensitiveItems else {
            throw SpacesError.teleportRefused(
                "\(sensitive.count) approved entries are sensitive "
                + "(\(sensitive.map(\.relativePath).joined(separator: ", "))); "
                + "pass acknowledgingSensitiveItems: true to send them")
        }
        return Approval(app: app, scope: scope, space: space.id,
                        includes: includes,
                        acknowledgedSensitive: !sensitive.isEmpty,
                        approvedBytes: selected.reduce(0) { $0 + $1.estimatedBytes },
                        approvedAt: Date())
    }

    /// Proof a human agreed, and to what. Only `approving(_:into:)` and
    /// `approvingServerDefault(into:)` mint one, and only
    /// `SessionTeleport.send` accepts one.
    public struct Approval: Sendable, Hashable {
        let app: TeleportableApp
        let scope: TeleportScope
        let space: SpaceID
        /// Per-path `include`, mapping straight onto `teleport_app`'s
        /// `include`. `nil` means send no `include` at all and let the **server**
        /// pick its default set — which is never "everything".
        let includes: [String]?
        /// A human acknowledged sensitive items in this selection (the gate
        /// above refuses to mint one otherwise); carried to the server, which
        /// checks the same thing again.
        let acknowledgedSensitive: Bool
        /// What the approved items were estimated to weigh, so a caller can log
        /// the size it agreed to without re-deriving it.
        public let approvedBytes: Int
        public let approvedAt: Date

        public var approvedPaths: [String] { includes ?? [] }
        public var usesServerDefault: Bool { includes == nil }
        public var appID: String { app.id }
    }
}

/// What actually moved.
public struct TeleportReceipt: Sendable, Hashable {
    public let app: TeleportableApp
    public let space: SpaceID
    /// `import_session` (TeleportService.ImportSession) on every provider.
    public let method: String
    /// The paths the Space imported, relative to the app's profile: what the
    /// Keyvault delivered, which is what the user keeps in it for the app.
    public let transferredPaths: [String]
    /// The backend's own answer, kept rather than parsed into a claim.
    public let rawResult: String

    public init(app: TeleportableApp, space: SpaceID, method: String = "import_session",
                transferredPaths: [String], rawResult: String) {
        self.app = app
        self.space = space
        self.method = method
        self.transferredPaths = transferredPaths
        self.rawResult = rawResult
    }

    /// The earlier spelling of `transferredPaths`.
    public var included: [String] { transferredPaths }
}

/// Session teleport: arriving already signed in.
///
/// The differentiated primitive, and the largest security surface — so consent
/// is a type, not a default.
public struct SessionTeleport: Sendable {
    let space: Space

    /// Preview exactly what would be transferred. **Never moves a byte.**
    ///
    /// The manifest is a property of the **host**, not of the Space — the
    /// underlying tool takes no `space` argument — but it hangs off the Space
    /// anyway, because every caller reads a manifest in order to teleport into a
    /// particular Space, and a Space that cannot teleport should say so here
    /// rather than after the user has chosen.
    public func manifest(for app: TeleportableApp,
                         scope: TeleportScope = .full) async throws -> TeleportManifest {
        try requireTeleport()
        let raw = try await space.connection.callTool("teleport_manifest", [
            "app": .string(app.id), "scope": .string(scope.rawValue),
        ])
        let manifest = TeleportManifest(app: app, scope: scope, raw: raw)
        guard !manifest.items.isEmpty else {
            throw SpacesError.malformedResponse(
                tool: "teleport_manifest", detail: "no items for \(app.id) in \(raw)")
        }
        return manifest
    }

    /// Move the session. Takes an `Approval` and nothing else, which is the
    /// whole design: there is no overload that takes an app id.
    ///
    /// The Cua Keyvault moves it, never this process: `teleport_app` files a
    /// Keyvault request, the user approves it in Cua (Touch ID or the login
    /// password), and the Keyvault delivers what it holds for the app. This
    /// waits up to `consentTimeout` for that decision, then returns what the
    /// Space imported. It throws `teleportRefused` when the user declines,
    /// when the wait runs out, or with `requires_cua_app` when no Keyvault is
    /// reachable (an embedded runtime: the user needs the Cua app).
    @discardableResult
    public func send(_ approval: TeleportManifest.Approval,
                     consentTimeout: Duration = .seconds(120)) async throws -> TeleportReceipt {
        guard approval.space == space.id else {
            throw SpacesError.teleportRefused(
                "this approval was granted for \(approval.space), not \(space.id)")
        }
        try requireTeleport()
        var args: [String: JSONValue] = [
            "space": .string(space.id.rawValue),
            "app": .string(approval.app.id),
            "scope": .string(approval.scope.rawValue),
        ]
        // A `nil` include is the point, not an omission: it hands the choice
        // back to the server, whose default set leaves the expensive items out.
        if let includes = approval.includes, !includes.isEmpty {
            args["include"] = .array(includes.map(JSONValue.string))
        }
        if approval.acknowledgedSensitive {
            args["acknowledge_sensitive"] = .bool(true)
        }
        let deadline = ContinuousClock.now + consentTimeout
        var raw = try await space.connection.callTool("teleport_app", args)
        // Bounded: each retry waits on the Keyvault (up to 20 s server side),
        // and the deadline ends the loop.
        for _ in 0..<10_000 {
            let object = TeleportManifest.object(from: raw)
            let message = object["message"]?.stringValue
            if object["moved"]?.boolValue == true {
                return TeleportReceipt(
                    app: approval.app, space: space.id,
                    transferredPaths: (object["transferred_paths"]?.arrayValue ?? [])
                        .compactMap(\.stringValue),
                    rawResult: raw.stringValue ?? raw.description)
            }
            if let error = object["error"]?.objectValue {
                let code = error["code"]?.stringValue ?? "error"
                throw SpacesError.teleportRefused(
                    "\(code): \(error["message"]?.stringValue ?? "the Keyvault refused")")
            }
            if object["denied"]?.boolValue == true {
                throw SpacesError.teleportRefused(message ?? "the user declined the teleport")
            }
            guard object["consent_required"]?.boolValue == true else {
                throw SpacesError.malformedResponse(
                    tool: "teleport_app", detail: raw.stringValue ?? raw.description)
            }
            guard let request = object["request_id"]?.stringValue else {
                // No Keyvault behind this runtime to file the request with.
                throw SpacesError.teleportRefused(Self.requiresCuaApp)
            }
            guard ContinuousClock.now < deadline else {
                throw SpacesError.teleportRefused(
                    "Keyvault request \(request) is still waiting for the user's approval in Cua")
            }
            args["request_id"] = .string(request)
            raw = try await space.connection.callTool("teleport_app", args)
        }
        throw SpacesError.teleportRefused("the Keyvault did not answer")
    }

    /// The Keyvault's `RequiresCuaApp` refusal (cua-keyvault `embedded`).
    static let requiresCuaApp = "requires_cua_app: Teleport requires the Cua app, which keeps your "
        + "sessions in its Keyvault and asks you before anything moves. Install it from "
        + "https://cua.ai/download."

    /// Read the manifest and send exactly the logged-in session — the smallest
    /// teleport that still leaves an in-Space agent authenticated, and the one
    /// that cannot accidentally carry the 900 MB transcript item.
    ///
    /// The acknowledgement is not defaulted, because the logged-in session is a
    /// credential.
    @discardableResult
    public func sendLoginOnly(for app: TeleportableApp,
                              acknowledgingSensitiveItems: Bool) async throws -> TeleportReceipt {
        let manifest = try await manifest(for: app)
        return try await send(manifest.approving(
            manifest.loginOnlySelection, into: space,
            acknowledgingSensitiveItems: acknowledgingSensitiveItems))
    }

    /// Read the manifest and defer the selection to the server's own default
    /// set. Sends no `include`, so the server decides — and its default is not
    /// everything.
    @discardableResult
    public func sendServerDefault(for app: TeleportableApp,
                                  acknowledgingSensitiveItems: Bool) async throws -> TeleportReceipt {
        let manifest = try await manifest(for: app)
        return try await send(manifest.approvingServerDefault(
            into: space, acknowledgingSensitiveItems: acknowledgingSensitiveItems))
    }

    /// For a script or a CI job, which still has to name what it is approving.
    /// Verbose by design — and on the Claude Code manifest this is the ~900 MB
    /// call, which is why it is spelled out rather than reached by a default.
    @discardableResult
    public func sendApprovingEverything(in manifest: TeleportManifest,
                                        acknowledgingSensitiveItems: Bool) async throws -> TeleportReceipt {
        try await send(manifest.approving(
            manifest.items, into: space,
            acknowledgingSensitiveItems: acknowledgingSensitiveItems))
    }

    private func requireTeleport() throws {
        guard space.capabilities.teleport else {
            throw SpacesError.unsupportedByProvider(
                tool: "teleport_app", provider: space.provider,
                detail: "this provider has no teleport receiver")
        }
    }
}

extension TeleportManifest {
    /// Read whatever shape the manifest came back in, without inventing items.
    /// An unreadable payload yields no items rather than a guess, and
    /// `manifest(for:)` turns that into a thrown error.
    init(app: TeleportableApp, scope: TeleportScope, raw: JSONValue) {
        let object = TeleportManifest.object(from: raw)
        let rows: [[String: JSONValue]]
        if let array = raw.arrayValue {
            rows = array.compactMap(\.objectValue)
        } else {
            rows = ((object["items"] ?? object["entries"])?.arrayValue ?? [])
                .compactMap(\.objectValue)
        }
        let display = object["display_name"]?.stringValue ?? object["app_display_name"]?.stringValue
        self.init(
            app: TeleportableApp(id: object["provider_id"]?.stringValue
                                    ?? object["app"]?.stringValue ?? app.id,
                                 displayName: display ?? app.displayName,
                                 isInstalledOnHost: app.isInstalledOnHost),
            scope: scope,
            entries: rows.map(TeleportItem.init(row:)).filter { !$0.relativePath.isEmpty },
            displayName: display,
            serverScope: object["scope"]?.stringValue,
            totalEstimatedBytes: object["total_estimated_bytes"]?.intValue
                ?? object["total_est_bytes"]?.intValue,
            notes: (object["notes"]?.arrayValue ?? []).compactMap(\.stringValue))
    }

    /// The payload arrives as an object on some backends and as a JSON string
    /// on others (`FRICTION.md` §4). Both land here.
    static func object(from raw: JSONValue) -> [String: JSONValue] {
        if let object = raw.objectValue { return object }
        if let text = raw.stringValue,
           let data = text.data(using: .utf8),
           let decoded = try? JSONSerialization.jsonObject(with: data),
           let dictionary = decoded as? [String: Any],
           let object = JSONValue(any: dictionary).objectValue {
            return object
        }
        return [:]
    }
}
