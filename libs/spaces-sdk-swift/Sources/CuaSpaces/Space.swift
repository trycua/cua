import CoreGraphics
import Cua
import Foundation

/// A handle on one Space.
///
/// It carries its own id, so the id is not restated on every call —
/// `FRICTION.md` §7: *"Every call restates the Space id."* Everything a product
/// does to a Space hangs off this type.
public struct Space: Sendable, Identifiable {

    public let info: SpaceInfo
    let connection: SpacesConnection
    /// The guest's own `$HOME`, as its shell reported it at attach (cua SDK
    /// backends). `nil` falls back to the provider's documented home.
    let guestHome: String?

    init(info: SpaceInfo, connection: SpacesConnection, guestHome: String? = nil) {
        self.info = info
        self.connection = connection
        self.guestHome = guestHome.flatMap { $0.isEmpty ? nil : $0 }
    }

    /// The generated `CuaSDK.Space` behind this handle (cua SDK backends), for
    /// anything this overlay does not model: typed files, streams, presence,
    /// hotspot. `nil` against a test fake.
    public func native() async throws -> CuaSDK.Space? {
        try await connection.nativeSpace(id)
    }

    public var id: SpaceID { info.id }
    public var provider: SpaceProvider { info.provider }
    public var state: SpaceState { info.state }
    public var isReady: Bool { info.isReady }
    /// `$HOME` inside the Space (§6): the guest's own answer when the SDK
    /// could ask, else the provider's documented home.
    public var home: String { guestHome ?? info.home }
    public var capabilities: ProviderCapabilities { info.capabilities }

    private var spaceArg: [String: JSONValue] { ["space": .string(id.rawValue)] }

    func args(_ extra: [String: JSONValue]) -> [String: JSONValue] {
        extra.merging(spaceArg) { a, _ in a }
    }

    /// Re-read this Space's own row, e.g. after waiting for it to come up.
    public func refreshed() async throws -> Space {
        try await connection.attach(to: id, requireReady: false)
    }

    // MARK: - Agent runs

    /// Start an agent. The returned handle is the only thing a caller needs
    /// afterwards.
    public func startAgent(_ request: AgentStartRequest) async throws -> AgentRun {
        guard capabilities.agents else {
            throw SpacesError.unsupportedByProvider(
                tool: "agent_start", provider: provider, detail: "no agent harness")
        }
        var payload: [String: JSONValue] = [
            "agent": .string(request.agent),
            "prompt": .string(RunMetadata.encode(prompt: request.prompt,
                                                 metadata: request.metadata)),
            "show": .bool(request.showsWindow),
        ]
        if let endpoint = request.endpoint {
            payload["base_url"] = .string(endpoint.baseURL)
            if let model = endpoint.model { payload["model"] = .string(model) }
            if !endpoint.envFromHost.isEmpty {
                payload["env_from_host"] = .array(endpoint.envFromHost.map { .string($0) })
            }
        }
        if let timeout = request.timeout {
            // Reserved. No backend reads this key today
            // (`capabilities.serverBackstop == false`), and an older server
            // ignores an unknown key, so sending it is free and the day a
            // server honours it nothing at the call site changes.
            payload["timeout_seconds"] = .number(
                Double(timeout.components.seconds)
                    + Double(timeout.components.attoseconds) / 1e18)
        }
        let d = try await connection.object("agent_start", args(payload))
        guard let raw = d["run_id"]?.stringValue, !raw.isEmpty else {
            throw SpacesError.malformedResponse(tool: "agent_start", detail: "no run_id in \(d)")
        }
        await connection.noteStarted(RunID(raw))
        // The first turn's boundary is the moment the run started.
        await connection.openTurn(on: RunID(raw), message: nil, delivery: nil)
        return AgentRun(id: RunID(raw),
                        space: self,
                        agent: d["agent"]?.stringValue ?? request.agent,
                        turnModel: TurnModel(d["capabilities"]?.objectValue ?? [:]),
                        metadata: request.metadata,
                        notes: (d["notes"]?.arrayValue ?? []).compactMap(\.stringValue))
    }

    /// Convenience for the common case.
    public func startAgent(prompt: String, agent: String = "claude-code",
                           metadata: [String: String] = [:],
                           showsWindow: Bool = true) async throws -> AgentRun {
        try await startAgent(AgentStartRequest(agent: agent, prompt: prompt,
                                               metadata: metadata, showsWindow: showsWindow))
    }

    /// Start an agent, hand it to `body`, and **delete the whole run** on every
    /// exit path including a thrown error or a cancellation.
    ///
    /// `FRICTION.md` §8: cleanup is four guest paths the app should not know,
    /// and a suite that must leave a demo Space pristine had to know all four.
    public func withAgentRun<T: Sendable>(
        _ request: AgentStartRequest,
        _ body: @Sendable (AgentRun) async throws -> T
    ) async throws -> T {
        let run = try await startAgent(request)
        do {
            let value = try await body(run)
            _ = try? await run.delete()
            return value
        } catch {
            _ = try? await run.delete()
            throw error
        }
    }

    /// Every run in this Space, as `RunSnapshot` — the **same** type
    /// `AgentRun.status()` returns (`FRICTION.md` §22). This is the cheap call:
    /// it omits the output tail (`outputTail == nil`) but never the reason.
    public func runs() async throws -> [RunSnapshot] {
        let rows = try await connection.array("agent_list", spaceArg, unwrapping: "runs")
        var out: [RunSnapshot] = []
        for row in rows.compactMap(\.objectValue) {
            let id = RunID(row["run_id"]?.stringValue ?? "")
            guard !id.isEmpty else { continue }
            var enriched = row
            // `agent_list` echoes the prompt as `summary`, marker and all.
            if let summary = row["summary"]?.stringValue {
                enriched["summary"] = .string(RunMetadata.strip(summary))
                // The carrier is kept alongside the readable projection, so a
                // relaunched app can recover the metadata it started a run with.
                enriched["raw_summary"] = .string(summary)
            }
            let snapshot = RunSnapshot.decode(
                enriched, id: id, space: self.id, requestedTail: nil,
                carryingReasonFrom: await connection.lastSnapshot(id))
            await connection.remember(snapshot)
            out.append(snapshot)
        }
        return out
    }

    /// A handle on a run that already exists, e.g. one recovered from `runs()`
    /// after a relaunch.
    public func run(_ id: RunID) -> AgentRun {
        AgentRun(id: id, space: self, agent: "", turnModel: nil, metadata: [:], notes: [])
    }

    /// One poll for the whole roster.
    ///
    /// `FRICTION.md` §2 and §25: status wants to be a subscription, and the
    /// cost is per run while a roster needs every Bot at once — *"Nine Bots on
    /// the home screen is ten round trips per tick"*. This is the SDK being the
    /// only thing that polls: one `agent_list` per tick regardless of roster
    /// size, output fetched only for the runs a caller actually asked to watch,
    /// and change detection inside.
    public func roster(pollingEvery interval: Duration = .seconds(3),
                       detailed: Set<RunID> = [],
                       tail: Int = 400) -> RosterStream {
        RosterStream(space: self, interval: interval, detailed: detailed, tail: tail)
    }

    // MARK: - Windows

    /// The icon the Space's desktop shows for a window's app, or `nil` when
    /// the Space has none or is not backed by the cua SDK. Show no icon
    /// then, never a placeholder. A window list asks ``appIcons(_:)`` once.
    public func appIcon(app: String, appID: String, processID: UInt32) async throws -> AppIcon? {
        try await appIcons([AppIconRequest(app: app, appID: appID, processID: processID)]).first ?? nil
    }

    /// Icons for many windows' apps, in request order, from the cua SDK's
    /// one icon cache (`Space.appIcons`: memory, then `$CUA_HOME/cache/icons`,
    /// every miss in one guest round trip). Keep no cache of your own: ask
    /// again whenever the rows change; a cached answer costs microseconds.
    public func appIcons(_ requests: [AppIconRequest]) async throws -> [AppIcon?] {
        guard let native = try await native() else { return requests.map { _ in nil } }
        return try await native.appIcons(requests: requests.map {
            SpaceAppIconRequest(appName: $0.app, appId: $0.appID, pid: $0.processID)
        }).map { $0.map { AppIcon(data: $0.bytes, contentType: $0.contentType, data1x: $0.bytes1x) } }
    }

    /// Every window in the Space, each already carrying the rcdp target that
    /// streams it — one spelling, never empty by accident (§7).
    public func windows() async throws -> [SpaceWindow] {
        try await connection.array("list_space_windows", spaceArg, unwrapping: "windows")
            .compactMap(\.objectValue)
            .map(SpaceWindow.init(row:))
            .filter { !$0.id.isEmpty }
    }

    /// The window a run is working in, or `nil` when the join cannot be made.
    ///
    /// `FRICTION.md` §37: *"`list_space_windows` on the live Space returns 92
    /// entries. Sixty-odd of them are titled some variant of `lume —
    /// watch.command — tail -f out.log`… The product question — which window is
    /// this Bot working in? — has no answer in that payload."*
    ///
    /// §37 says how to answer it: `agent_start` knows the process it spawned
    /// and the window list knows the owning pid, so joining the two turns an
    /// unanswerable question into a field. The SDK performs that join — reading
    /// the run's own pid from its run directory and matching it against the
    /// window list's owner — and returns `nil` honestly when either side does
    /// not publish enough to make it. It never guesses by title.
    public func window(for run: RunID) async throws -> SpaceWindow? {
        let windows = try await windows()
        guard windows.contains(where: { $0.processID != nil }) else { return nil }
        guard let pid = try await runProcessID(run) else { return nil }
        // The agent's own pid, its shell's, or its terminal's may own the
        // window; match the run's whole process group.
        let family = try await processFamily(of: pid)
        return windows.first { w in w.processID.map(family.contains) ?? false }
    }

    private func runProcessID(_ run: RunID) async throws -> Int? {
        let text = try await bash("cat \(run.directory)/pid 2>/dev/null || true")
        return Int(text.trimmingCharacters(in: .whitespacesAndNewlines))
    }

    private func processFamily(of pid: Int) async throws -> Set<Int> {
        let text = try await bash("ps -Ao pid=,ppid= 2>/dev/null || true")
        var parent: [Int: Int] = [:]
        for line in text.split(separator: "\n") {
            let parts = line.split(separator: " ", omittingEmptySubsequences: true)
            if parts.count >= 2, let p = Int(parts[0]), let pp = Int(parts[1]) { parent[p] = pp }
        }
        var family: Set<Int> = [pid]
        // Walk each process up to the root, claiming anything descended from
        // the run.
        for p in parent.keys {
            var cursor = p
            var hops = 0
            while let up = parent[cursor], hops < 64 {
                if up == pid { family.insert(p); break }
                cursor = up
                hops += 1
            }
        }
        return family
    }

    // MARK: - Files

    /// Push a file into the Space and report **the path it actually wrote**.
    ///
    /// `FRICTION.md` §13 and §14: the raw tool clobbers silently, returns no
    /// path, and publishes no limits.
    @discardableResult
    public func upload(_ localFile: URL, to placement: UploadPlacement = .collisionSafeDefault,
                       limits: TransferLimits = .none) async throws -> RemoteFile {
        try await upload([localFile], to: placement, limits: limits)[0]
    }

    /// Push several files as one batch, so a count or total-size limit means
    /// something. Every limit is checked before any byte moves.
    @discardableResult
    public func upload(_ localFiles: [URL], to placement: UploadPlacement = .collisionSafeDefault,
                       limits: TransferLimits = .none) async throws -> [RemoteFile] {
        guard capabilities.upload else {
            throw SpacesError.unsupportedByProvider(
                tool: "upload", provider: provider, detail: "no file transport")
        }
        for url in localFiles where !FileManager.default.fileExists(atPath: url.path) {
            throw SpacesError.localFileUnavailable(url.path)
        }
        try limits.check(localFiles)

        var written: [RemoteFile] = []
        for url in localFiles {
            let dest = try await destination(for: url, placement: placement)
            _ = try await connection.callTool("upload", args([
                "path": .string(url.path), "dest": .string(dest),
            ]))
            let size = (try? url.resourceValues(forKeys: [.fileSizeKey]).fileSize) ?? nil
            written.append(RemoteFile(path: dest, name: url.lastPathComponent, byteCount: size))
        }
        return written
    }

    private func destination(for url: URL, placement: UploadPlacement) async throws -> String {
        let name = url.lastPathComponent
        switch placement {
        case let .exactPath(path):
            return path
        case let .clobbering(directory):
            let dir = directory.isEmpty ? provider.defaultUploadDirectory : directory
            _ = try await bash("mkdir -p '\(escaped(dir))'")
            return "\(dir)/\(name)"
        case let .collisionSafe(directory):
            let dir = directory.isEmpty ? provider.defaultUploadDirectory : directory
            // A unique *directory*, not a mangled filename: two drops of
            // notes.txt stay two files, and the agent still sees `notes.txt`.
            let slot = "\(dir)/\(UUID().uuidString.prefix(8).lowercased())"
            _ = try await bash("mkdir -p '\(escaped(slot))'")
            return "\(slot)/\(name)"
        }
    }

    /// Bring a file back out, and report where it actually landed.
    ///
    /// §4: the local path answers with JSON and the cloud path with prose. The
    /// SDK reads both and returns a `URL` either way.
    @discardableResult
    public func download(_ remotePath: String, into directory: URL? = nil) async throws -> URL {
        // Expanded for the same reason `fileExists` expands: a guest path the
        // caller wrote with `~` must mean the Space's home, not a directory
        // literally named `~`.
        var a: [String: JSONValue] = ["path": .string(guestPath(remotePath))]
        if let directory { a["dest"] = .string(directory.path) }
        let out = try await connection.callTool("download", args(a))
        if let dest = out["dest"]?.stringValue, !dest.isEmpty {
            return URL(fileURLWithPath: dest)
        }
        let dir = directory
            ?? URL(fileURLWithPath: NSString(string: "~/Downloads/cua-spaces").expandingTildeInPath)
        return dir.appendingPathComponent((remotePath as NSString).lastPathComponent)
    }

    /// Delete a file inside the Space.
    public func removeFile(_ remotePath: String) async throws {
        _ = try await bash("rm -f '\(escaped(guestPath(remotePath)))'")
    }

    /// Whether a path exists inside the Space.
    ///
    /// Three things were wrong with the obvious implementation, and all three
    /// are fixed here.
    ///
    /// 1. **The answer was a substring search.** `out.contains("yes")` is true
    ///    for any output containing those three letters anywhere — including a
    ///    shell diagnostic, or a path with `yes` in it. The reply is now a
    ///    per-call nonce that cannot appear by coincidence.
    /// 2. **A `~` never expanded.** Inside single quotes the shell does not
    ///    expand a tilde, so `test -e '~/x'` asked about a directory literally
    ///    named `~`, and every `~` path answered "missing". Paths are expanded
    ///    against the Space's own `$HOME` before quoting.
    /// 3. **An unreadable answer was reported as `false`.** A failed probe is
    ///    not an absence. It throws now.
    public func fileExists(_ remotePath: String) async throws -> Bool {
        let nonce = UUID().uuidString.prefix(12)
        let out = try await bash(
            "test -e '\(escaped(guestPath(remotePath)))' "
            + "&& printf '%s' 'E\(nonce)' || printf '%s' 'M\(nonce)'")
        if out.contains("E\(nonce)") { return true }
        if out.contains("M\(nonce)") { return false }
        throw SpacesError.malformedResponse(
            tool: "space_bash",
            detail: "existence probe for \(remotePath) answered neither way: \(out)")
    }

    /// A path as the Space's shell will read it: `~` expanded to the
    /// provider's own home, because quoting a tilde stops it expanding.
    func guestPath(_ path: String) -> String {
        if path == "~" { return home }
        if path.hasPrefix("~/") { return home + String(path.dropFirst(1)) }
        return path
    }

    // MARK: - Shell

    /// Run a command inside the Space and return its output.
    @discardableResult
    public func bash(_ command: String) async throws -> String {
        // The SDK's typed call returns stdout on its own; the contract tool
        // renders `stdout[stderr]…[exit N]` for a model to read.
        if let handle = try await native() {
            SpacesCallCounter.record("space_bash")
            do {
                return try await handle.bash(command: command, timeoutMs: nil).stdout
            } catch let error as CuaError {
                throw SpacesError(cua: error, tool: "space_bash")
            }
        }
        let out = try await connection.callTool("space_bash", args(["command": .string(command)]))
        if let o = out.objectValue {
            return o["stdout"]?.stringValue ?? o["output"]?.stringValue ?? out.description
        }
        return out.stringValue ?? out.description
    }

    private func escaped(_ s: String) -> String {
        s.replacingOccurrences(of: "'", with: "'\\''")
    }

    // MARK: - Presentation vs frames

    /// Hand me the Space's **frames**, for a surface inside this product.
    ///
    /// `FRICTION.md` §10: *"`show_space_pip`, `open_space_viewer` and
    /// `stream_space_window` are the three tools whose names say 'show me the
    /// Space'. All three open a window on the machine running the MCP client,
    /// which is exactly what an app embedding the Space in its own UI must not
    /// do … The MCP has no 'give me frames' tool at all."*
    ///
    /// So the SDK separates the two, and this is the one a product needs:
    /// an endpoint and a live token that `CuaSpacesStreaming` decodes into a
    /// view. `present(...)` below is the other one, and it says on its face
    /// that it draws somewhere else.
    public func streamEndpoint(forceRefresh: Bool = false) async throws -> StreamEndpoint {
        if !forceRefresh, let cached = await connection.cachedEndpoint(id) { return cached }
        guard capabilities.rcdpStreaming else {
            throw SpacesError.unsupportedByProvider(
                tool: "stream_endpoint", provider: provider,
                detail: "this provider has no stream")
        }
        // `stream_endpoint` mints a media ticket for any provider: no ssh read
        // of a per-boot token, no `local_rcdp`. The ticket is in the URL.
        let d = try await connection.object("stream_endpoint", spaceArg)
        guard let ws = d["ws_url"]?.stringValue, let parsed = URLComponents(string: ws),
              let host = parsed.host else {
            throw SpacesError.malformedResponse(
                tool: "stream_endpoint", detail: "no usable ws_url in \(d)")
        }
        let ticket = d["ticket"]?.stringValue
            ?? parsed.queryItems?.first { $0.name == "ticket" }?.value ?? ""
        let size = d["frame_size"]?.arrayValue ?? []
        let endpoint = StreamEndpoint(
            host: host, port: parsed.port ?? (parsed.scheme == "wss" ? 443 : 80),
            token: ticket, webSocketURL: ws,
            mediaSessionID: d["media_session_id"]?.stringValue ?? "",
            codec: d["codec"]?.stringValue ?? "",
            frameSize: CGSize(width: size.first?.doubleValue ?? 0,
                              height: size.dropFirst().first?.doubleValue ?? 0),
            needsHeaders: d["needs_gateway_headers"]?.boolValue ?? false)
        await connection.cacheEndpoint(endpoint, for: id)
        return endpoint
    }

    /// Show this Space **to the operator**, on the machine running the backend.
    ///
    /// Named for what it actually does. An app that wants pixels in its own UI
    /// wants `streamEndpoint()`, not this.
    public func present(_ presentation: OperatorPresentation) async throws {
        switch presentation {
        case .pictureInPicture:
            _ = try await connection.callTool("show_space_pip", spaceArg)
        case .hidePictureInPicture:
            _ = try await connection.callTool("hide_space_pip", spaceArg)
        case .viewer:
            _ = try await connection.callTool("open_space_viewer", spaceArg)
        case let .window(id):
            _ = try await connection.callTool(
                "stream_space_window", args(["window_id": .string(id.rawValue)]))
        }
    }
}

/// The operator-facing display surfaces, kept separate from frames (§10).
public enum OperatorPresentation: Sendable, Hashable {
    case pictureInPicture
    case hidePictureInPicture
    case viewer
    case window(WindowID)
}

/// An app icon from the SDK's icon cache: the 64 px PNG (or an SVG the
/// guest could not rasterize) and the 32 px PNG.
public struct AppIcon: Sendable, Hashable {
    /// 64 px PNG (2x), or the SVG document.
    public let data: Data
    public let contentType: String
    /// 32 px PNG (1x); empty for an SVG.
    public let data1x: Data

    public init(data: Data, contentType: String, data1x: Data = Data()) {
        self.data = data
        self.contentType = contentType
        self.data1x = data1x
    }
}

/// One window's app, for ``Space/appIcons(_:)``.
public struct AppIconRequest: Sendable, Hashable {
    public var app: String
    public var appID: String
    public var processID: UInt32

    public init(app: String, appID: String = "", processID: UInt32 = 0) {
        self.app = app
        self.appID = appID
        self.processID = processID
    }
}
