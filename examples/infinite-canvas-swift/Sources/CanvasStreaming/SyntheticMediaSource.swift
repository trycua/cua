// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CanvasModel
import CoreGraphics
import CoreMedia
import CoreVideo
import CuaSpacesStreaming
import Foundation
import VideoToolbox

/// A local H.264 test pattern for the benchmark and the tests.
///
/// It exercises exactly the path a Space's frames take after the network:
/// Annex B access units, keyframe gating, preference changes (fps and long
/// edge), keyframe requests. It is never used by the app when a Space is
/// attached, and the app labels it when it is.
public final class SyntheticMediaSource: MediaSource, @unchecked Sendable {
    public let size: CGSize
    public let seed: Int
    private let queue: DispatchQueue
    private let lock = NSLock()
    private var timer: DispatchSourceTimer?
    private var encoder: VTCompressionSession?
    private weak var sink: MediaSink?
    private var fps: UInt16 = 30
    private var maxDimension: UInt32 = 1280
    private var encodeSize: CGSize = .zero
    private var epoch: UInt64 = 1
    private var frameIndex: Int64 = 0
    private var forceKeyframe = true
    private var pool: CVPixelBufferPool?

    public init(size: CGSize = CGSize(width: 1280, height: 800), seed: Int = 0) {
        self.size = size
        self.seed = seed
        queue = DispatchQueue(label: "canvas.synthetic.\(seed)", qos: .utility)
    }

    public func start(_ sink: MediaSink) async throws {
        lock.withLock { self.sink = sink }
        queue.sync { self.reconfigureLocked() }
    }

    public func stop() async {
        queue.sync {
            timer?.cancel()
            timer = nil
            if let encoder {
                VTCompressionSessionCompleteFrames(encoder, untilPresentationTimeStamp: .invalid)
                VTCompressionSessionInvalidate(encoder)
            }
            encoder = nil
        }
        lock.withLock { sink = nil }
    }

    public func setPreferences(_ p: StreamPreferences) {
        queue.async {
            let changed = p.maxFps != self.fps || p.maxDimension != self.maxDimension
            self.fps = max(p.maxFps, 1)
            self.maxDimension = p.maxDimension
            if changed { self.reconfigureLocked() }
        }
    }

    public func requestKeyframe() {
        queue.async { self.forceKeyframe = true }
    }

    public func send(_ events: [InteractiveInputEvent]) {}

    /// No network, no latency to report.
    public func ping(nonce: UInt64) {}

    /// Fit the source into the long-edge cap, even dimensions.
    static func encodedSize(_ size: CGSize, maxDimension: UInt32) -> CGSize {
        let longEdge = max(size.width, size.height)
        let cap = maxDimension == 0 ? longEdge : min(longEdge, CGFloat(maxDimension))
        let s = cap / longEdge
        func even(_ v: CGFloat) -> CGFloat { max(2, (v * s / 2).rounded() * 2) }
        return CGSize(width: even(size.width), height: even(size.height))
    }

    private func reconfigureLocked() {
        let newSize = Self.encodedSize(size, maxDimension: maxDimension)
        if newSize != encodeSize || encoder == nil {
            if let encoder { VTCompressionSessionInvalidate(encoder) }
            encoder = nil
            encodeSize = newSize
            epoch += 1
            forceKeyframe = true
            var session: VTCompressionSession?
            let spec: [String: Any] = [
                kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder as String: true,
            ]
            VTCompressionSessionCreate(allocator: nil, width: Int32(newSize.width), height: Int32(newSize.height),
                                       codecType: kCMVideoCodecType_H264, encoderSpecification: spec as CFDictionary,
                                       imageBufferAttributes: nil, compressedDataAllocator: nil,
                                       outputCallback: nil, refcon: nil, compressionSessionOut: &session)
            if let session {
                VTSessionSetProperty(session, key: kVTCompressionPropertyKey_RealTime, value: kCFBooleanTrue)
                VTSessionSetProperty(session, key: kVTCompressionPropertyKey_AllowFrameReordering, value: kCFBooleanFalse)
                VTSessionSetProperty(session, key: kVTCompressionPropertyKey_ProfileLevel,
                                     value: kVTProfileLevel_H264_Main_AutoLevel)
                VTSessionSetProperty(session, key: kVTCompressionPropertyKey_MaxKeyFrameInterval, value: 120 as CFNumber)
                VTCompressionSessionPrepareToEncodeFrames(session)
            }
            encoder = session
            let attrs: [String: Any] = [
                kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA,
                kCVPixelBufferWidthKey as String: Int(newSize.width),
                kCVPixelBufferHeightKey as String: Int(newSize.height),
                kCVPixelBufferIOSurfacePropertiesKey as String: [:] as [String: Any],
            ]
            pool = nil
            CVPixelBufferPoolCreate(nil, nil, attrs as CFDictionary, &pool)
        }
        timer?.cancel()
        let t = DispatchSource.makeTimerSource(queue: queue)
        t.schedule(deadline: .now(), repeating: .nanoseconds(Int(1_000_000_000 / Int(max(fps, 1)))),
                   leeway: .milliseconds(1))
        t.setEventHandler { [weak self] in self?.tick() }
        t.resume()
        timer = t
    }

    private func tick() {
        guard let encoder, let pool else { return }
        var pixel: CVPixelBuffer?
        CVPixelBufferPoolCreatePixelBuffer(nil, pool, &pixel)
        guard let pixel else { return }
        draw(into: pixel, frame: frameIndex)
        let pts = CMTime(value: frameIndex, timescale: CMTimeScale(max(fps, 1)))
        frameIndex += 1
        var props: [String: Any] = [:]
        if forceKeyframe {
            props[kVTEncodeFrameOptionKey_ForceKeyFrame as String] = true
            forceKeyframe = false
        }
        let epoch = self.epoch
        let size = encodeSize
        VTCompressionSessionEncodeFrame(encoder, imageBuffer: pixel, presentationTimeStamp: pts,
                                        duration: .invalid, frameProperties: props as CFDictionary,
                                        infoFlagsOut: nil) { [weak self] status, _, sample in
            guard status == noErr, let sample, let self else { return }
            guard let (data, key) = Self.annexB(sample) else { return }
            self.lock.lock(); let sink = self.sink; self.lock.unlock()
            let us = UInt64(max(0, CMSampleBufferGetPresentationTimeStamp(sample).seconds * 1_000_000))
            sink?.accessUnit(data, keyframe: key, codecEpoch: epoch, ptsMicros: us, size: size)
        }
    }

    /// A moving pattern: a hue per seed, a sweeping bar and a grid, so motion
    /// and scaling artefacts are visible.
    private func draw(into buffer: CVPixelBuffer, frame: Int64) {
        CVPixelBufferLockBaseAddress(buffer, [])
        defer { CVPixelBufferUnlockBaseAddress(buffer, []) }
        let w = CVPixelBufferGetWidth(buffer), h = CVPixelBufferGetHeight(buffer)
        guard let base = CVPixelBufferGetBaseAddress(buffer),
              let ctx = CGContext(data: base, width: w, height: h, bitsPerComponent: 8,
                                  bytesPerRow: CVPixelBufferGetBytesPerRow(buffer),
                                  space: CGColorSpaceCreateDeviceRGB(),
                                  bitmapInfo: CGImageAlphaInfo.premultipliedFirst.rawValue
                                      | CGBitmapInfo.byteOrder32Little.rawValue) else { return }
        let hue = CGFloat((seed * 47) % 360) / 360
        ctx.setFillColor(CGColor(srgbRed: 0.1 + hue * 0.2, green: 0.12, blue: 0.16, alpha: 1))
        ctx.fill(CGRect(x: 0, y: 0, width: w, height: h))
        ctx.setStrokeColor(CGColor(gray: 1, alpha: 0.08))
        let step = max(w / 16, 8)
        for x in stride(from: 0, to: w, by: step) { ctx.stroke(CGRect(x: x, y: 0, width: 1, height: h)) }
        for y in stride(from: 0, to: h, by: step) { ctx.stroke(CGRect(x: 0, y: y, width: w, height: 1)) }
        let barX = CGFloat(Int(frame * 7) % max(w, 1))
        ctx.setFillColor(CGColor(srgbRed: 0.38, green: 0.74, blue: 1, alpha: 0.9))
        ctx.fill(CGRect(x: barX, y: 0, width: CGFloat(max(w / 40, 2)), height: CGFloat(h)))
    }

    /// AVCC sample to one Annex B access unit, parameter sets first on a
    /// keyframe (as the Spaces encoders send it).
    static func annexB(_ sample: CMSampleBuffer) -> (Data, Bool)? {
        guard let block = CMSampleBufferGetDataBuffer(sample),
              let format = CMSampleBufferGetFormatDescription(sample) else { return nil }
        var key = true
        if let attachments = CMSampleBufferGetSampleAttachmentsArray(sample, createIfNecessary: false),
           CFArrayGetCount(attachments) > 0 {
            let dict = unsafeBitCast(CFArrayGetValueAtIndex(attachments, 0), to: CFDictionary.self) as NSDictionary
            if let notSync = dict[kCMSampleAttachmentKey_NotSync as String] as? Bool { key = !notSync }
        }
        let start: [UInt8] = [0, 0, 0, 1]
        var out = Data()
        if key {
            var count = 0
            CMVideoFormatDescriptionGetH264ParameterSetAtIndex(format, parameterSetIndex: 0, parameterSetPointerOut: nil,
                                                               parameterSetSizeOut: nil, parameterSetCountOut: &count,
                                                               nalUnitHeaderLengthOut: nil)
            for i in 0 ..< count {
                var ptr: UnsafePointer<UInt8>?
                var len = 0
                CMVideoFormatDescriptionGetH264ParameterSetAtIndex(format, parameterSetIndex: i, parameterSetPointerOut: &ptr,
                                                                   parameterSetSizeOut: &len, parameterSetCountOut: nil,
                                                                   nalUnitHeaderLengthOut: nil)
                if let ptr { out.append(contentsOf: start); out.append(ptr, count: len) }
            }
        }
        var length = 0
        var pointer: UnsafeMutablePointer<CChar>?
        guard CMBlockBufferGetDataPointer(block, atOffset: 0, lengthAtOffsetOut: nil, totalLengthOut: &length,
                                          dataPointerOut: &pointer) == noErr, let pointer else { return nil }
        var offset = 0
        while offset + 4 <= length {
            var nalLength: UInt32 = 0
            memcpy(&nalLength, pointer + offset, 4)
            let n = Int(UInt32(bigEndian: nalLength))
            offset += 4
            guard n > 0, offset + n <= length else { break }
            out.append(contentsOf: start)
            out.append(Data(bytes: pointer + offset, count: n))
            offset += n
        }
        return (out, key)
    }
}
