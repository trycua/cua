// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSDK
import CuaSpacesFFI
import Foundation
import Observation

/// Runs the core's timers. The app uses real time; tests step a manual
/// clock so hover timing is checked to the millisecond without sleeping.
public protocol NotchScheduler: AnyObject {
    /// Calls `fire` after `ms`; returns a cancel.
    @MainActor func after(_ ms: UInt32, _ fire: @escaping @MainActor () -> Void) -> () -> Void
}

/// Real time (`Task.sleep`).
public final class SystemNotchScheduler: NotchScheduler {
    public init() {}
    @MainActor public func after(_ ms: UInt32, _ fire: @escaping @MainActor () -> Void) -> () -> Void {
        let task = Task { @MainActor in
            try? await Task.sleep(for: .milliseconds(Int(ms)))
            guard !Task.isCancelled else { return }
            fire()
        }
        return { task.cancel() }
    }
}

/// The notch panel's state: the core's `AppNotchState` plus the timers its
/// effects ask for. Hover dwell, close delay, drag phases, copy and the
/// commit are the core's decisions (`appNotchReduce`, `appNotchView`); this
/// runs the effects and holds the images the core cannot (thumbnails, the
/// dragged window's ghost).
@MainActor
@Observable
public final class NotchModel {
    public private(set) var state: AppNotchState = appNotchInitial()
    public var spaces: [AppSpace] = []
    /// A captured image of the dragged window (the additive ghost).
    public private(set) var ghost: NSImage?
    /// Latest thumbnail per Space id, for the tiles: the same store the
    /// detail's preview cover reads (`AppModel.thumbnails`).
    public let thumbnails = SpaceThumbnails()
    /// A forced hover or pressed look on one control (snapshot tests and
    /// debug start states only; real input never sets it).
    public var highlight: NotchHighlight?
    /// Called when a drag commits to a Space (`spaceId`, dragged window id).
    @ObservationIgnored public var onCommit: ((String, UInt32?) -> Void)?
    /// Captures a window's image (the SDK's thumbnail capture).
    @ObservationIgnored public var capture: ((UInt32) -> NSImage?)?
    @ObservationIgnored private let scheduler: NotchScheduler
    @ObservationIgnored private var cancelTimer: (() -> Void)?

    public init(scheduler: NotchScheduler = SystemNotchScheduler()) {
        self.scheduler = scheduler
    }

    public var view: AppNotchView { appNotchView(state: state, spaces: spaces) }
    public static let motion = appNotchMotion()

    public func send(_ event: AppNotchEvent) {
        let draggedWindow = state.drag.windowId
        let t = appNotchReduce(state: state, event: event)
        if t.state != state { state = t.state }
        for effect in t.effects { run(effect, draggedWindow: draggedWindow) }
    }

    /// The hotspot or a transfer changed (the indicator left of the notch).
    public func setActivity(hotspot: Bool, transfer: AppNotchTransfer?) {
        guard hotspot != state.hotspot || transfer != state.transfer else { return }
        send(.activity(hotspot: hotspot, transfer: transfer))
    }

    /// Keyvault sign-ins went live in a Space, changed, or stopped (the
    /// core's sharing label less dismissed copies, nil when nothing shows;
    /// the Space ids whose tiles carry the key).
    public func setKeyvault(label: String?, signedIn: [String] = []) {
        guard label != state.keyvault || signedIn != state.signedIn else { return }
        send(.keyvault(label: label, signedIn: signedIn))
    }

    /// The header's search text.
    public func search(_ query: String) {
        guard query != state.query else { return }
        send(.search(query: query))
    }

    /// Sets the dragged window's ghost directly (debug start states).
    func setGhost(_ image: NSImage?) { ghost = image }

    private func run(_ effect: AppNotchEffect, draggedWindow: UInt32?) {
        switch effect {
        case .cancelTimers:
            cancelTimer?()
            cancelTimer = nil
        case .startDwell(let ms):
            schedule(ms) { $0.send(.dwellElapsed) }
        case .startClose(let ms):
            schedule(ms) { $0.send(.closeElapsed) }
        case .drag(let effect):
            switch effect {
            case .capture(let windowId):
                ghost = windowId.flatMap { capture?($0) }
                send(.drag(event: .ghostReady(ghost: ghost == nil ? nil : "captured")))
            case .commit(let spaceId):
                onCommit?(spaceId, draggedWindow)
                ghost = nil
            }
        }
    }

    private func schedule(_ ms: UInt32, _ fire: @escaping @MainActor (NotchModel) -> Void) {
        cancelTimer?()
        cancelTimer = scheduler.after(ms) { [weak self] in
            guard let self else { return }
            self.cancelTimer = nil
            fire(self)
        }
    }
}
