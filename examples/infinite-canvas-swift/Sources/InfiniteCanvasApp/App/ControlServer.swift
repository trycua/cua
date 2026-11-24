// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CanvasModel
import CanvasStreaming
import Foundation

/// A local control socket for scripted runs (the recording and the perf
/// sweep). Newline-delimited JSON commands in, one JSON line back.
///
/// It drives the same code paths as the keyboard and pointer: `hotkey` calls
/// what the global hotkey calls, `type` edits a thread's draft one character
/// at a time, `send` presses the send button's action. It never synthesizes
/// system input events, so a scripted run cannot touch any other app. The
/// socket is created 0600 in a directory you choose.
final class ControlServer: @unchecked Sendable {
    let path: String
    private weak var app: CanvasApp?
    private var fd: Int32 = -1
    private var running = false

    init(path: String, app: CanvasApp) {
        self.path = path
        self.app = app
    }

    func start() {
        unlink(path)
        fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { return }
        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)
        let bytes = Array(path.utf8.prefix(103)) + [0]
        withUnsafeMutableBytes(of: &addr.sun_path) { raw in
            for (i, b) in bytes.enumerated() { raw[i] = b }
        }
        let ok = withUnsafePointer(to: &addr) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { bind(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) }
        }
        guard ok == 0 else { close(fd); return }
        chmod(path, 0o600)
        listen(fd, 4)
        running = true
        Thread.detachNewThread { [self] in acceptLoop() }
    }

    func stop() {
        running = false
        if fd >= 0 { close(fd) }
        unlink(path)
    }

    private func acceptLoop() {
        while running {
            let c = accept(fd, nil, nil)
            guard c >= 0 else { continue }
            Thread.detachNewThread { [self] in serve(c) }
        }
    }

    private func serve(_ c: Int32) {
        defer { close(c) }
        var buffer = Data()
        var chunk = [UInt8](repeating: 0, count: 4096)
        while running {
            let n = read(c, &chunk, chunk.count)
            guard n > 0 else { return }
            buffer.append(contentsOf: chunk[0 ..< n])
            // Bounded: one command line is never more than 1 MiB.
            if buffer.count > 1 << 20 { return }
            while let nl = buffer.firstIndex(of: 0x0A) {
                let line = buffer[buffer.startIndex ..< nl]
                buffer.removeSubrange(buffer.startIndex ... nl)
                let reply = handle(Data(line))
                var out = (try? JSONSerialization.data(withJSONObject: reply)) ?? Data("{}".utf8)
                out.append(0x0A)
                _ = out.withUnsafeBytes { write(c, $0.baseAddress, out.count) }
            }
        }
    }

    private func handle(_ line: Data) -> [String: Any] {
        guard let cmd = (try? JSONSerialization.jsonObject(with: line)) as? [String: Any],
              let name = cmd["cmd"] as? String else { return ["ok": false, "error": "bad json"] }
        let sem = DispatchSemaphore(value: 0)
        nonisolated(unsafe) var reply: [String: Any] = ["ok": false]
        nonisolated(unsafe) let args = cmd
        DispatchQueue.main.async { [weak self] in
            MainActor.assumeIsolated {
                if let app = self?.app {
                    Task { @MainActor in
                        reply = await Commands.run(name, args, app: app)
                        sem.signal()
                    }
                } else {
                    sem.signal()
                }
            }
        }
        // Perf runs can take a while; everything else is quick.
        _ = sem.wait(timeout: .now() + 600)
        return reply
    }
}

@MainActor
enum Commands {
    static func tileID(_ args: [String: Any], _ canvas: CanvasController) -> String? {
        if let id = args["tile"] as? String, canvas.layout.tile(id) != nil { return id }
        if let t = (args["title"] as? String)?.lowercased() {
            return canvas.layout.tiles.first { $0.title.lowercased().contains(t) }?.id
        }
        return nil
    }

    static func run(_ name: String, _ a: [String: Any], app: CanvasApp) async -> [String: Any] {
        let canvas = app.canvas
        switch name {
        case "hotkey":
            app.hotkeyPressed()
            return ["ok": true, "phase": "\(app.overlay.phase)"]
        case "fit":
            canvas.fitAll()
            return ["ok": true]
        case "zoomInto":
            guard let id = tileID(a, canvas) else { return ["ok": false, "error": "no tile"] }
            canvas.zoomInto(id)
            return ["ok": true, "tile": id]
        case "select":
            guard let id = tileID(a, canvas) else { return ["ok": false, "error": "no tile"] }
            canvas.select(id)
            return ["ok": true, "tile": id]
        case "focus":
            guard let id = tileID(a, canvas) else { return ["ok": false, "error": "no tile"] }
            canvas.focus(id)
            return ["ok": true, "tile": id]
        case "unfocus":
            canvas.unfocus()
            return ["ok": true]
        case "camera":
            let c = Camera(center: CGPoint(x: a["x"] as? Double ?? canvas.camera.center.x,
                                           y: a["y"] as? Double ?? canvas.camera.center.y),
                           zoom: a["zoom"] as? Double ?? canvas.camera.zoom)
            if a["fly"] as? Bool == true { canvas.scroll.fly(to: c) } else { canvas.setCamera(c) }
            return ["ok": true]
        case "flyToRect":
            guard let id = tileID(a, canvas), let t = canvas.layout.tile(id) else { return ["ok": false] }
            let pad = a["padding"] as? Double ?? 120
            canvas.scroll.fly(to: Camera.fitting(t.frame, in: canvas.scroll.bounds.size, padding: pad,
                                                 maxZoom: a["maxZoom"] as? Double ?? 1.5))
            return ["ok": true]
        case "flyToTiles":
            // {"titles": ["Terminal", "Agent"], "padding": 60}
            let rects = (a["titles"] as? [String] ?? []).compactMap { t in
                tileID(["title": t], canvas).flatMap { canvas.layout.tile($0)?.frame }
            }
            guard let first = rects.first else { return ["ok": false] }
            let union = rects.dropFirst().reduce(first) { $0.union($1) }
                .insetBy(dx: 0, dy: -TileBaseView.headerHeight)
            canvas.scroll.fly(to: Camera.fitting(union, in: canvas.scroll.bounds.size,
                                                 padding: a["padding"] as? Double ?? 60,
                                                 maxZoom: a["maxZoom"] as? Double ?? 1.2))
            return ["ok": true]
        case "place":
            // {"tile" or "title", "x", "y", "w", "h" (threads)}: a scripted layout.
            guard let id = tileID(a, canvas), let t = canvas.layout.tile(id) else { return ["ok": false] }
            let w = a["w"] as? Double ?? t.frame.width
            let h = a["h"] as? Double ?? (t.kind.isStream ? w / t.aspect : t.frame.height)
            canvas.tileResize(id, toWorld: CGSize(width: w, height: h))
            let cur = canvas.layout.tile(id)!.frame
            canvas.tileMove(id, byWorld: CGVector(dx: (a["x"] as? Double ?? cur.minX) - cur.minX,
                                                  dy: (a["y"] as? Double ?? cur.minY) - cur.minY))
            let f = canvas.layout.tile(id)!.frame
            return ["ok": true, "tile": id, "x": f.minX, "y": f.minY, "w": f.width, "h": f.height]
        case "move":
            guard let id = tileID(a, canvas) else { return ["ok": false] }
            canvas.tileMove(id, byWorld: CGVector(dx: a["dx"] as? Double ?? 0, dy: a["dy"] as? Double ?? 0))
            return ["ok": true]
        case "search":
            let text = a["text"] as? String ?? ""
            let cps = a["cps"] as? Double ?? 12
            for ch in text {
                canvas.updateSearch(canvas.query + String(ch))
                try? await Task.sleep(for: .seconds(1 / cps))
            }
            return ["ok": true, "matches": canvas.matches]
        case "key":
            switch a["name"] as? String {
            case "return": if !canvas.query.isEmpty { canvas.commitSearch() } else if let s = canvas.selectedID { canvas.zoomInto(s) }
            case "tab": canvas.nextMatch()
            case "escape": if canvas.focusedID != nil { canvas.unfocus() } else { canvas.updateSearch("") }
            default: return ["ok": false]
            }
            return ["ok": true]
        case "type":
            guard let id = tileID(a, canvas), let thread = canvas.threads[id] else { return ["ok": false, "error": "no thread"] }
            canvas.focus(id)
            let cps = a["cps"] as? Double ?? 14
            for ch in (a["text"] as? String ?? "") {
                thread.draft.append(ch)
                try? await Task.sleep(for: .seconds(1 / cps))
            }
            return ["ok": true]
        case "send":
            guard let id = tileID(a, canvas), let thread = canvas.threads[id] else { return ["ok": false] }
            thread.send()
            return ["ok": true]
        case "state":
            return state(canvas, app: app)
        case "perf":
            return await PerfRun.run(canvas: canvas, seconds: a["seconds"] as? Double ?? 20,
                                     out: a["out"] as? String, label: a["label"] as? String ?? "")
        case "coworker":
            // A person on the Space: {"title": "Untitled", "x": 0.3, "y": 0.3,
            // "text": "…", "name": "Maya", "color": "#f58231"}
            guard let id = tileID(a, canvas), let (spaceID, windowID) = canvas.windowOf(id),
                  let s = canvas.spaceLookup?(spaceID) else { return ["ok": false, "error": "no window tile"] }
            let worker = Coworker(space: s.space, name: a["name"] as? String ?? "Maya",
                                  color: a["color"] as? String ?? "#f58231")
            let click = CGPoint(x: a["x"] as? Double ?? 0.3, y: a["y"] as? Double ?? 0.3)
            let text = a["text"] as? String ?? "hi, i'm your coworker's cursor!"
            Task { @MainActor in
                do { try await worker.run(windowID: windowID, click: click, text: text) } catch {
                    NSLog("infinite-canvas: coworker: %@", "\(error)")
                }
            }
            return ["ok": true, "tile": id]
        case "windowID":
            return ["ok": true, "windowNumber": app.window.windowNumber]
        case "quit":
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) { NSApp.terminate(nil) }
            return ["ok": true]
        default:
            return ["ok": false, "error": "unknown command \(name)"]
        }
    }

    static func state(_ canvas: CanvasController, app: CanvasApp) -> [String: Any] {
        var tiles: [[String: Any]] = []
        for t in canvas.layout.tiles {
            var d: [String: Any] = ["id": t.id, "title": t.title, "x": t.frame.minX, "y": t.frame.minY,
                                    "w": t.frame.width, "h": t.frame.height]
            if let tier = canvas.tier(of: t.id) { d["tier"] = tier.description }
            if let s = canvas.streams[t.id] {
                let c = s.decoder.counters.snapshot()
                d["decoded"] = c.decoded
                d["received"] = c.received
                d["hardware"] = c.hardware as Any
            }
            if let thread = canvas.threads[t.id] {
                d["messages"] = thread.messages.count
                d["turn"] = thread.isTurnRunning
                if let s = canvas.threadStyle(tileID: t.id) {
                    d["bubble"] = String(format: "#%02X%02X%02X", Int(s.fill.r * 255), Int(s.fill.g * 255), Int(s.fill.b * 255))
                }
            }
            tiles.append(d)
        }
        var agents: [[String: Any]] = []
        for (space, members) in canvas.presence.members {
            for m in members.values {
                agents.append(["space": space, "participant": m.participantID, "color": m.color, "name": m.name,
                               "agent": m.isAgent, "principal": m.principalID,
                               "thread": canvas.presence.thread(boundTo: m.participantID, in: space) as Any])
            }
        }
        var spaces: [[String: Any]] = []
        for id in Set(canvas.tileMeta.values.map(\.space)) {
            if let sp = canvas.spaceLookup?(id) {
                spaces.append(["id": id, "display": "\(sp.display)", "windows": sp.windows.count,
                               "placements": sp.placements(tiled: canvas.tiledWindows(in: id)).map { "\($0.windowID) \($0.bounds)" }])
            }
        }
        let c = canvas.camera
        return ["ok": true, "spaces": spaces, "cursorEvents": canvas.cursorEvents, "lastCursor": canvas.lastCursorDebug, "phase": "\(app.overlay.phase)", "zoom": c.zoom, "x": c.center.x, "y": c.center.y,
                "tiles": tiles, "agents": agents, "focused": canvas.focusedID as Any]
    }
}
