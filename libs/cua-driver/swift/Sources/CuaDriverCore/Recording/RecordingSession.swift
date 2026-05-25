import CoreGraphics
import Foundation
import os

/// Session-scoped trajectory recorder. When enabled, every action-tool
/// invocation writes a numbered turn folder containing the post-action
/// app state, screenshot, action metadata, and (for clicks) a
/// screenshot variant with a dot drawn at the click point.
///
/// State lives in memory only — the daemon re-starts with recording
/// disabled. Recording failures never bubble up to the tool caller:
/// they're logged and swallowed so a broken disk mount (or a missing
/// Screen Recording grant, or anything else that would make a
/// screenshot fail) doesn't poison the action loop itself.
public actor RecordingSession {
    public static let shared = RecordingSession()

    private var enabled = false
    private var outputDirectory: URL?
    private var nextTurn = 1
    private let capture = WindowCapture()
    /// Full-display H.264 video recorder. Present only when the current
    /// session was armed with `videoExperimental = true`; nil otherwise.
    /// Lives independently of turn-folder emission — its failure doesn't
    /// affect the trajectory pipeline and vice versa.
    private var videoRecorder: VideoRecorder?
    /// ~60Hz cursor-position sampler writing `cursor.jsonl`. Only armed
    /// when `videoExperimental = true` — the render-with-zoom pipeline is
    /// the sole consumer. Nil otherwise.
    private var cursorSampler: CursorSampler?
    /// URL of the most recently auto-rendered post-processed video (written
    /// by `teardownSession` when `videoExperimental` is true). Nil if no
    /// auto-render has run yet or the last render failed.
    public private(set) var lastAutoRenderURL: URL? = nil
    /// Monotonic anchor for this session. Captured at configure-on time
    /// and reused for `cursor.jsonl` `t_ms` + every turn's new
    /// `t_ms_from_session_start` field on `action.json`. Zero when no
    /// session is active.
    private var sessionStartMonotonicNs: UInt64 = 0
    /// Wall-clock timestamp of session start, serialized into `session.json`
    /// as an ISO-8601 string for human consumption. Nil when no session
    /// is active.
    private var sessionStartWallClock: Date?
    /// True for sessions armed with `--video-experimental`. Drives
    /// whether `session.json` reports `video.present` / `cursor.present`
    /// as true at finalize time.
    private var sessionVideoExperimental: Bool = false
    /// Injected at bootstrap by the server so the recorder reuses the same
    /// `AppStateEngine` other tools see. Left nil in unit-test / CLI-only
    /// contexts — `record(...)` skips the AX snapshot when this is nil.
    private var engine: AppStateEngine?
    private let log = Logger(
        subsystem: "com.trycua.driver",
        category: "RecordingSession"
    )

    public init() {}

    /// Point the recorder at the server-wide shared `AppStateEngine`.
    /// Called once during startup so the recorded `app_state.json`
    /// shares the same turn-id counter and element-index map the live
    /// action tools use.
    public func bindAppStateEngine(_ engine: AppStateEngine) {
        self.engine = engine
    }

    /// Snapshot of the current recording state.
    public struct State: Sendable {
        public let enabled: Bool
        public let outputDirectory: URL?
        public let nextTurn: Int
        public let videoExperimental: Bool
    }

    public func currentState() -> State {
        State(
            enabled: enabled,
            outputDirectory: outputDirectory,
            nextTurn: nextTurn,
            videoExperimental: videoRecorder != nil
        )
    }

    public func isEnabled() -> Bool { enabled }

    /// Turn recording on or off. Switching off is always allowed; turning
    /// on requires a valid `outputDir` that either exists or can be
    /// created as a directory. The turn counter resets to 1 every time we
    /// point at a fresh output directory — a new session starts counting
    /// from `turn-00001/` regardless of any existing contents there.
    ///
    /// When `videoExperimental` is true (and `enabled` is true), a
    /// `VideoRecorder` is spun up alongside the turn-folder pipeline and
    /// writes `recording.mp4` at the root of `outputDir`. The video side
    /// is best-effort: a failure to start capture (e.g. Screen Recording
    /// permission denied) is logged and swallowed so the trajectory
    /// pipeline still arms.
    public func configure(
        enabled: Bool,
        outputDir: URL?,
        videoExperimental: Bool = false
    ) async throws {
        // Tearing down the prior session happens on every transition —
        // whether we're disabling or re-configuring into a new directory.
        // Stop cursor sampling + video capture + finalize the previous
        // `session.json` before mutating state, so the old recording dir
        // ends up with a self-consistent footprint even when re-armed.
        await teardownSession()

        if !enabled {
            self.enabled = false
            self.outputDirectory = nil
            return
        }
        guard let dir = outputDir else {
            throw RecordingError.missingOutputDir
        }
        try FileManager.default.createDirectory(
            at: dir, withIntermediateDirectories: true
        )
        self.enabled = true
        self.outputDirectory = dir
        self.nextTurn = 1
        self.sessionStartMonotonicNs = clock_gettime_nsec_np(CLOCK_UPTIME_RAW)
        self.sessionStartWallClock = Date()
        self.sessionVideoExperimental = videoExperimental

        if videoExperimental {
            let recorder = VideoRecorder(
                outputURL: dir.appendingPathComponent("recording.mp4")
            )
            do {
                try await recorder.start()
                self.videoRecorder = recorder
            } catch {
                log.error(
                    "video capture start failed; continuing with trajectory-only recording: \(error.localizedDescription, privacy: .public)"
                )
            }

            // Arm the cursor sampler alongside video. It shares the
            // session's monotonic anchor so `cursor.jsonl` `t_ms` values
            // line up with `t_ms_from_session_start` on every
            // `action.json` + with `session.json`'s `started_at_monotonic_ns`.
            let sampler = CursorSampler(
                outputURL: dir.appendingPathComponent("cursor.jsonl"),
                sessionStartMonotonicNs: self.sessionStartMonotonicNs
            )
            do {
                try await sampler.start()
                self.cursorSampler = sampler
            } catch {
                log.error(
                    "cursor sampler start failed; continuing without cursor telemetry: \(error.localizedDescription, privacy: .public)"
                )
            }
        }

        // Initial `session.json`. Written unconditionally (even without
        // `--video-experimental`) so downstream tools can rely on a
        // single consistent schema at the output dir root.
        writeInitialSessionJSON(to: dir)
    }

    /// Stop the video recorder + cursor sampler + finalize `session.json`
    /// for the currently-active output directory. Safe to call from a
    /// fully-idle state (no-op). Called from both the "disable" path
    /// and the "re-configure into a new dir" path.
    private func teardownSession() async {
        let priorDir = outputDirectory
        let priorStartWallClock = sessionStartWallClock
        let priorStartMonotonicNs = sessionStartMonotonicNs
        let priorVideoExperimental = sessionVideoExperimental

        var videoMetadata: VideoRecorder.FinalMetadata? = nil
        if let existing = videoRecorder {
            videoMetadata = await existing.stop()
            videoRecorder = nil
        }

        var cursorSampleCount: Int = 0
        if let existing = cursorSampler {
            cursorSampleCount = await existing.stop()
            cursorSampler = nil
        }

        if let dir = priorDir, let startWall = priorStartWallClock {
            writeFinalSessionJSON(
                to: dir,
                startedAtWallClock: startWall,
                startedAtMonotonicNs: priorStartMonotonicNs,
                videoExperimental: priorVideoExperimental,
                videoMetadata: videoMetadata,
                cursorSampleCount: cursorSampleCount
            )
        }

        // Auto post-process: render the raw capture into a zoom-on-click MP4
        // saved next to recording.mp4 as recording_rendered.mp4.
        if let dir = priorDir, priorVideoExperimental {
            let renderedURL = dir.appendingPathComponent("recording_rendered.mp4")
            log.info(
                "auto-rendering \(dir.path, privacy: .public) -> recording_rendered.mp4"
            )
            do {
                try await RecordingRenderer.render(from: dir, to: renderedURL)
                lastAutoRenderURL = renderedURL
                log.info(
                    "auto-render complete: \(renderedURL.path, privacy: .public)"
                )
            } catch {
                lastAutoRenderURL = nil
                log.error(
                    "auto-render failed: \(error.localizedDescription, privacy: .public)"
                )
            }
        } else {
            lastAutoRenderURL = nil
        }

        sessionStartMonotonicNs = 0
        sessionStartWallClock = nil
        sessionVideoExperimental = false
    }

    /// Record a single turn. No-op when disabled. Never throws — every
    /// failure inside is logged and swallowed so the action loop stays
    /// on its happy path.
    ///
    /// `actionStartNs` is the `CLOCK_UPTIME_RAW` timestamp captured
    /// **before** the tool's animation ran; 0 means "unknown" and falls
    /// back to the end timestamp for both start and end fields.
    public func record(
        toolName: String,
        arguments: [String: Any],
        pid: pid_t?,
        clickPoint: CGPoint?,
        resultSummary: String,
        actionStartNs: UInt64 = 0
    ) async {
        guard enabled, let root = outputDirectory else { return }

        let turnIndex = nextTurn
        nextTurn += 1

        let turnDir = root.appendingPathComponent(
            String(format: "turn-%05d", turnIndex), isDirectory: true
        )
        do {
            try FileManager.default.createDirectory(
                at: turnDir, withIntermediateDirectories: true
            )
        } catch {
            log.error(
                "failed to create turn dir \(turnDir.path, privacy: .public): \(error.localizedDescription, privacy: .public)"
            )
            return
        }

        writeActionJSON(
            to: turnDir,
            toolName: toolName,
            arguments: arguments,
            pid: pid,
            clickPoint: clickPoint,
            resultSummary: resultSummary,
            actionStartNs: actionStartNs
        )

        // AX snapshot — only when we have a pid to target.
        if let pid {
            await writeAppStateJSON(to: turnDir, pid: pid)
        }

        // Screenshot + optional click-marker overlay.
        if let pid {
            let shot: Screenshot?
            do {
                shot = try await capture.captureFrontmostWindow(pid: pid)
            } catch {
                log.error(
                    "captureFrontmostWindow(\(pid)) failed: \(error.localizedDescription, privacy: .public)"
                )
                shot = nil
            }
            if let shot {
                let pngURL = turnDir.appendingPathComponent("screenshot.png")
                do {
                    try shot.imageData.write(to: pngURL, options: .atomic)
                } catch {
                    log.error(
                        "write screenshot.png failed: \(error.localizedDescription, privacy: .public)"
                    )
                }
                if let clickPoint {
                    ClickMarkerRenderer.writeMarker(
                        baseImageData: shot.imageData,
                        scaleFactor: shot.scaleFactor,
                        clickPointInPoints: clickPoint,
                        destination: turnDir.appendingPathComponent("click.png")
                    )
                }
            }
        }
    }

    // MARK: - action.json

    private func writeActionJSON(
        to turnDir: URL,
        toolName: String,
        arguments: [String: Any],
        pid: pid_t?,
        clickPoint: CGPoint?,
        resultSummary: String,
        actionStartNs: UInt64 = 0
    ) {
        // `timestamp` stays on the existing ISO-8601 wall-clock format —
        // other consumers (replay tooling, users reading turn folders
        // by hand) rely on it. The render pipeline needs a monotonic
        // offset from session start to overlay turn events on video
        // frames, so we add a sibling `t_ms_from_session_start` computed
        // off the same anchor `session.json` + `cursor.jsonl` use.
        let nowNs = clock_gettime_nsec_np(CLOCK_UPTIME_RAW)
        let anchor = sessionStartMonotonicNs
        let tMsFromSessionStart: Int =
            anchor > 0 && nowNs >= anchor
            ? Int((nowNs - anchor) / 1_000_000)
            : 0
        let tStartMsFromSessionStart: Int =
            anchor > 0 && actionStartNs >= anchor
            ? Int((actionStartNs - anchor) / 1_000_000)
            : tMsFromSessionStart

        var payload: [String: Any] = [
            "tool": toolName,
            "arguments": makeJSONSafe(arguments),
            "result_summary": resultSummary,
            "timestamp": ISO8601DateFormatter().string(from: Date()),
            "t_ms_from_session_start": tMsFromSessionStart,
            "t_start_ms_from_session_start": tStartMsFromSessionStart,
        ]
        if let pid {
            payload["pid"] = Int(pid)
            // Record the frontmost window bounds so the render pipeline can
            // zoom to the target window without needing screen recording to
            // re-analyze the video. This query is best-effort (post-action)
            // — the window rarely moves during an action.
            if let win = WindowEnumerator.frontmostWindow(forPid: pid) {
                let b = win.bounds
                payload["window_bounds"] = [
                    "x": b.x, "y": b.y,
                    "width": b.width, "height": b.height,
                ]
            }
        }
        if let clickPoint {
            payload["click_point"] = [
                "x": clickPoint.x,
                "y": clickPoint.y,
            ]
        }
        let url = turnDir.appendingPathComponent("action.json")
        do {
            let data = try JSONSerialization.data(
                withJSONObject: payload,
                options: [.prettyPrinted, .sortedKeys]
            )
            try data.write(to: url, options: .atomic)
        } catch {
            log.error(
                "write action.json failed: \(error.localizedDescription, privacy: .public)"
            )
        }
    }

    // MARK: - app_state.json

    /// Serialize the same `AppStateSnapshot` shape `get_window_state` returns
    /// (sans screenshot fields — the screenshot is already captured to its
    /// own file, and inlining a base64 PNG here would double every
    /// turn-folder's disk footprint).
    ///
    /// The engine's `snapshot(pid:windowId:)` now requires a window_id for
    /// element-index cache keying. Recording doesn't have a caller-supplied
    /// window_id, so we resolve the pid's frontmost window via the same
    /// "visible, on current Space, highest z, fall back to max-area" rule
    /// the tool layer uses for pid-only callers. Skip the snapshot if the
    /// pid has no resolvable window — the screenshot + action.json paths
    /// still run.
    private func writeAppStateJSON(to turnDir: URL, pid: Int32) async {
        guard let engine else {
            log.debug("no AppStateEngine bound; skipping app_state.json")
            return
        }
        guard let windowId = WindowEnumerator.frontmostWindowID(forPid: pid) else {
            log.debug(
                "no window for pid \(pid, privacy: .public); skipping app_state.json"
            )
            return
        }
        let snapshot: AppStateSnapshot
        do {
            snapshot = try await engine.snapshot(pid: pid, windowId: windowId)
        } catch {
            log.error(
                "AppStateEngine.snapshot(\(pid), \(windowId)) failed: \(error.localizedDescription, privacy: .public)"
            )
            return
        }
        let url = turnDir.appendingPathComponent("app_state.json")
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        do {
            let data = try encoder.encode(snapshot)
            try data.write(to: url, options: .atomic)
        } catch {
            log.error(
                "write app_state.json failed: \(error.localizedDescription, privacy: .public)"
            )
        }
    }

    // MARK: - session.json

    /// Schema version for `session.json`. Bump when the on-disk shape
    /// changes in a way that breaks downstream tool expectations.
    private static let sessionJSONSchemaVersion: Int = 1

    /// Write the initial `session.json` at recording-start. End-of-session
    /// fields (`ended_at_monotonic_ns`, `duration_ms`, `frame_count`,
    /// `sample_count`) are zero-stubbed here and overwritten by
    /// `writeFinalSessionJSON` when the session stops.
    ///
    /// Emitted unconditionally (even for trajectory-only recordings) so
    /// downstream tools can consume one consistent schema; `video.present`
    /// / `cursor.present` tell them whether the paired files exist.
    private func writeInitialSessionJSON(to dir: URL) {
        let payload = makeSessionJSONPayload(
            startedAtWallClock: sessionStartWallClock ?? Date(),
            startedAtMonotonicNs: sessionStartMonotonicNs,
            endedAtMonotonicNs: 0,
            durationMs: 0,
            videoExperimental: sessionVideoExperimental,
            videoMetadata: nil,
            cursorSampleCount: 0
        )
        writeSessionJSON(payload: payload, to: dir)
    }

    /// Overwrite `session.json` with finalized end-of-session values.
    /// Called from `teardownSession()` after both the video recorder
    /// and cursor sampler have flushed.
    private func writeFinalSessionJSON(
        to dir: URL,
        startedAtWallClock: Date,
        startedAtMonotonicNs: UInt64,
        videoExperimental: Bool,
        videoMetadata: VideoRecorder.FinalMetadata?,
        cursorSampleCount: Int
    ) {
        let endedAtMonotonicNs = clock_gettime_nsec_np(CLOCK_UPTIME_RAW)
        let durationMs: Int =
            endedAtMonotonicNs >= startedAtMonotonicNs
            ? Int((endedAtMonotonicNs - startedAtMonotonicNs) / 1_000_000)
            : 0

        let payload = makeSessionJSONPayload(
            startedAtWallClock: startedAtWallClock,
            startedAtMonotonicNs: startedAtMonotonicNs,
            endedAtMonotonicNs: endedAtMonotonicNs,
            durationMs: durationMs,
            videoExperimental: videoExperimental,
            videoMetadata: videoMetadata,
            cursorSampleCount: cursorSampleCount
        )
        writeSessionJSON(payload: payload, to: dir)
    }

    /// Build the `session.json` dict. Kept pure so start + finalize
    /// share one shape definition; `video.present` / `cursor.present`
    /// flip to true only when `videoExperimental` is on.
    private func makeSessionJSONPayload(
        startedAtWallClock: Date,
        startedAtMonotonicNs: UInt64,
        endedAtMonotonicNs: UInt64,
        durationMs: Int,
        videoExperimental: Bool,
        videoMetadata: VideoRecorder.FinalMetadata?,
        cursorSampleCount: Int
    ) -> [String: Any] {
        let isoFormatter = ISO8601DateFormatter()
        isoFormatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]

        let videoPayload: [String: Any] = [
            "path": "recording.mp4",
            "present": videoExperimental,
            "width": videoMetadata?.width ?? 0,
            "height": videoMetadata?.height ?? 0,
            "framerate_target": VideoRecorder.targetFrameRate,
            "frame_count": videoMetadata?.frameCount ?? 0,
        ]
        let cursorPayload: [String: Any] = [
            "path": "cursor.jsonl",
            "present": videoExperimental,
            "sample_hz": 60,
            "sample_count": cursorSampleCount,
        ]
        let displayScaleFactor = ScreenInfo.mainScreenSize()?.scaleFactor ?? 1.0

        return [
            "schema_version": Self.sessionJSONSchemaVersion,
            "display_scale_factor": displayScaleFactor,
            "started_at_wall_clock": isoFormatter.string(from: startedAtWallClock),
            "started_at_monotonic_ns": startedAtMonotonicNs,
            "ended_at_monotonic_ns": endedAtMonotonicNs,
            "duration_ms": durationMs,
            "video": videoPayload,
            "cursor": cursorPayload,
        ]
    }

    private func writeSessionJSON(payload: [String: Any], to dir: URL) {
        let url = dir.appendingPathComponent("session.json")
        do {
            let data = try JSONSerialization.data(
                withJSONObject: payload,
                options: [.prettyPrinted, .sortedKeys]
            )
            try data.write(to: url, options: .atomic)
        } catch {
            log.error(
                "write session.json failed: \(error.localizedDescription, privacy: .public)"
            )
        }
    }

    // MARK: - Arg sanitization

    /// Walk `dict` and replace any value type JSONSerialization would
    /// reject with its string representation. Keeps recursive
    /// dictionaries / arrays of scalars intact; non-JSON scalars get
    /// coerced rather than failing the whole write.
    private nonisolated func makeJSONSafe(_ value: Any) -> Any {
        if let dict = value as? [String: Any] {
            var out: [String: Any] = [:]
            for (k, v) in dict { out[k] = makeJSONSafe(v) }
            return out
        }
        if let array = value as? [Any] {
            return array.map { makeJSONSafe($0) }
        }
        if value is String || value is Int || value is Double
            || value is Bool || value is NSNull
        {
            return value
        }
        return String(describing: value)
    }
}

public enum RecordingError: Error, CustomStringConvertible, Sendable {
    case missingOutputDir

    public var description: String {
        switch self {
        case .missingOutputDir:
            return "output_dir is required when enabling recording."
        }
    }
}
