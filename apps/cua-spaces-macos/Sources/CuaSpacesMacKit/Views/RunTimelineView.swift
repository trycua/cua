// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import CuaSpacesFFI
import SwiftUI

/// A run's conversation, as the web UI's `Timeline`: the prompt as a bubble
/// on the right, the agent's prose, and each stretch of activity as a muted
/// group that shows its summary and last step until opened. It follows the
/// end while the run writes; scrolled up, a pill brings the reader back.
struct RunTimelineView: View {
    let timeline: RunTimelineModel
    @Environment(\.dismiss) private var dismiss
    /// Activity groups the reader opened or closed (by item index).
    @State private var toggled: Set<Int> = []
    @State private var position = ScrollPosition(edge: .bottom)
    @State private var atEnd = true

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
            content.frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .frame(minWidth: 560, idealWidth: 720, minHeight: 420, idealHeight: 640)
        .task { await timeline.poll() }
    }

    private var header: some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                Text(appAgentName(agent: timeline.run.agent)).font(.headline)
                Text(appAgentSubtitle(run: timeline.run))
                    .font(.caption).foregroundStyle(.secondary).lineLimit(1).truncationMode(.tail)
            }
            Spacer(minLength: 8)
            Text(appAgentStatusLabel(status: timeline.status)).font(.caption).foregroundStyle(.secondary)
            Button("Done") { dismiss() }.keyboardShortcut(.defaultAction)
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 12)
    }

    @ViewBuilder private var content: some View {
        if timeline.items.isEmpty, let error = timeline.error {
            centered("Couldn't read this run", error)
        } else if timeline.items.isEmpty, timeline.loading {
            ProgressView().controlSize(.small)
        } else {
            ZStack(alignment: .bottom) {
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 4) {
                        ForEach(Array(timeline.items.enumerated()), id: \.offset) { i, item in
                            row(i, item).frame(maxWidth: 720, alignment: .leading)
                        }
                        if let footer = timeline.footer {
                            Text(footer.text)
                                .font(.caption)
                                .foregroundStyle(footer.failed ? AnyShapeStyle(.red) : AnyShapeStyle(.secondary))
                                .padding(.top, 4)
                                .frame(maxWidth: 720, alignment: .leading)
                                .accessibilityIdentifier("run-timeline-footer")
                        }
                    }
                    .padding(.horizontal, 32)
                    .padding(.vertical, 24)
                    .frame(maxWidth: .infinity)
                }
                .scrollPosition($position)
                .defaultScrollAnchor(.bottom, for: .sizeChanges)
                .onScrollGeometryChange(for: Bool.self) { g in
                    g.contentOffset.y + g.containerSize.height >= g.contentSize.height - 32
                } action: { _, now in
                    atEnd = now
                }
                if !atEnd {
                    Button {
                        withAnimation { position.scrollTo(edge: .bottom) }
                    } label: {
                        Label(timeline.live ? "Follow the run" : "Jump to latest", systemImage: "arrow.down")
                    }
                    .buttonBorderShape(.capsule)
                    .controlSize(.small)
                    .padding(.bottom, 16)
                }
            }
            .accessibilityIdentifier("run-timeline")
        }
    }

    @ViewBuilder private func row(_ i: Int, _ item: AgentTranscriptItem) -> some View {
        switch item.kind {
        case "user":
            HStack {
                Spacer(minLength: 80)
                Text(item.text)
                    .textSelection(.enabled)
                    .padding(.horizontal, 14)
                    .padding(.vertical, 8)
                    .background(.quaternary, in: UnevenRoundedRectangle(topLeadingRadius: 16, bottomLeadingRadius: 16,
                                                                        bottomTrailingRadius: 6, topTrailingRadius: 16))
            }
            .padding(.vertical, 4)
        case "message":
            Text(Self.markdown(item.text))
                .textSelection(.enabled)
                .padding(.vertical, 4)
        default:
            activity(i, item).padding(.vertical, 2)
        }
    }

    private func activity(_ i: Int, _ item: AgentTranscriptItem) -> some View {
        let open = (i == timeline.openActivity) != toggled.contains(i)
        return VStack(alignment: .leading, spacing: 4) {
            Button {
                if toggled.contains(i) { toggled.remove(i) } else { toggled.insert(i) }
            } label: {
                HStack(spacing: 6) {
                    Image(systemName: "chevron.right")
                        .font(.system(size: 9, weight: .semibold))
                        .rotationEffect(.degrees(open ? 90 : 0))
                    Text(item.text).fontWeight(.medium)
                    if !open, let last = item.steps.last {
                        Text(last).foregroundStyle(.tertiary).lineLimit(1).truncationMode(.tail)
                    }
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityValue(open ? "expanded" : "collapsed")
            if open {
                VStack(alignment: .leading, spacing: 2) {
                    ForEach(Array(item.steps.enumerated()), id: \.offset) { _, step in
                        Text(step)
                            .font(.system(size: 11.5, design: .monospaced))
                            .lineLimit(1).truncationMode(.tail)
                            .help(step)
                    }
                }
                .padding(.leading, 14)
                .overlay(alignment: .leading) { Rectangle().fill(.separator).frame(width: 1).padding(.leading, 5) }
            }
        }
        .font(.caption)
        .foregroundStyle(.secondary)
    }

    private func centered(_ title: String, _ detail: String) -> some View {
        VStack(spacing: 6) {
            Text(title).font(.headline)
            Text(detail).font(.callout).foregroundStyle(.secondary).multilineTextAlignment(.center)
        }
        .frame(maxWidth: 360)
        .padding(32)
    }

    /// The agent's Markdown, inline styles and links; blocks keep their lines.
    static func markdown(_ text: String) -> AttributedString {
        (try? AttributedString(markdown: text, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace)))
            ?? AttributedString(text)
    }
}
