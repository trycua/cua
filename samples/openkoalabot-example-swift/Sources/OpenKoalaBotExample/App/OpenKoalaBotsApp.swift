// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

#if canImport(AppKit)
import AppKit
import SwiftUI

/// The binary's single entry point.
///
/// **Why this is an `enum` and not `@main struct OpenKoalaBotsApp: App`.**
/// SwiftPM gives this sample one executable target, and that target has to be
/// two things: an app you launch, and the `export` / `spaces-probe` /
/// `live-tiers` / `live-shell` command line that the rubric and the streaming
/// evidence harnesses are driven through. Those two do not compose by default —
/// a target whose entry point is top-level code in `main.swift` rejects `@main`
/// outright, and `App`'s synthesised `main()` runs `NSApplication` before any
/// argument could be looked at.
///
/// So the entry point is argument dispatch, exactly once, at startup:
/// `CLI.run` gets the arguments first and exits if it recognised one, and only
/// a bare (or unrecognised) invocation reaches `OpenKoalaBotsApp.main()` — which
/// is `App`'s own default implementation, so the app gets the ordinary SwiftUI
/// lifecycle with nothing simulated. `OpenKoalaBotsApp` carries no `@main` of its
/// own for that reason, and for that reason only.
///
/// Recorded as `FRICTION.md` §26.
@main
enum OpenKoalaBotsEntryPoint {
    static func main() {
        if CLI.run(CommandLine.arguments) { return }
        // A SwiftPM executable is a bare Mach-O with no `.app` bundle, and
        // AppKit gives an unbundled process an activation policy that keeps it
        // out of the Dock and off the window server — the window is created and
        // never composited. `live-shell` already had to do this by hand; the
        // app does too, and it is the difference between "builds" and "runs".
        NSApplication.shared.setActivationPolicy(.regular)
        MainActor.assumeIsolated { KoalaAppIcon.install() }
        OpenKoalaBotsApp.main()
    }
}

/// The app proper: one window over one `AppModel`.
struct OpenKoalaBotsApp: App {
    @StateObject private var model = AppModel()

    var body: some Scene {
        WindowGroup(AppIdentity.name) {
            RootView(model: model)
                .frame(minWidth: 820, minHeight: 560)
                .task { await model.start() }
                // Escape backs out a level.
                .onExitCommand { model.back() }
                .background(WindowConfigurator(model: model))
        }
        .windowStyle(.hiddenTitleBar)
        .defaultSize(width: 1200, height: 800)
        .commands {
            CommandGroup(replacing: .newItem) {
                Button("New Bot") { model.open(.roster) }
                    .keyboardShortcut("n", modifiers: [.command])
                Button("New Bot with a Task\u{2026}") { model.hiring = true }
                    .keyboardShortcut("n", modifiers: [.command, .option])
                Button("New Space\u{2026}") { model.creatingSpace = true }
                    .keyboardShortcut("n", modifiers: [.command, .shift])
                Divider()
                Button("Back") { model.back() }
                    .keyboardShortcut("[", modifiers: [.command])
            }
            CommandGroup(after: .sidebar) {
                Button(model.sidebarVisible ? "Hide Sidebar" : "Show Sidebar") {
                    model.sidebarVisible.toggle()
                }
                .keyboardShortcut("s", modifiers: [.command, .control])
                Button("Agent Computer") {
                    if let id = model.route.botID ?? model.store.bots.first?.id {
                        model.escalate(from: id)
                    }
                }
                .keyboardShortcut("d", modifiers: [.command])
                Button("Pop out the Agent Computer") { model.togglePiP() }
                    .keyboardShortcut("p", modifiers: [.command, .shift])
            }
        }
    }
}

/// Reaches the `NSWindow` behind the `WindowGroup`.
///
/// Two things need it and neither is expressible in pure SwiftUI: shutting the
/// session and the poll loop down cleanly when the window closes (so a run this
/// app started never outlives it), and the capture run, which renders the live
/// window's own layer tree.
private struct WindowConfigurator: NSViewRepresentable {
    var model: AppModel

    func makeNSView(context: Context) -> NSView {
        let view = NSView()
        DispatchQueue.main.async {
            guard let window = view.window else { return }
            window.title = AppIdentity.name
            // Opaque: the window never shows a material or the desktop
            // through it.
            window.isOpaque = true
            window.backgroundColor = NSColor(name: nil) { appearance in
                appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
                    ? NSColor(srgbRed: 0x12 / 255.0, green: 0x12 / 255.0, blue: 0x12 / 255.0, alpha: 1)
                    : .white
            }
            AppWindowRegistry.shared.register(window, model: model)
        }
        return view
    }

    func updateNSView(_ nsView: NSView, context: Context) {}
}

/// Where the app's one window is found from outside the view tree.
@MainActor
final class AppWindowRegistry: NSObject, NSWindowDelegate {
    static let shared = AppWindowRegistry()
    private(set) weak var window: NSWindow?
    private weak var model: AppModel?

    func register(window: NSWindow) { self.window = window }

    /// Live only when `OPENKOALABOTS_MAIN_THREAD_PROBE` is set.
    private(set) var probe: MainThreadProbe?

    func register(_ window: NSWindow, model: AppModel) {
        self.window = window
        self.model = model
        probe = MainThreadProbe.startIfRequested()
        // Closing the window ends the app's work: the poll loop stops, the PiP
        // comes back in, and the rcdp session is torn down. The Space itself is
        // never touched — the app attached to a Space it does not own.
        NotificationCenter.default.addObserver(
            forName: NSWindow.willCloseNotification, object: window, queue: .main) { _ in
                MainActor.assumeIsolated {
                    self.probe?.stop()
                    guard let model = self.model else { return }
                    Task { await model.shutDown() }
                }
            }
        UICapture.startIfRequested(model: model, window: window)
    }
}
#endif
