// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Cua
import CuaBotsCore
import Foundation

/// Keeps each bot reachable from the iOS app: publishes the bot's snapshot
/// into its own Space and applies the commands the phone drops there. The
/// phone reaches the Space's spacesd over the relay (or directly on a LAN),
/// so this needs nothing but Space files.
@MainActor
public final class RemoteBridge {
    let store: BotStore
    let engine: CuaEngine
    private var lastPublished: [String: Date] = [:]
    private var lastSnapshot: [String: RemoteSnapshot] = [:]
    private var task: Task<Void, Never>?

    public init(store: BotStore, engine: CuaEngine) {
        self.store = store
        self.engine = engine
    }

    public func start(every interval: Duration = .seconds(3)) {
        task?.cancel()
        task = Task { [weak self] in
            while !Task.isCancelled {
                await self?.tick()
                try? await Task.sleep(for: interval)
            }
        }
    }

    public func stop() { task?.cancel(); task = nil }

    func tick() async {
        for bot in store.bots where bot.spaceID != nil {
            await drainOutbox(bot)
            await publish(bot)
        }
    }

    /// Write the snapshot when it changed.
    func publish(_ bot: Bot) async {
        guard var snap = store.snapshot(bot.id) else { return }
        if var previous = lastSnapshot[bot.id] {
            previous.written = snap.written
            if previous == snap { return }
        }
        snap.written = Date()
        guard let space = try? await engine.space(bot),
              let home = try? await engine.home(bot, space),
              let data = try? snap.encoded() else { return }
        if (try? await space.write(path: "\(home)/\(RemoteSnapshot.path)", content: data)) != nil {
            lastSnapshot[bot.id] = snap
            lastPublished[bot.id] = Date()
        }
    }

    /// Apply and remove every command in the outbox, oldest first.
    func drainOutbox(_ bot: Bot) async {
        guard let space = try? await engine.space(bot),
              let home = try? await engine.home(bot, space) else { return }
        let dir = "\(home)/\(RemoteSnapshot.outbox)"
        guard let listing = try? await space.bash(command: "ls -1tr '\(dir)' 2>/dev/null | head -20", timeoutMs: 5000)
        else { return }
        for name in listing.stdout.split(separator: "\n").map(String.init) where name.hasSuffix(".json") {
            let path = "\(dir)/\(name)"
            let staging = FileManager.default.temporaryDirectory.appendingPathComponent("cua-bots-outbox-\(UUID().uuidString)")
            try? FileManager.default.createDirectory(at: staging, withIntermediateDirectories: true)
            defer { try? FileManager.default.removeItem(at: staging) }
            _ = try? await space.download(remotePath: path, destDir: staging.path)
            _ = try? await space.bash(command: "rm -f '\(path)'", timeoutMs: 5000)
            guard let data = try? Data(contentsOf: staging.appendingPathComponent(name)),
                  let command = try? RemoteCommand.decode(data), command.botID == bot.id else { continue }
            await store.apply(command)
        }
    }
}
