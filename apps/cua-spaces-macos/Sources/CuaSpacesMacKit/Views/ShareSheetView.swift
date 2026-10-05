// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import CuaSpacesFFI
import SwiftUI

/// "Share": one line per person with a role menu, an email field and Done.
/// Every word is the app core's.
struct ShareSheetView: View {
    @Bindable var model: ShareModel
    let onDone: () -> Void

    var body: some View {
        let v = model.view
        Form {
            if let reason = v.disabledReason {
                Text(reason).foregroundStyle(.secondary)
            }
            Section {
                HStack {
                    TextField(v.whoPlaceholder, text: Binding(
                        get: { v.who },
                        set: { who in Task { await model.send(.setWho(who: who)) } }))
                        .textFieldStyle(.roundedBorder)
                        .onSubmit { Task { await model.send(.submit) } }
                    roleMenu(v, selection: v.role) { role in Task { await model.send(.setRole(role: role)) } }
                    Button(v.shareLabel) { Task { await model.send(.submit) } }
                        .disabled(!v.canShare)
                        .keyboardShortcut(.defaultAction)
                }
                if let hint = v.hint { Text(hint).foregroundStyle(.secondary) }
            }
            Section {
                if v.rows.isEmpty { Text(v.emptyText).foregroundStyle(.secondary) }
                ForEach(v.rows, id: \.who) { row in
                    HStack {
                        Circle().fill(row.connected ? Color.green : Color.secondary.opacity(0.4)).frame(width: 7, height: 7)
                        Text(row.who).lineLimit(1).truncationMode(.middle)
                        Spacer()
                        roleMenu(v, selection: row.role) { role in
                            Task { await model.send(.changeRole(who: row.who, role: role)) }
                        }
                        Button(v.removeLabel) { Task { await model.send(.remove(who: row.who)) } }
                            .disabled(v.busy)
                    }
                }
            }
            if let error = v.error { Text(error).foregroundStyle(.red) }
        }
        .formStyle(.grouped)
        .navigationTitle(v.title)
        .frame(minWidth: 440, minHeight: 280)
        .toolbar {
            ToolbarItem(placement: .confirmationAction) { Button(v.doneLabel, action: onDone) }
        }
        .task { await model.load() }
    }

    private func roleMenu(_ v: AppShareSheetView, selection: String, _ change: @escaping (String) -> Void) -> some View {
        Picker("", selection: Binding(get: { selection }, set: change)) {
            ForEach(v.roles, id: \.id) { Text($0.label).tag($0.id) }
        }
        .labelsHidden()
        .fixedSize()
        .disabled(v.busy || v.disabledReason != nil)
    }
}
