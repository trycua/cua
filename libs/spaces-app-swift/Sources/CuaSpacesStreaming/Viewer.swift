// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CoreGraphics
import CoreVideo
import CuaSpaces
import Foundation

// The screen, as one noun.
//
// The codec is not part of this API. A `Viewer` carries pixels, input and
// control together, and you obtain one by naming the agent you want to watch
// rather than by hunting a window id through a list of ninety-two entries.
//
// `RCDPConnection`, `H264Decoder`, `InputEncoder` and `RCDPWire` remain public
// for a caller writing a second renderer; nothing in *this* file mentions them.

/// Where a stream is, in one type.
///
/// Typed, because branching on a status string is how a UI ends up with five
/// spellings of "connecting" — and it keeps `rawStatus`, the way `SpaceState`
/// keeps `rawPhase`, so normalising never discards what the transport said.
public struct StreamStatus: Sendable, Hashable {
    public enum Phase: String, Sendable, Hashable {
        case idle, connecting, live, reconnecting, stopped, failed
    }

    public let phase: Phase
    /// Whatever the transport actually said. Normalisation never discards it.
    public let rawStatus: String
    /// Whether this stream ended the way it was supposed to.
    ///
    /// A clean stop and a dropped connection are both "not live", and a UI that
    /// cannot tell them apart shows a reconnect spinner after a deliberate
    /// close. `true` for a stream that is live or was stopped on purpose.
    public let isClean: Bool

    public init(phase: Phase, rawStatus: String, isClean: Bool) {
        self.phase = phase
        self.rawStatus = rawStatus
        self.isClean = isClean
    }

    public var isLive: Bool { phase == .live }

    init(_ status: LiveStreamSession.Status) {
        switch status {
        case .idle:
            self.init(phase: .idle, rawStatus: "idle", isClean: true)
        case .connecting:
            self.init(phase: .connecting, rawStatus: "connecting", isClean: true)
        case .streaming:
            self.init(phase: .live, rawStatus: "streaming", isClean: true)
        case let .suspended(why):
            self.init(phase: .reconnecting, rawStatus: why, isClean: false)
        case let .failed(why):
            self.init(phase: .failed, rawStatus: why, isClean: false)
        }
    }
}

/// What a viewer can say about itself without being awaited.
///
/// **A SwiftUI `body` cannot `await`.** Every stream in this SDK is therefore
/// paired with a synchronous snapshot of the same facts, so a view renders the
/// current state on the first frame rather than an empty placeholder until the
/// first element arrives.
public struct ViewerSnapshot: Sendable {
    public let status: StreamStatus
    public let surfaceSize: CGSize
    public let framesDecoded: Int
    /// Frames actually **presented** on screen, which is a different number
    /// from frames decoded: a session decodes perfectly into a view that was
    /// never mounted, and every other counter would call that "working".
    public let framesPresented: Int
    public let inputSent: Int
    public let inputAcknowledged: UInt64
    public var hasPixels: Bool { framesDecoded > 0 }
    public var isPresented: Bool { framesPresented > 0 }

    /// One line to paste into a bug report. "Laggy" is not a bug report.
    public var evidenceLine: String {
        "presented=\(framesPresented) decoded=\(framesDecoded) "
        + "input=\(inputSent)/\(inputAcknowledged) status=\(status.rawStatus) "
        + "surface=\(Int(surfaceSize.width))x\(Int(surfaceSize.height))"
    }
}

/// What to watch.
public enum WatchTarget: Sendable, Hashable {
    case desktop
    case window(WindowID)
    /// The window an agent is working in, resolved server-side by pid.
    case agent(RunID)
}

/// One live view of a Space: pixels out, input in.
///
/// Coordinates are **normalized `0...1` and nothing else**. That is the
/// vocabulary the wire already speaks (`x_normalized`, `y_normalized`), and a
/// surface pixel is the wrong unit for an API whose surface can be rescaled
/// between two frames. `StreamGeometry` still exists for the view layer, which
/// genuinely has to convert points to that vocabulary once, at the boundary.
@MainActor
public final class Viewer {

    /// The session underneath. Public because the SwiftUI views in this module
    /// take one, and because a caller mid-migration should not be stuck.
    public let session: LiveStreamSession

    public init(session: LiveStreamSession) {
        self.session = session
    }

    /// The current state, synchronously, for a `body` that cannot await.
    public var snapshot: ViewerSnapshot {
        ViewerSnapshot(status: StreamStatus(session.status),
                       surfaceSize: session.surfaceSize,
                       framesDecoded: session.decodedFrameCount,
                       framesPresented: session.presentation.framesPresented,
                       inputSent: session.inputEventsSent,
                       inputAcknowledged: session.inputEventsAcknowledged)
    }

    /// Status changes, for a caller that would rather be told.
    public var statusUpdates: AsyncStream<StreamStatus> {
        AsyncStream { continuation in
            let task = Task { @MainActor in
                var last: StreamStatus?
                while !Task.isCancelled {
                    let now = StreamStatus(session.status)
                    if now != last {
                        continuation.yield(now)
                        last = now
                    }
                    try? await Task.sleep(for: .milliseconds(120))
                }
                continuation.finish()
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    /// Decoded frames. Paired with `snapshot`, which answers the same question
    /// without awaiting.
    public var frames: AsyncStream<CVPixelBuffer> {
        AsyncStream { continuation in
            let task = Task { @MainActor in
                var lastCount = -1
                while !Task.isCancelled {
                    if session.decodedFrameCount != lastCount, let frame = session.frame {
                        lastCount = session.decodedFrameCount
                        // A decoded buffer is never written again (see
                        // `DecodedFrame`), so the stream may share it.
                        nonisolated(unsafe) let shared = frame
                        continuation.yield(shared)
                    }
                    try? await Task.sleep(for: .milliseconds(8))
                }
                continuation.finish()
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    /// Retarget without tearing the viewer down.
    public func show(_ target: WatchTarget, in space: Space) async throws {
        await session.select(try await Screen.source(for: target, in: space, session: session))
    }

    public func stop() async { await session.stop() }

    // MARK: - Input, in normalized coordinates only

    /// `at` is normalized: `(0, 0)` is the top-left of the streamed surface and
    /// `(1, 1)` the bottom-right, whatever size the surface currently is.
    public func click(at point: CGPoint, button: PointerButton = .left) {
        guard Viewer.isNormalized(point) else { return }
        session.send([
            .pointer(phase: .down, button: button, x: point.x, y: point.y, modifiers: []),
            .pointer(phase: .up, button: button, x: point.x, y: point.y, modifiers: []),
        ])
    }

    public func move(to point: CGPoint) {
        guard Viewer.isNormalized(point) else { return }
        session.send([.pointer(phase: .move, button: nil, x: point.x, y: point.y, modifiers: [])])
    }

    public func scroll(at point: CGPoint, by delta: CGSize, precise: Bool = true) {
        guard Viewer.isNormalized(point) else { return }
        session.send([.scroll(x: point.x, y: point.y,
                              deltaX: delta.width, deltaY: delta.height,
                              phase: .changed, momentum: .none, precise: precise)])
    }

    public func type(_ text: String) {
        guard !text.isEmpty else { return }
        session.send([.textCommit(text)])
    }

    public func press(_ key: String, holding modifiers: [InputModifier] = []) {
        guard !key.isEmpty else { return }
        session.send([
            .key(key: key, down: true, modifiers: modifiers, repeatKey: false),
            .key(key: key, down: false, modifiers: modifiers, repeatKey: false),
        ])
    }

    /// Out-of-range coordinates are **dropped, not clamped**: the server
    /// rejects a whole batch containing one invalid event, and a clamped click
    /// lands somewhere the caller did not ask for.
    static func isNormalized(_ point: CGPoint) -> Bool {
        point.x.isFinite && point.y.isFinite
            && (0...1).contains(point.x) && (0...1).contains(point.y)
    }
}

/// The Space's screen.
@MainActor
public struct Screen {
    public let space: Space

    public init(space: Space) { self.space = space }

    /// Every window that can be watched.
    public func windows() async throws -> [SpaceWindow] {
        try await space.windows()
    }

    /// Only the windows worth offering a person: no watcher terminals, no
    /// consent dialogs, no sixty identical `tail -f` windows.
    public func presentableWindows() async throws -> [SpaceWindow] {
        try await windows().filter { window in
            guard window.visible, !window.app.isEmpty else { return false }
            let title = window.title.lowercased()
            if title.contains("watch.command") || title.contains("tail -f") { return false }
            if window.app == "RCDP Host" { return false }
            return true
        }
    }

    /// Start watching. The returned `Viewer` is the only object a caller needs.
    @discardableResult
    public func watch(_ target: WatchTarget = .desktop) async throws -> Viewer {
        let session = LiveStreamSession(space: space)
        let viewer = Viewer(session: session)
        await session.select(try await Screen.source(for: target, in: space, session: session))
        return viewer
    }

    /// Watch the window an agent is working in.
    @discardableResult
    public func watch(agent: RunID) async throws -> Viewer {
        try await watch(.agent(agent))
    }

    /// Scoped: the viewer is stopped on every exit path. A stream nobody is
    /// looking at is still a stream, and still a decode loop.
    @discardableResult
    public func watching<T>(_ target: WatchTarget = .desktop,
                            _ body: (Viewer) async throws -> T) async throws -> T {
        let viewer = try await watch(target)
        do {
            let value = try await body(viewer)
            await viewer.stop()
            return value
        } catch {
            await viewer.stop()
            throw error
        }
    }

    static func source(for target: WatchTarget, in space: Space,
                       session: LiveStreamSession) async throws -> StreamSource {
        switch target {
        case .desktop:
            return .desktop
        case let .window(id):
            await session.refreshWindows()
            if let match = session.windows.first(where: { $0.id == id.rawValue }) {
                return .window(match)
            }
            throw SpacesError.malformedResponse(
                tool: "list_space_windows", detail: "no window \(id) in this Space")
        case let .agent(run):
            guard let window = try await space.window(for: run) else {
                throw SpacesError.malformedResponse(
                    tool: "list_space_windows",
                    detail: "\(run) could not be joined to a window by pid")
            }
            return try await source(for: .window(window.id), in: space, session: session)
        }
    }
}

extension Space {
    /// The Space's screen: frames, input, and the windows worth showing.
    @MainActor
    public var screen: Screen { Screen(space: self) }
}
