// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CoreVideo
import Cua
import Foundation
import Testing
@testable import CuaSpaces
@testable import CuaSpacesStreaming

/// What a live stream may hold on to: decoded frames wait in a one-slot
/// mailbox (newest wins) instead of one main-actor hop each, each holding
/// its IOSurface; and an ended presence stream is not polled in a loop.
@MainActor
@Suite("Stream memory bounds")
struct StreamFrameBacklogTests {

    /// Frames posted faster than they are taken: one is held, one hop is
    /// asked for, and every replaced frame is released at once (the pool's
    /// allocation threshold refuses a buffer while too many are alive).
    @Test func theMailboxHoldsOneFrameAndReleasesTheRest() throws {
        let pool = try Self.pool(width: 64, height: 64)
        let aux = [kCVPixelBufferPoolAllocationThresholdKey: 3] as CFDictionary
        let mailbox = FrameMailbox()
        var hops = 0
        for i in 0..<10_000 {
            var buffer: CVPixelBuffer?
            let status = CVPixelBufferPoolCreatePixelBufferWithAuxAttributes(nil, pool, aux, &buffer)
            #expect(status == kCVReturnSuccess, "buffer \(i): earlier frames were not released")
            guard let buffer else { return }
            if mailbox.post(buffer, Self.descriptor(width: 64, height: 64, sequence: UInt64(i))) { hops += 1 }
            #expect(mailbox.held == 1)
        }
        #expect(hops == 1, "one hop queued, however many frames arrive before it runs")
        let (frame, skipped) = try #require(mailbox.take())
        #expect(frame.descriptor.sequence == 9_999 && skipped == 9_999)
        #expect(mailbox.held == 0 && mailbox.take() == nil)
        // After a take, the next frame asks for a new hop.
        var buffer: CVPixelBuffer?
        CVPixelBufferPoolCreatePixelBuffer(nil, pool, &buffer)
        #expect(mailbox.post(try #require(buffer), Self.descriptor(width: 64, height: 64, sequence: 0)))
        mailbox.clear()
        #expect(mailbox.held == 0)
    }

    @Test func keyframeRequestsCoalesce() {
        let c = Coalescer()
        #expect(c.arm())
        for _ in 0..<100 { #expect(!c.arm()) }
        c.disarm()
        #expect(c.arm())
    }

    /// A presence stream that ended answers nil at once, forever; one
    /// that lost its connection fails at once. Either way the session stops
    /// asking (it used to spin the main actor on SDK calls, about 85,000 a
    /// second, for as long as the stream stayed open) and drops the cursors.
    @Test(arguments: [EndedPresence.Ending.ends, .fails])
    func anEndedPresenceStreamIsNotPolledInALoop(_ ending: EndedPresence.Ending) async throws {
        let presence = EndedPresence(ending)
        let provider = FrameProvider(presence: presence)
        let session = LiveStreamSession(provider: provider)
        await session.start()
        await session.joinPresence(as: "Me")
        #expect(session.presenceView != nil)
        presence.end()
        try await Task.sleep(for: .milliseconds(1_200))
        let calls = presence.callsAfterEnd
        #expect(calls <= 3, "\(calls) calls in 1.2 s after the presence stream ended")
        if ending == .ends {
            #expect(session.presenceView == nil && session.participants.isEmpty)
        }
        await session.stop()
    }

    @Test func presenceRetriesBackOff() {
        let waits = (1..<LiveStreamSession.presenceRetries).map { LiveStreamSession.presenceBackoff($0) }
        #expect(waits == waits.sorted() && waits.first! >= .milliseconds(250) && waits.last! <= .seconds(5))
    }

    // MARK: - Fixtures

    static func pool(width: Int, height: Int) throws -> CVPixelBufferPool {
        var pool: CVPixelBufferPool?
        let attributes: [CFString: Any] = [
            kCVPixelBufferPixelFormatTypeKey: kCVPixelFormatType_32BGRA,
            kCVPixelBufferWidthKey: width, kCVPixelBufferHeightKey: height,
            kCVPixelBufferIOSurfacePropertiesKey: [:] as CFDictionary,
        ]
        CVPixelBufferPoolCreate(nil, nil, attributes as CFDictionary, &pool)
        return try #require(pool)
    }

    static func descriptor(width: Int, height: Int, sequence: UInt64) -> VideoFrameDescriptor {
        VideoFrameDescriptor(videoFrame(Data(), width: width, height: height, sequence: sequence),
                             session: SessionID("test"))
    }

    static func videoFrame(_ data: Data, width: Int, height: Int, sequence: UInt64) -> VideoFrame {
        VideoFrame(sequence: sequence, codec: "h264", keyframe: true, width: UInt32(width), height: UInt32(height),
                   captureTimestampUs: sequence * 16_666, codecEpoch: 1, geometryEpoch: 1,
                   data: data, headerJson: "")
    }
}

/// A media session that never touches Rust.
private final class StillMedia: SpaceStreamSession, @unchecked Sendable {
    init() { super.init(noHandle: NoHandle()) }
    required init(unsafeFromHandle handle: UInt64) { fatalError("not used") }
    override func codec() -> String { "h264" }
    override func mediaSessionId() -> String { "media-backlog" }
    override func requestKeyframe() throws {}
    override func sendText(json: String) throws {}
    override func isOpen() -> Bool { true }
    override func close() async throws -> SpaceStreamStats {
        SpaceStreamStats(frames: 0, keyframes: 0, framesDropped: 0, framesGated: 0, keyframeRequests: 0,
                         audioPackets: 0, audioLost: 0, events: 0, malformed: 0)
    }
}

/// Presence that answers normally until `end()`, then as an ended stream
/// (nil at once) or a lost connection (`Closed` at once).
final class EndedPresence: SpacePresence, @unchecked Sendable {
    enum Ending: Sendable { case ends, fails }
    let ending: Ending
    private let lock = NSLock()
    private var ended = false
    private var after = 0

    init(_ ending: Ending) {
        self.ending = ending
        super.init(noHandle: NoHandle())
    }
    required init(unsafeFromHandle handle: UInt64) { fatalError("not used") }

    func end() { lock.lock(); ended = true; lock.unlock() }
    var callsAfterEnd: Int { lock.lock(); defer { lock.unlock() }; return after }

    override func view() -> PresenceView { PresenceView(me: "p-me") }
    override func me() async throws -> PresenceParticipant {
        PresenceParticipant(participantId: "p-me", principalId: "u", displayName: "Me", color: "#3b82f6",
                            kind: "human")
    }
    override func roster() async throws -> [PresenceMember] { [] }
    override func updateCursor(cursor: PresenceCursor) async throws {}
    override func leave() async throws {}
    override func nextEvent(timeoutMs: UInt64?) async throws -> CuaSDK.PresenceEvent? {
        lock.lock()
        let done = ended
        if done { after += 1 }
        lock.unlock()
        guard done else {
            try await Task.sleep(for: .milliseconds(20))
            throw CuaError.Timeout(message: "waiting for a presence event")
        }
        switch ending {
        case .ends: return nil
        case .fails: throw CuaError.Closed(message: "presence session")
        }
    }
}

/// Opens at once and keeps the frame sink, so the test is the transport.
private final class FrameProvider: SpaceStreamSourceProviding, @unchecked Sendable {
    private let lock = NSLock()
    private var held: FrameSink?
    private let presence: SpacePresence?

    init(presence: SpacePresence? = nil) { self.presence = presence }
    var sink: FrameSink? { lock.lock(); defer { lock.unlock() }; return held }

    func availableWindows() async throws -> [StreamWindow] { [] }

    func openSession(_ source: StreamSource, frames: FrameSink,
                     audio: AudioSink?) async throws -> SpaceStreamSession {
        lock.lock(); held = frames; lock.unlock()
        return StillMedia()
    }

    func joinPresence(name: String, color: String?) async throws -> SpacePresence {
        guard let presence else { throw StreamError.noStream("no presence in this test") }
        return presence
    }
}
