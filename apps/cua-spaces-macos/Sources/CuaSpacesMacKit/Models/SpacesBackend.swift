// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSDK
import CuaSpacesFFI
import CuaSpaces
import CuaSpacesStreaming
import Foundation

/// The platform side of Spaces: the cua SDK calls the app makes. Every
/// decision about what they mean (names, status, grouping, what is allowed)
/// is the app core's; this only moves data.
public protocol SpacesBackend: AnyObject, Sendable {
    /// The registry, one row per Space, with a bounded reachability probe.
    func rows() async throws -> [AppSpaceRow]
    /// Creates a Space (`create_space`), reporting the SDK's create progress
    /// (pulling, booting, waiting for cua-spacesd, connecting) to `progress`
    /// on an SDK thread until it is ready; returns its id. `createId` (the
    /// pending row's id) is what `cancelCreate` finds it by. A cancelled
    /// create throws `CuaError.Cancelled`.
    func create(_ args: AppCreateSpaceArgs, createId: String,
                progress: @escaping @Sendable (SpaceCreateProgress) -> Void) async throws -> String
    /// Cancels a create still running (`cancel_create`, by the `createId`
    /// it was started with): returns once what it made is removed.
    func cancelCreate(createId: String) async throws
    /// The GPU option of each local runtime that has one (the SDK's
    /// `gpu_support`), for the New Space wizard; `nil` when unknown.
    func gpuChoices() async -> [AppGpuChoice]?
    /// Your machines that provide Spaces (the SDK's `Spaces.hosts()`), for
    /// the New Space wizard's Run on menu; none when they cannot be read.
    func hosts() async -> [AppSpaceHost]
    /// Adds a machine that already runs cua-spacesd.
    func add(url: String, token: String?, name: String?) async throws
    /// Deletes a created Space, or forgets one added by address.
    func remove(id: String, removeOnly: Bool) async throws
    /// Turns a Space off (`on` false: suspended or stopped, as its provider
    /// can) or back on (`stop_space` / `start_space`).
    func setPower(id: String, on: Bool) async throws
    /// A stream source for the Space (desktop and per-window streams).
    func streamProvider(id: String) async throws -> SpaceStreamSourceProviding
    /// Local runtimes that are ready (`local_status().backends`), when known.
    func localBackends() async -> [String]?
    /// The local runtime doctor, when known: the ready backends and, for
    /// each one that is not, why (the check's detail).
    func localRuntimes() async -> (ready: [String], details: [String: String])?
    /// Free space where local Spaces are written and which images are
    /// pulled (`local().storage()`), when known.
    func localStorage() async -> LocalStorage?
    /// Cua Cloud can be used (signed in or client credentials).
    func cloudAvailable() async -> Bool
    /// This account's Cua Cloud rates (`Fleet.usagePricing()`), when known:
    /// the New Space estimate. `nil` shows none.
    func cloudPricing() async -> AppCloudPricing?
    /// The SDK's teleport handle and the Space handle, for "Teleport an app".
    func teleportContext(id: String) async throws -> (CuaSpacesFFI.Teleport, CuaSDK.Space)?
    /// The SDK's teleport handle, for drops (no Space needed).
    func teleportHandle() -> CuaSpacesFFI.Teleport?
    /// Sends files into the Space's `~/Downloads`; what landed (verified).
    func sendFiles(id: String, paths: [String]) async throws -> [AppSentFileInfo]
    /// The Space's coding-agent runs as rows (the app core's mapping of the
    /// SDK's run records, attention first).
    func agentRuns(id: String) async throws -> [AppSpaceAgentRun]
    /// The Space's latest thumbnail from the SDK's shared cache (the cua
    /// daemon's): no older than `maxAgeMs` (nil: any age), captured fresh
    /// when the cache is older; `nil` when there is none and the Space
    /// cannot capture.
    func thumbnail(id: String, maxAgeMs: UInt64?) async -> SpaceThumbnailData?
    /// Icons the Space's desktop shows for many apps, in order (the SDK's
    /// `Space.appIcons`, through its one icon cache): 64 px PNG or SVG
    /// bytes, `nil` where the Space has none.
    func appIcons(id: String, requests: [SpaceAppIconRequest]) async -> [Data?]
    /// The Space's primary display size (its display list), or `nil`.
    func primaryDisplay(id: String) async -> AppStreamDisplay?
    /// Memory and storage use now (the SDK's `Space.usage`), or `nil`.
    func usage(id: String) async -> AppSpaceUsage?
    /// Who the Space is shared with (`space_shares`).
    func shares(id: String) async throws -> [AppShareEntryInput]
    /// Shares the Space with `who` as `role` (`share_space`; presence first).
    func share(id: String, who: String, role: String) async throws -> [AppShareEntryInput]
    /// Stops sharing with `who` (`unshare_space`).
    func unshare(id: String, who: String) async throws -> [AppShareEntryInput]
}

public extension SpacesBackend {
    func thumbnail(id: String, maxAgeMs: UInt64?) async -> SpaceThumbnailData? { nil }
    func appIcons(id: String, requests: [SpaceAppIconRequest]) async -> [Data?] { requests.map { _ in nil } }
    func primaryDisplay(id: String) async -> AppStreamDisplay? { nil }
    func usage(id: String) async -> AppSpaceUsage? { nil }
    func cloudPricing() async -> AppCloudPricing? { nil }
    func gpuChoices() async -> [AppGpuChoice]? { nil }
    func hosts() async -> [AppSpaceHost] { [] }
    func localRuntimes() async -> (ready: [String], details: [String: String])? {
        await localBackends().map { (ready: $0, details: [:]) }
    }
    func shares(id: String) async throws -> [AppShareEntryInput] { [] }
    func share(id: String, who: String, role: String) async throws -> [AppShareEntryInput] { [] }
    func unshare(id: String, who: String) async throws -> [AppShareEntryInput] { [] }
}

/// The wizard's GPU choices from the SDK's `gpu_support`: the first option
/// of each runtime that has one (a runtime with none gets no GPU row).
func appGpuChoices(_ support: [GpuSupport]) -> [AppGpuChoice] {
    support.compactMap { s in
        guard let o = s.options.first else { return nil }
        let reason = o.reason.trimmingCharacters(in: .whitespacesAndNewlines)
        return AppGpuChoice(runtime: s.runtime, id: o.id, label: o.label, experimental: o.experimental,
                            supported: o.supported, reason: reason.isEmpty ? nil : reason,
                            learnMore: o.learnMore)
    }
}

/// A create ended because it was cancelled (here, or elsewhere:
/// `cua spaces cancel`), not because it failed.
func isCancelled(_ error: Error) -> Bool {
    if case CuaError.Cancelled = error { return true }
    return false
}

/// The SDK's share rows as the app core reads them.
func appShareRows(_ s: SpaceShares) -> [AppShareEntryInput] {
    s.shares.map { AppShareEntryInput(who: $0.who, role: $0.role, connected: $0.connected) }
}

/// The live backend: the cua SDK in this process, connected to a running
/// `cua daemon` when there is one (so the CLI, MCP clients and this app share
/// Spaces), else embedded.
public final class LiveSpacesBackend: SpacesBackend, @unchecked Sendable {
    public let cua: Cua
    private let connection: SpacesConnection

    public init(cua: Cua) {
        self.cua = cua
        self.connection = SpacesConnection(cua: cua)
    }

    /// `cua daemon` when it accepts connections, else the runtime in this
    /// process. `Cua.auto` probes the daemon's socket, so files left by a
    /// daemon that exited (`daemon.json`, `cua.sock`) fall back to embedded
    /// instead of failing every call. Cua Cloud uses the signed-in session
    /// when no client credentials are set (the Tauri app does the same).
    public static func make() throws -> LiveSpacesBackend {
        LiveSpacesBackend(cua: try Cua.auto(config: CuaConfig(fleetFromSession: true)))
    }

    public func rows() async throws -> [AppSpaceRow] {
        let spaces = cua.spaces()
        let infos = try await spaces.list()
        return await withTaskGroup(of: (Int, AppSpaceRow).self) { group in
            for (index, info) in infos.enumerated() {
                group.addTask {
                    var row = AppSpaceRow(
                        id: info.id, name: info.name, provider: info.provider,
                        spacesdVersion: info.spacesdVersion, features: info.features,
                        addedAt: info.addedAt, os: AppSpaceOs(word: info.os),
                        osName: info.osName.isEmpty ? nil : info.osName,
                        osPrettyName: info.osPrettyName.isEmpty ? nil : info.osPrettyName,
                        image: info.image.isEmpty ? nil : info.image,
                        imageDigest: info.imageDigest.isEmpty ? nil : info.imageDigest,
                        kind: AppSpaceKind(word: info.kind), arch: info.arch.isEmpty ? nil : info.arch,
                        reachable: false, error: nil, host: info.host.isEmpty ? nil : info.host, hostName: info.hostName.isEmpty ? nil : info.hostName,
                        power: info.power.isEmpty ? nil : info.power,
                        powerState: info.powerState.isEmpty ? nil : info.powerState,
                        cloud: info.cloud.isEmpty ? nil : info.cloud,
                        cloudPlace: info.cloudPlace.isEmpty ? nil : info.cloudPlace,
                        cloudDelete: info.cloudDelete.isEmpty ? nil : info.cloudDelete)
                    // Turned off on purpose: no probe (it would only time out).
                    if ["suspended", "stopped"].contains(info.powerState) {
                        return (index, row)
                    }
                    // Bounded connect: an unreachable Space must not stall the list.
                    let probe = await withTimeout(seconds: 4) {
                        try await spaces.space(space: info.id)
                    }
                    switch probe {
                    case .success(let space):
                        row.reachable = true
                        row.os = AppSpaceOs(word: space.info().os) ?? row.os
                        let info = space.info()
                        if !info.osName.isEmpty { row.osName = info.osName }
                        if !info.osPrettyName.isEmpty { row.osPrettyName = info.osPrettyName }
                        if !info.image.isEmpty { row.image = info.image }
                        if !info.imageDigest.isEmpty { row.imageDigest = info.imageDigest }
                        if let kind = AppSpaceKind(word: info.kind) { row.kind = kind }
                        if !info.arch.isEmpty { row.arch = info.arch }
                    case .failure(let error):
                        row.error = Self.words(error)
                    }
                    return (index, row)
                }
            }
            var out = [(Int, AppSpaceRow)]()
            for await r in group { out.append(r) }
            return out.sorted { $0.0 < $1.0 }.map(\.1)
        }
    }

    public func create(_ args: AppCreateSpaceArgs, createId: String,
                       progress: @escaping @Sendable (SpaceCreateProgress) -> Void) async throws -> String {
        let options = SpaceCreateOptions(
            image: args.image, on: args.on, kind: args.kind.word, runtime: args.runtime.word,
            name: args.name, cpus: args.cpus, memoryMb: args.memoryMb.map(UInt64.init),
            diskGb: args.diskGb, timeoutMs: nil, wait: nil, reuse: false, command: nil, env: [:], services: [:],
            spacesd: args.spacesd, gpu: args.gpu, createId: createId)
        let result = try await cua.spaces().createWithProgress(
            options: options, listener: CreateListener(progress))
        return result.space?.id ?? result.pendingId ?? ""
    }

    public func cancelCreate(createId: String) async throws {
        _ = try await cua.spaces().cancelCreate(space: createId)
    }

    /// Bounded, as the other probes the New Space sheet waits for.
    public func gpuChoices() async -> [AppGpuChoice]? {
        let result = await withTimeout(seconds: Self.probeSeconds) { [cua] in
            try await cua.spaces().gpuSupport(on: "local")
        }
        return (try? result.get()).map(appGpuChoices)
    }

    /// Bounded: each machine is asked for its limits (the SDK waits a few
    /// seconds for each, all at once).
    public func hosts() async -> [AppSpaceHost] {
        let result = await withTimeout(seconds: Self.probeSeconds) { [cua] in
            try await cua.spaces().hosts()
        }
        return ((try? result.get()) ?? []).map { appSpaceHost(host: $0) }
    }

    public func add(url: String, token: String?, name: String?) async throws {
        _ = try await cua.spaces().add(url: url, token: token, name: name)
    }

    public func remove(id: String, removeOnly: Bool) async throws {
        if removeOnly {
            try await cua.spaces().remove(space: id)
        } else {
            _ = try await cua.spaces().delete(space: id)
        }
    }

    /// Through the Spaces connection, which forgets its handle to the Space
    /// (a suspended Space answers nothing; a resumed one is attached again).
    public func setPower(id: String, on: Bool) async throws {
        if on {
            try await connection.startSpace(SpaceID(id))
        } else {
            try await connection.stopSpace(SpaceID(id))
        }
    }

    public func streamProvider(id: String) async throws -> SpaceStreamSourceProviding {
        let space = try await connection.attach(to: SpaceID(id), requireReady: false)
        return SpaceStreamProvider(space: space)
    }

    /// Bounded: the New Space sheet waits for these, and a hung engine
    /// probe (`docker info`, `lume --version`) must not keep it shut.
    public func localBackends() async -> [String]? {
        await localRuntimes()?.ready
    }

    public func localRuntimes() async -> (ready: [String], details: [String: String])? {
        let result = await withTimeout(seconds: Self.probeSeconds) { [cua] in
            try await cua.local().doctor()
        }
        guard case .success(let report) = result else { return nil }
        let ready = report.checks.filter { $0.status == .ok }.map { $0.name.lowercased() }
        var details: [String: String] = [:]
        for check in report.checks where check.status != .ok { details[check.name.lowercased()] = check.detail }
        return (ready: ready, details: details)
    }

    public func localStorage() async -> LocalStorage? {
        let result = await withTimeout(seconds: Self.probeSeconds) { [cua] in
            try await cua.local().storage()
        }
        return try? result.get()
    }

    /// How long a probe the New Space sheet waits for may take.
    static let probeSeconds: Double = 10

    public func cloudAvailable() async -> Bool {
        let env = ProcessInfo.processInfo.environment
        if env["FLEETS_TOKEN"] != nil || env["CUA_CLIENT_ID"] != nil { return true }
        return (try? cua.auth().status().loggedIn) ?? false
    }

    public func cloudPricing() async -> AppCloudPricing? {
        guard await cloudAvailable(), let fleet = try? cua.fleet(),
              case .success(let got) = await withTimeout(seconds: Self.probeSeconds, { try await fleet.usagePricing() }),
              let p = got
        else { return nil }
        return AppCloudPricing(vcpuHourUsd: p.vcpuHourUsd, memoryGibHourUsd: p.memoryGibHourUsd)
    }

    public func teleportContext(id: String) async throws -> (CuaSpacesFFI.Teleport, CuaSDK.Space)? {
        (cua.teleport(), try await cua.spaces().space(space: id))
    }

    public func teleportHandle() -> CuaSpacesFFI.Teleport? { cua.teleport() }

    public func sendFiles(id: String, paths: [String]) async throws -> [AppSentFileInfo] {
        let space = try await cua.spaces().space(space: id)
        var sent: [AppSentFileInfo] = []
        for path in paths {
            let report = try await space.sendFile(localPath: path, options: SpaceSendFileOptions(
                targetDirectory: nil, respectIgnoreFiles: true, conflict: nil))
            sent.append(appSentFile(report: report))
        }
        return sent
    }

    public func shares(id: String) async throws -> [AppShareEntryInput] {
        appShareRows(try await cua.spaces().space(space: id).shares())
    }

    public func share(id: String, who: String, role: String) async throws -> [AppShareEntryInput] {
        appShareRows(try await cua.spaces().space(space: id).share(who: who, role: role))
    }

    public func unshare(id: String, who: String) async throws -> [AppShareEntryInput] {
        appShareRows(try await cua.spaces().space(space: id).unshare(who: who))
    }

    public func agentRuns(id: String) async throws -> [AppSpaceAgentRun] {
        let runs = try await cua.spaces().space(space: id).agentList()
        return appAgentRows(runsJson: runs.map(\.json))
    }

    public func thumbnail(id: String, maxAgeMs: UInt64?) async -> SpaceThumbnailData? {
        let result = await withTimeout(seconds: 5) { [cua] in
            try await cua.spaces().space(space: id).thumbnail(maxAgeMs: maxAgeMs)
        }
        guard let t = try? result.get() else { return nil }
        return SpaceThumbnailData(image: t.image,
                                  capturedAt: Date(timeIntervalSince1970: TimeInterval(t.capturedAtMs) / 1000))
    }

    public func appIcons(id: String, requests: [SpaceAppIconRequest]) async -> [Data?] {
        let result = await withTimeout(seconds: 30) { [cua] in
            try await cua.spaces().space(space: id).appIcons(requests: requests)
        }
        guard let icons = try? result.get() else { return requests.map { _ in nil } }
        return icons.map { $0?.bytes }
    }

    public func usage(id: String) async -> AppSpaceUsage? {
        let result = await withTimeout(seconds: 5) { [cua] in
            try await cua.spaces().space(space: id).usage()
        }
        guard let u = try? result.get() else { return nil }
        return AppSpaceUsage(memoryUsed: u.memoryUsed, memoryTotal: u.memoryTotal, memoryLimited: u.memoryLimited,
                             diskUsed: u.diskUsed, diskTotal: u.diskTotal, diskLimited: u.diskLimited)
    }

    public func primaryDisplay(id: String) async -> AppStreamDisplay? {
        let result = await withTimeout(seconds: 5) { [cua] in
            try await cua.spaces().space(space: id).displays()
        }
        guard let d = (try? result.get())?.first, d.widthPx > 0, d.heightPx > 0 else { return nil }
        return AppStreamDisplay(widthPx: d.widthPx, heightPx: d.heightPx)
    }

    static func words(_ error: Error) -> String {
        // Every CuaError case carries one `message`: show it, not the case.
        if let e = error as? CuaError {
            if let message = Mirror(reflecting: e).children.first?.value,
               let text = Mirror(reflecting: message).children.first?.value as? String ?? message as? String {
                return text
            }
            return e.localizedDescription
        }
        return String(describing: error)
    }
}

/// The SDK's create progress, to a closure.
final class CreateListener: SpaceCreateListener {
    private let forward: @Sendable (SpaceCreateProgress) -> Void
    init(_ forward: @escaping @Sendable (SpaceCreateProgress) -> Void) { self.forward = forward }
    func onProgress(progress: SpaceCreateProgress) { forward(progress) }
}

enum TimeoutError: Error { case timedOut }

/// Runs `body`, giving up after `seconds`.
func withTimeout<T: Sendable>(seconds: Double, _ body: @escaping @Sendable () async throws -> T) async -> Result<T, Error> {
    await withTaskGroup(of: Result<T, Error>.self) { group in
        group.addTask {
            do { return .success(try await body()) } catch { return .failure(error) }
        }
        group.addTask {
            try? await Task.sleep(for: .seconds(seconds))
            return .failure(TimeoutError.timedOut)
        }
        let first = await group.next() ?? .failure(TimeoutError.timedOut)
        group.cancelAll()
        return first
    }
}

extension AppSpaceOs {
    init?(word: String) {
        switch word.lowercased() {
        case "macos", "darwin": self = .macos
        case "windows": self = .windows
        case "linux": self = .linux
        default: return nil
        }
    }

    public var label: String {
        switch self {
        case .macos: return "macOS"
        case .windows: return "Windows"
        case .linux: return "Linux"
        }
    }
}

extension AppLocation {
    var word: String {
        switch self {
        case .cloud: "cloud"
        case .local: "local"
        case .yours: "yours"
        case .host: "host"
        }
    }
}
extension AppSpaceKind {
    var word: String { self == .vm ? "vm" : "container" }

    /// The SDK's `container` / `vm`; `nil` when empty or unknown.
    public init?(word: String) {
        switch word {
        case "container": self = .container
        case "vm": self = .vm
        default: return nil
        }
    }
}
extension AppRuntime {
    var word: String {
        switch self {
        case .auto: return "auto"
        case .gvisor: return "gvisor"
        case .runc: return "runc"
        case .qemu: return "qemu"
        case .lume: return "lume"
        case .kubevirt: return "kubevirt"
        }
    }
}

/// A fixture failure, worded like the SDK's.
public struct FixtureError: Error, CustomStringConvertible {
    public let message: String
    public var description: String { message }
}

/// Synthetic Spaces for previews, tests and snapshot references. Never
/// contacts anything. Its state lives on the main actor: the async calls
/// hop there, so a test reading it races nothing.
public final class FixtureSpacesBackend: SpacesBackend, @unchecked Sendable {
    public var fixtureRows: [AppSpaceRow]
    public private(set) var created: [AppCreateSpaceArgs] = []
    public private(set) var added: [String] = []
    public private(set) var removed: [String] = []
    /// The ids of those that were only forgotten (Remove from List).
    public private(set) var forgotten: [String] = []
    /// The fixture cloud is connected (`cloud_connect` ran).
    public var fixtureCloudConnected = false
    /// The cloud tools called, in order.
    public internal(set) var fixtureCloudCalls: [String] = []

    public init(rows: [AppSpaceRow] = FixtureSpacesBackend.sample) {
        self.fixtureRows = rows
    }

    public static let sample: [AppSpaceRow] = [
        AppSpaceRow(id: "local:aurora", name: "0123456789ab", provider: "local", spacesdVersion: "0.4.0",
                    features: ["desktop_stream", "window_stream"], addedAt: "2026-09-25T08:00:00Z",
                    os: .linux, osName: "Ubuntu", osPrettyName: "Ubuntu 24.04.3 LTS",
                    image: "ghcr.io/trycua/linux:24.04",
                    imageDigest: "sha256:4f1c2a9d8e7b6c5a4f3e2d1c0b9a8f7e6d5c4b3a2f1e0d9c8b7a6f5e4d3c2b1a",
                    kind: .container, arch: "arm64", reachable: true, error: nil, host: nil, hostName: nil,
                    power: "suspend", powerState: "running", cloud: nil, cloudPlace: nil, cloudDelete: nil),
        AppSpaceRow(id: "cloud:builder", name: "builder", provider: "cloud", spacesdVersion: "0.4.0",
                    features: ["desktop_stream"], addedAt: "2026-09-24T08:00:00Z",
                    os: .linux, osName: "Arch Linux", osPrettyName: nil, image: nil, imageDigest: nil,
                    kind: nil, arch: nil, reachable: false, error: "timed out", host: nil, hostName: nil,
                    power: nil, powerState: nil, cloud: nil, cloudPlace: nil, cloudDelete: nil),
        AppSpaceRow(id: "direct:10.0.0.5:3211", name: "10.0.0.5:3211", provider: "direct",
                    spacesdVersion: "0.4.0", features: [], addedAt: nil, os: .windows,
                    osName: "Windows", osPrettyName: "Windows Server 2022 Datacenter 10.0.20348",
                    image: nil, imageDigest: nil, kind: nil, arch: nil, reachable: true, error: nil, host: nil, hostName: nil,
                    power: nil, powerState: nil, cloud: nil, cloudPlace: nil, cloudDelete: nil),
    ]

    public func rows() async throws -> [AppSpaceRow] { await MainActor.run { fixtureRows } }

    /// Progress a fixture create reports, in order (the SDK's words).
    public var createPhases: [SpaceCreateProgress] = [
        SpaceCreateProgress(phase: "preparing", fraction: nil, detail: ""),
        SpaceCreateProgress(phase: "pulling", fraction: 0.5, detail: "ghcr.io/trycua/linux:24.04"),
        SpaceCreateProgress(phase: "booting", fraction: nil, detail: ""),
        SpaceCreateProgress(phase: "ready", fraction: nil, detail: ""),
    ]
    /// Makes the next create fail with this message.
    public var createError: String?
    /// Holds each create until the test releases it (`releaseCreate`).
    public var holdCreates = false
    private var held: [CheckedContinuation<Void, Never>] = []
    private var released = false

    @MainActor public func releaseCreate() {
        released = true
        for c in held { c.resume() }
        held.removeAll()
    }

    public func create(_ args: AppCreateSpaceArgs, createId: String,
                       progress: @escaping @Sendable (SpaceCreateProgress) -> Void) async throws -> String {
        let (phases, hold) = await MainActor.run {
            created.append(args)
            createIds.append(createId)
            return (createPhases, holdCreates)
        }
        for p in phases.dropLast() { progress(p) }
        if hold {
            await withCheckedContinuation { c in
                Task { @MainActor in
                    if self.released { c.resume() } else { self.held.append(c) }
                }
            }
        }
        let (failure, cancelled): (String?, Bool) = await MainActor.run {
            defer { createError = nil }
            return (createError, cancelledCreates.contains(createId))
        }
        if cancelled { throw CuaError.Cancelled(message: "Cancelled \(createId); removed what it made.") }
        if let failure { throw FixtureError(message: failure) }
        if let last = phases.last { progress(last) }
        return await MainActor.run {
            let id = "local:\(args.name ?? "space-\(created.count)")"
            fixtureRows.append(AppSpaceRow(id: id, name: args.name ?? "", provider: args.on,
                                           spacesdVersion: "0.4.0", features: ["desktop_stream"],
                                           addedAt: nil, os: .linux, osName: nil, osPrettyName: nil,
                                           image: args.image, imageDigest: nil, kind: args.kind, arch: nil,
                                           reachable: true, error: nil, host: nil, hostName: nil,
                                           power: nil, powerState: nil, cloud: nil, cloudPlace: nil, cloudDelete: nil))
            return id
        }
    }

    public func add(url: String, token: String?, name: String?) async throws {
        await MainActor.run { added.append(url) }
    }

    /// The `createId` each create was started with, in order.
    public private(set) var createIds: [String] = []
    /// Creates cancelled (here or "elsewhere": a test adds an id to end
    /// that create cancelled, as `cua spaces cancel` would).
    public var cancelledCreates: Set<String> = []
    /// Makes the next cancel fail with this message.
    public var cancelError: String?
    /// The keys `cancelCreate` was called with.
    public private(set) var cancelRequests: [String] = []

    /// Cancels a held create: it ends with `CuaError.Cancelled` once
    /// released, as the SDK's does after its clean-up.
    public func cancelCreate(createId: String) async throws {
        let failure: String? = await MainActor.run {
            cancelRequests.append(createId)
            defer { cancelError = nil }
            if cancelError == nil { cancelledCreates.insert(createId) }
            return cancelError
        }
        if let failure { throw FixtureError(message: failure) }
        await releaseCreate()
    }

    /// The fixture Mac's GPU options (none: no GPU row).
    public var fixtureGpus: [AppGpuChoice]?
    public func gpuChoices() async -> [AppGpuChoice]? { await MainActor.run { fixtureGpus } }

    /// Holds each remove until the test releases it (`releaseRemove`).
    public var holdRemoves = false
    /// Makes the next remove fail with this message.
    public var removeError: String?
    private var heldRemoves: [CheckedContinuation<Void, Never>] = []

    @MainActor public func releaseRemove() {
        holdRemoves = false
        for c in heldRemoves { c.resume() }
        heldRemoves.removeAll()
    }

    /// `(id, on)` of each power press, in order.
    public private(set) var powerRequests: [(id: String, on: Bool)] = []
    /// Makes the next power action fail with this message.
    public var powerError: String?
    /// Holds each power action until the test releases it (`releasePower`).
    public var holdPower = false
    private var heldPower: [CheckedContinuation<Void, Never>] = []

    @MainActor public func releasePower() {
        holdPower = false
        for c in heldPower { c.resume() }
        heldPower.removeAll()
    }

    /// Records the press, then leaves the row as the SDK would: suspended
    /// or stopped (not answering), or running again.
    public func setPower(id: String, on: Bool) async throws {
        let hold = await MainActor.run {
            powerRequests.append((id: id, on: on))
            return holdPower
        }
        if hold {
            await withCheckedContinuation { c in
                Task { @MainActor in
                    if self.holdPower { self.heldPower.append(c) } else { c.resume() }
                }
            }
        }
        let failure: String? = await MainActor.run {
            defer { powerError = nil }
            return powerError
        }
        if let failure { throw FixtureError(message: failure) }
        await MainActor.run {
            guard let i = fixtureRows.firstIndex(where: { $0.id == id }) else { return }
            let off = fixtureRows[i].power == "stop" ? "stopped" : "suspended"
            fixtureRows[i].powerState = on ? "running" : off
            fixtureRows[i].reachable = on
        }
    }

    public func remove(id: String, removeOnly: Bool) async throws {
        let hold = await MainActor.run {
            removed.append(id)
            if removeOnly { forgotten.append(id) }
            return holdRemoves
        }
        if hold {
            await withCheckedContinuation { c in
                Task { @MainActor in
                    if self.holdRemoves { self.heldRemoves.append(c) } else { c.resume() }
                }
            }
        }
        let failure: String? = await MainActor.run {
            defer { removeError = nil }
            return removeError
        }
        if let failure { throw FixtureError(message: failure) }
        await MainActor.run { fixtureRows.removeAll { $0.id == id } }
    }

    public func streamProvider(id: String) async throws -> SpaceStreamSourceProviding {
        OfflineStreamSourceProvider()
    }

    /// The fixture Spaces' display.
    public var fixtureDisplay: AppStreamDisplay? = AppStreamDisplay(widthPx: 1280, heightPx: 800)
    public func primaryDisplay(id: String) async -> AppStreamDisplay? { await MainActor.run { fixtureDisplay } }

    /// The fixture Spaces' memory and storage: a 4 GB container limit on a
    /// host disk (Storage hides).
    public var fixtureUsage: AppSpaceUsage? = AppSpaceUsage(
        memoryUsed: 2_254_857_830, memoryTotal: 4 << 30, memoryLimited: true,
        diskUsed: 150_000_000_000, diskTotal: 494_000_000_000, diskLimited: false)
    public func usage(id: String) async -> AppSpaceUsage? { await MainActor.run { fixtureUsage } }

    /// The fixture Mac's ready local runtimes.
    public var fixtureBackends: [String] = ["docker"]
    public func localBackends() async -> [String]? { await MainActor.run { fixtureBackends } }
    /// The fixture Mac's free space and pulled images (none: the wizard
    /// shows no room lines).
    public var fixtureStorage: LocalStorage?
    public func localStorage() async -> LocalStorage? { await MainActor.run { fixtureStorage } }
    /// Signed in to Cua Cloud (off: the fixture account is local only).
    public var fixtureCloud = false
    public func cloudAvailable() async -> Bool { await MainActor.run { fixtureCloud } }
    /// The fixture account's Cua Cloud rates (none: no estimate).
    public var fixturePricing: AppCloudPricing?
    public func cloudPricing() async -> AppCloudPricing? { await MainActor.run { fixturePricing } }
    public func teleportContext(id: String) async throws -> (CuaSpacesFFI.Teleport, CuaSDK.Space)? { nil }
    public func teleportHandle() -> CuaSpacesFFI.Teleport? { nil }
    /// Run records (`agent_list` JSON), per Space id.
    public var fixtureRuns: [String: [String]] = [:]
    public var agentRunsError: Error?
    public func agentRuns(id: String) async throws -> [AppSpaceAgentRun] {
        let (error, runs) = await MainActor.run { (agentRunsError, fixtureRuns[id] ?? []) }
        if let error { throw error }
        return appAgentRows(runsJson: runs)
    }
    public private(set) var sent: [String] = []
    public func sendFiles(id: String, paths: [String]) async throws -> [AppSentFileInfo] {
        await MainActor.run { sent += paths }
        return paths.map { p in
            let name = URL(fileURLWithPath: p).lastPathComponent
            return AppSentFileInfo(name: name, dest: "/home/cua/Downloads/\(name)", bytes: 1024)
        }
    }
}
