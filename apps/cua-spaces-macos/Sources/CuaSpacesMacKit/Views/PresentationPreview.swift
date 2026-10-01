// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSDK
import CuaSpacesFFI
import SwiftUI

/// A "Where should Cua Spaces show up?" card's animated miniature: the
/// notch opening into the Space tiles, or the menu bar icon's menu dropping
/// down. The scene and every frame are the core's
/// (`appPresentationPreview`, `appPresentationPreviewFrame`), so the Tauri
/// app plays the same beats; this view only draws them, with the notch's
/// own shapes, tab label and tiles.
///
/// It loops while it is on screen in the key window, and stops otherwise.
/// With Reduce Motion it is the core's still: the expanded state.
struct PresentationPreview: View {
    let menuBar: Bool
    /// A fixed moment in the loop (snapshots); nil plays it.
    var fixedMs: UInt32?
    /// Overrides the system's Reduce Motion (tests).
    var stillOverride: Bool?
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.appearsActive) private var appearsActive
    @State private var onScreen = false
    @State private var origin = Date()

    var body: some View {
        let scene = Self.scene(menuBar)
        Group {
            if let fixedMs {
                PresentationPreviewCanvas(scene: scene, frame: appPresentationPreviewFrame(menuBar: menuBar, tMs: fixedMs))
            } else if stillOverride ?? reduceMotion {
                PresentationPreviewCanvas(scene: scene, frame: appPresentationPreviewStill(menuBar: menuBar))
            } else {
                TimelineView(.animation(minimumInterval: 1.0 / 30, paused: !onScreen || !appearsActive)) { context in
                    let elapsed = max(0, context.date.timeIntervalSince(origin)) * 1000
                    let ms = UInt32(elapsed.truncatingRemainder(dividingBy: Double(scene.loopMs)))
                    PresentationPreviewCanvas(scene: scene, frame: appPresentationPreviewFrame(menuBar: menuBar, tMs: ms))
                }
            }
        }
        .frame(width: scene.width, height: scene.height)
        // The desktop and the menu bar span the card; the stage sits in
        // them, centred under the notch or at the menu bar's trailing end.
        .frame(maxWidth: .infinity, alignment: menuBar ? .trailing : .center)
        .background(alignment: .top) {
            ZStack(alignment: .top) {
                LinearGradient(colors: [PreviewColors.wallpaperTop, PreviewColors.wallpaperBottom],
                               startPoint: .top, endPoint: .bottom)
                Rectangle().fill(PreviewColors.menuBar).frame(height: scene.menuBarHeight)
            }
        }
        // The loop starts when the card appears, not when the view is built:
        // the preview opens on its first frame however long the page took to
        // lay out.
        .onAppear {
            origin = Date()
            onScreen = true
        }
        .onDisappear { onScreen = false }
    }

    @MainActor private static var scenes: [Bool: AppPresentationPreview] = [:]

    @MainActor static func scene(_ menuBar: Bool) -> AppPresentationPreview {
        if let hit = scenes[menuBar] { return hit }
        let s = appPresentationPreview(menuBar: menuBar)
        scenes[menuBar] = s
        return s
    }
}

/// One frame of a miniature, on its stage (points, origin top left).
struct PresentationPreviewCanvas: View {
    let scene: AppPresentationPreview
    let frame: AppPreviewFrame

    var body: some View {
        ZStack(alignment: .topLeading) {
            Color.clear.frame(width: scene.width, height: scene.height)
            if let n = scene.notch { NotchMiniature(scene: scene, notch: n, frame: frame) }
            if let m = scene.menu { MenuMiniature(scene: scene, menu: m, frame: frame) }
            PreviewPointer(points: scene.pointer)
                .scaleEffect(frame.pressed ? 0.85 : 1, anchor: .topLeading)
                .offset(x: frame.pointer.x, y: frame.pointer.y)
        }
        .frame(width: scene.width, height: scene.height, alignment: .topLeading)
        .clipped()
        .accessibilityHidden(true)
    }
}

/// The miniature's desktop colours (the Tauri app's `--ob-preview-*`).
enum PreviewColors {
    static let wallpaperTop = dynamic(light: 0xDFE6EF, dark: 0x2A3240)
    static let wallpaperBottom = dynamic(light: 0xC6D0DD, dark: 0x1B212B)
    static let menuBar = Color(nsColor: NSColor(name: nil) { a in
        a.bestMatch(from: [.darkAqua]) == .darkAqua ? NSColor(white: 0, alpha: 0.28) : NSColor(white: 1, alpha: 0.55)
    })

    static func dynamic(light: UInt32, dark: UInt32) -> Color {
        func rgb(_ v: UInt32) -> NSColor {
            NSColor(srgbRed: CGFloat((v >> 16) & 0xFF) / 255, green: CGFloat((v >> 8) & 0xFF) / 255,
                    blue: CGFloat(v & 0xFF) / 255, alpha: 1)
        }
        return Color(nsColor: NSColor(name: nil) { a in a.bestMatch(from: [.darkAqua]) == .darkAqua ? rgb(dark) : rgb(light) })
    }
}

/// The standard arrow pointer from the core's outline: black, white edge.
struct PreviewPointer: View {
    let points: [AppPreviewPoint]

    var body: some View {
        let shape = PolygonShape(points: points)
        ZStack(alignment: .topLeading) {
            shape.stroke(.white, style: StrokeStyle(lineWidth: 1.6, lineJoin: .round))
            shape.fill(.black)
        }
        .frame(width: 12, height: 16, alignment: .topLeading)
        .shadow(color: .black.opacity(0.25), radius: 1, y: 0.5)
    }
}

struct PolygonShape: Shape {
    let points: [AppPreviewPoint]

    func path(in rect: CGRect) -> Path {
        var p = Path()
        guard let first = points.first else { return p }
        p.move(to: CGPoint(x: rect.minX + first.x, y: rect.minY + first.y))
        for pt in points.dropFirst() { p.addLine(to: CGPoint(x: rect.minX + pt.x, y: rect.minY + pt.y)) }
        p.closeSubpath()
        return p
    }
}

/// The notch card: the closed notch and its "N Spaces" tab morphing into
/// the open panel (the header row and the tiles, at real size, scaled).
struct NotchMiniature: View {
    let scene: AppPresentationPreview
    let notch: AppNotchPreview
    let frame: AppPreviewFrame

    var body: some View {
        let n = notch
        let o = CGFloat(frame.open)
        let lerp = { (a: Double, b: Double) in CGFloat(a + (b - a) * frame.open) }
        let w = lerp(n.closed.width, n.open.width)
        let h = lerp(n.closed.height, n.open.height)
        let top = lerp(n.closedRadii.top, n.openRadii.top)
        let bottom = lerp(n.closedRadii.bottom, n.openRadii.bottom)
        let x = (CGFloat(scene.width) - w) / 2
        let motion = NotchModel.motion
        let grow = 1 + (motion.hoverScale - 1) * frame.hover
        let c = CGFloat(frame.content)
        let contentScale = CGFloat(motion.contentScale) + (1 - CGFloat(motion.contentScale)) * c
        ZStack(alignment: .topLeading) {
            ZStack(alignment: .topLeading) {
                tab(n)
                NotchShape(top: top, bottom: bottom)
                    .fill(.black)
                    .shadow(color: .black.opacity(0.45 * min(max(o, 0), 1)), radius: 14 * n.scale, y: 6 * n.scale)
                    .frame(width: w, height: h)
                    .offset(x: x)
            }
            .frame(width: scene.width, height: scene.height, alignment: .topLeading)
            .scaleEffect(x: grow, y: 1, anchor: .top)
            ZStack(alignment: .top) {
                NotchMiniContent(notch: n)
                    .scaleEffect(n.scale, anchor: .topLeading)
                    .frame(width: n.open.width, height: n.open.height, alignment: .topLeading)
                    .scaleEffect(contentScale, anchor: .top)
                    .opacity(c)
            }
            .frame(width: w, height: h, alignment: .top)
            .clipShape(NotchShape(top: top, bottom: bottom))
            .offset(x: x)
        }
        .environment(\.colorScheme, .dark)
    }

    /// The tab right of the notch; it tucks under the notch as it opens.
    private func tab(_ n: AppNotchPreview) -> some View {
        let tuck = 20 * n.scale
        let shown = CGFloat(frame.tab)
        let ear = n.closedRadii.top
        return NotchTabShape(ear: ear, corner: 10 * n.scale).fill(.black)
            .frame(width: n.tab.width + tuck, height: n.tab.height)
            .overlay {
                NotchTabLabel(tab: n.view.tab)
                    .scaleEffect(n.scale)
                    .offset(x: (tuck - ear - 6 * n.scale) / 2)
            }
        .offset(x: n.tab.x - tuck - (1 - shown) * n.tab.width)
        .opacity(shown)
    }
}

/// The open panel's content at real size: the search left of the camera
/// housing, the buttons right of it, then the tiles.
struct NotchMiniContent: View {
    let notch: AppNotchPreview

    var body: some View {
        let n = notch
        let gap: CGFloat = 12
        let zone = max(0, (n.contentWidth - n.notch.width / n.scale) / 2 - n.side - gap)
        VStack(alignment: .leading, spacing: 0) {
            if let h = n.view.header {
                HStack(spacing: 0) {
                    HStack(spacing: 6) {
                        Image(systemName: "magnifyingglass")
                            .font(.system(size: 12, weight: .medium))
                            .foregroundStyle(.white.opacity(0.5))
                        Text(h.placeholder)
                            .font(.system(size: 13))
                            .foregroundStyle(.white.opacity(0.35))
                    }
                    .frame(width: zone, alignment: .leading)
                    Spacer(minLength: 0)
                    HStack(spacing: 2) {
                        Spacer(minLength: 0)
                        ForEach(h.buttons, id: \.symbol) { b in
                            NotchIconLabel(symbol: b.symbol, state: NotchInteractionState())
                        }
                    }
                    .frame(width: zone)
                }
                .frame(height: n.notchHeight)
            }
            Spacer().frame(height: 15)
            HStack(alignment: .top, spacing: 12) {
                ForEach(n.view.tiles, id: \.id) { tile in
                    // No live thumbnails here: the OS mark stands in.
                    TileView(tile: tile).overlay(alignment: .top) {
                        OsIconImage(id: tile.symbol, size: 26)
                            .foregroundStyle(.white.opacity(0.22))
                            .frame(height: 80)
                    }
                }
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, n.side)
        .frame(width: n.contentWidth, height: n.contentHeight, alignment: .topLeading)
    }
}

/// The menu bar card: the Cua Spaces icon and the clock at the menu bar's
/// right end, and the icon's menu (the core's items) when it drops down.
struct MenuMiniature: View {
    let scene: AppPresentationPreview
    let menu: AppMenuPreview
    let frame: AppPreviewFrame

    var body: some View {
        let m = menu
        ZStack(alignment: .topLeading) {
            if frame.active {
                RoundedRectangle(cornerRadius: 3, style: .continuous)
                    .fill(Color.primary.opacity(0.14))
                    .frame(width: m.highlight.width, height: m.highlight.height)
                    .offset(x: m.highlight.x, y: m.highlight.y)
            }
            Group {
                if let icon = Self.icon {
                    Image(nsImage: icon).resizable().renderingMode(.template)
                } else {
                    Color.clear
                }
            }
            .foregroundStyle(.primary)
            .frame(width: m.icon.width, height: m.icon.height)
            .offset(x: m.icon.x, y: m.icon.y)
            Text(m.clock)
                .font(.system(size: 6.5, weight: .medium))
                .monospacedDigit()
                .fixedSize()
                .frame(width: 40, height: scene.menuBarHeight, alignment: .trailing)
                .offset(x: m.clockRight - 40)
            if frame.open > 0 {
                menuBody(m)
                    .offset(x: m.menu.x, y: m.menu.y)
                    .opacity(frame.open)
            }
        }
    }

    private func menuBody(_ m: AppMenuPreview) -> some View {
        ZStack(alignment: .topLeading) {
            RoundedRectangle(cornerRadius: m.radius, style: .continuous)
                .fill(Color(nsColor: .windowBackgroundColor))
                .overlay(RoundedRectangle(cornerRadius: m.radius, style: .continuous)
                    .strokeBorder(Color(nsColor: .separatorColor), lineWidth: 0.5))
                .shadow(color: .black.opacity(0.18), radius: 6, y: 3)
                .frame(width: m.menu.width, height: m.menu.height)
            ForEach(Array(m.rows.enumerated()), id: \.offset) { i, row in
                self.row(row, highlighted: frame.highlighted == UInt32(i), m: m)
                    .frame(width: row.frame.width, height: row.frame.height)
                    .offset(x: row.frame.x - m.menu.x, y: row.frame.y - m.menu.y)
            }
        }
        .frame(width: m.menu.width, height: m.menu.height, alignment: .topLeading)
    }

    @ViewBuilder private func row(_ row: AppMenuPreviewRow, highlighted: Bool, m: AppMenuPreview) -> some View {
        if row.item.id == .separator {
            Rectangle()
                .fill(Color(nsColor: .separatorColor))
                .frame(height: 0.5)
                .padding(.horizontal, m.inset)
        } else {
            HStack(spacing: 4) {
                Text(row.item.label)
                Spacer(minLength: 0)
                if let k = row.item.shortcut {
                    Text(k).foregroundStyle(highlighted ? AnyShapeStyle(.white) : AnyShapeStyle(.secondary))
                }
            }
            .font(.system(size: m.fontSize))
            .foregroundStyle(highlighted ? AnyShapeStyle(.white) : row.item.enabled ? AnyShapeStyle(.primary) : AnyShapeStyle(.secondary))
            .lineLimit(1)
            .padding(.horizontal, m.inset)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background {
                if highlighted {
                    RoundedRectangle(cornerRadius: 3, style: .continuous)
                        .fill(Color.accentColor)
                        .padding(.horizontal, m.inset / 2)
                }
            }
        }
    }

    /// The menu bar icon (the tray template both apps ship).
    static let icon: NSImage? = {
        guard let url = ModuleResources.url(forResource: "tray-template@2x", withExtension: "png"),
              let image = NSImage(contentsOf: url) else { return nil }
        image.isTemplate = true
        image.size = NSSize(width: 18, height: 18)
        return image
    }()
}
