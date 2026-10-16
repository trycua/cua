// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpacesFFI
import SwiftUI

/// The getting-started checklist: the core's rows (`appChecklistView`),
/// each step's button, and × to close it (Getting Started in the Help menu
/// or the command palette opens it again).
struct GettingStartedView: View {
    let model: AppModel

    var body: some View {
        let view = model.checklist.view
        Form {
            Section {
                ForEach(view.rows, id: \.step) { row in
                    ChecklistRowView(row: row) {
                        Task { await model.runChecklistStep(row.step) }
                    }
                }
            } header: {
                HStack(alignment: .firstTextBaseline, spacing: 10) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(view.title).font(.headline)
                        Text(view.detail).font(.callout).foregroundStyle(.secondary)
                    }
                    Spacer()
                    Text(view.progress)
                        .font(.callout.monospacedDigit())
                        .foregroundStyle(.secondary)
                        .accessibilityIdentifier("checklist-progress")
                    Button {
                        model.checklist.send(.dismiss)
                        model.selection = model.sidebar.selectedId.map { .space($0) }
                    } label: {
                        Image(systemName: "xmark")
                    }
                    .buttonStyle(.borderless)
                    .help(view.closeLabel)
                    .accessibilityLabel(view.closeLabel)
                    .accessibilityIdentifier("checklist-close")
                }
                .padding(.bottom, 4)
            }
        }
        .formStyle(.grouped)
        .navigationTitle(view.title)
        .onAppear { model.checklist.send(.shown) }
    }
}

/// One step: a tick when done, its title and line, and its button until done.
struct ChecklistRowView: View {
    let row: AppChecklistRow
    let action: () -> Void

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: row.done ? "checkmark.circle.fill" : "circle")
                .font(.title3)
                .foregroundStyle(row.done ? Color.accentColor : Color.secondary)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 2) {
                Text(row.title)
                    .strikethrough(row.done)
                    .foregroundStyle(row.done ? .secondary : .primary)
                Text(row.detail)
                    .font(.callout)
                    .foregroundStyle(.secondary)
            }
            Spacer(minLength: 8)
            if !row.done {
                Button(row.action, action: action)
            }
        }
        .accessibilityElement(children: .combine)
        .accessibilityValue(row.done ? "Done" : "")
        .accessibilityIdentifier("checklist-step")
    }
}
