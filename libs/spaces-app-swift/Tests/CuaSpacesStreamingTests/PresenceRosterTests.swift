// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CoreGraphics
import Cua
import Foundation
import Testing
@testable import CuaSpacesStreaming

/// The presence fold over cua SDK events and `waitFor`, which the three
/// OpenKoalaBots samples share.
@Suite(.serialized) final class PresenceRosterTests {

    private func p(_ id: String, _ kind: String = "human") -> PresenceParticipant {
        PresenceParticipant(participantId: id, principalId: "u-\(id)", displayName: id.uppercased(),
                            color: "", kind: kind)
    }

    private func moved(_ id: String, _ x: Double, _ y: Double) -> CuaSDK.PresenceEvent {
        CuaSDK.PresenceEvent(kind: "cursor_moved", participant: nil, participantId: id,
                             cursor: PresenceCursor(displayId: "", windowId: nil, x: x, y: y, visible: true))
    }

    @Test func testSDKEventsFoldIntoTheRoster() {
        var roster = PresenceRoster()
        _ = roster.apply(me: p("me"), members: [PresenceMember(participant: p("me"), cursor: nil)])
        _ = roster.apply(event: CuaSDK.PresenceEvent(kind: "joined", participant: p("koala", "agent"),
                                                     participantId: nil, cursor: nil))
        XCTAssertEqual(roster.others.map(\.id), ["koala"])
        XCTAssertTrue(roster.others.first?.isAgent == true, "a participant that joined as an agent is one")
        XCTAssertEqual(roster.others.first?.color, PresenceRoster.fallbackColor)
        // A cursor before the first frame: no surface size yet, still drawable.
        _ = roster.apply(event: moved("koala", 0.25, 0.75), surfaceSize: .zero)
        XCTAssertEqual(roster.participants["koala"]?.normalizedCursor(in: .zero), CGPoint(x: 0.25, y: 0.75))
        _ = roster.apply(event: moved("koala", 0.5, 0.5), surfaceSize: CGSize(width: 1280, height: 800))
        XCTAssertEqual(roster.participants["koala"]?.cursor?.point, CGPoint(x: 640, y: 400))
        XCTAssertTrue(roster.apply(event: CuaSDK.PresenceEvent(kind: "keep_alive", participant: nil,
                                                               participantId: nil, cursor: nil)).isEmpty)
        _ = roster.apply(event: CuaSDK.PresenceEvent(kind: "left", participant: nil,
                                                     participantId: "koala", cursor: nil))
        XCTAssertNil(roster.participants["koala"])
        XCTAssertTrue(roster.others.isEmpty)
    }

    final class ScriptedPresence: SpacePresenceProtocol, @unchecked Sendable {
        var queue: [CuaSDK.PresenceEvent]
        init(_ q: [CuaSDK.PresenceEvent]) { queue = q }
        func leave() async throws {}
        func me() async throws -> PresenceParticipant {
            PresenceParticipant(participantId: "me", principalId: "", displayName: "Me", color: "", kind: "human")
        }
        func nextEvent(timeoutMs: UInt64?) async throws -> CuaSDK.PresenceEvent? {
            queue.isEmpty ? nil : queue.removeFirst()
        }
        func roster() async throws -> [PresenceMember] { [] }
        func updateCursor(cursor: PresenceCursor) async throws {}
        func usesDatagrams() -> Bool { false }
        func view() -> PresenceView { PresenceView(me: "me", delayMs: nil) }
    }

    @Test func testWaitForFoldsAndIsBounded() async throws {
        let session = ScriptedPresence([
            CuaSDK.PresenceEvent(kind: "joined", participant: p("k"), participantId: nil, cursor: nil),
            moved("k", 0.1, 0.2),
        ])
        var roster = PresenceRoster()
        let e = try await session.waitFor(timeoutMs: 1_000, maxEvents: 10, roster: &roster) { $0.kind == "cursor_moved" }
        XCTAssertEqual(e.participantId, "k")
        XCTAssertEqual(roster.participants["k"]?.normalizedCursor(in: .zero), CGPoint(x: 0.1, y: 0.2))
        do {
            _ = try await session.waitFor(timeoutMs: 50, maxEvents: 3) { _ in true }
            XCTFail("an empty stream matched")
        } catch {}
    }
}

@Suite final class PresenceColorTests {
    /// Pinned to the Rust core's and the TypeScript SDK's answers.
    @Test func testAgentColorsAreStableAndReadable() {
        XCTAssertEqual(["ada", "bo", "koala", "inbox", "sales"].map(PresenceColors.color(for:)),
                       ["#bcf60c", "#4363d8", "#46f0f0", "#bcf60c", "#f58231"])
        XCTAssertEqual(PresenceColors.textColor(on: "#000075"), "#ffffff")
        XCTAssertEqual(PresenceColors.textColor(on: "#bcf60c"), "#000000")
        let who = PresenceColors.agentIdentity(id: "ada", displayName: "Ada")
        XCTAssertTrue(who.agent)
        XCTAssertEqual(who.color, "#bcf60c")
    }
}

@Suite final class RosterColorOfTests {
    @Test func testTheAssignedColorWinsWhilePresent() {
        let stable = PresenceColors.color(for: "koala")
        var roster = PresenceRoster()
        _ = roster.apply(me: PresenceParticipant(participantId: "me", principalId: "operator",
                                                 displayName: "Op", color: stable, kind: "human"),
                         members: [])
        XCTAssertEqual(roster.colorOf(principalID: "koala"), stable)
        _ = roster.apply(event: CuaSDK.PresenceEvent(
            kind: "joined",
            participant: PresenceParticipant(participantId: "k", principalId: "koala",
                                             displayName: "Koala", color: "#123456", kind: "agent"),
            participantId: nil, cursor: nil))
        XCTAssertEqual(roster.colorOf(principalID: "koala"), "#123456")
        _ = roster.apply(event: CuaSDK.PresenceEvent(kind: "left", participant: nil,
                                                     participantId: "k", cursor: nil))
        XCTAssertEqual(roster.colorOf(principalID: "koala"), stable)
    }
}
