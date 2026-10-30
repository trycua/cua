// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
@testable import CuaSpacesMacKit
import CuaSpacesStreaming
import Foundation
import Testing
import WebKit

/// Native video in the web UI: the page's slot messages, the rect to view
/// frame mapping (clip, zoom, flip), occlusion, the stream lifecycle per
/// Space and tier, the fallback when a stream can't open, keyboard focus,
/// and the synthetic harness's H.264 splitting.
@Suite("Web UI native video")
@MainActor
struct WebUIVideoSurfacesTests {
    // MARK: - Messages

    @Test func readsSurfaceAndFocusMessages() {
        let body: [String: Any] = [
            "type": "surfaces",
            "update": [
                ["surfaceId": "a#tile#1", "spaceId": "a", "tier": "tile", "interactive": false,
                 "rect": ["x": 10, "y": 20, "width": 232, "height": 145],
                 "clip": ["x": 10, "y": 40, "width": 232, "height": 125],
                 "radius": 8, "occluded": true, "visible": true],
                // Malformed: no rect, an empty id.
                ["surfaceId": "b", "spaceId": "b"],
                ["surfaceId": "", "spaceId": "c", "rect": ["x": 0, "y": 0, "width": 1, "height": 1]],
            ],
            "remove": ["old", 3],
        ]
        guard case let .surfaces(update, remove)? = VideoSurfaceMessage(body: body) else {
            Issue.record("not a surfaces message")
            return
        }
        #expect(remove == ["old"])
        #expect(update == [VideoSurfaceUpdate(surfaceId: "a#tile#1", spaceId: "a", tier: .tile, interactive: false,
                                              rect: CGRect(x: 10, y: 20, width: 232, height: 145),
                                              clip: CGRect(x: 10, y: 40, width: 232, height: 125),
                                              radius: 8, occluded: true, visible: true)])
        #expect(VideoSurfaceMessage(body: ["type": "focus", "surfaceId": "x"]) == .focus("x"))
        #expect(VideoSurfaceMessage(body: ["type": "focus", "surfaceId": NSNull()]) == .focus(nil))
        #expect(VideoSurfaceMessage(body: ["type": "surface"]) == nil)
        #expect(VideoSurfaceMessage(body: "nope") == nil)
        // A negative or non-finite size is no rect.
        #expect(VideoSurfaceUpdate.rect(["x": 0, "y": 0, "width": -1, "height": 4]) == nil)
    }

    // MARK: - Layout

    private func update(rect: CGRect, clip: CGRect?, radius: CGFloat = 12, occluded: Bool = false,
                        visible: Bool = true) -> VideoSurfaceUpdate {
        VideoSurfaceUpdate(surfaceId: "s", spaceId: "a", tier: .full, interactive: true, rect: rect, clip: clip,
                           radius: radius, occluded: occluded, visible: visible)
    }

    @Test func mapsAFullyVisibleSlotOntoAFlippedWebView() {
        let r = CGRect(x: 265, y: 245, width: 592, height: 286)
        let l = VideoSurfaceLayout.make(update(rect: r, clip: r), zoom: 1, viewHeight: 700, flipped: true)
        #expect(!l.hidden)
        #expect(l.container == r)
        #expect(l.video == CGRect(x: 0, y: 0, width: 592, height: 286))
        #expect(l.radius == 12)
    }

    @Test func clipsASlotHalfUnderItsScrollContainer() {
        // The tile's top 30 px are scrolled under the container's top edge.
        let r = CGRect(x: 100, y: 10, width: 300, height: 200)
        let clip = CGRect(x: 100, y: 40, width: 300, height: 170)
        let l = VideoSurfaceLayout.make(update(rect: r, clip: clip), zoom: 1, viewHeight: 800, flipped: true)
        #expect(l.container == clip)
        // The video keeps its full size, shifted up inside the clip.
        #expect(l.video == CGRect(x: 0, y: -30, width: 300, height: 200))
    }

    @Test func countsYFromTheBottomOnAViewThatIsNotFlipped() {
        let r = CGRect(x: 10, y: 100, width: 200, height: 100)
        let l = VideoSurfaceLayout.make(update(rect: r, clip: r), zoom: 1, viewHeight: 700, flipped: false)
        #expect(l.container == CGRect(x: 10, y: 500, width: 200, height: 100))
    }

    @Test func scalesCSSPixelsByThePageZoom() {
        let r = CGRect(x: 10, y: 20, width: 200, height: 100)
        let l = VideoSurfaceLayout.make(update(rect: r, clip: r, radius: 8), zoom: 1.5, viewHeight: 900, flipped: true)
        #expect(l.container == CGRect(x: 15, y: 30, width: 300, height: 150))
        #expect(l.radius == 12)
        // A nonsense zoom is 1.
        #expect(VideoSurfaceLayout.make(update(rect: r, clip: r), zoom: 0, viewHeight: 900, flipped: true).container == r)
    }

    @Test func hidesASlotThatIsOccludedOffScreenOrEmpty() {
        let r = CGRect(x: 0, y: 0, width: 200, height: 100)
        let make = { (u: VideoSurfaceUpdate) in VideoSurfaceLayout.make(u, zoom: 1, viewHeight: 500, flipped: true) }
        #expect(make(update(rect: r, clip: r, occluded: true)).hidden)
        #expect(make(update(rect: r, clip: r, visible: false)).hidden)
        #expect(make(update(rect: r, clip: nil)).hidden)
        #expect(make(update(rect: CGRect(x: 0, y: 0, width: 0, height: 100), clip: r)).hidden)
        // A clip that misses the rect.
        #expect(make(update(rect: r, clip: CGRect(x: 300, y: 0, width: 10, height: 10))).hidden)
        #expect(!make(update(rect: r, clip: r)).hidden)
    }

    @Test func tilesAskForALowRateAndSizeAndTheViewerForFull() {
        #expect(VideoTier.tile.maxFPS == 10)
        #expect(VideoTier.tile.maxDimension == 960)
        #expect(VideoTier.full.maxFPS == 0)
        #expect(VideoTier.full.maxDimension == 0)
    }

    @Test func theExperimentFlagReadsTheEnvironmentAndDefaults() throws {
        let suite = "webui-video-\(UUID().uuidString)"
        let defaults = try #require(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        // On by default with the New UI.
        #expect(WebUIVideoSurfaces.enabled(environment: [:], defaults: defaults))
        // `=0` turns it off whatever the default says.
        #expect(!WebUIVideoSurfaces.enabled(environment: ["CUA_WEBUI_NATIVE_VIDEO": "0"], defaults: defaults))
        // An explicit false default opts out; `=1` still turns it on for a run.
        defaults.set(false, forKey: WebUIVideoSurfaces.defaultsKey)
        #expect(!WebUIVideoSurfaces.enabled(environment: [:], defaults: defaults))
        #expect(WebUIVideoSurfaces.enabled(environment: ["CUA_WEBUI_NATIVE_VIDEO": "1"], defaults: defaults))
        defaults.set(true, forKey: WebUIVideoSurfaces.defaultsKey)
        #expect(WebUIVideoSurfaces.enabled(environment: [:], defaults: defaults))
    }

    // MARK: - Surfaces

    /// A web view in a window (focus needs one) and the events the page would hear.
    final class Host {
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 900, height: 700), styleMask: [.titled],
                              backing: .buffered, defer: true)
        let webView = WKWebView(frame: NSRect(x: 0, y: 0, width: 900, height: 700))
        var events: [(String, [String: Any])] = []
        var opened: [(String, VideoTier)] = []

        init() { window.contentView = webView }

        func phases(_ id: String) -> [String] {
            events.filter { $0.0 == "video.surface" && $0.1["surfaceId"] as? String == id }.compactMap { $0.1["state"] as? String }
        }
    }

    private func surfaces(_ host: Host, open: @escaping WebUIVideoSurfaces.Opener) -> WebUIVideoSurfaces {
        let s = WebUIVideoSurfaces(webView: host.webView) { id, tier in
            await MainActor.run { host.opened.append((id, tier)) }
            return try await open(id, tier)
        }
        s.emit = { event, payload in host.events.append((event, payload)) }
        return s
    }

    private func message(_ entries: [[String: Any]], remove: [String] = []) -> [String: Any] {
        ["type": "surfaces", "update": entries, "remove": remove]
    }

    private func entry(_ id: String, space: String, tier: String = "full", interactive: Bool = true,
                       rect: [String: Any] = ["x": 20, "y": 30, "width": 400, "height": 250],
                       occluded: Bool = false) -> [String: Any] {
        ["surfaceId": id, "spaceId": space, "tier": tier, "interactive": interactive, "rect": rect, "clip": rect,
         "radius": 11, "occluded": occluded, "visible": true]
    }

    private func settle(tries: Int = 150, _ until: () -> Bool) async {
        for _ in 0..<tries where !until() { try? await Task.sleep(for: .milliseconds(20)) }
    }

    @Test func aStreamThatCannotOpenFallsBackAndShowsNothing() async {
        let host = Host()
        let s = surfaces(host) { _, _ in throw StreamError.noStream("no stream for this Space") }
        s.handle(message([entry("v", space: "a")]))
        await settle { host.phases("v").contains("failed") }
        #expect(host.phases("v") == ["failed"])
        let payload = host.events.first { $0.0 == "video.surface" }?.1
        #expect((payload?["reason"] as? String)?.contains("no stream") == true)
        // No stream was ever opened: the page says why once (`opening`).
        #expect(payload?["opening"] as? Bool == true)
        #expect(VideoSurfacePhase.failed("ended").payload["opening"] == nil)
        #expect(s.surfaces["v"]?.view.isHidden == true)
        // A second slot of the same Space hears it at once, without another open.
        s.handle(message([entry("t", space: "a")]))
        #expect(host.phases("t") == ["failed"])
        #expect(host.opened.count == 1)
    }

    @Test func slotsShareAStreamPerSpaceAndTierAndStopItWhenTheLastGoes() async {
        let host = Host()
        let s = surfaces(host) { _, _ in OfflineStreamSourceProvider() }
        s.handle(message([entry("t1", space: "a", tier: "tile", interactive: false),
                          entry("t2", space: "a", tier: "tile", interactive: false),
                          entry("v", space: "a")]))
        await settle { host.opened.count == 2 }
        #expect(host.opened.map(\.1).sorted { $0.rawValue < $1.rawValue } == [.full, .tile])
        #expect(Set(s.stats().map { $0["users"] as? Int }) == [1, 2])
        // Placed over the web view, and hidden until a frame draws.
        let tile = try? #require(s.surfaces["t1"])
        #expect(tile?.view.superview === host.webView)
        #expect(tile?.view.frame == CGRect(x: 20, y: 30, width: 400, height: 250))
        #expect(tile?.view.isHidden == true)
        s.handle(message([], remove: ["t1", "t2"]))
        #expect(s.stats().count == 1)
        #expect(tile?.view.superview == nil)
        s.removeAll()
        #expect(s.surfaces.isEmpty)
        #expect(s.stats().isEmpty)
    }

    @Test func aTileTakesNoInputAndTheViewerTakesPointerInput() {
        let host = Host()
        let s = surfaces(host) { _, _ in OfflineStreamSourceProvider() }
        s.handle(message([entry("t", space: "a", tier: "tile", interactive: false), entry("v", space: "b")]))
        let tile = s.surfaces["t"]!.view, viewer = s.surfaces["v"]!.view
        #expect(!tile.interactive && !tile.input.isInteractive)
        #expect(viewer.interactive && viewer.input.isInteractive)
        // A tile is never hit: clicks and scrolls reach the page.
        tile.isHidden = false
        #expect(tile.hitTest(NSPoint(x: 100, y: 100)) == nil)
        // A viewer that isn't showing isn't either.
        #expect(viewer.hitTest(NSPoint(x: 100, y: 100)) == nil)
        viewer.isHidden = false
        #expect(viewer.hitTest(NSPoint(x: 100, y: 100)) === viewer.input)
    }

    /// 64×48, an IDR (with SPS/PPS) then a P frame.
    static let tinyH264 = Data(base64Encoded: """
    AAAAAWdCwAraEewEQAAAAwBAAAAFA8SJqAAAAAFozg/IAAABBgX//03cRem95tlIt5Ys2CDZI+7veDI2NCAtIGNvcmUgMTY1IHIzMjIyIGIzNTYwNWEgLSBILjI2NC9NUEVHLTQgQVZDIGNvZGVjIC0gQ29weWxlZnQgMjAwMy0yMDI1IC0gaHR0cDovL3d3dy52aWRlb2xhbi5vcmcveDI2NC5odG1sIC0gb3B0aW9uczogY2FiYWM9MCByZWY9MSBkZWJsb2NrPTA6MDowIGFuYWx5c2U9MDowIG1lPWRpYSBzdWJtZT0wIHBzeT0xIHBzeV9yZD0xLjAwOjAuMDAgbWl4ZWRfcmVmPTAgbWVfcmFuZ2U9MTYgY2hyb21hX21lPTEgdHJlbGxpcz0wIDh4OGRjdD0wIGNxbT0wIGRlYWR6b25lPTIxLDExIGZhc3RfcHNraXA9MSBjaHJvbWFfcXBfb2Zmc2V0PTAgdGhyZWFkcz0xIGxvb2thaGVhZF90aHJlYWRzPTEgc2xpY2VkX3RocmVhZHM9MCBucj0wIGRlY2ltYXRlPTEgaW50ZXJsYWNlZD0wIGJsdXJheV9jb21wYXQ9MCBjb25zdHJhaW5lZF9pbnRyYT0wIGJmcmFtZXM9MCB3ZWlnaHRwPTAga2V5aW50PTIga2V5aW50X21pbj0xIHNjZW5lY3V0PTAgaW50cmFfcmVmcmVzaD0wIHJjPWNyZiBtYnRyZWU9MCBjcmY9MjMuMCBxY29tcD0wLjYwIHFwbWluPTAgcXBtYXg9NjkgcXBzdGVwPTQgaXBfcmF0aW89MS40MCBhcT0wAIAAAAFliIQ6EYoAAg0xwABBejgACBTJycnXXXXXXXXgAAAAAUGaIBOhsA==
    """, options: .ignoreUnknownCharacters)!

    @Test func splitsAnAnnexBStreamIntoAccessUnits() {
        let units = AccessUnit.split(Self.tinyH264)
        #expect(units.count == 2)
        #expect(units.map(\.keyframe) == [true, false])
        #expect(units.map(\.data.count).reduce(0, +) == Self.tinyH264.count)
        #expect(AccessUnit.split(Data()).isEmpty)
    }

    @Test func aLiveStreamShowsTheViewerTakesTheKeyboardAndControlOptionGivesItBack() async {
        let host = Host()
        let units = AccessUnit.split(Self.tinyH264)
        let s = surfaces(host) { _, _ in SyntheticStreamProvider(units: units, size: CGSize(width: 64, height: 48), fps: 30) }
        s.handle(message([entry("v", space: "a")]))
        await settle { host.phases("v").contains("live") }
        #expect(host.phases("v").last == "live")
        let viewer = s.surfaces["v"]!.view
        #expect(!viewer.isHidden)

        // The page gives it the keyboard; the page hears it.
        s.handle(["type": "focus", "surfaceId": "v"])
        #expect(host.window.firstResponder === viewer.input)
        #expect(s.focusedId == "v")
        #expect(host.events.last?.0 == "video.focus")
        #expect(host.events.last?.1["surfaceId"] as? String == "v")

        // Control+Option pressed and released alone gives it back (the
        // view's KeyCapture, as in the Space window).
        releaseChord(viewer.input, in: host.window)
        #expect(s.focusedId == nil)
        #expect(host.window.firstResponder === host.webView)
        #expect(host.events.last?.1["surfaceId"] is NSNull)

        // Page UI over it: hidden, and it can't take the keyboard.
        s.handle(message([entry("v", space: "a", occluded: true)]))
        #expect(viewer.isHidden)
        s.handle(["type": "focus", "surfaceId": "v"])
        #expect(s.focusedId == nil)

        // Covered while it has the keyboard: the keyboard goes back to the page.
        s.handle(message([entry("v", space: "a")]))
        s.handle(["type": "focus", "surfaceId": "v"])
        #expect(s.focusedId == "v")
        s.handle(message([entry("v", space: "a", occluded: true)]))
        #expect(s.focusedId == nil)
        s.removeAll()
    }

    /// The video bench times each presented frame from its arrival
    /// (`decodeMs`, compared with the Electron app's arrival-to-drawn).
    @Test func theBenchTimesEachPresentedFrameFromItsArrival() async {
        let host = Host()
        let units = AccessUnit.split(Self.tinyH264)
        let s = surfaces(host) { _, _ in SyntheticStreamProvider(units: units, size: CGSize(width: 64, height: 48), fps: 30) }
        // Every frame reaches the layer 25 ms after the newest one arrived,
        // whatever the machine's load.
        var arrivals: [TimeInterval] = []
        s.now = { [weak s] in
            let arrival = s?.surfaces["v"].flatMap { s?.sessionFor($0) }?.lastFrameReceivedAt ?? 0
            arrivals.append(arrival)
            return arrival + 0.025
        }
        s.handle(message([entry("v", space: "a")]))
        await settle(tries: 1500) { (s.stats().first?["decodeMs"] as? [Double])?.isEmpty == false }
        let samples = s.stats().first?["decodeMs"] as? [Double] ?? []
        #expect(!samples.isEmpty)
        #expect(samples.allSatisfy { abs($0 - 25) < 1e-6 })
        // The arrival is the SDK's stamp on the uptime clock: set, and not
        // in the future.
        #expect(!arrivals.isEmpty)
        #expect(arrivals.allSatisfy { $0 > 0 && $0 <= ProcessInfo.processInfo.systemUptime })
        s.removeAll()
    }

    @Test func theBenchKeepsTheNewestSamples() {
        var samples: [Double] = []
        for i in 0..<(WebUIVideoSurfaces.latencySamples + 3) { WebUIVideoSurfaces.appendSample(&samples, Double(i)) }
        #expect(samples.count == WebUIVideoSurfaces.latencySamples)
        #expect(samples.first == 3)
    }

    /// After a reconnect the viewer that had the keyboard has
    /// it again, with no click; one that didn't, or whose keys the user
    /// moved meanwhile, does not.
    @Test func aViewerThatHadTheKeyboardGetsItBackAfterAReconnect() async throws {
        let host = Host()
        let units = AccessUnit.split(Self.tinyH264)
        let s = surfaces(host) { _, _ in SyntheticStreamProvider(units: units, size: CGSize(width: 64, height: 48), fps: 30) }
        s.handle(message([entry("v", space: "a")]))
        await settle { host.phases("v").last == "live" }
        let viewer = try #require(s.surfaces["v"]).view
        s.handle(["type": "focus", "surfaceId": "v"])
        #expect(host.window.firstResponder === viewer.input)
        let session = try #require(s.sessionFor(s.surfaces["v"]!))

        // The stream drops: no frame, the viewer hides, the page has the keys.
        await session.stop()
        await settle { host.phases("v").last == "connecting" }
        #expect(s.focusedId == nil)
        #expect(host.window.firstResponder !== viewer.input)
        // Back: the keyboard comes back by itself.
        await session.start()
        await settle { host.phases("v").last == "live" }
        await settle { s.focusedId == "v" }
        #expect(s.focusedId == "v")
        #expect(host.window.firstResponder === viewer.input)
        #expect(host.events.last { $0.0 == "video.focus" }?.1["surfaceId"] as? String == "v")

        // The page took the keys back before the drop: they stay with it.
        s.handle(["type": "focus", "surfaceId": NSNull()])
        await session.stop()
        await settle { host.phases("v").last == "connecting" }
        await session.start()
        await settle { host.phases("v").last == "live" }
        try? await Task.sleep(for: .milliseconds(100))
        #expect(s.focusedId == nil)

        // The page moved the keys during the drop: they stay where it put them.
        s.handle(["type": "focus", "surfaceId": "v"])
        await session.stop()
        await settle { host.phases("v").last == "connecting" }
        s.userMovedKeyboard()
        await session.start()
        await settle { host.phases("v").last == "live" }
        try? await Task.sleep(for: .milliseconds(100))
        #expect(s.focusedId == nil)
        s.removeAll()
    }

    // MARK: - Command chords

    private func chord(_ chars: String, code: UInt16, _ flags: NSEvent.ModifierFlags, in window: NSWindow) -> NSEvent {
        NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: flags, timestamp: 0,
                         windowNumber: window.windowNumber, context: nil, characters: chars,
                         charactersIgnoringModifiers: chars, isARepeat: false, keyCode: code)!
    }

    /// Control+Option pressed and released alone, through the view's key capture.
    private func releaseChord(_ input: LiveStreamInputView, in window: NSWindow) {
        for flags in [NSEvent.ModifierFlags.control, [.control, .option], .option, []] {
            let e = NSEvent.keyEvent(with: .flagsChanged, location: .zero, modifierFlags: flags, timestamp: 0,
                                     windowNumber: window.windowNumber, context: nil, characters: "",
                                     charactersIgnoringModifiers: "", isARepeat: false, keyCode: 59)!
            #expect(!input.capture(e), "modifier changes pass on")
        }
    }

    private func modifiers(_ event: InteractiveInputEvent) -> [InputModifier]? {
        guard case let .key(_, _, modifiers, _) = event else { return nil }
        return modifiers
    }

    @Test func commandChordsKeepTheirModifiersAndGoAsControlToALinuxOrWindowsGuest() {
        // The view's encoder keeps every flag of a chord.
        let flags = InputEncoder.modifiers(from: [.command, .shift])
        #expect(flags == [.command, .shift])
        let copy = InputEncoder.keyEvents(name: "c", characters: "c", modifiers: [.command], down: true, isRepeat: false)
        #expect(copy.count == 1)
        #expect(copy.first.flatMap(modifiers) == [.command])

        // Which guests take ⌘ as Control.
        #expect(InputEncoder.commandAsControl(guestOS: "linux"))
        #expect(InputEncoder.commandAsControl(guestOS: "Windows"))
        #expect(!InputEncoder.commandAsControl(guestOS: "macos"))
        #expect(!InputEncoder.commandAsControl(guestOS: "darwin"))
        #expect(!InputEncoder.commandAsControl(guestOS: ""))

        // ⌘C is Ctrl+C, ⌘⇧C is Ctrl+Shift+C, ⌃⌘C is one Control; the rest stays.
        let events: [InteractiveInputEvent] = [
            .key(key: "c", down: true, modifiers: [.command], repeatKey: false),
            .key(key: "c", down: true, modifiers: [.command, .shift], repeatKey: false),
            .key(key: "c", down: false, modifiers: [.control, .command, .option], repeatKey: false),
            .key(key: "enter", down: true, modifiers: [.shift], repeatKey: false),
            .textCommit("c"),
            .pointer(phase: .down, button: .left, x: 0.5, y: 0.5, modifiers: [.command]),
        ]
        let mapped = InputEncoder.commandAsControl(events)
        #expect(mapped.map(modifiers) == [[.control], [.control, .shift], [.control, .option], [.shift], nil, nil])
        guard case .textCommit("c") = mapped[4] else { Issue.record("text changed: \(mapped[4])"); return }
        guard case let .pointer(_, _, _, _, pointerModifiers) = mapped[5] else { Issue.record("pointer changed"); return }
        #expect(pointerModifiers == [.command])
        guard case let .key(key, down, _, _) = mapped[2] else { return }
        #expect(key == "c" && !down)
    }

    /// The viewer's key monitor sends ⌘ chords to the Space with the guest's
    /// modifiers (the same `LiveStreamSession.send` as the Space window), ⌘Esc
    /// too; Control+Option pressed and released alone gives the keyboard back
    /// without reaching the Space (#4609).
    @Test(arguments: [("linux", "control"), ("macos", "command")])
    func commandChordsReachTheSpaceWithTheGuestsModifiers(os: String, expected: String) async {
        let host = Host()
        let units = AccessUnit.split(Self.tinyH264)
        let provider = SyntheticStreamProvider(units: units, size: CGSize(width: 64, height: 48), fps: 30, guestOS: os)
        let s = surfaces(host) { _, _ in provider }
        s.handle(message([entry("v", space: "a")]))
        await settle { host.phases("v").contains("live") }
        s.handle(["type": "focus", "surfaceId": "v"])
        #expect(s.focusedId == "v")
        let sent = { provider.lastMedia?.sentInputs.flatMap(SyntheticMedia.events(in:)) ?? [] }
        let start = sent().count

        #expect(s.handleKey(chord("c", code: 8, .command, in: host.window)))
        await settle { sent().count >= start + 2 }
        let copy = Array(sent().dropFirst(start))
        #expect(copy.map { $0["key"] as? String } == ["c", "c"])
        #expect(copy.map { $0["state"] as? String } == ["down", "up"])
        #expect(copy.map { $0["modifiers"] as? [String] } == [[expected], [expected]])

        let start2 = sent().count
        #expect(s.handleKey(chord("c", code: 8, [.command, .shift], in: host.window)))
        await settle { sent().count >= start2 + 2 }
        #expect(sent().dropFirst(start2).first?["modifiers"] as? [String] == [expected, "shift"])

        // ⌘Esc is the Space's too.
        let start3 = sent().count
        #expect(s.handleKey(chord("\u{1b}", code: 53, .command, in: host.window)))
        await settle { sent().count >= start3 + 2 }
        #expect(sent().dropFirst(start3).map { $0["state"] as? String } == ["down", "up"])
        #expect(s.focusedId == "v")

        // Control+Option alone: the keyboard goes back to the page and nothing goes to the Space.
        let beforeRelease = provider.lastMedia?.sentInputs.count ?? 0
        releaseChord(s.surfaces["v"]!.view.input, in: host.window)
        #expect(s.focusedId == nil)
        try? await Task.sleep(for: .milliseconds(200))
        #expect(provider.lastMedia?.sentInputs.count == beforeRelease)
        // With the keyboard back on the page, ⌘C is the page's again.
        #expect(!s.handleKey(chord("c", code: 8, .command, in: host.window)))
        s.removeAll()
    }

    @Test func theWindowHiddenStopsEveryStreamUntilItShows() async {
        let host = Host()
        let units = AccessUnit.split(Self.tinyH264)
        let s = surfaces(host) { _, _ in SyntheticStreamProvider(units: units, size: CGSize(width: 64, height: 48), fps: 30) }
        s.handle(message([entry("v", space: "a")]))
        await settle { host.phases("v").contains("live") }
        s.setPaused(true)
        await settle { host.phases("v").last == "connecting" }
        #expect(host.phases("v").last == "connecting")
        #expect(s.surfaces["v"]?.view.isHidden == true)
        s.setPaused(false)
        await settle { host.phases("v").last == "live" }
        #expect(host.phases("v").last == "live")
        s.removeAll()
    }

    // MARK: - Picture in picture

    /// Regression: closing the desktop's picture in picture showed "Could
    /// not connect to the desktop", then "Connecting…", before the inline
    /// desktop came back: the panel opened a second stream, the inline one
    /// closed, and closing the panel closed that one and dialled again. The
    /// panel now holds the viewer's own session; the slot that comes back
    /// binds to it, live at once, with no second open and no failure.
    @Test func pictureInPictureHandsTheSameLiveSessionBackWithNoReopen() async throws {
        let host = Host()
        let units = AccessUnit.split(Self.tinyH264)
        let s = surfaces(host) { _, _ in SyntheticStreamProvider(units: units, size: CGSize(width: 64, height: 48), fps: 30) }
        // Only the panel's hold keeps the session here (no slot grace).
        s.slotGrace = .zero
        s.handle(message([entry("v1", space: "a")]))
        await settle { host.phases("v1").contains("live") }
        let first = try #require(s.surfaces["v1"])
        let inline = try #require(s.sessionFor(first))

        // Popped out: the panel holds the same session, and the page's slot goes.
        let held = try await s.holdViewer(spaceId: "a")
        #expect(held === inline)
        s.handle(message([], remove: ["v1"]))
        #expect(host.opened.count == 1)
        #expect(held.status == .streaming)
        // A hidden window pauses slots, never the panel's stream.
        s.setPaused(true)
        try? await Task.sleep(for: .milliseconds(100))
        #expect(held.status == .streaming)
        s.setPaused(false)

        // Closed: the slot that comes back is live at once, on the same session.
        s.releaseViewer(spaceId: "a", after: .milliseconds(200))
        s.handle(message([entry("v2", space: "a")]))
        // Its first word is live, on the next main-queue turn (the session's current frame).
        await settle(tries: 5) { !host.phases("v2").isEmpty }
        #expect(host.phases("v2") == ["live"])
        let back = try #require(s.surfaces["v2"])
        #expect(s.sessionFor(back) === inline)
        try? await Task.sleep(for: .milliseconds(400))
        #expect(host.opened.count == 1)
        #expect(!host.events.contains { $0.0 == "video.surface" && $0.1["state"] as? String == "failed" })
        #expect(s.stats().first?["users"] as? Int == 1)
        s.removeAll()
        #expect(s.stats().isEmpty)
    }

    @Test func aReleasedHoldWithNoSlotStopsTheStreamAfterTheGrace() async throws {
        let host = Host()
        let units = AccessUnit.split(Self.tinyH264)
        let s = surfaces(host) { _, _ in SyntheticStreamProvider(units: units, size: CGSize(width: 64, height: 48), fps: 30) }
        // No inline slot (auto-connect off): the panel opens the viewer stream itself.
        let held = try await s.holdViewer(spaceId: "a")
        #expect(host.opened.map(\.1) == [.full])
        #expect(s.stats().count == 1)
        s.releaseViewer(spaceId: "a", after: .zero)
        await settle { s.stats().isEmpty }
        #expect(s.stats().isEmpty)
        await settle { held.status == .idle }
        #expect(held.status == .idle)
    }

    @Test func aHoldOnAStreamThatCannotOpenThrowsAndKeepsNothing() async {
        let host = Host()
        let s = surfaces(host) { _, _ in throw StreamError.noStream("no stream for this Space") }
        await #expect(throws: (any Error).self) { try await s.holdViewer(spaceId: "a") }
        #expect(s.stats().isEmpty)
    }

    // MARK: - Moving the viewer (call view and thread view)

    /// The Space page moves its viewer between the call view's stage and
    /// thread view's mini-player: the old slot goes and a new one comes.
    /// The new slot binds to the same live session (no second open, no
    /// "Connecting…"), as long as it comes within `slotGrace`.
    @Test func aViewerMovedToANewSlotKeepsItsLiveSession() async throws {
        let host = Host()
        let units = AccessUnit.split(Self.tinyH264)
        let s = surfaces(host) { _, _ in SyntheticStreamProvider(units: units, size: CGSize(width: 64, height: 48), fps: 30) }
        // Shorter than the default, so the move back below does not wait it out.
        s.slotGrace = .milliseconds(300)
        s.handle(message([entry("call", space: "a")]))
        await settle { host.phases("call").contains("live") }
        let callSlot = try #require(s.surfaces["call"])
        let session = try #require(s.sessionFor(callSlot))

        // One message: the stage's slot goes, the mini-player's comes.
        var mini = entry("mini", space: "a", interactive: false)
        mini["rect"] = ["x": 500, "y": 400, "width": 320, "height": 200]
        s.handle(message([mini], remove: ["call"]))
        await settle(tries: 5) { !host.phases("mini").isEmpty }
        #expect(host.phases("mini") == ["live"])
        let miniSlot = try #require(s.surfaces["mini"])
        #expect(s.sessionFor(miniSlot) === session)
        // And back again, after the grace has passed for the first move.
        try? await Task.sleep(for: s.slotGrace + .milliseconds(200))
        s.handle(message([entry("call2", space: "a")], remove: ["mini"]))
        await settle(tries: 5) { !host.phases("call2").isEmpty }
        #expect(host.phases("call2") == ["live"])
        let back = try #require(s.surfaces["call2"])
        #expect(s.sessionFor(back) === session)
        #expect(host.opened.count == 1)
        #expect(session.status == .streaming)
        s.removeAll()
    }

    @Test func aViewerSlotThatDoesNotComeBackStopsItsSessionAfterTheGrace() async throws {
        let host = Host()
        let units = AccessUnit.split(Self.tinyH264)
        let s = surfaces(host) { _, _ in SyntheticStreamProvider(units: units, size: CGSize(width: 64, height: 48), fps: 30) }
        s.slotGrace = .milliseconds(150)
        s.handle(message([entry("v", space: "a")]))
        await settle { host.phases("v").contains("live") }
        let slot = try #require(s.surfaces["v"])
        let session = try #require(s.sessionFor(slot))
        s.handle(message([], remove: ["v"]))
        // Still there during the grace...
        #expect(s.stats().count == 1)
        // ...then gone, and the session stopped.
        await settle { s.stats().isEmpty }
        #expect(s.stats().isEmpty)
        await settle { session.status == .idle }
        #expect(session.status == .idle)
        // A page that goes away (removeAll) stops at once, and tiles never wait.
        s.handle(message([entry("t", space: "b", tier: "tile", interactive: false), entry("w", space: "c")]))
        await settle { host.opened.count == 3 }
        s.handle(message([], remove: ["t"]))
        #expect(s.stats().count == 1)
        s.removeAll()
        #expect(s.stats().isEmpty)
    }

    // MARK: - Coming back to a Space (warm sessions)

    /// Regression: leaving a Space for another page and returning sat on
    /// "Connecting…" for as long as a fresh open takes, because the viewer's
    /// session outlived its slot by only 1.5 s. Returning within the grace
    /// binds the new slot to the live session: live on its first word, no
    /// second open.
    @Test func comingBackToASpaceWithinTheGraceIsLiveAtOnceOnTheSameSession() async throws {
        let host = Host()
        let units = AccessUnit.split(Self.tinyH264)
        let s = surfaces(host) { _, _ in SyntheticStreamProvider(units: units, size: CGSize(width: 64, height: 48), fps: 30) }
        // The default is a minute or so, not the old 1.5 s.
        #expect(s.slotGrace >= .seconds(30) && s.slotGrace <= .seconds(120))
        s.slotGrace = .seconds(30)
        s.handle(message([entry("v1", space: "a")]))
        await settle { host.phases("v1").contains("live") }
        let first = try #require(s.surfaces["v1"])
        let session = try #require(s.sessionFor(first))

        // The page goes to another route: its slot is removed. Well past the old grace.
        s.handle(message([], remove: ["v1"]))
        try? await Task.sleep(for: .milliseconds(1800))
        #expect(s.stats().count == 1)
        #expect(session.status == .streaming)

        let returned = Date()
        s.handle(message([entry("v2", space: "a")]))
        await settle(tries: 5) { !host.phases("v2").isEmpty }
        #expect(host.phases("v2") == ["live"])
        // The timing guard: generous, it only catches a renegotiation creeping back in.
        #expect(Date().timeIntervalSince(returned) < 1.0)
        let slot2 = try #require(s.surfaces["v2"])
        #expect(s.sessionFor(slot2) === session)
        #expect(host.opened.count == 1)
        s.removeAll()
    }

    @Test func comingBackAfterTheGraceOpensAFreshStream() async throws {
        let host = Host()
        let units = AccessUnit.split(Self.tinyH264)
        let s = surfaces(host) { _, _ in SyntheticStreamProvider(units: units, size: CGSize(width: 64, height: 48), fps: 30) }
        s.slotGrace = .milliseconds(150)
        s.handle(message([entry("v1", space: "a")]))
        await settle { host.phases("v1").contains("live") }
        let oldSlot = try #require(s.surfaces["v1"])
        let old = try #require(s.sessionFor(oldSlot))
        s.handle(message([], remove: ["v1"]))
        await settle { s.stats().isEmpty }
        #expect(s.stats().isEmpty)
        s.handle(message([entry("v2", space: "a")]))
        await settle { host.phases("v2").contains("live") }
        #expect(host.opened.count == 2)
        let slot2 = try #require(s.surfaces["v2"])
        #expect(s.sessionFor(slot2) !== old)
        s.removeAll()
    }

    /// A warm session is never kept for a Space that stopped or a machine
    /// that went away: its transport drops, and it is released at once
    /// instead of reconnecting for nobody.
    @Test func aWarmSessionWhoseSpaceStoppedIsReleasedNotReconnected() async throws {
        let host = Host()
        let units = AccessUnit.split(Self.tinyH264)
        let provider = SyntheticStreamProvider(units: units, size: CGSize(width: 64, height: 48), fps: 30)
        let s = surfaces(host) { _, _ in provider }
        s.slotGrace = .seconds(30)
        s.handle(message([entry("v1", space: "a")]))
        await settle { host.phases("v1").contains("live") }
        let slot = try #require(s.surfaces["v1"])
        let session = try #require(s.sessionFor(slot))
        s.handle(message([], remove: ["v1"]))
        #expect(s.stats().count == 1)

        provider.lastMedia?.dropTransport()
        await settle { s.stats().isEmpty }
        #expect(s.stats().isEmpty)
        await settle { session.status == .idle }
        #expect(session.status == .idle)

        // Coming back opens again (and would say why if it could not).
        s.handle(message([entry("v2", space: "a")]))
        await settle { host.phases("v2").contains("live") }
        #expect(host.opened.count == 2)
        s.removeAll()
    }

    @Test func aSessionThatAlreadyDroppedIsNotKeptWarm() async throws {
        let host = Host()
        let units = AccessUnit.split(Self.tinyH264)
        let provider = SyntheticStreamProvider(units: units, size: CGSize(width: 64, height: 48), fps: 30)
        let s = surfaces(host) { _, _ in provider }
        s.slotGrace = .seconds(30)
        s.handle(message([entry("v1", space: "a")]))
        await settle { host.phases("v1").contains("live") }
        let slot = try #require(s.surfaces["v1"])
        let session = try #require(s.sessionFor(slot))
        provider.lastMedia?.dropTransport()
        await settle { session.status != .streaming }
        s.handle(message([], remove: ["v1"]))
        #expect(s.stats().isEmpty)
        s.removeAll()
    }

    @Test func aHiddenWindowAndAClosedWindowKeepNothingWarm() async throws {
        let host = Host()
        let units = AccessUnit.split(Self.tinyH264)
        let s = surfaces(host) { _, _ in SyntheticStreamProvider(units: units, size: CGSize(width: 64, height: 48), fps: 30) }
        s.slotGrace = .seconds(30)
        s.handle(message([entry("v1", space: "a")]))
        await settle { host.phases("v1").contains("live") }
        s.handle(message([], remove: ["v1"]))
        #expect(s.stats().count == 1)
        s.setPaused(true)
        #expect(s.stats().isEmpty)
        s.setPaused(false)

        // Closed window: every warm session goes.
        s.handle(message([entry("v2", space: "a")]))
        await settle { host.phases("v2").contains("live") }
        s.handle(message([], remove: ["v2"]))
        #expect(s.stats().count == 1)
        s.removeAll()
        #expect(s.stats().isEmpty)
    }

    @Test func atMostMaxWarmViewerSessionsStayWarmOldestFirst() async throws {
        let host = Host()
        let units = AccessUnit.split(Self.tinyH264)
        let s = surfaces(host) { _, _ in SyntheticStreamProvider(units: units, size: CGSize(width: 64, height: 48), fps: 30) }
        s.slotGrace = .seconds(30)
        s.maxWarm = 2
        for space in ["a", "b", "c"] {
            s.handle(message([entry("v-\(space)", space: space)]))
            await settle { host.phases("v-\(space)").contains("live") }
            s.handle(message([], remove: ["v-\(space)"]))
        }
        let warm = Set(s.stats().compactMap { $0["spaceId"] as? String })
        #expect(warm == ["b", "c"])
        s.removeAll()
    }

    // MARK: - Holes

    @Test func readsHolesAndMapsThemIntoTheClipContainer() throws {
        var u = update(rect: CGRect(x: 100, y: 100, width: 400, height: 300), clip: CGRect(x: 100, y: 120, width: 400, height: 280))
        u.holes = [CGRect(x: 420, y: 90, width: 160, height: 60), CGRect(x: 700, y: 0, width: 10, height: 10)]
        let l = VideoSurfaceLayout.make(u, zoom: 2, viewHeight: 1000, flipped: true)
        #expect(!l.hidden)
        // In the container (from 100,120, doubled), cut to it; the hole off the slot is dropped.
        #expect(l.holes == [CGRect(x: 640, y: 0, width: 160, height: 60)])
        #expect(l.inHole(CGPoint(x: 700, y: 10)))
        #expect(!l.inHole(CGPoint(x: 10, y: 10)))
        let mask = try #require(l.maskPath)
        #expect(!mask.contains(CGPoint(x: 700, y: 10)))
        #expect(mask.contains(CGPoint(x: 10, y: 10)))
        #expect(VideoSurfaceLayout.make(update(rect: u.rect, clip: u.clip), zoom: 1, viewHeight: 1000, flipped: true).maskPath == nil)

        let parsed = VideoSurfaceUpdate(json: ["surfaceId": "s", "spaceId": "a", "rect": ["x": 0, "y": 0, "width": 10, "height": 10],
                                               "holes": [["x": 1, "y": 2, "width": 3, "height": 4], "bad"]])
        #expect(parsed?.holes == [CGRect(x: 1, y: 2, width: 3, height: 4)])
    }

    /// Regression: the header's "⋯" menu turned the desktop black. A menu
    /// over the viewer is a hole now: the video stays shown and live, and
    /// clicks in the hole reach the page's menu.
    @Test func aMenuOverTheViewerCutsAHoleAndTheVideoStaysLive() async throws {
        let host = Host()
        let units = AccessUnit.split(Self.tinyH264)
        let s = surfaces(host) { _, _ in SyntheticStreamProvider(units: units, size: CGSize(width: 64, height: 48), fps: 30) }
        s.handle(message([entry("v", space: "a")]))
        await settle { host.phases("v").contains("live") }
        var withMenu = entry("v", space: "a")
        withMenu["holes"] = [["x": 300, "y": 30, "width": 160, "height": 40]]
        s.handle(message([withMenu]))
        let view = try #require(s.surfaces["v"]?.view)
        #expect(!view.isHidden)
        #expect(view.layer?.mask != nil)
        #expect(view.layout.inHole(CGPoint(x: 330, y: 10)))
        #expect(host.phases("v").last == "live")
        // Closed: no mask.
        s.handle(message([entry("v", space: "a")]))
        #expect(view.layer?.mask == nil)
        s.removeAll()
    }

    /// A modal dialog over the viewer (the page's dimmed layer is up): the page reports the
    /// viewer occluded and non-interactive, so the native video is not drawn over the dialog
    /// or the dimmed page, takes no clicks, and gives the keyboard back; the stream keeps
    /// running, and the video, the pointer and the keys come back when the dialog closes.
    /// (A popover or menu over it is still a hole: `aMenuOverTheViewerCutsAHoleAndTheVideoStaysLive`.)
    @Test func aModalDialogOverTheViewerHidesItAndTakesItsKeysAndClicks() async throws {
        let host = Host()
        let units = AccessUnit.split(Self.tinyH264)
        let s = surfaces(host) { _, _ in SyntheticStreamProvider(units: units, size: CGSize(width: 64, height: 48), fps: 30) }
        s.handle(message([entry("v", space: "a")]))
        await settle { host.phases("v").contains("live") }
        s.handle(["type": "focus", "surfaceId": "v"])
        #expect(s.focusedId == "v")
        let view = try #require(s.surfaces["v"]?.view)
        // The dialog opens: occluded, not interactive, no holes (the whole video is hidden).
        s.handle(message([entry("v", space: "a", interactive: false, occluded: true)]))
        #expect(view.isHidden)
        #expect(s.focusedId == nil)
        #expect(view.layer?.mask == nil)
        s.handle(["type": "focus", "surfaceId": "v"])
        #expect(s.focusedId == nil)
        // Closed: it is drawn again and takes the keyboard again, still live.
        s.handle(message([entry("v", space: "a")]))
        #expect(!view.isHidden)
        #expect(host.phases("v").last == "live")
        s.handle(["type": "focus", "surfaceId": "v"])
        #expect(s.focusedId == "v")
        s.removeAll()
    }

    @Test func theHarnessAddsRunningFixtureSpaces() {
        #expect(FixtureSpacesBackend.rows(environment: [:]).count == FixtureSpacesBackend.sample.count)
        let rows = FixtureSpacesBackend.rows(environment: ["CUA_SPACES_FIXTURE_SPACES": "3"])
        #expect(rows.prefix(3).map(\.id) == ["local:bench-1", "local:bench-2", "local:bench-3"])
        #expect(rows.count == FixtureSpacesBackend.sample.count + 3)
    }
}
