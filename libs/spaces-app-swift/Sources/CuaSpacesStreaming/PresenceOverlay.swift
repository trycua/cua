// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

#if canImport(AppKit)
import AppKit
import Cua
import QuartzCore
import SwiftUI

/// The multiplayer cursor layer over a live stream.
///
/// Every participant is drawn with the shared presence art
/// (`PresenceCursorArt`) in their presence color, including you:
///
/// * **Your cursor** is drawn at the local pointer on every frame, with no
///   network round trip. Only its *shape* comes from the Space (the shape
///   the guest shows at your position). The system cursor is hidden exactly
///   while your cursor is drawn (``SystemCursorRule``), never both at once.
/// * **Everyone else** comes from the SDK's `PresenceView`: interpolated a
///   little in the past, faded when idle, dropped when their run ends or
///   their heartbeat stops (libs/cua/proto/PRESENCE.md section 4).
///
/// The layer never takes clicks or keys (`hitTest` returns nil), so input
/// still goes to the stream view beneath it unchanged.
public struct PresenceOverlay: NSViewRepresentable {
    @ObservedObject public var session: LiveStreamSession

    public init(session: LiveStreamSession) {
        _session = ObservedObject(wrappedValue: session)
    }

    public func makeNSView(context: Context) -> PresenceOverlayView {
        let view = PresenceOverlayView()
        view.session = session
        return view
    }

    public func updateNSView(_ view: PresenceOverlayView, context: Context) {
        view.session = session
        view.surfaceSize = session.surfaceSize
        view.presenceView = session.presenceView
        view.windowID = session.presenceWindowID
        view.isStreamLive = session.status.isLive
    }

    public static func dismantleNSView(_ view: PresenceOverlayView, coordinator: ()) {
        view.stop()
    }
}

/// One cursor to draw, in view coordinates.
public struct PresenceCursorPlacement: Equatable, Sendable {
    public var participantID: String
    public var name: String
    public var color: String
    public var shape: String
    public var isMe: Bool
    public var isAgent: Bool
    public var alpha: CGFloat
    /// Where the pointer's hot spot goes, in view points.
    public var tip: CGPoint
}

/// Which drawables land where. Pure, so it is tested without a window.
public enum PresenceOverlayLayout {
    /// Places `drawables` over the letterboxed picture of `geometry`.
    /// Remote cursors on another target (another window, or a window while
    /// this view shows the desktop) are not drawn here; yours always is.
    public static func place(_ drawables: [CuaSDK.PresenceDrawable], geometry: StreamGeometry,
                             windowID: String?) -> [PresenceCursorPlacement] {
        guard geometry.isUsable else { return [] }
        return drawables.compactMap { d in
            if !d.isMe && (d.windowId ?? "") != (windowID ?? "") { return nil }
            guard d.alpha > 0.001 else { return nil }
            return PresenceCursorPlacement(
                participantID: d.participantId, name: d.displayName, color: d.color, shape: d.shape,
                isMe: d.isMe, isAgent: d.isAgent, alpha: CGFloat(d.alpha),
                tip: geometry.point(forNormalized: CGPoint(x: d.x, y: d.y)))
        }
    }
}

/// When the host's system cursor is hidden over a stream: exactly while
/// your own presence cursor is drawn there. Pure, so it is tested without a
/// window.
///
/// Your cursor is drawn only when presence is live (you have joined and the
/// SDK draws you), the stream is live with a real size, the pointer is over
/// the picture, and the window has the keyboard in the active app (AppKit
/// cannot hide the cursor for a background app, and a window that is not
/// key gets no pointer moves to follow). Anything else draws no custom
/// cursor, so the system cursor stays visible.
public enum SystemCursorRule {
    /// Whether your own cursor may be drawn (and the system one hidden).
    public static func drawsOwnCursor(presenceLive: Bool, streamLive: Bool, geometryUsable: Bool,
                                      hovering: Bool, focused: Bool) -> Bool {
        presenceLive && streamLive && geometryUsable && hovering && focused
    }

    /// Whether the system cursor is hidden, given what is actually drawn.
    public static func hidesSystemCursor(drawn: [PresenceCursorPlacement]) -> Bool {
        drawn.contains { $0.isMe && $0.alpha > 0.001 }
    }
}

/// Hides and shows the system cursor, balanced: `NSCursor.hide()` and
/// `unhide()` nest, so each owner hides at most once and always unhides
/// what it hid.
@MainActor
public final class SystemCursorHider {
    public private(set) var isHidden = false
    private let hide: () -> Void
    private let unhide: () -> Void

    public init(hide: @escaping () -> Void = { NSCursor.hide() },
                unhide: @escaping () -> Void = { NSCursor.unhide() }) {
        self.hide = hide
        self.unhide = unhide
    }

    public func set(hidden: Bool) {
        guard hidden != isHidden else { return }
        isHidden = hidden
        if hidden { hide() } else { unhide() }
    }
}

/// The overlay's AppKit view.
public final class PresenceOverlayView: NSView {
    /// Cursor art size in points (the canvas maps onto this square).
    public static let cursorSize: CGFloat = 24

    public weak var session: LiveStreamSession?
    /// The SDK model to draw. Nil hides everything (no presence).
    public var presenceView: PresenceView? {
        didSet {
            if presenceView !== oldValue, presenceView == nil { render(localMs: 0) }
        }
    }
    /// The streamed window, or nil for the desktop.
    public var windowID: String?
    public var surfaceSize: CGSize = .zero
    /// Whether the stream beneath is live. A stream that failed or ended
    /// draws no cursor of yours.
    public var isStreamLive = true
    /// Whether the window has the keyboard in the active app. Tests inject.
    public var isFocused: () -> Bool = { false }
    /// The system cursor's visibility for this view. Tests inject.
    public var cursorHider = SystemCursorHider()
    /// The local pointer in view points while it is over the view.
    public private(set) var localPoint: CGPoint?

    private var layers: [String: PresenceCursorLayer] = [:]
    private var monitor: Any?
    private var link: CADisplayLink?
    private var timer: Timer?
    private var lastExpire: Double = 0
    private var trackingArea: NSTrackingArea?
    private var observers: [NSObjectProtocol] = []

    public override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        wantsLayer = true
        layer?.masksToBounds = true
        isFocused = { [weak self] in
            guard let window = self?.window else { return false }
            return window.isKeyWindow && NSApp.isActive
        }
    }

    public required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }

    public override var isFlipped: Bool { true }

    /// Clicks and keys go to the stream view beneath.
    public override func hitTest(_ point: NSPoint) -> NSView? { nil }

    public var geometry: StreamGeometry {
        StreamGeometry(surfaceSize: surfaceSize, viewSize: bounds.size)
    }

    /// The picture, where your cursor can be drawn while presence is live.
    /// Nil otherwise.
    public var pictureRect: CGRect? {
        guard presenceView != nil, geometry.isUsable else { return nil }
        let rect = geometry.contentRect.intersection(bounds)
        return rect.isEmpty ? nil : rect
    }

    /// Whether the local pointer is over the picture.
    public var isHoveringStream: Bool {
        guard let localPoint, let rect = pictureRect else { return false }
        return rect.contains(localPoint)
    }

    /// Whether your cursor is drawn now (``SystemCursorRule``).
    public var drawsOwnCursor: Bool {
        SystemCursorRule.drawsOwnCursor(presenceLive: presenceView != nil, streamLive: isStreamLive,
                                        geometryUsable: geometry.isUsable, hovering: isHoveringStream,
                                        focused: isFocused())
    }

    // MARK: - Tracking

    /// Enter and exit reach this view directly (it is the owner), whatever
    /// `hitTest` says, so leaving the picture always restores the cursor.
    public override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let trackingArea { removeTrackingArea(trackingArea) }
        let area = NSTrackingArea(rect: bounds,
                                  options: [.activeAlways, .mouseEnteredAndExited, .mouseMoved, .inVisibleRect],
                                  owner: self, userInfo: nil)
        addTrackingArea(area)
        trackingArea = area
    }

    public override func mouseMoved(with event: NSEvent) { observe(event) }
    public override func mouseEntered(with event: NSEvent) { observe(event) }
    public override func mouseExited(with event: NSEvent) { pointer(at: nil) }

    // MARK: - Pointer

    public override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        stop()
        if window != nil { start() }
    }

    private func start() {
        guard monitor == nil else { return }
        // Losing the keyboard or the app going to the background ends your
        // cursor at once: no pointer moves arrive until it is back.
        let center = NotificationCenter.default
        let lost: @Sendable (Notification) -> Void = { [weak self] _ in
            MainActor.assumeIsolated { self?.pointer(at: nil) }
        }
        observers = [
            center.addObserver(forName: NSWindow.didResignKeyNotification, object: window, queue: .main,
                               using: lost),
            center.addObserver(forName: NSApplication.didResignActiveNotification, object: nil, queue: .main,
                               using: lost),
        ]
        // Observe (never consume) the pointer: moves and drags alike, since
        // a drag's events go to the view that took the mouse-down.
        monitor = NSEvent.addLocalMonitorForEvents(matching: [
            .mouseMoved, .leftMouseDragged, .rightMouseDragged, .otherMouseDragged,
            .mouseExited, .mouseEntered,
        ]) { [weak self] event in
            self?.observe(event)
            return event
        }
        let link = displayLink(target: self, selector: #selector(frame(_:)))
        link.add(to: .main, forMode: .common)
        self.link = link
    }

    /// Tears down the monitor and the frame loop.
    public func stop() {
        if let monitor { NSEvent.removeMonitor(monitor) }
        monitor = nil
        observers.forEach(NotificationCenter.default.removeObserver)
        observers = []
        link?.invalidate()
        link = nil
        pointer(at: nil)
        cursorHider.set(hidden: false)
    }

    private func observe(_ event: NSEvent) {
        guard let window, event.window === window else { return }
        let p = convert(event.locationInWindow, from: nil)
        pointer(at: bounds.contains(p) ? p : nil)
    }

    /// Moves your cursor. Drawn at once (next frame), published to the Space
    /// in the background, hidden for everyone when it leaves the picture.
    public func pointer(at point: CGPoint?) {
        let wasOver = isHoveringStream
        localPoint = point
        let over = isHoveringStream
        if let session, presenceView != nil, over || wasOver {
            session.movePresenceCursor(viewPoint: point ?? .zero, in: bounds.size, visible: over)
        }
        render(localMs: presenceNowMs())
    }

    @objc private func frame(_ link: CADisplayLink) {
        render(localMs: presenceNowMs())
    }

    // MARK: - Drawing

    /// Draws one frame at `localMs` on the presence clock. Public for tests.
    public func render(localMs: Double) {
        guard let presenceView else {
            layers.values.forEach { $0.removeFromSuperlayer() }
            layers = [:]
            cursorHider.set(hidden: false)
            return
        }
        if localMs - lastExpire >= 1_000 {
            lastExpire = localMs
            _ = presenceView.expire(localMs: localMs)
        }
        let pointer: PresencePoint? = drawsOwnCursor ? localPoint
            .flatMap { geometry.normalized(for: $0) }
            .map { PresencePoint(x: Double($0.x), y: Double($0.y)) } : nil
        let placed = PresenceOverlayLayout.place(presenceView.drawables(localMs: localMs, pointer: pointer),
                                                 geometry: geometry, windowID: windowID)
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        var seen = Set<String>()
        for p in placed {
            seen.insert(p.participantID)
            let l = layers[p.participantID] ?? {
                let l = PresenceCursorLayer(size: Self.cursorSize)
                layer?.addSublayer(l)
                layers[p.participantID] = l
                return l
            }()
            l.apply(p)
        }
        for (id, l) in layers where !seen.contains(id) {
            l.removeFromSuperlayer()
            layers[id] = nil
        }
        CATransaction.commit()
        cursorHider.set(hidden: SystemCursorRule.hidesSystemCursor(drawn: placed))
    }

    /// What is drawn now, for tests and diagnostics.
    public var drawn: [PresenceCursorPlacement] {
        layers.values.compactMap(\.placement).sorted { $0.participantID < $1.participantID }
    }
}

/// One participant's cursor: the shared art in their color with a white
/// outline, and a name pill for everyone but you.
public final class PresenceCursorLayer: CALayer {
    private let outline = CAShapeLayer()
    private let fill = CAShapeLayer()
    private let pill = CALayer()
    private let label = CATextLayer()
    private let size: CGFloat
    public private(set) var placement: PresenceCursorPlacement?
    private var shape = ""
    /// The art's bounds relative to the hot spot, in points.
    private var artBounds = CGRect.zero

    public init(size: CGFloat) {
        self.size = size
        super.init()
        anchorPoint = .zero
        bounds = CGRect(x: 0, y: 0, width: 1, height: 1)
        masksToBounds = false
        outline.fillColor = nil
        outline.lineJoin = .round
        fill.strokeColor = nil
        addSublayer(outline)
        addSublayer(fill)
        pill.cornerRadius = 7
        pill.anchorPoint = .zero
        label.fontSize = 11
        label.font = NSFont.systemFont(ofSize: 11, weight: .semibold)
        label.alignmentMode = .center
        label.contentsScale = NSScreen.main?.backingScaleFactor ?? 2
        label.anchorPoint = .zero
        pill.addSublayer(label)
        addSublayer(pill)
    }

    public override init(layer: Any) {
        size = (layer as? PresenceCursorLayer)?.size ?? 24
        super.init(layer: layer)
    }

    public required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }

    public func apply(_ p: PresenceCursorPlacement) {
        let reshaped = p.shape != shape
        if reshaped {
            shape = p.shape
            let art = PresenceCursorArt.art(for: p.shape)
            let k = size / CGFloat(art.canvas)
            let hot = PresenceCursorArt.hotspot(for: p.shape)
            // Canvas units, y-down, hot spot at this layer's origin.
            var t = CGAffineTransform(scaleX: k, y: k).translatedBy(x: -hot.x, y: -hot.y)
            let path = PresenceCursorArt.path(for: p.shape).copy(using: &t)
            outline.path = path
            fill.path = path
            outline.lineWidth = CGFloat(art.outlineWidth) * k
            outline.strokeColor = PresenceCursorArt.color(hex: art.outlineColor)
            artBounds = path?.boundingBoxOfPath ?? .zero
        }
        let color = PresenceCursorArt.color(hex: p.color)
        fill.fillColor = color
        // Your own cursor has no name; nor does a participant without one.
        pill.isHidden = p.isMe || p.name.isEmpty
        if !pill.isHidden, reshaped || placement?.name != p.name || placement?.color != p.color {
            pill.backgroundColor = color
            label.foregroundColor = PresenceCursorArt.color(hex: presenceTextColor(background: p.color))
            label.string = p.name
            let font = NSFont.systemFont(ofSize: 11, weight: .semibold)
            let width = ceil((p.name as NSString).size(withAttributes: [.font: font]).width) + 14
            // Below the art, starting at its middle, whatever the shape.
            pill.frame = CGRect(x: artBounds.midX, y: artBounds.maxY + 2, width: width, height: 18)
            label.frame = CGRect(x: 0, y: 1.5, width: width, height: 15)
        }
        opacity = Float(p.alpha)
        position = p.tip
        placement = p
    }
}

// MARK: - Presence identity

private struct PresenceNameKey: EnvironmentKey {
    static let defaultValue: String? = nil
}

extension EnvironmentValues {
    /// The name stream views join presence under (`SpaceScreenView` joins
    /// when it is set). Apps set it once at their root, usually to the
    /// signed-in account.
    public var presenceName: String? {
        get { self[PresenceNameKey.self] }
        set { self[PresenceNameKey.self] = newValue }
    }
}
#endif
