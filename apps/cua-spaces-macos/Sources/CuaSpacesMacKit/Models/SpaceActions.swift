// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpacesFFI
import Foundation

/// The bulk delete-and-leave confirm on screen: the core's view of the
/// Spaces it asks about (`appBulkConfirm`) and the items it was built from.
public struct BulkAsk: Identifiable, Equatable {
    public let id = UUID()
    public let items: [AppBulkItem]
    public let view: AppBulkConfirmView
}

/// A Space's right-click menu, the sidebar's multiple selection and
/// deleting or leaving several Spaces at once in the classic window
/// (CUA-2204). Every word and rule is the core's: the menu's items
/// (`appSpaceMenu`), the confirm (`appBulkConfirm`), its count while it runs
/// (`appBulkProgress`) and the done toast (`appBulkDone`); this maps the
/// app's Spaces into the core's inputs and runs what was chosen.
extension AppModel {
    // MARK: - What a Space allows

    /// Its sharing facts; a Space the core has none for yet (a create still
    /// going) is yours, with what its detail offers.
    func access(_ space: AppSpace) -> AppSpaceAccess {
        space.access ?? AppSpaceAccess(
            kind: .mine, role: .owner, ownerName: nil, ownerEmail: nil, teamName: nil, place: "",
            people: [], canPower: true, canTeleport: true, canShare: true, canRename: false,
            canDelete: true, canLeave: false)
    }

    /// Yours to delete (a team's Space you made too); otherwise left.
    func isMine(_ space: AppSpace) -> Bool { access(space).role == .owner }

    /// What the core's menu reads for this Space: its sharing facts, as far
    /// as this app offers each action for it (the detail's toolbar: Share
    /// stays out while Settings, Experiments has Sharing off).
    public func menuInput(_ space: AppSpace) -> AppSpaceMenuInput {
        let a = access(space)
        let d = detail(space)
        let offers = { (id: AppDetailActionId) in d.actions.contains { $0.id == id } }
        let button = appPowerButton(space: space)
        let power: AppSpaceMenuPower
        if let button {
            power = button.busy ? .busy : button.turnOn ? .off : .running
        } else {
            power = space.status == .running ? .running : .off
        }
        let busy = space.status == .deleting || space.status == .provisioning || isDeleting(space.id)
        return AppSpaceMenuInput(
            kind: menuKind(a.kind), role: menuRole(a.role),
            maker: a.kind == .team && a.canDelete,
            power: busy ? .busy : power,
            shared: !a.people.isEmpty,
            canPower: a.canPower && button != nil,
            canTeleport: a.canTeleport && offers(.teleport),
            canShare: a.canShare && offers(.share),
            // This app has no Rename for a Space yet: the item stays out
            // until it does.
            canRename: false,
            canDelete: a.canDelete && offers(.delete),
            canLeave: a.canLeave,
            // Pin to notch is in the web UI's menus (the only Spaces UI); this
            // model-level menu keeps to the actions the classic window had.
            canPin: false,
            pinned: settings.notchPinned.contains(space.id))
    }

    /// A Space's right-click menu (none for this Mac's own row).
    public func spaceMenu(_ space: AppSpace) -> [AppSpaceMenuItem] {
        if detail(space).isHost { return [] }
        return appSpaceMenu(input: menuInput(space))
    }

    /// Runs a menu item on `space`: the same flows as the Space's own
    /// detail (its power button, Share sheet, teleport picker and Open), and
    /// Delete Space or Leave through the one confirm.
    public func runMenuItem(_ id: AppSpaceMenuItemId, on space: AppSpace) {
        switch id {
        case .open:
            open(space, then: detail(space).actions.contains { $0.id == .open && $0.enabled } ? .open : nil)
        case .power:
            if let button = appPowerButton(space: space) { setPower(space, on: button.turnOn) }
        case .share, .manageSharing: open(space, then: .share)
        case .teleport: open(space, then: .teleport)
        case .rename: break
        case .pin: togglePin(space.id)
        case .delete, .leave:
            if removable(space) {
                askRemove([space])
            } else {
                // Remove from List, a Space another device made in your
                // cloud, a failed create: the detail's own confirm says how.
                open(space, then: nil)
                confirmDeleteId = space.id
            }
        }
    }

    /// Shows `space` alone in the detail, then runs `action` there.
    private func open(_ space: AppSpace, then action: AppDetailActionId?) {
        pickedSpaceIds = []
        select(space.id)
        if let action { detailRequest = DetailRequest(spaceId: space.id, action: action) }
    }

    // MARK: - The sidebar's selection

    /// A Space row that can be one of several picked (not this Mac).
    func pickable(_ id: String) -> Bool {
        guard let space = spaces.first(where: { $0.id == id }) else { return false }
        return !detail(space).isHost
    }

    /// The picked Spaces in the sidebar's order; empty unless two or more
    /// (and gone once anything else is selected).
    public var pickedSpaces: [AppSpace] {
        guard pickedSpaceIds.count > 1, let id = selectedSpaceId, pickedSpaceIds.contains(id) else { return [] }
        let order = sidebar.sections.flatMap(\.rows).map(\.id)
        let picked = order.filter { pickedSpaceIds.contains($0) }
        let spaces = picked.compactMap { id in self.spaces.first { $0.id == id } }
        return spaces.count > 1 ? spaces : []
    }

    /// What the sidebar's list shows selected.
    public var sidebarSelection: Set<MainSelection> {
        let picked = pickedSpaces
        if !picked.isEmpty { return Set(picked.map { .space($0.id) }) }
        return selection.map { [$0] } ?? []
    }

    /// The list's new selection (a click, ⌘- or ⇧-click, ⌘A): two or more
    /// Spaces are picked together (anything else in it is dropped, as ⌘A
    /// selects every row); one new item that is not a Space, or this Mac,
    /// is selected alone.
    public func setSidebarSelection(_ new: Set<MainSelection>) {
        let old = sidebarSelection
        guard new != old else { return }
        let added = new.subtracting(old)
        let ids = new.compactMap { item -> String? in
            if case .space(let id) = item { return id }
            return nil
        }.filter(pickable)
        let lone: MainSelection? = added.count == 1 ? added.first : nil
        let loneIsSpace = lone.map { item -> Bool in
            if case .space(let id) = item { return pickable(id) }
            return false
        } ?? true
        if ids.count >= 2, loneIsSpace {
            pickedSpaceIds = Set(ids)
            let addedId = added.compactMap { item -> String? in
                if case .space(let id) = item, ids.contains(id) { return id }
                return nil
            }.first
            let keep = selectedSpaceId.flatMap { ids.contains($0) ? $0 : nil }
            if let id = addedId ?? keep ?? ids.first { selection = .space(id) }
            return
        }
        pickedSpaceIds = []
        selection = lone ?? (new.count == 1 ? new.first : ids.first.map { .space($0) } ?? new.first)
    }

    /// ⌘A: every Space the sidebar lists (not this Mac).
    public func selectAllSpaces() {
        let ids = sidebar.sections.flatMap(\.rows).map(\.id).filter(pickable)
        guard let first = ids.first else { return }
        guard ids.count > 1 else { return select(first) }
        pickedSpaceIds = Set(ids)
        if selectedSpaceId.map({ !ids.contains($0) }) ?? true { selection = .space(first) }
    }

    /// Esc: back to the one Space last chosen. False when nothing was
    /// picked (the key is not this list's).
    @discardableResult
    public func clearPickedSpaces() -> Bool {
        guard pickedSpaceIds.count > 1 else { return false }
        pickedSpaceIds = []
        return true
    }

    // MARK: - Power for several

    /// Turn on or Turn off for the picked Spaces, by what is picked (only
    /// when every one may): off while any of them runs. Nil: not offered.
    public func pickedPower(_ spaces: [AppSpace]) -> (label: String, on: Bool)? {
        guard !spaces.isEmpty else { return nil }
        let buttons = spaces.map { space -> AppPowerButton? in
            access(space).canPower && access(space).role != .viewer ? appPowerButton(space: space) : nil
        }
        guard buttons.allSatisfy({ $0.map { !$0.busy && $0.enabled } ?? false }) else { return nil }
        let anyRunning = buttons.contains { $0?.turnOn == false }
        return anyRunning ? ("Turn off", false) : ("Turn on", true)
    }

    /// Turns each of `spaces` on or off (those already so stay).
    public func setPower(_ spaces: [AppSpace], on: Bool) {
        for space in spaces where appPowerButton(space: space)?.turnOn == on {
            setPower(space, on: on)
        }
    }

    // MARK: - Delete and leave

    /// It can go in the one confirm: yours to delete (an ordinary delete,
    /// not Remove from List), or someone else's to leave; not while it is
    /// still being created or already going.
    public func removable(_ space: AppSpace) -> Bool {
        guard !isDeleting(space.id), !appCreatesIsPending(id: space.id),
              space.status != .deleting else { return false }
        let a = access(space)
        guard isMine(space) else { return a.canLeave }
        let d = detail(space)
        return a.canDelete && d.actions.contains { $0.id == .delete && $0.enabled } && !d.removeOnly
            && d.confirm.confirmEnabled && d.confirm.removeLabel == nil
    }

    /// What the core's confirm reads for one Space.
    public func bulkItem(_ space: AppSpace) -> AppBulkItem {
        let a = access(space)
        let mine = isMine(space)
        let running = appPowerButton(space: space).map { !$0.turnOn } ?? (space.status == .running)
        return AppBulkItem(
            id: space.id, name: space.name, os: Self.osWord(space.os), running: running,
            // Sizes on disk reach this app with `resources.get` (CUA-2204);
            // until then the confirm shows none and no total.
            bytes: nil,
            mine: mine,
            place: a.place.isEmpty ? (space.hostName ?? space.cloudPlace ?? "") : a.place,
            team: a.kind == .team,
            people: mine ? a.people.map { AppBulkPerson(who: $0.who, name: $0.name) } : [])
    }

    /// The confirm for the picked Spaces (only those that can go), or nil.
    public func bulkConfirm(_ spaces: [AppSpace]) -> AppBulkConfirmView? {
        appBulkConfirm(items: spaces.filter(removable).map(bulkItem))
    }

    /// Asks about deleting and leaving `spaces` (nothing when none can go,
    /// or while a run is going).
    public func askRemove(_ spaces: [AppSpace]) {
        guard bulkRunItems == nil else { return }
        let items = spaces.filter(removable).map(bulkItem)
        guard let view = appBulkConfirm(items: items) else { return }
        bulkAsk = BulkAsk(items: items, view: view)
    }

    /// While a run goes: the core's "Deleting 1 of 3…".
    public var bulkProgressText: String? {
        guard let items = bulkRunItems else { return nil }
        return appBulkProgress(items: items, finished: bulkFinished)
    }

    /// Yes on the confirm: yours are deleted and the others left, every row
    /// says Deleting until its own result is in; then the core's toast.
    public func confirmBulk() async {
        guard let ask = bulkAsk, bulkRunItems == nil else { return }
        bulkAsk = nil
        bulkRunItems = ask.items
        bulkFinished = []
        let note: (SpaceBulkResult) -> Void = { [weak self] r in self?.bulkFinished.append(r.id) }
        async let deleted: [SpaceBulkResult] = ask.view.deleteIds.isEmpty
            ? [] : bulk(ids: ask.view.deleteIds, leave: false, onResult: note)
        async let left: [SpaceBulkResult] = ask.view.leaveIds.isEmpty
            ? [] : bulk(ids: ask.view.leaveIds, leave: true, onResult: note)
        let results = await deleted + left
        bulkRunItems = nil
        bulkFinished = []
        let gone = Set(results.filter(\.ok).map(\.id))
        pickedSpaceIds.subtract(gone)
        if pickedSpaceIds.count < 2 { pickedSpaceIds = [] }
        bulkToast = appBulkDone(items: ask.items, results: results.map {
            AppBulkResult(id: $0.id, ok: $0.ok, error: $0.error)
        })
    }

    // MARK: - Mapping

    static func osWord(_ os: AppSpaceOs) -> String {
        switch os {
        case .macos: "macos"
        case .linux: "linux"
        case .windows: "windows"
        case .unknown: "unknown"
        }
    }

    private func menuKind(_ k: AppSpaceAccessKind) -> AppSpaceMenuKind {
        switch k {
        case .mine: .mine
        case .withMe: .withMe
        case .team: .team
        }
    }

    private func menuRole(_ r: AppSpaceAccessRole) -> AppSpaceMenuRole {
        switch r {
        case .owner: .owner
        case .editor: .editor
        case .viewer: .viewer
        }
    }
}
