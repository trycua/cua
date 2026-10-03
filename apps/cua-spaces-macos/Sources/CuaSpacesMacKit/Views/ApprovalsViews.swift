// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpacesFFI
import SwiftUI

/// Settings → Permissions: one switch per thing an agent can do, on to
/// make it ask for a fingerprint first; the gates that always ask are listed
/// without a switch. Every word is the core's.
struct PermissionsSettingsView: View {
    @Bindable var model: ApprovalsModel

    var body: some View {
        let view = model.view
        Form {
            if let notice = view.notice {
                Section {
                    Label(notice, systemImage: "exclamationmark.triangle")
                        .foregroundStyle(.orange)
                        .accessibilityIdentifier("permissions-notice")
                }
            }
            Section {
                ForEach(view.rows, id: \.id) { row in
                    Toggle(isOn: Binding(
                        get: { model.requires(row) },
                        set: { value in Task { await model.set(row, to: value) } })) {
                        Text(row.title)
                        Text(row.detail)
                    }
                    .disabled(model.pending[row.id] != nil)
                    .accessibilityIdentifier("permission-\(row.id)")
                }
            } header: {
                Text(view.intro).font(.body).foregroundStyle(.secondary).textCase(nil)
            } footer: {
                if let error = model.error {
                    Text(error).foregroundStyle(.red).accessibilityIdentifier("permissions-error")
                }
            }
            Section(view.lockedTitle) {
                ForEach(view.locked, id: \.title) { gate in
                    LabeledContent {
                        Text(gate.detail)
                    } label: {
                        Label(gate.title, systemImage: "lock")
                    }
                    .accessibilityIdentifier("permission-locked")
                }
            }
        }
        .formStyle(.grouped)
        .frame(width: 520, height: view.notice == nil ? 790 : 860)
    }
}
