// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import Cua
import CuaBotsCore
import CuaBotsCua
import CuaSpaces
import CuaSpacesFFI
import CuaSpacesStreaming
import Foundation
import SwiftUI
import UserNotifications

struct BotDraft {
    var step = 0
    var name = ""
    var avatar = AvatarConfig(color: .sky, eyes: .star, ears: .scalloped)
    var harness: Harness = .hermes
    var placement: Placement = .local
    var customizedLook = false
}

enum Route: Hashable {
    case bot(String)
    case scheduled
    case approvals
    case outputs
    case newBot
}

/// Everything the window shows, and the one place the Cua pieces are wired:
/// the store, the engine (a Space per bot), the routine clock, the Keyvault
/// and host access.
@MainActor
final class AppModel: ObservableObject, Notifier {
    let store: BotStore
    let engine: CuaEngine?
    let clock: RoutineClock
    let keyvault: KeyvaultBridge
    let host: HostAccessBridge
    let remote: RemoteBridge?

    @Published var route: Route = .newBot
    @Published var draft = BotDraft()
    @Published var showProfile = true
    @Published var showComputer = false
    @Published var editingAvatar = false
    @Published var editingRule: CustomRule?
    @Published var showingRules = false
    @Published var showingMemory = false
    @Published var signInFor: ApprovalRequest?
    @Published var confirmReset: Bot?
    @Published var pairing: Bot?
    @Published var toast: BotNotification?
    @Published var openOutput: String?
    @Published var systemNotifications = UNAuthorizationStatus.notDetermined
    @Published var streams: [String: LiveStreamSession] = [:]
    @Published var engineError: String?
    let pips = PiPControllers()

    private var cancellable: AnyCancellable?

    init() {
        let env = ProcessInfo.processInfo.environment
        // From $HOME, not FileManager's lookup (which reads the account
        // database), so a run with a scratch HOME keeps everything in it.
        let home = env["HOME"].map(URL.init(fileURLWithPath:)) ?? FileManager.default.homeDirectoryForCurrentUser
        let support = home.appendingPathComponent("Library/Application Support/Cua Bots")
        let cuaHome = env["CUA_HOME"] ?? home.appendingPathComponent(".cua").path
        let dataDir = env["CUA_BOTS_DATA_DIR"].map(URL.init(fileURLWithPath:)) ?? support
        let volumeRoot = env["CUA_BOTS_VOLUME"].map(URL.init(fileURLWithPath:)) ?? support.appendingPathComponent("Volume")
        let volume = LocalVolume(root: volumeRoot)

        var engine: CuaEngine?
        // Persistent agents, the Cua Volume, teleport and the Keyvault for the
        // runtime this app embeds (the Cua Spaces extensions). Before the
        // first `Cua`.
        cuaSpacesRegister()
        do {
            let cua = env["CUA_BOTS_SPACES"] == "daemon" ? try Cua.connect(address: nil, token: nil)
                                                        : try Cua.embedded()
            engine = CuaEngine(cua: cua)
        } catch {
            engineError = "Couldn't start the Cua runtime: \(error.localizedDescription)"
        }
        self.engine = engine
        store = BotStore(volume: volume, dataDirectory: dataDir, engine: engine)
        clock = RoutineClock(fileURL: dataDir.appendingPathComponent("routines.json"), store: store)
        keyvault = KeyvaultBridge(cuaHome: cuaHome)
        keyvault.engine = engine
        host = HostAccessBridge(cuaHome: cuaHome)
        let st = store
        remote = engine.map { RemoteBridge(store: st, engine: $0) }
        store.notifier = self
        store.signIn = keyvault
        if let first = store.bots.first { route = .bot(first.id) }
        cancellable = store.objectWillChange.sink { [weak self] _ in self?.objectWillChange.send() }
        store.reconnect()
        clock.start()
        remote?.start()
        Task {
            await keyvault.refresh()
            await host.refresh()
            await refreshNotificationStatus()
        }
    }

    var selectedBot: Bot? {
        if case .bot(let id) = route { return store.bot(id) }
        return nil
    }

    func lastActive(_ id: String) -> Date? {
        store.messages(for: id).last { $0.role == .bot }?.date
    }

    // MARK: - Notifications

    /// System notifications need a bundled app; `CUA_BOTS_SYSTEM_NOTIFICATIONS=0`
    /// keeps them in the window only.
    var canUseSystemNotifications: Bool {
        Bundle.main.bundleIdentifier != nil
            && ProcessInfo.processInfo.environment["CUA_BOTS_SYSTEM_NOTIFICATIONS"] != "0"
    }

    func refreshNotificationStatus() async {
        guard canUseSystemNotifications else { return }
        systemNotifications = await UNUserNotificationCenter.current().notificationSettings().authorizationStatus
    }

    func enableNotifications() async {
        guard canUseSystemNotifications else { return }
        _ = try? await UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound, .badge])
        await refreshNotificationStatus()
    }

    func post(_ notification: BotNotification, bot: Bot) {
        withAnimation(.spring(response: 0.2, dampingFraction: 0.9)) { toast = notification }
        let id = notification.id
        Task {
            try? await Task.sleep(for: .seconds(6))
            if toast?.id == id { withAnimation(.easeIn(duration: 0.15)) { toast = nil } }
        }
        guard canUseSystemNotifications, systemNotifications == .authorized else { return }
        let content = UNMutableNotificationContent()
        content.title = notification.title
        content.body = notification.body
        content.threadIdentifier = bot.id
        UNUserNotificationCenter.current().add(
            UNNotificationRequest(identifier: notification.id, content: content, trigger: nil))
    }

    // MARK: - Actions

    func createBot(name: String, avatar: AvatarConfig, harness: Harness, placement: Placement) {
        Task {
            let bot = await store.createBot(name: name, avatar: avatar, harness: harness, placement: placement)
            route = .bot(bot.id)
        }
        route = .bot(Bot.agentName(for: name))
    }

    func createFromDraft() {
        let d = draft
        draft = BotDraft()
        createBot(name: d.name.trimmingCharacters(in: .whitespaces), avatar: d.avatar, harness: d.harness,
                  placement: d.placement)
    }

    func send(_ text: String) {
        guard let bot = selectedBot else { return }
        Task { await store.send(bot.id, text) }
    }

    func decide(_ approval: ApprovalRequest, _ approve: Bool) {
        if case .login = approval.source, approve {
            signInFor = approval
            return
        }
        Task { await store.decide(approval.id, approve: approve) }
    }

    func togglePause(_ bot: Bot) {
        Task { bot.isPaused ? await store.resume(bot.id) : await store.pause(bot.id) }
    }

    func reset(_ bot: Bot) {
        Task {
            streams[bot.id].map { s in Task { await s.stop() } }
            streams[bot.id] = nil
            await store.reset(bot.id)
            route = store.bots.first.map { .bot($0.id) } ?? .newBot
        }
    }

    // MARK: - Computer

    /// The bot's live screen, opened once and shared by the pane and PiP.
    func stream(for bot: Bot) async -> LiveStreamSession? {
        if let s = streams[bot.id] { return s }
        guard let engine, bot.spaceID != nil else { return nil }
        do {
            let space = try await engine.streamSpace(bot)
            let session = LiveStreamSession(space: space)
            streams[bot.id] = session
            await session.start()
            return session
        } catch {
            store.lastError = "Couldn't open \(bot.computerTitle): \(error.localizedDescription)"
            return nil
        }
    }

    func popOut(_ bot: Bot) {
        Task {
            guard let s = await stream(for: bot) else { return }
            pips.controller(for: bot.id).popOut(session: s, interactive: true)
        }
    }

    // MARK: - This Mac

    func setHostAccess(_ bot: Bot, allow: Bool) {
        Task {
            do {
                if allow {
                    try await host.allow()
                    store.setHostAccess(bot.id, .allowed(machine: host.machineName))
                } else {
                    try await host.revoke()
                    store.setHostAccess(bot.id, .off)
                }
            } catch {
                store.lastError = error.localizedDescription
            }
        }
    }
}

@MainActor
final class PiPControllers {
    private var controllers: [String: StreamPiPController] = [:]
    func controller(for id: String) -> StreamPiPController {
        if let c = controllers[id] { return c }
        let c = StreamPiPController()
        controllers[id] = c
        return c
    }
}

import Combine
