// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSDK
import CuaSpacesFFI
import CuaSpacesNotchUI
import Observation
import SwiftUI

/// Owns the panel: picks the screen, asks the core for the layout, feeds
/// window drags to the core, and keeps the tiles' thumbnails fresh while
/// the panel is open. The views (`CuaSpacesNotchUI`, shared with the
/// Electron app's notch helper) read it as their `NotchSurface`.
@MainActor
@Observable
public final class NotchController {
    @ObservationIgnored let model: NotchModel
    @ObservationIgnored private var panel: NotchPanel?
    @ObservationIgnored private var observers: [NSObjectProtocol] = []
    @ObservationIgnored private var dragMonitor: TeleportWindowDragMonitor?
    @ObservationIgnored private var teleport: CuaSpacesFFI.Teleport?
    @ObservationIgnored private var permissionPoll: Timer?
    @ObservationIgnored private var outsideClick: Any?
    @ObservationIgnored private var thumbnailTask: Task<Void, Never>?
    @ObservationIgnored private var lastThumbnails = Date.distantPast
    /// The core's layout for the notch screen (global AppKit points).
    @ObservationIgnored private(set) var layout: AppNotchLayout?
    /// Whether `layout` has the row above the tiles.
    @ObservationIgnored private var layoutRow = false
    /// The same, in the panel's own coordinates, for the view.
    public private(set) var geometry = NotchGeometry.fallback
    /// The dragged window's app, classified by the SDK at drag start.
    @ObservationIgnored private var draggedApp: TeleportCatalogEntry?
    /// The core's window-drag trigger: move or resize, the line above the
    /// notch, the dwell, expand and collapse.
    @ObservationIgnored private(set) var trigger = appDragTriggerInitial()
    /// The notch screen's trigger geometry (global top-left points).
    @ObservationIgnored private(set) var dragDisplays: [AppDragDisplay] = []
    /// The primary display's top (AppKit), for the drag events' top-left
    /// points.
    @ObservationIgnored private var primaryTop: Double = 0
    /// Runs the trigger's dwell tick; tests step a manual clock.
    @ObservationIgnored var scheduler: NotchScheduler = SystemNotchScheduler()
    /// Milliseconds on a monotonic clock (the trigger's times).
    @ObservationIgnored var clock: () -> UInt64 = { UInt64(ProcessInfo.processInfo.systemUptime * 1000) }
    @ObservationIgnored private var cancelTick: (() -> Void)?
    /// A drag committed to a Space: open its teleport review on that app.
    @ObservationIgnored public var onTeleport: ((String, TeleportCatalogEntry?) -> Void)?
    /// A tile was clicked.
    @ObservationIgnored public var onOpenSpace: ((String) -> Void)?
    /// The live-access line was clicked: the Keyvault's Access page.
    @ObservationIgnored public var onOpenAccess: (() -> Void)?
    /// Its Dismiss: hide the indicator and the tiles' key (nothing is
    /// revoked or wiped).
    @ObservationIgnored public var onDismissAccess: (() -> Void)?
    /// Opens (or brings forward) the main window, which may have been
    /// closed while the notch stayed up. Set by the notch view.
    @ObservationIgnored public var showMain: (() -> Void)?
    /// Opens Settings. Set by the notch view.
    @ObservationIgnored var showSettings: (() -> Void)?
    /// Apps or files were dropped on a tile.
    @ObservationIgnored public var onDropURLs: ((String, [URL]) -> Void)?
    /// The main window's Space rows, the other target for a window drag.
    @ObservationIgnored public var sidebar: SidebarDropTargets?
    /// A Space's screenshot for its tile (the SDK's `Space.screenshot`).
    @ObservationIgnored public var thumbnail: ((String) async -> NSImage?)?
    /// Opens a URL (System Settings); tests capture it.
    @ObservationIgnored var openURL: (URL) -> Void = { NSWorkspace.shared.open($0) }
    /// The permission check; defaults to the SDK's.
    @ObservationIgnored var permitted: () -> Bool = { false }

    public init(model: NotchModel) {
        self.model = model
        model.onCommit = { [weak self] spaceId, _ in
            guard let self else { return }
            self.onTeleport?(spaceId, self.draggedApp)
            self.draggedApp = nil
        }
    }

    /// The panel's window (captures and tests).
    var window: NSWindow? { panel }

    /// The search field takes the keyboard: the non-activating panel becomes
    /// key without bringing the app forward.
    public func takeKeyboard() { panel?.makeKey() }

    /// Whether the notch shows ("Spaces tab in the notch"). Hidden, nothing
    /// is on screen and neither hover nor window drags are followed.
    public private(set) var isShown = true
    /// Tests turn this off so `show()` never puts a panel on screen.
    @ObservationIgnored var makesPanel = true

    /// Shows or hides the notch (the setting), immediately.
    public func setShown(_ shown: Bool) {
        if shown { show() } else { hide() }
    }

    /// Shows the panel on the notched display (else the main one).
    public func show() {
        if isShown, panel != nil { return }
        if !isShown {
            isShown = true
            model.send(.visibility(shown: true))
        }
        if let panel {
            place()
            panel.orderFrontRegardless()
            recheckPermission()
            return
        }
        guard makesPanel else {
            recheckPermission()
            return
        }
        let panel = NotchPanel()
        let view = NotchHostingView(rootView: NotchContentView(surface: self))
        view.sizingOptions = []
        panel.contentView = view
        self.panel = panel
        place()
        panel.orderFrontRegardless()
        observers.append(NotificationCenter.default.addObserver(
            forName: NSApplication.didChangeScreenParametersNotification, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated {
                self?.place()
                // `NSScreen.screens` can lag the notification.
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { self?.place() }
            }
        })
        observers.append(NotificationCenter.default.addObserver(
            forName: NSApplication.didBecomeActiveNotification, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.recheckPermission() }
        })
        watchLayoutInputs()
    }

    public func hide() {
        isShown = false
        model.send(.visibility(shown: false))
        panel?.orderOut(nil)
        dragMonitor?.stop()
        dragMonitor = nil
        dragsRunning = false
        trigger = appDragTriggerInitial()
        cancelTick?()
        cancelTick = nil
        permissionPoll?.invalidate()
        permissionPoll = nil
        stageChanged(open: false)
    }

    /// The screen: the built-in notched display when there is one.
    static func screen() -> NSScreen? { NotchScreens.screen() }

    static func facts(_ s: NSScreen) -> AppScreenFacts { NotchScreens.facts(s).ffi }

    /// The layout needs the prompt row when the open panel shows a line
    /// above the tiles (a drop hint or the permission line).
    static func needsRow(_ v: AppNotchView) -> Bool { NotchScreens.needsRow(v.data) }

    /// Re-reads the layout and sizes the panel to its stage frame. The panel
    /// only changes size here (screen change, the row appearing), never
    /// during a spring.
    func place() {
        place(screen: Self.screen().map(Self.facts), primary: NotchScreens.primary().map(Self.facts))
    }

    /// Lays out for a screen (and the primary one, whose top the drag
    /// events measure from).
    func place(screen facts: AppScreenFacts?, primary: AppScreenFacts?) {
        layoutRow = Self.needsRow(model.view)
        apply(facts.map { appNotchLayout(screen: $0, prompt: layoutRow) })
        guard let facts else { return }
        let primary = primary ?? facts
        primaryTop = primary.frame.y + primary.frame.height
        // The primary first (it sets the top), then the notch screen: the
        // panel only shows there, so it is the one trigger display.
        let all = appDragDisplays(screens: primary == facts ? [facts] : [primary, facts])
        dragDisplays = all.last.map { [$0] } ?? []
    }

    func apply(_ layout: AppNotchLayout?) {
        guard let layout else { return }
        self.layout = layout
        let g = NotchGeometry(layout.data)
        if g != geometry {
            withAnimation(.spring(response: NotchModel.motion.openResponse,
                                  dampingFraction: NotchModel.motion.closeDamping)) { geometry = g }
        }
        let s = layout.stageFrame
        let frame = NSRect(x: s.x, y: s.y, width: s.width, height: s.height)
        if let panel, panel.frame != frame { panel.setFrame(frame, display: true) }
    }

    /// Re-places when the row above the tiles comes or goes.
    private func watchLayoutInputs() {
        let row = withObservationTracking { Self.needsRow(model.view) } onChange: { [weak self] in
            DispatchQueue.main.async { self?.watchLayoutInputs() }
        }
        if row != layoutRow { place() }
    }

    /// The view finished stepping to a new stage.
    public func stageChanged(open: Bool) {
        if open {
            if outsideClick == nil {
                // Observes only: the click still reaches the app under it.
                outsideClick = NSEvent.addGlobalMonitorForEvents(matching: [.leftMouseDown, .rightMouseDown]) { [weak self] _ in
                    MainActor.assumeIsolated { self?.clickedOutside() }
                }
            }
            startThumbnails()
        } else {
            if let outsideClick { NSEvent.removeMonitor(outsideClick) }
            outsideClick = nil
            thumbnailTask?.cancel()
            thumbnailTask = nil
        }
        if model.view.hoverCue { refreshThumbnailsSoon() }
    }

    private func clickedOutside() {
        guard let panel, model.state.drag.phase == .idle else { return }
        if !panel.frame.contains(NSEvent.mouseLocation) { model.send(.dismiss) }
    }

    // MARK: - Thumbnails

    /// While open: every tile's screenshot now, then every 3 s.
    private func startThumbnails() {
        guard thumbnail != nil, thumbnailTask == nil else { return }
        thumbnailTask = Task { [weak self] in
            // Bounded: stops when the panel closes (cancel) or after 10 min.
            for _ in 0..<200 {
                guard let self, !Task.isCancelled else { return }
                await self.refreshThumbnails()
                try? await Task.sleep(for: .seconds(3))
            }
        }
    }

    /// The hover cue warms the thumbnails before the panel opens.
    private func refreshThumbnailsSoon() {
        guard thumbnail != nil, Date().timeIntervalSince(lastThumbnails) > 3 else { return }
        Task { await refreshThumbnails() }
    }

    private func refreshThumbnails() async {
        guard let thumbnail else { return }
        lastThumbnails = Date()
        var open = appNotchInitial()
        open.open = true
        let live = appNotchView(state: open, spaces: model.spaces).tiles.filter { $0.dropTarget && !$0.dim }
        await withTaskGroup(of: (String, NSImage?).self) { group in
            for tile in live {
                group.addTask { (tile.id, await thumbnail(tile.id)) }
            }
            for await (id, image) in group {
                if let image { model.thumbnails[id] = image }
            }
        }
    }

    // MARK: - Window drags onto the notch

    /// Follows window drags. Detection needs the Accessibility permission;
    /// without it the open panel says so (the core's line) and this checks
    /// again when the app becomes active and every 2 s until it is granted.
    public func followWindowDrags(teleport: CuaSpacesFFI.Teleport) {
        guard teleport.windowDragSupported() else { return }
        self.teleport = teleport
        permitted = { teleport.windowDragPermitted() }
        startDrags = { [weak self] in
            let listener = DragListener { event in
                Task { @MainActor in self?.handle(event) }
            }
            // The SDK waits up to 5 s for its event tap to start: never on
            // the main thread (it would stall the windows' first frames).
            Task.detached {
                do {
                    let monitor = try teleport.startWindowDrag(listener: listener)
                    await MainActor.run { self?.dragsStarted(monitor) ?? monitor.stop() }
                } catch {
                    NSLog("Cua Spaces: window-drag detection did not start: \(error)")
                    await MainActor.run { self?.dragsFailed() }
                }
            }
        }
        recheckPermission()
    }

    /// The SDK's monitor is up: kept while drags are still followed (the
    /// notch may have been hidden meanwhile).
    private func dragsStarted(_ monitor: TeleportWindowDragMonitor) {
        guard isShown, dragsRunning, dragMonitor == nil else { return monitor.stop() }
        dragMonitor = monitor
    }

    /// The monitor did not start after all: say so and check again later.
    private func dragsFailed() {
        guard dragsRunning, dragMonitor == nil else { return }
        dragsRunning = false
        model.send(.dragPermission(granted: false))
        pollPermission()
    }

    /// Starts the SDK's drag monitor without waiting for it (set by
    /// `followWindowDrags`; tests inject their own).
    @ObservationIgnored var startDrags: (() throws -> Void)?
    @ObservationIgnored private(set) var dragsRunning = false

    func recheckPermission() {
        guard isShown, let startDrags, !dragsRunning else { return }
        if permitted() {
            do {
                try startDrags()
                dragsRunning = true
                permissionPoll?.invalidate()
                permissionPoll = nil
                model.send(.dragPermission(granted: true))
                return
            } catch {
                NSLog("Cua Spaces: window-drag detection did not start: \(error)")
            }
        }
        model.send(.dragPermission(granted: false))
        pollPermission()
    }

    /// Checks again every 2 s (until the monitor runs or the notch hides).
    private func pollPermission() {
        guard isShown, permissionPoll == nil else { return }
        permissionPoll = Timer.scheduledTimer(withTimeInterval: 2, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.recheckPermission() }
        }
    }

    /// The permission line's button: registers the app for the permission
    /// (so it is listed) and opens that System Settings pane.
    public func openPermissionSettings(pane: String) {
        if let teleport { _ = try? teleport.requestWindowDragPermission() }
        if let url = Self.settingsURL(pane) { openURL(url) }
    }

    static func settingsURL(_ pane: String) -> URL? {
        switch pane {
        case "accessibility":
            return URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
        default:
            return URL(string: "x-apple.systempreferences:com.apple.preference.security")
        }
    }

    func handle(_ event: TeleportWindowDragEvent) {
        // Hidden: window drags are not followed (the monitor is stopped; a
        // late event is dropped).
        guard isShown else { return }
        let wasMove = trigger.kind == .move
        let now = clock()
        if event.phase == "start" { draggedApp = event.app }
        if event.phase != "start", let frame = event.frame { feed(.frame(frame: frame), event) }
        switch event.phase {
        case "start":
            feed(.start(windowId: event.window?.windowId, appName: event.window?.appName ?? event.app?.name,
                        x: event.x, y: event.y, tMs: now, startFrame: event.startFrame, frame: event.frame), event)
        case "move":
            feed(.cursor(x: event.x, y: event.y, tMs: now), event)
        case "end":
            feed(.end(x: event.x, y: event.y, tMs: now), event)
        default:
            feed(.cancel, event)
        }
        // A moved window released over a Space row in the main window
        // commits there; the notch drag then ends without a drop. A resize
        // never targets a row.
        guard let sidebar else { return }
        let isMove = event.phase == "end" ? wasMove : trigger.kind == .move
        guard isMove else {
            sidebar.handle(phase: "cancel", target: nil)
            return
        }
        let target = sidebar.row(atGlobal: CGPoint(x: event.x, y: event.y), dragged: event.window?.windowId)
        if let spaceId = sidebar.handle(phase: event.phase, target: target) {
            model.send(.drag(event: .cancel))
            onTeleport?(spaceId, event.app ?? draggedApp)
            draggedApp = nil
        }
    }

    /// Runs one trigger event: its overlay events go to the notch (a drop
    /// carries the tile under the cursor), the tiles are hit-tested while
    /// expanded, and the dwell tick is scheduled.
    private func feed(_ e: AppDragTriggerEvent, _ event: TeleportWindowDragEvent) {
        let t = appDragTriggerApply(state: trigger, event: e, displays: dragDisplays)
        trigger = t.state
        for o in t.overlay {
            if case .drop = o {
                model.send(.drag(event: .drop(spaceId: tile(atGlobal: event.x, event.y))))
            } else {
                model.send(.drag(event: o))
            }
        }
        if t.state.phase == .expanded {
            if let tile = tile(atGlobal: event.x, event.y) {
                model.send(.drag(event: .over(spaceId: tile)))
            } else {
                model.send(.drag(event: .out))
            }
        }
        cancelTick?()
        cancelTick = nil
        if let at = t.tickAtMs {
            let wait = at > clock() ? at - clock() : 0
            cancelTick = scheduler.after(UInt32(min(wait, UInt64(UInt32.max)))) { [weak self] in
                guard let self else { return }
                self.cancelTick = nil
                self.feed(.tick(tMs: self.clock()), event)
            }
        }
    }

    /// The drop-target tile under a global top-left point (the core's hit
    /// test over this panel's layout).
    func tile(atGlobal x: Double, _ y: Double) -> String? {
        guard let layout, model.view.phase == .tiles else { return nil }
        return appNotchTileAt(layout: layout, tiles: model.view.tiles, row: Self.needsRow(model.view),
                              x: x, y: primaryTop - y)
    }
}

// MARK: - The views' surface

extension NotchController: NotchSurface {
    public var view: NotchData.View { model.view.data }
    public var query: String { model.state.query }
    public var motion: NotchData.Motion { NotchModel.motion.data }
    public var radii: NotchData.RadiiPair { NotchModel.radii }
    public var highlight: NotchHighlight? { model.highlight }
    public var ghost: NSImage? { model.ghost }
    public func thumbnail(_ spaceId: String) -> NSImage? { model.thumbnails[spaceId] }
    public func osIcon(_ id: String) -> NotchData.OsIcon? { NotchModel.osIcon(id) }
    public func estimatedProgress(elapsedMs: Int64, estimateMs: UInt32) -> UInt32 {
        appNotchEstimatedProgress(elapsedMs: elapsedMs, estimateMs: estimateMs)
    }

    public func send(_ event: NotchData.Event) { model.send(event.ffi) }
    public func search(_ query: String) { model.search(query) }
    public func openSpace(_ spaceId: String) { onOpenSpace?(spaceId) }

    /// A header button: closes the panel and brings the app forward on the
    /// main window or Settings.
    public func run(_ button: NotchData.ButtonId) {
        model.send(.dismiss)
        NSApp.activate()
        switch button {
        case .list: showMain?()
        case .settings: showSettings?()
        }
    }

    public func openAccess() { onOpenAccess?() }
    public func dismissAccess() { onDismissAccess?() }
    public func drop(_ urls: [URL], on spaceId: String) { onDropURLs?(spaceId, urls) }

    public func attach(openMain: @escaping () -> Void, openSettings: @escaping () -> Void) {
        showMain = openMain
        showSettings = openSettings
    }
}

final class DragListener: TeleportWindowDragListener, @unchecked Sendable {
    let handler: (TeleportWindowDragEvent) -> Void
    init(_ handler: @escaping (TeleportWindowDragEvent) -> Void) { self.handler = handler }
    func onEvent(event: TeleportWindowDragEvent) { handler(event) }
}
