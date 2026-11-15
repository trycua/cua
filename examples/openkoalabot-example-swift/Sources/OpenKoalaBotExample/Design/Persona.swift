// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import SwiftUI

/// A Bot's colour and shape, derived **deterministically from its agent id**.
///
/// This is not a palette rotation. The app picks a persona's colour
/// and shape by hashing the agent id with FNV-1a and running the result through
/// a fixed PRNG, so the same Bot is the same colour on every machine and in
/// every session, with no stored attribute. Rotating a local palette by roster
/// index — which is what this app used to do — gives a Bot a *different* colour
/// depending on what order it happened to be created in, and a different colour
/// again after a restart reorders the roster.
///
/// The arithmetic is 32-bit throughout: the multiply wraps (like JavaScript's
/// `Math.imul`) and every shift is on a `UInt32`, so every step is
/// done in `UInt32` here and the operands are taken `&*` / `&+` to wrap rather
/// than trap. Getting that wrong does not crash — it silently yields different
/// colours, which is exactly the failure this type exists to prevent.
enum Persona {

    // MARK: The persona catalogue

    /// The eleven named persona colours, light and dark.
    ///
    /// `black` is in the table but is **not** in the fallback rotation (the
    /// fallback picks from the ten names below it), so it
    /// is reachable only when a Bot carries an explicit colour.
    static let colors: [String: (light: UInt32, dark: UInt32)] = [
        "black":   (0x000000, 0xFFFFFF),
        "brown":   (0xA27952, 0x855C36),
        "red":     (0xFF3E51, 0xE02135),
        "orange":  (0xFF781C, 0xFF6700),
        "yellow":  (0xFFAF38, 0xFF9800),
        "green":   (0x00C972, 0x009957),
        "cyan":    (0x1CC3B0, 0x00A592),
        "blue":    (0x2A92FE, 0x0E74E0),
        "violet":  (0xA97EFE, 0x804EE0),
        "magenta": (0xFF5EB1, 0xE02A88),
        "gray":    (0x959595, 0x777777),
    ]

    /// The ten names the deterministic fallback draws from, in order. The
    /// index is `floor(random() * 10)`, so the order is load-bearing.
    static let colorRotation = ["brown", "red", "orange", "yellow", "green",
                                "cyan", "blue", "violet", "magenta", "gray"]

    /// The eight persona shape names, in order. The modulus is over this
    /// array, so the order is load-bearing here too.
    static let shapeRotation = ["blob", "pebble", "squircle", "tablet",
                                "wedge", "hex", "cloud", "teardrop"]

    /// The persona shape names mapped onto this app's drawn vocabulary.
    ///
    /// `BlobShape` predates the persona shape names, so the two vocabularies
    /// are joined here rather than one being renamed into the other: renaming
    /// would move the nine fixture Bots, and those are in the export renders.
    static func blobShape(named name: String) -> BlobShape {
        switch name {
        case "blob":     return .circle
        case "pebble":   return .egg
        case "squircle": return .squircle
        case "tablet":   return .lozenge
        case "wedge":    return .triangle
        case "hex":      return .hexagon
        case "cloud":    return .cloud
        case "teardrop": return .teardrop
        default:         return .circle
        }
    }

    // MARK: The hash

    /// FNV-1a, 32-bit, over UTF-16 code units, not bytes. A non-BMP character
    /// is two code units in UTF-16 and one scalar in Swift, so iterating
    /// scalars would give a different hash (and a different colour) on an
    /// emoji-bearing id than a UTF-16 client computes.
    static func hash(_ value: String) -> UInt32 {
        var h: UInt32 = 2166136261
        for unit in value.utf16 {
            h = (h ^ UInt32(unit)) &* 16777619
        }
        return h
    }

    /// The persona PRNG: sfc-style, seeded once, first draw taken. Only the
    /// first value is ever used, but the stepping is kept whole so the
    /// sequence is stable.
    static func random(seed: UInt32) -> Double {
        var value = seed &+ 1831565813
        var next = (value ^ (value >> 15)) &* (1 | value)
        next = ((next &+ ((next ^ (next >> 7)) &* (61 | next))) ^ next)
        return Double((next ^ (next >> 14))) / 4294967296.0
    }

    /// The colour index. The seed is XORed with the golden-ratio constant
    /// twice — once into the seed, once on the way into the PRNG — which looks
    /// redundant and is not: `Math.imul(1, 2654435769)` is that constant as a
    /// *signed* 32-bit value, and the second XOR is against the same bits, so
    /// the two cancel only for the low word. Kept exactly as is: changing it
    /// would recolour every existing Bot.
    static func colorIndex(agentID: String) -> Int {
        let seed = hash(agentID) ^ 2654435769
        let r = random(seed: seed ^ 2654435769)
        return min(colorRotation.count - 1, Int(r * Double(colorRotation.count)))
    }

    /// The shape hash: a second avalanche over the FNV result, so shape
    /// and colour do not correlate.
    static func shapeHash(agentID: String) -> UInt32 {
        var h = hash(agentID)
        h = (h ^ (h >> 16)) &* 73244475
        h = (h ^ (h >> 13)) &* 3266489909
        return h ^ (h >> 16)
    }

    // MARK: Resolution

    /// The persona colour name for an agent id, honouring an explicit colour
    /// when the Bot carries one.
    static func colorName(agentID: String, explicit: String? = nil) -> String {
        if let explicit, colors[explicit] != nil { return explicit }
        return colorRotation[colorIndex(agentID: agentID)]
    }

    /// The persona shape name for an agent id.
    static func shapeName(agentID: String, explicit: String? = nil) -> String {
        if let explicit, shapeRotation.contains(explicit) { return explicit }
        return shapeRotation[Int(shapeHash(agentID: agentID) % UInt32(shapeRotation.count))]
    }

    /// Everything a roster row needs, in one call.
    static func resolve(agentID: String, dark: Bool = true,
                        color: String? = nil, shape: String? = nil)
        -> (colorHex: UInt32, shape: BlobShape, colorName: String)
    {
        let name = colorName(agentID: agentID, explicit: color)
        let pair = colors[name] ?? colors["gray"]!
        return (dark ? pair.dark : pair.light,
                blobShape(named: shapeName(agentID: agentID, explicit: shape)),
                name)
    }

    /// Mint a `Bot` whose look is derived rather than assigned.
    static func bot(id: String, name: String, dark: Bool = true,
                    preview: String = "", timestamp: String = "",
                    screenIndex: Int = 0) -> Bot {
        let r = resolve(agentID: id, dark: dark)
        return Bot(id: id, name: name, shape: r.shape, colorHex: r.colorHex,
                   preview: preview, timestamp: timestamp, screenIndex: screenIndex)
    }
}
