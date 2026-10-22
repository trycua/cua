// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CanvasModel
import CanvasStreaming
import Foundation
import os

/// A scripted pan and zoom sweep over every tile, measured.
///
/// The camera is driven once per display frame along a Lissajous pan with a
/// slow log-scale zoom oscillation (overview to past 1:1), which is what a
/// trackpad does to the scroll view, so every level-of-detail transition
/// happens during the run. It records display-link intervals, main-thread
/// time per tick, process and main-thread CPU, whole-GPU utilization, the
/// physical footprint, and per-tile decode counters.
@MainActor
enum PerfRun {
    static func run(canvas: CanvasController, seconds: Double, out: String?, label: String) async -> [String: Any] {
        let signposter = OSSignposter(subsystem: Signposts.subsystem, category: "perf")
        let interval = signposter.beginInterval("sweep")
        let bounds = canvas.layout.bounds
        guard !bounds.isNull else { return ["ok": false, "error": "no tiles"] }
        let view = canvas.scroll.bounds.size
        let fit = Camera.fitting(bounds, in: view, padding: 40, maxZoom: 1).zoom
        let zMin = fit * 0.85, zMax = 1.4
        let before = canvas.streams.mapValues { $0.decoder.counters.snapshot() }
        var cpu = CPUMeter()
        _ = cpu.sample()
        canvas.frameStats.reset()
        let start = CACurrentMediaTime()
        var gpu: [Double] = []
        var footprint: UInt64 = ProcessMetrics.footprint()
        var tierSamples: [String: Int] = [:]
        canvas.onTick = { now in
            let t = now - start
            let u = t / seconds
            let z = exp(log(zMin) + (log(zMax) - log(zMin)) * (0.5 - 0.5 * cos(u * .pi * 4)))
            let cx = bounds.midX + bounds.width * 0.42 * sin(u * .pi * 2 * 1.5)
            let cy = bounds.midY + bounds.height * 0.40 * sin(u * .pi * 2 * 1.0 + 0.6)
            canvas.setCamera(Camera(center: CGPoint(x: cx, y: cy), zoom: z))
        }
        let steps = Int(seconds * 4)
        for _ in 0 ..< steps {
            try? await Task.sleep(for: .milliseconds(250))
            if let g = ProcessMetrics.gpuUtilization() { gpu.append(g) }
            footprint = max(footprint, ProcessMetrics.footprint())
            for (tier, n) in canvas.census.counts { tierSamples[tier.description, default: 0] += n }
        }
        canvas.onTick = nil
        let (proc, main) = cpu.sample()
        signposter.endInterval("sweep", interval)
        let refresh = Double(NSScreen.main?.maximumFramesPerSecond ?? 60)
        let frames = canvas.frameStats.summary(nominal: 1 / refresh)
        var tiles: [[String: Any]] = []
        var decodedTotal = 0
        var hardwareAll = true
        for (id, s) in canvas.streams {
            let a = before[id], b = s.decoder.counters.snapshot()
            let decoded = b.decoded - (a?.decoded ?? 0)
            decodedTotal += decoded
            if b.hardware == false { hardwareAll = false }
            tiles.append(["id": id, "decoded": decoded, "received": b.received - (a?.received ?? 0),
                          "skipped": b.skipped - (a?.skipped ?? 0), "decodeP50Ms": b.decodeP50Ms,
                          "decodeP95Ms": b.decodeP95Ms, "hardware": b.hardware as Any,
                          "keyframeRequests": s.keyframeRequests, "tierChanges": s.tierChanges])
        }
        let report: [String: Any] = [
            "ok": true, "label": label, "seconds": seconds, "tiles": canvas.streams.count,
            "displayHz": refresh,
            "frames": ["count": frames.frames, "meanMs": frames.meanMs, "p50Ms": frames.p50Ms, "p95Ms": frames.p95Ms,
                       "p99Ms": frames.p99Ms, "maxMs": frames.maxMs, "fps": frames.fps, "late": frames.late],
            "cpuPercent": proc, "mainThreadCpuPercent": main,
            "gpuPercentMean": gpu.isEmpty ? 0 : gpu.reduce(0, +) / Double(gpu.count),
            "gpuPercentMax": gpu.max() ?? 0,
            "footprintMB": Double(footprint) / 1_048_576,
            "decodedFramesTotal": decodedTotal, "decodeFps": Double(decodedTotal) / seconds,
            "hardwareDecodeAllTiles": hardwareAll,
            "tierSamples": tierSamples, "perTile": tiles,
        ]
        if let out, let data = try? JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys]) {
            try? data.write(to: URL(fileURLWithPath: out))
        }
        return report
    }
}
