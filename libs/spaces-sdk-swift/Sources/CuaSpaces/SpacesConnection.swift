import Cua
import Foundation

/// A connection to a Spaces backend, and the only thing in the SDK that can
/// create or destroy a Space.
///
/// The two calls that matter are deliberately different calls:
///
/// * `attach(to:)` takes a `SpaceID` and reaches **that** Space or throws.
/// * `createSpace(options:)` is the only call that can cost money, and where
///   it runs (`on: .local` or `on: .cloud`) is a required argument.
///
/// `FRICTION.md` §5, §28 and §33 are all the same bug wearing different hats:
/// the recommended entry point could silently create a cloud sandbox, it could
/// not see a local Space at all, and a harness that took a `<space>` argument
/// and threaded it nowhere attached to a cloud sandbox instead of the macOS Space
/// it had been handed. §33's fix in the app was an environment variable read
/// deep inside the resolver; here, a caller with a Space in hand has an
/// argument to pass, and a caller without one gets an error rather than an
/// invoice.
public actor SpacesConnection {

    private let transport: SpacesTransport
    /// The generated Spaces object, when this connection runs on the cua SDK
    /// (always, outside tests). Typed primitives — shell, files, streams,
    /// presence — use it directly; everything else goes through `transport`.
    public nonisolated let native: CuaSDK.Spaces?
    private var handles: [SpaceID: CuaSDK.Space] = [:]
    /// Last-known snapshot per run, which is what lets the cheap roster call
    /// report a `reason` it was not given (`FRICTION.md` §22).
    private var lastSnapshots: [RunID: RunSnapshot] = [:]
    private var endpointCache: [SpaceID: StreamEndpoint] = [:]

    /// The Spaces contract's tool names (`libs/cua/spaces-contract`), from the
    /// SDK linked into this process. 79 tools.
    public static var contractTools: [String] {
        spacesToolMethods().map(\.tool)
    }

    /// A connection over any transport (tests pass an in-process fake).
    public init(transport: SpacesTransport) {
        self.transport = transport
        self.native = (transport as? CuaSpacesTransport)?.spaces
    }

    /// A connection over the cua SDK: `Cua.embedded(...)` runs the Spaces
    /// runtime in this process, `Cua.connect(...)` uses a running
    /// `cua daemon`, whose registry, hotspots and host reads are shared by
    /// every process on the machine.
    public init(cua: Cua) {
        self.init(transport: CuaSpacesTransport(cua: cua))
    }

    /// The Spaces runtime in this process (registry `~/.cua` unless
    /// `spacesHome` says otherwise).
    public static func embedded(spacesHome: String? = nil,
                                teleportHome: String? = nil) throws -> SpacesConnection {
        SpacesConnection(cua: try Cua.embedded(
            spacesHome: spacesHome, teleportHome: teleportHome))
    }

    /// A running `cua daemon` (`nil` address: `~/.cua/daemon.json`, then
    /// `~/.cua/cua.sock`).
    public static func daemon(address: String? = nil, token: String? = nil) throws -> SpacesConnection {
        SpacesConnection(cua: try Cua.connect(address: address, token: token))
    }

    /// The tool names the backend offers. Useful as a liveness probe and to
    /// check a tool exists before depending on it.
    public func availableTools() async throws -> [String] {
        try await transport.availableTools()
    }

    /// Register a machine that already runs cua-spacesd (`host:port` or
    /// `http(s)://…`), after a capabilities handshake. Never creates anything.
    public func add(url: String, token: String? = nil, name: String? = nil) async throws -> Space {
        var args: [String: JSONValue] = ["url": .string(url)]
        if let token { args["token"] = .string(token) }
        if let name { args["name"] = .string(name) }
        let d = try await transport.object("add_space", args)
        guard let raw = d["id"]?.stringValue, !raw.isEmpty else {
            throw SpacesError.malformedResponse(tool: "add_space", detail: "no id in \(d)")
        }
        return try await attach(to: SpaceID(raw))
    }

    /// Forget a Space (registry entry and stored token). The machine is not
    /// touched.
    public func remove(_ id: SpaceID) async throws {
        _ = try await transport.callTool("remove_space", ["space": .string(id.rawValue)])
        handles[id] = nil
    }

    /// The generated `CuaSDK.Space` handle for `id`, cached per connection.
    /// `nil` when this connection is not backed by the cua SDK (a test fake).
    public func nativeSpace(_ id: SpaceID) async throws -> CuaSDK.Space? {
        guard let native else { return nil }
        if let cached = handles[id] { return cached }
        let handle: CuaSDK.Space
        do {
            handle = try await native.space(space: id.rawValue)
        } catch let error as CuaError {
            throw SpacesError(cua: error, tool: "space")
        }
        handles[id] = handle
        return handle
    }

    // MARK: - Spaces

    /// Every Space the account can see, from every provider.
    ///
    /// Local, cloud, direct and relay Spaces alike: §5 records that the old
    /// recommended entry point could not see the very Space being demoed from.
    public func spaces() async throws -> [SpaceInfo] {
        try await transport.array("list_spaces", [:], unwrapping: "spaces")
            .compactMap(\.objectValue)
            .map(SpaceInfo.init(row:))
            .filter { !$0.id.isEmpty }
    }

    /// Attach to a Space that already exists. Never creates one.
    ///
    /// - Throws: `SpacesError.spaceUnavailable` when no Space with that id is
    ///   visible, or when it exists but is not ready — a caller learns which,
    ///   rather than getting a handle that fails on first use.
    public func attach(to id: SpaceID, requireReady: Bool = true) async throws -> Space {
        let all = try await spaces()
        guard let info = all.first(where: { $0.id == id }) else {
            throw SpacesError.spaceUnavailable(
                id, "not among the \(all.count) Spaces this account can see")
        }
        if requireReady, !info.isReady {
            throw SpacesError.spaceUnavailable(id, "phase is \(info.rawPhase), not ready")
        }
        return try await space(for: info)
    }

    /// A handle, with the guest's real `$HOME` when the SDK can ask for it.
    private func space(for info: SpaceInfo) async throws -> Space {
        guard info.isReady, let handle = try? await nativeSpace(info.id) else {
            return Space(info: info, connection: self)
        }
        let home = try? await handle.home()
        return Space(info: info, connection: self, guestHome: home)
    }

    /// Attach to the first ready Space matching a predicate, without
    /// creating one. Returns `nil` rather than creating one.
    public func attachToFirstReady(
        where matches: @Sendable (SpaceInfo) -> Bool = { _ in true }
    ) async throws -> Space? {
        guard let info = try await spaces().first(where: { $0.isReady && matches($0) })
        else { return nil }
        return try await space(for: info)
    }

    /// Create a new Space where `options.on` says. **A cloud Space is metered**;
    /// a local one is free. `on` has no default, so no call site creates a
    /// billable Space by accident (§28: opening a window used to silently
    /// create billable infrastructure).
    ///
    /// With `options.wait == false` the returned handle is `.starting`; attach
    /// to its id later. `options.reuse` returns a reachable registered Space
    /// in the same location instead of creating one.
    public func createSpace(options: SpaceCreateOptions) async throws -> Space {
        let d = try await transport.object("create_space", options.arguments)
        guard let raw = d["id"]?.stringValue, !raw.isEmpty else {
            throw SpacesError.malformedResponse(tool: "create_space", detail: "no id in \(d)")
        }
        var row = d
        if row["provider"] == nil { row["provider"] = .string(options.on.rawValue) }
        if row["phase"] == nil { row["phase"] = .string(options.wait ? "ready" : "starting") }
        return try await space(for: SpaceInfo(row: row))
    }

    /// `createSpace(options:)` spelled inline: `createSpace(on: .local)`.
    public func createSpace(on: SpaceLocation, kind: SpaceKind = .auto,
                            runtime: SpaceRuntime = .auto, image: String? = nil,
                            name: String? = nil, reuse: Bool = false,
                            wait: Bool = true) async throws -> Space {
        try await createSpace(options: SpaceCreateOptions(
            on: on, kind: kind, runtime: runtime, image: image, name: name,
            reuse: reuse, wait: wait))
    }

    /// Delete a Space's sandbox and forget it. Irreversible for a Space that
    /// `createSpace` made (a cloud Space stops metering); a Space added by
    /// address is only forgotten, because cua did not create it. Returns the
    /// server's account of what happened. To forget without deleting, use
    /// `remove(_:)`.
    @discardableResult
    public func deleteSpace(_ id: SpaceID) async throws -> String {
        let result = try await transport.callTool("delete_space", ["space": .string(id.rawValue)])
        handles[id] = nil
        endpointCache[id] = nil
        return result.stringValue ?? ""
    }

    /// Turn a Space off the way its provider can: a local container or QEMU
    /// VM is suspended (its memory is kept), a Lume VM or a Space one of
    /// your machines provides or one in your own cloud (AWS, Google Cloud)
    /// is stopped (its disk is kept). Fleet cloud Spaces, Modal sandboxes
    /// and Spaces added by address cannot. Returns `space`, `state`,
    /// `power` and `message`.
    @discardableResult
    public func stopSpace(_ id: SpaceID) async throws -> JSONValue {
        let result = try await transport.callTool("stop_space", ["space": .string(id.rawValue)])
        handles[id] = nil
        endpointCache[id] = nil
        return result
    }

    /// Turn a Space back on: resume a suspended one or boot a stopped one.
    @discardableResult
    public func startSpace(_ id: SpaceID) async throws -> JSONValue {
        let result = try await transport.callTool("start_space", ["space": .string(id.rawValue)])
        handles[id] = nil
        endpointCache[id] = nil
        return result
    }

    // MARK: - Escape hatch

    /// Call any tool the SDK does not model. Present on purpose: an SDK that
    /// cannot be gone around gets forked instead.
    @discardableResult
    public func callTool(_ name: String, _ arguments: [String: JSONValue] = [:]) async throws -> JSONValue {
        try await transport.callTool(name, arguments)
    }

    // MARK: - Internals used by Space and AgentRun

    func object(_ name: String, _ args: [String: JSONValue]) async throws -> [String: JSONValue] {
        try await transport.object(name, args)
    }

    func array(_ name: String, _ args: [String: JSONValue],
               unwrapping key: String? = nil) async throws -> [JSONValue] {
        try await transport.array(name, args, unwrapping: key)
    }

    func remember(_ snapshot: RunSnapshot) {
        lastSnapshots[snapshot.id] = snapshot
    }

    func lastSnapshot(_ id: RunID) -> RunSnapshot? { lastSnapshots[id] }

    func cachedEndpoint(_ id: SpaceID) -> StreamEndpoint? { endpointCache[id] }

    func cacheEndpoint(_ endpoint: StreamEndpoint, for id: SpaceID) {
        endpointCache[id] = endpoint
    }

    // MARK: - Transcripts and turn boundaries

    /// The process-local outbox. There is no backend queue; this is it.
    public let outbox = Outbox()

    private var transcripts: [RunID: TranscriptWindow] = [:]
    private var turnRecords: [RunID: [Turn]] = [:]

    /// Runs this process started, so a roster can be "conversations I have
    /// had" rather than "everything happening in the Space".
    private var startedHere: Set<RunID> = []

    func noteStarted(_ id: RunID) { startedHere.insert(id) }

    func runsStartedHere() -> Set<RunID> { startedHere }

    func transcript(of id: RunID) -> TranscriptWindow { transcripts[id] ?? TranscriptWindow() }

    func recordTranscript(_ window: TranscriptWindow, for id: RunID) {
        transcripts[id] = window
    }

    /// The SDK's own record of where a turn began, taken at the moment the
    /// message was delivered — so a boundary does not depend on the app having
    /// been watching.
    @discardableResult
    func openTurn(on id: RunID, message: String?, delivery: Delivery?) -> Turn.ID {
        let window = transcripts[id] ?? TranscriptWindow()
        let began = OutputCursor(line: window.lines.count)
        var turns = turnRecords[id] ?? []
        if let last = turns.indices.last, turns[last].ended == nil {
            let closing = turns[last]
            turns[last] = Turn(id: closing.id, run: id, message: closing.message,
                               startedAt: closing.startedAt, began: closing.began,
                               ended: began, delivery: closing.delivery,
                               outputLost: window.lostBefore)
        }
        let turn = Turn(id: Turn.ID("\(id.rawValue)#\(turns.count)"), run: id,
                        message: message, startedAt: Date(), began: began, ended: nil,
                        delivery: delivery, outputLost: window.lostBefore)
        turns.append(turn)
        turnRecords[id] = turns
        return turn.id
    }

    func currentTurn(of id: RunID) -> Turn.ID? { turnRecords[id]?.last?.id }

    func turns(of id: RunID) -> [Turn] { turnRecords[id] ?? [] }
}
