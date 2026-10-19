import CoreGraphics
import Foundation

/// Where a Space runs: the location word the backend reports as `provider`.
///
/// `FRICTION.md` §6: *"Local and cloud Spaces disagree on their vocabulary at
/// every level."* The SDK's answer is that an app branches on a **capability**
/// or on a normalised value, never on a string it had to learn empirically.
public enum SpaceProvider: String, Sendable, Hashable, Codable, CaseIterable {
    /// A sandbox on the operator's own machine (container or VM). Free.
    case local
    /// A Cua Cloud sandbox. Metered.
    case cloud
    /// Any machine running cua-spacesd, added by `host:port`.
    case direct
    /// A machine reached through a relay, added by id.
    case relay
    /// A client with no Space behind it.
    case demo
    case unknown

    public init(rawProvider: String) {
        self = SpaceProvider(rawValue: rawProvider.lowercased()) ?? .unknown
    }

    /// `$HOME` inside the Space. §6 records that this differs per provider and
    /// that the difference leaks into every default upload destination, so the
    /// SDK publishes it rather than making each caller name absolute paths.
    public var home: String {
        switch self {
        case .local: return "/Users/lume"
        case .cloud, .direct, .relay: return "/root"
        case .demo, .unknown: return "/tmp"
        }
    }

    /// The directory the SDK uploads into when the caller does not choose one.
    public var defaultUploadDirectory: String { "\(home)/Downloads" }

    public var capabilities: ProviderCapabilities {
        switch self {
        case .local:
            return ProviderCapabilities(
                agents: true, windowList: true, upload: true, download: true,
                rcdpStreaming: true, teleport: true, creation: true,
                notes: ["createSpace(options:) with on: .local makes one for free; "
                        + "attach(to:) reaches one that exists (FRICTION.md §5).",
                        "Teleport lands through the Space's own TeleportService.ImportSession."])
        case .cloud:
            return ProviderCapabilities(
                agents: true, windowList: true, upload: true, download: true,
                rcdpStreaming: true, teleport: true, creation: true,
                notes: ["createSpace(options:) with on: .cloud is metered.",
                        "stream_endpoint mints a media ticket for cloud Spaces too; attaching "
                        + "needs the gateway headers (Space.native().websocketHeaders()) or the "
                        + "daemon's media bridge."])
        case .direct, .relay:
            return ProviderCapabilities(
                agents: true, windowList: true, upload: true, download: true,
                rcdpStreaming: true, teleport: true, creation: false,
                notes: ["A Space added by address is any machine running cua-spacesd; "
                        + "delete only forgets it, because cua did not create it."])
        case .demo, .unknown:
            return ProviderCapabilities(
                agents: false, windowList: false, upload: false, download: false,
                rcdpStreaming: false, teleport: false, creation: false,
                notes: ["No Space is attached."])
        }
    }
}

/// What a provider can actually do, declared rather than discovered by calling
/// a tool and reading the prose it fails with (`FRICTION.md` §6).
public struct ProviderCapabilities: Sendable, Hashable {
    public var agents: Bool
    public var windowList: Bool
    public var upload: Bool
    public var download: Bool
    /// Whether `Space.streamEndpoint()` can hand back frames for an
    /// in-product surface (`FRICTION.md` §10). The name predates the media
    /// ticket; every provider with a cua-spacesd streams now.
    public var rcdpStreaming: Bool
    public var teleport: Bool
    /// Whether `createSpace(options:)` can make new Spaces here at all.
    public var creation: Bool
    public var notes: [String]

    /// Whether the **server** will end a run or a Space that the client stopped
    /// watching.
    ///
    /// `false` everywhere, and this is the most expensive `false` in the SDK.
    /// Every comparable sandbox product makes the server the backstop; we have
    /// none. A `SIGKILL`ed client never runs its `defer`, which is how one demo
    /// Space accumulated roughly 112 orphaned windows. `AgentStartRequest.timeout`
    /// and `Space.setIdleTimeout(_:)` are reserved against the day this flips,
    /// so gaining it is not a breaking change.
    public var serverBackstop: Bool

    public init(agents: Bool, windowList: Bool, upload: Bool, download: Bool,
                rcdpStreaming: Bool, teleport: Bool, creation: Bool, notes: [String],
                serverBackstop: Bool = false) {
        self.serverBackstop = serverBackstop
        self.agents = agents
        self.windowList = windowList
        self.upload = upload
        self.download = download
        self.rcdpStreaming = rcdpStreaming
        self.teleport = teleport
        self.creation = creation
        self.notes = notes
    }
}

/// What a Space can do, as a set rather than a wall of booleans.
///
/// An `OptionSet` and not a struct of `Bool`s on purpose: a struct's memberwise
/// initialiser is part of its API, so every capability added afterwards breaks
/// every caller that constructed one. Adding a case here breaks nobody.
public struct SpaceFeature: OptionSet, Sendable, Hashable {
    public let rawValue: Int
    public init(rawValue: Int) { self.rawValue = rawValue }

    /// Agent threads: start, message, status, stop.
    public static let agents = SpaceFeature(rawValue: 1 << 0)
    /// File teleport, both directions.
    public static let files = SpaceFeature(rawValue: 1 << 1)
    /// Frames and input for a surface inside *this* product.
    public static let streaming = SpaceFeature(rawValue: 1 << 2)
    /// Session teleport — arriving already signed in.
    public static let sessions = SpaceFeature(rawValue: 1 << 3)
    /// The window list.
    public static let windowList = SpaceFeature(rawValue: 1 << 4)
    /// Creating a new Space. The only family of calls that can cost money.
    public static let creation = SpaceFeature(rawValue: 1 << 5)
    /// The server ends work the client stopped watching. Nothing has this yet.
    public static let serverBackstop = SpaceFeature(rawValue: 1 << 6)
}

extension ProviderCapabilities {
    /// This provider's capabilities as a set.
    public var features: SpaceFeature {
        var f: SpaceFeature = []
        if agents { f.insert(.agents) }
        if upload || download { f.insert(.files) }
        if rcdpStreaming { f.insert(.streaming) }
        if teleport { f.insert(.sessions) }
        if windowList { f.insert(.windowList) }
        if creation { f.insert(.creation) }
        if serverBackstop { f.insert(.serverBackstop) }
        return f
    }
}

/// One normalised readiness vocabulary.
///
/// A ready Space reports `phase: "ready"` from the Rust backend, and older
/// backends said `running` (local) or `Bound` (cloud). Both become `.ready` here; the string that produced it is kept on
/// `rawPhase` so nothing is hidden, only normalised.
public enum SpaceState: Sendable, Hashable {
    case ready
    case starting
    case stopped
    case failed
    case unknown

    public init(phase: String) {
        switch phase.lowercased() {
        case "running", "bound", "ready": self = .ready
        case "pending", "starting", "provisioning", "creating": self = .starting
        case "stopped", "released", "terminated": self = .stopped
        case "failed", "error": self = .failed
        default: self = .unknown
        }
    }

    public var isReady: Bool { self == .ready }
}

/// A Space as the account sees it, before a handle is taken on it.
public struct SpaceInfo: Sendable, Hashable, Identifiable {
    public let id: SpaceID
    public let provider: SpaceProvider
    public let operatingSystem: String
    public let state: SpaceState
    /// The phase string the server actually sent. Normalisation never discards
    /// the original.
    public let rawPhase: String
    public let ipAddress: String?

    public init(id: SpaceID, provider: SpaceProvider, operatingSystem: String,
                state: SpaceState, rawPhase: String, ipAddress: String?) {
        self.id = id
        self.provider = provider
        self.operatingSystem = operatingSystem
        self.state = state
        self.rawPhase = rawPhase
        self.ipAddress = ipAddress
    }

    public var isReady: Bool { state.isReady }
    public var home: String { provider.home }
    public var capabilities: ProviderCapabilities { provider.capabilities }

    init(row: [String: JSONValue]) {
        let id = SpaceID(row["id"]?.stringValue ?? "")
        let phase = row["phase"]?.stringValue ?? ""
        let declared = row["provider"]?.stringValue ?? id.providerPrefix ?? ""
        self.init(id: id,
                  provider: SpaceProvider(rawProvider: declared),
                  operatingSystem: row["os"]?.stringValue ?? "",
                  state: SpaceState(phase: phase),
                  rawPhase: phase,
                  ipAddress: row["ip"]?.stringValue)
    }
}

/// A window inside a Space, and the rcdp target that streams it.
public struct SpaceWindow: Sendable, Hashable, Identifiable {
    public let id: WindowID
    public let app: String
    public let title: String
    public let pixelSize: CGSize
    public let scaleFactor: CGFloat
    public let visible: Bool
    /// The owning process, when the server reports it. This is the join key
    /// `FRICTION.md` §37 asks for: a run's window is the window whose pid the
    /// run owns.
    public let processID: Int?
    /// The app's id as the guest reports it (a bundle id, a WM class), or
    /// empty. With `app` and `processID` it finds the app's icon
    /// (`Space.appIcon`).
    public let appID: String

    public init(id: WindowID, app: String, title: String,
                pixelSize: CGSize = .zero, scaleFactor: CGFloat = 1,
                visible: Bool = true, processID: Int? = nil, appID: String = "") {
        self.id = id
        self.app = app
        self.title = title
        self.pixelSize = pixelSize
        self.scaleFactor = scaleFactor
        self.visible = visible
        self.processID = processID
        self.appID = appID
    }

    public var displayName: String { title.isEmpty ? app : "\(app) — \(title)" }

    init(row: [String: JSONValue]) {
        // §7: the list tool names this `window`, the stream tool takes it as
        // `window_id`. Reading only one spelling yields an empty id for every
        // window — silently, because the JSON parse succeeded. The SDK reads
        // all of them and publishes one.
        let raw = row["window"]?.stringValue
            ?? row["window_id"]?.stringValue
            ?? row["id"]?.stringValue ?? ""
        let g = row["geometry"]?.objectValue ?? [:]
        // The Rust window list reports `bounds: [x, y, w, h]` in points and
        // `on_screen`; the older list `geometry` in pixels and `visible`.
        let bounds = row["bounds"]?.arrayValue ?? []
        let size = g.isEmpty && bounds.count == 4
            ? CGSize(width: bounds[2].doubleValue ?? 0, height: bounds[3].doubleValue ?? 0)
            : CGSize(width: g["width_px"]?.doubleValue ?? 0, height: g["height_px"]?.doubleValue ?? 0)
        self.init(id: WindowID(raw),
                  app: row["app_name"]?.stringValue ?? row["app"]?.stringValue ?? "",
                  title: row["title"]?.stringValue ?? "",
                  pixelSize: size,
                  scaleFactor: CGFloat(g["scale_factor"]?.doubleValue ?? 1),
                  visible: row["visible"]?.boolValue ?? row["on_screen"]?.boolValue ?? true,
                  processID: row["pid"]?.intValue ?? row["owner_pid"]?.intValue,
                  appID: row["app_id"]?.stringValue ?? "")
    }
}

/// Where an in-product stream connects: a minted media session.
/// Distinct from the operator-facing display tools — see
/// `Space.streamEndpoint()` and `FRICTION.md` §10.
public struct StreamEndpoint: Sendable, Hashable {
    public let host: String
    public let port: Int
    /// Kept for source compatibility; the spacesd serves everything on one
    /// port now.
    public let driverPort: Int
    /// The media ticket (also in `webSocketURL`). Short-lived and single-use
    /// per attach, so a cached endpoint is refreshed with `forceRefresh`.
    public let token: String
    /// The media WebSocket URL, ticket included (rcdp wire v2).
    public let webSocketURL: String
    /// The media session, for closing it.
    public let mediaSessionID: String
    /// `h264`, `bgra` or `png`.
    public let codec: String
    /// Initial frame size in pixels.
    public let frameSize: CGSize
    /// Attaching needs extra headers (cloud gateway, daemon passthrough).
    public let needsHeaders: Bool

    public init(host: String, port: Int = 3211, driverPort: Int = 3211, token: String,
                webSocketURL: String = "", mediaSessionID: String = "", codec: String = "",
                frameSize: CGSize = .zero, needsHeaders: Bool = false) {
        self.host = host
        self.port = port
        self.driverPort = driverPort
        self.token = token
        self.webSocketURL = webSocketURL
        self.mediaSessionID = mediaSessionID
        self.codec = codec
        self.frameSize = frameSize
        self.needsHeaders = needsHeaders
    }
}

// MARK: - Creating a Space: where, what kind, which engine

/// Where a new Space runs. There is no default in this SDK on purpose: where
/// it runs decides whether it is metered, so every call site says it
/// (`FRICTION.md` §5, §28, §33). An existing machine is not created; it is
/// added with `SpacesConnection.add(url:token:name:)`.
public enum SpaceLocation: String, Sendable, Hashable, Codable, CaseIterable {
    /// This machine. Free.
    case local
    /// Cua Cloud. Metered.
    case cloud

    /// Whether a Space created here is billed.
    public var isMetered: Bool { self == .cloud }
}

/// What kind of machine. `auto` lets the image decide: macOS and Windows
/// images are VMs, an image with a container rootfs is a container, a
/// disk-only image is a VM.
public enum SpaceKind: String, Sendable, Hashable, Codable, CaseIterable {
    case auto
    case container
    case vm
}

/// Which engine runs it. `auto` picks the safest one the location offers for
/// the kind. Local: `gvisor`, `runc` (containers), `qemu`, `lume` (VMs).
/// Cloud: `gvisor` (containers), `kubevirt` (VMs). An impossible combination
/// fails on the server with `invalid_placement`, listing the valid values.
public enum SpaceRuntime: String, Sendable, Hashable, Codable, CaseIterable {
    case auto
    case gvisor
    case runc
    case qemu
    case lume
    case kubevirt

    /// The kind this engine implies (`nil` for `auto`).
    public var kind: SpaceKind? {
        switch self {
        case .auto: return nil
        case .gvisor, .runc: return .container
        case .qemu, .lume, .kubevirt: return .vm
        }
    }

    /// The engines `location` offers for `kind`, `auto` first.
    public static func offered(on location: SpaceLocation, kind: SpaceKind) -> [SpaceRuntime] {
        switch (location, kind) {
        case (.local, .container): return [.auto, .gvisor, .runc]
        case (.local, .vm): return [.auto, .qemu, .lume]
        case (.local, .auto): return [.auto, .gvisor, .runc, .qemu, .lume]
        case (.cloud, .container): return [.auto, .gvisor]
        case (.cloud, .vm): return [.auto, .kubevirt]
        case (.cloud, .auto): return [.auto, .gvisor, .kubevirt]
        }
    }
}

/// Everything `SpacesConnection.createSpace(options:)` takes (the
/// `create_space` tool's arguments).
public struct SpaceCreateOptions: Sendable, Hashable {
    /// Where it runs, and so whether it is metered. Required.
    public var on: SpaceLocation
    public var kind: SpaceKind
    public var runtime: SpaceRuntime
    /// Guest image. `nil`: the canonical Linux image.
    public var image: String?
    /// `nil`: a generated `space-<hex>`.
    public var name: String?
    /// Return a reachable registered Space in the same location instead of
    /// creating one (get-or-create). Off by default: reuse is asked for too.
    public var reuse: Bool
    /// Wait until the Space is ready. With `false` the result is `.starting`.
    public var wait: Bool

    public init(on: SpaceLocation, kind: SpaceKind = .auto, runtime: SpaceRuntime = .auto,
                image: String? = nil, name: String? = nil, reuse: Bool = false,
                wait: Bool = true) {
        self.on = on
        self.kind = kind
        self.runtime = runtime
        self.image = image
        self.name = name
        self.reuse = reuse
        self.wait = wait
    }

    /// The `create_space` arguments.
    var arguments: [String: JSONValue] {
        var a: [String: JSONValue] = [
            "on": .string(on.rawValue),
            "kind": .string(kind.rawValue),
            "runtime": .string(runtime.rawValue),
            "reuse": .bool(reuse),
            "wait": .bool(wait),
        ]
        if let image { a["image"] = .string(image) }
        if let name { a["name"] = .string(name) }
        return a
    }
}
