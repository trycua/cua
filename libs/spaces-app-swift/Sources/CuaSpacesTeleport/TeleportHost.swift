// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Cua
import CuaSpacesFFI
import Foundation

/// What the "Teleport an app…" model needs from its host: the cua SDK
/// (``SDKTeleportHost``) in apps, a fake in tests. The records are the
/// SDK's own (`TeleportCatalogEntry`, `TeleportPlan`, ...), so every client
/// shares one shape.
public protocol TeleportAppHost: Sendable {
    /// The classified catalog, narrowed to the Space.
    func catalog() async throws -> [TeleportCatalogEntry]
    /// What teleporting `entry` will install, send and import.
    func plan(_ entry: TeleportCatalogEntry, _ options: TeleportPlanOptions) async throws -> TeleportPlan
    /// Runs an approved plan with progress.
    func run(_ plan: TeleportPlan, consent: TeleportConsent,
             onEvent: @escaping @Sendable (TeleportRunEvent) -> Void) async throws -> TeleportRunReport
    /// An app's icon as PNG bytes (the SDK's icon cache: rendered once per
    /// app version, then from memory or disk).
    func icon(_ entry: TeleportCatalogEntry) -> Data?
    /// The catalog row for a dropped app bundle, `.desktop` entry or shortcut.
    func entry(forDroppedPath path: String) throws -> TeleportCatalogEntry
    /// Sorts dropped items (paths or `file://` URIs) into apps, files and URLs.
    func parseDrop(_ items: [String]) -> TeleportDrop
}

/// ``TeleportAppHost`` over the cua SDK: `Cua.teleport()` and a Space.
public final class SDKTeleportHost: TeleportAppHost, @unchecked Sendable {
    public let teleport: Teleport
    public let space: CuaSDK.Space
    private let roots: [String]?
    private let recentsPath: String?
    private let hint = HintCache()

    /// `roots` and `recentsPath` default to this machine's; tests pass
    /// fixture directories.
    public init(teleport: Teleport, space: CuaSDK.Space, roots: [String]? = nil, recentsPath: String? = nil) {
        self.teleport = teleport
        self.space = space
        self.roots = roots
        self.recentsPath = recentsPath
    }

    private func options() async throws -> TeleportCatalogOptions {
        let h = try await hint.get { try await self.teleport.spaceHint(space: self.space) }
        return TeleportCatalogOptions(roots: roots, spaceOs: h.spaceOs, spaceArch: h.spaceArch, recentsPath: recentsPath)
    }

    public func catalog() async throws -> [TeleportCatalogEntry] {
        try await teleport.catalog(options: options())
    }

    public func plan(_ entry: TeleportCatalogEntry, _ options: TeleportPlanOptions) async throws -> TeleportPlan {
        try await teleport.plan(app: entry, space: space, options: options)
    }

    public func run(_ plan: TeleportPlan, consent: TeleportConsent,
                    onEvent: @escaping @Sendable (TeleportRunEvent) -> Void) async throws -> TeleportRunReport {
        try await teleport.run(plan: plan, space: space, consent: consent, listener: RunRelay(onEvent))
    }

    public func icon(_ entry: TeleportCatalogEntry) -> Data? {
        entry.hostPath.flatMap { teleport.appIconPng(path: $0, size: 64) }
    }

    public func entry(forDroppedPath path: String) throws -> TeleportCatalogEntry {
        try teleport.catalogEntryForPath(path: path, options: nil)
    }

    public func parseDrop(_ items: [String]) -> TeleportDrop {
        teleport.parseDrop(items: items)
    }
}

private actor HintCache {
    private var value: TeleportCatalogOptions?
    func get(_ load: () async throws -> TeleportCatalogOptions) async throws -> TeleportCatalogOptions {
        if let value { return value }
        let v = try await load()
        value = v
        return v
    }
}

final class RunRelay: TeleportRunListener, @unchecked Sendable {
    let handler: @Sendable (TeleportRunEvent) -> Void
    init(_ handler: @escaping @Sendable (TeleportRunEvent) -> Void) { self.handler = handler }
    func onEvent(event: TeleportRunEvent) { handler(event) }
}

/// What a drop onto a Space means.
public enum TeleportDropOutcome: Equatable, Sendable {
    /// An app: open "Teleport an app…" at its options, with any dropped
    /// files preselected.
    case app(TeleportCatalogEntry, files: [String])
    /// Only files or folders: the existing file transfer handles them.
    case files([URL])
    /// Nothing teleport handles.
    case none
}

public enum TeleportDropHandler {
    /// Sorts dropped URLs (a Finder or Dock drag carries `file://` URLs of
    /// `.app` bundles) through the SDK's drop parser.
    public static func outcome(for urls: [URL], host: TeleportAppHost) -> TeleportDropOutcome {
        let items = urls.map { $0.isFileURL ? $0.path : $0.absoluteString }
        let drop = host.parseDrop(items)
        if let app = drop.apps.first, let entry = try? host.entry(forDroppedPath: app) {
            return .app(entry, files: drop.files)
        }
        let files = urls.filter { $0.isFileURL && !drop.apps.contains($0.path) }
        return files.isEmpty ? .none : .files(files)
    }
}
