// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CanvasModel
import Foundation

/// Command line:
///
/// ```
/// infinite-canvas [--spaces FILE] [--hotkey option+space] [--windowed 1600x1000]
///                 [--show] [--no-activate] [--control SOCKET] [--synthetic N]
///                 [--perf-hud] [--registry DIR]
/// ```
struct Options {
    var spacesFile: String?
    var hotkey = HotkeySpec.default
    var windowed: CGSize?
    var showAtLaunch = false
    var activate = true
    var controlSocket: String?
    var synthetic = 0
    var perfHUD = false
    var registry: String?
    /// Borderless window of this size (recordings): no title bar, floating.
    var chromeless = false

    static func parse(_ args: [String]) throws -> Options {
        var o = Options()
        var it = args.dropFirst().makeIterator()
        func value(_ flag: String) throws -> String {
            guard let v = it.next() else { throw OptionError("\(flag) needs a value") }
            return v
        }
        while let a = it.next() {
            switch a {
            case "--spaces": o.spacesFile = try value(a)
            case "--hotkey":
                let v = try value(a)
                guard let spec = HotkeySpec(parsing: v) else { throw OptionError("bad hotkey \(v)") }
                o.hotkey = spec
            case "--windowed":
                let v = try value(a).split(separator: "x").compactMap { Double($0) }
                guard v.count == 2 else { throw OptionError("--windowed WxH") }
                o.windowed = CGSize(width: v[0], height: v[1])
            case "--show": o.showAtLaunch = true
            case "--no-activate": o.activate = false
            case "--control": o.controlSocket = try value(a)
            case "--synthetic": o.synthetic = Int(try value(a)) ?? 0
            case "--perf-hud": o.perfHUD = true
            case "--registry": o.registry = try value(a)
            case "--chromeless": o.chromeless = true
            case "-h", "--help":
                print("infinite-canvas [--spaces FILE] [--hotkey option+space] [--windowed WxH] [--show] "
                    + "[--no-activate] [--control SOCKET] [--synthetic N] [--perf-hud] [--registry DIR]")
                exit(0)
            default: throw OptionError("unknown option \(a)")
            }
        }
        return o
    }
}

struct OptionError: Error, CustomStringConvertible {
    var description: String
    init(_ s: String) { description = s }
}
