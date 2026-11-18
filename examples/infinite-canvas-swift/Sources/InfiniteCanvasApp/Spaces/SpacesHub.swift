// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CanvasModel
import Cua
import CuaSpaces
import Foundation

/// The Spaces the canvas shows, from a JSON file (`--spaces FILE`):
///
/// ```json
/// {"spaces": [{"label": "Linux", "os": "linux", "url": "127.0.0.1:32768", "token": "…",
///              "maxWindows": 3, "apps": ["firefox"],
///              "agent": {"harness": "claude-code", "baseURL": "http://…:8787",
///                        "model": "claude-mock-1", "env": {"ANTHROPIC_API_KEY": "…"}}}],
///  "threads": ["Linux"]}
/// ```
///
/// Every Space is an existing machine running cua-spacesd, added to a
/// throwaway registry (`add_space`); the canvas never creates or deletes one.
struct SpacesConfig: Decodable {
    struct Entry: Decodable {
        var label: String
        var os: SpaceOS
        var url: String
        var token: String?
        var maxWindows: Int?
        /// Window apps or titles to prefer, case-insensitive substrings.
        var apps: [String]?
        var desktop: Bool?
        var agent: Agent?
    }

    struct Agent: Decodable {
        var harness: String
        var baseURL: String?
        var model: String?
        var env: [String: String]?
    }

    var spaces: [Entry]
    /// Labels of Spaces that get an agent thread window.
    var threads: [String]?

    static func load(_ path: String) throws -> SpacesConfig {
        try JSONDecoder().decode(SpacesConfig.self, from: Data(contentsOf: URL(fileURLWithPath: path)))
    }
}

/// One attached Space and what the canvas keeps about it.
@MainActor
final class AttachedSpace {
    let entry: SpacesConfig.Entry
    let id: String
    let space: CuaSDK.Space
    let spacesd: SpacesdClient
    var presence: SpacePresence?
    var windows: [CuaSDK.SpaceWindow] = []
    /// Primary display size in points (for display-normalized cursors).
    var display: CGSize = .zero
    var presenceTask: Task<Void, Never>?

    init(entry: SpacesConfig.Entry, id: String, space: CuaSDK.Space, spacesd: SpacesdClient) {
        self.entry = entry
        self.id = id
        self.space = space
        self.spacesd = spacesd
    }

    /// Where the canvas's windows sit on the Space's display, for placing
    /// display-normalized agent cursors. Only windows with a tile count (a
    /// cursor over an untiled window has nowhere to be drawn). The window
    /// list carries no stacking order, so where tiled windows overlap the
    /// smaller one wins: it is the one drawn on top of the larger in
    /// practice (a dialog over its app, a window over a maximized browser).
    func placements(tiled: Set<String>) -> [WindowPlacement] {
        windows.compactMap { w in
            let b = w.bounds
            guard tiled.contains(w.windowId), b.count == 4, b[2] > 0, b[3] > 0 else { return nil }
            return WindowPlacement(windowID: w.windowId, bounds: CGRect(x: b[0], y: b[1], width: b[2], height: b[3]),
                                   z: -Int(b[2] * b[3]))
        }
    }
}

/// Connects to every configured Space through the cua SDK.
@MainActor
final class SpacesHub {
    let cua: Cua
    let connection: SpacesConnection
    private(set) var spaces: [AttachedSpace] = []

    /// `registryHome`: a temp directory for the Spaces registry, so the
    /// user's `~/.cua` is never read or written.
    init(registryHome: String) throws {
        cua = try Cua.embedded(spacesHome: registryHome, teleportHome: registryHome + "/teleport")
        connection = SpacesConnection(cua: cua)
    }

    func attach(_ entry: SpacesConfig.Entry) async throws -> AttachedSpace {
        // A Space that stops answering must not hold up the others.
        try await withTimeout(seconds: 20, "attaching \(entry.label)") { try await self.attachNow(entry) }
    }

    private func attachNow(_ entry: SpacesConfig.Entry) async throws -> AttachedSpace {
        let space = try await connection.add(url: entry.url, token: entry.token, name: entry.label)
        guard let native = try await space.native() else {
            throw SpacesError.malformedResponse(tool: "add_space", detail: "no SDK handle for \(entry.label)")
        }
        let spacesd = try await cua.spacesd(url: entry.url, token: entry.token)
        let attached = AttachedSpace(entry: entry, id: space.id.rawValue, space: native, spacesd: spacesd)
        attached.display = (try? await Self.primaryDisplay(spacesd)) ?? .zero
        spaces.append(attached)
        return attached
    }

    /// Windows worth a tile: visible, not tiny, titled; preferred apps
    /// first, then largest first; at most `maxWindows`.
    static func pickWindows(_ all: [CuaSDK.SpaceWindow], prefer: [String], max: Int) -> [CuaSDK.SpaceWindow] {
        let usable = all.filter { w in
            let b = w.bounds
            let big = b.count == 4 && b[2] >= 200 && b[3] >= 120
            return big && !(w.title.isEmpty && w.appName.isEmpty)
        }
        func rank(_ w: CuaSDK.SpaceWindow) -> Int {
            let hay = (w.appName + " " + w.title).lowercased()
            return prefer.firstIndex { hay.contains($0.lowercased()) } ?? prefer.count
        }
        func area(_ w: CuaSDK.SpaceWindow) -> Double { w.bounds.count == 4 ? w.bounds[2] * w.bounds[3] : 0 }
        let sorted = usable.sorted { (rank($0), -area($0)) < (rank($1), -area($1)) }
        // One window per app first (the largest), then the rest.
        var seen = Set<String>()
        var first: [CuaSDK.SpaceWindow] = []
        var rest: [CuaSDK.SpaceWindow] = []
        for w in sorted {
            if seen.insert(w.appName.lowercased()).inserted { first.append(w) } else { rest.append(w) }
        }
        return Array((first + rest).prefix(max))
    }

    static func primaryDisplay(_ client: SpacesdClient) async throws -> CGSize {
        let json = try await client.displays()
        guard let data = json.data(using: .utf8),
              let any = try? JSONSerialization.jsonObject(with: data) else { return .zero }
        let list = (any as? [[String: Any]]) ?? ((any as? [String: Any])?["displays"] as? [[String: Any]]) ?? []
        let d = list.first { ($0["primary"] as? Bool) == true } ?? list.first
        guard let b = d?["bounds"] as? [String: Any] else { return .zero }
        let w = (b["width"] as? NSNumber)?.doubleValue ?? 0
        let h = (b["height"] as? NSNumber)?.doubleValue ?? 0
        return CGSize(width: w, height: h)
    }

    /// Join the Space's presence as a human participant and deliver events
    /// on the main actor until cancelled.
    func joinPresence(_ s: AttachedSpace, onEvent: @escaping @MainActor (CuaSDK.PresenceEvent) -> Void,
                      onJoined: @escaping @MainActor (CuaSDK.PresenceParticipant, [CuaSDK.PresenceMember]) -> Void) async {
        let identity = PresenceIdentity(id: "infinite-canvas-\(ProcessInfo.processInfo.processIdentifier)",
                                        displayName: "Canvas", color: "", agent: false)
        let p: SpacePresence
        do {
            p = try await s.space.joinPresence(identity: identity, timeoutMs: 10_000)
        } catch {
            NSLog("infinite-canvas: presence unavailable on %@: %@", s.entry.label, "\(error)")
            return
        }
        s.presence = p
        do {
            let me = try await p.me()
            let roster = try await p.roster()
            NSLog("infinite-canvas: presence on %@: %d members", s.entry.label, roster.count)
            onJoined(me, roster)
        } catch {
            NSLog("infinite-canvas: presence roster on %@ failed: %@", s.entry.label, "\(error)")
        }
        s.presenceTask = Task {
            while !Task.isCancelled {
                // Bounded wait per call; the loop ends with the task.
                do {
                    if let e = try await p.nextEvent(timeoutMs: 1_000) {
                        if e.kind == "joined", let q = e.participant, ProcessInfo.processInfo.environment["CANVAS_DEBUG_PRESENCE"] != nil {
                            NSLog("presence joined %@ principal=%@ name=%@ color=%@", q.participantId, q.principalId, q.displayName, q.color)
                        }
                        onEvent(e)
                    }
                } catch {
                    // Closed or failed: back off instead of spinning.
                    try? await Task.sleep(for: .seconds(1))
                }
            }
        }
    }

    func refreshWindows(_ s: AttachedSpace) async {
        if let w = try? await s.space.windows(app: nil) { s.windows = w }
    }

    func shutdown() async {
        for s in spaces {
            s.presenceTask?.cancel()
            if let p = s.presence { try? await p.leave() }
            try? await connection.remove(SpaceID(s.id))
        }
        spaces = []
    }
}

struct TimeoutError: Error, CustomStringConvertible {
    var description: String
}

/// Run `body`, or throw after `seconds`.
@MainActor
func withTimeout<T: Sendable>(seconds: Double, _ what: String,
                              _ body: @escaping @MainActor () async throws -> T) async throws -> T {
    try await withThrowingTaskGroup(of: T.self) { group in
        group.addTask { @MainActor in try await body() }
        group.addTask {
            try await Task.sleep(for: .seconds(seconds))
            throw TimeoutError(description: "timed out \(what)")
        }
        defer { group.cancelAll() }
        return try await group.next()!
    }
}
