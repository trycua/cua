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

/// The command palette (⌘K): actions, pages and your Spaces, matched as you
/// type in the core's order (`appPaletteRank`, the React palette's too).
struct CommandPaletteView: View {
    let model: AppModel
    @State private var query = ""
    @State private var active = 0
    @FocusState private var focused: Bool

    struct Command: Identifiable {
        let id: String
        let entry: AppPaletteEntry
        var symbol: String?
        var osIcon: String?
        var shortcut: String?
        let run: () -> Void
    }

    var body: some View {
        let all = commands
        let results = appPaletteRank(entries: all.map(\.entry), query: query).map { all[Int($0)] }
        let current = min(active, max(0, results.count - 1))
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
                TextField("Search for a Space or command", text: $query)
                    .textFieldStyle(.plain)
                    .font(.title3)
                    .focused($focused)
                    .onSubmit { run(results, current) }
                    .onKeyPress(.downArrow) { active = (current + 1) % max(1, results.count); return .handled }
                    .onKeyPress(.upArrow) { active = (current - 1 + max(1, results.count)) % max(1, results.count); return .handled }
                    .accessibilityIdentifier("palette-search")
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 12)
            Divider()
            if results.isEmpty {
                Text("No matches")
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                ScrollViewReader { scroller in
                    ScrollView {
                        LazyVStack(alignment: .leading, spacing: 0) {
                            ForEach(Array(results.enumerated()), id: \.element.id) { i, cmd in
                                if i == 0 || results[i - 1].entry.group != cmd.entry.group {
                                    Text(cmd.entry.group)
                                        .font(.caption.weight(.medium))
                                        .foregroundStyle(.secondary)
                                        .padding(.horizontal, 10)
                                        .padding(.top, 8)
                                        .padding(.bottom, 2)
                                }
                                row(cmd, active: i == current)
                                    .id(cmd.id)
                                    .contentShape(.rect)
                                    .onTapGesture { run(results, i) }
                                    .onHover { if $0 { active = i } }
                            }
                        }
                        .padding(6)
                    }
                    .onChange(of: current) { _, i in
                        if results.indices.contains(i) { scroller.scrollTo(results[i].id) }
                    }
                }
            }
        }
        .frame(width: 560, height: 420)
        .onChange(of: query) { active = 0 }
        .onExitCommand { model.showingPalette = false }
        .onAppear { focused = true }
        .accessibilityIdentifier("command-palette")
    }

    private func row(_ cmd: Command, active: Bool) -> some View {
        HStack(spacing: 10) {
            Group {
                if let os = cmd.osIcon {
                    OsIconImage(id: os, size: 13)
                } else if let symbol = cmd.symbol {
                    Image(systemName: symbol)
                }
            }
            .foregroundStyle(.secondary)
            .frame(width: 18)
            Text(cmd.entry.label).lineLimit(1)
            Spacer(minLength: 8)
            if let hint = cmd.entry.hint {
                Text(hint).font(.callout).foregroundStyle(.secondary)
            }
            if let shortcut = cmd.shortcut {
                Text(shortcut).font(.callout.monospaced()).foregroundStyle(.secondary)
            }
        }
        .padding(.horizontal, 10)
        .frame(height: 30)
        .background(active ? Color.primary.opacity(0.08) : .clear, in: .rect(cornerRadius: 6))
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(active ? .isSelected : [])
    }

    private func run(_ results: [Command], _ i: Int) {
        guard results.indices.contains(i) else { return }
        model.showingPalette = false
        results[i].run()
    }

    private var commands: [Command] {
        func entry(_ group: String, _ label: String, hint: String? = nil, keywords: String? = nil) -> AppPaletteEntry {
            AppPaletteEntry(group: group, label: label, hint: hint, keywords: keywords)
        }
        let chrome = model.chrome
        var out: [Command] = [
            Command(id: "space:new", entry: entry("Actions", chrome.newSpaceLabel, keywords: "create add"),
                    symbol: "plus", shortcut: "⌘N") { Task { await model.openNewSpace() } },
            Command(id: "machine:add", entry: entry("Actions", "Add a computer", keywords: "connect computer address"),
                    symbol: "plus.rectangle.on.rectangle") {
                model.selection = .machines
                Task { await model.openAddMachine() }
            },
        ]
        if let signIn = chrome.signInLabel {
            out.append(Command(id: "session:signIn", entry: entry("Actions", signIn, keywords: "account log in cloud"),
                               symbol: "person.crop.circle") { Task { await model.beginSignIn() } })
        }
        out += NavTarget.all(model).map { t in
            Command(id: "nav:\(t.command)", entry: entry("Go to", t.label, keywords: "page"), symbol: t.symbol,
                    shortcut: SharedShortcuts.label(t.command)) { model.go(t.command) }
        }
        out += [
            Command(id: "nav:notifications", entry: entry("Go to", "Notifications", keywords: "page"), symbol: "bell") {
                model.selection = .notifications
            },
            Command(id: "nav:settings", entry: entry("Go to", chrome.settingsLabel, keywords: "page preferences"),
                    symbol: "gearshape", shortcut: SharedShortcuts.label("nav.settings")) { MainUIRouter(model: model).openSettings() },
            Command(id: "nav:checklist", entry: entry("Go to", "Getting started", keywords: "checklist steps tour help first"),
                    symbol: "checklist") { model.openGettingStarted() },
        ]
        let rows = appSidebar(spaces: model.spaces, query: "", selectedId: "").sections.flatMap(\.rows)
        for space in model.realSpaces {
            let row = rows.first { $0.id == space.id }
            let hint = row.map { CoreMessages.text($0.statusText) }
            let keywords = "\(row?.osIcon ?? "") \(hint ?? "")"
            out.append(Command(id: "space:open:\(space.id)", entry: entry("Spaces", "Open \(space.name)", hint: hint, keywords: keywords),
                               osIcon: row?.osIcon) { model.selection = .space(space.id) })
            if let power = appPowerButton(space: space), power.enabled, !power.busy {
                let label = power.turnOn ? "Start \(space.name)" : "Stop \(space.name)"
                out.append(Command(id: "space:power:\(space.id)", entry: entry("Spaces", label, hint: hint, keywords: keywords),
                                   osIcon: row?.osIcon) { model.setPower(space, on: power.turnOn) })
            }
        }
        return out
    }
}
