// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Combine
import CoreGraphics
import CoreVideo
import Cua
import CuaSpaces
import Foundation

/// One live view of a Space, owning the transport for whichever source is
/// selected and publishing decoded frames.
///
/// The session is deliberately **not** owned by a view. Both the in-app view and
/// the PiP pop-out observe the same `LiveStreamSession`, so popping out and
/// back in is a change of which view is mounted, not a reconnect: the media
/// session, the decoder and the input sequence all survive it. That is the
/// only way to pop out without dropping frames or restarting the H.264 GOP.
///
/// The transport is the cua SDK's `SpaceStreamSession` (rcdp wire v2, ticketed,
/// keyframe-gated per codec epoch in Rust): it hands this class encoded access
/// units on its delivery thread, and this class decodes and publishes them.
@MainActor
public final class LiveStreamSession: ObservableObject {
    public enum Status: Equatable {
        case idle
        case connecting
        case streaming
        case suspended(String)
        case failed(String)

        var isLive: Bool { self == .streaming }
    }

    // MARK: - Published state

    @Published public private(set) var frame: CVPixelBuffer?
    /// Size of the surface currently being decoded. Authoritative for the
    /// coordinate mapping — see `StreamGeometry`.
    @Published public private(set) var surfaceSize: CGSize = .zero
    @Published public private(set) var status: Status = .idle
    @Published public private(set) var windows: [StreamWindow] = []
    @Published public private(set) var source: StreamSource = .desktop
    @Published public private(set) var title: String = ""
    /// True while the PiP window holds this session.
    @Published public var isPoppedOut = false

    // MARK: - Presence

    /// Everyone else on this Space's daemon, with their last known cursor —
    /// other viewers, the person at the physical machine (`isHost`) and the CUA
    /// agent (`isAgent`).
    ///
    /// This is the piece that was missing: the streaming layer carried a single
    /// local cursor overlay, so an SDK consumer could not build the shipped
    /// multi-participant primitive. Presence now rides the Space's
    /// `PresenceService` through the SDK (`SpacePresence`).
    /// The SDK keeps the model; **drawing is still the app's**, which is why
    /// this is data and not a view.
    @Published public private(set) var participants: [Participant] = []
    /// This client's own identity, as the daemon assigned it on `join`.
    @Published public private(set) var localParticipant: Participant?
    /// The display name this session joined under, if it has joined.
    @Published public private(set) var presenceName: String?
    /// The SDK's presence model (interpolated cursors, shapes, idle fade,
    /// staleness), fed every event of this session. `PresenceOverlay` draws
    /// it. Nil until presence is joined.
    @Published public private(set) var presenceView: PresenceView?
    /// The window presence cursors are reported against, nil for the desktop.
    public var presenceWindowID: String? { currentTargetHandle?.value }

    /// Every roster and cursor change, for a consumer that wants the events
    /// rather than the snapshot.
    public let presenceEvents = PassthroughSubject<PresenceEvent, Never>()

    // MARK: - Evidence counters

    @Published public private(set) var decodedFrameCount = 0
    @Published public private(set) var droppedBeforeKeyframeCount = 0
    @Published public private(set) var decodeFailureCount = 0
    @Published public private(set) var inputEventsSent = 0
    @Published public private(set) var inputEventsAcknowledged: UInt64 = 0
    /// Why the Space refused the last input batch (`nil` once one is
    /// delivered). Input failures used to be silent: the acknowledgement's
    /// name did not match, so a viewer whose every click was refused looked
    /// exactly like one whose clicks landed.
    @Published public private(set) var inputFailure: String?
    /// True once the Space refused background input to the streamed window
    /// (`would_require_activation`) and the session was reopened with
    /// `allow_activation`: input now activates that window on the Space, as
    /// it does on the desktop stream. Reset by a source switch or a stop.
    @Published public private(set) var activatesOnInput = false
    @Published public private(set) var lastFrameDimensions: CGSize = .zero

    // MARK: - Internals

    private let provider: SpaceStreamSourceProviding
    private var media: SpaceStreamSession?
    private var relay: FrameRelay?
    private var decoder: H264Decoder?
    private var sessionID: SessionID?
    /// The window presence cursors are reported against. `nil` for the desktop
    /// source, which is what the protocol wants for a screen-wide cursor.
    private var currentTargetHandle: TargetHandle?
    private var presence: SpacePresence?
    private var presenceTask: Task<Void, Never>?
    private var presenceJoining = false
    private var roster = PresenceRoster()
    private var nextInputSequence: UInt64 = 1
    /// Recent batches by first sequence, so the ones the Space refused as
    /// would-require-activation are resent after the activation reopen.
    private var sentBatches: [(first: UInt64, events: [InteractiveInputEvent])] = []
    private static let resendableBatches = 32
    private var lastKeyframeRequest = Date.distantPast
    /// Bumped by every open, stop and source switch. An `open` that finds it
    /// changed after its `await` was superseded: it closes what it opened and
    /// touches nothing else, and its decoder's frames are dropped. Without
    /// this, a stop (or a second start) racing an open in flight left two
    /// transports running, and the stale one's frames and status won.
    private var generation: UInt64 = 0
    /// The open in flight, so a second `start()` joins it instead of opening
    /// a second transport on the same Space.
    private var opening: Task<Void, Never>?

    /// Whether this session's frames are actually on screen, and how many of
    /// them have been presented rather than merely decoded.
    ///
    /// `FRICTION.md` §18 — a session decodes perfectly well into a view that
    /// was never mounted, and every counter above this one would say the
    /// stream was "working" in that case. This one would not.
    public let presentation = StreamPresentation()

    public init(provider: SpaceStreamSourceProviding) {
        self.provider = provider
    }

    /// Stream a Space directly. This is the call `FRICTION.md` §10 says was
    /// missing: "hand me its frames", separate from the display tools that
    /// draw on the operator's own machine.
    ///
    /// It also removes the three-layer routing §10 records — the tiers
    /// depended on a screen source, which carried a session, which was built
    /// against a two-method protocol the Spaces client did not implement.
    /// Here the Space *is* the provider.
    public convenience init(space: CuaSpaces.Space) {
        self.init(provider: SpaceStreamProvider(space: space))
    }

    // MARK: - Lifecycle

    public func refreshWindows() async {
        do {
            windows = try await provider.availableWindows()
        } catch {
            status = .failed("could not list windows: \(error)")
        }
    }

    /// Switch sources without disturbing anything the new source does not need.
    public func select(_ newSource: StreamSource) async {
        guard newSource != source || status == .idle else { return }
        await teardownTransport()
        if newSource != source { activatesOnInput = false }
        source = newSource
        title = newSource.label
        await begin()
    }

    /// Opens the current source. A start while one is in flight joins it;
    /// a start while streaming does nothing.
    public func start() async {
        if let opening {
            await opening.value
            return
        }
        // A transport already open (streaming or suspended) stays.
        guard media == nil else { return }
        await begin()
    }

    public func stop() async {
        await teardownTransport()
        activatesOnInput = false
        status = .idle
        frame = nil
        decodedFrameCount = 0
        droppedBeforeKeyframeCount = 0
        decodeFailureCount = 0
    }

    /// A fresh open of the current source, superseding any in flight.
    private func begin() async {
        generation &+= 1
        let generation = generation
        status = .connecting
        frame = nil
        let source = source
        let task = Task { @MainActor [weak self] in
            guard let self else { return }
            await self.open(source, generation: generation)
        }
        opening = task
        await task.value
        if self.generation == generation { opening = nil }
    }

    private func teardownTransport() async {
        // Anything opening now is superseded; it closes its own transport.
        generation &+= 1
        opening = nil
        presenceTask?.cancel()
        presenceTask = nil
        if let presence { try? await presence.leave() }
        presence = nil
        presenceView = nil
        roster = PresenceRoster()
        relay?.detach()
        relay = nil
        if let media { _ = try? await media.close() }
        media = nil
        sessionID = nil
        currentTargetHandle = nil
        participants = []
        localParticipant = nil
        nextInputSequence = 1
        sentBatches = []
        inputFailure = nil
        decoder?.reset()
        decoder = nil
    }

    // MARK: - Source

    private func open(_ source: StreamSource, generation: UInt64) async {
        let decoder = H264Decoder()
        // Frames and events of a superseded open never reach this session.
        decoder.onFrame = { [weak self] buffer, descriptor in
            // A decoded buffer is never written again: handing it to the
            // main actor is safe, which CoreVideo cannot declare.
            let frame = DecodedFrame(buffer: buffer)
            Task { @MainActor [weak self] in
                guard let self, self.generation == generation else { return }
                self.ingest(frame.buffer, descriptor: descriptor)
            }
        }
        decoder.onNeedsKeyframe = { [weak self] in
            Task { @MainActor [weak self] in
                guard let self, self.generation == generation else { return }
                self.requestKeyframeThrottled()
            }
        }
        let relay = FrameRelay(decoder: decoder) { [weak self] event in
            Task { @MainActor [weak self] in
                guard let self, self.generation == generation else { return }
                self.handle(event)
            }
        }
        do {
            // Join presence before the stream opens: the SDK names this
            // viewer's participant when it opens the media session, so the
            // Space attributes this viewer's input to its own presence cursor
            // (never an agent's) and draws no agent cursor for it.
            if let presenceName, presence == nil { await startPresence(presenceName, color: nil) }
            guard self.generation == generation else { return }
            let policy = activatesOnInput ? "allow_activation" : SpaceStreamProvider.inputPolicy(for: source)
            let media = try await provider.openSession(source, policy: policy, frames: relay, audio: nil)
            guard self.generation == generation else {
                // Stopped, restarted or switched while this was opening.
                relay.detach()
                _ = try? await media.close()
                return
            }
            self.decoder = decoder
            self.relay = relay
            self.media = media
            let id = SessionID(media.mediaSessionId())
            sessionID = id
            relay.setSession(id)
            if case let .window(window) = source {
                currentTargetHandle = TargetHandle(window.id)
                if window.surfaceSize != .zero { surfaceSize = window.surfaceSize }
            } else {
                title = "Full desktop"
            }
            guard media.codec() == "h264" else {
                status = .failed("the server selected \(media.codec()); this client decodes h264")
                return
            }
            status = .streaming
            // A session joined mid-GOP sees only P-frames until the next
            // natural keyframe. The SDK gates on a keyframe; ask for one now.
            try? media.requestKeyframe()
            if let presenceName { await joinPresence(as: presenceName) }
        } catch {
            relay.detach()
            guard self.generation == generation else { return }
            status = .failed("\(error)")
        }
    }

    private func handle(_ event: MediaEvent) {
        let payload = (try? JSONSerialization.jsonObject(with: Data(event.json.utf8)))
            .flatMap { $0 as? [String: Any] } ?? [:]
        let body = payload["payload"] as? [String: Any] ?? payload
        switch event.kind {
        case "lifecycle":
            let kind = body["kind"] as? String ?? body["event"] as? String ?? ""
            if let g = body["geometry"] as? [String: Any],
               let w = (g["width_px"] as? NSNumber)?.doubleValue,
               let h = (g["height_px"] as? NSNumber)?.doubleValue {
                // Authoritative for the coordinate space — see `StreamGeometry`.
                surfaceSize = CGSize(width: w, height: h)
                decoder?.reset()
                requestKeyframeThrottled(force: true)
            }
            if let newTitle = body["title"] as? String, !newTitle.isEmpty { title = newTitle }
            if kind.contains("suspend") { status = .suspended(body["reason"] as? String ?? kind) }
            if kind.contains("resume") { status = .streaming }
            if kind.contains("close") { status = .failed("the streamed target closed") }
        case "interactive_input_acknowledgement", "interactive_input_ack", "input_ack":
            if let through = (body["through_sequence"] as? NSNumber)?.uint64Value {
                inputEventsAcknowledged = through
            }
            // The video keeps streaming whatever happened to the input, so
            // this is not a stream status: it is its own, shown over it.
            if body["delivered"] as? Bool == false {
                let error = body["error"] as? [String: Any]
                if error?["code"] as? String == "would_require_activation",
                   case .window = source, !activatesOnInput {
                    let through = (body["through_sequence"] as? NSNumber)?.uint64Value ?? 0
                    reopenActivating(resending: through)
                    return
                }
                inputFailure = (error?["message"] as? String).flatMap { $0.isEmpty ? nil : $0 }
                    ?? "input was not delivered"
            } else {
                inputFailure = nil
            }
        case "error":
            status = .failed("stream \(body["code"] ?? "error"): \(body["message"] ?? "")")
        case "closed":
            if status == .streaming || status == .connecting {
                status = .failed("connection closed \(body["reason"] ?? "")")
            }
        default:
            break
        }
    }

    // MARK: - Presence

    /// Announce this client as a participant, so the cursor it publishes is
    /// attributed rather than dropped. Needs the Space's `presence` feature.
    public func joinPresence(as name: String, color: String? = nil) async {
        presenceName = name
        guard presence == nil, media != nil else { return }
        await startPresence(name, color: color)
    }

    private func startPresence(_ name: String, color: String?) async {
        // One join at a time: the stream's open and the view may both ask.
        guard presence == nil, !presenceJoining else { return }
        presenceJoining = true
        defer { presenceJoining = false }
        do {
            let session = try await provider.joinPresence(name: name, color: color)
            presence = session
            presenceView = session.view()
            let me = try await session.me()
            let members = try await session.roster()
            publish(roster.apply(me: me, members: members, surfaceSize: surfaceSize))
            presenceTask = Task { [weak self] in
                // Bounded per call; ends with the task (teardown cancels it).
                while !Task.isCancelled {
                    guard let event = try? await session.nextEvent(timeoutMs: 1_000) else {
                        if Task.isCancelled { return }
                        continue
                    }
                    self?.apply(event)
                }
            }
        } catch {
            // Presence is optional: a Space without it still streams.
        }
    }

    private func apply(_ event: CuaSDK.PresenceEvent) {
        _ = presenceView?.apply(event: event, localMs: presenceNowMs())
        publish(roster.apply(event: event, surfaceSize: surfaceSize))
    }

    private func publish(_ events: [PresenceEvent]) {
        localParticipant = roster.me
        participants = roster.others
        for e in events { presenceEvents.send(e) }
    }

    /// The color an agent shows as right now, for its avatar and its cursor
    /// alike (`PresenceRoster.colorOf(principalID:)` over this session's
    /// roster).
    public func color(ofAgent principalID: String) -> String {
        roster.colorOf(principalID: principalID)
    }

    /// Publish the local pointer over the streamed surface.
    ///
    /// Takes a point **in the view**, and converts it with the same
    /// `StreamGeometry` that maps a click — so a presence cursor and the click
    /// it precedes cannot land in different places.
    public func publishCursor(viewPoint: CGPoint, in viewSize: CGSize,
                              visible: Bool = true, pressed: Bool = false) async {
        movePresenceCursor(viewPoint: viewPoint, in: viewSize, visible: visible, pressed: pressed)
        await cursorSend?.value
    }

    /// `publishCursor` without waiting: the newest position replaces any not
    /// yet sent, one send is in flight at a time, and the SDK throttles to
    /// 30 Hz without dropping the final position or a hide.
    public func movePresenceCursor(viewPoint: CGPoint, in viewSize: CGSize,
                                   visible: Bool = true, pressed: Bool = false) {
        guard presence != nil else { return }
        let geometry = StreamGeometry(surfaceSize: surfaceSize, viewSize: viewSize)
        let normalized = geometry.normalized(for: viewPoint)
        // Outside the letterboxed content the pointer has left the surface:
        // a hide, never a dropped update (a swallowed hide strands the cursor
        // on every other viewer).
        let isVisible = visible && normalized != nil
        let point = normalized ?? .zero
        pendingCursor = PresenceCursor(
            displayId: "", windowId: currentTargetHandle?.value,
            x: Double(point.x), y: Double(point.y), visible: isVisible,
            pressed: pressed, shape: "arrow", shapeSource: "unspecified", atMs: 0, receivedMs: 0)
        guard cursorSend == nil else { return }
        cursorSend = Task { [weak self] in
            while let self, let presence = self.presence, let next = self.pendingCursor {
                self.pendingCursor = nil
                try? await presence.updateCursor(cursor: next)
            }
            self?.cursorSend = nil
        }
    }

    private var pendingCursor: PresenceCursor?
    private var cursorSend: Task<Void, Never>?

    private func ingest(_ buffer: CVPixelBuffer, descriptor: VideoFrameDescriptor) {
        frame = buffer
        lastFrameDimensions = CGSize(width: CVPixelBufferGetWidth(buffer),
                                     height: CVPixelBufferGetHeight(buffer))
        surfaceSize = CGSize(width: descriptor.width_px, height: descriptor.height_px)
        // Counted here rather than read from the decoder, which is replaced on
        // every source switch.
        decodedFrameCount += 1
        droppedBeforeKeyframeCount = decoder?.droppedBeforeKeyframeCount ?? droppedBeforeKeyframeCount
        decodeFailureCount = decoder?.decodeFailureCount ?? decodeFailureCount
        if status != .streaming { status = .streaming }
    }

    private func requestKeyframeThrottled(force: Bool = false) {
        guard let media else { return }
        let now = Date()
        guard force || now.timeIntervalSince(lastKeyframeRequest) > 1 else { return }
        lastKeyframeRequest = now
        try? media.requestKeyframe()
    }

    // MARK: - Input

    /// Send pointer/scroll/key input, already normalized by the view's own
    /// `StreamGeometry` (the session never invents one, which keeps the
    /// single-scale rule true across the in-app view and the PiP window).
    /// Desktop and window sources take the same `interactive_input` batches.
    public func send(_ events: [InteractiveInputEvent]) {
        let batch = events.filter(\.isDispatchable)
        guard !batch.isEmpty, let media, let sessionID else { return }
        guard let text = try? interactiveInputText(
            session: sessionID, firstSequence: nextInputSequence, events: batch) else { return }
        sentBatches.append((first: nextInputSequence, events: batch))
        if sentBatches.count > Self.resendableBatches { sentBatches.removeFirst() }
        nextInputSequence += UInt64(batch.count)
        inputEventsSent += batch.count
        try? media.sendText(json: text)
    }

    /// The Space can reach the streamed window only by activating it: reopen
    /// the window with `allow_activation` and resend the refused batch and
    /// everything sent after it.
    private func reopenActivating(resending through: UInt64) {
        activatesOnInput = true
        let refused = sentBatches
            .filter { $0.first + UInt64($0.events.count) > through }
            .map(\.events)
        Task { @MainActor [weak self] in
            guard let self else { return }
            await self.teardownTransport()
            await self.begin()
            guard self.media != nil else { return }
            for events in refused { self.send(events) }
        }
    }
}

/// A decoded frame on its way to the main actor. VideoToolbox never writes a
/// buffer it has handed out, so sharing it is safe.
struct DecodedFrame: @unchecked Sendable {
    let buffer: CVPixelBuffer
}

/// Receives the SDK's frames on its delivery thread and feeds the decoder in
/// order; control events hop to the session.
final class FrameRelay: FrameSink, @unchecked Sendable {
    // Everything mutable is behind `lock`: the SDK calls `onFrame` on its
    // delivery thread while the session sets the id and detaches on the main
    // actor. (The id used to be a bare `var` written from the main actor and
    // read here: a torn read of a `String`.)
    private let lock = NSLock()
    private var decoder: H264Decoder?
    private var session = SessionID("")
    private let onEvent: @Sendable (MediaEvent) -> Void

    init(decoder: H264Decoder, onEvent: @escaping @Sendable (MediaEvent) -> Void) {
        self.decoder = decoder
        self.onEvent = onEvent
    }

    func setSession(_ id: SessionID) {
        lock.lock(); session = id; lock.unlock()
    }

    func detach() {
        lock.lock(); decoder = nil; lock.unlock()
    }

    func onFrame(frame: VideoFrame) {
        lock.lock(); let d = decoder; let id = session; lock.unlock()
        guard let d, frame.codec == "h264" else { return }
        d.decode(payload: frame.data, descriptor: VideoFrameDescriptor(frame, session: id))
    }

    func onEvent(event: MediaEvent) {
        onEvent(event)
    }
}
