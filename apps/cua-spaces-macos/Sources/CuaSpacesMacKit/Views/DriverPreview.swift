// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSDK
import CuaSpacesFFI
import SwiftUI

/// The AI agents page's background computer-use card: its animated
/// miniature over one checkbox line, inside the page's own card (no second
/// frame around it). The miniature is a back window where
/// the cua-driver agent cursor ticks boxes while the user's own pointer
/// selects text in a front window, both at once. The scene and every frame
/// are the core's (`appDriverPreview`, `appDriverPreviewFrame`), so the
/// Tauri app plays the same beats; this view only draws them, like the
/// presentation cards' `PresentationPreview`.
struct DriverCardView: View {
    let title: String
    let imageLabel: String
    @Binding var isOn: Bool
    var fixedMs: UInt32?
    /// Overrides the system's Reduce Motion (tests).
    var still: Bool?

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            DriverPreview(fixedMs: fixedMs, stillOverride: still)
                .clipShape(.rect(cornerRadius: 6))
                .accessibilityElement(children: .ignore)
                .accessibilityLabel(imageLabel)
            Toggle(title, isOn: $isOn)
                .toggleStyle(.checkbox)
                .lineLimit(1)
                .accessibilityIdentifier("agents-driver")
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// The card's miniature. It loops while it is on screen in the key window
/// and stops otherwise; with Reduce Motion it is the core's still.
struct DriverPreview: View {
    /// A fixed moment in the loop (snapshots); nil plays it.
    var fixedMs: UInt32?
    /// Overrides the system's Reduce Motion (tests).
    var stillOverride: Bool?
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.appearsActive) private var appearsActive
    @State private var onScreen = false
    @State private var origin = Date()

    var body: some View {
        let scene = Self.scene
        Group {
            if let fixedMs {
                DriverPreviewCanvas(scene: scene, frame: appDriverPreviewFrame(tMs: fixedMs))
            } else if stillOverride ?? reduceMotion {
                DriverPreviewCanvas(scene: scene, frame: appDriverPreviewStill())
            } else {
                TimelineView(.animation(minimumInterval: 1.0 / 30, paused: !onScreen || !appearsActive)) { context in
                    let elapsed = max(0, context.date.timeIntervalSince(origin)) * 1000
                    let ms = UInt32(elapsed.truncatingRemainder(dividingBy: Double(scene.loopMs)))
                    DriverPreviewCanvas(scene: scene, frame: appDriverPreviewFrame(tMs: ms))
                }
            }
        }
        .frame(width: scene.width, height: scene.height)
        // The desktop spans the card; the stage sits centred in it.
        .frame(maxWidth: .infinity)
        .background {
            LinearGradient(colors: [PreviewColors.wallpaperTop, PreviewColors.wallpaperBottom],
                           startPoint: .top, endPoint: .bottom)
        }
        .onAppear {
            origin = Date()
            onScreen = true
        }
        .onDisappear { onScreen = false }
    }

    @MainActor static let scene = appDriverPreview()
}

/// One frame of the miniature, on its stage (points, origin top left):
/// the back window, the agent cursor working in it, the front window over
/// both, then the user's pointer.
struct DriverPreviewCanvas: View {
    let scene: AppDriverPreview
    let frame: AppDriverFrame

    var body: some View {
        ZStack(alignment: .topLeading) {
            Color.clear.frame(width: scene.width, height: scene.height)
            MiniWindow(window: scene.back) {
                ForEach(Array(scene.checkboxes.enumerated()), id: \.offset) { i, box in
                    MiniCheckbox(tick: i < frame.checked.count ? frame.checked[i] : 0)
                        .frame(width: box.width, height: box.height)
                        .offset(x: box.x - scene.back.frame.x, y: box.y - scene.back.frame.y)
                }
                ForEach(Array(scene.labels.enumerated()), id: \.offset) { _, bar in
                    MiniBar(rect: bar, origin: scene.back.frame)
                }
            }
            AgentCursor(scene: scene, frame: frame)
            MiniWindow(window: scene.front) {
                if Int(scene.selectedLine) < scene.lines.count, frame.selection > 0 {
                    let line = scene.lines[Int(scene.selectedLine)]
                    RoundedRectangle(cornerRadius: 1.5)
                        .fill(Color.accentColor.opacity(0.3))
                        .frame(width: line.width * frame.selection, height: 8)
                        .offset(x: line.x - scene.front.frame.x, y: line.y + line.height / 2 - 4 - scene.front.frame.y)
                }
                ForEach(Array(scene.lines.enumerated()), id: \.offset) { _, bar in
                    MiniBar(rect: bar, origin: scene.front.frame)
                }
            }
            PreviewPointer(points: scene.pointer)
                .scaleEffect(frame.pressed ? 0.85 : 1, anchor: .topLeading)
                .offset(x: frame.pointer.x, y: frame.pointer.y)
        }
        .frame(width: scene.width, height: scene.height, alignment: .topLeading)
        .clipped()
        .accessibilityHidden(true)
    }
}

/// A plain window: rounded, hairline edge, soft shadow, a title bar with the
/// three window buttons; `content` is laid out in the window's own points.
struct MiniWindow<Content: View>: View {
    let window: AppPreviewWindow
    @ViewBuilder let content: () -> Content

    var body: some View {
        let f = window.frame
        ZStack(alignment: .topLeading) {
            RoundedRectangle(cornerRadius: window.radius, style: .continuous)
                .fill(Color(nsColor: .windowBackgroundColor))
                .shadow(color: .black.opacity(0.18), radius: 4, y: 2)
            Rectangle()
                .fill(Color(nsColor: .separatorColor).opacity(0.5))
                .frame(height: 0.5)
                .offset(y: window.titleBar)
            HStack(spacing: 2.5) {
                ForEach(0..<3, id: \.self) { _ in
                    Circle().fill(Color.secondary.opacity(0.35)).frame(width: 3.6, height: 3.6)
                }
            }
            .offset(x: 5, y: (window.titleBar - 3.6) / 2)
            content()
            RoundedRectangle(cornerRadius: window.radius, style: .continuous)
                .strokeBorder(Color(nsColor: .separatorColor), lineWidth: 0.5)
        }
        .frame(width: f.width, height: f.height, alignment: .topLeading)
        .clipShape(.rect(cornerRadius: window.radius, style: .continuous))
        .offset(x: f.x, y: f.y)
    }
}

/// A placeholder text bar.
struct MiniBar: View {
    let rect: AppLogicalRect
    let origin: AppLogicalRect

    var body: some View {
        Capsule()
            .fill(Color.secondary.opacity(0.28))
            .frame(width: rect.width, height: rect.height)
            .offset(x: rect.x - origin.x, y: rect.y - origin.y)
    }
}

/// A checkbox whose tick fades and fills in with `tick` (0 to 1).
private struct MiniCheckbox: View {
    let tick: Double

    var body: some View {
        ZStack {
            RoundedRectangle(cornerRadius: 2)
                .strokeBorder(Color.secondary.opacity(0.6), lineWidth: 0.75)
            RoundedRectangle(cornerRadius: 2)
                .fill(Color.accentColor)
                .opacity(tick)
            Path { p in
                p.move(to: CGPoint(x: 2, y: 4.2))
                p.addLine(to: CGPoint(x: 3.5, y: 5.7))
                p.addLine(to: CGPoint(x: 6.2, y: 2.4))
            }
            .trim(from: 0, to: tick)
            .stroke(.white, style: StrokeStyle(lineWidth: 1.1, lineCap: .round, lineJoin: .round))
        }
    }
}

/// The cua-driver agent cursor: the driver theme's arrow in its fill with a
/// white edge and a soft glow of the fill, and its click rays.
private struct AgentCursor: View {
    let scene: AppDriverPreview
    let frame: AppDriverFrame

    var body: some View {
        let fill = Self.color(scene.agentFill)
        let shape = PolygonShape(points: scene.agentPointer)
        // The outline's tip is at the origin; it spreads right and down.
        let box = CGSize(width: 16, height: 16)
        ZStack(alignment: .topLeading) {
            if frame.ripple > 0 {
                Path { p in
                    for r in scene.agentRays {
                        p.move(to: CGPoint(x: r.from.x, y: r.from.y))
                        p.addLine(to: CGPoint(x: r.to.x, y: r.to.y))
                    }
                }
                .stroke(fill, style: StrokeStyle(lineWidth: 1.2, lineCap: .round))
                .scaleEffect(1 + 0.4 * frame.ripple, anchor: .topLeading)
                .opacity(1 - frame.ripple)
            }
            ZStack(alignment: .topLeading) {
                shape.stroke(.white, style: StrokeStyle(lineWidth: 1.4, lineJoin: .round))
                shape.fill(fill)
            }
            .frame(width: box.width, height: box.height, alignment: .topLeading)
            .shadow(color: fill.opacity(0.7), radius: 2.5)
            .scaleEffect(frame.agentPressed ? 0.85 : 1, anchor: .topLeading)
        }
        .frame(width: box.width, height: box.height, alignment: .topLeading)
        .offset(x: frame.agent.x, y: frame.agent.y)
    }

    /// `#RRGGBB`.
    static func color(_ hex: String) -> Color {
        let v = UInt32(hex.trimmingCharacters(in: CharacterSet(charactersIn: "#")), radix: 16) ?? 0x5EC0E8
        return Color(.sRGB, red: Double((v >> 16) & 0xFF) / 255, green: Double((v >> 8) & 0xFF) / 255,
                     blue: Double(v & 0xFF) / 255)
    }
}
