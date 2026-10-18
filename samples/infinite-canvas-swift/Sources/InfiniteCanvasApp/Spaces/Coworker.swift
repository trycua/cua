// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CanvasModel
import Cua
import CuaSpacesStreaming
import Foundation

/// A human collaborator on a Space, for demos: a second presence
/// participant (a person, not an agent) with its own name and color, whose
/// pointer moves like a hand does (`HumanPath`) and whose clicks and
/// keystrokes are real input on the window, sent as `interactive_input`
/// over its own media session, exactly what a person's viewer sends.
@MainActor
final class Coworker {
    let space: CuaSDK.Space
    let name: String
    let color: String
    private var presence: SpacePresence?
    private var session: SpaceStreamSession?
    private var sequence: UInt64 = 1
    private var position = CGPoint(x: 0.92, y: 0.9)
    private var seed: UInt64 = 0xC0FFEE

    init(space: CuaSDK.Space, name: String, color: String) {
        self.space = space
        self.name = name
        self.color = color
    }

    /// Join, glide to `click` in `windowID` (window fractions), click, type
    /// `text` at a person's pace, drift away, and leave after `linger`.
    func run(windowID: String, click: CGPoint, text: String, linger: TimeInterval = 4) async throws {
        let identity = PresenceIdentity(id: "coworker-\(ProcessInfo.processInfo.processIdentifier)",
                                        displayName: name, color: color, agent: false)
        let p = try await space.joinPresence(identity: identity, timeoutMs: 10_000)
        presence = p
        var options = SpaceStreamOptions(maxFps: 2, audio: false, policy: "background_only")
        options.windowId = windowID
        options.codecs = ["h264"]
        options.maxDimension = 320
        let s = try await space.streamSession(options: options, frames: DiscardFrames(), audio: nil)
        session = s
        defer { Task { [s, p] in _ = try? await s.close(); try? await p.leave() } }

        try await cursor(windowID, at: position, visible: true)
        try await Task.sleep(for: .milliseconds(400))
        try await move(windowID, to: CGPoint(x: click.x + 0.18, y: click.y + 0.22))
        try await Task.sleep(for: .milliseconds(350))
        try await move(windowID, to: click)
        try await Task.sleep(for: .milliseconds(220))
        send([.pointer(phase: .move, button: nil, x: click.x, y: click.y, modifiers: []),
              .pointer(phase: .down, button: .left, x: click.x, y: click.y, modifiers: [])])
        try await Task.sleep(for: .milliseconds(90))
        send([.pointer(phase: .up, button: .left, x: click.x, y: click.y, modifiers: [])])
        try await Task.sleep(for: .milliseconds(450))
        // Hands leave the mouse to type: the pointer eases a little aside.
        try await move(windowID, to: CGPoint(x: click.x + 0.06, y: click.y + 0.09))
        for (ch, delay) in zip(text, HumanPath.typingDelays(text, seed: nextSeed())) {
            try await Task.sleep(for: .seconds(delay))
            send([.textCommit(String(ch))])
        }
        try await Task.sleep(for: .milliseconds(600))
        try await move(windowID, to: CGPoint(x: click.x + 0.3, y: min(click.y + 0.35, 0.95)))
        try await Task.sleep(for: .seconds(linger))
        try await cursor(windowID, at: position, visible: false)
    }

    private func nextSeed() -> UInt64 {
        seed &+= 0x9E37_79B9
        return seed
    }

    private func move(_ windowID: String, to target: CGPoint) async throws {
        let start = ContinuousClock.now
        for sample in HumanPath.samples(from: position, to: target, seed: nextSeed(), rate: 30) {
            let due = start + .milliseconds(Int(sample.t * 1000))
            try await Task.sleep(until: due, clock: .continuous)
            try await cursor(windowID, at: sample.point, visible: true)
        }
        position = target
    }

    private func cursor(_ windowID: String, at p: CGPoint, visible: Bool) async throws {
        try await presence?.updateCursor(cursor: PresenceCursor(
            displayId: "", windowId: windowID,
            x: Double(min(max(p.x, 0), 1)), y: Double(min(max(p.y, 0), 1)), visible: visible))
    }

    private func send(_ events: [InteractiveInputEvent]) {
        guard let session else { return }
        let first = sequence
        sequence += UInt64(events.count)
        if let text = try? interactiveInputText(session: SessionID(session.mediaSessionId()),
                                                firstSequence: first, events: events) {
            try? session.sendText(json: text)
        }
    }
}

/// The coworker's media session exists for input; its frames are dropped.
final class DiscardFrames: FrameSink, @unchecked Sendable {
    func onFrame(frame: VideoFrame) {}
    func onEvent(event: MediaEvent) {}
}
