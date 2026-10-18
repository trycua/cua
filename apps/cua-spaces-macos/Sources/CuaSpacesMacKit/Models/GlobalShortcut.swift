// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import Carbon.HIToolbox

/// ⌘⇧Space from any app brings Cua Spaces to the front: the main window,
/// or the New UI window while that is the one in use. Another app may hold
/// the keys; then the system refuses them, Settings says so, and each read
/// of Settings tries again (`ensure`).
@MainActor
final class GlobalShortcut {
    static let label = "\u{2318}\u{21e7}Space"

    /// The app's one shortcut, once `AppDelegate.start` registered it.
    static var installed: GlobalShortcut?

    private let tryRegister: () -> Bool
    private(set) var registered = false

    init(tryRegister: @escaping () -> Bool) {
        self.tryRegister = tryRegister
    }

    /// Registers the keys unless they already are; whether the system gave them.
    @discardableResult
    func ensure() -> Bool {
        if !registered { registered = tryRegister() }
        return registered
    }

    /// `settings.get`'s `globalShortcut`.
    var bridgeValue: [String: Any] { ["label": Self.label, "registered": ensure()] }

    /// Registered now, pressing it brings the app to the front.
    static func install(_ model: AppModel) {
        let hotKey = CarbonHotKey(keyCode: UInt32(kVK_Space), modifiers: UInt32(cmdKey | shiftKey)) {
            bringForward(model)
        }
        let shortcut = GlobalShortcut(tryRegister: hotKey.register)
        shortcut.ensure()
        installed = shortcut
    }

    /// The window of the UI in use, in front.
    static func bringForward(_ model: AppModel) {
        NSApp.activate()
        if CuaSpacesMacApp.opensWebUIAtLaunch(model) {
            WebUIWindowController.show(model: model)
        } else {
            WebUIWindowController.actions?.openMain()
        }
    }
}

/// A system-wide hot key through Carbon's `RegisterEventHotKey`, which needs
/// no Accessibility permission and never sees other keystrokes. Exclusive:
/// it fails when another app registered the same keys.
@MainActor
final class CarbonHotKey {
    nonisolated static let signature: OSType = 0x4355_4153 // 'CUAS'
    private let keyCode: UInt32
    private let modifiers: UInt32
    private let onPress: () -> Void
    private var handler: EventHandlerRef?
    private var ref: EventHotKeyRef?

    init(keyCode: UInt32, modifiers: UInt32, onPress: @escaping () -> Void) {
        self.keyCode = keyCode
        self.modifiers = modifiers
        self.onPress = onPress
    }

    func register() -> Bool {
        if ref != nil { return true }
        if handler == nil {
            var type = EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))
            let status = InstallEventHandler(GetApplicationEventTarget(), { _, event, data -> OSStatus in
                var id = EventHotKeyID()
                GetEventParameter(event, EventParamName(kEventParamDirectObject), EventParamType(typeEventHotKeyID),
                                  nil, MemoryLayout<EventHotKeyID>.size, nil, &id)
                guard let data, id.signature == CarbonHotKey.signature else { return OSStatus(eventNotHandledErr) }
                let hotKey = Unmanaged<CarbonHotKey>.fromOpaque(data).takeUnretainedValue()
                DispatchQueue.main.async { MainActor.assumeIsolated { hotKey.onPress() } }
                return noErr
            }, 1, &type, Unmanaged.passUnretained(self).toOpaque(), &handler)
            guard status == noErr else { return false }
        }
        let id = EventHotKeyID(signature: Self.signature, id: 1)
        return RegisterEventHotKey(keyCode, modifiers, id, GetApplicationEventTarget(),
                                   OptionBits(kEventHotKeyExclusive), &ref) == noErr
    }
}
