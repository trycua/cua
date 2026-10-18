// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CoreGraphics
import Cua
import Foundation
import Testing
@testable import CuaSpacesStreaming

/// The presence netcode model through the Swift binding (the Rust core's
/// `PresenceView`, run against the same conformance vectors the Rust and
/// TypeScript suites run), the roster's shapes and heartbeats, and the
/// shared cursor art rendered in a participant's color.
@Suite(.serialized) final class PresenceNetcodeTests {

    private func str(_ v: Any?) -> String? { v as? String }
    private func num(_ v: Any?) -> Double { (v as? NSNumber)?.doubleValue ?? 0 }

    /// The Rust serde form of an event (the vectors' format) -> the SDK record.
    private func event(_ e: [String: Any]) -> CuaSDK.PresenceEvent {
        let kind = str(e["kind"]) ?? ""
        var participant: PresenceParticipant?
        if let p = e["participant"] as? [String: Any] {
            participant = PresenceParticipant(participantId: str(p["participant_id"]) ?? "",
                                              principalId: str(p["principal_id"]) ?? "",
                                              displayName: str(p["display_name"]) ?? "",
                                              color: str(p["color"]) ?? "", kind: str(p["kind"]) ?? "")
        }
        var cursor: PresenceCursor?
        if let c = e["cursor"] as? [String: Any] {
            cursor = PresenceCursor(displayId: str(c["display_id"]) ?? "", windowId: str(c["window_id"]),
                                    x: num(c["x"]), y: num(c["y"]), visible: c["visible"] as? Bool ?? true,
                                    pressed: false, shape: str(c["shape"]) ?? "arrow",
                                    shapeSource: str(c["shape_source"]) ?? "unspecified",
                                    atMs: num(c["at_ms"]), receivedMs: num(c["received_ms"]))
        }
        return CuaSDK.PresenceEvent(kind: kind, participant: participant,
                                    participantId: str(e["participant_id"]), cursor: cursor,
                                    shape: str(e["shape"]), shapeSource: str(e["source"]),
                                    reason: str(e["reason"]),
                                    participantIds: e["participant_ids"] as? [String])
    }

    @Test func conformanceVectorsThroughTheBinding() throws {
        let doc = try #require(try JSONSerialization.jsonObject(
            with: Data(presenceConformanceJson().utf8)) as? [String: Any])
        let tolerance = num(doc["tolerance"])
        let cases = try #require(doc["cases"] as? [[String: Any]])
        #expect(cases.count >= 10)
        for kase in cases {
            let name = str(kase["name"]) ?? "?"
            let view = PresenceView(me: str(kase["me"]) ?? "", delayMs: num(kase["delay_ms"]))
            for (i, step) in (kase["steps"] as? [[String: Any]] ?? []).enumerated() {
                let at = num(step["at"])
                if let e = step["event"] as? [String: Any] {
                    _ = view.apply(event: event(e), localMs: at)
                    continue
                }
                let pointer = (step["pointer"] as? [NSNumber]).map {
                    PresencePoint(x: $0[0].doubleValue, y: $0[1].doubleValue)
                }
                let got = view.drawables(localMs: at, pointer: pointer)
                let want = step["expect"] as? [[String: Any]] ?? []
                #expect(got.count == want.count, "\(name)#\(i): \(got)")
                for (g, w) in zip(got, want) {
                    #expect(g.participantId == str(w["participant_id"]), "\(name)#\(i)")
                    #expect(abs(g.x - num(w["x"])) < tolerance, "\(name)#\(i) x \(g.x)")
                    #expect(abs(g.y - num(w["y"])) < tolerance, "\(name)#\(i) y")
                    #expect(abs(g.alpha - num(w["alpha"])) < tolerance, "\(name)#\(i) alpha \(g.alpha)")
                    #expect(g.shape == str(w["shape"]), "\(name)#\(i)")
                    #expect(g.isMe == (w["is_me"] as? Bool), "\(name)#\(i)")
                }
            }
        }
    }

    private func p(_ id: String) -> PresenceParticipant {
        PresenceParticipant(participantId: id, principalId: id, displayName: id, color: "#123456",
                            kind: "human")
    }

    @Test func rosterFollowsShapesAndDropsWhoAHeartbeatOmits() {
        var roster = PresenceRoster()
        _ = roster.apply(me: p("me"), members: [PresenceMember(participant: p("a"), cursor: nil),
                                                PresenceMember(participant: p("b"), cursor: nil)])
        _ = roster.apply(event: CuaSDK.PresenceEvent(
            kind: "cursor_moved", participant: nil, participantId: "a",
            cursor: PresenceCursor(displayId: "", windowId: nil, x: 0.2, y: 0.3, visible: true,
                                   pressed: false, shape: "text", shapeSource: "hit_test",
                                   atMs: 0, receivedMs: 0)))
        #expect(roster.participants["a"]?.cursor?.shapeName == "text")
        #expect(roster.participants["a"]?.cursor?.shape == .text)
        _ = roster.apply(event: CuaSDK.PresenceEvent(kind: "shape_changed", participant: nil,
                                                     participantId: "a", cursor: nil,
                                                     shape: "resize_ew", shapeSource: "probe"))
        #expect(roster.participants["a"]?.cursor?.shapeName == "resize_ew")
        _ = roster.apply(event: CuaSDK.PresenceEvent(kind: "shape_changed", participant: nil,
                                                     participantId: "me", cursor: nil,
                                                     shape: "pointer", shapeSource: "system"))
        #expect(roster.myShape == "pointer", "your own shape comes from the server")
        let events = roster.apply(event: CuaSDK.PresenceEvent(kind: "heartbeat", participant: nil,
                                                              participantId: nil, cursor: nil,
                                                              participantIds: ["me", "a"]))
        #expect(roster.participants["b"] == nil)
        #expect(events.contains { if case .participantLeft(let x) = $0 { return x.id == "b" }; return false })
        #expect(roster.me?.id == "me")
    }

    @Test func everyShapeParsesInsideTheCanvas() {
        #expect(PresenceCursorArt.shapes.count == 14)
        let canvas = PresenceCursorArt.canvas
        for shape in PresenceCursorArt.shapes {
            let box = PresenceCursorArt.path(for: shape).boundingBoxOfPath
            #expect(!box.isEmpty, "\(shape)")
            #expect(box.minX >= 0 && box.minY >= 0 && box.maxX <= canvas + 0.5 && box.maxY <= canvas + 0.5,
                    "\(shape) \(box)")
            let h = PresenceCursorArt.hotspot(for: shape)
            #expect(h.x >= 0 && h.y >= 0 && h.x <= canvas && h.y <= canvas)
        }
        #expect(PresenceCursorArt.art(for: "nonsense").shape == "arrow")
    }

    /// Pixel probe of a rendered cursor (y-down canvas units at 1 px each).
    private func pixel(_ image: CGImage, _ x: Int, _ y: Int) -> (r: UInt8, g: UInt8, b: UInt8, a: UInt8) {
        let w = image.width, h = image.height
        var buf = [UInt8](repeating: 0, count: w * h * 4)
        let ctx = CGContext(data: &buf, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w * 4,
                            space: CGColorSpace(name: CGColorSpace.sRGB)!,
                            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
        ctx.draw(image, in: CGRect(x: 0, y: 0, width: w, height: h))
        let i = (y * w + x) * 4  // CGContext memory is top row first
        return (buf[i], buf[i + 1], buf[i + 2], buf[i + 3])
    }

    @Test func cursorsRenderInTheParticipantsColorWithAWhiteOutline() throws {
        let red = PresenceCursorArt.color(hex: "#e6194b")
        // The I-beam's stem centre is fill; just outside its bar is outline.
        let text = try #require(PresenceCursorArt.image(shape: "text", color: red, size: 32, scale: 1))
        let fill = pixel(text, 16, 16)
        #expect(fill.r == 0xe6 && fill.g == 0x19 && fill.b == 0x4b && fill.a == 255, "\(fill)")
        let outline = pixel(text, 16, 3)
        #expect(outline.r == 255 && outline.g == 255 && outline.b == 255 && outline.a > 200, "\(outline)")
        let empty = pixel(text, 2, 30)
        #expect(empty.a == 0)
        // The arrow's body just below its tip, in another color.
        let green = PresenceCursorArt.color(hex: "#3cb44b")
        let arrow = try #require(PresenceCursorArt.image(shape: "arrow", color: green, size: 32, scale: 1))
        let body = pixel(arrow, 4, 12)
        #expect(body.g == 0xb4 && body.r == 0x3c, "\(body)")
        // Faded cursors keep their color, lose opacity.
        let faded = try #require(PresenceCursorArt.image(shape: "text", color: red, size: 32, scale: 1, alpha: 0.5))
        #expect((100...160).contains(Int(pixel(faded, 16, 16).a)))
    }
}
