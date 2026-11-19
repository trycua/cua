// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import Carbon.HIToolbox
import CanvasModel
import CanvasStreaming
import Cua
import SwiftUI

/// Wires the window, the hotkey, the Spaces and the canvas together.
@MainActor
final class CanvasApp: NSObject, NSApplicationDelegate {
    let options: Options
    let canvas = CanvasController()
    private(set) var window: CanvasWindow!
    private var hud: HUDLayer!
    private var hotkey: GlobalHotkey?
    private(set) var overlay = OverlayState()
    private var hub: SpacesHub?
    private var control: ControlServer?
    private var restingCamera: Camera?
    private let registryHome: String

    init(options: Options) {
        self.options = options
        registryHome = options.registry
            ?? (NSTemporaryDirectory() as NSString).appendingPathComponent("infinite-canvas-\(ProcessInfo.processInfo.processIdentifier)")
        super.init()
    }

    func applicationDidFinishLaunching(_ note: Notification) {
        ProcessMetrics.captureMainThread()
        buildWindow()
        hotkey = GlobalHotkey(options.hotkey) { [weak self] in self?.hotkeyPressed() }
        if hotkey == nil { NSLog("infinite-canvas: could not register %@", options.hotkey.description) }
        if let path = options.controlSocket {
            control = ControlServer(path: path, app: self)
            control?.start()
        }
        if options.synthetic > 0 { addSyntheticTiles(options.synthetic) }
        if let file = options.spacesFile {
            Task { await attachSpaces(file) }
        }
        canvas.startDisplayLink()
        if options.showAtLaunch { hotkeyPressed() } else { canvas.isPresented = false }
    }

    func applicationWillTerminate(_ note: Notification) {
        control?.stop()
    }

    // MARK: Window

    private func buildWindow() {
        let screen = NSScreen.main ?? NSScreen.screens[0]
        if let size = options.windowed {
            window = options.chromeless ? CanvasWindow.chromeless(size: size, on: screen) : CanvasWindow.windowed(size: size, on: screen)
        } else {
            window = CanvasWindow.overlay(on: screen)
        }
        let content = NSView(frame: window.contentLayoutRect)
        content.wantsLayer = true
        content.layer?.backgroundColor = Palette.canvas.cgColor
        canvas.scroll.frame = content.bounds
        canvas.scroll.autoresizingMask = [.width, .height]
        content.addSubview(canvas.scroll)
        window.contentView = content
        hud = HUDLayer(in: content, canvas: canvas, hotkey: options.hotkey.description, showPerf: options.perfHUD)
        window.keyHandler = { [weak self] e in self?.handleKey(e) ?? false }
        window.alphaValue = 0
        canvas.setCamera(Camera(center: .zero, zoom: 0.5))
    }

    // MARK: Hotkey and transitions

    func hotkeyPressed() {
        switch overlay.hotkey(at: ProcessInfo.processInfo.systemUptime) {
        case .present?: present()
        case .dismiss?: dismiss()
        case nil: break
        }
    }

    /// Fade in while the camera settles out from a slight zoom: the canvas
    /// arrives, rather than appears.
    private func present() {
        canvas.isPresented = true
        if options.activate { NSApp.activate() }
        window.makeKeyAndOrderFront(nil)
        let rest = restingCamera ?? canvas.scroll.flightTarget ?? canvas.camera
        canvas.setCamera(Camera(center: rest.center, zoom: rest.zoom * 1.12))
        canvas.scroll.fly(to: rest)
        NSAnimationContext.runAnimationGroup({ ctx in
            ctx.duration = 0.28
            ctx.timingFunction = CAMediaTimingFunction(name: .easeOut)
            window.animator().alphaValue = 1
        }, completionHandler: { [weak self] in
            MainActor.assumeIsolated { self?.overlay.transitionFinished() }
        })
        canvas.updateLevelOfDetail(now: CACurrentMediaTime())
    }

    private func dismiss() {
        restingCamera = canvas.camera
        canvas.unfocus()
        let c = canvas.camera
        canvas.scroll.fly(to: Camera(center: c.center, zoom: c.zoom * 1.08))
        NSAnimationContext.runAnimationGroup({ ctx in
            ctx.duration = 0.22
            ctx.timingFunction = CAMediaTimingFunction(name: .easeIn)
            window.animator().alphaValue = 0
        }, completionHandler: { [weak self] in
            MainActor.assumeIsolated {
                guard let self else { return }
                self.overlay.transitionFinished()
                if self.overlay.phase == .hidden {
                    self.window.orderOut(nil)
                    self.canvas.scroll.cancelFlight()
                    if let r = self.restingCamera { self.canvas.setCamera(r) }
                    self.canvas.isPresented = false
                    self.canvas.updateLevelOfDetail(now: CACurrentMediaTime())
                }
            }
        })
    }

    // MARK: Keys (no tile focused)

    func handleKey(_ e: NSEvent) -> Bool {
        let cmd = e.modifierFlags.contains(.command)
        if cmd, e.charactersIgnoringModifiers == "0" { canvas.fitAll(); return true }
        if cmd { return false }
        switch Int(e.keyCode) {
        case kVK_Escape:
            if !canvas.query.isEmpty { canvas.updateSearch(""); return true }
            if canvas.focusedID != nil { canvas.unfocus(); return true }
            if overlay.escape() == .dismiss { dismiss() }
            return true
        case kVK_Return, kVK_ANSI_KeypadEnter:
            if !canvas.query.isEmpty { canvas.commitSearch() }
            else if let s = canvas.selectedID { canvas.zoomInto(s) }
            return true
        case kVK_Tab:
            canvas.nextMatch()
            return true
        case kVK_Delete:
            if !canvas.query.isEmpty { canvas.updateSearch(String(canvas.query.dropLast())) }
            return true
        default:
            if let c = e.characters, !c.isEmpty, c.unicodeScalars.allSatisfy({ $0.value >= 0x20 && $0.value < 0xF700 }) {
                canvas.updateSearch(canvas.query + c)
                return true
            }
            return false
        }
    }

    // MARK: Spaces

    private func attachSpaces(_ file: String) async {
        let config: SpacesConfig
        do {
            config = try SpacesConfig.load(file)
            hub = try SpacesHub(registryHome: registryHome)
        } catch {
            NSLog("infinite-canvas: %@", "\(error)")
            return
        }
        guard let hub else { return }
        canvas.spaceLookup = { [weak hub] id in hub?.spaces.first { $0.id == id } }
        var groups: [[String]] = []
        var threadIDs: [String] = []
        for entry in config.spaces {
            do {
                let s = try await hub.attach(entry)
                var group: [String] = []
                await hub.refreshWindows(s)
                let picked = SpacesHub.pickWindows(s.windows, prefer: entry.apps ?? [], max: entry.maxWindows ?? 3)
                if entry.desktop == true || picked.isEmpty {
                    let id = "\(s.id)#desktop"
                    let stream = TileStream(id: id, source: SpaceMediaSource(space: s.space, windowID: nil))
                    let size = s.display == .zero ? CGSize(width: 1280, height: 800) : s.display
                    canvas.addStream(Tile(id: id, kind: .desktop(spaceID: s.id), title: "Desktop",
                                          subtitle: entry.os.label, frame: CGRect(origin: .zero, size: size)),
                                     stream: stream, windowOf: nil)
                    group.append(id)
                    canvas.decorate(id, os: entry.os, icon: nil)
                    Task { try? await stream.start(initial: self.canvas.isPresented ? .medium : .paused) }
                }
                canvas.spaceInfo[s.id] = (entry.label, s.id, entry.os)
                if let caps = try? await s.spacesd.capabilities() {
                    canvas.osLabels[s.id] = HoverInfo.osLabel(kind: entry.os, name: caps.osName, version: caps.osVersion)
                } else {
                    canvas.osLabels[s.id] = entry.os.label
                }
                let icons = AppIcons(fetch: AppIcons.spaceFetch(space: s.space))
                var iconTiles: [(String, AppIconKey)] = []
                for w in picked {
                    let id = "\(s.id)#\(w.windowId)"
                    let stream = TileStream(id: id, source: SpaceMediaSource(space: s.space, windowID: w.windowId))
                    let b = w.bounds.count == 4 ? CGSize(width: w.bounds[2], height: w.bounds[3]) : CGSize(width: 960, height: 600)
                    // The icon names the app and the mark names the OS, so the
                    // title is the window's own (the app name when untitled).
                    let title = w.title.isEmpty ? w.appName : w.title
                    canvas.addStream(Tile(id: id, kind: .window(spaceID: s.id, windowID: w.windowId),
                                          title: title, subtitle: "\(w.appName) \(entry.label) \(entry.os.label)",
                                          frame: CGRect(origin: .zero, size: b)),
                                     stream: stream, windowOf: (s.id, w.windowId))
                    group.append(id)
                    canvas.decorate(id, os: entry.os, icon: nil)
                    canvas.tileMeta[id] = (w.appName, w.title.isEmpty ? w.appName : w.title, s.id)
                    iconTiles.append((id, AppIconKey(spaceID: s.id, os: entry.os, app: w.appName, appID: w.appId, pid: w.pid)))
                    Task {
                        // A slow Space (an emulated VM) can miss the first
                        // open; retry with backoff before showing the error.
                        for attempt in 1 ... 4 {
                            do {
                                try await stream.start(initial: self.canvas.isPresented ? .medium : .paused)
                                (self.canvas.views[id] as? StreamTileView)?.setError(nil)
                                return
                            } catch {
                                (self.canvas.views[id] as? StreamTileView)?.setError("\(error)")
                                try? await Task.sleep(for: .seconds(Double(attempt) * 5))
                            }
                        }
                    }
                }
                // Every tile's app icon in one SDK call.
                let os = entry.os
                Task { [weak self] in
                    let images = await icons.icons(for: iconTiles.map(\.1))
                    for ((id, _), image) in zip(iconTiles, images) {
                        self?.canvas.decorate(id, os: os, icon: image)
                    }
                }
                if (config.threads ?? []).contains(entry.label), let agent = entry.agent {
                    let tid = "thread:\(entry.label)"
                    let spacesd = s.spacesd
                    let thread = AgentThread(
                        id: tid, spaceID: s.id, spaceLabel: entry.label,
                        launch: AgentLaunch(harness: agent.harness, baseURL: agent.baseURL, model: agent.model,
                                            env: agent.env ?? [:]),
                        agents: { try await spacesd.agents() })
                    thread.onToolCall = { [weak self] title in self?.canvas.agentToolCalled(title, space: s.id) }
                    canvas.addThread(Tile(id: tid, kind: .thread(threadID: tid), title: thread.title,
                                          subtitle: "agent", frame: CGRect(x: 0, y: 0, width: 420, height: 560)),
                                     thread: thread)
                    // The thread sits with its Space's windows, so its agent's
                    // cursor has a short way to go.
                    group.append(tid)
                }
                groups.append(group)
                await hub.joinPresence(s, onEvent: { [weak self] e in self?.canvas.presenceEvent(e, space: s.id) },
                                       onJoined: { [weak self] me, m in self?.canvas.presenceJoined(me: me, members: m, space: s.id) })
                // Windows move; keep the cursor mapping current.
                Task { [weak self] in
                    for _ in 0 ..< 100_000 {
                        try? await Task.sleep(for: .seconds(2))
                        guard let self, let hub = self.hub else { return }
                        await hub.refreshWindows(s)
                    }
                }
            } catch {
                NSLog("infinite-canvas: could not attach %@: %@", entry.label, "\(error)")
            }
        }
        if !threadIDs.isEmpty { groups.append(threadIDs) }
        arrange(groups)
        // Streams report their real sizes as their first frames decode; lay
        // out again once they have, so no two tiles overlap.
        Task { [weak self] in
            try? await Task.sleep(for: .seconds(6))
            self?.arrange(groups)
        }
    }

    func arrange(_ groups: [[String]]) {
        // Pick the group width whose overall shape best matches the view,
        // so "fit all" fills the screen instead of a thin strip.
        let view = canvas.scroll.bounds.size
        let target = view.height > 0 ? view.width / view.height : 16 / 9
        var best: CanvasLayout?
        var bestScore = CGFloat.infinity
        for width in stride(from: CGFloat(700), through: 2600, by: 100) {
            var candidate = canvas.layout
            candidate.arrange(groups: groups, origin: .zero, groupWidth: width)
            let b = candidate.bounds
            guard b.height > 0 else { continue }
            let score = abs(log((b.width / b.height) / target))
            if score < bestScore { bestScore = score; best = candidate }
        }
        var layout = best ?? canvas.layout
        if best == nil { layout.arrange(groups: groups, origin: .zero) }
        for t in layout.tiles {
            if let cur = canvas.layout.tile(t.id), cur.frame != t.frame {
                canvas.tileMove(t.id, byWorld: CGVector(dx: t.frame.minX - cur.frame.minX, dy: t.frame.minY - cur.frame.minY))
                canvas.tileResize(t.id, toWorld: t.frame.size)
            }
        }
        canvas.fitAll()
    }

    // MARK: Synthetic tiles (benchmark and tests only)

    func addSyntheticTiles(_ n: Int) {
        var ids: [String] = []
        for i in 0 ..< n {
            let id = "synthetic-\(i)"
            let size = i % 3 == 0 ? CGSize(width: 1440, height: 900) : CGSize(width: 1280, height: 800)
            let stream = TileStream(id: id, source: SyntheticMediaSource(size: size, seed: i))
            canvas.addStream(Tile(id: id, kind: .window(spaceID: "synthetic", windowID: "\(i)"),
                                  title: "Test pattern \(i + 1)", subtitle: "synthetic",
                                  frame: CGRect(origin: .zero, size: size), sourcePixels: size),
                             stream: stream, windowOf: nil)
            ids.append(id)
            Task { try? await stream.start(initial: self.canvas.isPresented ? .medium : .paused) }
        }
        arrange([ids])
    }
}
