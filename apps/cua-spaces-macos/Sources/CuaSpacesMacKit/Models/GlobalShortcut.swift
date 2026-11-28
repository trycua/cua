// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import Carbon.HIToolbox

/// ⌘⇧Space (or the keys chosen in Settings, General: the settings file's
/// `hotkey`) from any app brings Cua Spaces to the front: the main window,
/// or the New UI window while that is the one in use. Another app may hold
/// the keys; then the system refuses them, Settings says so, and each read
/// of Settings tries again (`ensure`). Settings may also turn it off.
@MainActor
final class GlobalShortcut {
    /// The app's one shortcut, once `AppDelegate.start` registered it.
    static var installed: GlobalShortcut?

    /// The keys; nil while turned off.
    private(set) var keys: ShortcutKeys?
    private let register: (ShortcutKeys) -> Bool
    private let unregister: () -> Void
    private let save: (String) -> Void
    private(set) var registered = false

    init(keys: ShortcutKeys? = .default, register: @escaping (ShortcutKeys) -> Bool,
         unregister: @escaping () -> Void = {}, save: @escaping (String) -> Void = { _ in }) {
        self.keys = keys
        self.register = register
        self.unregister = unregister
        self.save = save
    }

    /// Registers the keys unless they already are; whether the system gave them.
    @discardableResult
    func ensure() -> Bool {
        guard let keys else { return false }
        if !registered { registered = register(keys) }
        return registered
    }

    /// `settings.get`'s `globalShortcut`: an empty label while turned off.
    var bridgeValue: [String: Any] { ["label": keys?.label ?? "", "registered": ensure()] }

    /// Settings' `global-shortcut` row: `off`, or the keys the page recorded
    /// (`Control+Alt+Command+F`). The new keys are tried first: when another
    /// app holds them the old ones stay, and this throws why.
    func change(_ option: String) throws {
        if option == ShortcutKeys.off {
            if registered { unregister() }
            registered = false
            keys = nil
            save("")
            return
        }
        guard let next = ShortcutKeys(accelerator: option) else { throw Refusal.notAShortcut(option) }
        if next == keys {
            guard ensure() else { throw Refusal.taken(next.label) }
            return
        }
        guard register(next) else { throw Refusal.taken(next.label) }
        keys = next
        registered = true
        save(next.label)
    }

    enum Refusal: Error, Equatable, CustomStringConvertible {
        case notAShortcut(String)
        case taken(String)

        var description: String {
            switch self {
            case .notAShortcut(let option): "Not a shortcut Cua Spaces can use: \(option)"
            case .taken(let label): "Another app is using \(label). Pick other keys."
            }
        }
    }

    /// Registered now, pressing it brings the app to the front.
    static func install(_ model: AppModel) {
        let hotKey = CarbonHotKey { bringForward(model) }
        let shortcut = GlobalShortcut(keys: ShortcutKeys.saved(model.settings.hotkey), register: hotKey.register,
                                      unregister: hotKey.unregister) { label in
            model.settings.hotkey = label
            model.saveSettings()
        }
        shortcut.ensure()
        installed = shortcut
    }

    /// The main window, in front.
    static func bringForward(_ model: AppModel) {
        NSApp.activate()
        MainUIRouter(model: model).openMain()
    }
}

/// A shortcut's keys: one key and at least one of Command, Control or
/// Option, so a bare letter never grabs the keyboard from every app. Read
/// from the settings file's glyphs (`⌘⇧Space`, `⌃⌥⇧⌘F19`, any modifier order)
/// or from the page's recorder (`Control+Alt+Shift+Command+F19`).
struct ShortcutKeys: Equatable {
    /// Settings' option that turns the shortcut off.
    static let off = "off"
    static let `default` = ShortcutKeys(label: "\u{2318}\u{21e7}Space")!

    let keyCode: UInt32
    /// Carbon's modifier flags (`cmdKey`, ...).
    let modifiers: UInt32
    /// As the Mac writes it (`⌘⇧Space`).
    let label: String

    static func == (a: ShortcutKeys, b: ShortcutKeys) -> Bool { a.keyCode == b.keyCode && a.modifiers == b.modifiers }

    private static let glyphs: [Character: Int] = ["\u{2318}": cmdKey, "\u{2303}": controlKey, "\u{2325}": optionKey,
                                                   "\u{21e7}": shiftKey]
    private static let acceleratorGlyphs = ["Command": "\u{2318}", "Control": "\u{2303}", "Alt": "\u{2325}",
                                            "Shift": "\u{21e7}"]
    private static let acceleratorKeys = ["Return": "\u{21a9}", "Backspace": "\u{232b}", "Up": "\u{2191}",
                                          "Down": "\u{2193}", "Left": "\u{2190}", "Right": "\u{2192}"]
    private static let named: [String: Int] = [
        "Space": kVK_Space, "\u{21a9}": kVK_Return, "Return": kVK_Return, "Tab": kVK_Tab, "\u{232b}": kVK_Delete,
        "Backspace": kVK_Delete, "\u{2191}": kVK_UpArrow, "\u{2193}": kVK_DownArrow, "\u{2190}": kVK_LeftArrow,
        "\u{2192}": kVK_RightArrow,
    ]
    private static let letters = [kVK_ANSI_A, kVK_ANSI_B, kVK_ANSI_C, kVK_ANSI_D, kVK_ANSI_E, kVK_ANSI_F, kVK_ANSI_G,
                                  kVK_ANSI_H, kVK_ANSI_I, kVK_ANSI_J, kVK_ANSI_K, kVK_ANSI_L, kVK_ANSI_M, kVK_ANSI_N,
                                  kVK_ANSI_O, kVK_ANSI_P, kVK_ANSI_Q, kVK_ANSI_R, kVK_ANSI_S, kVK_ANSI_T, kVK_ANSI_U,
                                  kVK_ANSI_V, kVK_ANSI_W, kVK_ANSI_X, kVK_ANSI_Y, kVK_ANSI_Z]
    private static let digits = [kVK_ANSI_0, kVK_ANSI_1, kVK_ANSI_2, kVK_ANSI_3, kVK_ANSI_4, kVK_ANSI_5, kVK_ANSI_6,
                                 kVK_ANSI_7, kVK_ANSI_8, kVK_ANSI_9]
    /// F1 to F20: the Mac has no F21 to F24.
    private static let functionKeys = [kVK_F1, kVK_F2, kVK_F3, kVK_F4, kVK_F5, kVK_F6, kVK_F7, kVK_F8, kVK_F9, kVK_F10,
                                       kVK_F11, kVK_F12, kVK_F13, kVK_F14, kVK_F15, kVK_F16, kVK_F17, kVK_F18, kVK_F19,
                                       kVK_F20]

    private static func keyCode(_ key: String) -> Int? {
        if let code = named[key] { return code }
        let upper = key.uppercased()
        if upper.count == 1, let c = upper.unicodeScalars.first {
            if ("A"..."Z").contains(c) { return letters[Int(c.value - 65)] }
            if ("0"..."9").contains(c) { return digits[Int(c.value - 48)] }
        }
        if upper.hasPrefix("F"), let n = Int(upper.dropFirst()), (1...20).contains(n), upper == "F\(n)" {
            return functionKeys[n - 1]
        }
        return nil
    }

    init?(label: String) {
        var modifiers = 0
        var rest = Substring(label)
        while let first = rest.first, let flag = Self.glyphs[first] {
            modifiers |= flag
            rest = rest.dropFirst()
        }
        guard modifiers & (cmdKey | controlKey | optionKey) != 0, let code = Self.keyCode(String(rest)) else { return nil }
        keyCode = UInt32(code)
        self.modifiers = UInt32(modifiers)
        self.label = label
    }

    /// The page's accelerator (`Command+Shift+Space`), labelled in its order.
    init?(accelerator: String) {
        let parts = accelerator.split(separator: "+", omittingEmptySubsequences: false).map(String.init)
        guard let key = parts.last else { return nil }
        var label = ""
        for modifier in parts.dropLast() {
            guard let glyph = Self.acceleratorGlyphs[modifier] else { return nil }
            label += glyph
        }
        self.init(label: label + (Self.acceleratorKeys[key] ?? key))
    }

    /// The keys the settings file keeps: an empty `hotkey` is turned off, and
    /// one that is not a shortcut reads as the default.
    static func saved(_ hotkey: String) -> ShortcutKeys? {
        hotkey.isEmpty ? nil : ShortcutKeys(label: hotkey) ?? .default
    }
}

/// A system-wide hot key through Carbon's `RegisterEventHotKey`, which needs
/// no Accessibility permission and never sees other keystrokes. Exclusive:
/// it fails when another app registered the same keys.
@MainActor
final class CarbonHotKey {
    nonisolated static let signature: OSType = 0x4355_4153 // 'CUAS'
    private let onPress: () -> Void
    private var handler: EventHandlerRef?
    private var ref: EventHotKeyRef?

    init(onPress: @escaping () -> Void) {
        self.onPress = onPress
    }

    /// Registers `keys` and then lets the previous ones go; when the system
    /// refuses `keys`, the previous ones stay.
    func register(_ keys: ShortcutKeys) -> Bool {
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
        var next: EventHotKeyRef?
        guard RegisterEventHotKey(keys.keyCode, keys.modifiers, id, GetApplicationEventTarget(),
                                  OptionBits(kEventHotKeyExclusive), &next) == noErr else { return false }
        unregister()
        ref = next
        return true
    }

    func unregister() {
        if let ref { UnregisterEventHotKey(ref) }
        ref = nil
    }
}
