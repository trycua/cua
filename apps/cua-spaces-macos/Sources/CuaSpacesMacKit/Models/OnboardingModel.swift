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
    let statePath: String?
    /// Done's "Launch at login" checkbox, applied when the first run
    /// finishes (the app registers it, or not).
    public var onLaunchAtLogin: ((Bool) -> Void)?

    public init(statePath: String?) {
        self.statePath = statePath
        let saved = statePath.flatMap { Self.read($0) }
        self.completed = saved?.completed ?? false
        self.mode = saved?.mode ?? .client
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

    static func readCompleted(_ path: String) -> Bool? { read(path)?.completed }

    private static func read(_ path: String) -> (completed: Bool, mode: AppOnboardingMode)? {
        guard let data = FileManager.default.contents(atPath: path),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let completed = obj["completed"] as? Bool else { return nil }
        return (completed, (obj["mode"] as? String) == "host" ? .host : .client)
    }
}
