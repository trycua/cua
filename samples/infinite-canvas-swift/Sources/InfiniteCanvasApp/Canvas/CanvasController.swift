// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CanvasModel
import CanvasStreaming
import Combine
import Cua
import CuaSpacesStreaming
import os
import QuartzCore
import SwiftUI

/// Owns the canvas: the tile layout, the camera, selection and focus, the
/// streams and their level of detail, agent cursors and threads.
///
/// Main actor, but nothing here runs per decoded frame: frames go from the
/// SDK's delivery thread to VideoToolbox to the display layer. Per display
/// frame this does O(tiles) arithmetic (level of detail every 100 ms, cursor
/// glides while one moves).
@MainActor
final class CanvasController: NSObject, ObservableObject, TileViewDelegate {
    @Published private(set) var layout = CanvasLayout()
    /// The live camera (every scroll/zoom event). Not published: the HUD
    /// reads `hudCamera`, refreshed at most 20 times a second, so a pan
    /// never re-renders SwiftUI at display rate.
    private(set) var camera = Camera()
    @Published private(set) var hudCamera = Camera()
    private var lastHUD: CFTimeInterval = 0
    @Published private(set) var selectedID: String?
    @Published private(set) var focusedID: String?
    @Published var query = ""
    @Published private(set) var matches: [String] = []
    @Published private(set) var census = LODCensus([])
    @Published private(set) var presence = PresenceDirectory(colors: .sdk)
    /// App icons by tile, as the Space supplied them (absent: none).
    @Published private(set) var icons: [String: NSImage] = [:]

    let scroll = CanvasScrollView(frame: NSRect(x: 0, y: 0, width: 1280, height: 800))
    private(set) var views: [String: TileBaseView] = [:]
    private(set) var streams: [String: TileStream] = [:]
    private(set) var threads: [String: AgentThread] = [:]
    private var threadStyles: [String: ThreadStyleSource] = [:]
    /// tile id -> (space id, window id) for cursor placement.
    private var windowTiles: [String: (space: String, window: String)] = [:]
    private var lod: [String: LODState] = [:]
    let policy = LODPolicy()
    private var lastLOD: CFTimeInterval = 0
    private var lastTick: CFTimeInterval = 0
    private var displayLink: CADisplayLink?
    var frameStats = FrameStats()
    /// Main-thread time spent inside `tick`, for the benchmark.
    private(set) var tickWork = FrameStats()
    var onTick: ((CFTimeInterval) -> Void)?
    private let signposter = OSSignposter(subsystem: Signposts.subsystem, category: "canvas")
    /// Space id -> the canvas's cursor placement helpers.
    var spaceLookup: ((String) -> AttachedSpace?)?

    override init() {
        super.init()
        scroll.onCameraChange = { [weak self] c in self?.cameraDidChange(c) }
        scroll.world.onBackgroundClick = { [weak self] _ in
            self?.unfocus()
            self?.select(nil)
        }
    }

    // MARK: Display link

    func startDisplayLink() {
        guard displayLink == nil else { return }
        let link = scroll.displayLink(target: self, selector: #selector(displayTick(_:)))
        link.add(to: .main, forMode: .common)
        displayLink = link
    }

    func stopDisplayLink() {
        displayLink?.invalidate()
        displayLink = nil
    }

    @objc private func displayTick(_ link: CADisplayLink) {
        tick(now: link.timestamp)
    }

    /// One display frame (the display link calls this; tests call it
    /// directly).
    func tick(now: CFTimeInterval) {
        let started = CACurrentMediaTime()
        frameStats.tick(now)
        let dt = lastTick == 0 ? 1 / 120 : now - lastTick
        lastTick = now
        let state = signposter.beginInterval("tick")
        scroll.tick(now)
        onTick?(now)
        let z = camera.zoom
        for case let v as StreamTileView in views.values {
            _ = v.stepCursors(dt: dt, now: now, zoom: z)
        }
        if now - lastPresenceSweep >= 0.05 {
            lastPresenceSweep = now
            sweepPresence(localMs: presenceNowMs())
        }
        if !transits.isEmpty { stepTransits(now: CACurrentMediaTime(), zoom: z) }
        if camera != hudCamera, now - lastHUD > 0.05 {
            lastHUD = now
            hudCamera = camera
            // The center-most tile changes as the camera moves.
            if hoveredID == nil { refreshCard() }
        }
        if CACurrentMediaTime() - lastZoomChange > 0.2 {
            for case let v as ThreadTileView in views.values where v.isFrozen { v.thaw() }
        }
        if now - lastLOD > 0.1 {
            lastLOD = now
            updateLevelOfDetail(now: now)
        }
        signposter.endInterval("tick", state)
        let spent = CACurrentMediaTime() - started
        tickWork.tick(tickWorkClock)
        tickWorkClock += spent
    }

    private var tickWorkClock: Double = 0

    // MARK: Tiles

    func addStream(_ tile: Tile, stream: TileStream, windowOf: (space: String, window: String)?) {
        layout.add(tile)
        streams[tile.id] = stream
        if let windowOf { windowTiles[tile.id] = windowOf }
        let v = StreamTileView(tileID: tile.id, title: tile.title, stream: stream)
        v.delegate = self
        views[tile.id] = v
        scroll.world.addSubview(v)
        stream.onEvent = { [weak v] kind, json in
            if kind == "error" || kind == "closed" { v?.setError(json) }
        }
        stream.onSurfaceSize = { [weak self, weak v] size in
            guard let self else { return }
            v?.surfaceSize = size
            v?.setPlaceholder(nil)
            if v?.error != nil { v?.setError(nil) }
            self.layout.setSourcePixels(tile.id, size)
            self.syncFrame(tile.id)
        }
        lod[tile.id] = LODState(tier: .medium)
        syncFrame(tile.id)
        if !isPresented {
            // Hidden at launch: open the stream paused.
            DispatchQueue.main.async { [weak self] in self?.updateLevelOfDetail(now: CACurrentMediaTime()) }
        }
    }

    func addThread(_ tile: Tile, thread: AgentThread) {
        layout.add(tile)
        threads[tile.id] = thread
        let styles = ThreadStyleSource(style: presence.avatarStyle(threadID: thread.id))
        threadStyles[tile.id] = styles
        let content = ThreadView(thread: thread, styleSource: styles) { [weak self] in self?.focus(tile.id) }
        let v = ThreadTileView(tileID: tile.id, title: tile.title, content: AnyView(content))
        v.delegate = self
        views[tile.id] = v
        dockedThreads.insert(thread.id)
        v.setAppIcon(DockGlyph.image(styles.style))
        scroll.world.addSubview(v)
        thread.onTurnChange = { [weak self] running in
            self?.objectWillChange.send()
            if !running { self?.returnToDock(threadID: thread.id) }
        }
        syncFrame(tile.id)
    }

    /// Put a tile's view where the layout says, in z order.
    func syncFrame(_ id: String) {
        guard let t = layout.tile(id), let v = views[id] else { return }
        let f = t.frame
        let doc = WorldView.doc(CGRect(x: f.minX, y: f.minY - TileBaseView.headerHeight,
                                       width: f.width, height: f.height + TileBaseView.headerHeight))
        if v.frame != doc { v.frame = doc }
        v.layer?.zPosition = CGFloat(t.z)
    }

    private func syncOrder() {
        for t in layout.tiles { views[t.id]?.layer?.zPosition = CGFloat(t.z) }
        // Hit testing follows subview order, so keep it in z order too.
        let ordered = layout.tiles.sorted { $0.z < $1.z }.compactMap { views[$0.id] }
        scroll.world.subviews = ordered
    }

    /// Set a tile's OS mark and, when the Space has one, its app icon.
    func decorate(_ id: String, os: SpaceOS?, icon: NSImage?) {
        views[id]?.setOS(os)
        views[id]?.setAppIcon(icon)
        icons[id] = icon
    }

    // MARK: Stats card

    /// The fixed card in the bottom-left corner: the hovered tile's stats,
    /// otherwise the tile nearest the middle of the view.
    let hover = HoverModel()
    /// Per tile: the app and window title; per Space: its OS label.
    var tileMeta: [String: (app: String, title: String, space: String)] = [:]
    var osLabels: [String: String] = [:]
    /// Per Space: its name, address or id, and OS kind.
    var spaceInfo: [String: (name: String, address: String, os: SpaceOS)] = [:]
    private(set) var hoveredID: String?
    private(set) var cardTarget: String?
    private var cardTimer: Timer?

    func tileHover(_ id: String, inside: Bool) {
        guard streams[id] != nil else { return }
        if inside { hoveredID = id } else if hoveredID == id { hoveredID = nil }
        refreshCard()
    }

    /// Start the card's 2 Hz refresh (stats only; the stream layers are
    /// never touched).
    func startCard() {
        cardTimer?.invalidate()
        cardTimer = Timer.scheduledTimer(withTimeInterval: 0.5, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.refreshCard() }
        }
        refreshCard()
    }

    /// The card's data for a tile (also what the tests check).
    func hoverInfo(for id: String) -> HoverInfo? {
        guard let stream = streams[id], let meta = tileMeta[id] else { return nil }
        let st = stream.stats
        let sp = spaceInfo[meta.space]
        return HoverInfo(spaceName: sp?.name ?? "", spaceAddress: sp?.address ?? meta.space, osKind: sp?.os,
                         os: osLabels[meta.space] ?? sp?.os.label ?? "", app: meta.app, title: meta.title,
                         latencyMs: st.latencyMs, fps: st.fps, resolution: st.resolution)
    }

    func refreshCard() {
        let candidates = layout.tiles.filter { streams[$0.id] != nil && tileMeta[$0.id] != nil }
            .map { (id: $0.id, frame: $0.frame) }
        let target = InfoTarget.pick(hovered: hoveredID, tiles: candidates,
                                     viewport: camera.viewport(in: scroll.bounds.size))
        cardTarget = target
        guard let target, let info = hoverInfo(for: target) else {
            if hover.info != nil { hover.info = nil }
            return
        }
        if hover.targetID != target { hover.targetID = target }
        if hover.info != info { hover.info = info }
        if hover.icon !== icons[target] { hover.icon = icons[target] }
    }

    // MARK: Selection and focus

    var focusedTileID: String? { focusedID }
    var zoom: CGFloat { camera.zoom }

    func select(_ id: String?) {
        selectedID = id
        if let id {
            layout.bringToFront(id)
            syncOrder()
        }
        for (tid, v) in views { v.setSelected(tid == id, zoom: camera.zoom) }
    }

    func focus(_ id: String) {
        select(id)
        focusedID = id
        if let v = views[id] { scroll.window?.makeFirstResponder(v) }
        updateLevelOfDetail(now: CACurrentMediaTime())
    }

    func unfocus() {
        focusedID = nil
        scroll.window?.makeFirstResponder(scroll)
    }

    func tileSelect(_ id: String) { select(id) }
    func tileFocus(_ id: String) { focus(id) }
    func tileZoomInto(_ id: String) { zoomInto(id) }
    func tileEscape(_ id: String) { unfocus() }

    func tileMove(_ id: String, byWorld delta: CGVector) {
        layout.move(id, by: delta)
        syncFrame(id)
    }

    func tileResize(_ id: String, toWorld size: CGSize) {
        guard let t = layout.tile(id) else { return }
        _ = t
        layout.resize(id, to: CGSize(width: size.width, height: size.height))
        syncFrame(id)
    }

    func tileEndGesture(_ id: String) {
        updateLevelOfDetail(now: CACurrentMediaTime())
    }

    // MARK: Camera

    private var titleZoom: CGFloat = 0

    private var lastZoomChange: CFTimeInterval = 0

    private func cameraDidChange(_ c: Camera) {
        if abs(c.zoom - camera.zoom) > 1e-6 {
            lastZoomChange = CACurrentMediaTime()
            for case let v as ThreadTileView in views.values where !v.isFrozen && focusedID != v.tileID {
                v.freeze()
            }
        }
        camera = c
        // Title scale changes in 10% steps, not every frame.
        if abs(log(max(c.zoom, 0.01) / max(titleZoom, 0.01))) > 0.1 || titleZoom == 0 {
            titleZoom = c.zoom
            for v in views.values { v.setZoom(c.zoom) }
        }
        if let s = selectedID { views[s]?.setSelected(true, zoom: c.zoom) }
    }

    /// Fly so the tile fills the view (and never magnifies its pixels past
    /// 1:1 of the stream by more than 2x).
    func zoomInto(_ id: String) {
        guard let t = layout.tile(id) else { return }
        select(id)
        let rect = t.frame.insetBy(dx: 0, dy: -TileBaseView.headerHeight / 2)
        let target = Camera.fitting(rect, in: scroll.bounds.size, padding: 40, maxZoom: 2.5)
        scroll.fly(to: target) { [weak self] in self?.focus(id) }
    }

    func fitAll() {
        let b = layout.bounds
        guard !b.isNull else { return }
        unfocus()
        scroll.fly(to: Camera.fitting(b.insetBy(dx: 0, dy: -TileBaseView.headerHeight), in: scroll.bounds.size,
                                      padding: 72, maxZoom: 1))
    }

    func setCamera(_ c: Camera) { scroll.setCamera(c) }

    // MARK: Search

    func updateSearch(_ q: String) {
        query = q
        matches = TileSearch.rank(q, layout.tiles.map { ($0.id, $0.title + " " + $0.subtitle) })
        if let first = matches.first { select(first) }
    }

    func nextMatch() {
        guard !matches.isEmpty else { return }
        let i = (matches.firstIndex(of: selectedID ?? "") ?? -1) + 1
        let id = matches[i % matches.count]
        select(id)
        if let t = layout.tile(id), !camera.viewport(in: scroll.bounds.size).intersects(t.frame) {
            scroll.fly(to: Camera(center: CGPoint(x: t.frame.midX, y: t.frame.midY), zoom: camera.zoom))
        }
    }

    func commitSearch() {
        let id = selectedID ?? matches.first
        query = ""
        matches = []
        if let id { zoomInto(id) }
    }

    // MARK: Level of detail

    func visibility(of id: String) -> TileVisibility? {
        guard let t = layout.tile(id) else { return nil }
        let scale = scroll.window?.backingScaleFactor ?? 2
        let size = scroll.bounds.size
        let r = camera.screenRect(forWorld: t.frame, in: size)
        return TileVisibility(
            screenRect: CGRect(x: r.minX * scale, y: r.minY * scale, width: r.width * scale, height: r.height * scale),
            screenBounds: CGRect(x: 0, y: 0, width: size.width * scale, height: size.height * scale),
            focused: focusedID == id)
    }

    func updateLevelOfDetail(now: CFTimeInterval) {
        var tiers: [StreamTier] = []
        for (id, stream) in streams {
            guard let v = visibility(of: id) else { continue }
            var state = lod[id] ?? LODState()
            if !isPresented {
                // The overlay is hidden (and its display link stopped): pause
                // everything now, with no hold.
                if state.tier != .paused {
                    state = LODState(tier: .paused)
                    stream.apply(.paused, keyframe: false)
                }
            } else if case let .apply(tier, keyframe)? = state.update(v, now: now, policy: policy) {
                stream.apply(tier, keyframe: keyframe)
            }
            lod[id] = state
            tiers.append(state.tier)
        }
        let c = LODCensus(tiers)
        if c != census { census = c }
    }

    var isPresented = true

    func tier(of id: String) -> StreamTier? { lod[id]?.tier }

    // MARK: Presence

    /// One SDK `PresenceRoster` per Space: it folds the Space's presence
    /// events into participants (server-assigned colors, cursors) and says
    /// what changed; the canvas only draws.
    private var rosters: [String: PresenceRoster] = [:]
    /// One SDK `PresenceView` per Space, fed the same events: it decides
    /// which cursors are idle (faded) and which participants are stale
    /// (heartbeat stopped, run ended), so no cursor outlives its owner.
    private var presenceViews: [String: PresenceView] = [:]
    private var lastPresenceSweep: CFTimeInterval = 0
    private var lastPresenceExpire: Double = 0

    /// Seed a Space's roster when the canvas joins its presence.
    func presenceJoined(me: CuaSDK.PresenceParticipant, members: [CuaSDK.PresenceMember], space: String) {
        var roster = rosters[space] ?? PresenceRoster()
        let events = roster.apply(me: me, members: members)
        rosters[space] = roster
        let view = PresenceView(me: me.participantId, delayMs: 100)
        view.upsert(participant: me)
        for m in members { view.upsert(participant: m.participant) }
        presenceViews[space] = view
        handle(events, space: space)
    }

    /// One event from `SpacePresence.nextEvent`. One agent run is one
    /// participant: cua-spacesd keys the driver cursor by the run
    /// (`X-Cua-Agent-Session`), not by each MCP session its harness opens.
    func presenceEvent(_ e: CuaSDK.PresenceEvent, space: String, localMs: Double = presenceNowMs()) {
        let view = presenceViews[space] ?? PresenceView(me: rosters[space]?.me?.id ?? "", delayMs: 100)
        presenceViews[space] = view
        _ = view.apply(event: e, localMs: localMs)
        var roster = rosters[space] ?? PresenceRoster()
        let events = roster.apply(event: e)
        rosters[space] = roster
        handle(events, space: space)
    }

    private func handle(_ events: [CuaSpacesStreaming.PresenceEvent], space: String) {
        for e in events {
            switch e {
            case let .participantJoined(p): participantJoined(p, space: space)
            case let .participantLeft(p): participantLeft(p.id, space: space)
            case let .cursorMoved(p):
                if presence.member(p.id, in: space) == nil { participantJoined(p, space: space) }
                if let c = p.cursor { agentCursor(p.id, cursor: c, space: space) }
            case .joined, .rosterChanged: break
            }
        }
    }

    private func participantJoined(_ p: Participant, space: String) {
        let m = CanvasModel.PresenceMember(participantID: p.id, principalID: p.name, name: Self.agentName(p.name),
                                           color: p.color, isAgent: p.isAgent)
        presence.join(m, in: space)
        if m.isAgent {
            let active = threads.values.filter { $0.spaceID == space && $0.isTurnRunning }.map(\.id)
            presence.agentAppeared(m.participantID, in: space, activeThreads: active)
            refreshThreadStyles()
        }
    }

    /// `CUA agent 3f2a` -> `Agent`; the thread's own title names the Space.
    /// People keep their own names.
    static func agentName(_ raw: String) -> String {
        raw.hasPrefix("CUA agent") || raw.isEmpty || raw == "cua-agent" ? "Agent" : raw
    }

    private func participantLeft(_ participantID: String, space: String) {
        if let t = presence.thread(boundTo: participantID, in: space) { returnToDock(threadID: t) }
        presence.leave(participantID, in: space)
        for (id, (s, _)) in windowTiles where s == space {
            (views[id] as? StreamTileView)?.removeCursor(participantID)
        }
    }

    private var lastAction: [String: String] = [:]

    private(set) var cursorEvents = 0
    private(set) var lastCursorDebug = ""

    /// An agent's cursor moved: drawn on the tile of the window it is over.
    private func agentCursor(_ participantID: String, cursor c: CursorState, space: String) {
        guard let member = presence.member(participantID, in: space),
              participantID != rosters[space]?.me?.id else { return }
        let s = spaceLookup?(space)
        let style = presence.cursorStyle(participantID: participantID, in: space)
        let n = c.normalized ?? c.point
        var target: (String, CGPoint)?
        if let w = c.window?.value, !w.isEmpty {
            target = (w, n)
        } else if let s, s.display.width > 0,
                  let hit = WindowPlacement.locate(n, display: s.display,
                                                   windows: s.placements(tiled: tiledWindows(in: space))) {
            target = (hit.windowID, hit.point)
        }
        cursorEvents += 1
        lastCursorDebug = "\(participantID) n=\(n) win=\(c.window?.value ?? "-") target=\(target.map { "\($0.0) \($0.1)" } ?? "none")"
        let action = pendingAction.removeValue(forKey: space)
        // An agent bound to a thread whose cursor is still docked in the
        // thread window's title: detach and glide there first.
        if member.isAgent, let target, c.isVisible,
           let thread = presence.thread(boundTo: participantID, in: space),
           let tile = windowTiles.first(where: { $0.value.space == space && $0.value.window == target.0 })?.key,
           let v = views[tile] as? StreamTileView, v.cursors[participantID] == nil {
            if var t = transits[participantID] {
                t.tile = tile
                t.fraction = target.1
                transits[participantID] = t
                return
            }
            if dockedThreads.contains(thread), let from = dockPoint(threadID: thread) {
                launchTransit(participantID, thread: thread, style: style, from: from, tile: tile, fraction: target.1)
                return
            }
        }
        for (id, (sp, win)) in windowTiles where sp == space {
            guard let v = views[id] as? StreamTileView else { continue }
            if let target, target.0 == win, c.isVisible {
                v.cursor(participantID, style: style, at: target.1, action: member.isAgent ? action : nil,
                         human: !member.isAgent, shape: c.shapeName)
            } else {
                v.removeCursor(participantID)
            }
        }
    }

    /// Fades idle cursors and removes stale ones, from each Space's
    /// `PresenceView` (idle 5 s then a 300 ms fade; gone when a heartbeat
    /// omits them, their heartbeats stop or their run ends).
    func sweepPresence(localMs: Double) {
        let expire = localMs - lastPresenceExpire >= 1_000
        if expire { lastPresenceExpire = localMs }
        for (space, view) in presenceViews {
            if expire { _ = view.expire(localMs: localMs) }
            var alpha: [String: Float] = [:]
            for d in view.drawables(localMs: localMs, pointer: nil) { alpha[d.participantId] = Float(d.alpha) }
            for (id, (s, _)) in windowTiles where s == space {
                guard let v = views[id] as? StreamTileView else { continue }
                for (pid, c) in v.cursors {
                    if view.participant(participantId: pid) == nil {
                        participantLeft(pid, space: space)
                    } else {
                        c.opacity = alpha[pid] ?? 0
                    }
                }
            }
        }
    }

    func windowOf(_ tileID: String) -> (String, String)? {
        windowTiles[tileID].map { ($0.space, $0.window) }
    }

    func tiledWindows(in space: String) -> Set<String> {
        Set(windowTiles.values.filter { $0.space == space }.map(\.window))
    }

    /// A thread's agent called a tool: the next cursor update in its Space
    /// plays the matching theme action.
    private var pendingAction: [String: String] = [:]

    func agentToolCalled(_ title: String, space: String) {
        let t = title.lowercased()
        let action: String
        if t.contains("click") { action = "action_click" }
        else if t.contains("type") || t.contains("text") { action = "action_text" }
        else if t.contains("key") || t.contains("press") { action = "action_key" }
        else if t.contains("scroll") { action = "action_scroll" }
        else if t.contains("drag") { action = "action_drag" }
        else if t.contains("launch") || t.contains("open") { action = "action_app" }
        else if t.contains("screenshot") || t.contains("window") || t.contains("snapshot") { action = "action_observe" }
        else { return }
        pendingAction[space] = action
    }

    func refreshThreadStyles() {
        for (tileID, thread) in threads {
            let s = presence.avatarStyle(threadID: thread.id)
            if threadStyles[tileID]?.style != s {
                threadStyles[tileID]?.style = s
                views[tileID]?.setAppIcon(dockedThreads.contains(thread.id) ? DockGlyph.image(s) : nil)
            }
        }
    }

    func threadStyle(tileID: String) -> AgentStyle? { threadStyles[tileID]?.style }

    // MARK: Agent cursor dock

    /// Threads whose agent cursor sits in the thread window's title.
    private(set) var dockedThreads: Set<String> = []
    struct Transit {
        var layer: AgentCursorLayer
        var thread: String
        var from: CGPoint
        var tile: String?
        var fraction: CGPoint
        var toDock: Bool
        var start: CFTimeInterval
        var path: [HumanPath.Sample]
        var target: CGPoint
    }
    private(set) var transits: [String: Transit] = [:]

    /// The dock glyph's center, in document coordinates.
    func dockPoint(threadID: String) -> CGPoint? {
        guard let tileID = threads.first(where: { $0.value.id == threadID })?.key, let v = views[tileID] else { return nil }
        let k = min(max(1 / max(camera.zoom, 0.01), 1), 3.2)
        let h = TileBaseView.headerHeight
        return CGPoint(x: v.frame.minX + 4 + 9 * k, y: v.frame.minY + h - 5 - 10 * k)
    }

    /// A point inside a stream tile, in document coordinates.
    func docPoint(tile: String, fraction: CGPoint) -> CGPoint? {
        guard let v = views[tile] as? StreamTileView else { return nil }
        let local = v.cursorPoint(for: fraction)
        return CGPoint(x: v.frame.minX + local.x, y: v.frame.minY + TileBaseView.headerHeight + local.y)
    }

    private func launchTransit(_ pid: String, thread: String, style: AgentStyle, from: CGPoint, tile: String?,
                               fraction: CGPoint, toDock: Bool = false) {
        let layer = AgentCursorLayer(style: style, at: from)
        layer.zPosition = 10_000
        scroll.world.layer?.addSublayer(layer)
        dockedThreads.remove(thread)
        if let tv = threads.first(where: { $0.value.id == thread })?.key { views[tv]?.setAppIcon(nil) }
        let to = toDock ? (dockPoint(threadID: thread) ?? from) : (tile.flatMap { docPoint(tile: $0, fraction: fraction) } ?? from)
        let scale = max(hypot(to.x - from.x, to.y - from.y), 1)
        transits[pid] = Transit(layer: layer, thread: thread, from: from, tile: tile, fraction: fraction, toDock: toDock,
                                start: CACurrentMediaTime(),
                                path: HumanPath.samples(from: from, to: to, seed: UInt64(abs(pid.hashValue) % 10_000),
                                                        rate: 120, scale: scale),
                                target: to)
    }

    func stepTransits(now: CFTimeInterval, zoom: CGFloat) {
        let k = min(max(1 / pow(max(zoom, 0.01), 0.7), 0.8), 6)
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        for (pid, t) in transits {
            // Follow a moving target: re-aim the path's end.
            var target = t.target
            if !t.toDock, let tile = t.tile, let p = docPoint(tile: tile, fraction: t.fraction) { target = p }
            if t.toDock, let p = dockPoint(threadID: t.thread) { target = p }
            let elapsed = now - t.start
            let end = t.path.last?.t ?? 0
            let sample = t.path.last { $0.t <= elapsed } ?? t.path.first
            var p = sample?.point ?? target
            // Blend toward the live target as the flight completes.
            let u = CGFloat(min(max(elapsed / max(end, 0.01), 0), 1))
            p = CGPoint(x: p.x + (target.x - t.target.x) * u, y: p.y + (target.y - t.target.y) * u)
            t.layer.position = p
            t.layer.setAffineTransform(CGAffineTransform(scaleX: k, y: k))
            t.layer.updateHeading(movedTo: p, dt: 1.0 / 120)
            if elapsed >= end {
                t.layer.removeFromSuperlayer()
                transits[pid] = nil
                if t.toDock {
                    dockedThreads.insert(t.thread)
                    if let tv = threads.first(where: { $0.value.id == t.thread })?.key {
                        views[tv]?.setAppIcon(DockGlyph.image(t.layer.agentStyle))
                    }
                } else if let tile = t.tile, let v = views[tile] as? StreamTileView {
                    v.cursor(pid, style: t.layer.agentStyle, at: t.fraction, action: "action_click")
                }
            }
        }
        CATransaction.commit()
    }

    /// The thread's turn ended (or its agent left): its cursor glides from
    /// wherever it is back into the thread window's title.
    func returnToDock(threadID: String) {
        guard !dockedThreads.contains(threadID),
              let binding = presence.bindings[threadID] else { return }
        let pid = binding.participantID
        if transits[pid] != nil { return }
        var from: CGPoint?
        var style = presence.avatarStyle(threadID: threadID)
        for (tile, v) in views {
            guard let sv = v as? StreamTileView, let c = sv.cursors[pid] else { continue }
            from = CGPoint(x: sv.frame.minX + c.position.x, y: sv.frame.minY + TileBaseView.headerHeight + c.position.y)
            style = c.agentStyle
            sv.removeCursor(pid)
            _ = tile
        }
        guard let from else {
            dockedThreads.insert(threadID)
            if let tv = threads.first(where: { $0.value.id == threadID })?.key { views[tv]?.setAppIcon(DockGlyph.image(style)) }
            return
        }
        launchTransit(pid, thread: threadID, style: style, from: from, tile: nil, fraction: .zero, toDock: true)
    }

    // MARK: Teardown

    func stopAll() async {
        stopDisplayLink()
        for s in streams.values { await s.stop() }
        for t in threads.values { await t.stop() }
    }
}

extension PresenceColorSource {
    /// The cua SDK's presence colors (Rust `presence::color_for` /
    /// `text_color_on`, through `PresenceColors`).
    static let sdk = PresenceColorSource(stable: { PresenceColors.color(for: $0) },
                                         textOn: { PresenceColors.textColor(on: $0) })
}
