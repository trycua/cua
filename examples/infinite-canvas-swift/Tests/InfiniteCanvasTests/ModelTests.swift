// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CanvasModel
import CoreGraphics
import Foundation
import Testing

@Suite struct CameraTests {
    let view = CGSize(width: 1600, height: 1000)

    @Test func screenAndWorldRoundTrip() {
        let c = Camera(center: CGPoint(x: 300, y: -120), zoom: 0.37)
        let p = CGPoint(x: 811, y: 222)
        let back = c.screenPoint(forWorld: c.worldPoint(forScreen: p, in: view), in: view)
        #expect(abs(back.x - p.x) < 1e-9 && abs(back.y - p.y) < 1e-9)
    }

    @Test func zoomKeepsTheAnchorFixed() {
        let c = Camera(center: .zero, zoom: 0.5)
        let anchor = CGPoint(x: 1200, y: 300)
        let before = c.worldPoint(forScreen: anchor, in: view)
        let z = c.zoomed(by: 2.7, anchor: anchor, in: view)
        let after = z.worldPoint(forScreen: anchor, in: view)
        #expect(abs(before.x - after.x) < 1e-9 && abs(before.y - after.y) < 1e-9)
        #expect(abs(z.zoom - 1.35) < 1e-9)
    }

    @Test func zoomIsClamped() {
        #expect(Camera(zoom: 100).zoom == Camera.maxZoom)
        #expect(Camera(zoom: 0.0001).zoom == Camera.minZoom)
        #expect(Camera(zoom: .nan).zoom == 1)
    }

    @Test func panMovesContentWithTheFingers() {
        let c = Camera(center: .zero, zoom: 2).panned(byScreen: CGVector(dx: 100, dy: -40))
        #expect(c.center == CGPoint(x: -50, y: 20))
    }

    @Test func fittingShowsTheWholeRect() {
        let r = CGRect(x: -500, y: 200, width: 4000, height: 900)
        let c = Camera.fitting(r, in: view, padding: 50)
        let v = c.viewport(in: view)
        #expect(v.contains(r.insetBy(dx: 1, dy: 1)))
        #expect(abs(c.zoom - (1500 / 4000)) < 1e-9)
    }

    @Test func flightStartsAndEndsExactly() {
        let a = Camera(center: .zero, zoom: 0.2), b = Camera(center: CGPoint(x: 5000, y: 3000), zoom: 1.2)
        let f = CameraFlight(from: a, to: b, viewSize: view)
        #expect(f.camera(at: 0) == a)
        #expect(f.camera(at: f.duration) == b)
        #expect(f.duration >= 0.32 && f.duration <= 0.9)
        // Long hops zoom out on the way (van Wijk): some midpoint is wider
        // than both ends.
        let mid = (1 ..< 20).map { f.camera(at: f.duration * Double($0) / 20).zoom }.min()!
        #expect(mid < a.zoom)
    }

    @Test func pureZoomFlightIsMonotonic() {
        let a = Camera(center: CGPoint(x: 10, y: 10), zoom: 0.3), b = Camera(center: CGPoint(x: 10, y: 10), zoom: 1.5)
        let f = CameraFlight(from: a, to: b, viewSize: view)
        var last = a.zoom
        for i in 1 ... 30 {
            let z = f.camera(at: f.duration * Double(i) / 30).zoom
            #expect(z >= last - 1e-9)
            last = z
        }
    }
}

@Suite struct LayoutTests {
    func tile(_ id: String, _ r: CGRect, pixels: CGSize = .zero, kind: TileKind? = nil) -> Tile {
        Tile(id: id, kind: kind ?? .window(spaceID: "s", windowID: id), title: id, frame: r, sourcePixels: pixels)
    }

    @Test func addingRaisesAndBringToFrontReorders() {
        var l = CanvasLayout()
        l.add(tile("a", CGRect(x: 0, y: 0, width: 400, height: 300)))
        l.add(tile("b", CGRect(x: 100, y: 100, width: 400, height: 300)))
        #expect(l.hitTest(CGPoint(x: 150, y: 150))?.id == "b")
        l.bringToFront("a")
        #expect(l.hitTest(CGPoint(x: 150, y: 150))?.id == "a")
        #expect(l.frontToBack.map(\.id) == ["a", "b"])
    }

    @Test func moveAndHitTest() {
        var l = CanvasLayout(tiles: [tile("a", CGRect(x: 0, y: 0, width: 200, height: 100))])
        l.move("a", by: CGVector(dx: 50, dy: -20))
        #expect(l.tile("a")?.frame.origin == CGPoint(x: 50, y: -20))
        #expect(l.hitTest(CGPoint(x: 10, y: 10)) == nil)
    }

    @Test func streamResizeKeepsTheSourceAspect() {
        var l = CanvasLayout(tiles: [tile("a", CGRect(x: 0, y: 0, width: 1280, height: 800), pixels: CGSize(width: 1280, height: 800))])
        l.resize("a", to: CGSize(width: 640, height: 900))
        #expect(l.tile("a")?.frame.size == CGSize(width: 640, height: 400))
    }

    @Test func threadResizeIsFreeButNotBelowTheMinimum() {
        var l = CanvasLayout(tiles: [tile("t", CGRect(x: 0, y: 0, width: 400, height: 500), kind: .thread(threadID: "t"))])
        l.resize("t", to: CGSize(width: 500, height: 700))
        #expect(l.tile("t")?.frame.size == CGSize(width: 500, height: 700))
        l.resize("t", to: CGSize(width: 10, height: 10))
        #expect(l.tile("t")?.frame.size == Tile.minSize)
    }

    @Test func sourceSizeFixesTheAspectOnce() {
        var l = CanvasLayout(tiles: [tile("a", CGRect(x: 0, y: 0, width: 900, height: 900))])
        l.setSourcePixels("a", CGSize(width: 1800, height: 1200))
        #expect(l.tile("a")?.frame.size == CGSize(width: 900, height: 600))
    }

    @Test func justifiedRowsFillTheWidth() {
        let rows = CanvasLayout.justifiedRows([1.6, 1.6, 1.6, 0.6], width: 1800, gap: 40, maxRowHeight: 620)
        for r in rows.dropLast() {
            let w = r.widths.reduce(0, +) + CGFloat(r.widths.count - 1) * 40
            #expect(abs(w - 1800) <= CGFloat(r.widths.count))
        }
        #expect(rows.allSatisfy { $0.height <= 620 })
        #expect(rows.flatMap(\.widths).count == 4)
    }

    @Test func arrangeGroupsNeverOverlap() {
        var l = CanvasLayout()
        for i in 0 ..< 9 {
            l.add(tile("t\(i)", CGRect(x: 0, y: 0, width: 1000, height: CGFloat(500 + i * 40))))
        }
        l.arrange(groups: [["t0", "t1", "t2", "t3"], ["t4", "t5"], ["t6", "t7", "t8"]])
        let frames = l.tiles.map(\.frame)
        for i in frames.indices {
            for j in frames.indices where j > i {
                #expect(!frames[i].insetBy(dx: 1, dy: 1).intersects(frames[j].insetBy(dx: 1, dy: 1)))
            }
        }
        // Groups run left to right.
        #expect(l.tile("t0")!.frame.maxX < l.tile("t4")!.frame.minX)
        #expect(l.tile("t5")!.frame.maxX < l.tile("t6")!.frame.minX)
    }
}

@Suite struct LevelOfDetailTests {
    let screen = CGRect(x: 0, y: 0, width: 3200, height: 2000)
    let policy = LODPolicy()

    func v(_ edge: CGFloat, at x: CGFloat = 100, focused: Bool = false) -> TileVisibility {
        TileVisibility(screenRect: CGRect(x: x, y: 100, width: edge, height: edge * 0.6), screenBounds: screen, focused: focused)
    }

    @Test func tiersFollowDrawnSize() {
        #expect(policy.rawTier(for: v(100)) == .thumbnail)
        #expect(policy.rawTier(for: v(400)) == .low)
        #expect(policy.rawTier(for: v(900)) == .medium)
        #expect(policy.rawTier(for: v(1600)) == .full)
    }

    @Test func offScreenPausesAndFocusIsFull() {
        #expect(policy.rawTier(for: v(900, at: 9000)) == .paused)
        #expect(policy.rawTier(for: v(900, at: 3300)) == .medium) // inside the warm margin
        #expect(policy.rawTier(for: v(60, focused: true)) == .full)
        #expect(!StreamTier.paused.decodes && StreamTier.thumbnail.decodes)
    }

    @Test func preferencesGrowWithTheTier() {
        let tiers = StreamTier.allCases
        for (a, b) in zip(tiers, tiers.dropFirst()) {
            #expect(a.preferences.maxFps < b.preferences.maxFps)
            #expect(a.preferences.maxDimension < b.preferences.maxDimension)
        }
    }

    @Test func upgradesAreImmediateAndAskForAKeyframe() {
        var s = LODState(tier: .thumbnail)
        #expect(s.update(v(1600), now: 0, policy: policy) == .apply(.full, keyframe: true))
        #expect(s.tier == .full)
    }

    @Test func downgradesWaitOutTheHold() {
        var s = LODState(tier: .full)
        #expect(s.update(v(300), now: 0, policy: policy) == nil)
        #expect(s.update(v(300), now: 0.2, policy: policy) == nil)
        #expect(s.update(v(300), now: 0.5, policy: policy) == .apply(.low, keyframe: false))
    }

    @Test func aZoomPassingThroughDoesNotChangeTheTier() {
        var s = LODState(tier: .full)
        _ = s.update(v(300), now: 0, policy: policy)
        #expect(s.update(v(1600), now: 0.2, policy: policy) == nil)
        #expect(s.update(v(1600), now: 1.0, policy: policy) == nil)
        #expect(s.tier == .full)
    }

    @Test func hysteresisStopsFlapping() {
        var s = LODState(tier: .medium)
        // Just under the medium floor (560) but within the hysteresis band.
        for t in stride(from: 0.0, to: 3.0, by: 0.1) {
            #expect(s.update(v(520), now: t, policy: policy) == nil)
        }
        #expect(s.tier == .medium)
    }

    @Test func censusCountsDecodingTilesAndRequestedRate() {
        let c = LODCensus([.paused, .paused, .full, .low])
        #expect(c.decoding == 2)
        #expect(c.requestedFps == 1 + 1 + 60 + 12)
    }
}

@Suite struct HotkeyTests {
    @Test func toggles() {
        var o = OverlayState()
        #expect(o.hotkey(at: 1) == .present)
        o.transitionFinished()
        #expect(o.phase == .shown)
        #expect(o.hotkey(at: 2) == .dismiss)
        o.transitionFinished()
        #expect(o.phase == .hidden)
    }

    @Test func repeatsAreDebounced() {
        var o = OverlayState()
        #expect(o.hotkey(at: 1) == .present)
        #expect(o.hotkey(at: 1.05) == nil)
        #expect(o.phase == .presenting)
    }

    @Test func aPressMidTransitionReverses() {
        var o = OverlayState()
        _ = o.hotkey(at: 1)
        #expect(o.hotkey(at: 1.2) == .dismiss)
        #expect(o.phase == .dismissing)
        #expect(o.hotkey(at: 1.4) == .present)
        o.transitionFinished()
        #expect(o.phase == .shown)
    }

    @Test func escapeOnlyDismissesAVisibleOverlay() {
        var o = OverlayState()
        #expect(o.escape() == nil)
        _ = o.hotkey(at: 1)
        o.transitionFinished()
        #expect(o.escape() == .dismiss)
    }

    @Test func parsesHotkeys() {
        #expect(HotkeySpec(parsing: "option+space") == .default)
        let k = HotkeySpec(parsing: "cmd+shift+k")
        #expect(k?.keyCode == 40 && k?.modifiers == [.command, .shift])
        #expect(k?.description == "⇧⌘K")
        #expect(HotkeySpec(parsing: "space") == nil) // a bare key would steal typing
        #expect(HotkeySpec(parsing: "hyper+space") == nil)
        #expect(HotkeySpec.default.description == "⌥Space")
    }
}

@Suite struct SearchTests {
    @Test func subsequenceAndWordStartsRank() {
        let items = [("a", "Calculator macOS"), ("b", "Mozilla Firefox Linux"), ("c", "Terminal Linux")]
        #expect(TileSearch.rank("calc", items) == ["a"])
        #expect(TileSearch.rank("fx", items) == ["b"])
        #expect(TileSearch.rank("lin", items).first == "b" || TileSearch.rank("lin", items).first == "c")
        #expect(TileSearch.rank("zzz", items).isEmpty)
        #expect(TileSearch.score("", in: "x") == 0)
    }
}

/// Colors as the SDK hands them out, for model tests (the app injects
/// `PresenceColors`).
let testColors = PresenceColorSource(stable: { id in id.hasSuffix("1") ? "#4363d8" : "#bcf60c" },
                                     textOn: { hex in (RGB(hex: hex)?.luminance ?? 0) > 0.3 ? "#000000" : "#ffffff" })

@Suite struct PresenceModelTests {
    @Test func stylesUseTheInjectedTextColor() {
        let s = testColors.style(color: "#000075", name: "A")
        #expect(s.text == .white && s.fill == RGB(hex: "#000075"))
        #expect(testColors.style(color: "#ffffff", name: "").text == .black)
        #expect(testColors.style(color: "nope", name: "").fill == .unassigned)
    }

    @Test func serverColorWinsOnceJoinedStableColorBefore() {
        var d = PresenceDirectory(colors: testColors)
        // Before the agent joins: the thread's stable color.
        #expect(d.avatarStyle(threadID: "t1").fill == RGB(hex: "#4363d8"))
        d.join(PresenceMember(participantID: "p1", principalID: "cua-driver:1", name: "Agent", color: "#911eb4", isAgent: true), in: "s")
        #expect(d.agentAppeared("p1", in: "s", activeThreads: ["t1"]) == "t1")
        #expect(d.avatarStyle(threadID: "t1") == d.cursorStyle(participantID: "p1", in: "s"))
        #expect(d.avatarStyle(threadID: "t1").fill == RGB(hex: "#911eb4"))
        // Ambiguous: two threads mid-turn, nobody is guessed.
        d.join(PresenceMember(participantID: "p2", principalID: "x", name: "Agent", color: "#e6194b", isAgent: true), in: "s")
        #expect(d.agentAppeared("p2", in: "s", activeThreads: ["t2", "t3"]) == nil)
        #expect(d.avatarStyle(threadID: "t2").fill == RGB(hex: testColors.stable("t2")))
    }

    @Test func cursorGlideSettlesWithoutOvershoot() {
        var g = CursorGlide(at: .zero)
        g.target = CGPoint(x: 100, y: 0)
        var maxX: CGFloat = 0
        for _ in 0 ..< 240 {
            g.step(1 / 120)
            maxX = max(maxX, g.position.x)
        }
        #expect(g.isSettled && g.position == g.target)
        #expect(maxX <= 100.0001)
    }

    @Test func displayCursorsLandOnTheFrontWindow() {
        let windows = [WindowPlacement(windowID: "back", bounds: CGRect(x: 0, y: 0, width: 800, height: 600), z: 1),
                       WindowPlacement(windowID: "front", bounds: CGRect(x: 400, y: 300, width: 400, height: 300), z: 2)]
        let hit = WindowPlacement.locate(CGPoint(x: 0.75, y: 0.75), display: CGSize(width: 800, height: 600), windows: windows)
        #expect(hit?.windowID == "front")
        #expect(hit.map { abs($0.point.x - 0.5) < 1e-9 && abs($0.point.y - 0.5) < 1e-9 } == true)
        #expect(WindowPlacement.locate(CGPoint(x: 0.99, y: 0.01), display: CGSize(width: 800, height: 600),
                                       windows: [windows[1]]) == nil)
    }
}

@Suite struct FrameStatsTests {
    @Test func percentilesAndLateFrames() {
        var s = FrameStats()
        var t = 0.0
        for i in 0 ..< 100 {
            s.tick(t)
            t += i == 50 ? 0.030 : 1 / 120
        }
        let sum = s.summary(nominal: 1 / 120)
        #expect(sum.frames == 99)
        #expect(sum.late == 1)
        #expect(abs(sum.p50Ms - 8.333) < 0.01)
    }
}

@Suite struct HoverInfoTests {
    @Test func statsLineAndOSLabel() {
        let i = HoverInfo(os: "macOS 26.0", app: "Calculator", title: "Calculator", latencyMs: [3.2, 4.6],
                          fps: 29.6, resolution: CGSize(width: 1280, height: 800))
        #expect(i.statsLine == "5 ms · 30 fps · 1280×800")
        let fresh = HoverInfo(os: "", app: "", title: "", latencyMs: [], fps: 0, resolution: .zero)
        #expect(fresh.statsLine == "0 fps")
        #expect(HoverInfo.osLabel(kind: .linux, name: "ubuntu", version: "24.04") == "Ubuntu 24.04")
        #expect(HoverInfo.osLabel(kind: .linux, name: "Linux", version: "") == "Linux")
        #expect(HoverInfo.osLabel(kind: .macos, name: "macos", version: "26.5.2") == "macOS 26")
        #expect(HoverInfo.osLabel(kind: .omarchy, name: "arch", version: "rolling") == "Omarchy")
        #expect(HoverInfo.osLabel(kind: .windows, name: "Windows Server 2022", version: "") == "Windows Server 2022")
        #expect(HoverInfo.osLabel(kind: .windows, name: "windows", version: "") == "Windows")
        let named = HoverInfo(spaceName: "aurora", spaceAddress: "direct:127.0.0.1:3211", os: "", app: "", title: "",
                              latencyMs: [], fps: 0, resolution: .zero)
        #expect(named.spaceLine == "aurora · direct:127.0.0.1:3211")
        #expect(HoverInfo(spaceAddress: "local:x", os: "", app: "", title: "", latencyMs: [], fps: 0,
                          resolution: .zero).spaceLine == "local:x")
    }

    @Test func sparklineIsNormalized() {
        #expect(HoverInfo.sparkline([10, 20, 15]) == [0, 1, 0.5])
        #expect(HoverInfo.sparkline([4, 4]) == [0.5, 0.5])
        #expect(HoverInfo.sparkline([]).isEmpty)
        var ring = SampleRing(capacity: 3)
        for v in [1.0, 2, 3, 4, .nan] { ring.append(v) }
        #expect(ring.values == [2, 3, 4])
    }

    let tiles: [(id: String, frame: CGRect)] = [
        ("left", CGRect(x: -900, y: -300, width: 800, height: 600)),
        ("middle", CGRect(x: -100, y: -200, width: 400, height: 300)),
        ("far", CGRect(x: 5000, y: 0, width: 800, height: 600)),
    ]
    let viewport = CGRect(x: -1000, y: -500, width: 2000, height: 1000)

    @Test func hoverBeatsTheCenterMostTile() {
        #expect(InfoTarget.pick(hovered: nil, tiles: tiles, viewport: viewport) == "middle")
        #expect(InfoTarget.pick(hovered: "left", tiles: tiles, viewport: viewport) == "left")
        // A stale hover (tile gone) falls back to the center.
        #expect(InfoTarget.pick(hovered: "gone", tiles: tiles, viewport: viewport) == "middle")
    }

    @Test func offScreenTilesAreNeverTheTarget() {
        let away = CGRect(x: 4800, y: -500, width: 2000, height: 1000)
        #expect(InfoTarget.pick(hovered: nil, tiles: tiles, viewport: away) == "far")
        #expect(InfoTarget.pick(hovered: nil, tiles: tiles, viewport: CGRect(x: 90_000, y: 0, width: 10, height: 10)) == nil)
    }
}

@Suite struct StreamErrorTests {
    @Test func knownErrorsGetShortHumanMessages() {
        let transport = StreamErrorMessage.describe(#"Transport(message: "transport: transport error: transport error (stream error received: stream no longer needed)")"#)
        #expect(transport.headline == "Stream unavailable")
        #expect(transport.detail == "The Space closed the stream.")
        #expect(StreamErrorMessage.describe("cua-spacesd is not available: Connection refused (os error 61)").detail
            == "The Space is not reachable.")
        #expect(StreamErrorMessage.describe("stale_target: window epoch 3").detail == "The window closed or is hidden.")
        #expect(StreamErrorMessage.describe("the server selected png; this client decodes h264").detail
            == "The Space offers no H.264 stream.")
    }

    @Test func unknownErrorsAreTruncatedButKeptWhole() {
        let raw = String(repeating: "x", count: 500)
        let m = StreamErrorMessage.describe(raw)
        #expect(m.detail.count == 118 && m.detail.hasSuffix("…"))
        #expect(m.raw == raw)
    }
}

@Suite struct HumanPathTests {
    @Test func startsAndEndsWhereAsked() {
        let a = CGPoint(x: 0.9, y: 0.9), b = CGPoint(x: 0.2, y: 0.3)
        let path = HumanPath.samples(from: a, to: b, seed: 7)
        #expect(path.first!.t == 0)
        #expect(abs(path.first!.point.x - a.x) < 0.01 && abs(path.first!.point.y - a.y) < 0.01)
        #expect(path.last!.point == b)
        #expect(zip(path, path.dropFirst()).allSatisfy { $0.t < $1.t })
    }

    @Test func curvesOvershootsSlightlyAndIsNotAStraightLine() {
        let a = CGPoint(x: 0, y: 0), b = CGPoint(x: 1, y: 0)
        let path = HumanPath.samples(from: a, to: b, seed: 3)
        let maxOff = path.map { abs($0.point.y) }.max()!
        #expect(maxOff > 0.03 && maxOff < 0.2)   // bowed, not a straight line
        let maxX = path.map(\.point.x).max()!
        #expect(maxX > 1.0 && maxX < 1.06)        // a small overshoot that settles
        // Slow at the ends, fast in the middle (minimum jerk).
        let step = { (i: Int) in hypot(path[i + 1].point.x - path[i].point.x, path[i + 1].point.y - path[i].point.y) }
        #expect(step(path.count / 3) > step(1) * 3)
    }

    @Test func deterministicPerSeedAndHumanPaced() {
        let a = HumanPath.samples(from: .zero, to: CGPoint(x: 0.5, y: 0.5), seed: 11)
        let b = HumanPath.samples(from: .zero, to: CGPoint(x: 0.5, y: 0.5), seed: 11)
        #expect(a == b)
        let d = HumanPath.typingDelays("hi, i'm your coworker's cursor!", seed: 5)
        #expect(d.count == 31 && d.allSatisfy { $0 >= 0.07 && $0 < 0.7 })
        #expect(HumanPath.duration(from: .zero, to: CGPoint(x: 1, y: 1), scale: 1) <= 1.1)
    }
}

@Suite struct CursorHeadingTests {
    func settle(_ h: inout CursorHeading, _ v: CGVector, seconds: Double = 1) {
        for _ in 0 ..< Int(seconds * 120) { h.step(velocity: v, dt: 1.0 / 120) }
    }

    @Test func restsAtTheClassicTilt() {
        var h = CursorHeading()
        #expect(h.heading == CursorHeading.rest && h.rotation == 0)
        settle(&h, .zero)
        #expect(abs(h.rotation) < 1e-9)
    }

    @Test func aStraightLinePointsTheTipAlongTravel() {
        // Same convention as the driver overlay: heading = tangent + π.
        var right = CursorHeading()
        settle(&right, CGVector(dx: 400, dy: 0))
        #expect(abs(CursorHeading.shortest(from: right.heading, to: .pi)) < 1e-3)
        var down = CursorHeading()
        settle(&down, CGVector(dx: 0, dy: 400))
        #expect(abs(CursorHeading.shortest(from: down.heading, to: .pi / 2 + .pi)) < 1e-3)
        // Travelling up-left is the art's own orientation: no rotation.
        var upLeft = CursorHeading()
        settle(&upLeft, CGVector(dx: -300, dy: -300))
        #expect(abs(upLeft.rotation) < 1e-3)
    }

    @Test func aCurveTurnsSmoothlyAndTheShortWayRound() {
        var h = CursorHeading()
        settle(&h, CGVector(dx: 400, dy: 0))
        var last = h.heading
        var maxStep = 0.0
        // A quarter circle: the velocity turns from +x to +y over half a second.
        for i in 0 ..< 60 {
            let a = Double(i) / 59 * .pi / 2
            h.step(velocity: CGVector(dx: 400 * cos(a), dy: 400 * sin(a)), dt: 1.0 / 120)
            maxStep = max(maxStep, abs(CursorHeading.shortest(from: last, to: h.heading)))
            last = h.heading
        }
        #expect(maxStep < 0.1)  // no snapping
        // Flipping direction turns through the short side, never a full spin.
        var flip = CursorHeading()
        settle(&flip, CGVector(dx: 400, dy: 1))
        flip.step(velocity: CGVector(dx: -400, dy: 1), dt: 1.0 / 120)
        #expect(abs(CursorHeading.shortest(from: .pi, to: flip.heading)) < .pi)
    }

    @Test func slowJitterIsRestAndItEasesBack() {
        var h = CursorHeading()
        settle(&h, CGVector(dx: 400, dy: 0), seconds: 0.5)
        #expect(abs(h.rotation) > 1)
        // Below the speed floor counts as idle: it drifts back, not snaps.
        h.step(velocity: CGVector(dx: 10, dy: -5), dt: 1.0 / 120)
        #expect(abs(h.rotation) > 0.9)
        settle(&h, CGVector(dx: 5, dy: 5), seconds: 2)
        #expect(abs(h.rotation) < 0.01)
    }
}
