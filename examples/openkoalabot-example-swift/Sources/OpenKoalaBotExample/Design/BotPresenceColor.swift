// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Cua
import CuaSpaces
import CuaSpacesStreaming
import SwiftUI

/// A Bot's one color, for its avatar's background and its presence cursor
/// alike. Before the Bot is present on the Space it is the SDK's stable
/// per-agent color (`PresenceColors`), which the Bot requests when it joins;
/// once it is present, the color the server assigned it is authoritative (the
/// server keeps a requested color unless another participant already holds
/// it), read from the roster through `PresenceColorBook`.
enum BotPresenceColor {
    /// `#rrggbb`: the avatar's background, the same as the Bot's cursor.
    static func hex(for botID: String) -> String {
        PresenceColorBook.shared.assignedColor(for: botID) ?? PresenceColors.color(for: botID)
    }

    /// Black or white, whichever reads on the avatar's background.
    static func textHex(for botID: String) -> String { PresenceColors.textColor(on: hex(for: botID)) }

    static func fill(for botID: String) -> Color { Color(hexString: hex(for: botID)) }
    static func text(for botID: String) -> Color { Color(hexString: textHex(for: botID)) }

    /// The identity this Bot joins presence with (requesting its stable color).
    static func identity(for bot: Bot) -> PresenceIdentity {
        PresenceColors.agentIdentity(id: bot.id, displayName: bot.name)
    }
}

/// The colors the server assigned to whoever is present, by the id they
/// joined with, from the Space's presence roster. Views that draw a Bot's
/// color observe it so an avatar follows its cursor.
final class PresenceColorBook: ObservableObject, @unchecked Sendable {
    static let shared = PresenceColorBook()

    private let lock = NSLock()
    private var assigned: [String: String] = [:]
    /// Bumped on every change; observers re-render.
    @Published private(set) var version = 0

    func assignedColor(for principalID: String) -> String? {
        lock.lock(); defer { lock.unlock() }
        return assigned[principalID]
    }

    /// Replace the book with this roster (the local participant included).
    @MainActor func update(_ participants: [Participant]) {
        var next: [String: String] = [:]
        for p in participants {
            if let id = p.principalID, !id.isEmpty, !p.color.isEmpty { next[id] = p.color.lowercased() }
        }
        lock.lock()
        let changed = next != assigned
        assigned = next
        lock.unlock()
        if changed { version += 1 }
    }
}

extension Color {
    /// `#RRGGBB`, or the SDK's fallback blue.
    init(hexString: String) {
        let hex = hexString.trimmingCharacters(in: CharacterSet(charactersIn: "#"))
        let v = UInt32(hex, radix: 16) ?? 0x3B82F6
        self.init(red: Double((v >> 16) & 0xFF) / 255, green: Double((v >> 8) & 0xFF) / 255,
                  blue: Double(v & 0xFF) / 255)
    }
}
