import Foundation

/// Scrubbing playback: "what did the screen look like at time T?"
///
/// The answer is produced by replaying every output event with a timestamp
/// `<= T` through a `TerminalEmulator` and reading the resulting grid. There
/// is no interpolation and no guessing: a frame is the deterministic result of
/// the bytes that had arrived by that instant.
///
/// Fidelity limits, stated rather than discovered:
///
/// * **Resolution is per event, not per byte.** A cast event is a chunk as the
///   kernel handed it over. Asking for a time in the middle of a chunk gives
///   the state *before* that chunk, because a half-applied chunk is a state
///   the terminal never actually had.
/// * **Time is relative to the recording, not the CLI.** The recorder stamps
///   events when it read them; a CLI that buffers draws them later than it
///   decided them.
/// * **Scrollback is not retained.** The emulator keeps one screen. Claude
///   Code runs on the alternate buffer, where lines scrolled off the top are
///   gone from the terminal too, so the frame matches what a user could see
///   without scrolling — but content that scrolled away earlier in the turn is
///   not recoverable from a frame, and `scrolledLines` says how much left.
/// * **Terminal size is fixed** at the cast header's dimensions. Resize events
///   (`"r"`) are not applied; `unsupportedResize` reports if any were present.
public struct CastPlayer: Sendable {
    public let recording: CastRecording
    public let columns: Int
    public let rows: Int

    public init(recording: CastRecording) {
        self.recording = recording
        self.columns = recording.header.width
        self.rows = recording.header.height
    }

    public init(contentsOf url: URL) throws {
        self.init(recording: try CastRecording(contentsOf: url))
    }

    /// The rendered state at `time`.
    public func frame(at time: TimeInterval) -> RenderedFrame {
        var emulator = TerminalEmulator(columns: columns, rows: rows)
        var applied = 0
        var lastTime: TimeInterval = 0
        var sawResize = false
        for event in recording.events {
            if event.time > time { break }
            switch event.kind {
            case .output:
                emulator.feed(event.data)
                applied += 1
                lastTime = event.time
            case .resize:
                sawResize = true
            case .input, .marker:
                lastTime = event.time
            }
        }
        return RenderedFrame(screen: emulator.screen,
                             windowTitle: emulator.windowTitle,
                             requestedTime: time,
                             effectiveTime: lastTime,
                             eventsApplied: applied,
                             unsupportedSequences: emulator.unsupportedSequences,
                             unsupportedResize: sawResize,
                             cli: recording.header.cli,
                             cliVersion: recording.header.cliVersion)
    }

    /// The final state of the recording.
    public func finalFrame() -> RenderedFrame {
        frame(at: recording.duration + 1)
    }

    /// Every distinct instant at which the screen could have changed. Useful
    /// for stepping a scrubber and for finding the moment a golden should
    /// capture.
    public var outputTimes: [TimeInterval] {
        recording.events.filter { $0.kind == .output }.map(\.time)
    }
}

/// A screen, plus everything known about how it was produced.
///
/// `RenderedFrame` is the *only* input to the parser. Anything the parser
/// claims has to be justifiable from this value.
public struct RenderedFrame: Sendable {
    public let screen: ScreenBuffer
    /// The last OSC window title set at or before this instant. Claude Code
    /// writes the live spinner glyph and the turn's subject here.
    public let windowTitle: String
    /// The time that was asked for.
    public let requestedTime: TimeInterval
    /// The timestamp of the last event actually applied. Never greater than
    /// `requestedTime`.
    public let effectiveTime: TimeInterval
    public let eventsApplied: Int
    /// Escape sequences the emulator did not implement. Non-zero lowers
    /// confidence in the frame and is carried into the parse.
    public let unsupportedSequences: Int
    /// The recording contained a resize the player did not apply.
    public let unsupportedResize: Bool
    public let cli: String?
    public let cliVersion: String?

    public var lines: [String] { screen.lines }
    public var text: String { screen.text }

    public init(screen: ScreenBuffer, windowTitle: String, requestedTime: TimeInterval,
                effectiveTime: TimeInterval, eventsApplied: Int,
                unsupportedSequences: Int, unsupportedResize: Bool,
                cli: String?, cliVersion: String?) {
        self.screen = screen
        self.windowTitle = windowTitle
        self.requestedTime = requestedTime
        self.effectiveTime = effectiveTime
        self.eventsApplied = eventsApplied
        self.unsupportedSequences = unsupportedSequences
        self.unsupportedResize = unsupportedResize
        self.cli = cli
        self.cliVersion = cliVersion
    }

    /// Build a frame from a raw terminal stream with no recording behind it —
    /// the `RunSnapshot.outputTail` case. `time` is meaningless here and is
    /// reported as zero.
    public static func render(stream: String, columns: Int, rows: Int,
                              cli: String? = nil, cliVersion: String? = nil) -> RenderedFrame {
        var emulator = TerminalEmulator(columns: columns, rows: rows)
        emulator.feed(stream)
        return RenderedFrame(screen: emulator.screen,
                             windowTitle: emulator.windowTitle,
                             requestedTime: 0, effectiveTime: 0, eventsApplied: 1,
                             unsupportedSequences: emulator.unsupportedSequences,
                             unsupportedResize: false,
                             cli: cli, cliVersion: cliVersion)
    }
}
