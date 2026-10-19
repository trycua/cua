// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CanvasModel
import CanvasStreaming
import CoreGraphics
import Foundation

// canvas-bench: the canvas's hot paths, headless.
//
//   canvas-bench [--tiles N] [--seconds S] [--json FILE]
//
// 1. Model: level-of-detail decisions, camera flights and layout, per call.
// 2. Decode: N tiles of the local H.264 test pattern (hardware encode,
//    hardware decode, frames enqueued into display layers, exactly the
//    tile pipeline) at the full tier for S seconds, then at a mix of tiers.
// Reports decode rate and latency, CPU and memory. The display side (frame
// pacing at 120 Hz while panning) needs a window: see the `perf` control
// command and README.md.

var tiles = 8
var seconds = 10.0
var jsonOut: String?
var args = CommandLine.arguments.dropFirst().makeIterator()
while let a = args.next() {
    switch a {
    case "--tiles": tiles = Int(args.next() ?? "") ?? tiles
    case "--seconds": seconds = Double(args.next() ?? "") ?? seconds
    case "--json": jsonOut = args.next()
    default:
        print("canvas-bench [--tiles N] [--seconds S] [--json FILE]")
        exit(2)
    }
}

func time(_ n: Int, _ body: () -> Void) -> Double {
    let t0 = DispatchTime.now().uptimeNanoseconds
    for _ in 0 ..< n { body() }
    return Double(DispatchTime.now().uptimeNanoseconds - t0) / Double(n)
}

var report: [String: Any] = [:]

// 1. Model
let screen = CGRect(x: 0, y: 0, width: 3024, height: 1964)
let policy = LODPolicy()
var states = [LODState](repeating: LODState(), count: 100)
var tick = 0.0
let lodNs = time(10_000) {
    tick += 1 / 120
    for i in states.indices {
        let edge = 200 + CGFloat((i * 37 + Int(tick * 400)) % 1800)
        let v = TileVisibility(screenRect: CGRect(x: CGFloat(i * 90 % 3000), y: 200, width: edge, height: edge * 0.62),
                               screenBounds: screen)
        _ = states[i].update(v, now: tick, policy: policy)
    }
}
let flight = CameraFlight(from: Camera(center: .zero, zoom: 0.1), to: Camera(center: CGPoint(x: 9000, y: 4000), zoom: 1.4),
                          viewSize: CGSize(width: 1512, height: 982))
var e = 0.0
let flightNs = time(100_000) { e += 0.00001; _ = flight.camera(at: e.truncatingRemainder(dividingBy: flight.duration)) }
var layout = CanvasLayout()
for i in 0 ..< 60 {
    layout.add(Tile(id: "\(i)", kind: .window(spaceID: "s\(i % 4)", windowID: "\(i)"), title: "\(i)",
                    frame: CGRect(x: 0, y: 0, width: 800 + i * 7, height: 500 + i * 3)))
}
let groups = (0 ..< 4).map { g in (0 ..< 60).filter { $0 % 4 == g }.map(String.init) }
let arrangeNs = time(200) { var l = layout; l.arrange(groups: groups) }
report["model"] = ["lod100TilesPerFrameUs": lodNs / 1000, "cameraFlightSampleNs": flightNs,
                   "arrange60TilesUs": arrangeNs / 1000]
print(String(format: "model: LOD for 100 tiles %.1f us/frame, flight sample %.0f ns, arrange 60 tiles %.0f us",
             lodNs / 1000, flightNs, arrangeNs / 1000))

// 2. Decode
func runDecode(_ tier: (Int) -> StreamTier, tiles: Int, seconds: Double) async -> [String: Any] {
    var streams: [TileStream] = []
    for i in 0 ..< tiles {
        let size = i % 3 == 0 ? CGSize(width: 1440, height: 900) : CGSize(width: 1280, height: 800)
        let s = TileStream(id: "bench-\(i)", source: SyntheticMediaSource(size: size, seed: i))
        streams.append(s)
        try? await s.start(initial: tier(i))
    }
    try? await Task.sleep(for: .seconds(1)) // warm up: first keyframes, sessions
    let before = streams.map { $0.decoder.counters.snapshot() }
    var cpu = CPUMeter()
    _ = cpu.sample()
    var footprint = ProcessMetrics.footprint()
    for _ in 0 ..< Int(seconds * 4) {
        try? await Task.sleep(for: .milliseconds(250))
        footprint = max(footprint, ProcessMetrics.footprint())
    }
    let (proc, _) = cpu.sample()
    let after = streams.map { $0.decoder.counters.snapshot() }
    for s in streams { await s.stop() }
    let decoded = zip(before, after).map { $1.decoded - $0.decoded }.reduce(0, +)
    let skipped = zip(before, after).map { $1.skipped - $0.skipped }.reduce(0, +)
    let p50 = after.map(\.decodeP50Ms).sorted()[after.count / 2]
    let p95 = after.map(\.decodeP95Ms).max() ?? 0
    return ["tiles": tiles, "decodeFps": Double(decoded) / seconds, "skippedFramesPerSec": Double(skipped) / seconds,
            "decodeP50Ms": p50, "decodeP95MsWorstTile": p95,
            "hardware": !after.contains { $0.hardware == false } && after.contains { $0.hardware == true },
            "cpuPercentIncludingTestPatternEncode": proc, "footprintMB": Double(footprint) / 1_048_576]
}

let full = await runDecode({ _ in .full }, tiles: tiles, seconds: seconds)
print("decode, \(tiles) tiles at full (60 fps asked):", full)
let mixed = await runDecode({ [.full, .medium, .low, .thumbnail, .paused, .paused, .low, .medium][$0 % 8] }, tiles: tiles, seconds: seconds)
print("decode, \(tiles) tiles at mixed tiers:", mixed)
report["decodeFull"] = full
report["decodeMixed"] = mixed
report["tiles"] = tiles
report["seconds"] = seconds
if let jsonOut, let data = try? JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys]) {
    try? data.write(to: URL(fileURLWithPath: jsonOut))
}
