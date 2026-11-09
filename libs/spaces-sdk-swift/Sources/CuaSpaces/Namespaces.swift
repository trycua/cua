import Cua
import Foundation

// The four nouns a OpenKoalaBots developer thinks in — a Space, its agents, its
// files, its screen — plus the one that makes Spaces different, its sessions.
//
// Each namespace is a view onto the `Space` it came from. Nothing here is a
// second implementation: `space.files.upload(…)` is `space.upload(…)`, reached
// by the name a developer looks for it under. The flat calls stay, because the
// sample and the tests use them and a namespace is not worth a breaking change.

extension Space {
    /// Agent threads.
    public var agents: Agents { Agents(space: self) }
    /// File teleport, both directions.
    public var files: Files { Files(space: self) }
    /// Session teleport — arriving already signed in.
    public var sessions: SessionTeleport { SessionTeleport(space: self) }
}

// MARK: - Agents

/// The Space's agents, so a roster screen has one object to hold.
public struct Agents: Sendable {
    let space: Space

    /// Start an agent and return immediately.
    @discardableResult
    public func start(_ prompt: String,
                      using kind: AgentKind = .claudeCode,
                      as name: String? = nil,
                      metadata: [String: String] = [:],
                      showsWindow: Bool = true,
                      timeout: Duration? = nil) async throws -> AgentRun {
        var meta = metadata
        if let name { meta["cua.name"] = name }
        var request = AgentStartRequest(agent: kind.rawValue, prompt: prompt,
                                        metadata: meta, showsWindow: showsWindow)
        request.timeout = timeout
        return try await space.startAgent(request)
    }

    /// Start an agent, wait for it to end, and return everything it said.
    ///
    /// The one-line form. It still deletes the run on every exit path.
    public func run(_ prompt: String,
                    using kind: AgentKind = .claudeCode,
                    timeout: Duration = .seconds(600)) async throws -> OutputPage {
        try await space.withAgentRun(
            AgentStartRequest(agent: kind.rawValue, prompt: prompt)
        ) { run in
            _ = try await run.wait(upTo: timeout) { $0.state.hasEnded }
            return try await run.output()
        }
    }

    /// Every run in the Space, one round trip, `reason` carried for all of
    /// them. `agent_list` takes no `tail`, so this never carries output.
    public func list() async throws -> [RunSnapshot] { try await space.runs() }

    /// Runs this process started. The shape a chat sidebar wants: a roster is
    /// "conversations I have had", not "work happening in the Space".
    public func mine() async throws -> [RunSnapshot] {
        let started = await space.connection.runsStartedHere()
        return try await space.runs().filter { started.contains($0.id) }
    }

    /// Every run, including work this process did not start. Explicit, because
    /// adopting all of them is what fills a sidebar with threads nobody opened.
    public func all() async throws -> [RunSnapshot] { try await space.runs() }

    /// Batch detail in one pass. Still one `agent_status` per named run —
    /// there is no batch tool — but the loop is here rather than in every app,
    /// and the cost is the count of ids you passed.
    public func statuses(for ids: [RunID], tail: Int = 0) async throws -> [RunSnapshot] {
        var out: [RunSnapshot] = []
        for id in ids {
            out.append(try await space.run(id).status(tail: tail))
        }
        return out
    }

    /// A live roster with an explicit refresh policy.
    public func live(_ policy: RosterPolicy = .default) -> RosterStream {
        RosterStream(space: space, policy: policy)
    }

    /// A handle on a run that outlived the process. **Hydrated**: the handle
    /// comes back carrying its agent kind, its turn model and its metadata,
    /// rather than the empty strings a bare `space.run(id)` hands you.
    public func agent(_ id: RunID) async throws -> AgentRun {
        try await space.hydratedRun(id)
    }

    /// Find a run by the application metadata it was started with, rather than
    /// by a marker smuggled into the prompt.
    public func agent(where match: [String: String]) async throws -> AgentRun? {
        for snapshot in try await space.runs() {
            let run = try await space.hydratedRun(snapshot.id)
            if match.allSatisfy({ run.metadata[$0.key] == $0.value }) { return run }
        }
        return nil
    }

    /// Start an agent, hand it to `body`, and delete the whole run on every
    /// exit path including a throw and a cancellation.
    @discardableResult
    public func withAgent<T: Sendable>(_ prompt: String,
                                       using kind: AgentKind = .claudeCode,
                                       metadata: [String: String] = [:],
                                       showsWindow: Bool = true,
                                       _ body: @Sendable (AgentRun) async throws -> T) async throws -> T {
        try await space.withAgentRun(
            AgentStartRequest(agent: kind.rawValue, prompt: prompt,
                              metadata: metadata, showsWindow: showsWindow),
            body)
    }

    /// Recurring work. **Client-side** — see `Scheduler.isServerBacked`, which
    /// is `false`, because `spaces_mcp.py` has no scheduler of any kind.
    public func scheduler(store: ScheduleStore = InMemoryScheduleStore()) -> Scheduler {
        space.scheduler(store: store)
    }
}

// MARK: - Files

/// A file inside a Space: one handle, both directions.
///
/// `space.files.send([url])` returns these, and `url()` brings one back out.
/// There is no separate `download` concept, because a local copy of a remote
/// file is what this *is*.
public struct SpaceFile: Sendable, Hashable, Identifiable {
    public var id: String { path }
    /// Where it actually is in the Space — the path that was written, not the
    /// one you asked for.
    public let path: String
    public let name: String
    public let byteCount: Int?
    let space: Space

    init(path: String, name: String, byteCount: Int?, space: Space) {
        self.path = path
        self.name = name
        self.byteCount = byteCount
        self.space = space
    }

    public static func == (a: SpaceFile, b: SpaceFile) -> Bool {
        a.path == b.path && a.space.id == b.space.id
    }
    public func hash(into hasher: inout Hasher) {
        hasher.combine(path)
        hasher.combine(space.id)
    }

    /// A local URL for this file, fetching it once and caching it.
    public func url() async throws -> URL {
        if let cached = localURL { return cached }
        let url = try await space.download(path)
        SpaceFileCache.shared.store(url, for: path, in: space.id)
        return url
    }

    /// A local URL if one is already on disk, **synchronously** and without a
    /// round trip.
    ///
    /// This exists for exactly one reason: `NSItemProvider` needs a file path
    /// at the instant a drag begins and cannot `await`.
    public var localURL: URL? { SpaceFileCache.shared.url(for: path, in: space.id) }

    /// Start the fetch without waiting, so a later drag is a cache hit.
    public func prefetch() {
        Task { _ = try? await url() }
    }

    public func contents() async throws -> Data { try await Data(contentsOf: url()) }

    public func exists() async throws -> Bool { try await space.fileExists(path) }

    public func delete() async throws { try await space.removeFile(path) }

    /// The `RemoteFile` this file is, for callers holding the older type.
    public var remote: RemoteFile { RemoteFile(path: path, name: name, byteCount: byteCount) }
}

final class SpaceFileCache: @unchecked Sendable {
    static let shared = SpaceFileCache()
    private let lock = NSLock()
    private var urls: [String: URL] = [:]

    private func key(_ path: String, _ space: SpaceID) -> String { "\(space.rawValue)\u{0}\(path)" }

    func url(for path: String, in space: SpaceID) -> URL? {
        lock.lock(); defer { lock.unlock() }
        guard let url = urls[key(path, space)],
              FileManager.default.fileExists(atPath: url.path) else { return nil }
        return url
    }

    func store(_ url: URL, for path: String, in space: SpaceID) {
        lock.lock(); defer { lock.unlock() }
        urls[key(path, space)] = url
    }
}

/// Files in and out of the Space.
public struct Files: Sendable {
    let space: Space

    /// Copy files in, non-clobbering by default, reporting the paths actually
    /// written.
    @discardableResult
    public func send(_ locals: [URL],
                     to placement: UploadPlacement = .collisionSafeDefault,
                     within limits: TransferLimits = .none) async throws -> [SpaceFile] {
        try await space.upload(locals, to: placement, limits: limits)
            .map { SpaceFile(path: $0.path, name: $0.name, byteCount: $0.byteCount, space: space) }
    }

    @discardableResult
    public func send(_ local: URL,
                     to placement: UploadPlacement = .collisionSafeDefault,
                     within limits: TransferLimits = .none) async throws -> SpaceFile {
        try await send([local], to: placement, within: limits)[0]
    }

    /// A handle on a file in the Space. Cheap and synchronous: nothing is
    /// fetched until you ask for `url()`.
    public func file(_ path: String) -> SpaceFile {
        SpaceFile(path: path, name: (path as NSString).lastPathComponent,
                  byteCount: nil, space: space)
    }

    public func exists(_ path: String) async throws -> Bool { try await space.fileExists(path) }

    public func remove(_ path: String) async throws { try await space.removeFile(path) }

    /// The transfer caps.
    ///
    /// `isServerPublished` is `false`: no tool publishes limits, and the only
    /// server-side cap is a hard-coded 25 MB per file. This returns the SDK's
    /// own numbers and says so, rather than presenting them as policy the
    /// server agreed to.
    public func limits() async throws -> TransferLimits { .conservativeDefault }
}

// MARK: - Recovering a real handle

extension Space {
    /// A handle on an existing run, **hydrated** from the server.
    ///
    /// `Space.run(_:)` is the cheap, synchronous form and returns
    /// `agent: ""` — an honest empty, but an empty. This one costs a round
    /// trip and contractually comes back with the agent kind and the metadata
    /// the run was started with, which is what an app relaunching into an
    /// existing roster actually needs.
    public func hydratedRun(_ id: RunID) async throws -> AgentRun {
        let bare = run(id)
        let snapshot = try await bare.status(tail: 0)
        guard snapshot.state != .unknown || !snapshot.reason.isEmpty else {
            throw SpacesError.runNotFound(id)
        }
        let metadata = RunMetadata.decode(from: snapshot.rawPrompt ?? snapshot.summary)
        return AgentRun(id: id, space: self, agent: snapshot.agent,
                        turnModel: nil, metadata: metadata, notes: [])
    }

    /// Attach to a Space, hand it to `body`, and drop the connection on every
    /// exit path. The documented form.
    ///
    /// `attach` cannot create and cannot bill; `createSpace(options:)` can, and
    /// takes where it runs (and so whether it is metered) as a required argument.
    @discardableResult
    public static func attach<T: Sendable>(
        _ id: SpaceID,
        using connection: SpacesConnection,
        requireReady: Bool = true,
        _ body: @Sendable (Space) async throws -> T
    ) async throws -> T {
        let space = try await connection.attach(to: id, requireReady: requireReady)
        return try await body(space)
    }

    /// Reserve the server backstop.
    ///
    /// No backend implements an idle timeout — `capabilities.serverBackstop` is
    /// `false` — so this throws rather than pretending. It exists now so that
    /// the day a server can end a Space nobody is watching, apps gain it
    /// without a breaking change.
    public func setIdleTimeout(_ timeout: Duration?) async throws {
        guard capabilities.serverBackstop else {
            throw SpacesError.notImplementedYet(
                "no Spaces backend ends an idle Space; ProviderCapabilities.serverBackstop "
                + "is false. A SIGKILLed client never runs its defer, which is how a demo "
                + "Space reaches a hundred orphaned windows.")
        }
    }
}

// MARK: - Entry point

/// Finding a backend without being told where it is.
public enum Spaces {
    /// The cua SDK's Spaces runtime, found without being told where it is:
    /// a running `cua daemon` when there is one (shared registry, hotspots
    /// that outlive this process), else the runtime in this process. Zero
    /// arguments, and no Python.
    public static func local() throws -> SpacesConnection {
        let env = ProcessInfo.processInfo.environment
        if env["CUA_SPACES_EMBEDDED"] != "1",
           let daemon = try? Cua.connect(address: env["CUA_DAEMON"], token: env["CUA_DAEMON_TOKEN"]),
           FileManager.default.fileExists(atPath: daemonDiscoveryPath()) {
            return SpacesConnection(cua: daemon)
        }
        return try SpacesConnection.embedded()
    }

    private static func daemonDiscoveryPath() -> String {
        let env = ProcessInfo.processInfo.environment
        let home = env["CUA_HOME"].flatMap { $0.isEmpty ? nil : $0 }
            ?? NSString(string: "~/.cua").expandingTildeInPath
        return (home as NSString).appendingPathComponent("daemon.json")
    }

    /// The Space named by `CUA_SPACE`, attached, never created.
    public static func attachToEnvironmentSpace() async throws -> Space {
        guard let raw = ProcessInfo.processInfo.environment["CUA_SPACE"], !raw.isEmpty else {
            throw SpacesError.wouldCreate(
                "CUA_SPACE is not set, and attaching is the only thing this call will do")
        }
        return try await local().attach(to: SpaceID(raw))
    }
}
