// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import SwiftUI

/// A tiny SVG path reader for the koala geometry: absolute `M`, `L`, `H`,
/// `V`, `C` and `Z`, the only commands the koala design's curves use.
enum SVGPath {
    static func parse(_ d: String) -> Path {
        var path = Path()
        var scanner = Tokens(d)
        var command: Character = "M"
        var current = CGPoint.zero
        while let token = scanner.next() {
            switch token {
            case .command(let c):
                command = c
                if c == "Z" || c == "z" { path.closeSubpath() }
            case .number(let first):
                switch command {
                case "M":
                    let p = CGPoint(x: first, y: scanner.number())
                    path.move(to: p)
                    current = p
                    command = "L"  // implicit lineto after a moveto pair
                case "L":
                    let p = CGPoint(x: first, y: scanner.number())
                    path.addLine(to: p)
                    current = p
                case "H":
                    let p = CGPoint(x: first, y: current.y)
                    path.addLine(to: p)
                    current = p
                case "V":
                    let p = CGPoint(x: current.x, y: first)
                    path.addLine(to: p)
                    current = p
                case "C":
                    let c1 = CGPoint(x: first, y: scanner.number())
                    let c2 = CGPoint(x: scanner.number(), y: scanner.number())
                    let p = CGPoint(x: scanner.number(), y: scanner.number())
                    path.addCurve(to: p, control1: c1, control2: c2)
                    current = p
                default:
                    break
                }
            }
        }
        return path
    }

    private enum Token { case command(Character), number(Double) }

    private struct Tokens {
        let chars: [Character]
        var i = 0
        init(_ s: String) { chars = Array(s) }

        mutating func next() -> Token? {
            while i < chars.count, chars[i] == " " || chars[i] == "," || chars[i] == "\n" { i += 1 }
            guard i < chars.count else { return nil }
            let c = chars[i]
            if c.isLetter && c != "e" {
                i += 1
                return .command(c)
            }
            var s = ""
            if c == "-" || c == "+" { s.append(c); i += 1 }
            var seenDot = false
            while i < chars.count {
                let d = chars[i]
                if d.isNumber { s.append(d); i += 1; continue }
                if d == "." && !seenDot { seenDot = true; s.append(d); i += 1; continue }
                if d == "e" { s.append(d); i += 1
                    if i < chars.count, chars[i] == "-" || chars[i] == "+" { s.append(chars[i]); i += 1 }
                    continue
                }
                break
            }
            return .number(Double(s) ?? 0)
        }

        mutating func number() -> Double {
            if case .number(let v)? = next() { return v }
            return 0
        }
    }
}
