// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CanvasModel
import SwiftUI

/// Where a thread view reads its agent's style: the canvas's presence
/// directory. The avatar background is the agent's cursor color by
/// construction: both read `PresenceDirectory` (see `AgentStyle`).
@MainActor
final class ThreadStyleSource: ObservableObject {
    @Published var style: AgentStyle
    init(style: AgentStyle) { self.style = style }
}

/// An agent thread as a canvas window: the transcript and a composer.
struct ThreadView: View {
    @ObservedObject var thread: AgentThread
    @ObservedObject var styleSource: ThreadStyleSource
    var onFocus: () -> Void
    @FocusState private var composerFocused: Bool

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider().opacity(0.4)
            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 8) {
                        ForEach(thread.messages) { m in
                            row(m).id(m.id)
                        }
                        if thread.isTurnRunning {
                            WorkingDots(color: .secondary)
                                .padding(.leading, 4)
                                .id(-1)
                        }
                    }
                    .padding(14)
                }
                .onChange(of: thread.messages.count) {
                    withAnimation(.smooth(duration: 0.25)) { proxy.scrollTo(thread.messages.last?.id, anchor: .bottom) }
                }
            }
            Divider().opacity(0.4)
            composer
        }
        .background(Color(nsColor: Palette.tileBackground))
    }

    @ViewBuilder
    private func row(_ m: ThreadMessage) -> some View {
        switch m.role {
        case .user:
            HStack {
                Spacer(minLength: 40)
                Text(m.text)
                    .font(.system(size: 13))
                    .foregroundStyle(.primary)
                    .padding(.horizontal, 12).padding(.vertical, 8)
                    .background(Color.white.opacity(0.10), in: .rect(cornerRadius: 14, style: .continuous))
            }
        case .agent:
            HStack {
                Text(m.text)
                    .font(.system(size: 13))
                    .foregroundStyle(.primary)
                    .padding(.horizontal, 12).padding(.vertical, 8)
                    .background(Color.white.opacity(0.05), in: .rect(cornerRadius: 14, style: .continuous))
                    .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(Color.white.opacity(0.08)))
                Spacer(minLength: 40)
            }
        case .tool:
            Text(m.text).font(.system(size: 12)).foregroundStyle(.secondary)
                .padding(.leading, 4)
        case .status:
            Text(m.text).font(.system(size: 12)).foregroundStyle(.secondary)
        }
    }

    /// The agent's avatar (its presence color) and what it is doing.
    private var header: some View {
        HStack(spacing: 8) {
            AgentAvatar(style: styleSource.style, size: 22)
            Text(styleSource.style.name).font(.system(size: 13, weight: .semibold))
            Spacer(minLength: 0)
            Text(thread.isTurnRunning ? "Working" : (thread.status.isEmpty ? "Idle" : thread.status))
                .font(.system(size: 12))
                .foregroundStyle(.secondary)
                .lineLimit(1)
        }
        .padding(.horizontal, 12)
        .frame(height: 40)
    }

    private var composer: some View {
        HStack(spacing: 8) {
            TextField("Message", text: $thread.draft)
                .textFieldStyle(.plain)
                .font(.system(size: 13))
                .focused($composerFocused)
                .onSubmit { thread.send() }
                .onChange(of: composerFocused) { if composerFocused { onFocus() } }
            Button {
                thread.send()
            } label: {
                Image(systemName: "arrow.up")
                    .font(.system(size: 12, weight: .semibold))
                    .frame(width: 22, height: 22)
            }
            .buttonStyle(.glassProminent)
            .buttonBorderShape(.circle)
            .disabled(thread.draft.trimmingCharacters(in: .whitespaces).isEmpty)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
    }
}

/// Three dots while a turn runs, in the agent's color.
struct WorkingDots: View {
    var color: Color
    var body: some View {
        TimelineView(.animation(minimumInterval: 1 / 30)) { ctx in
            let t = ctx.date.timeIntervalSinceReferenceDate
            HStack(spacing: 4) {
                ForEach(0 ..< 3, id: \.self) { i in
                    Circle().fill(color)
                        .frame(width: 6, height: 6)
                        .opacity(0.35 + 0.65 * max(0, sin(t * 4 - Double(i) * 0.7)))
                }
            }
        }
    }
}

/// An agent's avatar: the Cua cursor on a disc of the agent's presence
/// color, the same `AgentStyle` its cursor on the canvas is drawn with.
struct AgentAvatar: View {
    let style: AgentStyle
    var size: CGFloat = 22

    /// What the disc is filled with (the cursor's fill).
    var background: RGB { style.fill }

    var body: some View {
        ZStack {
            Circle().fill(background.color)
            CursorGlyph()
                .fill(style.text.color)
                .frame(width: size * 0.56, height: size * 0.56)
                .offset(x: size * 0.03, y: size * 0.03)
        }
        .frame(width: size, height: size)
        .accessibilityLabel(style.name)
    }
}

/// The body path of Cua Driver's default cursor (`action_idle`, layer
/// "Cursor body"), scaled into the rect.
struct CursorGlyph: Shape {
    private static let body: CGPath? = DotLottie.bundledTheme?.animations["action_idle"]?.layers
        .first { $0.name == "Cursor body" }?.items.first?.paths.first

    func path(in rect: CGRect) -> Path {
        guard let p = Self.body else { return Path(ellipseIn: rect) }
        let b = p.boundingBoxOfPath
        let s = min(rect.width / b.width, rect.height / b.height)
        var t = CGAffineTransform(translationX: rect.midX, y: rect.midY)
            .scaledBy(x: s, y: s)
            .translatedBy(x: -b.midX, y: -b.midY)
        return Path(p.copy(using: &t) ?? p)
    }
}
