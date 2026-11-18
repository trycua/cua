// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CoreGraphics
import Foundation
import Testing
@testable import CuaSpacesStreaming

/// Presence: the roster and the send cadence, with no daemon in sight.
///
/// Every rule here is one an app would otherwise write for itself, and the
/// departure rule is one the shipping viewer gets wrong today, because it
/// ignores the roster message and has no sweeper, so a peer that dies without a
/// trailing hide leaves a frozen arrow on screen.
@Suite(.serialized) final class PresenceTests {

    private func user(_ id: String, _ name: String = "x", _ color: String = "#ff0000") -> PresenceUser {
        PresenceUser(user_id: id, name: name, color: color)
    }

    @Test func testJoinRecordsTheDaemonAssignedIdentity() {
        var roster = PresenceRoster()
        let events = roster.apply(joined: user("user-3", "me", "#00ff00"),
                                  roster: [user("user-3", "me", "#00ff00"), user("user-4", "bob")])
        guard case let .joined(me, _)? = events.first else { return XCTFail("\(events)") }
        XCTAssertEqual(me.id, "user-3")
        XCTAssertEqual(roster.me?.color, "#00ff00")
        XCTAssertEqual(roster.others.map(\.id), ["user-4"],
                       "an app must not draw a second marker for its own pointer")
    }

    /// The daemon has no leave message: departure is the roster getting
    /// shorter. The SDK diffs it so every consumer does not.
    @Test func testAShrinkingRosterIsADeparture() {
        var roster = PresenceRoster()
        _ = roster.apply(roster: [user("user-1"), user("user-2")])
        let events = roster.apply(roster: [user("user-1")])
        let left = events.compactMap { event -> Participant? in
            if case let .participantLeft(p) = event { return p }
            return nil
        }
        XCTAssertEqual(left.map(\.id), ["user-2"])
        XCTAssertNil(roster.participants["user-2"],
                     "a departed participant's cursor must not survive the roster")
        XCTAssertTrue(events.contains { if case .rosterChanged = $0 { return true }; return false })
    }

    @Test func testARosterUpdatePreservesEachSurvivorsCursor() {
        var roster = PresenceRoster()
        _ = roster.apply(roster: [user("user-1"), user("user-2")])
        _ = roster.apply(cursor: CursorState(userID: "user-1", name: "a", color: "#111111",
                                             window: TargetHandle("w"), point: CGPoint(x: 5, y: 6),
                                             isVisible: true, isPressed: false, shape: .default))
        _ = roster.apply(roster: [user("user-1"), user("user-2"), user("user-3")])
        XCTAssertEqual(roster.participants["user-1"]?.cursor?.point, CGPoint(x: 5, y: 6))
    }

    /// The host and the CUA agent are broadcast by the daemon itself and never
    /// join as clients, so gating cursors on the roster would silently drop the
    /// two most interesting cursors in the product.
    @Test func testTheHostAndAgentCursorsAreAdmittedWithoutAJoin() {
        var roster = PresenceRoster()
        _ = roster.apply(cursor: CursorState(userID: PresenceRoster.hostUserID,
                                             name: "host (desktop)", color: "#e8e8e8",
                                             window: nil, point: .zero,
                                             isVisible: true, isPressed: false, shape: .default))
        _ = roster.apply(cursor: CursorState(userID: PresenceRoster.agentUserID,
                                             name: "cua agent", color: "#3b82f6",
                                             window: nil, point: .zero,
                                             isVisible: true, isPressed: false, shape: .unknown))
        XCTAssertTrue(roster.participants[PresenceRoster.hostUserID]?.isHost == true)
        XCTAssertTrue(roster.participants[PresenceRoster.agentUserID]?.isAgent == true)
        XCTAssertEqual(roster.ordered.map(\.id),
                       [PresenceRoster.hostUserID, PresenceRoster.agentUserID],
                       "render order must be stable, or the overlay flickers")
    }

    /// Movement is throttled; state edges are not. A swallowed `visible: false`
    /// strands this cursor on every other viewer, because the daemon has no
    /// timeout that would clean it up.
    @Test func testTheBroadcastGateThrottlesMovementButNeverAnEdge() {
        var gate = CursorBroadcastGate(interval: .milliseconds(33))
        let t0 = ContinuousClock.now
        XCTAssertTrue(gate.shouldSend(visible: true, pressed: false, now: t0))
        XCTAssertFalse(gate.shouldSend(visible: true, pressed: false,
                                       now: t0.advanced(by: .milliseconds(10))))
        XCTAssertTrue(gate.shouldSend(visible: true, pressed: false,
                                      now: t0.advanced(by: .milliseconds(40))))
        // A press one millisecond later still goes.
        XCTAssertTrue(gate.shouldSend(visible: true, pressed: true,
                                      now: t0.advanced(by: .milliseconds(41))))
        // And so does the hide.
        XCTAssertTrue(gate.shouldSend(visible: false, pressed: false,
                                      now: t0.advanced(by: .milliseconds(42))))
    }

    /// ~30 Hz, which is what the product plan states and what the shipping
    /// viewer's 33 ms gate implements.
    @Test func testTheDefaultCadenceIsThirtyHertz() {
        XCTAssertEqual(CursorBroadcastGate().interval, .milliseconds(33))
    }
}
