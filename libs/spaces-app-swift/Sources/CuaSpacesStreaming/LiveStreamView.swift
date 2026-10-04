// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

#if canImport(AppKit)
import AppKit
import CoreVideo
import SwiftUI

/// SwiftUI wrapper around the live stream renderer.
///
/// Drop it anywhere a Space's screen should appear. It renders whatever
/// `session` currently has, aspect-fit and letterboxed, and — when
/// `isInteractive` — forwards mouse and key input to the Space.
public struct LiveStreamView: NSViewRepresentable {
    @ObservedObject public var session: LiveStreamSession
    public var isInteractive: Bool = true
    /// Draws a local cursor dot at the pointer. Its position comes from the
    /// *same* mapping as the coordinate being sent, so the two cannot disagree.
    public var showsCursorOverlay: Bool = true

    public init(session: LiveStreamSession, isInteractive: Bool = true,
                showsCursorOverlay: Bool = true) {
        _session = ObservedObject(wrappedValue: session)
        self.isInteractive = isInteractive
        self.showsCursorOverlay = showsCursorOverlay
    }

    public func makeNSView(context: Context) -> LiveStreamInputView {
        let view = LiveStreamInputView()
        view.onInput = { [weak session] events in
            session?.send(events)
        }
        // FRICTION.md §18: the fact that a stream is *presented* is a
        // view-layer fact, and only the view can report it.
        session.presentation.viewDidAttach()
        view.onDetach = { [weak session] in session?.presentation.viewDidDetach() }
        return view
    }

    public func updateNSView(_ view: LiveStreamInputView, context: Context) {
        view.isInteractive = isInteractive
        view.showsCursorOverlay = showsCursorOverlay
        view.surfaceSize = session.surfaceSize
        view.present(session.frame)
        session.presentation.viewDidPresent(pixels: view.layerHasPixels,
                                            contentSize: view.geometry.contentRect.size,
                                            interactive: isInteractive)
    }

    public static func dismantleNSView(_ view: LiveStreamInputView, coordinator: ()) {
        view.onDetach?()
    }
}

/// The renderer and input surface.
///
/// One `CALayer` holding the decoded `CVPixelBuffer` as its `contents`. No
/// intermediate `NSImage`, no per-frame CGImage: the IOSurface-backed buffer
/// VideoToolbox produced is handed straight to Core Animation, which is what
/// keeps a 30 fps stream cheap enough to run two of them (in-app and PiP) at
/// once off one decode.
public final class LiveStreamInputView: NSView {
    public var onInput: (([InteractiveInputEvent]) -> Void)?
    /// Called when SwiftUI tears this view down, so the presentation count
    /// falls back to zero (§18).
    public var onDetach: (() -> Void)?
    public var isInteractive = true
    public var showsCursorOverlay = true {
        didSet { cursorLayer.isHidden = !showsCursorOverlay || cursorPoint == nil }
    }

    /// Size of the surface being shown. Setting it re-lays-out the content rect
    /// and therefore also the input mapping — one value drives both.
    public var surfaceSize: CGSize = .zero {
        didSet { if surfaceSize != oldValue { layoutContent() } }
    }

    private let videoLayer = CALayer()
    private let cursorLayer = CAShapeLayer()
    private var cursorPoint: CGPoint?
    private var trackingArea: NSTrackingArea?
    private var isDragging = false
    private var dragButton: PointerButton = .left
    /// The last in-frame point seen during a drag. A drag that wanders outside
    /// the frame keeps reporting this rather than dropping samples, so the
    /// button-up still lands somewhere the user pointed at — and never at a
    /// sentinel.
    private var lastInFramePoint: CGPoint?

    /// The single source of the view↔surface mapping.
    public var geometry: StreamGeometry {
        StreamGeometry(surfaceSize: surfaceSize, viewSize: bounds.size)
    }

    private var encoder: InputEncoder { InputEncoder(geometry: geometry) }

    public override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        configure()
    }

    public required init?(coder: NSCoder) {
        super.init(coder: coder)
        configure()
    }

    private func configure() {
        wantsLayer = true
        layer?.backgroundColor = NSColor.black.cgColor
        videoLayer.contentsGravity = .resize
        videoLayer.magnificationFilter = .trilinear
        videoLayer.minificationFilter = .trilinear
        videoLayer.isOpaque = true
        layer?.addSublayer(videoLayer)

        cursorLayer.path = CGPath(ellipseIn: CGRect(x: -5, y: -5, width: 10, height: 10), transform: nil)
        cursorLayer.fillColor = NSColor.systemBlue.withAlphaComponent(0.55).cgColor
        cursorLayer.strokeColor = NSColor.white.cgColor
        cursorLayer.lineWidth = 1.5
        cursorLayer.isHidden = true
        layer?.addSublayer(cursorLayer)
    }

    /// `true` so view coordinates are top-left origin, matching the surface's
    /// own origin and RCDP's normalized space. Getting this wrong flips every
    /// click vertically, which reads as "input lands, but in the wrong place".
    public override var isFlipped: Bool { true }

    public override func layout() {
        super.layout()
        layoutContent()
    }

    private func layoutContent() {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        videoLayer.frame = geometry.contentRect
        if let cursorPoint {
            cursorLayer.position = cursorPoint
        }
        CATransaction.commit()
    }

    /// Whether the renderer's layer currently holds a frame. This is the fact
    /// the live-tier harness had to walk the view tree for (§18).
    public var layerHasPixels: Bool { videoLayer.contents != nil }

    public func present(_ buffer: CVPixelBuffer?) {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        videoLayer.contents = buffer
        videoLayer.frame = geometry.contentRect
        CATransaction.commit()
    }

    // MARK: - Tracking

    public override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let trackingArea { removeTrackingArea(trackingArea) }
        let area = NSTrackingArea(rect: bounds,
                                  options: [.activeInKeyWindow, .mouseMoved, .mouseEnteredAndExited, .inVisibleRect],
                                  owner: self, userInfo: nil)
        addTrackingArea(area)
        trackingArea = area
    }

    public override var acceptsFirstResponder: Bool { isInteractive }
    public override func acceptsFirstMouse(for event: NSEvent?) -> Bool { isInteractive }

    private func viewPoint(for event: NSEvent) -> CGPoint {
        convert(event.locationInWindow, from: nil)
    }

    /// Place the overlay. The position is *derived from the mapping that
    /// produces the wire coordinate*, never computed separately — so if the two
    /// scales ever diverged, the cursor would visibly stop tracking the pointer
    /// instead of silently clicking elsewhere.
    private func updateOverlay(_ point: CGPoint) {
        guard showsCursorOverlay, let overlay = encoder.overlayPoint(for: point) else {
            cursorPoint = nil
            cursorLayer.isHidden = true
            return
        }
        cursorPoint = overlay
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        cursorLayer.position = overlay
        CATransaction.commit()
        cursorLayer.isHidden = false
    }

    // MARK: - Mouse

    public override func mouseMoved(with event: NSEvent) {
        guard isInteractive else { return }
        let point = viewPoint(for: event)
        updateOverlay(point)
        if let move = encoder.pointerMove(at: point, modifiers: InputEncoder.modifiers(from: event.modifierFlags)) {
            lastInFramePoint = point
            onInput?([move])
        }
    }

    public override func mouseExited(with event: NSEvent) {
        cursorPoint = nil
        cursorLayer.isHidden = true
        // Deliberately no event: there is no "pointer left" in the normalized
        // space, and the sentinel position that would express it is the exact
        // thing that breaks GHOST-style apps.
    }

    public override func mouseDown(with event: NSEvent) { beginDrag(event, button: .left) }
    public override func rightMouseDown(with event: NSEvent) { beginDrag(event, button: .right) }
    public override func otherMouseDown(with event: NSEvent) { beginDrag(event, button: .middle) }

    private func beginDrag(with event: NSEvent, button: PointerButton) { beginDrag(event, button: button) }

    private func beginDrag(_ event: NSEvent, button: PointerButton) {
        guard isInteractive else { return }
        window?.makeFirstResponder(self)
        let point = viewPoint(for: event)
        updateOverlay(point)
        let events = encoder.pointerDown(at: point, button: button,
                                         modifiers: InputEncoder.modifiers(from: event.modifierFlags))
        guard !events.isEmpty else { return }
        isDragging = true
        dragButton = button
        lastInFramePoint = point
        onInput?(events)
    }

    public override func mouseDragged(with event: NSEvent) { continueDrag(event) }
    public override func rightMouseDragged(with event: NSEvent) { continueDrag(event) }
    public override func otherMouseDragged(with event: NSEvent) { continueDrag(event) }

    private func continueDrag(_ event: NSEvent) {
        guard isInteractive, isDragging else { return }
        let point = viewPoint(for: event)
        updateOverlay(point)
        if let drag = encoder.drag(to: point, modifiers: InputEncoder.modifiers(from: event.modifierFlags)) {
            lastInFramePoint = point
            onInput?([drag])
        }
    }

    public override func mouseUp(with event: NSEvent) { endDrag(event) }
    public override func rightMouseUp(with event: NSEvent) { endDrag(event) }
    public override func otherMouseUp(with event: NSEvent) { endDrag(event) }

    private func endDrag(_ event: NSEvent) {
        guard isInteractive, isDragging else { return }
        isDragging = false
        let point = viewPoint(for: event)
        let modifiers = InputEncoder.modifiers(from: event.modifierFlags)
        // Release where the pointer actually is; if that is outside the frame,
        // release at the last point inside it. Never at a synthesized sentinel.
        if let up = encoder.pointerUp(at: point, button: dragButton, modifiers: modifiers) {
            onInput?([up])
        } else if let fallback = lastInFramePoint,
                  let up = encoder.pointerUp(at: fallback, button: dragButton, modifiers: modifiers) {
            onInput?([up])
        }
    }

    public override func scrollWheel(with event: NSEvent) {
        guard isInteractive else { return }
        let point = viewPoint(for: event)
        updateOverlay(point)
        let phase: GesturePhase
        switch event.phase {
        case .began: phase = .began
        case .changed: phase = .changed
        case .ended: phase = .ended
        case .cancelled: phase = .cancelled
        case .mayBegin: phase = .mayBegin
        default: phase = .none
        }
        let momentum: GesturePhase
        switch event.momentumPhase {
        case .began: momentum = .began
        case .changed: momentum = .changed
        case .ended: momentum = .ended
        case .cancelled: momentum = .cancelled
        default: momentum = .none
        }
        if let scroll = encoder.scroll(at: point,
                                       deltaX: Double(event.scrollingDeltaX),
                                       deltaY: Double(event.scrollingDeltaY),
                                       phase: phase, momentum: momentum,
                                       precise: event.hasPreciseScrollingDeltas) {
            onInput?([scroll])
        }
    }

    // MARK: - Keyboard

    public override func keyDown(with event: NSEvent) {
        guard isInteractive else { return super.keyDown(with: event) }
        onInput?(encoder.keyEvents(for: event, down: true))
    }

    public override func keyUp(with event: NSEvent) {
        guard isInteractive else { return super.keyUp(with: event) }
        onInput?(encoder.keyEvents(for: event, down: false))
    }

    /// Swallow the command chords AppKit would otherwise eat, so ⌘C in the
    /// stream reaches the Space rather than the host app.
    public override func performKeyEquivalent(with event: NSEvent) -> Bool {
        guard isInteractive, window?.firstResponder === self else { return false }
        // AppKit does not reliably send the key-up of a Command chord to the
        // view, and a press with no release is a stuck key on the Space:
        // send the chord whole. A late real key-up is a harmless repeat.
        onInput?(encoder.keyEvents(for: event, down: true) + encoder.keyEvents(for: event, down: false))
        return true
    }
}
#endif
