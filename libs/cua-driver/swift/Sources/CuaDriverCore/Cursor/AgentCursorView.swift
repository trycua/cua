import AppKit
import SwiftUI

/// SwiftUI overlay view that drives `AgentCursorRenderer.shared` every
/// display frame and draws the cursor arrow via `Canvas`. Hosted inside
/// `AgentCursorOverlayWindow` via an `NSHostingView`.
///
/// The cursor tip points in the direction of `renderer.heading`. Shape
/// matches the existing gradient-arrow design: a classic pointer with
/// the tip at upper-left, scaled for legibility at any display density.
public struct AgentCursorView: View {
    @Bindable var renderer: AgentCursorRenderer

    public init(renderer: AgentCursorRenderer = .shared) {
        self.renderer = renderer
    }

    public var body: some View {
        TimelineView(.animation(minimumInterval: 1.0 / 120.0)) { ctx in
            Canvas { gctx, size in
                renderer.tick(now: ctx.date.timeIntervalSinceReferenceDate)
                drawFocusRect(in: gctx, canvasSize: size)
                drawCursor(in: gctx)
            }
            .ignoresSafeArea()
            .allowsHitTesting(false)
        }
    }

    /// Draw a glowing highlight rectangle around the currently targeted AX
    /// element (if `renderer.focusRect` is set). Color is derived from the
    /// current cursor style's bloom color so the focus rect always matches
    /// the cursor's visual identity.
    private func drawFocusRect(in ctx: GraphicsContext, canvasSize: CGSize) {
        guard let screenRect = renderer.focusRect else { return }
        let r = CGRect(
            x: screenRect.minX,
            y: screenRect.minY,
            width: screenRect.width,
            height: screenRect.height
        )
        let cornerRadius: CGFloat = 4
        let rounded = Path(roundedRect: r, cornerRadius: cornerRadius)
        let baseColor = Color(nsColor: renderer.style.bloomColor)

        ctx.fill(rounded, with: .color(baseColor.opacity(0.08)))
        ctx.stroke(rounded, with: .color(baseColor.opacity(0.90)), lineWidth: 2)
        ctx.stroke(rounded, with: .color(baseColor.opacity(0.30)), lineWidth: 8)
    }

    /// Draw the cursor centered on `renderer.position`, rotated to
    /// `renderer.heading`. When `renderer.style.image` is set, draws that
    /// image instead of the default gradient arrow.
    private func drawCursor(in ctx: GraphicsContext) {
        let p = renderer.position
        guard p.x > -100 else { return }   // skip until first moveTo

        let style = renderer.style
        let bloomColor = Color(nsColor: style.bloomColor)
        let bloomR: CGFloat = 22
        let bloomRect = CGRect(x: p.x - bloomR, y: p.y - bloomR,
                               width: bloomR * 2, height: bloomR * 2)

        // Bloom halo — drawn first (underneath) regardless of cursor mode.
        ctx.fill(
            Path(ellipseIn: bloomRect),
            with: .radialGradient(
                Gradient(colors: [
                    bloomColor.opacity(style.bloomCenterAlpha),
                    bloomColor.opacity(style.bloomMidAlpha),
                    bloomColor.opacity(0),
                ]),
                center: p,
                startRadius: 0,
                endRadius: bloomR
            )
        )

        if let nsImage = style.image {
            // Custom image mode: draw the image centered on `p`, rotated
            // to heading. GraphicsContext is a value type — copy before
            // applying the transform so the bloom above is unaffected.
            var imgCtx = ctx
            imgCtx.translateBy(x: p.x, y: p.y)
            imgCtx.rotate(by: Angle(radians: renderer.heading + .pi))
            let s = style.shapeSize
            imgCtx.draw(Image(nsImage: nsImage),
                        in: CGRect(x: -s / 2, y: -s / 2, width: s, height: s))
        } else {
            // Procedural arrow mode: 4-vertex pointer shape with gradient fill.
            var shape = Path()
            shape.move(to: CGPoint(x: 14, y: 0))
            shape.addLine(to: CGPoint(x: -8, y: -9))
            shape.addLine(to: CGPoint(x: -3, y: 0))
            shape.addLine(to: CGPoint(x: -8, y: 9))
            shape.closeSubpath()

            let transform = CGAffineTransform(translationX: p.x, y: p.y)
                .rotated(by: CGFloat(renderer.heading + .pi))
            let transformed = shape.applying(transform)

            let gradientColors = style.strokeGradientStops.isEmpty
                ? AgentCursorStyle.defaultGradientStops.map { Color(nsColor: $0.color) }
                : style.strokeGradientStops.map { Color(nsColor: $0.color) }

            ctx.fill(
                transformed,
                with: .linearGradient(
                    Gradient(colors: gradientColors),
                    startPoint: CGPoint(x: p.x + 14, y: p.y - 9),
                    endPoint: CGPoint(x: p.x - 8, y: p.y + 9)
                )
            )
            ctx.stroke(transformed, with: .color(.white), lineWidth: style.strokeWidth)
        }
    }
}
