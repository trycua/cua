// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpacesFFI
import SwiftUI

/// "Connect a cloud": one row per provider (a check mark when this Mac has
/// its sign-in) plus "No cloud", the region, project or environment, what
/// the selected provider touches, Test (creates nothing) and Connect. Every
/// word and decision is the core's (`appCloudConnectView`); this sheet
/// renders it.
public struct ConnectCloudSheet: View {
    @Bindable var cloud: CloudModel

    public init(cloud: CloudModel) {
        self.cloud = cloud
    }

    public var body: some View {
        let v = cloud.view
        VStack(spacing: 0) {
            Form {
                Section {
                    Picker(v.title, selection: Binding(
                        get: { v.rows.first(where: \.selected)?.id ?? "" },
                        set: { id in Task { await cloud.send(.select(name: id)) } })) {
                        ForEach(v.rows, id: \.id) { row in
                            HStack(spacing: 6) {
                                Text(row.title)
                                Text(row.detail).foregroundStyle(.secondary)
                                if row.found {
                                    Image(systemName: "checkmark").foregroundStyle(.secondary)
                                        .accessibilityLabel("Sign-in found")
                                }
                            }
                            .tag(row.id)
                        }
                    }
                    .pickerStyle(.radioGroup)
                    .labelsHidden()
                    .accessibilityIdentifier("cloud-providers")
                }
                if v.field != nil {
                    Section {
                        if let f = v.field { text(f) { await cloud.send(.setValue(text: $0)) } }
                        if let f = v.profileField { text(f) { await cloud.send(.setProfile(text: $0)) } }
                        Toggle(v.makeDefaultLabel, isOn: Binding(
                            get: { v.makeDefault },
                            set: { on in Task { await cloud.send(.setMakeDefault(on: on)) } }))
                    }
                }
                if !v.touches.isEmpty {
                    Section("What Cua will touch") {
                        ForEach(v.touches, id: \.self) { line in
                            Text(line).font(.callout).foregroundStyle(.secondary)
                        }
                    }
                }
                if !v.checks.isEmpty || v.result != nil {
                    Section {
                        ForEach(v.checks, id: \.text) { check in
                            Label(check.text, systemImage: check.ok ? "checkmark.circle" : "xmark.circle")
                                .foregroundStyle(check.ok ? Color.primary : Color.red)
                        }
                        if let result = v.result {
                            Text(result).foregroundStyle(.secondary)
                                .accessibilityIdentifier("cloud-test-result")
                        }
                    }
                }
                if let error = v.error {
                    Text(error).foregroundStyle(.red).font(.callout)
                }
            }
            .formStyle(.grouped)
            Divider()
            HStack {
                Button(v.cancelLabel) { cloud.showing = false }
                    .keyboardShortcut(.cancelAction)
                Spacer()
                Button(v.testLabel) { Task { await cloud.send(.test) } }
                    .disabled(!v.canTest)
                    .help(v.testHelp)
                Button(v.connectLabel) { Task { await cloud.send(.connect) } }
                    .keyboardShortcut(.defaultAction)
                    .disabled(!v.canConnect)
            }
            .padding(14)
        }
        .frame(width: 480, height: 440)
        .navigationTitle(v.title)
    }

    private func text(_ f: AppCloudField, _ set: @escaping (String) async -> Void) -> some View {
        TextField(f.label, text: Binding(get: { f.value }, set: { v in Task { await set(v) } }),
                  prompt: Text(f.placeholder))
            .accessibilityIdentifier("cloud-field-\(f.id)")
    }
}
