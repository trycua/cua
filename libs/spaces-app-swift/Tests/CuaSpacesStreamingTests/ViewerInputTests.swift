// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

#if canImport(AppKit)
import AppKit
import CoreGraphics
import Cua
import Foundation
import SwiftUI
import Testing
@testable import CuaSpacesStreaming

/// A media session that records the text frames the viewer sends.
private final class RecordingMedia: SpaceStreamSession, @unchecked Sendable {
    private let lock = NSLock()
    private var texts: [String] = []

    init() { super.init(noHandle: NoHandle()) }
    required init(unsafeFromHandle handle: UInt64) { fatalError("not used") }

    var sent: [String] { lock.lock(); defer { lock.unlock() }; return texts }

    override func codec() -> String { "h264" }
    override func mediaSessionId() -> String { "media-1" }
    override func requestKeyframe() throws {}
    override func sendText(json: String) throws { lock.lock(); texts.append(json); lock.unlock() }
    override func isOpen() -> Bool { true }
    override func close() async throws -> SpaceStreamStats {
        SpaceStreamStats(frames: 0, keyframes: 0, framesDropped: 0, framesGated: 0,
                         keyframeRequests: 0, audioPackets: 0, audioLost: 0, events: 0, malformed: 0)
    }
}

/// Opens one `RecordingMedia` and keeps the frame sink, so a test can play
/// the Space's side (acknowledgements).
private final class RecordingProvider: SpaceStreamSourceProviding, @unchecked Sendable {
    let media = RecordingMedia()
    private let lock = NSLock()
    private var sink: FrameSink?
    private(set) var sources: [StreamSource] = []
    private(set) var policies: [String] = []

    var frames: FrameSink? { lock.lock(); defer { lock.unlock() }; return sink }

    func availableWindows() async throws -> [StreamWindow] { [] }
    func openSession(_ source: StreamSource, frames: FrameSink,
                     audio: AudioSink?) async throws -> SpaceStreamSession {
        try await openSession(source, policy: SpaceStreamProvider.inputPolicy(for: source),
                              frames: frames, audio: audio)
    }
    func openSession(_ source: StreamSource, policy: String, frames: FrameSink,
                     audio: AudioSink?) async throws -> SpaceStreamSession {
        lock.lock(); sink = frames; sources.append(source); policies.append(policy); lock.unlock()
        return media
    }
    func joinPresence(name: String, color: String?) async throws -> SpacePresence {
        throw StreamError.noStream("no presence in this test")
    }
}

@MainActor
private func eventually(_ what: String, _ cond: @MainActor () -> Bool) async {
    for _ in 0..<400 {
        if cond() { return }
        try? await Task.sleep(for: .milliseconds(5))
    }
    Issue.record("timed out waiting for \(what)")
}

/// A mouse event at `point` (window coordinates, bottom-left origin). Built
/// and handed to the view directly: nothing is posted to the system.
@MainActor
private func mouse(_ type: NSEvent.EventType, at point: CGPoint, in window: NSWindow) -> NSEvent {
    NSEvent.mouseEvent(with: type, location: point, modifierFlags: [], timestamp: 0,
                       windowNumber: window.windowNumber, context: nil, eventNumber: 0,
                       clickCount: 1, pressure: 1)!
}

/// An offscreen window (never ordered front, never key).
@MainActor
private func offscreenWindow(_ size: CGSize) -> NSWindow {
    let window = NSWindow(contentRect: CGRect(origin: .zero, size: size),
                          styleMask: [.titled], backing: .buffered, defer: true)
    window.isReleasedWhenClosed = false
    return window
}

private func pointerEvents(in text: String) -> [[String: Any]] {
    guard let root = try? JSONSerialization.jsonObject(with: Data(text.utf8)) as? [String: Any],
          let message = root["message"] as? [String: Any],
          message["type"] as? String == "interactive_input",
          let payload = message["payload"] as? [String: Any],
          let events = payload["events"] as? [[String: Any]] else { return [] }
    return events.filter { $0["kind"] as? String == "pointer" }
}

/// Clicks in the Swift viewer reach the Space: the view takes the mouse,
/// maps it through the letterboxed 1024x768 guest surface, and the session
/// sends it on the media socket with a policy the Space accepts for a
/// desktop. Every step here was a place the Dock click could be lost.
@Suite(.serialized) @MainActor struct ViewerInputTests {

    /// The desktop is driven like a physical mouse (`allow_activation`); a
    /// display has no window to take background input, and the driver
    /// refuses `background_only` there. Windows stay background-only.
    @Test func desktopStreamsAskForActivationWindowsStayInTheBackground() {
        #expect(SpaceStreamProvider.inputPolicy(for: .desktop) == "allow_activation")
        #expect(SpaceStreamProvider.inputPolicy(for: .window(StreamWindow(id: "w", app: "A", title: "")))
                == "background_only")
    }

    /// 1024x768 in a wider view is pillarboxed; in a taller one letterboxed.
    /// Points in the bars send nothing; the Dock's Finder icon maps to the
    /// same normalized point at any view size (Retina or not: the mapping
    /// is in points against the surface's aspect, never in backing pixels).
    @Test func guestSurfaceMappingHandlesLetterboxingAndScale() throws {
        let surface = CGSize(width: 1024, height: 768)
        let finder = CGPoint(x: 60.0 / 1024.0, y: 735.0 / 768.0)
        for view in [CGSize(width: 1200, height: 600), CGSize(width: 512, height: 900),
                     CGSize(width: 1024, height: 768), CGSize(width: 2048, height: 1536)] {
            let g = StreamGeometry(surfaceSize: surface, viewSize: view)
            let rect = g.contentRect
            #expect(abs(rect.width / rect.height - 1024.0 / 768.0) < 0.01, "aspect kept in \(view)")
            let p = g.point(forNormalized: finder)
            let back = try #require(g.normalized(for: p))
            #expect(abs(back.x - finder.x) < 0.002 && abs(back.y - finder.y) < 0.002, "\(view)")
            if rect.minX > 1 { #expect(g.normalized(for: CGPoint(x: rect.minX / 2, y: view.height / 2)) == nil) }
            if rect.minY > 1 { #expect(g.normalized(for: CGPoint(x: view.width / 2, y: rect.minY / 2)) == nil) }
        }
    }

    /// Printable keys go as committed text once, not also as a key (which
    /// typed every character twice); chords and named keys go as keys.
    @Test func printableKeysAreCommittedOnceChordsAreKeys() {
        let down = InputEncoder.keyEvents(name: "a", characters: "a", modifiers: [], down: true, isRepeat: false)
        #expect(down.count == 1)
        if case let .textCommit(t) = down.first { #expect(t == "a") } else { Issue.record("\(down)") }
        #expect(InputEncoder.keyEvents(name: "a", characters: "a", modifiers: [], down: false, isRepeat: false).isEmpty)

        let chord = InputEncoder.keyEvents(name: "w", characters: "w", modifiers: [.command], down: true, isRepeat: false)
        #expect(chord.count == 1)
        if case let .key(key, isDown, modifiers, _) = chord.first {
            #expect(key == "w" && isDown && modifiers == [.command])
        } else { Issue.record("\(chord)") }

        let enter = InputEncoder.keyEvents(name: "enter", characters: "\r", modifiers: [], down: true, isRepeat: false)
        if case let .key(key, _, _, _) = enter.first { #expect(key == "enter") } else { Issue.record("\(enter)") }
    }

    /// The stream view is the hit-test target inside the app's grouped Form
    /// (no SwiftUI container swallows the click) and takes the first mouse,
    /// so a click on an inactive window reaches the Space too.
    @Test func streamViewIsTheClickTargetInsideAGroupedForm() throws {
        let session = LiveStreamSession(provider: OfflineStreamSourceProvider())
        let root = Form {
            Section {
                LiveStreamView(session: session, isInteractive: true)
                    .frame(minHeight: 260)
                    .listRowInsets(EdgeInsets())
            }
            Section { LabeledContent("Host", value: "192.168.64.89") }
        }
        .formStyle(.grouped)
        let window = offscreenWindow(CGSize(width: 700, height: 600))
        let host = NSHostingView(rootView: root)
        host.frame = CGRect(x: 0, y: 0, width: 700, height: 600)
        window.contentView = host
        host.layoutSubtreeIfNeeded()
        defer { window.close() }

        func find(_ v: NSView) -> LiveStreamInputView? {
            if let s = v as? LiveStreamInputView { return s }
            for sub in v.subviews { if let f = find(sub) { return f } }
            return nil
        }
        let stream = try #require(find(host), "the stream view is in the hierarchy")
        #expect(stream.bounds.width > 100 && stream.bounds.height > 100, "\(stream.bounds)")
        let center = stream.convert(CGPoint(x: stream.bounds.midX, y: stream.bounds.midY), to: nil)
        // hitTest takes a point in the receiver's superview's coordinates.
        let superview = try #require(host.superview)
        let hit = host.hitTest(superview.convert(center, from: nil))
        #expect(hit === stream || hit?.isDescendant(of: stream) == true,
                "the click lands on \(String(describing: hit)), not the stream")
        #expect(stream.acceptsFirstMouse(for: mouse(.leftMouseDown, at: center, in: window)))
        #expect(stream.acceptsFirstResponder)
    }

    /// A click on the Dock's Finder icon, through the real view and session:
    /// one move, a down and an up at the icon's normalized point, sent as
    /// `interactive_input` on the media session.
    @Test func aClickInTheViewIsSentAtTheGuestPoint() async throws {
        let provider = RecordingProvider()
        let session = LiveStreamSession(provider: provider)
        await session.start()
        #expect(session.status == .streaming)
        #expect(provider.sources == [.desktop])

        let window = offscreenWindow(CGSize(width: 800, height: 700))
        let view = LiveStreamInputView(frame: CGRect(x: 0, y: 0, width: 800, height: 700))
        window.contentView = view
        defer { window.close() }
        view.surfaceSize = CGSize(width: 1024, height: 768)
        view.onInput = { [weak session] in session?.send($0) }
        // The icon's point in the view (top-left origin), then in the window.
        let inView = view.geometry.point(forNormalized: CGPoint(x: 60.0 / 1024.0, y: 735.0 / 768.0))
        let inWindow = view.convert(inView, to: nil)
        view.mouseDown(with: mouse(.leftMouseDown, at: inWindow, in: window))
        view.mouseUp(with: mouse(.leftMouseUp, at: inWindow, in: window))

        let pointers = provider.media.sent.flatMap(pointerEvents)
        #expect(pointers.map { $0["phase"] as? String } == ["move", "down", "up"], "\(pointers)")
        for p in pointers {
            let x = try #require(p["x_normalized"] as? Double)
            let y = try #require(p["y_normalized"] as? Double)
            #expect(abs(x - 60.0 / 1024.0) < 0.003 && abs(y - 735.0 / 768.0) < 0.003, "\(p)")
        }
        #expect(session.inputEventsSent == 3)

        // A refused batch is shown, not swallowed; a delivered one clears it.
        let sink = try #require(provider.frames)
        sink.onEvent(event: MediaEvent(kind: "interactive_input_acknowledgement", json: """
            {"type":"interactive_input_acknowledgement","payload":{"session_id":"media-1","through_sequence":3,
             "delivered":false,"error":{"code":"unsupported","message":"not on this display"}}}
            """))
        await eventually("the refusal") { session.inputFailure != nil }
        #expect(session.inputFailure == "not on this display")
        #expect(session.inputEventsAcknowledged == 3)
        #expect(session.status == .streaming, "a refused click does not stop the video")
        sink.onEvent(event: MediaEvent(kind: "interactive_input_acknowledgement", json: """
            {"type":"interactive_input_acknowledgement","payload":{"session_id":"media-1","through_sequence":4,"delivered":true}}
            """))
        await eventually("the refusal to clear") { session.inputFailure == nil }
        await session.stop()
    }

    /// A window the Space can reach only by activating it (Hyprland outside
    /// its qualified apps) refuses the background session as
    /// would-require-activation. The viewer reopens that window with
    /// `allow_activation` and resends the refused click, instead of showing
    /// "Input not delivered" for every click.
    @Test func aWindowThatNeedsActivationIsReopenedActivatingAndTheClickResent() async throws {
        let provider = RecordingProvider()
        let session = LiveStreamSession(provider: provider)
        let window = StreamWindow(id: "w-1", app: "foot", title: "term")
        await session.select(.window(window))
        #expect(provider.policies == ["background_only"])
        let click: [InteractiveInputEvent] = [
            .pointer(phase: .down, button: .left, x: 0.5, y: 0.5, modifiers: []),
            .pointer(phase: .up, button: .left, x: 0.5, y: 0.5, modifiers: []),
        ]
        session.send(click)
        let sink = try #require(provider.frames)
        sink.onEvent(event: MediaEvent(kind: "interactive_input_acknowledgement", json: """
            {"type":"interactive_input_acknowledgement","payload":{"session_id":"media-1","through_sequence":2,
             "delivered":false,"error":{"code":"would_require_activation","message":"needs activation"}}}
            """))
        await eventually("the activating reopen") { provider.policies.count == 2 }
        #expect(provider.policies == ["background_only", "allow_activation"])
        #expect(provider.sources == [.window(window), .window(window)])
        await eventually("the resent click") { provider.media.sent.count == 2 }
        #expect(provider.media.sent.flatMap(pointerEvents).map { $0["phase"] as? String }
                == ["down", "up", "down", "up"])
        #expect(session.activatesOnInput)
        #expect(session.inputFailure == nil, "the refusal that led to the reopen is not shown")

        // A refusal after the reopen is shown as any other.
        let reopened = try #require(provider.frames)
        reopened.onEvent(event: MediaEvent(kind: "interactive_input_acknowledgement", json: """
            {"type":"interactive_input_acknowledgement","payload":{"session_id":"media-1","through_sequence":2,
             "delivered":false,"error":{"code":"would_require_activation","message":"still refused"}}}
            """))
        await eventually("the refusal") { session.inputFailure != nil }
        #expect(provider.policies.count == 2, "reopened once")

        // A desktop stream never reopens: it already activates.
        await session.select(.desktop)
        #expect(!session.activatesOnInput)
        #expect(provider.policies.last == "allow_activation")
        await session.stop()
    }
}
#endif
