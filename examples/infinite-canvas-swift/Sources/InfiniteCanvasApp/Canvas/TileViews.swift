// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CanvasModel
import CanvasStreaming
import CuaSpacesStreaming
import QuartzCore
import SwiftUI

/// What a tile view asks of the canvas.
@MainActor
protocol TileViewDelegate: AnyObject {
    func tileSelect(_ id: String)
    func tileFocus(_ id: String)
    func tileZoomInto(_ id: String)
    func tileMove(_ id: String, byWorld delta: CGVector)
    func tileResize(_ id: String, toWorld size: CGSize)
    func tileEndGesture(_ id: String)
    func tileEscape(_ id: String)
    func tileHover(_ id: String, inside: Bool)
    var focusedTileID: String? { get }
    var zoom: CGFloat { get }
}

/// A tile on the canvas: a title strip (the drag handle), the content, a
/// selection ring and a resize corner. Frame = tile frame plus the strip above.
@MainActor
class TileBaseView: NSView {
    static let headerHeight: CGFloat = 30
    let tileID: String
    weak var delegate: TileViewDelegate?
    let titleLayer = CATextLayer()
    /// Icon and title, scaled together (see `setZoom`).
    let labelLayer = CALayer()
    /// The app's real icon from the Space, or nothing.
    let iconLayer = CALayer()
    /// The Space's OS mark, top right.
    let osLayer = CALayer()
    private(set) var hasAppIcon = false
    private var lastZoom: CGFloat = 1
    let ring = CALayer()
    let contentHost = FlippedView()
    private var dragMode: DragMode = .none
    private var dragStart: CGPoint = .zero
    private var sizeAtStart: CGSize = .zero
    private var pressOrigin: CGPoint = .zero

    enum DragMode { case none, move, resize, forward, press }

    init(tileID: String, title: String) {
        self.tileID = tileID
        super.init(frame: .zero)
        wantsLayer = true
        layerContentsRedrawPolicy = .never
        contentHost.wantsLayer = true
        contentHost.layer?.backgroundColor = Palette.tileBackground.cgColor
        contentHost.layer?.cornerRadius = 10
        contentHost.layer?.cornerCurve = .continuous
        contentHost.layer?.masksToBounds = true
        contentHost.layer?.borderColor = Palette.hairline.cgColor
        contentHost.layer?.borderWidth = 1
        addSubview(contentHost)
        titleLayer.fontSize = 13
        titleLayer.font = NSFont.systemFont(ofSize: 13, weight: .medium)
        titleLayer.foregroundColor = Palette.title.cgColor
        titleLayer.truncationMode = .end
        titleLayer.contentsScale = 2
        titleLayer.string = title
        labelLayer.anchorPoint = CGPoint(x: 0, y: 1)
        iconLayer.contentsGravity = .resizeAspect
        iconLayer.isHidden = true
        labelLayer.addSublayer(iconLayer)
        labelLayer.addSublayer(titleLayer)
        layer?.addSublayer(labelLayer)
        osLayer.anchorPoint = CGPoint(x: 1, y: 1)
        osLayer.contentsGravity = .resizeAspect
        osLayer.isHidden = true
        layer?.addSublayer(osLayer)
        ring.borderColor = Palette.accent.cgColor
        ring.cornerRadius = 12
        ring.cornerCurve = .continuous
        ring.isHidden = true
        layer?.addSublayer(ring)
    }

    required init?(coder: NSCoder) { fatalError() }

    override var isFlipped: Bool { true }
    override var acceptsFirstResponder: Bool { true }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    func setTitle(_ s: String) { titleLayer.string = s }

    /// Show the app's icon left of the title; `nil` shows nothing (no
    /// placeholder).
    func setAppIcon(_ image: NSImage?) {
        hasAppIcon = image != nil
        iconLayer.contents = image
        iconLayer.isHidden = image == nil
        setZoom(lastZoom)
    }

    /// The Space's OS as a small monochrome mark in the top-right corner.
    func setOS(_ os: SpaceOS?) {
        guard let os, let tinted = OSMark.tinted(os, color: Palette.secondary, size: 14) else {
            osLayer.isHidden = true
            return
        }
        osLayer.contents = tinted
        osLayer.isHidden = false
        setZoom(lastZoom)
    }

    var contentRect: CGRect {
        CGRect(x: 0, y: Self.headerHeight, width: bounds.width, height: max(bounds.height - Self.headerHeight, 0))
    }

    override func layout() {
        super.layout()
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        contentHost.frame = contentRect
        setZoom(delegate?.zoom ?? 1)
        ring.frame = contentRect.insetBy(dx: -3, dy: -3)
        CATransaction.commit()
        layoutContent()
    }

    func layoutContent() {}

    /// Keep the title readable when zoomed out: it grows up to 3x in world
    /// units (so about 13 pt on screen down to a third of 1:1), anchored to
    /// the tile's top-left and extending upward.
    func setZoom(_ zoom: CGFloat) {
        lastZoom = zoom
        let k = min(max(1 / max(zoom, 0.01), 1), 3.2)
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        let osWidth: CGFloat = osLayer.isHidden ? 0 : 22
        let width = max(bounds.width / k - 8 - osWidth, 10)
        labelLayer.bounds = CGRect(x: 0, y: 0, width: width, height: 20)
        labelLayer.position = CGPoint(x: 4, y: Self.headerHeight - 5)
        labelLayer.setAffineTransform(CGAffineTransform(scaleX: k, y: k))
        let iconSize: CGFloat = hasAppIcon ? 18 : 0
        iconLayer.frame = CGRect(x: 0, y: 1, width: iconSize, height: iconSize)
        let tx: CGFloat = hasAppIcon ? 24 : 0
        titleLayer.frame = CGRect(x: tx, y: 2, width: max(width - tx, 4), height: 18)
        osLayer.bounds = CGRect(x: 0, y: 0, width: 14, height: 14)
        osLayer.position = CGPoint(x: bounds.width - 4, y: Self.headerHeight - 8)
        osLayer.setAffineTransform(CGAffineTransform(scaleX: k, y: k))
        CATransaction.commit()
    }

    func setSelected(_ on: Bool, zoom: CGFloat) {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        ring.isHidden = !on
        ring.borderWidth = max(2 / max(zoom, 0.01), 2)
        ring.frame = contentRect.insetBy(dx: -ring.borderWidth - 1, dy: -ring.borderWidth - 1)
        CATransaction.commit()
    }

    // MARK: Mouse

    private func resizeHit(_ p: CGPoint) -> Bool {
        let zoom = delegate?.zoom ?? 1
        let grab = max(18, 14 / max(zoom, 0.01))
        return p.x > bounds.width - grab && p.y > bounds.height - grab
    }

    var isFocused: Bool { delegate?.focusedTileID == tileID }

    private var hoverArea: NSTrackingArea?
    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let hoverArea { removeTrackingArea(hoverArea) }
        let t = NSTrackingArea(rect: .zero, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect],
                               owner: self, userInfo: ["hover": true])
        addTrackingArea(t)
        hoverArea = t
    }

    override func mouseEntered(with event: NSEvent) {
        if event.trackingArea === hoverArea { delegate?.tileHover(tileID, inside: true) }
    }

    override func mouseExited(with event: NSEvent) {
        if event.trackingArea === hoverArea { delegate?.tileHover(tileID, inside: false) }
    }

    override func mouseDown(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        dragStart = superview?.convert(event.locationInWindow, from: nil) ?? p
        if event.clickCount == 2, !isFocused {
            dragMode = .none
            delegate?.tileZoomInto(tileID)
            return
        }
        if p.y < Self.headerHeight {
            dragMode = .move
            delegate?.tileSelect(tileID)
        } else if resizeHit(p) {
            dragMode = .resize
            sizeAtStart = contentRect.size
            delegate?.tileSelect(tileID)
        } else if isFocused {
            dragMode = .forward
            forwardMouse(event, phase: .down)
        } else {
            // Not focused: a drag moves the tile, a click focuses it.
            dragMode = .press
            pressOrigin = dragStart
            delegate?.tileSelect(tileID)
        }
    }

    override func mouseDragged(with event: NSEvent) {
        let now = superview?.convert(event.locationInWindow, from: nil) ?? .zero
        let delta = CGVector(dx: now.x - dragStart.x, dy: now.y - dragStart.y)
        switch dragMode {
        case .move:
            dragStart = now
            delegate?.tileMove(tileID, byWorld: delta)
        case .resize:
            delegate?.tileResize(tileID, toWorld: CGSize(width: sizeAtStart.width + delta.dx,
                                                         height: sizeAtStart.height + delta.dy))
        case .forward:
            forwardMouse(event, phase: .move)
        case .press:
            if hypot(now.x - pressOrigin.x, now.y - pressOrigin.y) * (delegate?.zoom ?? 1) > 4 {
                dragMode = .move
                delegate?.tileMove(tileID, byWorld: CGVector(dx: now.x - pressOrigin.x, dy: now.y - pressOrigin.y))
                dragStart = now
            }
        case .none:
            break
        }
    }

    override func mouseUp(with event: NSEvent) {
        if dragMode == .forward { forwardMouse(event, phase: .up) }
        if dragMode == .press { delegate?.tileFocus(tileID) }
        if dragMode == .move || dragMode == .resize { delegate?.tileEndGesture(tileID) }
        dragMode = .none
    }

    override func rightMouseDown(with event: NSEvent) {
        if isFocused { forwardMouse(event, phase: .down) } else { super.rightMouseDown(with: event) }
    }

    override func rightMouseUp(with event: NSEvent) {
        if isFocused { forwardMouse(event, phase: .up) } else { super.rightMouseUp(with: event) }
    }

    override func keyDown(with event: NSEvent) {
        if event.keyCode == 53 { delegate?.tileEscape(tileID); return }
        if isFocused { forwardKey(event, down: true) } else { super.keyDown(with: event) }
    }

    override func keyUp(with event: NSEvent) {
        if event.keyCode == 53 { return }
        if isFocused { forwardKey(event, down: false) } else { super.keyUp(with: event) }
    }

    func forwardMouse(_ event: NSEvent, phase: PointerPhase) {}
    func forwardKey(_ event: NSEvent, down: Bool) {}
}

/// A streamed window: the display layer the decoder enqueues into, and the
/// agent cursors over it.
@MainActor
final class StreamTileView: TileBaseView {
    let stream: TileStream
    let cursorLayer = CALayer()
    private(set) var cursors: [String: AgentCursorLayer] = [:]
    private let placeholder = CATextLayer()
    /// A failed stream: a one-line headline and up to three lines of
    /// detail, both fitted to the tile; the raw error is the tooltip.
    let errorHeadline = CATextLayer()
    let errorDetail = CATextLayer()
    private(set) var error: StreamErrorMessage?
    var surfaceSize: CGSize = .zero

    init(tileID: String, title: String, stream: TileStream) {
        self.stream = stream
        super.init(tileID: tileID, title: title)
        contentHost.layer?.addSublayer(stream.layer)
        placeholder.string = "Connecting"
        placeholder.fontSize = 13
        placeholder.foregroundColor = Palette.secondary.cgColor
        placeholder.alignmentMode = .center
        placeholder.contentsScale = 2
        contentHost.layer?.addSublayer(placeholder)
        for (l, size, weight, color) in [(errorHeadline, CGFloat(14), NSFont.Weight.semibold, Palette.title),
                                         (errorDetail, CGFloat(12), NSFont.Weight.regular, Palette.secondary)] {
            l.fontSize = size
            l.font = NSFont.systemFont(ofSize: size, weight: weight)
            l.foregroundColor = color.cgColor
            l.alignmentMode = .center
            l.contentsScale = 2
            l.isHidden = true
            contentHost.layer?.addSublayer(l)
        }
        errorHeadline.truncationMode = .end
        errorDetail.isWrapped = true
        errorDetail.truncationMode = .end
        cursorLayer.masksToBounds = false
        // Above the content host's layer, which AppKit may re-stack.
        cursorLayer.zPosition = 100
        layer?.addSublayer(cursorLayer)
    }

    required init?(coder: NSCoder) { fatalError() }

    func setPlaceholder(_ text: String?) {
        placeholder.string = text ?? ""
        placeholder.isHidden = text == nil
    }

    /// Show a stream failure, or clear it with nil.
    func setError(_ raw: String?) {
        error = raw.map(StreamErrorMessage.describe)
        placeholder.isHidden = error != nil || placeholder.isHidden
        errorHeadline.string = error?.headline
        errorDetail.string = error?.detail
        errorHeadline.isHidden = error == nil
        errorDetail.isHidden = error == nil
        toolTip = error?.raw
        layoutContent()
    }

    override func layoutContent() {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        stream.layer.frame = contentHost.bounds
        placeholder.frame = CGRect(x: 0, y: contentHost.bounds.midY - 9, width: contentHost.bounds.width, height: 18)
        let b = contentHost.bounds
        let w = max(b.width - 32, 40)
        let lineHeight = ceil(errorDetail.fontSize * 1.25)
        let text = (errorDetail.string as? String) ?? ""
        let needed = ceil((text as NSString).boundingRect(
            with: CGSize(width: w, height: 1000), options: [.usesLineFragmentOrigin],
            attributes: [.font: NSFont.systemFont(ofSize: errorDetail.fontSize)]).height)
        let detailHeight = min(max(needed, lineHeight), lineHeight * 3, max(b.height - 60, lineHeight))
        errorHeadline.frame = CGRect(x: 16, y: b.midY - (18 + 6 + detailHeight) / 2, width: w, height: 18)
        errorDetail.frame = CGRect(x: 16, y: errorHeadline.frame.maxY + 6, width: w, height: detailHeight)
        cursorLayer.frame = contentRect
        CATransaction.commit()
    }

    private var geometry: StreamGeometry {
        StreamGeometry(surfaceSize: surfaceSize, viewSize: contentRect.size)
    }

    private func contentPoint(_ event: NSEvent) -> CGPoint {
        let p = convert(event.locationInWindow, from: nil)
        return CGPoint(x: p.x, y: p.y - Self.headerHeight)
    }

    override func forwardMouse(_ event: NSEvent, phase: PointerPhase) {
        let encoder = InputEncoder(geometry: geometry)
        let p = contentPoint(event)
        let mods = InputEncoder.modifiers(from: event.modifierFlags)
        let button = InputEncoder.button(for: event)
        switch phase {
        case .down: stream.send(encoder.pointerDown(at: p, button: button, modifiers: mods))
        case .up: if let e = encoder.pointerUp(at: p, button: button, modifiers: mods) { stream.send([e]) }
        default: if let e = encoder.drag(to: p, modifiers: mods) { stream.send([e]) }
        }
    }

    override func mouseMoved(with event: NSEvent) {
        guard isFocused else { return }
        if let e = InputEncoder(geometry: geometry).pointerMove(at: contentPoint(event)) { stream.send([e]) }
    }

    override func scrollWheel(with event: NSEvent) {
        guard isFocused else { super.scrollWheel(with: event); return }
        let enc = InputEncoder(geometry: geometry)
        if let e = enc.scroll(at: contentPoint(event), deltaX: event.scrollingDeltaX, deltaY: event.scrollingDeltaY,
                              phase: Self.phase(event.phase), momentum: Self.phase(event.momentumPhase),
                              precise: event.hasPreciseScrollingDeltas) {
            stream.send([e])
        }
    }

    static func phase(_ p: NSEvent.Phase) -> GesturePhase {
        switch p {
        case .began: return .began
        case .changed: return .changed
        case .ended: return .ended
        case .cancelled: return .cancelled
        case .mayBegin: return .mayBegin
        default: return .none
        }
    }

    override func forwardKey(_ event: NSEvent, down: Bool) {
        stream.send(InputEncoder(geometry: geometry).keyEvents(for: event, down: down))
    }

    private var trackingArea: NSTrackingArea?
    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let trackingArea { removeTrackingArea(trackingArea) }
        let t = NSTrackingArea(rect: contentRect, options: [.mouseMoved, .activeInKeyWindow, .inVisibleRect],
                               owner: self, userInfo: nil)
        addTrackingArea(t)
        trackingArea = t
    }

    // MARK: Cursors

    /// A window fraction in this view's content coordinates.
    func cursorPoint(for fraction: CGPoint) -> CGPoint {
        let g = geometry
        let rect = g.isUsable ? g.contentRect : CGRect(origin: .zero, size: contentRect.size)
        return CGPoint(x: rect.minX + fraction.x * rect.width, y: rect.minY + fraction.y * rect.height)
    }

    /// Place (or move) an agent cursor at a point inside the window, as
    /// `[0, 1]` fractions of the window.
    func cursor(_ participantID: String, style: AgentStyle, at fraction: CGPoint, action: String?,
                human: Bool = false, shape: String = "arrow") {
        let g = geometry
        let rect = g.isUsable ? g.contentRect : CGRect(origin: .zero, size: contentRect.size)
        let p = CGPoint(x: rect.minX + fraction.x * rect.width, y: rect.minY + fraction.y * rect.height)
        if let c = cursors[participantID] {
            if c.agentStyle != style { c.apply(style) }
            c.setShape(shape)
            c.glide.target = p
            if let action { c.play(action) }
        } else {
            let c = AgentCursorLayer(style: style, at: p, human: human)
            c.setShape(shape)
            cursorLayer.addSublayer(c)
            cursors[participantID] = c
            if let action { c.play(action) }
        }
    }

    func removeCursor(_ participantID: String) {
        cursors.removeValue(forKey: participantID)?.removeFromSuperlayer()
    }

    /// Step every cursor's glide; returns whether any is still moving.
    func stepCursors(dt: TimeInterval, now: CFTimeInterval, zoom: CGFloat) -> Bool {
        guard !cursors.isEmpty else { return false }
        var moving = false
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        // Keep cursors legible at any zoom: counter-scale part of the zoom.
        let k = min(max(1 / pow(max(zoom, 0.01), 0.7), 0.8), 6)
        for c in cursors.values {
            c.glide.step(dt)
            c.position = c.glide.position
            c.updateHeading(velocity: c.glide.velocity, dt: dt)
            c.setAffineTransform(CGAffineTransform(scaleX: k, y: k))
            c.advance(to: now)
            moving = moving || !c.glide.isSettled || c.isAnimating
        }
        CATransaction.commit()
        return moving
    }
}

/// An agent thread: SwiftUI content inside a tile.
@MainActor
final class ThreadTileView: TileBaseView {
    let hosting: NSHostingView<AnyView>

    init(tileID: String, title: String, content: AnyView) {
        hosting = NSHostingView(rootView: content)
        super.init(tileID: tileID, title: title)
        hosting.sizingOptions = []
        contentHost.addSubview(hosting)
    }

    required init?(coder: NSCoder) { fatalError() }

    override func layoutContent() {
        hosting.frame = contentHost.bounds
        frozen.frame = contentHost.bounds
    }

    /// While the zoom changes, the SwiftUI view is swapped for a bitmap of
    /// itself: a magnification change otherwise re-lays out and re-renders
    /// the whole hosting view every display frame.
    private let frozen = CALayer()
    private(set) var isFrozen = false

    func freeze() {
        guard !isFrozen, hosting.superview != nil, hosting.bounds.width > 0 else { return }
        if let rep = hosting.bitmapImageRepForCachingDisplay(in: hosting.bounds) {
            hosting.cacheDisplay(in: hosting.bounds, to: rep)
            frozen.contents = rep.cgImage
        }
        frozen.contentsGravity = .resize
        frozen.frame = contentHost.bounds
        if frozen.superlayer == nil { contentHost.layer?.addSublayer(frozen) }
        frozen.isHidden = false
        hosting.removeFromSuperview()
        isFrozen = true
    }

    func thaw() {
        guard isFrozen else { return }
        contentHost.addSubview(hosting)
        hosting.frame = contentHost.bounds
        frozen.isHidden = true
        isFrozen = false
    }

    override func mouseDown(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        if p.y >= Self.headerHeight, !isFocused, event.clickCount < 2 {
            delegate?.tileFocus(tileID)
        }
        super.mouseDown(with: event)
    }
}

/// A y-down container, like the tiles and the streamed surfaces.
final class FlippedView: NSView {
    override var isFlipped: Bool { true }
}
