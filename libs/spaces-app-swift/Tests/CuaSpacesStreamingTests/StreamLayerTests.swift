// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CoreGraphics
import Cua
import Foundation
import Testing
@testable import CuaSpacesStreaming

/// The stream layer's own vocabulary against the SDK's types. Kept apart from
/// the suites that import `CuaSpaces`, whose `Space` / `SpaceWindow` /
/// `TeleportManifest` share names with the SDK's.
@Suite(.serialized) final class StreamLayerTests {
    /// The SDK's participants become roster users with a colour, never an
    /// empty one (an empty colour draws an invisible cursor).
    @Test func testSDKParticipantsBecomeRosterUsers() throws {
        let user = PresenceUser(PresenceParticipant(
            participantId: "p-9", principalId: "bob", displayName: "bob", color: "", kind: "human"))
        XCTAssertEqual(user.user_id, "p-9")
        XCTAssertEqual(user.name, "bob")
        XCTAssertEqual(user.color, PresenceRoster.fallbackColor)
        let cursor = CursorState(userID: "p-9", name: "bob", color: "#ff5f5f",
                                 window: TargetHandle("target-1"), point: CGPoint(x: 100, y: 200),
                                 isVisible: true, isPressed: true, shape: .unknown)
        XCTAssertEqual(cursor.normalizedPoint(in: SurfaceGeometry(
            width_px: 200, height_px: 400, scale_factor: 1)), CGPoint(x: 0.5, y: 0.5))
    }

    /// Input goes out as one `interactive_input` batch per send, with the
    /// batch's first sequence, in the envelope rcdp wire v2 accepts.
    @Test func testInputBatchesEncodeTheWireShape() throws {
        let text = try interactiveInputText(
            session: SessionID("media-1"), firstSequence: 7,
            events: [.pointer(phase: .down, button: .left, x: 0.5, y: 0.25, modifiers: [.shift]),
                     .textCommit("hi")])
        XCTAssertTrue(text.contains("\"type\":\"interactive_input\""), text)
        XCTAssertTrue(text.contains("\"direction\":\"client\""), text)
        XCTAssertTrue(text.contains("\"first_sequence\":7"), text)
        XCTAssertTrue(text.contains("\"session_id\":\"media-1\""), text)
        XCTAssertTrue(text.contains("\"x_normalized\":0.5"), text)
        XCTAssertFalse(InteractiveInputEvent.pointer(phase: .move, button: nil, x: 1.5, y: 0,
                                                     modifiers: []).isDispatchable,
                       "an off-surface coordinate would sink the whole batch")
    }

}
