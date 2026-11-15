// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CoreGraphics
import Cua
import Foundation

/// A window in the Space that can be streamed.
///
/// Deliberately a *value* type owned by `Streaming/`, not the Spaces client's
/// `SpaceWindow`. The two carry the same human fields, but a stream also needs
/// the target epoch and the surface geometry, and the streaming code must
/// compile and be testable with no Spaces implementation present at all.
public struct StreamWindow: Identifiable, Hashable, Sendable {
    /// Opaque window handle from the Space's window list. Never a PID.
    public var id: String
    public var app: String
    public var title: String
    /// Target identity generation. A handle is only valid with its current
    /// epoch; reusing a stale pair returns `stale_target`.
    public var epoch: UInt64
    public var surfaceSize: CGSize
    public var scaleFactor: CGFloat
    /// The app's id and process as the window list reports them (empty and
    /// 0 when unknown): what `Space.appIcon` looks the icon up by.
    public var appID: String
    public var processID: UInt32
    /// For a window another Space shows here: the Space it lives in
    /// (id, name, OS). `nil` for the Space's own windows.
    public var origin: StreamWindowOrigin?

    public init(id: String, app: String, title: String, epoch: UInt64 = 1,
         surfaceSize: CGSize = .zero, scaleFactor: CGFloat = 1,
         appID: String = "", processID: UInt32 = 0, origin: StreamWindowOrigin? = nil) {
        self.origin = origin
        self.id = id
        self.app = app
        self.title = title
        self.epoch = epoch
        self.surfaceSize = surfaceSize
        self.scaleFactor = scaleFactor
        self.appID = appID
        self.processID = processID
    }

    /// Build a stream target from the three fields every window list has.
    public static func fromListing(id: String, app: String, title: String, epoch: UInt64 = 1) -> StreamWindow {
        StreamWindow(id: id, app: app, title: title, epoch: epoch)
    }

    /// What the source switcher shows.
    public var displayName: String {
        title.isEmpty ? app : "\(app): \(title)"
    }
}

/// The Space a shown window lives in.
public struct StreamWindowOrigin: Hashable, Sendable {
    public var spaceID: String
    public var name: String
    public var os: String

    public init(spaceID: String, name: String, os: String = "") {
        self.spaceID = spaceID
        self.name = name
        self.os = os
    }
}

/// Which surface a stream view is showing.
public enum StreamSource: Hashable, Sendable {
    /// The whole (primary) display: a display target of the same media
    /// session a window uses, so frames, input and presence work the same.
    case desktop
    case window(StreamWindow)

    public var label: String {
        switch self {
        case .desktop: return "Full desktop"
        case let .window(window): return window.displayName
        }
    }
}

/// The narrow contract `Streaming/` needs from whatever knows about Spaces:
/// a list of windows, a way to open a media session on a source, and presence.
public protocol SpaceStreamSourceProviding: AnyObject, Sendable {
    /// Windows the user could reasonably pick.
    func availableWindows() async throws -> [StreamWindow]

    /// Opens a media session on `source` and starts delivering encoded frames
    /// to `frames` (and audio to `audio`) on the SDK's delivery thread.
    func openSession(_ source: StreamSource, frames: FrameSink,
                     audio: AudioSink?) async throws -> SpaceStreamSession

    /// Opens `source` under an explicit input `policy` (`background_only`,
    /// `allow_activation`). A viewer asks for `allow_activation` on a window
    /// after the Space refused background input to it.
    func openSession(_ source: StreamSource, policy: String, frames: FrameSink,
                     audio: AudioSink?) async throws -> SpaceStreamSession

    /// Joins the Space's presence as `name`.
    func joinPresence(name: String, color: String?) async throws -> SpacePresence

    /// Joins presence for the surface being watched. A window another Space
    /// shows here, streamed straight from its origin, joins the origin's
    /// channel and this Space's together (one avatar and one cursor per
    /// viewer); any other source is `joinPresence(name:color:)`.
    func joinPresence(name: String, color: String?, source: StreamSource) async throws -> SpacePresence

    /// Whether ⌘ chords go to the Space with Control instead of Command: a
    /// Linux or Windows guest, where Command arrives as Super (⌘C would be
    /// Super+C, which types "c"). Known once a session opened.
    var commandAsControl: Bool { get }
}

extension SpaceStreamSourceProviding {
    public func joinPresence(name: String, color: String?, source: StreamSource) async throws -> SpacePresence {
        try await joinPresence(name: name, color: color)
    }

    /// Providers that don't know the guest send ⌘ as it is.
    public var commandAsControl: Bool { false }

    /// Providers that choose their own policy ignore the requested one.
    public func openSession(_ source: StreamSource, policy: String, frames: FrameSink,
                            audio: AudioSink?) async throws -> SpaceStreamSession {
        try await openSession(source, frames: frames, audio: audio)
    }
}

/// A provider with no Space behind it, for previews and for exercising the view
/// layer offline. It reports no windows and refuses to stream, so a view bound
/// to it shows its failure state rather than pretending.
public final class OfflineStreamSourceProvider: SpaceStreamSourceProviding, @unchecked Sendable {
    public init() {}

    public func availableWindows() async throws -> [StreamWindow] { [] }

    public func openSession(_ source: StreamSource, frames: FrameSink,
                            audio: AudioSink?) async throws -> SpaceStreamSession {
        throw StreamError.noStream("no Space is attached in this build")
    }

    public func joinPresence(name: String, color: String?) async throws -> SpacePresence {
        throw StreamError.noStream("no Space is attached in this build")
    }
}
