// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AVFoundation
import CoreMedia
import CuaSpacesStreaming
import Foundation
import os
import VideoToolbox

/// Hardware H.264 decode for one tile, straight into its display layer.
///
/// Runs entirely off the main thread: `decode` is called on the SDK's frame
/// delivery thread, VideoToolbox decodes asynchronously on its own threads,
/// and the output callback enqueues the decoded `CVPixelBuffer` (NV12,
/// IOSurface-backed) into the layer's `AVSampleBufferVideoRenderer`, which
/// is safe to feed from any thread. SwiftUI never sees a frame.
///
/// The Annex B to AVCC rewrite reuses the SDK's `H264Decoder` helpers; this
/// type differs from `H264Decoder` in three ways the canvas needs: it
/// requires the hardware decoder, it emits NV12 rather than BGRA (a third of
/// the memory traffic with nothing to convert), and it can be told to stop
/// decoding (a paused tile) and then resume at the next keyframe.
public final class TileDecoder: @unchecked Sendable {
    public let renderer: AVSampleBufferVideoRenderer
    /// Called when the decoder needs an IDR (paused, lost references, a new
    /// codec epoch without one).
    public var onNeedsKeyframe: (@Sendable () -> Void)?
    /// Called with the decoded frame size when it changes.
    public var onSize: (@Sendable (CGSize) -> Void)?

    /// Guards the decode state. Never taken by the output callback:
    /// `rebuildLocked` waits for in-flight frames while holding it.
    private let lock = NSRecursiveLock()
    /// Guards the output side (`present`).
    private let outLock = NSLock()
    private var session: VTDecompressionSession?
    private var format: CMVideoFormatDescription?
    private var sps: Data?
    private var pps: Data?
    private var epoch: UInt64 = .max
    private var haveKeyframe = false
    private var enabled = true
    private var lastSize: CGSize = .zero
    private var outputFormat: CMVideoFormatDescription?

    public let counters = DecodeCounters()
    private let signposter: OSSignposter
    private let label: String

    public init(renderer: AVSampleBufferVideoRenderer, label: String) {
        self.renderer = renderer
        self.label = label
        signposter = OSSignposter(subsystem: Signposts.subsystem, category: "decode")
    }

    deinit {
        if let session {
            VTDecompressionSessionWaitForAsynchronousFrames(session)
            VTDecompressionSessionInvalidate(session)
        }
    }

    /// Stop or resume decoding. Resuming waits for a keyframe (the references
    /// are stale) and asks for one.
    public func setEnabled(_ on: Bool) {
        lock.lock()
        let changed = enabled != on
        enabled = on
        if on && changed { haveKeyframe = false }
        lock.unlock()
        if on && changed { onNeedsKeyframe?() }
    }

    /// The last decoded frame's size in pixels.
    public var lastDecodedSize: CGSize {
        outLock.lock(); defer { outLock.unlock() }
        return lastSize
    }

    public var isEnabled: Bool {
        lock.lock(); defer { lock.unlock() }
        return enabled
    }

    /// Feed one access unit (Annex B, one AU per call).
    public func decode(_ payload: Data, keyframe: Bool, codecEpoch: UInt64, ptsMicros: UInt64) {
        counters.received(bytes: payload.count)
        lock.lock()
        guard enabled else {
            lock.unlock()
            counters.skipped()
            return
        }
        if codecEpoch != epoch {
            teardownLocked()
            epoch = codecEpoch
            haveKeyframe = false
        }
        let nals = H264Decoder.splitAnnexB(payload)
        var samples: [Data] = []
        var parameterSetsChanged = false
        for nal in nals {
            guard let first = nal.first else { continue }
            switch first & 0x1F {
            case 7: if sps != nal { sps = nal; parameterSetsChanged = true }
            case 8: if pps != nal { pps = nal; parameterSetsChanged = true }
            case 9, 12: continue
            default: samples.append(nal)
            }
        }
        if parameterSetsChanged { rebuildLocked() }
        if keyframe { haveKeyframe = true }
        guard haveKeyframe else {
            lock.unlock()
            counters.droppedBeforeKeyframe()
            onNeedsKeyframe?()
            return
        }
        guard !samples.isEmpty, let session, let format,
              let block = H264Decoder.makeAVCCBlockBuffer(from: samples),
              let sample = H264Decoder.makeSampleBuffer(blockBuffer: block, formatDescription: format,
                                                        timestampMicroseconds: ptsMicros)
        else {
            lock.unlock()
            return
        }
        let id = signposter.makeSignpostID()
        let state = signposter.beginInterval("decode", id: id)
        let started = DispatchTime.now().uptimeNanoseconds
        var info = VTDecodeInfoFlags()
        let status = VTDecompressionSessionDecodeFrame(
            session, sampleBuffer: sample, flags: [._EnableAsynchronousDecompression],
            infoFlagsOut: &info
        ) { [weak self] status, _, image, pts, _ in
            guard let self else { return }
            self.signposter.endInterval("decode", state)
            guard status == noErr, let image else {
                self.counters.failed()
                self.onNeedsKeyframe?()
                return
            }
            self.counters.decoded(nanos: DispatchTime.now().uptimeNanoseconds - started)
            self.present(image, pts: pts)
        }
        if status != noErr {
            haveKeyframe = false
            rebuildLocked()
            lock.unlock()
            counters.failed()
            signposter.endInterval("decode", state)
            onNeedsKeyframe?()
            return
        }
        lock.unlock()
    }

    private func present(_ image: CVImageBuffer, pts: CMTime) {
        let size = CGSize(width: CVPixelBufferGetWidth(image), height: CVPixelBufferGetHeight(image))
        outLock.lock()
        let sizeChanged = size != lastSize
        if sizeChanged { lastSize = size }
        var fmt = outputFormat
        if fmt == nil || sizeChanged || !CMVideoFormatDescriptionMatchesImageBuffer(fmt!, imageBuffer: image) {
            var created: CMVideoFormatDescription?
            CMVideoFormatDescriptionCreateForImageBuffer(allocator: kCFAllocatorDefault,
                                                         imageBuffer: image, formatDescriptionOut: &created)
            outputFormat = created
            fmt = created
        }
        outLock.unlock()
        if sizeChanged { onSize?(size) }
        guard let fmt else { return }
        var timing = CMSampleTimingInfo(duration: .invalid, presentationTimeStamp: pts, decodeTimeStamp: .invalid)
        var buffer: CMSampleBuffer?
        guard CMSampleBufferCreateReadyWithImageBuffer(allocator: kCFAllocatorDefault, imageBuffer: image,
                                                       formatDescription: fmt, sampleTiming: &timing,
                                                       sampleBufferOut: &buffer) == noErr,
              let buffer else { return }
        if let attachments = CMSampleBufferGetSampleAttachmentsArray(buffer, createIfNecessary: true),
           CFArrayGetCount(attachments) > 0 {
            let dict = unsafeBitCast(CFArrayGetValueAtIndex(attachments, 0), to: CFMutableDictionary.self)
            CFDictionarySetValue(dict, Unmanaged.passUnretained(kCMSampleAttachmentKey_DisplayImmediately).toOpaque(),
                                 Unmanaged.passUnretained(kCFBooleanTrue).toOpaque())
        }
        if renderer.status == .failed { renderer.flush() }
        renderer.enqueue(buffer)
        counters.presented()
    }

    private func teardownLocked() {
        if let session {
            VTDecompressionSessionWaitForAsynchronousFrames(session)
            VTDecompressionSessionInvalidate(session)
        }
        session = nil
        format = nil
        sps = nil
        pps = nil
    }

    private func rebuildLocked() {
        if let session {
            VTDecompressionSessionWaitForAsynchronousFrames(session)
            VTDecompressionSessionInvalidate(session)
        }
        session = nil
        format = nil
        guard let sps, let pps else { return }
        var description: CMFormatDescription?
        let made: OSStatus = sps.withUnsafeBytes { s in
            pps.withUnsafeBytes { p in
                let pointers = [s.bindMemory(to: UInt8.self).baseAddress!, p.bindMemory(to: UInt8.self).baseAddress!]
                let sizes = [sps.count, pps.count]
                return pointers.withUnsafeBufferPointer { pb in
                    sizes.withUnsafeBufferPointer { sb in
                        CMVideoFormatDescriptionCreateFromH264ParameterSets(
                            allocator: kCFAllocatorDefault, parameterSetCount: 2,
                            parameterSetPointers: pb.baseAddress!, parameterSetSizes: sb.baseAddress!,
                            nalUnitHeaderLength: 4, formatDescriptionOut: &description)
                    }
                }
            }
        }
        guard made == noErr, let description else { return }
        format = description
        let spec: [String: Any] = [
            kVTVideoDecoderSpecification_RequireHardwareAcceleratedVideoDecoder as String: true,
        ]
        let attributes: [String: Any] = [
            kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
            kCVPixelBufferIOSurfacePropertiesKey as String: [:] as [String: Any],
            kCVPixelBufferMetalCompatibilityKey as String: true,
        ]
        var created: VTDecompressionSession?
        var status = VTDecompressionSessionCreate(
            allocator: kCFAllocatorDefault, formatDescription: description,
            decoderSpecification: spec as CFDictionary, imageBufferAttributes: attributes as CFDictionary,
            outputCallback: nil, decompressionSessionOut: &created)
        if status != noErr {
            // No hardware decoder for this stream (unusual on Apple silicon):
            // fall back to software rather than show nothing, and count it.
            counters.softwareFallback()
            status = VTDecompressionSessionCreate(
                allocator: kCFAllocatorDefault, formatDescription: description,
                decoderSpecification: nil, imageBufferAttributes: attributes as CFDictionary,
                outputCallback: nil, decompressionSessionOut: &created)
        }
        guard status == noErr, let created else { return }
        VTSessionSetProperty(created, key: kVTDecompressionPropertyKey_RealTime, value: kCFBooleanTrue)
        session = created
        var hw: CFBoolean?
        if VTSessionCopyProperty(created, key: kVTDecompressionPropertyKey_UsingHardwareAcceleratedVideoDecoder,
                                 allocator: kCFAllocatorDefault, valueOut: &hw) == noErr {
            counters.setHardware(hw == kCFBooleanTrue)
        }
    }
}

/// Thread-safe counters for one tile's decode path.
public final class DecodeCounters: @unchecked Sendable {
    private let lock = NSLock()
    private var _received = 0
    private var _bytes = 0
    private var _skipped = 0
    private var _dropped = 0
    private var _decoded = 0
    private var _presented = 0
    private var _failed = 0
    private var _fallbacks = 0
    private var _hardware: Bool?
    private var _decodeNanos: [UInt64] = []

    public struct Snapshot: Sendable, Equatable, Codable {
        public var received: Int
        public var bytes: Int
        public var skipped: Int
        public var droppedBeforeKeyframe: Int
        public var decoded: Int
        public var presented: Int
        public var failed: Int
        public var softwareFallbacks: Int
        public var hardware: Bool?
        public var decodeP50Ms: Double
        public var decodeP95Ms: Double
    }

    func received(bytes: Int) { lock.lock(); _received += 1; _bytes += bytes; lock.unlock() }
    func skipped() { lock.lock(); _skipped += 1; lock.unlock() }
    func droppedBeforeKeyframe() { lock.lock(); _dropped += 1; lock.unlock() }
    func failed() { lock.lock(); _failed += 1; lock.unlock() }
    func presented() { lock.lock(); _presented += 1; lock.unlock() }
    func softwareFallback() { lock.lock(); _fallbacks += 1; lock.unlock() }
    func setHardware(_ on: Bool) { lock.lock(); _hardware = on; lock.unlock() }
    func decoded(nanos: UInt64) {
        lock.lock()
        _decoded += 1
        if _decodeNanos.count < 50_000 { _decodeNanos.append(nanos) }
        lock.unlock()
    }

    public func snapshot() -> Snapshot {
        lock.lock(); defer { lock.unlock() }
        let s = _decodeNanos.sorted()
        func pct(_ p: Double) -> Double {
            guard !s.isEmpty else { return 0 }
            return Double(s[min(Int(p * Double(s.count - 1)), s.count - 1)]) / 1e6
        }
        return Snapshot(received: _received, bytes: _bytes, skipped: _skipped, droppedBeforeKeyframe: _dropped,
                        decoded: _decoded, presented: _presented, failed: _failed,
                        softwareFallbacks: _fallbacks, hardware: _hardware,
                        decodeP50Ms: pct(0.5), decodeP95Ms: pct(0.95))
    }
}

public enum Signposts {
    public static let subsystem = "ai.cua.infinite-canvas"
}
