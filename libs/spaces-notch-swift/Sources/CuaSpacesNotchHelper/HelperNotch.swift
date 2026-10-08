// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSpacesNotchUI
import Observation
import SwiftUI

/// The helper's notch: the shared views' surface over the notch protocol.
/// Electron's core decides everything (the view, the layout, timing); this
/// holds the latest state, owns the panel and turns input into messages.
@MainActor
@Observable
public final class HelperNotch: NotchSurface {
    public private(set) var view: NotchData.View = .empty
    public private(set) var query = ""
    public private(set) var geometry = NotchGeometry(HelperNotch.unplaced)
    public private(set) var motion = HelperNotch.unsetMotion
    public private(set) var radii = NotchData.RadiiPair(closed: .init(top: 0, bottom: 0), open: .init(top: 0, bottom: 0))
    public private(set) var highlight: NotchHighlight?
    public private(set) var ghost: NSImage?
    /// Latest thumbnail per Space id.
    public private(set) var thumbnails: [String: NSImage] = [:]
    /// The layout for the notch screen (none until Electron sent one).
    @ObservationIgnored public private(set) var layout: NotchData.Layout?
    /// Whether the notch shows (the setting).
    @ObservationIgnored public private(set) var shown = false
    /// A window drag runs (a click outside then does not dismiss).
    @ObservationIgnored public private(set) var dragging = false
    /// Hello came (motion and radii are the core's).
    @ObservationIgnored public private(set) var greeted = false
    @ObservationIgnored private var icons: [String: NotchData.OsIcon] = [:]
    /// Search texts sent and not yet echoed by a state, oldest first: a
    /// state carrying an older one never undoes newer typing.
    @ObservationIgnored private var sentQueries: [String] = []
    @ObservationIgnored private let emit: (HelperMessage) -> Void
    @ObservationIgnored private var panel: NotchPanel?
    @ObservationIgnored private var outsideClick: Any?
    @ObservationIgnored private var observers: [NSObjectProtocol] = []
    /// Tests and `--selftest` turn this off so nothing goes on screen.
    @ObservationIgnored public var makesPanel = true

    public init(emit: @escaping (HelperMessage) -> Void) {
        self.emit = emit
    }

    /// The panel's window (tests).
    public var window: NSWindow? { panel }

    /// Applies one message from Electron; false for `quit`.
    @discardableResult
    public func receive(_ message: HostMessage) -> Bool {
        switch message {
        case .hello(let version, let motion, let radii):
            guard version == NotchProtocol.version else {
                emit(.error(message: "Electron speaks notch protocol \(version); this helper speaks \(NotchProtocol.version)"))
                return false
            }
            self.motion = motion
            self.radii = radii
            greeted = true
            emit(.hello(version: NotchProtocol.version, pid: ProcessInfo.processInfo.processIdentifier))
            reportScreens()
        case .state(let s):
            icons = s.icons
            if s.view != view { view = s.view }
            acceptQuery(s.query)
            highlight = s.highlight.flatMap { NotchHighlight.parse($0, firstTile: s.view.tiles.first(where: \.dropTarget)?.id) }
            dragging = s.dragging
            if let layout = s.layout { apply(layout) }
            if s.shown != shown || panel == nil { setShown(s.shown) }
        case .thumbnail(let id, let image):
            thumbnails[id] = image.flatMap(NSImage.init(data:))
        case .ghost(let image):
            ghost = image.flatMap(NSImage.init(data:))
        case .quit:
            return false
        }
        return true
    }

    /// Re-reads the screens and tells Electron (it answers with a layout).
    public func reportScreens() {
        emit(.screens(notch: NotchScreens.screen().map(NotchScreens.facts),
                      primary: NotchScreens.primary().map(NotchScreens.facts)))
    }

    /// Sizes the panel to the layout's stage frame, springing the shape to
    /// the new geometry (as the SwiftUI app does).
    func apply(_ layout: NotchData.Layout) {
        let g = NotchGeometry(layout)
        let first = self.layout == nil
        self.layout = layout
        if g != geometry {
            if first {
                geometry = g
            } else {
                withAnimation(.spring(response: motion.openResponse, dampingFraction: motion.closeDamping)) { geometry = g }
            }
        }
        let s = layout.stageFrame
        let frame = NSRect(x: s.x, y: s.y, width: s.width, height: s.height)
        if let panel, panel.frame != frame { panel.setFrame(frame, display: true) }
    }

    /// Shows the panel once there is a layout and the setting allows it;
    /// hides it otherwise.
    func setShown(_ on: Bool) {
        shown = on
        guard on, greeted, let layout else {
            panel?.orderOut(nil)
            stopOutsideClicks()
            return
        }
        guard makesPanel else { return }
        if panel == nil {
            let panel = NotchPanel()
            let host = NotchHostingView(rootView: NotchContentView(surface: self))
            host.sizingOptions = []
            panel.contentView = host
            self.panel = panel
            watchScreens()
        }
        let s = layout.stageFrame
        panel?.setFrame(NSRect(x: s.x, y: s.y, width: s.width, height: s.height), display: true)
        panel?.orderFrontRegardless()
    }

    private func watchScreens() {
        guard observers.isEmpty else { return }
        observers.append(NotificationCenter.default.addObserver(
            forName: NSApplication.didChangeScreenParametersNotification, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated {
                self?.reportScreens()
                // `NSScreen.screens` can lag the notification.
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { self?.reportScreens() }
            }
        })
    }

    private func stopOutsideClicks() {
        if let outsideClick { NSEvent.removeMonitor(outsideClick) }
        outsideClick = nil
    }

    private func clickedOutside() {
        guard let panel, !dragging else { return }
        if !panel.frame.contains(NSEvent.mouseLocation) { send(.dismiss) }
    }

    // MARK: - NotchSurface

    public func thumbnail(_ spaceId: String) -> NSImage? { thumbnails[spaceId] }
    public func osIcon(_ id: String) -> NotchData.OsIcon? { icons[id] }
    /// Electron sends real or estimated progress in `permille`.
    public func estimatedProgress(elapsedMs: Int64, estimateMs: UInt32) -> UInt32 { 0 }

    public func send(_ event: NotchData.Event) { emit(.event(event)) }

    public func search(_ query: String) {
        guard query != self.query else { return }
        self.query = query
        sentQueries.append(query)
        if sentQueries.count > 64 { sentQueries.removeFirst() }
        emit(.event(.search(query: query)))
    }

    /// A state's search text: an echo of typing still in flight is skipped;
    /// anything else (the core cleared it) wins.
    private func acceptQuery(_ q: String) {
        if let i = sentQueries.firstIndex(of: q) {
            sentQueries.removeFirst(i + 1)
            if !sentQueries.isEmpty { return }
        } else {
            sentQueries.removeAll()
        }
        if q != query { query = q }
    }

    public func openSpace(_ spaceId: String) { emit(.action(.openSpace(spaceId: spaceId))) }

    /// A header button: closes the panel; Electron brings its window forward
    /// on the Spaces list or Settings.
    public func run(_ button: NotchData.ButtonId) {
        emit(.event(.dismiss))
        switch button {
        case .list: emit(.action(.openMain))
        case .settings: emit(.action(.openSettings))
        }
    }

    public func openAccess() { emit(.action(.openAccess)) }
    public func dismissAccess() { emit(.action(.dismissAccess)) }
    public func openPermissionSettings(pane: String) { emit(.action(.openPermissionSettings(pane: pane))) }
    public func drop(_ urls: [URL], on spaceId: String) {
        emit(.action(.drop(spaceId: spaceId, paths: urls.map(\.path))))
    }

    public func stageChanged(open: Bool) {
        if open {
            if outsideClick == nil, makesPanel {
                // Observes only: the click still reaches the app under it.
                outsideClick = NSEvent.addGlobalMonitorForEvents(matching: [.leftMouseDown, .rightMouseDown]) { [weak self] _ in
                    MainActor.assumeIsolated { self?.clickedOutside() }
                }
            }
        } else {
            stopOutsideClicks()
        }
        emit(.stage(open: open))
    }

    public func takeKeyboard() { panel?.makeKey() }

    /// The helper has no scenes: Electron opens its own windows.
    public func attach(openMain: @escaping () -> Void, openSettings: @escaping () -> Void) {}

    // MARK: - Before the first state

    /// A zero-size layout (nothing is on screen until Electron sends one).
    static let unplaced: NotchData.Layout = {
        let zero = NotchData.Rect(x: 0, y: 0, width: 0, height: 0)
        return NotchData.Layout(hasNotch: false, notch: zero, closedFrame: zero, openFrame: zero, promptFrame: zero,
                                tabFrame: zero, tabInsetNotch: 0, tabInsetOuter: 0, stageFrame: zero, notchStyle: false)
    }()

    /// Placeholder motion until `hello` brings the core's (never drawn: the
    /// panel waits for it).
    static let unsetMotion = NotchData.Motion(
        hoverDwellMs: 0, closeDelayMs: 0, openResponse: 0.4, openDamping: 1, closeResponse: 0.4, closeDamping: 1,
        reducedDuration: 0.2, hoverResponse: 0.3, hoverDamping: 1, hoverScale: 1, hoverScaleY: 1, contentDelayMs: 0,
        contentIn: 0.2, contentOut: 0.2, contentScale: 1)
}
