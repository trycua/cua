// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpaces
import CuaSpacesStreaming
#if canImport(AppKit)
import AppKit
import Combine
import Foundation
import SwiftUI

/// Everything the running app holds: the backend it attached to, the live
/// `BotStore` behind every screen, the one shared `LiveStreamSession` the Agent
/// Computer tiers and the PiP all observe, and where the user currently is.
///
/// There is deliberately **one** of these. The app model is one persistent Space per
/// account with roughly fifty Bots inside it, so the app has one store, one
/// stream session, and one navigation stack, not one of anything per Bot.
@MainActor
final class AppModel: ObservableObject {

    // MARK: Where the user is

    /// The app's navigation. Flat and value-typed rather than a
    /// `NavigationStack` path, because the two surfaces (mobile canvas and
    /// desktop shell) show the *same* route differently and a shared enum is
    /// the only thing both can read.
    enum Route: Hashable {
        /// The roster: home.
        case roster
        /// One Bot's single long-lived thread.
        case thread(String)
        /// Agent Computer tier 2: the preview pinned above the composer.
        case preview(String)
        /// Agent Computer tier 3: the full-screen takeover.
        case takeover(String)

        /// A Bot's Routines surface: recurring tasks it runs on a schedule.
        /// Carries a **bot** id.
        case routines(String)
        /// A thread with more than one Bot in it. Carries a **chat** id, not a
        /// bot id: a group has members rather than an owner, which is why this
        /// is the one route whose payload `botID` deliberately does not answer.
        case groupThread(String)

        var botID: String? {
            switch self {
            case .roster, .groupThread: return nil
            case .thread(let id), .preview(let id), .takeover(let id),
                 .routines(let id): return id
            }
        }

        /// The group chat this route is about, if it is one.
        var chatID: String? {
            if case .groupThread(let id) = self { return id }
            return nil
        }
    }

    /// Which surface the window is showing. Both are real surfaces over
    /// the same store, which is the point: they are checked separately
    /// because their layouts genuinely differ.
    enum Surface: String, CaseIterable, Identifiable {
        case mobile = "Mobile"
        case desktop = "Desktop"
        var id: String { rawValue }
    }

    @Published var signedIn = false
    @Published var route: Route = .roster
    @Published var surface: Surface = .mobile
    /// `system`, `light` or `dark` (`AppAppearance`). The window follows
    /// macOS unless the user picks one.
    @Published var desktopTheme: String = AppAppearance.system.rawValue
    /// The New Space wizard is open.
    @Published var creatingSpace = false
    /// Spaces this app created, newest first, for the sidebar and the status
    /// line. The roster keeps running on the Space the app attached to.
    @Published private(set) var createdSpaces: [String] = []
    /// The sidebar is showing.
    @Published var sidebarVisible = true
    /// A hire sheet is open over the roster.
    @Published var hiring = false
    /// A `NewGroupSheet` is open over the roster.
    @Published var creatingGroup = false
    /// The last thing that did not go through, shown under the composer.
    @Published var lastRefusal: String?
    /// What the backend attachment is doing, for the status strip.
    @Published private(set) var backendNote: String = "not connected"

    // MARK: What the app is made of

    let store: BotStore
    /// The app's single `RoutineStore`. One per app, not one per Bot: routines
    /// are persisted to a single file and fired by a single scheduler, and a
    /// store per Bot would mean a scheduler per Bot racing over that file.
    let routines = RoutineStore()
    /// The app's single `GroupChatStore`, for the same reason.
    let groups = GroupChatStore()
    let pip = StreamPiPController()
    /// The desktop shell's chrome: which pane is open, whether the header is
    /// in compose mode, which overlay is up. One per app, beside the store.
    let shell = ShellState()
    /// The one rcdp session every Agent Computer surface observes. `nil` until
    /// a Space with a streamable window is attached. The app is usable
    /// without it, it just has no live pixels.
    @Published private(set) var session: LiveStreamSession?
    /// Pop-outs of single Space windows, each on a stream of its own. `nil`
    /// until a Space is attached.
    @Published private(set) var windowPiPs: StreamPiPSet?
    /// App icons for the window list, looked up in the same Space.
    @Published private(set) var windowIcons: WindowIcons?

    private let client: SpacesClient
    /// The concrete MCP client, when there is one. Needed for `local_rcdp`,
    /// which is not in the `SpacesClient` protocol.
    private let mcp: SDKSpacesClient?
    let isLiveBackend: Bool

    // MARK: Construction

    init() {
        let (client, mcp, note) = Self.resolveBackend()
        self.client = client
        self.mcp = mcp
        self.isLiveBackend = mcp != nil
        self.store = BotStore(client: client, rosterFile: BotStore.rosterFileURL())
        self.backendNote = note
    }

    /// Choose a backend without ever creating a sandbox.
    ///
    /// The app attaches to the Space named by `OPENKOALABOTS_TEST_SPACE` through
    /// the cua SDK (a running `cua daemon`, or the Spaces runtime embedded in
    /// this process, `OPENKOALABOTS_SPACES=daemon|embedded`), and otherwise runs
    /// against `DemoSpacesClient` so that launching the app is never
    /// destructive and never fails. It deliberately does **not** fall through
    /// to `create_space` on its own: that can create a metered cloud sandbox
    /// (`FRICTION.md` §5), which is not something double-clicking an app should
    /// ever do.
    private static func resolveBackend() -> (SpacesClient, SDKSpacesClient?, String) {
        guard SDKSpacesClient.overriddenSpace != nil else {
            return (DemoSpacesClient(), nil,
                    "offline: set \(SDKSpacesClient.spaceOverrideVariable) to attach to a Space")
        }
        do {
            // Pure construction: no handshake and no subprocess. The first
            // real call (made from `start()`, which is `async`) pays for the
            // connection off the main actor (`FRICTION.md` §30, §47).
            let c = try SDKSpacesClient(backend: SDKSpacesClient.backendFromEnvironment())
            return (c, c, "live")
        } catch {
            return (DemoSpacesClient(), nil, "offline, the cua SDK did not start: \(error)")
        }
    }

    // MARK: Lifecycle

    private var started = false

    /// Attach, read the roster back, and start streaming if there is anything
    /// to stream. Safe to call more than once.
    func start() async {
        guard !started else { return }
        started = true
        await store.connect()

        // Routines and group chats both send *through* the roster store, so
        // they can only be attached once `BotStore.connect()` has decided what
        // is behind it. Before this point a fired routine would have nowhere to
        // go; the scheduler is therefore started here and not in `init`.
        routines.load()
        routines.attach(runner: BotStoreRoutineRunner(store: store))
        routines.startScheduler()
        groups.attach(messenger: BotStoreGroupMessenger(store: store))

        if case .attached(let space) = store.connection {
            backendNote = isLiveBackend ? "attached to \(space)" : backendNote
            await store.refreshWindows()
            attachStream(space: space)
        }
        store.startPolling(every: .seconds(3))
    }

    /// Shut down everything this app started. It never deletes the Space:
    /// the app attaches to a Space it does not own.
    func shutDown() async {
        routines.stopScheduler()
        _ = routines.save()
        _ = store.saveRosterIdentities()
        store.stopPolling()
        pip.popIn()
        windowPiPs?.popInAll()
        await session?.stop()
    }

    // MARK: Sign in

    /// The sign-in gate. Entirely local (there is no account system behind
    /// this sample), but it is a real gate: nothing but `SignInScreen` is
    /// reachable until it passes.
    func signIn() {
        signedIn = true
        route = .roster
    }

    // MARK: Streaming

    /// Build the one shared session over the Space's own frames.
    ///
    /// This used to pull `local_rcdp` apart by hand, rebuild an endpoint, and
    /// hold the two closures in an adapter: the three layers `FRICTION.md`
    /// §10 records as existing only to route around a missing capability. The
    /// SDK has the capability now: `Space.streamEndpoint()` is "hand me its
    /// frames", kept distinct from `Space.present(_:)`, which is "show this to
    /// the operator" and would draw on the wrong machine.
    private func attachStream(space: String) {
        guard let mcp else { return }
        Task { [weak self] in
            guard let handle = try? await mcp.sdkSpace(space) else { return }
            await MainActor.run {
                guard let self else { return }
                let s = LiveStreamSession(space: handle)
                self.session = s
                self.windowPiPs = StreamPiPSet(provider: SpaceStreamProvider(space: handle))
                self.windowIcons = WindowIcons(space: handle)
                // The shared presence cursor: joins once the stream is open,
                // and a Space without presence still streams.
                Task { await s.joinPresence(as: Self.presenceName) }
                // Avatars follow the colors the server assigned.
                self.presenceColors = s.$participants
                    .combineLatest(s.$localParticipant)
                    .receive(on: DispatchQueue.main)
                    .sink { others, me in
                        PresenceColorBook.shared.update(others + (me.map { [$0] } ?? []))
                    }
                Task { await s.refreshWindows() }
            }
        }
    }

    private var presenceColors: AnyCancellable?

    /// This operator's name on the Space's presence roster.
    static let presenceName = ProcessInfo.processInfo.environment["OPENKOALABOTS_PRESENCE_NAME"] ?? "Operator"

    /// What an Agent Computer tier should draw. `.fixture` when there is
    /// no Space behind the app, which keeps every surface usable offline.
    var screenSource: AgentScreenSource {
        session.map { .live($0) } ?? .fixture
    }

    /// Pop the stream out into a floating panel that survives navigation. This
    /// is what makes the PiP "available throughout": it is an `NSPanel` owned
    /// by the model, not a view inside any route.
    func togglePiP() {
        guard let session else { return }
        pip.toggle(session: session)
    }

    /// Pop one Space window out into a floating panel of its own. It streams
    /// that window only, beside the desktop pop-out, until its panel closes.
    func toggleWindowPiP(_ window: StreamWindow) {
        windowPiPs?.toggle(.window(window))
    }

    // MARK: Navigation

    func open(_ route: Route) {
        self.route = route
        lastRefusal = nil
        // Tell the poll loop which transcript is actually on screen, so it
        // spends its one per-tick `agent_status` on that Bot rather than on all
        // of them (`BotStore.startPolling`, `FRICTION.md` §51).
        store.focus(route.botID)
        if let id = route.botID { Task { await store.refresh(id) } }
    }

    /// Back out one level: takeover → thread → roster.
    func back() {
        switch route {
        case .roster: break
        case .groupThread:
            route = .roster
        case .thread:
            route = .roster
        case .routines(let id):
            // Routines is pushed *from* a thread, so backing out returns to it
            // rather than skipping a level to the roster.
            route = .thread(id)
        case .preview(let id), .takeover(let id):
            route = .thread(id)
        }
        lastRefusal = nil
    }

    /// Walk *up* the Agent Computer's three tiers for one Bot.
    ///
    /// The tiers are an escalation, not three destinations: tier 1 is the
    /// status glyph already inline in the transcript, tier 2 pins a preview
    /// above the composer, tier 3 takes the whole screen. The monitor button in
    /// the thread header is the escalator, and it wraps back to the thread so
    /// one control can walk the whole ladder in both directions.
    func escalate(from botID: String) {
        switch route {
        case .thread(botID):   open(.preview(botID))
        case .preview(botID):  open(.takeover(botID))
        case .takeover(botID): open(.thread(botID))
        default:               open(.preview(botID))
        }
    }

    /// The Bot the current route is about, falling back to the first in the
    /// roster so the desktop shell always has something to show.
    var focusedBot: Bot? {
        if let id = route.botID, let b = store.bot(id) { return b }
        return store.bots.first
    }

    // MARK: Talking to a Bot

    /// Send, and show the answer either way.
    ///
    /// A mid-turn refusal is correct behaviour, not an error, and it is exactly
    /// the thing this app must not swallow (`FRICTION.md` §24): it lands in
    /// `lastRefusal` and is drawn under the composer, as well as in the
    /// transcript and `store.notices`, which the store already does.
    func send(_ text: String, to botID: String) {
        Task {
            let outcome = await store.send(text, to: botID)
            lastRefusal = outcome.accepted
                ? nil
                : "Not delivered: \(outcome.reason.isEmpty ? "refused" : outcome.reason)"
        }
    }

    func stop(_ botID: String) { Task { await store.stop(botID) } }

    // MARK: Creating a Space

    /// What `Create` in the New Space wizard calls. The live client when the
    /// app is attached through the SDK; otherwise a client built on demand,
    /// because creating a Space is an explicit request and does not need the
    /// app to be attached to one first. Tests inject a fake.
    var spaceCreator: SpaceCreating?

    private func resolvedCreator() throws -> SpaceCreating {
        if let spaceCreator { return spaceCreator }
        if let mcp { spaceCreator = mcp; return mcp }
        let c = try SDKSpacesClient(backend: SDKSpacesClient.backendFromEnvironment())
        spaceCreator = c
        return c
    }

    /// Create the Space a wizard plan describes, through the SDK. With
    /// `openWhenReady`, its desktop opens in the floating viewer.
    @discardableResult
    func createSpace(_ plan: SpacePlan) async throws -> String {
        guard plan.isValid else {
            throw SpaceCreationError.invalidPlan(plan.systemError ?? plan.nameError ?? "invalid plan")
        }
        let creator = try resolvedCreator()
        let id = try await SpaceCreationFlow.create(plan, with: creator)
        createdSpaces.insert(id, at: 0)
        creatingSpace = false
        if plan.openWhenReady, let sdk = creator as? SDKSpacesClient {
            openDesktop(of: id, via: sdk)
        }
        return id
    }

    private func openDesktop(of space: String, via sdk: SDKSpacesClient) {
        Task { [weak self] in
            guard let handle = try? await sdk.sdkSpace(space) else { return }
            await MainActor.run {
                guard let self else { return }
                let s = LiveStreamSession(space: handle)
                Task { await s.refreshWindows() }
                self.pip.toggle(session: s)
            }
        }
    }

    // MARK: Group chats

    /// Open a freshly created group. Called back by `NewGroupSheet`.
    func opened(_ chat: GroupChat) {
        creatingGroup = false
        open(.groupThread(chat.id))
    }

    // MARK: Hiring

    /// The shapes and colours a new Bot is drawn from, so a hired Bot looks
    /// like it belongs on the roster rather than like a placeholder.
    static let paletteShapes: [BlobShape] =
        [.circle, .teardrop, .cloud, .hexagon, .egg, .lozenge, .squircle, .triangle, .arch]
    static let paletteColors: [UInt32] =
        [0x8B5CF6, 0x2F80F0, 0x18BE4B, 0x11B5A0, 0xF97316, 0xF4234B, 0x5B5BE8, 0xEE3E8F, 0xE0AE09]

    /// Hire a Bot from the UI: mint a roster identity for it, then start its
    /// one long-lived agent thread in the **shared** Space.
    ///
    /// Note what this does *not* do. It does not create a Space. The app
    /// supports roughly fifty Bots per account and they all live in one
    /// persistent VM, so a Space per Bot would be fifty sandboxes per user and
    /// wrong besides: screens are a presentation split, not a security
    /// boundary. `BotStore.hire` starts a run inside the Space the app is
    /// already attached to.
    /// `Create new Bot`, from the sidebar `+` or from the Marketplace's `Add`.
    ///
    /// **This is the path that replaces hiring, and the ordering is the
    /// point.** The agent is started first; only when that resolves does a
    /// roster row exist; then the conversation opens. There is consequently no
    /// moment at which the user can see a Bot that has nothing behind it, and
    /// therefore no state that needs a word like "not hired yet". If the start
    /// fails, no row appears and the failure is shown.
    @discardableResult
    func createBot(named name: String) async -> String? {
        do {
            let bot = try await store.createConversation(
                named: name, dark: desktopTheme != "light")
            hiring = false
            open(.thread(bot.id))
            return bot.id
        } catch {
            lastRefusal = "Could not create \(name): \(error)"
            return nil
        }
    }

    @discardableResult
    func hire(name: String, prompt: String) async -> String? {
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        let task = prompt.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, !task.isEmpty else { return nil }

        let id = Self.identifier(for: trimmed, taken: Set(store.bots.map(\.id)))
        let index = store.bots.count
        var bot = Persona.bot(id: id, name: trimmed,
                              dark: desktopTheme != "light",
                              preview: task, timestamp: BotStore.timestamp(Date()),
                              screenIndex: index)
        bot.preview = task
        store.register(bot)
        do {
            _ = try await store.hire(id, prompt: task)
            hiring = false
            open(.thread(id))
            return id
        } catch {
            lastRefusal = "Could not create \(trimmed): \(error)"
            hiring = false
            open(.thread(id))
            return nil
        }
    }

    /// A stable, readable id from a display name, unique within the roster.
    /// One implementation, in the store, because the id is also what the
    /// persona colour and shape are derived from and two implementations would
    /// eventually disagree about a Bot's colour.
    static func identifier(for name: String, taken: Set<String>) -> String {
        BotStore.identifier(for: name, taken: taken)
    }
}
#endif
