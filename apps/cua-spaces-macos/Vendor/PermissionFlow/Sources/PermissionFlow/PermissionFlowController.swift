#if os(macOS)
import AppKit
import Combine
import SystemSettingsKit
import SwiftUI

@available(macOS 13.0, *)
@MainActor
public final class PermissionFlowController: ObservableObject {
    /// The package exposes a single active floating panel at a time so opening
    /// a second permission flow closes the previous panel automatically.
    private static var activeController: PermissionFlowController?
    private let systemSettingsBundleIdentifier = "com.apple.systempreferences"

    /// Apps currently represented in the floating panel.
    @Published public private(set) var droppedApps: [URL]

    /// The permission pane currently being guided.
    @Published public private(set) var currentPane: PermissionFlowPane?

    /// Drives the visibility of the "reopen settings" action.
    @Published var isSettingsFrontmost = false

    /// Drives the header icon animation while the app card is being dragged.
    @Published var isDraggingApp = false

    /// True while System Settings has been asked to open but its window is
    /// not on screen yet. The floating panel shows a loading state instead of
    /// the drag guide.
    @Published var isAwaitingSettingsWindow = false

    /// Drives the locale environment used by the floating SwiftUI panel.
    @Published public private(set) var localeIdentifier: String?

    public var onDrop: ((URL) -> Void)?

    /// Cua Spaces addition: what the panel says and whether it offers the
    /// app to drag, chosen by the host app for what the pane really shows.
    /// Nil keeps PermissionFlow's own "Drag ... to the list above" guide.
    @Published public private(set) var guidance: PermissionFlowGuidance?

    /// Sets `guidance` and refits the panel under System Settings, so the
    /// same panel moves from "drag it in" to "turn on the switch" to "on"
    /// without closing.
    public func setGuidance(_ guidance: PermissionFlowGuidance?) {
        guard self.guidance != guidance else { return }
        self.guidance = guidance
        guard let panel else { return }
        panel.rebuildContent()
        if let frame = tracker.currentFrame {
            panel.snap(to: frame)
        }
    }

    /// Cua Spaces addition (CUA-3551): docks the panel again under System
    /// Settings as it is now, from its live window frame and the screens'
    /// current Dock and menu bar. Cheap when nothing moved; skipped while the
    /// app's tile is being dragged (the panel is see-through then).
    public func refit() {
        guard let panel, isDraggingApp == false, isAwaitingSettingsWindow == false,
              let frame = tracker.currentFrame else { return }
        panel.snap(to: frame)
    }

    private let configuration: PermissionFlowConfiguration
    private let tracker = SettingsWindowTracker()

    private var panel: FloatingDropPanel?
    private var pendingLaunchSourceFrame: CGRect?
    private var previousFrontmostApplicationPID: pid_t?
    private var previousFrontmostApplicationBundleIdentifier: String?
    private var cancellables = Set<AnyCancellable>()

    public init(configuration: PermissionFlowConfiguration = .init()) {
        self.configuration = configuration
        self.droppedApps = configuration.requiredAppURLs.uniqueAppURLs()
        self.localeIdentifier = configuration.localeIdentifier

        updateFrontmostAppState()
        bindTrackerCallbacks()
        observeFrontmostApplication()
        observeSpacesAndScreens()
    }

    /// Opens the requested privacy pane and starts the floating guidance flow.
    ///
    /// - Parameters:
    ///   - pane: The permission pane to open inside System Settings.
    ///   - suggestedAppURLs: Optional `.app` bundle URLs that should appear in
    ///     the floating panel as drag candidates. This parameter defaults to an
    ///     empty array, which means no explicit app list is injected here.
    ///     When this value is empty and no previously registered app is
    ///     available, the floating panel falls back to `Bundle.main.bundleURL`
    ///     if the current host bundle is itself an `.app`.
    ///   - sourceFrameInScreen: Optional source rect in screen coordinates used
    ///     as the launch point for the fly-to-settings animation. If omitted,
    ///     the panel still appears, but it skips the source-origin animation.
    public func authorize(
        pane: PermissionFlowPane,
        suggestedAppURLs: [URL] = [],
        sourceFrameInScreen: CGRect? = nil
    ) {
        closeOtherActivePanelIfNeeded()

        rememberPreviousFrontmostApplication()
        currentPane = pane
        pendingLaunchSourceFrame = sourceFrameInScreen
        mergeDroppedApps(with: suggestedAppURLs)
        SystemSettings.open(url: pane.settingsURL)

        guard pane.supportsFloatingAuthorizationPanel else { return }

        Self.activeController = self
        // Tracking runs first so an already-open Settings window can be placed
        // immediately. A cold launch has no frame yet; the panel still appears,
        // in a loading state, until the window shows up.
        tracker.startTracking(promptIfNeeded: configuration.promptForAccessibilityTrust)
        showPanel()
    }

    /// Shows the floating guide.
    ///
    /// When System Settings is already on screen, the panel flies to that
    /// window. When it is still launching, the panel appears at the click
    /// anchor with a loading state instead of waiting invisibly.
    public func showPanel() {
        if panel == nil {
            panel = FloatingDropPanel(controller: self)
        }

        guard let panel else { return }

        if let settingsFrame = tracker.currentFrame {
            presentPanel(panel, for: settingsFrame)
            return
        }

        isAwaitingSettingsWindow = true
        panel.rebuildContent()
        panel.showWaiting(anchoredAt: pendingLaunchSourceFrame)
    }

    public func closePanel(returnToPreviousApp: Bool = false) {
        tracker.stopTracking()
        isAwaitingSettingsWindow = false
        panel?.close()
        panel = nil
        pendingLaunchSourceFrame = nil

        if Self.activeController === self {
            Self.activeController = nil
        }

        if returnToPreviousApp {
            reactivatePreviousFrontmostApplication()
        }
    }

    public func resetDroppedApps() {
        droppedApps = configuration.requiredAppURLs.uniqueAppURLs()
    }

    /// Updates the locale injected into the floating panel.
    public func setLocaleIdentifier(_ localeIdentifier: String?) {
        guard self.localeIdentifier != localeIdentifier else { return }
        self.localeIdentifier = localeIdentifier
        panel?.updateLocaleIdentifier(localeIdentifier)
    }

    /// Registers a unique `.app` bundle URL and notifies the host if needed.
    public func registerDroppedApp(_ url: URL) {
        guard url.pathExtension.lowercased() == "app" else { return }
        let normalizedURL = url.standardizedFileURL
        guard droppedApps.contains(normalizedURL) == false else { return }
        droppedApps.append(normalizedURL)
        onDrop?(normalizedURL)
    }

    /// The panel always renders a single primary app card. If the host has not
    /// supplied one yet, the host application's bundle becomes the fallback.
    var preferredAppURL: URL? {
        if let first = droppedApps.first {
            return first
        }
        let bundleURL = Bundle.main.bundleURL.standardizedFileURL
        return bundleURL.pathExtension.lowercased() == "app" ? bundleURL : nil
    }

    /// The panel becomes mouse-transparent while dragging so System Settings
    /// underneath can receive the drop.
    func setPanelDragging(_ isDragging: Bool) {
        isDraggingApp = isDragging
        panel?.setDraggingPassthrough(isDragging)
    }

    /// Keeps System Settings visually present whenever the floating panel is
    /// clicked or momentarily considered for focus.
    func keepSettingsVisible() {
        SystemSettings.activate()
        panel?.orderFrontRegardless()
    }

    func reopenCurrentSettingsPane() {
        guard let currentPane else { return }
        SystemSettings.open(url: currentPane.settingsURL)
        panel?.orderFrontRegardless()
    }

    /// Merges unique app bundle URLs into the current panel list.
    func mergeDroppedApps(with urls: [URL]) {
        for url in urls.uniqueAppURLs() {
            registerDroppedApp(url)
        }
    }

    private func bindTrackerCallbacks() {
        tracker.onFrameChange = { [weak self] frame in
            Task { @MainActor [weak self] in
                guard let self else { return }
                self.presentPanel(self.panel, for: frame)
            }
        }
        tracker.onTrackingEnded = { [weak self] in
            Task { @MainActor [weak self] in
                self?.closePanel()
            }
        }
    }

    private func observeFrontmostApplication() {
        NSWorkspace.shared.notificationCenter
            .publisher(for: NSWorkspace.didActivateApplicationNotification)
            .receive(on: RunLoop.main)
            .sink { [weak self] _ in
                self?.updateFrontmostAppState()
            }
            .store(in: &cancellables)
    }

    /// A switch of Space (out of a full-screen app to System Settings'
    /// desktop) or a change of screens, Dock or menu bar moves the area the
    /// panel may use: dock it again then.
    private func observeSpacesAndScreens() {
        NSWorkspace.shared.notificationCenter
            .publisher(for: NSWorkspace.activeSpaceDidChangeNotification)
            .merge(with: NotificationCenter.default.publisher(for: NSApplication.didChangeScreenParametersNotification))
            .receive(on: RunLoop.main)
            .sink { [weak self] _ in
                self?.refit()
            }
            .store(in: &cancellables)
    }

    private func closeOtherActivePanelIfNeeded() {
        if let activeController = Self.activeController, activeController !== self {
            activeController.closePanel()
        }
    }

    private func rememberPreviousFrontmostApplication() {
        let frontmostApplication = NSWorkspace.shared.frontmostApplication
        guard frontmostApplication?.bundleIdentifier != systemSettingsBundleIdentifier else { return }
        previousFrontmostApplicationPID = frontmostApplication?.processIdentifier
        previousFrontmostApplicationBundleIdentifier = frontmostApplication?.bundleIdentifier
    }

    private func reactivatePreviousFrontmostApplication() {
        defer {
            previousFrontmostApplicationPID = nil
            previousFrontmostApplicationBundleIdentifier = nil
        }

        if let previousFrontmostApplicationPID,
           let application = NSRunningApplication(processIdentifier: previousFrontmostApplicationPID) {
            application.activate(options: [.activateIgnoringOtherApps])
            return
        }

        guard let previousFrontmostApplicationBundleIdentifier else { return }
        NSRunningApplication.runningApplications(withBundleIdentifier: previousFrontmostApplicationBundleIdentifier)
            .first?
            .activate(options: [.activateIgnoringOtherApps])
    }

    private func presentPanel(_ panel: FloatingDropPanel?, for settingsFrame: CGRect) {
        guard let panel else { return }

        if isAwaitingSettingsWindow {
            isAwaitingSettingsWindow = false
            panel.rebuildContent()
        }

        if let sourceFrame = pendingLaunchSourceFrame {
            panel.present(from: sourceFrame, to: settingsFrame)
            pendingLaunchSourceFrame = nil
        } else {
            panel.snap(to: settingsFrame)
        }
    }

    private func updateFrontmostAppState() {
        isSettingsFrontmost =
            NSWorkspace.shared.frontmostApplication?.bundleIdentifier == systemSettingsBundleIdentifier
    }
}

@available(macOS 13.0, *)
private extension Array where Element == URL {
    /// Normalizes and de-duplicates `.app` bundle URLs.
    func uniqueAppURLs() -> [URL] {
        var seen = Set<String>()
        return compactMap { url in
            let normalized = url.standardizedFileURL
            guard normalized.pathExtension.lowercased() == "app" else { return nil }
            return seen.insert(normalized.path).inserted ? normalized : nil
        }
    }

    /// Uses normalized file paths for containment because equivalent file URLs
    /// can differ in their string representation.
    func contains(_ url: URL) -> Bool {
        contains(where: { $0.standardizedFileURL.path == url.standardizedFileURL.path })
    }
}
#endif
