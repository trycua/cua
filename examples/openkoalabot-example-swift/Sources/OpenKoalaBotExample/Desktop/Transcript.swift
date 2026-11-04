// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

#if canImport(AppKit)
import AppKit
import CuaSpaces
import CuaSpacesStreaming
import SwiftUI

// MARK: - Transcript row

/// One transcript row.
///
/// The user's turns sit on the right in a rounded bubble. The Bot's replies
/// are plain text on the left with no bubble, and a small row of actions
/// under them. Cards (files, approvals, choices) are bordered panels.
struct TranscriptRow: View {
    var message: Message
    var theme: DesktopTheme
    var columnWidth: CGFloat
    var onChoose: (String) -> Void = { _ in }
    var onDismissChoices: () -> Void = {}

    /// The widest a user bubble may be.
    private var maxBubble: CGFloat { Metrics.bubbleMaxWidth(in: columnWidth) }
    private var opposingGutter: CGFloat { max(0, columnWidth - maxBubble) }

    var body: some View {
        switch message.body {
        case .systemEvent(let t):
            Text(t).font(.system(size: 11)).foregroundStyle(theme.secondary)
                .frame(maxWidth: .infinity, alignment: .center)

        case .prose(let t):
            if message.sender == .user {
                HStack(spacing: 0) {
                    Spacer(minLength: opposingGutter)
                    Text(t).font(.system(size: 14)).foregroundStyle(theme.text)
                        .lineSpacing(3)
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                        .padding(.horizontal, 16).padding(.vertical, 10)
                        .background(RoundedRectangle(cornerRadius: 20, style: .continuous)
                            .fill(theme.userBubble))
                }
            } else {
                VStack(alignment: .leading, spacing: 6) {
                    reply(t)
                    ReplyActions(text: t, theme: theme, reaction: message.reaction)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }

        case .choices(let card):
            ChoiceCardView(card: card, theme: theme,
                           onChoose: onChoose, onDismiss: onDismissChoices)
                .frame(maxWidth: min(columnWidth, 560), alignment: .leading)

        case .linkFile(let lf):
            panel {
                HStack(spacing: 10) {
                    FileGlyph(kind: lf.kind).frame(width: 22, height: 27)
                    VStack(alignment: .leading, spacing: 1) {
                        Text(lf.title).font(.system(size: 13, weight: .semibold))
                            .foregroundStyle(theme.text)
                        Text(lf.subtitle).font(.system(size: 12))
                            .foregroundStyle(theme.secondary)
                    }
                    Spacer(minLength: 0)
                }
            }

        case .card(let c):
            panel {
                Text(c.title).font(.system(size: 13, weight: .semibold)).foregroundStyle(theme.text)
                ForEach(Array(c.bodyLines.enumerated()), id: \.offset) { _, l in
                    Text(l).font(.system(size: 13)).foregroundStyle(theme.text)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
            }

        case .approval(let a):
            panel {
                Label(a.title, systemImage: "lock.shield")
                    .font(.system(size: 13, weight: .semibold)).foregroundStyle(theme.text)
                Text(a.detail).font(.system(size: 13)).foregroundStyle(theme.secondary)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }

        case .activity(let group):
            ActivityGroupRow(group: group, theme: theme)

        case .computerStatus(let cs):
            HStack(spacing: 8) {
                Circle().fill(cs.active ? theme.accent : theme.secondary)
                    .frame(width: 7, height: 7)
                Text(cs.text).font(.system(size: 12, weight: .medium)).foregroundStyle(theme.secondary)
                Spacer(minLength: 0)
            }
        }
    }

    /// The Bot's reply. Markdown goes through `MarkdownText`; a plain sentence
    /// is one `Text`. Only the Bot's side is parsed: what the user typed is
    /// shown exactly as typed.
    @ViewBuilder private func reply(_ t: String) -> some View {
        if MarkdownParser.isPlain(t) {
            Text(t).font(.system(size: 14)).foregroundStyle(theme.text)
                .lineSpacing(4)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
        } else {
            MarkdownText(source: t, theme: theme, maxWidth: columnWidth)
                .frame(maxWidth: columnWidth, alignment: .leading)
        }
    }

    @ViewBuilder private func panel<C: View>(@ViewBuilder _ c: () -> C) -> some View {
        HStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 8) { c() }
                .padding(.horizontal, 14).padding(.vertical, 12)
                .frame(maxWidth: min(columnWidth, 560), alignment: .leading)
                .background(RoundedRectangle(cornerRadius: 14, style: .continuous)
                    .fill(theme.inset))
                .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous)
                    .strokeBorder(theme.divider))
            Spacer(minLength: 0)
        }
    }
}

/// What the agent did between messages, muted: small secondary text, no
/// bubble, one line per step. Collapsed it is one line (the SDK's summary,
/// `5 steps`, or the step itself when there is only one); a click shows
/// every step. Like the tool and status rows of agent chat apps.
struct ActivityGroupRow: View {
    var group: ActivityGroup
    var theme: DesktopTheme
    /// Starts open.
    var expanded = false
    @State private var open: Bool?

    /// A capture run opens the groups with a step containing this text
    /// (`OPENKOALABOTS_THREAD_CAPTURE`); `nil` in normal use.
    nonisolated(unsafe) static var openContaining: String?

    private var isOpen: Bool {
        open ?? (expanded || Self.openContaining.map { t in group.steps.contains { $0.contains(t) } } ?? false)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            if group.steps.count == 1 {
                line(group.steps[0])
            } else {
                Button { open = !isOpen } label: {
                    HStack(spacing: 5) {
                        Image(systemName: isOpen ? "chevron.down" : "chevron.right")
                            .font(.system(size: 8, weight: .semibold))
                            .frame(width: 9)
                        Text(group.summary).font(.system(size: 11))
                            .lineLimit(1)
                        Spacer(minLength: 0)
                    }
                    .foregroundStyle(theme.secondary)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(group.summary)
                .accessibilityHint(isOpen ? "Hides the steps" : "Shows the steps")
                if isOpen {
                    ForEach(Array(group.steps.enumerated()), id: \.offset) { _, step in
                        line(step).padding(.leading, 14)
                    }
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func line(_ text: String) -> some View {
        Text(text).font(.system(size: 11)).foregroundStyle(theme.secondary.opacity(0.9))
            .lineLimit(1).truncationMode(.tail)
            .frame(maxWidth: .infinity, alignment: .leading)
            .help(text)
    }
}

/// The small actions under a reply.
struct ReplyActions: View {
    var text: String
    var theme: DesktopTheme
    var reaction: String?
    @State private var copied = false

    var body: some View {
        HStack(spacing: 2) {
            action(copied ? "checkmark" : "doc.on.doc", copied ? "Copied" : "Copy") {
                #if canImport(AppKit)
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(text, forType: .string)
                #endif
                copied = true
                DispatchQueue.main.asyncAfter(deadline: .now() + 1.2) { copied = false }
            }
            if let reaction {
                Text(reaction).font(.system(size: 12)).padding(.leading, 6)
            }
        }
    }

    private func action(_ symbol: String, _ label: String, _ run: @escaping () -> Void) -> some View {
        Button(action: run) {
            Image(systemName: symbol).font(.system(size: 12))
                .foregroundStyle(theme.secondary)
                .frame(width: 26, height: 24)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .help(label)
        .accessibilityLabel(label)
    }
}

// MARK: - Choice card

/// The lettered choice panel, as one reusable component.
///
/// Both places the app shows one (a Bot's opener and a new Bot's
/// onboarding question) are the same component with different content, so
/// this takes a `ChoiceCard` and nothing else. The letters come from the
/// index, so an option cannot be given the wrong one.
struct ChoiceCardView: View {
    var card: ChoiceCard
    var theme: DesktopTheme
    var onChoose: (String) -> Void = { _ in }
    var onDismiss: () -> Void = {}
    @State private var ownAnswer = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .top, spacing: 10) {
                Text(card.heading)
                    .font(DS.font(12, .semibold)).foregroundStyle(theme.text)
                    .fixedSize(horizontal: false, vertical: true)
                Spacer(minLength: 8)
                Image(systemName: "xmark")
                    .font(.system(size: 10, weight: .medium))
                    .foregroundStyle(theme.secondary)
                    .contentShape(Rectangle())
                    .onTapGesture(perform: onDismiss)
                    .accessibilityLabel("Dismiss")
            }

            VStack(spacing: 6) {
                ForEach(Array(card.options.enumerated()), id: \.offset) { i, option in
                    HStack(spacing: 10) {
                        Text(ChoiceCard.letter(i))
                            .font(DS.font(10, .semibold))
                            .foregroundStyle(theme.text)
                            .frame(width: 18, height: 18)
                            .background(RoundedRectangle(cornerRadius: Corner.base - 2,
                                                         style: .continuous)
                                .fill(theme.inset))
                        Text(option).font(DS.font(12)).foregroundStyle(theme.text)
                            .fixedSize(horizontal: false, vertical: true)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                    .padding(.horizontal, 10).padding(.vertical, 8)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(RoundedRectangle(cornerRadius: Corner.xl, style: .continuous)
                        .fill(card.resolved == option ? theme.selectedRow : theme.inset))
                    .contentShape(Rectangle())
                    .onTapGesture { if card.resolved == nil { onChoose(option) } }
                }
            }

            HStack(spacing: 8) {
                TextField(card.freeTextPlaceholder, text: $ownAnswer)
                    .textFieldStyle(.plain)
                    .font(DS.font(12))
                    .foregroundStyle(theme.text)
                    .onSubmit {
                        let t = ownAnswer.trimmingCharacters(in: .whitespacesAndNewlines)
                        guard !t.isEmpty else { return }
                        ownAnswer = ""
                        onChoose(t)
                    }
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 10).frame(height: 30)
            .background(RoundedRectangle(cornerRadius: Corner.xl, style: .continuous)
                .fill(theme.inset))
        }
        .padding(14)
        .background(RoundedRectangle(cornerRadius: Corner.bubble, style: .continuous)
            .fill(theme.botCard))
    }
}

// MARK: - Typing indicator

/// Three dots in a bubble, on the shell's own clock.
///
/// `TimelineView`, not an implicit animation, for the reason `FRICTION.md` §11
/// and §46 both record: `ImageRenderer` has no run loop, so a live modifier on
/// a render path is either inert or moves the render.
struct ShellTypingIndicator: View {
    var theme: DesktopTheme
    var fixedTime: Double? = nil

    /// The dot's vertical offset at a moment. Pure, so it is testable.
    static func offset(index: Int, time: Double) -> CGFloat {
        let phase = time * 2.6 - Double(index) * 0.32
        return CGFloat(-2.4 * max(0, sin(phase)))
    }

    var body: some View {
        TimelineView(.animation(minimumInterval: 1.0 / 30.0, paused: fixedTime != nil)) { ctx in
            let t = fixedTime ?? ctx.date.timeIntervalSinceReferenceDate
            HStack(spacing: 4) {
                ForEach(0..<3, id: \.self) { i in
                    Circle().fill(theme.secondary)
                        .frame(width: 5, height: 5)
                        .offset(y: Self.offset(index: i, time: t))
                }
            }
            .padding(.vertical, 8)
        }
        .accessibilityLabel("Typing")
    }
}

#endif
