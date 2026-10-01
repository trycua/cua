// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

#if canImport(AppKit)
import AppKit
import Combine
import CoreGraphics
import Foundation

/// "This stream is being presented", published from the **view** layer.
///
/// `FRICTION.md` §18: *"Every counter that says 'the stream is working' —
/// decoded frames, surface size, status — lives on the session, and a session
/// decodes perfectly well into a view that was never mounted."* Proving live
/// pixels are in a product surface needed the view, and nothing in the stack
/// answered it, so OpenKoalaBots's harness had to host each tier in a real
/// `NSWindow`, walk the view tree for the renderer, and read its layer
/// contents.
///
/// This is that signal, as API: attach/detach, and frames **presented** as
/// opposed to frames decoded. It is deliberately a separate object from the
/// session — a session can have zero presenters, and that is the fact a caller
/// wants to know.
@MainActor
public final class StreamPresentation: ObservableObject {

    /// How many views currently host this stream. Zero means the stream is
    /// decoding into nothing.
    @Published public private(set) var presenterCount = 0

    /// Frames actually handed to a layer, which is strictly less than or equal
    /// to the session's decoded count.
    @Published public private(set) var framesPresented = 0

    /// Whether the most recently presented frame carried pixels, rather than
    /// being the `nil` a disconnected session publishes.
    @Published public private(set) var hasPixels = false

    /// The size of the last presented frame's content rect, in view points.
    @Published public private(set) var presentedContentSize: CGSize = .zero

    /// Whether any presenting view is currently forwarding input.
    @Published public private(set) var isInteractive = false

    public init() {}

    public var isPresented: Bool { presenterCount > 0 }

    /// A one-line summary in the shape the live-tier harness prints, so the
    /// evidence a caller produces does not have to be reinvented.
    public var evidenceLine: String {
        "streamViewMounted=\(isPresented) layerHasPixels=\(hasPixels) "
            + "framesPresented=\(framesPresented) interactive=\(isInteractive)"
    }

    // MARK: Called by the view layer

    public func viewDidAttach() { presenterCount += 1 }

    public func viewDidDetach() { presenterCount = max(0, presenterCount - 1) }

    public func viewDidPresent(pixels: Bool, contentSize: CGSize, interactive: Bool) {
        framesPresented += 1
        hasPixels = pixels
        presentedContentSize = contentSize
        isInteractive = interactive
    }
}
#endif
