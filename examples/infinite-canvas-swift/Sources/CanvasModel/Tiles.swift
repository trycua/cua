// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CoreGraphics
import Foundation

/// The operating system a Space runs, which decides its label and nothing
/// else.
public enum SpaceOS: String, Codable, Sendable, CaseIterable {
    case macos, linux, omarchy, windows

    public var label: String {
        switch self {
        case .macos: return "macOS"
        case .linux: return "Linux"
        case .omarchy: return "Omarchy"
        case .windows: return "Windows"
        }
    }
}

/// What a tile shows.
public enum TileKind: Equatable, Sendable {
    /// One window of a Space, streamed on its own.
    case window(spaceID: String, windowID: String)
    /// A Space's whole display.
    case desktop(spaceID: String)
    /// An agent thread (a conversation with an agent running in a Space).
    case thread(threadID: String)

    public var spaceID: String? {
        switch self {
        case let .window(id, _), let .desktop(id): return id
        case .thread: return nil
        }
    }

    public var isStream: Bool {
        if case .thread = self { return false }
        return true
    }
}

public struct Tile: Identifiable, Equatable, Sendable {
    public let id: String
    public var kind: TileKind
    public var title: String
    public var subtitle: String
    /// Position and size in world points.
    public var frame: CGRect
    /// Source pixels (stream surface), for aspect-correct resizing and for the
    /// level-of-detail pixel budget. `.zero` until known.
    public var sourcePixels: CGSize
    public var z: Int

    public init(id: String, kind: TileKind, title: String, subtitle: String = "",
                frame: CGRect, sourcePixels: CGSize = .zero, z: Int = 0) {
        self.id = id
        self.kind = kind
        self.title = title
        self.subtitle = subtitle
        self.frame = frame
        self.sourcePixels = sourcePixels
        self.z = z
    }

    /// Smallest a tile can be resized to, in world points.
    public static let minSize = CGSize(width: 160, height: 100)
}

/// The tiles on the canvas and every operation on them. A value type: the app
/// holds one and publishes changes, the tests drive it directly.
public struct CanvasLayout: Equatable, Sendable {
    public private(set) var tiles: [Tile] = []
    private var nextZ = 1

    public init(tiles: [Tile] = []) {
        for t in tiles { add(t) }
    }

    public func tile(_ id: String) -> Tile? { tiles.first { $0.id == id } }

    /// Tiles from front to back.
    public var frontToBack: [Tile] { tiles.sorted { $0.z > $1.z } }

    public mutating func add(_ tile: Tile) {
        var t = tile
        t.z = nextZ
        nextZ += 1
        tiles.removeAll { $0.id == t.id }
        tiles.append(t)
    }

    public mutating func remove(_ id: String) {
        tiles.removeAll { $0.id == id }
    }

    public mutating func bringToFront(_ id: String) {
        guard let i = tiles.firstIndex(where: { $0.id == id }) else { return }
        if tiles[i].z == nextZ - 1 { return }
        tiles[i].z = nextZ
        nextZ += 1
    }

    public mutating func move(_ id: String, by delta: CGVector) {
        guard let i = tiles.firstIndex(where: { $0.id == id }) else { return }
        tiles[i].frame = tiles[i].frame.offsetBy(dx: delta.dx, dy: delta.dy)
    }

    public mutating func setOrigin(_ id: String, _ origin: CGPoint) {
        guard let i = tiles.firstIndex(where: { $0.id == id }) else { return }
        tiles[i].frame.origin = origin
    }

    /// Resize from the bottom-right corner. Stream tiles keep their source
    /// aspect ratio (the stream is letterboxed otherwise); thread tiles resize
    /// freely. Never below `Tile.minSize`.
    public mutating func resize(_ id: String, to proposed: CGSize) {
        guard let i = tiles.firstIndex(where: { $0.id == id }) else { return }
        var size = CGSize(width: max(proposed.width, Tile.minSize.width),
                          height: max(proposed.height, Tile.minSize.height))
        let t = tiles[i]
        if t.kind.isStream, t.sourcePixels.width > 0, t.sourcePixels.height > 0 {
            let aspect = t.sourcePixels.width / t.sourcePixels.height
            size.height = size.width / aspect
            if size.height < Tile.minSize.height {
                size.height = Tile.minSize.height
                size.width = size.height * aspect
            }
        }
        tiles[i].frame.size = size
    }

    /// Record a stream's real size. The tile keeps its width and takes the
    /// source aspect, so a window never looks stretched.
    public mutating func setSourcePixels(_ id: String, _ pixels: CGSize) {
        guard let i = tiles.firstIndex(where: { $0.id == id }),
              pixels.width > 0, pixels.height > 0 else { return }
        if tiles[i].sourcePixels == pixels { return }
        tiles[i].sourcePixels = pixels
        let w = tiles[i].frame.width
        tiles[i].frame.size = CGSize(width: w, height: (w * pixels.height / pixels.width).rounded())
    }

    /// The front-most tile under a world point.
    public func hitTest(_ p: CGPoint) -> Tile? {
        frontToBack.first { $0.frame.contains(p) }
    }

    /// The union of every tile, for "fit all".
    public var bounds: CGRect {
        tiles.reduce(CGRect.null) { $0.union($1.frame) }
    }

    /// Lay tiles out in groups (one per Space, threads last), groups left to
    /// right, and each group as justified rows no wider than `groupWidth`.
    /// Rows keep each tile's aspect ratio; the row height is what makes the
    /// row exactly `groupWidth` wide, capped at `maxRowHeight`.
    public mutating func arrange(groups: [[String]], origin: CGPoint = .zero,
                                 groupWidth: CGFloat = 1800, gap: CGFloat = 40,
                                 groupGap: CGFloat = 160, maxRowHeight: CGFloat = 620,
                                 rowGap: CGFloat? = nil) {
        // Rows need room for the title strip above each tile, which grows
        // when zoomed out.
        let rowGap = rowGap ?? gap + 80
        var x = origin.x
        for group in groups {
            let members = group.compactMap { id in tiles.first { $0.id == id } }
            guard !members.isEmpty else { continue }
            let rows = Self.justifiedRows(members.map(\.aspect), width: groupWidth,
                                          gap: gap, maxRowHeight: maxRowHeight)
            var y = origin.y
            var k = 0
            var usedWidth: CGFloat = 0
            for row in rows {
                var rx = x
                for w in row.widths {
                    let id = members[k].id
                    if let i = tiles.firstIndex(where: { $0.id == id }) {
                        tiles[i].frame = CGRect(x: rx, y: y, width: w, height: row.height).integral
                    }
                    rx += w + gap
                    k += 1
                }
                usedWidth = max(usedWidth, rx - gap - x)
                y += row.height + rowGap
            }
            x += usedWidth + groupGap
        }
    }

    public struct Row: Equatable {
        public var height: CGFloat
        public var widths: [CGFloat]
    }

    /// Greedy justified rows: add items until the row, scaled to `width`,
    /// would be shorter than `maxRowHeight`.
    public static func justifiedRows(_ aspects: [CGFloat], width: CGFloat, gap: CGFloat,
                                     maxRowHeight: CGFloat) -> [Row] {
        var rows: [Row] = []
        var current: [CGFloat] = []
        func flush(justify: Bool) {
            guard !current.isEmpty else { return }
            let sum = current.reduce(0, +)
            let free = width - gap * CGFloat(current.count - 1)
            var h = free / sum
            if !justify || h > maxRowHeight { h = min(h, maxRowHeight) }
            rows.append(Row(height: h.rounded(), widths: current.map { ($0 * h).rounded() }))
            current = []
        }
        for a in aspects {
            current.append(max(a, 0.2))
            let sum = current.reduce(0, +)
            let free = width - gap * CGFloat(current.count - 1)
            if free / sum <= maxRowHeight { flush(justify: true) }
        }
        flush(justify: false)
        return rows
    }
}

extension Tile {
    /// Width over height, from the source when known.
    public var aspect: CGFloat {
        if sourcePixels.width > 0, sourcePixels.height > 0 {
            return sourcePixels.width / sourcePixels.height
        }
        return frame.height > 0 ? frame.width / frame.height : 16 / 10
    }
}
