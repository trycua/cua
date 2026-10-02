// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import CuaSpacesFFI
import SwiftUI

/// The Keyvault, as a sidebar and list: the sidebar picks a category or an
/// app; this pane lists it. The vault list is a password manager's: items
/// grouped by the app they came from, each with a lock. Everything shown is
/// derived by the core from what the broker returned; nothing here decides
/// access and no secret value is ever here.
struct KeyvaultDetail: View {
    @Bindable var keyvault: KeyvaultModel
    let selection: KvSelection

    var body: some View {
        let page = keyvault.page
        Group {
            if !page.ready {
                unavailable(page)
            } else {
                let list = keyvault.list
                if list.vault {
                    VaultList(keyvault: keyvault, page: page)
                } else if case .app = selection {
                    ContentUnavailableView { Text(list.emptyText ?? "") }
                } else {
                    CategoryList(keyvault: keyvault, page: page)
                }
            }
        }
        .navigationTitle(keyvault.list.title.isEmpty ? "Keyvault" : keyvault.list.title)
        .task(id: keyvault.overview.items.count) { await keyvault.loadIcons() }
        .onAppear { keyvault.selection = selection }
        .onChange(of: selection) { _, new in keyvault.selection = new }
        .task(id: "kv-poll") {
            while !Task.isCancelled {
                await keyvault.refresh()
                try? await Task.sleep(for: .seconds(5))
            }
        }
        .safeAreaInset(edge: .top) { banners(page) }
    }

    @ViewBuilder private func banners(_ page: KvPage) -> some View {
        VStack(spacing: 0) {
            if let banner = page.disabledBanner { Banner(text: banner, error: true) }
            if let notice = page.resetNotice { Banner(text: notice, error: false) }
            if let error = keyvault.error { Banner(text: error, error: true) }
            if let key = keyvault.recoveryKey { Banner(text: kvRecoveryKeyText(key: key), error: false) }
        }
    }

    private func unavailable(_ page: KvPage) -> some View {
        ContentUnavailableView {
            Text(page.unavailableTitle ?? "The Keyvault is unavailable")
        } description: {
            Text(page.message ?? "")
        } actions: {
            if let form = page.form {
                CredentialForm(keyvault: keyvault, form: form)
            }
        }
    }
}

/// Set up or unlock: one button for Touch ID, or secure fields for a
/// passphrase (twice for setup) when the daemon cannot use the OS key store.
struct CredentialForm: View {
    @Bindable var keyvault: KeyvaultModel
    let form: KvCredentialForm

    var body: some View {
        VStack(spacing: 8) {
            if form.method == .passphrase {
                SecureField(form.passphraseLabel ?? "", text: $keyvault.passphrase)
                if let confirm = form.confirmLabel {
                    SecureField(confirm, text: $keyvault.passphraseConfirm)
                }
            }
            Button(form.submitLabel) { Task { await keyvault.submitCredential() } }
                .buttonStyle(.glassProminent)
                .keyboardShortcut(.defaultAction)
                .disabled(!keyvault.canSubmitCredential)
            Text(keyvault.passphraseCheck?.hint ?? form.help)
                .font(.caption)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)
        }
        .textFieldStyle(.roundedBorder)
        .frame(width: 340)
        .onSubmit { Task { await keyvault.submitCredential() } }
    }
}

struct Banner: View {
    let text: String
    let error: Bool

    var body: some View {
        Text(text)
            .font(.callout)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 16).padding(.vertical, 8)
            .background(error ? Color.red.opacity(0.12) : Color.accentColor.opacity(0.10))
    }
}

/// The vault list: apps, their sites and items, each with a lock; checkboxes
/// select items, a site or a whole app for the batch bar (lock, unlock,
/// delete). Search matches a domain, a key, an app or a type.
struct VaultList: View {
    @Bindable var keyvault: KeyvaultModel
    let page: KvPage

    var body: some View {
        let view = keyvault.vaultView
        VStack(spacing: 0) {
            SearchBar(keyvault: keyvault, view: view)
            if view.namesHidden { HiddenNames(view: view, keyvault: keyvault) }
            if let empty = view.emptyText {
                ContentUnavailableView { Text(empty) }
            } else {
                List {
                    ForEach(VaultLine.lines(view), id: \.id) { line in
                        VaultLineRow(line: line, keyvault: keyvault, blocked: page.disabled)
                            .listRowSeparator(.visible)
                    }
                }
                .listStyle(.inset)
                .accessibilityIdentifier("vault-list")
            }
        }
        .safeAreaInset(edge: .bottom) {
            if view.selection.count > 0 { BatchBar(keyvault: keyvault, selection: view.selection, blocked: page.disabled) }
        }
        .sheet(item: $keyvault.unlockPrompt) { pending in
            UnlockPromptSheet(prompt: pending.prompt) { answer in Task { await keyvault.answerUnlock(answer) } }
        }
        .sheet(item: $keyvault.deleteConfirm) { pending in
            DeleteConfirmSheet(confirm: pending.confirm,
                               onDelete: { Task { await keyvault.confirmDelete() } },
                               onCancel: { keyvault.deleteConfirm = nil })
        }
    }
}

/// Search by domain, key, app or type, and Select All.
struct SearchBar: View {
    let keyvault: KeyvaultModel
    let view: KvVaultView

    var body: some View {
        HStack(spacing: 10) {
            HStack(spacing: 6) {
                Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
                TextField(view.searchPrompt, text: Binding(get: { keyvault.query }, set: { keyvault.query = $0 }))
                    .textFieldStyle(.plain)
                    .accessibilityIdentifier("vault-search")
                if !keyvault.query.isEmpty {
                    Button { keyvault.query = "" } label: {
                        Image(systemName: "xmark.circle.fill").foregroundStyle(.secondary)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Clear search")
                }
            }
            .padding(.horizontal, 8).padding(.vertical, 5)
            .background(RoundedRectangle(cornerRadius: 7).fill(Color.secondary.opacity(0.12)))
            if view.canSelectAll {
                Button(view.selection.count == view.shown ? "Select None" : "Select All") {
                    keyvault.send(view.selection.count == view.shown ? .clear : .selectAll)
                }
                .buttonStyle(.link)
                .accessibilityIdentifier("vault-select-all")
            }
        }
        .padding(.horizontal, 16).padding(.vertical, 10)
    }
}

/// The names are hidden until the user confirms with Touch ID.
struct HiddenNames: View {
    let view: KvVaultView
    let keyvault: KeyvaultModel

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: "eye.slash").foregroundStyle(.secondary)
            Text(view.hiddenNote ?? "").foregroundStyle(.secondary).lineLimit(2)
            Spacer()
            Button(view.showNamesLabel) { Task { await keyvault.showItems() } }
                .disabled(keyvault.busy)
                .accessibilityIdentifier("vault-show-items")
        }
        .padding(.horizontal, 16).padding(.vertical, 10)
        .background(.bar)
        .overlay(alignment: .bottom) { Divider() }
    }
}

/// One line of the flattened list: an app, a site, the files of an app, or
/// an item. The core decides what is open.
enum VaultLine: Identifiable {
    case app(KvVaultApp)
    case site(KvVaultApp, KvVaultSite)
    case files(KvVaultApp, KvVaultFiles)
    case item(KvVaultRow, indent: Int)

    var id: String {
        switch self {
        case .app(let a): return "app:\(a.key)"
        case .site(_, let s): return "site:\(s.key)"
        case .files(_, let f): return "files:\(f.key)"
        case .item(let r, _): return "item:\(r.id)"
        }
    }

    static func lines(_ view: KvVaultView) -> [VaultLine] {
        var out: [VaultLine] = []
        for app in view.apps {
            out.append(.app(app))
            guard app.open else { continue }
            for site in app.sites {
                out.append(.site(app, site))
                if site.open { out += site.rows.map { .item($0, indent: 2) } }
            }
            if let files = app.files {
                out.append(.files(app, files))
                if files.open { out += files.rows.map { .item($0, indent: 2) } }
            }
        }
        return out
    }
}

struct VaultLineRow: View {
    let line: VaultLine
    let keyvault: KeyvaultModel
    let blocked: Bool

    var body: some View {
        switch line {
        case .app(let app):
            GroupRow(indent: 0, selected: app.selected, open: app.open,
                     onSelect: { keyvault.send(.toggleGroup(key: app.key)) },
                     onOpen: { keyvault.send(.toggleOpen(key: app.key)) }) {
                AppIcon(keyvault: keyvault, providerId: app.providerId, name: app.name)
            } title: {
                Text(app.name).font(.headline).lineLimit(1)
            } detail: {
                Text(app.summary)
            } trailing: {
                updated(app.updated)
                LockButton(state: app.lock, name: app.name, unlock: app.unlockIds, lock: app.lockIds,
                           keyvault: keyvault, blocked: blocked)
            }
        case .site(_, let site):
            GroupRow(indent: 1, selected: site.selected, open: site.open,
                     onSelect: { keyvault.send(.toggleGroup(key: site.key)) },
                     onOpen: { keyvault.send(.toggleOpen(key: site.key)) }) {
                SiteIcon(keyvault: keyvault, site: site.site)
            } title: {
                Text(site.site).lineLimit(1)
            } detail: {
                Text(site.counts)
            } trailing: {
                updated(site.updated)
                LockButton(state: site.lock, name: site.site, unlock: site.unlockIds, lock: site.lockIds,
                           keyvault: keyvault, blocked: blocked)
            }
        case .files(_, let files):
            GroupRow(indent: 1, selected: files.selected, open: files.open,
                     onSelect: { keyvault.send(.toggleGroup(key: files.key)) },
                     onOpen: { keyvault.send(.toggleOpen(key: files.key)) }) {
                Image(systemName: "folder").foregroundStyle(.secondary).frame(width: 20)
            } title: {
                Text("Files").lineLimit(1)
            } detail: {
                Text("\(files.count) file\(files.count == 1 ? "" : "s")")
            } trailing: {
                LockButton(state: files.lock, name: "files", unlock: files.unlockIds, lock: files.lockIds,
                           keyvault: keyvault, blocked: blocked)
            }
        case .item(let row, let indent):
            ItemRow(row: row, indent: indent, keyvault: keyvault, blocked: blocked)
        }
    }

    private func updated(_ text: String) -> some View {
        Text(text).font(.caption).foregroundStyle(.secondary).lineLimit(1)
    }
}

/// A checkbox that is one of off, on or mixed.
struct TriCheckbox: View {
    let state: KvTri
    let label: String
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Image(systemName: state == .on ? "checkmark.square.fill" : state == .mixed ? "minus.square.fill" : "square")
                .foregroundStyle(state == .off ? Color.secondary : Color.accentColor)
                .imageScale(.large)
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
        .accessibilityValue(state == .on ? "selected" : state == .mixed ? "some selected" : "not selected")
    }
}

/// An app, a site or a file group: checkbox, disclosure, icon, name, detail.
struct GroupRow<Icon: View, Title: View, Detail: View, Trailing: View>: View {
    let indent: Int
    let selected: KvTri
    let open: Bool
    let onSelect: () -> Void
    let onOpen: () -> Void
    @ViewBuilder let icon: Icon
    @ViewBuilder let title: Title
    @ViewBuilder let detail: Detail
    @ViewBuilder let trailing: Trailing

    init(indent: Int, selected: KvTri, open: Bool, onSelect: @escaping () -> Void, onOpen: @escaping () -> Void,
         @ViewBuilder icon: () -> Icon, @ViewBuilder title: () -> Title, @ViewBuilder detail: () -> Detail,
         @ViewBuilder trailing: () -> Trailing) {
        self.indent = indent; self.selected = selected; self.open = open
        self.onSelect = onSelect; self.onOpen = onOpen
        self.icon = icon(); self.title = title(); self.detail = detail(); self.trailing = trailing()
    }

    var body: some View {
        HStack(spacing: 8) {
            TriCheckbox(state: selected, label: "Select", action: onSelect)
            Button(action: onOpen) {
                Image(systemName: open ? "chevron.down" : "chevron.right")
                    .font(.caption.weight(.semibold)).foregroundStyle(.secondary).frame(width: 12)
            }
            .buttonStyle(.plain)
            .accessibilityLabel(open ? "Collapse" : "Expand")
            icon
            title
            detail.font(.callout).foregroundStyle(.secondary).lineLimit(1)
            Spacer(minLength: 8)
            trailing
        }
        .padding(.leading, CGFloat(indent) * 22)
        .padding(.vertical, 3)
        .contentShape(Rectangle())
        .onTapGesture(perform: onOpen)
    }
}

/// One secret: its type icon, its key, its domain in grey, when it was
/// saved in grey, and its lock. Never its value.
struct ItemRow: View {
    let row: KvVaultRow
    let indent: Int
    let keyvault: KeyvaultModel
    let blocked: Bool

    var body: some View {
        HStack(spacing: 8) {
            TriCheckbox(state: row.selected ? .on : .off, label: "Select \(row.title)") {
                keyvault.send(.toggle(id: row.id))
            }
            Image(systemName: row.symbol).foregroundStyle(.secondary).frame(width: 20)
                .help(row.kindLabel)
            VStack(alignment: .leading, spacing: 1) {
                Text(row.title).lineLimit(1).truncationMode(.middle)
                if !row.subtitle.isEmpty {
                    Text(row.subtitle).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                }
            }
            Spacer(minLength: 8)
            Text(row.updated).font(.caption).foregroundStyle(.secondary).lineLimit(1)
            LockButton(state: row.locked ? .locked : .unlocked, name: row.title,
                       unlock: row.locked && !row.identityProvider ? [row.id] : [],
                       lock: row.locked ? [] : [row.id], keyvault: keyvault, blocked: blocked,
                       help: row.lockHelp)
        }
        .padding(.leading, CGFloat(indent) * 22)
        .padding(.vertical, 2)
        .contentShape(Rectangle())
        .onTapGesture { keyvault.send(.toggle(id: row.id)) }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("vault-item-\(row.id)")
    }
}

/// The lock: closed while every use needs approval, open once unattended
/// access is allowed. A click flips it for what it stands for (an item, a
/// site, an app); unlocking asks first, then Touch ID once.
struct LockButton: View {
    let state: KvLock
    let name: String
    let unlock: [String]
    let lock: [String]
    let keyvault: KeyvaultModel
    let blocked: Bool
    var help: String?

    var body: some View {
        Button {
            Task {
                if state == .unlocked { await keyvault.lock(ids: lock) }
                else if !unlock.isEmpty { await keyvault.requestUnlock(ids: unlock, name: unlock.count == 1 ? name : nil) }
            }
        } label: {
            Image(systemName: state.symbol)
                .imageScale(.medium)
                .foregroundStyle(state == .locked ? Color.secondary : Color.orange)
                .opacity(state == .mixed ? 0.55 : 1)
                .frame(width: 22, height: 22)
        }
        .buttonStyle(.plain)
        .disabled(blocked || keyvault.busy || (state == .locked && unlock.isEmpty))
        .help(help ?? (state == .unlocked ? "Unlocked. Click to lock." : state == .mixed ? "Some unlocked. Click to unlock the rest." : "Locked. Click to allow unattended access."))
        .accessibilityLabel(state == .unlocked ? "Lock \(name)" : "Unlock \(name)")
        .accessibilityValue(state == .unlocked ? "unlocked" : state == .mixed ? "partly unlocked" : "locked")
    }
}

extension KvLock {
    /// The SF Symbol: the core's, mirrored here for the closed and open lock.
    var symbol: String {
        switch self {
        case .locked: return "lock.fill"
        case .unlocked: return "lock.open"
        case .mixed: return "lock.open"
        }
    }
}

/// The batch bar: what the selection can do, together.
struct BatchBar: View {
    let keyvault: KeyvaultModel
    let selection: KvVaultSelection
    let blocked: Bool

    var body: some View {
        HStack(spacing: 10) {
            Text(selection.title).fontWeight(.medium)
            Button("Clear") { keyvault.send(.clear) }.buttonStyle(.link)
            Spacer()
            Button { Task { await keyvault.lock(ids: selection.lockIds) } } label: {
                Label("Lock", systemImage: "lock.fill")
            }
            .disabled(!selection.canLock || keyvault.busy)
            .accessibilityIdentifier("batch-lock")
            Button { Task { await keyvault.requestUnlock(ids: selection.unlockIds) } } label: {
                Label("Unlock", systemImage: "lock.open")
            }
            .disabled(!selection.canUnlock || blocked || keyvault.busy)
            .help(selection.alwaysAsk > 0 ? "Identity provider sessions always ask and stay locked." : "")
            .accessibilityIdentifier("batch-unlock")
            Button(role: .destructive) { keyvault.requestDelete(ids: selection.ids) } label: {
                Label("Delete", systemImage: "trash")
            }
            .disabled(keyvault.busy)
            .accessibilityIdentifier("batch-delete")
        }
        .padding(.horizontal, 16).padding(.vertical, 10)
        .background(.bar)
        .overlay(alignment: .top) { Divider() }
    }
}

/// A site's icon (the browser's own, else Google's when allowed), else the
/// globe. It asks for the icon once, as it first appears.
struct SiteIcon: View {
    let keyvault: KeyvaultModel
    let site: String

    var body: some View {
        Group {
            if let image = keyvault.siteIcons[site] {
                Image(nsImage: image).resizable().interpolation(.high).clipShape(RoundedRectangle(cornerRadius: 3))
                    .frame(width: 16, height: 16)
            } else {
                Image(systemName: "globe").foregroundStyle(.secondary)
            }
        }
        .frame(width: 20)
        .accessibilityHidden(true)
        .task(id: site) { await keyvault.siteIcon(site) }
    }
}

/// An app's icon from the cache, else its first letter.
struct AppIcon: View {
    let keyvault: KeyvaultModel
    let providerId: String
    let name: String

    var body: some View {
        Group {
            if let image = keyvault.icons[providerId] {
                Image(nsImage: image).resizable().interpolation(.high)
            } else {
                RoundedRectangle(cornerRadius: 5).fill(Color.secondary.opacity(0.18))
                    .overlay(Text(String(name.prefix(1)).uppercased()).font(.system(size: 11, weight: .semibold)))
            }
        }
        .frame(width: 22, height: 22)
        .accessibilityHidden(true)
    }
}

/// "Allow unattended access?": what unlocking allows, then Deny, Allow or
/// Never ask again. Touch ID comes next, from the daemon.
struct UnlockPromptSheet: View {
    let prompt: KvUnlockPrompt
    let onAnswer: (UnlockAnswer) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(alignment: .top, spacing: 12) {
                Image(systemName: "lock.open.fill").font(.system(size: 26)).foregroundStyle(.orange)
                VStack(alignment: .leading, spacing: 2) {
                    Text(prompt.title).font(.headline)
                    Text(prompt.subject).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
                }
            }
            Text(prompt.message).fixedSize(horizontal: false, vertical: true)
            HStack {
                Button(prompt.neverAsk) { onAnswer(.neverAskAgain) }
                    .accessibilityIdentifier("unlock-never-ask")
                Spacer()
                Button(prompt.deny) { onAnswer(.deny) }
                    .keyboardShortcut(.cancelAction)
                    .accessibilityIdentifier("unlock-deny")
                Button(prompt.allow) { onAnswer(.allow) }
                    .keyboardShortcut(.defaultAction)
                    .buttonStyle(.borderedProminent)
                    .accessibilityIdentifier("unlock-allow")
            }
        }
        .padding(20)
        .frame(width: 440)
    }
}

/// "Delete 3 items?": says when live copies in Spaces are wiped too.
struct DeleteConfirmSheet: View {
    let confirm: KvDeleteConfirm
    let onDelete: () -> Void
    let onCancel: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(alignment: .top, spacing: 12) {
                Image(systemName: "trash").font(.system(size: 24)).foregroundStyle(.red)
                Text(confirm.title).font(.headline)
            }
            Text(confirm.message).fixedSize(horizontal: false, vertical: true)
            HStack {
                Spacer()
                Button(confirm.cancel, action: onCancel).keyboardShortcut(.cancelAction)
                Button(confirm.confirm, role: .destructive, action: onDelete)
                    .keyboardShortcut(.defaultAction)
                    .accessibilityIdentifier("delete-confirm")
            }
        }
        .padding(20)
        .frame(width: 420)
    }
}

/// Waiting, Access or Recent.
struct CategoryList: View {
    @Bindable var keyvault: KeyvaultModel
    let page: KvPage

    var body: some View {
        let list = keyvault.list
        ScrollViewReader { proxy in
        Form {
            if !list.pending.isEmpty {
                Section {
                    ForEach(list.pending, id: \.id) { p in
                        HStack {
                            Text("\(p.caller) (\(p.badge.text)) \u{b7} \(p.summary) \u{b7} \(p.wants)")
                                .lineLimit(1).help(p.claims.joined(separator: "\n"))
                            Spacer()
                            Button(page.labels.deny) { Task { await keyvault.deny(p.id) } }
                            Button(page.labels.review) { keyvault.openApproval(p.id) }
                                .disabled(page.disabled || keyvault.busy)
                        }
                    }
                }
            }
            if !list.access.isEmpty {
                Section {
                    if page.revokeAll {
                        HStack {
                            Spacer()
                            Button(page.labels.revokeAll) { Task { await keyvault.run(.revokeGrant(id: "*")) } }
                                .disabled(keyvault.busy)
                        }
                    }
                    ForEach(list.access, id: \.key) { a in
                        HStack {
                            Text(a.detail.isEmpty ? a.text : "\(a.text) \u{b7} \(a.detail)").lineLimit(1).help(a.detail)
                            Spacer()
                            // Hides the notch's indicator only; Wipe removes it.
                            if !a.imports.isEmpty, !keyvault.isDismissed(a) {
                                Button("Dismiss") { keyvault.dismiss(a.imports) }
                                    .help("Hide from the notch. Access stays until you wipe it.")
                            }
                            Button(a.actionLabel) { Task { await keyvault.run(a.command) } }
                                .disabled(keyvault.busy)
                        }
                        .id(a.key)
                        .listRowBackground(keyvault.focusKey == a.key ? Color.accentColor.opacity(0.12) : nil)
                    }
                }
            }
            if !list.recent.isEmpty {
                Section {
                    ForEach(list.recent, id: \.decision.entry.seq) { r in
                        LabeledContent {
                            Text(r.age).foregroundStyle(.secondary)
                        } label: {
                            Text("\(r.decision.verb) \(r.decision.what)").lineLimit(1)
                        }
                    }
                } footer: {
                    if let log = page.logStatus { Text(log) }
                }
            }
            if let empty = list.emptyText {
                Text(empty).foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
        .task(id: keyvault.focusKey) {
            // A Space's "Signed in" badge: bring its row forward, then let
            // the highlight go.
            guard let key = keyvault.focusKey else { return }
            withAnimation { proxy.scrollTo(key, anchor: .center) }
            try? await Task.sleep(for: .seconds(2))
            if keyvault.focusKey == key { withAnimation { keyvault.focusKey = nil } }
        }
        }
    }
}

/// The approval sheet: nothing is selected until the user ticks items;
/// Approve sends exactly those, and the Cua daemon confirms with Touch ID.
struct ApprovalSheet: View {
    @Bindable var keyvault: KeyvaultModel

    var body: some View {
        let labels = keyvault.page.labels
        if let v = keyvault.approvalView {
            VStack(alignment: .leading, spacing: 12) {
                Text(v.title).font(.headline)
                Text("\(v.badge.text) · to \(v.targets) · \(v.wants)")
                    .foregroundStyle(.secondary)
                    .help(v.claims.joined(separator: "\n"))
                Form {
                    ForEach(v.rows, id: \.key) { row in
                        Toggle(isOn: Binding(
                            get: { row.selected },
                            set: { _ in keyvault.sendApproval(.toggle(key: row.key)) })) {
                            Text("\(row.title) \u{b7} \(row.account)").lineLimit(1)
                        }
                        .toggleStyle(.checkbox)
                    }
                }
                .formStyle(.grouped)
                if let reason = v.blockedReason {
                    Text(reason).font(.callout).foregroundStyle(.secondary)
                }
                HStack {
                    Text(labels.confirmNote).font(.callout).foregroundStyle(.secondary)
                    Spacer()
                    Button(labels.cancel) { keyvault.approval = nil }.keyboardShortcut(.cancelAction)
                    Button(labels.deny) { Task { await keyvault.deny(v.requestId) } }
                    Button(v.approveLabel) { Task { await keyvault.approve() } }
                        .keyboardShortcut(.defaultAction)
                        .disabled(!v.canApprove || keyvault.busy)
                }
            }
            .padding(20)
            .frame(width: 480, height: 360)
        }
    }
}
