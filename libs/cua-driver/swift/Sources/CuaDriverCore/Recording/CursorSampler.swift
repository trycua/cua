import AppKit
import CoreGraphics
import Foundation
import os

/// Periodic cursor-position sampler. When armed alongside video capture,
/// writes one JSON line per sample to `cursor.jsonl` at the root of the
/// recording output dir. The render-with-zoom post-process consumes this
/// to reconstruct a smooth cursor timeline aligned against video frames.
///
/// One actor per recording session. Stops cleanly on `stop()`; the file
/// handle is flushed and closed there. Failures (file open, write) log
/// and tear down — they never bubble up to the trajectory pipeline.
///
/// Coordinate convention: `NSEvent.mouseLocation` returns Cocoa screen
/// points (y-up, origin at bottom-left of the main screen). We flip Y
/// to match cua-driver's top-left-origin click-coord convention before
/// writing, so downstream tools consume a single consistent screen-point
/// space.
public actor CursorSampler {
    /// Target 60Hz — render pipeline expects ~2× video framerate to
    /// interpolate cleanly. Best-effort; `Task.sleep` has per-platform
    /// slack, and that's fine — timestamps are recorded off the same
    /// monotonic clock the session uses, so gaps surface as jitter in
    /// `t_ms`, not as drift.
    private static let sampleIntervalNanoseconds: UInt64 = 16_000_000

    private let outputURL: URL
    /// Anchor clock for `t_ms`. Captured at session start so every sample
    /// shares an origin with `session.json`'s `started_at_monotonic_ns`
    /// and (new) `t_ms_from_session_start` on each `action.json`.
    private let sessionStartMonotonicNs: UInt64

    private var fileHandle: FileHandle?
    private var samplingTask: Task<Void, Never>?
    /// Count of successfully-written samples. Surfaced to RecordingSession
    /// at finalize time so `session.json` can cite it.
    private var sampleCount: Int = 0

    private let log = Logger(
        subsystem: "com.trycua.driver",
        category: "CursorSampler"
    )

    public init(outputURL: URL, sessionStartMonotonicNs: UInt64) {
        self.outputURL = outputURL
        self.sessionStartMonotonicNs = sessionStartMonotonicNs
    }

    /// Open `cursor.jsonl` for writing (truncating any prior content) and
    /// start the sampling loop. Throws if the file can't be opened — the
    /// caller (RecordingSession) logs + proceeds without cursor telemetry.
    public func start() throws {
        try? FileManager.default.removeItem(at: outputURL)
        FileManager.default.createFile(atPath: outputURL.path, contents: nil)
        guard let handle = try? FileHandle(forWritingTo: outputURL) else {
            throw CursorSamplerError.openFailed(outputURL.path)
        }
        self.fileHandle = handle

        let anchor = sessionStartMonotonicNs
        samplingTask = Task { [weak self] in
            while !Task.isCancelled {
                await self?.sampleOnce(anchorNs: anchor)
                try? await Task.sleep(
                    nanoseconds: CursorSampler.sampleIntervalNanoseconds
                )
            }
        }
    }

    /// Stop the sampling loop, flush, and close the file handle. Safe to
    /// call from any state — repeated or before `start()`. Returns the
    /// total sample count so the caller can stamp `session.json`.
    @discardableResult
    public func stop() async -> Int {
        samplingTask?.cancel()
        samplingTask = nil
        if let handle = fileHandle {
            try? handle.synchronize()
            try? handle.close()
        }
        fileHandle = nil
        return sampleCount
    }

    /// Take a single cursor-position sample and append it as one JSON
    /// line. Run on the actor so per-sample state mutation is serialized
    /// with start/stop.
    private func sampleOnce(anchorNs: UInt64) {
        guard let handle = fileHandle else { return }
        let nowNs = clock_gettime_nsec_np(CLOCK_UPTIME_RAW)
        // `t_ms` is best-effort signed-nonneg; if the monotonic clock
        // somehow returns a time before the anchor (shouldn't, but
        // CLOCK_UPTIME_RAW is cheap, not magic) clamp to 0.
        let tMs = nowNs >= anchorNs ? Int((nowNs - anchorNs) / 1_000_000) : 0

        let cocoaPoint = NSEvent.mouseLocation
        // Cocoa y-up → top-left-origin: y_top = mainHeight - y_cocoa.
        // Matches the convention used elsewhere in cua-driver for click
        // coords (see MouseInput.cocoaLocation(fromScreenPoint:)).
        let mainScreenHeight = NSScreen.main?.frame.height
            ?? NSScreen.screens.first?.frame.height
            ?? 0
        let xTopLeft = cocoaPoint.x
        let yTopLeft = mainScreenHeight - cocoaPoint.y

        let payload: [String: Any] = [
            "t_ms": tMs,
            "x": xTopLeft,
            "y": yTopLeft,
        ]
        do {
            let data = try JSONSerialization.data(
                withJSONObject: payload, options: [.sortedKeys]
            )
            handle.write(data)
            handle.write(Data([0x0A])) // '\n'
            sampleCount += 1
        } catch {
            log.error(
                "encode cursor sample failed: \(error.localizedDescription, privacy: .public)"
            )
        }
    }
}

public enum CursorSamplerError: Error, CustomStringConvertible, Sendable {
    case openFailed(String)

    public var description: String {
        switch self {
        case .openFailed(let path):
            return "failed to open cursor.jsonl for writing: \(path)"
        }
    }
}
