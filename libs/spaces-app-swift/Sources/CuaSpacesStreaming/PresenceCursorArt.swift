// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CoreGraphics
import Cua
import Foundation

/// The shared presence cursor art, drawn in a participant's color.
///
/// The art is the cua SDK's (`presenceCursorArt(shape:)`, from
/// `presence-cursors.json`, the one source every Cua client draws): one path
/// per shape on a square canvas, filled with the participant's color over a
/// white outline. This type turns it into Core Graphics: a `CGPath` for a
/// `CAShapeLayer`, or a finished `CGImage` for anything that wants pixels.
///
/// Coordinates are y-down (the canvas's own), matching a flipped `NSView`
/// or a `CALayer` with `isGeometryFlipped`; the hot spot is where the
/// pointer's position goes.
public enum PresenceCursorArt {
    /// Every shape name, in wire order.
    public static var shapes: [String] { presenceCursorArtAll().map(\.shape) }

    /// The SDK's art record for `shape` (unknown names get the arrow).
    public static func art(for shape: String) -> CuaSDK.CursorArt {
        cache.art(shape)
    }

    /// The shape's outline path in canvas units.
    public static func path(for shape: String) -> CGPath {
        cache.path(shape)
    }

    /// The hot spot in canvas units.
    public static func hotspot(for shape: String) -> CGPoint {
        let a = art(for: shape)
        return CGPoint(x: a.hotspotX, y: a.hotspotY)
    }

    /// The canvas size (square) in canvas units.
    public static var canvas: CGFloat { CGFloat(art(for: "arrow").canvas) }

    /// Parses SVG path data restricted to absolute `M`, `L`, `C` and `Z`
    /// (what the shared art uses). Unknown commands end the parse.
    public static func parsePath(_ d: String) -> CGPath {
        let path = CGMutablePath()
        var tokens: [String] = []
        var number = ""
        for ch in d {
            if ch.isLetter {
                if !number.isEmpty { tokens.append(number); number = "" }
                tokens.append(String(ch))
            } else if ch == " " || ch == "," {
                if !number.isEmpty { tokens.append(number); number = "" }
            } else if ch == "-" && !number.isEmpty {
                tokens.append(number)
                number = "-"
            } else {
                number.append(ch)
            }
        }
        if !number.isEmpty { tokens.append(number) }
        var i = 0
        var command = ""
        func next() -> CGFloat? {
            guard i < tokens.count, let v = Double(tokens[i]) else { return nil }
            i += 1
            return CGFloat(v)
        }
        while i < tokens.count {
            if let c = tokens[i].first, c.isLetter {
                command = tokens[i]
                i += 1
                if command == "Z" { path.closeSubpath() }
                continue
            }
            switch command {
            case "M":
                guard let x = next(), let y = next() else { return path }
                path.move(to: CGPoint(x: x, y: y))
                command = "L"  // implicit lineto after a moveto
            case "L":
                guard let x = next(), let y = next() else { return path }
                path.addLine(to: CGPoint(x: x, y: y))
            case "C":
                guard let x1 = next(), let y1 = next(), let x2 = next(), let y2 = next(),
                      let x = next(), let y = next() else { return path }
                path.addCurve(to: CGPoint(x: x, y: y), control1: CGPoint(x: x1, y: y1),
                              control2: CGPoint(x: x2, y: y2))
            default:
                return path
            }
        }
        return path
    }

    /// `#rrggbb` (or `#rgb`) to an sRGB color; anything else is the SDK's
    /// fallback blue.
    public static func color(hex: String) -> CGColor {
        var s = hex.trimmingCharacters(in: .whitespaces)
        if s.hasPrefix("#") { s.removeFirst() }
        if s.count == 3 { s = s.map { "\($0)\($0)" }.joined() }
        guard s.count == 6, let v = UInt32(s, radix: 16) else {
            return CGColor(srgbRed: 0x3b / 255, green: 0x82 / 255, blue: 0xf6 / 255, alpha: 1)
        }
        return CGColor(srgbRed: CGFloat((v >> 16) & 0xff) / 255, green: CGFloat((v >> 8) & 0xff) / 255,
                       blue: CGFloat(v & 0xff) / 255, alpha: 1)
    }

    /// The cursor as an image `size` points square at `scale` pixels per
    /// point: the outline, then the fill in `color`, at `alpha`. Place its
    /// top-left at `pointer - hotspot(for:) * size / canvas`.
    public static func image(shape: String, color: CGColor, size: CGFloat = 24, scale: CGFloat = 2,
                             alpha: CGFloat = 1) -> CGImage? {
        let px = max(1, Int((size * scale).rounded()))
        guard let space = CGColorSpace(name: CGColorSpace.sRGB),
              let ctx = CGContext(data: nil, width: px, height: px, bitsPerComponent: 8, bytesPerRow: 0,
                                  space: space,
                                  bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else {
            return nil
        }
        // Canvas is y-down; a bitmap context is y-up.
        let k = CGFloat(px) / canvas
        ctx.translateBy(x: 0, y: CGFloat(px))
        ctx.scaleBy(x: k, y: -k)
        ctx.setAlpha(alpha)
        ctx.beginTransparencyLayer(auxiliaryInfo: nil)
        draw(shape: shape, color: color, in: ctx)
        ctx.endTransparencyLayer()
        return ctx.makeImage()
    }

    /// Draws the cursor into `ctx` in canvas units (outline under fill).
    public static func draw(shape: String, color: CGColor, in ctx: CGContext) {
        let a = art(for: shape)
        let p = path(for: shape)
        ctx.addPath(p)
        ctx.setStrokeColor(Self.color(hex: a.outlineColor))
        ctx.setLineWidth(CGFloat(a.outlineWidth))
        ctx.setLineJoin(.round)
        ctx.strokePath()
        ctx.addPath(p)
        ctx.setFillColor(color)
        ctx.fillPath(using: .winding)
    }

    private final class Cache: @unchecked Sendable {
        private let lock = NSLock()
        private var arts: [String: CuaSDK.CursorArt] = [:]
        private var paths: [String: CGPath] = [:]

        func art(_ shape: String) -> CuaSDK.CursorArt {
            lock.lock(); defer { lock.unlock() }
            if let a = arts[shape] { return a }
            let a = presenceCursorArt(shape: shape)
            arts[shape] = a
            return a
        }

        func path(_ shape: String) -> CGPath {
            let d = art(shape).pathD
            lock.lock(); defer { lock.unlock() }
            if let p = paths[shape] { return p }
            let p = PresenceCursorArt.parsePath(d)
            paths[shape] = p
            return p
        }
    }

    private static let cache = Cache()
}
