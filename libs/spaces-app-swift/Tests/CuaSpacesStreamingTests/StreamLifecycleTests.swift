// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CoreGraphics
import Cua
import Foundation
import Testing
@testable import CuaSpaces
@testable import CuaSpacesStreaming

/// A media session that never touches Rust: counts closes and answers the
/// three calls `LiveStreamSession` makes.
private final class FakeMedia: SpaceStreamSession, @unchecked Sendable {
    let id: String
    private let lock = NSLock()
    private var closes = 0

    init(id: String) {
        self.id = id
        super.init(noHandle: NoHandle())
    }

    required init(unsafeFromHandle handle: UInt64) { fatalError("not used") }

    var closeCount: Int { lock.lock(); defer { lock.unlock() }; return closes }

    override func codec() -> String { "h264" }
    override func mediaSessionId() -> String { id }
    override func requestKeyframe() throws {}
    override func sendText(json: String) throws {}
    override func isOpen() -> Bool { closeCount == 0 }
    override func close() async throws -> SpaceStreamStats {
        lock.lock(); closes += 1; lock.unlock()
        return SpaceStreamStats(frames: 0, keyframes: 0, framesDropped: 0, framesGated: 0,
                                keyframeRequests: 0, audioPackets: 0, audioLost: 0,
                                events: 0, malformed: 0)
    }
}

/// Opens are held until `release()`; every opened session and its frame
/// sink are kept, so a test can check each was closed and poke a stale one.
private actor Gate {
    private var waiting: [CheckedContinuation<Void, Never>] = []
    private var open = false
    private(set) var arrivals = 0

    func pass() async {
        arrivals += 1
        if open { return }
        await withCheckedContinuation { waiting.append($0) }
    }

    func release() {
        open = true
        for c in waiting { c.resume() }
        waiting.removeAll()
    }
}

private final class GatedProvider: SpaceStreamSourceProviding, @unchecked Sendable {
    let gate = Gate()
    private let lock = NSLock()
    private var medias: [FakeMedia] = []
    private var sinks: [FrameSink] = []
    /// Yield this many times inside every open, to shuffle interleavings.
    let yields: Int

    init(gated: Bool = true, yields: Int = 0) {
        self.yields = yields
        if !gated { Task { [gate] in await gate.release() } }
    }

    var opened: [FakeMedia] { lock.lock(); defer { lock.unlock() }; return medias }
    var frameSinks: [FrameSink] { lock.lock(); defer { lock.unlock() }; return sinks }

    func availableWindows() async throws -> [StreamWindow] { [] }

    func openSession(_ source: StreamSource, frames: FrameSink,
                     audio: AudioSink?) async throws -> SpaceStreamSession {
        await gate.pass()
        for _ in 0..<yields { await Task.yield() }
        lock.lock()
        let media = FakeMedia(id: "media-\(medias.count)")
        medias.append(media)
        sinks.append(frames)
        lock.unlock()
        return media
    }

    func joinPresence(name: String, color: String?) async throws -> SpacePresence {
        throw StreamError.noStream("no presence in this test")
    }
}

/// Bounded wait (never an unbounded poll).
@MainActor
private func eventually(_ what: String, _ cond: @MainActor () async -> Bool) async {
    for _ in 0..<400 {
        if await cond() { return }
        try? await Task.sleep(for: .milliseconds(5))
    }
    Issue.record("timed out waiting for \(what)")
}

/// The stream's lifecycle under concurrency: the crash in
/// `LiveStreamSession.open` (SIGSEGV comparing `media.codec()`) had a stale
/// build as its cause, but the same code let a start and a stop (or two
/// starts) race across the `await` in `open`: two transports opened, the
/// late one leaked and its frames and status won. These run under
/// `swift test --sanitize=thread` too.
@Suite(.serialized) @MainActor struct StreamLifecycleTests {

    @Test func concurrentStartsOpenOneTransport() async {
        let provider = GatedProvider()
        let session = LiveStreamSession(provider: provider)
        let starts = (0..<8).map { _ in Task { await session.start() } }
        await eventually("the first open") { await provider.gate.arrivals >= 1 }
        #expect(session.status == .connecting)
        await provider.gate.release()
        for s in starts { await s.value }
        #expect(provider.opened.count == 1, "one transport, not \(provider.opened.count)")
        #expect(session.status == .streaming)
        await session.stop()
        #expect(provider.opened.map(\.closeCount) == [1])
    }

    @Test func stopWhileOpeningClosesTheLateTransportAndIgnoresIt() async throws {
        let provider = GatedProvider()
        let session = LiveStreamSession(provider: provider)
        let start = Task { await session.start() }
        await eventually("the open") { await provider.gate.arrivals >= 1 }
        await session.stop()
        #expect(session.status == .idle)
        await provider.gate.release()
        await start.value
        let late = try #require(provider.opened.first)
        #expect(late.closeCount == 1, "the superseded transport is closed")
        #expect(session.status == .idle, "a stopped session stays stopped")
        // The superseded transport's events never reach the session.
        let sink = try #require(provider.frameSinks.first)
        sink.onEvent(event: MediaEvent(kind: "error", json: #"{"code":"x","message":"stale"}"#))
        try await Task.sleep(for: .milliseconds(50))
        #expect(session.status == .idle)
    }

    @Test func switchingSourceWhileOpeningKeepsOnlyTheNewSource() async throws {
        let provider = GatedProvider()
        let session = LiveStreamSession(provider: provider)
        let start = Task { await session.start() }
        await eventually("the open") { await provider.gate.arrivals >= 1 }
        let window = StreamWindow(id: "w-1", app: "Terminal", title: "t", epoch: 1)
        let select = Task { await session.select(.window(window)) }
        await eventually("the second open") { await provider.gate.arrivals >= 2 }
        await provider.gate.release()
        await start.value
        await select.value
        #expect(provider.opened.count == 2)
        #expect(provider.opened.map(\.closeCount).sorted() == [0, 1],
                "the desktop open is closed, the window's is live")
        #expect(session.status == .streaming)
        #expect(session.source == .window(window))
        await session.stop()
        #expect(provider.opened.allSatisfy { $0.closeCount == 1 })
    }

    /// Starts, stops and source switches from many tasks at once: every
    /// transport opened is closed exactly once and the session ends idle.
    @Test func startStopStormLeaksNothing() async {
        let provider = GatedProvider(gated: false, yields: 3)
        let session = LiveStreamSession(provider: provider)
        let window = StreamWindow(id: "w-2", app: "Files", title: "f", epoch: 1)
        await withTaskGroup(of: Void.self) { group in
            for i in 0..<60 {
                group.addTask { @MainActor in
                    switch i % 4 {
                    case 0, 1: await session.start()
                    case 2: await session.stop()
                    default: await session.select(i % 8 == 3 ? .window(window) : .desktop)
                    }
                }
            }
        }
        await session.stop()
        #expect(session.status == .idle)
        let opened = provider.opened
        #expect(!opened.isEmpty)
        #expect(opened.allSatisfy { $0.closeCount == 1 },
                "every transport closed once: \(opened.map(\.closeCount))")
    }

    /// A Space still being created is not dialled: the stream fails with
    /// words, and nothing reaches the SDK.
    @Test func aStartingSpaceIsNotStreamed() async throws {
        let connection = SpacesConnection(transport: FakeSpacesBackend())
        let info = SpaceInfo(id: SpaceID("local:cua-e2e-starting"), provider: .local,
                             operatingSystem: "linux", state: .starting,
                             rawPhase: "starting", ipAddress: nil)
        let session = LiveStreamSession(provider: SpaceStreamProvider(
            space: Space(info: info, connection: connection)))
        await session.start()
        guard case let .failed(why) = session.status else {
            Issue.record("expected a failure, got \(session.status)")
            return
        }
        #expect(why.contains("still starting"), "\(why)")
    }

    /// The frame relay's session id is written on the main actor while the
    /// SDK's delivery thread reads it (it was an unguarded `var`).
    @Test func relaySessionIdIsSafeAcrossThreads() async {
        let relay = FrameRelay(decoder: H264Decoder()) { _ in }
        let frame = VideoFrame(sequence: 1, codec: "vp8", keyframe: true, width: 2, height: 2,
                               captureTimestampUs: 0, codecEpoch: 1, geometryEpoch: 1,
                               data: Data([0, 0, 0, 1]), headerJson: "")
        let reader = Task.detached {
            for _ in 0..<2_000 { relay.onFrame(frame: frame) }
        }
        for i in 0..<2_000 { relay.setSession(SessionID("session-\(i)-with-a-long-enough-id")) }
        await reader.value
        relay.detach()
    }
}
