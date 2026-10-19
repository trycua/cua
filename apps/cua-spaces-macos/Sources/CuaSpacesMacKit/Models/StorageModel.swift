// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSDK
import CuaSpacesFFI
import Foundation
import Observation

/// Settings, Storage: where the Cua Volume keeps its files, the Finder
/// volume and the block cache. The rows, words and decisions are the app
/// core's (`appStorageSection`, the Tauri app draws the same); this runs the
/// daemon tools the section asks for. S3 keys stay in the core's form state
/// until `volume_storage_set` sends them once; they are never written
/// anywhere by the app.
@MainActor @Observable
public final class StorageModel {
    private let tools: AgentsToolRunning?
    /// The tools' answers, as they came (nil: not read, or no answer).
    var storage: Any?
    var mount: Any?
    var cache: Any?
    public private(set) var state = appStorageInitial()
    /// Shows a path in Finder.
    public var reveal: (String) -> Void = { path in
        // The daemon may answer `~/Cua Volume`.
        let full = homeExpanded(path)
        NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: full)])
    }
    /// Opens a URL (System Settings' file system extensions).
    public var openURL: (URL) -> Void = { NSWorkspace.shared.open($0) }

    /// Where usage events go (a storage choice saved, the Finder volume on
    /// or off, Open in Finder: the app core's, like the Tauri app).
    @ObservationIgnored public var telemetry: TelemetryRunning?

    /// Every step: the usage events the core derives, then the reducer.
    private func reduce(_ action: AppStorageAction) {
        telemetry?.record(appTelemetryStorage(input: input, state: state, action: action))
        state = appStorageReduce(state: state, action: action)
    }

    public init(tools: AgentsToolRunning?) {
        self.tools = tools
    }

    var input: AppStorageInput {
        var i: [String: Any] = ["os": "macos", "home": userHome()]
        i["storage"] = storage ?? NSNull()
        i["mount"] = mount ?? NSNull()
        i["cache"] = cache ?? NSNull()
        return (try? appStorageInputFromJson(json: Self.json(i)))
            ?? (try! appStorageInputFromJson(json: "{\"os\":\"macos\"}"))
    }

    public var section: AppSettingsSection { appStorageSection(input: input, state: state) }

    static func json(_ v: Any) -> String {
        (try? JSONSerialization.data(withJSONObject: v, options: [.fragmentsAllowed]))
            .map { String(decoding: $0, as: UTF8.self) } ?? "null"
    }

    private func call(_ tool: String, _ args: [String: Any] = [:]) async throws -> Any {
        guard let tools else { throw AgentsToolError(message: "No cua daemon") }
        return try await tools.agentsTool(tool, args)
    }

    /// Reads the three answers (each on its own: a daemon without the
    /// drive's tools leaves them nil and the section says so).
    public func load() async {
        async let s = try? call("volume_storage")
        async let m = try? call("volume_mount_status")
        async let c = try? call("volume_cache_stats")
        (storage, mount, cache) = await (s, m, c)
        if let storage, let loaded = try? appStorageActionFromJson(
            json: Self.json(["type": "loaded", "storage": storage])) {
            let before = state
            reduce(loaded)
            // A bucket the agent connected: adopt it now.
            if !before.busy, case .adopt? = state.request { await runPending() }
        }
    }

    /// The agent prompt shows: poll for the bucket it sets up.
    public var watching: Bool { section.rows.contains { $0.id == "s3-prompt" } }

    private func runPending() async {
        guard let request = state.request, case let .adopt(update) = request else { return }
        do {
            let args = (try? JSONSerialization.jsonObject(with: Data(appStorageUpdateJson(update: update).utf8)))
                as? [String: Any] ?? [:]
            let check = try await call("volume_storage_set", args)
            reduce(try appStorageActionFromJson(
                json: Self.json(["type": "adopted", "check": check])))
        } catch {
            reduce(.adopted(check: AppDriveCheckInput(
                ok: false, reachable: false, authorized: false, versioning: false,
                detail: LiveSpacesBackend.words(error), applied: false)))
        }
    }

    public func press(_ id: String) async {
        if let a = appStoragePress(input: input, id: id) { await send(a) }
    }

    public func choose(_ id: String, _ option: String) async {
        if let a = appStorageChoose(id: id, option: option) { await send(a) }
    }

    public func edit(_ id: String, _ value: String) {
        if let a = appStorageEdit(id: id, value: value) { reduce(a) }
    }

    /// Advances the section and runs what it asks for.
    public func send(_ action: AppStorageAction) async {
        let before = state
        reduce(action)
        guard !before.busy, let request = state.request else { return }
        do {
            switch request {
            case let .test(update), let .save(update), let .adopt(update):
                let args = (try? JSONSerialization.jsonObject(with: Data(appStorageUpdateJson(update: update).utf8)))
                    as? [String: Any] ?? [:]
                let check = try await call("volume_storage_set", args)
                var type = "saved"
                if case .test = request { type = "checked" }
                if case .adopt = request { type = "adopted" }
                reduce(try appStorageActionFromJson(
                    json: Self.json(["type": type, "check": check])))
                if type != "checked" { await load() }
                return
            case .mount: _ = try await call("volume_mount")
            case .unmount: _ = try await call("volume_unmount")
            case let .reveal(path): reveal(path)
            case let .openUrl(url): if let u = URL(string: url) { openURL(u) }
            case let .setCache(bytes): _ = try await call("volume_cache_set", ["capacity_bytes": bytes])
            case .clearCache: _ = try await call("volume_cache_clear")
            }
            reduce(.done)
            await load()
        } catch {
            reduce(.failed(error: LiveSpacesBackend.words(error)))
        }
    }
}

/// The user's home folder (`$HOME` first, as the daemon sees it).
func userHome() -> String {
    ProcessInfo.processInfo.environment["HOME"].flatMap { $0.isEmpty ? nil : $0 } ?? NSHomeDirectory()
}

/// `~/x` under `userHome()`; anything else unchanged.
func homeExpanded(_ path: String) -> String {
    if path == "~" { return userHome() }
    if path.hasPrefix("~/") { return userHome() + path.dropFirst() }
    return path
}
