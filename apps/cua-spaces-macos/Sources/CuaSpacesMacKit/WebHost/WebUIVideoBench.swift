// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CoreImage
import CuaSpacesStreaming
import WebKit

/// The native video harness's hooks (debug builds; docs/native-video.md):
///
/// - `CUA_SPACES_VIDEO_BENCH=<spaceId>`: beside the New UI window (the
///   Spaces grid), a second web UI window on that Space's viewer, so a grid
///   of tiles and one full viewer run at once.
/// - `CUA_SPACES_VIDEO_STATS=<file>`: once a second, every stream's decoded
///   and presented frame counts, as JSON.
/// - `CUA_SPACES_VIDEO_SHOT=<dir>`: after `CUA_SPACES_VIDEO_SHOT_AFTER`
///   seconds (default 8), each window's composite (the page's snapshot with
///   the native video drawn where it is) as PNG.
extension WebUIWindowController {
    nonisolated(unsafe) static var bench: WebUIWindowController?

    static func startVideoHarness(model: AppModel) {
        let env = DevHooks.environment
        if let id = env["CUA_SPACES_VIDEO_BENCH"], !id.isEmpty, bench == nil {
            let c = WebUIWindowController(model: model, route: "spaces/\(id)")
            bench = c
            c.load()
            if let main = shared?.window, let w = c.window {
                main.setFrame(NSRect(x: 40, y: 80, width: 1240, height: 860), display: true)
                w.setFrame(NSRect(x: 1300, y: 80, width: 900, height: 700), display: true)
            }
        }
        if let file = env["CUA_SPACES_VIDEO_STATS"], !file.isEmpty {
            Task { @MainActor in
                let start = Date()
                while true {
                    try? await Task.sleep(for: .seconds(1))
                    let windows = [shared, bench].compactMap { $0 }
                    let streams = windows.flatMap { $0.videoSurfaces?.stats() ?? [] }
                    let out: [String: Any] = ["t": Date().timeIntervalSince(start), "streams": streams,
                                              "inputBatches": SyntheticMedia.totalInputBatches]
                    if let data = try? JSONSerialization.data(withJSONObject: out, options: [.sortedKeys]) {
                        try? data.write(to: URL(fileURLWithPath: file))
                    }
                }
            }
        }
        if let file = env["CUA_SPACES_VIDEO_CHECK"], !file.isEmpty {
            Task { @MainActor in
                let result = await runVideoCheck(shots: env["CUA_SPACES_VIDEO_SHOT"])
                if let data = try? JSONSerialization.data(withJSONObject: result, options: [.prettyPrinted, .sortedKeys]) {
                    try? data.write(to: URL(fileURLWithPath: file))
                }
                NSApp.terminate(nil)
            }
            return
        }
        if let dir = env["CUA_SPACES_VIDEO_SHOT"], !dir.isEmpty {
            let after = env["CUA_SPACES_VIDEO_SHOT_AFTER"].flatMap(Double.init) ?? 8
            Task { @MainActor in
                try? await Task.sleep(for: .seconds(after))
                let url = URL(fileURLWithPath: dir, isDirectory: true)
                try? FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
                for (name, c) in [("grid", shared), ("viewer", bench)] {
                    guard let c, let png = await c.compositeSnapshot() else { continue }
                    try? png.write(to: url.appendingPathComponent("native-video-\(name).png"))
                }
            }
        }
    }

    /// `CUA_SPACES_VIDEO_CHECK=<file>`: drives the grid and the viewer the
    /// way a person would and writes what happened: every slot goes live;
    /// a dialog over the grid hides the tiles and closing it shows them; a
    /// click on the viewer gives it the keyboard (the page hears it), the
    /// click and a key reach the Space, Control+Option pressed and released
    /// alone gives the keyboard back.
    static func runVideoCheck(shots: String?) async -> [String: Any] {
        var out: [String: Any] = [:]
        func wait(_ what: () -> Bool, seconds: Double = 30) async -> Bool {
            let end = Date().addingTimeInterval(seconds)
            while Date() < end {
                if what() { return true }
                try? await Task.sleep(for: .milliseconds(200))
            }
            return what()
        }
        func shot(_ c: WebUIWindowController?, _ name: String) async {
            guard let shots, let c, let png = await c.compositeSnapshot() else { return }
            try? png.write(to: URL(fileURLWithPath: shots).appendingPathComponent("\(name).png"))
        }
        if let shots { try? FileManager.default.createDirectory(atPath: shots, withIntermediateDirectories: true) }
        guard let grid = shared?.videoSurfaces, let gridWindow = shared else { return ["error": "no grid surfaces"] }
        let live: (WebUIVideoSurfaces) -> Int = { s in s.surfaces.values.filter { $0.phase == .live }.count }
        let shown: (WebUIVideoSurfaces) -> Int = { s in s.surfaces.values.filter { !$0.view.isHidden }.count }
        out["gridLive"] = await wait({ live(grid) >= 9 })
        out["gridSurfaces"] = grid.surfaces.count
        out["gridShownBefore"] = shown(grid)
        await shot(gridWindow, "check-grid")
        // A dialog (New Space) over the grid.
        _ = try? await gridWindow.webView.evaluateJavaScript(
            "[...document.querySelectorAll('button')].find(b => b.textContent.includes('New Space'))?.click(); true")
        out["gridHiddenUnderDialog"] = await wait({ shown(grid) == 0 }, seconds: 5)
        await shot(gridWindow, "check-grid-dialog")
        _ = try? await gridWindow.webView.evaluateJavaScript(
            "[...document.querySelectorAll('button')].find(b => b.textContent.trim() === 'Cancel')?.click(); true")
        let shownBefore = out["gridShownBefore"] as? Int ?? 1
        out["gridShownAfterDialog"] = await wait({ shown(grid) >= shownBefore }, seconds: 5)

        guard let bench, let viewer = bench.videoSurfaces, let window = bench.window else { return out }
        out["viewerLive"] = await wait({ live(viewer) >= 1 })
        guard let surface = viewer.surfaces.values.first(where: { $0.update.interactive }) else { return out }
        window.makeKeyAndOrderFront(nil)
        let before = SyntheticMedia.totalInputBatches
        let input = surface.view.input
        let center = input.convert(CGPoint(x: input.bounds.midX, y: input.bounds.midY), to: nil)
        for type in [NSEvent.EventType.leftMouseDown, .leftMouseUp] {
            if let e = NSEvent.mouseEvent(with: type, location: center, modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime,
                                          windowNumber: window.windowNumber, context: nil, eventNumber: 0, clickCount: 1, pressure: 1) {
                window.sendEvent(e)
            }
        }
        out["viewerFocusedAfterClick"] = viewer.focusedId == surface.id
        out["clickReachedSpace"] = await wait({ SyntheticMedia.totalInputBatches > before }, seconds: 3)
        try? await Task.sleep(for: .milliseconds(500))
        let pageFocus = try? await bench.webView.evaluateJavaScript(
            "document.querySelector('[data-video-focus]')?.dataset.videoFocus ?? null")
        out["pageHeardFocus"] = (pageFocus as? String) == "space"
        await shot(bench, "check-viewer-focused")
        let beforeKey = SyntheticMedia.totalInputBatches
        func key(_ chars: String, code: UInt16, flags: NSEvent.ModifierFlags) -> NSEvent? {
            NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: flags, timestamp: ProcessInfo.processInfo.systemUptime,
                             windowNumber: window.windowNumber, context: nil, characters: chars,
                             charactersIgnoringModifiers: chars, isARepeat: false, keyCode: code)
        }
        if let a = key("a", code: 0, flags: []) { window.sendEvent(a) }
        out["keyReachedSpace"] = await wait({ SyntheticMedia.totalInputBatches > beforeKey }, seconds: 3)
        // ⌘C through the key monitor: on a Linux fixture it arrives as Ctrl+C.
        let beforeChord = SyntheticMedia.totalInputBatches
        if let copy = key("c", code: 8, flags: .command) { out["cmdCHandled"] = viewer.handleKey(copy) }
        if await wait({ SyntheticMedia.totalInputBatches > beforeChord }, seconds: 3),
           let first = SyntheticMedia.events(in: SyntheticMedia.lastInput).first {
            let modifiers = (first["modifiers"] as? [String] ?? []).joined(separator: "+")
            out["cmdCArrivedAs"] = "\(modifiers)+\(first["key"] ?? "")"
        }
        // Control+Option pressed and released alone, through the view's key capture.
        for flags in [NSEvent.ModifierFlags.control, [.control, .option], .option, []] {
            if let e = NSEvent.keyEvent(with: .flagsChanged, location: .zero, modifierFlags: flags,
                                        timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: window.windowNumber,
                                        context: nil, characters: "", charactersIgnoringModifiers: "", isARepeat: false,
                                        keyCode: 59) { _ = input.capture(e) }
        }
        out["viewerFocusedAfterRelease"] = viewer.focusedId != nil
        try? await Task.sleep(for: .milliseconds(500))
        let after = try? await bench.webView.evaluateJavaScript(
            "document.querySelector('[data-video-focus]')?.dataset.videoFocus ?? null")
        out["pageFocusAfterRelease"] = after as? String ?? NSNull()
        return out
    }

    /// The page's own snapshot with each live surface's current frame drawn
    /// at its place (a window capture from outside misses WKWebView's
    /// content, and the web snapshot misses the native views).
    func compositeSnapshot() async -> Data? {
        guard let page = try? await webView.takeSnapshot(configuration: nil),
              let pageCG = page.cgImage(forProposedRect: nil, context: nil, hints: nil) else { return nil }
        let scale = CGFloat(pageCG.width) / max(1, webView.bounds.width)
        let width = pageCG.width, height = pageCG.height
        guard let ctx = CGContext(data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0,
                                  space: CGColorSpace(name: CGColorSpace.sRGB)!,
                                  bitmapInfo: CGImageAlphaInfo.premultipliedFirst.rawValue) else { return nil }
        ctx.draw(pageCG, in: CGRect(x: 0, y: 0, width: width, height: height))
        let ci = CIContext()
        for frame in videoSurfaces?.visibleFrames() ?? [] {
            guard let image = ci.createCGImage(CIImage(cvPixelBuffer: frame.buffer),
                                               from: CIImage(cvPixelBuffer: frame.buffer).extent) else { continue }
            // Web view (flipped, points) -> bitmap (bottom-left, pixels).
            let clip = frame.clip
            let content = frame.content
            ctx.saveGState()
            let clipPx = CGRect(x: clip.minX * scale, y: CGFloat(height) - clip.maxY * scale,
                                width: clip.width * scale, height: clip.height * scale)
            ctx.addPath(CGPath(roundedRect: CGRect(x: frame.video.minX * scale, y: CGFloat(height) - frame.video.maxY * scale,
                                                   width: frame.video.width * scale, height: frame.video.height * scale),
                               cornerWidth: frame.radius * scale, cornerHeight: frame.radius * scale, transform: nil))
            ctx.clip()
            ctx.clip(to: clipPx)
            ctx.setFillColor(NSColor.black.cgColor)
            ctx.fill(clipPx)
            ctx.draw(image, in: CGRect(x: content.minX * scale, y: CGFloat(height) - content.maxY * scale,
                                       width: content.width * scale, height: content.height * scale))
            ctx.restoreGState()
        }
        guard let out = ctx.makeImage() else { return nil }
        return NSBitmapImageRep(cgImage: out).representation(using: .png, properties: [:])
    }
}

extension WebUIVideoSurfaces {
    /// Per stream: how many slots show it, frames decoded and presented,
    /// and each presented frame's time from its arrival (`decodeMs`, the
    /// Electron bench's arrival-to-drawn; apps/cua-spaces-desktop/docs/video.md).
    func stats() -> [[String: Any]] {
        streamsSnapshot().map { key, stream in
            let s = stream.session
            return ["spaceId": key.spaceId, "tier": key.tier.rawValue, "users": stream.users,
                    "decoded": s?.decodedFrameCount ?? 0, "presented": s?.presentation.framesPresented ?? 0,
                    "decodeMs": stream.presentedMs,
                    "presenters": s?.presentation.presenterCount ?? 0,
                    "size": s.map { "\(Int($0.lastFrameDimensions.width))x\(Int($0.lastFrameDimensions.height))" } ?? "",
                    "failure": stream.failure ?? NSNull()]
        }
    }

    struct VisibleFrame {
        let buffer: CVPixelBuffer
        /// The clip container, the video and its content rect, in the web view's flipped points.
        let clip: CGRect
        let video: CGRect
        let content: CGRect
        let radius: CGFloat
    }

    /// The frames on screen now and where (the harness's composite).
    func visibleFrames() -> [VisibleFrame] {
        surfaces.values.compactMap { surface in
            guard !surface.view.isHidden, let session = sessionFor(surface), let buffer = session.frame else { return nil }
            let clip = surface.view.frame
            let video = surface.view.input.frame.offsetBy(dx: clip.minX, dy: clip.minY)
            let content = surface.view.input.geometry.contentRect.offsetBy(dx: video.minX, dy: video.minY)
            return VisibleFrame(buffer: buffer, clip: clip, video: video, content: content, radius: surface.layout.radius)
        }
    }
}
