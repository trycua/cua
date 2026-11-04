// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation

/// The overlay's visibility, driven by the global hotkey.
///
/// Presses during a transition reverse it rather than queue (pressing twice
/// quickly lands you where you started), and a press within `debounce` of
/// the previous one is a key repeat, not a second press.
public struct OverlayState: Equatable, Sendable {
    public enum Phase: Equatable, Sendable {
        case hidden, presenting, shown, dismissing

        public var isVisible: Bool { self != .hidden }
    }

    public enum Action: Equatable, Sendable {
        case present
        case dismiss
    }

    public private(set) var phase: Phase = .hidden
    private var lastPress: TimeInterval = -.infinity
    public var debounce: TimeInterval = 0.12

    public init() {}

    /// The hotkey was pressed at `now`.
    public mutating func hotkey(at now: TimeInterval) -> Action? {
        defer { lastPress = now }
        guard now - lastPress >= debounce else { return nil }
        switch phase {
        case .hidden, .dismissing:
            phase = .presenting
            return .present
        case .shown, .presenting:
            phase = .dismissing
            return .dismiss
        }
    }

    /// Escape dismisses a visible overlay and does nothing otherwise.
    public mutating func escape() -> Action? {
        switch phase {
        case .shown, .presenting:
            phase = .dismissing
            return .dismiss
        case .hidden, .dismissing:
            return nil
        }
    }

    /// The running transition finished.
    public mutating func transitionFinished() {
        switch phase {
        case .presenting: phase = .shown
        case .dismissing: phase = .hidden
        case .hidden, .shown: break
        }
    }
}

/// A global hotkey, stored as a key code plus modifiers so it can be parsed
/// from a setting and registered with Carbon unchanged.
public struct HotkeySpec: Equatable, Sendable, CustomStringConvertible {
    public struct Modifiers: OptionSet, Sendable, Hashable {
        public let rawValue: Int
        public init(rawValue: Int) { self.rawValue = rawValue }
        public static let command = Modifiers(rawValue: 1)
        public static let option = Modifiers(rawValue: 2)
        public static let control = Modifiers(rawValue: 4)
        public static let shift = Modifiers(rawValue: 8)
    }

    public var keyCode: UInt32
    public var key: String
    public var modifiers: Modifiers

    public init(keyCode: UInt32, key: String, modifiers: Modifiers) {
        self.keyCode = keyCode
        self.key = key
        self.modifiers = modifiers
    }

    /// Option-Space.
    public static let `default` = HotkeySpec(keyCode: 49, key: "space", modifiers: [.option])

    private static let keyCodes: [String: UInt32] = [
        "space": 49, "return": 36, "tab": 48, "escape": 53, "`": 50,
        "a": 0, "s": 1, "d": 2, "f": 3, "h": 4, "g": 5, "z": 6, "x": 7, "c": 8, "v": 9,
        "b": 11, "q": 12, "w": 13, "e": 14, "r": 15, "y": 16, "t": 17, "o": 31, "u": 32,
        "i": 34, "p": 35, "l": 37, "j": 38, "k": 40, "n": 45, "m": 46,
        "1": 18, "2": 19, "3": 20, "4": 21, "5": 23, "6": 22, "7": 26, "8": 28, "9": 25, "0": 29,
    ]

    /// Parse `option+space`, `cmd+shift+k`, `ctrl+\``. Nil on anything else.
    public init?(parsing text: String) {
        let parts = text.lowercased().split(separator: "+").map { $0.trimmingCharacters(in: .whitespaces) }
        guard let keyName = parts.last, let code = Self.keyCodes[keyName] else { return nil }
        var mods: Modifiers = []
        for m in parts.dropLast() {
            switch m {
            case "cmd", "command", "⌘": mods.insert(.command)
            case "opt", "option", "alt", "⌥": mods.insert(.option)
            case "ctrl", "control", "⌃": mods.insert(.control)
            case "shift", "⇧": mods.insert(.shift)
            default: return nil
            }
        }
        // A bare key would steal typing everywhere.
        guard !mods.isEmpty else { return nil }
        self.init(keyCode: code, key: keyName, modifiers: mods)
    }

    public var description: String {
        var s = ""
        if modifiers.contains(.control) { s += "⌃" }
        if modifiers.contains(.option) { s += "⌥" }
        if modifiers.contains(.shift) { s += "⇧" }
        if modifiers.contains(.command) { s += "⌘" }
        return s + (key == "space" ? "Space" : key.uppercased())
    }
}

/// Type-to-find over tile titles: every query character must appear in order;
/// word starts and contiguous runs score higher. Returns ids best first.
public enum TileSearch {
    public static func score(_ query: String, in text: String) -> Int? {
        let q = Array(query.lowercased().filter { !$0.isWhitespace })
        guard !q.isEmpty else { return 0 }
        let t = Array(text.lowercased())
        var qi = 0
        var score = 0
        var run = 0
        var prev: Character = " "
        for c in t {
            if qi < q.count, c == q[qi] {
                qi += 1
                run += 1
                score += 1 + run * 2
                if prev == " " || prev == "-" || prev == "." || prev == "/" { score += 6 }
            } else {
                run = 0
            }
            prev = c
        }
        return qi == q.count ? score : nil
    }

    public static func rank(_ query: String, _ items: [(id: String, text: String)]) -> [String] {
        var scored: [(id: String, score: Int)] = []
        for item in items {
            if let s = score(query, in: item.text) { scored.append((item.id, s)) }
        }
        scored.sort { a, b in a.score != b.score ? a.score > b.score : a.id < b.id }
        return scored.map { $0.id }
    }
}
