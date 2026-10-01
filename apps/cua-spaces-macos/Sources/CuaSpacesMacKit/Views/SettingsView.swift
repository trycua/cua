// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import CuaSpacesFFI
import SwiftUI

/// Settings: the core's page (Account, General, Privacy, AI agents) with
/// the core's Storage section after General while the Cua Volume
/// experiment is on, each row one line. What a row does is wired in
/// `AppModel.press` / `choose`, and for Storage in `StorageModel`.
struct SettingsView: View {
    @Bindable var model: AppModel

    var body: some View {
        let storage = model.storage.section
        let sections = model.settingsPageWithStorage.sections
        ScrollViewReader { proxy in
            Form {
                ForEach(sections, id: \.id) { section in
                    Section {
                        ForEach(section.rows, id: \.id) { row($0, in: section.id) }
                    } header: {
                        HStack {
                            Text(section.title)
                            Spacer()
                            if let button = section.button {
                                Button(button) { Task { await headerButton(section.id) } }
                                    .controlSize(.small)
                                    .disabled(!section.buttonEnabled)
                                    .help(section.buttonHelp ?? "")
                            }
                        }
                    }
                    .id(section.id)
                }
            }
            .formStyle(.grouped)
            .task(id: "\(model.settingsSection ?? "")/\(storage.rows.count)") {
                guard let id = model.settingsSection else { return }
                try? await Task.sleep(for: .milliseconds(600))
                proxy.scrollTo(id, anchor: .top)
            }
        }
        .frame(width: 520)
        .frame(minHeight: 480)
        .task { await model.loadSettings() }
        .background { watchTask }
        // While the mount settles (mounting, or waiting for the extension's
        // approval in System Settings), follow it.
        .task(id: Self.settling(storage)) {
            while Self.settling(model.storage.section), !Task.isCancelled {
                try? await Task.sleep(for: .seconds(2))
                await model.storage.load()
            }
        }
    }

    /// The agent prompt shows: poll for the bucket it sets up.
    private var watchTask: some View {
        Color.clear.frame(width: 0, height: 0)
            .task(id: model.storage.watching) {
                while model.storage.watching, !Task.isCancelled {
                    try? await Task.sleep(for: .seconds(2))
                    await model.storage.load()
                }
            }
    }

    /// The mount is on its way: mounting, or waiting for approval.
    static func settling(_ s: AppSettingsSection) -> Bool {
        s.rows.contains { $0.id == "mount-approval" || ($0.id == "mount-path" && $0.button == nil) }
    }

    private func headerButton(_ section: String) async {
        switch section {
        case "storage": await model.storage.send(.save)
        default: await model.configureAllAgents()
        }
    }

    private func press(_ r: AppSettingsRow, in section: String) {
        Task {
            if section == "storage" { await model.storage.press(r.id) } else { await model.press(row: r.id) }
        }
    }

    private func choose(_ r: AppSettingsRow, _ option: String, in section: String) {
        Task {
            if section == "storage" {
                await model.storage.choose(r.id, option)
            } else {
                await model.choose(row: r.id, option: option)
            }
        }
    }

    @ViewBuilder private func row(_ r: AppSettingsRow, in section: String) -> some View {
        switch r.kind {
        case .text:
            LabeledContent {
                HStack(spacing: 8) {
                    if let value = r.value {
                        Text(value).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
                    }
                    if let button = r.button {
                        Button(button) { press(r, in: section) }
                            .disabled(!r.enabled)
                    }
                }
            } label: {
                Text(r.label).lineLimit(1)
            }
            .opacity(r.enabled || r.button != nil ? 1 : 0.5)
            .help(r.help ?? "")
        case .choice:
            Picker(r.label, selection: Binding(
                get: { r.options.first(where: \.active)?.id ?? "" },
                set: { id in choose(r, id, in: section) })) {
                ForEach(r.options, id: \.id) { Text($0.label).tag($0.id) }
            }
            .pickerStyle(.segmented)
            .disabled(!r.enabled)
            .help(r.help ?? "")
        case .toggle:
            Toggle(r.label, isOn: Binding(
                get: { r.options.first(where: { $0.id == "on" })?.active ?? false },
                set: { choose(r, $0 ? "on" : "off", in: section) }))
                .disabled(!r.enabled)
                .help(r.help ?? "")
                .accessibilityIdentifier("settings-\(r.id)")
        case .field:
            TextField(r.label, text: Binding(get: { r.value ?? "" }, set: { model.storage.edit(r.id, $0) }),
                      prompt: r.placeholder.map { Text($0) })
                .disabled(!r.enabled)
                .accessibilityIdentifier("settings-\(r.id)")
        case .secret:
            SecureField(r.label, text: Binding(get: { r.value ?? "" }, set: { model.storage.edit(r.id, $0) }),
                        prompt: r.placeholder.map { Text($0) })
                .disabled(!r.enabled)
                .accessibilityIdentifier("settings-\(r.id)")
        case .prompt:
            PromptBox(label: r.label, text: r.value ?? "", copyLabel: r.button ?? "Copy")
        case .link:
            Button(r.label) { press(r, in: section) }
                .buttonStyle(.link)
                .disabled(!r.enabled)
                .accessibilityIdentifier("settings-\(r.id)")
        case .note:
            HStack(spacing: 4) {
                Text(r.label).font(.callout).foregroundStyle(.secondary).lineLimit(1).help(r.label)
                if let label = r.linkLabel, let url = r.linkUrl.flatMap(URL.init(string:)) {
                    Link(label, destination: url).font(.callout)
                }
            }
        case .error:
            Text(r.label).foregroundStyle(.red).lineLimit(1).help(r.label)
        }
    }
}
