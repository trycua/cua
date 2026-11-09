// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import SwiftUI

/// The notice under the menu bar item when the window closes and Cua Spaces
/// keeps running (`BackgroundPolicy.windowClosed`): a small popover, the
/// kind the menu bar items of Tailscale and UTM show. "Don't show this
/// again" turns it off (Settings, General has the switch back);
/// "Settings" opens the page. It closes itself after a while, or on a click
/// elsewhere, and never takes the keyboard.
@MainActor
public final class BackgroundNotice: NSObject, NSPopoverDelegate {
    public static let shared = BackgroundNotice()

    private var popover: NSPopover?
    private var panel: NSPanel?
    private var timer: Timer?

    /// How long it stays when nobody touches it.
    static let visibleFor: TimeInterval = 14

    /// The status item's window of this app (the menu bar icon), when there is one.
    static func statusWindow() -> NSWindow? {
        // An item-sized window: the menu bar's own (a full-width one is not the icon).
        NSApp.windows.first {
            String(describing: type(of: $0)).contains("StatusBar") && $0.isVisible
                && $0.frame.width < 300 && $0.frame.height < 60
        }
    }

    public func show(turnOff: @escaping @MainActor () -> Void, openSettings: @escaping @MainActor () -> Void) {
        dismiss()
        let anchor = Self.statusWindow()?.contentView
        NSLog("Cua Spaces: notice shown %@", anchor == nil ? "at the top right" : "under the menu bar icon")
        let view = BackgroundNoticeView(
            card: anchor == nil,
            turnOff: { [weak self] in turnOff(); self?.dismiss() },
            openSettings: { [weak self] in openSettings(); self?.dismiss() })
        let host = NSHostingController(rootView: view)
        host.sizingOptions = [.preferredContentSize]
        if let anchor {
            let popover = NSPopover()
            popover.behavior = .transient
            popover.animates = true
            popover.contentViewController = host
            popover.delegate = self
            popover.show(relativeTo: anchor.bounds, of: anchor, preferredEdge: .minY)
            self.popover = popover
        } else if let screen = NSScreen.main {
            // No menu bar item to point at (a hidden or full menu bar): the
            // same card at the top right, under the menu bar.
            let size = host.view.fittingSize
            let frame = NSRect(x: screen.visibleFrame.maxX - size.width - 12, y: screen.visibleFrame.maxY - size.height - 8,
                               width: size.width, height: size.height)
            let panel = NSPanel(contentRect: frame, styleMask: [.nonactivatingPanel, .fullSizeContentView],
                                backing: .buffered, defer: false)
            panel.contentViewController = host
            panel.level = .statusBar
            panel.isOpaque = false
            panel.backgroundColor = .clear
            panel.hasShadow = true
            panel.collectionBehavior = [.canJoinAllSpaces, .transient]
            panel.setFrame(frame, display: true)
            panel.orderFrontRegardless()
            self.panel = panel
        }
        timer = Timer.scheduledTimer(withTimeInterval: Self.visibleFor, repeats: false) { [weak self] _ in
            Task { @MainActor in self?.dismiss() }
        }
    }

    public func dismiss() {
        timer?.invalidate()
        timer = nil
        popover?.close()
        popover = nil
        panel?.orderOut(nil)
        panel = nil
    }

    public func popoverDidClose(_ notification: Notification) { popover = nil }
}

struct BackgroundNoticeView: View {
    /// Drawn as its own card (no popover around it).
    var card = false
    let turnOff: () -> Void
    let openSettings: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(BackgroundPolicy.noticeTitle).font(.headline)
            Text(BackgroundPolicy.noticeText)
                .font(.callout)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            HStack {
                Spacer()
                Button("Don\u{2019}t Show Again", action: turnOff)
                Button("Settings\u{2026}", action: openSettings).keyboardShortcut(.defaultAction)
            }
            .padding(.top, 6)
        }
        .padding(14)
        .frame(width: 300)
        .background {
            if card {
                RoundedRectangle(cornerRadius: 10, style: .continuous).fill(.regularMaterial)
                    .shadow(radius: 8, y: 2)
            }
        }
    }
}
