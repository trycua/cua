// Zoom-on-click renderer. Reads a recording directory produced by
// `cua-driver recording start … --video-experimental`, runs each video
// frame through `FrameTransform.transformedFrame` with the scale/focus
// curve from `ZoomRegionGenerator.sampleCurve`, and writes a new MP4.
//
// Pipeline:
//   AVAssetReader → CMSampleBuffer → CIImage
//      → FrameTransform.transformedFrame (crop+Lanczos scale)
//      → CIContext.render(into: CVPixelBuffer) (from adaptor's pool)
//      → AVAssetWriterInputPixelBufferAdaptor.append(…)
//
// Single-pass, no disk spill. The reader back-pressures on the writer's
// `isReadyForMoreMediaData` queue so memory stays flat.

import AVFoundation
import CoreImage
import CoreMedia
import CoreVideo
import Foundation
import Metal
import os

public enum RecordingRendererError: Error, CustomStringConvertible, Sendable {
    case assetLoadFailed(URL, String)
    case noVideoTrack(URL)
    case readerSetupFailed(String)
    case writerSetupFailed(String)
    case writerInputRejected
    case encodeFailed(String)
    case noPixelBufferPool
    case ciRenderFailed
    case readerFailed(String)

    public var description: String {
        switch self {
        case .assetLoadFailed(let url, let msg):
            return "failed to load asset at \(url.path): \(msg)"
        case .noVideoTrack(let url):
            return "no video track found in \(url.path)"
        case .readerSetupFailed(let msg):
            return "AVAssetReader setup failed: \(msg)"
        case .writerSetupFailed(let msg):
            return "AVAssetWriter setup failed: \(msg)"
        case .writerInputRejected:
            return "AVAssetWriter rejected the video input configuration"
        case .encodeFailed(let msg):
            return "encode failed: \(msg)"
        case .noPixelBufferPool:
            return "AVAssetWriterInputPixelBufferAdaptor.pixelBufferPool was nil"
        case .ciRenderFailed:
            return "CIContext.render(to:) failed"
        case .readerFailed(let msg):
            return "AVAssetReader failed mid-stream: \(msg)"
        }
    }
}

/// Progress callback signature. Called from the render loop on an
/// arbitrary worker thread; the CLI entry point formats for stderr.
public typealias RecordingRendererProgress = @Sendable (_ frameIndex: Int, _ tMs: Double) -> Void

public enum RecordingRenderer {
    /// Options that control a single render. Kept as a separate struct so
    /// future knobs (quality, codec, bitrate override) don't bloat the
    /// primary `render(from:to:)` signature.
    public struct Options: Sendable {
        /// When true, skip the zoom curve entirely — the output is a
        /// straight re-encode of the input. Useful as a baseline sanity
        /// check (same resolution, same duration, no visual change).
        public let noZoom: Bool
        /// Scale factor passed through to `ZoomRegionGenerator.generate`.
        /// The default matches the math library's (2.0).
        public let defaultScale: Double
        /// How often to invoke `progress`. Interpreted as video-time
        /// milliseconds, not wall-clock; a 30fps input with
        /// `progressIntervalMs = 1000` means "every ~30 frames".
        public let progressIntervalMs: Double
        /// When true (default), use action spans from the trajectory to
        /// drive variable-speed rendering (1× inside spans, 5× outside)
        /// and window-bbox zoom. Falls back to legacy click-zoom at 1×
        /// speed when no action spans are present in the recording.
        public let enableSpeedZones: Bool

        public init(
            noZoom: Bool = false,
            defaultScale: Double = 2.0,
            progressIntervalMs: Double = 1000,
            enableSpeedZones: Bool = true
        ) {
            self.noZoom = noZoom
            self.defaultScale = defaultScale
            self.progressIntervalMs = progressIntervalMs
            self.enableSpeedZones = enableSpeedZones
        }
    }

    private static let log = Logger(
        subsystem: "com.trycua.driver",
        category: "RecordingRenderer"
    )

    /// Render a recording directory → output MP4. Throws on any fatal
    /// I/O / AVFoundation setup error. Returns the number of frames
    /// appended to the output on success.
    @discardableResult
    public static func render(
        from inputDirectory: URL,
        to outputURL: URL,
        options: Options = Options(),
        progress: RecordingRendererProgress? = nil
    ) async throws -> Int {
        // --- Load trajectory + session metadata --------------------------
        let (metadata, clicks, cursorSamples) = try TrajectoryLoader.load(from: inputDirectory)
        let rawActionSpans = TrajectoryLoader.loadActionSpans(from: inputDirectory)
        let actionSpans = options.enableSpeedZones && !rawActionSpans.isEmpty
            ? ActionSpanGenerator.generate(from: rawActionSpans)
            : []

        let videoURL = inputDirectory.appendingPathComponent("recording.mp4")

        // --- Reader setup ------------------------------------------------
        let asset = AVURLAsset(url: videoURL)
        let tracks: [AVAssetTrack]
        do {
            tracks = try await asset.loadTracks(withMediaType: .video)
        } catch {
            throw RecordingRendererError.assetLoadFailed(videoURL, error.localizedDescription)
        }
        guard let track = tracks.first else {
            throw RecordingRendererError.noVideoTrack(videoURL)
        }
        let naturalSize: CGSize
        let nominalFrameRate: Float
        do {
            naturalSize = try await track.load(.naturalSize)
            nominalFrameRate = try await track.load(.nominalFrameRate)
        } catch {
            throw RecordingRendererError.assetLoadFailed(videoURL, error.localizedDescription)
        }

        // Prefer `session.json`'s width/height for the output — they're
        // the authoritative capture dimensions. Fall back to the track's
        // natural size when the JSON is older-schema.
        let outputWidth = metadata.videoWidth > 0 ? metadata.videoWidth : Int(naturalSize.width)
        let outputHeight = metadata.videoHeight > 0 ? metadata.videoHeight : Int(naturalSize.height)
        let frameSize = CGSize(width: outputWidth, height: outputHeight)
        // Use the recorded display scale factor to convert screen points → video
        // pixels. Defaults to 1.0 for older recordings without this field.
        let pointsToPixels = metadata.displayScaleFactor > 0 ? metadata.displayScaleFactor : 1.0

        // --- Build zoom regions -------------------------------------------
        // When action spans are present (new recordings with window_bounds),
        // derive zoom regions from them: each span zooms to its target
        // window with eased in/out transitions. Otherwise fall back to the
        // legacy click-event zoom.
        let regions: [ZoomRegion]
        if options.noZoom {
            regions = []
        } else if !actionSpans.isEmpty {
            regions = zoomRegions(from: actionSpans, frameSize: frameSize,
                                  pointsToPixels: pointsToPixels, defaultScale: options.defaultScale)
        } else {
            regions = ZoomRegionGenerator.generate(
                clicks: clicks,
                cursorSamples: cursorSamples,
                defaultScale: options.defaultScale
            )
        }

        let reader: AVAssetReader
        do {
            reader = try AVAssetReader(asset: asset)
        } catch {
            throw RecordingRendererError.readerSetupFailed(error.localizedDescription)
        }
        let readerOutputSettings: [String: Any] = [
            kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA,
        ]
        let readerOutput = AVAssetReaderTrackOutput(
            track: track, outputSettings: readerOutputSettings
        )
        readerOutput.alwaysCopiesSampleData = false
        guard reader.canAdd(readerOutput) else {
            throw RecordingRendererError.readerSetupFailed("canAdd(readerOutput) == false")
        }
        reader.add(readerOutput)

        // --- Writer setup ------------------------------------------------
        // Overwrite any prior output at the target path. Matches the
        // capture pipeline's "start clean" behavior so repeated renders
        // don't accumulate stale bytes.
        try? FileManager.default.removeItem(at: outputURL)
        let writer: AVAssetWriter
        do {
            writer = try AVAssetWriter(outputURL: outputURL, fileType: .mp4)
        } catch {
            throw RecordingRendererError.writerSetupFailed(error.localizedDescription)
        }

        // Bitrate heuristic mirrors VideoRecorder's (~4 bits/pixel,
        // 2Mbps floor). Matching the capture profile keeps re-renders
        // visually consistent with the input.
        let bitrate = max(2_000_000, outputWidth * outputHeight * 4)
        let expectedFrameRate = nominalFrameRate > 0 ? Int(nominalFrameRate.rounded()) : 30
        // Mirror the capture pipeline's Rec.709 color tagging so the
        // re-encoded file stays interpretable by the same players that
        // render the capture correctly. Without this, a dark gamma shift
        // creeps in on re-render.
        let colorProperties: [String: Any] = [
            AVVideoColorPrimariesKey: AVVideoColorPrimaries_ITU_R_709_2,
            AVVideoTransferFunctionKey: AVVideoTransferFunction_ITU_R_709_2,
            AVVideoYCbCrMatrixKey: AVVideoYCbCrMatrix_ITU_R_709_2,
        ]
        let videoSettings: [String: Any] = [
            AVVideoCodecKey: AVVideoCodecType.h264,
            AVVideoWidthKey: outputWidth,
            AVVideoHeightKey: outputHeight,
            AVVideoColorPropertiesKey: colorProperties,
            AVVideoCompressionPropertiesKey: [
                AVVideoAverageBitRateKey: bitrate,
                AVVideoProfileLevelKey: AVVideoProfileLevelH264HighAutoLevel,
                AVVideoExpectedSourceFrameRateKey: expectedFrameRate,
            ] as [String: Any],
        ]
        let writerInput = AVAssetWriterInput(
            mediaType: .video, outputSettings: videoSettings
        )
        writerInput.expectsMediaDataInRealTime = false
        guard writer.canAdd(writerInput) else {
            throw RecordingRendererError.writerInputRejected
        }
        writer.add(writerInput)

        let adaptorAttrs: [String: Any] = [
            kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA,
            kCVPixelBufferWidthKey as String: outputWidth,
            kCVPixelBufferHeightKey as String: outputHeight,
        ]
        let adaptor = AVAssetWriterInputPixelBufferAdaptor(
            assetWriterInput: writerInput,
            sourcePixelBufferAttributes: adaptorAttrs
        )

        // --- Kick both sides off -----------------------------------------
        guard reader.startReading() else {
            let err = reader.error?.localizedDescription ?? "unknown"
            throw RecordingRendererError.readerSetupFailed("startReading failed: \(err)")
        }
        guard writer.startWriting() else {
            let err = writer.error?.localizedDescription ?? "unknown"
            throw RecordingRendererError.writerSetupFailed("startWriting failed: \(err)")
        }
        writer.startSession(atSourceTime: .zero)

        // --- Coordinate conversion factor --------------------------------
        // Focus coordinates come back from `sampleCurve` in cursor-space
        // screen points; video frames are in display-native pixels. v1
        // assumes a Retina scale factor of 2.0 (the default on modern
        // Macs). If the display is non-Retina the user ends up with a
        // focus offset that's off by 2× — acceptable for a preview and
        // documented as the first follow-up.
        //
        // `pointsToPixels` is now read from `session.json` (set above from
        // `metadata.displayScaleFactor`). Kept as a local so the render loop
        // captures it without re-reading the outer `pointsToPixels` binding.

        // --- Render loop -------------------------------------------------
        // Pin CIContext color pipeline to sRGB so the CoreImage transform
        // doesn't apply a color-space conversion we didn't ask for. Paired
        // with the Rec.709 tags on the writer input, the full path stays
        // tagged end-to-end (source → CIImage → pixel buffer → MP4).
        let srgb = CGColorSpace(name: CGColorSpace.sRGB) ?? CGColorSpaceCreateDeviceRGB()
        // Use a Metal-backed CIContext to keep all frame processing on the GPU,
        // eliminating the CPU↔GPU round-trips that cause laggy zoom transitions.
        let ciContext: CIContext
        if let metalDevice = MTLCreateSystemDefaultDevice() {
            ciContext = CIContext(mtlDevice: metalDevice, options: [
                .workingColorSpace: srgb,
                .outputColorSpace: srgb,
            ])
        } else {
            ciContext = CIContext(options: [
                .useSoftwareRenderer: false,
                .workingColorSpace: srgb,
                .outputColorSpace: srgb,
            ])
        }

        // Hand-off queue the writer expects its input to be fed from.
        // Realtime flag is off (we're batching), so we rely on
        // `requestMediaDataWhenReady` to back-pressure.
        let writerQueue = DispatchQueue(label: "com.trycua.driver.recording.render")

        let renderState = RenderLoopState(
            reader: reader,
            readerOutput: readerOutput,
            writer: writer,
            writerInput: writerInput,
            adaptor: adaptor,
            ciContext: ciContext,
            regions: regions,
            cursorSamples: cursorSamples,
            frameSize: frameSize,
            pointsToPixels: pointsToPixels,
            actionSpans: actionSpans,
            progress: progress,
            progressIntervalMs: options.progressIntervalMs
        )

        let appended = try await withCheckedThrowingContinuation {
            (cont: CheckedContinuation<Int, Error>) in
            writerInput.requestMediaDataWhenReady(on: writerQueue) {
                renderState.pumpUntilBlocked(continuation: cont)
            }
        }

        return appended
    }
}

/// Mutable state for the streaming pump. Lives outside the `render`
/// function so the `requestMediaDataWhenReady` block can resume it on
/// every writer-ready callback without recapturing locals. Not thread-safe
/// on its own — the pump runs serially on the writer's queue.
private final class RenderLoopState: @unchecked Sendable {
    private let reader: AVAssetReader
    private let readerOutput: AVAssetReaderTrackOutput
    private let writer: AVAssetWriter
    private let writerInput: AVAssetWriterInput
    private let adaptor: AVAssetWriterInputPixelBufferAdaptor
    private let ciContext: CIContext
    private let regions: [ZoomRegion]
    private let cursorSamples: [CursorSample]
    private let frameSize: CGSize
    private let pointsToPixels: Double
    /// Merged, padded action spans used for variable-speed + window zoom.
    /// Empty when the recording pre-dates window_bounds / speed-zone support.
    private let actionSpans: [ActionSpan]
    private let progress: RecordingRendererProgress?
    private let progressIntervalMs: Double

    private var frameIndex: Int = 0
    private var finished: Bool = false
    private var lastProgressEmittedMs: Double = -Double.infinity

    private let log = Logger(
        subsystem: "com.trycua.driver",
        category: "RecordingRenderer"
    )

    init(
        reader: AVAssetReader,
        readerOutput: AVAssetReaderTrackOutput,
        writer: AVAssetWriter,
        writerInput: AVAssetWriterInput,
        adaptor: AVAssetWriterInputPixelBufferAdaptor,
        ciContext: CIContext,
        regions: [ZoomRegion],
        cursorSamples: [CursorSample],
        frameSize: CGSize,
        pointsToPixels: Double,
        actionSpans: [ActionSpan],
        progress: RecordingRendererProgress?,
        progressIntervalMs: Double
    ) {
        self.reader = reader
        self.readerOutput = readerOutput
        self.writer = writer
        self.writerInput = writerInput
        self.adaptor = adaptor
        self.ciContext = ciContext
        self.regions = regions
        self.cursorSamples = cursorSamples
        self.frameSize = frameSize
        self.pointsToPixels = pointsToPixels
        self.actionSpans = actionSpans
        self.progress = progress
        self.progressIntervalMs = progressIntervalMs
    }

    /// Drain as many frames as the writer will accept, then return so
    /// `requestMediaDataWhenReady` re-invokes us on the next drain. On
    /// reader EOF or hard error, finalize the writer and resume the
    /// continuation with the final frame count (or an error).
    func pumpUntilBlocked(
        continuation: CheckedContinuation<Int, Error>
    ) {
        if finished { return }

        while writerInput.isReadyForMoreMediaData {
            guard let sampleBuffer = readerOutput.copyNextSampleBuffer() else {
                // End of stream (or reader error — check status below).
                finished = true
                writerInput.markAsFinished()

                if reader.status == .failed {
                    let err = reader.error?.localizedDescription ?? "unknown"
                    continuation.resume(
                        throwing: RecordingRendererError.readerFailed(err)
                    )
                    return
                }

                let frameCountAtFinish = frameIndex
                // `AVAssetWriter` is not `Sendable` under Swift 6, so
                // escaping it into the `finishWriting` @Sendable
                // closure below produces a warning the project-level
                // `@unchecked Sendable` on `RenderLoopState` can't
                // silence. The capture is safe here: `self.writer`
                // is touched only at end-of-render on a single
                // actor-serialized path, and `finishWriting` is
                // AVFoundation's own completion callback — the
                // framework owns the cross-thread transition.
                nonisolated(unsafe) let capturedWriter = self.writer
                capturedWriter.finishWriting { [log] in
                    if capturedWriter.status == .completed {
                        continuation.resume(returning: frameCountAtFinish)
                    } else {
                        let err = capturedWriter.error?.localizedDescription
                            ?? "status=\(capturedWriter.status.rawValue)"
                        log.error("writer finalize failed: \(err, privacy: .public)")
                        continuation.resume(
                            throwing: RecordingRendererError.encodeFailed(err)
                        )
                    }
                }
                return
            }

            let pts = CMSampleBufferGetPresentationTimeStamp(sampleBuffer)
            let tMs = CMTimeGetSeconds(pts) * 1000.0

            guard let pixelBuffer = CMSampleBufferGetImageBuffer(sampleBuffer) else {
                continue
            }

            do {
                try renderAndAppend(pixelBuffer: pixelBuffer, pts: pts, tMs: tMs)
            } catch {
                finished = true
                writerInput.markAsFinished()
                continuation.resume(throwing: error)
                return
            }

            frameIndex += 1
            if let progress, tMs - lastProgressEmittedMs >= progressIntervalMs {
                progress(frameIndex, tMs)
                lastProgressEmittedMs = tMs
            }
        }
        // Fell out of the ready loop — the writer is back-pressured.
        // `requestMediaDataWhenReady` will re-invoke us when it drains.
    }

    /// Transform one frame and append. Errors thrown here abort the
    /// pump and are propagated to the continuation.
    private func renderAndAppend(
        pixelBuffer: CVPixelBuffer,
        pts: CMTime,
        tMs: Double
    ) throws {
        // --- PTS remapping (variable speed) ----------------------------------
        // When action spans are available, remap the presentation timestamp
        // so segments inside spans play at 1× and segments outside play at
        // 5×. For 1×-span frames the PTS is shifted (no scale change); for
        // 5×-fast frames the PTS is compressed. The mapping is monotonically
        // increasing so the encoder always sees valid PTS order.
        let outPts: CMTime
        if !actionSpans.isEmpty {
            let outMs = ActionSpanGenerator.mapPts(tMs, spans: actionSpans)
            outPts = CMTimeMake(value: Int64(outMs.rounded()), timescale: 1000)
        } else {
            outPts = pts
        }

        // --- Zoom curve sampling ---------------------------------------------
        // Use the pre-built zoom regions (derived from action spans or legacy
        // clicks). `sampleCurve` returns eased (scale, focusX, focusY) in
        // screen-point space; multiply by pointsToPixels for FrameTransform.
        let curve = ZoomRegionGenerator.sampleCurve(
            atMs: tMs,
            regions: regions,
            cursorSamples: cursorSamples
        )
        let focusXPixels = curve.focusX * pointsToPixels
        let focusYPixels = curve.focusY * pointsToPixels

        let input = CIImage(cvPixelBuffer: pixelBuffer)
        let transformed = FrameTransform.transformedFrame(
            input,
            scale: curve.scale,
            focusX: focusXPixels,
            focusY: focusYPixels,
            frameSize: frameSize
        )

        guard let pool = adaptor.pixelBufferPool else {
            throw RecordingRendererError.noPixelBufferPool
        }
        var outBuffer: CVPixelBuffer?
        let poolStatus = CVPixelBufferPoolCreatePixelBuffer(nil, pool, &outBuffer)
        guard poolStatus == kCVReturnSuccess, let out = outBuffer else {
            throw RecordingRendererError.encodeFailed(
                "CVPixelBufferPoolCreatePixelBuffer status=\(poolStatus)"
            )
        }

        let renderRect = CGRect(origin: .zero, size: frameSize)
        // CoreImage expects the output's extent / orientation to match
        // the source's. We pinned the image to top-left origin in
        // FrameTransform; CIContext.render writes rows top-down for
        // BGRA destinations, so the output lands right-side up.
        ciContext.render(
            transformed,
            to: out,
            bounds: renderRect,
            colorSpace: nil
        )

        if !adaptor.append(out, withPresentationTime: outPts) {
            let err = writer.error?.localizedDescription ?? "unknown"
            throw RecordingRendererError.encodeFailed("adaptor.append failed: \(err)")
        }
    }
}

// MARK: - Span → ZoomRegion conversion

/// Build `ZoomRegion`s from merged action spans so the existing
/// `ZoomRegionGenerator.sampleCurve` machinery produces eased zoom
/// transitions to / from each window's bounding box.
///
/// - Spans without `windowBounds` are silently dropped (no window
///   to zoom into; the video stays at scale 1.0 during that span).
/// - Scale is computed as the largest factor that fits the window
///   inside the frame while maintaining aspect ratio (letterbox).
/// - Zoom-in and zoom-out transitions use 400 ms, which sits
///   comfortably inside the ±1 s padding ActionSpanGenerator adds.
private func zoomRegions(
    from spans: [ActionSpan],
    frameSize: CGSize,
    pointsToPixels: Double,
    defaultScale: Double = 2.0
) -> [ZoomRegion] {
    return spans.compactMap { span -> ZoomRegion? in
        // Waypoints are already in screen points (clickPoint / window centre);
        // no pointsToPixels here — the renderer multiplies at sample time.
        let waypoints = span.focusWaypoints

        // Try window-bbox fit first: largest scale that fits the window exactly.
        // Focus is the window centre in screen points; FrameTransform clamps the
        // crop to the video boundary, which centres the window as much as possible
        // without going off the edge of the display (letterbox case).
        if let wb = span.windowBounds {
            let winWPx = (wb.width + 32) * pointsToPixels
            let winHPx = (wb.height + 32) * pointsToPixels
            if winWPx > 0, winHPx > 0 {
                let scale = min(
                    Double(frameSize.width) / winWPx,
                    Double(frameSize.height) / winHPx
                )
                if scale > 1.0 {
                    let focusX = wb.x + wb.width / 2
                    let focusY = wb.y + wb.height / 2
                    // Don't pass waypoints here: scale exactly fits the window,
                    // so any focus deviation from the window centre clips an edge.
                    // focusX/Y (window centre) is already the right target.
                    return ZoomRegion(
                        startMs: span.startMs,
                        endMs: span.endMs,
                        focusX: focusX,
                        focusY: focusY,
                        scale: scale,
                        zoomInDurationMs: ZoomDefaults.zoomInWindowMs,
                        zoomOutDurationMs: ZoomDefaults.transitionWindowMs,
                        waypoints: nil
                    )
                }
            }
        }
        // Fallback: window absent or fills/exceeds frame. Zoom to click point.
        if let cp = span.clickPoint {
            return ZoomRegion(
                startMs: span.startMs,
                endMs: span.endMs,
                focusX: cp.x,
                focusY: cp.y,
                scale: defaultScale,
                zoomInDurationMs: ZoomDefaults.zoomInWindowMs,
                zoomOutDurationMs: ZoomDefaults.transitionWindowMs,
                waypoints: waypoints
            )
        }
        return nil
    }
}
