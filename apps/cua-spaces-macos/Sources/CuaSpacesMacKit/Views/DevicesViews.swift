// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import CuaSpacesFFI
import SwiftUI

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

/// The relay allowance for the month is used up: one line above the main
/// window with Upgrade, until dismissed. Nothing stops at the limit, so it
/// only says so. The words are the core's (`appRelayUsageView`).
struct RelayAllowanceBanner: View {
    @Bindable var devices: DevicesModel
    @Environment(\.openURL) private var openURL

    var body: some View {
        if let notice = devices.relayNotice {
            HStack(spacing: 8) {
                Text(notice.text).lineLimit(2).foregroundStyle(.secondary).help(notice.text)
                Spacer(minLength: 8)
                if let url = notice.upgradeUrl.flatMap(URL.init(string:)) {
                    Button(notice.upgrade) {
                        devices.recordUpsell("upsell_meter_click")
                        openURL(url)
                    }
                        .controlSize(.small)
                        .accessibilityIdentifier("relay-upgrade")
                }
                Button {
                    devices.dismissRelayNotice()
                } label: {
                    Image(systemName: "xmark")
                }
                .buttonStyle(.borderless)
                .controlSize(.small)
                .accessibilityLabel(notice.dismiss)
                .accessibilityIdentifier("relay-dismiss")
            }
            .font(.callout)
            .padding(.horizontal, 12)
            .padding(.vertical, 7)
            .background(.bar)
            .overlay(alignment: .bottom) { Divider() }
            .accessibilityIdentifier("relay-allowance")
            .onAppear { devices.recordUpsell("upsell_meter_shown") }
        }
    }
}

/// Sharing is for teams (CUA-1883): one line, once, saying who can no longer
/// open a shared computer, with "Invite to your team". The words are the
/// app core's; closing it tells the account it was seen.
struct ShareRemovalBanner: View {
    @Bindable var model: AppModel
    @Environment(\.openURL) private var openURL

    var body: some View {
        if let note = model.shareRemovalNote {
            HStack(spacing: 8) {
                Text(note.text).lineLimit(2).foregroundStyle(.secondary).help(note.text)
                Spacer(minLength: 8)
                if let url = URL(string: note.action.url) {
                    Button(note.action.label) { openURL(url) }
                        .controlSize(.small)
                        .accessibilityIdentifier("share-removal-invite")
                }
                Button {
                    model.dismissShareRemovalNote()
                } label: {
                    Image(systemName: "xmark")
                }
                .buttonStyle(.borderless)
                .controlSize(.small)
                .accessibilityLabel(note.dismiss)
                .accessibilityIdentifier("share-removal-dismiss")
            }
            .font(.callout)
            .padding(.horizontal, 12)
            .padding(.vertical, 7)
            .background(.bar)
            .overlay(alignment: .bottom) { Divider() }
            .accessibilityIdentifier("share-removal")
        }
    }
}
