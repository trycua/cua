// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpaces
import CuaSpacesStreaming
import SwiftUI
import AppKit

/// Every screen the export can render.
///
/// The renders are regression baselines, and `RUBRIC.md` says so.
///
/// The phone screens are gone with the rest of the mobile surface: this is a
/// macOS desktop app.
struct Screen {
    var name: String
    var size: CGSize
    var view: AnyView
}

@MainActor
func screens() -> [Screen] {
    let d = DS.desktopCanvas
    return [
        Screen(name: "desktop-01-dark", size: d,
               view: AnyView(DesktopShell(theme: .dark))),
        Screen(name: "desktop-02-light", size: d,
               view: AnyView(DesktopShell(theme: .light, bot: Fixtures.bot("cos"),
                                          thread: Fixtures.cosThread))),
        Screen(name: "desktop-03-no-panel", size: d,
               view: AnyView(DesktopShell(theme: .dark, showRightPanel: false))),
    ]
}

@MainActor
func export(to dir: String) {
    let fm = FileManager.default
    try? fm.createDirectory(atPath: dir, withIntermediateDirectories: true)
    for s in screens() {
        let renderer = ImageRenderer(content:
            s.view.frame(width: s.size.width, height: s.size.height))
        renderer.scale = 2
        renderer.isOpaque = true
        guard let cg = renderer.cgImage else {
            FileHandle.standardError.write("FAILED to render \(s.name)\n".data(using: .utf8)!)
            continue
        }
        let rep = NSBitmapImageRep(cgImage: cg)
        guard let png = rep.representation(using: .png, properties: [:]) else { continue }
        let path = "\(dir)/\(s.name).png"
        try? png.write(to: URL(fileURLWithPath: path))
        print("\(cg.width)x\(cg.height)  \(path)")
    }
}

// MARK: - Entry

/// The command-line half of the binary.
///
/// This file used to be `main.swift`, and its subcommand dispatch used to be
/// top-level code. That is exactly what made an app impossible: a SwiftPM
/// executable target with top-level code **is** its entry point, and `@main`
/// in the same module is rejected ("'main' attribute cannot be used in a module
/// that contains top-level code"). Rather than sacrifice either half, the
/// dispatch moved into `CLI.run`, and `App/OpenKoalaBotsApp.swift` carries the
/// real `@main`, which calls this first and falls through to the SwiftUI app
/// when no subcommand matched. Every subcommand below is byte-for-byte the one
/// that shipped before, `export` in particular.
///
/// Returns `true` when it handled the arguments (in practice it exits), and
/// `false` when the caller should launch the GUI instead.
enum CLI {

    /// Run an `async` SDK call from the CLI's synchronous dispatch.
    ///
    /// The SDK is async all the way down (`FRICTION.md` §30), which is what an
    /// app wants, and what a command line has to bridge back. This is the only
    /// bridge, and it is deliberately confined to argument dispatch, where
    /// there is no UI to hitch.
    static func blocking<T: Sendable>(_ work: @escaping @Sendable () async throws -> T) throws -> T {
        let box = ResultBox<T>()
        Task.detached {
            do { await box.finish(with: .success(try await work())) }
            catch { await box.finish(with: .failure(error)) }
        }
        while box.value == nil {
            RunLoop.current.run(mode: .default, before: Date().addingTimeInterval(0.02))
        }
        return try box.value!.get()
    }

    private final class ResultBox<T: Sendable>: @unchecked Sendable {
        private let lock = NSLock()
        private var stored: Result<T, Error>?
        var value: Result<T, Error>? { lock.lock(); defer { lock.unlock() }; return stored }
        func finish(with result: Result<T, Error>) { lock.lock(); stored = result; lock.unlock() }
    }

    @discardableResult
    static func run(_ args: [String]) -> Bool {

if args.count >= 3, args[1] == "export" {
    MainActor.assumeIsolated { export(to: args[2]) }
    exit(0)
}

if args.count >= 2, args[1] == "spaces-probe" {
    // Verifies the Spaces wiring is real: opens the cua SDK's Spaces runtime
    // (`spaces-probe [daemon|embedded]`) and lists the contract tools.
    var env = ProcessInfo.processInfo.environment
    if args.count >= 3 { env["OPENKOALABOTS_SPACES"] = args[2] }
    do {
        let c = try SDKSpacesClient(backend: SDKSpacesClient.backendFromEnvironment(env))
        let tools = try blocking { try await c.handshake() }
        print("handshake OK: \(tools.count) tools offered by the cua Spaces runtime")
        let needed = ["create_space", "delete_space", "agent_start", "agent_message",
                      "agent_status", "agent_stop", "list_space_windows", "upload"]
        for n in needed {
            print("  \(tools.contains(n) ? "live " : "MISSING") \(n)")
        }
        print("capabilities:")
        for n in c.capabilities.notes { print("  - \(n)") }
    } catch {
        print("spaces probe failed: \(error)")
        exit(1)
    }
    exit(0)
}

if args.count >= 3, args[1] == "routine-fire" {
    // The scheduler proof. `Routine.swift` and `RoutineStore.swift` both refer
    // to this subcommand by name; it was documented but never written, because
    // the routines work and the rename of `main.swift` to `CLI.swift` were in
    // flight at the same time and neither branch owned this file. Written here.
    //
    //   OpenKoalaBotExample routine-fire <space> [botID]
    //
    // The backend is the cua SDK (`OPENKOALABOTS_SPACES=daemon|embedded`).
    //
    // It attaches to a Space it does not own, registers one routine due
    // immediately, lets the *real* scheduler tick fire it through
    // `BotStoreRoutineRunner`, prints the firing record, and stops the run it
    // started. Nothing is created and nothing is deleted.
    let space = args[2]
    let wanted: String? = args.count >= 4 ? args[3] : nil

    // **Pin the Space before anything else.**
    //
    // `BotStore.connect()` resolves its Space through
    // `SDKSpacesClient.ensureSpace()`, which honours the
    // `OPENKOALABOTS_TEST_SPACE` override and otherwise falls through to
    // `create_space` with `reuse`, and that can *create a cloud sandbox*
    // (`FRICTION.md` §5). The first version of this subcommand took a `<space>`
    // argument, never passed it anywhere, and duly created a sandbox on the
    // very first run instead of attaching to the local Space it had been
    // handed. Measured, not theorised (`FRICTION.md` §33). Taking a Space on
    // the command line and resolving a different one inside is the bug; this
    // makes the argument authoritative, and the assertion below makes a silent
    // repeat impossible.
    setenv(SDKSpacesClient.spaceOverrideVariable, space, 1)

    MainActor.assumeIsolated {
        func sync<T>(_ work: @escaping @MainActor () async -> T) -> T {
            var result: T?
            Task { @MainActor in result = await work() }
            while result == nil {
                RunLoop.current.run(mode: .default, before: Date().addingTimeInterval(0.02))
            }
            return result!
        }
        do {
            let client = try SDKSpacesClient(backend: SDKSpacesClient.backendFromEnvironment())
            _ = try blocking { try await client.handshake() }
            let store = BotStore(client: client)
            sync { await store.connect() }
            guard case .attached(let attached) = store.connection else {
                print("routine-fire: not attached: \(store.connection)")
                exit(1)
            }
            guard attached == space else {
                // Never proceed against a Space the caller did not name.
                print("routine-fire: asked for \(space) but attached to "
                      + "\(attached); refusing, nothing was started")
                exit(1)
            }
            print("attached to \(attached)")

            let botID = wanted ?? store.bots.first?.id
            guard let botID else { print("routine-fire: no bots on the roster"); exit(1) }

            // A throwaway file, not the app's. `RoutineStore(fileURL: nil)`
            // resolves to `defaultFileURL()`, which is the *user's* routine
            // list: a proof harness that appends a "routine-fire proof" entry
            // to it on every run, and leaves it there, is a harness that
            // vandalises the thing it is testing. The second run of this
            // command found two routines due for that reason.
            let scratch = URL(fileURLWithPath: NSTemporaryDirectory())
                .appendingPathComponent("openkoalabots-routine-fire-\(UUID().uuidString).json")
            defer { try? FileManager.default.removeItem(at: scratch) }
            let routines = RoutineStore(fileURL: scratch)
            routines.attach(runner: BotStoreRoutineRunner(store: store))
            let routine = routines.create(
                botID: botID,
                title: "routine-fire proof",
                prompt: "echo openkoalabots-routine-fire; date",
                // Due immediately: created a minute in the past on a
                // one-minute cadence, so the very next tick finds it.
                schedule: .everyMinutes(1),
                now: Date().addingTimeInterval(-120))

            print("routine \(routine.id) armed for \(botID); due now = "
                  + "\(routines.due(at: Date()).count)")

            let records = sync { await routines.tick() }
            for r in records {
                print("fired routine=\(r.routineID) \"\(r.title)\" "
                      + "at=\(r.at): \(r.firing.summary)")
            }
            if records.isEmpty { print("routine-fire: the scheduler found nothing due") }

            // Everything this run started, stopped.
            let outcome = sync { await store.stop(botID) }
            print("teardown: stopped \(botID), stopped=\(outcome?.stopped.description ?? "no run")")
            exit(records.contains { $0.firing.runID != nil } ? 0 : 1)
        } catch {
            print("routine-fire failed: \(error)")
            exit(1)
        }
    }
    exit(0)
}

if args.count >= 4, args[1] == "live-tiers" {
    // Proof harness for the Agent Computer tiers: mounts the *real* tier views
    // against a live Space and reports what each mounted view is holding.
    //
    //   OpenKoalaBotExample live-tiers <host:port> <token> [pngDir] [windowID]
    //
    // The spacesd at <host:port> is added to a temp Spaces registry (never
    // ~/.cua) and streamed through the SDK's `SpaceStreamProvider`.
    //
    // It opens windows, so unlike `export` it needs a window server. It never
    // creates, deletes or re-displays a Space, and the only input it sends is a
    // pointer move.
    let host = args[2], token = args[3]
    let pngDir: String? = args.count >= 5 ? args[4] : nil
    let pinnedWindow: String? = args.count >= 6 ? args[5] : nil
    if let pngDir { try? FileManager.default.createDirectory(atPath: pngDir,
                                                             withIntermediateDirectories: true) }

    let app = NSApplication.shared
    app.setActivationPolicy(.accessory)

    MainActor.assumeIsolated {
        let registry = FileManager.default.temporaryDirectory
            .appendingPathComponent("openkoalabots-live-tiers-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: registry) }
        let spaceHandle: CuaSpaces.Space
        do {
            let connection = try SpacesConnection.embedded(spacesHome: registry.path)
            spaceHandle = try blocking { try await connection.add(url: host, token: token) }
        } catch {
            print("live-tiers: could not add \(host): \(error)"); exit(1)
        }
        let provider = SpaceStreamProvider(space: spaceHandle)
        let session = LiveStreamSession(provider: provider)
        let bot = Fixtures.bot("cos")

        func sync(_ work: @escaping @MainActor () async -> Void) {
            var done = false
            Task { @MainActor in await work(); done = true }
            while !done { RunLoop.current.run(mode: .default, before: Date().addingTimeInterval(0.02)) }
        }

        sync { await session.refreshWindows() }
        print("windows listed: \(session.windows.count)")
        guard !session.windows.isEmpty else {
            print("no streamable windows, nothing to prove"); exit(1)
        }
        let first = session.windows.first { pinnedWindow == nil || $0.id == pinnedWindow }
            ?? session.windows[0]
        let second = session.windows.first { $0.id != first.id } ?? first
        print("target A: \(first.displayName)  [\(first.id)]")
        print("target B: \(second.displayName)  [\(second.id)]")

        // ---- Tier 2: the pinned preview, non-interactive.
        sync { await session.select(.window(first)) }
        let tier2Window = LiveTierHarness.host(
            AgentScreen(source: .live(session), isInteractive: false,
                        showsControls: false)
                .frame(width: 520, height: 325)
                .streamWindowDropTarget(.live(session))
                .background(DesktopTheme.dark.pane),
            size: CGSize(width: 560, height: 420))
        LiveTierHarness.pump(seconds: 6)
        print(LiveTierHarness.report("tier2-pinned-preview", session: session,
                                     window: tier2Window, pngDirectory: pngDir).line)

        // ---- Window drag-and-drop into the tier-2 pane.
        // Exactly the path the drop handler runs: encode on the drag source,
        // decode on the pane, resolve against the live list, select.
        let framesBefore = session.decodedFrameCount
        let payload = StreamWindowDrag.data(for: second)
        guard let dropped = StreamWindowDrag.window(from: payload) else { exit(1) }
        sync { await session.select(StreamWindowDrag.resolve(dropped, against: session.windows)) }
        LiveTierHarness.pump(seconds: 5)
        print("window-drop into tier-2 pane: source=\(session.source.label) "
              + "framesBefore=\(framesBefore) framesAfter=\(session.decodedFrameCount)")
        tier2Window.close()

        // ---- PiP: pop out, install the drop target, drop a window on it.
        let pip = StreamPiPController()
        pip.popOut(session: session)
        LiveTierHarness.pump(seconds: 1)
        let installed = StreamPiPDropTarget.install(session: session)
        let catcher = StreamPiPDropTarget.installedCatcher()
        let pipFramesBefore = session.decodedFrameCount
        let acceptedByPiP = catcher?.accept(StreamWindowDrag.data(for: first)) ?? false
        LiveTierHarness.pump(seconds: 5)
        print("pip: installed=\(installed) catcher=\(catcher != nil) accepted=\(acceptedByPiP) "
              + "source=\(session.source.label) framesBefore=\(pipFramesBefore) "
              + "framesAfter=\(session.decodedFrameCount)")
        pip.popIn()
        LiveTierHarness.pump(seconds: 1)

        // ---- Tier 3: the takeover, interactive, with input actually sent.
        let tier3Window = LiveTierHarness.host(
            AgentScreen(source: .live(session), isInteractive: true,
                        showsControls: false)
                .frame(width: 512, height: 320),
            size: CGSize(width: 512, height: 320))
        LiveTierHarness.pump(seconds: 5)
        let before = LiveTierHarness.report("tier3-takeover", session: session,
                                            window: tier3Window, pngDirectory: pngDir)
        print(before.line)

        // A pointer move is the least invasive way to show input reaching the
        // Space: it moves nothing and clicks nothing, and the daemon still
        // acknowledges the sequence.
        session.send([.pointer(phase: .move, button: nil, x: 0.5, y: 0.5, modifiers: [])])
        LiveTierHarness.pump(seconds: 3)
        print(LiveTierHarness.report("tier3-takeover-after-input", session: session,
                                     window: tier3Window, pngDirectory: pngDir).line)

        // Hand control back: the same view, now forwarding nothing.
        let handedBack = LiveTierHarness.host(
            AgentScreen(source: .live(session), isInteractive: false,
                        showsControls: false)
                .frame(width: 512, height: 320),
            size: CGSize(width: 512, height: 320))
        LiveTierHarness.pump(seconds: 3)
        let sentBefore = session.inputEventsSent
        if let view = LiveTierHarness.findStreamView(in: handedBack) {
            print("hand-back: interactive=\(view.isInteractive) "
                  + "layerHasPixels=\(view.hasDecodedPixels) inputSentUnchanged="
                  + "\(session.inputEventsSent == sentBefore)")
        }
        tier3Window.close()
        handedBack.close()

        sync { await session.stop() }
        print("session stopped cleanly")
    }
    exit(0)
}

if args.count >= 4, args[1] == "live-pip" {
    // Proof harness for picture-in-picture: the desktop (the shared session)
    // and one Space window (a session of its own) popped out at once through
    // `StreamPiPSet`, with frame counts sampled twice to show both update.
    //
    //   OpenKoalaBotExample live-pip <host:port> <token> [pngDir] [window title substring]
    //
    // The spacesd is added to a temp Spaces registry (never ~/.cua), or to
    // `OPENKOALABOTS_PIP_REGISTRY` when set, which is then kept for the app.
    // Creates nothing in the Space and sends no input.
    let host = args[2], token = args[3]
    let pngDir: String? = args.count >= 5 ? args[4] : nil
    let wanted: String? = args.count >= 6 ? args[5].lowercased() : nil
    if let pngDir { try? FileManager.default.createDirectory(atPath: pngDir,
                                                             withIntermediateDirectories: true) }
    let app = NSApplication.shared
    app.setActivationPolicy(.accessory)

    MainActor.assumeIsolated {
        let kept = ProcessInfo.processInfo.environment["OPENKOALABOTS_PIP_REGISTRY"]
        let registry = kept.map { URL(fileURLWithPath: $0) } ?? FileManager.default.temporaryDirectory
            .appendingPathComponent("openkoalabots-live-pip-\(UUID().uuidString)")
        defer { if kept == nil { try? FileManager.default.removeItem(at: registry) } }
        let spaceHandle: CuaSpaces.Space
        do {
            let connection = try SpacesConnection.embedded(spacesHome: registry.path)
            spaceHandle = try blocking { try await connection.add(url: host, token: token) }
        } catch {
            print("live-pip: could not add \(host): \(error)"); exit(1)
        }
        print("space: \(spaceHandle.id.rawValue)")
        let provider = SpaceStreamProvider(space: spaceHandle)
        let desktop = LiveStreamSession(provider: provider)
        func sync(_ work: @escaping @MainActor () async -> Void) {
            var done = false
            Task { @MainActor in await work(); done = true }
            while !done { RunLoop.current.run(mode: .default, before: Date().addingTimeInterval(0.02)) }
        }
        sync { await desktop.refreshWindows() }
        guard let target = desktop.windows.first(where: {
            wanted == nil || $0.title.lowercased().contains(wanted!) || $0.app.lowercased().contains(wanted!)
        }) else {
            print("live-pip: no matching window among \(desktop.windows.map(\.displayName))"); exit(1)
        }
        print("window: \(target.displayName) [\(target.id)]")
        sync { await desktop.select(.desktop) }

        let pips = StreamPiPSet(provider: provider)
        pips.popOut(.desktop, sharing: desktop)
        pips.popOut(.window(target))
        app.activate(ignoringOtherApps: true)
        // Side by side, so a capture of one never includes the other.
        if let d = pips.controller(for: .desktop)?.window,
           let w = pips.controller(for: .window(target))?.window,
           let screen = NSScreen.main?.visibleFrame {
            d.setFrameTopLeftPoint(NSPoint(x: screen.minX + 40, y: screen.maxY - 40))
            w.setFrameTopLeftPoint(NSPoint(x: d.frame.maxX + 40, y: screen.maxY - 40))
        }
        let windowSession = pips.controller(for: .window(target))?.session
        LiveTierHarness.pump(seconds: 6)
        let d1 = desktop.decodedFrameCount, w1 = windowSession?.decodedFrameCount ?? 0
        LiveTierHarness.pump(seconds: 5)
        let d2 = desktop.decodedFrameCount, w2 = windowSession?.decodedFrameCount ?? 0
        print("pip desktop: status=\(desktop.status) frames \(d1) -> \(d2) size=\(desktop.surfaceSize)")
        print("pip window: status=\(windowSession.map { "\($0.status)" } ?? "none") frames \(w1) -> \(w2) "
              + "size=\(windowSession?.surfaceSize ?? .zero) owned=\(pips.controller(for: .window(target))?.ownsSession ?? false)")
        if let pngDir {
            for (name, source) in [("pip-desktop", StreamSource.desktop), ("pip-window", .window(target))] {
                guard let win = pips.controller(for: source)?.window else { continue }
                let path = "\(pngDir)/\(name).png"
                let proc = Process()
                proc.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
                proc.arguments = ["-o", "-x", "-l", String(win.windowNumber), path]
                try? proc.run()
                proc.waitUntilExit()
                print("captured \(path)")
            }
        }
        let ok = d2 > d1 && w2 > w1
        pips.popInAll()
        LiveTierHarness.pump(seconds: 1)
        print("window session after close: \(windowSession.map { "\($0.status)" } ?? "none")")
        sync { await desktop.stop() }
        print(ok ? "live-pip: PASS" : "live-pip: FAIL")
        exit(ok ? 0 : 1)
    }
}

if args.count >= 3, args[1] == "live-shell" {
    // The product surface, fully wired: the desktop shell with live tier-2
    // pixels, a live window list to drag from, file drop → `upload`, and
    // produced files draggable back out through `download`.
    //
    //   OpenKoalaBotExample live-shell <space> [agentFile …]
    //
    // The backend is the cua SDK (`OPENKOALABOTS_SPACES=daemon|embedded`).
    //
    // Runs until the window is closed. Creates nothing and deletes nothing.
    let space = args[2]
    let agentFiles = Array(args.dropFirst(3))
    let app = NSApplication.shared
    app.setActivationPolicy(.regular)

    MainActor.assumeIsolated {
        do {
            let client = try SDKSpacesClient(backend: SDKSpacesClient.backendFromEnvironment())
            _ = try blocking { try await client.handshake() }

            // This used to be three things: an `endpoint(forceRefresh:)`
            // closure that pulled `local_rcdp` apart by hand, a window-listing
            // closure, and a `SpacesStreamSourceAdapter` to hold them. All of
            // that was routing around the fact that the client could hand over
            // a *view* of the Space but not its *frames* (FRICTION.md §10).
            // The SDK separates those, so a Space is now itself a stream
            // source.
            let handle = try blocking { try await client.sdkSpace(space) }
            let session = LiveStreamSession(space: handle)

            let intake = AttachmentIntake()
            intake.upload = { local, remote in
                try await client.upload(space: space, localPath: local.path, remotePath: remote)
            }
            let artifacts = AgentArtifactExport()
            artifacts.download = { path in
                try await client.download(space: space, remotePath: path, localDirectory: nil)
            }

            // The same one drop zone as the app. This harness has no Bot
            // thread, so files go straight to `intake`.
            let zone = SpaceDropZone(
                spaceID: space,
                onFiles: { intake.accept($0) },
                onSendFile: {
                    let panel = NSOpenPanel()
                    panel.allowsMultipleSelection = true
                    panel.canChooseDirectories = false
                    if panel.runModal() == .OK { intake.accept(panel.urls) }
                })
            let shell = DesktopShell(screen: .live(session), intake: intake, dropZone: zone,
                                     artifacts: artifacts, agentFiles: agentFiles)
            let window = LiveTierHarness.host(shell, size: DS.desktopCanvas)
            window.title = "OpenKoalaBots - \(space)"
            app.activate(ignoringOtherApps: true)

            // A bounded, self-reporting run, so the product surface can be
            // smoke-tested without a human closing a window.
            if let seconds = ProcessInfo.processInfo.environment["OPENKOALABOTS_SHELL_SECONDS"]
                .flatMap(Double.init) {
                LiveTierHarness.pump(seconds: seconds)
                print(LiveTierHarness.report("desktop-tier2-right-panel", session: session,
                                             window: window, pngDirectory: nil).line)
                print("windows offered to drag: \(session.windows.count)")
                MainActor.assumeIsolated { Task { await session.stop() } }
                LiveTierHarness.pump(seconds: 1)
                exit(0)
            }
            app.run()
            _ = window
        } catch {
            print("live-shell failed: \(error)")
            exit(1)
        }
    }
    exit(0)
}

if args.count >= 2, args[1] == "scenario" {
    // The shared headless scenario (samples/openkoalabot-example-scenario). No windows.
    exit(ScenarioRunner.main(Array(args.dropFirst(2))))
}

// Anything else is the app. `help` prints the map and stops; a bare invocation
// (or an unrecognised argument) falls through to the SwiftUI app, which is what
// double-clicking the binary does.
if args.count >= 2, ["help", "--help", "-h"].contains(args[1]) {
    print(usage)
    exit(0)
}
return false
    }

    static let usage = """
OpenKoalaBots: a chat app for agent coworkers, on Cua Spaces.

  OpenKoalaBotExample                                        launch the app
  OpenKoalaBotExample export <dir>                           render every screen to PNG (no windows)
  OpenKoalaBotExample spaces-probe [daemon|embedded]         open the cua SDK's Spaces runtime and report wiring
  OpenKoalaBotExample live-tiers <host:port> <token> [dir]   mount the live Agent Computer tiers and report
  OpenKoalaBotExample live-shell <space>                     the desktop shell, live: stream + drag-and-drop
  OpenKoalaBotExample routine-fire <space> [botID]           fire one routine through the real scheduler
  OpenKoalaBotExample scenario --spec <json> --lane <fixture|docker|cloud> --out <json>
                                                             the shared OpenKoalaBots scenario, headless
"""
}
