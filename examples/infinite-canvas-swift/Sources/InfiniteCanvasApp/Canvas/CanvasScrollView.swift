// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CanvasModel
import QuartzCore

/// The infinite canvas: an `NSScrollView` whose document is a huge, empty,
/// layer-backed world view.
///
/// Pan and zoom are the scroll view's own: trackpad scrolling with momentum,
/// pinch to magnify around the fingers, smart magnify, all handled by AppKit
/// on the compositor. Tiles are subviews at fixed world positions, so a pan or
/// zoom changes one clip-view bounds and nothing else: no tile moves, no
/// layout runs, no SwiftUI view updates. The `Camera` value is derived from
/// the clip view after every change and drives level of detail, the HUD and
/// the minimap.
final class CanvasScrollView: NSScrollView {
    /// The world is `extent` points square with the world origin at its
    /// center; far beyond anything a person can pan to.
    static let extent: CGFloat = 2_000_000
    static let originOffset = CGPoint(x: extent / 2, y: extent / 2)

    let world = WorldView(frame: NSRect(x: 0, y: 0, width: extent, height: extent))
    let grid = DotGridLayer()
    var onCameraChange: ((Camera) -> Void)?
    /// Called for scroll events with no tile in front: ⌘-scroll zooms.
    private var flight: (CameraFlight, CFTimeInterval)?
    private var flightCompletion: (() -> Void)?

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        hasHorizontalScroller = false
        hasVerticalScroller = false
        horizontalScrollElasticity = .none
        verticalScrollElasticity = .none
        allowsMagnification = true
        minMagnification = Camera.minZoom
        maxMagnification = Camera.maxZoom
        drawsBackground = false
        usesPredominantAxisScrolling = false
        documentView = world
        contentView.postsBoundsChangedNotifications = true
        NotificationCenter.default.addObserver(self, selector: #selector(boundsChanged),
                                               name: NSView.boundsDidChangeNotification, object: contentView)
        layer?.backgroundColor = Palette.canvas.cgColor
        grid.zPosition = -1
        layer?.insertSublayer(grid, at: 0)
    }

    required init?(coder: NSCoder) { fatalError() }

    // MARK: Camera <-> clip view

    var camera: Camera {
        let b = contentView.bounds
        return Camera(center: CGPoint(x: b.midX - Self.originOffset.x, y: b.midY - Self.originOffset.y),
                      zoom: magnification)
    }

    func setCamera(_ c: Camera) {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        let z = Camera.clamp(c.zoom)
        if abs(magnification - z) > 1e-6 { magnification = z }
        let size = contentView.bounds.size
        let origin = CGPoint(x: c.center.x + Self.originOffset.x - size.width / 2,
                             y: c.center.y + Self.originOffset.y - size.height / 2)
        contentView.setBoundsOrigin(origin)
        CATransaction.commit()
        boundsChanged()
    }

    @objc private func boundsChanged() {
        let c = camera
        grid.update(camera: c, viewSize: bounds.size)
        onCameraChange?(c)
    }

    override func layout() {
        super.layout()
        grid.frame = bounds
        boundsChanged()
    }

    // MARK: Flights

    /// Animate to `target` along a smooth zoom path, stepped by the display
    /// link (see `tick`).
    func fly(to target: Camera, completion: (() -> Void)? = nil) {
        flight = (CameraFlight(from: camera, to: target, viewSize: bounds.size), CACurrentMediaTime())
        flightCompletion = completion
    }

    var isFlying: Bool { flight != nil }

    /// Where the running flight ends, if one runs.
    var flightTarget: Camera? { flight?.0.to }

    func cancelFlight() {
        flight = nil
        flightCompletion = nil
    }

    /// One display frame.
    func tick(_ now: CFTimeInterval) {
        guard let (f, start) = flight else { return }
        let elapsed = now - start
        setCamera(f.camera(at: elapsed))
        if elapsed >= f.duration {
            flight = nil
            let done = flightCompletion
            flightCompletion = nil
            done?()
        }
    }

    // MARK: Events

    override func scrollWheel(with event: NSEvent) {
        if flight != nil { cancelFlight() }
        // ⌘ or ⌥ scroll (and any non-trackpad wheel with ⌘) zooms about the
        // pointer; plain scroll pans, as on a trackpad.
        if event.modifierFlags.contains(.command) || event.modifierFlags.contains(.option) {
            let dy = event.hasPreciseScrollingDeltas ? event.scrollingDeltaY : event.scrollingDeltaY * 8
            let factor = exp(dy * 0.006)
            let p = convert(event.locationInWindow, from: nil)
            let flipped = CGPoint(x: p.x, y: bounds.height - p.y)
            setCamera(camera.zoomed(by: factor, anchor: flipped, in: bounds.size))
            return
        }
        super.scrollWheel(with: event)
    }

    override func magnify(with event: NSEvent) {
        if flight != nil { cancelFlight() }
        super.magnify(with: event)
    }
}

/// The document view: flipped (y down, like the streamed windows), layer
/// backed, never draws.
final class WorldView: NSView {
    override var isFlipped: Bool { true }
    override var wantsUpdateLayer: Bool { true }

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layerContentsRedrawPolicy = .never
    }

    required init?(coder: NSCoder) { fatalError() }

    /// World point -> document coordinates.
    static func doc(_ p: CGPoint) -> CGPoint {
        CGPoint(x: p.x + CanvasScrollView.originOffset.x, y: p.y + CanvasScrollView.originOffset.y)
    }

    static func doc(_ r: CGRect) -> CGRect {
        CGRect(origin: doc(r.origin), size: r.size)
    }

    static func world(_ p: CGPoint) -> CGPoint {
        CGPoint(x: p.x - CanvasScrollView.originOffset.x, y: p.y - CanvasScrollView.originOffset.y)
    }

    var onBackgroundClick: ((NSEvent) -> Void)?

    override func mouseDown(with event: NSEvent) {
        onBackgroundClick?(event)
    }
}

/// A faint dot grid in screen space, so panning over empty canvas still reads
/// as motion. One layer, repositioned modulo the spacing: O(1) per frame.
final class DotGridLayer: CALayer {
    private var spacing: CGFloat = 0
    private var patternCache: [Int: CGColor] = [:]

    override init() {
        super.init()
        masksToBounds = true
    }

    override init(layer: Any) { super.init(layer: layer) }
    required init?(coder: NSCoder) { fatalError() }

    private let tile = CALayer()

    func update(camera c: Camera, viewSize: CGSize) {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        // World spacing doubles as you zoom out so dots stay 14-28 pt apart.
        var world: CGFloat = 32
        while world * c.zoom < 14 { world *= 2 }
        while world * c.zoom > 28 { world /= 2 }
        let s = (world * c.zoom).rounded()
        let key = Int(s)
        if key != Int(spacing) || tile.superlayer == nil {
            spacing = s
            if patternCache[key] == nil { patternCache[key] = Self.pattern(spacing: s) }
            tile.backgroundColor = patternCache[key]
            if tile.superlayer == nil { addSublayer(tile) }
        }
        let origin = c.screenPoint(forWorld: .zero, in: viewSize)
        let ox = origin.x.truncatingRemainder(dividingBy: s)
        let oy = origin.y.truncatingRemainder(dividingBy: s)
        tile.frame = CGRect(x: ox - s, y: -(oy) - s, width: viewSize.width + 3 * s, height: viewSize.height + 3 * s)
        CATransaction.commit()
    }

    private static func pattern(spacing s: CGFloat) -> CGColor {
        let size = NSSize(width: s, height: s)
        let image = NSImage(size: size, flipped: false) { _ in
            Palette.gridDot.setFill()
            NSBezierPath(ovalIn: NSRect(x: 0, y: 0, width: 1.6, height: 1.6)).fill()
            return true
        }
        return NSColor(patternImage: image).cgColor
    }
}
