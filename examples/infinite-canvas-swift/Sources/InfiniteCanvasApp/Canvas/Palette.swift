// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CanvasModel
import SwiftUI

/// Grays plus one accent, from the system where the system has one.
enum Palette {
    static let canvas = NSColor(srgbRed: 0.07, green: 0.075, blue: 0.085, alpha: 1)
    static let gridDot = NSColor(white: 1, alpha: 0.09)
    static let tileBackground = NSColor(srgbRed: 0.11, green: 0.115, blue: 0.13, alpha: 1)
    static let hairline = NSColor(white: 1, alpha: 0.10)
    static let title = NSColor(white: 1, alpha: 0.82)
    static let secondary = NSColor(white: 1, alpha: 0.5)
    static var accent: NSColor { .controlAccentColor }
}

extension RGB {
    var nsColor: NSColor { NSColor(srgbRed: r, green: g, blue: b, alpha: 1) }
    var color: Color { Color(.sRGB, red: r, green: g, blue: b, opacity: 1) }
}
