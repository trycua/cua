// Read-only loader for a `cua-driver recording … --video-experimental`
// output directory. Turns `session.json` + `cursor.jsonl` + every
// `turn-*/action.json` into the inputs the zoom-math library + renderer
// expect: a `SessionMetadata` block, a `[ClickEvent]`, and a
// `[CursorSample]` (already sorted ascending by `tMs`).
//
// Failure philosophy: missing optional files degrade to empty arrays
// (no-op zoom). Missing `session.json` / `recording.mp4` throws — a
// render pipeline with no video + no metadata has nothing to do.

import Foundation

/// Minimum session metadata the renderer needs. A full `session.json`
/// carries more (see `RecordingSession.makeSessionJSONPayload`) — we
/// intentionally surface only the fields used at render time so this
/// type stays easy to keep in sync.
public struct SessionMetadata: Sendable, Equatable {
    public let videoWidth: Int
    public let videoHeight: Int
    /// Total number of cursor samples written. Informational only —
    /// the renderer decides from the loaded `[CursorSample]` itself.
    public let cursorSampleCount: Int
    /// Backing scale factor of the main display at recording time
    /// (e.g. 1.0 for non-Retina, 2.0 for Retina). Used by the renderer
    /// to convert screen points → video pixels. Defaults to 1.0 for
    /// recordings made before this field was added.
    public let displayScaleFactor: Double

    public init(videoWidth: Int, videoHeight: Int, cursorSampleCount: Int, displayScaleFactor: Double = 1.0) {
        self.videoWidth = videoWidth
        self.videoHeight = videoHeight
        self.cursorSampleCount = cursorSampleCount
        self.displayScaleFactor = displayScaleFactor
    }
}

public enum TrajectoryLoaderError: Error, CustomStringConvertible, Sendable {
    case sessionJSONMissing(URL)
    case sessionJSONMalformed(URL, String)
    case recordingVideoMissing(URL)

    public var description: String {
        switch self {
        case .sessionJSONMissing(let url):
            return "session.json not found at \(url.path)"
        case .sessionJSONMalformed(let url, let detail):
            return "session.json at \(url.path) is malformed: \(detail)"
        case .recordingVideoMissing(let url):
            return "recording.mp4 not found at \(url.path)"
        }
    }
}

public enum TrajectoryLoader {
    /// Read a recording directory and project it into render-ready inputs.
    /// Throws on structurally-required files (`session.json`,
    /// `recording.mp4`); silently tolerates an absent `cursor.jsonl` or
    /// a turn folder tree that has zero click-class actions.
    public static func load(from directory: URL) throws -> (
        metadata: SessionMetadata,
        clicks: [ClickEvent],
        cursorSamples: [CursorSample]
    ) {
        let sessionURL = directory.appendingPathComponent("session.json")
        let cursorURL = directory.appendingPathComponent("cursor.jsonl")
        let videoURL = directory.appendingPathComponent("recording.mp4")

        let metadata = try loadSessionMetadata(at: sessionURL)

        guard FileManager.default.fileExists(atPath: videoURL.path) else {
            throw TrajectoryLoaderError.recordingVideoMissing(videoURL)
        }

        let cursorSamples = loadCursorSamples(at: cursorURL)
        let clicks = loadClicks(from: directory)

        return (metadata, clicks, cursorSamples)
    }

    // MARK: - session.json

    /// Parse the minimum fields off `session.json`. Malformed JSON or a
    /// missing `video` block throws — we can't render without the video
    /// dimensions, and silent zero-fallback would produce a garbled MP4.
    public static func loadSessionMetadata(at url: URL) throws -> SessionMetadata {
        guard FileManager.default.fileExists(atPath: url.path) else {
            throw TrajectoryLoaderError.sessionJSONMissing(url)
        }
        let data: Data
        do {
            data = try Data(contentsOf: url)
        } catch {
            throw TrajectoryLoaderError.sessionJSONMalformed(url, error.localizedDescription)
        }
        let object: Any
        do {
            object = try JSONSerialization.jsonObject(with: data)
        } catch {
            throw TrajectoryLoaderError.sessionJSONMalformed(url, error.localizedDescription)
        }
        guard let dict = object as? [String: Any] else {
            throw TrajectoryLoaderError.sessionJSONMalformed(url, "top-level is not a JSON object")
        }
        guard let video = dict["video"] as? [String: Any] else {
            throw TrajectoryLoaderError.sessionJSONMalformed(url, "missing `video` object")
        }
        let width = (video["width"] as? Int) ?? Int(video["width"] as? Double ?? 0)
        let height = (video["height"] as? Int) ?? Int(video["height"] as? Double ?? 0)
        let cursor = dict["cursor"] as? [String: Any]
        let sampleCount = (cursor?["sample_count"] as? Int)
            ?? Int(cursor?["sample_count"] as? Double ?? 0)
        let displayScaleFactor = (dict["display_scale_factor"] as? Double)
            ?? Double(dict["display_scale_factor"] as? Int ?? 1)

        return SessionMetadata(
            videoWidth: width,
            videoHeight: height,
            cursorSampleCount: sampleCount,
            displayScaleFactor: displayScaleFactor
        )
    }

    // MARK: - cursor.jsonl

    /// Parse one `CursorSample` per non-empty line. Bad lines log to
    /// stderr and are skipped so a partial write at teardown doesn't
    /// kill the whole load.
    public static func loadCursorSamples(at url: URL) -> [CursorSample] {
        guard FileManager.default.fileExists(atPath: url.path) else { return [] }
        guard let data = try? Data(contentsOf: url) else { return [] }
        guard let text = String(data: data, encoding: .utf8) else { return [] }

        var out: [CursorSample] = []
        out.reserveCapacity(text.utf8.count / 48) // rough, avoids large initial reallocs

        for line in text.split(separator: "\n", omittingEmptySubsequences: true) {
            guard let lineData = line.data(using: .utf8),
                  let obj = try? JSONSerialization.jsonObject(with: lineData),
                  let dict = obj as? [String: Any] else { continue }

            let tMs = doubleValue(dict["t_ms"]) ?? 0
            guard let x = doubleValue(dict["x"]),
                  let y = doubleValue(dict["y"]) else { continue }
            out.append(CursorSample(tMs: tMs, x: x, y: y))
        }
        // Sampler writes strictly monotonic, but protect the binary-search
        // in `CursorTelemetry.positionAt` by enforcing the invariant here.
        out.sort { $0.tMs < $1.tMs }
        return out
    }

    // MARK: - turn-*/action.json → clicks

    /// Walk every `turn-*/action.json` and project click-class turns into
    /// `ClickEvent`s. v1 heuristic: treat `arguments.x` + `arguments.y` as
    /// screen points when both are present, and skip the turn otherwise.
    /// Coordinate coercion (window-local vs screen-space) is a follow-up.
    public static func loadClicks(from directory: URL) -> [ClickEvent] {
        guard let entries = try? FileManager.default.contentsOfDirectory(
            at: directory,
            includingPropertiesForKeys: nil,
            options: [.skipsHiddenFiles]
        ) else {
            return []
        }

        var clicks: [ClickEvent] = []
        for entry in entries {
            // Only descend into `turn-NNNNN/` directories.
            guard entry.lastPathComponent.hasPrefix("turn-") else { continue }
            var isDir: ObjCBool = false
            guard FileManager.default.fileExists(atPath: entry.path, isDirectory: &isDir),
                  isDir.boolValue else { continue }

            let actionURL = entry.appendingPathComponent("action.json")
            guard let click = parseClickTurn(at: actionURL) else { continue }
            clicks.append(click)
        }
        clicks.sort { $0.tMs < $1.tMs }
        return clicks
    }

    /// Parse one turn's `action.json` into a `ClickEvent`, returning nil
    /// when the turn isn't a click-class tool or when no click coordinate
    /// can be recovered. `t_ms_from_session_start` is required — turns
    /// written by an older recorder without it return nil (can't place
    /// them on the timeline).
    ///
    /// Coordinate recovery walks two sources in order:
    /// 1. `arguments.x` + `arguments.y` — present on **pixel-addressed**
    ///    click turns (the caller passed `{pid, x, y}`). Coordinates are
    ///    window-local screenshot pixels.
    /// 2. `click_point.x` + `click_point.y` — **always** written by
    ///    `RecordingSession` at dispatch time, whatever the addressing
    ///    mode. Coordinates are screen-absolute points (converted from
    ///    the resolved AX element or from the window-local pixel pair).
    ///
    /// The fallback to `click_point` is what makes zoom work for
    /// element-indexed clicks (`{pid, window_id, element_index}`) —
    /// those turns have no `arguments.x/y` at all, and without the
    /// fallback the renderer silently drops every such click, producing
    /// a 1:1 copy of the raw recording with no zoom regions.
    private static func parseClickTurn(at url: URL) -> ClickEvent? {
        guard let data = try? Data(contentsOf: url),
              let obj = try? JSONSerialization.jsonObject(with: data),
              let dict = obj as? [String: Any] else { return nil }

        guard let tool = dict["tool"] as? String,
              isClickClassTool(tool) else { return nil }

        let x: Double
        let y: Double
        if let args = dict["arguments"] as? [String: Any],
           let ax = doubleValue(args["x"]),
           let ay = doubleValue(args["y"]) {
            x = ax
            y = ay
        } else if let cp = dict["click_point"] as? [String: Any],
                  let cx = doubleValue(cp["x"]),
                  let cy = doubleValue(cp["y"]) {
            x = cx
            y = cy
        } else {
            return nil
        }

        guard let tMs = doubleValue(dict["t_ms_from_session_start"]) else {
            return nil
        }

        return ClickEvent(tMs: tMs, x: x, y: y)
    }

    private static func isClickClassTool(_ name: String) -> Bool {
        switch name {
        case "click", "double_click", "right_click":
            return true
        default:
            return false
        }
    }

    // MARK: - turn-*/action.json → ActionSpans

    /// Walk every `turn-*/action.json` and project all action-class turns
    /// into `ActionSpan`s (one per turn). Unlike `loadClicks`, this
    /// includes keyboard / scroll / set_value turns — anything that mutates
    /// UI state and was recorded with `t_start_ms_from_session_start`.
    public static func loadActionSpans(from directory: URL) -> [ActionSpan] {
        guard let entries = try? FileManager.default.contentsOfDirectory(
            at: directory,
            includingPropertiesForKeys: nil,
            options: [.skipsHiddenFiles]
        ) else { return [] }

        var spans: [ActionSpan] = []
        for entry in entries {
            guard entry.lastPathComponent.hasPrefix("turn-") else { continue }
            var isDir: ObjCBool = false
            guard FileManager.default.fileExists(atPath: entry.path, isDirectory: &isDir),
                  isDir.boolValue else { continue }

            let actionURL = entry.appendingPathComponent("action.json")
            if let span = parseActionSpan(at: actionURL) {
                spans.append(span)
            }
        }
        return spans.sorted { $0.startMs < $1.startMs }
    }

    private static func isClickOrTypeClassTool(_ name: String) -> Bool {
        switch name {
        case "click", "double_click", "right_click", "type_text", "type_text_chars":
            return true
        default:
            return false
        }
    }

    private static func parseActionSpan(at url: URL) -> ActionSpan? {
        guard let data = try? Data(contentsOf: url),
              let obj = try? JSONSerialization.jsonObject(with: data),
              let dict = obj as? [String: Any] else { return nil }

        // Only click/type-class actions get normal-speed spans; other agent
        // actions (scroll, hotkey, press_key, etc.) are fast-forwarded.
        guard let tool = dict["tool"] as? String,
              isClickOrTypeClassTool(tool) else { return nil }

        // Require an end timestamp so we can place the span on the timeline.
        guard let endMs = doubleValue(dict["t_ms_from_session_start"]) else { return nil }
        let startMs = doubleValue(dict["t_start_ms_from_session_start"]) ?? endMs

        let windowBounds: WindowBounds?
        if let wb = dict["window_bounds"] as? [String: Any],
           let x = doubleValue(wb["x"]),
           let y = doubleValue(wb["y"]),
           let w = doubleValue(wb["width"]),
           let h = doubleValue(wb["height"]),
           w > 0, h > 0
        {
            windowBounds = WindowBounds(x: x, y: y, width: w, height: h)
        } else {
            windowBounds = nil
        }

        let clickPoint: ClickPoint?
        if let cp = dict["click_point"] as? [String: Any],
           let cx = doubleValue(cp["x"]),
           let cy = doubleValue(cp["y"])
        {
            clickPoint = ClickPoint(x: cx, y: cy)
        } else {
            clickPoint = nil
        }

        return ActionSpan(startMs: startMs, endMs: endMs, windowBounds: windowBounds, clickPoint: clickPoint)
    }

    // MARK: - helpers

    /// Coerce a JSON value (Int, Double, String) to Double. Returns nil
    /// for types JSONSerialization can surface that don't fit (dict, array,
    /// NSNull). Needed because `action.json` + `cursor.jsonl` mix Int and
    /// Double numerics depending on the writer path.
    private static func doubleValue(_ raw: Any?) -> Double? {
        if let d = raw as? Double { return d }
        if let i = raw as? Int { return Double(i) }
        if let n = raw as? NSNumber { return n.doubleValue }
        if let s = raw as? String, let d = Double(s) { return d }
        return nil
    }
}
