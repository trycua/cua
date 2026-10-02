// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSDK
import CuaSpacesFFI
import Foundation
import Observation

/// Items waiting for the user's answer to the unlock prompt.
public struct PendingUnlock: Identifiable, Equatable {
    public let ids: [String]
    public let prompt: KvUnlockPrompt
    public var id: String { ids.joined(separator: ",") }
}

/// What the user answered.
public enum UnlockAnswer { case deny, allow, neverAskAgain }

/// Items waiting for the user's confirmation to delete.
public struct PendingDelete: Identifiable, Equatable {
    public let ids: [String]
    public let confirm: KvDeleteConfirm
    public var id: String { ids.joined(separator: ",") }
}

/// The Keyvault browser: the broker's overview (through the core's
/// `KeyvaultClient`) and the core's views of it. The app never reads secret
/// values (the broker has none to give), never runs its own Touch ID prompt
/// (the daemon asks for presence when access widens), and approves only the
/// items the user ticks.
@MainActor
@Observable
public final class KeyvaultModel {
    let client: KeyvaultClientProtocol?
    public private(set) var overview: KeyvaultOverview { didSet { changed() } }
    /// The sidebar's pick. An app narrows the vault list to it.
    public var selection: KvSelection = .category(category: .all) {
        didSet { changed(); applySelection() }
    }
    /// The vault list's own state (search, selection, open groups); the
    /// core's reducer owns it.
    public private(set) var vault = KvVaultState(query: "", selected: [], expanded: [], app: nil) {
        didSet { changed() }
    }
    /// Items to unlock once the user answers the prompt (nil: no prompt).
    public var unlockPrompt: PendingUnlock?
    /// Items to delete once the user confirms (nil: nothing to confirm).
    public var deleteConfirm: PendingDelete?
    /// App icons: NSWorkspace once per app and version, then memory and disk
    /// (`AppIconCache`). Nil in tests that do not draw icons.
    public var appIcons: AppIconCache?
    public private(set) var icons: [String: NSImage] = [:]
    @ObservationIgnored private var iconsAsked: Set<String> = []
    /// Site icons: the browser's own, read locally and kept in the vault,
    /// then Google's service when the setting allows it, then the globe.
    public var siteIconStore: SiteIconStore?
    /// The "Load site icons from Google" setting.
    public var siteIconsFromGoogle: @MainActor () -> Bool = { true }
    public private(set) var siteIcons: [String: NSImage] = [:]
    @ObservationIgnored private var localSiteIcons: [String: Int] = [:]
    @ObservationIgnored private var siteIconsAsked: Set<String> = []
    /// The backing scale the icons are fetched for.
    public var iconScale = 2

    /// Bumped when the overview, the list state or the sidebar pick change:
    /// the memoized views below recompute only then. Reading it in a view's
    /// getter keeps the view observing even when the memo answers.
    private(set) var revision = 0
    @ObservationIgnored private var memo: [String: (rev: Int, value: Any)] = [:]
    /// How many times each memoized view was computed (the tests' proof).
    @ObservationIgnored public private(set) var computes: [String: Int] = [:]
    private func changed() { revision &+= 1 }
    private func memoized<T>(_ name: String, _ make: () -> T) -> T {
        let rev = revision
        if let hit = memo[name], hit.rev == rev, let v = hit.value as? T { return v }
        let v = make()
        memo[name] = (rev, v)
        computes[name, default: 0] += 1
        return v
    }
    public var approval: KvApprovalState?
    public private(set) var busy = false
    public var error: String?
    public var recoveryKey: String?
    /// The setup and unlock form's secure fields. Held only while the user
    /// types; cleared when sent, never logged or stored. The passphrase goes
    /// only to the broker over the verified Keyvault socket.
    public var passphrase = ""
    public var passphraseConfirm = ""
    let clock: () -> Date

    public init(client: KeyvaultClientProtocol?, overview: KeyvaultOverview? = nil,
                clock: @escaping () -> Date = Date.init) {
        self.client = client
        self.clock = clock
        self.overview = overview ?? KeyvaultOverview(
            availability: "not_running", message: "The Cua daemon is not running, so the Keyvault is unavailable.",
            status: nil, serverVerified: false, items: [], namesVisible: false, itemsTotal: 0,
            pending: [], grants: [], rules: [],
            deliveries: [], audit: [], auditVerification: nil, partialErrors: [])
    }

    var nowMs: Int64 { Int64(clock().timeIntervalSince1970 * 1000) }

    public var page: KvPage { memoized("page") { kvPage(overview: overview, nowMs: nowMs) } }

    /// The always-visible signal while sign-ins are live in a Space (nil
    /// when nothing is): the notch indicator and the menu bar line.
    public var sharingLabel: String? { kvSharingLabel(overview: overview, nowMs: nowMs) }

    /// Told the sharing label after every refresh (and after a dismissal).
    public var onSharing: (@MainActor (String?) -> Void)?

    /// Copies (import ids) the user dismissed from the notch. Dismiss hides
    /// the indicator and the tiles' key; it revokes and wipes nothing.
    public var dismissed: [String] = []
    /// Told the dismissed copies when they change (the app saves them).
    public var onDismissed: (@MainActor ([String]) -> Void)?
    /// The Access row to bring forward (a Space's "Signed in" badge).
    public var focusKey: String?

    /// The notch's indicator: the sharing label without dismissed copies.
    public var notchLabel: String? {
        kvVisibleSharingLabel(overview: overview, nowMs: nowMs, dismissed: dismissed)
    }

    /// The ids of `spaces` signed in through the Keyvault: all of them
    /// (`notch: false`, the Spaces list), or less the dismissed copies.
    public func signedIn(_ spaces: [AppSpace], notch: Bool = false) -> [String] {
        kvSignedInSpaces(overview: overview, nowMs: nowMs, dismissed: notch ? dismissed : [], spaces: spaces)
    }

    /// The Access row of `space`'s copies.
    public func accessKey(for space: AppSpace) -> String? {
        kvSpaceAccessKey(overview: overview, nowMs: nowMs, space: space)
    }

    /// Hides `imports` (or every live copy) from the notch.
    public func dismiss(_ imports: [String]? = nil) {
        let ids = imports ?? overview.deliveries.map(\.importId)
        let next = dismissed + ids.filter { !dismissed.contains($0) }
        guard next != dismissed else { return }
        dismissed = next
        onDismissed?(dismissed)
        onSharing?(sharingLabel)
    }

    /// Whether every copy of an Access row is dismissed.
    public func isDismissed(_ row: KvAccessRow) -> Bool {
        !row.imports.isEmpty && row.imports.allSatisfy(dismissed.contains)
    }
    public var sidebar: KvSidebar { memoized("sidebar") { kvSidebar(overview: overview, nowMs: nowMs) } }
    public var list: KvListView { memoized("list") { kvList(overview: overview, selection: selection, nowMs: nowMs) } }

    // MARK: - The vault list (the core's grouping, selection and search)

    /// The vault list as drawn: apps, sites and items with their locks.
    public var vaultView: KvVaultView { memoized("vaultView") { kvVaultView(overview: overview, state: vault, nowMs: nowMs) } }

    /// The search text.
    public var query: String {
        get { vault.query }
        set { send(.query(text: newValue)) }
    }

    /// Feeds one input to the list (search, select, open or close).
    public func send(_ action: KvVaultAction) {
        vault = kvVaultReduce(overview: overview, state: vault, action: action)
    }

    private func applySelection() {
        var next = vault
        switch selection {
        case .app(let key): next.app = key
        case .category: next.app = nil
        }
        if next.app != vault.app { next.selected = [] }
        vault = kvVaultReduce(overview: overview, state: next, action: .query(text: next.query))
    }

    /// An app the list has not shown before opens, so its sites are in view;
    /// one the user closed stays closed.
    @ObservationIgnored private var seenApps: Set<String> = []

    private func openNewApps() {
        for id in Set(overview.items.map(\.providerId)).subtracting(seenApps).sorted() {
            seenApps.insert(id)
            if !vault.expanded.contains(id) { send(.toggleOpen(key: id)) }
        }
    }

    /// Shows the items' names (Touch ID, asked by the daemon).
    public func showItems() async { await run(.browse) }

    // MARK: - Locks (one prompt and one Touch ID for a batch)

    /// A click on a lock, or the batch bar's Unlock: asks first (unless the
    /// user chose Never ask again), then the daemon asks for Touch ID once.
    public func requestUnlock(ids: [String], name: String? = nil) async {
        guard !ids.isEmpty else { return }
        if let prompt = kvUnlockPrompt(overview: overview, count: UInt32(ids.count), name: name) {
            unlockPrompt = PendingUnlock(ids: ids, prompt: prompt)
        } else {
            await run(kvUnlockCommand(ids: ids))
        }
    }

    /// The prompt's answers: Allow, Never ask again (stored in the vault,
    /// revertible in Settings), Deny.
    public func answerUnlock(_ answer: UnlockAnswer) async {
        guard let pending = unlockPrompt else { return }
        unlockPrompt = nil
        switch answer {
        case .deny: return
        case .allow:
            await run(kvUnlockCommand(ids: pending.ids))
        case .neverAskAgain:
            await run(.setSkipUnlockPrompt(on: true))
            await run(kvUnlockCommand(ids: pending.ids))
        }
    }

    /// Locks items (narrowing: no prompt, no Touch ID).
    public func lock(ids: [String]) async {
        guard !ids.isEmpty else { return }
        await run(kvLockCommand(ids: ids))
    }

    /// The batch bar's and a row's delete: confirm first.
    public func requestDelete(ids: [String]) {
        guard !ids.isEmpty else { return }
        let copies = kvLiveCopySpaces(overview: overview, ids: ids, nowMs: nowMs)
        deleteConfirm = PendingDelete(ids: ids, confirm: kvDeleteConfirm(count: UInt32(ids.count), liveCopies: copies))
    }

    /// Deletes the items and wipes their live copies in Spaces.
    public func confirmDelete() async {
        guard let pending = deleteConfirm else { return }
        deleteConfirm = nil
        await run(kvDeleteCommand(ids: pending.ids))
    }

    /// "Never ask again" is on (Settings turns it back off).
    public var unlockPromptShows: Bool? { overview.status?.skipUnlockPrompt.map { !$0 } }

    public func setSkipUnlockPrompt(_ on: Bool) async { await run(.setSkipUnlockPrompt(on: on)) }

    // MARK: - App icons

    /// Icons for what the list shows, each asked for once: app icons from
    /// the cache, the browser's own site icons from the vault, and site icons
    /// already on disk. Nothing here touches the network (`siteIcon` does,
    /// lazily, as a row appears).
    public func loadIcons() async {
        if let cache = appIcons {
            var seen: [String: String] = [:]
            for i in overview.items { seen[i.providerId] = i.appDisplay }
            for (id, name) in seen.sorted(by: { $0.key < $1.key }) where !iconsAsked.contains(id) {
                iconsAsked.insert(id)
                if let image = cache.image(providerId: id, name: name) { icons[id] = image }
            }
        }
        guard overview.namesVisible, let client else { return }
        for f in await client.favicons() {
            // Decoded once: a changed icon (a different size) decodes again.
            guard localSiteIcons[f.site] != f.png.count, let data = Data(base64Encoded: f.png),
                  let image = NSImage(data: data) else { continue }
            localSiteIcons[f.site] = f.png.count
            siteIcons[f.site] = image
        }
    }

    /// Called as a site row first appears: the downloaded icon on hand, else
    /// (when the setting allows it and the browser had none) one lookup of
    /// the domain. Never blocks drawing; the globe stands in meanwhile.
    public func siteIcon(_ site: String) async {
        guard siteIcons[site] == nil, let store = siteIconStore else { return }
        if let hit = store.cached(site) { siteIcons[site] = hit; return }
        guard siteIconsFromGoogle(), !siteIconsAsked.contains(site) else { return }
        siteIconsAsked.insert(site)
        if let image = await store.load(site, scale: iconScale) { siteIcons[site] = image }
    }

    public var approvalView: KvApprovalView? {
        approval.map { kvApprovalView(overview: overview, state: $0) }
    }

    /// Re-reads the broker (never throws: unavailability is a page state).
    public func refresh() async {
        guard let client else { return }
        overview = await client.overview()
        vault = kvVaultPrune(overview: overview, state: vault)
        openNewApps()
        // Forget dismissals of copies that were wiped or expired (only
        // when the broker answered: an unavailable page lists none).
        if overview.availability == "ready" {
            let live = kvPruneDismissed(overview: overview, nowMs: nowMs, dismissed: dismissed)
            if live != dismissed {
                dismissed = live
                onDismissed?(live)
            }
        }
        onSharing?(sharingLabel)
        // The recovery key is shown once: never again after the vault locks.
        if overview.availability == "locked" { recoveryKey = nil }
        if let a = approval, !overview.pending.contains(where: { $0.id == a.requestId }) {
            approval = nil
        }
    }

    // MARK: - Approval sheet (nothing selected by default)

    public func openApproval(_ requestId: String) {
        approval = kvApprovalOpen(requestId: requestId)
    }

    public func sendApproval(_ action: KvApprovalAction) {
        guard let a = approval else { return }
        approval = kvApprovalReduce(overview: overview, state: a, action: action)
    }

    public func approve() async {
        guard let a = approval,
              let command = kvApprovalApproveCommand(overview: overview, state: a) else { return }
        approval = nil
        await run(command)
    }

    public func deny(_ requestId: String) async {
        approval = nil
        await run(kvApprovalDenyCommand(state: kvApprovalOpen(requestId: requestId)))
    }

    // MARK: - Setup and unlock (Touch ID or a passphrase, as the daemon allows)

    /// The passphrase fields' hint and whether the form can be sent (nil
    /// when the form uses Touch ID).
    public var passphraseCheck: KvPassphraseCheck? {
        guard let form = page.form, form.method == .passphrase else { return nil }
        return kvPassphraseCheck(mode: form.mode, passphrase: passphrase, confirm: passphraseConfirm)
    }

    public var canSubmitCredential: Bool {
        guard let form = page.form, !busy else { return false }
        return form.method == .touchId || passphraseCheck?.canSubmit == true
    }

    /// Sets up or unlocks the vault the way the form offers.
    public func submitCredential() async {
        guard let client, let form = page.form, canSubmitCredential else { return }
        let secret = passphrase
        passphrase = ""
        passphraseConfirm = ""
        busy = true
        error = nil
        do {
            switch (form.mode, form.method) {
            case (.setup, .touchId):
                if case .recoveryKey(let key) = try await client.execute(command: .setup) { recoveryKey = key }
            case (.setup, .passphrase):
                recoveryKey = try await client.setupWithPassphrase(passphrase: secret)
            case (.unlock, .touchId):
                _ = try await client.execute(command: .unlock)
            case (.unlock, .passphrase):
                try await client.unlockWithPassphrase(passphrase: secret)
            }
        } catch {
            self.error = Self.words(error)
        }
        busy = false
        await refresh()
    }

    /// The broker's sentence, without the SDK error kind in front
    /// ("invalid argument: ...").
    static func words(_ error: Error) -> String {
        let text = LiveSpacesBackend.words(error)
        guard error is CuaError, let colon = text.range(of: ": "),
              text[..<colon.lowerBound].allSatisfy({ $0.isLowercase || $0 == " " }) else { return text }
        return String(text[colon.upperBound...])
    }

    // MARK: - Actions (each one broker request)

    public func setDisabled(_ disabled: Bool) async { await run(.setDisabled(disabled: disabled)) }

    /// Auto-wipe of access given to Spaces (off by default; turning it off
    /// makes the daemon ask for Touch ID).
    public var autoWipe: Bool? { overview.status?.autoWipe }

    public func setAutoWipe(_ on: Bool) async { await run(.setAutoWipe(on: on)) }

    public func run(_ command: KvCommand) async {
        guard let client else { return }
        busy = true
        error = nil
        do {
            let outcome = try await client.execute(command: command)
            if case .recoveryKey(let key) = outcome { recoveryKey = key }
        } catch {
            self.error = Self.words(error)
        }
        busy = false
        await refresh()
    }
}
