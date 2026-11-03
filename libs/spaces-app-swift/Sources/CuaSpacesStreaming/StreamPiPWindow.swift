// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

#if canImport(AppKit)
import AppKit
import Combine
import SwiftUI

/// The picture-in-picture pop-out.
///
/// A floating always-on-top panel carrying the **same** `LiveStreamSession` as
/// the in-app view. Popping out does not open a second RCDP session, does not
/// reconnect, and does not reset the decoder: the controller only moves which
/// view is hosting a session that stays live throughout. Popping back in is the
/// same move in reverse.
///
/// This also sidesteps the failure this stack has seen twice — a second stream
/// window silently evicting the first. One session, two observers, no second
/// claim on the target.
///
/// A panel can also own a session of its own (``popOut(provider:source:interactive:)``):
/// a single Space window, streamed on its own target, stopped when the panel
/// closes. ``StreamPiPSet`` keeps one panel per source.
@MainActor
public final class StreamPiPController: NSObject, ObservableObject, NSWindowDelegate {
    @Published public private(set) var isOpen = false

    /// What the panel shows. `.desktop` for the shared-session pop-out.
    public private(set) var source: StreamSource = .desktop

    /// Whether the panel owns its session (``popOut(provider:source:interactive:)``)
    /// rather than borrowing the app's.
    public private(set) var ownsSession = false

    private var panel: NSPanel?
    private weak var sharedSession: LiveStreamSession?
    private var ownedSession: LiveStreamSession?
    private var sizeWatch: AnyCancellable?

    /// The session on screen in the panel, shared or owned.
    public var session: LiveStreamSession? { ownedSession ?? sharedSession }

    /// Pop the stream out into a floating panel.
    public func popOut(session: LiveStreamSession, interactive: Bool = true) {
        if let panel {
            if Self.presentsPanels { panel.makeKeyAndOrderFront(nil) }
            isOpen = true
            session.isPoppedOut = true
            return
        }
        self.sharedSession = session
        self.source = session.source
        self.ownsSession = false
        present(session, title: session.title.isEmpty ? "Space" : session.title, interactive: interactive)
    }

    /// Pop out one source on a session of its own: a single window of the
    /// Space (or its desktop) streamed independently of the app's session.
    /// The session starts now and stops when the panel closes.
    public func popOut(provider: SpaceStreamSourceProviding, source: StreamSource,
                       interactive: Bool = true) {
        if let panel {
            if Self.presentsPanels { panel.makeKeyAndOrderFront(nil) }
            isOpen = true
            return
        }
        let session = LiveStreamSession(provider: provider)
        self.ownedSession = session
        self.source = source
        self.ownsSession = true
        present(session, title: source.pipTitle, interactive: interactive)
        Task { await session.select(source) }
    }

    private func present(_ session: LiveStreamSession, title: String, interactive: Bool) {
        let aspect = Self.aspect(of: session.surfaceSize)
        let width: CGFloat = Self.defaultWidth
        let contentRect = NSRect(x: 0, y: 0, width: width, height: (width / aspect).rounded())

        let panel = NSPanel(contentRect: contentRect,
                            styleMask: [.titled, .closable, .resizable, .utilityWindow, .nonactivatingPanel],
                            backing: .buffered,
                            defer: false)
        panel.title = title
        panel.isFloatingPanel = true
        // Always on top, and present on top of full-screen apps too — a PiP that
        // disappears behind the thing you are watching is not a PiP.
        panel.level = .floating
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        panel.hidesOnDeactivate = false
        panel.isMovableByWindowBackground = true
        panel.backgroundColor = .black
        panel.isReleasedWhenClosed = false
        panel.delegate = self
        // Keep the panel's shape locked to the stream so the letterbox stays
        // empty and the click mapping keeps a 1:1 aspect.
        panel.contentAspectRatio = NSSize(width: aspect, height: 1)

        let host = NSHostingView(rootView: PiPStreamContent(session: session, isInteractive: interactive))
        host.autoresizingMask = [.width, .height]
        panel.contentView = host
        panel.center()
        if Self.presentsPanels { panel.makeKeyAndOrderFront(nil) }

        // A window's size is only known once its first frame decodes: follow
        // it, so a tall window gets a tall panel.
        sizeWatch = session.$surfaceSize
            .removeDuplicates()
            .sink { [weak panel] size in
                guard let panel, size.width > 0, size.height > 0 else { return }
                let a = Self.aspect(of: size)
                panel.contentAspectRatio = NSSize(width: a, height: 1)
                let w = panel.contentLayoutRect.width
                panel.setContentSize(NSSize(width: w, height: (w / a).rounded()))
            }

        self.panel = panel
        isOpen = true
        session.isPoppedOut = true
    }

    /// Pop back in. A shared session keeps streaming; an owned one stops.
    public func popIn() {
        let panel = self.panel
        self.panel = nil
        panel?.delegate = nil
        panel?.orderOut(nil)
        panel?.contentView = nil
        panel?.close()
        finish()
    }

    public func toggle(session: LiveStreamSession) {
        if isOpen { popIn() } else { popOut(session: session) }
    }

    public func windowWillClose(_ notification: Notification) {
        // Closing the panel is "pop back in", never "stop the app's stream".
        panel = nil
        finish()
    }

    private func finish() {
        sizeWatch = nil
        isOpen = false
        sharedSession?.isPoppedOut = false
        sharedSession = nil
        if let owned = ownedSession {
            ownedSession = nil
            owned.isPoppedOut = false
            Task { await owned.stop() }
        }
    }

    /// The panel's window, for window-only captures and tests.
    public var window: NSWindow? { panel }

    static let defaultWidth: CGFloat = 480
    /// Off in tests: build the panel, never put it on screen.
    static var presentsPanels = true

    static func aspect(of size: CGSize) -> CGFloat {
        size.width > 0 && size.height > 0 ? size.width / size.height : 16.0 / 10.0
    }
}

/// Several pop-outs at once, one per source: the desktop and any number of
/// single windows, each in its own floating panel.
///
/// The desktop can borrow the app's session (``toggle(_:sharing:)``), so
/// popping it out does not open a second stream; every window gets a session
/// of its own that stops when its panel closes.
@MainActor
public final class StreamPiPSet: ObservableObject {
    /// Keys (``StreamSource/pipKey``) of the panels that are open.
    @Published public private(set) var openKeys: Set<String> = []

    private let provider: SpaceStreamSourceProviding
    private var controllers: [String: StreamPiPController] = [:]
    private var watches: [String: AnyCancellable] = [:]

    public init(provider: SpaceStreamSourceProviding) {
        self.provider = provider
    }

    public func isOpen(_ source: StreamSource) -> Bool { openKeys.contains(source.pipKey) }

    /// The controller for an open source.
    public func controller(for source: StreamSource) -> StreamPiPController? {
        controllers[source.pipKey]
    }

    /// Pop `source` out. With `sharing`, the panel shows that session (the
    /// app's own desktop stream) instead of opening another.
    public func popOut(_ source: StreamSource, sharing session: LiveStreamSession? = nil,
                       interactive: Bool = true) {
        let key = source.pipKey
        let controller = controllers[key] ?? StreamPiPController()
        controllers[key] = controller
        watches[key] = controller.$isOpen.dropFirst().sink { [weak self] open in
            guard let self, !open else { return }
            self.openKeys.remove(key)
            self.controllers[key] = nil
            self.watches[key] = nil
        }
        if let session {
            controller.popOut(session: session, interactive: interactive)
        } else {
            controller.popOut(provider: provider, source: source, interactive: interactive)
        }
        openKeys.insert(key)
    }

    public func popIn(_ source: StreamSource) {
        controllers[source.pipKey]?.popIn()
    }

    public func toggle(_ source: StreamSource, sharing session: LiveStreamSession? = nil) {
        if isOpen(source) { popIn(source) } else { popOut(source, sharing: session) }
    }

    /// Close every panel (app shutdown).
    public func popInAll() {
        for c in Array(controllers.values) { c.popIn() }
    }
}

public extension StreamSource {
    /// One panel per desktop or window handle, whatever its epoch or size.
    var pipKey: String {
        switch self {
        case .desktop: return "desktop"
        case let .window(w): return "window:\(w.id)"
        }
    }

    /// The pop-out's title: the window's name, or "Desktop".
    var pipTitle: String {
        switch self {
        case .desktop: return "Desktop"
        case let .window(w): return w.title.isEmpty ? w.app : w.title
        }
    }
}

/// The panel's contents: the live stream plus a minimal chrome that does not
/// steal space from the picture.
private struct PiPStreamContent: View {
    @ObservedObject public var session: LiveStreamSession
    public var isInteractive: Bool

    public var body: some View {
        ZStack(alignment: .topLeading) {
            LiveStreamView(session: session, isInteractive: isInteractive,
                           showsCursorOverlay: session.presenceView == nil)
            PresenceOverlay(session: session)
            if !session.status.isLive {
                StreamStatusBadge(status: session.status)
                    .padding(8)
            }
        }
        .background(Color.black)
    }
}

/// Small status chip used by both the in-app view and the PiP.
public struct StreamStatusBadge: View {
    public var status: LiveStreamSession.Status

    public var body: some View {
        HStack(spacing: 6) {
            Circle().fill(color).frame(width: 7, height: 7)
            Text(text).font(.system(size: 11, weight: .medium))
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 4)
        .background(.black.opacity(0.65), in: Capsule())
        .foregroundStyle(.white)
    }

    private var color: Color {
        switch status {
        case .streaming: return .green
        case .connecting: return .yellow
        case .suspended: return .orange
        case .failed: return .red
        case .idle: return .gray
        }
    }

    private var text: String {
        switch status {
        case .idle: return "Idle"
        case .connecting: return "Connecting…"
        case .streaming: return "Live"
        case let .suspended(reason): return "Paused: \(reason)"
        case let .failed(reason): return "Failed: \(reason)"
        }
    }
}
#endif
