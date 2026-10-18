// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CoreGraphics
import Cua
import CuaSpacesFFI
import CuaSpacesStreaming
import Foundation

/// Synthetic Space streams for the native video harness
/// (`CUA_SPACES_SYNTHETIC_VIDEO=<dir>`, debug builds; docs/native-video.md).
///
/// Plays a raw H.264 Annex B file (the output of
/// `apps/cua-spaces-desktop/scripts/video/encode.sh`: no B-frames, SPS/PPS
/// before every IDR, one access unit per frame) into
/// the stream pipeline as if it came off the media socket: the frames go to
/// the `FrameSink` a `LiveStreamSession` hands over, so the decoder, the
/// one-slot mailbox, the view and the input mapping are the production
/// ones. Only the network is missing. Each session starts at a different
/// keyframe, answers `requestKeyframe` by jumping to the next one, loops,
/// and counts the input batches it was sent.
final class SyntheticStreamProvider: SpaceStreamSourceProviding, @unchecked Sendable {
    let units: [AccessUnit]
    let size: CGSize
    let fps: Double
    let startOffset: Int
    /// The fixture Space's OS (`linux`, `macos`): a Linux one takes ⌘ as Control.
    let guestOS: String

    init(units: [AccessUnit], size: CGSize, fps: Double, startOffset: Int = 0, guestOS: String = "") {
        self.units = units
        self.size = size
        self.fps = fps
        self.startOffset = startOffset
        self.guestOS = guestOS
    }

    var commandAsControl: Bool { InputEncoder.commandAsControl(guestOS: guestOS) }

    /// The harness's stream for a tier, from `<dir>/tile30.h264` (1280×800 at
    /// 30 fps) or `<dir>/full60.h264` (1920×1080 at 60 fps); `offset` picks
    /// the starting keyframe so tiles don't show the same picture.
    /// `tile` names the tile stream: `tile30` (1280×800 at 30 fps), or `tile10`
    /// (960×600 at 10 fps, what `VideoTier.tile` asks a Space for).
    static func harness(dir: URL, tier: VideoTier, offset: Int, tile: String = "tile30",
                        guestOS: String = "") throws -> SyntheticStreamProvider {
        let (name, size, fps): (String, CGSize, Double) = tier == .full
            ? ("full60", CGSize(width: 1920, height: 1080), 60)
            : tile == "tile10" ? ("tile10", CGSize(width: 960, height: 600), 10)
            : ("tile30", CGSize(width: 1280, height: 800), 30)
        let units = try cachedUnits(dir.appendingPathComponent("\(name).h264"))
        return SyntheticStreamProvider(units: units, size: size, fps: fps, startOffset: offset, guestOS: guestOS)
    }

    private static let cacheLock = NSLock()
    nonisolated(unsafe) private static var cache: [URL: [AccessUnit]] = [:]

    static func cachedUnits(_ file: URL) throws -> [AccessUnit] {
        cacheLock.lock()
        defer { cacheLock.unlock() }
        if let hit = cache[file] { return hit }
        let units = AccessUnit.split(try Data(contentsOf: file, options: .mappedIfSafe))
        guard units.contains(where: \.keyframe) else {
            throw StreamError.noStream("\(file.lastPathComponent) has no keyframe")
        }
        cache[file] = units
        return units
    }

    func availableWindows() async throws -> [StreamWindow] { [] }

    /// The last session opened (tests read what it was sent).
    private(set) weak var lastMedia: SyntheticMedia?

    func openSession(_ source: StreamSource, frames: FrameSink, audio: AudioSink?) async throws -> SpaceStreamSession {
        let media = SyntheticMedia(provider: self, sink: frames)
        lastMedia = media
        return media
    }

    func joinPresence(name: String, color: String?) async throws -> SpacePresence {
        throw StreamError.noStream("synthetic streams have no presence")
    }
}

/// One H.264 access unit of an Annex B stream.
struct AccessUnit: Sendable {
    let data: Data
    let keyframe: Bool

    /// Splits an Annex B stream into access units: a new unit starts at a
    /// non-VCL NAL after a VCL one, or at a slice whose first_mb_in_slice is
    /// 0 (its ue(v) starts with a 1 bit).
    static func split(_ buf: Data) -> [AccessUnit] {
        let bytes = [UInt8](buf)
        var starts: [(code: Int, start: Int)] = []
        var i = 0
        while i + 3 <= bytes.count {
            if bytes[i] == 0, bytes[i + 1] == 0, bytes[i + 2] == 1 {
                starts.append((i > 0 && bytes[i - 1] == 0 ? i - 1 : i, i + 3))
                i += 3
            } else {
                i += 1
            }
        }
        var units: [AccessUnit] = []
        var from = -1, to = 0, key = false, hasVCL = false
        for (k, s) in starts.enumerated() where s.start < bytes.count {
            let end = k + 1 < starts.count ? starts[k + 1].code : bytes.count
            let type = bytes[s.start] & 0x1f
            let vcl = type == 1 || type == 5
            let firstSlice = vcl && s.start + 1 < bytes.count && bytes[s.start + 1] & 0x80 != 0
            if from < 0 || (hasVCL && (!vcl || firstSlice)) {
                if from >= 0 { units.append(AccessUnit(data: Data(bytes[from..<to]), keyframe: key)) }
                from = s.code
                key = false
                hasVCL = false
            }
            to = end
            if vcl { hasVCL = true }
            if type == 5 { key = true }
        }
        if from >= 0 { units.append(AccessUnit(data: Data(bytes[from..<to]), keyframe: key)) }
        return units
    }
}

/// A media session that plays a synthetic stream into a `FrameSink`.
final class SyntheticMedia: SpaceStreamSession, @unchecked Sendable {
    private let provider: SyntheticStreamProvider
    private let sink: FrameSink
    private let queue = DispatchQueue(label: "cua.synthetic-video", qos: .userInteractive)
    private let lock = NSLock()
    private var timer: DispatchSourceTimer?
    private var index: Int
    private var sequence: UInt64 = 0
    private var jumpToKeyframe = true
    private var open = true
    private(set) var inputBatches = 0
    private var sent: UInt64 = 0

    /// Every session's input batches, for the harness's check that input arrives.
    nonisolated(unsafe) static var totalInputBatches = 0
    /// The last batch any session was sent (its `interactive_input` JSON),
    /// for the checks that a chord arrives with the right modifiers.
    nonisolated(unsafe) static var lastInput = ""
    private var inputs: [String] = []
    /// The events of one `interactive_input` batch.
    static func events(in json: String) -> [[String: Any]] {
        guard let data = json.data(using: .utf8),
              let header = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let message = header["message"] as? [String: Any],
              let payload = message["payload"] as? [String: Any] else { return [] }
        return payload["events"] as? [[String: Any]] ?? []
    }

    /// This session's input batches, oldest first.
    var sentInputs: [String] { lock.lock(); defer { lock.unlock() }; return inputs }

    init(provider: SyntheticStreamProvider, sink: FrameSink) {
        self.provider = provider
        self.sink = sink
        let keys = provider.units.indices.filter { provider.units[$0].keyframe }
        index = keys.isEmpty ? 0 : keys[provider.startOffset % keys.count]
        super.init(noHandle: NoHandle())
        let timer = DispatchSource.makeTimerSource(queue: queue)
        timer.schedule(deadline: .now(), repeating: 1 / provider.fps, leeway: .milliseconds(1))
        timer.setEventHandler { [weak self] in self?.tick() }
        self.timer = timer
        timer.resume()
    }

    required init(unsafeFromHandle handle: UInt64) { fatalError("not used") }

    private func tick() {
        lock.lock()
        guard open else { lock.unlock(); return }
        let units = provider.units
        if jumpToKeyframe {
            jumpToKeyframe = false
            while !units[index].keyframe { index = (index + 1) % units.count }
        }
        let unit = units[index]
        index = (index + 1) % units.count
        sequence += 1
        sent += 1
        let seq = sequence
        lock.unlock()
        sink.onFrame(frame: VideoFrame(
            sequence: seq, codec: "h264", keyframe: unit.keyframe,
            width: UInt32(provider.size.width), height: UInt32(provider.size.height),
            captureTimestampUs: UInt64(Date().timeIntervalSince1970 * 1_000_000),
            codecEpoch: 1, geometryEpoch: 1, data: unit.data, headerJson: ""))
    }

    override func codec() -> String { "h264" }
    override func mediaSessionId() -> String { "synthetic-\(ObjectIdentifier(self).hashValue)" }
    override func isOpen() -> Bool { lock.lock(); defer { lock.unlock() }; return open }

    override func requestKeyframe() throws {
        lock.lock(); jumpToKeyframe = true; lock.unlock()
    }

    override func sendText(json: String) throws {
        lock.lock()
        inputBatches += 1
        inputs.append(json)
        Self.totalInputBatches += 1
        Self.lastInput = json
        lock.unlock()
    }

    override func stats() -> SpaceStreamStats {
        lock.lock(); defer { lock.unlock() }
        return SpaceStreamStats(frames: sent, keyframes: 0, framesDropped: 0, framesGated: 0, keyframeRequests: 0,
                                audioPackets: 0, audioLost: 0, events: 0, malformed: 0)
    }

    override func close() async throws -> SpaceStreamStats {
        shutdown()
        return stats()
    }

    private func shutdown() {
        lock.lock()
        open = false
        let timer = self.timer
        self.timer = nil
        lock.unlock()
        timer?.cancel()
    }
}

extension FixtureSpacesBackend {
    /// `CUA_SPACES_FIXTURE_SPACES=<n>` (debug builds, with fixtures): the
    /// sample Spaces plus `n` running Linux Spaces on this Mac, for the video
    /// harness's grid of tiles.
    static func rows(environment: [String: String]) -> [AppSpaceRow] {
        guard let n = environment["CUA_SPACES_FIXTURE_SPACES"].flatMap(Int.init), n > 0,
              let template = sample.first else { return sample }
        let extra = (1...min(n, 32)).map { i -> AppSpaceRow in
            var row = template
            row.id = "local:bench-\(i)"
            row.name = "bench-\(i)"
            return row
        }
        return extra + sample
    }
}
