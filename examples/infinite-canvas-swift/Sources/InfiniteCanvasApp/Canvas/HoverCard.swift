// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CanvasModel
import SwiftUI

/// What the hover card shows, published by the canvas at a low rate.
@MainActor
final class HoverModel: ObservableObject {
    @Published var info: HoverInfo?
    @Published var icon: NSImage?
    /// The tile described, so a change of tile animates and a stats tick
    /// does not.
    @Published var targetID: String?
}

/// A compact glass card, fixed in the bottom-left corner: the app, a
/// latency sparkline and one line of stream stats, the Space and its OS.
/// The window title is not repeated (it sits above each tile).
struct HoverCard: View {
    @ObservedObject var model: HoverModel
    static let size = CGSize(width: 264, height: 128)

    var body: some View {
        Group {
            if let info = model.info {
                VStack(alignment: .leading, spacing: 4) {
                    HStack(spacing: 6) {
                        if let icon = model.icon {
                            Image(nsImage: icon).resizable().frame(width: 16, height: 16)
                        }
                        Text(info.app).font(.system(size: 13, weight: .semibold))
                            .lineLimit(1).truncationMode(.tail).help(info.app)
                    }
                    Sparkline(values: HoverInfo.sparkline(info.latencyMs))
                        .stroke(Color.accentColor, style: StrokeStyle(lineWidth: 1.5, lineCap: .round, lineJoin: .round))
                        .frame(height: 20)
                        .opacity(info.latencyMs.count > 1 ? 1 : 0.25)
                    Text(info.statsLine).font(.system(size: 11).monospacedDigit()).foregroundStyle(.secondary)
                    HStack(spacing: 5) {
                        Image(systemName: "desktopcomputer").font(.system(size: 9))
                            .foregroundStyle(.secondary).frame(width: 11, height: 11)
                        Text(info.spaceLine).font(.system(size: 11)).foregroundStyle(.secondary)
                            .lineLimit(1).truncationMode(.middle).help(info.spaceLine)
                    }
                    HStack(spacing: 5) {
                        if let os = info.osKind {
                            OSMarkView(os: os).frame(width: 11, height: 11)
                        }
                        Text(info.os).font(.system(size: 11)).foregroundStyle(.secondary)
                            .lineLimit(1).truncationMode(.tail).help(info.os)
                    }
                }
                .padding(12)
                .id(model.targetID)
                .transition(.opacity.combined(with: .offset(y: 4)))
            }
        }
        .animation(.smooth(duration: 0.28), value: model.targetID)
        .frame(width: Self.size.width, alignment: .topLeading)
        .fixedSize(horizontal: false, vertical: true)
        .glassEffect(.regular, in: .rect(cornerRadius: 14))
        .opacity(model.info != nil ? 1 : 0)
    }
}

struct Sparkline: Shape {
    var values: [Double]

    func path(in rect: CGRect) -> Path {
        var p = Path()
        guard values.count > 1 else {
            p.move(to: CGPoint(x: rect.minX, y: rect.midY))
            p.addLine(to: CGPoint(x: rect.maxX, y: rect.midY))
            return p
        }
        for (i, v) in values.enumerated() {
            let pt = CGPoint(x: rect.minX + rect.width * CGFloat(i) / CGFloat(values.count - 1),
                             y: rect.maxY - rect.height * CGFloat(v))
            if i == 0 { p.move(to: pt) } else { p.addLine(to: pt) }
        }
        return p
    }
}

/// The OS mark as a SwiftUI view, the same image the tiles show.
struct OSMarkView: View {
    let os: SpaceOS

    var body: some View {
        if let image = OSMark.tinted(os, color: .secondaryLabelColor, size: 11) {
            Image(nsImage: image).resizable().scaledToFit()
        }
    }
}
