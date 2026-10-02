// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSDK
import CuaSpacesFFI
import SwiftUI

/// The notch's two-radius outline: concave "ears" at the top corners that
/// meet the menu bar, rounded bottom corners. Both radii animate, so the
/// ears morph with the spring.
///
/// Path adapted from DynamicNotchKit's `NotchShape` (MIT, see
/// THIRD_PARTY_NOTICES): quad curves inward for the ears, then down,
/// around the bottom corners and back up.
struct NotchShape: Shape {
    var top: CGFloat
    var bottom: CGFloat

    var animatableData: AnimatablePair<CGFloat, CGFloat> {
        get { AnimatablePair(top, bottom) }
        set { top = newValue.first; bottom = newValue.second }
    }

    func path(in rect: CGRect) -> Path {
        var p = Path()
        let t = max(0, min(top, rect.width / 4))
        let b = max(0, min(bottom, rect.height / 2, (rect.width - 2 * t) / 2))
        p.move(to: CGPoint(x: rect.minX, y: rect.minY))
        p.addQuadCurve(to: CGPoint(x: rect.minX + t, y: rect.minY + t),
                       control: CGPoint(x: rect.minX + t, y: rect.minY))
        p.addLine(to: CGPoint(x: rect.minX + t, y: rect.maxY - b))
        p.addQuadCurve(to: CGPoint(x: rect.minX + t + b, y: rect.maxY),
                       control: CGPoint(x: rect.minX + t, y: rect.maxY))
        p.addLine(to: CGPoint(x: rect.maxX - t - b, y: rect.maxY))
        p.addQuadCurve(to: CGPoint(x: rect.maxX - t, y: rect.maxY - b),
                       control: CGPoint(x: rect.maxX - t, y: rect.maxY))
        p.addLine(to: CGPoint(x: rect.maxX - t, y: rect.minY + t))
        p.addQuadCurve(to: CGPoint(x: rect.maxX, y: rect.minY),
                       control: CGPoint(x: rect.maxX - t, y: rect.minY))
        p.closeSubpath()
        return p
    }
}

/// The "N Spaces" tab: square on the left (it tucks under the notch), a
/// concave ear on the top right and a rounded bottom-right corner, so the
/// notch and the tab read as one shape.
struct NotchTabShape: Shape {
    var ear: CGFloat = 6
    var corner: CGFloat = 10

    func path(in rect: CGRect) -> Path {
        var p = Path()
        p.move(to: CGPoint(x: rect.minX, y: rect.minY))
        p.addLine(to: CGPoint(x: rect.maxX, y: rect.minY))
        p.addQuadCurve(to: CGPoint(x: rect.maxX - ear, y: rect.minY + ear),
                       control: CGPoint(x: rect.maxX - ear, y: rect.minY))
        p.addLine(to: CGPoint(x: rect.maxX - ear, y: rect.maxY - corner))
        p.addQuadCurve(to: CGPoint(x: rect.maxX - ear - corner, y: rect.maxY),
                       control: CGPoint(x: rect.maxX - ear, y: rect.maxY))
        p.addLine(to: CGPoint(x: rect.minX, y: rect.maxY))
        p.closeSubpath()
        return p
    }
}

/// Sizes in the panel's own coordinates (top-left origin), from the core's
/// layout. The panel is the layout's stage frame, so the notch is centred
/// at the top.
struct NotchGeometry: Equatable {
    var stage: CGSize
    var notch: CGSize
    var open: CGSize
    var prompt: CGSize
    var closedHit: CGSize
    var tab: CGSize
    /// The closed tabs' content insets (toward the notch, outside).
    var tabInsetNotch: CGFloat
    var tabInsetOuter: CGFloat
    var notchStyle: Bool

    init(_ l: AppNotchLayout) {
        func size(_ r: AppLogicalRect) -> CGSize { CGSize(width: r.width, height: r.height) }
        stage = size(l.stageFrame)
        notch = size(l.notch)
        open = size(l.openFrame)
        prompt = size(l.promptFrame)
        closedHit = size(l.closedFrame)
        tab = size(l.tabFrame)
        tabInsetNotch = CGFloat(l.tabInsetNotch)
        tabInsetOuter = CGFloat(l.tabInsetOuter)
        notchStyle = l.notchStyle
    }

    /// A 14-inch MacBook Pro, for previews and tests without a screen.
    static let fallbackScreen = AppScreenFacts(
        frame: AppLogicalRect(x: 0, y: 0, width: 1512, height: 982),
        visibleFrame: AppLogicalRect(x: 0, y: 0, width: 1512, height: 945),
        safeAreaTop: 32, auxLeftWidth: 662, auxRightWidth: 662)
    static let fallback = NotchGeometry(appNotchLayout(screen: fallbackScreen, prompt: false))

    /// The shape's size for a stage shape (the ears sit outside the notch).
    func size(_ shape: NotchStage.Shape, radii: (closed: AppNotchRadii, open: AppNotchRadii)) -> CGSize {
        switch shape {
        case .closed, .cue:
            let ears = notchStyle ? 2 * radii.closed.top : 0
            return CGSize(width: notch.width + ears, height: notch.height)
        case .prompt: return prompt
        case .tiles: return open
        }
    }
}

/// What the notch panel draws: the closed notch with its "N Spaces" tab, the
/// "Teleport to Cua" box during a window drag, or the Space tiles. Phase,
/// tiles, copy and timing are the core's; `NotchStage` sequences the motion.
struct NotchContentView: View {
    let model: NotchModel
    let controller: NotchController
    @State private var stage: NotchStage
    /// The shape's current target size (animated).
    @State private var shape: NotchStage.Shape
    /// The content showing (animated in after the shape).
    @State private var content: NotchStage.Shape?
    /// The "N Spaces" tab (animated with the shape, not with the core).
    @State private var tabShown: Bool
    @State private var inside = false
    /// A click opened the panel: the search takes the keyboard (hover never
    /// steals it from the app in front).
    @State private var focusOnOpen = false
    @FocusState private var searchFocused: Bool
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.openWindow) private var openWindow
    @Environment(\.openSettings) private var openSettings

    init(model: NotchModel, controller: NotchController) {
        self.model = model
        self.controller = controller
        let v = model.view
        let s = NotchStage(settledOn: v.phase, cue: v.hoverCue)
        _stage = State(initialValue: s)
        _shape = State(initialValue: s.shape)
        _content = State(initialValue: s.content)
        _tabShown = State(initialValue: v.showTab && !s.shape.isOpen)
    }

    private var motion: AppNotchMotion { NotchModel.motion }
    private var radii: (closed: AppNotchRadii, open: AppNotchRadii) {
        let r = appNotchRadii()
        return (r[0], r[1])
    }

    var body: some View {
        let v = model.view
        let g = controller.geometry
        let size = g.size(shape, radii: radii)
        let r = shape.isOpen ? radii.open : radii.closed
        ZStack(alignment: .top) {
            // The hover and drop margin around the closed notch: nearly
            // transparent, so it takes events while the rest of the panel
            // lets clicks through to the menu bar.
            if !shape.isOpen {
                Rectangle().fill(Color.black.opacity(0.001))
                    .frame(width: g.closedHit.width, height: g.closedHit.height)
            }
            ZStack(alignment: .top) {
                if g.notchStyle, tabShown {
                    tab(v, g: g)
                        .transition(.offset(x: -g.tab.width).combined(with: .opacity))
                    if let a = v.activity {
                        activity(a, g: g)
                            .transition(.offset(x: g.tab.width).combined(with: .opacity))
                    }
                }
                surface(g: g, top: r.top, bottom: r.bottom)
                    .frame(width: size.width, height: size.height)
                    .overlay {
                        if !g.notchStyle, !shape.isOpen { closedCapsuleLabel(v) }
                    }
            }
            // The cue grows the notch and its tab as one, about the notch's
            // top centre.
            .frame(width: g.stage.width, alignment: .top)
            .scaleEffect(x: shape == .cue && !reduceMotion ? motion.hoverScale : 1,
                         y: shape == .cue && !reduceMotion ? motion.hoverScaleY : 1, anchor: .top)
            // Reduce Motion: the cue is a faint rim instead of a spring.
            .overlay(alignment: .top) {
                if shape == .cue, reduceMotion, g.notchStyle {
                    NotchShape(top: radii.closed.top, bottom: radii.closed.bottom)
                        .stroke(.white.opacity(0.28), lineWidth: 1)
                        .frame(width: size.width, height: size.height)
                        .transition(.opacity)
                }
            }
            .animation(.spring(response: motion.openResponse, dampingFraction: motion.closeDamping), value: g)
            // Always present, so its frame (and the clip) springs with the
            // shape; the content inside is laid out at its final size.
            ZStack(alignment: .top) {
                if let content {
                    let full = g.size(content, radii: radii)
                    contentView(content, v, g: g)
                        .frame(width: full.width, height: full.height, alignment: .top)
                        .transition(.asymmetric(
                            insertion: .opacity.combined(with: .scale(scale: motion.contentScale, anchor: .top)),
                            removal: .opacity))
                        .id(content)
                }
            }
            .frame(width: size.width, height: size.height, alignment: .top)
            .clipShape(NotchShape(top: r.top, bottom: r.bottom))
        }
        .frame(width: g.stage.width, height: g.stage.height, alignment: .top)
        .environment(\.colorScheme, .dark)
        .onContinuousHover { phase in hover(phase, g: g) }
        // A click on the closed notch opens it. Once open the gesture is off
        // (`.subviews`), so it never competes with the tiles and buttons.
        .gesture(TapGesture().onEnded { if !stage.shape.isOpen { focusOnOpen = true; model.send(.click) } },
                 including: stage.shape.isOpen ? .subviews : .all)
        .onExitCommand { model.send(.escape) }
        .dropDestination(for: URL.self) { _, _ in false } isTargeted: { model.send(.dropTargeted(targeted: $0)) }
        .onChange(of: v.phase) { _, _ in advance() }
        .onChange(of: v.hoverCue) { _, _ in advance() }
        .onChange(of: v.showTab) { _, _ in advance() }
        .accessibilityElement(children: .contain)
        .accessibilityLabel(v.label)
        // The notch outlives the main window: a drop or a tile click opens
        // it through this scene's `openWindow`.
        .onAppear { controller.showMain = { [openWindow] in openWindow(id: "main") } }
    }

    // MARK: - Motion

    /// Runs the stage's steps for the core's new phase.
    private func advance() {
        let v = model.view
        let steps = stage.update(phase: v.phase, cue: v.hoverCue, motion: motion, reduceMotion: reduceMotion)
        for step in steps {
            switch step {
            case .shape(let s, let m):
                withAnimation(NotchStage.animation(m, motion)) { shape = s }
            case .content(let c, let m):
                withAnimation(NotchStage.animation(m, motion)) { content = c }
            }
        }
        if stage.shape == .tiles, focusOnOpen, v.header != nil {
            focusOnOpen = false
            controller.takeKeyboard()
            DispatchQueue.main.async { searchFocused = true }
        } else if !stage.shape.isOpen {
            focusOnOpen = false
            searchFocused = false
        }
        // The tab tucks into the notch as it opens and slides back out
        // once it has closed.
        let tab = v.showTab && !stage.shape.isOpen
        if tab != tabShown {
            let m: NotchStage.Motion = reduceMotion ? .fade : tab ? .close(delay: motion.contentOut) : .open
            withAnimation(NotchStage.animation(m, motion)) { tabShown = tab }
        }
        controller.stageChanged(open: stage.shape.isOpen)
    }

    /// Hover over the drawn notch (or its margin while closed).
    private func hover(_ phase: HoverPhase, g: NotchGeometry) {
        var now = false
        if case .active(let p) = phase {
            let open = stage.shape.isOpen
            let s = open ? g.size(stage.shape, radii: radii) : g.closedHit
            var rect = CGRect(x: (g.stage.width - s.width) / 2, y: 0, width: s.width, height: s.height)
            if !open, g.notchStyle { rect = rect.union(CGRect(x: g.stage.width / 2 + g.notch.width / 2, y: 0,
                                                              width: g.tab.width, height: g.tab.height)) }
            now = rect.contains(p)
        }
        guard now != inside else { return }
        inside = now
        model.send(now ? .hoverEnter : .hoverExit)
    }

    // MARK: - Pieces

    @ViewBuilder private func surface(g: NotchGeometry, top: Double, bottom: Double) -> some View {
        if g.notchStyle {
            NotchShape(top: top, bottom: bottom)
                .fill(.black)
                .shadow(color: .black.opacity(shape.isOpen ? 0.45 : 0), radius: 14, y: 6)
        } else {
            RoundedRectangle(cornerRadius: shape.isOpen ? 20 : g.notch.height / 2, style: .continuous)
                .fill(.clear)
                .glassEffect(.regular, in: .rect(cornerRadius: shape.isOpen ? 20 : g.notch.height / 2))
        }
    }

    /// The "N Spaces" tab beside the closed notch; tucks into it during a
    /// drag. Two rows, like the Tauri app: the count over the word. Hover
    /// brightens the word; a press scales the label (never the black shape,
    /// which stays joined to the notch). The tab is as wide as its rows plus
    /// the core's insets (none toward the notch, a hair outside), so it
    /// barely widens the notch.
    private func tab(_ v: AppNotchView, g: NotchGeometry) -> some View {
        let tuck: CGFloat = 20
        let ear = CGFloat(radii.closed.top)
        return Button { focusOnOpen = true; model.send(.click) } label: { Color.clear }
            .buttonStyle(NotchButtonStyle(forced: model.highlight.state(for: .tab), pressedScale: 1) { _, s in
                HStack(spacing: 0) {
                    Color.clear.frame(width: tuck + g.tabInsetNotch)
                    NotchTabLabel(tab: v.tab, hovered: s.hovered)
                        .scaleEffect(s.pressed && !reduceMotion ? NotchPress.buttonScale : 1)
                    Color.clear.frame(width: g.tabInsetOuter + ear)
                }
                .frame(height: g.tab.height)
                .background(NotchTabShape(ear: ear, corner: 10).fill(.black))
                .contentShape(.rect)
            })
            .frame(width: g.tab.width + tuck, height: g.tab.height, alignment: .leading)
            .offset(x: g.notch.width / 2 + (g.tab.width + tuck) / 2 - tuck)
            .help(v.countLabel)
            .accessibilityElement(children: .ignore)
            .accessibilityLabel(v.countLabel)
            .accessibilityAddTraits(.isButton)
    }

    /// The indicator left of the closed notch (the tab's mirror): a transfer,
    /// the hotspot or a Space starting, as the core picks. As wide as its
    /// ring plus the core's insets.
    private func activity(_ a: AppNotchActivity, g: NotchGeometry) -> some View {
        let tuck: CGFloat = 20
        let ear = CGFloat(radii.closed.top)
        return HStack(spacing: 0) {
            Color.clear.frame(width: ear + g.tabInsetOuter)
            NotchActivityGlyph(activity: a)
            Color.clear.frame(width: g.tabInsetNotch + tuck)
        }
        .frame(height: g.tab.height)
        .background(NotchTabShape(ear: ear, corner: 10).fill(.black).scaleEffect(x: -1, y: 1))
        .frame(width: g.tab.width + tuck, height: g.tab.height, alignment: .trailing)
        .offset(x: -(g.notch.width / 2 + (g.tab.width + tuck) / 2 - tuck))
        .allowsHitTesting(false)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(a.label)
        .help(a.label)
    }

    private func closedCapsuleLabel(_ v: AppNotchView) -> some View {
        HStack(spacing: 6) {
            if let a = v.activity { NotchActivityGlyph(activity: a, size: 14) }
            Text(v.countLabel)
                .font(.system(size: 12, weight: .medium))
                .foregroundStyle(.primary)
        }
        .frame(maxHeight: .infinity)
    }

    @ViewBuilder
    private func contentView(_ c: NotchStage.Shape, _ v: AppNotchView, g: NotchGeometry) -> some View {
        let inset = g.notchStyle || v.header != nil ? g.notch.height : 0
        let side = CGFloat((g.notchStyle ? radii.open.top : 0) + 15)
        switch c {
        case .prompt:
            VStack(spacing: 0) {
                Spacer().frame(height: inset)
                Text(v.prompt ?? "")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(.white)
                    .lineLimit(1)
                    .frame(maxHeight: .infinity)
            }
        default:
            VStack(spacing: 0) {
                if let h = v.header {
                    header(h, g: g).frame(height: g.notch.height)
                    Spacer().frame(height: inset - g.notch.height + 15)
                } else {
                    Spacer().frame(height: inset + 15)
                }
                if let p = v.permission {
                    permissionRow(p).frame(height: 36, alignment: .top)
                } else if let a = v.access {
                    accessRow(a).frame(height: 36, alignment: .top)
                } else if let prompt = v.prompt {
                    Text(prompt)
                        .font(.system(size: 12, weight: .medium))
                        .foregroundStyle(.white.opacity(0.7))
                        .lineLimit(1)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .frame(height: 36, alignment: .top)
                }
                if let empty = v.empty {
                    Text(empty)
                        .font(.system(size: 12))
                        .foregroundStyle(.white.opacity(0.6))
                        .lineLimit(1)
                        .frame(maxWidth: .infinity, minHeight: TileView.height, maxHeight: TileView.height, alignment: .topLeading)
                } else {
                    tiles(v)
                }
                Spacer(minLength: 0)
            }
            .padding(.horizontal, side)
        }
    }

    /// The row inline with the notch: the search left of the camera
    /// housing, the buttons right of it, nothing under it. Without a notch
    /// it is one full-width row.
    private func header(_ h: AppNotchHeader, g: NotchGeometry) -> some View {
        let side = CGFloat((g.notchStyle ? radii.open.top : 0) + 15)
        let gap: CGFloat = 12
        let zone = g.notchStyle ? max(0, (g.open.width - g.notch.width) / 2 - side - gap) : nil
        let query = Binding(get: { model.state.query }, set: { model.search($0) })
        return HStack(spacing: 0) {
            NotchSearchZone(forced: model.highlight.state(for: .search), help: h.searchLabel,
                            focus: { controller.takeKeyboard(); searchFocused = true }) {
                Image(systemName: "magnifyingglass")
                    .font(.system(size: 12, weight: .medium))
                    .foregroundStyle(.white.opacity(0.5))
                TextField(h.placeholder, text: query)
                    .textFieldStyle(.plain)
                    .font(.system(size: 13))
                    .foregroundStyle(.white)
                    .focused($searchFocused)
                    .accessibilityLabel(h.searchLabel)
                if let n = h.matchCount {
                    Text("\(n)")
                        .font(.system(size: 10, weight: .semibold))
                        .monospacedDigit()
                        .foregroundStyle(.white.opacity(0.7))
                        .padding(.horizontal, 6)
                        .padding(.vertical, 1)
                        .background(.white.opacity(0.12), in: .capsule)
                }
            }
            .frame(width: zone, alignment: .leading)
            .frame(maxWidth: zone == nil ? .infinity : nil, alignment: .leading)
            if let zone {
                // The camera housing: nothing is drawn here.
                Spacer().frame(width: g.open.width - 2 * side - 2 * zone)
            }
            HStack(spacing: 2) {
                Spacer(minLength: 0)
                ForEach(h.buttons, id: \.symbol) { b in
                    Button { run(b.id) } label: { Image(systemName: b.symbol) }
                        .buttonStyle(NotchButtonStyle(forced: model.highlight.state(for: .button(b.id))) { _, s in
                            NotchIconLabel(symbol: b.symbol, state: s)
                        })
                    .help(b.help)
                    .accessibilityLabel(b.label)
                }
            }
            .frame(width: zone)
        }
    }

    private func run(_ id: AppNotchButtonId) {
        model.send(.dismiss)
        NSApp.activate()
        switch id {
        case .list: openWindow(id: "main")
        case .settings: openSettings()
        }
    }

    private func permissionRow(_ p: AppNotchPermission) -> some View {
        HStack(spacing: 8) {
            Text(p.text)
                .font(.system(size: 12))
                .foregroundStyle(.white.opacity(0.8))
                .lineLimit(1)
            Spacer(minLength: 8)
            Button(p.action) { controller.openPermissionSettings(pane: p.pane) }
                .buttonStyle(.borderedProminent)
                .controlSize(.small)
        }
    }

    /// Live Keyvault sign-ins: the key, what is live (opens the Access
    /// page) and Dismiss, which only hides this.
    private func accessRow(_ a: AppNotchAccess) -> some View {
        HStack(spacing: 8) {
            Button { model.send(.dismiss); controller.onOpenAccess?() } label: {
                HStack(spacing: 6) {
                    Image(systemName: "key.fill")
                        .font(.system(size: 10, weight: .semibold))
                        .foregroundStyle(Color(red: 0x34 / 255, green: 0xc7 / 255, blue: 0x59 / 255))
                    Text(a.text)
                        .font(.system(size: 12))
                        .foregroundStyle(.white.opacity(0.8))
                        .lineLimit(1)
                }
            }
            .buttonStyle(.plain)
            .help("Show in Keyvault")
            Spacer(minLength: 8)
            Button(a.dismiss) { controller.onDismissAccess?() }
                .buttonStyle(.bordered)
                .controlSize(.small)
                .help("Hide this until something new is shared. Access stays.")
                .accessibilityIdentifier("notch-dismiss-access")
        }
    }

    @ViewBuilder private func tiles(_ v: AppNotchView) -> some View {
        let row = HStack(alignment: .top, spacing: 12) {
            ForEach(v.tiles, id: \.id) { tile in
                Button { controller.onOpenSpace?(tile.id) } label: {
                    TileView(tile: tile, thumbnail: model.thumbnails[tile.id],
                             ghost: tile.targeted ? model.ghost : nil)
                }
                .buttonStyle(NotchButtonStyle(forced: model.highlight.state(for: .tile(tile.id)),
                                              pressedScale: NotchPress.tileScale) { label, s in
                    label.environment(\.notchTileHovered, s.hovered)
                })
                .help(tile.label)
                .accessibilityLabel(tile.label)
                    .dropDestination(for: URL.self) { urls, _ in
                        guard tile.dropTarget else { return false }
                        controller.onDropURLs?(tile.id, urls)
                        return true
                    }
            }
        }
        // Left to right from the content inset (the drag hit test assumes it).
        if v.tiles.count > 4 {
            ScrollView(.horizontal, showsIndicators: false) { row }.frame(height: TileView.height)
        } else {
            row.frame(maxWidth: .infinity, minHeight: TileView.height, maxHeight: TileView.height, alignment: .topLeading)
        }
    }
}

/// A Space tile: a header line with the OS logo and, in secondary grey,
/// where it runs ("This Mac", "Mac mini"); its live thumbnail (or a plain
/// frame until one arrives); then one line with the status dot and the
/// name. Dimmed when not live; outlined, with the dragged window's ghost,
/// under a drag.
struct TileView: View {
    let tile: AppNotchTile
    var thumbnail: NSImage?
    var ghost: NSImage?
    /// Hovered (from the tile button's style): the frame lifts and its
    /// hairline brightens.
    @Environment(\.notchTileHovered) private var hovered

    /// The header line's height (the core's `TILE_HEADER_HEIGHT`).
    static let headerHeight: CGFloat = 14
    /// The whole tile's height (the core's `TILE_ROW_HEIGHT`): the header,
    /// 5 pt, the 80 pt thumbnail, 6 pt and the 16 pt caption.
    static let height: CGFloat = headerHeight + 5 + 80 + 6 + 16

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            header
            VStack(alignment: .leading, spacing: 6) {
                thumbnailFrame
                caption
            }
        }
        .opacity(tile.dim ? 0.5 : 1)
        .contentShape(.rect)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(tile.label)
        .accessibilityAddTraits(.isButton)
    }

    /// The OS logo, then where the Space runs in secondary grey; a long
    /// machine name keeps both ends ("Dillon's Mac…Rack 3").
    private var header: some View {
        HStack(spacing: 5) {
            OsIconImage(id: tile.symbol, size: 11)
                .foregroundStyle(.white.opacity(0.85))
            Text(tile.location)
                .font(.system(size: 10, weight: .medium))
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .truncationMode(.middle)
        }
        .frame(width: 128, height: Self.headerHeight, alignment: .leading)
    }

    private var caption: some View {
        HStack(spacing: 5) {
            Circle().fill(dot).frame(width: 6, height: 6)
            Text(tile.name)
                .font(.system(size: 11, weight: .medium))
                .foregroundStyle(.white)
                .lineLimit(1)
                .truncationMode(.tail)
        }
        .frame(width: 128, height: 16, alignment: .leading)
    }

    private var thumbnailFrame: some View {
        ZStack {
            RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Color(white: 0.13))
            if let thumbnail {
                Image(nsImage: thumbnail)
                    .resizable()
                    .aspectRatio(contentMode: .fill)
                    .frame(width: 128, height: 80)
                    .clipped()
                    .transition(.opacity)
            }
            if let progress = tile.progress {
                // Being created: the core's progress and phase.
                VStack(spacing: 6) {
                    NotchActivityGlyph(activity: AppNotchActivity(
                        kind: .provisioning, label: tile.label, symbol: nil, permille: progress,
                        startedAt: nil, estimateMs: 0), size: 22)
                    if let label = tile.progressLabel {
                        Text(label)
                            .font(.system(size: 10, weight: .medium))
                            .foregroundStyle(.white.opacity(0.6))
                            .lineLimit(1)
                    }
                }
            } else if thumbnail == nil, let label = tile.progressLabel {
                Text(label)
                    .font(.system(size: 10, weight: .medium))
                    .foregroundStyle(.white.opacity(0.6))
            }
            if tile.signedIn {
                SignedInKey()
                    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topTrailing)
                    .padding(5)
            }
            if let ghost {
                Image(nsImage: ghost)
                    .resizable()
                    .aspectRatio(contentMode: .fit)
                    .clipShape(.rect(cornerRadius: 4))
                    .shadow(color: .black.opacity(0.6), radius: 5, y: 2)
                    .padding(8)
                    .transition(.scale(scale: 1.06).combined(with: .opacity))
            }
        }
        .frame(width: 128, height: 80)
        .clipShape(.rect(cornerRadius: 8, style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: 8, style: .continuous)
                .strokeBorder(tile.targeted ? Color.accentColor : .white.opacity(hovered ? 0.32 : 0.1),
                              lineWidth: tile.targeted ? 2 : 1)
        }
        .shadow(color: .black.opacity(hovered ? 0.5 : 0), radius: 6, y: 3)
        .offset(y: hovered ? -NotchPress.tileLift : 0)
        .animation(.easeOut(duration: 0.15), value: tile.targeted)
        .animation(NotchPress.spring, value: hovered)
    }

    private var dot: Color {
        switch tile.status {
        case .running, .local: return .green
        case .approval: return .orange
        case .provisioning: return .blue
        case .suspended, .deleting: return .gray
        }
    }
}

/// A Space signed in through the Keyvault: a small key on its thumbnail.
struct SignedInKey: View {
    var body: some View {
        Image(systemName: "key.fill")
            .font(.system(size: 8, weight: .bold))
            .foregroundStyle(.white)
            .frame(width: 16, height: 16)
            .background(Circle().fill(.black.opacity(0.65)))
            .overlay(Circle().strokeBorder(.white.opacity(0.18), lineWidth: 0.5))
            .accessibilityLabel("Signed in")
            .help("Signed in through the Keyvault")
    }
}

/// The tab's two rows (the Tauri tab's type): the count at 11 pt bold with
/// tabular digits over the word at 8 pt semibold, uppercase, 70% white.
struct NotchTabLabel: View {
    let tab: AppNotchTab
    var hovered = false

    var body: some View {
        VStack(spacing: 1) {
            Text(tab.count)
                .font(.system(size: 11, weight: .bold))
                .monospacedDigit()
                .foregroundStyle(.white)
            Text(tab.word)
                .font(.system(size: 8, weight: .semibold))
                .textCase(.uppercase)
                .tracking(0.24)
                .foregroundStyle(.white.opacity(hovered ? 1 : 0.7))
        }
        .lineLimit(1)
        .fixedSize()
    }
}

/// The activity glyph: the hotspot symbol (pulsing) or a thin progress
/// ring, real progress when the core has it, else the core's estimate.
struct NotchActivityGlyph: View {
    let activity: AppNotchActivity
    var size: CGFloat = 18
    @State private var appeared = Date()
    @State private var pulse = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        if let symbol = activity.symbol {
            Image(systemName: symbol)
                .font(.system(size: size * 0.9, weight: .medium))
                .foregroundStyle(Color(red: 0x34 / 255, green: 0xc7 / 255, blue: 0x59 / 255))
                .opacity(pulse ? 0.55 : 1)
                .onAppear {
                    guard !reduceMotion else { return }
                    withAnimation(.easeInOut(duration: 1).repeatForever(autoreverses: true)) { pulse = true }
                }
        } else {
            TimelineView(.periodic(from: .now, by: 0.2)) { context in
                ring(fraction(at: context.date))
            }
        }
    }

    /// Thousandths to 0...1: the core's real progress, else its estimate
    /// from the earliest start (or from when the ring appeared).
    func fraction(at now: Date) -> Double {
        if let p = activity.permille { return Double(p) / 1000 }
        let start = activity.startedAt.map { Date(timeIntervalSince1970: Double($0) / 1000) } ?? appeared
        let elapsed = Int64(now.timeIntervalSince(start) * 1000)
        return Double(appNotchEstimatedProgress(elapsedMs: elapsed, estimateMs: activity.estimateMs)) / 1000
    }

    private func ring(_ f: Double) -> some View {
        let line = max(1.5, size * 0.1)
        return ZStack {
            Circle().stroke(.white.opacity(0.08), lineWidth: line)
            Circle()
                .trim(from: 0, to: f)
                .stroke(.white.opacity(0.25), style: StrokeStyle(lineWidth: line))
                .rotationEffect(.degrees(-90))
        }
        .frame(width: size - line, height: size - line)
        .animation(.linear(duration: 0.2), value: f)
    }
}

/// A Space's OS icon from the core's id: the system symbol when the core
/// names one (macOS: `apple.logo`), else the core's SVG as a template image.
struct OsIconImage: View {
    let id: String
    var size: CGFloat = 11

    var body: some View {
        if let symbol = appOsIconSystemSymbol(id: id) {
            Image(systemName: symbol)
                .font(.system(size: size, weight: .medium))
                .frame(width: size, height: size)
        } else if let image = Self.image(id) {
            Image(nsImage: image)
                .renderingMode(.template)
                .resizable()
                .aspectRatio(contentMode: .fit)
                .frame(width: size, height: size)
        }
    }

    @MainActor private static var cache: [String: NSImage] = [:]

    @MainActor static func image(_ id: String) -> NSImage? {
        if let hit = cache[id] { return hit }
        guard let svg = appOsIconSvg(id: id), let image = NSImage(data: Data(svg.utf8)) else { return nil }
        image.isTemplate = true
        cache[id] = image
        return image
    }
}

private struct NotchTileHoveredKey: EnvironmentKey {
    static let defaultValue = false
}

extension EnvironmentValues {
    /// A Space tile's hover, from its button style to its frame.
    var notchTileHovered: Bool {
        get { self[NotchTileHoveredKey.self] }
        set { self[NotchTileHoveredKey.self] = newValue }
    }
}

/// The header's search zone: the whole zone left of the camera housing is
/// the hit target (a click anywhere in it focuses the field), with a faint
/// rounded fill on hover and the text cursor.
struct NotchSearchZone<Content: View>: View {
    var forced: NotchInteractionState?
    let help: String
    let focus: () -> Void
    @ViewBuilder let content: Content
    @State private var hovered = false

    var body: some View {
        let on = hovered || forced?.hovered == true
        HStack(spacing: 6) { content }
            .padding(.horizontal, 8)
            .frame(minHeight: NotchPress.minHit)
            .background(RoundedRectangle(cornerRadius: 8, style: .continuous)
                .fill(.white.opacity(on ? 0.08 : 0)))
            .contentShape(.rect)
            // The fill reaches past the glyph; the glyph stays on the tiles'
            // left edge.
            .padding(.horizontal, -8)
            .onTapGesture(perform: focus)
            .onHover { hovered = $0 }
            .pointerStyle(.horizontalText)
            .animation(NotchPress.spring, value: on)
            .help(help)
    }
}
