// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

#if canImport(AppKit)
import AppKit
#endif
import SwiftUI

/// The OpenKoalaBots koala, drawn with SwiftUI shapes.
///
/// Same geometry and colours as `samples/openkoalabots-assets/*.svg`: every
/// primitive below is one element of the SVG, in the SVG's own user units,
/// scaled into whatever frame the view is given. Edit the SVG and this file
/// together.
enum KoalaArt {

    /// One SVG element.
    enum Primitive {
        case circle(cx: CGFloat, cy: CGFloat, r: CGFloat, fill: UInt32)
        case ellipse(cx: CGFloat, cy: CGFloat, rx: CGFloat, ry: CGFloat, fill: UInt32)
        case roundedRect(x: CGFloat, y: CGFloat, w: CGFloat, h: CGFloat, r: CGFloat, fill: UInt32)
        /// A quadratic-curve stroke: `M a q c b` in SVG terms.
        case quadStroke(from: CGPoint, control: CGPoint, to: CGPoint, width: CGFloat, color: UInt32)
        /// A closed leaf: `M from c c1 c2 to` then a second curve back, filled.
        case leaf(Path, fill: UInt32)
        case curveStroke(Path, width: CGFloat, color: UInt32)
        case text(String, x: CGFloat, y: CGFloat, size: CGFloat, fill: UInt32)
    }

    // MARK: Palette (from the SVGs)

    static let earOuter: UInt32 = 0x8E98A3
    static let earInner: UInt32 = 0xE6E9ED
    static let head: UInt32 = 0xA9B2BC
    static let muzzle: UInt32 = 0xC3CAD1
    static let eye: UInt32 = 0x1B1E23
    static let glint: UInt32 = 0xFFFFFF
    static let nose: UInt32 = 0x2A2E35
    static let noseShine: UInt32 = 0x4A505A
    static let iconGround: UInt32 = 0x111214

    // MARK: koala-mark.svg (viewBox 0 0 256 256)

    static let markViewBox = CGSize(width: 256, height: 256)

    static let mark: [Primitive] = [
        .circle(cx: 60, cy: 88, r: 50, fill: earOuter),
        .circle(cx: 196, cy: 88, r: 50, fill: earOuter),
        .circle(cx: 56, cy: 90, r: 30, fill: earInner),
        .circle(cx: 200, cy: 90, r: 30, fill: earInner),
        .ellipse(cx: 128, cy: 142, rx: 84, ry: 76, fill: head),
        .ellipse(cx: 128, cy: 176, rx: 46, ry: 30, fill: muzzle),
        .circle(cx: 94, cy: 128, r: 9, fill: eye),
        .circle(cx: 162, cy: 128, r: 9, fill: eye),
        .circle(cx: 97, cy: 125, r: 3, fill: glint),
        .circle(cx: 165, cy: 125, r: 3, fill: glint),
        .ellipse(cx: 128, cy: 160, rx: 22, ry: 29, fill: nose),
        .ellipse(cx: 121, cy: 149, rx: 6, ry: 8, fill: noseShine),
    ]

    // MARK: app-icon.svg (viewBox 0 0 512 512)

    static let iconViewBox = CGSize(width: 512, height: 512)

    /// The mark, placed by the icon's `translate(96 104) scale(1.25)`.
    static let appIcon: [Primitive] =
        [.roundedRect(x: 28, y: 28, w: 456, h: 456, r: 104, fill: iconGround)]
        + mark.map { transformed($0, dx: 96, dy: 104, scale: 1.25) }

    // MARK: koala-peek.svg (viewBox 0 0 320 220)

    static let peekViewBox = CGSize(width: 320, height: 220)

    static let peek: [Primitive] = [
        .circle(cx: 102, cy: 70, r: 40, fill: earOuter),
        .circle(cx: 218, cy: 70, r: 40, fill: earOuter),
        .circle(cx: 99, cy: 72, r: 24, fill: earInner),
        .circle(cx: 221, cy: 72, r: 24, fill: earInner),
        .ellipse(cx: 160, cy: 128, rx: 70, ry: 64, fill: head),
        .ellipse(cx: 160, cy: 156, rx: 38, ry: 24, fill: muzzle),
        .circle(cx: 132, cy: 116, r: 7.5, fill: eye),
        .circle(cx: 188, cy: 116, r: 7.5, fill: eye),
        .circle(cx: 134.5, cy: 113.5, r: 2.5, fill: glint),
        .circle(cx: 190.5, cy: 113.5, r: 2.5, fill: glint),
        .ellipse(cx: 160, cy: 142, rx: 18, ry: 24, fill: nose),
        .ellipse(cx: 154, cy: 133, rx: 5, ry: 6.5, fill: noseShine),
        .roundedRect(x: 20, y: 170, w: 280, h: 44, r: 14, fill: 0xD9DDE2),
        .ellipse(cx: 112, cy: 172, rx: 24, ry: 14, fill: 0x9AA3AD),
        .ellipse(cx: 208, cy: 172, rx: 24, ry: 14, fill: 0x9AA3AD),
        .circle(cx: 100, cy: 176, r: 3, fill: 0x7B848E),
        .circle(cx: 112, cy: 178, r: 3, fill: 0x7B848E),
        .circle(cx: 124, cy: 176, r: 3, fill: 0x7B848E),
        .circle(cx: 196, cy: 176, r: 3, fill: 0x7B848E),
        .circle(cx: 208, cy: 178, r: 3, fill: 0x7B848E),
        .circle(cx: 220, cy: 176, r: 3, fill: 0x7B848E),
    ]

    // MARK: koala-sleep.svg (viewBox 0 0 256 256)

    static let sleep: [Primitive] = {
        // `M186 232c28-18 44-48 38-80-30 8-50 34-50 64`, absolute.
        var leaf = Path()
        leaf.move(to: CGPoint(x: 186, y: 232))
        leaf.addCurve(to: CGPoint(x: 224, y: 152),
                      control1: CGPoint(x: 214, y: 214), control2: CGPoint(x: 230, y: 184))
        leaf.addCurve(to: CGPoint(x: 174, y: 216),
                      control1: CGPoint(x: 194, y: 160), control2: CGPoint(x: 174, y: 186))
        leaf.closeSubpath()
        // `M204 170c-10 18-18 38-22 58`, absolute.
        var stem = Path()
        stem.move(to: CGPoint(x: 204, y: 170))
        stem.addCurve(to: CGPoint(x: 182, y: 228),
                      control1: CGPoint(x: 194, y: 188), control2: CGPoint(x: 186, y: 208))
        let face = mark.filter {
            // Eyes open become eyes closed: drop the eye and glint circles.
            if case let .circle(_, _, _, fill) = $0 { return fill != eye && fill != glint }
            return true
        }
        let head = Array(face.prefix(6)), nose = Array(face.suffix(2))
        return [.leaf(leaf, fill: 0x6FA37A),
                .curveStroke(stem, width: 4, color: 0x4D7D57)]
            + head
            + [.quadStroke(from: CGPoint(x: 83, y: 130), control: CGPoint(x: 94, y: 139),
                           to: CGPoint(x: 105, y: 130), width: 5, color: eye),
               .quadStroke(from: CGPoint(x: 151, y: 130), control: CGPoint(x: 162, y: 139),
                           to: CGPoint(x: 173, y: 130), width: 5, color: eye)]
            + nose
            + [.text("z", x: 196, y: 44, size: 26, fill: 0x8E98A3),
               .text("z", x: 216, y: 26, size: 18, fill: 0xB3BAC2)]
    }()

    // MARK: Drawing

    static func transformed(_ p: Primitive, dx: CGFloat, dy: CGFloat, scale s: CGFloat) -> Primitive {
        switch p {
        case let .circle(cx, cy, r, fill):
            return .circle(cx: dx + cx * s, cy: dy + cy * s, r: r * s, fill: fill)
        case let .ellipse(cx, cy, rx, ry, fill):
            return .ellipse(cx: dx + cx * s, cy: dy + cy * s, rx: rx * s, ry: ry * s, fill: fill)
        case let .roundedRect(x, y, w, h, r, fill):
            return .roundedRect(x: dx + x * s, y: dy + y * s, w: w * s, h: h * s, r: r * s, fill: fill)
        default:
            return p
        }
    }

    static func draw(_ primitives: [Primitive], viewBox: CGSize,
                     in ctx: inout GraphicsContext, size: CGSize) {
        let k = min(size.width / viewBox.width, size.height / viewBox.height)
        let ox = (size.width - viewBox.width * k) / 2
        let oy = (size.height - viewBox.height * k) / 2
        ctx.translateBy(x: ox, y: oy)
        ctx.scaleBy(x: k, y: k)
        for p in primitives {
            switch p {
            case let .circle(cx, cy, r, fill):
                ctx.fill(Path(ellipseIn: CGRect(x: cx - r, y: cy - r, width: 2 * r, height: 2 * r)),
                         with: .color(Color(hex: fill)))
            case let .ellipse(cx, cy, rx, ry, fill):
                ctx.fill(Path(ellipseIn: CGRect(x: cx - rx, y: cy - ry, width: 2 * rx, height: 2 * ry)),
                         with: .color(Color(hex: fill)))
            case let .roundedRect(x, y, w, h, r, fill):
                ctx.fill(Path(roundedRect: CGRect(x: x, y: y, width: w, height: h), cornerRadius: r),
                         with: .color(Color(hex: fill)))
            case let .quadStroke(from, control, to, width, color):
                var path = Path()
                path.move(to: from)
                path.addQuadCurve(to: to, control: control)
                ctx.stroke(path, with: .color(Color(hex: color)),
                           style: StrokeStyle(lineWidth: width, lineCap: .round))
            case let .leaf(path, fill):
                ctx.fill(path, with: .color(Color(hex: fill)))
            case let .curveStroke(path, width, color):
                ctx.stroke(path, with: .color(Color(hex: color)),
                           style: StrokeStyle(lineWidth: width, lineCap: .round))
            case let .text(s, x, y, size, fill):
                // SVG `y` is the baseline; SwiftUI anchors on the glyph box.
                ctx.draw(Text(s).font(.system(size: size, weight: .semibold))
                            .foregroundColor(Color(hex: fill)),
                         at: CGPoint(x: x, y: y), anchor: .bottomLeading)
            }
        }
    }
}

/// The koala mark (`koala-mark.svg`).
struct KoalaMark: View {
    var size: CGFloat = 32
    var body: some View {
        Canvas { ctx, s in KoalaArt.draw(KoalaArt.mark, viewBox: KoalaArt.markViewBox, in: &ctx, size: s) }
            .frame(width: size, height: size)
            .accessibilityLabel("OpenKoalaBots koala")
    }
}

/// The koala peeking over a ledge (`koala-peek.svg`): empty states.
struct KoalaPeek: View {
    var width: CGFloat = 160
    var body: some View {
        Canvas { ctx, s in KoalaArt.draw(KoalaArt.peek, viewBox: KoalaArt.peekViewBox, in: &ctx, size: s) }
            .frame(width: width, height: width * KoalaArt.peekViewBox.height / KoalaArt.peekViewBox.width)
            .accessibilityHidden(true)
    }
}

/// The sleeping koala (`koala-sleep.svg`): idle states.
struct KoalaSleep: View {
    var size: CGFloat = 96
    var body: some View {
        Canvas { ctx, s in KoalaArt.draw(KoalaArt.sleep, viewBox: KoalaArt.markViewBox, in: &ctx, size: s) }
            .frame(width: size, height: size)
            .accessibilityHidden(true)
    }
}

/// The app icon (`app-icon.svg`).
struct KoalaAppIcon: View {
    var size: CGFloat = 512
    var body: some View {
        Canvas { ctx, s in KoalaArt.draw(KoalaArt.appIcon, viewBox: KoalaArt.iconViewBox, in: &ctx, size: s) }
            .frame(width: size, height: size)
    }

    #if canImport(AppKit)
    /// A SwiftPM executable has no bundle and so no `.icns`: the Dock icon is
    /// drawn from the same geometry at launch.
    @MainActor
    static func install() {
        let renderer = ImageRenderer(content: KoalaAppIcon(size: 512))
        renderer.scale = 2
        if let image = renderer.nsImage { NSApplication.shared.applicationIconImage = image }
    }
    #endif
}
