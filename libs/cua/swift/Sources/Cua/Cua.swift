// The public `Cua` module: the generated binding (`CuaSDK`) plus a few
// conveniences. Everything else — Sandboxes, SpacesdClient, MediaSession with
// FrameSink/AudioSink, Fleet, Local, Spaces (Spaces/Space/SpaceStreamSession/
// SpacePresence/TeleportApprover) — comes from `CuaSDK` unchanged.
@_exported import CuaSDK

public extension Cua {
    /// An SDK runtime in this process (no I/O until the first call).
    static func embedded(
        stateDir: String? = nil,
        fleet: FleetSettings? = nil,
        fleetFromEnv: Bool = true,
        envProbeTimeoutMs: UInt32? = nil,
        spacesHome: String? = nil,
        teleportHome: String? = nil,
        fleetFromSession: Bool = false
    ) throws -> Cua {
        try Cua.embedded(config: CuaConfig(
            stateDir: stateDir,
            fleet: fleet,
            fleetFromEnv: fleetFromEnv,
            envProbeTimeoutMs: envProbeTimeoutMs,
            spacesHome: spacesHome,
            teleportHome: teleportHome,
            fleetFromSession: fleetFromSession
        ))
    }
}

public extension SpacesdCommand {
    /// A command with arguments and defaults for everything else.
    init(_ program: String, _ args: [String] = [], stdin: Bool = false) {
        self.init(
            program: program, args: args, env: [:], cwd: nil, user: nil,
            timeoutMs: nil, tag: nil, stdin: stdin, pty: nil
        )
    }
}

/// Canonical images: `Image.linux()` is `ghcr.io/trycua/linux:24.04`
/// (`CUA_IMAGE_LINUX` overrides it), `Image.windows()`
/// `ghcr.io/trycua/windows:2022`, `Image.macos()` `ghcr.io/trycua/macos:26`.
/// These are the full tier (dev tooling); `tier: "slim"` is the minimal image
/// CI runs and, on macOS, `tier: "xcode"` adds a pinned Xcode.
/// `Image.omarchy()` is `ghcr.io/trycua/omarchy:edge`, an amd64 VM. Images CI
/// has not published yet throw `CuaError.ImageNotPublished`; pass the
/// reference to `fromRegistry` to use one anyway.
/// `Image.resolve` is the one resolver (digest-pinned variant per backend).
public enum Image {
    public static func linux(_ version: String? = nil, tier: String? = nil) throws -> String {
        try canonicalImageTier(os: "linux", version: version, tier: tier)
    }

    public static func windows(_ version: String? = nil, tier: String? = nil) throws -> String {
        try canonicalImageTier(os: "windows", version: version, tier: tier)
    }

    public static func macos(_ version: String? = nil, tier: String? = nil) throws -> String {
        try canonicalImageTier(os: "macos", version: version, tier: tier)
    }

    /// Omarchy (Arch Linux, Hyprland) with cua-spacesd: an amd64 VM.
    public static func omarchy(_ channel: String? = nil) throws -> String {
        try omarchyImage(channel: channel)
    }

    public static func fromRegistry(_ reference: String) -> String {
        reference  // literal: `ubuntu:24.04` is Docker Hub's image
    }

    public static func resolve(
        _ reference: String, backend: String = "local", arch: String? = nil
    ) throws -> ResolvedImage {
        try resolveImage(reference: reference, backend: backend, arch: arch)
    }
}

/// `Pool.apply(fleet, name:, spec:, options:)`: the one pool writer, the
/// same call as `fleet.apply`. A `SandboxSpec` is what the sandbox runs and
/// `PoolOptions` how the pool keeps capacity for it; `fleet.checkPoolSpec`
/// raises `CuaError.PoolSpecMismatch` (with a diff) for a named pool whose
/// template differs, `fleet.applyPoolTemplate` updates it, and
/// `fleet.exportPool(name:).terraform` prints the `fleets_pool` block.
public enum Pool {
    @discardableResult
    public static func apply(
        _ fleet: FleetProtocol, name: String, spec: SandboxSpec, options: PoolOptions = PoolOptions()
    ) async throws -> FleetPool {
        try await fleet.apply(name: name, spec: spec, options: options)
    }
}

public extension CuaError {
    /// The link to this error's entry (cause and fix) on the errors
    /// reference.
    var docUrl: String {
        errorDocUrl(variant: Mirror(reflecting: self).children.first?.label ?? "")
    }
}

public extension CuaError {
    /// The qualified refs (`local:box`, `cloud:box`) an `.AmbiguousSandbox`
    /// lists: a bare sandbox name that matches sandboxes in more than one
    /// location. Empty for every other case.
    var ambiguousCandidates: [String] {
        if case .AmbiguousSandbox(let message) = self {
            return ambiguousSandboxCandidates(message: message)
        }
        return []
    }
}

