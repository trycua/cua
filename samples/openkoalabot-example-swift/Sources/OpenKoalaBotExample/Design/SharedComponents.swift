// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import SwiftUI

// Components that outlived the phone surface.
//
// These were written for the mobile canvas and are still used by the desktop
// shell, the group chat and the Agent Computer panes. When `Mobile/` was
// deleted they moved here rather than being duplicated into each caller.

/// A 110pt circular icon button — the app's only chrome affordance.
struct CircleIcon: View {
    var symbol: String
    var dark: Bool = false
    var weight: Font.Weight = .medium
    var scale: CGFloat = 0.36
    var diameter: CGFloat = DS.iconButton

    var body: some View {
        ZStack {
            Circle().fill(dark ? DS.darkChip : DS.surface)
            Image(systemName: symbol)
                .font(.system(size: diameter * scale, weight: weight))
                .foregroundStyle(dark ? DS.onDark : DS.onSurface)
        }
        .frame(width: diameter, height: diameter)
    }
}

struct TypingIndicator: View {
    var tint: Color = DS.secondary
    /// Dot diameter. The default matches the mobile canvas; the desktop shell
    /// passes a smaller one.
    var dot: CGFloat = 16

    var body: some View {
        TimelineView(.animation(minimumInterval: 1.0 / 20.0, paused: false)) { context in
            let t = context.date.timeIntervalSinceReferenceDate
            HStack(spacing: dot * 0.7) {
                ForEach(0..<3, id: \.self) { i in
                    let phase = t * 2.4 - Double(i) * 0.35
                    let lift = max(0, sin(phase))
                    Circle()
                        .fill(tint.opacity(0.45 + 0.45 * lift))
                        .frame(width: dot, height: dot)
                        .offset(y: -dot * 0.45 * lift)
                }
            }
            .padding(.horizontal, dot * 1.5)
            .padding(.vertical, dot * 1.2)
            .background(Capsule().fill(DS.surface))
        }
        .accessibilityLabel("Working")
    }
}

/// A resolved reaction, as it sits under a bubble, for the live affordance.
/// `MessageRow` keeps its own inline copy of the chip's look because that one
/// is on the graded path and must not move.
struct ReactionChip: View {
    var emoji: String
    var count: Int = 1

    var body: some View {
        HStack(spacing: 8) {
            Text(emoji).font(DS.font(30))
            if count > 1 {
                Text("\(count)").font(DS.font(24, .medium)).foregroundStyle(DS.secondary)
            }
        }
        .padding(.horizontal, 14).padding(.vertical, 8)
        .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(DS.surface))
    }
}

struct FileGlyph: View {
    var kind: LinkFile.Kind

    private var tint: Color {
        switch kind {
        case .slides: return Color(hex: 0xF9AB00)
        case .doc:    return Color(hex: 0x4285F4)
        case .sheet:  return Color(hex: 0x0F9D58)
        case .pdf:    return Color(hex: 0xE02F2F)
        case .image:  return Color(hex: 0x8B5CF6)
        case .file:   return DS.secondary
        }
    }

    var body: some View {
        GeometryReader { g in
            let w = g.size.width, h = g.size.height
            let fold = w * 0.34
            ZStack(alignment: .topTrailing) {
                Path { p in
                    p.move(to: CGPoint(x: 0, y: 0))
                    p.addLine(to: CGPoint(x: w - fold, y: 0))
                    p.addLine(to: CGPoint(x: w, y: fold))
                    p.addLine(to: CGPoint(x: w, y: h))
                    p.addLine(to: CGPoint(x: 0, y: h))
                    p.closeSubpath()
                }
                .fill(tint)
                // The little white content mark inside the page.
                RoundedRectangle(cornerRadius: w * 0.06)
                    .fill(.white)
                    .frame(width: w * 0.46, height: h * 0.26)
                    .position(x: w * 0.5, y: h * 0.58)
            }
        }
    }
}

struct OptionalTap: ViewModifier {
    var action: (() -> Void)?

    func body(content: Content) -> some View {
        if let action {
            content
                .contentShape(Rectangle())
                .onTapGesture(perform: action)
        } else {
            content
        }
    }
}

/// Scrolls a column, or leaves it exactly as it was.
///
/// Same rule as `OptionalTap`, and found the same way: wrapping the desktop
/// roster in a `ScrollView` unconditionally moved all three graded desktop
/// PNGs even though the fixture content is far shorter than the viewport and
/// nothing could actually scroll. A `ScrollView` participates in layout
/// whether or not it has anything to do. So the export path must take a branch
/// with no `ScrollView` in it at all — not one with a `ScrollView` that
/// happens not to scroll. See `FRICTION.md` §49.
struct OptionalScroll<Content: View>: View {
    var scrolls: Bool
    @ViewBuilder var content: Content

    var body: some View {
        if scrolls {
            ScrollView(.vertical, showsIndicators: false) { content }
        } else {
            content
        }
    }
}

