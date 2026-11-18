// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

#if canImport(AppKit)
import AppKit
import CuaSpaces
import Foundation

/// Measures how long the main thread is unavailable.
///
/// "The app feels laggy" is not a defect report anyone can act on or verify a
/// fix against, and a screenshot cannot show it. This is the instrument that
/// turns it into a number.
///
/// **How it works.** A repeating timer is scheduled on the main run loop at a
/// fixed interval. If the main thread is free, it fires on time. If something
/// is occupying it — a blocking MCP round trip, a 400-line transcript parse, a
/// three-megapixel blit — the timer cannot fire until that work finishes, and
/// the overshoot *is* the stall. A user clicking the toolbar at that moment
/// waits exactly as long. So the distribution of overshoots is the distribution
/// of click latencies, measured rather than guessed.
///
/// Deliberately a run-loop timer and not a `Task`: a `Task { @MainActor }`
/// would queue behind cooperative-executor work but a *synchronous* block of
/// the thread (which is what `MCPSpacesClient` does) is only visible to
/// something scheduled on the run loop itself.
///
/// Enabled with `OPENKOALABOTS_MAIN_THREAD_PROBE=<path>`; writes a summary to
/// that path at shutdown and prints it. Off — and costing nothing — otherwise.
///
/// See `FRICTION.md` §53.
@MainActor
final class MainThreadProbe {

    static let variable = "OPENKOALABOTS_MAIN_THREAD_PROBE"

    /// The scheduling interval. Overshoot beyond this is stall.
    private let interval: TimeInterval = 0.05
    private var timer: Timer?
    private var last = Date()
    private var stalls: [Double] = []
    /// (seconds into the run, stall in ms), for attribution.
    private var worst: [(Double, Double)] = []
    private var started = Date()
    private let path: String

    private init(path: String) { self.path = path }

    static func startIfRequested() -> MainThreadProbe? {
        guard let path = ProcessInfo.processInfo.environment[variable],
              !path.isEmpty else { return nil }
        let probe = MainThreadProbe(path: path)
        probe.start()
        return probe
    }

    private func start() {
        started = Date()
        last = Date()
        let t = Timer(timeInterval: interval, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated {
                guard let self else { return }
                let now = Date()
                let overshoot = now.timeIntervalSince(self.last) - self.interval
                self.last = now
                // Sub-millisecond jitter is the run loop breathing, not a stall.
                if overshoot > 0.001 {
                    self.stalls.append(overshoot * 1000)
                    self.worst.append((now.timeIntervalSince(self.started), overshoot * 1000))
                }
            }
        }
        // `.common` so the probe keeps sampling during menu tracking and window
        // resize, which run the loop in a different mode and are exactly when a
        // user notices a freeze.
        RunLoop.main.add(t, forMode: .common)
        timer = t
    }

    /// Mark a user interaction and return how long the main thread was blocked
    /// while it was being handled.
    @discardableResult
    func interaction(_ label: String, _ body: () -> Void) -> Double {
        let t0 = CFAbsoluteTimeGetCurrent()
        body()
        let elapsed = (CFAbsoluteTimeGetCurrent() - t0) * 1000
        interactions.append((label, elapsed))
        return elapsed
    }

    private var interactions: [(String, Double)] = []

    /// Run `body` without charging the app for the time it takes.
    ///
    /// The capture harness photographs the window with `CALayer.render(in:)`,
    /// which is main-thread work and, at a 1600x1000 window on a 2x display,
    /// several hundred milliseconds of it. That is the *instrument* blocking
    /// the thread, not the program under test, and leaving it in the sample
    /// stream would let the harness's own cost masquerade as app lag in both
    /// the before and after numbers. Discarding the interval means the next
    /// sample measures from the end of the capture rather than the start.
    @discardableResult
    func excluding<T>(_ body: () -> T) -> T {
        let result = body()
        last = Date()
        return result
    }

    func stop() {
        timer?.invalidate()
        timer = nil
        let text = report()
        try? text.write(toFile: path, atomically: true, encoding: .utf8)
        print(text)
    }

    func report() -> String {
        let sorted = stalls.sorted()
        func pct(_ p: Double) -> Double {
            guard !sorted.isEmpty else { return 0 }
            return sorted[min(sorted.count - 1, Int(p * Double(sorted.count)))]
        }
        var out = "main-thread stalls over \(fmt(Date().timeIntervalSince(started)))s\n"
        out += "  samples over 1ms : \(stalls.count)\n"
        out += "  max              : \(fmt(sorted.last ?? 0))ms\n"
        out += "  p99              : \(fmt(pct(0.99)))ms\n"
        out += "  p50              : \(fmt(pct(0.50)))ms\n"
        out += "  over 100ms       : \(stalls.filter { $0 > 100 }.count)\n"
        out += "  over 1000ms      : \(stalls.filter { $0 > 1000 }.count)\n"
        // Timestamped, so a residual stall can be *attributed* to a moment in
        // the run rather than left as an unexplained number.
        let top = worst.sorted { $0.1 > $1.1 }.prefix(6)
        if !top.isEmpty {
            out += "worst stalls (seconds into the run)\n"
            for (at, ms) in top { out += "  t+\(fmt(at))s  \(fmt(ms))ms\n" }
        }
        let calls = SpacesCallCounter.counted
        if !calls.isEmpty {
            out += "spaces round trips over the same window\n"
            for (tool, n) in calls.sorted(by: { $0.value > $1.value }) {
                out += "  \(tool.padding(toLength: 24, withPad: " ", startingAt: 0)) \(n)\n"
            }
            out += "  \("TOTAL".padding(toLength: 24, withPad: " ", startingAt: 0)) "
                + "\(calls.values.reduce(0, +))\n"
        }
        if !interactions.isEmpty {
            out += "interactions (main-thread time to handle the click)\n"
            for (label, ms) in interactions {
                out += "  \(label.padding(toLength: 28, withPad: " ", startingAt: 0)) \(fmt(ms))ms\n"
            }
        }
        return out
    }

    private func fmt(_ v: Double) -> String { String(format: "%.1f", v) }
}
#endif
