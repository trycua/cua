// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CanvasModel
import CanvasStreaming
import Cua
import CuaSpacesStreaming
import SwiftUI
@testable import InfiniteCanvasApp
import Testing

/// A stream tile for tests: the local test-pattern encoder, not a Space.
@MainActor
func syntheticTile(_ c: CanvasController, id: String, space: String = "s1", window: String = "w1") -> TileStream {
    let size = CGSize(width: 640, height: 400)
    let stream = TileStream(id: id, source: SyntheticMediaSource(size: size, seed: 1))
    c.addStream(Tile(id: id, kind: .window(spaceID: space, windowID: window), title: "Window \(id)",
                     frame: CGRect(origin: .zero, size: size), sourcePixels: size),
                stream: stream, windowOf: (space, window))
    return stream
}

@MainActor
func thread(_ c: CanvasController, space: String = "s1") -> AgentThread {
    let t = AgentThread(id: "thread:\(space)", spaceID: space, spaceLabel: "Linux",
                        launch: AgentLaunch(harness: "claude-code", baseURL: nil, model: nil, env: [:]),
                        agents: { throw CancellationError() })
    c.addThread(Tile(id: t.id, kind: .thread(threadID: t.id), title: t.title,
                     frame: CGRect(x: 800, y: 0, width: 420, height: 560)), thread: t)
    return t
}

@Suite(.serialized) @MainActor struct AgentColorTests {
    /// The user's rule: an agent's avatar background (in its thread window)
    /// is exactly its cursor's color, both from the Space's presence
    /// assignment, and the message bubbles stay neutral.
    @Test func avatarBackgroundEqualsCursorColor() {
        let c = CanvasController()
        _ = syntheticTile(c, id: "tile-1")
        let t = thread(c)
        // Before the agent joins: the SDK's stable color for the thread.
        #expect(c.threadStyle(tileID: t.id)?.fill == RGB(hex: PresenceColors.color(for: t.id)))
        t.setTurn(true)
        let assigned = "#911eb4"
        let agent = PresenceParticipant(participantId: "agent-1", principalId: "cua-driver:1",
                                        displayName: "CUA agent 1", color: assigned, kind: "agent")
        c.presenceEvent(CuaSDK.PresenceEvent(kind: "joined", participant: agent, participantId: nil, cursor: nil),
                        space: "s1")
        c.presenceEvent(CuaSDK.PresenceEvent(kind: "cursor_moved", participant: nil, participantId: "agent-1",
                                             cursor: PresenceCursor(displayId: "", windowId: "w1", x: 0.4, y: 0.6,
                                                                    visible: true)),
                        space: "s1")
        // The cursor leaves the thread's dock first.
        c.stepTransits(now: CACurrentMediaTime() + 5, zoom: 1)
        let view = c.views["tile-1"] as! StreamTileView
        let cursor = try! #require(view.cursors["agent-1"])
        let threadStyle = try! #require(c.threadStyle(tileID: t.id))
        let avatar = AgentAvatar(style: threadStyle)
        #expect(cursor.agentStyle.fill == RGB(hex: assigned))
        #expect(avatar.background == cursor.agentStyle.fill)
        #expect(threadStyle == c.presence.cursorStyle(participantID: "agent-1", in: "s1"))
        #expect(avatar.style.text == RGB(hex: PresenceColors.textColor(on: assigned)))
        #expect(!cursor.isHuman)
    }

    /// The agent's cursor waits in its thread window's title, glides out to
    /// the first window it acts in, and glides home when the turn ends.
    @Test func theAgentCursorLeavesItsDockAndComesBack() throws {
        let c = CanvasController()
        _ = syntheticTile(c, id: "tile-3", window: "w3")
        let t = thread(c)
        #expect(c.dockedThreads.contains(t.id))
        #expect(c.views[t.id]?.hasAppIcon == true) // the glyph
        t.setTurn(true)
        let agent = PresenceParticipant(participantId: "agent-9", principalId: "cua-driver:9",
                                        displayName: "CUA agent 9", color: "#4363d8", kind: "agent")
        c.presenceEvent(CuaSDK.PresenceEvent(kind: "joined", participant: agent, participantId: nil, cursor: nil), space: "s1")
        c.presenceEvent(CuaSDK.PresenceEvent(kind: "cursor_moved", participant: nil, participantId: "agent-9",
                                             cursor: PresenceCursor(displayId: "", windowId: "w3", x: 0.5, y: 0.5, visible: true)),
                        space: "s1")
        #expect(!c.dockedThreads.contains(t.id))
        #expect(c.transits["agent-9"] != nil)
        let v = c.views["tile-3"] as! StreamTileView
        #expect(v.cursors["agent-9"] == nil) // still flying
        c.stepTransits(now: CACurrentMediaTime() + 5, zoom: 1)
        #expect(c.transits.isEmpty)
        let cursor = try #require(v.cursors["agent-9"])
        #expect(cursor.agentStyle.fill == RGB(hex: "#4363d8"))
        // Turn over: back to the dock.
        t.setTurn(false)
        #expect(v.cursors["agent-9"] == nil && c.transits["agent-9"]?.toDock == true)
        c.stepTransits(now: CACurrentMediaTime() + 5, zoom: 1)
        #expect(c.dockedThreads.contains(t.id) && c.views[t.id]?.hasAppIcon == true)
    }

    /// The canvas draws participants as they come: one agent run is already
    /// one participant (cua-spacesd keys the driver cursor by the run), so
    /// two agent participants are two agents and keep two cursors.
    @Test func twoAgentParticipantsAreTwoCursors() throws {
        let c = CanvasController()
        _ = syntheticTile(c, id: "tile-4", window: "w4")
        for (id, color) in [("agent-a", "#3cb44b"), ("agent-b", "#4363d8")] {
            let p = PresenceParticipant(participantId: id, principalId: "cua-driver:__cua_runtime_x:agent-run-\(id)",
                                        displayName: "CUA agent", color: color, kind: "agent")
            c.presenceEvent(CuaSDK.PresenceEvent(kind: "joined", participant: p, participantId: nil, cursor: nil), space: "s1")
            c.presenceEvent(CuaSDK.PresenceEvent(kind: "cursor_moved", participant: nil, participantId: id,
                                                 cursor: PresenceCursor(displayId: "", windowId: "w4", x: 0.2, y: 0.2,
                                                                        visible: true)),
                            space: "s1")
        }
        let v = c.views["tile-4"] as! StreamTileView
        #expect(v.cursors.keys.sorted() == ["agent-a", "agent-b"])
        #expect(v.cursors["agent-b"]?.agentStyle.fill == RGB(hex: "#4363d8"))
    }

    /// A person on the Space is drawn too, as a plain pointer in their own
    /// color, with no agent theme and no thread binding.
    @Test func peopleGetAPlainPointerInTheirColor() throws {
        let c = CanvasController()
        _ = syntheticTile(c, id: "tile-2", window: "w2")
        let person = PresenceParticipant(participantId: "human-1", principalId: "coworker-1",
                                         displayName: "Maya", color: "#f58231", kind: "human")
        c.presenceEvent(CuaSDK.PresenceEvent(kind: "joined", participant: person, participantId: nil, cursor: nil),
                        space: "s1")
        c.presenceEvent(CuaSDK.PresenceEvent(kind: "cursor_moved", participant: nil, participantId: "human-1",
                                             cursor: PresenceCursor(displayId: "", windowId: "w2", x: 0.5, y: 0.5,
                                                                    visible: true)),
                        space: "s1")
        let v = c.views["tile-2"] as! StreamTileView
        let cursor = try #require(v.cursors["human-1"])
        #expect(cursor.isHuman)
        #expect(cursor.agentStyle.fill == RGB(hex: "#f58231") && cursor.agentStyle.name == "Maya")
    }

    func personOnTile(_ c: CanvasController, shape: String = "text") -> StreamTileView {
        _ = syntheticTile(c, id: "tile-2", window: "w2")
        let person = PresenceParticipant(participantId: "human-1", principalId: "coworker-1",
                                         displayName: "Maya", color: "#f58231", kind: "human")
        c.presenceEvent(CuaSDK.PresenceEvent(kind: "joined", participant: person, participantId: nil, cursor: nil),
                        space: "s1", localMs: 0)
        c.presenceEvent(CuaSDK.PresenceEvent(kind: "cursor_moved", participant: nil, participantId: "human-1",
                                             cursor: PresenceCursor(displayId: "", windowId: "w2", x: 0.5, y: 0.5,
                                                                    visible: true, pressed: false, shape: shape,
                                                                    shapeSource: "probe", atMs: 1_000,
                                                                    receivedMs: 1_000)),
                        space: "s1", localMs: 1_000)
        return c.views["tile-2"] as! StreamTileView
    }

    /// People's cursors take the shape their Space reports, in the shared art.
    @Test func peopleCursorsTakeTheSpacesShape() throws {
        let c = CanvasController()
        let v = personOnTile(c, shape: "text")
        #expect(try #require(v.cursors["human-1"]).humanShape == "text")
    }

    @Test func anIdleCursorFadesAndMovingBringsItBack() throws {
        let c = CanvasController()
        let v = personOnTile(c)
        c.sweepPresence(localMs: 6_000)
        #expect(v.cursors["human-1"]?.opacity == 1)
        c.sweepPresence(localMs: 6_150)
        let fading = try #require(v.cursors["human-1"]).opacity
        #expect(fading > 0.3 && fading < 0.7, "\(fading)")
        c.sweepPresence(localMs: 6_400)
        #expect(v.cursors["human-1"]?.opacity == 0)
        c.presenceEvent(CuaSDK.PresenceEvent(kind: "cursor_moved", participant: nil, participantId: "human-1",
                                             cursor: PresenceCursor(displayId: "", windowId: "w2", x: 0.6, y: 0.5,
                                                                    visible: true, pressed: false, shape: "arrow",
                                                                    shapeSource: "probe", atMs: 6_500,
                                                                    receivedMs: 6_500)),
                        space: "s1", localMs: 6_500)
        c.sweepPresence(localMs: 6_600)
        #expect(v.cursors["human-1"]?.opacity == 1)
    }

    @Test func runEndRemovesTheCursor() {
        let c = CanvasController()
        let v = personOnTile(c)
        c.presenceEvent(CuaSDK.PresenceEvent(kind: "left", participant: nil, participantId: "human-1", cursor: nil,
                                             reason: "run_ended"), space: "s1", localMs: 2_000)
        #expect(v.cursors["human-1"] == nil)
    }

    @Test func stoppedHeartbeatsRemoveTheCursor() {
        let c = CanvasController()
        let v = personOnTile(c)
        c.presenceEvent(CuaSDK.PresenceEvent(kind: "heartbeat", participant: nil, participantId: nil, cursor: nil,
                                             participantIds: ["human-1"]), space: "s1", localMs: 1_500)
        c.sweepPresence(localMs: 2_000)
        #expect(v.cursors["human-1"] != nil)
        // Three 5 s intervals without a heartbeat: stale, gone.
        c.sweepPresence(localMs: 17_000)
        #expect(v.cursors["human-1"] == nil)
    }
}

@Suite @MainActor struct IconTests {
    func key(_ os: SpaceOS = .linux, app: String = "Firefox", pid: UInt32 = 7) -> AppIconKey {
        AppIconKey(spaceID: "s", os: os, app: app, appID: "org.mozilla.firefox", pid: pid)
    }

    static func png() -> Data {
        let image = NSImage(size: NSSize(width: 8, height: 8), flipped: false) { r in
            NSColor.orange.setFill(); r.fill(); return true
        }
        let rep = NSBitmapImageRep(data: image.tiffRepresentation!)!
        return rep.representation(using: .png, properties: [:])!
    }

    @Test func everyTilesIconInOneBatchedCall() async {
        var asked: [[AppIconKey]] = []
        let icons = AppIcons { keys in asked.append(keys); return keys.map { _ in Self.png() } }
        let images = await icons.icons(for: [key(pid: 1), key(app: "Thunar", pid: 2), key(.macos, pid: 3)])
        #expect(images.allSatisfy { $0 != nil })
        #expect(icons.calls == 1 && asked.first?.count == 3)
        #expect(await icons.icons(for: []).isEmpty && icons.calls == 1)
    }

    @Test func noIconMeansNoIconNotAPlaceholder() async {
        let none = AppIcons { keys in keys.map { _ in nil } }
        #expect(await none.icons(for: [key()]) == [nil])
        let garbage = AppIcons { keys in keys.map { _ in Data("not an image".utf8) } }
        #expect(await garbage.icons(for: [key()]) == [nil])

        let c = CanvasController()
        _ = syntheticTile(c, id: "t")
        let v = c.views["t"]!
        c.decorate("t", os: .linux, icon: nil)
        #expect(v.iconLayer.isHidden && !v.hasAppIcon && v.iconLayer.contents == nil)
        #expect(v.titleLayer.frame.minX == 0)
        #expect(c.icons["t"] == nil)
        c.decorate("t", os: .linux, icon: NSImage(data: Self.png()))
        #expect(!v.iconLayer.isHidden && v.titleLayer.frame.minX > 0)
        #expect(c.icons["t"] != nil)
    }

    @Test func everyOSHasAMark() {
        for os in SpaceOS.allCases {
            #expect(OSMark.image(os) != nil, "\(os)")
        }
    }
}

@Suite @MainActor struct TileChromeTests {
    @Test func aLongErrorStaysInsideTheTile() throws {
        let c = CanvasController()
        _ = syntheticTile(c, id: "e")
        let v = c.views["e"] as! StreamTileView
        v.frame = CGRect(x: 0, y: 0, width: 360, height: 260)
        v.layoutSubtreeIfNeeded()
        let raw = "Transport(message: \"" + String(repeating: "transport: transport error ", count: 40) + "\")"
        v.setError(raw)
        let content = v.contentHost.bounds
        #expect(content.contains(v.errorHeadline.frame))
        #expect(content.contains(v.errorDetail.frame))
        #expect(v.errorDetail.frame.height <= ceil(v.errorDetail.fontSize * 1.25) * 3)
        #expect(v.errorHeadline.string as? String == "Stream unavailable")
        #expect(v.toolTip == raw)
        // A snapshot of the tile content, for review.
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("infinite-canvas-snapshots")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let rep = v.contentHost.bitmapImageRepForCachingDisplay(in: v.contentHost.bounds)!
        v.contentHost.cacheDisplay(in: v.contentHost.bounds, to: rep)
        try rep.representation(using: .png, properties: [:])!.write(to: dir.appendingPathComponent("tile-error.png"))
        v.setError(nil)
        #expect(v.errorHeadline.isHidden && v.toolTip == nil)
    }

    @Test func hoverCardDataComesFromTheTile() {
        let c = CanvasController()
        _ = syntheticTile(c, id: "h")
        c.tileMeta["h"] = ("Calculator", "Calculator — Basic", "s1")
        c.osLabels["s1"] = "macOS 26.0"
        c.spaceInfo["s1"] = ("aurora", "direct:127.0.0.1:3211", .macos)
        let info = c.hoverInfo(for: "h")
        #expect(info?.spaceLine == "aurora · direct:127.0.0.1:3211" && info?.osKind == .macos)
        #expect(info?.app == "Calculator" && info?.title == "Calculator — Basic" && info?.os == "macOS 26.0")
        #expect(c.hoverInfo(for: "missing") == nil)
    }

    @Test func theCardFollowsHoverElseTheCenterMostTile() throws {
        let c = CanvasController()
        c.scroll.frame = CGRect(x: 0, y: 0, width: 1200, height: 800)
        for (i, id) in ["a", "b"].enumerated() {
            _ = syntheticTile(c, id: id, window: "w\(i)")
            c.tileMeta[id] = ("App \(id)", "Title \(id)", "s1")
        }
        c.tileMove("b", byWorld: CGVector(dx: 2000, dy: 0))
        c.setCamera(Camera(center: CGPoint(x: 320, y: 200), zoom: 0.5))
        c.refreshCard()
        #expect(c.cardTarget == "a" && c.hover.info?.app == "App a")
        c.tileHover("b", inside: true)
        #expect(c.cardTarget == "b" && c.hover.info?.title == "Title b")
        c.tileHover("b", inside: false)
        #expect(c.cardTarget == "a")

        // A snapshot of the card, for review.
        c.hover.info = HoverInfo(spaceName: "Linux", spaceAddress: "direct:127.0.0.1:34802", osKind: .linux,
                                 os: "Ubuntu 24.04", app: "Firefox", title: "Infinite canvas - Wikipedia",
                                 latencyMs: [2.1, 2.4, 1.9, 3.2, 2.2], fps: 30, resolution: CGSize(width: 1280, height: 749))
        let host = NSHostingView(rootView: HoverCard(model: c.hover))
        host.frame = CGRect(origin: .zero, size: CGSize(width: HoverCard.size.width, height: host.fittingSize.height))
        host.layoutSubtreeIfNeeded()
        let rep = try #require(host.bitmapImageRepForCachingDisplay(in: host.bounds))
        host.cacheDisplay(in: host.bounds, to: rep)
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("infinite-canvas-snapshots")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        try rep.representation(using: .png, properties: [:])!.write(to: dir.appendingPathComponent("stats-card.png"))
    }
}

@Suite(.serialized) @MainActor struct UISmokeTests {
    /// Build the real canvas in a real (offscreen) window with live H.264
    /// tiles: frames decode in hardware into the display layers, the camera
    /// flies, focus lands, level of detail pauses what is off screen, and
    /// the hotkey state machine drives the overlay.
    @Test func canvasRendersFliesAndFocuses() async throws {
        let c = CanvasController()
        let window = NSWindow(contentRect: NSRect(x: -4000, y: -4000, width: 1200, height: 800),
                              styleMask: [.borderless], backing: .buffered, defer: false)
        c.scroll.frame = window.contentView!.bounds
        window.contentView!.addSubview(c.scroll)
        window.orderFrontRegardless()
        defer { window.orderOut(nil) }
        var streams: [TileStream] = []
        for i in 0 ..< 4 { streams.append(syntheticTile(c, id: "t\(i)", window: "w\(i)")) }
        var layout = c.layout
        layout.arrange(groups: [c.layout.tiles.map(\.id)])
        for t in layout.tiles {
            let cur = c.layout.tile(t.id)!
            c.tileMove(t.id, byWorld: CGVector(dx: t.frame.minX - cur.frame.minX, dy: t.frame.minY - cur.frame.minY))
        }
        c.setCamera(Camera.fitting(c.layout.bounds, in: c.scroll.bounds.size))
        for s in streams { try await s.start() }
        // Frames arrive (bounded wait).
        for _ in 0 ..< 100 where streams.contains(where: { $0.decoder.counters.snapshot().presented == 0 }) {
            try await Task.sleep(for: .milliseconds(50))
        }
        for s in streams {
            let snap = s.decoder.counters.snapshot()
            #expect(snap.presented > 0, "\(s.id) presented nothing")
            #expect(snap.hardware == true, "\(s.id) not hardware decoded")
        }
        #expect(c.views.count == 4)
        #expect(c.scroll.world.subviews.count == 4)

        // Zoom into one tile: the flight ends on it and it takes focus.
        c.zoomInto("t2")
        var now = CACurrentMediaTime()
        for _ in 0 ..< 240 where c.scroll.isFlying {
            now += 1 / 120
            c.tick(now: now + 1)
        }
        #expect(!c.scroll.isFlying)
        #expect(c.focusedID == "t2")
        let t2 = c.layout.tile("t2")!
        #expect(c.camera.viewport(in: c.scroll.bounds.size).contains(CGPoint(x: t2.frame.midX, y: t2.frame.midY)))
        c.updateLevelOfDetail(now: now + 10)
        #expect(c.tier(of: "t2") == .full)

        // Pan far away: everything pauses after the hold.
        c.unfocus()
        c.setCamera(Camera(center: CGPoint(x: 1_000_000, y: 0), zoom: 1))
        c.updateLevelOfDetail(now: now + 20)
        c.updateLevelOfDetail(now: now + 21)
        #expect(c.layout.tiles.allSatisfy { c.tier(of: $0.id) == .paused })
        #expect(streams.allSatisfy { !$0.decoder.isEnabled })

        // Search finds and selects by title.
        c.updateSearch("window t3")
        #expect(c.selectedID == "t3")
        for s in streams { await s.stop() }
    }

    @Test func hotkeyPresentsAndDismissesTheWindow() async throws {
        var o = Options()
        o.windowed = CGSize(width: 640, height: 400)
        o.activate = false
        o.hotkey = HotkeySpec(parsing: "cmd+ctrl+option+shift+9")!
        let app = CanvasApp(options: o)
        app.applicationDidFinishLaunching(Notification(name: NSApplication.didFinishLaunchingNotification))
        #expect(app.overlay.phase == .hidden)
        app.hotkeyPressed()
        #expect(app.overlay.phase == .presenting)
        for _ in 0 ..< 40 where app.overlay.phase != .shown { try await Task.sleep(for: .milliseconds(25)) }
        #expect(app.overlay.phase == .shown)
        #expect(app.window.isVisible)
        try await Task.sleep(for: .milliseconds(200))
        app.hotkeyPressed()
        for _ in 0 ..< 40 where app.overlay.phase != .hidden { try await Task.sleep(for: .milliseconds(25)) }
        #expect(app.overlay.phase == .hidden)
        #expect(!app.window.isVisible)
        app.canvas.stopDisplayLink()
    }
}
