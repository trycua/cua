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
        // Open at every launch (see `ignoreSavedWindowState`), except while
        // New UI is on or the first run is due: then the New UI window opens
        // instead (`AppDelegate.start`).
        .defaultLaunchBehavior(Self.opensWebUIAtLaunch(model) ? .suppressed : .presented)
        .commands {
            CommandGroup(replacing: .newItem) {
                Button(model.chrome.newSpaceLabel) { Task { await model.openNewSpace() } }
                    .keyboardShortcut("n")
            }
            // No `.appSettings` group here: the `Settings` scene's item is the
            // app menu's one "Settings…". The New UI window answers its
            // action itself (`WebUIAppWindow`), so ⌘, opens its Settings.
            CommandGroup(before: .sidebar) {
                NavigationCommands(model: model)
                Divider()
            }
            CommandGroup(after: .help) {
                GettingStartedCommand(model: model)
                SystemStatusCommand(model: model)
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

        // Not inserted in an offscreen run (`OffscreenRun`).
        MenuBarExtra(isInserted: .constant(!OffscreenRun.isOn)) {
            MenuBarContent(model: model)
        } label: {
            MenuBarLabel(model: model)
        }
    }

    /// New UI is on, or the first run is due (it is the web UI's onboarding):
    /// a launch opens the New UI window rather than the native main window.
    static func opensWebUIAtLaunch(_ model: AppModel) -> Bool {
        opensWebUIAtLaunch(webUi: model.settings.experiments.webUi, onboarded: model.onboarding.completed)
    }

    static func opensWebUIAtLaunch(webUi: Bool, onboarded: Bool) -> Bool { webUi || !onboarded }
}

/// The menu bar icon. It is there from launch, so it also hands the New UI
/// window the scenes' actions when the main window never opened.
struct MenuBarLabel: View {
    let model: AppModel
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        Group {
            if let icon = MenuBarIcon.image {
                Image(nsImage: icon)
            } else {
                Text("Cua")
            }
        }
        .onAppear {
            if WebUIWindowController.actions == nil {
                WebUIWindowController.actions = .make(openWindow: openWindow)
            }
        }
    }
}

/// The main window. The first run is not drawn here: it is the shared web
/// onboarding, in the New UI window (see `MainWindowSwitch`).
struct RootView: View {
    @Bindable var model: AppModel
    @Environment(\.dismissWindow) private var dismissWindow
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        if !model.startup.isReady {
            // The launch (the keychain, the daemon) until the live services
            // are in; then the first run or the main window.
            StartupView(startup: model.startup)
                .onAppear {
                    if model.onboarding.completed, AppDelegate.takeQuietStart() { dismissWindow(id: "main") }
                }
        } else if model.onboarding.completed {
            MainWindow(model: model)
                // New UI on: whatever opened this window gets New UI in its place.
                .background(WindowReader { MainWindowSwitch.nativeAppeared($0, model: model) })
                .onAppear {
                    // The New UI window's bridge opens these scenes.
                    WebUIWindowController.actions = .make(openWindow: openWindow)
                    // Opened at login: the menu bar item and the notch only,
                    // as after the window is closed (once per launch).
                    if AppDelegate.takeQuietStart() { dismissWindow(id: "main"); return }
                    MainWindowSwitch.nativeAppeared(nil, model: model)
                }
        } else {
            // First run: the web onboarding opens in this window's place.
            Color.clear
                .background(WindowReader { MainWindowSwitch.firstRunAppeared($0, model: model) })
                .onAppear {
                    WebUIWindowController.actions = .make(openWindow: openWindow)
                    MainWindowSwitch.firstRunAppeared(nil, model: model)
                }
        }
    }
}

/// Help, Getting Started: the checklist again, in the main window.
struct GettingStartedCommand: View {
    let model: AppModel
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        Button("Getting Started") {
            guard model.startup.isReady, model.onboarding.completed else { return }
            model.openGettingStarted()
            // New UI shows the checklist on its Spaces page.
            MainUIRouter(model: model).open(.home) {
                NSApp.activate()
                openWindow(id: "main")
            }
        }
    }
}

/// Help, System Status: the public status page, in the browser. The address is
/// the About pane's own link (the core's `appAboutView`), so both open the same page.
struct SystemStatusCommand: View {
    let model: AppModel
    @Environment(\.openURL) private var openURL

    var body: some View {
        if let link = model.updates.view.links.first(where: { $0.id == .status }),
           let url = link.url.flatMap(URL.init(string:)) {
            Button(link.label) { openURL(url) }
        }
    }
}

/// The menu bar item's menu: the core's items in order.
struct MenuBarContent: View {
    let model: AppModel
    var disk: LowDiskMonitor = .shared
    @Environment(\.openWindow) private var openWindow
    @Environment(\.openSettings) private var openSettings

    var body: some View {
        // This Mac is short of space: the warning, with the Cua key, first.
        // Nothing when there's room. It opens the Spaces, biggest first.
        if let text = disk.warning.menuText {
            Button {
                MainUIRouter(model: model).open(.path("/spaces?sort=size")) { activate(); openWindow(id: "main") }
            } label: {
                if let key = MenuBarKey.image {
                    Label { Text(text) } icon: { Image(nsImage: key) }
                } else {
                    Text(text)
                }
            }
            Divider()
        }
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
        let router = MainUIRouter(model: model)
        switch id {
        case .open: router.open(.home) { activate(); openWindow(id: "main") }
        case .newSpace:
            activate()
            // The native sheet needs the main window; New UI opens its own.
            if !model.settings.experiments.webUi { openWindow(id: "main") }
            Task { await model.openNewSpace() }
        case .settings: router.open(.settings) { activate(); openSettings() }
        case .volumeConflicts:
            router.open(.drive) {
                activate(); openWindow(id: "main")
                model.selection = .drive
            }
        case .quit: NSApp.terminate(nil)
        case .status, .separator: break
        }
    }

    private func activate() { NSApp.activate() }
}

/// The Cua key at menu item size, for the low-disk warning.
enum MenuBarKey {
    static let image: NSImage? = {
        guard let key = CuaKey.image?.copy() as? NSImage else { return nil }
        key.size = NSSize(width: 16, height: 16)
        return key
    }()
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

    /// Whether a launch brings the app to the front: a user's launch does,
    /// one at login never does, a fixtures run only with
    /// `CUA_SPACES_ACTIVATE=1`.
    static func activatesAtLaunch(loginLaunch: Bool, fixtures: Bool, asked: Bool) -> Bool {
        if asked { return true }
        return !loginLaunch && !fixtures
    }

    func applicationWillFinishLaunching(_ notification: Notification) {
        Self.launchedAtLogin = LoginLaunch.isLoginLaunch(NSAppleEventManager.shared().currentAppleEvent)
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        start(Self.model)
        // The menu bar's low-disk warning: read now, then every 5 minutes.
        LowDiskMonitor.shared.start()
    }

    /// The Dock icon (or opening the app again) with no window up: the main
    /// UI in use, New UI while it is on. The notch and the menu bar item's
    /// panels don't count as windows.
    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        let model = Self.model
        guard model.startup.isReady, model.onboarding.completed else { return true }
        guard !NSApp.windows.contains(where: { $0.isVisible && $0.canBecomeMain }) else { return true }
        MainUIRouter(model: model).open(.home) { WebUIWindowController.actions?.openMain() }
        return false
    }

    /// `cua://add/<CODE>` links (`cua-local://` in Cua Spaces Local, which
    /// answers to its own scheme only): the page's Machines at the code,
    /// which it confirms before joining. Never joins by itself.
    func application(_ application: NSApplication, open urls: [URL]) {
        guard let route = urls.lazy.compactMap(JoinWithCode.route(for:)).first else { return }
        NSApp.activate()
        WebUIWindowController.show(model: Self.model, at: .path(route))
    }

    func start(_ model: AppModel) {
        guard !started else { return }
        started = true
        // The one-time move of the host service into this app (Cua Spaces
        // Helper): its own window, over whichever UI is up.
        HelperMoveWindow.follow(model.host, loginLaunch: Self.launchedAtLogin)
        // New Space opens New UI's wizard while its experiment is on.
        model.openWebNewSpace = { on in WebUIWindowController.showNewSpace(model: model, on: on) }
        // New UI on and off: one window, its frame kept, never two.
        model.openWebUI = { MainWindowSwitch.switchToWebUI(model: model) }
        model.closeWebUI = { WebUIWindowController.closeShared() }
        // "Show again" for the first run: the web onboarding's window.
        model.openOnboarding = { WebUIWindowController.show(model: model, at: .onboarding) }
        let controller = NotchController(model: model.notch)
        controller.sidebar = model.dropTargets
        controller.pageTiles = model.pageDropTargets
        controller.showMain = { WebUIWindowController.actions?.openMain() }
        // Each of these shows its result in the main window (the New UI
        // window while its experiment is on): open it when it was closed
        // (the notch stays up without it), or a drop would commit to
        // nothing visible.
        let router = MainUIRouter(model: model)
        controller.onOpenSpace = { [weak controller] id in
            model.select(id)
            router.open(.space(id)) {
                NSApp.activate()
                controller?.showMain?()
            }
        }
        controller.onOpenList = { [weak controller] in
            router.open(.home) { controller?.showMain?() }
        }
        controller.onOpenSettings = { [weak controller] in
            router.open(.settings) { controller?.showSettings?() }
        }
        controller.onOpenAccess = { [weak controller] in
            router.open(.access) {
                model.selection = .keyvault(.category(category: .access))
                NSApp.activate()
                controller?.showMain?()
            }
        }
        controller.onDismissAccess = { model.keyvault.dismiss() }
        controller.onTeleport = { [weak controller] spaceId, entry in
            model.select(spaceId)
            // The page opens Teleport at the app as for a drop of it.
            let app = entry?.hostPath.map { URL(fileURLWithPath: $0) }
            router.open(.space(spaceId, dropped: app.map { [$0] } ?? [])) {
                model.pendingTeleport = PendingTeleport(spaceId: spaceId, entry: entry, files: [])
                NSApp.activate()
                controller?.showMain?()
            }
        }
        controller.onDropURLs = { [weak controller] spaceId, urls in
            model.select(spaceId)
            router.open(.space(spaceId, dropped: urls)) {
                Task {
                    guard let context = try? await model.backend.teleportContext(id: spaceId) else { return }
                    let parsed = context.0.parseDrop(items: urls.map(\.path))
                    let entry = parsed.apps.first.flatMap { try? context.0.catalogEntryForPath(path: $0, options: nil) }
                    model.pendingTeleport = PendingTeleport(spaceId: spaceId, entry: entry, files: parsed.files)
                    NSApp.activate()
                    controller?.showMain?()
                }
            }
        }
        // "Spaces tab in the notch": applied now and on every change (an
        // offscreen run never puts the notch up).
        if !OffscreenRun.isOn { Self.followNotchSetting(model, controller) }
        // The tiles read the shared thumbnail store (the SDK's cache);
        // open, every tile asks for one no older than the open interval.
        let thumbnails = model.thumbnails
        let openAge = TimeInterval(SpaceThumbnails.policy.openIntervalMs) / 1000
        controller.thumbnail = { id in await thumbnails.refresh(id, maxAge: openAge) }
        // Closed too: running Spaces' thumbnails stay a minute or two old
        // (paused while the app is hidden or in Low Power Mode).
        thumbnails.keepFresh(running: { model.streamableSpaceIds })
        // Once the launch has the SDK (it comes after the window).
        model.onLive { [weak controller] live in
            let teleport = live.cua.teleport()
            model.notch.capture = { id in
                (try? teleport.captureWindowThumbnail(windowId: id, maxWidth: 320)).flatMap { $0 }.flatMap(NSImage.init(data:))
            }
            controller?.followWindowDrags(teleport: teleport)
            // The picker's app list and icons, cached before it first opens.
            teleport.prefetch()
        }
        notch = controller
        // ⌘⇧Space from any app: the window of the UI in use, in front.
        if !OffscreenRun.isOn { GlobalShortcut.install(model) }
        AppEnvironment.applyStartView(model, notch: controller)
        // New UI on, or the first run due: the New UI window is the one a
        // launch opens (the main window is suppressed), the first run at
        // `/onboarding`; not when opened at login.
        if CuaSpacesMacApp.opensWebUIAtLaunch(model), !Self.launchedAtLogin {
            if let route = MainWindowSwitch.firstRunRoute(onboarded: model.onboarding.completed) {
                WebUIWindowController.show(model: model, at: route)
            } else if DevHooks.value("CUA_SPACES_START_VIEW") == nil {
                WebUIWindowController.show(model: model)
            }
        }
        // The notch shows the Spaces (and takes drops on them) with the main
        // window closed: keep the roster, and so each tile's reachability,
        // fresh from here rather than from the window.
        // Bounded, and it reconnects to the daemon when the list goes stale.
        _ = model.startListPoll()
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
        // A click on one of the app's notifications: the main UI in use
        // (a Space that came up: its page).
        DeviceNotifier.shared.onOpen = { id in
            if let spaceId = SpaceReadyNote.spaceId(fromIdentifier: id) {
                model.select(spaceId)
                router.open(.space(spaceId)) { WebUIWindowController.actions?.openMain() }
                return
            }
            router.open(id.hasPrefix("agent-") ? .agents : .home) { WebUIWindowController.actions?.openMain() }
        }
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
        // Opened by the user: in front, with its window (the launch shows
        // there while it waits). Opened at login: in the background, never
        // taking focus. Fixtures (UI tests, captures) come forward only
        // when asked.
        if !OffscreenRun.isOn,
           Self.activatesAtLaunch(loginLaunch: Self.launchedAtLogin,
                                  fixtures: DevHooks.value("CUA_SPACES_FIXTURES") == "1",
                                  asked: DevHooks.value("CUA_SPACES_ACTIVATE") == "1") {
            NSApp.activate()
            DispatchQueue.main.async {
                NSApp.windows.first { $0.isVisible && $0.canBecomeMain }?.makeKeyAndOrderFront(nil)
            }
        }
    }
}
