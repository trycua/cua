import Cua
import Foundation

/// One streamable target from `StreamService.ListTargets`.
struct Target {
    let kind: String  // "display" | "window"
    let id: String
    let title: String
    let x: Double, y: Double, width: Double, height: Double
    let available: Bool
    let primary: Bool
}

/// The shared scenario (SCENARIO.md "Steps") and the bench lane.
final class Scenario: @unchecked Sendable {
    let cfg: Config
    let env: SpacesdClient
    /// Window mode: frames and audio also go here.
    let presenter: StreamPresenter?

    init(cfg: Config, env: SpacesdClient, presenter: StreamPresenter?) {
        self.cfg = cfg
        self.env = env
        self.presenter = presenter
    }

    /// Connects to the spacesd with a throwaway SDK state directory (no
    /// host state is read or written).
    static func connect(cfg: Config) async throws -> SpacesdClient {
        let tmp = NSTemporaryDirectory() + "cua-streaming-swift-\(getpid())"
        try FileManager.default.createDirectory(atPath: tmp, withIntermediateDirectories: true)
        let cua = try Cua.embedded(
            stateDir: tmp + "/state", fleetFromEnv: false,
            spacesHome: tmp + "/home", teleportHome: tmp + "/home")
        return try await cua.spacesd(url: cfg.url, token: cfg.token)
    }

    // MARK: Step 3: targets

    func listTargets() async throws -> [Target] {
        let out = try await env.callJson(
            method: "StreamService/ListTargets", requestJson: #"{"include_windows":true}"#)
        guard let root = parseJSON(out) as? [String: Any], let items = root["targets"] as? [[String: Any]] else {
            throw ExampleError("unexpected ListTargets response: \(out.prefix(300))")
        }
        return items.compactMap { t in
            let available = t.bool("available") ?? false
            if let d = t.obj("display") {
                let b = d.obj("bounds") ?? [:]
                let n = d.obj("native_size") ?? [:]
                return Target(
                    kind: "display", id: d.str("id") ?? "", title: d.str("name") ?? "",
                    x: b.num("x") ?? 0, y: b.num("y") ?? 0,
                    width: n.num("width") ?? b.num("width") ?? 0, height: n.num("height") ?? b.num("height") ?? 0,
                    available: available, primary: d.bool("primary") ?? false)
            }
            if let w = t.obj("window") {
                let b = w.obj("bounds") ?? [:]
                return Target(
                    kind: "window", id: w.obj("ref")?.str("id") ?? "", title: w.str("title") ?? "",
                    x: b.num("x") ?? 0, y: b.num("y") ?? 0, width: b.num("width") ?? 0, height: b.num("height") ?? 0,
                    available: available, primary: false)
            }
            return nil
        }
    }

    /// Polls ListTargets (bounded) until a window with `title` shows up.
    func waitForWindow(_ title: String, attempts: Int = 40) async throws -> (Target, [Target]) {
        var last: [Target] = []
        for _ in 0..<attempts {
            last = try await listTargets()
            if let w = last.first(where: { $0.kind == "window" && $0.title == title && $0.width > 0 }) {
                return (w, last)
            }
            await sleepSeconds(0.25)
        }
        throw ExampleError("window \"\(title)\" not listed after \(attempts) attempts")
    }

    // MARK: Step 4/5: one stream

    struct StreamResult {
        var summary: [String: Any]
        var frames: UInt64
    }

    /// H.264 at 30 fps. `interactive` asks for a session that accepts input
    /// (the media-plane click); the default policy is view-only.
    func options(for target: Target, audio: Bool, interactive: Bool = false) -> MediaOpenOptions {
        let policy = interactive ? #","policy":"SESSION_POLICY_ALLOW_ACTIVATION""# : ""
        return MediaOpenOptions(
            display: target.kind == "display" ? target.id : nil,
            windowHandle: target.kind == "window" ? target.id : nil,
            maxFps: 30, maxDimension: 0, audio: audio, disableVideo: false,
            requestJson: #"{"codecs":["MEDIA_CODEC_H264"]"# + policy + "}")
    }

    /// Streams `target` for `seconds`; `during` runs while the session is
    /// open (the click step).
    func stream(
        _ target: Target, name: String, interactive: Bool = false,
        during: ((MediaSession, StreamRecorder) async -> Void)? = nil
    ) async throws -> (StreamResult, [String: Any]?) {
        let rec = StreamRecorder(presenter: presenter)
        let t0 = monoSeconds()
        let session = try await env.openMediaDecodedWithAudio(
            options: options(for: target, audio: true, interactive: interactive), frames: rec, pcm: rec)
        print("\(name): session \(session.sessionId()) codec \(session.codec())")
        var extra: [String: Any]?
        if let during {
            await during(session, rec)
            extra = [:]
        }
        let remaining = cfg.seconds - (monoSeconds() - t0)
        await sleepSeconds(remaining)
        let stats = session.stats()
        try? await session.close()
        let (frames, last) = rec.snapshot()
        let wavPath = (cfg.outDir as NSString).appendingPathComponent("\(name).wav")
        let wrote = (try? rec.saveWav(path: wavPath)) ?? false
        let summary: [String: Any] = [
            "frames": frames,
            // Not exposed by the decoded callbacks (README "SDK gaps").
            "keyframes": NSNull(),
            "bytes": NSNull(),
            "first_frame_ms": jnum(rec.firstFrameMs()),
            "fps": jnum(rec.fps()),
            "audio_packets": stats.audioPackets,
            "pcm_frames": rec.pcmFrames,
            "frames_dropped": stats.framesDropped,
            "last_hash": last.map { fnv1a64($0.data) } ?? NSNull(),
            "last_size": last.map { "\($0.width)x\($0.height)" } ?? NSNull(),
            "hash_of": "decoded_bgra",
            "wav": wrote ? wavPath : NSNull(),
            "decode_errors": rec.decodeErrors.count,
        ]
        print("\(name): \(toJSON(summary))")
        return (StreamResult(summary: summary, frames: frames), extra)
    }

    // MARK: Step 6: click

    func click(window: Target, session: MediaSession, rec: StreamRecorder) async -> [String: Any] {
        var result: [String: Any] = ["sent": false, "via": NSNull(), "logged": false, "pixel_ok": false]
        // Wait (bounded) for a first frame.
        for _ in 0..<100 where rec.snapshot().frames == 0 { await sleepSeconds(0.05) }
        await sleepSeconds(0.5)
        guard let frame = rec.snapshot().last else {
            result["error"] = "no frame before click"
            return result
        }
        let scale = window.width > 0 ? Double(frame.width) / window.width : 1
        let px = Int((200 * scale).rounded()), py = Int((280 * scale).rounded())
        let before = await pressLines()

        let sessionId = mediaSessionId(rec) ?? session.sessionId()
        let actionId = "swift-click-\(unixNanos())"
        let action: [String: Any] = [
            "type": "action",
            "payload": [
                "action_id": actionId, "session_id": sessionId, "tool": "click",
                "arguments": ["x": px, "y": py],
                "basis": ["kind": "pixel", "geometry_epoch": frame.geometryEpoch, "frame_sequence": frame.sequence],
            ] as [String: Any],
        ]
        let mark = rec.eventCount()
        var delivered = false
        var actionError: Any = NSNull()
        do {
            try session.sendControl(json: toJSON(action))
            result["sent"] = true
            result["via"] = "action"
            // Bounded wait for the action_result.
            outer: for _ in 0..<60 {
                for e in rec.events(from: mark) where e.kind == "action_result" {
                    let p = (parseJSON(e.json) as? [String: Any])?.obj("payload") ?? [:]
                    if p.str("action_id") == actionId {
                        delivered = p.bool("delivered") ?? false
                        actionError = p["error"] ?? NSNull()
                        break outer
                    }
                }
                await sleepSeconds(0.05)
            }
        } catch {
            actionError = "\(error)"
        }
        result["action_delivered"] = delivered
        result["action_error"] = actionError
        // Verify via the fixture log (bounded).
        func waitLogged(_ tries: Int) async -> Bool {
            for _ in 0..<tries {
                if await pressLines() > before { return true }
                await sleepSeconds(0.25)
            }
            return false
        }
        var logged = delivered ? await waitLogged(8) : false
        if !logged {
            // Fallback: the env pointer API in screen coordinates (window
            // origin + content offset), foreground delivery. The default
            // (auto) delivery picks X11 XSendEvent, which GTK3 ignores.
            let req: [String: Any] = [
                "target": ["delivery": "DELIVERY_FOREGROUND"],
                "click": ["position": ["x": window.x + 200, "y": window.y + 280], "button": "MOUSE_BUTTON_LEFT", "count": 1],
            ]
            do {
                let resp = try await env.pointerJson(requestJson: toJSON(req))
                result["sent"] = true
                result["via"] = "env"
                result["env_report"] = (parseJSON(resp) as? [String: Any])?["report"] ?? NSNull()
                logged = await waitLogged(12)
            } catch {
                result["env_error"] = "\(error)"
            }
        }
        result["logged"] = logged
        // Verify via the decoded pixel (let a few frames arrive first).
        await sleepSeconds(0.4)
        if let (r, g, b) = rec.pixel(x: px, y: py) {
            result["pixel"] = [r, g, b]
            result["pixel_ok"] = abs(r - 72) <= 24 && abs(g - 153) <= 24 && abs(b - 128) <= 24
        }
        result["at"] = [px, py]
        return result
    }

    /// `session_id` from the socket's `session_opened` message.
    private func mediaSessionId(_ rec: StreamRecorder) -> String? {
        for e in rec.events(from: 0) where e.kind == "session_opened" {
            if let p = (parseJSON(e.json) as? [String: Any])?.obj("payload"), let id = p.str("session_id") {
                return id
            }
        }
        return nil
    }

    /// Number of `button_press` records for cell [2, 3] in the grid log.
    private func pressLines() async -> Int {
        guard let out = try? await env.sh(line: "cat /tmp/cua-fixtures/grid.jsonl 2>/dev/null || true", timeoutMs: 10_000)
        else { return 0 }
        return String(decoding: out.stdout, as: UTF8.self).split(separator: "\n").reduce(0) { n, line in
            guard let o = parseJSON(String(line)) as? [String: Any] else { return n }
            let kind = o.str("type") ?? o.str("event") ?? o.str("kind")
            let cell = (o["cell"] as? [NSNumber])?.map(\.intValue)
            return kind == "button_press" && cell == [2, 3] ? n + 1 : n
        }
    }

    // MARK: The whole scenario

    func run() async throws -> Int32 {
        // 1. Connect (done by the caller) and health.
        print("health: \(try await env.health())")
        // 2. Grid fixture.
        let started = try await env.sh(line: "cua-fixtures start grid", timeoutMs: 30_000)
        print("fixture: exit \(started.exit.code.map { String($0) } ?? "?") \(String(decoding: started.stdout, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines))")
        // 3. Targets.
        let (grid, targets) = try await waitForWindow("CUA Fixture Grid")
        for t in targets {
            print("target \(t.kind) \(t.id) \"\(t.title)\" \(Int(t.width))x\(Int(t.height))+\(Int(t.x))+\(Int(t.y)) available=\(t.available)")
        }
        guard let display = targets.first(where: { $0.kind == "display" && $0.primary })
            ?? targets.first(where: { $0.kind == "display" })
        else { throw ExampleError("no display target") }
        // 4. Desktop.
        let (desktop, _) = try await stream(display, name: "desktop")
        // 5 + 6. Window with the click.
        var click: [String: Any] = [:]
        let (window, _) = try await stream(grid, name: "window", interactive: true) { session, rec in
            click = await self.click(window: grid, session: session, rec: rec)
        }
        // 7. Summary.
        let summary: [String: Any] = [
            "example": "swift", "desktop": desktop.summary, "window": window.summary, "click": click,
        ]
        print("SUMMARY \(toJSON(summary))")
        let ok = desktop.frames > 0 && window.frames > 0 && (click["logged"] as? Bool ?? false)
        return ok ? 0 : 1
    }

    // MARK: Bench lane (SCENARIO.md "Benchmark JSONL")

    func runBench(path: String) async throws -> Int32 {
        let spec = cfg.benchTarget ?? "display:"
        let targets = try await listTargets()
        let target: Target
        var locator: TimecodeLocator?
        if spec.hasPrefix("window:") {
            let title = String(spec.dropFirst("window:".count))
            guard let w = targets.first(where: { $0.kind == "window" && $0.title == title }) else {
                throw ExampleError("bench target \(spec) not listed")
            }
            target = w
            locator = TimecodeLocator(originX: 0, originY: 0, contentWidth: w.width)
        } else {
            let id = spec.hasPrefix("display:") ? String(spec.dropFirst("display:".count)) : ""
            guard let d = targets.first(where: { $0.kind == "display" && (id.isEmpty ? $0.primary : $0.id == id) })
                ?? targets.first(where: { $0.kind == "display" })
            else { throw ExampleError("bench target \(spec) not listed") }
            target = d
            // On a display the strip sits at the bench window's position.
            if let w = targets.first(where: { $0.kind == "window" && $0.title == "CUA Bench Timecode" }) {
                locator = TimecodeLocator(originX: w.x - d.x, originY: w.y - d.y, contentWidth: d.width)
            }
        }
        let out = try JsonlWriter(path: path)
        let rec = StreamRecorder(bench: out, timecode: locator, presenter: presenter)
        out.line("{\"t\":\"open\",\"unix_ns\":\(unixNanos())}")
        let session: MediaSession
        if cfg.benchAudio {
            session = try await env.openMediaDecodedWithAudio(
                options: options(for: target, audio: true), frames: rec, pcm: rec)
        } else {
            session = try await env.openMediaDecoded(options: options(for: target, audio: false), frames: rec)
        }
        await sleepSeconds(cfg.benchSeconds)
        try? await session.close()
        let cpu = cpuSeconds()
        out.line(String(format: "{\"t\":\"end\",\"unix_ns\":%llu,\"cpu_user_s\":%.3f,\"cpu_sys_s\":%.3f}", unixNanos(), cpu.user, cpu.sys))
        out.close()
        let (frames, _) = rec.snapshot()
        print("SUMMARY \(toJSON(["example": "swift", "bench": ["target": spec, "frames": frames, "tc_decoded": rec.tcDecoded, "jsonl": path] as [String: Any]]))")
        return frames > 0 ? 0 : 1
    }
}
