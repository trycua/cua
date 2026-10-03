// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import CuaSpacesFFI
import SwiftUI

/// The Settings window: General (the core's settings page), Devices,
/// Experiments and About (with the updater).
struct SettingsScene: View {
    @Bindable var model: AppModel

    var body: some View {
        TabView(selection: $model.settingsTab) {
            SettingsView(model: model)
                .tabItem { Label("General", systemImage: "gearshape") }
                .tag(SettingsTab.general)
            DevicesSettingsView(devices: model.devices)
                .tabItem { Label(model.devices.view.labels.title, systemImage: "laptopcomputer.and.iphone") }
                .tag(SettingsTab.devices)
            ExperimentsSettingsView(model: model)
                .tabItem { Label(model.experimentsPage.title, systemImage: "flask") }
                .tag(SettingsTab.experiments)
            AboutSettingsView(updates: model.updates)
                .tabItem { Label("About", systemImage: "info.circle") }
                .tag(SettingsTab.about)
        }
    }
}

/// Settings → Experiments: the core's page, one switch per experiment with
/// its one line under the title (all off until turned on).
struct ExperimentsSettingsView: View {
    @Bindable var model: AppModel

    var body: some View {
        let page = model.experimentsPage
        Form {
            ForEach(page.sections, id: \.id) { section in
                Section {
                    ForEach(section.rows.filter { $0.kind == .toggle }, id: \.id) { r in
                        let note = section.rows.first { $0.id == "\(r.id)-note" }?.label
                        Toggle(isOn: Binding(
                            get: { r.options.first(where: { $0.id == "on" })?.active ?? false },
                            set: { model.chooseExperiment(row: r.id, option: $0 ? "on" : "off") })) {
                            Text(r.label)
                            if let note { Text(note) }
                        }
                        .disabled(!r.enabled)
                        .accessibilityIdentifier("settings-\(r.id)")
                    }
                }
            }
        }
        .formStyle(.grouped)
        .frame(width: 520)
        .frame(minHeight: 320)
    }
}

/// Settings → Devices: this Mac's enrollment, the account's devices (Rename,
/// Revoke with its confirmation, Approve) and Recent Access. Everything
/// shown is the core's `appDevicesView`.
struct DevicesSettingsView: View {
    @Bindable var devices: DevicesModel
    @State private var renaming: AppDeviceRow?
    @State private var newName = ""
    @State private var revoking: AppDeviceRow?

    var body: some View {
        let v = devices.view
        Form {
            if !devices.signedIn {
                Text(v.labels.signedOut).foregroundStyle(.secondary)
            } else {
                Section(v.labels.thisDevice) {
                    LabeledContent {
                        HStack(spacing: 8) {
                            Text(devices.thisDeviceText(v.thisDevice))
                                .foregroundStyle(v.thisDevice.kind == .enrolled ? Color.secondary : Color.orange)
                                .lineLimit(1)
                                .accessibilityIdentifier("devices-this-status")
                            if let label = v.thisDevice.actionLabel {
                                Button(label) { devices.startEnroll(in: .settings) }
                                    .accessibilityIdentifier("devices-enroll")
                            }
                        }
                    } label: {
                        Label(v.thisDevice.name ?? Foundation.Host.current().localizedName ?? "This Mac",
                              systemImage: "laptopcomputer")
                            .lineLimit(1)
                    }
                    if let banner = v.banner {
                        Text(banner.text)
                            .font(.callout)
                            .foregroundStyle(.secondary)
                            .lineLimit(2)
                    }
                }
                if !v.unconfirmedMachines.isEmpty {
                    Section(v.labels.newMachines) {
                        ForEach(v.unconfirmedMachines, id: \.id) { m in
                            HStack(spacing: 10) {
                                Image(systemName: "questionmark.circle.fill")
                                    .foregroundStyle(.orange)
                                    .frame(width: 20)
                                Text(m.title).lineLimit(1)
                                Spacer()
                                Button(v.labels.confirmMachine) { devices.confirming = m }
                                    .controlSize(.small)
                            }
                            .accessibilityIdentifier("unconfirmed-machine-\(m.id)")
                        }
                    }
                }
                if !v.rows.isEmpty {
                    Section(v.labels.devices) {
                        ForEach(v.rows, id: \.id) { row($0, labels: v.labels) }
                    }
                }
                Section(v.labels.recent) {
                    if v.recent.isEmpty {
                        Text(v.labels.recentEmpty).foregroundStyle(.secondary)
                    }
                    ForEach(Array(v.recent.enumerated()), id: \.offset) { _, a in
                        LabeledContent {
                            Text(devices.relativeText(a.ts)).foregroundStyle(.secondary)
                        } label: {
                            HStack(spacing: 6) {
                                if a.notable {
                                    Image(systemName: "person.2.fill").foregroundStyle(.orange)
                                        .help("Another account or an unenrolled device")
                                }
                                Text(a.text).lineLimit(1).truncationMode(.middle)
                            }
                        }
                    }
                }
                .accessibilityIdentifier("devices-recent")
                if let error = devices.error {
                    Text(error).foregroundStyle(.red).lineLimit(2).help(error)
                }
            }
        }
        .formStyle(.grouped)
        .frame(width: 520)
        .frame(minHeight: 480)
        .task { await devices.refresh() }
        .sheet(isPresented: sheetBinding(\.enroll, .settings, { devices.enrollSurface })) {
            EnrollSheet(devices: devices)
        }
        .sheet(isPresented: sheetBinding(\.approval, .settings, { devices.approvalSurface })) {
            DeviceApprovalSheet(devices: devices)
        }
        .alert(v.labels.renameTitle, isPresented: Binding(get: { renaming != nil },
                                                           set: { if !$0 { renaming = nil } })) {
            TextField(v.labels.renameTitle, text: $newName)
            Button(v.labels.renameConfirm) {
                if let r = renaming { Task { await devices.rename(id: r.id, to: newName) } }
                renaming = nil
            }
            .disabled(appDevicesCleanName(name: newName) == nil)
            Button(v.labels.cancel, role: .cancel) { renaming = nil }
        }
        .alert(revoking?.revokeConfirm?.title ?? "", isPresented: Binding(get: { revoking?.revokeConfirm != nil },
                                                                          set: { if !$0 { revoking = nil } })) {
            if let r = revoking, let c = r.revokeConfirm {
                Button(c.confirmLabel, role: .destructive) {
                    revoking = nil
                    Task { await devices.revoke(id: r.id) }
                }
                Button(c.cancelLabel, role: .cancel) { revoking = nil }
            }
        } message: {
            Text(revoking?.revokeConfirm?.message ?? "")
        }
        .alert(devices.confirming?.confirm.title ?? "",
               isPresented: Binding(get: { devices.confirming != nil }, set: { if !$0 { devices.confirming = nil } })) {
            if let m = devices.confirming {
                Button(m.confirm.confirmLabel) {
                    devices.confirming = nil
                    Task { await devices.confirmMachine(id: m.id) }
                }
                Button(m.confirm.cancelLabel, role: .cancel) { devices.confirming = nil }
            }
        } message: {
            Text(devices.confirming?.confirm.message ?? "")
        }
    }

    /// A sheet binding for Settings' copy of a Devices sheet.
    private func sheetBinding<T>(_ key: ReferenceWritableKeyPath<DevicesModel, T?>, _ surface: DevicesSurface,
                                 _ current: @escaping () -> DevicesSurface) -> Binding<Bool> {
        Binding(get: { devices[keyPath: key] != nil && current() == surface },
                set: { if !$0 { devices[keyPath: key] = nil } })
    }

    @ViewBuilder private func row(_ r: AppDeviceRow, labels: AppDevicesLabels) -> some View {
        HStack(spacing: 10) {
            Image(systemName: Self.symbol(r.platform))
                .foregroundStyle(.secondary)
                .frame(width: 20)
            VStack(alignment: .leading, spacing: 2) {
                Text(r.title).lineLimit(1)
                Text(devices.rowDetail(r)).font(.caption).foregroundStyle(.secondary).lineLimit(1)
            }
            Spacer()
            if r.actions.contains(.approve) {
                Button(labels.approve) { devices.openApproval(deviceId: r.id, in: .settings) }
                    .controlSize(.small)
                Button(labels.deny) { Task { await devices.denyApproval(deviceId: r.id) } }
                    .controlSize(.small)
                    .disabled(devices.busy)
            }
            if r.actions.contains(.rename) || r.actions.contains(.revoke) {
                Menu {
                    if r.actions.contains(.rename) {
                        Button(labels.rename) {
                            newName = r.name
                            renaming = r
                        }
                    }
                    if r.actions.contains(.revoke) {
                        Button(labels.revoke, role: .destructive) { revoking = r }
                    }
                } label: {
                    Image(systemName: "ellipsis.circle")
                }
                .menuStyle(.borderlessButton)
                .menuIndicator(.hidden)
                .fixedSize()
                .disabled(devices.busy)
            }
        }
        .accessibilityIdentifier("device-\(r.id)")
    }

    static func symbol(_ platform: String) -> String {
        switch platform {
        case "macOS": return "laptopcomputer"
        case "Windows", "Linux", "FreeBSD": return "desktopcomputer"
        case "iOS", "Android": return "iphone"
        default: return "questionmark.circle"
        }
    }
}

/// "Enroll This Device": Sign in again, or Approve from another device
/// (the one-time code shows while this Mac waits). The core's
/// `appEnrollView`.
struct EnrollSheet: View {
    @Bindable var devices: DevicesModel

    var body: some View {
        if let v = devices.enrollView {
            VStack(alignment: .leading, spacing: 14) {
                Text(v.title).font(.headline)
                Text(v.lede).foregroundStyle(.secondary)
                if !v.options.isEmpty {
                    VStack(spacing: 8) {
                        ForEach(v.options, id: \.title) { o in
                            Button {
                                Task { await devices.chooseEnroll(o.method) }
                            } label: {
                                HStack(spacing: 10) {
                                    Image(systemName: o.method == .signIn ? "person.crop.circle" : "number")
                                        .font(.title3)
                                        .frame(width: 24)
                                    VStack(alignment: .leading, spacing: 2) {
                                        Text(o.title)
                                        Text(o.detail).font(.caption).foregroundStyle(.secondary)
                                    }
                                    Spacer()
                                    Image(systemName: "chevron.right").foregroundStyle(.tertiary)
                                }
                                .contentShape(Rectangle())
                                .padding(10)
                            }
                            .buttonStyle(.plain)
                            .background(.quaternary.opacity(0.5), in: RoundedRectangle(cornerRadius: 8))
                            .accessibilityIdentifier("enroll-\(o.method == .signIn ? "sign-in" : "approve")")
                        }
                    }
                }
                if let code = v.code {
                    Text(code)
                        .font(.system(size: 30, weight: .semibold, design: .monospaced))
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 6)
                        .accessibilityIdentifier("enroll-code")
                }
                if let help = v.codeHelp {
                    Text((try? AttributedString(markdown: help)) ?? AttributedString(help))
                        .font(.callout).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                }
                if let status = v.status {
                    HStack(spacing: 8) {
                        if v.busy { ProgressView().controlSize(.small) }
                        if v.done { Image(systemName: "checkmark.circle.fill").foregroundStyle(.green) }
                        Text(status).foregroundStyle(.secondary)
                    }
                }
                if let error = v.error {
                    Text(error).foregroundStyle(.red).lineLimit(3)
                }
                HStack {
                    Spacer()
                    if let back = v.backLabel { Button(back) { devices.backEnroll() } }
                    Button(v.closeLabel) { devices.closeEnroll() }
                        .keyboardShortcut(v.done ? .defaultAction : .cancelAction)
                }
            }
            .padding(20)
            .frame(width: 440)
        }
    }
}

/// A device asking to join (or to be re-verified): its name, the code it
/// shows, Approve (Touch ID or the login password first) and Deny. The
/// core's `appApproveView`.
struct DeviceApprovalSheet: View {
    @Bindable var devices: DevicesModel

    var body: some View {
        if let v = devices.approvalView {
            VStack(alignment: .leading, spacing: 14) {
                HStack(spacing: 10) {
                    Image(systemName: "laptopcomputer.and.iphone").font(.title2).foregroundStyle(.secondary)
                    Text(v.title).font(.headline)
                }
                Text(v.message).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                if v.needsCode {
                    TextField(v.codeLabel, text: Binding(get: { v.code }, set: { devices.setCode($0) }),
                              prompt: Text(v.codePlaceholder))
                        .font(.system(size: 20, weight: .medium, design: .monospaced))
                        .textFieldStyle(.roundedBorder)
                        .disabled(v.busy)
                        .onSubmit { Task { await devices.approve() } }
                        .accessibilityIdentifier("approve-code")
                }
                if let error = v.error {
                    Text(error).foregroundStyle(.red).lineLimit(2)
                }
                HStack {
                    if v.busy { ProgressView().controlSize(.small) }
                    Spacer()
                    Button(v.denyLabel) { Task { await devices.deny() } }
                        .disabled(v.busy)
                        .accessibilityIdentifier("approve-deny")
                    Button {
                        Task { await devices.approve() }
                    } label: {
                        Label(v.approveLabel, systemImage: "touchid")
                    }
                    .buttonStyle(.borderedProminent)
                    .keyboardShortcut(.defaultAction)
                    .disabled(!v.canApprove)
                    .accessibilityIdentifier("approve-approve")
                }
            }
            .padding(20)
            .frame(width: 420)
        }
    }
}

/// The main window's slim bar when this Mac needs enrolling or
/// re-verifying (the core's banner).
struct DevicesBanner: View {
    @Bindable var devices: DevicesModel

    var body: some View {
        if let b = devices.banner {
            HStack(spacing: 8) {
                Image(systemName: b.tone == .critical ? "exclamationmark.triangle.fill" : "exclamationmark.circle")
                    .foregroundStyle(b.tone == .critical ? .red : .orange)
                Text(b.text).lineLimit(1).truncationMode(.tail).help(b.text)
                Spacer(minLength: 8)
                if let label = b.actionLabel {
                    Button(label) { devices.startEnroll(in: .main) }
                        .controlSize(.small)
                        .accessibilityIdentifier("banner-enroll")
                }
            }
            .font(.callout)
            .padding(.horizontal, 12)
            .padding(.vertical, 7)
            .background(.bar)
            .overlay(alignment: .bottom) { Divider() }
            .accessibilityIdentifier("devices-banner")
        }
    }
}
