// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpacesFFI
import Foundation
import Observation

/// Whether the first run is done, and the file that says so
/// (`onboarding.json`). The pages are the shared web UI's (`/onboarding` in
/// apps/cua-spaces-web), in the New UI window: `RootView` opens it while
/// this is not `completed`, and the page asks and answers through the
/// bridge (`onboarding.get`, `onboarding.complete`), as the Electron shell's
/// does. The file is the same one both shells write.
@MainActor
@Observable
public final class OnboardingModel {
    public private(set) var completed: Bool
    /// What the first run chose: this Mac as a host, or a client.
    public private(set) var mode: AppOnboardingMode
    /// No state file yet: the app's very first launch here. The first run
    /// then opens even if the page kept a "Skip for now" from an earlier
    /// install (`markSeen` ends it).
    public private(set) var firstLaunch: Bool
    let statePath: String?
    /// Done's "Launch at login" checkbox, applied when the first run
    /// finishes (the app registers it, or not).
    public var onLaunchAtLogin: ((Bool) -> Void)?

    public init(statePath: String?) {
        self.statePath = statePath
        let saved = statePath.flatMap { Self.read($0) }
        self.completed = saved?.completed ?? false
        self.mode = saved?.mode ?? .client
        self.firstLaunch = statePath.map { !FileManager.default.fileExists(atPath: $0) } ?? false
    }

    /// The first run was shown at the first launch: saved as not done, so
    /// later launches keep to the page's "Skip for now".
    public func markSeen() {
        guard firstLaunch else { return }
        firstLaunch = false
        guard let statePath, !FileManager.default.fileExists(atPath: statePath) else { return }
        let url = URL(fileURLWithPath: statePath)
        try? FileManager.default.createDirectory(at: url.deletingLastPathComponent(),
                                                 withIntermediateDirectories: true)
        try? Data("{\"completed\":false,\"mode\":\"client\"}".utf8).write(to: url, options: .atomic)
    }

    /// "Start using Cua Spaces": saved to the state file.
    public func finish(mode: AppOnboardingMode = .client, launchAtLogin: Bool? = nil) {
        completed = true
        self.mode = mode
        if let launchAtLogin { onLaunchAtLogin?(launchAtLogin) }
        guard let statePath else { return }
        let url = URL(fileURLWithPath: statePath)
        try? FileManager.default.createDirectory(at: url.deletingLastPathComponent(),
                                                 withIntermediateDirectories: true)
        let word = mode == .host ? "host" : "client"
        try? Data("{\"completed\":true,\"mode\":\"\(word)\"}".utf8).write(to: url, options: .atomic)
    }

    /// Shows the flow again (Settings, Welcome): due until it finishes. The
    /// file stays as it was.
    public func restart() {
        completed = false
    }

    /// Settings, About, "Reset & Show": the saved first run is forgotten, so it
    /// is due from its first step. The state file is removed, as the app core's
    /// reset plan says (`onboarding_reset.rs`: `onboarding.json`). Safe to run
    /// twice; a `.damaged` copy kept earlier is left alone.
    public func reset() {
        completed = false
        mode = .client
        if let statePath { try? FileManager.default.removeItem(atPath: statePath) }
    }

    static func readCompleted(_ path: String) -> Bool? { read(path)?.completed }

    private static func read(_ path: String) -> (completed: Bool, mode: AppOnboardingMode)? {
        // No file: the first run is due. A file that is not valid state: due too, and kept as `.damaged`.
        guard let data = FileManager.default.contents(atPath: path) else { return nil }
        guard let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let completed = obj["completed"] as? Bool else {
            DamagedFile.keep(path)
            return nil
        }
        return (completed, (obj["mode"] as? String) == "host" ? .host : .client)
    }
}
