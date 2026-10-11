// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSDK
import CuaSpacesFFI
@testable import CuaSpacesMacKit
import CuaSpacesStreaming
import Foundation
import Testing

/// Live, opt-in: the app's own stream stack against a real macOS Space. The
/// Space's backend and stream provider are the app's (`LiveSpacesBackend`),
/// the session is the one the detail view opens (the desktop), and input
/// goes through the real `LiveStreamInputView` (AppKit events in, encoder,
/// `interactive_input` batches out). Only the window server is left out: the
/// events are handed to the view, never posted to the host.
///
/// Set `CUA_SPACES_LIVE_INPUT` to a macOS Space id (e.g. `local:sf-mac`)
/// with a temp `HOME` and `CUA_HOME` that know it, on a fresh Space with no
/// Finder window open. The only effects are in the guest: a Dock click that
/// opens a Finder window, and ⌘W that closes it.
@MainActor
@Suite("Live Space input", .serialized)
struct LiveSpaceInputTests {
    static var spaceID: String? {
        ProcessInfo.processInfo.environment["CUA_SPACES_LIVE_INPUT"].flatMap { $0.isEmpty ? nil : $0 }
    }

    static func until(_ seconds: Double, _ done: () async -> Bool) async -> Bool {
        let deadline = Date().addingTimeInterval(seconds)
        while Date() < deadline {
            if await done() { return true }
            try? await Task.sleep(for: .milliseconds(250))
        }
        return false
    }

    static func mouse(_ type: NSEvent.EventType, at p: CGPoint, in view: NSView) -> NSEvent {
        NSEvent.mouseEvent(with: type, location: view.convert(p, to: nil), modifierFlags: [],
                           timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: 0, context: nil,
                           eventNumber: 0, clickCount: 1, pressure: type == .leftMouseUp ? 0 : 1)!
    }

    static func key(_ type: NSEvent.EventType, _ chars: String, code: UInt16,
                    flags: NSEvent.ModifierFlags) -> NSEvent {
        NSEvent.keyEvent(with: type, location: .zero, modifierFlags: flags,
                         timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: 0, context: nil,
                         characters: chars, charactersIgnoringModifiers: chars, isARepeat: false,
                         keyCode: code)!
    }

    @Test func clicksAndKeysFromTheAppReachTheSpace() async throws {
        guard let id = Self.spaceID else { return }
        let backend = try LiveSpacesBackend.make()
        let provider = try await backend.streamProvider(id: id)

        // The Stream section's list: only streamable windows, with titles.
        let listed = try await provider.availableWindows()
        #expect(listed.allSatisfy { !$0.title.isEmpty }, "\(listed.map(\.app))")
        let finders = { (try? await provider.availableWindows())?.filter { $0.app == "Finder" }.count ?? 0 }
        let before = await finders()

        let session = LiveStreamSession(provider: provider)
        await session.start()
        #expect(await Self.until(20) { session.status == .streaming && session.surfaceSize != .zero },
                "status \(session.status)")
        let size = session.surfaceSize
        let view = LiveStreamInputView(frame: CGRect(origin: .zero, size: size))
        view.surfaceSize = size
        view.onInput = { [weak session] in session?.send($0) }
        // A window that is never shown, so the view can be first responder
        // (AppKit sends Command chords to `performKeyEquivalent` then).
        let window = NSWindow(contentRect: view.frame, styleMask: [.borderless], backing: .buffered, defer: true)
        window.contentView = view
        window.makeFirstResponder(view)

        // The Dock's first icon, the Finder: 60 px in, 33 px up.
        let finder = CGPoint(x: 60 * size.width / 1024, y: size.height - 33 * size.height / 768)
        view.mouseMoved(with: Self.mouse(.mouseMoved, at: finder, in: view))
        view.mouseDown(with: Self.mouse(.leftMouseDown, at: finder, in: view))
        view.mouseUp(with: Self.mouse(.leftMouseUp, at: finder, in: view))
        let opened = await Self.until(15) { await finders() > before }
        #expect(opened, "a Finder window opened (\(before) before)")
        #expect(session.inputEventsAcknowledged > 0, "the Space acknowledged the click")
        #expect(session.inputFailure == nil, "\(session.inputFailure ?? "")")

        // ⌘W through the same view closes it, once the new window has the
        // keyboard.
        try? await Task.sleep(for: .seconds(2))
        let sent = session.inputEventsSent
        #expect(view.performKeyEquivalent(with: Self.key(.keyDown, "w", code: 13, flags: .command)))
        #expect(await Self.until(5) { session.inputEventsAcknowledged >= UInt64(sent + 2) },
                "acked \(session.inputEventsAcknowledged) of \(session.inputEventsSent)")
        let closed = await Self.until(15) { await finders() == before }
        #expect(closed, "⌘W closed the Finder window")
        #expect(session.inputFailure == nil, "\(session.inputFailure ?? "")")
        await session.stop()
    }
}
