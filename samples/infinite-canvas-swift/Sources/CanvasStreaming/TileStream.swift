// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AVFoundation
import CanvasModel
import Cua
import CuaSpacesStreaming
import Foundation
import os

/// Where a tile's encoded frames come from.
///
/// `SpaceMediaSource` is the real one (a cua SDK `SpaceStreamSession` on one
/// window of a Space). `SyntheticMediaSource` encodes a test pattern locally
/// with VideoToolbox; only the benchmark and the tests use it.
public protocol MediaSource: AnyObject, Sendable {
    /// Start delivering access units to `sink` on a background thread.
    func start(_ sink: MediaSink) async throws
    func stop() async
    func setPreferences(_ p: StreamPreferences)
    func requestKeyframe()
    /// Interactive input, when the source takes it.
    func send(_ events: [InteractiveInputEvent])
    /// A media-socket ping; the source reports `pong` with the same nonce
    /// as an event. Sources without a network do nothing.
    func ping(nonce: UInt64)
}

/// What a source delivers to.
public protocol MediaSink: AnyObject, Sendable {
    func accessUnit(_ data: Data, keyframe: Bool, codecEpoch: UInt64, ptsMicros: UInt64, size: CGSize)
    func sourceEvent(kind: String, json: String)
}

/// One tile's stream: source, hardware decoder, display layer, and the
/// level-of-detail tier currently applied.
public final class TileStream: MediaSink, @unchecked Sendable {
    public let id: String
    public let layer: AVSampleBufferDisplayLayer
    public let decoder: TileDecoder
    public let source: MediaSource
    /// Main-thread callbacks.
    public var onSurfaceSize: ((CGSize) -> Void)?
    public var onEvent: ((String, String) -> Void)?

    public private(set) var tier: StreamTier = .medium
    public private(set) var tierChanges = 0
    public private(set) var keyframeRequests = 0
    private let signposter = OSSignposter(subsystem: Signposts.subsystem, category: "lod")
    private var lastKeyframeAsk = DispatchTime(uptimeNanoseconds: 0)
    private let stateLock = NSLock()

    // Stats for the hover card, sampled at 1 Hz off the main thread.
    private var latency = SampleRing(capacity: 40)
    private var pings: [UInt64: UInt64] = [:]
    private var nextNonce: UInt64 = 1
    private var fpsValue: Double = 0
    private var lastPresented = 0
    private var statsTimer: DispatchSourceTimer?

    public struct Stats: Equatable, Sendable {
        public var latencyMs: [Double]
        public var fps: Double
        public var resolution: CGSize
    }

    public var stats: Stats {
        stateLock.lock(); defer { stateLock.unlock() }
        return Stats(latencyMs: latency.values, fps: fpsValue, resolution: decoder.lastDecodedSize)
    }

    public init(id: String, source: MediaSource) {
        self.id = id
        self.source = source
        let layer = AVSampleBufferDisplayLayer()
        layer.videoGravity = .resizeAspect
        layer.backgroundColor = CGColor(gray: 0.08, alpha: 1)
        self.layer = layer
        decoder = TileDecoder(renderer: layer.sampleBufferRenderer, label: id)
        decoder.onNeedsKeyframe = { [weak self] in self?.askKeyframe(force: false) }
        decoder.onSize = { [weak self] size in
            DispatchQueue.main.async { self?.onSurfaceSize?(size) }
        }
    }

    public func start(initial: StreamTier = .medium) async throws {
        tier = initial
        decoder.setEnabled(initial.decodes)
        try await source.start(self)
        source.setPreferences(initial.preferences)
        startStats()
    }

    private func startStats() {
        let t = DispatchSource.makeTimerSource(queue: .global(qos: .utility))
        t.schedule(deadline: .now() + 1, repeating: 1, leeway: .milliseconds(100))
        t.setEventHandler { [weak self] in self?.sampleStats() }
        t.resume()
        statsTimer = t
    }

    private func sampleStats() {
        let presented = decoder.counters.snapshot().presented
        stateLock.lock()
        fpsValue = Double(presented - lastPresented)
        lastPresented = presented
        let decoding = tier.decodes
        let nonce = nextNonce
        nextNonce += 1
        if decoding { pings[nonce] = DispatchTime.now().uptimeNanoseconds }
        // Forget pings that never came back.
        if pings.count > 8 { pings = pings.filter { $0.key + 8 > nonce } }
        stateLock.unlock()
        if decoding { source.ping(nonce: nonce) }
    }

    public func stop() async {
        statsTimer?.cancel()
        statsTimer = nil
        await source.stop()
        decoder.setEnabled(false)
    }

    /// Apply a level-of-detail change: tell the Space what to encode, and
    /// start or stop decoding here.
    public func apply(_ newTier: StreamTier, keyframe: Bool) {
        stateLock.lock()
        let old = tier
        tier = newTier
        tierChanges += 1
        stateLock.unlock()
        signposter.emitEvent("tier", "\(self.id, privacy: .public) \(old.description, privacy: .public)->\(newTier.description, privacy: .public)")
        source.setPreferences(newTier.preferences)
        decoder.setEnabled(newTier.decodes)
        if keyframe { askKeyframe(force: true) }
    }

    public func send(_ events: [InteractiveInputEvent]) { source.send(events) }

    /// At most one keyframe request per 400 ms unless forced (the server
    /// also rate-limits).
    private func askKeyframe(force: Bool) {
        let now = DispatchTime.now()
        stateLock.lock()
        let recent = now.uptimeNanoseconds - lastKeyframeAsk.uptimeNanoseconds < 400_000_000
        if recent && !force {
            stateLock.unlock()
            return
        }
        lastKeyframeAsk = now
        keyframeRequests += 1
        stateLock.unlock()
        source.requestKeyframe()
    }

    // MARK: MediaSink

    public func accessUnit(_ data: Data, keyframe: Bool, codecEpoch: UInt64, ptsMicros: UInt64, size: CGSize) {
        decoder.decode(data, keyframe: keyframe, codecEpoch: codecEpoch, ptsMicros: ptsMicros)
    }

    public func sourceEvent(kind: String, json: String) {
        if kind == "pong", let nonce = Self.nonce(json) {
            let now = DispatchTime.now().uptimeNanoseconds
            stateLock.lock()
            if let sent = pings.removeValue(forKey: nonce) { latency.append(Double(now - sent) / 1e6) }
            stateLock.unlock()
            return
        }
        guard let onEvent else { return }
        DispatchQueue.main.async { onEvent(kind, json) }
    }
}

extension TileStream {
    static func nonce(_ json: String) -> UInt64? {
        guard let d = try? JSONSerialization.jsonObject(with: Data(json.utf8)) as? [String: Any] else { return nil }
        let body = (d["payload"] as? [String: Any]) ?? ((d["message"] as? [String: Any])?["payload"] as? [String: Any]) ?? d
        return (body["nonce"] as? NSNumber)?.uint64Value
    }
}

/// A window (or the whole display) of a Space through the cua SDK.
public final class SpaceMediaSource: MediaSource, FrameSink, @unchecked Sendable {
    public let space: CuaSDK.Space
    public let windowID: String?
    private let lock = NSLock()
    private var session: SpaceStreamSession?
    private weak var sink: MediaSink?
    private var nextInputSequence: UInt64 = 1
    private var pendingPreferences: StreamPreferences?

    public init(space: CuaSDK.Space, windowID: String?) {
        self.space = space
        self.windowID = windowID
    }

    public func start(_ sink: MediaSink) async throws {
        lock.withLock { self.sink = sink }
        var options = SpaceStreamOptions(maxFps: 30, audio: false, policy: "background_only")
        options.codecs = ["h264"]
        options.maxDimension = 1280
        options.windowId = windowID
        let session = try await space.streamSession(options: options, frames: self, audio: nil)
        let pending: StreamPreferences? = lock.withLock {
            self.session = session
            defer { pendingPreferences = nil }
            return pendingPreferences
        }
        if let pending { setPreferences(pending) }
        try? session.requestKeyframe()
    }

    public func stop() async {
        let s: SpaceStreamSession? = lock.withLock {
            defer { session = nil; sink = nil }
            return session
        }
        if let s { _ = try? await s.close() }
    }

    private var sessionID: String? {
        lock.lock(); defer { lock.unlock() }
        return session?.mediaSessionId()
    }

    /// The media socket's `set_stream_preferences`: the per-window fps and
    /// long-edge caps the Space's encoder applies from its next frame.
    public func setPreferences(_ p: StreamPreferences) {
        lock.lock()
        guard let session else {
            pendingPreferences = p
            lock.unlock()
            return
        }
        lock.unlock()
        let message: [String: Any] = [
            "type": "set_stream_preferences",
            "payload": ["session_id": session.mediaSessionId(), "max_fps": p.maxFps,
                        "max_dimension": p.maxDimension],
        ]
        if let data = try? JSONSerialization.data(withJSONObject: message),
           let text = String(data: data, encoding: .utf8) {
            try? session.sendText(json: text)
        }
    }

    public func requestKeyframe() {
        lock.lock(); let s = session; lock.unlock()
        try? s?.requestKeyframe()
    }

    public func ping(nonce: UInt64) {
        lock.lock(); let s = session; lock.unlock()
        try? s?.sendText(json: "{\"type\":\"ping\",\"payload\":{\"nonce\":\(nonce)}}")
    }

    public func send(_ events: [InteractiveInputEvent]) {
        let batch = events.filter(\.isDispatchable)
        lock.lock()
        guard let session, !batch.isEmpty else { lock.unlock(); return }
        let first = nextInputSequence
        nextInputSequence += UInt64(batch.count)
        lock.unlock()
        if let text = try? interactiveInputText(session: SessionID(session.mediaSessionId()),
                                                firstSequence: first, events: batch) {
            try? session.sendText(json: text)
        }
    }

    // MARK: FrameSink (SDK delivery thread)

    public func onFrame(frame: VideoFrame) {
        lock.lock(); let sink = self.sink; lock.unlock()
        guard frame.codec == "h264" else { return }
        sink?.accessUnit(frame.data, keyframe: frame.keyframe, codecEpoch: frame.codecEpoch,
                         ptsMicros: frame.captureTimestampUs,
                         size: CGSize(width: Double(frame.width), height: Double(frame.height)))
    }

    public func onEvent(event: MediaEvent) {
        lock.lock(); let sink = self.sink; lock.unlock()
        sink?.sourceEvent(kind: event.kind, json: event.json)
    }
}
