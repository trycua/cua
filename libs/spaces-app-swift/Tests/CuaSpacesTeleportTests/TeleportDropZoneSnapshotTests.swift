// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import SwiftUI
import Testing
@testable import CuaSpacesTeleport

/// Snapshots of the drop well, idle and while something is dragged over it
/// (dashed outline idle; solid accent outline and fill while
/// over). Rendered offscreen; `SNAPSHOT_RECORD=1` (or a missing reference)
/// records and fails, so a recording is never a pass.
@Suite(.serialized) @MainActor struct TeleportDropZoneSnapshotTests {
    static let snapshots = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
        .appendingPathComponent("Snapshots")

    init() { _ = NSApplication.shared }

    func zone() -> TeleportDropZone {
        TeleportDropZone(spaceID: "local:aurora", teleport: nil, onFiles: { _ in }, onApp: { _, _ in },
                         onSendFile: {}, onTeleportApp: {})
    }

    @Test func idle() throws {
        try check(zone().padding(16), "teleport-drop-zone-idle")
    }

    @Test func dragOver() throws {
        try check(zone().padding(16).environment(\.teleportDropZoneHighlighted, true), "teleport-drop-zone-over")
    }

    @Test func theWellIsTallAndStatesDiffer() throws {
        let idle = render(zone().padding(16))
        let over = render(zone().padding(16).environment(\.teleportDropZoneHighlighted, true))
        #expect(TeleportDropZone.minimumHeight >= 160)
        #expect(difference(idle, over) > 0.01, "the drag-over state must look different")
    }

    func render<V: View>(_ view: V) -> NSBitmapImageRep {
        let size = CGSize(width: 480, height: 240)
        let host = NSHostingView(rootView: view.frame(width: size.width, height: size.height)
            .environment(\.colorScheme, .light))
        host.frame = CGRect(origin: .zero, size: size)
        let window = NSWindow(contentRect: host.frame, styleMask: [.borderless], backing: .buffered, defer: false)
        window.contentView = host
        host.layoutSubtreeIfNeeded()
        RunLoop.main.run(until: Date().addingTimeInterval(0.3))
        // A fixed 2x bitmap, not bitmapImageRepForCachingDisplay: that follows
        // the display's backing scale (1x on CI runners, 2x on a Retina Mac),
        // and the references are 2x.
        let rep = NSBitmapImageRep(
            bitmapDataPlanes: nil, pixelsWide: Int(size.width) * 2, pixelsHigh: Int(size.height) * 2,
            bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
            colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
        rep.size = size
        host.cacheDisplay(in: host.bounds, to: rep)
        return rep
    }

    func check<V: View>(_ view: V, _ name: String) throws {
        let rep = render(view)
        let png = rep.representation(using: .png, properties: [:])!
        let url = Self.snapshots.appendingPathComponent("\(name).png")
        let record = ProcessInfo.processInfo.environment["SNAPSHOT_RECORD"] == "1"
        guard !record, let reference = NSBitmapImageRep(data: (try? Data(contentsOf: url)) ?? Data()) else {
            try png.write(to: url)
            Issue.record("recorded \(url.lastPathComponent); run again to compare")
            return
        }
        let diff = difference(rep, reference)
        #expect(diff <= 0.02, "\(name) differs from its reference in \(Int(diff * 100))% of pixels")
    }

    func difference(_ a: NSBitmapImageRep, _ b: NSBitmapImageRep) -> Double {
        guard a.pixelsWide == b.pixelsWide, a.pixelsHigh == b.pixelsHigh else { return 1 }
        var differing = 0, total = 0
        for y in stride(from: 0, to: a.pixelsHigh, by: 2) {
            for x in stride(from: 0, to: a.pixelsWide, by: 2) {
                total += 1
                guard let ca = a.colorAt(x: x, y: y), let cb = b.colorAt(x: x, y: y) else { continue }
                let d = abs(ca.redComponent - cb.redComponent) + abs(ca.greenComponent - cb.greenComponent)
                    + abs(ca.blueComponent - cb.blueComponent)
                if d > 0.3 { differing += 1 }
            }
        }
        return total == 0 ? 0 : Double(differing) / Double(total)
    }
}
