// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation
import Testing
@testable import OpenKoalaBotExample

/// The pending state behind the send indicator.
///
/// The defect these exist for: sending a message showed nothing at all until
/// the whole reply landed, because the only thing driving the indicator was
/// `typing`, which means "this Bot owes its opening greeting" and is cleared
/// the moment the tail is non-empty. On a send the tail is *already* non-empty,
/// so that test could never fire.
///
/// The fix adds `pending`, and the thing worth testing about it is not that it
/// goes up — it is that it comes **down on every path**. A spinner that can get
/// stuck is worse than no spinner, so there is one test per exit.
@MainActor
@Suite final class PendingSendTests {

    /// A store attached to a Space with one Bot whose run is in flight.
    private func attached(_ c: ScriptedSpacesClient) async throws -> (BotStore, String) {
        let s = BotStore(client: c, identities: Fixtures.bots)
        await s.connect()
        c.statusToReturn = AgentStatus(state: .running, reason: "turn in flight",
                                       acceptsMessage: true, exitCode: nil,
                                       summary: "", tail: "greeting\n")
        _ = try await s.hire("cos", prompt: "do the cos job")
        return (s, "cos")
    }

    // MARK: The indicator goes up at all

    /// The whole point: the indicator is up *before* the reply, and the user's
    /// own message is already in the transcript beside it.
    @Test func testSendingShowsPendingAndTheUsersMessageImmediately() async throws {
        let c = ScriptedSpacesClient()
        let (s, id) = try await attached(c)
        // Hold the tail still, so nothing clears the pending state for us.
        c.statusToReturn.tail = "greeting\n"

        XCTAssertFalse(s.isAwaitingReply(id), "nothing sent yet")
        _ = await s.send("write me a function", to: id)

        XCTAssertTrue(s.isAwaitingReply(id),
                      "no indicator after a send the harness accepted")
        let texts = s.thread(for: id).messages.compactMap { m -> String? in
            if case .prose(let t) = m.body, m.sender == .user { return t }
            return nil
        }
        XCTAssertTrue(texts.contains("write me a function"),
                      "the user's own message is not in the transcript: \(texts)")
    }

    /// `typing` and `pending` are different states and must stay different.
    /// Collapsing them is what would re-create the defect.
    @Test func testPendingIsNotTyping() async throws {
        let c = ScriptedSpacesClient()
        let (s, id) = try await attached(c)
        c.statusToReturn.tail = "greeting\n"
        _ = await s.send("hello", to: id)
        XCTAssertTrue(s.isAwaitingReply(id))
        XCTAssertFalse(s.typing.contains(id),
                       "a send set the greeting flag; those are different states")
    }

    // MARK: Every way it comes down

    /// Exit 1 — the first token. The tail grows past where it stood when the
    /// user spoke, so the reply is visibly arriving and an indicator beside it
    /// would be a lie.
    @Test func testPendingClearsOnTheFirstToken() async throws {
        let c = ScriptedSpacesClient()
        let (s, id) = try await attached(c)
        c.statusToReturn.tail = "greeting\n"
        _ = await s.send("hello", to: id)
        XCTAssertTrue(s.isAwaitingReply(id))

        c.statusToReturn.tail = "greeting\nhere is the f"
        await s.refresh(id)
        XCTAssertFalse(s.isAwaitingReply(id),
                       "the reply began arriving and the indicator stayed up")
    }

    /// Exit 2 — the turn ended without the tail moving. Nothing more is coming,
    /// so the indicator must not outlive the run.
    @Test func testPendingClearsWhenTheTurnEndsEvenIfTheTailNeverMoved() async throws {
        for terminal in [AgentState.idle, .finished, .failed, .crashed, .awaitingInput] {
            let c = ScriptedSpacesClient()
            let (s, id) = try await attached(c)
            c.statusToReturn.tail = "greeting\n"
            _ = await s.send("hello", to: id)
            XCTAssertTrue(s.isAwaitingReply(id), "\(terminal): never went pending")

            c.statusToReturn.state = terminal
            await s.refresh(id)
            XCTAssertFalse(s.isAwaitingReply(id),
                           "\(terminal): the run stopped and the indicator stayed up")
        }
    }

    /// Exit 3 — **refusal**. FRICTION §24: there is no outbox, so a refused
    /// message is never going to be answered. A pending bubble left behind it
    /// would promise a reply that cannot arrive.
    @Test func testARefusedSendLeavesNoPendingBubble() async throws {
        let c = ScriptedSpacesClient()
        let (s, id) = try await attached(c)
        c.statusToReturn.tail = "greeting\n"
        c.nextOutcome = MessageOutcome(accepted: false, reason: "agent is busy")

        let outcome = await s.send("hello", to: id)
        XCTAssertFalse(outcome.accepted)
        XCTAssertFalse(s.isAwaitingReply(id),
                       "a refused send left the indicator spinning forever")
        // And the refusal is still shown rather than swallowed.
        let events = s.thread(for: id).messages.compactMap { m -> String? in
            if case .systemEvent(let t) = m.body { return t }
            return nil
        }
        XCTAssertTrue(events.contains { Message.isRefusal($0) && $0.contains("busy") },
                      "the refusal was cleared away along with the indicator: \(events)")
    }

    /// Exit 4 — the transport threw. Same rule: no reply is coming.
    @Test func testATransportFailureLeavesNoPendingBubble() async throws {
        let c = ScriptedSpacesClient()
        let (s, id) = try await attached(c)
        c.statusToReturn.tail = "greeting\n"
        c.messageError = ScriptedSpacesClient.Boom()

        let outcome = await s.send("hello", to: id)
        XCTAssertFalse(outcome.accepted)
        XCTAssertFalse(s.isAwaitingReply(id),
                       "a failed send left the indicator spinning forever")
    }

    /// Exit 5 — the status probe itself failed. "Unknown" is not "still
    /// working", and the probe is the only thing that could ever take the
    /// indicator down, so it must come down here too.
    @Test func testAFailedStatusProbeClearsPendingRatherThanSpinningForever() async throws {
        let c = ScriptedSpacesClient()
        let (s, id) = try await attached(c)
        c.statusToReturn.tail = "greeting\n"
        _ = await s.send("hello", to: id)
        XCTAssertTrue(s.isAwaitingReply(id))

        c.statusError = ScriptedSpacesClient.Boom()
        await s.refresh(id)
        XCTAssertEqual(s.presence(for: id).state, .unknown)
        XCTAssertFalse(s.isAwaitingReply(id),
                       "the Space went away and the indicator kept spinning")
    }

    /// A second send after the first resolved must be able to go pending again
    /// — the state is per-turn, not a latch.
    @Test func testPendingCanBeRaisedAgainAfterItCleared() async throws {
        let c = ScriptedSpacesClient()
        let (s, id) = try await attached(c)
        c.statusToReturn.tail = "greeting\n"

        _ = await s.send("one", to: id)
        c.statusToReturn.tail = "greeting\nfirst reply"
        await s.refresh(id)
        XCTAssertFalse(s.isAwaitingReply(id))

        _ = await s.send("two", to: id)
        XCTAssertTrue(s.isAwaitingReply(id), "the second send never went pending")
    }

    // MARK: The indicator itself

    /// The dots are a pure function of time, so the animation is testable
    /// without a run loop: three dots, staggered, none of them ever below the
    /// baseline.
    @Test func testTheTypingIndicatorDotsAreStaggeredAndBounded() {
        for t in stride(from: 0.0, to: 4.0, by: 0.05) {
            let offsets = (0..<3).map { ShellTypingIndicator.offset(index: $0, time: t) }
            for o in offsets {
                XCTAssertLessThanOrEqual(o, 0.0001, "a dot rose above its rest position")
                XCTAssertGreaterThanOrEqual(o, -3.0, "a dot travelled further than the design")
            }
        }
        // Staggered: at some moment the three dots are not all in the same
        // place, which is the whole point of the per-dot phase offset.
        let differs = stride(from: 0.0, to: 2.0, by: 0.01).contains { t in
            let o = (0..<3).map { ShellTypingIndicator.offset(index: $0, time: t) }
            return abs(o[0] - o[1]) > 0.2 || abs(o[1] - o[2]) > 0.2
        }
        XCTAssertTrue(differs, "all three dots move together — there is no stagger")
    }
}
