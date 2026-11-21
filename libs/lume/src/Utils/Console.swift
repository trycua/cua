// SPDX-License-Identifier: MIT

import Foundation

#if canImport(Darwin)
    import Darwin
#else
    import Glibc
#endif

/// The one writer of Lume's terminal output: log lines and live progress.
///
/// Progress lives in regions pinned below the log lines. Every write erases
/// the regions, writes the log line, then draws the regions again, so a log
/// line never lands inside a progress frame and an old frame never stays
/// behind. Region lines are cut to the terminal width, because a wrapped
/// line would break the count of lines to erase.
///
/// On a terminal, log lines carry no timestamp and `info` no level. Piped
/// (a file, CI, `lume serve` logs), every line is `[timestamp] LEVEL: text`
/// with no escape codes, and a region logs its last line every
/// `pipedInterval` instead of redrawing.
///
/// A process whose stdout carries a protocol (`lume serve --mcp`) calls
/// `reserveStdout()`: from then on every line goes to stderr and progress
/// is logged as piped lines there.
///
/// Progress is info-level output: below `minLevel` info, regions draw
/// nothing and their final lines are dropped, as log lines are.
final class Console: @unchecked Sendable {
    enum Stream { case out, err }

    /// Where a region sits among the others: layers above the aggregate line.
    enum Slot: Int, Comparable {
        case layers
        case transfer

        static func < (a: Slot, b: Slot) -> Bool { a.rawValue < b.rawValue }
    }

    /// A live progress block owned by one operation. The block leaves the
    /// screen when the operation ends it, or when the handle is released (an
    /// operation that threw before it could end its block).
    final class Region: @unchecked Sendable {
        fileprivate let id: Int
        private weak var console: Console?

        fileprivate init(console: Console, id: Int) {
            self.console = console
            self.id = id
        }

        deinit { console?.remove(id, final: nil) }

        /// Replaces the region's lines.
        func update(_ lines: [String]) { console?.update(id, lines) }

        /// Removes the region; `final`, when given, stays as a log line.
        func end(final: String? = nil) { console?.remove(id, final: final) }
    }

    private struct RegionState {
        let id: Int
        let slot: Slot
        let pipedInterval: TimeInterval?
        var lines: [String] = []
        var lastPiped: Date?
    }

    static let shared = Console()

    /// How often a piped region logs its last line.
    static let defaultPipedInterval: TimeInterval = 10

    let stdoutIsTerminal: Bool
    let stderrIsTerminal: Bool
    private let color: Bool
    private let write: @Sendable (Stream, String) -> Void
    private let width: @Sendable () -> Int?
    private let height: @Sendable () -> Int?
    private let now: @Sendable () -> Date
    private let minLevel: @Sendable () -> Logger.Level

    private let lock = NSLock()
    private var regions: [RegionState] = []
    private var nextId = 0
    /// Set once stdout carries a protocol; nothing else may write there.
    private var stdoutReserved = false
    /// Lines the regions take on screen right now.
    private var drawn = 0

    init(
        stdoutIsTerminal: Bool = isatty(STDOUT_FILENO) == 1,
        stderrIsTerminal: Bool = isatty(STDERR_FILENO) == 1,
        color: Bool = ProcessInfo.processInfo.environment["NO_COLOR"] == nil,
        width: @escaping @Sendable () -> Int? = { Console.terminalSize()?.width },
        height: @escaping @Sendable () -> Int? = { Console.terminalSize()?.height },
        now: @escaping @Sendable () -> Date = { Date() },
        minLevel: @escaping @Sendable () -> Logger.Level = { Logger.minLevel },
        write: @escaping @Sendable (Stream, String) -> Void = Console.writeToProcess
    ) {
        self.stdoutIsTerminal = stdoutIsTerminal
        self.stderrIsTerminal = stderrIsTerminal
        self.color = color
        self.width = width
        self.height = height
        self.now = now
        self.minLevel = minLevel
        self.write = write
    }

    // MARK: Log lines

    /// Sends every later line to stderr, for a process whose stdout is a protocol.
    func reserveStdout() {
        lock.withLock {
            write(.out, eraseSequence())
            stdoutReserved = true
        }
    }

    /// Whether regions redraw in place (stdout is a terminal Lume may draw on).
    private var drawsInPlace: Bool { stdoutIsTerminal && !stdoutReserved }

    func log(_ level: Logger.Level, _ message: String, metadata: Logger.Metadata = [:]) {
        let reserved = lock.withLock { stdoutReserved }
        let stream: Stream = level >= .warning || reserved ? .err : .out
        let terminal = stream == .out ? stdoutIsTerminal : stderrIsTerminal
        let meta = metadata.keys.sorted().map { " \($0)=\(metadata[$0]!)" }.joined()
        let line: String
        if terminal {
            line = Self.terminalPrefix(level, color: color) + message + meta
        } else {
            let stamp = ISO8601DateFormatter().string(from: now())
            line = "[\(stamp)] \(level.rawValue.uppercased()): \(message)\(meta)"
        }
        lock.withLock { emit(stream, line + "\n") }
    }

    private static func terminalPrefix(_ level: Logger.Level, color: Bool) -> String {
        let (label, code): (String, String)
        switch level {
        case .info: return ""
        case .debug: (label, code) = ("debug", "2")
        case .warning: (label, code) = ("warning", "33")
        case .error: (label, code) = ("error", "31")
        }
        return color ? "\u{1B}[\(code)m\(label):\u{1B}[0m " : "\(label): "
    }

    // MARK: Regions

    /// A new, empty region. `pipedInterval` nil keeps it silent when piped.
    func region(_ slot: Slot, pipedInterval: TimeInterval? = nil) -> Region {
        lock.withLock {
            nextId += 1
            regions.append(RegionState(id: nextId, slot: slot, pipedInterval: pipedInterval))
            regions.sort { ($0.slot, $0.id) < ($1.slot, $1.id) }
            return Region(console: self, id: nextId)
        }
    }

    private var showsProgress: Bool { minLevel() <= .info }

    private func update(_ id: Int, _ lines: [String]) {
        guard showsProgress else { return }
        var piped: String?
        lock.withLock {
            guard let index = regions.firstIndex(where: { $0.id == id }) else { return }
            regions[index].lines = lines
            if drawsInPlace {
                write(.out, eraseSequence() + drawSequence())
            } else if let interval = regions[index].pipedInterval, let last = lines.last {
                let t = now()
                if regions[index].lastPiped.map({ t.timeIntervalSince($0) >= interval }) ?? true {
                    regions[index].lastPiped = t
                    piped = last.trimmingCharacters(in: .whitespaces)
                }
            }
        }
        if let piped { log(.info, piped) }
    }

    private func remove(_ id: Int, final: String?) {
        lock.withLock {
            guard let index = regions.firstIndex(where: { $0.id == id }) else { return }
            regions.remove(at: index)
            if drawsInPlace, drawn > 0 { write(.out, eraseSequence() + drawSequence()) }
        }
        if let final, showsProgress { log(.info, final) }
    }

    // MARK: Drawing (callers hold `lock`)

    private func emit(_ stream: Stream, _ text: String) {
        guard drawsInPlace, drawn > 0 || !regions.isEmpty else {
            write(stream, text)
            return
        }
        if stream == .out {
            write(.out, eraseSequence() + text + drawSequence())
        } else {
            write(.out, eraseSequence())
            write(.err, text)
            write(.out, drawSequence())
        }
    }

    /// Moves to the region's first line and clears to the end of the screen.
    private func eraseSequence() -> String {
        defer { drawn = 0 }
        guard drawn > 0 else { return "" }
        return "\r\u{1B}[\(drawn)A\u{1B}[J"
    }

    private func drawSequence() -> String {
        var lines = regions.flatMap(\.lines)
        // Keep at least one row free so the top line never scrolls away.
        if let rows = height(), rows > 1, lines.count > rows - 1 {
            let keep = max(rows - 2, 0)
            lines = ["  … \(lines.count - keep) more"] + lines.suffix(keep)
        }
        // One column short: a full-width line leaves the cursor pending a wrap.
        let columns = max((width() ?? 80) - 1, 1)
        drawn = lines.count
        return lines.map { Self.cut($0, to: columns) + "\n" }.joined()
    }

    private static func cut(_ line: String, to columns: Int) -> String {
        line.count > columns ? String(line.prefix(max(columns - 1, 0))) + "…" : line
    }

    // MARK: Process defaults

    static func terminalSize() -> (width: Int, height: Int)? {
        var ws = winsize()
        #if canImport(Darwin)
            let rc = ioctl(STDOUT_FILENO, TIOCGWINSZ, &ws)
        #else
            let rc = ioctl(STDOUT_FILENO, UInt(TIOCGWINSZ), &ws)
        #endif
        guard rc == 0, ws.ws_col > 0 else { return nil }
        return (Int(ws.ws_col), Int(ws.ws_row))
    }

    @Sendable static func writeToProcess(_ stream: Stream, _ text: String) {
        switch stream {
        case .out:
            // Through stdio, so other `print` output keeps its order.
            print(text, terminator: "")
            fflush(nil)
        case .err:
            fflush(nil)
            FileHandle.standardError.write(Data(text.utf8))
        }
    }
}

/// The one style for sizes, rates, durations and bars in progress output.
enum ProgressFormat {
    /// Transfer time an ETA needs before it is shown.
    static let etaMinimumElapsed: TimeInterval = 5

    static func bytes(_ count: Int64) -> String {
        let units = ["B", "KB", "MB", "GB", "TB"]
        var size = Double(max(count, 0))
        var unit = 0
        while size >= 1024 && unit < units.count - 1 {
            size /= 1024
            unit += 1
        }
        return String(format: "%.1f %@", size, units[unit])
    }

    static func rate(_ bytesPerSecond: Double) -> String {
        bytes(Int64(max(bytesPerSecond, 0))) + "/s"
    }

    static func duration(_ seconds: TimeInterval) -> String {
        let total = Int(max(seconds, 0).rounded())
        let (h, m, s) = (total / 3600, total % 3600 / 60, total % 60)
        if h > 0 { return "\(h)h \(m)m" }
        if m > 0 { return "\(m)m \(s)s" }
        return "\(s)s"
    }

    static func bar(_ fraction: Double, width: Int = 25) -> String {
        let filled = Int((min(max(fraction, 0), 1) * Double(width)).rounded(.down))
        return String(repeating: "█", count: filled)
            + String(repeating: "░", count: width - filled)
    }

    /// The time left at a smoothed `rate`, or nil until `elapsed` seconds of
    /// transfer make the rate worth trusting.
    static func eta(remaining: Int64, rate: Double, elapsed: TimeInterval) -> TimeInterval? {
        guard elapsed >= etaMinimumElapsed, rate > 0, remaining > 0 else { return nil }
        return Double(remaining) / rate
    }

    /// The aggregate line: `9/300 layers · 238.4 MB / 21.5 GB · 163.0 MB/s · ETA 2m 10s`.
    static func aggregate(
        done: Int, total: Int, noun: String,
        bytes doneBytes: Int64, of totalBytes: Int64,
        rate: Double, eta: TimeInterval?, extra: [String] = []
    ) -> String {
        var parts = ["\(done)/\(total) \(noun)", "\(bytes(doneBytes)) / \(bytes(totalBytes))"]
        if rate > 0 { parts.append(self.rate(rate)) }
        if let eta { parts.append("ETA \(duration(eta))") }
        parts += extra
        return "  " + parts.joined(separator: " · ")
    }
}
