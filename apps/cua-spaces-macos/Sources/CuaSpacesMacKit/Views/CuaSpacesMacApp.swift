// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSDK
import CuaSpacesFFI
import Observation
import CuaSpacesStreaming
import SwiftUI

/// Cua Spaces for macOS: the main window, Settings, the menu bar extra and
/// the notch panel, all over one `AppModel`.
public struct CuaSpacesMacApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate
    @State private var model: AppModel

    public init() {
        Self.ignoreSavedWindowState()
        _model = State(initialValue: AppDelegate.model)
    }

    /// A menu-bar app keeps running with its window closed, so quitting
    /// from the menu bar saved a state with no windows; the next launch
    /// restored that empty state and skipped the main window (only
    /// `-ApplePersistenceIgnoreState YES` showed it). Window frames still
    /// persist through their autosave names.
    static func ignoreSavedWindowState(_ defaults: UserDefaults = .standard) {
        defaults.register(defaults: ["ApplePersistenceIgnoreState": true])
    }

    public var body: some Scene {
        Window("Cua Spaces", id: "main") {
            RootView(model: model)
                .environment(\.presenceName, model.presenceName)
                .onAppear { delegate.start(model) }
        }
        .defaultSize(width: 980, height: 660)
        // Open at every launch (see `ignoreSavedWindowState`).
        .defaultLaunchBehavior(.presented)
        .commands {
            CommandGroup(replacing: .newItem) {
                Button(model.chrome.newSpaceLabel) { Task { await model.openNewSpace() } }
                    .keyboardShortcut("n")
            }
        }

        // A Space's desktop in its own window ("Open"); never at launch.
        WindowGroup(id: "space", for: String.self) { $id in
            if let id {
                SpaceWindowView(model: model, spaceId: id)
                    .environment(\.presenceName, model.presenceName)
            }
        }
        .defaultLaunchBehavior(.suppressed)
        .restorationBehavior(.disabled)

        Settings {
            SettingsScene(model: model)
        }

        MenuBarExtra {
            MenuBarContent(model: model)
        } label: {
            if let icon = MenuBarIcon.image {
                Image(nsImage: icon)
            } else {
                Text("Cua")
            }
        }
    }
}

/// First run, else the main window.
struct RootView: View {
    @Bindable var model: AppModel
    @Environment(\.dismissWindow) private var dismissWindow

    var body: some View {
        if model.onboarding.completed {
            MainWindow(model: model)
                .onAppear {
                    // Opened at login: the menu bar item and the notch only,
                    // as after the window is closed (once per launch).
                    if AppDelegate.takeQuietStart() { dismissWindow(id: "main") }
                }
        } else {
            OnboardingView(onboarding: model.onboarding, onSignIn: model.account == nil ? nil : {
                await model.beginSignIn()
                return model.identity
            })
                .toolbar(removing: .title)
                .toolbarBackgroundVisibility(.hidden, for: .windowToolbar)
        }
    }
}

/// The menu bar item's menu: the core's items in order.
struct MenuBarContent: View {
    let model: AppModel
    @Environment(\.openWindow) private var openWindow
    @Environment(\.openSettings) private var openSettings

    var body: some View {
        ForEach(Array(model.menuBar.enumerated()), id: \.offset) { _, item in
            switch item.id {
            case .status: Text(item.label)
            case .separator: Divider()
            default:
                if let key = Self.key(item.shortcut) {
                    Button(item.label) { run(item.id) }.keyboardShortcut(key)
                } else {
                    Button(item.label) { run(item.id) }
                }
            }
        }
    }

    /// "⌘," / "⌘Q" as a key equivalent.
    static func key(_ shortcut: String?) -> KeyEquivalent? {
        guard let last = shortcut?.last, shortcut?.first == "\u{2318}" else { return nil }
        return KeyEquivalent(Character(last.lowercased()))
    }

    private func run(_ id: AppMenuItemId) {
        switch id {
        case .open: activate(); openWindow(id: "main")
        case .newSpace:
            activate(); openWindow(id: "main")
            Task { await model.openNewSpace() }
        case .settings: activate(); openSettings()
        case .volumeConflicts:
            activate(); openWindow(id: "main")
            model.selection = .drive
        case .quit: NSApp.terminate(nil)
        case .status, .separator: break
        }
    }

    private func activate() { NSApp.activate() }
}

/// The Cua mark as a template image (from apps/cua-spaces/src-tauri/icons,
/// copied into the bundle by scripts/build-app.sh).
enum MenuBarIcon {
    static let image: NSImage? = {
        guard let url = Bundle.main.url(forResource: "tray-template", withExtension: "png"),
              let image = NSImage(contentsOf: url) else { return nil }
        image.isTemplate = true
        image.size = NSSize(width: 18, height: 18)
        return image
    }()
}

/// Starts the notch and wires it to the model.
@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    /// Shows or hides the notch with the "Spaces tab in the notch" setting
    /// (`menuBar`): now, and again whenever it changes (Settings, the
    /// onboarding choice). The setting persists with the other settings.
    static func followNotchSetting(_ model: AppModel, _ controller: NotchController) {
        let menuBar = withObservationTracking { model.settings.menuBar } onChange: {
            DispatchQueue.main.async { followNotchSetting(model, controller) }
        }
        controller.setShown(!menuBar)
    }

    /// The one model: the scenes and the notch share it. Made on first use
    /// so the notch starts at launch even when no window opens (a restored
    /// session with the main window closed).
    static let model = AppEnvironment.makeModel()
    private var notch: NotchController?
    private var started = false

    /// The system opened the app at login (read as the launch begins).
    private(set) static var launchedAtLogin = false
    private static var quietStartTaken = false

    /// Once per launch: whether the main window should close because the
    /// app was opened at login.
    static func takeQuietStart() -> Bool {
        guard launchedAtLogin, !quietStartTaken else { return false }
        quietStartTaken = true
        return true
    }

    func applicationWillFinishLaunching(_ notification: Notification) {
        Self.launchedAtLogin = LoginLaunch.isLoginLaunch(NSAppleEventManager.shared().currentAppleEvent)
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        start(Self.model)
    }

    func start(_ model: AppModel) {
        guard !started else { return }
        started = true
        let controller = NotchController(model: model.notch)
        controller.sidebar = model.dropTargets
        // Each of these shows its result in the main window: open it when
        // it was closed (the notch stays up without it), or a drop would
        // commit to nothing visible.
        controller.onOpenSpace = { [weak controller] id in
            model.select(id)
            NSApp.activate()
            controller?.showMain?()
        }
        controller.onOpenAccess = { [weak controller] in
            model.selection = .keyvault(.category(category: .access))
            NSApp.activate()
            controller?.showMain?()
        }
        controller.onDismissAccess = { model.keyvault.dismiss() }
        controller.onTeleport = { [weak controller] spaceId, entry in
            model.select(spaceId)
            model.pendingTeleport = PendingTeleport(spaceId: spaceId, entry: entry, files: [])
            NSApp.activate()
            controller?.showMain?()
        }
        controller.onDropURLs = { [weak controller] spaceId, urls in
            model.select(spaceId)
            Task {
                guard let context = try? await model.backend.teleportContext(id: spaceId) else { return }
                let parsed = context.0.parseDrop(items: urls.map(\.path))
                let entry = parsed.apps.first.flatMap { try? context.0.catalogEntryForPath(path: $0, options: nil) }
                model.pendingTeleport = PendingTeleport(spaceId: spaceId, entry: entry, files: parsed.files)
                NSApp.activate()
                controller?.showMain?()
            }
        }
        // "Spaces tab in the notch": applied now and on every change.
        Self.followNotchSetting(model, controller)
        // The tiles read the shared thumbnail store (the SDK's cache);
        // open, every tile asks for one no older than the open interval.
        let thumbnails = model.thumbnails
        let openAge = TimeInterval(SpaceThumbnails.policy.openIntervalMs) / 1000
        controller.thumbnail = { id in await thumbnails.refresh(id, maxAge: openAge) }
        // Closed too: running Spaces' thumbnails stay a minute or two old
        // (paused while the app is hidden or in Low Power Mode).
        thumbnails.keepFresh(running: { model.streamableSpaceIds })
        if let live = model.backend as? LiveSpacesBackend {
            let teleport = live.cua.teleport()
            model.notch.capture = { id in
                (try? teleport.captureWindowThumbnail(windowId: id, maxWidth: 320)).flatMap { $0 }.flatMap(NSImage.init(data:))
            }
            controller.followWindowDrags(teleport: teleport)
            // The picker's app list and icons, cached before it first opens.
            teleport.prefetch()
        }
        notch = controller
        AppEnvironment.applyStartView(model, notch: controller)
        // The notch shows the Spaces (and takes drops on them) with the main
        // window closed: keep the roster, and so each tile's reachability,
        // fresh from here rather than from the window.
        Task { @MainActor in
            while !Task.isCancelled {
                await model.refresh()
                try? await Task.sleep(for: .seconds(10))
            }
        }
        // Keyvault access is never silent: the notch indicator and the menu
        // bar line follow live deliveries even with the main window closed.
        Task { @MainActor in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(10))
                await model.keyvault.refresh()
            }
        }
        // A device asking to join is announced (a notification and the
        // approval sheet) even with the window closed; this Mac's own
        // enrollment shows as the main window's banner. Nothing else asks.
        model.devices.notify = { DeviceNotifier.shared.post($0) }
        // Persistent agents' answers and requests, from the daemon's feed,
        // while the app runs (the daemon keeps them while it does not).
        model.persistent.post = { AgentNotifier.shared.post($0) }
        _ = model.persistent.startPolling()
        // Launch at login: read it, and turn it on for an install that never
        // chose and serves Spaces or persistent agents (the core's rule).
        Task { @MainActor in await model.applyLaunchAtLogin() }
        Task { @MainActor in
            while !Task.isCancelled {
                await model.devices.refresh()
                try? await Task.sleep(for: .seconds(60))
            }
        }
        // Captures and UI tests bring this app forward; normal launches do not
        // steal focus.
        if DevHooks.value("CUA_SPACES_ACTIVATE") == "1" {
            DispatchQueue.main.asyncAfter(deadline: .now() + 1) {
                NSApp.activate(ignoringOtherApps: true)
            }
        }
    }
}
