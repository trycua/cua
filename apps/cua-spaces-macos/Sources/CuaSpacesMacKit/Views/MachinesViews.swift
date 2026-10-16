// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpacesFFI
import SwiftUI

/// Machines: the hardware. The list in groups (your machines, those
/// offline for 30+ days, team machines) with one line each, and the
/// selected machine's page: its two switches, the Spaces on it as links,
/// New Space here, Details and Remove from account. Everything shown is
/// the core's `machines_view::view` (`AppModel.machinesView`), the same the
/// New UI draws. A Space's own actions (open, start, stop, teleport, drop
/// files) stay on the Space.
struct MachinesPageView: View {
    @Bindable var model: AppModel
    @State private var selectedId: String?

    /// `selected`: the machine whose page shows first (default: the first).
    init(model: AppModel, selected: String? = nil) {
        self.model = model
        _selectedId = State(initialValue: selected)
    }
    @State private var query = ""
    @State private var oldOpen = false
    @State private var removingOld: AppMachinesViewRemoveAll?
    @State private var failure: String?

    var body: some View {
        let view = model.machinesView
        let selected = view.machines.first { $0.id == selectedId } ?? view.machines.first
        HStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 0) {
                searchField
                List(selection: $selectedId) {
                    ForEach(view.groups, id: \.id) { group in
                        groupSection(group, view: view)
                    }
                }
                .listStyle(.sidebar)
                .accessibilityIdentifier("machines-list")
            }
            .frame(width: 300)
            Divider()
            Group {
                if let machine = selected {
                    MachinePage(model: model, machine: machine, onError: { failure = $0 })
                        .id(machine.id)
                } else {
                    ContentUnavailableView {
                        Label("No machines yet", systemImage: "server.rack")
                    } description: {
                        Text("Set up a Mac, Linux or Windows computer and it shows here, ready to run Spaces.")
                    }
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .navigationTitle("Machines")
        .navigationSubtitle(view.machines.isEmpty ? "" : view.summary)
        // Each machine's own daemon checks its health while the page shows
        // (This Mac, and your other machines that are online).
        .task {
            while !Task.isCancelled {
                await model.checkMachineHealth()
                try? await Task.sleep(for: .seconds(60))
            }
        }
        .toolbar {
            ToolbarItem(placement: .primaryAction) {
                Button {
                    Task { await model.openAddMachine() }
                } label: {
                    Label("Add a machine", systemImage: "plus")
                }
                .help("Add a machine")
                .accessibilityIdentifier("machines-add")
            }
        }
        .alert(removingOld?.confirm.title ?? "", isPresented: Binding(
            get: { removingOld != nil }, set: { if !$0 { removingOld = nil } })) {
            if let all = removingOld {
                Button(all.confirm.confirmLabel, role: .destructive) {
                    removingOld = nil
                    Task {
                        for r in all.removes {
                            do {
                                try await model.removeMachine(machine: r.machine, device: r.device)
                            } catch {
                                failure = LiveSpacesBackend.words(error)
                                return
                            }
                        }
                    }
                }
                Button(all.confirm.cancelLabel, role: .cancel) { removingOld = nil }
            }
        } message: {
            Text(removingOld?.confirm.message ?? "")
        }
        .alert("Couldn\u{2019}t change the machine", isPresented: Binding(
            get: { failure != nil }, set: { if !$0 { failure = nil } })) {
            Button("OK") { failure = nil }
        } message: {
            Text(failure ?? "")
        }
    }

    private var searchField: some View {
        HStack(spacing: 6) {
            Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
            TextField("Search machines", text: $query)
                .textFieldStyle(.plain)
                .accessibilityIdentifier("machines-search")
        }
        .padding(.horizontal, 8)
        .frame(height: 28)
        .background(.background, in: .rect(cornerRadius: 7))
        .overlay(RoundedRectangle(cornerRadius: 7).strokeBorder(.separator))
        .padding(.horizontal, 12)
        .padding(.top, 12)
        .padding(.bottom, 4)
    }

    private func matches(_ m: AppMachinesViewMachine) -> Bool {
        let q = query.trimmingCharacters(in: .whitespaces).lowercased()
        return q.isEmpty || "\(m.name) \(m.line)".lowercased().contains(q)
    }

    @ViewBuilder
    private func groupSection(_ group: AppMachinesViewGroup, view: AppMachinesView) -> some View {
        let rows = group.ids.compactMap { id in view.machines.first { $0.id == id } }.filter(matches)
        if !rows.isEmpty {
            let open = !group.collapsed || oldOpen || !query.isEmpty
            Section {
                if open {
                    ForEach(rows, id: \.id) { m in
                        MachineRowView(machine: m).tag(m.id)
                    }
                }
            } header: {
                if group.collapsed {
                    HStack {
                        Button {
                            oldOpen.toggle()
                        } label: {
                            Label(group.title, systemImage: open ? "chevron.down" : "chevron.right")
                        }
                        .buttonStyle(.plain)
                        .accessibilityIdentifier("machines-group-\(group.id)")
                        Spacer()
                        if let all = group.removeAll {
                            Button(all.label) { removingOld = all }
                                .buttonStyle(.link)
                                .font(.caption)
                                .accessibilityIdentifier("machines-remove-old")
                        }
                    }
                } else {
                    Text(group.title).accessibilityIdentifier("machines-group-\(group.id)")
                }
            }
        }
    }
}

/// The OS mark for a machine's system (`os-macos`, ..., `computer`).
private func osIcon(_ os: String) -> String { os.isEmpty ? "computer" : "os-\(os)" }

/// The green dot of a machine that can be used now, grey otherwise;
/// orange or red while it has issues (`health` `warn`, `fail`).
private struct StatusDot: View {
    let online: Bool
    var health: String = ""
    var size: CGFloat = 8
    var body: some View {
        Circle().fill(color).frame(width: size, height: size)
    }

    private var color: Color {
        switch health {
        case "fail": .red
        case "warn": .orange
        default: online ? .green : .secondary.opacity(0.5)
        }
    }
}

/// One machine: its dot, name ("This Mac" beside this one), its one line,
/// and its system's mark.
struct MachineRowView: View {
    let machine: AppMachinesViewMachine

    var body: some View {
        HStack(spacing: 10) {
            StatusDot(online: machine.online, health: machine.health)
            VStack(alignment: .leading, spacing: 1) {
                HStack(spacing: 6) {
                    Text(machine.name).fontWeight(.medium).lineLimit(1)
                    if let badge = machine.badge {
                        Text(badge)
                            .font(.caption2.weight(.medium))
                            .foregroundStyle(.secondary)
                            .padding(.horizontal, 5)
                            .padding(.vertical, 1)
                            .background(.fill.tertiary, in: .rect(cornerRadius: 4))
                    }
                }
                Text(machine.line)
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .accessibilityIdentifier("machine-line")
            }
            Spacer(minLength: 4)
            OsIconImage(id: osIcon(machine.os), size: 14).foregroundStyle(.tertiary)
        }
        .padding(.vertical, 2)
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("machine-row")
    }
}

/// A machine's page: name and rename, status, the two switches, the Spaces
/// on it, New Space here, Details and Remove from account.
struct MachinePage: View {
    @Bindable var model: AppModel
    let machine: AppMachinesViewMachine
    let onError: (String) -> Void
    @State private var renaming = false
    @State private var newName = ""
    @State private var removing: AppMachinesViewRemove?
    @State private var detailsOpen = false
    @State private var fullLog: HostLog?

    private var host: HostModel { model.host }

    var body: some View {
        if machine.current, host.form != nil {
            // This Mac's setup form takes the page while it shows.
            HostFormView(host: host, buttons: true)
        } else {
            page
        }
    }

    private var page: some View {
        let m = machine
        let panel = host.panel
        return ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                HStack(spacing: 6) {
                    Text(m.name).font(.title2.weight(.semibold)).lineLimit(1).textSelection(.enabled)
                    if let r = m.rename {
                        Button {
                            newName = m.name
                            renaming = true
                        } label: {
                            Image(systemName: "pencil")
                        }
                        .buttonStyle(.borderless)
                        .help(r.title)
                        .accessibilityLabel(r.title)
                        .accessibilityIdentifier("machine-rename")
                    }
                }
                HStack(spacing: 6) {
                    StatusDot(online: m.online, size: 7)
                    Text(m.status)
                    if let by = m.sharedBy {
                        Text("\u{b7}").foregroundStyle(.tertiary)
                        Text(by)
                    }
                }
                .foregroundStyle(.secondary)
                .padding(.top, 4)
                .accessibilityIdentifier("machine-status")

                if !m.issues.isEmpty {
                    Text(m.issuesTitle)
                        .font(.callout.weight(.semibold))
                        .foregroundStyle(.secondary)
                        .padding(.top, 22)
                        .padding(.bottom, 8)
                    card {
                        ForEach(Array(m.issues.enumerated()), id: \.element.id) { i, issue in
                            if i > 0 { Divider() }
                            issueRow(issue)
                        }
                    }
                    .accessibilityIdentifier("machine-issues")
                }

                card {
                    if let notice = m.notice {
                        HStack(spacing: 8) {
                            Image(systemName: "info.circle")
                            Text(notice).fixedSize(horizontal: false, vertical: true)
                            Spacer(minLength: 8)
                            if m.current, let action = m.noticeAction {
                                Button(action.label) { Task { await host.run(action.id) } }
                                    .disabled(host.busy || !action.enabled)
                            }
                        }
                        .font(.callout)
                        .foregroundStyle(.secondary)
                        .padding(.horizontal, 16)
                        .padding(.vertical, 10)
                        .accessibilityIdentifier("machine-notice")
                        Divider()
                    }
                    ForEach(Array(m.toggles.enumerated()), id: \.element.id) { i, t in
                        if i > 0 { Divider() }
                        toggleRow(t, dimmed: !t.editable && m.togglesNote == nil)
                    }
                    if let note = m.togglesNote {
                        Divider()
                        Label(note, systemImage: m.team ? "person.2" : "lock")
                            .font(.callout)
                            .foregroundStyle(.secondary)
                            .padding(.horizontal, 16)
                            .padding(.vertical, 9)
                            .accessibilityIdentifier("machine-toggles-note")
                    }
                }
                .padding(.top, 22)

                if m.current {
                    // What needs the owner now: permissions to grant, a
                    // switch that failed.
                    if panel.permissionsTitle != nil || host.actionFailure != nil {
                        VStack(alignment: .leading, spacing: 8) {
                            if let title = panel.permissionsTitle {
                                Text(title).font(.callout.weight(.semibold)).foregroundStyle(.secondary)
                                card {
                                    PermissionRows(rows: panel.permissions, openLabel: panel.openSettingsLabel)
                                        .padding(.horizontal, 16).padding(.vertical, 8)
                                }
                            }
                            if let failure = host.actionFailure {
                                HostSetupFailureView(failure: failure, retrying: host.busy) {
                                    Task { await host.retryFailedAction() }
                                }
                            }
                        }
                        .padding(.top, 20)
                    }
                    if let progress = host.progress {
                        HostSetupProgressView(text: progress).padding(.top, 12)
                    }
                }

                Text(m.spacesTitle)
                    .font(.callout.weight(.semibold))
                    .foregroundStyle(.secondary)
                    .padding(.top, 28)
                    .padding(.bottom, 8)
                card {
                    ForEach(Array(m.spaces.enumerated()), id: \.element.id) { i, s in
                        if i > 0 { Divider() }
                        Button {
                            model.select(s.id)
                        } label: {
                            HStack(spacing: 8) {
                                Text(s.name).font(.system(.callout, design: .monospaced)).lineLimit(1)
                                if !s.os.isEmpty {
                                    Text("\u{b7}").foregroundStyle(.tertiary)
                                    Text(s.os).foregroundStyle(.secondary).lineLimit(1)
                                }
                                Spacer(minLength: 8)
                                StatusDot(online: s.running, size: 7)
                                Text(s.status).font(.callout).foregroundStyle(.secondary)
                                Image(systemName: "chevron.right").font(.caption).foregroundStyle(.tertiary)
                            }
                            .contentShape(Rectangle())
                            .padding(.horizontal, 16)
                            .padding(.vertical, 10)
                        }
                        .buttonStyle(.plain)
                        .help("Open \(s.name)")
                        .accessibilityIdentifier("machine-space-\(s.id)")
                    }
                    if let empty = m.spacesEmpty {
                        Text(empty).font(.callout).foregroundStyle(.secondary)
                            .padding(.horizontal, 16).padding(.vertical, 12)
                    }
                    Divider()
                    HStack {
                        Button {
                            Task { await model.openNewSpace(on: m.newSpace.on) }
                        } label: {
                            Label(m.newSpace.label, systemImage: "plus")
                        }
                        .disabled(!m.newSpace.enabled)
                        .help(m.newSpace.help ?? "")
                        .accessibilityIdentifier("machine-new-space")
                        Spacer()
                    }
                    .padding(.horizontal, 12)
                    .padding(.vertical, 10)
                }

                DisclosureGroup(isExpanded: $detailsOpen) {
                    VStack(alignment: .leading, spacing: 16) {
                        card {
                            ForEach(Array(m.details.enumerated()), id: \.element.label) { i, f in
                                if i > 0 { Divider() }
                                HStack(alignment: .firstTextBaseline) {
                                    Text(f.label).foregroundStyle(.secondary).frame(width: 140, alignment: .leading)
                                    Text(f.value).textSelection(.enabled)
                                    Spacer()
                                }
                                .padding(.horizontal, 16)
                                .padding(.vertical, 9)
                            }
                        }
                        if m.current { logs(panel) }
                    }
                    .padding(.top, 8)
                } label: {
                    Text(m.detailsTitle).font(.callout.weight(.semibold)).foregroundStyle(.secondary)
                }
                .padding(.top, 24)
                .accessibilityIdentifier("machine-details")

                if let r = m.remove {
                    Button(r.label, role: .destructive) { removing = r }
                        .buttonStyle(.borderless)
                        .foregroundStyle(.red)
                        .disabled(!r.enabled)
                        .help(r.help ?? "")
                        .padding(.top, 28)
                        .accessibilityIdentifier("machine-remove")
                }
            }
            .frame(maxWidth: 620, alignment: .leading)
            .padding(.horizontal, 36)
            .padding(.vertical, 28)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .alert(m.rename?.title ?? "", isPresented: $renaming) {
            TextField("Name", text: $newName)
            Button(m.rename?.confirmLabel ?? "Rename") { rename() }
            Button("Cancel", role: .cancel) {}
        }
        .alert(removing?.confirm.title ?? "", isPresented: Binding(
            get: { removing != nil }, set: { if !$0 { removing = nil } })) {
            if let r = removing {
                Button(r.confirm.confirmLabel, role: .destructive) {
                    removing = nil
                    remove(r)
                }
                Button(r.confirm.cancelLabel, role: .cancel) { removing = nil }
            }
        } message: {
            Text(removing?.confirm.message ?? "")
        }
        .sheet(item: $fullLog) { log in
            switch log {
            case .access:
                HostLogSheet(title: panel.recentTitle ?? "", rows: panel.recentAll,
                             backgroundRows: panel.recentWithBackground) { fullLog = nil }
            case .activity:
                HostLogSheet(title: panel.activityTitle ?? "", rows: panel.activityAll,
                             backgroundRows: nil) { fullLog = nil }
            }
        }
        .accessibilityIdentifier("machine-page")
    }

    /// A rounded card, as the page's groups are drawn.
    private func card<Content: View>(@ViewBuilder _ content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: 0, content: content)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(.background.secondary, in: .rect(cornerRadius: 10))
            .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(.separator))
    }

    private func toggleRow(_ t: AppMachinesViewToggle, dimmed: Bool) -> some View {
        HStack(spacing: 20) {
            VStack(alignment: .leading, spacing: 1) {
                Text(t.label).fontWeight(.medium)
                Text(t.help).font(.callout).foregroundStyle(.secondary)
            }
            .opacity(dimmed ? 0.5 : 1)
            Spacer()
            Toggle(t.label, isOn: Binding(get: { t.on }, set: { _ in flip(t) }))
                .labelsHidden()
                .toggleStyle(.switch)
                .disabled(!t.editable || !t.enabled || host.busy)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 14)
        .accessibilityIdentifier("machine-toggle-\(t.id)")
    }

    /// One issue: its mark, one sentence, and its fix when there is one
    /// here (a System Settings pane, or This Mac's host action).
    private func issueRow(_ issue: AppMachinesViewIssue) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            Image(systemName: issue.level == "fail" ? "xmark.octagon.fill" : "exclamationmark.triangle.fill")
                .foregroundStyle(issue.level == "fail" ? Color.red : Color.orange)
                .accessibilityLabel(issue.level == "fail" ? "Problem" : "Warning")
            Text(issue.text).fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 12)
            if let action = issue.action {
                Button(action.label) { fix(action) }
                    .buttonStyle(.bordered)
                    .controlSize(.small)
                    .disabled(action.hostAction != nil && (!machine.current || host.busy))
                    .accessibilityIdentifier("machine-issue-fix-\(issue.id)")
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
        .accessibilityIdentifier("machine-issue-\(issue.id)")
    }

    private func fix(_ action: AppMachinesViewIssueAction) {
        if let url = action.url {
            PermissionRows.open(url)
        } else if let id = action.hostAction, machine.current {
            Task {
                await host.run(id)
                await model.checkMachineHealth()
            }
        }
    }

    /// This Mac's switch: its actions in order, or the setup form.
    private func flip(_ t: AppMachinesViewToggle) {
        if let profile = t.setup {
            host.openForm()
            host.send(.setProfile(profile: profile))
            return
        }
        Task { for a in t.actions { await host.run(a) } }
    }

    @ViewBuilder
    private func logs(_ panel: AppHostPanelView) -> some View {
        if let title = panel.recentTitle, !panel.recent.isEmpty {
            VStack(alignment: .leading, spacing: 6) {
                Text(title).font(.callout.weight(.semibold)).foregroundStyle(.secondary)
                card {
                    TimedRows(rows: panel.recent).padding(.horizontal, 16).padding(.vertical, 6)
                }
                if let more = panel.recentMore { ShowAllButton(label: more) { fullLog = .access } }
            }
        }
        if let title = panel.activityTitle, !panel.activity.isEmpty {
            VStack(alignment: .leading, spacing: 6) {
                Text(title).font(.callout.weight(.semibold)).foregroundStyle(.secondary)
                card {
                    TimedRows(rows: panel.activity).padding(.horizontal, 16).padding(.vertical, 6)
                }
                if let more = panel.activityMore { ShowAllButton(label: more) { fullLog = .activity } }
            }
        }
    }

    private func rename() {
        guard let r = machine.rename else { return }
        let name = newName.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty, name != machine.name else { return }
        Task {
            if r.target == "device" {
                await model.devices.rename(id: r.id, to: name)
                await model.refresh()
                return
            }
            do {
                try await model.renameMachine(id: r.id, name: name)
            } catch {
                onError(LiveSpacesBackend.words(error))
            }
        }
    }

    private func remove(_ r: AppMachinesViewRemove) {
        Task {
            if r.host {
                await host.run(.remove)
                return
            }
            do {
                try await model.removeMachine(machine: r.machine, device: r.device)
            } catch {
                onError(LiveSpacesBackend.words(error))
            }
        }
    }
}
