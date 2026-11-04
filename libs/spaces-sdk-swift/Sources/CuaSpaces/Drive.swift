import Foundation

// MARK: - Cua Volume: the volume every Space and agent shares

/// Whose view of the drive a call takes. `.user` (the default) sees the
/// whole drive; `.agent` sees exactly what that persistent agent sees: it
/// reads `public/`, writes its own home and its Space's folder, and
/// anything wider needs a grant. A view only ever narrows.
public enum DriveView: Sendable, Hashable {
    case user
    case agent(String, inSpace: SpaceID? = nil)

    var arguments: [String: JSONValue] {
        switch self {
        case .user:
            return [:]
        case let .agent(name, space):
            var a: [String: JSONValue] = ["as_agent": .string(name)]
            if let space { a["in_space"] = .string(space.rawValue) }
            return a
        }
    }
}

/// One row of a drive listing.
public struct DriveEntry: Sendable, Hashable {
    public let path: String
    public let name: String
    public let isFolder: Bool
    public let size: Int
    public let modifiedMs: Int
    public let etag: String
    /// `r` or `rw`: what the view may do there.
    public let mode: String
    /// Set when the file is uploading, conflicted, or last written by
    /// another device.
    public let sync: DriveFileSync?

    init(_ o: [String: JSONValue]) {
        path = o["path"]?.stringValue ?? ""
        name = o["name"]?.stringValue ?? ""
        isFolder = o["folder"]?.boolValue ?? false
        size = o["size"]?.intValue ?? 0
        modifiedMs = o["modified_ms"]?.intValue ?? 0
        etag = o["etag"]?.stringValue ?? ""
        mode = o["mode"]?.stringValue ?? "r"
        sync = o["sync"]?.objectValue.map(DriveFileSync.init)
    }
}

/// One file's sync state.
public struct DriveFileSync: Sendable, Hashable {
    /// `synced`, `pending_upload`, `conflict` or `conflict_copy`.
    public let state: String
    /// The device that last wrote it, when another one did.
    public let writtenBy: String?
    public let conflictPath: String?

    init(_ o: [String: JSONValue]) {
        state = o["state"]?.stringValue ?? "synced"
        writtenBy = o["written_by"]?.stringValue
        conflictPath = o["conflict_path"]?.stringValue
    }
}

/// A file's content and version.
public struct DriveFile: Sendable, Hashable {
    public let path: String
    public let etag: String
    public let version: String
    public let content: Data
    public let sync: DriveFileSync?

    init(_ o: [String: JSONValue]) {
        path = o["path"]?.stringValue ?? ""
        etag = o["etag"]?.stringValue ?? ""
        version = o["version"]?.stringValue ?? ""
        sync = o["sync"]?.objectValue.map(DriveFileSync.init)
        let text = o["content"]?.stringValue ?? ""
        if o["encoding"]?.stringValue == "base64" {
            content = Data(base64Encoded: text) ?? Data()
        } else {
            content = Data(text.utf8)
        }
    }
}

/// A written version (or one entry of a file's history).
public struct DriveVersion: Sendable, Hashable {
    public let version: String
    public let etag: String
    public let size: Int
    public let isDeleted: Bool
    public let isLatest: Bool

    init(_ o: [String: JSONValue]) {
        version = o["version"]?.stringValue ?? ""
        etag = o["etag"]?.stringValue ?? ""
        size = o["size"]?.intValue ?? 0
        isDeleted = o["deleted"]?.boolValue ?? false
        isLatest = o["latest"]?.boolValue ?? false
    }
}

/// A widening of an agent's or a Space's access.
public struct DriveGrant: Sendable, Hashable, Identifiable {
    public let id: String
    public let principal: String
    public let prefix: String
    public let mode: String
    public let isRevoked: Bool

    init(_ o: [String: JSONValue]) {
        id = o["id"]?.stringValue ?? ""
        principal = o["principal"]?.stringValue ?? ""
        prefix = o["prefix"]?.stringValue ?? ""
        mode = o["mode"]?.stringValue ?? "r"
        isRevoked = o["revoked"]?.boolValue ?? false
    }
}

/// An agent's request for more access, waiting for the user.
public struct DriveAccessRequest: Sendable, Hashable, Identifiable {
    public let id: String
    public let principal: String
    public let prefix: String
    public let mode: String
    /// The agent's own words, shown as unverified.
    public let reason: String

    init(_ o: [String: JSONValue]) {
        id = o["id"]?.stringValue ?? ""
        principal = o["principal"]?.stringValue ?? ""
        prefix = o["prefix"]?.stringValue ?? ""
        mode = o["mode"]?.stringValue ?? "r"
        reason = o["reason"]?.stringValue ?? ""
    }
}

/// One audit event.
public struct DriveAuditEvent: Sendable, Hashable {
    public let seq: Int
    public let principal: String
    public let action: String
    public let path: String
    public let detail: String

    init(_ o: [String: JSONValue]) {
        seq = o["seq"]?.intValue ?? 0
        principal = o["principal"]?.stringValue ?? ""
        action = o["action"]?.stringValue ?? ""
        path = o["path"]?.stringValue ?? ""
        detail = o["detail"]?.stringValue ?? ""
    }
}

/// Cua Volume: `public/`, `agents/<agent>/`, `spaces/<space>/`. Access is
/// checked by the runtime (the daemon, or this process when embedded);
/// grants and approvals ask the user for presence there.
public struct Drive: Sendable {
    let connection: SpacesConnection

    public func list(_ path: String = "", as view: DriveView = .user) async throws -> [DriveEntry] {
        var a = view.arguments
        a["path"] = .string(path)
        let o = try await connection.object("volume_ls", a)
        return (o["entries"]?.arrayValue ?? []).compactMap(\.objectValue).map(DriveEntry.init)
    }

    public func read(_ path: String, version: String? = nil,
                     as view: DriveView = .user) async throws -> DriveFile {
        var a = view.arguments
        a["path"] = .string(path)
        if let version { a["version"] = .string(version) }
        return DriveFile(try await connection.object("volume_read", a))
    }

    /// A new version. `ifEtag` makes it a compare-and-swap, `createOnly`
    /// create-only.
    @discardableResult
    public func write(_ content: Data, to path: String, ifEtag: String? = nil,
                      createOnly: Bool = false, as view: DriveView = .user) async throws -> DriveVersion {
        var a = view.arguments
        a["path"] = .string(path)
        a["content"] = .string(content.base64EncodedString())
        a["encoding"] = "base64"
        a["create_only"] = .bool(createOnly)
        if let ifEtag { a["if_etag"] = .string(ifEtag) }
        return DriveVersion(try await connection.object("volume_write", a))
    }

    public func delete(_ path: String, ifEtag: String? = nil, as view: DriveView = .user) async throws {
        var a = view.arguments
        a["path"] = .string(path)
        if let ifEtag { a["if_etag"] = .string(ifEtag) }
        _ = try await connection.object("volume_delete", a)
    }

    public func history(_ path: String, as view: DriveView = .user) async throws -> [DriveVersion] {
        var a = view.arguments
        a["path"] = .string(path)
        let o = try await connection.object("volume_history", a)
        return (o["versions"]?.arrayValue ?? []).compactMap(\.objectValue).map(DriveVersion.init)
    }

    @discardableResult
    public func restore(_ path: String, version: String, as view: DriveView = .user) async throws -> DriveVersion {
        var a = view.arguments
        a["path"] = .string(path)
        a["version"] = .string(version)
        return DriveVersion(try await connection.object("volume_restore", a))
    }

    /// Widen `principal`'s access (`agent:<name>` or `space:<id>`). The user
    /// confirms with presence before anything widens.
    @discardableResult
    public func grant(_ principal: String, _ mode: String, on prefix: String,
                      expiresInSecs: Int? = nil) async throws -> DriveGrant {
        var a: [String: JSONValue] = ["principal": .string(principal),
                                      "prefix": .string(prefix), "mode": .string(mode)]
        if let expiresInSecs { a["expires_in_secs"] = .number(Double(expiresInSecs)) }
        return DriveGrant(try await connection.object("volume_grant", a))
    }

    @discardableResult
    public func revoke(_ grantID: String) async throws -> DriveGrant {
        DriveGrant(try await connection.object("volume_revoke", ["grant_id": .string(grantID)]))
    }

    public func grants(includingExpired all: Bool = false) async throws -> [DriveGrant] {
        let o = try await connection.object("volume_grants", ["all": .bool(all)])
        return (o["grants"]?.arrayValue ?? []).compactMap(\.objectValue).map(DriveGrant.init)
    }

    /// `agent` asks the user for more access; nothing widens until the user
    /// approves.
    @discardableResult
    public func requestAccess(for agent: String, _ mode: String, on prefix: String,
                              reason: String) async throws -> DriveAccessRequest {
        DriveAccessRequest(try await connection.object("volume_request_access", [
            "as_agent": .string(agent), "prefix": .string(prefix),
            "mode": .string(mode), "reason": .string(reason)]))
    }

    public func requests() async throws -> [DriveAccessRequest] {
        let o = try await connection.object("volume_requests", [:])
        return (o["requests"]?.arrayValue ?? []).compactMap(\.objectValue).map(DriveAccessRequest.init)
    }

    @discardableResult
    public func approve(_ requestID: String) async throws -> DriveGrant {
        DriveGrant(try await connection.object("volume_approve", ["request_id": .string(requestID)]))
    }

    public func deny(_ requestID: String) async throws {
        _ = try await connection.object("volume_deny", ["request_id": .string(requestID)])
    }

    /// The newest audit events and whether the hash chain verified.
    public func audit(limit: Int = 50) async throws -> (events: [DriveAuditEvent], verified: Bool) {
        let o = try await connection.object("volume_audit", ["limit": .number(Double(limit))])
        let events = (o["events"]?.arrayValue ?? []).compactMap(\.objectValue).map(DriveAuditEvent.init)
        return (events, o["verified"]?.boolValue ?? false)
    }
}

// MARK: - The drive's runtime: storage, the mount, sync, the cache

/// Where the drive keeps its bytes.
public struct DriveStorage: Sendable, Hashable {
    /// `fs` or `s3`.
    public let backend: String
    public let fsPath: String
    public let endpoint: String?
    public let region: String
    public let bucket: String
    public let root: String
    public let pathStyle: Bool
    /// S3 keys are saved (never returned).
    public let hasKeys: Bool
    /// Always false in this release.
    public let cloudAvailable: Bool

    init(_ o: [String: JSONValue]) {
        backend = o["backend"]?.stringValue ?? "fs"
        fsPath = o["fs_path"]?.stringValue ?? ""
        let s3 = o["s3"]?.objectValue ?? [:]
        endpoint = s3["endpoint"]?.stringValue
        region = s3["region"]?.stringValue ?? "us-east-1"
        bucket = s3["bucket"]?.stringValue ?? ""
        root = s3["root"]?.stringValue ?? ""
        pathStyle = s3["path_style"]?.boolValue ?? false
        hasKeys = o["has_keys"]?.boolValue ?? false
        cloudAvailable = o["cloud_available"]?.boolValue ?? false
    }
}

/// What a storage test found.
public struct DriveStorageCheck: Sendable, Hashable {
    public let ok: Bool
    public let reachable: Bool
    public let authorized: Bool
    public let versioning: Bool
    public let detail: String?
    /// `unreachable`, `bad_keys`, `forbidden`, `bucket_missing`,
    /// `versioning_off`, `path_style_needed`.
    public let problem: String?
    public let applied: Bool

    init(_ o: [String: JSONValue]) {
        ok = o["ok"]?.boolValue ?? false
        problem = o["problem"]?.stringValue
        reachable = o["reachable"]?.boolValue ?? false
        authorized = o["authorized"]?.boolValue ?? false
        versioning = o["versioning"]?.boolValue ?? false
        detail = o["detail"]?.stringValue
        applied = o["applied"]?.boolValue ?? false
    }
}

/// The drive as a volume.
public struct DriveMountStatus: Sendable, Hashable {
    public let enabled: Bool
    /// `off`, `mounting`, `mounted`, `needs_approval`, `unsupported`, `error`.
    public let state: String
    /// `nfs`, `fuse`, `fskit`, `none`.
    public let method: String
    public let path: String?
    public let volumeName: String
    public let detail: String?
    public let settingsURL: String?

    init(_ o: [String: JSONValue]) {
        enabled = o["enabled"]?.boolValue ?? false
        state = o["state"]?.stringValue ?? "off"
        method = o["method"]?.stringValue ?? "none"
        path = o["path"]?.stringValue
        volumeName = o["volume_name"]?.stringValue ?? "Cua Volume"
        detail = o["detail"]?.stringValue
        settingsURL = o["settings_url"]?.stringValue
    }
}

/// Sync across devices sharing the bucket.
public struct DriveSyncStatus: Sendable, Hashable {
    public let deviceID: String
    public let deviceName: String
    /// `live`, `off` (a store on this machine), `offline` (the bucket
    /// stopped answering; see `last_error`).
    public let feed: String
    public let lastPollMs: Int
    public let pendingUploads: Int
    public let pendingBytes: Int
    /// `(path, conflictPath)` of each unresolved conflict, newest first.
    public let conflicts: [DriveConflict]
    /// Every device seen, this one first.
    public let devices: [DriveDevice]
    public let lastError: String?
    /// The files still uploading, largest first.
    public let pending: [DrivePendingUpload]
    /// `fs` (This Mac) or `s3` (your bucket).
    public let backend: String
    /// This machine's mount state.
    public let mount: String
    public let cache: DriveCacheStats?
    /// The volume mounted in Spaces.
    public let volumes: [DriveSpaceVolume]

    init(_ o: [String: JSONValue]) {
        deviceID = o["device_id"]?.stringValue ?? ""
        deviceName = o["device_name"]?.stringValue ?? ""
        feed = o["feed"]?.stringValue ?? "off"
        lastPollMs = o["last_poll_ms"]?.intValue ?? 0
        pendingUploads = o["pending_uploads"]?.intValue ?? 0
        pendingBytes = o["pending_bytes"]?.intValue ?? 0
        conflicts = (o["conflicts"]?.arrayValue ?? []).compactMap(\.objectValue).map(DriveConflict.init)
        devices = (o["devices"]?.arrayValue ?? []).compactMap(\.objectValue).map(DriveDevice.init)
        lastError = o["last_error"]?.stringValue
        pending = (o["pending"]?.arrayValue ?? []).compactMap(\.objectValue).map(DrivePendingUpload.init)
        backend = o["backend"]?.stringValue ?? ""
        mount = o["mount"]?.stringValue ?? ""
        cache = o["cache"]?.objectValue.map(DriveCacheStats.init)
        volumes = (o["volumes"]?.arrayValue ?? []).compactMap(\.objectValue).map(DriveSpaceVolume.init)
    }
}

/// A file waiting to upload (other devices do not see it yet).
public struct DrivePendingUpload: Sendable, Hashable {
    public let path: String
    public let bytes: Int

    init(_ o: [String: JSONValue]) {
        path = o["path"]?.stringValue ?? ""
        bytes = o["bytes"]?.intValue ?? 0
    }
}

/// The volume mounted in a Space's guest.
public struct DriveSpaceVolume: Sendable, Hashable {
    public let space: String
    /// `/volume` (Linux) or `~/Cua Volume` (macOS), as the guest sees it.
    public let mountPath: String
    /// `fs` (FUSE) or `nfs`.
    public let backend: String
    /// `space:<folder>`, or `agent:<name>` while a persistent agent runs.
    public let principal: String

    init(_ o: [String: JSONValue]) {
        space = o["space"]?.stringValue ?? ""
        mountPath = o["mount_path"]?.stringValue ?? ""
        backend = o["backend"]?.stringValue ?? ""
        principal = o["principal"]?.stringValue ?? ""
    }
}

/// A write that lost to a later one; kept in history and as a copy.
public struct DriveConflict: Sendable, Hashable {
    public let path: String
    public let conflictPath: String
    public let loserDevice: String
    public let tsMs: Int

    init(_ o: [String: JSONValue]) {
        path = o["path"]?.stringValue ?? ""
        conflictPath = o["conflict_path"]?.stringValue ?? ""
        loserDevice = o["loser_device"]?.stringValue ?? ""
        tsMs = o["ts_ms"]?.intValue ?? 0
    }
}

/// A device sharing the drive.
public struct DriveDevice: Sendable, Hashable {
    public let id: String
    public let name: String
    public let isThisDevice: Bool
    public let lastSeenMs: Int
    public let lastChangeMs: Int

    init(_ o: [String: JSONValue]) {
        id = o["id"]?.stringValue ?? ""
        name = o["name"]?.stringValue ?? ""
        isThisDevice = o["this_device"]?.boolValue ?? false
        lastSeenMs = o["last_seen_ms"]?.intValue ?? 0
        lastChangeMs = o["last_change_ms"]?.intValue ?? 0
    }
}

/// One sync event.
public struct DriveSyncEvent: Sendable, Hashable {
    public let seq: Int
    public let tsMs: Int
    /// `remote_change`, `remote_delete`, `upload_started`, `upload_done`,
    /// `upload_failed`, `conflict`, `error`.
    public let kind: String
    public let path: String
    public let device: String
    public let detail: String

    init(_ o: [String: JSONValue]) {
        seq = o["seq"]?.intValue ?? 0
        tsMs = o["ts_ms"]?.intValue ?? 0
        kind = o["kind"]?.stringValue ?? ""
        path = o["path"]?.stringValue ?? ""
        device = o["device"]?.stringValue ?? ""
        detail = o["detail"]?.stringValue ?? ""
    }
}

/// The block cache in front of a remote store.
public struct DriveCacheStats: Sendable, Hashable {
    public let sizeBytes: Int
    public let capacityBytes: Int
    public let blocks: Int
    public let hits: Int
    public let misses: Int
    public let hitRate: Double

    init(_ o: [String: JSONValue]) {
        sizeBytes = o["size_bytes"]?.intValue ?? 0
        capacityBytes = o["capacity_bytes"]?.intValue ?? 0
        blocks = o["blocks"]?.intValue ?? 0
        hits = o["hits"]?.intValue ?? 0
        misses = o["misses"]?.intValue ?? 0
        hitRate = o["hit_rate"]?.doubleValue ?? 0
    }
}

extension Drive {
    public func storage() async throws -> DriveStorage {
        DriveStorage(try await connection.object("volume_storage", [:]))
    }

    /// Tests (`dryRun`) or saves and switches to a backend: `fs`, or `s3`
    /// with a bucket and keys.
    @discardableResult
    public func setStorage(backend: String, endpoint: String? = nil, region: String = "us-east-1",
                           bucket: String = "", root: String = "", pathStyle: Bool = false,
                           accessKeyID: String? = nil, secretAccessKey: String? = nil,
                           dryRun: Bool = false) async throws -> DriveStorageCheck {
        var a: [String: JSONValue] = ["backend": .string(backend), "dry_run": .bool(dryRun)]
        if backend == "s3" {
            var s3: [String: JSONValue] = ["region": .string(region), "bucket": .string(bucket),
                                           "root": .string(root), "path_style": .bool(pathStyle)]
            if let endpoint { s3["endpoint"] = .string(endpoint) }
            a["s3"] = .object(s3)
        }
        if let accessKeyID { a["access_key_id"] = .string(accessKeyID) }
        if let secretAccessKey { a["secret_access_key"] = .string(secretAccessKey) }
        return DriveStorageCheck(try await connection.object("volume_storage_set", a))
    }

    public func mountStatus() async throws -> DriveMountStatus {
        DriveMountStatus(try await connection.object("volume_mount_status", [:]))
    }

    @discardableResult
    public func mount() async throws -> DriveMountStatus {
        DriveMountStatus(try await connection.object("volume_mount", [:]))
    }

    @discardableResult
    public func unmount() async throws -> DriveMountStatus {
        DriveMountStatus(try await connection.object("volume_unmount", [:]))
    }

    public func syncStatus() async throws -> DriveSyncStatus {
        DriveSyncStatus(try await connection.object("volume_sync_status", [:]))
    }

    /// Events after `sinceSeq`, waiting up to `waitMs` for one.
    public func syncEvents(since sinceSeq: Int = 0, waitMs: Int = 0) async throws
        -> (events: [DriveSyncEvent], nextSeq: Int) {
        let o = try await connection.object("volume_sync_events",
                                            ["since_seq": .number(Double(sinceSeq)),
                                             "wait_ms": .number(Double(waitMs))])
        let events = (o["events"]?.arrayValue ?? []).compactMap(\.objectValue).map(DriveSyncEvent.init)
        return (events, o["next_seq"]?.intValue ?? sinceSeq)
    }

    public func resolveConflict(_ path: String) async throws {
        _ = try await connection.object("volume_sync_resolve", ["path": .string(path)])
    }

    public func cacheStats() async throws -> DriveCacheStats {
        DriveCacheStats(try await connection.object("volume_cache_stats", [:]))
    }

    @discardableResult
    public func setCacheCapacity(_ bytes: Int) async throws -> DriveCacheStats {
        DriveCacheStats(try await connection.object("volume_cache_set",
                                                    ["capacity_bytes": .number(Double(bytes))]))
    }

    @discardableResult
    public func clearCache() async throws -> DriveCacheStats {
        DriveCacheStats(try await connection.object("volume_cache_clear", [:]))
    }
}

extension SpacesConnection {
    /// Cua Volume, shared by every Space and agent of this account.
    public var drive: Drive { Drive(connection: self) }
}
