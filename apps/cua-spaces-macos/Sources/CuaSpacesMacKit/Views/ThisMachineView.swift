// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSDK
import CuaSpacesFFI
import SwiftUI

/// "This machine": how it is shared, who is connected, the permissions
/// left to grant and its buttons; or the host setup form. Everything shown
/// is the core's `appHostPanel` / `appHostFormView`.
struct ThisMachineView: View {
    @Bindable var host: HostModel
    /// A button waiting for the user's confirmation (the core's `confirm`).
    @State private var confirming: AppHostAction?

    var body: some View {
        let panel = host.panel
        Group {
            if host.form != nil {
                HostFormView(host: host, buttons: true)
            } else {
                Form {
                    if let intro = panel.intro {
                        // Before setup: what this machine is and why set it
                        // up, then the form's choices inline.
                        Section {
                            Text(intro)
                                .foregroundStyle(.secondary)
                                .fixedSize(horizontal: false, vertical: true)
                                .accessibilityIdentifier("host-summary")
                        }
                        Section {
                            ForEach(panel.setupChoices, id: \.id) { choice in
                                LabeledContent(choice.label) {
                                    Button(choice.buttonLabel) {
                                        host.openForm()
                                        host.send(.setProfile(profile: choice.id))
                                    }
                                    .disabled(host.busy)
                                }
                                .accessibilityIdentifier("host-choice-\(choice.id)")
                            }
                        }
                    } else {
                        Section {
                            Text(panel.summary)
                                .lineLimit(1)
                                .accessibilityIdentifier("host-summary")
                        }
                    }
                    if !panel.facts.isEmpty {
                        Section {
                            ForEach(panel.facts, id: \.label) { fact in
                                LabeledContent(fact.label) {
                                    Text(fact.value).lineLimit(1).truncationMode(.middle).textSelection(.enabled)
                                }
                            }
                        }
                    }
                    if !panel.toggles.isEmpty {
                        Section {
                            ForEach(panel.toggles, id: \.id) { toggle in
                                Toggle(isOn: Binding(
                                    get: { toggle.on },
                                    set: { _ in Task { await host.run(toggle.action) } })) {
                                    Text(toggle.label)
                                    Text(toggle.help).foregroundStyle(.secondary)
                                }
                                .disabled(host.busy || !toggle.enabled)
                                .accessibilityIdentifier("host-toggle-\(toggle.id)")
                            }
                            if let limits = panel.limits {
                                Text(limits).foregroundStyle(.secondary).lineLimit(2)
                                    .accessibilityIdentifier("host-limits")
                            }
                        }
                    }
                    if let title = panel.providedTitle {
                        Section(title) {
                            if let empty = panel.providedEmpty {
                                Text(empty).foregroundStyle(.secondary)
                            }
                            TimedRows(rows: panel.provided)
                        }
                        .accessibilityIdentifier("host-provided")
                    }
                    if let warning = panel.accessWarning {
                        Text(warning).foregroundStyle(.red).lineLimit(2)
                            .accessibilityIdentifier("host-access-warning")
                    }
                    if let title = panel.clientsTitle {
                        Section(title) {
                            if let empty = panel.clientsEmpty {
                                Text(empty).foregroundStyle(.secondary)
                            }
                            ForEach(panel.clients, id: \.self) { Text($0).lineLimit(1) }
                        }
                    }
                    if let title = panel.recentTitle, !panel.recent.isEmpty {
                        Section(title) {
                            ForEach(Array(panel.recent.enumerated()), id: \.offset) { _, row in
                                LabeledContent {
                                    Text(Date(timeIntervalSince1970: Double(row.atMs) / 1000),
                                         format: .relative(presentation: .named))
                                        .foregroundStyle(.secondary)
                                } label: {
                                    Text(row.text).lineLimit(1).truncationMode(.middle)
                                }
                            }
                        }
                        .accessibilityIdentifier("host-recent-access")
                    }
                    if let warning = panel.activityWarning {
                        Text(warning).foregroundStyle(.red).lineLimit(2)
                            .accessibilityIdentifier("host-activity-warning")
                    }
                    if let title = panel.activityTitle, !panel.activity.isEmpty {
                        Section(title) { TimedRows(rows: panel.activity) }
                            .accessibilityIdentifier("host-activity")
                    }
                    if let title = panel.permissionsTitle {
                        Section(title) {
                            PermissionRows(rows: panel.permissions, openLabel: panel.openSettingsLabel)
                        }
                    }
                    if let error = host.error {
                        Text(error).foregroundStyle(.red).lineLimit(1).help(error)
                    }
                    if panel.setupChoices.isEmpty {
                        Section {
                            HStack {
                                ForEach(Array(panel.actions.enumerated()), id: \.offset) { index, action in
                                    let button = Button(action.label, role: action.destructive ? .destructive : nil) {
                                        if action.confirm != nil {
                                            confirming = action
                                        } else {
                                            Task { await host.run(action.id) }
                                        }
                                    }
                                    .disabled(host.busy)
                                    .accessibilityIdentifier("host-action-\(index)")
                                    if index == 0 {
                                        button.buttonStyle(.borderedProminent).tint(action.destructive ? .red : .accentColor)
                                    } else {
                                        button
                                    }
                                }
                            }
                        }
                    }
                }
                .formStyle(.grouped)
            }
        }
        .navigationTitle(host.form == nil ? panel.title : (host.formView?.title ?? panel.title))
        .task { await host.refresh() }
        .alert(confirming?.confirm?.title ?? "", isPresented: Binding(
            get: { confirming?.confirm != nil },
            set: { if !$0 { confirming = nil } })) {
            if let action = confirming, let confirm = action.confirm {
                Button(confirm.confirmLabel, role: .destructive) {
                    confirming = nil
                    Task { await host.run(action.id) }
                }
                Button(confirm.cancelLabel, role: .cancel) { confirming = nil }
            }
        } message: {
            Text(confirming?.confirm?.message ?? "")
        }
    }
}

/// Rows with a relative time after them (provided Spaces, activity).
struct TimedRows: View {
    let rows: [AppHostAccessRow]

    var body: some View {
        ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
            LabeledContent {
                Text(Date(timeIntervalSince1970: Double(row.atMs) / 1000),
                     format: .relative(presentation: .named))
                    .foregroundStyle(.secondary)
            } label: {
                Text(row.text).lineLimit(1).truncationMode(.middle).help(row.text)
            }
        }
    }
}

/// Permission panes to grant: the title, "Open Settings", the
/// instructions as the tooltip. Only System Settings privacy panes open.
struct PermissionRows: View {
    let rows: [AppPermissionRow]
    let openLabel: String

    var body: some View {
        ForEach(rows, id: \.id) { row in
            LabeledContent(row.title) {
                if let url = row.settingsUrl {
                    Button(openLabel) { Self.open(url) }
                }
            }
            .help(row.help)
        }
    }

    static func open(_ url: String) {
        guard url.hasPrefix("x-apple.systempreferences:"), let u = URL(string: url) else { return }
        NSWorkspace.shared.open(u)
    }
}

/// The host setup form (relay by default; a direct `ip:port` under
/// Advanced): the core's fields in order.
struct HostFormView: View {
    @Bindable var host: HostModel
    /// Draws Back and the submit button (onboarding draws its own).
    var buttons: Bool

    var body: some View {
        if let v = host.formView {
            Form {
                Section {
                    Text(v.lede).foregroundStyle(.secondary).lineLimit(1).help(v.lede)
                    ForEach(v.fields.filter { !$0.advanced }, id: \.id) { field($0) }
                    Button {
                        host.send(.toggleAdvanced)
                    } label: {
                        Label(v.advancedLabel, systemImage: v.advancedOpen ? "chevron.down" : "chevron.right")
                    }
                    .buttonStyle(.plain)
                    .foregroundStyle(.secondary)
                    .accessibilityIdentifier("host-advanced")
                    ForEach(v.fields.filter(\.advanced), id: \.id) { field($0) }
                    if let error = v.error {
                        Text(error).foregroundStyle(.red).lineLimit(1).help(error)
                    }
                }
                if buttons {
                    Section {
                        HStack {
                            Spacer()
                            Button(v.backLabel) { host.closeForm() }
                            Button(v.submitLabel) { Task { await host.submit() } }
                                .buttonStyle(.borderedProminent)
                                .keyboardShortcut(.defaultAction)
                                .disabled(!v.canSubmit)
                                .accessibilityIdentifier("host-setup")
                        }
                    }
                }
            }
            .formStyle(.grouped)
        }
    }

    @ViewBuilder private func field(_ f: AppHostFormField) -> some View {
        if !f.choices.isEmpty {
            Picker(f.label, selection: Binding(get: { f.value }, set: { host.send(.setProfile(profile: $0)) })) {
                ForEach(f.choices, id: \.id) { Text($0.label).tag($0.id) }
            }
            .pickerStyle(.radioGroup)
            .accessibilityIdentifier("host-\(f.id)")
        } else if f.toggle {
            Toggle(f.label, isOn: Binding(get: { f.on }, set: { host.send(.setDirect(on: $0)) }))
                .accessibilityIdentifier("host-\(f.id)")
        } else {
            TextField(f.label, text: Binding(get: { f.value }, set: { host.send(Self.action(f.id, $0)) }),
                      prompt: f.placeholder.map { Text($0) })
                .foregroundStyle(f.invalid ? Color.red : Color.primary)
                .accessibilityIdentifier("host-\(f.id)")
        }
    }

    static func action(_ id: String, _ text: String) -> AppHostFormAction {
        switch id {
        case "allow": return .setAllow(allow: text)
        case "listen": return .setListen(listen: text)
        case "relay": return .setRelayUrl(url: text)
        default: return .setName(name: text)
        }
    }
}
