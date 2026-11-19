// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpaces
import CuaSpacesStreaming
#if canImport(AppKit)
import AppKit
import CoreImage
import CoreVideo
import SwiftUI

/// Proves that the Agent Computer tiers show **live** pixels, by mounting the
/// real tier views against a real Space and reporting what the views ended up
/// holding.
///
/// It deliberately does not assert from the session alone. A session can be
/// decoding perfectly into a view that was never mounted, which is exactly the
/// claim ("live streaming is wired into the tiers") that needs proving. So the
/// harness walks the mounted view tree, finds the `LiveStreamInputView` that
/// `SpaceScreenView` puts up, and reports its layer's contents and its
/// interactivity flag — the same flag that decides whether input is forwarded.
@MainActor
enum LiveTierHarness {

    struct Report {
        var tier: String
        var decodedFrames: Int
        var surface: CGSize
        var status: String
        /// The tier's own `LiveStreamInputView` was found in the window.
        var streamViewMounted: Bool
        /// That view's `CALayer` is holding a decoded frame.
        var layerHasPixels: Bool
        /// The view is forwarding input to the Space.
        var interactive: Bool
        var inputSent: Int
        var inputAcknowledged: UInt64
        var pngPath: String?

        var line: String {
            """
            \(tier): frames=\(decodedFrames) surface=\(Int(surface.width))x\(Int(surface.height)) \
            status=\(status) streamViewMounted=\(streamViewMounted) layerHasPixels=\(layerHasPixels) \
            interactive=\(interactive) inputSent=\(inputSent) inputAcked=\(inputAcknowledged)\
            \(pngPath.map { " png=\($0)" } ?? "")
            """
        }
    }

    /// Put a view on screen in an ordinary window and let it run.
    ///
    /// An offscreen `ImageRenderer` cannot be used here: `LiveStreamView` is an
    /// `NSViewRepresentable` drawing decoded frames straight into a `CALayer`,
    /// and `ImageRenderer` never instantiates the NSView at all. Proving that
    /// the *tier* is live therefore requires a real window server.
    static func host<V: View>(_ view: V, size: CGSize) -> NSWindow {
        let window = NSWindow(contentRect: NSRect(origin: .zero, size: size),
                              styleMask: [.titled, .closable, .resizable],
                              backing: .buffered, defer: false)
        window.contentView = NSHostingView(rootView: view.frame(width: size.width, height: size.height))
        window.makeKeyAndOrderFront(nil)
        return window
    }

    /// Spin the run loop so AppKit, the WebSocket and VideoToolbox all make
    /// progress. `Task.sleep` alone would starve the main run loop and nothing
    /// would ever be decoded.
    static func pump(seconds: Double) {
        let deadline = Date().addingTimeInterval(seconds)
        while Date() < deadline {
            RunLoop.current.run(mode: .default, before: Date().addingTimeInterval(0.05))
        }
    }

    static func findStreamView(in window: NSWindow) -> LiveStreamInputView? {
        func walk(_ view: NSView) -> LiveStreamInputView? {
            if let found = view as? LiveStreamInputView { return found }
            for sub in view.subviews { if let found = walk(sub) { return found } }
            return nil
        }
        return window.contentView.flatMap(walk)
    }

    static func writePNG(_ buffer: CVPixelBuffer?, to path: String) -> String? {
        guard let buffer else { return nil }
        let image = CIImage(cvPixelBuffer: buffer)
        let context = CIContext()
        guard let cg = context.createCGImage(image, from: image.extent) else { return nil }
        let rep = NSBitmapImageRep(cgImage: cg)
        guard let data = rep.representation(using: .png, properties: [:]) else { return nil }
        try? data.write(to: URL(fileURLWithPath: path))
        return path
    }

    static func report(_ tier: String, session: LiveStreamSession, window: NSWindow,
                       pngDirectory: String?) -> Report {
        let view = findStreamView(in: window)
        let png = pngDirectory.flatMap {
            writePNG(session.frame, to: "\($0)/\(tier).png")
        }
        return Report(tier: tier,
                      decodedFrames: session.decodedFrameCount,
                      surface: session.surfaceSize,
                      status: "\(session.status)",
                      streamViewMounted: view != nil,
                      layerHasPixels: view?.hasDecodedPixels ?? false,
                      interactive: view?.isInteractive ?? false,
                      inputSent: session.inputEventsSent,
                      inputAcknowledged: session.inputEventsAcknowledged,
                      pngPath: png)
    }
}

extension LiveStreamInputView {
    /// Whether the layer this view actually presents is holding a frame.
    ///
    /// Read off the view rather than the session on purpose: this is the last
    /// link in the chain, and the only one that proves the *tier* — not just
    /// the transport — is showing the Space.
    var hasDecodedPixels: Bool {
        guard let sublayers = layer?.sublayers else { return false }
        return sublayers.contains { $0.contents != nil }
    }
}
#endif
