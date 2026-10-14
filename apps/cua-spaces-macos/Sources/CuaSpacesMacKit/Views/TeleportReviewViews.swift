// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpacesFFI
import SwiftUI

/// The consent review, as the vault list again: where it sends from (the
/// live app, or the app's saved Keyvault items), which sites of a browser
/// and which other items, with counts. What the review shows and what is
/// sent are the core's; the SDK's approval check enforces them.
struct TeleportReview: View {
    @Bindable var teleport: TeleportModel
    let review: AppReviewView

    var body: some View {
        Form {
            if review.offersVault { source }
            switch review.source {
            case .vault:
                Section("Saved in the Keyvault") { SavedItemsChoice(teleport: teleport) }
            case .live:
                if review.offersDomains { Section { sites } header: { Text("Sites") } }
                if !review.toggles.isEmpty { Section("Also sent") { items } }
            }
            if review.offersPasswords {
                Section {
                    Toggle(review.passwordsLabel, isOn: Binding(
                        get: { review.includePasswords },
                        set: { teleport.send(.togglePasswords(value: $0)) }))
                        .toggleStyle(.checkbox)
                        .accessibilityIdentifier("review-passwords")
                    Text("Off by default. Passwords are re-encrypted for the browser in the Space and need " +
                         "that browser to have been opened once.")
                        .font(.caption).foregroundStyle(.secondary)
                }
            }
            Section { summary }
            if review.needsAcknowledgement {
                Toggle("Send the secrets listed above", isOn: Binding(
                    get: { review.acknowledged },
                    set: { teleport.send(.acknowledge(value: $0)) }))
                    .toggleStyle(.checkbox)
            }
            if review.offersSaveToKeyvault && review.source == .live {
                Toggle("Save to Keyvault for reuse", isOn: Binding(
                    get: { review.saveToKeyvault },
                    set: { teleport.send(.saveToKeyvault(value: $0)) }))
                    .toggleStyle(.checkbox)
                    .help("Keep these items sealed in your Cua Keyvault, grouped by app. Saving again " +
                          "updates them instead of adding copies.")
            }
            if review.needsRelayPlaintextAcknowledgement {
                Section {
                    VStack(alignment: .leading, spacing: 4) {
                        Toggle("Send without end-to-end encryption", isOn: Binding(
                            get: { review.acknowledgedRelayPlaintext },
                            set: { teleport.send(.acknowledgeRelayPlaintext(value: $0)) }))
                            .toggleStyle(.checkbox)
                        Text("This Space predates end-to-end sealing. The relay could read these secrets in transit.")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
            }
        }
        .formStyle(.grouped)
    }

    /// Live app or saved items.
    private var source: some View {
        Section {
            Picker("Send from", selection: Binding(get: { review.source }, set: { s in
                Task { await teleport.sendFrom(s) }
            })) {
                Text(review.liveLabel).tag(AppSendSource.live)
                Text(review.vaultLabel).tag(AppSendSource.vault)
            }
            .pickerStyle(.radioGroup)
            .accessibilityIdentifier("review-source")
            if !review.sourceNote.isEmpty {
                Text(review.sourceNote).font(.caption).foregroundStyle(.secondary)
            }
        }
    }

    /// The browser's sites, with counts.
    @ViewBuilder private var sites: some View {
        if review.needsDomains {
            HStack(spacing: 8) {
                ProgressView().controlSize(.small)
                Text("Reading the sites\u{2026}").foregroundStyle(.secondary)
            }
        } else if review.domains.isEmpty && review.domainQuery.isEmpty {
            Text("No sites to choose. The cookies listed under Also sent are sent as they are.")
                .font(.callout).foregroundStyle(.secondary)
        } else {
            HStack(spacing: 8) {
                Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
                TextField("Search sites", text: Binding(get: { review.domainQuery },
                                                        set: { teleport.send(.domainQuery(text: $0)) }))
                    .textFieldStyle(.plain)
                Text(review.domainSummary).font(.caption).foregroundStyle(.secondary)
                Button("All") { teleport.send(.selectShownDomains(value: true)) }.buttonStyle(.link)
                Button("None") { teleport.send(.selectShownDomains(value: false)) }.buttonStyle(.link)
            }
            .accessibilityIdentifier("review-sites-search")
            List {
                ForEach(review.domains, id: \.domain) { d in
                    VStack(alignment: .leading, spacing: 2) {
                        HStack(spacing: 8) {
                            TriCheckbox(state: d.selected ? .on : .off, label: "Send \(d.domain)") {
                                teleport.send(.toggleDomain(domain: d.domain))
                            }
                            .disabled(!d.selectable)
                            Image(systemName: "globe").foregroundStyle(.secondary).frame(width: 18)
                            Text(d.domain).lineLimit(1)
                            Text(d.counts).font(.callout).foregroundStyle(.secondary).lineLimit(1)
                            Spacer(minLength: 8)
                            if d.identityProvider {
                                Text("Identity provider").font(.caption).foregroundStyle(.orange)
                                    .help("Its session signs in to other apps. Send it only if you mean to.")
                            } else if d.signin {
                                Text("Signs you in").font(.caption).foregroundStyle(.secondary)
                            }
                        }
                        // What cannot be read, greyed, with why.
                        if d.unavailable > 0 {
                            Text(d.unavailableNote).font(.caption).foregroundStyle(.tertiary).lineLimit(2)
                                .padding(.leading, 52)
                                .accessibilityIdentifier("review-site-unavailable-\(d.domain)")
                        }
                    }
                    .opacity(d.selectable ? 1 : 0.55)
                    .contentShape(Rectangle())
                    .onTapGesture { if d.selectable { teleport.send(.toggleDomain(domain: d.domain)) } }
                    .accessibilityElement(children: .contain)
                    .accessibilityIdentifier("review-site-\(d.domain)")
                }
            }
            .listStyle(.inset)
            .frame(height: min(CGFloat(max(review.domains.count, 1)) * 26 + CGFloat(review.domains.filter { $0.unavailable > 0 }.count) * 14 + 8, 190))
            .accessibilityIdentifier("review-sites")
        }
    }

    /// The other lines the user can turn off.
    private var items: some View {
        ForEach(review.toggles, id: \.key) { t in
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                TriCheckbox(state: t.selected ? .on : .off, label: "Send \(t.label)") {
                    teleport.send(.toggleItem(key: t.key))
                }
                VStack(alignment: .leading, spacing: 1) {
                    Text(t.label)
                    if !t.detail.isEmpty { Text(t.detail).font(.caption).foregroundStyle(.secondary) }
                }
                Spacer()
                if t.sensitive { Text("Secret").font(.caption).foregroundStyle(.secondary) }
                else if t.bytes > 0 { Text(ByteCountFormatter.string(fromByteCount: Int64(t.bytes), countStyle: .file)).font(.caption).foregroundStyle(.secondary) }
            }
            .contentShape(Rectangle())
            .onTapGesture { teleport.send(.toggleItem(key: t.key)) }
        }
    }

    private var summary: some View {
        VStack(alignment: .leading, spacing: 4) {
            ForEach(review.items.filter { $0.kind == .install }, id: \.key) { i in
                LabeledContent(i.label) { Text(i.detail).foregroundStyle(.secondary) }
            }
            ForEach(review.warnings, id: \.self) { Text($0).font(.caption).foregroundStyle(.secondary) }
        }
    }
}

/// The app's saved Keyvault items, to choose from: grouped by site with
/// checkboxes, search and the names behind Touch ID.
struct SavedItemsChoice: View {
    @Bindable var teleport: TeleportModel

    var body: some View {
        if let view = teleport.reviewVaultView {
            if view.namesHidden {
                HStack(spacing: 10) {
                    Image(systemName: "eye.slash").foregroundStyle(.secondary)
                    Text(view.hiddenNote ?? "").foregroundStyle(.secondary)
                    Spacer()
                    Button(view.showNamesLabel) { Task { await teleport.keyvault?.showItems() } }
                }
            } else {
                HStack(spacing: 8) {
                    Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
                    TextField("Search saved items", text: Binding(get: { teleport.reviewVault.query },
                                                                   set: { teleport.sendVault(.query(text: $0)) }))
                        .textFieldStyle(.plain)
                        .lineLimit(1)
                    Text("\(view.selection.count) of \(view.shown) selected")
                        .font(.caption).foregroundStyle(.secondary)
                    Button("All") { teleport.sendVault(.selectAll) }.buttonStyle(.link)
                    Button("None") { teleport.sendVault(.clear) }.buttonStyle(.link)
                }
                List {
                    ForEach(VaultLine.lines(view).filter { line in
                        if case .app = line { return false }; return true
                    }, id: \.id) { line in
                        SavedLine(line: line, teleport: teleport)
                    }
                }
                .listStyle(.inset)
                .frame(height: 190)
                .accessibilityIdentifier("review-vault-list")
            }
        }
    }
}

/// A site or an item of the saved list: a checkbox, no lock.
struct SavedLine: View {
    let line: VaultLine
    let teleport: TeleportModel

    var body: some View {
        switch line {
        case .site(_, let site):
            row(selected: site.selected, key: site.key, icon: "globe", title: site.site, detail: site.counts,
                open: site.open)
        case .files(_, let files):
            row(selected: files.selected, key: files.key, icon: "folder", title: "Files",
                detail: "\(files.count) file\(files.count == 1 ? "" : "s")", open: files.open)
        case .item(let item, _):
            HStack(spacing: 8) {
                TriCheckbox(state: item.selected ? .on : .off, label: "Send \(item.title)") {
                    teleport.sendVault(.toggle(id: item.id))
                }
                Image(systemName: item.symbol).foregroundStyle(.secondary).frame(width: 18)
                VStack(alignment: .leading, spacing: 0) {
                    Text(item.title).lineLimit(1).truncationMode(.middle)
                    if !item.subtitle.isEmpty { Text(item.subtitle).font(.caption).foregroundStyle(.secondary).lineLimit(1) }
                }
                Spacer()
                Text(item.updated).font(.caption).foregroundStyle(.secondary)
            }
            .padding(.leading, 40)
            .contentShape(Rectangle())
            .onTapGesture { teleport.sendVault(.toggle(id: item.id)) }
        case .app:
            EmptyView()
        }
    }

    private func row(selected: KvTri, key: String, icon: String, title: String, detail: String, open: Bool) -> some View {
        HStack(spacing: 8) {
            TriCheckbox(state: selected, label: "Send \(title)") { teleport.sendVault(.toggleGroup(key: key)) }
            Button { teleport.sendVault(.toggleOpen(key: key)) } label: {
                Image(systemName: open ? "chevron.down" : "chevron.right").font(.caption.weight(.semibold))
                    .foregroundStyle(.secondary).frame(width: 12)
            }
            .buttonStyle(.plain)
            Image(systemName: icon).foregroundStyle(.secondary).frame(width: 18)
            Text(title).lineLimit(1)
            Text(detail).font(.callout).foregroundStyle(.secondary).lineLimit(1)
            Spacer()
        }
        .contentShape(Rectangle())
        .onTapGesture { teleport.sendVault(.toggleOpen(key: key)) }
    }
}
