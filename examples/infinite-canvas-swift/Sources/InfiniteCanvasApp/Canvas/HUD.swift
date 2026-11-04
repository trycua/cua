// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CanvasModel
import SwiftUI

/// The search field: appears when you type with no tile focused.
struct SearchPill: View {
    @ObservedObject var canvas: CanvasController

    var body: some View {
        let visible = !canvas.query.isEmpty
        HStack(spacing: 8) {
            Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
            Text(canvas.query.isEmpty ? "Search windows" : canvas.query)
                .foregroundStyle(canvas.query.isEmpty ? .secondary : .primary)
                .lineLimit(1)
            Spacer(minLength: 0)
            if visible {
                Text(canvas.matches.isEmpty ? "No match" : "\(canvas.matches.count)")
                    .foregroundStyle(.secondary)
                    .monospacedDigit()
            }
        }
        .font(.system(size: 14))
        .padding(.horizontal, 16)
        .frame(width: 380, height: 38)
        .glassEffect(.regular, in: .capsule)
        .opacity(visible ? 1 : 0)
        .scaleEffect(visible ? 1 : 0.96)
        .animation(.smooth(duration: 0.2), value: visible)
    }
}

/// Zoom and window count, top right.
struct StatusText: View {
    @ObservedObject var canvas: CanvasController
    var showPerf: Bool

    var body: some View {
        HStack(spacing: 10) {
            Text("\(Int((canvas.hudCamera.zoom * 100).rounded()))%").monospacedDigit()
            Text("\(canvas.layout.tiles.count) windows")
            if showPerf {
                Text("\(canvas.census.decoding) decoding · \(canvas.census.requestedFps) fps asked").monospacedDigit()
            }
        }
        .font(.system(size: 12, weight: .medium))
        .foregroundStyle(.secondary)
        .padding(.horizontal, 12)
        .frame(height: 28)
        .glassEffect(.regular, in: .capsule)
    }
}

/// Keys, one line.
struct HintBar: View {
    var hotkey: String

    var body: some View {
        HStack(spacing: 14) {
            hint(hotkey, "Hide")
            hint("Return", "Zoom in")
            hint("⌘0", "Fit all")
            hint("Esc", "Back")
        }
        .font(.system(size: 12))
        .padding(.horizontal, 14)
        .frame(height: 30)
        .glassEffect(.regular, in: .capsule)
    }

    private func hint(_ key: String, _ label: String) -> some View {
        HStack(spacing: 5) {
            Text(key).fontWeight(.semibold).foregroundStyle(.primary)
            Text(label).foregroundStyle(.secondary)
        }
    }
}

/// Every tile as a rectangle and the viewport; click to jump there.
struct Minimap: View {
    @ObservedObject var canvas: CanvasController
    var viewSize: () -> CGSize

    var body: some View {
        GeometryReader { geo in
            let world = canvas.layout.bounds.union(canvas.hudCamera.viewport(in: viewSize()))
            let pad: CGFloat = 10
            let s = min((geo.size.width - pad * 2) / max(world.width, 1), (geo.size.height - pad * 2) / max(world.height, 1))
            let ox = pad + (geo.size.width - pad * 2 - world.width * s) / 2
            let oy = pad + (geo.size.height - pad * 2 - world.height * s) / 2
            let map = { (r: CGRect) -> CGRect in
                CGRect(x: ox + (r.minX - world.minX) * s, y: oy + (r.minY - world.minY) * s,
                       width: max(r.width * s, 2), height: max(r.height * s, 2))
            }
            Canvas { ctx, _ in
                for t in canvas.layout.tiles {
                    let r = map(t.frame)
                    let selected = t.id == canvas.selectedID
                    ctx.fill(Path(roundedRect: r, cornerRadius: 2),
                             with: .color(selected ? Color.accentColor : Color.white.opacity(0.28)))
                    if let icon = canvas.icons[t.id] {
                        let side = min(r.width, r.height, 18) * 0.8
                        ctx.draw(Image(nsImage: icon),
                                 in: CGRect(x: r.midX - side / 2, y: r.midY - side / 2, width: side, height: side))
                    }
                }
                let v = map(canvas.hudCamera.viewport(in: viewSize()))
                ctx.stroke(Path(roundedRect: v, cornerRadius: 3), with: .color(.white.opacity(0.85)), lineWidth: 1.2)
            }
            .contentShape(Rectangle())
            .onTapGesture { p in
                let w = CGPoint(x: (p.x - ox) / s + world.minX, y: (p.y - oy) / s + world.minY)
                canvas.scroll.fly(to: Camera(center: w, zoom: canvas.hudCamera.zoom))
            }
        }
        .frame(width: 200, height: 128)
        .glassEffect(.regular, in: .rect(cornerRadius: 16))
    }
}

/// Overlays the HUD pieces on the canvas as separate hosting views, so
/// clicks anywhere else reach the canvas.
@MainActor
final class HUDLayer {
    private var views: [NSView] = []

    init(in container: NSView, canvas: CanvasController, hotkey: String, showPerf: Bool) {
        func host<V: View>(_ v: V) -> NSHostingView<V> {
            let h = NSHostingView(rootView: v)
            h.translatesAutoresizingMaskIntoConstraints = false
            h.sizingOptions = [.intrinsicContentSize]
            container.addSubview(h)
            views.append(h)
            return h
        }
        let search = host(SearchPill(canvas: canvas))
        let status = host(StatusText(canvas: canvas, showPerf: showPerf))
        let hints = host(HintBar(hotkey: hotkey))
        let map = host(Minimap(canvas: canvas) { [weak canvas] in canvas?.scroll.bounds.size ?? .zero })
        let card = host(HoverCard(model: canvas.hover))
        NSLayoutConstraint.activate([
            card.bottomAnchor.constraint(equalTo: container.bottomAnchor, constant: -20),
            card.leadingAnchor.constraint(equalTo: container.leadingAnchor, constant: 20),
            card.widthAnchor.constraint(equalToConstant: HoverCard.size.width),
        ])
        canvas.startCard()
        NSLayoutConstraint.activate([
            search.topAnchor.constraint(equalTo: container.safeAreaLayoutGuide.topAnchor, constant: 20),
            search.centerXAnchor.constraint(equalTo: container.centerXAnchor),
            status.topAnchor.constraint(equalTo: container.safeAreaLayoutGuide.topAnchor, constant: 20),
            status.trailingAnchor.constraint(equalTo: container.trailingAnchor, constant: -20),
            hints.bottomAnchor.constraint(equalTo: container.bottomAnchor, constant: -20),
            hints.centerXAnchor.constraint(equalTo: container.centerXAnchor),
            map.bottomAnchor.constraint(equalTo: container.bottomAnchor, constant: -20),
            map.trailingAnchor.constraint(equalTo: container.trailingAnchor, constant: -20),
        ])
    }

    func setHidden(_ hidden: Bool) {
        for v in views { v.isHidden = hidden }
    }
}
