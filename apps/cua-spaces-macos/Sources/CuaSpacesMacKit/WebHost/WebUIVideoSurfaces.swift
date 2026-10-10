// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import Combine
import CuaSpacesStreaming
import WebKit

/// Native live video for the web UI's video slots: the Space tiles on the
/// Spaces grid and the Space viewer (docs: apps/cua-spaces-macos/docs/native-video.md).
///
/// The page reports each slot's rect, visible part and whether page UI
/// covers it on the `cuaVideo` message handler (one `surfaces` message per
/// frame, only what changed). This places a `LiveStreamInputView` (the
/// view, VideoToolbox decode and input mapping the native Space window
/// uses) over the web view at that rect, inside a clip container at the
/// visible part. One `LiveStreamSession` per Space and tier feeds every
/// slot showing it.
///
/// - Tiles open at a low frame rate and size (`VideoTier.tile`), take no
///   input and let clicks and scrolls through to the page.
/// - The viewer opens at the Space's full rate and takes pointer, scroll
///   and keys like the native window (`allow_activation` on the desktop).
///   A click gives it the keyboard; Control+Option pressed and released
///   alone (the view's `KeyCapture`), a click on the page, or the page's
///   "Stop controlling" gives it back. Scrolls go to the page until
///   it has the keyboard. The system pointer always stays as it is.
/// - A slot shows nothing until its first frame; the page shows
///   "Connecting…" under it. A stream that can't open or fails tells the
///   page (`video.surface` `failed`), which shows its fallback (Open window
///   on the viewer, the drawn thumbnail on a tile).
/// - Nothing decodes while the window is fully hidden (minimized, covered,
///   another Space): the sessions stop and open again when it shows.
///
/// On whenever the New UI is used, as the Space detail's inline desktop
/// needs it. Opt out with `CUA_WEBUI_NATIVE_VIDEO=0` (one run) or the
/// `WebUINativeVideo` default set to false; off, the handler is not
/// registered and the page shows its fallback (Open window).
@MainActor
final class WebUIVideoSurfaces: NSObject, WKScriptMessageHandler {
    static let name = "cuaVideo"
    static let defaultsKey = "WebUINativeVideo"

    static var enabled: Bool {
        enabled(environment: ProcessInfo.processInfo.environment, defaults: .standard)
    }

    static func enabled(environment: [String: String], defaults: UserDefaults) -> Bool {
        switch environment["CUA_WEBUI_NATIVE_VIDEO"] {
        case "0": return false
        case "1": return true
        default: return defaults.object(forKey: defaultsKey) as? Bool ?? true
        }
    }

    /// Opens a Space's stream for a tier through the app's existing path.
    typealias Opener = (_ spaceId: String, _ tier: VideoTier) async throws -> SpaceStreamSourceProviding

    private weak var webView: WKWebView?
    private let open: Opener
    /// Tells the page something changed (`cua:event`).
    var emit: ((_ event: String, _ payload: [String: Any]) -> Void)?
    /// The viewer's presence name (the native window's), or nil.
    var presenceName: (@MainActor () -> String?)?
    /// Now, on the clock frames' arrival is stamped with
    /// (`ProcessInfo.systemUptime`, s): when a frame reaches the layer, for
    /// the video bench. Tests fix it.
    var now: () -> TimeInterval = { ProcessInfo.processInfo.systemUptime }

    private var streams: [StreamKey: Stream] = [:]
    private(set) var surfaces: [String: Surface] = [:]
    private(set) var focusedId: String?
    private var keyMonitor: Any?
    private var responderObservation: NSKeyValueObservation?
    private var occlusionObserver: NSObjectProtocol?
    private weak var observedWindow: NSWindow?
    /// The window is fully hidden: sessions are stopped until it shows.
    private(set) var paused = false

    struct StreamKey: Hashable {
        let spaceId: String
        let tier: VideoTier
    }

    /// One session per Space and tier, shared by its surfaces.
    final class Stream {
        var session: LiveStreamSession?
        var users = 0
        var opening: Task<Void, Never>?
        var failure: String?
        var status: AnyCancellable?
        /// Arrival to presented per presented frame, ms, newest last (the video bench).
        var presentedMs: [Double] = []
    }

    /// The most arrival-to-presented samples kept per stream (two minutes at 60 fps).
    static let latencySamples = 7200

    /// Adds one sample, keeping the newest `latencySamples`.
    static func appendSample(_ samples: inout [Double], _ ms: Double) {
        samples.append(ms)
        if samples.count > latencySamples { samples.removeFirst(samples.count - latencySamples) }
    }

    final class Surface {
        let id: String
        let key: StreamKey
        let view = VideoSurfaceView()
        var update: VideoSurfaceUpdate
        var layout = VideoSurfaceLayout.none
        var phase = VideoSurfacePhase.connecting
        var frames: AnyCancellable?
        var bound = false
        init(_ update: VideoSurfaceUpdate) {
            id = update.surfaceId
            key = StreamKey(spaceId: update.spaceId, tier: update.tier)
            self.update = update
        }
    }

    init(webView: WKWebView, open: @escaping Opener) {
        self.webView = webView
        self.open = open
    }

    // MARK: - Messages

    func userContentController(_ controller: WKUserContentController, didReceive message: WKScriptMessage) {
        handle(message.body)
    }

    func handle(_ body: Any) {
        guard let message = VideoSurfaceMessage(body: body) else { return }
        switch message {
        case let .surfaces(update, remove):
            CATransaction.begin()
            CATransaction.setDisableActions(true)
            for id in remove { self.remove(id) }
            for u in update { place(u) }
            CATransaction.commit()
        case let .focus(id):
            // The page moved the keyboard: no viewer takes it back by itself.
            userMovedKeyboard()
            focus(id)
        }
    }

    /// Tears every surface and session down (window closed, page replaced).
    func removeAll() {
        for id in Array(surfaces.keys) { remove(id) }
    }

    // MARK: - Surfaces

    private func place(_ u: VideoSurfaceUpdate) {
        guard let webView else { return }
        if let existing = surfaces[u.surfaceId], existing.key != StreamKey(spaceId: u.spaceId, tier: u.tier) {
            remove(u.surfaceId)
        }
        let surface = surfaces[u.surfaceId] ?? attach(u, in: webView)
        surface.update = u
        surface.layout = VideoSurfaceLayout.make(u, zoom: webView.pageZoom * webView.magnification,
                                                 viewHeight: webView.bounds.height, flipped: webView.isFlipped)
        surface.view.interactive = u.interactive
        surface.view.input.isInteractive = u.interactive
        surface.view.input.showsCursorOverlay = u.interactive
        surface.view.apply(surface.layout)
        show(surface)
        // Keys never stay with a viewer that page UI covers or that left the screen.
        if focusedId == surface.id, surface.view.isHidden { focus(nil) }
    }

    private func attach(_ u: VideoSurfaceUpdate, in webView: WKWebView) -> Surface {
        let surface = Surface(u)
        surface.view.isHidden = true
        surface.view.focusOwner = webView
        surface.view.input.keyboardHome = webView
        webView.addSubview(surface.view)
        surfaces[surface.id] = surface
        watchWindow(webView.window)
        let stream = streams[surface.key] ?? Stream()
        streams[surface.key] = stream
        stream.users += 1
        if let failure = stream.failure {
            setPhase(surface, .failed(failure, opening: true))
        } else if let session = stream.session {
            bind(surface, to: session)
        } else if stream.opening == nil {
            openStream(stream, key: surface.key)
        }
        return surface
    }

    private func openStream(_ stream: Stream, key: StreamKey) {
        stream.opening = Task { [weak self] in
            guard let self else { return }
            do {
                let provider = try await self.open(key.spaceId, key.tier)
                guard self.streams[key] === stream, stream.users > 0, !Task.isCancelled else { return }
                let session = LiveStreamSession(provider: provider)
                stream.session = session
                stream.status = session.$status.receive(on: DispatchQueue.main).sink { [weak self] status in
                    self?.statusChanged(key, status)
                }
                for s in self.surfaces.values where s.key == key { self.bind(s, to: session) }
                if key.tier == .full, let name = self.presenceName?(), !name.isEmpty {
                    await session.joinPresence(as: name)
                }
                if !self.paused { await session.start() }
                stream.opening = nil
            } catch {
                guard self.streams[key] === stream else { return }
                stream.opening = nil
                let reason = LiveSpacesBackend.words(error)
                stream.failure = reason
                NSLog("cuaVideo: could not open %@ (%@): %@", key.spaceId, key.tier.rawValue, reason)
                for s in self.surfaces.values where s.key == key { self.setPhase(s, .failed(reason, opening: true)) }
            }
        }
    }

    private func statusChanged(_ key: StreamKey, _ status: LiveStreamSession.Status) {
        guard case let .failed(reason) = status else { return }
        for s in surfaces.values where s.key == key { setPhase(s, .failed(reason)) }
    }

    private func bind(_ surface: Surface, to session: LiveStreamSession) {
        guard !surface.bound else { return }
        surface.bound = true
        let input = surface.view.input
        input.onInput = { [weak session] events in session?.send(events) }
        session.presentation.viewDidAttach()
        input.onDetach = { [weak session] in session?.presentation.viewDidDetach() }
        surface.frames = session.$frame.receive(on: DispatchQueue.main).sink { [weak self, weak session, weak surface] frame in
            guard let self, let session, let surface else { return }
            let input = surface.view.input
            input.surfaceSize = session.surfaceSize
            input.present(frame)
            session.presentation.viewDidPresent(pixels: input.layerHasPixels,
                                                contentSize: input.geometry.contentRect.size,
                                                interactive: input.isInteractive)
            if frame != nil, session.lastFrameReceivedAt > 0, let stream = self.streams[surface.key] {
                let ms = (self.now() - session.lastFrameReceivedAt) * 1000
                Self.appendSample(&stream.presentedMs, max(0, ms))
            }
            if frame != nil {
                self.setPhase(surface, .live)
            } else if case .failed = surface.phase {
                // Stays failed until the slot is placed again.
            } else {
                self.setPhase(surface, .connecting)
            }
        }
    }

    private func setPhase(_ surface: Surface, _ phase: VideoSurfacePhase) {
        guard surface.phase != phase else { return }
        let wasLive = surface.phase == .live
        // Dropping (a reconnect): remember whether this viewer had the keys.
        if wasLive, phase != .live {
            surface.view.input.keyboardReturn.dropped(hadKeyboard: focusedId == surface.id)
            watchClicksWhileDropped()
        }
        surface.phase = phase
        show(surface)
        if focusedId == surface.id, phase != .live { focus(nil) }
        emit?("video.surface", ["surfaceId": surface.id].merging(phase.payload) { a, _ in a })
        // Back: the keys it had come back, unless the page or the user took them.
        if phase == .live, surface.view.input.keyboardReturn.back(keyboardIsFree: surface.view.input.keyboardIsFree) {
            focus(surface.id)
        }
        watchClicksWhileDropped()
    }

    /// A click in this window while a viewer waits to take the keyboard
    /// back (the user chose where keys go: a field on the page, say) ends
    /// the wait.
    private var clickMonitor: Any?

    private func watchClicksWhileDropped() {
        let waiting = surfaces.values.contains { $0.view.input.keyboardReturn.waiting }
        if waiting, clickMonitor == nil {
            clickMonitor = NSEvent.addLocalMonitorForEvents(matching: [.leftMouseDown, .rightMouseDown]) { [weak self] event in
                if let self, event.window === self.webView?.window { self.userMovedKeyboard() }
                return event
            }
        } else if !waiting, let monitor = clickMonitor {
            NSEvent.removeMonitor(monitor)
            clickMonitor = nil
        }
    }

    /// The user or the page moved the keyboard: no viewer takes it back.
    func userMovedKeyboard() {
        for s in surfaces.values { s.view.input.keyboardReturn.forget() }
        watchClicksWhileDropped()
    }

    /// A surface shows once it has a frame and the page says it can be seen.
    private func show(_ surface: Surface) {
        surface.view.isHidden = surface.layout.hidden || surface.phase != .live
    }

    private func remove(_ surfaceId: String) {
        guard let surface = surfaces.removeValue(forKey: surfaceId) else { return }
        if focusedId == surfaceId { focus(nil) }
        surface.view.input.keyboardReturn.forget()
        watchClicksWhileDropped()
        surface.frames?.cancel()
        surface.view.input.onDetach?()
        surface.view.input.onDetach = nil
        surface.view.input.onInput = nil
        surface.view.removeFromSuperview()
        if surfaces.isEmpty { unwatchWindow() }
        guard let stream = streams[surface.key] else { return }
        stream.users -= 1
        guard stream.users <= 0 else { return }
        streams[surface.key] = nil
        stream.opening?.cancel()
        stream.status?.cancel()
        if let session = stream.session { Task { await session.stop() } }
    }

    /// The streams and their sessions (the harness's stats).
    func streamsSnapshot() -> [(StreamKey, Stream)] { streams.map { ($0.key, $0.value) } }

    func sessionFor(_ surface: Surface) -> LiveStreamSession? { streams[surface.key]?.session }

    // MARK: - Keyboard focus

    /// Gives the keyboard to a viewer (it must be live and on screen) or
    /// back to the page.
    func focus(_ surfaceId: String?) {
        guard let webView, let window = webView.window else { return }
        if let surfaceId, let surface = surfaces[surfaceId], surface.update.interactive, !surface.view.isHidden {
            window.makeFirstResponder(surface.view.input)
        } else if focusedId != nil || surfaceId == nil {
            if window.firstResponder !== webView { window.makeFirstResponder(webView) }
        }
        responderChanged()
    }

    private func responderChanged() {
        let responder = webView?.window?.firstResponder
        let now = surfaces.values.first { $0.view.input === responder }?.id
        guard now != focusedId else { return }
        focusedId = now
        emit?("video.focus", ["surfaceId": now.map { $0 as Any } ?? NSNull()])
    }

    /// Every ⌘ chord, ⌘Esc too, goes to the Space (the web view would take
    /// ⌘C, ⌘V first) while a viewer has the keyboard. Control+Option
    /// pressed and released alone gives it back: the view's own key monitor
    /// (`KeyCapture`) hands the keyboard to its home, the web view.
    func handleKey(_ event: NSEvent) -> Bool {
        guard let id = focusedId, let surface = surfaces[id],
              event.window.map({ $0 === webView?.window }) ?? true else { return false }
        let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        if flags.contains(.command) { return surface.view.input.performKeyEquivalent(with: event) }
        return false
    }

    private func watchWindow(_ window: NSWindow?) {
        guard let window, observedWindow !== window else { return }
        unwatchWindow()
        observedWindow = window
        responderObservation = window.observe(\.firstResponder, options: [.new]) { [weak self] _, _ in
            MainActor.assumeIsolated { self?.responderChanged() }
        }
        keyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            MainActor.assumeIsolated { (self?.handleKey(event) ?? false) ? nil : event }
        }
        occlusionObserver = NotificationCenter.default.addObserver(
            forName: NSWindow.didChangeOcclusionStateNotification, object: window, queue: .main
        ) { [weak self, weak window] _ in
            MainActor.assumeIsolated {
                guard let window else { return }
                self?.setPaused(!window.occlusionState.contains(.visible))
            }
        }
    }

    private func unwatchWindow() {
        responderObservation?.invalidate()
        responderObservation = nil
        if let keyMonitor { NSEvent.removeMonitor(keyMonitor) }
        keyMonitor = nil
        if let occlusionObserver { NotificationCenter.default.removeObserver(occlusionObserver) }
        occlusionObserver = nil
        observedWindow = nil
    }

    /// Stops every session while the window can't be seen; opens them again when it can.
    func setPaused(_ hidden: Bool) {
        guard hidden != paused else { return }
        paused = hidden
        for stream in streams.values {
            guard let session = stream.session else { continue }
            Task { hidden ? await session.stop() : await session.start() }
        }
    }
}

/// A slot's native views: a clip container at the slot's visible part over
/// the web view, holding the stream view at the slot's full rect.
///
/// A tile (not interactive) is transparent to the mouse: clicks, hovers and
/// scrolls reach the page under it. The viewer takes pointer input; its
/// scrolls go to the page until it has the keyboard.
final class VideoSurfaceView: NSView {
    let input = LiveStreamInputView()
    var interactive = false
    /// Where scrolls go while the viewer doesn't have the keyboard.
    weak var focusOwner: NSView?

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        wantsLayer = true
        layer?.masksToBounds = true
        input.wantsLayer = true
        input.layer?.masksToBounds = true
        addSubview(input)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is unavailable") }

    override var isFlipped: Bool { true }

    func apply(_ layout: VideoSurfaceLayout) {
        frame = layout.container
        input.frame = layout.video
        input.layer?.cornerRadius = layout.radius
    }

    override func hitTest(_ point: NSPoint) -> NSView? {
        guard interactive, !isHidden else { return nil }
        if NSApp.currentEvent?.type == .scrollWheel, window?.firstResponder !== input { return nil }
        return super.hitTest(point)
    }
}

extension WebUIVideoSurfaces {
    /// `CUA_SPACES_SYNTHETIC_VIDEO=<dir>` (debug builds): every Space streams
    /// the harness's synthetic H.264 from `<dir>` (`tile30.h264`,
    /// `full60.h264`), with native video on whatever the experiment says.
    static var syntheticDirectory: URL? {
        guard let dir = DevHooks.value("CUA_SPACES_SYNTHETIC_VIDEO"), !dir.isEmpty else { return nil }
        return URL(fileURLWithPath: dir, isDirectory: true)
    }

    /// The web UI window's video surfaces: nil unless the experiment is on
    /// (or the synthetic harness runs).
    static func make(webView: WKWebView, model: AppModel) -> WebUIVideoSurfaces? {
        if let dir = syntheticDirectory {
            let tile = DevHooks.value("CUA_SPACES_SYNTHETIC_TILE") ?? "tile30"
            return WebUIVideoSurfaces(webView: webView) { [weak model] id, tier in
                let os = await MainActor.run { model?.spaces.first { $0.id == id }.map { "\($0.os)" } } ?? ""
                return try SyntheticStreamProvider.harness(dir: dir, tier: tier, offset: id.utf8.reduce(0) { $0 + Int($1) },
                                                           tile: tile, guestOS: os)
            }
        }
        guard enabled else { return nil }
        let backend = model.backend
        let surfaces = WebUIVideoSurfaces(webView: webView) { id, tier in
            let provider = try await backend.streamProvider(id: id)
            if let space = provider as? SpaceStreamProvider {
                space.maxFPS = tier.maxFPS
                space.maxDimension = tier.maxDimension
            }
            return provider
        }
        surfaces.presenceName = { [weak model] in model?.presenceName }
        return surfaces
    }
}
