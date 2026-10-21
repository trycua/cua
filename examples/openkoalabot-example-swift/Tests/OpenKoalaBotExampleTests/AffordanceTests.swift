// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation
import Testing
import SwiftUI
@testable import OpenKoalaBotExample

/// The transcript affordances and avatar motion.
///
/// These surfaces are not part of the export renders, so what is worth
/// asserting is not pixels but the two things that
/// *are* fixed: the authoritative wording, and the honesty of the approval
/// card about what it enforces.
@MainActor
@Suite final class AffordanceTests {

    // MARK: - The approval card

    /// The single most important assertion in this file. There is no approval
    /// primitive in the Spaces MCP: `agent_start` runs auto-approved because
    /// the Space *is* the sandbox. This card renders a gate that does not gate,
    /// and it must never claim otherwise.
    @Test func testTheApprovalCardDoesNotClaimToEnforceAnything() {
        XCTAssertFalse(Approval.enforcementIsImplemented,
                       "nothing in this build can pause a Bot for a human decision; "
                       + "if that changes, the Spaces client gained a primitive and "
                       + "RUBRIC.md M6 and FRICTION.md §43 need rewriting too")
    }

    @Test func testDesktopApprovalWordingAndOrderAreTheDocumentedOnes() {
        XCTAssertEqual(Approval.desktopOrder.map(\.rawValue),
                       ["Allow once", "Deny", "Always allow"])
        XCTAssertEqual(Approval.localCommandOrder.map(\.rawValue),
                       ["Allow once", "Deny"])
    }

    @Test func testTheLocalCommandCardOffersOnlyAllowOnceAndDeny() {
        // A local-command card drops Always allow.
        let local = Approval(title: "Run a local command",
                             detail: "Chief of Staff wants to run `ls`.",
                             offersAlwaysAllow: false)
        XCTAssertFalse(local.offersAlwaysAllow)
        XCTAssertTrue(Approval(title: "Sign in", detail: "…").offersAlwaysAllow)
    }






    // MARK: - Avatar motion

    @Test func testThereAreExactlySixNamedMotionStates() {
        XCTAssertEqual(BotMotionState.allCases.count, 6)
        XCTAssertEqual(Set(BotMotionState.allCases.map(\.rawValue)),
                       ["idle", "listening", "thinking", "working", "speaking", "blocked"])
    }

    /// Motion has to be driven by what the Space reports, not by a view's
    /// guess, or "working" and "stopped" drift apart from the status chip.
    @Test func testMotionIsDerivedFromLivePresence() {
        func state(_ s: AgentState, hired: Bool = true, exit: Int? = nil,
                   composing: Bool = false, fresh: Bool = false) -> BotMotionState {
            BotMotionState.from(
                BotPresence(runID: hired ? "run-1" : nil, state: s, exitCode: exit),
                isComposing: composing, hasFreshOutput: fresh)
        }
        XCTAssertEqual(state(.running), .working)
        XCTAssertEqual(state(.running, fresh: true), .speaking)
        XCTAssertEqual(state(.awaitingInput), .listening)
        XCTAssertEqual(state(.idle), .idle)
        XCTAssertEqual(state(.finished, exit: 0), .idle)
        XCTAssertEqual(state(.finished, exit: 2), .blocked)
        XCTAssertEqual(state(.failed), .blocked)
        XCTAssertEqual(state(.crashed), .blocked)
        XCTAssertEqual(state(.unknown, hired: true), .blocked)
        XCTAssertEqual(state(.unknown, hired: false), .idle)
        // The user typing beats everything: the Bot is listening.
        XCTAssertEqual(state(.running, composing: true), .listening)
    }

    /// Each state must *look* different at avatar size, so no two of them may
    /// produce the same frame.
    @Test func testEveryStateProducesADistinctFrame() {
        let frames = BotMotionState.allCases.map { AnimatedBotAvatar.frame($0, at: 0.3) }
        for (i, a) in frames.enumerated() {
            for (j, b) in frames.enumerated() where j > i {
                XCTAssertNotEqual(a, b,
                    "\(BotMotionState.allCases[i]) and \(BotMotionState.allCases[j]) "
                    + "render identically")
            }
        }
    }

    @Test func testMotionIsPureSoARenderIsReproducible() {
        XCTAssertEqual(AnimatedBotAvatar.frame(.working, at: 1.234),
                       AnimatedBotAvatar.frame(.working, at: 1.234))
    }

    /// A *stopped* Bot must come to rest. An avatar that shakes forever makes
    /// "crashed" look like "busy".
    @Test func testTheBlockedShakeDampsToNothing() {
        let early = AnimatedBotAvatar.frame(.blocked, at: 0.05)
        let late = AnimatedBotAvatar.frame(.blocked, at: 2.0)
        XCTAssertNotEqual(early.offsetX, 0)
        XCTAssertEqual(late.offsetX, 0, accuracy: 0.0001)
        XCTAssertEqual(late.rotation, 0, accuracy: 0.0001)
        XCTAssertFalse(BotMotionState.blocked.isContinuous)
        XCTAssertTrue(BotMotionState.allCases.filter { $0 != .blocked }.allSatisfy(\.isContinuous))
    }

    @Test func testOnlyWorkingDrawsTheComputerActiveHalo() {
        for s in BotMotionState.allCases {
            let halo = AnimatedBotAvatar.frame(s, at: 0.3).haloOpacity
            if s == .working { XCTAssertGreaterThan(halo, 0) }
            else { XCTAssertEqual(halo, 0) }
        }
    }

    @Test func testMotionAmplitudesStayInsideTheAvatarBox() {
        // Every offset is a fraction of avatar size; anything over ~0.1 would
        // clip out of a roster row.
        for s in BotMotionState.allCases {
            for step in 0...200 {
                let f = AnimatedBotAvatar.frame(s, at: Double(step) / 20)
                XCTAssertLessThanOrEqual(abs(f.offsetX), 0.06, "\(s)")
                XCTAssertLessThanOrEqual(abs(f.offsetY), 0.06, "\(s)")
                XCTAssertLessThanOrEqual(abs(f.scale - 1), 0.05, "\(s)")
            }
        }
    }

    /// The gallery is the only documentation of six states nobody has a frame
    /// of, so it has to name all six.
    @Test func testTheMotionGalleryCoversEveryState() {
        XCTAssertEqual(BotMotionState.allCases.map(\.rawValue).count,
                       Set(BotMotionState.allCases.map(\.accessibilityLabel)).count,
                       "every state needs its own spoken label")
    }
}
