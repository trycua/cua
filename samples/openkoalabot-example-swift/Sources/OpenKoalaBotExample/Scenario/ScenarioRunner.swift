// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CryptoKit
import Cua
import CuaSpaces
import CuaSpacesStreaming
import Foundation

/// The shared openkoalabots scenario (`samples/openkoalabot-example-scenario/scenario.json`),
/// run headlessly through this app's own Spaces code: `SDKSpacesClient` for the
/// Space and the agent thread, `SpaceStreamProvider` (what `LiveStreamSession`
/// streams from) for frames, and the SDK's Space handle for files, teleport
/// and presence.
///
/// Host safety: the Spaces registries are temp directories (never `~/.cua`),
/// teleport would read a **generated** Firefox profile under a temp teleport
/// home (no real profile, app or keychain), the agent is a fake CLI installed
/// in the guest, and nothing opens a window. Session teleport ships with Cua
/// Spaces (source-available): this runner's embedded runtime refuses it with
/// `HostCapabilityMissing`, and that step skips.
enum ScenarioRunner {

    struct StepResult {
        var id: String
        var status: String
        var ms: Int
        var detail: String
    }

    struct Failure: Error, CustomStringConvertible {
        let description: String
        init(_ d: String) { description = d }
    }

    struct Skip: Error { let reason: String }

    /// `OpenKoalaBotExample scenario --spec <scenario.json> --lane fixture|docker|cloud --out <result.json>`
    static func main(_ args: [String]) -> Int32 {
        func value(_ flag: String) -> String? {
            guard let i = args.firstIndex(of: flag), i + 1 < args.count else { return nil }
            return args[i + 1]
        }
        guard let specPath = value("--spec"), let lane = value("--lane"), let out = value("--out") else {
            print("usage: OpenKoalaBotExample scenario --spec <scenario.json> --lane fixture|docker|cloud --out <result.json>")
            return 2
        }
        let result: [String: Any]
        do {
            result = try CLI.blocking { try await run(specPath: specPath, lane: lane) }.value
        } catch {
            result = ["impl": "swift", "lane": lane, "ok": false, "totalMs": 0, "steps": [],
                      "error": "\(error)"]
        }
        do {
            let data = try JSONSerialization.data(withJSONObject: result,
                                                  options: [.prettyPrinted, .sortedKeys])
            try data.write(to: URL(fileURLWithPath: out))
        } catch {
            print("scenario: could not write \(out): \(error)")
            return 1
        }
        return (result["ok"] as? Bool) == true ? 0 : 1
    }

    /// A `Sendable` wrapper so the result can leave `CLI.blocking`.
    struct Box: @unchecked Sendable { let value: [String: Any] }

    static func run(specPath: String, lane: String) async throws -> Box {
        let specURL = URL(fileURLWithPath: specPath)
        let spec = try JSONSerialization.jsonObject(with: Data(contentsOf: specURL)) as? [String: Any] ?? [:]
        let steps = spec["steps"] as? [[String: Any]] ?? []
        let dir = specURL.deletingLastPathComponent()
        let env = ProcessInfo.processInfo.environment
        let nonce = String(format: "%08x", UInt32.random(in: .min ... .max))
        let marker = "openkoalabots-\(nonce)"
        let scratch = FileManager.default.temporaryDirectory
            .appendingPathComponent("openkoalabot-example-scenario-swift-\(nonce)")
        try FileManager.default.createDirectory(at: scratch, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: scratch) }

        func fill(_ s: String, _ extra: [String: String] = [:]) -> String {
            var out = s.replacingOccurrences(of: "{nonce}", with: nonce)
                .replacingOccurrences(of: "{marker}", with: marker)
            for (k, v) in extra { out = out.replacingOccurrences(of: "{\(k)}", with: v) }
            return out
        }

        // The generated teleport profile (fixtures/firefox-profile.json).
        let teleportHome = scratch.appendingPathComponent("teleport-home")
        if let profileStep = steps.first(where: { $0["op"] as? String == "teleport.app" }),
           let rel = profileStep["profile"] as? String {
            let profile = try JSONSerialization.jsonObject(
                with: Data(contentsOf: dir.appendingPathComponent(rel))) as? [String: Any] ?? [:]
            for root in profile["roots"] as? [String] ?? [] {
                for (path, content) in profile["files"] as? [String: String] ?? [:] {
                    let file = teleportHome.appendingPathComponent(root).appendingPathComponent(path)
                    try FileManager.default.createDirectory(at: file.deletingLastPathComponent(),
                                                            withIntermediateDirectories: true)
                    try fill(content).write(to: file, atomically: true, encoding: .utf8)
                }
            }
        }

        let client = try SDKSpacesClient(backend: .embedded(
            spacesHome: scratch.appendingPathComponent("spaces-a").path,
            teleportHome: teleportHome.path))
        var spaceID: String?
        var results: [StepResult] = []
        let started = Date()

        func handle() async throws -> CuaSDK.Space {
            guard let id = spaceID else { throw Failure("no Space (the space step failed)") }
            // #region docs:sw-native
            guard let native = try await client.sdkConnection.nativeSpace(SpaceID(id)) else {
                throw Failure("the connection is not backed by the cua SDK")
            }
            // #endregion docs:sw-native
            return native
        }
        func bash(_ command: String, timeoutMs: UInt64 = 120_000) async throws -> String {
            let r = try await handle().bash(command: command, timeoutMs: timeoutMs)
            guard r.exitCode == 0 else { throw Failure("`\(command)` failed: \(r.rendered)") }
            return r.stdout
        }

        for step in steps {
            let id = step["id"] as? String ?? "?"
            let op = step["op"] as? String ?? "?"
            let t0 = Date()
            var status = "pass"
            var detail = ""
            if spaceID == nil, op != "space.open", !(results.isEmpty) {
                // Every later step needs the Space; say so rather than fail each.
                if op != "space.delete" {
                    results.append(StepResult(id: id, status: "fail", ms: 0,
                                              detail: "no Space (the space step failed)"))
                    continue
                }
            }
            do {
                if let feature = step["requires"] as? String, spaceID != nil,
                   !(try await handle().supports(feature: feature)) {
                    throw Skip(reason: "the Space does not report \(feature)")
                }
                switch op {
                case "space.open":
                    let modes = step["modes"] as? [String: String] ?? [:]
                    let name = fill(step["name"] as? String ?? "openkoalabot-example-scenario-{nonce}")
                    if modes[lane] == "create" {
                        guard let image = env["OPENKOALABOTS_CLOUD_IMAGE"], !image.isEmpty else {
                            throw Skip(reason: "OPENKOALABOTS_CLOUD_IMAGE is unset")
                        }
                        guard let native = client.sdkConnection.native else {
                            throw Failure("no cua SDK Spaces object")
                        }
                        // A cloud Space, said explicitly: this is the metered call.
                        let created = try await native.create(options: CuaSDK.SpaceCreateOptions(
                            image: image, on: "cloud", name: name, wait: true, reuse: false))
                        guard let info = created.space else {
                            throw Failure("Space still starting: \(created.pendingId ?? "?")")
                        }
                        spaceID = info.id
                    } else {
                        guard let url = env["OPENKOALABOTS_SCENARIO_URL"], !url.isEmpty else {
                            throw Failure("OPENKOALABOTS_SCENARIO_URL is unset")
                        }
                        spaceID = try await client.addSpace(url: url,
                                                            token: env["OPENKOALABOTS_SCENARIO_TOKEN"],
                                                            name: name)
                    }
                    SDKSpacesClient.pinnedSpace = spaceID
                    // The app's own attach path: ensureSpace honours the pin and
                    // never creates anything.
                    let attached = try await client.ensureSpace()
                    guard attached == spaceID else { throw Failure("attached to \(attached), not \(spaceID!)") }
                    detail = "\(spaceID!) (\(modes[lane] ?? "add"))"

                case "stream.desktop":
                    detail = try await streamDesktop(step, space: try await client.sdkSpace(spaceID!))

                case "agent.thread":
                    detail = try await agentThread(step, dir: dir, fill: fill, client: client,
                                                   space: spaceID!, handle: handle, bash: bash)

                case "file.send":
                    detail = try await sendFile(step, fill: fill, scratch: scratch, handle: handle, bash: bash)

                case "teleport.app":
                    // #region docs:sw-teleport
                    let receipt = try await handle().teleport(
                        app: step["app"] as? String ?? "firefox",
                        scope: step["scope"] as? String,
                        approver: ApproveDefaults())
                    // #endregion docs:sw-teleport
                    guard !receipt.imported.isEmpty else { throw Failure("nothing imported: \(receipt)") }
                    let root = env["OPENKOALABOTS_SCENARIO_IMPORT_ROOT"].flatMap { $0.isEmpty ? nil : $0 } ?? "$HOME"
                    var found = ""
                    // The import is synchronous, but give a slow guest filesystem a moment.
                    for _ in 0..<10 {
                        found = try await bash(fill(step["verifyCommand"] as? String ?? "", ["importRoot": root]))
                        if !found.isEmpty { break }
                        try await Task.sleep(nanoseconds: 500_000_000)
                    }
                    let expect = step["expectContains"] as? String ?? ""
                    guard found.contains(expect) else {
                        throw Failure("marker \(marker) not found in the guest (\(found.debugDescription)); imported \(receipt.imported)")
                    }
                    detail = "\(receipt.imported.count) imported, marker in \(found.trimmingCharacters(in: .whitespacesAndNewlines))"

                case "presence.pair":
                    detail = try await presencePair(step, fill: fill, scratch: scratch, lane: lane,
                                                    spaceID: spaceID!, first: try await handle())

                case "routine.schedule":
                    detail = try await routineSchedule(step, spec: spec, fill: fill, scratch: scratch,
                                                       client: client, space: spaceID!)

                case "group.chat":
                    detail = try await groupChat(step, spec: spec, fill: fill, client: client, space: spaceID!)

                case "space.delete":
                    // A Space added by address is only forgotten; a created one is deleted.
                    guard let id = spaceID else { throw Skip(reason: "no Space to delete") }
                    let said = try await client.sdkConnection.native?.delete(space: id) ?? ""
                    detail = said
                    spaceID = nil
                    SDKSpacesClient.pinnedSpace = nil

                default:
                    throw Failure("unknown op \(op)")
                }
            } catch let skip as Skip {
                status = "skip"
                detail = skip.reason
            } catch let CuaError.HostCapabilityMissing(message) where op == "teleport.app" {
                // The embedded runtime has no teleport: it ships with Cua Spaces.
                status = "skip"
                detail = message
            } catch {
                status = "fail"
                detail = "\(error)"
            }
            results.append(StepResult(id: id, status: status,
                                      ms: Int(Date().timeIntervalSince(t0) * 1000), detail: detail))
            print("[\(status)] \(id) (\(results.last!.ms) ms) \(detail)")
        }
        // A failed run must still not leave a cloud Space behind.
        if let id = spaceID { _ = try? await client.sdkConnection.native?.delete(space: id) }

        let ok = !results.contains { $0.status == "fail" }
        return Box(value: [
            "impl": "swift", "lane": lane, "ok": ok,
            "totalMs": Int(Date().timeIntervalSince(started) * 1000),
            "steps": results.map { ["id": $0.id, "status": $0.status, "ms": $0.ms, "detail": $0.detail] },
        ])
    }

    // MARK: - Steps

    /// Counts frames; keeps no pixels (bounded by construction).
    final class FrameCounter: FrameSink, @unchecked Sendable {
        private let lock = NSLock()
        private(set) var frames = 0
        private(set) var keyframes = 0
        private(set) var firstIsKey: Bool?
        private(set) var size = (0, 0)
        func onFrame(frame: VideoFrame) {
            lock.lock(); defer { lock.unlock() }
            if firstIsKey == nil { firstIsKey = frame.keyframe }
            frames += 1
            if frame.keyframe { keyframes += 1 }
            size = (Int(frame.width), Int(frame.height))
        }
        func onEvent(event: MediaEvent) {}
        var snapshot: (Int, Int, Bool?, (Int, Int)) {
            lock.lock(); defer { lock.unlock() }
            return (frames, keyframes, firstIsKey, size)
        }
    }

    static func streamDesktop(_ step: [String: Any], space: CuaSpaces.Space) async throws -> String {
        // #region docs:sw-stream
        let provider = SpaceStreamProvider(space: space)
        provider.maxFPS = UInt32(step["maxFps"] as? Int ?? 5)
        let counter = FrameCounter()
        let session = try await provider.openSession(.desktop, frames: counter, audio: nil)
        // #endregion docs:sw-stream
        let pollMs = UInt64(step["pollMs"] as? Int ?? 100)
        let maxPolls = step["maxPolls"] as? Int ?? 300
        func wait(for n: Int) async throws {
            for _ in 0..<maxPolls {
                if counter.snapshot.0 >= n { return }
                try await Task.sleep(nanoseconds: pollMs * 1_000_000)
            }
            throw Failure("only \(counter.snapshot.0) frame(s) arrived")
        }
        do {
            try await wait(for: step["minFrames"] as? Int ?? 1)
            if step["firstFrameKeyframe"] as? Bool == true, counter.snapshot.2 != true {
                throw Failure("the first frame was not a keyframe")
            }
            if step["keyframeRequest"] as? Bool == true {
                try session.requestKeyframe()
                try await wait(for: counter.snapshot.0 + 1)
            }
        } catch {
            _ = try? await session.close()
            throw error
        }
        let stats = try await session.close()
        let s = counter.snapshot
        return "\(s.0) frames (\(s.1) keyframes, first is keyframe), \(s.3.0)x\(s.3.1) \(session.codec()); closed with \(stats.frames) frames"
    }

    static func agentThread(_ step: [String: Any], dir: URL, fill: (String, [String: String]) -> String,
                            client: SDKSpacesClient, space: String,
                            handle: () async throws -> CuaSDK.Space,
                            bash: (String, UInt64) async throws -> String) async throws -> String {
        // Agent runs speak the Agent Client Protocol through the cua-agents
        // runner, so a fake `claude` shell script can no longer stand in for
        // a model. Real harness runs: cua-agents' e2e_live.
        if step["fakeCli"] != nil {
            throw Skip(reason: "the step's fake CLI predates ACP agent runs; real runs are covered by cua-agents' e2e_live")
        }
        let fake = step["fakeCli"] as? [String: String] ?? [:]
        let script = try Data(contentsOf: dir.appendingPathComponent(fake["source"] ?? ""))
        let home = try await handle().home()
        let guestPath = (fake["guestPath"] ?? "~/.local/bin/claude").replacingOccurrences(of: "~", with: home)
        _ = try await bash("mkdir -p '\((guestPath as NSString).deletingLastPathComponent)'", 60_000)
        _ = try await handle().write(path: guestPath, content: script)
        _ = try await bash("chmod +x '\(guestPath)'", 60_000)

        let turns = step["turns"] as? [[String: String]] ?? []
        let tail = step["tail"] as? Int ?? 40
        let pollMs = UInt64(step["pollMs"] as? Int ?? 200)
        let maxPolls = step["maxPolls"] as? Int ?? 300
        SDKSpacesClient.showsAgentWindows = false
        var runID: String?
        var notes: [String] = []
        defer {
            // The app's own teardown for a run (`FRICTION.md` §8).
            if let runID {
                let c = client
                Task.detached { _ = try? await c.deleteRun(space: space, runID: runID) }
            }
        }
        for (i, turn) in turns.enumerated() {
            let expect = fill(turn["expectOutput"] ?? "", [:])
            if let prompt = turn["prompt"] {
                let run = try await client.startBot(space: space, bot: Fixtures.bot("inbox"),
                                                    prompt: fill(prompt, [:]))
                runID = run.runID
            } else if let message = turn["message"], let runID {
                let outcome = try await client.message(space: space, runID: runID,
                                                       text: fill(message, [:]), force: false)
                guard outcome.accepted else { throw Failure("turn \(i + 1) refused: \(outcome.reason)") }
            } else {
                throw Failure("turn \(i + 1) has neither prompt nor message")
            }
            var last: AgentStatus?
            for _ in 0..<maxPolls {
                let s = try await client.status(space: space, runID: runID!, tail: tail)
                last = s
                if s.tail.contains(expect), [.idle, .finished].contains(s.state) { break }
                try await Task.sleep(nanoseconds: pollMs * 1_000_000)
            }
            guard let last, last.tail.contains(expect) else {
                throw Failure("turn \(i + 1): expected \(expect.debugDescription); tail was \(last?.tail.debugDescription ?? "nil")")
            }
            guard [.idle, .finished].contains(last.state) else {
                throw Failure("turn \(i + 1) ended \(last.state) (\(last.reason))")
            }
            notes.append("turn \(i + 1) \(last.state.rawValue)")
        }
        let roster = try await client.listBots(space: space)
        guard roster.contains(where: { $0.id == runID }) else { throw Failure("\(runID!) missing from the roster") }
        return "\(runID!): " + notes.joined(separator: ", ") + "; on the roster"
    }

    static func xorshiftBytes(count: Int, seed: UInt64) -> Data {
        var x = seed
        var out = Data(count: count)
        out.withUnsafeMutableBytes { (buf: UnsafeMutableRawBufferPointer) in
            for i in 0..<count {
                x ^= x &<< 13
                x ^= x &>> 7
                x ^= x &<< 17
                buf[i] = UInt8(truncatingIfNeeded: x)
            }
        }
        return out
    }

    static func sendFile(_ step: [String: Any], fill: (String, [String: String]) -> String, scratch: URL,
                         handle: () async throws -> CuaSDK.Space,
                         bash: (String, UInt64) async throws -> String) async throws -> String {
        let gen = step["generate"] as? [String: Any] ?? [:]
        let seedText = (gen["seed"] as? String ?? "0").replacingOccurrences(of: "0x", with: "")
        guard let seed = UInt64(seedText, radix: 16) else { throw Failure("bad seed \(seedText)") }
        let bytes = xorshiftBytes(count: gen["bytes"] as? Int ?? 0, seed: seed)
        let sha = SHA256.hash(data: bytes).map { String(format: "%02x", $0) }.joined()
        guard sha == step["sha256"] as? String else {
            throw Failure("generated sha \(sha) differs from the spec's \(step["sha256"] ?? "")")
        }
        let file = scratch.appendingPathComponent(fill(gen["name"] as? String ?? "file.bin", [:]))
        try bytes.write(to: file)
        // #region docs:sw-send-file
        let report = try await handle().sendFile(
            localPath: file.path,
            options: SpaceSendFileOptions(targetDirectory: step["subdir"] as? String,
                                          respectIgnoreFiles: true, conflict: "overwrite"))
        // #endregion docs:sw-send-file
        guard report.verified, let sent = report.files.first else { throw Failure("send_file not verified: \(report)") }
        guard sent.sha256 == sha else { throw Failure("the driver hashed \(sent.sha256), expected \(sha)") }
        let guestSha = try await bash(fill(step["guestSha256Command"] as? String ?? "", ["path": sent.path]), 120_000)
            .trimmingCharacters(in: .whitespacesAndNewlines)
        _ = try? await bash(fill(step["cleanupCommand"] as? String ?? "true", ["path": sent.path]), 60_000)
        guard guestSha == sha else { throw Failure("guest sha256 \(guestSha) != \(sha)") }
        return "\(sent.path) \(report.bytes) bytes, sha256 \(sha.prefix(12))… verified by the guest"
    }

    /// Approves the manifest's defaults, acknowledging sensitive items: the
    /// profile is generated. Consent is still an explicit callback.
    final class ApproveDefaults: TeleportApprover, @unchecked Sendable {
        func approve(manifest: CuaSDK.TeleportManifest) -> TeleportDecision? {
            TeleportDecision(include: nil, acknowledgeSensitive: true)
        }
    }

    static func presencePair(_ step: [String: Any], fill: (String, [String: String]) -> String,
                             scratch: URL, lane: String, spaceID: String,
                             first: CuaSDK.Space) async throws -> String {
        let clients = step["clients"] as? [[String: Any]] ?? []
        guard clients.count == 2 else { throw Failure("presence.pair needs two clients") }
        // A second, independent runtime with its own registry.
        let other = try SpacesConnection.embedded(spacesHome: scratch.appendingPathComponent("spaces-b").path)
        let env = ProcessInfo.processInfo.environment
        let secondID: SpaceID
        if lane == "cloud" {
            secondID = SpaceID(spaceID)
        } else {
            secondID = try await other.add(url: env["OPENKOALABOTS_SCENARIO_URL"] ?? "",
                                           token: env["OPENKOALABOTS_SCENARIO_TOKEN"],
                                           name: "openkoalabots-presence-b").id
        }
        guard let second = try await other.nativeSpace(secondID) else { throw Failure("no second handle") }
        func identity(_ c: [String: Any]) -> PresenceIdentity {
            let id = fill(c["id"] as? String ?? "", [:])
            let name = c["displayName"] as? String ?? ""
            // An agent joins the way the app's Bots do: with its stable color.
            if c["agent"] as? Bool == true { return PresenceColors.agentIdentity(id: id, displayName: name) }
            return PresenceIdentity(id: id, displayName: name, color: "", agent: false)
        }
        let koalaPrincipal = fill(clients[1]["id"] as? String ?? "", [:])
        let timeout = UInt64(step["timeoutMs"] as? Int ?? 20_000)
        let maxEvents = step["maxEvents"] as? Int ?? 50
        // With takeKoalaColor the operator asks for Koala's stable color first,
        // so the server has to assign Koala another one.
        let takeKoalaColor = step["takeKoalaColor"] as? Bool == true
        var operatorIdentity = identity(clients[0])
        if takeKoalaColor { operatorIdentity.color = PresenceColors.color(for: koalaPrincipal) }
        let operatorP = try await first.joinPresence(identity: operatorIdentity, timeoutMs: timeout)
        defer { Task.detached { try? await operatorP.leave() } }
        let koala = try await second.joinPresence(identity: identity(clients[1]), timeoutMs: timeout)
        let koalaID = try await koala.me().participantId
        let operatorID = try await operatorP.me().participantId
        guard try await koala.roster().contains(where: { $0.participant.participantId == operatorID }) else {
            throw Failure("Koala's roster does not list the operator")
        }
        // The operator folds every event into the SDK's roster, as the app's
        // Computer pane does to draw the others' cursors.
        var roster = PresenceRoster()
        _ = roster.apply(me: try await operatorP.me(), members: try await operatorP.roster())
        func expectEvent(_ what: String, _ matches: (CuaSDK.PresenceEvent) -> Bool) async throws -> CuaSDK.PresenceEvent {
            do {
                return try await operatorP.waitFor(timeoutMs: timeout, maxEvents: maxEvents,
                                                   roster: &roster, where: matches)
            } catch {
                throw Failure("the operator never saw \(what)")
            }
        }
        let joined = try await expectEvent("Koala join") { $0.kind == "joined" && $0.participant?.participantId == koalaID }
        guard joined.participant?.kind == "agent" else { throw Failure("Koala joined as \(joined.participant?.kind ?? "?")") }
        let c = step["cursor"] as? [String: Double] ?? [:]
        let (x, y) = (c["x"] ?? 0.25, c["y"] ?? 0.75)
        try await koala.updateCursor(cursor: PresenceCursor(displayId: "", windowId: nil, x: x, y: y, visible: true))
        let moved = try await expectEvent("Koala's cursor") { $0.kind == "cursor_moved" && $0.participantId == koalaID }
        guard let at = moved.cursor, abs(at.x - x) < 1e-6, abs(at.y - y) < 1e-6 else {
            throw Failure("cursor arrived at \(String(describing: moved.cursor))")
        }
        let checkRoster = step["roster"] as? Bool == true
        var cursorColor = ""
        if checkRoster {
            guard let seen = roster.others.first(where: { $0.id == koalaID }), seen.isAgent,
                  let at = seen.normalizedCursor(in: CGSize(width: 1, height: 1)),
                  abs(at.x - x) < 1e-6, abs(at.y - y) < 1e-6 else {
                throw Failure("the operator's roster does not show Koala's cursor: \(roster.others)")
            }
            // One color for the cursor and the avatar's background: the app's
            // avatar color, fed from this roster the way the Computer pane
            // feeds it, must be exactly the cursor's.
            let present = Array(roster.participants.values)
            await MainActor.run { PresenceColorBook.shared.update(present) }
            let avatar = BotPresenceColor.hex(for: koalaPrincipal)
            let stable = PresenceColors.color(for: koalaPrincipal)
            guard avatar == seen.color.lowercased() else {
                throw Failure("Koala's cursor is \(seen.color), its avatar is \(avatar)")
            }
            if takeKoalaColor, avatar == stable {
                throw Failure("the operator holds \(stable), yet Koala kept it")
            }
            cursorColor = avatar + (avatar == stable ? "" : ", reassigned from \(stable)")
        }
        try await koala.leave()
        _ = try await expectEvent("Koala leave") { $0.kind == "left" && $0.participantId == koalaID }
        if checkRoster, roster.participants[koalaID] != nil {
            throw Failure("Koala is still on the operator's roster after leaving")
        }
        try await operatorP.leave()
        if lane != "cloud" { try? await other.remove(secondID) }
        return "operator saw joined(agent) → cursor_moved(\(x), \(y)) → left"
            + (checkRoster ? "; roster showed Koala's cursor (\(cursorColor), its avatar's background), then dropped Koala" : "")
    }

    // MARK: - Routines and group chats

    /// The model endpoint from the spec, or a skip when the lane has none.
    static func endpoint(_ spec: [String: Any]) throws -> AgentEndpoint {
        let model = spec["model"] as? [String: Any] ?? [:]
        let urlEnv = model["urlEnv"] as? String ?? "OPENKOALABOTS_SCENARIO_MODEL_URL"
        guard let url = ProcessInfo.processInfo.environment[urlEnv], !url.isEmpty else {
            throw Skip(reason: "no model endpoint (\(urlEnv) is unset)")
        }
        return AgentEndpoint(baseURL: url, model: model["name"] as? String,
                             envFromHost: [model["keyVar"] as? String ?? "ANTHROPIC_API_KEY"])
    }

    static func scenarioBot(_ spec: [String: Any], fill: (String, [String: String]) -> String,
                            index: Int) -> Bot {
        let look = Fixtures.bots[index % Fixtures.bots.count]
        return Bot(id: fill(spec["id"] as? String ?? "bot-\(index)", [:]),
                   name: spec["name"] as? String ?? "Bot \(index)",
                   shape: look.shape, colorHex: look.colorHex,
                   preview: "", timestamp: "", screenIndex: 0)
    }

    /// Polls until `done`, bounded by `maxPolls`.
    @MainActor
    static func poll(_ maxPolls: Int, every ms: UInt64, _ done: () async throws -> Bool) async throws -> Bool {
        for _ in 0..<max(1, maxPolls) {
            if try await done() { return true }
            try await Task.sleep(nanoseconds: ms * 1_000_000)
        }
        return false
    }

    /// `routine.schedule`: the app's `RoutineStore` and `BotStoreRoutineRunner`
    /// fire a real agent turn on schedule, once.
    @MainActor
    static func routineSchedule(_ step: [String: Any], spec: [String: Any],
                                fill: @escaping (String, [String: String]) -> String, scratch: URL,
                                client: SDKSpacesClient, space: String) async throws -> String {
        var started: [String] = []
        do {
            let r = try await routineBody(step, spec: spec, fill: fill, scratch: scratch, client: client,
                                          space: space, started: &started)
            await cleanUp(started, client: client, space: space)
            return r
        } catch {
            await cleanUp(started, client: client, space: space)
            throw error
        }
    }

    /// Stops and deletes the runs a step started, before the step returns:
    /// a detached cleanup would die with this process.
    static func cleanUp(_ runs: [String], client: SDKSpacesClient, space: String) async {
        for run in runs { _ = try? await client.deleteRun(space: space, runID: run) }
    }

    @MainActor
    static func routineBody(_ step: [String: Any], spec: [String: Any],
                                fill: @escaping (String, [String: String]) -> String, scratch: URL,
                                client: SDKSpacesClient, space: String,
                                started: inout [String]) async throws -> String {
        SDKSpacesClient.agentEndpoint = try endpoint(spec)
        SDKSpacesClient.showsAgentWindows = false
        let bots = BotStore(client: client)
        await bots.connect()
        let bot = scenarioBot(step["bot"] as? [String: Any] ?? [:], fill: fill, index: 0)
        bots.register(bot)
        let file = scratch.appendingPathComponent("routines.json")
        let routines = RoutineStore(fileURL: file, runner: BotStoreRoutineRunner(store: bots))
        let spec_ = step["routine"] as? [String: Any] ?? [:]
        let sched = spec_["schedule"] as? [String: Any] ?? [:]
        let schedule = try JSONDecoder().decode(RoutineSchedule.self,
                                                from: JSONSerialization.data(withJSONObject: sched))
        let createdAgo = Double(step["createdAgoMs"] as? Int ?? 61_000) / 1000
        let created = Date().addingTimeInterval(-createdAgo)
        let routine = routines.create(botID: bot.id, title: fill(spec_["title"] as? String ?? "", [:]),
                                      prompt: fill(spec_["prompt"] as? String ?? "", [:]),
                                      schedule: schedule, now: created)
        defer { routines.stopScheduler() }
        let notDue = routine.createdAt.addingTimeInterval(Double(step["notDueAtMs"] as? Int ?? 30_000) / 1000)
        guard routines.due(at: notDue).isEmpty else { throw Failure("the routine was due before its first slot") }

        let interval = step["schedulerIntervalMs"] as? Int ?? 250
        routines.startScheduler(every: .milliseconds(interval))
        let fired = try await poll(240, every: UInt64(interval)) { routines.routine(routine.id)?.lastFiredAt != nil }
        routines.stopScheduler()
        guard fired else { throw Failure("the scheduler never fired the routine") }
        let records = routines.log.filter { $0.routineID == routine.id }
        guard records.count == 1, case .started(let run) = records[0].firing else {
            throw Failure("expected one started firing, got \(records.map(\.firing.summary))")
        }
        started.append(run)
        // Right after the firing, not after the reply: the first turn installs
        // the harness, which can outlast the one-minute slot.
        guard (await routines.tick()).isEmpty else { throw Failure("a second tick fired again") }

        let expectPrompt = fill(step["expectPrompt"] as? String ?? "", [:])
        let expectOutput = fill(step["expectOutput"] as? String ?? "", [:])
        var last: AgentStatus?
        let settled = try await poll(step["maxPolls"] as? Int ?? 600,
                                     every: UInt64(step["pollMs"] as? Int ?? 1000)) {
            let s = try await client.status(space: space, runID: run, tail: 200)
            last = s
            return s.tail.contains(expectOutput) && [.idle, .finished].contains(s.state)
        }
        guard settled, let last else {
            throw Failure("the routine's run never answered \(expectOutput.debugDescription): \(last?.state.rawValue ?? "?") \(last?.tail.suffix(300) ?? "")")
        }
        guard last.tail.contains(expectPrompt) else {
            throw Failure("the run's prompt does not start with \(expectPrompt.debugDescription): \(last.tail.suffix(400))")
        }
        let reloaded = RoutineStore(fileURL: file)
        // Not due again at the instant it fired: the slot was used, and the
        // next one is a full interval later.
        guard let back = reloaded.routine(routine.id), back.lastRunID == run,
              let firedAt = back.lastFiredAt, !back.isDue(at: firedAt) else {
            throw Failure("the reloaded store lost the firing: \(String(describing: reloaded.routine(routine.id)))")
        }
        return "\(routine.schedule.label): fired once as \(run) (\(last.state.rawValue)); reply in the tail; no backlog; persisted"
    }

    /// `group.chat`: the app's `GroupChatStore` over `BotStoreGroupMessenger`,
    /// two real Bots.
    @MainActor
    static func groupChat(_ step: [String: Any], spec: [String: Any],
                          fill: @escaping (String, [String: String]) -> String,
                          client: SDKSpacesClient, space: String) async throws -> String {
        var started: [String] = []
        do {
            let r = try await groupBody(step, spec: spec, fill: fill, client: client, space: space,
                                        started: &started)
            await cleanUp(started, client: client, space: space)
            return r
        } catch {
            await cleanUp(started, client: client, space: space)
            throw error
        }
    }

    @MainActor
    static func groupBody(_ step: [String: Any], spec: [String: Any],
                          fill: @escaping (String, [String: String]) -> String,
                          client: SDKSpacesClient, space: String,
                          started: inout [String]) async throws -> String {
        SDKSpacesClient.agentEndpoint = try endpoint(spec)
        SDKSpacesClient.showsAgentWindows = false
        let bots = BotStore(client: client)
        await bots.connect()
        let members = (step["bots"] as? [[String: Any]] ?? []).enumerated().map {
            scenarioBot($0.element, fill: fill, index: $0.offset + 1)
        }
        guard members.count == 2 else { throw Failure("group.chat needs two bots") }
        members.forEach { bots.register($0) }
        let groups = GroupChatStore(messenger: BotStoreGroupMessenger(store: bots))
        var refusals: [String] = []
        for n in step["rejectSizes"] as? [Int] ?? [] {
            let ids = (0..<n).map { "seat-\($0)" }
            do {
                _ = try groups.create(title: "x", members: ids)
                throw Failure("a group of \(n) was accepted")
            } catch let e as GroupChatError {
                switch (n, e) {
                case (_, .tooFewBots) where n < GroupChat.minBots, (_, .tooManyBots) where n > GroupChat.maxBots:
                    refusals.append("\(n)")
                default: throw Failure("a group of \(n) was refused with \(e)")
                }
            }
        }
        let chat = try groups.create(title: fill(step["title"] as? String ?? "Group", [:]),
                                     members: members.map(\.id))
        for m in members {
            let framed = GroupChatStore.frame("x", for: m.id, in: chat, names: groups.displayName)
            let other = members.first { $0.id != m.id }!.name
            guard framed.contains(other) else { throw Failure("\(m.name)'s framing does not name \(other)") }
        }
        let message = fill(step["message"] as? String ?? "", [:])
        let deliveries = await groups.send(message, in: chat.id)
        started += members.compactMap { bots.runID(for: $0.id) }
        guard deliveries.count == 2, deliveries.allSatisfy(\.accepted) else {
            throw Failure("deliveries: \(deliveries)")
        }
        let expect = fill(step["expectReply"] as? String ?? "", [:])
        func answered(_ id: String) -> Bool {
            groups.chat(chat.id)?.messages.contains {
                $0.speaker == .bot(id) && !$0.undelivered && $0.text.contains(expect)
            } ?? false
        }
        let both = try await poll(step["maxPolls"] as? Int ?? 600,
                                  every: UInt64(step["pollMs"] as? Int ?? 1000)) {
            await groups.collectReplies(in: chat.id)
            // Settled too: a Bot still producing output may say more.
            return members.allSatisfy { answered($0.id) } && groups.workingBots(in: chat.id).isEmpty
        }
        guard both else {
            let lines = groups.chat(chat.id)?.messages.map { "\($0.speaker): \($0.text)" } ?? []
            throw Failure("not every Bot answered \(expect.debugDescription): \(lines)")
        }
        let again = await groups.collectReplies(in: chat.id)
        guard again.isEmpty else {
            throw Failure("collecting again added \(again.map { "\($0.speaker): \($0.text)" })")
        }
        return "refused sizes \(refusals.joined(separator: ", ")); \(chat.membershipLabel); both delivered; "
            + members.map { "\($0.name) answered" }.joined(separator: ", ") + "; no duplicates"
    }
}
