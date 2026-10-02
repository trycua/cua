// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import CuaSpacesFFI
import SwiftUI

/// The main window: a sidebar of Spaces and the Keyvault (with its global
/// switch), and the selected item's detail. Rows are one line; the
/// sections, search, selection fallback and dimming are the core's.
public struct MainWindow: View {
    @Bindable var model: AppModel

    public init(model: AppModel) {
        self.model = model
    }

    public var body: some View {
        let chrome = model.chrome
        NavigationSplitView {
            Sidebar(model: model)
                .navigationSplitViewColumnWidth(min: 200, ideal: 230, max: 300)
        } detail: {
            detail
                .safeAreaInset(edge: .top, spacing: 0) { DevicesBanner(devices: model.devices) }
        }
        .background(WindowReader { model.dropTargets.window = $0 })
        .searchable(text: $model.query, placement: .sidebar, prompt: chrome.searchPlaceholder)
        .toolbar {
            ToolbarItem(placement: .primaryAction) {
                Button {
                    Task { await model.openNewSpace() }
                } label: {
                    Label(chrome.newSpaceLabel, systemImage: "plus")
                }
                .keyboardShortcut("n", modifiers: .command)
                .help(chrome.newSpaceLabel)
            }
        }
        .sheet(isPresented: $model.showingNewSpace) {
            NewSpaceWizardView(wizard: model.wizard,
                               onCreate: { model.create($0) },
                               onAdd: { url, token, name in
                                   try await model.addByAddress(url: url, token: token, name: name)
                               },
                               onCancel: { model.cancelNewSpace() },
                               cloud: model.cloud)
        }
        .sheet(item: approvalBinding) { _ in
            ApprovalSheet(keyvault: model.keyvault)
        }
        .sheet(isPresented: devicesSheet(\.enroll) { model.devices.enrollSurface }) {
            EnrollSheet(devices: model.devices)
        }
        .sheet(isPresented: devicesSheet(\.approval) { model.devices.approvalSurface }) {
            DeviceApprovalSheet(devices: model.devices)
        }
        // Opening the window refreshes at once; the app keeps polling the
        // roster and the Keyvault every 10 s while it runs (see start()).
        .task {
            await model.refresh()
            await model.keyvault.refresh()
        }
    }

    /// The main window's copy of a Devices sheet (Settings has its own).
    private func devicesSheet<T>(_ key: ReferenceWritableKeyPath<DevicesModel, T?>,
                                 surface: @escaping () -> DevicesSurface) -> Binding<Bool> {
        let devices = model.devices
        return Binding(get: { devices[keyPath: key] != nil && surface() == .main },
                       set: { if !$0 { devices[keyPath: key] = nil } })
    }

    private var approvalBinding: Binding<KvApprovalState?> {
        Binding(get: { model.keyvault.approval }, set: { model.keyvault.approval = $0 })
    }

    @ViewBuilder private var detail: some View {
        switch model.selection {
        case .space:
            if let space = model.selectedSpace {
                if model.detail(space).isHost {
                    ThisMachineView(host: model.host)
                } else {
                    SpaceDetailView(model: model, space: space)
                        .id(space.id)
                }
            } else {
                EmptySpaces(model: model)
            }
        case .keyvault(let selection):
            KeyvaultDetail(keyvault: model.keyvault, selection: selection)
        case .agents:
            AgentsPageView(model: model.persistent)
                .onAppear {
                    model.recordFeature("agents_page_open")
                    let id = model.sidebar.thisMachine?.id
                    model.persistent.thisMachine = id?.hasPrefix("relay:") == true ? id : nil
                }
        case .drive where model.chrome.volumeLabel != nil:
            DrivePageView(model: model.persistent)
        case .drive:
            EmptySpaces(model: model)
        case .notifications:
            NotificationsPageView(model: model.persistent)
        case nil:
            EmptySpaces(model: model)
        }
    }
}

extension KvApprovalState: @retroactive Identifiable {
    public var id: String { requestId }
}

/// The sidebar: Spaces (one section per location) and the Keyvault.
struct Sidebar: View {
    @Bindable var model: AppModel

    /// The Spaces a Keyvault sign-in is live in (read once per draw).
    private var signedIn: Set<String> { model.signedInSpaceIds }

    var body: some View {
        let sidebar = model.sidebar
        let kv = model.keyvault
        List(selection: $model.selection) {
            if let row = sidebar.thisMachine {
                spaceRow(row)
            }
            ForEach(sidebar.sections, id: \.title) { section in
                Section(section.title) {
                    ForEach(section.rows, id: \.id) { row in
                        spaceRow(row)
                    }
                }
            }
            if let empty = sidebar.emptyText, sidebar.thisMachine == nil {
                Text(empty).foregroundStyle(.secondary)
            }
            Section {
                ForEach(kv.sidebar.categories, id: \.category) { row in
                    Label(row.title, systemImage: row.symbol)
                        .badge(badge(row))
                        .tag(MainSelection.keyvault(.category(category: row.category)))
                }
                ForEach(kv.sidebar.apps, id: \.key) { app in
                    HStack(spacing: 8) {
                        AppIcon(keyvault: kv, providerId: app.key, name: app.title)
                            .frame(width: 18, height: 18)
                        Text(app.title).lineLimit(1)
                    }
                    .badge(Int(app.items))
                    .tag(MainSelection.keyvault(.app(key: app.key)))
                }
            } header: {
                KeyvaultHeader(keyvault: kv, title: model.chrome.keyvaultTitle)
            }
            Section {
                Label("Agents", systemImage: "person.2").tag(MainSelection.agents)
                // Only with the Cua Volume experiment on (the core's chrome).
                if let volume = model.chrome.volumeLabel {
                    Label(volume, systemImage: "externaldrive").tag(MainSelection.drive)
                }
                Label("Notifications", systemImage: "bell").tag(MainSelection.notifications)
            }
        }
        .listStyle(.sidebar)
        .safeAreaInset(edge: .bottom) { SidebarFooter(model: model) }
        .onChange(of: model.selection) { _, new in
            if case .space(let id) = new { model.send(.select(id: id, now: Int64(Date().timeIntervalSince1970 * 1000))) }
        }
    }

    /// A Space row; also a drop target for a dragged real window.
    private func spaceRow(_ row: AppSidebarRow) -> some View {
        SpaceRowView(row: row, targeted: model.dropTargets.targetedId == row.id,
                     signedIn: signedIn.contains(row.id),
                     onSignedIn: { model.showAccess(spaceId: row.id) }) { on in
            if let space = model.spaces.first(where: { $0.id == row.id }) {
                model.setPower(space, on: on)
            }
        }
            .onGeometryChange(for: CGRect.self) { $0.frame(in: .global) } action: {
                model.dropTargets.setFrame(row.id, $0)
            }
            .onDisappear { model.dropTargets.removeFrame(row.id) }
            .tag(MainSelection.space(row.id))
    }

    private func badge(_ row: KvCategoryRow) -> Int { Int(row.badge ?? 0) }
}

/// The sidebar's foot: the account line, Sign in when offered, Settings.
struct SidebarFooter: View {
    let model: AppModel

    var body: some View {
        let chrome = model.chrome
        HStack(spacing: 8) {
            Text(chrome.account)
                .lineLimit(1)
                .truncationMode(.middle)
                .foregroundStyle(.secondary)
                .accessibilityIdentifier("sidebar-account")
            Spacer(minLength: 4)
            if let label = chrome.signInLabel {
                Button(label) { Task { await model.beginSignIn() } }
                    .controlSize(.small)
            }
            SettingsLink {
                Image(systemName: "gearshape")
            }
            .buttonStyle(.borderless)
            .help(chrome.settingsLabel)
            .accessibilityLabel(chrome.settingsLabel)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(.bar)
    }
}

/// One Space row: its OS icon, the name and a status dot, dimmed when not
/// live, and outlined while a dragged window is over it. A Space that turns
/// off and on has a power button before its dot, shown on hover and on the
/// selected row, and a spinner in its place while it runs.
struct SpaceRowView: View {
    let row: AppSidebarRow
    var targeted = false
    /// A Keyvault sign-in is live in it: "Signed in", which opens the
    /// Keyvault's Access page.
    var signedIn = false
    var onSignedIn: (() -> Void)?
    /// The power button was pressed: turn it on (`true`) or off.
    var onPower: ((Bool) -> Void)?
    @State private var hovering = false

    var body: some View {
        HStack(spacing: 8) {
            OsIconImage(id: row.osIcon, size: 13)
                .foregroundStyle(.secondary)
                .frame(width: 16)
            Text(row.name).lineLimit(1)
            if let place = row.place {
                // A Space in your cloud: where it runs.
                Text(place)
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.tail)
            }
            Spacer(minLength: 4)
            if let progress = row.progress {
                // Being created: the ring and the percentage, no dot.
                Text(row.trailing ?? "")
                    .font(.callout.monospacedDigit())
                    .foregroundStyle(.secondary)
                ProgressRing(permille: progress)
                    .accessibilityLabel(row.statusText)
            } else {
                if let trailing = row.trailing {
                    // The create or the power action failed: why, inline.
                    Text(trailing)
                        .font(.callout)
                        .foregroundStyle(.red)
                        .lineLimit(1)
                        .truncationMode(.tail)
                        .help(trailing)
                }
                if signedIn {
                    SignedInBadge { onSignedIn?() }
                }
                if let power = row.power, let onPower,
                   hovering || row.selected || power.busy {
                    PowerButton(button: power) { onPower(power.turnOn) }
                }
                Circle()
                    .fill(color)
                    .frame(width: 7, height: 7)
                    .accessibilityLabel(row.statusText)
            }
        }
        // A Space one of your machines provides sits under that machine.
        .padding(.leading, row.nested ? 18 : 0)
        .opacity(row.dim ? 0.5 : 1)
        .overlay {
            if targeted {
                RoundedRectangle(cornerRadius: 5).strokeBorder(Color.accentColor, lineWidth: 2).padding(-3)
            }
        }
        .help(row.detail)
        .onHover { hovering = $0 }
    }

    private var color: Color {
        switch row.status {
        case .running, .local: return .green
        case .approval: return .orange
        case .provisioning: return .blue
        case .suspended, .deleting: return .secondary
        }
    }
}

/// "Signed in": a Keyvault sign-in is live in this Space. Opens the
/// Keyvault's Access page on it.
struct SignedInBadge: View {
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Label("Signed in", systemImage: "key.fill")
                .labelStyle(.titleAndIcon)
                .font(.caption2.weight(.medium))
                .foregroundStyle(.secondary)
                .padding(.horizontal, 5)
                .padding(.vertical, 1)
                .background(Capsule().fill(.quaternary))
        }
        .buttonStyle(.plain)
        .fixedSize()
        .help("Keyvault access is live in this Space. Show it in Keyvault.")
        .accessibilityLabel("Signed in. Show in Keyvault")
        .accessibilityIdentifier("space-signed-in")
    }
}

/// The power button (the core's `AppPowerButton`): the `power` symbol, a
/// small spinner while it runs, the core's words as its tooltip.
struct PowerButton: View {
    let button: AppPowerButton
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            if button.busy {
                ProgressView().controlSize(.mini)
            } else {
                Image(systemName: button.symbol)
            }
        }
        .buttonStyle(.borderless)
        .disabled(!button.enabled)
        .help(button.help)
        .accessibilityLabel(button.help)
        .accessibilityIdentifier("space-power")
    }
}

/// "Keyvault" with the global switch (on = working; off stops every
/// teleport, import and approval; turning it back on asks for Touch ID in
/// the Cua daemon).
struct KeyvaultHeader: View {
    let keyvault: KeyvaultModel
    let title: String

    var body: some View {
        let page = keyvault.page
        HStack {
            Text(title)
            Spacer()
            if page.killSwitchVisible {
                Toggle(title, isOn: Binding(
                    get: { !page.disabled },
                    set: { on in Task { await keyvault.setDisabled(!on) } }))
                    .toggleStyle(.switch)
                    .controlSize(.mini)
                    .labelsHidden()
                    .disabled(!page.killSwitchEnabled || keyvault.busy)
                    .help(page.killSwitchHelp)
                    .accessibilityIdentifier("keyvault-switch")
                    .padding(.trailing, 6)
            }
        }
    }
}

struct EmptySpaces: View {
    let model: AppModel

    var body: some View {
        let chrome = model.chrome
        ContentUnavailableView {
            Text(chrome.emptyTitle)
        } actions: {
            Button(chrome.emptyAction) { Task { await model.openNewSpace() } }
                .buttonStyle(.glassProminent)
        }
    }
}
