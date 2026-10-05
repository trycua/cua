// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import CuaSpacesFFI
import SwiftUI

/// One line: text, trailing text, up to two buttons and a switch.
struct LineRow: View {
    let line: AppLineView
    var onOpen: (() -> Void)?
    var onAction: (() -> Void)?
    var onSecondary: (() -> Void)?
    var onToggle: (() -> Void)?

    var body: some View {
        HStack(spacing: 8) {
            if let onOpen {
                Button(line.text, action: onOpen).buttonStyle(.link).lineLimit(1)
            } else {
                Text(line.text).lineLimit(1)
            }
            Spacer(minLength: 8)
            Text(line.trailing).foregroundStyle(.secondary).lineLimit(1)
            if let on = line.on, let onToggle {
                Toggle(line.text, isOn: Binding(get: { on }, set: { _ in onToggle() }))
                    .labelsHidden().toggleStyle(.switch).controlSize(.mini)
            }
            if let label = line.actionLabel, let onAction { Button(label, action: onAction).controlSize(.small) }
            if let label = line.secondaryLabel, let onSecondary { Button(label, action: onSecondary).controlSize(.small) }
        }
    }
}

/// "Agents": persistent agents and the selected one's memory, routines and
/// access to this computer. Every word is the app core's.
struct AgentsPageView: View {
    @Bindable var model: PersistentModel

    var body: some View {
        let v = model.agentsView()
        Form {
            Section(v.title) {
                if v.rows.isEmpty { Text(v.emptyText).foregroundStyle(.secondary) }
                ForEach(v.rows, id: \.name) { r in
                    HStack {
                        Button(r.name) { act(.select(name: r.name)) }.buttonStyle(.link)
                        Spacer()
                        Text("\(r.detail), \(r.state)").foregroundStyle(.secondary).lineLimit(1)
                        Button(r.actionLabel) { act(r.actionLabel == "Resume" ? .resume(name: r.name) : .pause(name: r.name)) }
                            .controlSize(.small)
                    }
                }
            }
            if let d = v.detail {
                Section {
                    Picker(d.name, selection: Binding(get: { d.tabs.first(where: \.selected)?.tab ?? .memory },
                                                      set: { act(.setTab(tab: $0)) })) {
                        ForEach(d.tabs, id: \.label) { Text($0.label).tag($0.tab) }
                    }
                    .pickerStyle(.segmented)
                    .labelsHidden()
                } header: {
                    Text("\(d.name), \(d.subtitle)")
                }
                switch d.tabs.first(where: \.selected)?.tab ?? .memory {
                case .memory: memory(d)
                case .routines: routines(d)
                case .access: access(d)
                }
            }
            if let e = v.error { Text(e).foregroundStyle(.red) }
        }
        .formStyle(.grouped)
        .task { await model.loadAgents() }
    }

    private func act(_ a: AppAgentsAction) { Task { await model.send(a) } }

    @ViewBuilder private func memory(_ d: AppAgentDetailView) -> some View {
        if let f = d.file {
            Section(f.path) {
                ScrollView { Text(f.text).font(.system(.body, design: .monospaced)).textSelection(.enabled) }
                    .frame(maxHeight: 240)
                ForEach(f.versions, id: \.id) { l in LineRow(line: l, onAction: { act(.restore(version: l.id)) }) }
                Button(f.closeLabel) { act(.closeFile) }
            }
        } else {
            Section {
                if d.memory.isEmpty { Text(d.memoryEmpty).foregroundStyle(.secondary) }
                ForEach(d.memory, id: \.id) { l in LineRow(line: l, onOpen: { act(.openFile(path: l.id)) }) }
            }
        }
    }

    @ViewBuilder private func routines(_ d: AppAgentDetailView) -> some View {
        Section {
            if d.routines.isEmpty { Text(d.routinesEmpty).foregroundStyle(.secondary) }
            ForEach(d.routines, id: \.id) { l in
                LineRow(line: l, onAction: { act(.removeRoutine(id: l.id)) }, onToggle: { act(.toggleRoutine(id: l.id)) })
            }
        }
        Section {
            TextField("Title", text: Binding(get: { d.form.title }, set: { act(.setTitle(title: $0)) }))
            TextField("Prompt", text: Binding(get: { d.form.prompt }, set: { act(.setPrompt(prompt: $0)) }))
            Picker("Schedule", selection: Binding(get: { d.form.schedule }, set: { act(.setSchedule(schedule: $0)) })) {
                Text("Every N minutes").tag(AppRoutineScheduleKind.every)
                Text("Every day").tag(AppRoutineScheduleKind.daily)
                Text("Every week").tag(AppRoutineScheduleKind.weekly)
            }
            if d.form.schedule == .every {
                Stepper("\(d.form.minutes) minutes", value: Binding(get: { Int(d.form.minutes) },
                                                                   set: { act(.setMinutes(minutes: UInt32(max(1, $0)))) }), in: 1...1440)
            } else {
                TextField("Time", text: Binding(get: { d.form.time }, set: { act(.setTime(time: $0)) }))
            }
            if d.form.schedule == .weekly {
                Picker("Day", selection: Binding(get: { d.form.weekday }, set: { act(.setWeekday(weekday: $0)) })) {
                    ForEach(["mon", "tue", "wed", "thu", "fri", "sat", "sun"], id: \.self) { Text($0).tag($0) }
                }
            }
            Button(d.addRoutineLabel) { act(.addRoutine) }.disabled(!d.canAddRoutine)
        }
    }

    @ViewBuilder private func access(_ d: AppAgentDetailView) -> some View {
        Section {
            if d.access.isEmpty { Text(d.accessEmpty).foregroundStyle(.secondary) }
            ForEach(d.access, id: \.id) { l in LineRow(line: l, onAction: { act(.revokeComputer(machine: l.id)) }) }
            if let label = d.allowThisMachineLabel, let machine = model.thisMachine {
                Button(label) { act(.allowComputer(machine: machine)) }
            }
        }
        if !d.audit.isEmpty {
            Section { ForEach(d.audit, id: \.id) { LineRow(line: $0) } }
        }
    }
}

/// "Volume": Open in Finder, sync per device, conflicts, requests and
/// grants. Not a file browser: Finder is.
struct DrivePageView: View {
    @Bindable var model: PersistentModel

    var body: some View {
        let v = model.driveView()
        Form {
            if let label = v.openLabel {
                Section {
                    HStack(spacing: 12) {
                        if let line = v.mountLine {
                            Text(line).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
                                .help(v.mountPath ?? line)
                        }
                        Spacer()
                        Button(label) { act(.openVolume(mounted: v.mountPath)) }
                            .buttonStyle(.borderedProminent)
                            .controlSize(.large)
                            .disabled(v.busy)
                            .accessibilityIdentifier("drive-open-in-finder")
                    }
                }
            }
            if !v.requests.isEmpty {
                Section(v.requestsTitle) {
                    ForEach(v.requests, id: \.id) { l in
                        LineRow(line: l, onAction: { act(.approve(id: l.id)) }, onSecondary: { act(.deny(id: l.id)) })
                    }
                }
            }
            Section(v.grantsTitle) {
                if v.grants.isEmpty { Text(v.grantsEmpty).foregroundStyle(.secondary) }
                ForEach(v.grants, id: \.id) { l in LineRow(line: l, onAction: { act(.revoke(id: l.id)) }) }
            }
            if !v.devices.isEmpty || v.syncNote != nil {
                Section(v.devicesTitle) {
                    ForEach(v.devices, id: \.id) { LineRow(line: $0) }
                    if let note = v.syncNote {
                        Text(note).foregroundStyle(v.syncError ? AnyShapeStyle(.red) : AnyShapeStyle(.secondary))
                            .lineLimit(1).help(note)
                    }
                }
            }
            if !v.conflicts.isEmpty {
                Section(v.conflictsTitle) {
                    ForEach(v.conflicts, id: \.path) { c in
                        HStack(spacing: 8) {
                            Text(c.text).lineLimit(1).truncationMode(.middle)
                            Spacer(minLength: 8)
                            Text(c.trailing).foregroundStyle(.secondary).lineLimit(1)
                            if let label = c.openLabel, let path = c.reveal {
                                Button(label) { act(.reveal(path: path)) }.controlSize(.small)
                            }
                            Button(c.resolveLabel) { act(.resolve(path: c.path)) }.controlSize(.small)
                        }
                    }
                }
            }
            if let e = v.error { Text(e).foregroundStyle(.red) }
        }
        .formStyle(.grouped)
        .task { await model.sendDrive(nil) }
        // The sync status changes on its own (other devices, uploads).
        .task {
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(5))
                await model.refreshDriveSync()
            }
        }
    }

    private func act(_ a: AppDriveAction) { Task { await model.sendDrive(a) } }
}

/// "Notifications": the daemon's feed, one line each.
struct NotificationsPageView: View {
    @Bindable var model: PersistentModel

    var body: some View {
        let v = model.notificationsView()
        Form {
            Section(v.title) {
                if v.rows.isEmpty { Text(v.emptyText).foregroundStyle(.secondary) }
                ForEach(v.rows, id: \.id) { l in
                    HStack {
                        Text(l.text).lineLimit(1).fontWeight(l.on == true ? .semibold : .regular)
                        Spacer()
                        Text(l.trailing).foregroundStyle(.secondary)
                    }
                }
                if let label = v.markAllLabel { Button(label) { Task { await model.markAllRead() } } }
            }
        }
        .formStyle(.grouped)
    }
}
