import Cua
import Foundation

/// Where a stream's frames and audio go besides the recorder (window mode).
protocol StreamPresenter: AnyObject, Sendable {
    func present(frame: DecodedVideoFrame)
    func play(audio: PcmAudio)
}

/// Where the timecode strip sits in the frame (bench mode).
struct TimecodeLocator: Sendable {
    /// Strip origin in content pixels of the target (window: 0,0; display:
    /// the bench window's position on the display).
    var originX: Double
    var originY: Double
    /// Width of the target in content pixels (window or display width), so
    /// the frame scale is `frame.width / contentWidth`.
    var contentWidth: Double
}

/// Receives decoded frames, PCM and control events for one media session and
/// keeps what the summary, the click check and the bench JSONL need.
///
/// The SDK calls it on the session's delivery thread; everything is guarded
/// by one lock and every callback returns quickly.
final class StreamRecorder: DecodedFrameSink, PcmSink, @unchecked Sendable {
    private let lock = NSLock()
    private let start = monoSeconds()
    private let bench: JsonlWriter?
    private let timecode: TimecodeLocator?
    private weak var presenter: StreamPresenter?

    private(set) var frames: UInt64 = 0
    private(set) var firstFrameAt: Double?
    private(set) var lastFrameAt: Double?
    private(set) var lastFrame: DecodedVideoFrame?
    private(set) var pcmFrames: UInt64 = 0
    private(set) var concealed: UInt64 = 0
    private(set) var audioTrack: UInt16?
    private(set) var sampleRate: UInt32 = 48_000
    private(set) var channels: UInt16 = 2
    private var pcm: [Int16] = []
    private let maxPcmSamples = 48_000 * 2 * 600  // 10 min stereo cap
    private var events: [MediaEvent] = []
    private let maxEvents = 4096
    private(set) var decodeErrors: [String] = []
    private(set) var closed = false
    private(set) var tcDecoded: UInt64 = 0

    init(bench: JsonlWriter? = nil, timecode: TimecodeLocator? = nil, presenter: StreamPresenter? = nil) {
        self.bench = bench
        self.timecode = timecode
        self.presenter = presenter
    }

    // MARK: DecodedFrameSink

    func onDecodedFrame(frame: DecodedVideoFrame) {
        let now = monoSeconds()
        let unixNs = unixNanos()
        var tc: UInt64?
        if bench != nil, let loc = timecode {
            let scale = loc.contentWidth > 0 ? Double(frame.width) / loc.contentWidth : 1
            tc = decodeTimecode(
                bgra: frame.data, width: Int(frame.width), height: Int(frame.height),
                stride: Int(frame.stride), originX: loc.originX * scale, originY: loc.originY * scale,
                scale: scale, clientUnixNs: unixNs)
        }
        lock.lock()
        frames += 1
        if firstFrameAt == nil { firstFrameAt = now }
        lastFrameAt = now
        lastFrame = frame
        if tc != nil { tcDecoded += 1 }
        lock.unlock()
        if let bench {
            // The decoded callback does not carry the encoded size or the
            // keyframe flag (see README "SDK gaps"): null.
            let tcs = tc.map { String($0) } ?? "null"
            bench.line(
                "{\"t\":\"frame\",\"unix_ns\":\(unixNs),\"seq\":\(frame.sequence),\"bytes\":null,\"key\":null,"
                    + "\"cap_us\":\(frame.captureTimestampUs),\"w\":\(frame.width),\"h\":\(frame.height),\"tc_ms\":\(tcs)}")
        }
        presenter?.present(frame: frame)
    }

    func onEvent(event: MediaEvent) {
        lock.lock()
        if events.count < maxEvents { events.append(event) }
        if event.kind == "decode_error", decodeErrors.count < 32 { decodeErrors.append(event.json) }
        if event.kind == "closed" { closed = true }
        lock.unlock()
    }

    // MARK: PcmSink

    func onPcm(audio: PcmAudio) {
        let unixNs = unixNanos()
        lock.lock()
        if audioTrack == nil {
            audioTrack = audio.trackId
            sampleRate = audio.sampleRate
            channels = max(1, audio.channels)
        }
        let mine = audio.trackId == audioTrack
        if mine {
            pcmFrames += 1
            if audio.concealed { concealed += 1 }
            if pcm.count + audio.samples.count <= maxPcmSamples { pcm.append(contentsOf: audio.samples) }
        }
        lock.unlock()
        if let bench {
            let per = audio.samples.count / Int(max(1, audio.channels))
            bench.line("{\"t\":\"audio\",\"unix_ns\":\(unixNs),\"pts_us\":\(audio.ptsUs),\"bytes\":null,\"samples\":\(per)}")
        }
        if mine { presenter?.play(audio: audio) }
    }

    // MARK: Queries

    func snapshot() -> (frames: UInt64, last: DecodedVideoFrame?) {
        lock.lock(); defer { lock.unlock() }
        return (frames, lastFrame)
    }

    /// Events seen so far, from index `from`.
    func events(from: Int) -> [MediaEvent] {
        lock.lock(); defer { lock.unlock() }
        return from < events.count ? Array(events[from...]) : []
    }

    func eventCount() -> Int {
        lock.lock(); defer { lock.unlock() }
        return events.count
    }

    func firstFrameMs() -> Double? {
        lock.lock(); defer { lock.unlock() }
        return firstFrameAt.map { ($0 - start) * 1000 }
    }

    func fps() -> Double? {
        lock.lock(); defer { lock.unlock() }
        guard let a = firstFrameAt, let b = lastFrameAt, frames > 1, b > a else { return nil }
        return Double(frames - 1) / (b - a)
    }

    /// `(r, g, b)` of the latest frame at frame pixel (x, y).
    func pixel(x: Int, y: Int) -> (Int, Int, Int)? {
        lock.lock(); defer { lock.unlock() }
        guard let f = lastFrame, x >= 0, y >= 0, x < Int(f.width), y < Int(f.height) else { return nil }
        let o = y * Int(f.stride) + x * 4
        return f.data.withUnsafeBytes { p in
            o + 2 < p.count ? (Int(p[o + 2]), Int(p[o + 1]), Int(p[o])) : nil
        }
    }

    func saveWav(path: String) throws -> Bool {
        lock.lock()
        let samples = pcm, rate = sampleRate, ch = channels, any = audioTrack != nil
        lock.unlock()
        guard any else { return false }
        try writeWav(path: path, sampleRate: rate, channels: ch, samples: samples)
        return true
    }
}
