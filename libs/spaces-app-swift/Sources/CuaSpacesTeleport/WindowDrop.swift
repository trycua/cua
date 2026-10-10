// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CoreGraphics
import Cua
import CuaSpacesFFI
import Foundation

/// The drop zone while a real app window is dragged toward a Space: the
/// dragged window's preview (the SDK's in-memory thumbnail of that one
/// window), its app and capability, and the Space under the cursor.
public struct WindowDropState: Equatable, Sendable {
    public var active = false
    public var window: TeleportWindow?
    public var app: TeleportCatalogEntry?
    /// PNG bytes, kept in memory only.
    public var thumbnail: Data?
    public var overId: String?
    public var x: Double = 0
    public var y: Double = 0

    public init() {}

    /// What a release over a Space commits.
    public struct Commit: Equatable, Sendable {
        public let targetId: String
        public let app: TeleportCatalogEntry
        public let window: TeleportWindow
    }

    /// A monitor event. `start` of an app teleport cannot bring up is
    /// ignored; returns the window id to capture on `start`, and the commit
    /// on `end` over a target.
    public mutating func apply(_ e: TeleportWindowDragEvent) -> (capture: UInt32?, commit: Commit?) {
        switch e.phase {
        case "start":
            // A window resized from an edge or a corner is not dragged
            // anywhere (the core compares its frames).
            let resized = e.startFrame.flatMap { s in e.frame.map { appDragClassify(start: s, now: $0) } } == .resize
            guard let w = e.window, let a = e.app, a.capability != .unsupported, !resized else {
                self = WindowDropState()
                return (nil, nil)
            }
            self = WindowDropState()
            active = true
            window = w
            app = a
            x = e.x
            y = e.y
            return (w.windowId, nil)
        case "move":
            if active { x = e.x; y = e.y }
            return (nil, nil)
        default:
            let commit: Commit? = active ? overId.flatMap { id in
                app.flatMap { a in window.map { Commit(targetId: id, app: a, window: $0) } }
            } : nil
            self = WindowDropState()
            return (nil, commit)
        }
    }
}

/// Relays `Teleport.startWindowDrag` (macOS; Accessibility permission) to
/// the main actor and keeps a ``WindowDropState``.
@MainActor
public final class WindowDragWatcher: ObservableObject {
    @Published public private(set) var state = WindowDropState()
    /// Called on release over a Space.
    public var onCommit: ((WindowDropState.Commit) -> Void)?
    /// The Space under a global (top-left) screen point, if any: the app
    /// hit-tests its own tiles or windows.
    public var targetAt: ((Double, Double) -> String?)?
    /// Drop zones by Space id, each reporting its current frame in global
    /// top-left screen points (``TeleportDropZone`` registers itself).
    private var zones: [String: () -> CGRect?] = [:]
    /// Off in tests, which must not read a real window.
    var capturesThumbnails = true
    private let teleport: Teleport
    private var monitor: TeleportWindowDragMonitor?

    public init(teleport: Teleport) { self.teleport = teleport }

    public var supported: Bool { teleport.windowDragSupported() }
    public var permitted: Bool { teleport.windowDragPermitted() }

    /// Starts watching. Throws without the permission (ask with
    /// `Teleport.requestWindowDragPermission`) and off macOS.
    public func start() throws {
        guard monitor == nil else { return }
        let relay = DragRelay { [weak self] e in Task { @MainActor in self?.handle(e) } }
        monitor = try teleport.startWindowDrag(listener: relay)
    }

    public func stop() {
        monitor?.stop()
        monitor = nil
        state = WindowDropState()
    }

    /// The Space under the cursor (the view hit-tests its own tiles).
    public func over(_ id: String?) {
        if state.active { state.overId = id }
    }

    /// Makes a region a drop target for `spaceID`: a window released inside
    /// `frame()` (global top-left points) commits to that Space.
    public func registerZone(_ spaceID: String, frame: @escaping () -> CGRect?) {
        zones[spaceID] = frame
    }

    public func unregisterZone(_ spaceID: String) {
        zones[spaceID] = nil
    }

    /// The Space whose zone holds a global top-left point, if any.
    public func zone(at x: Double, _ y: Double) -> String? {
        let p = CGPoint(x: x, y: y)
        return zones.first { $0.value()?.contains(p) == true }?.key
    }

    func handle(_ e: TeleportWindowDragEvent) {
        let (capture, commit) = state.apply(e)
        if state.active { state.overId = targetAt?(state.x, state.y) ?? zone(at: state.x, state.y) }
        if let capture, capturesThumbnails {
            let t = teleport
            Task.detached {
                let png = try? t.captureWindowThumbnail(windowId: capture, maxWidth: 320)
                await MainActor.run { [weak self] in
                    if self?.state.window?.windowId == capture { self?.state.thumbnail = png ?? nil }
                }
            }
        }
        if let commit { onCommit?(commit) }
    }
}

final class DragRelay: TeleportWindowDragListener, @unchecked Sendable {
    let handler: @Sendable (TeleportWindowDragEvent) -> Void
    init(_ handler: @escaping @Sendable (TeleportWindowDragEvent) -> Void) { self.handler = handler }
    func onEvent(event: TeleportWindowDragEvent) { handler(event) }
}
