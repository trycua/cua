// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

#if canImport(AppKit)
import AppKit
import Cua
import Foundation
import Testing
@testable import CuaSpacesStreaming

/// The presence overlay over a stream: your cursor at the local pointer
/// with no network, the system cursor hidden only over the picture, and
/// remote cursors that fade when idle and go away when their run ends or
/// their heartbeat stops. Offscreen: no window is shown and the real cursor
/// is never touched.
@MainActor
@Suite(.serialized) struct PresenceOverlayTests {
    static func participant(_ id: String, _ name: String, _ color: String, kind: String = "human")
        -> PresenceParticipant {
        PresenceParticipant(participantId: id, principalId: "u-\(id)", displayName: name, color: color, kind: kind)
    }

    static func joined(_ p: PresenceParticipant) -> CuaSDK.PresenceEvent {
        CuaSDK.PresenceEvent(kind: "joined", participant: p, participantId: nil, cursor: nil)
    }

    static func moved(_ id: String, _ x: Double, _ y: Double, at: Double, shape: String = "arrow",
                      window: String? = nil) -> CuaSDK.PresenceEvent {
        CuaSDK.PresenceEvent(kind: "cursor_moved", participant: nil, participantId: id,
                             cursor: PresenceCursor(displayId: "", windowId: window, x: x, y: y, visible: true,
                                                    pressed: false, shape: shape, shapeSource: "hit_test",
                                                    atMs: at, receivedMs: at))
    }

    /// A 640x400 overlay over a 1280x800 surface (no letterbox), with me and
    /// an agent present.
    static func overlay() -> (PresenceOverlayView, PresenceView) {
        let view = PresenceView(me: "me", delayMs: 100)
        _ = view.apply(event: joined(participant("me", "Me", "#e6194b")), localMs: 0)
        _ = view.apply(event: joined(participant("koala", "Koala", "#3cb44b", kind: "agent")), localMs: 0)
        let o = PresenceOverlayView(frame: CGRect(x: 0, y: 0, width: 640, height: 400))
        o.surfaceSize = CGSize(width: 1280, height: 800)
        o.presenceView = view
        o.isFocused = { true }
        o.cursorHider = SystemCursorHider(hide: {}, unhide: {})
        return (o, view)
    }

    /// Counts hide and unhide calls instead of touching the real cursor.
    final class CursorCalls {
        var hides = 0
        var unhides = 0
        var depth: Int { hides - unhides }
    }

    static func counting(_ o: PresenceOverlayView) -> CursorCalls {
        let calls = CursorCalls()
        o.cursorHider = SystemCursorHider(hide: { calls.hides += 1 }, unhide: { calls.unhides += 1 })
        return calls
    }

    @Test func yourCursorIsDrawnAtTheLocalPointerWithoutTheNetwork() {
        let (o, view) = Self.overlay()
        // No session: nothing can be sent or received.
        #expect(o.session == nil)
        o.pointer(at: CGPoint(x: 100, y: 50))
        o.render(localMs: 10)
        let me = o.drawn.first { $0.isMe }
        #expect(me?.tip == CGPoint(x: 100, y: 50))
        #expect(me?.color == "#e6194b")
        #expect(me?.shape == "arrow")
        // The Space reports the shape at your position; the position stays local.
        _ = view.apply(event: CuaSDK.PresenceEvent(kind: "shape_changed", participant: nil, participantId: "me",
                                                   cursor: nil, shape: "text", shapeSource: "system"),
                       localMs: 20)
        o.pointer(at: CGPoint(x: 101, y: 51))
        o.render(localMs: 21)
        let again = o.drawn.first { $0.isMe }
        #expect(again?.tip == CGPoint(x: 101, y: 51))
        #expect(again?.shape == "text")
        // Leaving the view removes your cursor at once.
        o.pointer(at: nil)
        o.render(localMs: 22)
        #expect(!o.drawn.contains { $0.isMe })
    }

    @Test func thePictureIsWhereYourCursorCanBeDrawn() {
        let o = PresenceOverlayView(frame: CGRect(x: 0, y: 0, width: 800, height: 400))
        o.surfaceSize = CGSize(width: 1280, height: 800)  // letterboxed: 640x400 centred
        #expect(o.pictureRect == nil, "no presence, no cursor of yours")
        o.presenceView = PresenceView(me: "me", delayMs: 100)
        #expect(o.pictureRect == CGRect(x: 80, y: 0, width: 640, height: 400))
        o.pointer(at: CGPoint(x: 40, y: 200))  // in the letterbox
        #expect(!o.isHoveringStream)
        o.pointer(at: CGPoint(x: 400, y: 200))
        #expect(o.isHoveringStream)
        o.pointer(at: nil)
        #expect(!o.isHoveringStream)
        // hitTest lets input through.
        #expect(o.hitTest(CGPoint(x: 400, y: 200)) == nil)
        o.presenceView = nil
        #expect(o.pictureRect == nil, "presence gone")
    }

    @Test func theRuleDrawsYourCursorOnlyWhenEverythingHolds() {
        let all = SystemCursorRule.drawsOwnCursor(presenceLive: true, streamLive: true, geometryUsable: true,
                                                  hovering: true, focused: true)
        #expect(all)
        for i in 0..<5 {
            var f = [true, true, true, true, true]
            f[i] = false
            #expect(!SystemCursorRule.drawsOwnCursor(presenceLive: f[0], streamLive: f[1], geometryUsable: f[2],
                                                     hovering: f[3], focused: f[4]))
        }
    }

    /// The system cursor is hidden exactly while your cursor is drawn, and
    /// every hide is matched by one unhide: mouse exit, stream loss, no
    /// presence, a zero-size stream, focus loss and teardown all restore it.
    @Test func theSystemCursorIsHiddenExactlyWhileYoursIsDrawn() {
        let (o, _) = Self.overlay()
        let calls = Self.counting(o)
        func hover() { o.pointer(at: CGPoint(x: 100, y: 50)); o.render(localMs: 10) }
        func drawnMe() -> Bool { o.drawn.contains { $0.isMe } }

        hover()
        #expect(drawnMe() && calls.depth == 1)
        hover()
        #expect(calls.hides == 1, "hidden once, however many moves")

        o.pointer(at: nil)  // mouse exit
        #expect(!drawnMe() && calls.depth == 0)

        hover()
        o.isStreamLive = false  // stream lost
        o.render(localMs: 11)
        #expect(!drawnMe() && calls.depth == 0)
        o.isStreamLive = true

        hover()
        o.surfaceSize = .zero  // no geometry
        o.render(localMs: 12)
        #expect(!drawnMe() && calls.depth == 0)
        o.surfaceSize = CGSize(width: 1280, height: 800)

        hover()
        o.isFocused = { false }  // the window lost the keyboard
        o.render(localMs: 13)
        #expect(!drawnMe() && calls.depth == 0)
        o.isFocused = { true }

        hover()
        o.presenceView = nil  // presence gone
        #expect(calls.depth == 0)

        let (o2, _) = Self.overlay()
        let calls2 = Self.counting(o2)
        o2.pointer(at: CGPoint(x: 10, y: 10))
        o2.render(localMs: 1)
        #expect(calls2.depth == 1)
        o2.stop()  // teardown
        #expect(calls2.depth == 0 && calls2.hides == 1)
    }

    @Test func noPresenceYetKeepsTheSystemCursor() {
        // Presence is connecting but you have not joined: nothing of yours
        // is drawn, so nothing is hidden.
        let o = PresenceOverlayView(frame: CGRect(x: 0, y: 0, width: 640, height: 400))
        o.surfaceSize = CGSize(width: 1280, height: 800)
        o.presenceView = PresenceView(me: "me", delayMs: 100)
        o.isFocused = { true }
        let calls = Self.counting(o)
        o.pointer(at: CGPoint(x: 100, y: 50))
        o.render(localMs: 10)
        #expect(!o.drawn.contains { $0.isMe })
        #expect(calls.hides == 0)
    }

    @Test func remoteCursorsAreDrawnInTheirColorAndShapeWithAName() {
        let (o, view) = Self.overlay()
        _ = view.apply(event: Self.moved("koala", 0.5, 0.25, at: 1_000, shape: "pointer"), localMs: 1_000)
        o.render(localMs: 1_200)
        let koala = o.drawn.first { $0.participantID == "koala" }
        #expect(koala?.tip == CGPoint(x: 320, y: 100))
        #expect(koala?.shape == "pointer")
        #expect(koala?.color == "#3cb44b")
        #expect(koala?.name == "Koala")
        #expect(koala?.isAgent == true)
    }

    @Test func aCursorOnAnotherWindowIsNotDrawnHere() {
        let (o, view) = Self.overlay()
        _ = view.apply(event: Self.moved("koala", 0.5, 0.5, at: 1_000, window: "w-2"), localMs: 1_000)
        o.windowID = "w-1"
        o.render(localMs: 1_200)
        #expect(!o.drawn.contains { $0.participantID == "koala" })
        o.windowID = "w-2"
        o.render(localMs: 1_201)
        #expect(o.drawn.contains { $0.participantID == "koala" })
    }

    @Test func runEndRemovesTheAgentCursor() {
        let (o, view) = Self.overlay()
        _ = view.apply(event: Self.moved("koala", 0.5, 0.5, at: 1_000), localMs: 1_000)
        o.render(localMs: 1_200)
        #expect(o.drawn.contains { $0.participantID == "koala" })
        _ = view.apply(event: CuaSDK.PresenceEvent(kind: "left", participant: nil, participantId: "koala",
                                                   cursor: nil, reason: "run_ended"),
                       localMs: 1_300)
        o.render(localMs: 1_301)
        #expect(!o.drawn.contains { $0.participantID == "koala" })
    }

    @Test func aMissedHeartbeatRemovesTheCursor() {
        let (o, view) = Self.overlay()
        view.setHeartbeatIntervalMs(ms: 1_000)
        _ = view.apply(event: CuaSDK.PresenceEvent(kind: "heartbeat", participant: nil, participantId: nil,
                                                   cursor: nil, participantIds: ["me", "koala"]),
                       localMs: 900)
        _ = view.apply(event: Self.moved("koala", 0.5, 0.5, at: 1_000), localMs: 1_000)
        o.render(localMs: 1_200)
        #expect(o.drawn.contains { $0.participantID == "koala" })
        // No heartbeat for three intervals: the overlay's once-a-second
        // expiry drops everyone but you.
        o.render(localMs: 4_500)
        #expect(!o.drawn.contains { $0.participantID == "koala" })
    }

    @Test func anIdleCursorFades() {
        let (o, view) = Self.overlay()
        _ = view.apply(event: Self.moved("koala", 0.5, 0.5, at: 1_000), localMs: 1_000)
        o.render(localMs: 6_000)
        #expect(o.drawn.first { $0.participantID == "koala" }?.alpha == 1)
        o.render(localMs: 6_150)
        let fading = o.drawn.first { $0.participantID == "koala" }?.alpha ?? 0
        #expect(fading > 0.3 && fading < 0.7, "\(fading)")
        o.render(localMs: 6_400)
        #expect(!o.drawn.contains { $0.participantID == "koala" })
        _ = view.apply(event: Self.moved("koala", 0.55, 0.5, at: 6_500), localMs: 6_500)
        o.render(localMs: 6_600)
        #expect(o.drawn.first { $0.participantID == "koala" }?.alpha == 1, "a move brings it back")
    }
}
#endif
