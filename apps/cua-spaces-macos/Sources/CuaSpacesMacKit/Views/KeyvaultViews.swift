// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import CuaSpacesFFI
import SwiftUI

/// The Keyvault, as a sidebar and list: the sidebar picks a category or a
/// site; this pane lists it. Everything shown is derived by the core from
/// what the broker returned; nothing here decides access and no secret value
/// is ever here.
struct KeyvaultDetail: View {
    @Bindable var keyvault: KeyvaultModel
    let selection: KvSelection

    var body: some View {
        let page = keyvault.page
        Group {
            if !page.ready {
                unavailable(page)
            } else {
                switch selection {
                case .site(let key):
                    if let detail = keyvault.siteDetail(key) {
                        SiteDetailView(keyvault: keyvault, detail: detail, blocked: page.disabled)
                    } else {
                        ContentUnavailableView { Text(keyvault.list.emptyText ?? "") }
                    }
                case .category:
                    CategoryList(keyvault: keyvault, page: page)
                }
            }
        }
        .navigationTitle(keyvault.list.title.isEmpty ? "Keyvault" : keyvault.list.title)
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

/// All, Waiting, Access or Recent.
struct CategoryList: View {
    @Bindable var keyvault: KeyvaultModel
    let page: KvPage

    var body: some View {
        let list = keyvault.list
        Form {
            if !list.sites.isEmpty {
                ForEach(list.sites, id: \.key) { group in
                    Section(group.title) {
                        ForEach(group.rows, id: \.item.id) { row in
                            AccountRow(keyvault: keyvault, group: group, row: row, blocked: page.disabled)
                        }
                    }
                }
            }
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
                            Button(a.actionLabel) { Task { await keyvault.run(a.command) } }
                                .disabled(keyvault.busy)
                        }
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
            if keyvault.selection == .category(category: .all), !page.protection.isEmpty {
                Section(page.labels.protectionTitle) {
                    ForEach(page.protection, id: \.label) { LabeledContent($0.label, value: $0.value) }
                }
            }
        }
        .formStyle(.grouped)
        .searchable(text: $keyvault.query)
    }
}

/// One account: its line and its unattended switch (identity providers
/// always ask; turning it on makes the daemon ask for Touch ID).
struct AccountRow: View {
    let keyvault: KeyvaultModel
    let group: KvSiteGroup
    let row: KvItemRow
    let blocked: Bool

    var body: some View {
        let state = row.consent.first { $0.kind != .asks }
        Toggle(isOn: Binding(
            get: { row.item.policy.unattended },
            set: { on in Task { await keyvault.setUnattended(itemIds: [row.item.id], on: on) } })) {
            HStack {
                Text(row.account).lineLimit(1)
                if let state {
                    Text(state.text).foregroundStyle(.secondary).lineLimit(1)
                }
            }
        }
        .toggleStyle(.switch)
        .disabled(!row.toggleEnabled || blocked || keyvault.busy)
        .help(row.toggleHelp)
        .accessibilityLabel("Allow \(row.account) on \(group.title) unattended")
    }
}

/// A site: every account, with the site-wide switch when there is more
/// than one.
struct SiteDetailView: View {
    let keyvault: KeyvaultModel
    let detail: KvSiteDetail
    let blocked: Bool

    var body: some View {
        let labels = keyvault.page.labels
        Form {
            Section {
                LabeledContent(labels.appLabel, value: detail.group.app)
                if detail.siteSwitch {
                    Toggle(labels.everyAccount, isOn: Binding(
                        get: { detail.siteState == .on },
                        set: { on in Task { await keyvault.toggleSite(detail.group, on: on) } }))
                        .toggleStyle(.switch)
                        .disabled(!detail.siteSwitchEnabled || blocked || keyvault.busy)
                        .help(detail.siteSwitchHelp)
                }
            }
            Section(labels.accountsTitle) {
                ForEach(detail.group.rows, id: \.item.id) { row in
                    AccountRow(keyvault: keyvault, group: detail.group, row: row, blocked: blocked)
                }
            }
        }
        .formStyle(.grouped)
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
