// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CoreMedia
import CoreVideo
import Foundation
import VideoToolbox

/// Decodes RCDP's H.264 video packets into `CVPixelBuffer`s with VideoToolbox.
///
/// What the wire actually delivers (`rcdp/docs/protocol-v1.md`, "Sessions and
/// video", and `rcdp-cua-provider/src/macos_h264.rs`):
///
/// * **one Annex B access unit per video packet** — start-code delimited, not
///   AVCC, and with no length prefix inside the payload;
/// * **SPS/PPS in-band before every IDR**, so each keyframe is independently
///   decodable and there is no out-of-band `avcC` record to wait for;
/// * `codec_epoch` starting at 1 and advancing whenever the encoder is replaced
///   (a resize, or encoder recovery). The first frame of a new epoch is always a
///   keyframe, and decoder state from the previous epoch is invalid.
///
/// VideoToolbox wants the opposite of all three: a `CMFormatDescription` built
/// from parameter sets up front, and AVCC (4-byte big-endian length prefixed)
/// samples. This type is the adapter: it splits the access unit, harvests
/// SPS/PPS, rebuilds the format description whenever the parameter sets change,
/// rewrites the remaining NAL units as AVCC, and feeds the session.
///
/// It also refuses to emit anything before the first keyframe of the current
/// epoch — decoding a P-frame against a dead reference is the classic way to get
/// a stream that *looks* connected and renders green mush or black.
///
/// Thread safety: `decode` runs on the SDK's delivery thread, VideoToolbox
/// calls back on its own queue, and the owner reads the counters and sets
/// the callbacks from the main actor. The decode pipeline is behind `lock`;
/// the counters and callbacks are behind `stateLock` (a separate lock, since
/// VideoToolbox may call back while `decode` holds `lock`).
public final class H264Decoder: @unchecked Sendable {
    private let stateLock = NSLock()
    private var state = SharedState()

    private struct SharedState {
        var onFrame: (@Sendable (CVPixelBuffer, VideoFrameDescriptor) -> Void)?
        var onNeedsKeyframe: (@Sendable () -> Void)?
        var decodedFrameCount = 0
        var droppedBeforeKeyframeCount = 0
        var decodeFailureCount = 0
        var lastError: OSStatus = noErr
    }

    private func withState<T>(_ body: (inout SharedState) -> T) -> T {
        stateLock.lock()
        defer { stateLock.unlock() }
        return body(&state)
    }

    public init() {}

    /// Called on the decoder's own queue with each successfully decoded frame.
    public var onFrame: (@Sendable (CVPixelBuffer, VideoFrameDescriptor) -> Void)? {
        get { withState { $0.onFrame } }
        set { withState { $0.onFrame = newValue } }
    }
    /// Called when dependency state is lost and the caller should ask the server
    /// for a keyframe. Throttled by the caller.
    public var onNeedsKeyframe: (@Sendable () -> Void)? {
        get { withState { $0.onNeedsKeyframe } }
        set { withState { $0.onNeedsKeyframe = newValue } }
    }

    public var decodedFrameCount: Int { withState { $0.decodedFrameCount } }
    public var droppedBeforeKeyframeCount: Int { withState { $0.droppedBeforeKeyframeCount } }
    public var decodeFailureCount: Int { withState { $0.decodeFailureCount } }
    public var lastError: OSStatus { withState { $0.lastError } }

    /// Counts a failure and returns the keyframe callback to run (outside the lock).
    private func failed(_ status: OSStatus) -> (@Sendable () -> Void)? {
        withState {
            $0.decodeFailureCount += 1
            $0.lastError = status
            return $0.onNeedsKeyframe
        }
    }

    private var session: VTDecompressionSession?
    private var formatDescription: CMFormatDescription?
    private var sps: Data?
    private var pps: Data?
    private var currentCodecEpoch: UInt64 = 0
    private var haveKeyframeThisEpoch = false
    private let lock = NSLock()

    deinit { teardown() }

    // MARK: - Entry point

    /// Feed one RCDP video packet. `payload` is the raw Annex B access unit.
    public func decode(payload: Data, descriptor: VideoFrameDescriptor) {
        lock.lock()
        defer { lock.unlock() }

        // A codec-epoch change invalidates the decoder outright. Nothing from the
        // previous epoch — not the format description, not the reference frames —
        // survives it.
        if descriptor.codec_epoch != currentCodecEpoch {
            teardownLocked()
            currentCodecEpoch = descriptor.codec_epoch
            haveKeyframeThisEpoch = false
        }

        let nals = Self.splitAnnexB(payload)
        guard !nals.isEmpty else { return }

        var parameterSetsChanged = false
        var samples: [Data] = []
        for nal in nals {
            guard let first = nal.first else { continue }
            switch first & 0x1F {
            case 7: // SPS
                if sps != nal { sps = nal; parameterSetsChanged = true }
            case 8: // PPS
                if pps != nal { pps = nal; parameterSetsChanged = true }
            case 9, 12: // access unit delimiter, filler — not worth a sample
                continue
            default:
                samples.append(nal)
            }
        }

        if parameterSetsChanged {
            rebuildSessionLocked()
        }

        if descriptor.keyframe {
            haveKeyframeThisEpoch = true
        }
        guard haveKeyframeThisEpoch else {
            // Decoding a dependent frame with no reference is how a second stream
            // window ends up rendering black while its overlay still tracks the
            // cursor. Drop it and ask for an IDR instead.
            let needs = withState {
                $0.droppedBeforeKeyframeCount += 1
                return $0.onNeedsKeyframe
            }
            needs?()
            return
        }
        guard !samples.isEmpty, let session, let formatDescription else { return }

        guard let blockBuffer = Self.makeAVCCBlockBuffer(from: samples),
              let sampleBuffer = Self.makeSampleBuffer(blockBuffer: blockBuffer,
                                                       formatDescription: formatDescription,
                                                       timestampMicroseconds: descriptor.capture_timestamp_us)
        else { return }

        var flagsOut = VTDecodeInfoFlags()
        let status = VTDecompressionSessionDecodeFrame(
            session,
            sampleBuffer: sampleBuffer,
            flags: [._EnableAsynchronousDecompression],
            infoFlagsOut: &flagsOut,
            outputHandler: { [weak self] status, _, imageBuffer, _, _ in
                guard let self else { return }
                guard status == noErr, let imageBuffer else {
                    self.failed(status)?()
                    return
                }
                let deliver = self.withState {
                    $0.decodedFrameCount += 1
                    return $0.onFrame
                }
                deliver?(imageBuffer, descriptor)
            })

        if status != noErr {
            let needs = failed(status)
            // A hard submission failure means the session is unusable; rebuild it
            // and wait for the next IDR rather than emitting corrupt frames.
            haveKeyframeThisEpoch = false
            rebuildSessionLocked()
            needs?()
        }
    }

    /// Discard all decoder state. The next keyframe re-establishes it.
    public func reset() {
        lock.lock()
        defer { lock.unlock() }
        teardownLocked()
        currentCodecEpoch = 0
        haveKeyframeThisEpoch = false
    }

    private func teardown() {
        lock.lock()
        defer { lock.unlock() }
        teardownLocked()
    }

    private func teardownLocked() {
        if let session {
            VTDecompressionSessionWaitForAsynchronousFrames(session)
            VTDecompressionSessionInvalidate(session)
        }
        session = nil
        formatDescription = nil
        sps = nil
        pps = nil
    }

    private func rebuildSessionLocked() {
        if let session {
            VTDecompressionSessionWaitForAsynchronousFrames(session)
            VTDecompressionSessionInvalidate(session)
        }
        session = nil
        formatDescription = nil

        guard let sps, let pps else { return }

        var description: CMFormatDescription?
        let created: OSStatus = sps.withUnsafeBytes { spsBuffer in
            pps.withUnsafeBytes { ppsBuffer in
                let pointers = [spsBuffer.bindMemory(to: UInt8.self).baseAddress!,
                                ppsBuffer.bindMemory(to: UInt8.self).baseAddress!]
                let sizes = [sps.count, pps.count]
                return pointers.withUnsafeBufferPointer { pointerBuffer in
                    sizes.withUnsafeBufferPointer { sizeBuffer in
                        CMVideoFormatDescriptionCreateFromH264ParameterSets(
                            allocator: kCFAllocatorDefault,
                            parameterSetCount: 2,
                            parameterSetPointers: pointerBuffer.baseAddress!,
                            parameterSetSizes: sizeBuffer.baseAddress!,
                            nalUnitHeaderLength: 4,
                            formatDescriptionOut: &description)
                    }
                }
            }
        }
        guard created == noErr, let description else {
            withState { $0.lastError = created }
            return
        }
        formatDescription = description

        // BGRA output keeps the render path trivially shareable between the
        // in-app view and the PiP window: both take the same CVPixelBuffer.
        let attributes: [String: Any] = [
            kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA,
            kCVPixelBufferIOSurfacePropertiesKey as String: [:] as CFDictionary,
            kCVPixelBufferMetalCompatibilityKey as String: true,
        ]
        var newSession: VTDecompressionSession?
        let status = VTDecompressionSessionCreate(
            allocator: kCFAllocatorDefault,
            formatDescription: description,
            decoderSpecification: nil,
            imageBufferAttributes: attributes as CFDictionary,
            outputCallback: nil,
            decompressionSessionOut: &newSession)
        guard status == noErr else {
            withState { $0.lastError = status }
            return
        }
        session = newSession
    }

    // MARK: - Annex B

    /// Split an Annex B access unit into its NAL units, stripped of start codes.
    /// Handles both 3-byte (`00 00 01`) and 4-byte (`00 00 00 01`) start codes,
    /// which the host encoder mixes freely.
    public static func splitAnnexB(_ data: Data) -> [Data] {
        var nals: [Data] = []
        let bytes = [UInt8](data)
        let count = bytes.count
        guard count > 3 else { return nals }

        func startCodeLength(at index: Int) -> Int? {
            if index + 3 < count, bytes[index] == 0, bytes[index + 1] == 0,
               bytes[index + 2] == 0, bytes[index + 3] == 1 { return 4 }
            if index + 2 < count, bytes[index] == 0, bytes[index + 1] == 0,
               bytes[index + 2] == 1 { return 3 }
            return nil
        }

        var index = 0
        var nalStart: Int?
        while index < count {
            if let length = startCodeLength(at: index) {
                if let start = nalStart, index > start {
                    nals.append(Data(bytes[start ..< index]))
                }
                index += length
                nalStart = index
            } else {
                index += 1
            }
        }
        if let start = nalStart, start < count {
            nals.append(Data(bytes[start ..< count]))
        }
        return nals.filter { !$0.isEmpty }
    }

    /// Rewrite NAL units as an AVCC elementary stream (4-byte big-endian length
    /// prefix per NAL), which is what `CMBlockBuffer` + VideoToolbox expect.
    public static func makeAVCCBlockBuffer(from nals: [Data]) -> CMBlockBuffer? {
        var avcc = Data()
        avcc.reserveCapacity(nals.reduce(0) { $0 + $1.count + 4 })
        for nal in nals {
            var length = UInt32(nal.count).bigEndian
            withUnsafeBytes(of: &length) { avcc.append(contentsOf: $0) }
            avcc.append(nal)
        }

        // CMBlockBufferCreateWithMemoryBlock does not copy, so hand it a block we
        // allocate and let it own — the Data would otherwise be gone by the time
        // the decoder reads it.
        let size = avcc.count
        let block = UnsafeMutableRawPointer.allocate(byteCount: size, alignment: 1)
        avcc.withUnsafeBytes { _ = memcpy(block, $0.baseAddress!, size) }

        var blockBuffer: CMBlockBuffer?
        let status = CMBlockBufferCreateWithMemoryBlock(
            allocator: kCFAllocatorDefault,
            memoryBlock: block,
            blockLength: size,
            blockAllocator: kCFAllocatorDefault, // frees `block` when the buffer dies
            customBlockSource: nil,
            offsetToData: 0,
            dataLength: size,
            flags: 0,
            blockBufferOut: &blockBuffer)
        guard status == kCMBlockBufferNoErr else {
            block.deallocate()
            return nil
        }
        return blockBuffer
    }

    public static func makeSampleBuffer(blockBuffer: CMBlockBuffer,
                                 formatDescription: CMFormatDescription,
                                 timestampMicroseconds: UInt64) -> CMSampleBuffer? {
        var sampleBuffer: CMSampleBuffer?
        var sampleSize = CMBlockBufferGetDataLength(blockBuffer)
        // The capture timestamp's origin is session-local; it orders frames but is
        // not a wall clock. Using it as the PTS keeps VideoToolbox's own ordering
        // sane without pretending it means anything else.
        var timing = CMSampleTimingInfo(
            duration: .invalid,
            presentationTimeStamp: CMTime(value: CMTimeValue(timestampMicroseconds), timescale: 1_000_000),
            decodeTimeStamp: .invalid)
        let status = CMSampleBufferCreateReady(
            allocator: kCFAllocatorDefault,
            dataBuffer: blockBuffer,
            formatDescription: formatDescription,
            sampleCount: 1,
            sampleTimingEntryCount: 1,
            sampleTimingArray: &timing,
            sampleSizeEntryCount: 1,
            sampleSizeArray: &sampleSize,
            sampleBufferOut: &sampleBuffer)
        return status == noErr ? sampleBuffer : nil
    }
}
