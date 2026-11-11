// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CoreGraphics
import Foundation

/// What the hover card says about a tile.
public struct HoverInfo: Equatable, Sendable {
    /// The Space's name and its address or id (`direct:127.0.0.1:3211`,
    /// `local:…`, a cloud ref).
    public var spaceName: String = ""
    public var spaceAddress: String = ""
    /// Which OS mark to draw next to `os`.
    public var osKind: SpaceOS?
    /// `macOS 26.0`, `Ubuntu 24.04`: the Space's own report.
    public var os: String
    public var app: String
    public var title: String
    /// Media-socket round trips (ping to pong), milliseconds, oldest first.
    public var latencyMs: [Double]
    public var fps: Double
    public var resolution: CGSize

    public init(spaceName: String = "", spaceAddress: String = "", osKind: SpaceOS? = nil,
                os: String, app: String, title: String, latencyMs: [Double], fps: Double, resolution: CGSize) {
        self.spaceName = spaceName
        self.spaceAddress = spaceAddress
        self.osKind = osKind
        self.os = os
        self.app = app
        self.title = title
        self.latencyMs = latencyMs
        self.fps = fps
        self.resolution = resolution
    }

    /// `12 ms · 30 fps · 1280×800`, leaving out what is not known yet.
    public var statsLine: String {
        var parts: [String] = []
        if let last = latencyMs.last { parts.append("\(Int(last.rounded())) ms") }
        parts.append("\(Int(fps.rounded())) fps")
        if resolution.width > 0 { parts.append("\(Int(resolution.width))×\(Int(resolution.height))") }
        return parts.joined(separator: " · ")
    }

    /// `aurora · direct:127.0.0.1:3211`, or whichever half is known.
    public var spaceLine: String {
        [spaceName, spaceAddress].filter { !$0.isEmpty }.joined(separator: " · ")
    }

    /// The OS as the card shows it: the distribution and version, never a
    /// bare family word next to a specific distro ("Ubuntu 24.04", "macOS
    /// 26", "Omarchy", "Windows Server 2022"). The family name is used only
    /// when the Space reports nothing more specific.
    public static func osLabel(kind: SpaceOS, name: String, version: String) -> String {
        let n = name.trimmingCharacters(in: .whitespaces)
        let v = version.trimmingCharacters(in: .whitespaces)
        switch kind {
        case .omarchy:
            // Omarchy is an Arch Linux distribution; the kernel reports Arch.
            return "Omarchy"
        case .macos:
            let major = v.split(separator: ".").first.map(String.init) ?? ""
            return major.isEmpty ? "macOS" : "macOS \(major)"
        case .windows:
            if n.lowercased().hasPrefix("windows"), n.count > "windows".count {
                return n.contains(v) || v.isEmpty ? n : "\(n) \(v)"
            }
            return v.isEmpty ? "Windows" : "Windows \(v)"
        case .linux:
            let generic = ["", "linux", "gnu/linux", "unix"]
            guard !generic.contains(n.lowercased()) else { return "Linux" }
            let pretty = prettyDistro(n)
            return v.isEmpty || pretty.contains(v) ? pretty : "\(pretty) \(v)"
        }
    }

    static func prettyDistro(_ raw: String) -> String {
        let known = ["ubuntu": "Ubuntu", "debian": "Debian", "fedora": "Fedora", "arch": "Arch Linux",
                     "arch linux": "Arch Linux", "alpine": "Alpine", "nixos": "NixOS", "centos": "CentOS"]
        if let k = known[raw.lowercased()] { return k }
        return raw.prefix(1).uppercased() + raw.dropFirst()
    }

    /// Normalize samples into `0...1` for a sparkline (bottom = lowest). A
    /// flat line sits in the middle.
    public static func sparkline(_ samples: [Double]) -> [Double] {
        guard let lo = samples.min(), let hi = samples.max() else { return [] }
        guard hi - lo > 1e-9 else { return samples.map { _ in 0.5 } }
        return samples.map { ($0 - lo) / (hi - lo) }
    }
}

/// A bounded ring of recent values.
public struct SampleRing: Equatable, Sendable {
    public let capacity: Int
    public private(set) var values: [Double] = []

    public init(capacity: Int = 40) { self.capacity = capacity }

    public mutating func append(_ v: Double) {
        guard v.isFinite else { return }
        values.append(v)
        if values.count > capacity { values.removeFirst(values.count - capacity) }
    }
}

/// Which tile the stats card describes: the hovered tile when there is
/// one, otherwise the stream tile nearest the middle of the viewport (among
/// those on screen).
public enum InfoTarget {
    public static func pick(hovered: String?, tiles: [(id: String, frame: CGRect)],
                            viewport: CGRect) -> String? {
        if let hovered, tiles.contains(where: { $0.id == hovered }) { return hovered }
        let c = CGPoint(x: viewport.midX, y: viewport.midY)
        return tiles.filter { $0.frame.intersects(viewport) }
            .min { a, b in
                func d(_ r: CGRect) -> CGFloat {
                    // Distance from the center to the rect (0 inside it), then
                    // to its center, so a tile under the center always wins.
                    let dx = max(r.minX - c.x, 0, c.x - r.maxX), dy = max(r.minY - c.y, 0, c.y - r.maxY)
                    return (dx * dx + dy * dy) * 1e6 + hypot(r.midX - c.x, r.midY - c.y)
                }
                return (d(a.frame), a.id) < (d(b.frame), b.id)
            }?.id
    }
}
