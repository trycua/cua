// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import ImageIO
import CuaSDK
import CuaSpacesFFI
import Foundation
import Observation

/// Where the picker's grid gets its windows, icons and previews: the SDK
/// (its icon and preview caches) in the app, fixtures in tests.
public struct TeleportPickerSources: Sendable {
    /// This machine's windows, front to back.
    public var openWindows: @Sendable () async -> [AppOpenWindow]
    /// The Space's windows.
    public var remoteWindows: @Sendable () async -> [AppRemoteWindow]
    /// This machine's app icon (`Teleport.appIconPng`, SDK-cached).
    public var hostIcon: @Sendable (_ path: String) async -> Data?
    /// The Space's app icons in one call (`Space.appIcons`, SDK-cached).
    public var guestIcons: @Sendable (_ requests: [SpaceAppIconRequest]) async -> [Data?]
    /// A live preview of this machine's window (the window-drag source).
    public var hostThumbnail: @Sendable (_ windowId: UInt32) async -> Data?
    /// A live preview of the Space's window.
    public var guestThumbnail: @Sendable (_ windowId: String, _ epoch: UInt64) async -> Data?
    /// The catalog entry of the app a window belongs to (its bundle).
    public var entryForPath: @Sendable (_ path: String) async -> TeleportCatalogEntry?

    public init(openWindows: @escaping @Sendable () async -> [AppOpenWindow] = { [] },
                remoteWindows: @escaping @Sendable () async -> [AppRemoteWindow] = { [] },
                hostIcon: @escaping @Sendable (String) async -> Data? = { _ in nil },
                guestIcons: @escaping @Sendable ([SpaceAppIconRequest]) async -> [Data?] = { $0.map { _ in nil } },
                hostThumbnail: @escaping @Sendable (UInt32) async -> Data? = { _ in nil },
                guestThumbnail: @escaping @Sendable (String, UInt64) async -> Data? = { _, _ in nil },
                entryForPath: @escaping @Sendable (String) async -> TeleportCatalogEntry? = { _ in nil }) {
        self.openWindows = openWindows
        self.remoteWindows = remoteWindows
        self.hostIcon = hostIcon
        self.guestIcons = guestIcons
        self.hostThumbnail = hostThumbnail
        self.guestThumbnail = guestThumbnail
        self.entryForPath = entryForPath
    }

    /// The SDK's: this Mac's windows and icons, the Space's windows, icons
    /// and previews, all through the SDK's caches.
    public static func live(teleport: CuaSpacesFFI.Teleport?, space: CuaSDK.Space?) -> TeleportPickerSources {
        TeleportPickerSources(
            openWindows: {
                ((try? teleport?.listWindows()) ?? []).map {
                    AppOpenWindow(windowId: $0.windowId, appId: "", appName: $0.appName, windowTitle: $0.title,
                                  supported: true, bundlePath: $0.bundlePath)
                }
            },
            remoteWindows: {
                ((try? await space?.windows(app: nil)) ?? []).map {
                    AppRemoteWindow(id: $0.windowId, appName: $0.appName, title: $0.title, visible: $0.onScreen,
                                    appId: $0.appId, targetEpoch: $0.epoch,
                                    widthPx: $0.bounds.count == 4 ? UInt32(max(0, $0.bounds[2])) : nil,
                                    heightPx: $0.bounds.count == 4 ? UInt32(max(0, $0.bounds[3])) : nil,
                                    pid: $0.pid > 0 ? $0.pid : nil)
                }
            },
            hostIcon: { path in teleport?.appIconPng(path: path, size: 64) },
            guestIcons: { requests in
                guard let icons = try? await space?.appIcons(requests: requests) else { return requests.map { _ in nil } }
                return icons.map { $0?.bytes }
            },
            hostThumbnail: { id in (try? teleport?.captureWindowThumbnail(windowId: id, maxWidth: 480)) ?? nil },
            guestThumbnail: { id, epoch in (try? await space?.windowThumbnail(windowId: id, epoch: epoch, maxDimension: 480)) ?? nil },
            entryForPath: { path in try? teleport?.catalogEntryForPath(path: path, options: nil) })
    }
}

/// "Teleport an app…" for one Space: the core's picker state machine over
/// the SDK's catalog, plan and run. The review gate (secrets need an
/// explicit acknowledgement) is the core's; the SDK re-checks the consent.
@MainActor
@Observable
public final class TeleportModel {
    public private(set) var state: AppPickerState
    let teleport: CuaSpacesFFI.Teleport?
    let space: CuaSDK.Space?
    private var sdkEntries: [String: TeleportCatalogEntry] = [:]
    private var sdkPlan: TeleportPlan?
    /// A run started (true) or ended (false): the notch shows a transfer.
    public var onTransfer: ((Bool) -> Void)?
    /// Streams one of the Space's windows here (From <Space>).
    public var onStreamWindow: ((String) -> Void)?

    // The grid (the core's `teleport::grid`): tabs, search and selection of
    // the window tabs, and the icons and previews of the tiles shown.
    let sources: TeleportPickerSources
    public var tab: AppPickerGridTab = .apps
    public private(set) var windowQuery = ""
    public private(set) var windowSelected: String?
    public private(set) var remoteSelected: String?
    public private(set) var openWindows: [AppOpenWindow]?
    public private(set) var remoteWindows: [AppRemoteWindow]?
    public private(set) var thumbnails: [String: NSImage] = [:]
    public private(set) var tileIcons: [String: NSImage] = [:]
    @ObservationIgnored private var iconsAsked: Set<String> = []
    @ObservationIgnored private var remoteLoad: Task<[AppRemoteWindow], Never>?

    public init(spaceName: String, teleport: CuaSpacesFFI.Teleport?, space: CuaSDK.Space?,
                sources: TeleportPickerSources? = nil) {
        self.state = appPickerInitial(spaceName: spaceName)
        self.teleport = teleport
        self.space = space
        self.sources = sources ?? .live(teleport: teleport, space: space)
    }

    public var tabs: [AppPickerGridTabItem] { appPickerGridTabs(spaceName: state.spaceName) }

    /// The active tab's tiles.
    public var grid: AppPickerGrid {
        switch tab {
        case .apps: return appPickerAppGrid(state: state, windows: openWindows ?? [])
        case .windows: return appPickerWindowGrid(windows: openWindows ?? [], query: windowQuery, selected: windowSelected)
        case .space: return appPickerRemoteGrid(windows: remoteWindows ?? [], query: windowQuery, selected: remoteSelected)
        }
    }

    /// The active tab's primary button (the core's).
    public var primary: AppPickerGridPrimary {
        appPickerGridPrimary(tab: tab, spaceName: state.spaceName, grid: grid)
    }

    /// The search text of the active tab.
    public var query: String { tab == .apps ? state.query : windowQuery }

    public func setQuery(_ text: String) {
        if tab == .apps { send(.query(query: text)) } else { windowQuery = text }
    }

    public func select(_ id: String) {
        switch tab {
        case .apps: send(.select(id: id))
        case .windows: windowSelected = id
        case .space: remoteSelected = id
        }
    }

    public var selectedTile: AppPickerTile? {
        grid.sections.flatMap(\.tiles).first { $0.selected }
    }

    /// Arrow keys (`columns` tiles per row for up and down).
    public func step(_ delta: Int32) {
        let current: String? = switch tab {
        case .apps: state.selectedId
        case .windows: windowSelected
        case .space: remoteSelected
        }
        if let next = appPickerGridStep(grid: grid, selected: current, delta: delta) { select(next) }
    }

    /// Opens the tile: the app's options (Apps, Open windows) or the
    /// Space's window streamed here (From <Space>).
    public func activate(_ tile: AppPickerTile) async {
        guard !tile.disabled else { return }
        select(tile.id)
        switch tab {
        case .apps:
            send(.choose(id: tile.id))
        case .windows:
            guard let path = openWindows?.first(where: { String($0.windowId) == tile.id })?.bundlePath,
                  let entry = await sources.entryForPath(path) else { return }
            tab = .apps
            preselect(entry)
        case .space:
            onStreamWindow?(tile.id)
        }
    }

    /// Reads the windows each tab lists: this machine's once, and the
    /// Space's in the background as soon as the picker opens (so its tab is
    /// ready when chosen), awaited when that tab shows.
    public func loadWindows() async {
        if remoteWindows == nil, remoteLoad == nil {
            let sources = sources
            remoteLoad = Task.detached { await sources.remoteWindows() }
        }
        if openWindows == nil {
            let sources = sources
            openWindows = await Task.detached { await sources.openWindows() }.value
        }
        if tab == .space, remoteWindows == nil, let remoteLoad {
            remoteWindows = await remoteLoad.value
        }
        await loadIcons()
    }

    /// Host icons asked at once (each is one SDK call, cached there).
    static let iconConcurrency = 8

    /// The shown tiles' icons, in tile order (the visible ones first): this
    /// machine's per app, a few at a time, the Space's in one call (both from
    /// the SDK's icon cache). Decoded off the main thread and applied in
    /// batches, so the grid redraws a few times rather than once per icon.
    public func loadIcons() async {
        let tiles = grid.sections.flatMap(\.tiles)
        var hosts: [(String, String)] = []
        var guests: [(String, SpaceAppIconRequest)] = []
        for t in tiles {
            let key = Self.iconKey(t.icon)
            guard !key.isEmpty, tileIcons[key] == nil, !iconsAsked.contains(key) else { continue }
            iconsAsked.insert(key)
            switch t.icon {
            case let .host(path):
                hosts.append((key, path))
            case let .guest(appName, appId, pid):
                guests.append((key, SpaceAppIconRequest(appName: appName, appId: appId, pid: pid)))
            default:
                break
            }
        }
        let sources = sources
        let asked = guests
        async let guestIcons = Self.guestIcons(sources, asked)
        var pending: [(String, NSImage)] = []
        var last = ContinuousClock.now
        await withTaskGroup(of: (String, NSImage?).self) { group in
            var next = 0
            func add() {
                guard next < hosts.count else { return }
                let (key, path) = hosts[next]
                next += 1
                group.addTask { (key, await sources.hostIcon(path).flatMap(Self.decode)) }
            }
            for _ in 0..<Self.iconConcurrency { add() }
            while let (key, image) = await group.next() {
                add()
                if let image { pending.append((key, image)) }
                if ContinuousClock.now - last > .milliseconds(30) {
                    apply(&pending)
                    last = .now
                }
            }
        }
        apply(&pending)
        var guest = await guestIcons
        apply(&guest)
        // Asked again next time when nothing came (the SDK remembers misses).
        for key in hosts.map(\.0) + guests.map(\.0) where tileIcons[key] == nil {
            iconsAsked.remove(key)
        }
    }

    /// The Space's icons in one SDK call, decoded off the main thread.
    nonisolated static func guestIcons(_ sources: TeleportPickerSources,
                                       _ guests: [(String, SpaceAppIconRequest)]) async -> [(String, NSImage)] {
        guard !guests.isEmpty else { return [] }
        let data = await sources.guestIcons(guests.map(\.1))
        return zip(guests, data).compactMap { g, d in d.flatMap(decode).map { (g.0, $0) } }
    }

    private func apply(_ icons: inout [(String, NSImage)]) {
        guard !icons.isEmpty else { return }
        var next = tileIcons
        for (key, image) in icons { next[key] = image }
        tileIcons = next
        icons.removeAll()
    }

    /// One tile's live preview, when it scrolls into view (captured and
    /// decoded off the main thread).
    public func loadThumbnail(_ tile: AppPickerTile) async {
        let key = Self.thumbnailKey(tile.thumbnail)
        guard !key.isEmpty, thumbnails[key] == nil else { return }
        let sources = sources
        let thumbnail = tile.thumbnail
        let image = await Task.detached { () -> NSImage? in
            let data: Data? = switch thumbnail {
            case let .hostWindow(windowId): await sources.hostThumbnail(windowId)
            case let .guestWindow(windowId, epoch): await sources.guestThumbnail(windowId, epoch)
            default: nil
            }
            return data.flatMap(Self.decode)
        }.value
        if let image { thumbnails[key] = image }
    }

    /// PNG or JPEG bytes as an image decoded now (not lazily at first draw
    /// on the main thread).
    nonisolated static func decode(_ data: Data) -> NSImage? {
        guard let source = CGImageSourceCreateWithData(data as CFData, nil),
              let cg = CGImageSourceCreateImageAtIndex(
                  source, 0, [kCGImageSourceShouldCacheImmediately: true] as CFDictionary)
        else { return nil }
        return NSImage(cgImage: cg, size: NSSize(width: cg.width, height: cg.height))
    }

    public func icon(for tile: AppPickerTile) -> NSImage? { tileIcons[Self.iconKey(tile.icon)] }
    public func thumbnail(for tile: AppPickerTile) -> NSImage? { thumbnails[Self.thumbnailKey(tile.thumbnail)] }

    static func iconKey(_ icon: AppPickerTileIcon) -> String {
        switch icon {
        case let .host(path): return "host\u{1f}\(path)"
        case let .guest(appName, appId, _): return "guest\u{1f}\(appName.lowercased())\u{1f}\(appId.lowercased())"
        default: return ""
        }
    }

    static func thumbnailKey(_ t: AppPickerTileThumbnail) -> String {
        switch t {
        case let .hostWindow(windowId): return "host:\(windowId)"
        case let .guestWindow(windowId, epoch): return "guest:\(windowId)@\(epoch)"
        default: return ""
        }
    }

    public var sections: [AppEntrySection] { appPickerSections(state: state) }
    public var review: AppReviewView? { appPickerReview(state: state) }
    public var canPlan: Bool { appPickerCanPlan(state: state) }
    public var progress: Double { appPickerProgress(state: state) }
    /// What the run is doing, in words, under the bar (the core's reading
    /// of the SDK's step events): "Reading Chrome cookies (macOS will ask
    /// for Keychain access)…", "Uploading 12 / 80 MB".
    public var status: String? { appPickerStatus(state: state) }

    public func send(_ event: AppPickerEvent) {
        state = appPickerReduce(state: state, event: event)
    }

    public func load() async {
        guard let teleport, let space else {
            send(.failed(message: "Teleport needs a reachable Space.", causeTexts: [], causeInstalled: false))
            return
        }
        do {
            let hint = try await teleport.spaceHint(space: space)
            let entries = try await teleport.catalog(options: hint)
            sdkEntries = Dictionary(entries.map { ($0.id, $0) }, uniquingKeysWith: { a, _ in a })
            send(.loaded(entries: entries.map(appCatalogEntry(entry:))))
        } catch {
            fail(error)
        }
    }

    /// Opens the options for an app dropped on the notch or a Space.
    public func preselect(_ entry: TeleportCatalogEntry, files: [String] = []) {
        sdkEntries[entry.id] = entry
        send(.preselect(entry: appCatalogEntry(entry: entry), files: files))
    }

    public func plan() async {
        guard canPlan, let teleport, let space, let entry = state.entry,
              let sdk = sdkEntries[entry.id], let move = state.moves else { return }
        send(.plan)
        do {
            let plan = try await teleport.plan(app: sdk, space: space, options: TeleportPlanOptions(
                moves: move.sdk, files: state.files, stateItems: nil,
                sensitiveGroups: appPickerPlanSensitive(state: state).map(appSensitiveGroupSdk(group:)),
                scope: nil, launch: nil))
            sdkPlan = plan
            send(.planned(plan: appTeleportPlan(plan: plan)))
            await loadChoices()
        } catch {
            fail(error)
        }
    }

    // MARK: - The review's choices: sites, items, the Keyvault as the source

    /// The Keyvault the review reads saved items and site counts from (the
    /// app's; the opener sets it).
    public var keyvault: KeyvaultModel?
    /// What was sent to each Space last time (the app's settings).
    public var rememberedChoices: [AppRememberedChoice] = []
    /// Told the choices to keep after a teleport.
    public var onRemember: (([AppRememberedChoice]) -> Void)?
    /// The saved-items list's own state while sending from the Keyvault.
    public private(set) var reviewVault = KvVaultState(query: "", selected: [], expanded: [], app: nil)
    /// The sites are being read (Touch ID may be asked).
    public private(set) var readingSites = false

    var providerId: String? { state.entry?.providerId }

    /// The key a choice is remembered under: the app and this Space.
    var rememberKey: String? {
        providerId.map { appReviewRememberKey(app: $0, space: state.spaceName) }
    }

    /// Reads what the review lets the user choose: the app's saved Keyvault
    /// items (counts only, no Touch ID) and, for a browser that sends its
    /// cookies, its sites with counts (nothing decrypted; the daemon asks for
    /// Touch ID to show names).
    public func loadChoices() async {
        guard let provider = providerId, let review else { return }
        if let keyvault {
            await keyvault.refresh()
            let source = kvVaultSource(overview: keyvault.overview, providerId: provider)
            reviewVault = KvVaultState(query: "", selected: source.ids, expanded: [provider], app: provider)
            send(.vaultItems(count: source.count, newestMs: source.newestMs,
                             nowMs: Int64(keyvault.clock().timeIntervalSince1970 * 1000), selected: source.ids,
                             passwordIds: source.passwordIds))
        }
        guard review.needsDomains else { return }
        guard let client = keyvault?.client else { send(.domainsFailed); return }
        readingSites = true
        defer { readingSites = false }
        do {
            let inventory = try await client.inventory(app: provider, profile: nil)
            let remembered = rememberKey.flatMap { appReviewRemembered(choices: rememberedChoices, key: $0) }
            send(.domainsLoaded(inventory: inventory, remembered: remembered))
        } catch {
            send(.domainsFailed)
        }
    }

    /// Where the review sends from. The saved items' names need Touch ID the
    /// first time (the Keyvault's browse window).
    public func sendFrom(_ source: AppSendSource) async {
        send(.sendFrom(source: source))
        if source == .vault, let keyvault, !keyvault.overview.namesVisible { await keyvault.showItems() }
    }

    /// The Keyvault as the review lists it: what a teleport can send. Saved
    /// passwords are never delivered (they sign in through site login), so
    /// they are not offered here.
    var reviewOverview: KeyvaultOverview? {
        guard var o = keyvault?.overview else { return nil }
        o.items.removeAll { $0.kind == "password" }
        return o
    }

    /// Next time the review starts from the sites sent now (to this Space).
    func rememberChoice() {
        let consent = appPickerConsent(state: state)
        guard let domains = consent.cookieDomains, consent.fromVault == nil, let key = rememberKey else { return }
        rememberedChoices = appReviewRemember(choices: rememberedChoices, key: key, domains: domains)
        onRemember?(rememberedChoices)
    }

    /// The saved items as the review lists them.
    public var reviewVaultView: KvVaultView? {
        guard let keyvault, let overview = reviewOverview else { return nil }
        return kvVaultView(overview: overview, state: reviewVault,
                           nowMs: Int64(keyvault.clock().timeIntervalSince1970 * 1000))
    }

    /// Search, select and open groups in the saved items; the core keeps the
    /// ones to send.
    public func sendVault(_ action: KvVaultAction) {
        guard let overview = reviewOverview else { return }
        reviewVault = kvVaultReduce(overview: overview, state: reviewVault, action: action)
        send(.vaultSelection(selected: reviewVault.selected))
    }

    public func confirm() async {
        guard let review, review.canConfirm, let teleport, let space, let plan = sdkPlan else { return }
        let consent = appPickerConsent(state: state)
        rememberChoice()
        send(.confirm)
        onTransfer?(true)
        defer { onTransfer?(false) }
        do {
            let report = try await teleport.run(
                plan: plan, space: space,
                consent: sdkConsent(consent),
                listener: RunEvents { [weak self] event in
                    Task { @MainActor in self?.send(.progress(event: appTeleportRunEvent(event: event))) }
                })
            send(.finished(report: AppTeleportRunReport(
                appId: report.appId, installed: report.installed, sent: report.sent,
                imported: report.imported, skipped: report.skipped, launched: report.launched)))
        } catch {
            fail(error)
        }
    }

    private func fail(_ error: Error) {
        let text = LiveSpacesBackend.words(error)
        send(.failed(message: text, causeTexts: [text], causeInstalled: false))
    }
}

/// The SDK's run events, on its worker thread, handed to `handler`.
final class RunEvents: TeleportRunListener, @unchecked Sendable {
    let handler: @Sendable (TeleportRunEvent) -> Void
    init(_ handler: @escaping @Sendable (TeleportRunEvent) -> Void) { self.handler = handler }
    func onEvent(event: TeleportRunEvent) { handler(event) }
}

extension AppTeleportMove {
    var sdk: TeleportMove {
        switch self {
        case .appOnly: return .appOnly
        case .appWithFiles: return .appWithFiles
        case .appWithState: return .appWithState
        }
    }

    public var label: String {
        switch self {
        case .appOnly: return "Just the app"
        case .appWithFiles: return "The app with files or folders"
        case .appWithState: return "The app with its signed-in state"
        }
    }
}

/// The SDK consent for the review's confirmed consent. Every field must be
/// carried over: `saveToKeyvault` defaults to false on the SDK type, so
/// leaving it out silently drops the "Save to Keyvault" checkbox.
func sdkConsent(_ consent: AppTeleportConsent) -> TeleportConsent {
    TeleportConsent(approved: consent.approved,
                    acknowledgeSensitive: consent.acknowledgeSensitive,
                    saveToKeyvault: consent.saveToKeyvault,
                    acknowledgeRelayPlaintext: consent.acknowledgeRelayPlaintext,
                    cookieDomains: consent.cookieDomains,
                    exclude: consent.exclude,
                    fromVault: consent.fromVault,
                    includePasswords: consent.includePasswords)
}
