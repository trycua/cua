// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSDK
import CuaSpacesFFI
@testable import CuaSpacesMacKit
import CuaSpacesStreaming
import QuartzCore
import Testing

/// Snapshots of the presence cursors the stream views draw: every shared
/// shape, in two participants' colors, rendered offscreen by the real
/// overlay (`PresenceOverlayView` over the SDK's `PresenceView`) and compared
/// with its reference in Snapshots/. `SNAPSHOT_RECORD=1` (or a missing
/// reference) records and fails, like the other snapshots.
@MainActor
@Suite("Presence cursor snapshots", .serialized)
struct PresenceCursorSnapshotTests {
    static let colors = ["#e6194b", "#4363d8"]
    static let cell = CGSize(width: 64, height: 64)

    /// One overlay holding one participant per (shape, color), each hovering
    /// its own cell, drawn at a fixed presence time.
    func overlay(shapes: [String]) -> PresenceOverlayView {
        let size = CGSize(width: Self.cell.width * CGFloat(shapes.count),
                          height: Self.cell.height * CGFloat(Self.colors.count))
        let view = PresenceView(me: "me", delayMs: 100)
        for (row, color) in Self.colors.enumerated() {
            for (col, shape) in shapes.enumerated() {
                let id = "\(shape)-\(row)"
                _ = view.apply(event: CuaSDK.PresenceEvent(
                    kind: "joined",
                    participant: PresenceParticipant(participantId: id, principalId: id, displayName: "",
                                                     color: color, kind: "human"),
                    participantId: nil, cursor: nil), localMs: 0)
                // Hot spot 16 pt into its cell, leaving room for the art.
                let x = (CGFloat(col) * Self.cell.width + 20) / size.width
                let y = (CGFloat(row) * Self.cell.height + 20) / size.height
                _ = view.apply(event: CuaSDK.PresenceEvent(
                    kind: "cursor_moved", participant: nil, participantId: id,
                    cursor: PresenceCursor(displayId: "", windowId: nil, x: Double(x), y: Double(y), visible: true,
                                           pressed: false, shape: shape, shapeSource: "hit_test",
                                           atMs: 1_000, receivedMs: 1_000)), localMs: 1_000)
            }
        }
        let o = PresenceOverlayView(frame: CGRect(origin: .zero, size: size))
        o.surfaceSize = size
        o.presenceView = view
        // Offscreen: no window has the keyboard, and the real cursor stays.
        o.isFocused = { true }
        o.cursorHider = SystemCursorHider(hide: {}, unhide: {})
        o.layer?.backgroundColor = CGColor(gray: 0.42, alpha: 1)
        o.render(localMs: 1_200)
        return o
    }

    /// The overlay's layer tree rendered to pixels at 2x, top row first.
    static func png(_ view: NSView) -> NSBitmapImageRep {
        let scale: CGFloat = 2
        let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: Int(view.bounds.width * scale),
                                   pixelsHigh: Int(view.bounds.height * scale), bitsPerSample: 8,
                                   samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
                                   colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
        let ctx = NSGraphicsContext(bitmapImageRep: rep)!.cgContext
        // The view is flipped: draw its y-down layer tree into a y-up bitmap.
        ctx.translateBy(x: 0, y: view.bounds.height * scale)
        ctx.scaleBy(x: scale, y: -scale)
        view.layer?.render(in: ctx)
        return rep
    }

    func assertSnapshot(_ view: NSView, _ name: String) throws {
        let rep = Self.png(view)
        let png = rep.representation(using: .png, properties: [:])!
        let url = SnapshotTests.snapshots.appendingPathComponent("\(name).png")
        let record = ProcessInfo.processInfo.environment["SNAPSHOT_RECORD"] == "1"
        guard !record, let reference = NSBitmapImageRep(data: (try? Data(contentsOf: url)) ?? Data()) else {
            try png.write(to: url)
            Issue.record("recorded \(url.lastPathComponent); run again to compare")
            return
        }
        let diff = SnapshotTests().difference(rep, reference)
        if diff > 0.01 {
            try png.write(to: SnapshotTests.snapshots.appendingPathComponent("\(name).actual.png"))
        }
        #expect(diff <= 0.01, "\(name) differs from its reference in \(Int(diff * 100))% of pixels")
    }

    @Test func everyShapeInTwoColors() throws {
        let shapes = PresenceCursorArt.shapes
        #expect(shapes.count == 14)
        let o = overlay(shapes: shapes)
        #expect(o.drawn.count == shapes.count * Self.colors.count)
        try assertSnapshot(o, "presence-cursors")
    }

    /// The pill: a remote participant's name in their color, next to the
    /// shaped cursor; yours has none.
    @Test func namePillsAndYourOwnCursor() throws {
        let size = CGSize(width: 360, height: 120)
        let view = PresenceView(me: "me", delayMs: 100)
        let people: [(String, String, String, String, Double)] = [
            ("me", "Dillon", "#e6194b", "arrow", 0),
            ("koala", "Koala", "#3cb44b", "pointer", 0.45),
            ("ana", "Ana", "#f58231", "text", 0.75),
        ]
        for (id, name, color, shape, x) in people {
            _ = view.apply(event: CuaSDK.PresenceEvent(
                kind: "joined",
                participant: PresenceParticipant(participantId: id, principalId: id, displayName: name,
                                                 color: color, kind: id == "koala" ? "agent" : "human"),
                participantId: nil, cursor: nil), localMs: 0)
            guard id != "me" else { continue }
            _ = view.apply(event: CuaSDK.PresenceEvent(
                kind: "cursor_moved", participant: nil, participantId: id,
                cursor: PresenceCursor(displayId: "", windowId: nil, x: x, y: 0.3, visible: true, pressed: false,
                                       shape: shape, shapeSource: "probe", atMs: 1_000, receivedMs: 1_000)),
                localMs: 1_000)
        }
        let o = PresenceOverlayView(frame: CGRect(origin: .zero, size: size))
        o.surfaceSize = size
        o.presenceView = view
        // Offscreen: no window has the keyboard, and the real cursor stays.
        o.isFocused = { true }
        o.cursorHider = SystemCursorHider(hide: {}, unhide: {})
        o.layer?.backgroundColor = CGColor(gray: 0.95, alpha: 1)
        o.pointer(at: CGPoint(x: 40, y: 36))
        o.render(localMs: 1_200)
        #expect(o.drawn.map(\.participantID) == ["ana", "koala", "me"])
        try assertSnapshot(o, "presence-pills")
    }

    /// Opt-in evidence: the overlay rendered offscreen over a real guest
    /// frame. `CUA_PRESENCE_EVIDENCE_FRAME` is a PNG of a Space's desktop;
    /// the render goes next to it as `presence-overlay.png`. The
    /// participants' positions and shapes are fed as presence events.
    @Test func evidenceOverARealFrame() throws {
        guard let path = ProcessInfo.processInfo.environment["CUA_PRESENCE_EVIDENCE_FRAME"],
              let frame = NSImage(contentsOfFile: path)?.cgImage(forProposedRect: nil, context: nil, hints: nil)
        else { return }
        let size = CGSize(width: frame.width, height: frame.height)
        let view = PresenceView(me: "me", delayMs: 100)
        let people: [(String, String, String, String, Double, Double)] = [
            ("me", "Dillon", "#e6194b", "text", 0, 0),
            ("koala", "Koala", "#3cb44b", "pointer", 325 / size.width, 202 / size.height),
            ("ana", "Ana", "#4363d8", "text", 300 / size.width, 153 / size.height),
        ]
        for (id, name, color, shape, x, y) in people {
            _ = view.apply(event: CuaSDK.PresenceEvent(
                kind: "joined",
                participant: PresenceParticipant(participantId: id, principalId: id, displayName: name, color: color,
                                                 kind: id == "koala" ? "agent" : "human"),
                participantId: nil, cursor: nil), localMs: 0)
            if id == "me" {
                _ = view.apply(event: CuaSDK.PresenceEvent(kind: "shape_changed", participant: nil, participantId: id,
                                                           cursor: nil, shape: shape, shapeSource: "system"),
                               localMs: 0)
                continue
            }
            _ = view.apply(event: CuaSDK.PresenceEvent(
                kind: "cursor_moved", participant: nil, participantId: id,
                cursor: PresenceCursor(displayId: "", windowId: nil, x: x, y: y, visible: true, pressed: false,
                                       shape: shape, shapeSource: "probe", atMs: 1_000, receivedMs: 1_000)),
                localMs: 1_000)
        }
        let o = PresenceOverlayView(frame: CGRect(origin: .zero, size: size))
        o.surfaceSize = size
        o.presenceView = view
        // Offscreen: no window has the keyboard, and the real cursor stays.
        o.isFocused = { true }
        o.cursorHider = SystemCursorHider(hide: {}, unhide: {})
        let background = CALayer()
        background.frame = o.bounds
        background.contents = frame
        background.contentsGravity = .resize
        background.isGeometryFlipped = true
        o.layer?.insertSublayer(background, at: 0)
        o.pointer(at: CGPoint(x: 700, y: 330))
        o.render(localMs: 1_200)
        let out = URL(fileURLWithPath: path).deletingLastPathComponent().appendingPathComponent("presence-overlay.png")
        try Self.png(o).representation(using: .png, properties: [:])!.write(to: out)
    }

    @Test func thePresenceNameIsTheAccountThenThisMac() {
        let signedIn = AccountProfile(name: "Ada Lovelace", email: "dillon@trycua.com")
        #expect(AppModel.presenceName(profile: signedIn, fullName: "D", user: "d") == "Ada Lovelace")
        #expect(AppModel.presenceName(profile: AccountProfile(email: "dillon@trycua.com"), fullName: "D", user: "d")
            == "dillon")
        #expect(AppModel.presenceName(profile: nil, fullName: "Ada Lovelace", user: "ada") == "Ada Lovelace")
        #expect(AppModel.presenceName(profile: nil, fullName: "", user: "ada") == "ada")
        // Never an agent's name, "You" or empty.
        for reserved in ["CUA agent", "You", ""] {
            let name = AppModel.presenceName(profile: AccountProfile(name: reserved), fullName: reserved, user: "sam")
            #expect(name == "sam")
        }
        #expect(!AppModel.presenceName(profile: nil, fullName: "", user: "").isEmpty)
    }
}
