// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpaces
import CuaSpacesStreaming
#if canImport(AppKit)
import AppKit
import SwiftUI

/// The Space's windows in the Computer pane: one line each, the app's own
/// icon (from the Space, `WindowIcons`) left of the name, with a button that
/// pops that window out into a floating panel of its own (`StreamPiPSet`).
/// Still draggable onto a stream pane.
struct SpaceWindowList: View {
    @ObservedObject var session: LiveStreamSession
    @ObservedObject var pips: StreamPiPSet
    @ObservedObject var icons: WindowIcons
    let p: KoalaPalette
    let onPopOut: (StreamWindow) -> Void

    /// Rows shown before the list scrolls.
    static let visibleRows = 6
    static let rowHeight: CGFloat = 26

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("Windows").font(.system(size: 11, weight: .semibold)).foregroundStyle(p.secondary)
            if session.windows.isEmpty {
                Text("No windows").font(.system(size: 12)).foregroundStyle(p.secondary)
                    .frame(height: Self.rowHeight)
            } else {
                ScrollView {
                    VStack(spacing: 0) {
                        ForEach(session.windows) { row($0) }
                    }
                }
                .frame(height: CGFloat(min(session.windows.count, Self.visibleRows)) * Self.rowHeight)
            }
        }
        .task { await session.refreshWindows() }
        .task(id: session.windows.map(WindowIcons.key)) { await icons.load(session.windows) }
    }

    private func row(_ window: StreamWindow) -> some View {
        let open = pips.isOpen(.window(window))
        return HStack(spacing: 6) {
            if let image = icons.image(for: window) {
                Image(nsImage: image).resizable().interpolation(.high)
                    .frame(width: Self.iconSize, height: Self.iconSize)
                    .accessibilityHidden(true)
            }
            Text(Self.label(window))
                .font(.system(size: 12)).foregroundStyle(p.text)
                .lineLimit(1).truncationMode(.middle)
            Spacer(minLength: 0)
            Button { onPopOut(window) } label: {
                Image(systemName: open ? "pip.exit" : "pip.enter")
                    .font(.system(size: 11))
                    .frame(width: 22, height: 20)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .foregroundStyle(open ? p.accent : p.secondary)
            .help(open ? ShellLabels.popIn : ShellLabels.popOutWindow)
            .accessibilityLabel("\(open ? ShellLabels.popIn : ShellLabels.popOutWindow): \(Self.label(window))")
        }
        .frame(height: Self.rowHeight)
        .overlay(alignment: .bottom) { Rectangle().fill(p.border).frame(height: 0.5) }
        .contentShape(Rectangle())
        .onDrag { StreamWindowDrag.itemProvider(for: window) }
    }

    static let iconSize: CGFloat = 16

    /// One line: the window's title, else its app.
    static func label(_ window: StreamWindow) -> String {
        let title = window.title.trimmingCharacters(in: .whitespacesAndNewlines)
        return title.isEmpty ? window.app : title
    }
}
#endif
