// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import CuaSpacesFFI
import Foundation
import Observation

/// The Keyvault browser: the broker's overview (through the core's
/// `KeyvaultClient`) and the core's views of it. The app never reads secret
/// values (the broker has none to give), never runs its own Touch ID prompt
/// (the daemon asks for presence when access widens), and approves only the
/// items the user ticks.
@MainActor
@Observable
public final class KeyvaultModel {
    let client: KeyvaultClientProtocol?
    public private(set) var overview: KeyvaultOverview
    public var selection: KvSelection = .category(category: .all)
    public var query = ""
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
            status: nil, serverVerified: false, items: [], pending: [], grants: [], rules: [],
            deliveries: [], audit: [], auditVerification: nil, partialErrors: [])
    }

    var nowMs: Int64 { Int64(clock().timeIntervalSince1970 * 1000) }

    public var page: KvPage { kvPage(overview: overview, nowMs: nowMs) }

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
    public var sidebar: KvSidebar { kvSidebar(overview: overview, nowMs: nowMs) }
    public var list: KvListView { kvList(overview: overview, selection: selection, nowMs: nowMs, query: query) }

    public func siteDetail(_ key: String) -> KvSiteDetail? {
        kvSiteDetail(overview: overview, key: key, nowMs: nowMs)
    }

    public var approvalView: KvApprovalView? {
        approval.map { kvApprovalView(overview: overview, state: $0) }
    }

    /// Re-reads the broker (never throws: unavailability is a page state).
    public func refresh() async {
        guard let client else { return }
        overview = await client.overview()
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

    public func setUnattended(itemIds: [String], on: Bool) async {
        await run(.setUnattended(itemIds: itemIds, unattended: on))
    }

    public func toggleSite(_ group: KvSiteGroup, on: Bool) async {
        await run(kvSiteToggle(group: group, on: on))
    }

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
