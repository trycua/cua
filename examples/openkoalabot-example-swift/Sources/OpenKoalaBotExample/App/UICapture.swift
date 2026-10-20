// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

#if canImport(AppKit)
import CuaSpaces
import AppKit
import Foundation
import SwiftUI

/// Screenshots the **running app**, one per surface, and quits.
///
/// Set `OPENKOALABOTS_UI_CAPTURE=<dir>` and launch the app normally. It signs in,
/// walks its own routes, and screenshots the real window at each stop.
///
/// Two deliberate choices:
///
/// * It drives the *model*, not the pointer. Synthesising clicks means
///   `CGEvent.post`, which needs Accessibility authorisation and raises a TCC
///   dialog on the operator's Mac. It calls exactly the closures the buttons
///   call — `model.signIn()`, `model.open(.thread(id))`,
///   `model.escalate(from:)`, `model.togglePiP()` — so the navigation under
///   test is the shipped one, not a parallel path.
/// * It captures with `screencapture -l <windowNumber>`, which photographs the
///   composited window as the user sees it, including the `CALayer` that the
///   rcdp decoder writes frames into. Rendering the SwiftUI tree offscreen
///   would not: `ImageRenderer` never instantiates an `NSViewRepresentable`,
///   which is the same reason the export path cannot show live pixels
///   (`RUBRIC.md`, and `FRICTION.md` §18).
@MainActor
enum UICapture {

    static let variable = "OPENKOALABOTS_UI_CAPTURE"

    /// The responsiveness harness: `OPENKOALABOTS_UI_PROBE=<dir>`.
    ///
    /// Same tour of the toolbar as the capture run, and deliberately **without
    /// hiring anything**. Two reasons. It measures the condition the user
    /// actually reported — a window attached to a Space that already has
    /// thirty-odd runs in it, whose poll loop is the thing competing for the
    /// main thread — and starting a run is not needed to reproduce that. And it
    /// means the measurement itself creates no run, no run directory, no
    /// LaunchAgent and no Terminal window, so it can be pointed at the live
    /// demo Space as often as needed without leaving a trace.
    static let probeVariable = "OPENKOALABOTS_UI_PROBE"

    /// The shell tour: `OPENKOALABOTS_SHELL_CAPTURE=<dir>`.
    ///
    /// Photographs every surface of the app shell in the **running
    /// app** — empty state, compose header, the dropdown, a real Bot being
    /// created, both affordance swaps, the details and Settings panes, the
    /// details pane. It drives
    /// `AppModel.shell`, which is the same object the controls drive, so what
    /// is photographed is the shipped path and not a parallel one.
    static let shellVariable = "OPENKOALABOTS_SHELL_CAPTURE"

    /// The markdown and pending-send tour: `OPENKOALABOTS_MD_CAPTURE=<dir>`.
    ///
    /// Photographs the two things the user reported, in the **running app**
    /// against a real Space: a reply that carries a fenced code block, a list
    /// and inline formatting, and the pending indicator that is up between
    /// pressing send and the first token arriving.
    ///
    /// It asks a real agent for the markdown rather than seeding a fixture,
    /// because a screenshot of a fixture would prove the renderer and not the
    /// path. It stops its run and shuts down in `finish`.
    static let markdownVariable = "OPENKOALABOTS_MD_CAPTURE"

    /// The design capture: `OPENKOALABOTS_DESIGN_CAPTURE=<stage>`.
    ///
    /// Puts the window in one state and leaves it there, for a screenshot
    /// taken from outside the app. Offline and hermetic: fixture Bots and
    /// transcripts, no Space call. Stages: `signin`, `empty`, `thread`,
    /// `wizard`, `wizard-local`, `computer`. `OPENKOALABOTS_APPEARANCE`
    /// (`light` / `dark`) pins the appearance.
    static let designVariable = "OPENKOALABOTS_DESIGN_CAPTURE"

    /// The picture-in-picture tour: `OPENKOALABOTS_PIP_CAPTURE=<dir>`, with the
    /// app attached to a live Space. Opens the Computer pane, pops the desktop
    /// out (the button on the stream) and one window from the pane's window
    /// list (`OPENKOALABOTS_PIP_WINDOW`, a title substring), samples both
    /// sessions' frame counts twice, and captures each window with
    /// `screencapture -l`, then the takeover. Registers one fixture Bot for the pane's header;
    /// hires nothing and creates nothing in the Space.
    static let pipVariable = "OPENKOALABOTS_PIP_CAPTURE"

    /// The shared presence cursor: `OPENKOALABOTS_PRESENCE_CAPTURE=<dir>`, with
    /// the app attached to a live Space where another participant is present.
    /// Opens the Computer pane, waits (bounded) until another participant's
    /// cursor is on the roster, and captures the window with `screencapture -l`.
    static let presenceVariable = "OPENKOALABOTS_PRESENCE_CAPTURE"

    /// A real thread next to the Computer pane: `OPENKOALABOTS_THREAD_CAPTURE=<dir>`,
    /// with the app attached to a live Space and a model endpoint
    /// (`OPENKOALABOTS_MODEL_URL`). Creates one Bot (`OPENKOALABOTS_THREAD_BOT`,
    /// default `Ada`; its agent opens with a
    /// greeting), sends `OPENKOALABOTS_THREAD_MESSAGE`, waits for the turn,
    /// opens the Computer pane with its window list and app icons, and
    /// captures the window with `screencapture -l`, the activity groups with
    /// a tool step opened and the rest collapsed. Stops its run afterwards.
    static let threadVariable = "OPENKOALABOTS_THREAD_CAPTURE"

    static func startIfRequested(model: AppModel, window: NSWindow) {
        let env = ProcessInfo.processInfo.environment
        if let stage = env[designVariable], !stage.isEmpty {
            if let a = env["OPENKOALABOTS_APPEARANCE"], AppAppearance(rawValue: a) != nil {
                model.desktopTheme = a
            }
            Task {
                await designStage(stage, model: model, window: window)
                // `OPENKOALABOTS_DESIGN_CAPTURE_OUT=<png>`: capture this window and quit.
                if let out = env["OPENKOALABOTS_DESIGN_CAPTURE_OUT"], !out.isEmpty {
                    await settle(2.0)
                    screencapture(window, out)
                    NSApp.terminate(nil)
                }
            }
            return
        }
        if let dir = env[presenceVariable], !dir.isEmpty {
            try? FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
            Task { await presenceRun(model: model, window: window, directory: dir) }
            return
        }
        if let dir = env[threadVariable], !dir.isEmpty {
            try? FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
            Task { await threadRun(model: model, window: window, directory: dir,
                                   message: env["OPENKOALABOTS_THREAD_MESSAGE"]
                                       ?? "Please check the memory on your computer.") }
            return
        }
        if let dir = env[pipVariable], !dir.isEmpty {
            try? FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
            Task { await pipRun(model: model, window: window, directory: dir,
                                wanted: env["OPENKOALABOTS_PIP_WINDOW"]?.lowercased()) }
            return
        }
        if let dir = env[markdownVariable], !dir.isEmpty {
            try? FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
            Task { await markdownRun(model: model, window: window, directory: dir) }
            return
        }
        if let dir = env[shellVariable], !dir.isEmpty {
            try? FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
            Task { await shellRun(model: model, window: window, directory: dir) }
            return
        }
        if let dir = env[probeVariable], !dir.isEmpty {
            try? FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
            Task { await probeRun(model: model, window: window, directory: dir) }
            return
        }
        guard let dir = env[variable], !dir.isEmpty else { return }
        try? FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
        Task { await run(model: model, window: window, directory: dir) }
    }

    private static func designStage(_ stage: String, model: AppModel, window: NSWindow) async {
        window.setContentSize(NSSize(width: 1240, height: 800))
        guard stage != "signin" else { return }
        model.signIn()
        await settle(0.5)
        for id in ["cos", "inbox", "sales", "growth", "support"] {
            var b = Fixtures.bot(id)
            b.pinned = false
            model.store.register(b)
            model.store.showFixture(Fixtures.thread(for: id))
        }
        model.store.showFixture(Thread(botID: "cos", messages: designThread))
        _ = model.routines.create(botID: "inbox", title: "Morning inbox sweep",
                                  prompt: "Triage the inbox and draft replies.",
                                  schedule: .dailyAt(hour: 8, minute: 0))
        if model.groups.chats.isEmpty {
            _ = try? model.groups.create(title: "Launch crew", members: ["cos", "sales", "growth"])
        }
        switch stage {
        case "empty":
            model.open(.roster)
        case "thread":
            model.open(.thread("cos"))
        case "computer":
            model.open(.preview("sales"))
        case "wizard":
            model.open(.roster)
            model.creatingSpace = true
        case "wizard-resources", "wizard-summary":
            model.open(.roster)
            designPlan = {
                var plan = SpacePlan()
                plan.select(image: "ghcr.io/trycua/linux:24.04-disk")
                plan.select(placement: .local)
                plan.cpus = 4
                plan.memoryGB = 8
                plan.name = "ubuntu-vm-local"
                return plan
            }()
            designStep = stage == "wizard-resources" ? .resources : .summary
            model.creatingSpace = true
        case "wizard-options":
            model.open(.roster)
            designPlan = {
                var plan = SpacePlan(placement: .cloud)
                plan.select(os: .windows)
                plan.name = "windows-cloud"
                return plan
            }()
            designStep = .options
            model.creatingSpace = true
        case "wizard-local":
            model.open(.roster)
            designPlan = {
                var plan = SpacePlan()
                plan.select(image: "ghcr.io/trycua/linux:24.04-disk")
                plan.select(placement: .local)
                plan.cpus = 4
                plan.memoryGB = 8
                return plan
            }()
            model.creatingSpace = true
        default:
            break
        }
    }

    /// The wizard's starting plan in a design capture, when a stage sets one.
    static var designPlan: SpacePlan?
    static var designStep: SpaceWizard.Step?

    private static let designThread: [Message] = [
        Message(sender: .bot, body: .systemEvent("Today 7:58 AM")),
        Message(sender: .user, body: .prose("Can you pull this week's signups and draft a short update for the team?")),
        Message(sender: .bot, body: .prose("""
            Pulled the numbers from the dashboard. Here is the draft:

            **Signups this week: 1,284** (up 12% on last week)

            - Organic search drove most of the growth
            - The new onboarding flow cut drop-off at step two by a third
            - Two enterprise trials started on Thursday

            Want me to post it in the team channel?
            """)),
        Message(sender: .user, body: .prose("Looks good. Post it and add a chart.")),
        Message(sender: .bot, body: .prose("Posted, with a chart of daily signups attached.")),
        Message(sender: .bot, body: .linkFile(LinkFile(
            title: "Weekly signups", subtitle: "chart.png, 84 KB", kind: .image))),
    ]

    /// The markdown transcript and the pending indicator, in the running app.
    private static func markdownRun(model: AppModel, window: NSWindow,
                                    directory: String) async {
        // Headless, for the reason `shellRun` gives: a run that opens a
        // Terminal window in the Space leaves a window behind, and Terminal's
        // restoration brings it *back*. This is somebody's demo machine.
        SDKSpacesClient.showsAgentWindows = false
        NSApp.activate(ignoringOtherApps: true)
        window.setContentSize(NSSize(width: 1180, height: 860))
        await settle(1.0)

        model.signIn()
        await settle(14.0)

        guard let botID = await model.createBot(named: "Markdown") else {
            shoot(window, "\(directory)/md-99-no-bot.png")
            finish()
            return
        }
        capturedRun = botID
        model.shell.opened()
        // Select the Bot that was just made. Without this the shot is of
        // whatever row the sidebar happened to have selected — which, on a
        // Space with earlier runs still in its roster, is somebody else's
        // conversation and looks exactly like a working screenshot.
        model.open(.thread(botID))
        // Let the kickstart greeting land, so the transcript is not empty
        // behind the shot that matters.
        await settle(22.0)

        // ---- The pending indicator, mid-send.
        //
        // Photographed immediately after `send`, which is the window the defect
        // was in: the user's message must already be on screen and the
        // indicator must be up beneath it, before any of the reply exists.
        let ask = """
            Reply with exactly this markdown and nothing else:

            ## Reading a file

            Use **readFile** for this, and note the `encoding` argument.

            ```swift
            func readFile(_ path: String) throws -> String {
                try String(contentsOfFile: path, encoding: .utf8)
            }
            ```

            Then:

            - it throws on a missing path
            - it assumes *UTF-8*
              - pass another encoding if you need one
            1. open
            2. read

            > Do not use it for very large files.

            See the [docs](https://example.com/a_(b)).
            """
        model.send(ask, to: botID)
        // Short: long enough for the row to rebuild, short enough that the
        // agent cannot have answered.
        await settle(0.9)
        shoot(window, "\(directory)/md-01-pending-mid-send.png")
        print("pending at shot time: \(model.store.isAwaitingReply(botID))")

        await settle(3.0)
        shoot(window, "\(directory)/md-02-pending-still-up.png")

        // ---- The rendered reply.
        for _ in 0..<40 {
            await settle(3.0)
            if !model.store.isAwaitingReply(botID) { break }
        }
        await settle(10.0)
        shoot(window, "\(directory)/md-03-markdown-rendered-dark.png")

        model.desktopTheme = "light"
        await settle(2.5)
        shoot(window, "\(directory)/md-04-markdown-rendered-light.png")
        model.desktopTheme = "dark"
        await settle(1.5)

        print("pending after reply: \(model.store.isAwaitingReply(botID))")
        await finish(model: model)
    }

    /// The rebuilt shell, surface by surface.
    private static func shellRun(model: AppModel, window: NSWindow,
                                 directory: String) async {
        // A capture run creates a real Bot in the user's Space, and by default
        // a run opens a Terminal window there to watch its log. Fifteen
        // screenshots is not worth fifteen windows on somebody's demo machine,
        // and Terminal's window restoration means a leaked window comes *back*
        // — `SpaceHygiene` and `FRICTION.md` §56 are the record of how badly.
        // The live suites already run headless for this reason; so does this.
        SDKSpacesClient.showsAgentWindows = false
        NSApp.activate(ignoringOtherApps: true)
        window.setContentSize(NSSize(width: 1280, height: 820))
        await settle(1.5)

        model.signIn()
        model.surface = .desktop
        // Long enough for `connect()` and the first `agent_list`. The roster
        // comes back **empty** — the whole point — because no conversation in
        // this Space belongs to this app yet.
        await settle(14.0)
        shoot(window, "\(directory)/s01-empty-sidebar-and-stage.png")

        // The sidebar `+`: the header is replaced by `To:`, and the dropdown
        // offers the two create rows.
        model.shell.beginCompose()
        await settle(1.5)
        shoot(window, "\(directory)/s02-compose-to-field-and-dropdown.png")

        // `Create new Bot`, for real. Agent first, then the row.
        let created = await model.createBot(named: BotStore.newBotName)
        model.shell.opened()
        await settle(1.5)
        shoot(window, "\(directory)/s03-created-row-and-typing.png")
        // Let the kickstart turn actually produce something.
        await settle(20.0)
        shoot(window, "\(directory)/s04-greeting-and-choice-card.png")

        guard let botID = created ?? model.store.bots.first?.id else {
            shoot(window, "\(directory)/s99-nothing-created.png")
            finish()
            return
        }
        capturedRun = created

        // Swap 1: the monitor control unmounts, the pane's close button and the
        // gear mount, and the transcript **narrows**.
        model.shell.tap(.monitor)
        await settle(4.0)
        shoot(window, "\(directory)/s05-details-pane-open-monitor-swapped.png")

        // Swap 2: the gear unmounts, a highlighted back control takes its place,
        // and the close button stays at the far right.
        model.shell.tap(.gear)
        await settle(2.0)
        shoot(window, "\(directory)/s06-settings-pane-gear-swapped.png")

        model.shell.tap(.back)
        await settle(1.5)
        shoot(window, "\(directory)/s07-back-to-details.png")
        model.shell.tap(.collapse)
        await settle(1.5)
        shoot(window, "\(directory)/s08-pane-closed-transcript-full-width.png")

        // A message, so a row has a subtitle and the transcript has both sides.
        model.send("Say hello and then stop.", to: botID)
        await settle(14.0)
        shoot(window, "\(directory)/s12-conversation-with-subtitle.png")

        model.desktopTheme = "light"
        await settle(2.0)
        shoot(window, "\(directory)/s13-light.png")
        model.desktopTheme = "dark"
        await settle(1.5)

        // Cramped, to prove the pane narrows the transcript rather than
        // overlaying it and that below the fitting width it does not open.
        window.setContentSize(NSSize(width: 760, height: 620))
        model.shell.tap(.monitor)
        await settle(2.5)
        shoot(window, "\(directory)/s14-narrow-window-pane-does-not-overlay.png")
        window.setContentSize(NSSize(width: 1280, height: 820))
        await settle(2.0)
        shoot(window, "\(directory)/s15-wide-again.png")

        await finish(model: model)
    }

    /// Photograph the frontmost popover panel, falling back to the window.
    private static func snapPopover(directory: String, name: String, main: NSWindow) {
        let panel = NSApp.windows.first {
            $0 !== main && $0.isVisible && $0.className.contains("Popover")
        } ?? NSApp.windows.first { $0 !== main && $0.isVisible }
        shoot(panel ?? main, "\(directory)/\(name)")
    }

    /// Sign in, let the live poll loop get going, then drive the toolbar and
    /// time every interaction. Creates nothing in the Space.
    private static func probeRun(model: AppModel, window: NSWindow, directory: String) async {
        NSApp.activate(ignoringOtherApps: true)
        await settle(2.0)
        model.signIn()
        // Long enough for `connect()`, the first `agent_list`, and several full
        // poll ticks over the Space's existing runs — i.e. the steady state the
        // user was clicking in.
        await settle(20.0)
        snap(window, "\(directory)/probe-01-roster-mobile.png")

        let probe = AppWindowRegistry.shared.probe
        let botID = model.store.bots.first(where: {
            model.store.presence(for: $0.id).hasThread
        })?.id ?? model.store.bots.first?.id

        probe?.interaction("surface -> Desktop") { model.surface = .desktop }
        await settle(3.0)
        snap(window, "\(directory)/probe-02-desktop-dark.png")

        probe?.interaction("theme -> Light") { model.desktopTheme = "light" }
        await settle(2.0)
        snap(window, "\(directory)/probe-03-desktop-light.png")
        probe?.interaction("theme -> Dark") { model.desktopTheme = "dark" }
        await settle(2.0)

        if let botID {
            probe?.interaction("open thread") { model.open(.thread(botID)) }
            await settle(3.0)
            snap(window, "\(directory)/probe-04-desktop-thread.png")
            probe?.interaction("back to roster") { model.back() }
            await settle(2.0)
        }

        probe?.interaction("surface -> Mobile") { model.surface = .mobile }
        await settle(2.0)
        snap(window, "\(directory)/probe-05-mobile-roster.png")
        probe?.interaction("surface -> Desktop") { model.surface = .desktop }
        await settle(2.0)
        probe?.interaction("open hire sheet") { model.hiring = true }
        await settle(2.0)
        snap(window, "\(directory)/probe-06-hire-sheet.png")
        probe?.interaction("dismiss hire sheet") { model.hiring = false }
        await settle(2.0)

        // A small window, to prove the chrome does not overlap when cramped.
        window.setContentSize(NSSize(width: 720, height: 560))
        await settle(3.0)
        snap(window, "\(directory)/probe-07-desktop-small.png")
        model.surface = .mobile
        await settle(2.0)
        snap(window, "\(directory)/probe-08-mobile-small.png")

        // …and a large one.
        window.setContentSize(NSSize(width: 1600, height: 1000))
        model.surface = .desktop
        await settle(3.0)
        snap(window, "\(directory)/probe-09-desktop-large.png")
        model.surface = .mobile
        await settle(2.0)
        snap(window, "\(directory)/probe-10-mobile-large.png")

        await model.shutDown()
        AppWindowRegistry.shared.probe?.stop()
        print("ui-probe complete")
        NSApp.terminate(nil)
    }

    /// The tour. Each stop names the surface it is evidence for.
    private static func run(model: AppModel, window: NSWindow, directory: String) async {
        NSApp.activate(ignoringOtherApps: true)
        await settle(1.5)

        shoot(window, "\(directory)/01-signin.png")

        model.signIn()
        await settle(2.5)
        shoot(window, "\(directory)/02-roster.png")

        // Hire a Bot, for real, from the UI's own hire path — this is the
        // requirement under test, not a fixture lookup. The Bot is started in
        // the Space the app is already attached to; no Space is created.
        model.hiring = true
        await settle(1.5)
        shoot(window, "\(directory)/03-hire-sheet.png")

        let name = "Capture Bot \(UUID().uuidString.prefix(6))"
        let hired = await model.hire(
            name: String(name),
            prompt: "echo openkoalabots-ui-capture; sw_vers -productName")
        await settle(6.0)

        // Whatever we ended up on: the Bot just hired, else any hired run in
        // the Space, else the first roster entry.
        let botID = hired
            ?? model.store.bots.first { model.store.presence(for: $0.id).hasThread }?.id
            ?? model.store.bots.first?.id
        guard let botID else {
            shoot(window, "\(directory)/99-no-bots.png")
            finish()
            return
        }
        capturedRun = hired

        model.open(.roster)
        await settle(2.0)
        shoot(window, "\(directory)/02b-roster-after-hire.png")

        model.open(.thread(botID))
        await settle(2.0)
        // Send through the store, exactly as the composer does. If the Bot is
        // mid-turn this is refused, and the refusal is what gets photographed —
        // which is the point (`FRICTION.md` §24).
        model.send("Say hello and then stop.", to: botID)
        await settle(6.0)
        shoot(window, "\(directory)/04-thread.png")

        // Tier 2, then tier 3 — the same escalator the header button drives.
        model.escalate(from: botID)          // thread -> preview
        await settle(8.0)
        shoot(window, "\(directory)/05-agent-computer-tier2.png")

        model.escalate(from: botID)          // preview -> takeover
        await settle(8.0)
        shoot(window, "\(directory)/06-agent-computer-tier3.png")

        model.togglePiP()
        await settle(5.0)
        // The PiP is a separate panel; photograph it on its own, then in place
        // over a different route, which is the whole claim about it.
        if let panel = NSApp.windows.first(where: { $0 is NSPanel && $0.isVisible }) {
            shoot(panel, "\(directory)/07-pip-panel.png")
        }
        model.open(.roster)
        await settle(3.0)
        shoot(window, "\(directory)/08-roster-with-pip-open.png")
        model.togglePiP()
        await settle(1.0)

        // The toolbar, timed. These are the exact closures the toolbar buttons
        // call, invoked on the main actor the same way a click invokes them, so
        // the number recorded is the number of milliseconds the main thread is
        // unavailable to the user for that click. This is the measurement
        // behind defect 1; `MainThreadProbe` explains the method.
        let probe = AppWindowRegistry.shared.probe
        probe?.interaction("surface -> Desktop") { model.surface = .desktop }
        await settle(3.0)
        probe?.interaction("theme -> Light") { model.desktopTheme = "light" }
        await settle(1.0)
        probe?.interaction("theme -> Dark") { model.desktopTheme = "dark" }
        await settle(1.0)
        probe?.interaction("surface -> Mobile") { model.surface = .mobile }
        await settle(1.0)
        probe?.interaction("surface -> Desktop") { model.surface = .desktop }
        await settle(1.0)
        probe?.interaction("back") { model.back() }
        await settle(1.0)
        probe?.interaction("open thread") { model.open(.thread(botID)) }
        await settle(1.0)
        probe?.interaction("open hire sheet") { model.hiring = true }
        await settle(1.0)
        probe?.interaction("dismiss hire sheet") { model.hiring = false }
        await settle(1.0)

        // The other surface, same store, same route.
        model.open(.thread(botID))
        model.surface = .desktop
        await settle(6.0)
        shoot(window, "\(directory)/09-desktop-shell-dark.png")

        model.desktopTheme = "light"
        await settle(3.0)
        shoot(window, "\(directory)/10-desktop-shell-light.png")
        model.desktopTheme = "dark"

        model.escalate(from: botID)          // thread -> preview
        model.escalate(from: botID)          // preview -> takeover
        await settle(8.0)
        shoot(window, "\(directory)/11-desktop-takeover.png")

        // Routines, in the desktop right panel, with a real routine in it.
        // Created through `RoutineStore.create` — the same call the panel's own
        // editor makes — so what is photographed is the populated panel and not
        // an empty state.
        model.open(.thread(botID))
        _ = model.routines.create(botID: botID,
                                  title: "Morning check",
                                  prompt: "Summarise anything new and stop.",
                                  schedule: .dailyAt(hour: 9, minute: 0))
        await settle(3.0)
        shoot(window, "\(directory)/12-desktop-routines-panel.png")

        // The same panel on the phone canvas, pushed from the thread header's
        // overflow — `onOverflow` calls exactly this.
        model.surface = .mobile
        model.open(.routines(botID))
        await settle(2.5)
        shoot(window, "\(directory)/13-mobile-routines.png")

        // A group chat: created the way `NewGroupSheet` creates one, then
        // opened the way `onCreated` opens it.
        model.back()
        let members = Array(model.store.bots.prefix(GroupChat.maxBots).map(\.id))
        if members.count >= GroupChat.minBots,
           let chat = try? model.groups.create(title: "Launch crew", members: members) {
            model.opened(chat)
            await settle(2.5)
            shoot(window, "\(directory)/14-group-chat.png")
            model.back()
        } else {
            print("group chat: only \(members.count) bots on the roster, "
                  + "need \(GroupChat.minBots), not captured")
        }

        model.open(.roster)
        await settle(2.0)
        shoot(window, "\(directory)/15-roster-mobile.png")

        await finish(model: model)
    }

    /// Everything this run started, stopped.
    ///
    /// The app attaches to a Space it does not own and must leave no run
    /// behind. The Space itself is never deleted — that is not the app's to
    /// do, and `FRICTION.md` §8 records that cleanup is the caller's problem in
    /// the first place.
    private static var capturedRun: String?

    private static func finish(model: AppModel) async {
        if let id = capturedRun {
            let outcome = await model.store.stop(id)
            print("teardown: stopped \(id), stopped=\(outcome?.stopped.description ?? "no run")")
        }
        await model.shutDown()
        AppWindowRegistry.shared.probe?.stop()
        print("ui-capture complete")
        NSApp.terminate(nil)
    }

    private static func finish() {
        print("ui-capture complete (nothing to tear down)")
        NSApp.terminate(nil)
    }

    /// Let AppKit, the WebSocket and VideoToolbox all make progress. A plain
    /// `Task.sleep` on the main actor would starve the run loop and nothing
    /// would ever decode — the same reason `LiveTierHarness.pump` exists.
    private static func settle(_ seconds: Double) async {
        let deadline = Date().addingTimeInterval(seconds)
        while Date() < deadline {
            await withCheckedContinuation { c in
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.03) { c.resume() }
            }
        }
    }

    /// Photograph the window, without ever asking for a permission.
    ///
    /// The obvious thing — spawn `screencapture -l <windowNumber>` — does not
    /// work from inside the app, and the way it fails is the interesting part.
    /// Screen Recording is a TCC grant attributed to the *responsible* process,
    /// which for a child of this binary is this binary; an unbundled SwiftPM
    /// executable has no grant and no stable identity to hang one on. So the
    /// call returns success and writes nothing, and the only way to change that
    /// is to trigger a consent dialog — which this run is explicitly forbidden
    /// to do. See `FRICTION.md` §29.
    ///
    /// Handing the shot to a *supervising* process that does hold the grant does
    /// not help either: `screencapture -l <windowNumber>` aimed at this app's
    /// window exits 0 and writes a zero-byte file there too, because the window
    /// belongs to the unbundled process. Measured, not assumed — the run that
    /// found this is in `FRICTION.md` §29.
    ///
    /// So: **`CALayer.render(in:)`**, in-process. It walks the real layer tree
    /// of the real window — including the sublayer the rcdp decoder writes
    /// frames into, which is why it captures live pixels where `ImageRenderer`
    /// cannot. No TCC is involved because nothing is reading the screen; the
    /// app is drawing itself.
    /// `shoot`, with the capture's own main-thread cost kept out of the
    /// latency sample (see `MainThreadProbe.excluding`).
    private static func snap(_ window: NSWindow, _ path: String) {
        if let probe = AppWindowRegistry.shared.probe {
            probe.excluding { shoot(window, path) }
        } else {
            shoot(window, path)
        }
    }

    private static func pipRun(model: AppModel, window: NSWindow, directory: String,
                               wanted: String?) async {
        window.setContentSize(NSSize(width: 1240, height: 800))
        model.signIn()
        var bot = Fixtures.bot("sales")
        bot.pinned = false
        model.store.register(bot)
        // Bounded wait for the stream and the Space's window list.
        for _ in 0..<150 {
            if let s = model.session, !s.windows.isEmpty, model.windowPiPs != nil { break }
            await settle(0.2)
        }
        guard let session = model.session, let pips = model.windowPiPs,
              let target = session.windows.first(where: {
                  wanted == nil || $0.title.lowercased().contains(wanted!) }) else {
            print("pip-capture: no live session or matching window")
            NSApp.terminate(nil)
            return
        }
        model.open(.preview(bot.id))
        await settle(4.0)
        model.togglePiP()
        model.toggleWindowPiP(target)
        let desktopPanel = model.pip.window
        let windowPanel = pips.controller(for: .window(target))?.window
        if let d = desktopPanel, let w = windowPanel, let screen = NSScreen.main?.visibleFrame {
            d.setFrameTopLeftPoint(NSPoint(x: screen.minX + 30, y: screen.maxY - 30))
            w.setFrameTopLeftPoint(NSPoint(x: d.frame.maxX + 30, y: screen.maxY - 30))
        }
        let windowSession = pips.controller(for: .window(target))?.session
        await settle(6.0)
        let d1 = session.decodedFrameCount, w1 = windowSession?.decodedFrameCount ?? 0
        await settle(5.0)
        let d2 = session.decodedFrameCount, w2 = windowSession?.decodedFrameCount ?? 0
        print("pip-capture desktop: \(session.status) frames \(d1) -> \(d2)")
        print("pip-capture window \(target.displayName): \(windowSession.map { "\($0.status)" } ?? "none") "
              + "frames \(w1) -> \(w2)")
        screencapture(window, "\(directory)/computer-pane.png")
        if let desktopPanel { screencapture(desktopPanel, "\(directory)/pip-desktop.png") }
        if let windowPanel { screencapture(windowPanel, "\(directory)/pip-window.png") }
        model.pip.popIn()
        pips.popInAll()
        await settle(1.0)
        // The takeover, with the same drop zone as the Computer pane.
        model.open(.takeover(bot.id))
        await settle(5.0)
        screencapture(window, "\(directory)/takeover.png")
        print(d2 > d1 && w2 > w1 ? "pip-capture: PASS" : "pip-capture: FAIL")
        await model.shutDown()
        NSApp.terminate(nil)
    }

    private static func threadRun(model: AppModel, window: NSWindow, directory: String,
                                  message: String) async {
        SDKSpacesClient.showsAgentWindows = false
        // Tool steps show opened; every other group stays collapsed.
        ActivityGroupRow.openContaining = "Tool "
        window.setContentSize(NSSize(width: 1240, height: 800))
        model.signIn()
        for _ in 0..<150 where model.store.spaceID == nil { await settle(0.2) }
        let name = ProcessInfo.processInfo.environment["OPENKOALABOTS_THREAD_BOT"] ?? "Ada"
        guard let botID = await model.createBot(named: name) else {
            print("thread-capture: could not create the Bot")
            await finish(model: model)
            return
        }
        capturedRun = botID
        model.open(.thread(botID))
        // The greeting (the run installs its harness first).
        for _ in 0..<120 where model.store.creating.contains(botID) { await settle(1.0) }
        await settle(3.0)
        model.send(message, to: botID)
        await settle(2.0)
        for _ in 0..<120 {
            await settle(1.0)
            if !model.store.isAwaitingReply(botID),
               model.store.presence(for: botID).state != .running { break }
        }
        await settle(4.0)
        model.open(.preview(botID))
        for _ in 0..<100 {
            if let s = model.session, !s.windows.isEmpty, let icons = model.windowIcons,
               s.windows.contains(where: { icons.image(for: $0) != nil }) { break }
            await settle(0.2)
        }
        await settle(4.0)
        screencapture(window, "\(directory)/thread.png")
        let kinds = model.store.thread(for: botID).messages.map { m -> String in
            switch m.body {
            case .prose(let t): return "\(m.sender == .user ? "user" : "bot"): \(t.prefix(60))"
            case .activity(let g): return "activity: \(g.summary) \(g.steps)"
            case .systemEvent(let t): return "sys: \(t)"
            default: return "other"
            }
        }
        print("thread-capture transcript:\n  " + kinds.joined(separator: "\n  "))
        print("thread-capture windows: " + (model.session?.windows.map {
            "\($0.app)=\(model.windowIcons?.image(for: $0) != nil ? "icon" : "none")" } ?? []).joined(separator: ", "))
        await finish(model: model)
    }

    private static func presenceRun(model: AppModel, window: NSWindow, directory: String) async {
        window.setContentSize(NSSize(width: 1240, height: 800))
        model.signIn()
        var bot = Fixtures.bot("sales")
        bot.pinned = false
        model.store.register(bot)
        for _ in 0..<150 where model.session == nil { await settle(0.2) }
        guard let session = model.session else {
            print("presence-capture: no live session")
            NSApp.terminate(nil)
            return
        }
        model.open(.preview(bot.id))
        var seen = false
        for _ in 0..<150 {
            if session.participants.contains(where: { $0.normalizedCursor(in: session.surfaceSize) != nil }),
               session.decodedFrameCount > 0 { seen = true; break }
            await settle(0.2)
        }
        await settle(2.0)
        print("presence-capture: me=\(session.localParticipant?.name ?? "-") others="
              + session.participants.map { "\($0.name)@\($0.normalizedCursor(in: session.surfaceSize).map { "\($0)" } ?? "-")" }
                .joined(separator: ","))
        screencapture(window, "\(directory)/computer-presence.png")
        print(seen ? "presence-capture: PASS" : "presence-capture: FAIL")
        await model.shutDown()
        NSApp.terminate(nil)
    }

    /// The composited window as the user sees it, decoded frames included.
    private static func screencapture(_ window: NSWindow, _ path: String) {
        let proc = Process()
        proc.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
        proc.arguments = ["-o", "-x", "-l", String(window.windowNumber), path]
        try? proc.run()
        proc.waitUntilExit()
        print("captured \(path) (window #\(window.windowNumber))")
    }

    private static func shoot(_ window: NSWindow, _ path: String) {
        window.displayIfNeeded()
        if renderLayerTree(window, to: path) { return }
        print("captured \(path) FAILED (window #\(window.windowNumber), "
              + "visible=\(window.isVisible), policy=\(NSApp.activationPolicy().rawValue))")
    }

    private static func renderLayerTree(_ window: NSWindow, to path: String) -> Bool {
        guard let root = window.contentView, let layer = root.layer else { return false }
        let scale = window.backingScaleFactor
        let size = root.bounds.size
        let w = Int(size.width * scale), h = Int(size.height * scale)
        guard w > 0, h > 0,
              let ctx = CGContext(data: nil, width: w, height: h, bitsPerComponent: 8,
                                  bytesPerRow: 0, space: CGColorSpaceCreateDeviceRGB(),
                                  bitmapInfo: CGImageAlphaInfo.premultipliedFirst.rawValue)
        else { return false }
        ctx.scaleBy(x: scale, y: scale)
        // CoreGraphics puts the origin bottom-left and AppKit's layer geometry
        // is top-left, so a straight `render(in:)` comes out upside down.
        ctx.translateBy(x: 0, y: size.height)
        ctx.scaleBy(x: 1, y: -1)
        layer.render(in: ctx)
        guard let cg = ctx.makeImage() else { return false }
        let rep = NSBitmapImageRep(cgImage: cg)
        guard let data = rep.representation(using: .png, properties: [:]) else { return false }
        try? data.write(to: URL(fileURLWithPath: path))
        guard isUsable(path) else { return false }
        print("captured \(path) ok (\(w)x\(h), layer render)")
        return true
    }

    private static func isUsable(_ path: String) -> Bool {
        guard let size = try? FileManager.default
            .attributesOfItem(atPath: path)[.size] as? Int else { return false }
        return size > 2048
    }
}
#endif
