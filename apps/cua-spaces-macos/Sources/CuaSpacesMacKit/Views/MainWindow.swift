// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import CuaSpacesFFI
import SwiftUI

/// The main window: a sidebar of Spaces and the Keyvault (with its global
/// switch), and the selected item's detail. Rows are one line; the
/// sections, search, selection fallback and dimming are the core's. ⌘K
/// opens the command palette.
public struct MainWindow: View {
    @Bindable var model: AppModel
    @Environment(\.openSettings) private var openSettings

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
                .safeAreaInset(edge: .top, spacing: 0) {
                    VStack(spacing: 0) {
                        DevicesBanner(devices: model.devices)
                        ShareRemovalBanner(model: model)
                        RelayAllowanceBanner(devices: model.devices)
                        if let error = model.rosterError {
                            SpacesDiscoveryNotice(error: error)
                        }
                    }
                }
        }
        .overlay(alignment: .bottomTrailing) { SignInToastView(model: model) }
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
                               cloud: model.cloud,
                               onRuntimeSwitch: { await model.applyRuntimeSwitch($0) })
        }
        .sheet(isPresented: $model.showingPalette) {
            CommandPaletteView(model: model)
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
            await model.reloadShareRemovals()
        }
        // The relay allowance: at once (and on every sign-in or out), every
        // 5 minutes, and when the app comes to the front.
        .task(id: model.devices.signedIn) {
            while !Task.isCancelled {
                await model.devices.readRelayUsage()
                try? await Task.sleep(for: DevicesModel.relayUsagePoll)
            }
        }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            Task { await model.devices.readRelayUsage(maxAge: DevicesModel.relayUsageFocusAge) }
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
            AgentsPageView(model: model.persistent, openAgentKeys: {
                model.settingsTab = .agents
                openSettings()
            })
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
        case .machines:
            MachinesPageView(model: model)
        case .gettingStarted:
            GettingStartedView(model: model)
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
        let checklist = model.checklist.view
        List(selection: $model.selection) {
            if checklist.visible {
                Label(checklist.title, systemImage: "checklist")
                    .badge(Text(checklist.progress))
                    .tag(MainSelection.gettingStarted)
                    .accessibilityIdentifier("sidebar-getting-started")
            }
            if let row = sidebar.thisMachine {
                spaceRow(row)
            }
            ForEach(sidebar.sections, id: \.title.code) { section in
                Section(CoreMessages.text(section.title)) {
                    ForEach(section.rows, id: \.id) { row in
                        spaceRow(row)
                    }
                }
            }
            if let empty = sidebar.emptyText, sidebar.thisMachine == nil, model.rosterError == nil {
                Text(CoreMessages.text(empty)).foregroundStyle(.secondary)
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
                Label("Computers", systemImage: "server.rack").tag(MainSelection.machines)
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

/// The sidebar's foot: every sign-in state in one place (the core's
/// `appSignInFooter`): signed in as, waiting for the browser with the code,
/// Open again and Cancel, or why it failed with Try again; and Settings.
struct SidebarFooter: View {
    let model: AppModel
    @State private var showingDetails = false

    var body: some View {
        let f = model.signInFooter
        HStack(alignment: .top, spacing: 8) {
            VStack(alignment: .leading, spacing: 4) {
                switch f.state {
                case .starting, .waiting:
                    HStack(spacing: 6) {
                        ProgressView().controlSize(.mini)
                        Text(f.text).lineLimit(2)
                    }
                    .accessibilityElement(children: .combine)
                    .accessibilityIdentifier("sidebar-signin-waiting")
                    // The code on its own line (it never wraps), then
                    // Open again and Cancel.
                    if let code = f.code {
                        Text("\(f.codeLabel) \(Text(code).font(.callout.monospaced().weight(.semibold)).foregroundStyle(.primary))")
                            .font(.callout)
                            .foregroundStyle(.secondary)
                            .textSelection(.enabled)
                            .lineLimit(1)
                            .fixedSize()
                            .padding(.leading, 20)
                    }
                    HStack(spacing: 4) {
                        if let reopen = f.reopenLabel {
                            Button(reopen) { model.reopenSignIn() }
                                .buttonStyle(.link)
                                .accessibilityIdentifier("sidebar-signin-reopen")
                            Text("\u{b7}")
                        }
                        if let cancel = f.cancelLabel {
                            Button(cancel) { model.cancelSignIn() }
                                .buttonStyle(.link)
                                .accessibilityIdentifier("sidebar-signin-cancel")
                        }
                    }
                    .lineLimit(1)
                    .fixedSize()
                    .padding(.leading, 20)
                    .font(.callout)
                    .foregroundStyle(.secondary)
                case .failed:
                    HStack(alignment: .firstTextBaseline, spacing: 6) {
                        Image(systemName: "exclamationmark.triangle")
                            .foregroundStyle(.red)
                        Text(f.text)
                            .lineLimit(4)
                            .accessibilityIdentifier("sidebar-signin-failed")
                    }
                    HStack(spacing: 8) {
                        if let retry = f.actionLabel {
                            Button(retry) { Task { await model.beginSignIn() } }
                                .controlSize(.small)
                                .accessibilityIdentifier("sidebar-signin-retry")
                        }
                        if let detail = f.detail {
                            Button(f.detailsLabel) { showingDetails.toggle() }
                                .buttonStyle(.link)
                                .font(.callout)
                                .popover(isPresented: $showingDetails) {
                                    Text(detail)
                                        .font(.callout.monospaced())
                                        .textSelection(.enabled)
                                        .frame(width: 280, alignment: .leading)
                                        .padding(12)
                                }
                        }
                    }
                    .padding(.leading, 20)
                case .signedIn:
                    // "Signed in as", then the whole address on its own line.
                    VStack(alignment: .leading, spacing: 1) {
                        Text("Signed in as").font(.caption).foregroundStyle(.secondary)
                        Text(f.identity ?? f.text)
                            .lineLimit(1)
                            .truncationMode(.middle)
                    }
                    .help(f.text)
                    .accessibilityElement(children: .combine)
                    .accessibilityIdentifier("sidebar-account")
                case .signedOut:
                    HStack(spacing: 8) {
                        Text(f.text)
                            .lineLimit(1)
                            .truncationMode(.middle)
                            .foregroundStyle(.secondary)
                            .accessibilityIdentifier("sidebar-account")
                        Spacer(minLength: 4)
                        if let label = f.actionLabel {
                            Button(label) { Task { await model.beginSignIn() } }
                                .controlSize(.small)
                        }
                    }
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            SettingsLink {
                Image(systemName: "gearshape")
            }
            .buttonStyle(.borderless)
            .help(model.chrome.settingsLabel)
            .accessibilityLabel(model.chrome.settingsLabel)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(.bar)
    }
}

/// A failed sign-in's toast: seen wherever the window is, then gone (the
/// sidebar's foot keeps the reason and Try again).
struct SignInToastView: View {
    let model: AppModel

    var body: some View {
        if let toast = model.signInToast {
            HStack(alignment: .top, spacing: 8) {
                Image(systemName: "exclamationmark.circle.fill").foregroundStyle(.red)
                VStack(alignment: .leading, spacing: 2) {
                    Text(toast.title).font(.headline)
                    if let body = toast.body {
                        Text(body).font(.callout).foregroundStyle(.secondary)
                    }
                }
                Button {
                    model.signInToast = nil
                } label: {
                    Image(systemName: "xmark").font(.caption)
                }
                .buttonStyle(.borderless)
                .accessibilityLabel("Dismiss")
            }
            .padding(12)
            .frame(width: 300, alignment: .leading)
            .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 12))
            .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(.separator))
            .shadow(radius: 8, y: 2)
            .padding(16)
            .accessibilityIdentifier("signin-toast")
            .task(id: toast) {
                try? await Task.sleep(for: .seconds(15))
                if model.signInToast == toast { model.signInToast = nil }
            }
        }
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
                    .accessibilityLabel(CoreMessages.text(row.statusText))
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
                    .accessibilityLabel(CoreMessages.text(row.statusText))
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
        .help(CoreMessages.text(button.help))
        .accessibilityLabel(CoreMessages.text(button.help))
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
    @State private var stacked = false

    var body: some View {
        let chrome = model.chrome
        let tiles = stacked ? AnyLayout(VStackLayout(spacing: 12)) : AnyLayout(HStackLayout(spacing: 12))
        ContentUnavailableView {
            Label(model.rosterError == nil ? chrome.emptyTitle : "Spaces could not be loaded",
                  systemImage: "desktopcomputer")
        } description: {
            if model.rosterError == nil { Text(chrome.emptyDetail) }
        } actions: {
            VStack(spacing: 18) {
                if model.rosterError == nil {
                    tiles {
                        tile("os-linux", chrome.emptyAction, chrome.emptyLinuxDetail, .linux)
                        tile("os-macos", chrome.emptyMacosAction, chrome.emptyMacosDetail, .macos)
                    }
                }
                Button(chrome.newSpaceLabel) { Task { await model.openNewSpace() } }
                    .buttonStyle(.glass)
            }
            .padding(.top, 10)
        }
        // Neither the actions slot nor this view reports the pane width, so measure a pane-filling frame.
        .frame(maxWidth: .infinity)
        .onGeometryChange(for: Bool.self) { $0.size.width < 640 } action: { stacked = $0 }
    }

    private func tile(_ icon: String, _ name: String, _ detail: String, _ os: AppSpaceOs) -> some View {
        Button { Task { await model.openNewSpace(quick: os) } } label: {
            HStack(spacing: 12) {
                OsIconImage(id: icon, size: 24)
                    .frame(width: 44, height: 44)
                    .background(.fill.tertiary, in: .rect(cornerRadius: 10))
                VStack(alignment: .leading, spacing: 2) {
                    Text(name).font(.headline)
                    Text(detail).font(.callout).foregroundStyle(.secondary)
                }
            }
            .padding(.horizontal, 16)
            .frame(width: 288, height: 80, alignment: .leading)
            .contentShape(.rect(cornerRadius: 16))
        }
        .buttonStyle(.plain)
        .glassEffect(.regular.interactive(), in: .rect(cornerRadius: 16))
        .accessibilityLabel("\(name), \(detail)")
    }
}

/// Persistent discovery failure notice above the selected Space.
struct SpacesDiscoveryNotice: View {
    let error: String

    var body: some View {
        Text(error)
            .foregroundStyle(.red)
            .padding()
            .frame(maxWidth: .infinity, alignment: .leading)
            .accessibilityIdentifier("spaces-discovery-error")
    }
}
