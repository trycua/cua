// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Compression
import CoreGraphics
import Foundation

/// Reads Cua Driver's default cursor theme (`cursor-overlay/assets/
/// cua.default.lottie`, copied verbatim into this target's resources by
/// `scripts/sync-cursor-assets.sh`) and evaluates the subset of Lottie it
/// uses: shape layers with static bezier paths, fills, strokes, and animated
/// layer opacity, position and scale.
///
/// Like the driver's own overlay, only the palette key (Cua blue) is recolored
/// with the session fill; white outlines stay white.
struct DotLottie {
    var animations: [String: LottieAnimation]

    /// The palette key the driver recolors.
    static let paletteKey: [Double] = [0.3686274509803922, 0.7529411764705882, 0.9098039215686274]

    init(data: Data) throws {
        var result: [String: LottieAnimation] = [:]
        for (name, bytes) in try ZipReader.entries(data) where name.hasPrefix("a/") && name.hasSuffix(".json") {
            let id = String(name.dropFirst(2).dropLast(5))
            result[id] = try LottieAnimation(json: bytes)
        }
        animations = result
    }

    /// Parsed once.
    nonisolated(unsafe) static let bundledTheme = bundled()

    static func bundled() -> DotLottie? {
        guard let url = Bundle.module.url(forResource: "cua.default", withExtension: "lottie"),
              let data = try? Data(contentsOf: url) else { return nil }
        return try? DotLottie(data: data)
    }
}

struct LottieAnimation {
    var size: CGSize
    var frameRate: Double
    var inPoint: Double
    var outPoint: Double
    /// Bottom-most first (draw order).
    var layers: [Layer]

    var duration: Double { max(outPoint - inPoint, 0) / max(frameRate, 1) }

    struct Layer {
        var name: String
        var anchor: Property
        var position: Property
        var scale: Property
        var opacity: Property
        var rotation: Property
        var items: [Item]
    }

    struct Item {
        var paths: [CGPath]
        var fill: Paint?
        var stroke: Stroke?
    }

    struct Paint {
        var color: [Double]
        var opacity: Double
        var isPaletteKey: Bool
    }

    struct Stroke {
        var paint: Paint
        var width: Double
        var roundCaps: Bool
    }

    /// A static or keyframed numeric vector.
    struct Property {
        var keyframes: [(t: Double, value: [Double], outX: Double, outY: Double, inX: Double, inY: Double)]
        var constant: [Double]

        func value(at frame: Double) -> [Double] {
            guard let first = keyframes.first else { return constant }
            if frame <= first.t { return first.value }
            for i in 0 ..< keyframes.count - 1 {
                let a = keyframes[i], b = keyframes[i + 1]
                if frame < b.t {
                    let u = (frame - a.t) / max(b.t - a.t, 1e-6)
                    let e = CubicBezier(x1: a.outX, y1: a.outY, x2: a.inX, y2: a.inY).solve(u)
                    return zip(a.value, b.value).map { $0 + ($1 - $0) * e }
                }
            }
            return keyframes.last!.value
        }
    }

    init(json: Data) throws {
        guard let root = try JSONSerialization.jsonObject(with: json) as? [String: Any] else {
            throw CocoaError(.fileReadCorruptFile)
        }
        size = CGSize(width: root["w"] as? Double ?? 128, height: root["h"] as? Double ?? 128)
        frameRate = root["fr"] as? Double ?? 30
        inPoint = root["ip"] as? Double ?? 0
        outPoint = root["op"] as? Double ?? 1
        let raw = root["layers"] as? [[String: Any]] ?? []
        layers = raw.reversed().compactMap(Self.layer)
    }

    private static func layer(_ l: [String: Any]) -> Layer? {
        guard (l["ty"] as? Int) == 4 else { return nil }
        let ks = l["ks"] as? [String: Any] ?? [:]
        var items: [Item] = []
        var paths: [CGPath] = []
        var fill: Paint?
        var stroke: Stroke?
        for s in l["shapes"] as? [[String: Any]] ?? [] {
            switch s["ty"] as? String {
            case "sh":
                if let k = (s["ks"] as? [String: Any])?["k"] as? [String: Any], let p = path(k) { paths.append(p) }
            case "fl":
                fill = paint(s)
            case "st":
                if let p = paint(s) {
                    let w = property(s["w"]).constant.first ?? 1
                    stroke = Stroke(paint: p, width: w, roundCaps: (s["lc"] as? Int) == 2)
                }
            default: break
            }
        }
        items.append(Item(paths: paths, fill: fill, stroke: stroke))
        return Layer(name: l["nm"] as? String ?? "", anchor: property(ks["a"]), position: property(ks["p"]),
                     scale: property(ks["s"], default: [100, 100]), opacity: property(ks["o"], default: [100]),
                     rotation: property(ks["r"], default: [0]), items: items)
    }

    private static func paint(_ s: [String: Any]) -> Paint? {
        guard let c = property(s["c"]).constant as [Double]?, c.count >= 3 else { return nil }
        let o = (property(s["o"], default: [100]).constant.first ?? 100) / 100
        let key = zip(c.prefix(3), DotLottie.paletteKey).allSatisfy { abs($0 - $1) < 0.002 }
        return Paint(color: Array(c.prefix(4)), opacity: o, isPaletteKey: key)
    }

    private static func property(_ any: Any?, default d: [Double] = [0, 0]) -> Property {
        guard let p = any as? [String: Any] else { return Property(keyframes: [], constant: d) }
        if (p["a"] as? Int) == 1, let kfs = p["k"] as? [[String: Any]] {
            var frames: [(Double, [Double], Double, Double, Double, Double)] = []
            for k in kfs {
                let t = k["t"] as? Double ?? 0
                let v = (k["s"] as? [Double]) ?? (k["s"] as? Double).map { [$0] } ?? d
                let o = k["o"] as? [String: Any]
                let i = k["i"] as? [String: Any]
                func first(_ x: Any?) -> Double { (x as? [Double])?.first ?? (x as? Double) ?? 0 }
                frames.append((t, v, first(o?["x"]), first(o?["y"]), i == nil ? 1 : first(i?["x"]),
                               i == nil ? 1 : first(i?["y"])))
            }
            return Property(keyframes: frames.map { (t: $0.0, value: $0.1, outX: $0.2, outY: $0.3, inX: $0.4, inY: $0.5) },
                            constant: frames.first?.1 ?? d)
        }
        if let v = p["k"] as? [Double] { return Property(keyframes: [], constant: v) }
        if let v = p["k"] as? Double { return Property(keyframes: [], constant: [v]) }
        return Property(keyframes: [], constant: d)
    }

    private static func path(_ k: [String: Any]) -> CGPath? {
        guard let v = k["v"] as? [[Double]], !v.isEmpty else { return nil }
        let ins = k["i"] as? [[Double]] ?? []
        let outs = k["o"] as? [[Double]] ?? []
        let closed = k["c"] as? Bool ?? false
        let p = CGMutablePath()
        func pt(_ a: [Double]) -> CGPoint { CGPoint(x: a[0], y: a[1]) }
        p.move(to: pt(v[0]))
        let n = v.count
        for idx in 1 ..< (closed ? n + 1 : n) {
            let prev = (idx - 1) % n, cur = idx % n
            let o = outs.indices.contains(prev) ? outs[prev] : [0, 0]
            let i = ins.indices.contains(cur) ? ins[cur] : [0, 0]
            let c1 = CGPoint(x: v[prev][0] + o[0], y: v[prev][1] + o[1])
            let c2 = CGPoint(x: v[cur][0] + i[0], y: v[cur][1] + i[1])
            p.addCurve(to: pt(v[cur]), control1: c1, control2: c2)
        }
        if closed { p.closeSubpath() }
        return p
    }
}

/// Lottie's easing: a unit cubic bezier from (0,0) to (1,1).
struct CubicBezier {
    var x1, y1, x2, y2: Double

    func solve(_ x: Double) -> Double {
        let x = min(max(x, 0), 1)
        var t = x
        for _ in 0 ..< 8 {
            let cx = bez(t, x1, x2) - x
            let d = dbez(t, x1, x2)
            if abs(cx) < 1e-5 || abs(d) < 1e-6 { break }
            t = min(max(t - cx / d, 0), 1)
        }
        return bez(t, y1, y2)
    }

    private func bez(_ t: Double, _ a: Double, _ b: Double) -> Double {
        3 * a * t * (1 - t) * (1 - t) + 3 * b * t * t * (1 - t) + t * t * t
    }

    private func dbez(_ t: Double, _ a: Double, _ b: Double) -> Double {
        3 * a * (1 - t) * (1 - t) + 6 * (b - a) * t * (1 - t) + 3 * (1 - b) * t * t
    }
}

/// Just enough ZIP to read a dotLottie: the central directory, stored and
/// deflated entries (raw deflate via the Compression framework).
enum ZipReader {
    static func entries(_ data: Data) throws -> [(String, Data)] {
        let bytes = [UInt8](data)
        func u16(_ o: Int) -> Int { Int(bytes[o]) | Int(bytes[o + 1]) << 8 }
        func u32(_ o: Int) -> Int { u16(o) | u16(o + 2) << 16 }
        guard bytes.count >= 22 else { throw CocoaError(.fileReadCorruptFile) }
        var eocd = bytes.count - 22
        while eocd >= 0, u32(eocd) != 0x0605_4B50 { eocd -= 1 }
        guard eocd >= 0 else { throw CocoaError(.fileReadCorruptFile) }
        let count = u16(eocd + 10)
        var cd = u32(eocd + 16)
        var out: [(String, Data)] = []
        for _ in 0 ..< count {
            guard cd + 46 <= bytes.count, u32(cd) == 0x0201_4B50 else { throw CocoaError(.fileReadCorruptFile) }
            let method = u16(cd + 10)
            let compressed = u32(cd + 20)
            let size = u32(cd + 24)
            let nameLen = u16(cd + 28), extraLen = u16(cd + 30), commentLen = u16(cd + 32)
            let local = u32(cd + 42)
            let name = String(decoding: bytes[(cd + 46) ..< (cd + 46 + nameLen)], as: UTF8.self)
            cd += 46 + nameLen + extraLen + commentLen
            guard local + 30 <= bytes.count else { continue }
            let start = local + 30 + u16(local + 26) + u16(local + 28)
            guard start + compressed <= bytes.count, size < 8 << 20 else { continue }
            let payload = Data(bytes[start ..< (start + compressed)])
            switch method {
            case 0: out.append((name, payload))
            case 8: out.append((name, inflate(payload, size: size)))
            default: continue
            }
        }
        return out
    }

    static func inflate(_ data: Data, size: Int) -> Data {
        var dst = [UInt8](repeating: 0, count: max(size, 1))
        let n = data.withUnsafeBytes { src in
            compression_decode_buffer(&dst, dst.count, src.bindMemory(to: UInt8.self).baseAddress!, data.count,
                                      nil, COMPRESSION_ZLIB)
        }
        return Data(dst.prefix(n))
    }
}
