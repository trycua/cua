// SPDX-License-Identifier: MIT

import Foundation
import Testing

@testable import lume

// MARK: - Helpers

/// A minimal VT100 screen: plain text, `\r`, `\n`, cursor up (`ESC[nA`),
/// erase to end of screen (`ESC[J`) and SGR colors (ignored). It keeps every
/// line that ever scrolled, so a test sees the scrollback a user would.
private final class Screen: @unchecked Sendable {
    private let lock = NSLock()
    private var rows: [[Character]] = [[]]
    private var row = 0
    private var col = 0
    private var raw: [(Console.Stream, String)] = []

    func write(_ stream: Console.Stream, _ text: String) {
        lock.withLock {
            raw.append((stream, text))
            var chars = Array(text)[...]
            while let c = chars.popFirst() {
                switch c {
                case "\r": col = 0
                case "\n":
                    row += 1
                    col = 0
                    if row == rows.count { rows.append([]) }
                case "\u{1B}":
                    guard chars.popFirst() == "[" else { continue }
                    var arg = ""
                    while let d = chars.first, d.isNumber || d == ";" {
                        arg.append(d)
                        chars.removeFirst()
                    }
                    switch chars.popFirst() {
                    case "A": row = max(0, row - (Int(arg) ?? 1))
                    case "J":
                        rows[row].removeSubrange(min(col, rows[row].count)...)
                        rows.removeSubrange((row + 1)...)
                    default: break
                    }
                default:
                    while rows[row].count < col { rows[row].append(" ") }
                    if col < rows[row].count { rows[row][col] = c } else { rows[row].append(c) }
                    col += 1
                }
            }
        }
    }

    /// Every line, scrollback included, without the empty line under the cursor.
    var lines: [String] {
        lock.withLock {
            var out = rows.map { String($0) }
            if out.last == "" { out.removeLast() }
            return out
        }
    }

    var writes: [(Console.Stream, String)] { lock.withLock { raw } }
}

private final class Clock: @unchecked Sendable {
    private let lock = NSLock()
    private var current = Date(timeIntervalSince1970: 1_800_000_000)
    var now: Date { lock.withLock { current } }
    func advance(_ s: TimeInterval) { lock.withLock { current += s } }
}

private func console(
    _ screen: Screen, terminal: Bool = true, width: Int = 120, height: Int = 40,
    clock: Clock = Clock(), minLevel: Logger.Level = .info
) -> Console {
    Console(
        stdoutIsTerminal: terminal, stderrIsTerminal: terminal, color: false,
        width: { width }, height: { height }, now: { clock.now }, minLevel: { minLevel },
        write: { screen.write($0, $1) })
}

// MARK: - Terminal output

struct ConsoleTerminalTests {
    @Test("a log line while progress is live goes above it, never onto it")
    func logLinesScrollAboveProgress() {
        let screen = Screen()
        let c = console(screen)
        let region = c.region(.transfer)
        region.update(["  0/2 layers · 0.0 B / 1.0 GB"])
        c.log(.info, "Downloading config layer")
        region.update(["  1/2 layers · 512.0 MB / 1.0 GB"])
        c.log(.info, "Saved nvram.bin")

        #expect(
            screen.lines == [
                "Downloading config layer",
                "Saved nvram.bin",
                "  1/2 layers · 512.0 MB / 1.0 GB",
            ])
    }

    @Test("a redraw with shorter lines leaves no tail of the longer one")
    func shorterRedrawLeavesNoRemnant() {
        let screen = Screen()
        let c = console(screen)
        let region = c.region(.layers)
        region.update(["  a3f1c9d2e4b5  Downloading  disk layer with a long tail", "  b"])
        region.update(["  a3f1"])
        #expect(screen.lines == ["  a3f1"])
    }

    @Test("finished work leaves the region once instead of piling up in scrollback")
    func endedRegionLeavesOneLine() {
        let screen = Screen()
        let c = console(screen)
        let layers = c.region(.layers)
        let total = c.region(.transfer)
        for i in 0..<50 {
            layers.update(["  layer-\(i % 3)  Downloading"])
            total.update(["  \(i)/50 layers"])
            if i % 10 == 0 { c.log(.info, "tick \(i)") }
        }
        layers.end()
        total.end(final: "Downloaded 1.0 GB in 9s")

        #expect(
            screen.lines == [
                "tick 0", "tick 10", "tick 20", "tick 30", "tick 40",
                "Downloaded 1.0 GB in 9s",
            ])
    }

    @Test("layer lines sit above the aggregate line whatever order they start in")
    func regionsKeepSlotOrder() {
        let screen = Screen()
        let c = console(screen)
        let total = c.region(.transfer)
        let layers = c.region(.layers)
        total.update(["  total"])
        layers.update(["  layer a", "  layer b"])
        #expect(screen.lines == ["  layer a", "  layer b", "  total"])
    }

    @Test("a line wider than the terminal is cut, so the redraw count stays right")
    func wideLinesAreCut() {
        let screen = Screen()
        let c = console(screen, width: 20)
        let region = c.region(.transfer)
        region.update([String(repeating: "x", count: 50)])
        region.update(["short"])
        #expect(screen.lines == ["short"])
        #expect(screen.writes.allSatisfy { !$0.1.contains(String(repeating: "x", count: 20)) })
    }

    @Test("more region lines than rows collapse into a count")
    func tallRegionsFitTheScreen() {
        let screen = Screen()
        let c = console(screen, height: 5)
        let region = c.region(.layers)
        region.update((1...10).map { "  l\($0)" })
        #expect(screen.lines == ["  … 7 more", "  l8", "  l9", "  l10"])
    }

    @Test("a region whose owner is gone leaves the screen")
    func droppedRegionIsErased() {
        let screen = Screen()
        let c = console(screen)
        do {
            let region = c.region(.transfer)
            region.update(["  stale progress"])
        }
        c.log(.info, "next")
        #expect(screen.lines == ["next"])
    }

    @Test("on a terminal info has no stamp or level; warnings and errors go to stderr")
    func terminalLevels() {
        let screen = Screen()
        let c = console(screen)
        c.log(.info, "Pulling macos-tahoe-cua:latest")
        c.log(.warning, "slow mirror")
        c.log(.error, "Failed to pull image", metadata: ["error": "boom"])
        #expect(
            screen.lines == [
                "Pulling macos-tahoe-cua:latest", "warning: slow mirror",
                "error: Failed to pull image error=boom",
            ])
        #expect(screen.writes.map(\.0) == [.out, .err, .err])
    }
}

// MARK: - Piped output

struct ConsolePipedTests {
    @Test("piped lines carry a stamp and a level and no escape codes")
    func pipedLines() {
        let screen = Screen()
        let c = console(screen, terminal: false)
        c.log(.info, "Pulling x:1", metadata: ["b": "2", "a": "1"])
        c.log(.error, "boom")
        #expect(
            screen.lines == [
                "[2027-01-15T08:00:00Z] INFO: Pulling x:1 a=1 b=2",
                "[2027-01-15T08:00:00Z] ERROR: boom",
            ])
        #expect(screen.writes.map(\.0) == [.out, .err])
    }

    @Test("piped progress is a periodic log line, not a redraw")
    func pipedProgressIsPeriodic() {
        let screen = Screen()
        let clock = Clock()
        let c = console(screen, terminal: false, clock: clock)
        let region = c.region(.transfer, pipedInterval: 10)
        for i in 0..<25 {
            region.update(["  \(i)/25 layers"])
            clock.advance(1)
        }
        let silent = c.region(.layers)
        silent.update(["  a3f1  Downloading"])
        region.end(final: "Downloaded 1.0 GB in 25s")

        #expect(
            screen.lines == [
                "[2027-01-15T08:00:00Z] INFO: 0/25 layers",
                "[2027-01-15T08:00:10Z] INFO: 10/25 layers",
                "[2027-01-15T08:00:20Z] INFO: 20/25 layers",
                "[2027-01-15T08:00:25Z] INFO: Downloaded 1.0 GB in 25s",
            ])
        #expect(screen.writes.allSatisfy { !$0.1.contains("\u{1B}") && !$0.1.contains("\r") })
    }
}

// MARK: - Reserved stdout

struct ConsoleReservedStdoutTests {
    @Test("once stdout carries a protocol, logs and progress go to stderr only")
    func reservedStdoutStaysClean() {
        for terminal in [true, false] {
            let screen = Screen()
            let clock = Clock()
            let c = console(screen, terminal: terminal, clock: clock)
            let region = c.region(.transfer, pipedInterval: 0)
            region.update(["  0/2 layers"])
            c.reserveStdout()
            c.log(.info, "Pulling x:1")
            region.update(["  1/2 layers"])
            region.end(final: "Downloaded 1.0 GB in 9s")
            let afterReserve = screen.writes.drop { $0.0 == .out || $0.1.isEmpty }
            #expect(afterReserve.allSatisfy { $0.0 == .err })
            #expect(afterReserve.map(\.1).joined().contains("Pulling x:1"))
            #expect(afterReserve.map(\.1).joined().contains("Downloaded 1.0 GB in 9s"))
            #expect(!afterReserve.map(\.1).joined().contains("\u{1B}["))
        }
    }
}

// MARK: - Levels

struct ConsoleLevelTests {
    @Test("above info, progress writes nothing, on a terminal or piped")
    func progressFollowsTheLevel() {
        for terminal in [true, false] {
            let screen = Screen()
            let c = console(screen, terminal: terminal, minLevel: .error)
            let region = c.region(.transfer, pipedInterval: 0)
            region.update(["  1/2 layers"])
            region.end(final: "Downloaded 1.0 GB in 9s")
            #expect(screen.writes.isEmpty)
        }
    }
}

struct LoggerLevelTests {
    @Test("LUME_LOG_LEVEL names map to levels, warn included")
    func levelNames() {
        #expect(Logger.Level(name: "DEBUG") == .debug)
        #expect(Logger.Level(name: "warn") == .warning)
        #expect(Logger.Level(name: "warning") == .warning)
        #expect(Logger.Level(name: "nope") == nil)
        #expect(Logger.Level.debug < .info && .info < .warning && .warning < .error)
    }
}

// MARK: - Progress lines

struct ProgressLineTests {
    @Test("no ETA from the first sample of a transfer")
    func noEtaFromOneSample() async {
        let clock = Clock()
        let tracker = ProgressTracker(now: { clock.now }, emitInterval: 0, console: nil)
        await tracker.setTotal(21_500_000_000, files: 300)
        clock.advance(1)
        await tracker.updateTransfer(id: "config", bytesWritten: 1_600)
        #expect(!(await tracker.line()).contains("ETA"))

        // After enough steady transfer the ETA appears and follows the rate.
        for _ in 0..<20 {
            clock.advance(1)
            await tracker.updateTransfer(
                id: "config", bytesWritten: (await tracker.snapshot().downloadedBytes) + 100_000_000)
        }
        let line = await tracker.line()
        #expect(line.contains("ETA 3m"), "\(line)")
        #expect(line.hasPrefix("  0/300 layers · "), "\(line)")
    }

    @Test("finished layers count in the aggregate line")
    func layersDoneCount() async {
        let tracker = ProgressTracker(emitInterval: 0, console: nil)
        await tracker.setTotal(300, files: 3)
        await tracker.addProgress(100)
        await tracker.finishTransfer(id: "b", bytes: 100)
        #expect(await tracker.line() == "  2/3 layers · 200.0 B / 300.0 B")
    }

    @Test("a pull shows one line per layer in flight, named by its digest")
    func layersByDigest() async {
        let clock = Clock()
        let display = LayerProgressDisplay(console: nil, now: { clock.now })
        let digests = ["sha256:a3f1c9d2e4b5ffff", "sha256:7b20e4f1aaaa0000", "sha256:ee91c5070000beef"]
        for (i, digest) in digests.enumerated() {
            await display.addLayer(
                id: "chunk-\(i)", label: "part \(i + 1)/3", totalBytes: 1024 * 1024, mode: .pull,
                digest: digest)
        }
        await display.updateStatus(id: "chunk-0", status: .downloading)
        await display.updateProgress(id: "chunk-0", completedBytes: 512 * 1024)
        await display.updateStatus(id: "chunk-1", status: .downloading)
        await display.markDone(id: "chunk-2")

        let lines = await display.lines()
        #expect(lines.count == 2)
        #expect(lines[0].hasPrefix("  a3f1c9d2e4b5  Downloading  "))
        #expect(lines[0].hasSuffix("512.0 KB / 1.0 MB"))
        #expect(lines[1].hasPrefix("  7b20e4f1aaaa  Downloading  "))
        #expect(!lines.joined().contains("ee91c5070000"))
    }

    @Test("ETA needs time and a rate, and durations read as h/m/s")
    func etaAndDuration() {
        #expect(ProgressFormat.eta(remaining: 100, rate: 10, elapsed: 1) == nil)
        #expect(ProgressFormat.eta(remaining: 100, rate: 0, elapsed: 60) == nil)
        #expect(ProgressFormat.eta(remaining: 100, rate: 10, elapsed: 5) == 10)
        #expect(ProgressFormat.duration(130) == "2m 10s")
        #expect(ProgressFormat.duration(3 * 3600 + 120) == "3h 2m")
        #expect(ProgressFormat.duration(9) == "9s")
    }
}
