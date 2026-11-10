// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CanvasModel
import CuaSpacesStreaming
import QuartzCore

/// One agent's cursor on a tile: Cua Driver's default theme, tinted with the
/// agent's presence color, plus a name pill in the same color.
///
/// The layer's `position` is the cursor tip (the theme's hotspot). Motion is
/// the model's `CursorGlide`, stepped by the canvas display link; this class
/// only draws.
final class AgentCursorLayer: CALayer {
    /// The theme canvas is drawn at this many points (the driver's
    /// `DISPLAY_SIZE`).
    static let displaySize: CGFloat = 42
    private static let theme = DotLottie.bundledTheme
    private static let hotspot = CGPoint(x: 55, y: 30)

    private let art = CALayer()
    private var parts: [(layer: CALayer, lottie: LottieAnimation.Layer)] = []
    private var tinted: [(CAShapeLayer, Bool, isStroke: Bool, LottieAnimation.Paint)] = []
    private let pill = CATextLayer()
    private let pillBackground = CALayer()
    private var animation: LottieAnimation?
    private var animationStart: CFTimeInterval = 0
    private(set) var agentStyle: AgentStyle

    var glide: CursorGlide
    /// Which way the arrow points (agents only; people stay upright).
    var heading = CursorHeading()
    private var lastPosition: CGPoint?

    /// A person's cursor: a plain pointer in their color, not the agent
    /// theme, so people and agents read differently at a glance.
    let isHuman: Bool
    private let humanArrow = CAShapeLayer()
    private let humanOutline = CAShapeLayer()
    /// The shared presence shape a person's cursor shows (`arrow`, `text`,
    /// `pointer`, ...), as their Space reports it.
    private(set) var humanShape = ""

    init(style: AgentStyle, at point: CGPoint, human: Bool = false) {
        agentStyle = style
        isHuman = human
        glide = CursorGlide(at: point)
        super.init()
        anchorPoint = .zero
        bounds = CGRect(x: 0, y: 0, width: 1, height: 1)
        masksToBounds = false
        let s = Self.displaySize / 128
        art.bounds = CGRect(x: 0, y: 0, width: 128, height: 128)
        art.anchorPoint = CGPoint(x: Self.hotspot.x / 128, y: Self.hotspot.y / 128)
        art.position = .zero
        art.setAffineTransform(CGAffineTransform(scaleX: s, y: s))
        art.isGeometryFlipped = false
        addSublayer(art)
        pillBackground.cornerRadius = 7
        pillBackground.anchorPoint = .zero
        addSublayer(pillBackground)
        pill.fontSize = 11
        pill.font = NSFont.systemFont(ofSize: 11, weight: .semibold)
        pill.alignmentMode = .center
        pill.contentsScale = 2
        pill.anchorPoint = .zero
        pillBackground.addSublayer(pill)
        if human {
            // The shared presence art (every Cua client draws the same
            // shapes), hot spot at the origin, outline under the fill.
            humanOutline.fillColor = nil
            humanOutline.strokeColor = CGColor(gray: 1, alpha: 1)
            humanOutline.lineJoin = .round
            humanArrow.strokeColor = nil
            addSublayer(humanOutline)
            addSublayer(humanArrow)
            setShape("arrow")
            art.isHidden = true
        } else {
            play("action_idle")
        }
        apply(style)
        position = point
    }

    override init(layer: Any) {
        let other = layer as! AgentCursorLayer
        agentStyle = other.agentStyle
        isHuman = other.isHuman
        glide = other.glide
        super.init(layer: layer)
    }

    required init?(coder: NSCoder) { fatalError() }

    /// A person's cursor takes the shape their Space reports for its
    /// position (the shared presence art, 24 pt).
    func setShape(_ shape: String) {
        guard isHuman, shape != humanShape else { return }
        humanShape = shape
        let art = PresenceCursorArt.art(for: shape)
        let k = 24 / CGFloat(art.canvas)
        let hot = PresenceCursorArt.hotspot(for: shape)
        var t = CGAffineTransform(scaleX: k, y: k).translatedBy(x: -hot.x, y: -hot.y)
        let path = PresenceCursorArt.path(for: shape).copy(using: &t)
        humanArrow.path = path
        humanOutline.path = path
        humanOutline.lineWidth = CGFloat(art.outlineWidth) * k
    }

    /// Recolor: the palette key takes the presence color, outlines stay.
    func apply(_ style: AgentStyle) {
        agentStyle = style
        let fill = CGColor(srgbRed: style.fill.r, green: style.fill.g, blue: style.fill.b, alpha: 1)
        for (shape, isKey, isStroke, paint) in tinted where isKey {
            let c = fill.copy(alpha: paint.opacity) ?? fill
            if isStroke { shape.strokeColor = c } else { shape.fillColor = c }
        }
        pillBackground.backgroundColor = fill
        humanArrow.fillColor = fill
        pill.foregroundColor = CGColor(srgbRed: style.text.r, green: style.text.g, blue: style.text.b, alpha: 1)
        pill.string = style.name
        let width = (style.name as NSString).size(withAttributes: [.font: NSFont.systemFont(ofSize: 11, weight: .semibold)]).width
        pillBackground.frame = CGRect(x: 16, y: 22, width: ceil(width) + 14, height: 18)
        pill.frame = CGRect(x: 0, y: 2, width: pillBackground.frame.width, height: 15)
    }

    /// Play a theme action (`action_click`, `action_text`, ...); idle when it
    /// ends.
    func play(_ name: String) {
        guard !isHuman, let theme = Self.theme, let anim = theme.animations[name] ?? theme.animations["action_idle"] else {
            return
        }
        if animation?.layers.count != anim.layers.count || parts.isEmpty || animationName != name {
            build(anim)
        }
        animation = anim
        animationName = name
        animationStart = CACurrentMediaTime()
        advance(to: animationStart)
    }

    private var animationName = ""

    private func build(_ anim: LottieAnimation) {
        art.sublayers?.forEach { $0.removeFromSuperlayer() }
        parts = []
        tinted = []
        for l in anim.layers {
            let container = CALayer()
            container.bounds = art.bounds
            container.anchorPoint = .zero
            container.position = .zero
            for item in l.items {
                let path = CGMutablePath()
                for p in item.paths { path.addPath(p) }
                if let f = item.fill {
                    let shape = CAShapeLayer()
                    shape.path = path
                    shape.fillColor = CGColor(srgbRed: f.color[0], green: f.color[1], blue: f.color[2], alpha: f.opacity)
                    shape.strokeColor = nil
                    container.addSublayer(shape)
                    tinted.append((shape, f.isPaletteKey, false, f))
                }
                if let s = item.stroke {
                    let shape = CAShapeLayer()
                    shape.path = path
                    shape.fillColor = nil
                    shape.lineWidth = s.width
                    shape.lineCap = s.roundCaps ? .round : .butt
                    shape.lineJoin = .round
                    shape.strokeColor = CGColor(srgbRed: s.paint.color[0], green: s.paint.color[1],
                                                blue: s.paint.color[2], alpha: s.paint.opacity)
                    container.addSublayer(shape)
                    tinted.append((shape, s.paint.isPaletteKey, true, s.paint))
                }
            }
            art.addSublayer(container)
            parts.append((container, l))
        }
        apply(agentStyle)
    }

    /// Turn the arrow toward its direction of travel. `velocity` is in the
    /// layer's parent coordinates per second. The art's anchor is the tip,
    /// so the rotation pivots on the hotspot and the tip never moves.
    func updateHeading(velocity: CGVector, dt: TimeInterval) {
        guard !isHuman else { return }
        heading.step(velocity: velocity, dt: dt)
        let s = Self.displaySize / 128
        art.setAffineTransform(CGAffineTransform(rotationAngle: CGFloat(heading.rotation)).scaledBy(x: s, y: s))
    }

    /// The heading from successive positions (for moves not driven by a
    /// glide, like a flight between tiles).
    func updateHeading(movedTo p: CGPoint, dt: TimeInterval) {
        defer { lastPosition = p }
        guard let last = lastPosition, dt > 0 else { return }
        updateHeading(velocity: CGVector(dx: (p.x - last.x) / dt, dy: (p.y - last.y) / dt), dt: dt)
    }

    /// Evaluate the running animation at `time` (display-link time).
    func advance(to time: CFTimeInterval) {
        guard let anim = animation else { return }
        var frame = (time - animationStart) * anim.frameRate + anim.inPoint
        if frame >= anim.outPoint - 1, animationName != "action_idle" {
            play("action_idle")
            return
        }
        frame = min(frame, anim.outPoint - 1)
        for (layer, l) in parts {
            let p = l.position.value(at: frame), a = l.anchor.value(at: frame)
            let s = l.scale.value(at: frame), r = l.rotation.value(at: frame).first ?? 0
            let o = (l.opacity.value(at: frame).first ?? 100) / 100
            var t = CGAffineTransform(translationX: p[0], y: p.count > 1 ? p[1] : 0)
            t = t.rotated(by: r * .pi / 180)
            t = t.scaledBy(x: (s.first ?? 100) / 100, y: (s.count > 1 ? s[1] : s.first ?? 100) / 100)
            t = t.translatedBy(x: -(a.first ?? 0), y: -(a.count > 1 ? a[1] : 0))
            layer.setAffineTransform(t)
            layer.opacity = Float(o)
        }
    }

    var isAnimating: Bool { animationName != "action_idle" }
}

/// The docked agent cursor: the Cua cursor body in the agent's presence
/// color, drawn as the thread window's title icon until the agent starts
/// using the computer.
enum DockGlyph {
    static func image(_ style: AgentStyle, size: CGFloat = 18) -> NSImage {
        NSImage(size: NSSize(width: size, height: size), flipped: true) { r in
            guard let body = DotLottie.bundledTheme?.animations["action_idle"]?.layers
                .first(where: { $0.name == "Cursor body" })?.items.first?.paths.first,
                let ctx = NSGraphicsContext.current?.cgContext else { return false }
            let b = body.boundingBoxOfPath
            let s = min(r.width / b.width, r.height / b.height) * 0.86
            ctx.translateBy(x: r.midX, y: r.midY)
            ctx.scaleBy(x: s, y: s)
            ctx.translateBy(x: -b.midX, y: -b.midY)
            ctx.addPath(body)
            ctx.setFillColor(CGColor(srgbRed: style.fill.r, green: style.fill.g, blue: style.fill.b, alpha: 1))
            ctx.setStrokeColor(CGColor(gray: 1, alpha: 1))
            ctx.setLineWidth(5)
            ctx.setLineJoin(.round)
            ctx.drawPath(using: .fillStroke)
            return true
        }
    }
}
