// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaBotsCore
import CuaBotsUI
import CuaSpacesStreaming
import SwiftUI

/// "<Name>'s computer": the bot's own Space, live, framed in its color. The
/// bot has control until you take over; its face rides next to its pointer.
struct ComputerPane: View {
    @EnvironmentObject var model: AppModel
    var bot: Bot
    @State private var session: LiveStreamSession?
    @State private var youHaveControl = false
    @State private var pointer: CGPoint?
    @State private var loading = true

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                Image(systemName: "desktopcomputer").foregroundStyle(.secondary)
                Text(bot.computerTitle).font(.headline)
                if let id = bot.spaceID {
                    Text(id).font(.caption.monospaced()).foregroundStyle(.tertiary)
                }
                Spacer()
                Button { model.popOut(bot) } label: { Image(systemName: "pip.enter") }
                    .buttonStyle(.borderless)
                    .help("Picture in picture")
                Button {
                    withAnimation(.spring(response: 0.2, dampingFraction: 0.95)) { model.showComputer = false }
                } label: { Image(systemName: "xmark") }
                    .buttonStyle(.borderless)
                    .help("Close")
            }
            .padding(.horizontal, 14).padding(.vertical, 10)

            GeometryReader { geo in
                // Fit the framed screen to the room there is, keeping the
                // screen's shape; the stream view never sizes the pane.
                let border: CGFloat = 18
                let w = min(geo.size.width, (geo.size.height - 2 * border) * aspect + 2 * border)
                let h = (w - 2 * border) / aspect + 2 * border
                ZStack {
                    RoundedRectangle(cornerRadius: 14).fill(bot.avatar.color.accent.opacity(0.85))
                    screen
                        .clipShape(RoundedRectangle(cornerRadius: 6))
                        .padding(border)
                }
                .frame(width: max(w, 0), height: max(h, 0))
                .position(x: geo.size.width / 2, y: geo.size.height / 2)
            }
            .padding(.horizontal, 14)

            controlPill.padding(.vertical, 12)
        }
        .task(id: bot.id) {
            loading = true
            session = await model.stream(for: bot)
            loading = false
        }
        .task(id: bot.id) {
            // The bot's pointer, a few times a second, while the pane is open.
            while !Task.isCancelled {
                pointer = await model.engine?.pointer(bot)
                try? await Task.sleep(for: .milliseconds(300))
            }
        }
    }

    @ViewBuilder var screen: some View {
        if let session {
            GeometryReader { geo in
                ZStack(alignment: .topLeading) {
                    SpaceScreenView(session: session, isInteractive: youHaveControl, showsControls: false)
                    if !youHaveControl, let p = pointer, session.lastFrameDimensions.width > 0 {
                        let fit = Self.aspectFit(session.lastFrameDimensions, in: geo.size)
                        let x = fit.minX + p.x / session.lastFrameDimensions.width * fit.width
                        let y = fit.minY + p.y / session.lastFrameDimensions.height * fit.height
                        KoalaAvatar(bot.avatar, mood: bot.mood)
                            .frame(width: 30, height: 30)
                            .position(x: x + 20, y: y + 22)
                            .animation(.spring(response: 0.18, dampingFraction: 0.85), value: p)
                            .allowsHitTesting(false)
                    }
                }
            }
        } else {
            VStack(spacing: 10) {
                if loading { ProgressView() }
                Text(loading ? "Connecting to \(bot.computerTitle)" : "\(bot.computerTitle) isn't reachable")
                    .font(.callout).foregroundStyle(.secondary)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(Color.black.opacity(0.85))
        }
    }

    var controlPill: some View {
        HStack(spacing: 10) {
            Text(youHaveControl ? "You have control" : "\(bot.name) has control")
                .font(.callout)
            Button(youHaveControl ? "Return control" : "Take over") {
                youHaveControl.toggle()
                if youHaveControl, let bot = model.store.bot(bot.id), !bot.isPaused {
                    Task { try? await model.engine?.interrupt(bot) }
                }
            }
            .buttonStyle(.borderedProminent)
            .controlSize(.small)
        }
        .padding(.leading, 14).padding(.trailing, 6).padding(.vertical, 5)
        .background(Capsule().fill(.background))
        .overlay(Capsule().strokeBorder(.primary.opacity(0.1)))
    }

    /// The frame follows the screen's shape (plus its 18-point border).
    var aspect: CGFloat {
        guard let d = session?.lastFrameDimensions, d.width > 0, d.height > 0 else { return 16 / 10 }
        return d.width / d.height
    }

    static func aspectFit(_ content: CGSize, in box: CGSize) -> CGRect {
        guard content.width > 0, content.height > 0 else { return CGRect(origin: .zero, size: box) }
        let s = min(box.width / content.width, box.height / content.height)
        let w = content.width * s, h = content.height * s
        return CGRect(x: (box.width - w) / 2, y: (box.height - h) / 2, width: w, height: h)
    }
}
