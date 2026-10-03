// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CoreGraphics
import Cua
import CuaSpaces
import Foundation

/// The Space itself, as a stream source.
///
/// `FRICTION.md` §10: the stream tiers used to depend on a two-method protocol
/// the Spaces client did not implement. The Space now carries both halves —
/// `Space.windows()` for targets and the SDK's `SpaceStreamSession` for frames
/// — so the adapter is this file.
public final class SpaceStreamProvider: SpaceStreamSourceProviding, @unchecked Sendable {
    private let space: CuaSpaces.Space
    /// Restrict the source list to one run's own window when the join in
    /// `Space.window(for:)` can be made (§37). `nil` lists everything.
    private let run: RunID?
    /// Frame-rate cap for sessions (0 = the driver's default).
    public var maxFPS: UInt32 = 0

    public init(space: CuaSpaces.Space, run: RunID? = nil) {
        self.space = space
        self.run = run
    }

    public func availableWindows() async throws -> [StreamWindow] {
        let windows = try await space.windows()
        if let run, let mine = (try? await space.window(for: run)) ?? nil {
            return [StreamWindow(mine)]
        }
        return windows.map(StreamWindow.init)
    }

    private func handle() async throws -> CuaSDK.Space {
        guard let native = try await space.native() else {
            throw StreamError.noStream("this Space is not backed by the cua SDK")
        }
        return native
    }

    public func openSession(_ source: StreamSource, frames: FrameSink,
                            audio: AudioSink?) async throws -> SpaceStreamSession {
        try await openSession(source, policy: Self.inputPolicy(for: source),
                              frames: frames, audio: audio)
    }

    public func openSession(_ source: StreamSource, policy: String, frames: FrameSink,
                            audio: AudioSink?) async throws -> SpaceStreamSession {
        // A Space still being created has no stream yet: say so rather than
        // dialling a guest that is not up.
        if space.state == .starting {
            throw StreamError.noStream("\(space.id.rawValue) is still starting")
        }
        var options = SpaceStreamOptions(maxFps: maxFPS, audio: audio != nil, policy: policy)
        switch source {
        case .desktop:
            break
        case let .window(window):
            options.windowId = window.id
        }
        return try await handle().streamSession(options: options, frames: frames, audio: audio)
    }

    /// The input policy a viewer of `source` opens its session with.
    ///
    /// A window takes input in the background (no focus change on the
    /// Space). A display has no window to address background input to: the
    /// driver refuses `background_only` there (Linux and macOS alike), so
    /// every click on a desktop stream went nowhere. The desktop is driven
    /// like a physical mouse and keyboard, which is `allow_activation`: the
    /// same policy the Tauri and HTML5 viewers use for it. A window whose
    /// input the Space can only deliver by activating it (Hyprland outside
    /// its qualified apps, X11 toolkits that drop synthetic events) is
    /// refused as would-require-activation; `LiveStreamSession` then reopens
    /// that window with `allow_activation`.
    public static func inputPolicy(for source: StreamSource) -> String {
        switch source {
        case .desktop: return "allow_activation"
        case .window: return "background_only"
        }
    }

    public func joinPresence(name: String, color: String?) async throws -> SpacePresence {
        let id = "swift-\(ProcessInfo.processInfo.processIdentifier)-\(name)"
        return try await handle().joinPresence(
            identity: PresenceIdentity(id: id, displayName: name, color: color ?? "", agent: false),
            timeoutMs: 10_000)
    }
}

extension StreamWindow {
    /// Build a stream target from a Space's own window listing. The window
    /// list does not carry a target epoch; epoch 1 is right for a freshly
    /// enumerated window.
    public init(_ window: CuaSpaces.SpaceWindow) {
        self.init(id: window.id.rawValue,
                  app: window.app,
                  title: window.title,
                  epoch: 1,
                  surfaceSize: window.pixelSize,
                  scaleFactor: window.scaleFactor,
                  appID: window.appID,
                  processID: UInt32(clamping: max(0, window.processID ?? 0)))
    }
}
