// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Carbon.HIToolbox
import Foundation
@testable import CuaSpacesMacKit
import Testing

/// General's "Open Cua Spaces from any app". Serialized: each test installs
/// its own `GlobalShortcut.installed`, which the bridge reads.
@Suite("Global shortcut", .serialized)
@MainActor
struct GlobalShortcutTests {
    @Test func theGlobalShortcutSaysWhetherTheSystemGaveIt() async throws {
        let (bridge, _) = WebUIHostTests().bridge()
        defer { GlobalShortcut.installed = nil }
        GlobalShortcut.installed = nil
        #expect((try await bridge.handle("settings.get", [:]) as? [String: Any])?["globalShortcut"] is NSNull)
        // Another app holds the keys at launch, then lets them go: each read tries again.
        var answers = [false, true]
        var tries = 0
        GlobalShortcut.installed = GlobalShortcut { _ in tries += 1; return answers.removeFirst() }
        GlobalShortcut.installed?.ensure()
        let refused = (try await bridge.handle("settings.get", [:]) as? [String: Any])?["globalShortcut"] as? [String: Any]
        #expect(refused?["label"] as? String == "\u{2318}\u{21e7}Space")
        #expect(refused?["registered"] as? Bool == true)
        #expect(tries == 2)
        _ = try await bridge.handle("settings.get", [:])
        #expect(tries == 2)
    }

    /// Settings' Change records other keys: they are registered before the
    /// old ones go, saved as the settings file's `hotkey`, and the row shows
    /// them. Keys another app holds leave the old ones; off empties the row.
    @Test func theGlobalShortcutChangesAndTurnsOff() async throws {
        let (bridge, _) = WebUIHostTests().bridge()
        defer { GlobalShortcut.installed = nil }
        var held: [ShortcutKeys] = []
        var taken: Set<String> = ["\u{2318}K"]
        var saved: [String] = []
        var unregistered = 0
        GlobalShortcut.installed = GlobalShortcut(register: { keys in
            guard !taken.contains(keys.label) else { return false }
            held.append(keys)
            return true
        }, unregister: { unregistered += 1 }, save: { saved.append($0) })
        GlobalShortcut.installed?.ensure()
        func row() async throws -> [String: Any]? {
            try await bridge.handle("settings.get", [:]) as? [String: Any]
        }

        let changed = try await bridge.handle("settings.choose", ["row": "global-shortcut", "option": "Control+Alt+Command+F"])
        let shortcut = (changed as? [String: Any])?["globalShortcut"] as? [String: Any]
        #expect(shortcut?["label"] as? String == "\u{2303}\u{2325}\u{2318}F")
        #expect(shortcut?["registered"] as? Bool == true)
        #expect(held.last == ShortcutKeys(label: "\u{2303}\u{2325}\u{2318}F"))
        #expect(saved == ["\u{2303}\u{2325}\u{2318}F"])

        await #expect(throws: WebUIBridge.Failure.self) {
            try await bridge.handle("settings.choose", ["row": "global-shortcut", "option": "Command+K"])
        }
        #expect(((try await row())?["globalShortcut"] as? [String: Any])?["label"] as? String == "\u{2303}\u{2325}\u{2318}F")
        await #expect(throws: WebUIBridge.Failure.self) {
            try await bridge.handle("settings.choose", ["row": "global-shortcut", "option": "Shift+K"])
        }
        #expect(saved.count == 1)

        _ = try await bridge.handle("settings.choose", ["row": "global-shortcut", "option": "off"])
        let off = (try await row())?["globalShortcut"] as? [String: Any]
        #expect(off?["label"] as? String == "")
        #expect(off?["registered"] as? Bool == false)
        #expect(unregistered == 1)
        #expect(saved == ["\u{2303}\u{2325}\u{2318}F", ""])
        taken = []
        _ = try await bridge.handle("settings.choose", ["row": "global-shortcut", "option": "Command+K"])
        #expect(held.last == ShortcutKeys(label: "\u{2318}K"))
    }

    /// Choosing the keys it already has, while another app still holds them,
    /// fails with why instead of reading as done.
    @Test func reChoosingTakenKeysSaysTheyAreTaken() async throws {
        let (bridge, _) = WebUIHostTests().bridge()
        defer { GlobalShortcut.installed = nil }
        GlobalShortcut.installed = GlobalShortcut { _ in false }
        GlobalShortcut.installed?.ensure()
        await #expect(throws: WebUIBridge.Failure.self) {
            try await bridge.handle("settings.choose", ["row": "global-shortcut", "option": "Shift+Command+Space"])
        }
        #expect(throws: GlobalShortcut.Refusal.taken("\u{21e7}\u{2318}Space")) {
            try GlobalShortcut.installed?.change("Shift+Command+Space")
        }
    }

    /// The settings file's `hotkey`, as written by hand or by Settings: glyphs
    /// in any order. Empty is off; one that is not a shortcut is the default.
    @Test func theSavedHotkeyIsTheShortcut() {
        let f19 = ShortcutKeys.saved("\u{2303}\u{2325}\u{21e7}\u{2318}F19")
        #expect(f19?.label == "\u{2303}\u{2325}\u{21e7}\u{2318}F19")
        #expect(f19?.keyCode == UInt32(kVK_F19))
        #expect(f19?.modifiers == UInt32(controlKey | optionKey | shiftKey | cmdKey))
        #expect(ShortcutKeys.saved("\u{21e7}\u{2318}Space") == ShortcutKeys.default)
        #expect(ShortcutKeys.saved("") == nil)
        #expect(ShortcutKeys.saved("\u{21e7}K") == ShortcutKeys.default)
        #expect(ShortcutKeys.saved("nonsense") == ShortcutKeys.default)
        #expect(ShortcutKeys(accelerator: "Shift+Command+Space") == ShortcutKeys.default)
        #expect(ShortcutKeys(accelerator: "Control+Up")?.keyCode == UInt32(kVK_UpArrow))
        #expect(ShortcutKeys(accelerator: "Command+F24") == nil)
        #expect(ShortcutKeys(accelerator: "Hyper+K") == nil)
    }
}
