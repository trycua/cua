// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpacesFFI
import SwiftUI

/// Machines: This machine and your other machines, each with its Spaces;
/// the selected one's page beside them (This machine's own page, or the
/// machine's detail with the Spaces it provides and Remove). The rows are
/// the core's sidebar rows (`AppModel.machineRows`).
struct MachinesPageView: View {
    @Bindable var model: AppModel
    @State private var selectedId: String?

    var body: some View {
        let rows = model.machineRows
        let selected = rows.first { $0.id == selectedId } ?? rows.first
        HStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 0) {
                List(selection: $selectedId) {
                    ForEach(rows, id: \.id) { row in
                        MachineRowView(row: row, isThisMachine: isThisMachine(row),
                                       spaces: isThisMachine(row) ? nil : model.hostedRows(row.id).count)
                            .tag(row.id)
                    }
                }
                .listStyle(.inset)
                .accessibilityIdentifier("machines-list")
                if rows.count <= 1 {
                    Text("Only this machine so far. Add another to run Spaces on it from here.")
                        .font(.callout)
                        .foregroundStyle(.secondary)
                        .padding(12)
                }
            }
            .frame(width: 300)
            Divider()
            Group {
                if let row = selected, let space = model.spaces.first(where: { $0.id == row.id }) {
                    if isThisMachine(row) {
                        ThisMachineView(host: model.host)
                    } else {
                        SpaceDetailView(model: model, space: space)
                            .id(space.id)
                    }
                } else {
                    ContentUnavailableView {
                        Label("No machines yet", systemImage: "server.rack")
                    } description: {
                        Text("Set up a Mac, Linux or Windows computer for access and it shows here, ready to run Spaces.")
                    }
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .navigationTitle("Machines")
        .toolbar {
            ToolbarItem(placement: .primaryAction) {
                Button {
                    Task { await model.openAddMachine() }
                } label: {
                    Label("Add a machine", systemImage: "plus.rectangle.on.rectangle")
                }
                .help("Add a machine")
                .accessibilityIdentifier("machines-add")
            }
        }
    }

    private func isThisMachine(_ row: AppSidebarRow) -> Bool {
        model.spaces.first { $0.id == row.id }.map { model.detail($0).isHost } ?? false
    }
}

/// One machine: its OS mark, name, status word and how many Spaces it runs.
struct MachineRowView: View {
    let row: AppSidebarRow
    let isThisMachine: Bool
    /// Spaces it provides (nil for This machine).
    let spaces: Int?

    var body: some View {
        HStack(spacing: 10) {
            OsIconImage(id: row.osIcon, size: 16)
                .foregroundStyle(.secondary)
                .frame(width: 30, height: 30)
                .background(.fill.tertiary, in: .rect(cornerRadius: 7))
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 4) {
                    Text(row.name).lineLimit(1)
                    if isThisMachine {
                        Text("(this machine)").foregroundStyle(.secondary)
                    }
                }
                Text(row.statusText)
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            Spacer(minLength: 4)
            if let spaces {
                Text(spaces == 1 ? "1 Space" : "\(spaces) Spaces")
                    .font(.callout.monospacedDigit())
                    .foregroundStyle(.secondary)
            }
        }
        .padding(.vertical, 3)
        .opacity(row.dim ? 0.6 : 1)
        .help(row.detail)
        .accessibilityIdentifier("machine-row")
    }
}
