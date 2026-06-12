import AVFoundation
import AppKit
import CoreGraphics
import CoreMedia
import CoreVideo
import Foundation
@preconcurrency import ScreenCaptureKit
import os

/// Full-display H.264 video capture — the optional sibling of the
/// per-turn trajectory recorder. When armed by `RecordingSession.configure`
/// with `videoExperimental = true`, this actor stands up an `SCStream`
/// against the main display and pipes every delivered `CMSampleBuffer`
/// straight into an `AVAssetWriter` writing `<output-dir>/recording.mp4`.
///
/// Capture only — no zoom, no cursor overlay, no compositing. Those land
/// in follow-up commits once telemetry is wired in.
///
/// Failure philosophy matches `RecordingSession`: permission revocation
/// or underlying framework errors log + tear down cleanly. The trajectory
/// turn-folder pipeline keeps running regardless; losing video mid-session
/// is degraded-but-acceptable, not fatal.
public actor VideoRecorder {
    /// Target output file (always `recording.mp4` under the recording
    /// output dir). Overwrites any existing file at the same path.
    private let outputURL: URL

    private var stream: SCStream?
    /// Owns the `AVAssetWriter` + input and holds the NSLock guarding
    /// sample append. SCStream delivers on a GCD queue, so all writer
    /// access happens there — the actor only participates in lifecycle
    /// (start / stop), never per-frame.
    private var streamOutput: StreamOutputHandler?

    /// Pixel dimensions of the captured display. Populated in `start()`
    /// and surfaced to `RecordingSession` at finalize time so
    /// `session.json` can cite width/height alongside the frame count.
    private var capturedWidth: Int = 0
    private var capturedHeight: Int = 0
    /// Target framerate baked into the AVAssetWriter compression
    /// settings + SCStream minimumFrameInterval. Surfaced to
    /// `session.json` so downstream tools don't have to re-read MP4
    /// metadata just to compute frame timing.
    public static let targetFrameRate: Int = 30

    private let log = Logger(
        subsystem: "com.trycua.driver",
        category: "VideoRecorder"
    )

    public init(outputURL: URL) {
        self.outputURL = outputURL
    }

    /// Snapshot of finalize-time video metadata for `session.json`.
    public struct FinalMetadata: Sendable {
        public let width: Int
        public let height: Int
        public let frameCount: Int
    }

    /// Snapshot live video metadata. Available any time after `start()`;
    /// `frameCount` reflects frames appended so far (0 once `stop()`
    /// nils the stream-output handler — call `stop()` itself for the
    /// final count).
    public func currentMetadata() -> FinalMetadata {
        let count = streamOutput?.currentFrameCount() ?? 0
        return FinalMetadata(
            width: capturedWidth, height: capturedHeight, frameCount: count
        )
    }

    /// Configure the asset writer + SCStream and start capture. Throws on
    /// any fatal setup error (no display, asset-writer init failure, etc.).
    /// Screen Recording permission errors surface as thrown errors here;
    /// the caller (RecordingSession) logs and proceeds without video.
    public func start() async throws {
        // Overwrite a stale recording.mp4 from a prior session. The
        // trajectory pipeline writes turn-N folders fresh alongside, so
        // matching that "start clean" behavior keeps the output dir's
        // contents consistent per-session.
        try? FileManager.default.removeItem(at: outputURL)

        let content = try await SCShareableContent.current
        guard let display = content.displays.first else {
            throw VideoRecorderError.noDisplay
        }

        // Native pixel resolution = logical points × backing scale factor
        // for the display the captured content lives on. Fall back to
        // SCDisplay's own width/height (which are already in pixels)
        // when NSScreen lookup fails.
        let scale = Self.mainScreenScale()
        let pixelWidth = Int(CGFloat(display.width) * scale)
        let pixelHeight = Int(CGFloat(display.height) * scale)
        self.capturedWidth = pixelWidth
        self.capturedHeight = pixelHeight

        // --- AVAssetWriter setup -------------------------------------
        let writer = try AVAssetWriter(outputURL: outputURL, fileType: .mp4)
        // H.264 High profile with an explicit average bitrate. The
        // writer's `canAdd` rejects width/height-only settings on macOS
        // 14+, so we include a target bitrate + profile to get past
        // validation. Bitrate scales with pixel count (~4 bits per
        // pixel) — a reasonable screen-recording sweet spot that keeps
        // 4K / multi-display captures under control without making 1080p
        // look muddy. Floor at 2Mbps for tiny displays.
        let bitrate = max(2_000_000, pixelWidth * pixelHeight * 4)
        // Tag the video with Rec.709 color metadata. Without this key, the
        // encoder produces untagged H.264 and players fall back to assuming
        // Rec.601, which renders Display-P3 captures dark/muddy. Rec.709 is
        // also what Screen Studio / OBS / every mainstream screen-recorder
        // uses for Mac capture, so downstream editors get the expected pipe.
        let colorProperties: [String: Any] = [
            AVVideoColorPrimariesKey: AVVideoColorPrimaries_ITU_R_709_2,
            AVVideoTransferFunctionKey: AVVideoTransferFunction_ITU_R_709_2,
            AVVideoYCbCrMatrixKey: AVVideoYCbCrMatrix_ITU_R_709_2,
        ]
        let videoSettings: [String: Any] = [
            AVVideoCodecKey: AVVideoCodecType.h264,
            AVVideoWidthKey: pixelWidth,
            AVVideoHeightKey: pixelHeight,
            AVVideoColorPropertiesKey: colorProperties,
            AVVideoCompressionPropertiesKey: [
                AVVideoAverageBitRateKey: bitrate,
                AVVideoProfileLevelKey: AVVideoProfileLevelH264HighAutoLevel,
                AVVideoExpectedSourceFrameRateKey: 30,
            ] as [String: Any],
        ]
        let input = AVAssetWriterInput(
            mediaType: .video, outputSettings: videoSettings
        )
        input.expectsMediaDataInRealTime = true
        guard writer.canAdd(input) else {
            throw VideoRecorderError.assetWriterInputRejected
        }
        writer.add(input)

        // Pixel-buffer adaptor keeps the append path decoupled from
        // whatever format description SCStream attaches to each sample
        // buffer (clean-aperture / pixel-aspect-ratio attachments cause
        // the writer's encoder to reject direct `append(sampleBuffer)`
        // with AVError.unknown / -11800). The adaptor rewraps the raw
        // CVPixelBuffer into a sample the encoder always accepts.
        let adaptorAttrs: [String: Any] = [
            kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA,
            kCVPixelBufferWidthKey as String: pixelWidth,
            kCVPixelBufferHeightKey as String: pixelHeight,
        ]
        let adaptor = AVAssetWriterInputPixelBufferAdaptor(
            assetWriterInput: input,
            sourcePixelBufferAttributes: adaptorAttrs
        )

        guard writer.startWriting() else {
            let err = writer.error?.localizedDescription ?? "unknown error"
            throw VideoRecorderError.startWritingFailed(err)
        }

        // --- SCStream setup ------------------------------------------
        let filter = SCContentFilter(display: display, excludingWindows: [])
        let config = SCStreamConfiguration()
        config.width = pixelWidth
        config.height = pixelHeight
        config.pixelFormat = kCVPixelFormatType_32BGRA
        config.minimumFrameInterval = CMTime(value: 1, timescale: 30)
        config.queueDepth = 8
        config.showsCursor = false
        config.sourceRect = CGRect(
            x: 0, y: 0,
            width: CGFloat(display.width),
            height: CGFloat(display.height)
        )

        // StreamOutputHandler owns the writer + input so that the SCStream
        // callback queue can append samples synchronously with no actor
        // hop (CMSampleBuffer isn't Sendable under Swift 6 strict
        // concurrency). The actor keeps a strong reference to keep it
        // alive for the stream's lifetime.
        let handler = StreamOutputHandler(
            writer: writer, input: input, adaptor: adaptor
        )
        self.streamOutput = handler

        let sc = SCStream(filter: filter, configuration: config, delegate: handler)
        try sc.addStreamOutput(
            handler, type: .screen, sampleHandlerQueue: .global(qos: .userInitiated)
        )
        try await sc.startCapture()
        self.stream = sc

        log.info(
            "video capture started: \(self.outputURL.path, privacy: .public) \(pixelWidth, privacy: .public)x\(pixelHeight, privacy: .public)"
        )
    }

    /// Stop SCStream, mark the asset-writer input finished, and wait for
    /// the MP4 to finalize before returning. Safe to call from any state
    /// — repeated or from a partially-initialized session.
    ///
    /// Returns the final video metadata (width/height/frame_count) so
    /// `RecordingSession` can stamp it into `session.json` without a
    /// separate call (the handler is nil'd here, so querying later
    /// would return 0).
    @discardableResult
    public func stop() async -> FinalMetadata {
        if let stream {
            do {
                try await stream.stopCapture()
            } catch {
                log.error(
                    "stopCapture failed: \(error.localizedDescription, privacy: .public)"
                )
            }
        }
        self.stream = nil

        let finalCount: Int
        if let handler = streamOutput {
            await handler.finalize(logURL: outputURL, log: log)
            finalCount = handler.currentFrameCount()
        } else {
            finalCount = 0
        }
        self.streamOutput = nil
        return FinalMetadata(
            width: capturedWidth,
            height: capturedHeight,
            frameCount: finalCount
        )
    }

    // MARK: - Helpers

    /// Backing scale factor of the main screen. Defaults to 2.0 on
    /// retina-only machines, 1.0 elsewhere. Looked up off the main
    /// actor to avoid hopping just for a cached value.
    private static func mainScreenScale() -> CGFloat {
        NSScreen.main?.backingScaleFactor ?? 2.0
    }
}

/// Owns the `AVAssetWriter` + input and receives SCStream samples on the
/// stream's sample-handler queue. All per-frame state lives here and is
/// guarded by `lock`; the owning `VideoRecorder` actor treats it as an
/// opaque lifecycle handle and only reaches in at start/finalize time.
private final class StreamOutputHandler: NSObject, SCStreamOutput, SCStreamDelegate, @unchecked Sendable {
    private let writer: AVAssetWriter
    private let input: AVAssetWriterInput
    private let adaptor: AVAssetWriterInputPixelBufferAdaptor
    private let lock = NSLock()
    /// Set on the first appended sample so `startSession(atSourceTime:)`
    /// fires exactly once with the actual first-frame PTS (vs `.zero`,
    /// which would leave the first second of wall-clock time as blank
    /// pre-roll in the MP4).
    private var sessionStarted = false
    /// Flipped true once `finalize` runs so any in-flight sample from the
    /// SCStream queue after `stopCapture` won't try to append into a
    /// finishing writer.
    private var finishing = false
    /// Count of successfully-appended frames. Queried by the owning
    /// `VideoRecorder` after finalize to stamp `session.json`.
    private var appendedFrameCount: Int = 0

    private let log = Logger(
        subsystem: "com.trycua.driver",
        category: "VideoRecorder"
    )

    init(
        writer: AVAssetWriter,
        input: AVAssetWriterInput,
        adaptor: AVAssetWriterInputPixelBufferAdaptor
    ) {
        self.writer = writer
        self.input = input
        self.adaptor = adaptor
    }

    // MARK: - SCStreamOutput

    func stream(
        _ stream: SCStream,
        didOutputSampleBuffer sampleBuffer: CMSampleBuffer,
        of type: SCStreamOutputType
    ) {
        guard type == .screen else { return }
        guard CMSampleBufferIsValid(sampleBuffer) else { return }
        // SCStream delivers status-bearing buffers with no image payload
        // (e.g. `.idle` when the screen isn't changing). They carry no
        // samples — filter on sample count.
        guard CMSampleBufferGetNumSamples(sampleBuffer) > 0 else { return }

        // Only accept sample buffers whose SCFrameStatus attachment is
        // `.complete` — SCStream stamps `.idle` / `.blank` on keep-alive
        // buffers that have a valid sample count but no image payload,
        // and appending those trips the encoder into AVError.unknown.
        if let attachments = CMSampleBufferGetSampleAttachmentsArray(
            sampleBuffer, createIfNecessary: false
        ) as? [[SCStreamFrameInfo: Any]],
           let first = attachments.first,
           let statusRaw = first[.status] as? Int,
           statusRaw != SCFrameStatus.complete.rawValue
        {
            return
        }

        guard let pixelBuffer = CMSampleBufferGetImageBuffer(sampleBuffer) else {
            return
        }

        lock.lock()
        defer { lock.unlock() }

        if finishing { return }

        let pts = CMSampleBufferGetPresentationTimeStamp(sampleBuffer)

        if !sessionStarted {
            writer.startSession(atSourceTime: pts)
            sessionStarted = true
        }

        guard input.isReadyForMoreMediaData else {
            // Rare under 30fps + realtime flag, but possible during
            // startup. Drop — trying to buffer here invites unbounded
            // growth and we already have no-audio, no-sync concerns.
            return
        }
        if !adaptor.append(pixelBuffer, withPresentationTime: pts) {
            let nsErr = writer.error as NSError?
            let err = nsErr?.localizedDescription ?? "unknown"
            let code = nsErr?.code ?? -1
            log.error(
                "adaptor.append failed: status=\(self.writer.status.rawValue, privacy: .public) code=\(code, privacy: .public) err=\(err, privacy: .public)"
            )
        } else {
            appendedFrameCount += 1
        }
    }

    /// Read the current appended-frame count. Lock-guarded because SCStream
    /// delivers on its own queue; the owning actor calls this after
    /// finalize so there's no live writer competing for the lock.
    func currentFrameCount() -> Int {
        lock.withLock { appendedFrameCount }
    }

    // MARK: - SCStreamDelegate

    func stream(_ stream: SCStream, didStopWithError error: Error) {
        // Log only — the recorder's `stop()` is the cleanup point. The
        // most common trigger here is the user revoking Screen Recording
        // mid-session, in which case SCStream tears itself down and we
        // want the trajectory pipeline to keep running.
        log.error(
            "SCStream stopped with error: \(error.localizedDescription, privacy: .public)"
        )
    }

    // MARK: - Finalize

    /// Mark the input finished and wait for the asset writer to flush
    /// the MP4 to disk. Called once from `VideoRecorder.stop()` after
    /// SCStream has been told to stop; taking the lock ensures no
    /// sample append races the `markAsFinished` call.
    func finalize(logURL: URL, log: Logger) async {
        let started: Bool = lock.withLock {
            let wasStarted = sessionStarted
            finishing = true
            if wasStarted {
                input.markAsFinished()
            }
            return wasStarted
        }

        guard started else {
            log.info("video capture stopped before any frame arrived; skipping finalize")
            return
        }

        await withCheckedContinuation { (cont: CheckedContinuation<Void, Never>) in
            writer.finishWriting {
                cont.resume()
            }
        }
        if writer.status == .completed {
            log.info("video capture finalized: \(logURL.path, privacy: .public)")
        } else {
            let err = writer.error?.localizedDescription
                ?? "status=\(writer.status.rawValue)"
            log.error("video capture finalize failed: \(err, privacy: .public)")
        }
    }
}

public enum VideoRecorderError: Error, CustomStringConvertible, Sendable {
    case noDisplay
    case assetWriterInputRejected
    case startWritingFailed(String)

    public var description: String {
        switch self {
        case .noDisplay:
            return "no main display available for video capture"
        case .assetWriterInputRejected:
            return "AVAssetWriter rejected the video input configuration"
        case .startWritingFailed(let msg):
            return "AVAssetWriter.startWriting failed: \(msg)"
        }
    }
}
