// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Cua
import CuaSpaces
import CuaSpacesStreaming
import Foundation
import Testing
@testable import OpenKoalaBotExample

/// The shared presence cursor against a live Space, two clients: the app's
/// `LiveStreamSession` (the Computer pane's session, joined as the operator)
/// and a second participant joined through the cua SDK. Each sees the
/// other's cursor. Skips without a live Space (`LiveSpace`).
@Suite(.liveSpace, .serialized) @MainActor final class PresenceLiveTests {
    @Test func testCursorsAreVisibleAcrossTwoClients() async throws {
        let target = try LiveSpace.require("the presence live test")
        let space = try await target.client.sdkSpace(target.space)
        guard let native = try await space.native(), native.supports(feature: "presence") else {
            try XCTSkipNow("this Space has no presence service")
        }
        let session = LiveStreamSession(space: space)
        await session.joinPresence(as: "Operator")
        await session.select(.desktop)
        defer { Task { await session.stop() } }
        for _ in 0..<100 where session.localParticipant == nil { try await Task.sleep(for: .milliseconds(100)) }
        let me = try #require(session.localParticipant, "the app's session never joined presence: \(session.status)")

        let koalaBot = Bot(id: "openkoalabots-test-\(UUID().uuidString)", name: "Koala",
                           shape: Fixtures.bots[0].shape, colorHex: 0, preview: "", timestamp: "", screenIndex: 0)
        let koala = try await native.joinPresence(identity: BotPresenceColor.identity(for: koalaBot),
                                                  timeoutMs: 10_000)
        let koalaID = try await koala.me().participantId
        var koalaLeft = false
        defer { if !koalaLeft { Task { try? await koala.leave() } } }

        // Koala's cursor reaches the app's roster (what the pane draws).
        try await koala.updateCursor(cursor: PresenceCursor(displayId: "", windowId: nil, x: 0.25, y: 0.75, visible: true))
        var drawn: CGPoint?
        for _ in 0..<100 {
            if let who = session.participants.first(where: { $0.id == koalaID }),
               let n = who.normalizedCursor(in: session.surfaceSize) { drawn = n; break }
            try await Task.sleep(for: .milliseconds(100))
        }
        let at = try #require(drawn, "the app never saw Koala's cursor: \(session.participants)")
        XCTAssertEqual(at.x, 0.25, accuracy: 0.01)
        XCTAssertEqual(at.y, 0.75, accuracy: 0.01)
        XCTAssertTrue(session.participants.first { $0.id == koalaID }?.isAgent == true)
        // The cursor the pane draws and the Bot's avatar background are one color.
        XCTAssertEqual(session.participants.first { $0.id == koalaID }?.color.lowercased(),
                       BotPresenceColor.hex(for: koalaBot.id))
        XCTAssertFalse(session.participants.contains { $0.id == me.id }, "the local pointer is never drawn")

        // The operator's pointer, published the way the pane's hover does,
        // reaches Koala. The view is the surface's own size, so the view
        // point is the surface point.
        for _ in 0..<150 where session.surfaceSize.width == 0 { try await Task.sleep(for: .milliseconds(100)) }
        let size = session.surfaceSize
        try #require(size.width > 0, "no frame arrived to size the surface: \(session.status)")
        await session.publishCursor(viewPoint: CGPoint(x: size.width * 0.5, y: size.height * 0.25), in: size)
        let moved = try await koala.waitFor(timeoutMs: 10_000, maxEvents: 50) {
            $0.kind == "cursor_moved" && $0.participantId == me.id
        }
        XCTAssertEqual(moved.cursor?.x ?? -1, 0.5, accuracy: 0.01)
        XCTAssertEqual(moved.cursor?.y ?? -1, 0.25, accuracy: 0.01)

        try await koala.leave()
        koalaLeft = true
        for _ in 0..<100 where session.participants.contains(where: { $0.id == koalaID }) {
            try await Task.sleep(for: .milliseconds(100))
        }
        XCTAssertFalse(session.participants.contains { $0.id == koalaID }, "Koala's cursor stays after leaving")
    }
}
