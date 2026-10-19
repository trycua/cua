// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Cua
import CuaSpacesFFI
import Foundation

/// The "Teleport an app…" state machine, the same flow as the TypeScript
/// `@trycua/cua/teleport` controller: pick (search, recents, capability
/// sections), options (what moves, chosen files), consent (every install,
/// path and secret; secrets need an acknowledgement), run (progress), done
/// or error.
@MainActor
public final class TeleportPickerModel: ObservableObject {
    public enum Step: String, Sendable { case loading, pick, options, planning, consent, running, done, error }

    public struct Section: Equatable {
        public let title: String
        public let entries: [TeleportCatalogEntry]
    }

    public let host: TeleportAppHost
    public let spaceName: String

    @Published public private(set) var step: Step = .loading
    @Published public private(set) var entries: [TeleportCatalogEntry] = []
    @Published public var query: String = "" {
        didSet { keepSelectionVisible() }
    }
    @Published public var selectedId: String?
    @Published public private(set) var entry: TeleportCatalogEntry?
    @Published public private(set) var move: TeleportMove?
    @Published public private(set) var files: [String] = []
    @Published public private(set) var plan: TeleportPlan?
    @Published public var acknowledged = false
    @Published public private(set) var events: [TeleportRunEvent] = []
    @Published public private(set) var report: TeleportRunReport?
    @Published public private(set) var error: String?
    /// Set when the error is the Keyvault's "needs the Cua app" refusal:
    /// show this prompt (install button) instead of the raw error.
    @Published public private(set) var installPrompt: InstallCuaPrompt?
    /// The loaded catalog's icons (from the SDK's icon cache; rebuilt on
    /// every load, never kept across loads).
    @Published public private(set) var icons: [String: Data] = [:]
    private var errorBack: Step = .pick

    public init(host: TeleportAppHost, spaceName: String) {
        self.host = host
        self.spaceName = spaceName
    }

    // MARK: selectors

    public var visibleEntries: [TeleportCatalogEntry] {
        let words = query.lowercased().split(whereSeparator: \.isWhitespace).map(String.init)
        return entries.filter { e in
            let hay = "\(e.name) \(e.id) \(e.hostAppId ?? "")".lowercased()
            return words.allSatisfy { hay.contains($0) }
        }
    }

    /// Recents, then available apps, then unavailable ones (shown disabled).
    public var sections: [Section] {
        let visible = visibleEntries
        let recents = visible.filter { $0.lastUsedMs != nil && $0.capability != .unsupported }
        let rest = visible.filter { e in !recents.contains(where: { $0.id == e.id }) }
        return [
            Section(title: "Recent", entries: recents),
            Section(title: "Apps", entries: rest.filter { $0.capability != .unsupported }),
            Section(title: "Not available", entries: rest.filter { $0.capability == .unsupported }),
        ].filter { !$0.entries.isEmpty }
    }

    public var canPlan: Bool {
        guard let entry, let move, entry.moves.contains(move) else { return false }
        return move != .appWithFiles || !files.isEmpty
    }

    public var canConfirm: Bool {
        guard let plan else { return false }
        return !plan.sensitive || acknowledged
    }

    /// Overall run progress in 0...1.
    public var progress: Double {
        guard let last = events.last else { return 0 }
        if last.phase == "done" { return 1 }
        let within: Double = last.totalBytes > 0
            ? min(1, Double(last.doneBytes) / Double(last.totalBytes))
            : (last.phase == "finished" ? 1 : 0)
        return min(1, (Double(last.step) + within) / Double(max(1, last.steps)))
    }

    /// What the run is doing, in words, under the progress bar ("Reading
    /// Chrome cookies (macOS will ask for Keychain access)…", "Uploading
    /// 12 / 80 MB"): the app core's reading of the SDK's step events.
    public var status: String? {
        step == .running ? appTeleportRunStatus(events: events) : nil
    }

    /// The least that moves (files when some were dropped).
    public static func defaultMove(_ entry: TeleportCatalogEntry, files: [String] = []) -> TeleportMove? {
        if !files.isEmpty, entry.moves.contains(.appWithFiles) { return .appWithFiles }
        return entry.moves.first
    }

    // MARK: transitions

    /// Loads the catalog (and icons, lazily).
    public func load() async {
        do {
            let loaded = try await host.catalog()
            entries = loaded
            if step == .loading || step == .error {
                step = .pick
                error = nil
                installPrompt = nil
            }
            if selectedId == nil { selectedId = loaded.first { $0.capability != .unsupported }?.id }
            var next: [String: Data] = [:]
            for e in loaded {
                if let png = host.icon(e) { next[e.id] = png }
            }
            icons = next
        } catch {
            fail(error, back: .loading)
        }
    }

    public func select(_ id: String) {
        if step == .pick { selectedId = id }
    }

    /// Opens the options for the selected (or given) app; unsupported apps
    /// cannot be chosen.
    public func choose(_ id: String? = nil) {
        guard step == .pick, let e = entries.first(where: { $0.id == (id ?? selectedId) }),
              e.capability != .unsupported else { return }
        toOptions(e, files: [])
    }

    /// Jumps to one app (a drop or a window drag).
    public func preselect(_ e: TeleportCatalogEntry, files: [String] = []) {
        if e.capability == .unsupported {
            entry = e
            error = "\(e.name) cannot be teleported: \(e.reason ?? "unsupported")"
            installPrompt = nil
            errorBack = .pick
            step = .error
            return
        }
        toOptions(e, files: files)
    }

    private func toOptions(_ e: TeleportCatalogEntry, files: [String]) {
        entry = e
        selectedId = e.id
        move = Self.defaultMove(e, files: files)
        self.files = files
        plan = nil
        acknowledged = false
        error = nil
        installPrompt = nil
        step = .options
    }

    public func setMove(_ m: TeleportMove) {
        if step == .options, entry?.moves.contains(m) == true { move = m }
    }

    public func addFiles(_ paths: [String]) {
        guard step == .options else { return }
        for p in paths where !files.contains(p) { files.append(p) }
    }

    public func removeFile(_ path: String) {
        if step == .options { files.removeAll { $0 == path } }
    }

    /// Builds the plan (sizes, installs, consent items).
    public func makePlan() async {
        guard step == .options, canPlan, let entry, let move else { return }
        step = .planning
        do {
            let p = try await host.plan(entry, TeleportPlanOptions(moves: move, files: files,
                                                                     stateItems: nil, scope: nil, launch: nil))
            guard step == .planning else { return }
            plan = p
            acknowledged = false
            step = .consent
        } catch {
            fail(error, back: .options)
        }
    }

    /// Runs the plan with the user's consent.
    public func confirm() async {
        guard step == .consent, canConfirm, let plan else { return }
        step = .running
        events = []
        do {
            let r = try await host.run(plan, consent: TeleportConsent(approved: true, acknowledgeSensitive: acknowledged)) { e in
                Task { @MainActor [weak self] in self?.record(e) }
            }
            report = r
            step = .done
        } catch {
            fail(error, back: .consent)
        }
    }

    private func record(_ e: TeleportRunEvent) {
        guard step == .running else { return }
        events.append(e)
        if events.count > 200 { events.removeFirst(events.count - 200) }
    }

    public func back() {
        switch step {
        case .options where !entries.isEmpty:
            plan = nil
            step = .pick
        case .consent:
            plan = nil
            acknowledged = false
            step = .options
        case .error:
            error = nil
            installPrompt = nil
            step = errorBack
        default:
            break
        }
    }

    private func fail(_ e: Error, back: Step) {
        error = String(describing: e)
        installPrompt = InstallCuaPrompt.detect(e)
        errorBack = back
        step = .error
    }

    private func keepSelectionVisible() {
        let visible = visibleEntries
        if !visible.contains(where: { $0.id == selectedId }) {
            selectedId = visible.first { $0.capability != .unsupported }?.id
        }
    }

    // MARK: labels

    public static func label(_ c: TeleportCapability) -> String {
        switch c {
        case .full: return "App and signed-in state"
        case .installOnly: return "App, empty or with files"
        case .unsupported: return "Not available"
        }
    }

    public static func label(_ m: TeleportMove) -> String {
        switch m {
        case .appOnly: return "Just the app"
        case .appWithFiles: return "The app with files or folders"
        case .appWithState: return "The app with its signed-in state"
        }
    }

    public static func bytes(_ n: UInt64) -> String {
        ByteCountFormatter.string(fromByteCount: Int64(clamping: n), countStyle: .file)
    }
}
