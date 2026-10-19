// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import SwiftUI

/// The one-time sheet that moves this Mac's sharing service into the app,
/// as "Cua Spaces Helper", and walks through the one thing macOS asks
/// again: Screen Recording and Accessibility for the helper
/// (`HostModel.helperMove`).
public struct HelperMoveSheet: View {
    let host: HostModel

    public init(host: HostModel) {
        self.host = host
    }

    static let screenRecordingURL =
        "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
    static let accessibilityURL =
        "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"

    public var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            switch host.helperMove {
            case .offer, nil:
                offer
            case .moving:
                HStack(spacing: 10) {
                    ProgressView().controlSize(.small)
                    Text("Moving the helper. This takes up to a minute.")
                }
                .frame(minHeight: 80)
            case .moved(let permissions):
                moved(permissions: permissions)
            case .rolledBack(let detail):
                rolledBack(detail)
            }
        }
        .padding(22)
        .frame(width: 460)
    }

    private var offer: some View {
        VStack(alignment: .leading, spacing: 14) {
            Label("Cua Spaces moved its background helper", systemImage: "arrow.triangle.2.circlepath")
                .font(.headline)
            Text("macOS needs you to allow it once more.")
            Text("The helper is what lets people and agents you share this Mac with see the screen and use the mouse and keyboard. It now lives inside Cua Spaces, so macOS remembers your answer after updates.")
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            HStack {
                Spacer()
                Button("Not now") { host.declineHelperMove() }
                    .keyboardShortcut(.cancelAction)
                Button("Continue") { Task { await host.moveToHelper() } }
                    .keyboardShortcut(.defaultAction)
            }
        }
    }

    @ViewBuilder
    private func moved(permissions: Bool) -> some View {
        VStack(alignment: .leading, spacing: 14) {
            Label("Allow Cua Spaces Helper", systemImage: "checkmark.shield")
                .font(.headline)
            if permissions {
                Text("Turn these on in System Settings:")
                step(1, "Screen Recording: Cua Spaces", "Open", Self.screenRecordingURL)
                step(2, "Accessibility: Cua Spaces Helper", "Open", Self.accessibilityURL)
                Text("The old entry there is called \u{201C}cua-spacesd\u{201D}. You can remove it: click it, then click the minus (\u{2212}) button below the list.")
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            } else {
                Text("All set. Your Mac kept its name, its Spaces and who it is shared with.")
            }
            HStack {
                Spacer()
                Button("Done") { host.closeHelperMove() }
                    .keyboardShortcut(.defaultAction)
            }
        }
    }

    private func step(_ n: Int, _ list: String, _ button: String, _ url: String) -> some View {
        HStack(alignment: .firstTextBaseline) {
            Text("\(n).").monospacedDigit()
            Text(list)
            Spacer()
            Button(button) {
                if let u = URL(string: url) { NSWorkspace.shared.open(u) }
            }
        }
    }

    private func rolledBack(_ detail: String) -> some View {
        VStack(alignment: .leading, spacing: 14) {
            Label("Nothing changed", systemImage: "arrow.uturn.backward")
                .font(.headline)
            Text("The helper could not move this time, so the old one keeps running and this Mac stays shared. Cua Spaces tries again after the next update.")
                .fixedSize(horizontal: false, vertical: true)
            DisclosureGroup("Details") {
                Text(detail).font(.caption).textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            HStack {
                Spacer()
                Button("OK") { host.closeHelperMove() }
                    .keyboardShortcut(.defaultAction)
            }
        }
    }
}

/// The move's own window, over whichever UI is up (the New UI window or the
/// main one). At a login launch it waits until the app is brought to the
/// front, so it only ever shows to someone who is there.
@MainActor
public final class HelperMoveWindow: NSObject, NSWindowDelegate {
    private static var shared: HelperMoveWindow?
    private let host: HostModel
    private var window: NSWindow?
    private var waiting: NSObjectProtocol?

    private init(host: HostModel) { self.host = host }

    public static func follow(_ host: HostModel, loginLaunch: Bool) {
        let me = HelperMoveWindow(host: host)
        shared = me
        host.onHelperMove = { [weak me] move in
            guard let me else { return }
            if move == nil { me.close(); return }
            if move != .offer { me.window?.level = .normal }
            if loginLaunch && !NSApp.isActive {
                me.waitForFront()
            } else {
                me.show()
            }
        }
    }

    private func waitForFront() {
        guard waiting == nil else { return }
        waiting = NotificationCenter.default.addObserver(
            forName: NSApplication.didBecomeActiveNotification, object: nil, queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated {
                guard let self else { return }
                if let w = self.waiting { NotificationCenter.default.removeObserver(w) }
                self.waiting = nil
                if self.host.helperMove != nil { self.show() }
            }
        }
    }

    private func show() {
        if let window { window.makeKeyAndOrderFront(nil); return }
        let w = NSWindow(contentViewController: NSHostingController(rootView: HelperMoveSheet(host: host)))
        w.title = "Cua Spaces"
        w.styleMask = [.titled, .closable]
        w.isReleasedWhenClosed = false
        w.delegate = self
        // Above the app's other windows while it asks (the New UI opens after
        // it); an ordinary window once System Settings needs the front.
        w.level = host.helperMove == .offer ? .floating : .normal
        w.setContentSize(NSSize(width: 460, height: 260))
        w.center()
        window = w
        NSApp.activate()
        w.makeKeyAndOrderFront(nil)
        w.orderFrontRegardless()
    }

    private func close() {
        window?.delegate = nil
        window?.close()
        window = nil
    }

    public func windowWillClose(_ notification: Notification) {
        window = nil
        // Closed with the button: as "Not now" (or Done).
        switch host.helperMove {
        case .moving: break
        case .offer: host.declineHelperMove()
        default: host.closeHelperMove()
        }
    }
}
