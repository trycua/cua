// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpacesFFI
import SwiftUI

/// The shared keyboard shortcuts: the core's `appKeybindings`, which are
/// the React UI's `keybindings.json`, so both bind the same keys.
enum SharedShortcuts {
    static let keys: [String: String] = Dictionary(
        appKeybindings().map { ($0.command, $0.key) }, uniquingKeysWith: { first, _ in first })

    private static func parts(_ command: String) -> (key: String, mods: [String])? {
        guard let spec = keys[command] else { return nil }
        var parts = spec.lowercased().split(separator: "+").map(String.init)
        guard let key = parts.popLast(), key.count == 1 else { return nil }
        return (key, parts)
    }

    /// `mod+k` as ⌘K.
    static func shortcut(_ command: String) -> KeyboardShortcut? {
        guard let (key, mods) = parts(command), let ch = key.first else { return nil }
        var modifiers: EventModifiers = []
        if mods.contains("mod") { modifiers.insert(.command) }
        if mods.contains("alt") { modifiers.insert(.option) }
        if mods.contains("shift") { modifiers.insert(.shift) }
        return KeyboardShortcut(KeyEquivalent(ch), modifiers: modifiers)
    }

    /// `mod+shift+l` as "⌘⇧L".
    static func label(_ command: String) -> String? {
        guard let (key, mods) = parts(command) else { return nil }
        let marks = [("mod", "⌘"), ("alt", "⌥"), ("shift", "⇧")].filter { mods.contains($0.0) }.map(\.1)
        return marks.joined() + key.uppercased()
    }
}

/// A page ⌘1–4 go to (the shared keybindings' `nav.*` commands).
struct NavTarget: Identifiable {
    let command: String
    let label: String
    let symbol: String
    var id: String { command }

    @MainActor static func all(_ model: AppModel) -> [NavTarget] {
        [NavTarget(command: "nav.spaces", label: "Spaces", symbol: "square.grid.2x2"),
         NavTarget(command: "nav.machines", label: "Computers", symbol: "server.rack"),
         NavTarget(command: "nav.agents", label: "Agents", symbol: "person.2"),
         NavTarget(command: "nav.keyvault", label: model.chrome.keyvaultTitle, symbol: "key")]
    }
}

extension AppModel {
    /// Goes to the page a `nav.*` command names.
    public func go(_ command: String) {
        switch command {
        case "nav.spaces": selection = sidebar.selectedId.map { .space($0) }
        case "nav.machines": selection = .machines
        case "nav.agents": selection = .agents
        case "nav.keyvault":
            if let first = keyvault.sidebar.categories.first {
                selection = .keyvault(.category(category: first.category))
            }
        default: break
        }
    }
}

/// View: Command Palette and ⌘1–4, with the shared keys. The web UI
/// window handles these keys itself while it is in front; from anywhere
/// else (a Space's window) they open it at the page.
struct NavigationCommands: View {
    let model: AppModel

    var body: some View {
        Button("Command Palette…") { inMain(.home) }
            .keyboardShortcut(SharedShortcuts.shortcut("commandPalette.toggle"))
        Divider()
        ForEach(NavTarget.all(model)) { target in
            Button(target.label) { inMain(MainUIRouter.route(forNav: target.command)) }
                .keyboardShortcut(SharedShortcuts.shortcut(target.command))
        }
    }

    private func inMain(_ route: MainUIRoute?) {
        if WebUIWindowController.shared?.window?.isKeyWindow == true { return }
        guard model.startup.isReady, model.onboarding.completed else { return }
        MainUIRouter(model: model).open(route ?? .home)
    }
}
