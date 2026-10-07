// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpaces
import CuaSpacesStreaming
import Foundation
import SwiftUI
import Testing
@testable import OpenKoalaBotExample

/// A Bot's avatar background and its presence cursor are one color, from the SDK.
@Suite(.serialized) @MainActor final class BotColorTests {
    init() { PresenceColorBook.shared.update([]) }

    @Test func testABotsAvatarBackgroundEqualsItsCursorColor() {
        for bot in Fixtures.bots {
            let identity = BotPresenceColor.identity(for: bot)
            XCTAssertTrue(identity.agent)
            XCTAssertEqual(identity.color, BotPresenceColor.hex(for: bot.id))
            XCTAssertEqual(identity.color, PresenceColors.color(for: bot.id))
            // The cursor overlay draws the participant's color; a Bot that
            // joined with its identity shows exactly its avatar's background.
            var roster = PresenceRoster()
            _ = roster.apply(roster: [PresenceUser(user_id: "p-\(bot.id)", name: bot.name,
                                                   color: identity.color, kind: "agent")])
            let drawn = roster.participants["p-\(bot.id)"]?.color
            XCTAssertEqual(drawn, BotPresenceColor.hex(for: bot.id), bot.id)
            XCTAssertEqual(Color(hexString: drawn ?? ""), bot.color, "the avatar's background")
        }
    }

    @Test func testAvatarEyesUseTheContrastColor() {
        for bot in Fixtures.bots {
            XCTAssertEqual(bot.onColor, Color(hexString: PresenceColors.textColor(on: BotPresenceColor.hex(for: bot.id))))
        }
        XCTAssertEqual(PresenceColors.textColor(on: "#000075"), "#ffffff")
        XCTAssertEqual(PresenceColors.textColor(on: "#bcf60c"), "#000000")
    }

    /// When a human already holds the Bot's stable color, the server assigns
    /// the Bot another; the avatar follows the cursor, not the stable color.
    @Test func testTheAvatarFollowsTheServerAssignedColor() {
        let bot = Fixtures.bots[0]
        let stable = PresenceColors.color(for: bot.id)
        var roster = PresenceRoster()
        _ = roster.apply(roster: [
            PresenceUser(user_id: "p-op", name: "Operator", color: stable, kind: "human", principal_id: "operator"),
            PresenceUser(user_id: "p-bot", name: bot.name, color: "#123456", kind: "agent", principal_id: bot.id),
        ])
        PresenceColorBook.shared.update(Array(roster.participants.values))
        defer { PresenceColorBook.shared.update([]) }
        XCTAssertEqual(BotPresenceColor.hex(for: bot.id), "#123456")
        XCTAssertEqual(bot.color, Color(hexString: roster.participants["p-bot"]!.color))
        XCTAssertEqual(roster.colorOf(principalID: bot.id), BotPresenceColor.hex(for: bot.id))
        PresenceColorBook.shared.update([])
        XCTAssertEqual(BotPresenceColor.hex(for: bot.id), stable, "gone from the roster: the stable color")
    }
}
