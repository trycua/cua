// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import Carbon.HIToolbox
import CanvasModel

/// The window the canvas lives in: a borderless full-screen panel above
/// everything (the default), or an ordinary window (`--windowed`) for
/// recordings and small screens. Both toggle with the global hotkey.
final class CanvasWindow: NSPanel {
    var keyHandler: ((NSEvent) -> Bool)?

    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { true }

    override func keyDown(with event: NSEvent) {
        if keyHandler?(event) == true { return }
        super.keyDown(with: event)
    }

    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        if event.modifierFlags.contains(.command), keyHandler?(event) == true { return true }
        return super.performKeyEquivalent(with: event)
    }

    static func overlay(on screen: NSScreen) -> CanvasWindow {
        let w = CanvasWindow(contentRect: screen.frame, styleMask: [.borderless, .nonactivatingPanel],
                             backing: .buffered, defer: false)
        w.level = .statusBar
        w.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary, .ignoresCycle]
        w.isOpaque = true
        w.backgroundColor = Palette.canvas
        w.hasShadow = false
        w.hidesOnDeactivate = false
        w.isReleasedWhenClosed = false
        w.setFrame(screen.frame, display: false)
        return w
    }

    /// A borderless floating window of `size` at the screen's top left, for
    /// window-only recordings (no title bar in the footage).
    static func chromeless(size: CGSize, on screen: NSScreen) -> CanvasWindow {
        let v = screen.visibleFrame
        let w = CanvasWindow(contentRect: CGRect(x: v.minX, y: v.maxY - size.height, width: size.width, height: size.height),
                             styleMask: [.borderless], backing: .buffered, defer: false)
        w.level = .floating
        w.backgroundColor = Palette.canvas
        w.isOpaque = true
        w.hasShadow = false
        w.isReleasedWhenClosed = false
        w.hidesOnDeactivate = false
        return w
    }

    static func windowed(size: CGSize, on screen: NSScreen) -> CanvasWindow {
        let frame = CGRect(x: screen.visibleFrame.maxX - size.width - 24,
                           y: screen.visibleFrame.minY + 24, width: size.width, height: size.height)
        let w = CanvasWindow(contentRect: frame, styleMask: [.titled, .closable, .resizable, .fullSizeContentView],
                             backing: .buffered, defer: false)
        w.titlebarAppearsTransparent = true
        w.titleVisibility = .hidden
        w.title = "infinite-canvas"
        w.backgroundColor = Palette.canvas
        w.isReleasedWhenClosed = false
        w.hidesOnDeactivate = false
        w.isFloatingPanel = false
        w.level = .normal
        return w
    }
}

/// A system-wide hotkey through Carbon's `RegisterEventHotKey`, which needs
/// no Accessibility permission and never sees other keystrokes.
final class GlobalHotkey {
    private var ref: EventHotKeyRef?
    private var handler: EventHandlerRef?
    private let action: () -> Void
    nonisolated(unsafe) private static var registry: [UInt32: GlobalHotkey] = [:]
    private static var nextID: UInt32 = 1
    private let id: UInt32

    init?(_ spec: HotkeySpec, action: @escaping () -> Void) {
        self.action = action
        id = Self.nextID
        Self.nextID += 1
        var mods: UInt32 = 0
        if spec.modifiers.contains(.command) { mods |= UInt32(cmdKey) }
        if spec.modifiers.contains(.option) { mods |= UInt32(optionKey) }
        if spec.modifiers.contains(.control) { mods |= UInt32(controlKey) }
        if spec.modifiers.contains(.shift) { mods |= UInt32(shiftKey) }
        var type = EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))
        let status = InstallEventHandler(GetApplicationEventTarget(), { _, event, _ -> OSStatus in
            var hk = EventHotKeyID()
            GetEventParameter(event, EventParamName(kEventParamDirectObject), EventParamType(typeEventHotKeyID),
                              nil, MemoryLayout<EventHotKeyID>.size, nil, &hk)
            if let target = GlobalHotkey.registry[hk.id] {
                DispatchQueue.main.async { target.action() }
            }
            return noErr
        }, 1, &type, nil, &handler)
        guard status == noErr else { return nil }
        let hkID = EventHotKeyID(signature: OSType(0x4355_4143), id: id) // 'CUAC'
        guard RegisterEventHotKey(spec.keyCode, mods, hkID, GetApplicationEventTarget(), 0, &ref) == noErr else {
            return nil
        }
        Self.registry[id] = self
    }

    deinit {
        if let ref { UnregisterEventHotKey(ref) }
        if let handler { RemoveEventHandler(handler) }
    }
}
