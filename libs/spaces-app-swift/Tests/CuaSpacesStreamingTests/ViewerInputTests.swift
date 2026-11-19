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

    /// Whether the Space's clipboard setting lets a paste send the clipboard.
    var sendsClipboard = true
    /// How long the clipboard takes to reach the Space.
    var clipboardDelay: Duration = .zero
    private var clipboardTexts: [String] = []
    private var framesBefore: [Int] = []
    /// The clipboard texts the Space received, in order.
    var clipboards: [String] { lock.lock(); defer { lock.unlock() }; return clipboardTexts }
    /// How many input frames the Space had when each clipboard arrived.
    var framesAtClipboard: [Int] { lock.lock(); defer { lock.unlock() }; return framesBefore }

    /// A longer delay for one text: a transfer that stalls.
    var clipboardDelays: [String: Duration] = [:]

    private var startedTexts: [String] = []
    /// The clipboard texts whose transfer started, in order.
    var clipboardsStarted: [String] { lock.lock(); defer { lock.unlock() }; return startedTexts }
    /// Delays for successive transfers of one text, used up in order.
    var clipboardDelaySequence: [String: [Duration]] = [:]

    func sendClipboard(_ text: String) async throws -> Bool {
        lock.lock()
        startedTexts.append(text)
        let next = clipboardDelaySequence[text]?.isEmpty == false ? clipboardDelaySequence[text]?.removeFirst() : nil
        lock.unlock()
        try? await Task.sleep(for: next ?? clipboardDelays[text] ?? clipboardDelay)
        guard sendsClipboard else { return false }
        lock.lock(); clipboardTexts.append(text); framesBefore.append(media.sent.count); lock.unlock()
        return true
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

/// A key event for `window`, handed to the view directly.
@MainActor
private func key(_ characters: String, _ flags: NSEvent.ModifierFlags, keyCode: UInt16,
                 in window: NSWindow) -> NSEvent {
    NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: flags, timestamp: 0,
                     windowNumber: window.windowNumber, context: nil, characters: characters,
                     charactersIgnoringModifiers: characters, isARepeat: false, keyCode: keyCode)!
}

/// The key and text events of each input frame the Space received, as
/// `key` names and `text:` commits.
private func typed(in texts: [String]) -> [[String]] {
    texts.map { text in
        guard let root = try? JSONSerialization.jsonObject(with: Data(text.utf8)) as? [String: Any],
              let message = root["message"] as? [String: Any],
              let payload = message["payload"] as? [String: Any],
              let events = payload["events"] as? [[String: Any]] else { return [] }
        return events.compactMap { e in
            if let key = e["key"] as? String { return key }
            if let commit = e["text"] as? String { return "text:\(commit)" }
            return nil
        }
    }
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

    /// A Linux or Windows guest gets ⌘ chords as Control chords (Command
    /// would arrive as Super); a macOS or unknown guest keeps ⌘.
    @Test func commandGoesAsControlToALinuxGuest() {
        #expect(InputEncoder.commandAsControl(guestOS: "linux"))
        #expect(InputEncoder.commandAsControl(guestOS: "windows"))
        #expect(!InputEncoder.commandAsControl(guestOS: "macos"))
        #expect(!InputEncoder.commandAsControl(guestOS: ""))
        let mapped = InputEncoder.commandAsControl([
            .key(key: "c", down: true, modifiers: [.command, .shift], repeatKey: false),
            .key(key: "v", down: true, modifiers: [.command, .control], repeatKey: false),
        ])
        guard case let .key(_, _, copy, _) = mapped[0], case let .key(_, _, paste, _) = mapped[1] else {
            Issue.record("\(mapped)"); return
        }
        #expect(copy == [.control, .shift])
        #expect(paste == [.control])
        #expect(OfflineStreamSourceProvider().commandAsControl == false)
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

    /// A window the Space can reach only by activating it (an XWayland
    /// window on Hyprland) refuses the background session as
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

    /// #7589: ⌘V in the viewer of a Linux Space reached it as Control+V, but
    /// nothing put this Mac's clipboard on the Space, so the paste inserted
    /// nothing. The clipboard goes first; a key typed right after waits
    /// behind the paste.
    @Test func pasteSendsTheClipboardBeforeTheChord() async throws {
        let provider = RecordingProvider()
        provider.clipboardDelay = .milliseconds(50)
        let session = LiveStreamSession(provider: provider)
        await session.start()
        let window = offscreenWindow(CGSize(width: 400, height: 300))
        let view = LiveStreamInputView(frame: CGRect(x: 0, y: 0, width: 400, height: 300))
        window.contentView = view
        defer { window.close() }
        view.onInput = { [weak session] in session?.send($0) }
        view.onPaste = { [weak session] in session?.paste($0, text: $1) }
        let pasteboard = NSPasteboard.withUniqueName()
        defer { pasteboard.releaseGlobally() }
        pasteboard.clearContents()
        pasteboard.setString("copied on the Mac", forType: .string)
        view.pasteboard = pasteboard
        #expect(window.makeFirstResponder(view))

        #expect(view.performKeyEquivalent(with: key("v", .command, keyCode: 9, in: window)))
        view.keyDown(with: key("x", [], keyCode: 7, in: window))
        await eventually("the paste and the key") { provider.media.sent.count == 2 }
        #expect(provider.clipboards == ["copied on the Mac"])
        #expect(provider.framesAtClipboard == [0], "the clipboard reached the Space before any key")
        #expect(typed(in: provider.media.sent) == [["v", "v"], ["text:x"]])
        await session.stop()
    }

    /// A Mac clipboard with no text still goes first, as empty text, so the
    /// paste inserts nothing instead of what the Space copied before.
    @Test func anEmptyClipboardStillGoesBeforeTheChord() async throws {
        let provider = RecordingProvider()
        let session = LiveStreamSession(provider: provider)
        await session.start()
        let window = offscreenWindow(CGSize(width: 400, height: 300))
        let view = LiveStreamInputView(frame: CGRect(x: 0, y: 0, width: 400, height: 300))
        window.contentView = view
        defer { window.close() }
        view.onPaste = { [weak session] in session?.paste($0, text: $1) }
        let pasteboard = NSPasteboard.withUniqueName()
        defer { pasteboard.releaseGlobally() }
        pasteboard.clearContents()
        view.pasteboard = pasteboard
        #expect(window.makeFirstResponder(view))

        #expect(view.performKeyEquivalent(with: key("v", .command, keyCode: 9, in: window)))
        await eventually("the paste") { provider.media.sent.count == 1 }
        #expect(provider.clipboards == [""])
        #expect(provider.framesAtClipboard == [0], "the empty clipboard reached the Space before the chord")
        await session.stop()
    }

    /// A second paste while the first's clipboard is on its way pastes its
    /// own text, not the first's; held input dies with the stream it was
    /// typed into.
    @Test func pastesQueueWithTheirOwnTextAndAStopDropsThem() async throws {
        let v: [InteractiveInputEvent] = [.key(key: "v", down: true, modifiers: [.command], repeatKey: false)]
        let provider = RecordingProvider()
        provider.clipboardDelay = .milliseconds(30)
        let session = LiveStreamSession(provider: provider)
        await session.start()
        session.paste(v, text: "first")
        session.paste(v, text: "second")
        await eventually("both pastes") { provider.media.sent.count == 2 }
        #expect(provider.clipboards == ["first", "second"])
        #expect(provider.framesAtClipboard == [0, 1], "each clipboard went just before its own chord")

        // The first transfer stalls past its wait: its chord goes without
        // it, the second paste's chord still waits for its own text, and
        // the late first text is put right after it lands.
        let stalls = RecordingProvider()
        stalls.clipboardDelays = ["stalled": .milliseconds(300)]
        let late = LiveStreamSession(provider: stalls)
        late.pasteWait = .milliseconds(20)
        await late.start()
        late.paste(v, text: "stalled")
        await eventually("the first chord, after its wait") { stalls.media.sent.count == 1 }
        late.paste(v, text: "next")
        await eventually("the second chord") { stalls.media.sent.count == 2 }
        #expect(stalls.clipboards == ["next"])
        #expect(stalls.framesAtClipboard == [1], "the second clipboard went before its chord")
        await eventually("the late text and the newest again") { stalls.clipboards.count == 3 }
        #expect(stalls.clipboards == ["next", "stalled", "next"])
        await late.stop()

        // The repair can stall too, past a third paste: it is a transfer
        // like any other, so its late landing is repaired in turn.
        let repairs = RecordingProvider()
        repairs.clipboardDelays = ["stalled": .milliseconds(100)]
        repairs.clipboardDelaySequence = ["next": [.zero, .milliseconds(400)]]
        let third = LiveStreamSession(provider: repairs)
        third.pasteWait = .milliseconds(20)
        await third.start()
        third.paste(v, text: "stalled")
        await eventually("the first chord") { repairs.media.sent.count == 1 }
        third.paste(v, text: "next")
        await eventually("the repair starting") { repairs.clipboardsStarted == ["stalled", "next", "next"] }
        third.paste(v, text: "third")
        await eventually("the third chord") { repairs.media.sent.count == 3 }
        await eventually("the late repair and the newest again") { repairs.clipboards.count == 5 }
        #expect(repairs.clipboards == ["next", "stalled", "third", "next", "third"])
        await third.stop()

        let slow = RecordingProvider()
        slow.clipboardDelay = .seconds(30)
        let stopped = LiveStreamSession(provider: slow)
        stopped.pasteWait = .milliseconds(50)
        await stopped.start()
        stopped.paste(v, text: "late")
        stopped.send(v)
        await stopped.stop()
        await stopped.start()
        try await Task.sleep(for: .milliseconds(200))
        #expect(slow.media.sent.isEmpty, "nothing held for the stopped stream reached the new one")
        await stopped.stop()
        await session.stop()
    }

    /// Under `ask` or `off` (or with no clipboard behind the provider)
    /// nothing is sent, and a clipboard that never arrives does not keep
    /// the keys: the chord goes after `pasteWait`.
    @Test func aPasteTheSpaceDoesNotTakeStillSendsTheChord() async throws {
        #expect(SpaceStreamProvider.pasteSendsClipboard(setting: "viewing"))
        #expect(SpaceStreamProvider.pasteSendsClipboard(setting: ""))
        #expect(!SpaceStreamProvider.pasteSendsClipboard(setting: "ask"))
        #expect(!SpaceStreamProvider.pasteSendsClipboard(setting: "off"))
        #expect(LiveStreamInputView.isPasteChord(characters: "v", flags: [.command, .shift]))
        #expect(!LiveStreamInputView.isPasteChord(characters: "v", flags: [.command, .option]))
        #expect(!LiveStreamInputView.isPasteChord(characters: "c", flags: .command))

        let provider = RecordingProvider()
        provider.sendsClipboard = false
        let session = LiveStreamSession(provider: provider)
        await session.start()
        session.paste([.key(key: "v", down: true, modifiers: [.command], repeatKey: false)], text: "a")
        await eventually("the chord") { provider.media.sent.count == 1 }
        #expect(provider.clipboards.isEmpty)

        provider.sendsClipboard = true
        provider.clipboardDelay = .seconds(30)
        session.pasteWait = .milliseconds(20)
        session.paste([.key(key: "v", down: true, modifiers: [.command], repeatKey: false)], text: "b")
        await eventually("the chord without the clipboard") { provider.media.sent.count == 2 }
        await session.stop()
    }

    /// A Command chord typed while the stream has the keyboard goes to the
    /// Space (Super on a Linux guest) before the app's own shortcuts: here a
    /// SwiftUI ⌘N in the same window, which used to take it (#4609).
    @Test func commandChordReachesTheSpaceBeforeTheAppsShortcuts() throws {
        _ = NSApplication.shared
        var shortcutFired = false
        let session = LiveStreamSession(provider: OfflineStreamSourceProvider())
        let root = VStack {
            Button("New Space") { shortcutFired = true }.keyboardShortcut("n", modifiers: .command)
            LiveStreamView(session: session, isInteractive: true).frame(width: 400, height: 300)
        }
        let window = NSWindow(contentRect: CGRect(x: 0, y: 0, width: 500, height: 400),
                              styleMask: [.titled], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        let host = NSHostingView(rootView: root)
        host.frame = CGRect(x: 0, y: 0, width: 500, height: 400)
        window.contentView = host
        host.layoutSubtreeIfNeeded()
        defer { window.contentView = nil; window.close() }

        func find(_ v: NSView) -> LiveStreamInputView? {
            if let s = v as? LiveStreamInputView { return s }
            for sub in v.subviews { if let f = find(sub) { return f } }
            return nil
        }
        let stream = try #require(find(host))
        var keys: [String] = []
        stream.onInput = { events in
            for case let .key(key, down, modifiers, _) in events {
                keys.append("\(modifiers.map(\.rawValue).joined(separator: "+"))+\(key) \(down ? "down" : "up")")
            }
        }
        #expect(window.makeFirstResponder(stream))
        NSApp.sendEvent(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: .command, timestamp: 0,
                                         windowNumber: window.windowNumber, context: nil, characters: "n",
                                         charactersIgnoringModifiers: "n", isARepeat: false, keyCode: 45)!)
        #expect(keys == ["command+n down", "command+n up"], "the chord reached the Space")
        #expect(!shortcutFired, "the app's ⌘N did not run")
    }
}
#endif
