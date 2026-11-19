// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CanvasModel
import Cua
import Foundation

/// Where a window's app icon is looked up: the Space itself.
///
/// The lookup and the cache are the cua SDK's (`Space.appIcons`, shared
/// with the Spaces apps and the OpenKoalaBots samples: one icon cache keyed
/// by the app and the Space's image, in memory and `$CUA_HOME/cache/icons`,
/// every miss in one guest round trip). When the Space has no icon for an
/// app (a bare X11 client, a fixture), the result is `nil` and the tile
/// shows no icon: never a stand-in glyph. This keeps no cache: it decodes
/// what one batched call returns.
struct AppIconKey: Hashable, Sendable {
    var spaceID: String
    var os: SpaceOS
    var app: String
    var appID: String
    var pid: UInt32
}

@MainActor
final class AppIcons {
    /// One batched lookup: icon bytes per key, in order.
    typealias Fetch = ([AppIconKey]) async -> [Data?]
    private let fetch: Fetch
    /// Batched calls made, for tests.
    private(set) var calls = 0

    init(fetch: @escaping Fetch) { self.fetch = fetch }

    /// Every key's icon in one call (nil where the Space has none).
    func icons(for keys: [AppIconKey]) async -> [NSImage?] {
        guard !keys.isEmpty else { return [] }
        calls += 1
        let data = await fetch(keys)
        return keys.indices.map { i in
            guard i < data.count, let d = data[i], !d.isEmpty, let image = NSImage(data: d),
                  image.size.width > 0 else { return nil }
            return image
        }
    }

    /// The SDK-backed fetch: `Space.appIcons`.
    static func spaceFetch(space: CuaSDK.Space) -> Fetch {
        { keys in
            let requests = keys.map { SpaceAppIconRequest(appName: $0.app, appId: $0.appID, pid: $0.pid) }
            guard let icons = try? await space.appIcons(requests: requests) else { return keys.map { _ in nil } }
            return icons.map { $0?.bytes }
        }
    }
}

/// Small monochrome marks for the Space's operating system: simple glyphs,
/// not official logos (the Apple mark is the system's SF Symbol).
enum OSMark {
    static func image(_ os: SpaceOS, size: CGFloat = 14) -> NSImage? {
        let config = NSImage.SymbolConfiguration(pointSize: size, weight: .regular)
        switch os {
        case .macos:
            return NSImage(systemSymbolName: "apple.logo", accessibilityDescription: "macOS")?
                .withSymbolConfiguration(config)
        case .linux, .omarchy, .windows:
            let image = NSImage(size: NSSize(width: size, height: size), flipped: true) { r in
                NSColor.white.setFill()
                NSColor.white.setStroke()
                path(os, in: r).fill()
                return true
            }
            image.isTemplate = true
            image.accessibilityDescription = os.label
            return image
        }
    }

    /// The mark drawn in one color (template images do not tint reliably
    /// inside layers or offscreen SwiftUI).
    static func tinted(_ os: SpaceOS, color: NSColor, size: CGFloat = 14) -> NSImage? {
        guard let image = image(os, size: size) else { return nil }
        return NSImage(size: image.size, flipped: false) { r in
            image.draw(in: r)
            color.set()
            r.fill(using: .sourceAtop)
            return true
        }
    }

    /// The glyph as a path in `r` (y down).
    static func path(_ os: SpaceOS, in r: CGRect) -> NSBezierPath {
        let p = NSBezierPath()
        switch os {
        case .windows:
            // Four panes.
            let g = r.width * 0.08, w = (r.width - g) / 2
            for (x, y) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                p.append(NSBezierPath(rect: CGRect(x: r.minX + CGFloat(x) * (w + g), y: r.minY + CGFloat(y) * (w + g),
                                                   width: w, height: w)))
            }
        case .linux:
            // Ubuntu's circle of friends, simplified: a ring and three heads.
            let c = CGPoint(x: r.midX, y: r.midY), ring = r.width * 0.32
            let outer = NSBezierPath(ovalIn: CGRect(x: c.x - ring, y: c.y - ring, width: ring * 2, height: ring * 2))
            let inner = NSBezierPath(ovalIn: CGRect(x: c.x - ring * 0.62, y: c.y - ring * 0.62, width: ring * 1.24, height: ring * 1.24))
            p.append(outer)
            p.append(inner.reversed)
            let head = r.width * 0.13
            for a in [0.0, 2.094, 4.189] {
                let hc = CGPoint(x: c.x + cos(a - .pi / 2) * ring * 1.05, y: c.y + sin(a - .pi / 2) * ring * 1.05)
                p.append(NSBezierPath(ovalIn: CGRect(x: hc.x - head, y: hc.y - head, width: head * 2, height: head * 2)))
            }
        case .omarchy:
            // A block "o": a square ring with a notch, after Omarchy's
            // pixel wordmark.
            let outer = NSBezierPath(rect: r.insetBy(dx: r.width * 0.12, dy: r.width * 0.12))
            let inner = NSBezierPath(rect: r.insetBy(dx: r.width * 0.34, dy: r.width * 0.34))
            p.append(outer)
            p.append(inner.reversed)
            p.append(NSBezierPath(rect: CGRect(x: r.maxX - r.width * 0.34, y: r.minY + r.width * 0.12,
                                               width: r.width * 0.22, height: r.width * 0.22)).reversed)
        case .macos:
            break
        }
        return p
    }
}
