// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpacesFFI
import SwiftUI

/// Machines: This machine, your other machines and your devices, each
/// computer once; the selected one's page beside them (This machine's own
/// page, a machine's detail with the Spaces it provides, or a device's
/// page). The rows are the core's (`AppModel.machinePageRows`,
/// `machines::page_rows`), the same the New UI lists.
struct MachinesPageView: View {
    @Bindable var model: AppModel
    @State private var selectedId: String?

    var body: some View {
        let rows = model.machinePageRows
        let selected = rows.first { $0.id == selectedId } ?? rows.first
        HStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 0) {
                List(selection: $selectedId) {
                    ForEach(rows, id: \.id) { row in
                        MachineRowView(row: row, status: status(row), spaces: spacesCount(row))
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
                if let row = selected {
                    if row.current {
                        ThisMachineView(host: model.host)
                    } else if let id = row.spaceId, let space = model.spaces.first(where: { $0.id == id }) {
                        SpaceDetailView(model: model, space: space)
                            .id(space.id)
                    } else {
                        MachineInfoView(model: model, row: row, status: status(row))
                            .id(row.id)
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

    /// The row's second line: the sidebar's status for a machine with a
    /// desktop row, the device's platform, state and last session for a
    /// device of its own, else whether it is online.
    private func status(_ row: AppMachinePageRow) -> String {
        if let id = row.spaceId,
           let s = model.sidebar.sections.flatMap(\.rows).first(where: { $0.id == id }) ?? model.sidebar.thisMachine.flatMap({ $0.id == id ? $0 : nil }) {
            return s.statusText
        }
        if row.deviceOnly, let d = model.devices.view.rows.first(where: { $0.id == row.device }) {
            return model.devices.rowDetail(d)
        }
        return row.online ? "Online" : "Offline"
    }

    /// The Spaces a machine provides (none for this machine or a device).
    private func spacesCount(_ row: AppMachinePageRow) -> Int? {
        guard !row.current, !row.deviceOnly else { return nil }
        return model.hostedRows(row.spaceId ?? "relay:\(row.id)").count
    }
}

/// The OS mark for a page row's system (`os-macos`, ..., `computer`).
private func osIcon(_ os: String) -> String { os.isEmpty ? "computer" : "os-\(os)" }

/// One machine: its OS mark, name, status line and how many Spaces it runs.
struct MachineRowView: View {
    let row: AppMachinePageRow
    let status: String
    /// Spaces it provides (nil for This machine and a device).
    let spaces: Int?

    var body: some View {
        HStack(spacing: 10) {
            OsIconImage(id: osIcon(row.os), size: 16)
                .foregroundStyle(.secondary)
                .frame(width: 30, height: 30)
                .background(.fill.tertiary, in: .rect(cornerRadius: 7))
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 4) {
                    Text(row.name).lineLimit(1)
                    if row.current {
                        Text("(this machine)").foregroundStyle(.secondary)
                    }
                }
                Text(status)
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
        .opacity(row.online ? 1 : 0.6)
        .accessibilityIdentifier("machine-row")
    }
}

/// A machine without a desktop row here: one of your devices that is not
/// set up for access, or a machine known only by the Spaces it provides.
/// Its name, whether it is online, its last session, the Spaces it
/// provides, and Revoke for a device.
struct MachineInfoView: View {
    @Bindable var model: AppModel
    let row: AppMachinePageRow
    let status: String
    @State private var revoking: AppDeviceRow?

    var body: some View {
        let device = model.devices.view.rows.first { $0.id == row.device }
        let hosted = row.deviceOnly ? [] : model.hostedRows("relay:\(row.id)")
        Form {
            Section {
                LabeledContent("Status", value: row.online ? "Online" : "Offline")
                if !row.os.isEmpty {
                    LabeledContent("System", value: device?.platform.isEmpty == false ? device!.platform : row.os)
                }
                if let seen = row.lastSeen {
                    LabeledContent("Last seen", value: model.devices.relativeText(seen))
                }
                if row.deviceOnly {
                    LabeledContent("Access", value: "Not set up for access")
                }
            } header: {
                HStack(spacing: 10) {
                    OsIconImage(id: osIcon(row.os), size: 18)
                        .foregroundStyle(.secondary)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(row.name).font(.title3.weight(.semibold))
                        Text(status).font(.callout).foregroundStyle(.secondary)
                    }
                }
                .padding(.bottom, 6)
            }
            if !hosted.isEmpty {
                Section("Spaces") {
                    ForEach(hosted, id: \.id) { s in
                        LabeledContent(s.name, value: s.statusText)
                    }
                }
            }
            if row.deviceOnly {
                Section {
                    Text("To run Spaces or share its desktop, run `cua host setup` on it.")
                        .foregroundStyle(.secondary)
                }
            }
            if let device, device.actions.contains(.revoke) {
                Section {
                    Button(model.devices.view.labels.revoke, role: .destructive) { revoking = device }
                        .disabled(model.devices.busy)
                        .accessibilityIdentifier("machine-revoke")
                }
            }
        }
        .formStyle(.grouped)
        .accessibilityIdentifier("machine-info")
        .alert(revoking?.revokeConfirm?.title ?? "", isPresented: Binding(get: { revoking?.revokeConfirm != nil },
                                                                          set: { if !$0 { revoking = nil } })) {
            if let r = revoking, let c = r.revokeConfirm {
                Button(c.confirmLabel, role: .destructive) {
                    revoking = nil
                    Task { await model.devices.revoke(id: r.id) }
                }
                Button(c.cancelLabel, role: .cancel) { revoking = nil }
            }
        } message: {
            Text(revoking?.revokeConfirm?.message ?? "")
        }
    }
}
